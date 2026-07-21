//! Dependency edges + the in-memory gating graph (crate plan §3.3, spine §3.2.1).
//!
//! Cycle gating uses the **full 4-type** `affects_ready_work` set (`blocks` / `parent-child` /
//! `conditional-blocks` / `waits-for`); a non-gating edge (e.g. `related`) can never create a
//! ready-gating cycle. The graph is built from the `dependencies` rows and reasoned over with
//! `petgraph` (a **private** dependency — no petgraph type appears in any public signature).
//!
//! ## Gating-graph edge orientation (D4, NORMATIVE — spine §3.2.1)
//!
//! [`build_gating_graph`] is the **single** orientation home shared by [`would_cycle_in_tx`] and
//! [`detect_cycles`] (so the D4 orientation lands exactly once). When building the cycle graph:
//! `blocks` / `conditional-blocks` / `waits-for` are inserted **FORWARD** (`issue_id -> depends_on_id`),
//! but `parent-child` is inserted **REVERSED** (`parent depends_on_id -> child issue_id`) — matching
//! unblock's own parent→child blocked propagation (`query.rs` pass 3) and the original
//! `check_cycle` (sqlite.rs:2440) / `load_dependency_cycle_graph` (sqlite.rs:11379). A uniform-forward
//! graph would mis-detect mixed `parent-child` + `blocks`/`waits-for`/`conditional-blocks` cycles.
//!
//! `blocking_only=true` restricts the graph to the 4 gating types (= `affects_ready_work`; the
//! original `detect_blocking_cycles`); `blocking_only=false` includes **all** dependency types (the
//! original `detect_all_cycles`, the integrity/lint view) — but `parent-child` is **still reversed**
//! in either branch (D19; "all types" = parent-child included-and-reversed + every other type
//! forward, NOT "all forward").

use std::collections::{BTreeMap, HashMap, HashSet};

use libsql::{Connection, Value};

use unblock_model::{DepTree, Dependency, DependencyType, GraphEdge};

use crate::error::{StorageError, map_libsql_err};

use super::events::append_event_in_tx;
use super::mappers::dependency_from_row;
use super::{WriteHook, with_immediate_tx};

/// The gating dependency types (`affects_ready_work`), as their wire strings, for SQL `IN (…)`.
const GATING_TYPES: [&str; 4] = ["blocks", "parent-child", "conditional-blocks", "waits-for"];

/// The `parent-child` wire string (the one type the gating graph inserts REVERSED, D4).
const PARENT_CHILD: &str = "parent-child";

/// Add a dependency edge + `Event(DependencyAdded)` (spine §3.2.1).
///
/// Rejects [`StorageError::SelfDependency`] and [`StorageError::DuplicateDependency`]. If the edge is
/// gating (`affects_ready_work`) and would close a cycle over the gating set, rejects
/// [`StorageError::CycleDetected`] with the concrete path.
pub(super) async fn add_dependency(
    conn: &Connection,
    hook: WriteHook<'_>,
    dep: &Dependency,
    actor: &str,
) -> Result<(), StorageError> {
    if dep.issue_id == dep.depends_on_id {
        return Err(StorageError::SelfDependency);
    }
    let dep = dep.clone();
    let actor = actor.to_string();

    with_immediate_tx(conn, hook, |tx| async move {
        // Duplicate edge?
        let mut rows = tx
            .query(
                "SELECT 1 FROM dependencies WHERE issue_id = ?1 AND depends_on_id = ?2 LIMIT 1",
                libsql::params![dep.issue_id.as_str(), dep.depends_on_id.as_str()],
            )
            .await
            .map_err(map_libsql_err)?;
        if rows.next().await.map_err(map_libsql_err)?.is_some() {
            return Err(StorageError::DuplicateDependency);
        }
        drop(rows);

        // Cycle check for a gating edge. `would_cycle_in_tx` returns the REAL ordered cycle path
        // (every node named, e.g. `a -> b -> c -> a`) reconstructed over the just-built graph — the
        // same graph that detected the cycle, never a re-query or a synthetic `a -> … -> a`
        // placeholder (D2/GATE-MUST-3, FR-5 AC).
        if dep.dep_type.affects_ready_work()
            && let Some(cycle) =
                would_cycle_in_tx(&tx, &dep.issue_id, &dep.depends_on_id, &dep.dep_type).await?
        {
            return Err(StorageError::CycleDetected {
                path: render_cycle_path(&cycle),
            });
        }

        // D42: bind ALL SEVEN columns. The pre-D42 statement bound 5 while the read side
        // (`mappers::dependency_from_row`) projected 7 — an asymmetry that made `metadata` and
        // `thread_id` accepted, typed, schema-published and then DISCARDED. It survived to GA
        // because it is DOUBLY masked: `DEFAULT '{}'` writes `'{}'` for an unbound value and the
        // read filter coerces `'{}'` back to `None`, so "never bound" is indistinguishable from
        // "explicitly absent" even by direct SQL inspection.
        //
        // Bind `None` as SQL NULL, NOT `'{}'`/`''`: `non_empty_text` maps NULL -> None, so
        // `None -> NULL -> None` round-trips exactly, while `Some("{}") -> '{}' -> None` preserves
        // the deliberate legacy coercion in `mappers.rs`. Do NOT "fix" that filter.
        //
        // Both columns are BASELINE-v1 (present in the original `SCHEMA_SQL`), so this needs no
        // forward migration and no `schema_version` bump.
        tx.execute(
            "INSERT INTO dependencies (issue_id, depends_on_id, type, created_at, created_by, \
             metadata, thread_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            libsql::params![
                dep.issue_id.as_str(),
                dep.depends_on_id.as_str(),
                dep.dep_type.as_str(),
                dep.created_at.to_rfc3339(),
                dep.created_by.as_deref().unwrap_or(actor.as_str()),
                dep.metadata.as_deref(),
                dep.thread_id.as_deref(),
            ],
        )
        .await
        .map_err(map_libsql_err)?;

        append_event_in_tx(
            &tx,
            &dep.issue_id,
            &unblock_model::EventType::DependencyAdded,
            &actor,
            None,
            Some(&dep.depends_on_id),
            None,
        )
        .await?;

        Ok(((), tx))
    })
    .await
}

/// Remove a dependency edge + `Event(DependencyRemoved)`.
pub(super) async fn remove_dependency(
    conn: &Connection,
    hook: WriteHook<'_>,
    issue_id: &str,
    depends_on_id: &str,
    dep_type: &DependencyType,
    actor: &str,
) -> Result<(), StorageError> {
    let issue_id = issue_id.to_string();
    let depends_on_id = depends_on_id.to_string();
    let dep_type = dep_type.clone();
    let actor = actor.to_string();

    with_immediate_tx(conn, hook, |tx| async move {
        let removed = tx
            .execute(
                "DELETE FROM dependencies WHERE issue_id = ?1 AND depends_on_id = ?2 AND type = ?3",
                libsql::params![issue_id.as_str(), depends_on_id.as_str(), dep_type.as_str(),],
            )
            .await
            .map_err(map_libsql_err)?;
        if removed == 0 {
            return Err(StorageError::DependencyNotFound);
        }
        append_event_in_tx(
            &tx,
            &issue_id,
            &unblock_model::EventType::DependencyRemoved,
            &actor,
            Some(&depends_on_id),
            None,
            None,
        )
        .await?;
        Ok(((), tx))
    })
    .await
}

/// List the dependencies declared **by** `id`.
pub(super) async fn list_dependencies(
    conn: &Connection,
    id: &str,
) -> Result<Vec<Dependency>, StorageError> {
    let mut rows = conn
        .query(
            "SELECT issue_id, depends_on_id, type, created_at, created_by, metadata, thread_id \
             FROM dependencies WHERE issue_id = ?1 ORDER BY depends_on_id ASC, type ASC",
            libsql::params![id],
        )
        .await
        .map_err(map_libsql_err)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(map_libsql_err)? {
        out.push(dependency_from_row(&row)?);
    }
    Ok(out)
}

/// Return the dependency subtree rooted at `id` as a [`DepTree`] (reachable forward edges).
pub(super) async fn dependency_tree(conn: &Connection, id: &str) -> Result<DepTree, StorageError> {
    let all = load_all_edges(conn).await?;
    let adjacency = adjacency(&all);

    let mut edges = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut stack = vec![id.to_string()];
    while let Some(node) = stack.pop() {
        if !seen.insert(node.clone()) {
            continue;
        }
        if let Some(neighbours) = adjacency.get(&node) {
            for (to, dep_type) in neighbours {
                edges.push(GraphEdge {
                    from: node.clone(),
                    to: to.clone(),
                    dep_type: dep_type.clone(),
                });
                stack.push(to.clone());
            }
        }
    }
    // Deterministic edge order for stable snapshots.
    edges.sort_by(|a, b| {
        (a.from.as_str(), a.to.as_str(), a.dep_type.as_str()).cmp(&(
            b.from.as_str(),
            b.to.as_str(),
            b.dep_type.as_str(),
        ))
    });
    Ok(DepTree {
        root: id.to_string(),
        edges,
    })
}

/// Return the dependency graph for a root set as a [`DepTree`]. An empty `roots` slice = the whole
/// graph; a non-empty `roots` returns the union of the subgraphs reachable from those roots.
pub(super) async fn dependency_graph(
    conn: &Connection,
    roots: &[String],
) -> Result<DepTree, StorageError> {
    let all = load_all_edges(conn).await?;

    if roots.is_empty() {
        let mut edges: Vec<GraphEdge> = all
            .iter()
            .map(|(from, to, dep_type)| GraphEdge {
                from: from.clone(),
                to: to.clone(),
                dep_type: dep_type.clone(),
            })
            .collect();
        edges.sort_by(|a, b| {
            (a.from.as_str(), a.to.as_str(), a.dep_type.as_str()).cmp(&(
                b.from.as_str(),
                b.to.as_str(),
                b.dep_type.as_str(),
            ))
        });
        return Ok(DepTree {
            root: String::new(),
            edges,
        });
    }

    // Union of reachable subgraphs from each root.
    let adjacency = adjacency(&all);
    let mut edges = Vec::new();
    let mut seen_edge = std::collections::HashSet::new();
    let mut seen_node = std::collections::HashSet::new();
    let mut stack: Vec<String> = roots.to_vec();
    while let Some(node) = stack.pop() {
        if !seen_node.insert(node.clone()) {
            continue;
        }
        if let Some(neighbours) = adjacency.get(&node) {
            for (to, dep_type) in neighbours {
                let key = (node.clone(), to.clone(), dep_type.as_str().to_string());
                if seen_edge.insert(key) {
                    edges.push(GraphEdge {
                        from: node.clone(),
                        to: to.clone(),
                        dep_type: dep_type.clone(),
                    });
                }
                stack.push(to.clone());
            }
        }
    }
    edges.sort_by(|a, b| {
        (a.from.as_str(), a.to.as_str(), a.dep_type.as_str()).cmp(&(
            b.from.as_str(),
            b.to.as_str(),
            b.dep_type.as_str(),
        ))
    });
    Ok(DepTree {
        root: roots.first().cloned().unwrap_or_default(),
        edges,
    })
}

/// Detect every dependency cycle as an **ordered traversal witness** (D3/D19, spine §3.2.1).
///
/// Builds the gating graph with the shared [`build_gating_graph`] orientation (`parent-child`
/// reversed, others forward), finds the strongly-connected components via `petgraph`'s Tarjan SCC,
/// then emits **one ordered witness per cyclic component**: a multi-node cycle is `[start, …, start]`
/// (the start repeated at the end), a self-loop is `[node, node]`; an acyclic graph returns `[]`.
/// The outer `Vec` is sorted deterministically (NFR-14 snapshot stability, mirroring the original
/// `cycle_witnesses_with_components_from_graph` sort, sqlite.rs:11440).
///
/// `blocking_only=true` restricts the graph to the 4 gating types (= `affects_ready_work`);
/// `=false` includes all dependency types (the integrity/lint view) — `parent-child` is reversed
/// regardless.
pub(super) async fn detect_cycles(
    conn: &Connection,
    blocking_only: bool,
) -> Result<Vec<Vec<String>>, StorageError> {
    let edges = load_all_edges(conn).await?;
    let graph = build_gating_graph(&edges, blocking_only);
    Ok(cycle_witnesses(&graph))
}

/// Whether adding the gating edge `(issue_id, depends_on_id)` of type `dep_type` would close a cycle.
/// Returns the **ordered cycle path** (`Some([issue_id, …, issue_id])`, naming every node on the
/// cycle) when it would, or `None` otherwise. Run within the caller's transaction.
///
/// Orientation (D4/GATE-MUST-1/GATE-MUST-4, source-cited vs `check_cycle` sqlite.rs:2440-2488,
/// `load_dependency_cycle_graph` sqlite.rs:11379): the existing rows are oriented by
/// [`build_gating_graph_from_typed`] — `parent-child` reversed (`parent depends_on_id -> child
/// issue_id`), the other gating types forward (`issue_id -> depends_on_id`). The **prospective edge
/// is oriented the SAME way as an existing row of its own type** — a `parent-child` prospective edge
/// is inserted REVERSED (`depends_on_id -> issue_id`), every other type FORWARD
/// (`issue_id -> depends_on_id`). This is the orientation-consistent reading of the original (whose
/// `check_cycle` treated the prospective edge as standard-forward, a latent bug that missed pure
/// `parent-child` cycles, e.g. the original's own `test_get_ready_issues_recursive_parent_cycle`
/// adds three `parent-child` edges that close a cycle yet all succeed); unblock's own parent→child
/// blocked propagation (`query.rs` pass 3) requires the consistent reversal so a mixed or
/// `parent-child`-only cycle is caught at add-time, matching the `reparent_*_cycle_is_rejected`
/// regression guards. Both `add_dependency` and `apply_reparent` route through here, so the
/// orientation + the real-path reconstruction land once.
pub(super) async fn would_cycle_in_tx(
    tx: &libsql::Transaction,
    issue_id: &str,
    depends_on_id: &str,
    dep_type: &DependencyType,
) -> Result<Option<Vec<String>>, StorageError> {
    // Load the existing gating edges WITH their type (the type is required so `parent-child` can be
    // inserted reversed — D4/GATE-MUST-1; a type-erased load could not orient the graph).
    let placeholders: Vec<String> = (1..=GATING_TYPES.len()).map(|i| format!("?{i}")).collect();
    let sql = format!(
        "SELECT issue_id, depends_on_id, type FROM dependencies WHERE type IN ({})",
        placeholders.join(", ")
    );
    let params: Vec<Value> = GATING_TYPES
        .iter()
        .map(|t| Value::Text((*t).to_string()))
        .collect();
    let mut rows = tx
        .query(&sql, libsql::params_from_iter(params))
        .await
        .map_err(map_libsql_err)?;

    let mut edges: Vec<(String, String, String)> = Vec::new();
    while let Some(row) = rows.next().await.map_err(map_libsql_err)? {
        let Value::Text(row_issue) = row.get_value(0).map_err(map_libsql_err)? else {
            continue;
        };
        let Value::Text(row_depends_on) = row.get_value(1).map_err(map_libsql_err)? else {
            continue;
        };
        let Value::Text(type_str) = row.get_value(2).map_err(map_libsql_err)? else {
            continue;
        };
        edges.push((row_issue, row_depends_on, type_str));
    }

    // Build the gating graph from the EXISTING rows (the add-time guard is always gating, matching
    // the original hardwired `blocking_only=true`), with `parent-child` reversed.
    let mut graph = build_gating_graph_from_typed(&edges, true);

    // Orient the prospective edge the same way an existing row of its own type would be oriented.
    let prospective_is_parent_child = dep_type.as_str() == PARENT_CHILD;
    let (graph_from, graph_to) = if prospective_is_parent_child {
        (depends_on_id, issue_id) // reversed: parent depends_on_id -> child issue_id
    } else {
        (issue_id, depends_on_id) // forward: issue_id -> depends_on_id
    };
    graph.entry(graph_to.to_string()).or_default();
    push_unique(graph.entry(graph_from.to_string()).or_default(), graph_to);

    // The prospective edge `graph_from -> graph_to` closes a cycle iff `graph_from` is reachable from
    // `graph_to` over the rest of the graph (there is a path `graph_to -> … -> graph_from`).
    // Reconstruct that real ordered path and prepend `graph_from` so the witness is the full ordered
    // cycle `[graph_from, graph_to, …, graph_from]`.
    Ok(find_cycle_path(&graph, graph_from, graph_to).map(|tail| {
        let mut cycle = vec![graph_from.to_string()];
        cycle.extend(tail);
        cycle
    }))
}

/// Render an ordered cycle witness as the `a -> b -> c -> a` path string carried by
/// [`StorageError::CycleDetected`]. Shared by `add_dependency` and `apply_reparent` (crud.rs).
pub(super) fn render_cycle_path(cycle: &[String]) -> String {
    cycle.join(" -> ")
}

/// Append `id` to `neighbours` only if it is not already present (the cycle graph stores each
/// directed edge once, mirroring the original `dedup`).
fn push_unique(neighbours: &mut Vec<String>, id: &str) {
    if !neighbours.iter().any(|n| n == id) {
        neighbours.push(id.to_string());
    }
}

/// Load every dependency edge (`issue_id, depends_on_id, dep_type`) from the table.
async fn load_all_edges(
    conn: &Connection,
) -> Result<Vec<(String, String, DependencyType)>, StorageError> {
    let mut rows = conn
        .query("SELECT issue_id, depends_on_id, type FROM dependencies", ())
        .await
        .map_err(map_libsql_err)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(map_libsql_err)? {
        let Value::Text(issue_id) = row.get_value(0).map_err(map_libsql_err)? else {
            continue;
        };
        let Value::Text(depends_on) = row.get_value(1).map_err(map_libsql_err)? else {
            continue;
        };
        let Value::Text(type_str) = row.get_value(2).map_err(map_libsql_err)? else {
            continue;
        };
        // `DependencyType::from_str` is infallible (an unknown type parses to `Custom`), so the Err
        // arm is unreachable — but to keep this panic-free in a lib path we map the impossible Err to
        // a `Custom` sink rather than the former `unwrap_or(Blocks)`, which was dead code that could
        // have fabricated a phantom gating `Blocks` edge from a malformed stored type
        // (D5/GATE-NIT-4).
        let dep_type = type_str
            .parse::<DependencyType>()
            .unwrap_or_else(|_| DependencyType::Custom(type_str.clone()));
        out.push((issue_id, depends_on, dep_type));
    }
    Ok(out)
}

/// Adjacency map `from -> [(to, dep_type)]` for tree/graph traversal.
fn adjacency(
    edges: &[(String, String, DependencyType)],
) -> HashMap<String, Vec<(String, DependencyType)>> {
    let mut map: HashMap<String, Vec<(String, DependencyType)>> = HashMap::new();
    for (from, to, dep_type) in edges {
        map.entry(from.clone())
            .or_default()
            .push((to.clone(), dep_type.clone()));
    }
    map
}

/// Build the cycle graph (id -> sorted, deduped neighbour ids) from typed edges, applying the D4
/// orientation. This is the **single** orientation home (GATE-SHOULD-1); [`build_gating_graph`] is a
/// thin wrapper over it that maps the strongly-typed [`DependencyType`] to its wire string.
///
/// - `parent-child` is inserted **REVERSED** (`depends_on_id -> issue_id`) regardless of
///   `blocking_only`.
/// - the other gating types (`blocks`/`conditional-blocks`/`waits-for`) are inserted **FORWARD**.
/// - `blocking_only=true` admits only the 4 gating types; `=false` admits **all** types forward
///   (with `parent-child` still reversed) — the integrity/lint view (D19).
///
/// Mirrors the original `load_dependency_cycle_graph` (sqlite.rs:11360-11395): a `BTreeMap` for a
/// deterministic node order, each neighbour list sorted+deduped.
fn build_gating_graph_from_typed(
    edges: &[(String, String, String)],
    blocking_only: bool,
) -> BTreeMap<String, Vec<String>> {
    let mut graph: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (issue_id, depends_on, type_str) in edges {
        let is_parent_child = type_str == PARENT_CHILD;
        let admit = if blocking_only {
            GATING_TYPES.contains(&type_str.as_str())
        } else {
            true
        };
        if !admit {
            continue;
        }
        // Determine orientation: parent-child reversed (parent depends_on -> child issue_id);
        // everything else forward (issue_id -> depends_on).
        let (from, to) = if is_parent_child {
            (depends_on, issue_id)
        } else {
            (issue_id, depends_on)
        };
        graph.entry(to.clone()).or_default();
        push_unique(graph.entry(from.clone()).or_default(), to);
    }
    for neighbours in graph.values_mut() {
        neighbours.sort();
        neighbours.dedup();
    }
    graph
}

/// Build the cycle graph from strongly-typed edges (the [`detect_cycles`] entry point), delegating to
/// [`build_gating_graph_from_typed`] — the single D4-orientation home.
fn build_gating_graph(
    edges: &[(String, String, DependencyType)],
    blocking_only: bool,
) -> BTreeMap<String, Vec<String>> {
    let typed: Vec<(String, String, String)> = edges
        .iter()
        .map(|(from, to, dep_type)| (from.clone(), to.clone(), dep_type.as_str().to_string()))
        .collect();
    build_gating_graph_from_typed(&typed, blocking_only)
}

/// Reconstruct the ordered path `start -> … -> target` within the cycle graph (an iterative DFS),
/// or `None` if `target` is unreachable from `start`. The returned path **begins at `start` and ends
/// at `target`** (both endpoints named). Mirrors the original `find_cycle_graph_path`
/// (sqlite.rs:10664-10692).
fn find_cycle_path(
    graph: &BTreeMap<String, Vec<String>>,
    target: &str,
    start: &str,
) -> Option<Vec<String>> {
    let mut visited: HashSet<String> = HashSet::new();
    let mut stack: Vec<(String, Vec<String>)> = vec![(start.to_string(), vec![start.to_string()])];

    while let Some((node, path)) = stack.pop() {
        if node == target {
            return Some(path);
        }
        if !visited.insert(node.clone()) {
            continue;
        }
        if let Some(neighbours) = graph.get(&node) {
            // Reverse so the smallest neighbour is popped first (deterministic, mirrors the original).
            for neighbour in neighbours.iter().rev() {
                if !visited.contains(neighbour) {
                    let mut next = path.clone();
                    next.push(neighbour.clone());
                    stack.push((neighbour.clone(), next));
                }
            }
        }
    }
    None
}

/// Emit one ordered witness per cyclic strongly-connected component of the cycle graph, sorted
/// deterministically. A size-1 component is a cycle only if it has a self-loop (→ `[node, node]`);
/// a larger cyclic component is reconstructed as `[start, …, start]`. Mirrors the original
/// `cycle_witnesses_with_components_from_graph` (sqlite.rs:11417-11442).
fn cycle_witnesses(graph: &BTreeMap<String, Vec<String>>) -> Vec<Vec<String>> {
    let (petgraph, id_of) = build_petgraph(graph);
    let sccs = petgraph::algo::tarjan_scc(&petgraph);

    let mut cycles: Vec<Vec<String>> = Vec::new();
    for scc in sccs {
        if scc.len() == 1 {
            let node = &id_of[&scc[0]];
            // A single node is a cycle only if it carries a self-edge.
            if graph
                .get(node)
                .is_some_and(|neighbours| neighbours.iter().any(|n| n == node))
            {
                cycles.push(vec![node.clone(), node.clone()]);
            }
            continue;
        }
        let component: HashSet<&str> = scc.iter().map(|n| id_of[n].as_str()).collect();
        if let Some(cycle) = witness_for_component(graph, &component) {
            cycles.push(cycle);
        }
    }

    // Deterministic outer order (NFR-14): sort by the witness itself.
    cycles.sort();
    cycles
}

/// Reconstruct an ordered `[start, …, start]` witness for a multi-node cyclic component. `start` is
/// the component's lexicographically-smallest node (deterministic); the path runs from a same-component
/// neighbour of `start` back to `start`. Mirrors `cycle_witness_for_component` (sqlite.rs:11514).
fn witness_for_component(
    graph: &BTreeMap<String, Vec<String>>,
    component: &HashSet<&str>,
) -> Option<Vec<String>> {
    let start = component.iter().copied().min()?;
    let neighbours = graph.get(start)?;
    for neighbour in neighbours {
        if neighbour.as_str() == start || !component.contains(neighbour.as_str()) {
            continue;
        }
        if let Some(tail) = find_cycle_path_in_component(graph, start, neighbour, component) {
            let mut cycle = vec![start.to_string()];
            cycle.extend(tail);
            return Some(cycle);
        }
    }
    None
}

/// DFS from `start_neighbour` back to `target` confined to `component`, returning the node path
/// (including `start_neighbour`, ending at `target`). Mirrors the component-confined
/// `find_cycle_graph_path` (sqlite.rs:10664).
fn find_cycle_path_in_component(
    graph: &BTreeMap<String, Vec<String>>,
    target: &str,
    start_neighbour: &str,
    component: &HashSet<&str>,
) -> Option<Vec<String>> {
    let mut visited: HashSet<String> = HashSet::new();
    let mut stack: Vec<(String, Vec<String>)> = vec![(
        start_neighbour.to_string(),
        vec![start_neighbour.to_string()],
    )];

    while let Some((node, path)) = stack.pop() {
        if node == target {
            return Some(path);
        }
        if !visited.insert(node.clone()) {
            continue;
        }
        if let Some(neighbours) = graph.get(&node) {
            for neighbour in neighbours.iter().rev() {
                if component.contains(neighbour.as_str()) && !visited.contains(neighbour) {
                    let mut next = path.clone();
                    next.push(neighbour.clone());
                    stack.push((neighbour.clone(), next));
                }
            }
        }
    }
    None
}

/// Build a `petgraph` `DiGraph` from the cycle graph (id -> neighbour ids), returning the
/// node-index → id map (used to translate Tarjan SCC results back to ids). `petgraph` stays a
/// **private** dependency (no petgraph type appears in any public signature).
fn build_petgraph(
    graph: &BTreeMap<String, Vec<String>>,
) -> (
    petgraph::graph::DiGraph<(), ()>,
    HashMap<petgraph::graph::NodeIndex, String>,
) {
    use petgraph::graph::{DiGraph, NodeIndex};

    let mut digraph = DiGraph::new();
    let mut index_of: HashMap<String, NodeIndex> = HashMap::new();
    let mut id_of: HashMap<NodeIndex, String> = HashMap::new();

    let mut node = |digraph: &mut DiGraph<(), ()>, id: &str| -> NodeIndex {
        if let Some(idx) = index_of.get(id) {
            return *idx;
        }
        let idx = digraph.add_node(());
        index_of.insert(id.to_string(), idx);
        id_of.insert(idx, id.to_string());
        idx
    };

    for (from, neighbours) in graph {
        let a = node(&mut digraph, from);
        for to in neighbours {
            let b = node(&mut digraph, to);
            digraph.update_edge(a, b, ());
        }
    }
    (digraph, id_of)
}
