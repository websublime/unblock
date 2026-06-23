//! Dependency edges + the in-memory gating graph (crate plan §3.3, spine §3.2.1).
//!
//! Cycle gating uses the **full 4-type** `affects_ready_work` set (`blocks` / `parent-child` /
//! `conditional-blocks` / `waits-for`); a non-gating edge (e.g. `related`) can never create a
//! ready-gating cycle. The graph is built from the `dependencies` rows and reasoned over with
//! `petgraph` (a **private** dependency — no petgraph type appears in any public signature).

use std::collections::HashMap;

use libsql::{Connection, Value};
use petgraph::graph::{DiGraph, NodeIndex};

use unblock_model::{DepTree, Dependency, DependencyType, GraphEdge};

use crate::error::{StorageError, map_libsql_err};

use super::events::append_event_in_tx;
use super::mappers::dependency_from_row;
use super::{WriteHook, with_immediate_tx};

/// The gating dependency types (`affects_ready_work`), as their wire strings, for SQL `IN (…)`.
const GATING_TYPES: [&str; 4] = ["blocks", "parent-child", "conditional-blocks", "waits-for"];

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

        // Cycle check for a gating edge.
        if dep.dep_type.affects_ready_work()
            && would_cycle_in_tx(&tx, &dep.issue_id, &dep.depends_on_id).await?
        {
            return Err(StorageError::CycleDetected {
                path: format!(
                    "{} -> {} -> … -> {}",
                    dep.issue_id, dep.depends_on_id, dep.issue_id
                ),
            });
        }

        tx.execute(
            "INSERT INTO dependencies (issue_id, depends_on_id, type, created_at, created_by) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            libsql::params![
                dep.issue_id.as_str(),
                dep.depends_on_id.as_str(),
                dep.dep_type.as_str(),
                dep.created_at.to_rfc3339(),
                dep.created_by.as_deref().unwrap_or(actor.as_str()),
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

/// Detect every cycle over the **gating** edge set, returning each as a path of ids.
pub(super) async fn detect_cycles(conn: &Connection) -> Result<Vec<Vec<String>>, StorageError> {
    let all = load_all_edges(conn).await?;
    let gating: Vec<(String, String)> = all
        .into_iter()
        .filter(|(_, _, t)| t.affects_ready_work())
        .map(|(from, to, _)| (from, to))
        .collect();

    let (graph, index_of, id_of) = build_petgraph(&gating);

    // Every strongly-connected component of size > 1 (or a self-loop) is a cycle. Use Tarjan's SCC.
    let sccs = petgraph::algo::tarjan_scc(&graph);
    let mut cycles = Vec::new();
    for scc in sccs {
        if scc.len() > 1 {
            let mut path: Vec<String> = scc.iter().map(|n| id_of[n].clone()).collect();
            path.sort();
            cycles.push(path);
        } else if let Some(node) = scc.first() {
            // A single node is a cycle only if it has a self-edge (gating self-deps are rejected on
            // insert, so this is defensive).
            if graph.contains_edge(*node, *node) {
                cycles.push(vec![id_of[node].clone()]);
            }
        }
    }
    let _ = &index_of; // index_of retained for symmetry / future path reconstruction.
    cycles.sort();
    Ok(cycles)
}

/// Whether adding the gating edge `from -> to` would close a cycle (i.e. `from` is already reachable
/// from `to` over the existing gating edges). Run within the caller's transaction.
pub(super) async fn would_cycle_in_tx(
    tx: &libsql::Transaction,
    from: &str,
    to: &str,
) -> Result<bool, StorageError> {
    // Load the existing gating edges.
    let placeholders: Vec<String> = (1..=GATING_TYPES.len()).map(|i| format!("?{i}")).collect();
    let sql = format!(
        "SELECT issue_id, depends_on_id FROM dependencies WHERE type IN ({})",
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

    let mut edges: Vec<(String, String)> = Vec::new();
    while let Some(row) = rows.next().await.map_err(map_libsql_err)? {
        let Value::Text(issue_id) = row.get_value(0).map_err(map_libsql_err)? else {
            continue;
        };
        let Value::Text(depends_on) = row.get_value(1).map_err(map_libsql_err)? else {
            continue;
        };
        edges.push((issue_id, depends_on));
    }
    // The prospective edge.
    edges.push((from.to_string(), to.to_string()));

    let (graph, index_of, _id_of) = build_petgraph(&edges);
    // A cycle exists iff the graph (now including from->to) is not a DAG.
    let _ = &index_of;
    Ok(petgraph::algo::is_cyclic_directed(&graph))
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
        let dep_type = type_str
            .parse::<DependencyType>()
            .unwrap_or(DependencyType::Blocks);
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

/// Build a `petgraph` `DiGraph` of `from -> to` id edges, returning the node-index maps.
fn build_petgraph(
    edges: &[(String, String)],
) -> (
    DiGraph<(), ()>,
    HashMap<String, NodeIndex>,
    HashMap<NodeIndex, String>,
) {
    let mut graph = DiGraph::new();
    let mut index_of: HashMap<String, NodeIndex> = HashMap::new();
    let mut id_of: HashMap<NodeIndex, String> = HashMap::new();

    let node = |graph: &mut DiGraph<(), ()>,
                index_of: &mut HashMap<String, NodeIndex>,
                id_of: &mut HashMap<NodeIndex, String>,
                id: &str|
     -> NodeIndex {
        if let Some(idx) = index_of.get(id) {
            return *idx;
        }
        let idx = graph.add_node(());
        index_of.insert(id.to_string(), idx);
        id_of.insert(idx, id.to_string());
        idx
    };

    for (from, to) in edges {
        let a = node(&mut graph, &mut index_of, &mut id_of, from);
        let b = node(&mut graph, &mut index_of, &mut id_of, to);
        graph.update_edge(a, b, ());
    }
    (graph, index_of, id_of)
}
