//! Read queries (crate plan §3.3, spine §3.2.1). All run on the read connection (FR-10).

use std::collections::HashSet;
use std::fmt::Write as _;

use chrono::{DateTime, Utc};
use libsql::{Connection, Value, params_from_iter};

use unblock_model::{CountBucket, CountGroupBy, Issue, ListFilters};

use crate::error::{StorageError, map_libsql_err};

use super::crud::get_issue;
use super::ids::escape_like_pattern;

/// The default result cap applied to `search` when the caller sets no `limit` (spine §3.2.1).
const SEARCH_DEFAULT_CAP: usize = 50;

/// Read a `Vec<Issue>` from a prepared query, hydrating each via `get_issue` (labels + deps).
async fn collect_hydrated(
    conn: &Connection,
    sql: &str,
    params: Vec<Value>,
) -> Result<Vec<Issue>, StorageError> {
    let mut rows = conn
        .query(sql, params_from_iter(params))
        .await
        .map_err(map_libsql_err)?;
    let mut ids = Vec::new();
    while let Some(row) = rows.next().await.map_err(map_libsql_err)? {
        if let Value::Text(id) = row.get_value(0).map_err(map_libsql_err)? {
            ids.push(id);
        }
    }
    let mut out = Vec::with_capacity(ids.len());
    for id in &ids {
        if let Some(issue) = get_issue(conn, id).await? {
            out.push(issue);
        }
    }
    Ok(out)
}

/// `list_issues`: compose `ListFilters` into a parameterized query (status/type OR, labels AND/OR,
/// priority range, text LIKE-escaped, `include_deferred/closed`, limit/offset).
pub(super) async fn list_issues(
    conn: &Connection,
    filters: &ListFilters,
) -> Result<Vec<Issue>, StorageError> {
    let (where_sql, params) = build_filter_where(filters);
    let mut sql = format!(
        "SELECT id FROM issues WHERE 1=1{where_sql} ORDER BY priority ASC, created_at DESC, id ASC"
    );
    append_limit_offset(&mut sql, filters);
    collect_hydrated(conn, &sql, params).await
}

/// `ready_issues`: `status='open'` + `id NOT IN <live blocked set>` + defer null-or-past + not
/// pinned/ephemeral/template, mirroring `idx_issues_ready` (wisp filter DROPPED). Ordered
/// `priority ASC, created_at ASC, id ASC`; default-complete unless `limit` set.
pub(super) async fn ready_issues(
    conn: &Connection,
    filters: &ListFilters,
) -> Result<Vec<Issue>, StorageError> {
    let blocked = live_blocked_ids(conn).await?;

    let mut sql = String::from(
        "SELECT id FROM issues WHERE status = 'open' \
         AND (defer_until IS NULL OR datetime(defer_until) <= datetime('now')) \
         AND (pinned = 0 OR pinned IS NULL) \
         AND (ephemeral = 0 OR ephemeral IS NULL) \
         AND (is_template = 0 OR is_template IS NULL)",
    );
    // Storage applies the same filter facets as list (type/priority/assignee) for parity.
    let (facet_sql, params) = build_facet_where(filters);
    sql.push_str(&facet_sql);
    sql.push_str(" ORDER BY priority ASC, created_at ASC, id ASC");

    // Build the candidate set, then drop blocked ids in Rust (the blocked set is computed live).
    let mut rows = conn
        .query(&sql, params_from_iter(params))
        .await
        .map_err(map_libsql_err)?;
    let mut ids = Vec::new();
    while let Some(row) = rows.next().await.map_err(map_libsql_err)? {
        if let Value::Text(id) = row.get_value(0).map_err(map_libsql_err)?
            && !blocked.contains(&id)
        {
            ids.push(id);
        }
    }
    if let Some(limit) = filters.limit {
        ids.truncate(limit);
    }

    let mut out = Vec::with_capacity(ids.len());
    for id in &ids {
        if let Some(issue) = get_issue(conn, id).await? {
            out.push(issue);
        }
    }
    Ok(out)
}

/// `blocked_issues`: `status NOT IN ('closed','tombstone')` (INCLUDES `in_progress/deferred`) filtered
/// to the live blocked set. Ordered `priority ASC, created_at DESC, id ASC`.
///
/// **Composes the `list` narrowing facets (D18, spine §3.2.1).** The same narrowing facet set as
/// `list_issues` — status-OR, `issue_type`-OR, inclusive priority range, `assignee`, `labels_all`
/// (AND) / `labels_any` (OR), and `text_contains` (title) — narrows the candidate rows **before**
/// the in-Rust blocked-set membership test (net = `{live blocked set} ∩ {facet-matched rows}`). The
/// three-pass blocked detection and the `ORDER BY` are unchanged — facets only filter.
///
/// The baseline `status NOT IN ('closed','tombstone')` is **deferred-INCLUSIVE** — `blocked` does
/// NOT inherit `list`'s default visibility branch (which strips `deferred`), so
/// `include_closed`/`include_deferred` are **no-ops** here (closed/tombstone can never be
/// blocked-visible; deferred is always shown).
pub(super) async fn blocked_issues(
    conn: &Connection,
    filters: &ListFilters,
) -> Result<Vec<Issue>, StorageError> {
    let blocked = live_blocked_ids(conn).await?;

    // Facets NARROW within blocked's OWN deferred-inclusive baseline; NO list visibility branch.
    let mut facet_sql = String::new();
    let mut params: Vec<Value> = Vec::new();
    compose_facets(filters, &mut facet_sql, &mut params);

    let sql = format!(
        "SELECT id FROM issues WHERE status NOT IN ('closed', 'tombstone'){facet_sql} \
         ORDER BY priority ASC, created_at DESC, id ASC"
    );

    let mut rows = conn
        .query(&sql, params_from_iter(params))
        .await
        .map_err(map_libsql_err)?;
    let mut ids = Vec::new();
    while let Some(row) = rows.next().await.map_err(map_libsql_err)? {
        if let Value::Text(id) = row.get_value(0).map_err(map_libsql_err)?
            && blocked.contains(&id)
        {
            ids.push(id);
        }
    }
    if let Some(limit) = filters.limit {
        ids.truncate(limit);
    }

    let mut out = Vec::with_capacity(ids.len());
    for id in &ids {
        if let Some(issue) = get_issue(conn, id).await? {
            out.push(issue);
        }
    }
    Ok(out)
}

/// `search_issues`: substring `instr(lower(col))` over title+description+id (needle lowercased, no
/// escaping). A `filters.text_contains` FILTER keeps `LIKE ? ESCAPE '\'` over title. Cap 50 when no
/// `limit`. Ordered `priority ASC, created_at DESC, id ASC`.
pub(super) async fn search_issues(
    conn: &Connection,
    query: &str,
    filters: &ListFilters,
) -> Result<Vec<Issue>, StorageError> {
    let needle = query.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return Ok(Vec::new());
    }

    let (facet_sql, mut params) = build_filter_where(filters);
    let mut sql = format!("SELECT id FROM issues WHERE 1=1{facet_sql}");

    // The substring needle over title + description + id.
    let base = params.len();
    let _ = write!(
        sql,
        " AND (instr(lower(title), ?{}) > 0 OR instr(lower(description), ?{}) > 0 OR instr(lower(id), ?{}) > 0)",
        base + 1,
        base + 2,
        base + 3
    );
    params.push(Value::Text(needle.clone()));
    params.push(Value::Text(needle.clone()));
    params.push(Value::Text(needle));

    sql.push_str(" ORDER BY priority ASC, created_at DESC, id ASC");

    let limit = filters.limit.unwrap_or(SEARCH_DEFAULT_CAP);
    let _ = write!(sql, " LIMIT {limit}");
    if let Some(offset) = filters.offset
        && offset > 0
    {
        let _ = write!(sql, " OFFSET {offset}");
    }

    collect_hydrated(conn, &sql, params).await
}

/// `count_issues`: total or grouped (by status/type/assignee/priority/label) count over `filters`.
pub(super) async fn count_issues(
    conn: &Connection,
    filters: &ListFilters,
    group_by: Option<CountGroupBy>,
) -> Result<Vec<CountBucket>, StorageError> {
    let (where_sql, params) = build_filter_where(filters);

    let Some(group) = group_by else {
        let sql = format!("SELECT COUNT(*) FROM issues WHERE 1=1{where_sql}");
        let mut rows = conn
            .query(&sql, params_from_iter(params))
            .await
            .map_err(map_libsql_err)?;
        let count = match rows.next().await.map_err(map_libsql_err)? {
            Some(row) => match row.get_value(0).map_err(map_libsql_err)? {
                Value::Integer(i) => usize::try_from(i).unwrap_or(0),
                _ => 0,
            },
            None => 0,
        };
        return Ok(vec![CountBucket {
            key: "total".to_string(),
            count,
        }]);
    };

    // Label grouping joins the labels table; the rest group on an issues column.
    let (key_expr, from_clause) = match group {
        CountGroupBy::Status => ("status", "issues"),
        CountGroupBy::Type => ("issue_type", "issues"),
        CountGroupBy::Assignee => ("COALESCE(assignee, '')", "issues"),
        CountGroupBy::Priority => ("CAST(priority AS TEXT)", "issues"),
        CountGroupBy::Label => (
            "labels.label",
            "issues JOIN labels ON labels.issue_id = issues.id",
        ),
    };
    let sql = format!(
        "SELECT {key_expr} AS k, COUNT(*) FROM {from_clause} WHERE 1=1{where_sql} GROUP BY k ORDER BY k ASC"
    );
    let mut rows = conn
        .query(&sql, params_from_iter(params))
        .await
        .map_err(map_libsql_err)?;
    let mut buckets = Vec::new();
    while let Some(row) = rows.next().await.map_err(map_libsql_err)? {
        let key = match row.get_value(0).map_err(map_libsql_err)? {
            Value::Text(s) => s,
            Value::Integer(i) => i.to_string(),
            _ => String::new(),
        };
        let count = match row.get_value(1).map_err(map_libsql_err)? {
            Value::Integer(i) => usize::try_from(i).unwrap_or(0),
            _ => 0,
        };
        buckets.push(CountBucket { key, count });
    }
    Ok(buckets)
}

/// `stale_issues`: issues with `updated_at < older_than` matching `filters`.
pub(super) async fn stale_issues(
    conn: &Connection,
    older_than: DateTime<Utc>,
    filters: &ListFilters,
) -> Result<Vec<Issue>, StorageError> {
    let (where_sql, mut params) = build_filter_where(filters);
    let idx = params.len() + 1;
    let mut sql = format!(
        "SELECT id FROM issues WHERE 1=1{where_sql} AND updated_at < ?{idx} \
         ORDER BY updated_at ASC, id ASC"
    );
    params.push(Value::Text(older_than.to_rfc3339()));
    append_limit_offset(&mut sql, filters);
    collect_hydrated(conn, &sql, params).await
}

// --------------------------------------------------------------------------------------------------
// Live blocked-set computation (THREE passes; spine §3.2.1) — replaces the original's
// `blocked_issues_cache` JOIN/NOT-IN. Mirrors `compute_blocked_issues_map_impl`
// (sqlite.rs:5720-5746) which combines direct blockers, `propagate_blocked_parents`
// (sqlite.rs:6371-6398), and the epic-open-child rollup.
// --------------------------------------------------------------------------------------------------

/// Compute the set of issue ids that are currently **blocked** (the live three-pass union):
/// 1. **direct 3-type blockers** — a `'blocks'`/`'conditional-blocks'`/`'waits-for'` edge whose
///    blocker is not `closed`/`tombstone` (`external:%` and template blockers excluded; a missing
///    blocker id, via `LEFT JOIN`, is treated as unresolved),
/// 2. **open epic-rollup children** — a `'parent-child'` edge where the parent's `issue_type='epic'`
///    and the child's status is non-terminal marks the **parent** blocked, and
/// 3. **transitive children of blocked parents** — a fixpoint BFS down the `'parent-child'` tree:
///    every issue with a `'parent-child'` edge to an already-blocked parent is itself blocked,
///    iterated until no new ids are added (mirrors `propagate_blocked_parents`, sqlite.rs:6371-6398;
///    the edges come from `load_local_parent_child_edges_impl`, sqlite.rs:6165-6191 — `parent =
///    depends_on_id`, `child = issue_id`, `external:%` excluded).
pub(super) async fn live_blocked_ids(conn: &Connection) -> Result<HashSet<String>, StorageError> {
    let mut blocked = HashSet::new();

    // Pass 1: direct 3-type blockers.
    let mut rows = conn
        .query(
            "SELECT DISTINCT d.issue_id \
             FROM dependencies d \
             LEFT JOIN issues i ON d.depends_on_id = i.id \
             WHERE d.type IN ('blocks', 'conditional-blocks', 'waits-for') \
               AND d.depends_on_id NOT LIKE 'external:%' \
               AND (i.status NOT IN ('closed', 'tombstone') OR i.id IS NULL) \
               AND (i.is_template = 0 OR i.is_template IS NULL OR i.id IS NULL)",
            (),
        )
        .await
        .map_err(map_libsql_err)?;
    while let Some(row) = rows.next().await.map_err(map_libsql_err)? {
        if let Value::Text(id) = row.get_value(0).map_err(map_libsql_err)? {
            blocked.insert(id);
        }
    }

    // Pass 2: epic parents with open children (the parent = depends_on_id is blocked).
    let mut rows = conn
        .query(
            "SELECT DISTINCT d.depends_on_id \
             FROM dependencies d \
             JOIN issues i ON d.issue_id = i.id \
             JOIN issues p ON d.depends_on_id = p.id \
             WHERE d.type = 'parent-child' \
               AND p.issue_type = 'epic' \
               AND i.status NOT IN ('closed', 'tombstone') \
               AND (i.is_template = 0 OR i.is_template IS NULL) \
               AND d.depends_on_id NOT LIKE 'external:%' \
               AND d.issue_id NOT LIKE 'external:%'",
            (),
        )
        .await
        .map_err(map_libsql_err)?;
    while let Some(row) = rows.next().await.map_err(map_libsql_err)? {
        if let Value::Text(id) = row.get_value(0).map_err(map_libsql_err)? {
            blocked.insert(id);
        }
    }

    // Pass 3: propagate blocked-state DOWN the parent-child tree to a fixpoint. A child of a blocked
    // parent is itself blocked (transitive). Mirrors `propagate_blocked_parents` (sqlite.rs:6371).
    propagate_blocked_to_children(conn, &mut blocked).await?;

    Ok(blocked)
}

/// Mark every transitive child of an already-blocked parent as blocked, iterating to a fixpoint
/// (BFS down the `'parent-child'` tree). The edge set is `parent (depends_on_id) -> [children
/// (issue_id)]`, with `external:%` excluded on both ends (sqlite.rs:6165-6191). Terminal/template
/// children stay blocked once marked — this mirrors the original `propagate_blocked_parents`, which
/// propagates over the structural edge regardless of the child's own status (a directly-blocked
/// child only *enters* the seed set via passes 1/2, but down-propagation is purely structural).
async fn propagate_blocked_to_children(
    conn: &Connection,
    blocked: &mut HashSet<String>,
) -> Result<(), StorageError> {
    // Load the parent-child edges once: parent_id -> [child_id, …].
    let mut rows = conn
        .query(
            "SELECT issue_id, depends_on_id FROM dependencies \
             WHERE type = 'parent-child' \
               AND issue_id NOT LIKE 'external:%' \
               AND depends_on_id NOT LIKE 'external:%'",
            (),
        )
        .await
        .map_err(map_libsql_err)?;

    let mut children_by_parent: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    while let Some(row) = rows.next().await.map_err(map_libsql_err)? {
        let Value::Text(child_id) = row.get_value(0).map_err(map_libsql_err)? else {
            continue;
        };
        let Value::Text(parent_id) = row.get_value(1).map_err(map_libsql_err)? else {
            continue;
        };
        children_by_parent
            .entry(parent_id)
            .or_default()
            .push(child_id);
    }

    if children_by_parent.is_empty() || blocked.is_empty() {
        return Ok(());
    }

    // BFS fixpoint: from each blocked parent, mark its children blocked and enqueue them.
    let mut queue: Vec<String> = blocked.iter().cloned().collect();
    while let Some(parent_id) = queue.pop() {
        if let Some(children) = children_by_parent.get(&parent_id) {
            for child_id in children {
                if blocked.insert(child_id.clone()) {
                    queue.push(child_id.clone());
                }
            }
        }
    }
    Ok(())
}

// --------------------------------------------------------------------------------------------------
// Filter composition
// --------------------------------------------------------------------------------------------------

/// Build the shared `WHERE` fragment (`" AND …"`, to follow `WHERE 1=1`) + params for the full
/// filter set (status/type OR, priority range, assignee, labels AND/OR, `text_contains` LIKE,
/// `include_deferred/closed`).
///
/// **Byte-identical** to the historical emit order for its four callers (list/search/count/stale):
/// `facets_into` → `text_contains` → visibility branch → labels. The visibility branch binds no
/// params, so the param-bearing fragments still emit in the order facets → text → labels and every
/// `?N` index is unchanged. `blocked_issues` does NOT use this helper — it owns its own
/// deferred-inclusive status predicate and uses [`compose_facets`] (the narrowing facets *without*
/// the visibility branch) — D18, spine §3.2.1.
fn build_filter_where(filters: &ListFilters) -> (String, Vec<Value>) {
    let mut sql = String::new();
    let mut params: Vec<Value> = Vec::new();

    facets_into(filters, &mut sql, &mut params);
    text_contains_filter(filters, &mut sql, &mut params);
    visibility_branch(filters, &mut sql);
    labels_filters(filters, &mut sql, &mut params);

    (sql, params)
}

/// Emit the **narrowing** facets (status-OR, type-OR, priority range, assignee, `text_contains`,
/// labels AND/OR) into `sql`/`params`. Does **NOT** emit the closed/deferred visibility branch —
/// callers that own their own status predicate (`blocked_issues`) append nothing for visibility;
/// list/search/count/stale add visibility separately via [`build_filter_where`] (D18, spine §3.2.1).
///
/// Param-emit order matches [`build_filter_where`] (`facets_into` → `text_contains` → labels), so a
/// caller composing this directly produces the same `?N` indices the full helper would.
fn compose_facets(filters: &ListFilters, sql: &mut String, params: &mut Vec<Value>) {
    facets_into(filters, sql, params);
    text_contains_filter(filters, sql, params);
    labels_filters(filters, sql, params);
}

/// `text_contains` FILTER: keeps `LIKE ? ESCAPE '\'` over `title` (distinct from the search needle).
fn text_contains_filter(filters: &ListFilters, sql: &mut String, params: &mut Vec<Value>) {
    if let Some(text) = &filters.text_contains {
        let idx = params.len() + 1;
        let _ = write!(sql, " AND title LIKE ?{idx} ESCAPE '\\'");
        params.push(Value::Text(format!("%{}%", escape_like_pattern(text))));
    }
}

/// Closed / deferred / tombstone visibility branch (default: exclude closed + tombstone; deferred
/// excluded unless asked). Binds **no** params — `list`/`search`/`count`/`stale` only.
/// `blocked_issues` does NOT apply this (its baseline is deferred-INCLUSIVE — D18, spine §3.2.1).
///
/// `include_tombstone` (FORK-1/D23, spine §1.10) is the WIDEST switch, checked OUTERMOST: it is set
/// only by the `unblock-sync` full-corpus export so tombstoned rows round-trip (FR-8). When it is
/// `false` (every non-export caller), this falls through to the EXACT prior 3-branch behaviour —
/// byte-identical SQL, so those callers are unchanged.
fn visibility_branch(filters: &ListFilters, sql: &mut String) {
    if filters.include_tombstone {
        // Export path: tombstones stay visible. With `include_closed` also set (the sync export),
        // append nothing → all statuses. Otherwise exclude only `closed` (deferred stays visible for
        // a full pull; tombstone is NOT excluded).
        if !filters.include_closed {
            sql.push_str(" AND status != 'closed'");
        }
    } else if filters.include_closed {
        sql.push_str(" AND status != 'tombstone'");
    } else if filters.include_deferred {
        sql.push_str(" AND status NOT IN ('closed', 'tombstone')");
    } else {
        sql.push_str(" AND status NOT IN ('closed', 'tombstone', 'deferred')");
    }
}

/// Labels AND / OR via membership subqueries (`EXISTS` correlated on `issues.id`).
fn labels_filters(filters: &ListFilters, sql: &mut String, params: &mut Vec<Value>) {
    for label in &filters.labels_all {
        let idx = params.len() + 1;
        let _ = write!(
            sql,
            " AND EXISTS (SELECT 1 FROM labels l WHERE l.issue_id = issues.id AND l.label = ?{idx})"
        );
        params.push(Value::Text(label.clone()));
    }
    if !filters.labels_any.is_empty() {
        let placeholders: Vec<String> = filters
            .labels_any
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", params.len() + 1 + i))
            .collect();
        let _ = write!(
            sql,
            " AND EXISTS (SELECT 1 FROM labels l WHERE l.issue_id = issues.id AND l.label IN ({}))",
            placeholders.join(", ")
        );
        for label in &filters.labels_any {
            params.push(Value::Text(label.clone()));
        }
    }
}

/// Build the facet-only `WHERE` fragment (status/type OR, priority range, assignee) for `ready`
/// (which has its own status/defer predicate and must not re-apply closed/deferred visibility).
fn build_facet_where(filters: &ListFilters) -> (String, Vec<Value>) {
    let mut sql = String::new();
    let mut params: Vec<Value> = Vec::new();
    // ready is open-only by definition, so the status-OR facet is not applied; only type/priority/
    // assignee narrow the candidate set.
    type_filter(filters, &mut sql, &mut params);
    priority_range(filters, &mut sql, &mut params);
    if let Some(assignee) = &filters.assignee {
        let idx = params.len() + 1;
        let _ = write!(sql, " AND assignee = ?{idx}");
        params.push(Value::Text(assignee.clone()));
    }
    (sql, params)
}

/// status-OR + type-OR + priority range + assignee facets shared by list/search/count/stale.
fn facets_into(filters: &ListFilters, sql: &mut String, params: &mut Vec<Value>) {
    if !filters.status.is_empty() {
        let placeholders: Vec<String> = filters
            .status
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", params.len() + 1 + i))
            .collect();
        let _ = write!(sql, " AND status IN ({})", placeholders.join(", "));
        for s in &filters.status {
            params.push(Value::Text(s.as_str().to_string()));
        }
    }
    type_filter(filters, sql, params);
    priority_range(filters, sql, params);
    if let Some(assignee) = &filters.assignee {
        let idx = params.len() + 1;
        let _ = write!(sql, " AND assignee = ?{idx}");
        params.push(Value::Text(assignee.clone()));
    }
}

/// type-OR facet (no-op when no types are requested — never emits an empty `IN ()`).
fn type_filter(filters: &ListFilters, sql: &mut String, params: &mut Vec<Value>) {
    if filters.issue_type.is_empty() {
        return;
    }
    let placeholders: Vec<String> = filters
        .issue_type
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", params.len() + 1 + i))
        .collect();
    let _ = write!(sql, " AND issue_type IN ({})", placeholders.join(", "));
    for t in &filters.issue_type {
        params.push(Value::Text(t.as_str().to_string()));
    }
}

/// inclusive priority range facet.
fn priority_range(filters: &ListFilters, sql: &mut String, params: &mut Vec<Value>) {
    if let Some(min) = filters.priority_min {
        let idx = params.len() + 1;
        let _ = write!(sql, " AND priority >= ?{idx}");
        params.push(Value::Integer(i64::from(min.0)));
    }
    if let Some(max) = filters.priority_max {
        let idx = params.len() + 1;
        let _ = write!(sql, " AND priority <= ?{idx}");
        params.push(Value::Integer(i64::from(max.0)));
    }
}

/// Append `LIMIT`/`OFFSET` from the filters (only when `limit` is set; offset alone uses `LIMIT -1`).
fn append_limit_offset(sql: &mut String, filters: &ListFilters) {
    match (filters.limit, filters.offset) {
        (Some(limit), offset) => {
            let _ = write!(sql, " LIMIT {limit}");
            if let Some(offset) = offset
                && offset > 0
            {
                let _ = write!(sql, " OFFSET {offset}");
            }
        }
        (None, Some(offset)) if offset > 0 => {
            let _ = write!(sql, " LIMIT -1 OFFSET {offset}");
        }
        _ => {}
    }
}
