//! Issue CRUD (crate plan §3.3, spine §3.2.1). Every mutation runs inside one `BEGIN IMMEDIATE`
//! transaction (rows + audit events commit together, FR-9).

use std::collections::{HashMap, HashSet};

use chrono::Utc;
use libsql::{Connection, Value, params_from_iter};

use unblock_model::{Comment, Dependency, Issue, IssueValidator, parse_id};

use crate::error::{StorageError, map_libsql_err};
use crate::filters::{DeleteMode, DeletePlan};

use super::events::append_event_in_tx;
use super::ids::update_child_counter_in_tx;
use super::mappers::{
    ISSUE_COLUMNS, bind_issue, comment_from_row, dependency_from_row, issue_from_row,
};
use super::{WriteHook, with_immediate_tx};

/// Create an issue: validate, guard against `id/external_ref` collisions, insert the row + relations,
/// write `Event(Created)` (+ per-relation events) — all in one tx. Returns the allocated id.
///
/// There is **no** content-hash dedup (spine §3.2.1): the hash is computed and stored, never used to
/// short-circuit. FR-26 import idempotency lives in `unblock-sync`, not here.
///
/// The per-record body is the shared [`insert_issue_in_tx`] helper — so the single-create path and the
/// atomic bulk path ([`create_issues`]) run **identical** in-tx logic (one source of truth, spine
/// §3.2.1 / crate plan §3.3). This wrapper just opens its OWN one-shot `with_immediate_tx`.
pub(super) async fn create_issue(
    conn: &Connection,
    hook: WriteHook<'_>,
    issue: &Issue,
    actor: &str,
) -> Result<String, StorageError> {
    IssueValidator::validate(issue).map_err(|e| StorageError::InvalidId {
        // A validation failure on create surfaces the offending id; the detailed field errors are a
        // model concern (the engine validates before calling, so this is a defensive backstop).
        id: format!("{}: {e}", issue.id),
    })?;

    let content_hash = issue.compute_content_hash();
    let issue = issue.clone();

    with_immediate_tx(conn, hook, |tx| async move {
        insert_issue_in_tx(&tx, &issue, &content_hash, actor).await?;
        let id = issue.id.clone();
        Ok((id, tx))
    })
    .await
}

/// Insert ONE fully-formed `Issue` inside the caller's already-open transaction (the shared per-record
/// body, D22/T2.3 — spine §3.2.1 / crate plan §3.3).
///
/// Runs the `create_issue` per-record work — the id-collision guard, the `external_ref`-collision
/// guard, the row INSERT (binding the supplied `content_hash`), the in-tx `child_counters` bump for a
/// hierarchical id, the deduped label/dependency/comment inserts with their per-relation
/// `Event(LabelAdded)`/`Event(DependencyAdded)`/`Event(Commented)`, and the defining `Event(Created)`.
///
/// It does **no minting and no validation** (the engine layer validates/mints first — storage stays
/// validation-free, like `create_issue`). It borrows `&tx` so the bulk path can call it N times inside
/// ONE `with_immediate_tx`; on any `Err` the caller's tx rolls back the WHOLE batch (ZERO rows
/// persist). Both `create_issue` (its own one-shot tx) and `create_issues` (the ONE shared tx, looped)
/// call this — never each other.
#[allow(clippy::too_many_lines)] // one cohesive record body: row + labels + deps + comments + events
pub(super) async fn insert_issue_in_tx(
    tx: &libsql::Transaction,
    issue: &Issue,
    content_hash: &str,
    actor: &str,
) -> Result<(), StorageError> {
    // Guard 1: id collision.
    if row_exists(tx, "SELECT 1 FROM issues WHERE id = ?1 LIMIT 1", &issue.id).await? {
        return Err(StorageError::IdCollision {
            id: issue.id.clone(),
        });
    }

    // Guard 2: external_ref collision.
    if let Some(ext_ref) = issue.external_ref.as_deref()
        && row_exists(
            tx,
            "SELECT 1 FROM issues WHERE external_ref = ?1 LIMIT 1",
            ext_ref,
        )
        .await?
    {
        return Err(StorageError::Backend {
            source: crate::error::BackendOpaque::from_message(format!(
                "external reference already exists: {ext_ref}"
            )),
        });
    }

    // Insert the issue row.
    let columns = format!(
        "INSERT INTO issues ({ISSUE_COLUMNS}) VALUES ({})",
        placeholders(38)
    );
    let params = bind_issue(issue, content_hash);
    tx.execute(&columns, params_from_iter(params))
        .await
        .map_err(map_libsql_err)?;

    // Maintain the child counter for a hierarchical id.
    if let Ok(parsed) = parse_id(&issue.id)
        && !parsed.is_root()
        && let (Some(parent), Some(&child)) = (parsed.parent(), parsed.child_path.last())
    {
        update_child_counter_in_tx(tx, &parent, child).await?;
    }

    // Insert labels (deduped) + Event(LabelAdded) each.
    let mut seen = HashSet::new();
    for label in &issue.labels {
        if !seen.insert(label.as_str()) {
            continue;
        }
        tx.execute(
            "INSERT INTO labels (issue_id, label) VALUES (?1, ?2)",
            libsql::params![issue.id.as_str(), label.as_str()],
        )
        .await
        .map_err(map_libsql_err)?;
        append_event_in_tx(
            tx,
            &issue.id,
            &unblock_model::EventType::LabelAdded,
            actor,
            None,
            Some(label),
            None,
        )
        .await?;
    }

    // Insert dependencies (deduped) + Event(DependencyAdded) each.
    let mut seen_deps = HashSet::new();
    for dep in &issue.dependencies {
        if dep.depends_on_id == issue.id {
            return Err(StorageError::SelfDependency);
        }
        if !seen_deps.insert(dep.depends_on_id.as_str()) {
            continue;
        }
        tx.execute(
            "INSERT INTO dependencies (issue_id, depends_on_id, type, created_at, created_by) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            libsql::params![
                issue.id.as_str(),
                dep.depends_on_id.as_str(),
                dep.dep_type.as_str(),
                dep.created_at.to_rfc3339(),
                dep.created_by.as_deref().unwrap_or(actor),
            ],
        )
        .await
        .map_err(map_libsql_err)?;
        append_event_in_tx(
            tx,
            &issue.id,
            &unblock_model::EventType::DependencyAdded,
            actor,
            None,
            Some(&dep.depends_on_id),
            None,
        )
        .await?;
    }

    // Insert comments + Event(Commented) each.
    //
    // SIX columns, binding `updated_at` + `redacted_at` from the caller-supplied `Comment` (D37).
    // This is the create/bulk/IMPORT seed path: it REPLAYS an existing comment and must persist
    // whatever state it carries. Spine §3.2.1 MUST-1 ("only `update` ever sets `updated_at`") is
    // scoped to `add_comment` ONLY — over-applying it here and leaving this INSERT at 4 columns
    // silently drops both fields, so a redacted comment imports back UN-REDACTED.
    for comment in &issue.comments {
        tx.execute(
            "INSERT INTO comments (issue_id, author, text, created_at, updated_at, redacted_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            libsql::params![
                issue.id.as_str(),
                comment.author.as_str(),
                comment.body.as_str(),
                comment.created_at.to_rfc3339(),
                comment.updated_at.map(|ts| ts.to_rfc3339()),
                comment.redacted_at.map(|ts| ts.to_rfc3339()),
            ],
        )
        .await
        .map_err(map_libsql_err)?;
        append_event_in_tx(
            tx,
            &issue.id,
            &unblock_model::EventType::Commented,
            actor,
            None,
            None,
            Some(&comment.body),
        )
        .await?;
    }

    // The defining event.
    append_event_in_tx(
        tx,
        &issue.id,
        &unblock_model::EventType::Created,
        actor,
        None,
        None,
        Some(&format!("Created issue: {}", issue.title)),
    )
    .await?;

    Ok(())
}

/// Create the WHOLE slice in **exactly ONE** `BEGIN IMMEDIATE` tx (D22/T2.3 — the ATOMIC bulk INSERT,
/// spine §3.2.1).
///
/// Opens ONE `with_immediate_tx` (the same commit chokepoint as every other mutation) and loops the
/// shared [`insert_issue_in_tx`] helper per record — it is **NEVER a loop of `create_issue`** (N
/// `create_issue` calls = N independent txs = a partial-commit hole on the first mid-batch failure, the
/// exact bug this primitive closes). For EACH `Issue` the helper runs the SAME per-record body
/// `create_issue` runs, committing ONCE. It does **no minting and no validation** (the engine
/// `Session::create_bulk` mints every id + validates each built `Issue` BEFORE this — storage stays
/// validation-free).
///
/// **Atomicity is the whole point:** a failure on record #k (a raced `IdCollision`, an `external_ref`
/// clash, an FK/CHECK violation, any backend error) returns `Err` → `with_immediate_tx` rolls back the
/// whole tx → records 1..k-1 staged in the same tx are discarded → ZERO rows persist (no partial
/// batch). A dependency edge pointing at a sibling minted earlier in the SAME batch resolves because
/// both rows live in the one uncommitted tx. Same-parent siblings arrive with ALREADY-DISTINCT
/// `parent.N` ids (the engine mints them via its in-batch per-parent counter); the per-record
/// `child_counters` UPSERT (high-water MAX) lands each row's `N` monotonically regardless of order.
pub(super) async fn create_issues(
    conn: &Connection,
    hook: WriteHook<'_>,
    issues: &[Issue],
    actor: &str,
) -> Result<(), StorageError> {
    // Pre-compute each content hash before entering the tx (no clone of the slice; the helper borrows).
    let hashes: Vec<String> = issues.iter().map(Issue::compute_content_hash).collect();

    with_immediate_tx(conn, hook, |tx| async move {
        for (issue, content_hash) in issues.iter().zip(hashes.iter()) {
            insert_issue_in_tx(&tx, issue, content_hash, actor).await?;
        }
        Ok(((), tx))
    })
    .await
}

/// Fetch a single issue (hydrated with labels + dependencies + comments), or `None` if absent.
pub(super) async fn get_issue(conn: &Connection, id: &str) -> Result<Option<Issue>, StorageError> {
    let sql = format!("SELECT {ISSUE_COLUMNS} FROM issues WHERE id = ?1");
    let mut rows = conn
        .query(&sql, libsql::params![id])
        .await
        .map_err(map_libsql_err)?;
    let Some(row) = rows.next().await.map_err(map_libsql_err)? else {
        return Ok(None);
    };
    let mut issue = issue_from_row(&row)?;
    hydrate(conn, &mut issue).await?;
    Ok(Some(issue))
}

/// Fetch multiple issues by id (hydrated), preserving the caller's id order. Unknown ids are simply
/// absent from the result.
///
/// Routes through the batched [`hydrate_ids`] helper (T3.5.1) instead of a per-id `get_issue` loop
/// (which was `1 + 3N` queries). The result keeps the caller's order and drops absent ids exactly
/// as the loop did. The ids are treated as a **lookup set**: a repeated id is hydrated at most once
/// (every production caller passes a de-duplicated set — `session::write` sorts+dedups its
/// candidate/blocker id lists and reconstructs its response via a by-id `HashMap::remove`).
pub(super) async fn get_issues(
    conn: &Connection,
    ids: &[String],
) -> Result<Vec<Issue>, StorageError> {
    hydrate_ids(conn, ids).await
}

/// Hydrate an issue's `labels` (sorted), `dependencies` and `comments` from their tables.
async fn hydrate(conn: &Connection, issue: &mut Issue) -> Result<(), StorageError> {
    // Labels.
    let mut rows = conn
        .query(
            "SELECT label FROM labels WHERE issue_id = ?1 ORDER BY label ASC",
            libsql::params![issue.id.as_str()],
        )
        .await
        .map_err(map_libsql_err)?;
    let mut labels = Vec::new();
    while let Some(row) = rows.next().await.map_err(map_libsql_err)? {
        if let Value::Text(label) = row.get_value(0).map_err(map_libsql_err)? {
            labels.push(label);
        }
    }
    issue.labels = labels;

    // Dependencies.
    let mut rows = conn
        .query(
            "SELECT issue_id, depends_on_id, type, created_at, created_by, metadata, thread_id \
             FROM dependencies WHERE issue_id = ?1 ORDER BY depends_on_id ASC, type ASC",
            libsql::params![issue.id.as_str()],
        )
        .await
        .map_err(map_libsql_err)?;
    let mut deps = Vec::new();
    while let Some(row) = rows.next().await.map_err(map_libsql_err)? {
        deps.push(dependency_from_row(&row)?);
    }
    issue.dependencies = deps;

    // Comments (D37) — canonical order `created_at ASC, id ASC` (spine §3.2.1).
    let mut rows = conn
        .query(
            "SELECT id, issue_id, author, text, created_at, updated_at, redacted_at \
             FROM comments WHERE issue_id = ?1 ORDER BY created_at ASC, id ASC",
            libsql::params![issue.id.as_str()],
        )
        .await
        .map_err(map_libsql_err)?;
    let mut comments = Vec::new();
    while let Some(row) = rows.next().await.map_err(map_libsql_err)? {
        comments.push(comment_from_row(&row)?);
    }
    issue.comments = comments;
    Ok(())
}

/// The maximum number of ids bound into a single `WHERE … IN (…)` batch in [`hydrate_ids`].
///
/// Each id is one bound parameter, bounded by `SQLITE_MAX_VARIABLE_NUMBER`. Kept safely under the
/// historical `999` floor (do **not** rely on the newer `32766` default): an unbounded read (no
/// `limit`, permitted by the API) can materialize an id set larger than any single `IN (…)` clause
/// may bind — e.g. the `scale`/read paths page at `1_000 > 999` — so the id list is chunked into
/// several bounded `IN` queries rather than emitting one oversized `IN (…)` that `SQLite` rejects at
/// runtime.
const HYDRATE_ID_CHUNK: usize = 900;

/// Batch-hydrate an **already-ordered** id list into fully-populated [`Issue`]s (labels +
/// dependencies + comments), preserving the input order.
///
/// This replaces the historical per-id `get_issue` loop (`1 + 4N` queries) with a batched fetch:
/// for each chunk of at most [`HYDRATE_ID_CHUNK`] ids it runs four `WHERE … IN (…)` queries — the
/// issue rows, the labels, the dependencies, and the comments (D37) — and folds **every** chunk
/// into ONE set of accumulators; reconstruction then maps over the full ordered `ids` exactly once.
/// Query count is `1 + 4·⌈N/CHUNK⌉` per call (the `1` is the caller's id query), versus `1 + 4N`.
///
/// **Byte-identical** to the old loop (`get_issue` per id):
/// - **outer order** — reassembly iterates `ids`, so the result order is the caller's id-query
///   order, never the arbitrary `IN`-result row order;
/// - **label sort** (`label ASC`), **dep sort** (`depends_on_id ASC, type ASC`) and **comment
///   sort** (`created_at ASC, id ASC`) — preserved by the per-relation `ORDER BY`; the leading
///   `issue_id` key only groups rows, leaving the same secondary keys to fully determine the
///   intra-issue order the single-id `SELECT`s produced;
/// - **skip-absent** — an id whose issue row has vanished (a tombstone/hard-delete race between the
///   id query and hydration) is dropped by `filter_map` returning `None`, never a panic or a
///   placeholder (identical to the old `if let Some(issue) = get_issue(…)`);
/// - **empty relations** — an issue with no labels/deps/comments gets an empty `Vec`
///   (`unwrap_or_default`);
/// - **empty id set** — an early `Ok(Vec::new())` guard (an `IN ()` is a SQL error).
///
/// The ids are treated as a lookup set: they are unique on every read path (each derives them from
/// a primary-key `SELECT`) and de-duplicated on the `get_issues` path, so a repeated id is hydrated
/// at most once.
pub(super) async fn hydrate_ids(
    conn: &Connection,
    ids: &[String],
) -> Result<Vec<Issue>, StorageError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    // One set of accumulators across ALL chunks (never a per-chunk map that resets — an issue's
    // labels/deps live wholly in the one chunk its id occupies, since ids are unique).
    let mut issues: HashMap<String, Issue> = HashMap::with_capacity(ids.len());
    let mut labels: HashMap<String, Vec<String>> = HashMap::new();
    let mut deps: HashMap<String, Vec<Dependency>> = HashMap::new();
    let mut comments: HashMap<String, Vec<Comment>> = HashMap::new();

    for chunk in ids.chunks(HYDRATE_ID_CHUNK) {
        let placeholder_list = placeholders(chunk.len());
        let params: Vec<Value> = chunk.iter().map(|id| Value::Text(id.clone())).collect();

        // Issue rows — the SAME 38-column projection as the single-id path, so `issue_from_row`
        // ordinals hold and `content_hash` recomputes identically (excludes labels/deps).
        let issue_sql =
            format!("SELECT {ISSUE_COLUMNS} FROM issues WHERE id IN ({placeholder_list})");
        let mut rows = conn
            .query(&issue_sql, params_from_iter(params.clone()))
            .await
            .map_err(map_libsql_err)?;
        while let Some(row) = rows.next().await.map_err(map_libsql_err)? {
            let issue = issue_from_row(&row)?;
            issues.insert(issue.id.clone(), issue);
        }

        // Labels — grouped by issue_id, `label ASC` within each (the added `issue_id` leading key
        // only groups; `label ASC` still fully orders each issue's labels).
        let labels_sql = format!(
            "SELECT issue_id, label FROM labels WHERE issue_id IN ({placeholder_list}) \
             ORDER BY issue_id ASC, label ASC"
        );
        let mut rows = conn
            .query(&labels_sql, params_from_iter(params.clone()))
            .await
            .map_err(map_libsql_err)?;
        while let Some(row) = rows.next().await.map_err(map_libsql_err)? {
            let Value::Text(issue_id) = row.get_value(0).map_err(map_libsql_err)? else {
                continue;
            };
            if let Value::Text(label) = row.get_value(1).map_err(map_libsql_err)? {
                labels.entry(issue_id).or_default().push(label);
            }
        }

        // Dependencies — grouped by issue_id, `depends_on_id ASC, type ASC` within each (identical
        // secondary ordering to the single-id `hydrate`).
        let deps_sql = format!(
            "SELECT issue_id, depends_on_id, type, created_at, created_by, metadata, thread_id \
             FROM dependencies WHERE issue_id IN ({placeholder_list}) \
             ORDER BY issue_id ASC, depends_on_id ASC, type ASC"
        );
        let mut rows = conn
            .query(&deps_sql, params_from_iter(params.clone()))
            .await
            .map_err(map_libsql_err)?;
        while let Some(row) = rows.next().await.map_err(map_libsql_err)? {
            let dep = dependency_from_row(&row)?;
            deps.entry(dep.issue_id.clone()).or_default().push(dep);
        }

        // Comments (D37) — grouped by issue_id, `created_at ASC, id ASC` within each (identical
        // secondary ordering to the single-id `hydrate`). The projection column ORDER is the
        // `comment_from_row` positional contract; note the mapper reads `issue_id` at ordinal 1,
        // so this SELECT deliberately keeps the natural column order rather than hoisting the
        // grouping key.
        let comments_sql = format!(
            "SELECT id, issue_id, author, text, created_at, updated_at, redacted_at \
             FROM comments WHERE issue_id IN ({placeholder_list}) \
             ORDER BY issue_id ASC, created_at ASC, id ASC"
        );
        let mut rows = conn
            .query(&comments_sql, params_from_iter(params))
            .await
            .map_err(map_libsql_err)?;
        while let Some(row) = rows.next().await.map_err(map_libsql_err)? {
            let comment = comment_from_row(&row)?;
            comments
                .entry(comment.issue_id.clone())
                .or_default()
                .push(comment);
        }
    }

    // Reconstruct by driving the ORDERED id vec (never the IN-row order). `remove` is safe: ids are
    // unique, and a missing row → `None` → the id is skipped.
    let out = ids
        .iter()
        .filter_map(|id| {
            let mut issue = issues.remove(id)?;
            issue.labels = labels.remove(id).unwrap_or_default();
            issue.dependencies = deps.remove(id).unwrap_or_default();
            issue.comments = comments.remove(id).unwrap_or_default();
            Some(issue)
        })
        .collect();
    Ok(out)
}

/// Accumulates a dynamic `UPDATE issues SET …` clause, tracking `?n` placeholder indices.
struct UpdateBuilder {
    set: Vec<String>,
    params: Vec<Value>,
}

impl UpdateBuilder {
    fn new() -> Self {
        Self {
            set: Vec::new(),
            params: Vec::new(),
        }
    }

    /// Append `col = ?n` with `val`. The placeholder index is `self.params.len() + 1`.
    fn push(&mut self, col: &str, val: Value) {
        let idx = self.params.len() + 1;
        self.set.push(format!("{col} = ?{idx}"));
        self.params.push(val);
    }

    fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    /// Apply a nullable-text patch field (`None` leave / `Some(None)` clear / `Some(Some)` set); no
    /// event. Body text columns are `DEFAULT ''`, so a cleared value stores `''` (bd convention).
    ///
    /// The `&Option<Option<String>>` shape mirrors the [`IssuePatch`](crate::filters::IssuePatch)
    /// field (outer = present-in-patch, inner = clear-vs-set), so the nested `Option` and the borrow
    /// are intentional.
    #[allow(clippy::option_option, clippy::ref_option)]
    fn push_opt_text(
        &mut self,
        col: &str,
        patch: &Option<Option<String>>,
        target: &mut Option<String>,
    ) {
        if let Some(new) = patch
            && new != target
        {
            self.push(col, Value::Text(new.clone().unwrap_or_default()));
            target.clone_from(new);
        }
    }
}

/// Apply an [`IssuePatch`](crate::filters::IssuePatch) (spine §3.2.1).
///
/// Builds a `SET` clause field-by-field; if **nothing** changes, the whole `UPDATE` is skipped —
/// no `SET`, no `updated_at` advance, no `content_hash` recompute, no `Event` (the empty-diff full
/// skip). When at least one stored column changes, `updated_at` advances and `content_hash` is
/// recomputed; per-field events are written **only** for the fields whose value actually changed
/// (see the §3.2.1 EventType-per-mutation table). A reparent (`parent`) is cycle-checked. Returns the
/// hydrated, updated issue.
///
/// `close_reason` is a `DEFAULT ''` body column persisted like the other body-text fields (no own
/// event — §3.2.1 table). It is patched **independently** of `status`: the `StatusChanged`/`Closed`
/// audit event does **not** carry the reason in v1 (wiring it would couple the two independent patch
/// fields through `append_event_in_tx`; deferred — the reason is still durably stored on the row).
#[allow(clippy::too_many_lines)]
pub(super) async fn update_issue(
    conn: &Connection,
    hook: WriteHook<'_>,
    id: &str,
    patch: &crate::filters::IssuePatch,
    actor: &str,
) -> Result<Issue, StorageError> {
    use unblock_model::{EventType, Status};

    let patch = patch.clone();
    let id_owned = id.to_string();
    let actor = actor.to_string();

    with_immediate_tx(conn, hook, |tx| async move {
        // Load the current row inside the tx (TOCTOU-safe).
        let sql = format!("SELECT {ISSUE_COLUMNS} FROM issues WHERE id = ?1");
        let mut rows = tx
            .query(&sql, libsql::params![id_owned.as_str()])
            .await
            .map_err(map_libsql_err)?;
        let Some(row) = rows.next().await.map_err(map_libsql_err)? else {
            return Err(StorageError::IssueNotFound { id: id_owned });
        };
        let mut issue = issue_from_row(&row)?;
        drop(rows);

        // A tombstone cannot be patched (the original rejects this).
        if issue.is_tombstone() {
            return Err(StorageError::IssueNotFound { id: id_owned });
        }

        let mut builder = UpdateBuilder::new();
        // (event_type, old, new) tuples, appended after the row update succeeds.
        let mut events: Vec<(EventType, Option<String>, Option<String>)> = Vec::new();

        // --- title (NOT NULL): change -> Updated ---
        if let Some(title) = patch.title.clone()
            && title != issue.title
        {
            events.push((
                EventType::Updated,
                Some(issue.title.clone()),
                Some(title.clone()),
            ));
            builder.push("title", Value::Text(title.clone()));
            issue.title = title;
        }

        // --- body text fields (no event) ---
        builder.push_opt_text("description", &patch.description, &mut issue.description);
        builder.push_opt_text("design", &patch.design, &mut issue.design);
        builder.push_opt_text(
            "acceptance_criteria",
            &patch.acceptance_criteria,
            &mut issue.acceptance_criteria,
        );
        builder.push_opt_text("notes", &patch.notes, &mut issue.notes);
        builder.push_opt_text("owner", &patch.owner, &mut issue.owner);
        // close_reason: a `DEFAULT ''` body column (no event); the close path sets it (T1.2,
        // spine §3.1). `Some(None)` clears to '' (→ None on load); `Some(Some(s))` sets it.
        builder.push_opt_text("close_reason", &patch.close_reason, &mut issue.close_reason);

        // --- external_ref (nullable, uniqueness-checked; no event) ---
        if let Some(ext) = patch.external_ref.clone()
            && ext != issue.external_ref
        {
            if let Some(ref ext_ref) = ext
                && external_ref_taken(&tx, ext_ref, &id_owned).await?
            {
                return Err(StorageError::Backend {
                    source: crate::error::BackendOpaque::from_message(format!(
                        "external reference already exists: {ext_ref}"
                    )),
                });
            }
            builder.push("external_ref", ext.clone().map_or(Value::Null, Value::Text));
            issue.external_ref = ext;
        }

        // --- status: change -> StatusChanged (+ Closed/Reopened/Deleted) ---
        if let Some(status) = patch.status.clone() {
            let old = issue.status.clone();
            if status.as_str() != old.as_str() {
                events.push((
                    EventType::StatusChanged,
                    Some(old.as_str().to_string()),
                    Some(status.as_str().to_string()),
                ));
                builder.push("status", Value::Text(status.as_str().to_string()));

                let was_terminal = old.is_terminal();
                if status == Status::Closed {
                    if !was_terminal {
                        events.push((EventType::Closed, None, None));
                    }
                    if issue.closed_at.is_none() {
                        let now = Utc::now();
                        builder.push("closed_at", Value::Text(now.to_rfc3339()));
                        issue.closed_at = Some(now);
                    }
                } else if status == Status::Tombstone {
                    if !was_terminal {
                        events.push((EventType::Deleted, None, None));
                    }
                    let now = Utc::now();
                    builder.push("deleted_at", Value::Text(now.to_rfc3339()));
                    builder.push("deleted_by", Value::Text(actor.clone()));
                    issue.deleted_at = Some(now);
                    issue.deleted_by = Some(actor.clone());
                } else {
                    if was_terminal && !status.is_terminal() {
                        events.push((EventType::Reopened, None, None));
                    }
                    if issue.closed_at.is_some() {
                        builder.push("closed_at", Value::Null);
                        issue.closed_at = None;
                    }
                }
                issue.status = status;
            }
        }

        // --- priority: change -> PriorityChanged ---
        if let Some(priority) = patch.priority
            && priority.0 != issue.priority.0
        {
            events.push((
                EventType::PriorityChanged,
                Some(issue.priority.0.to_string()),
                Some(priority.0.to_string()),
            ));
            builder.push("priority", Value::Integer(i64::from(priority.0)));
            issue.priority = priority;
        }

        // --- issue_type (no event) ---
        if let Some(it) = patch.issue_type.clone()
            && it.as_str() != issue.issue_type.as_str()
        {
            builder.push("issue_type", Value::Text(it.as_str().to_string()));
            issue.issue_type = it;
        }

        // --- assignee: change -> AssigneeChanged ---
        if let Some(assignee) = patch.assignee.clone()
            && assignee != issue.assignee
        {
            events.push((
                EventType::AssigneeChanged,
                issue.assignee.clone(),
                assignee.clone(),
            ));
            builder.push(
                "assignee",
                assignee.clone().map_or(Value::Null, Value::Text),
            );
            issue.assignee = assignee;
        }

        // --- estimated_minutes (scalar; no event) ---
        if let Some(minutes) = patch.estimated_minutes
            && Some(minutes) != issue.estimated_minutes
        {
            builder.push("estimated_minutes", Value::Integer(i64::from(minutes)));
            issue.estimated_minutes = Some(minutes);
        }

        // --- due_at (scalar; no event) ---
        if let Some(due) = patch.due_at
            && Some(due) != issue.due_at
        {
            builder.push("due_at", Value::Text(due.to_rfc3339()));
            issue.due_at = Some(due);
        }

        // --- label relation ops (LabelAdded/LabelRemoved) ---
        let label_changed = apply_labels(&tx, &id_owned, &actor, &patch, &mut issue).await?;

        // --- reparent (cycle-checked) ---
        // A real reparent emits DependencyRemoved/DependencyAdded into `events` and advances
        // `updated_at` (FR-1b) even when no row column otherwise changed.
        let parent_changed = apply_reparent(&tx, &id_owned, &patch, &actor, &mut events).await?;

        // Empty diff: nothing in the row, no relation change -> full skip (no updated_at, no event).
        if builder.is_empty() && !label_changed && !parent_changed {
            return Ok((issue, tx));
        }

        // A row column changed OR a real reparent occurred -> advance updated_at. A reparent with no
        // other row change still stamps `updated_at` so the modification is observable (FR-1b).
        // `content_hash` excludes `updated_at` + relations (spine §1.8), so the recompute on a
        // pure-reparent change is a no-op against the stored hash (parent/deps are not hashed).
        if !builder.is_empty() || parent_changed {
            let now = Utc::now();
            issue.updated_at = now;
            builder.push("updated_at", Value::Text(now.to_rfc3339()));
            let new_hash = issue.compute_content_hash();
            builder.push("content_hash", Value::Text(new_hash));

            let where_idx = builder.params.len() + 1;
            builder.params.push(Value::Text(id_owned.clone()));
            let update_sql = format!(
                "UPDATE issues SET {} WHERE id = ?{where_idx}",
                builder.set.join(", ")
            );
            tx.execute(&update_sql, params_from_iter(builder.params))
                .await
                .map_err(map_libsql_err)?;
        }

        // Append the per-field events after the successful row update.
        for (event_type, old, new) in &events {
            append_event_in_tx(
                &tx,
                &id_owned,
                event_type,
                &actor,
                old.as_deref(),
                new.as_deref(),
                None,
            )
            .await?;
        }

        Ok((issue, tx))
    })
    .await?;

    // Re-read with full hydration (labels sorted, deps fresh, content_hash recomputed on load).
    get_issue(conn, id)
        .await?
        .ok_or_else(|| StorageError::IssueNotFound { id: id.to_string() })
}

/// Whether `external_ref` is held by some **other** issue.
async fn external_ref_taken(
    tx: &libsql::Transaction,
    ext_ref: &str,
    id: &str,
) -> Result<bool, StorageError> {
    let mut rows = tx
        .query(
            "SELECT 1 FROM issues WHERE external_ref = ?1 AND id != ?2 LIMIT 1",
            libsql::params![ext_ref, id],
        )
        .await
        .map_err(map_libsql_err)?;
    Ok(rows.next().await.map_err(map_libsql_err)?.is_some())
}

/// Apply the `labels_set`/`labels_add`/`labels_remove` ops, emitting LabelAdded/LabelRemoved events.
/// Returns whether any label changed (so the empty-diff check accounts for label-only patches).
async fn apply_labels(
    tx: &libsql::Transaction,
    id: &str,
    actor: &str,
    patch: &crate::filters::IssuePatch,
    issue: &mut Issue,
) -> Result<bool, StorageError> {
    use unblock_model::EventType;
    let mut current: HashSet<String> = issue.labels.iter().cloned().collect();
    let before = current.clone();

    if let Some(set_labels) = &patch.labels_set {
        current = set_labels.iter().cloned().collect();
    }
    for add in &patch.labels_add {
        current.insert(add.clone());
    }
    for remove in &patch.labels_remove {
        current.remove(remove);
    }

    if current == before {
        return Ok(false);
    }

    // Reconcile: delete removed, insert added.
    for removed in before.difference(&current) {
        tx.execute(
            "DELETE FROM labels WHERE issue_id = ?1 AND label = ?2",
            libsql::params![id, removed.as_str()],
        )
        .await
        .map_err(map_libsql_err)?;
        append_event_in_tx(
            tx,
            id,
            &EventType::LabelRemoved,
            actor,
            Some(removed),
            None,
            None,
        )
        .await?;
    }
    for added in current.difference(&before) {
        tx.execute(
            "INSERT INTO labels (issue_id, label) VALUES (?1, ?2)",
            libsql::params![id, added.as_str()],
        )
        .await
        .map_err(map_libsql_err)?;
        append_event_in_tx(
            tx,
            id,
            &EventType::LabelAdded,
            actor,
            None,
            Some(added),
            None,
        )
        .await?;
    }

    let mut sorted: Vec<String> = current.into_iter().collect();
    sorted.sort();
    issue.labels = sorted;
    Ok(true)
}

/// Apply a reparent (`parent`): set/clear the `parent-child` dependency edge, cycle-checked.
///
/// A reparent is a genuine modification of the issue (FR-1b): on a **real** parent change it
/// advances `updated_at` (the caller stamps the row) and emits the same dependency audit events as
/// [`add_dependency`]/[`remove_dependency`] (`DependencyRemoved` for the dropped old parent edge,
/// `DependencyAdded` for the new parent edge), threading the real `actor` through. The events are
/// pushed into the caller's `events` vec so they commit transactionally in the single
/// `append_event_in_tx` loop after the row update.
///
/// Returns whether the parent **changed** — a reparent to the issue's current parent (or a detach
/// when there is no parent edge) is a no-op (returns `false`, pushes NO event, and so does not, on
/// its own, advance `updated_at`).
async fn apply_reparent(
    tx: &libsql::Transaction,
    id: &str,
    patch: &crate::filters::IssuePatch,
    actor: &str,
    events: &mut Vec<(unblock_model::EventType, Option<String>, Option<String>)>,
) -> Result<bool, StorageError> {
    use unblock_model::EventType;

    let Some(parent) = patch.parent.clone() else {
        return Ok(false);
    };

    // The current parent (the single `parent-child` edge declared by this issue, if any).
    let current_parent = existing_parent(tx, id).await?;
    if parent == current_parent {
        // No change: requested parent equals the current one (incl. detach when already parentless).
        return Ok(false);
    }

    // Remove any existing parent-child edge declared by this issue.
    tx.execute(
        "DELETE FROM dependencies WHERE issue_id = ?1 AND type = 'parent-child'",
        libsql::params![id],
    )
    .await
    .map_err(map_libsql_err)?;

    // Old parent edge dropped -> DependencyRemoved (old parent in `old_value`, mirroring
    // `remove_dependency`).
    if let Some(old_parent) = current_parent {
        events.push((EventType::DependencyRemoved, Some(old_parent), None));
    }

    if let Some(parent_id) = parent {
        if parent_id == id {
            return Err(StorageError::SelfDependency);
        }
        // Cycle check over the gating graph (a parent-child edge gates ready work). Reuses the same
        // `would_cycle_in_tx` as `add_dependency`, so the reparent cycle path is the REAL ordered
        // path naming every node — built from the SAME detecting graph, not a synthetic
        // `{id} -> {parent_id} -> … -> {id}` placeholder (D2/GATE-MUST-3, spine §3.2.1).
        if let Some(cycle) = super::deps::would_cycle_in_tx(
            tx,
            id,
            &parent_id,
            &unblock_model::DependencyType::ParentChild,
        )
        .await?
        {
            return Err(StorageError::CycleDetected {
                path: super::deps::render_cycle_path(&cycle),
            });
        }
        tx.execute(
            "INSERT INTO dependencies (issue_id, depends_on_id, type, created_at, created_by) \
             VALUES (?1, ?2, 'parent-child', ?3, ?4)",
            libsql::params![id, parent_id.as_str(), Utc::now().to_rfc3339(), actor],
        )
        .await
        .map_err(map_libsql_err)?;
        // New parent edge set -> DependencyAdded (new parent in `new_value`, mirroring
        // `add_dependency`).
        events.push((EventType::DependencyAdded, None, Some(parent_id)));
    }
    Ok(true)
}

/// The current parent id of `id` (its single `parent-child` `depends_on_id`), or `None` if it has no
/// parent edge.
async fn existing_parent(
    tx: &libsql::Transaction,
    id: &str,
) -> Result<Option<String>, StorageError> {
    let mut rows = tx
        .query(
            "SELECT depends_on_id FROM dependencies WHERE issue_id = ?1 AND type = 'parent-child' \
             LIMIT 1",
            libsql::params![id],
        )
        .await
        .map_err(map_libsql_err)?;
    match rows.next().await.map_err(map_libsql_err)? {
        Some(row) => match row.get_value(0).map_err(map_libsql_err)? {
            Value::Text(parent) => Ok(Some(parent)),
            _ => Ok(None),
        },
        None => Ok(None),
    }
}

/// Execute (or, for `DryRun`, plan) a delete (spine §3.2.1).
///
/// - `DryRun` resolves `cascade_children` and returns the plan, mutating nothing.
/// - `Tombstone` delegates to the model `Issue::into_tombstone`, bumps `updated_at` + recomputes the
///   hash, and writes `Event(Deleted)` **only** when the prior status was non-terminal.
/// - `Cascade` tombstones the targets and their resolved children.
/// - `Hard` permanently deletes the rows (and CASCADEs labels/deps/comments/events).
///
/// An already-tombstone target is a no-op (no event). Returns the resolved plan.
pub(super) async fn delete_issue(
    conn: &Connection,
    hook: WriteHook<'_>,
    plan: &DeletePlan,
    actor: &str,
) -> Result<DeletePlan, StorageError> {
    // Resolve cascade children for every mode (so a DryRun shows the full blast radius).
    let cascade_children = resolve_cascade_children(conn, &plan.targets).await?;
    let resolved = DeletePlan {
        mode: plan.mode,
        targets: plan.targets.clone(),
        cascade_children: cascade_children.clone(),
    };

    if plan.mode == DeleteMode::DryRun {
        return Ok(resolved);
    }

    let targets = plan.targets.clone();
    let mode = plan.mode;
    let actor = actor.to_string();

    with_immediate_tx(conn, hook, |tx| async move {
        let affected: Vec<String> = match mode {
            DeleteMode::Cascade => {
                let mut all = targets.clone();
                all.extend(cascade_children.iter().cloned());
                all
            }
            _ => targets.clone(),
        };

        for id in &affected {
            // `DryRun` already returned before this tx; `Hard` deletes rows, `Tombstone`/`Cascade`
            // soft-delete. The `else` covers `DryRun` without a panic (defensive — it is unreachable
            // by construction, but library code stays panic-free).
            if mode == DeleteMode::Hard {
                // The issues FK CASCADE removes labels/deps(issue_id)/comments/events/child rows;
                // dependencies referencing this id via depends_on_id (no FK) are cleaned explicitly.
                tx.execute(
                    "DELETE FROM dependencies WHERE depends_on_id = ?1",
                    libsql::params![id.as_str()],
                )
                .await
                .map_err(map_libsql_err)?;
                tx.execute(
                    "DELETE FROM issues WHERE id = ?1",
                    libsql::params![id.as_str()],
                )
                .await
                .map_err(map_libsql_err)?;
            } else if matches!(mode, DeleteMode::Tombstone | DeleteMode::Cascade) {
                tombstone_one(&tx, id, &actor).await?;
            }
        }
        Ok(((), tx))
    })
    .await?;

    Ok(resolved)
}

/// Tombstone one issue inside the tx: load it, delegate to the model `into_tombstone`, bump
/// `updated_at` + recompute the hash, and write `Event(Deleted)` only from a non-terminal status.
async fn tombstone_one(
    tx: &libsql::Transaction,
    id: &str,
    actor: &str,
) -> Result<(), StorageError> {
    let sql = format!("SELECT {ISSUE_COLUMNS} FROM issues WHERE id = ?1");
    let mut rows = tx
        .query(&sql, libsql::params![id])
        .await
        .map_err(map_libsql_err)?;
    let Some(row) = rows.next().await.map_err(map_libsql_err)? else {
        return Err(StorageError::IssueNotFound { id: id.to_string() });
    };
    let issue = issue_from_row(&row)?;

    // Already a tombstone → no-op (no event).
    if issue.is_tombstone() {
        return Ok(());
    }

    let was_terminal = issue.status.is_terminal();
    let now = Utc::now();
    let mut tomb = issue.into_tombstone(Some(actor.to_string()), None, now);
    tomb.updated_at = now;
    let new_hash = tomb.compute_content_hash();

    tx.execute(
        "UPDATE issues SET content_hash = ?1, status = 'tombstone', deleted_at = ?2, \
         deleted_by = ?3, delete_reason = ?4, original_type = ?5, updated_at = ?6 WHERE id = ?7",
        libsql::params![
            new_hash,
            tomb.deleted_at
                .map_or(Value::Null, |d| Value::Text(d.to_rfc3339())),
            tomb.deleted_by.clone().map_or(Value::Null, Value::Text),
            tomb.delete_reason
                .clone()
                .map_or(Value::Text(String::new()), Value::Text),
            tomb.original_type
                .clone()
                .map_or(Value::Text(String::new()), Value::Text),
            tomb.updated_at.to_rfc3339(),
            id,
        ],
    )
    .await
    .map_err(map_libsql_err)?;

    if !was_terminal {
        append_event_in_tx(
            tx,
            id,
            &unblock_model::EventType::Deleted,
            actor,
            None,
            None,
            Some("Deleted issue"),
        )
        .await?;
    }
    Ok(())
}

/// Restore (un-tombstone) a SOFT-deleted issue — the audited live inverse of `tombstone_one`
/// (FR-1c "recoverable", D20; spine §3.2.1). One `BEGIN IMMEDIATE` tx, TOCTOU-safe.
///
/// 1. Load the row inside the tx; missing → [`StorageError::IssueNotFound`] (this bounds restore to
///    SOFT deletes — a `Hard`-deleted row is gone).
/// 2. Not a tombstone (already active) → **idempotent no-op `Ok(issue)`**: no `UPDATE`, no event, no
///    `updated_at` bump (mirrors `tombstone_one`'s already-tombstone early `Ok`).
/// 3. Real tombstone → delegate to the model `Issue::restore_from_tombstone` (best-effort `status`
///    via `closed_at`; `issue_type` untouched; `original_type` + the tombstone fields cleared), bump
///    `updated_at`, recompute `content_hash`, and write a single `Event(Restored)` — **always** (a
///    restore is never a no-event case; never `StatusChanged`/`Reopened`, the §3.2.1 carve-out).
///
/// `closed_at` is bound from the restored value — NULL on the Open branch, the kept value on the
/// Closed branch — which is what satisfies the issues-table CHECK constraint on either side.
///
/// Re-reads via `get_issue` so labels/deps/comments hydrate. Since the row was just written, a
/// `None` re-read is an **internal invariant violation** → a `Backend` error, **never**
/// `IssueNotFound` (mirroring `claim_issue`'s in-tx holder re-`SELECT`).
pub(super) async fn restore_issue(
    conn: &Connection,
    hook: WriteHook<'_>,
    id: &str,
    actor: &str,
) -> Result<Issue, StorageError> {
    let id_owned = id.to_string();
    let actor = actor.to_string();

    with_immediate_tx(conn, hook, |tx| async move {
        // Load the current row inside the tx (TOCTOU-safe).
        let sql = format!("SELECT {ISSUE_COLUMNS} FROM issues WHERE id = ?1");
        let mut rows = tx
            .query(&sql, libsql::params![id_owned.as_str()])
            .await
            .map_err(map_libsql_err)?;
        let Some(row) = rows.next().await.map_err(map_libsql_err)? else {
            return Err(StorageError::IssueNotFound { id: id_owned });
        };
        let issue = issue_from_row(&row)?;
        drop(rows);

        // Already active → idempotent no-op (no UPDATE, no event, no updated_at bump).
        if !issue.is_tombstone() {
            return Ok((issue, tx));
        }

        let now = Utc::now();
        let mut restored = issue.restore_from_tombstone();
        restored.updated_at = now;
        let new_hash = restored.compute_content_hash();

        // `closed_at`: NULL for the Open branch, the kept value for the Closed branch — the CHECK
        // satisfier on both sides (restore_from_tombstone set it accordingly).
        tx.execute(
            "UPDATE issues SET status = ?1, original_type = '', deleted_at = NULL, \
             deleted_by = '', delete_reason = '', closed_at = ?2, updated_at = ?3, \
             content_hash = ?4 WHERE id = ?5",
            libsql::params![
                restored.status.as_str(),
                restored
                    .closed_at
                    .map_or(Value::Null, |d| Value::Text(d.to_rfc3339())),
                restored.updated_at.to_rfc3339(),
                new_hash,
                id_owned.as_str(),
            ],
        )
        .await
        .map_err(map_libsql_err)?;

        // A restore ALWAYS emits exactly one Restored event (contrast tombstone_one, which suppresses
        // the Deleted event on a terminal prior status).
        append_event_in_tx(
            &tx,
            &id_owned,
            &unblock_model::EventType::Restored,
            &actor,
            None,
            None,
            Some("Restored issue"),
        )
        .await?;

        Ok((restored, tx))
    })
    .await?;

    // Re-read with full hydration. The row was just written, so a `None` here is corruption — a
    // `Backend` invariant-violation error, NOT a caller-facing `IssueNotFound`.
    get_issue(conn, id).await?.ok_or_else(|| StorageError::Backend {
        source: crate::error::BackendOpaque::from_message(format!(
            "restore_issue: re-read of just-restored issue {id} returned no row (invariant violation)"
        )),
    })
}

/// Resolve the descendant ids of `targets` (hierarchical `{target}.*` ids) for the cascade plan.
async fn resolve_cascade_children(
    conn: &Connection,
    targets: &[String],
) -> Result<Vec<String>, StorageError> {
    let mut children = Vec::new();
    for target in targets {
        let pattern = format!("{}.%", super::ids::escape_like_pattern(target));
        let mut rows = conn
            .query(
                "SELECT id FROM issues WHERE id LIKE ?1 ESCAPE '\\' ORDER BY id ASC",
                libsql::params![pattern],
            )
            .await
            .map_err(map_libsql_err)?;
        while let Some(row) = rows.next().await.map_err(map_libsql_err)? {
            if let Value::Text(id) = row.get_value(0).map_err(map_libsql_err)? {
                children.push(id);
            }
        }
    }
    children.sort();
    children.dedup();
    Ok(children)
}

/// `?1, ?2, … ?n` placeholder list for an `INSERT … VALUES (...)`.
fn placeholders(n: usize) -> String {
    (1..=n)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether a 1-parameter existence query returns a row.
async fn row_exists(
    tx: &libsql::Transaction,
    sql: &str,
    param: &str,
) -> Result<bool, StorageError> {
    let mut rows = tx
        .query(sql, libsql::params![param])
        .await
        .map_err(map_libsql_err)?;
    Ok(rows.next().await.map_err(map_libsql_err)?.is_some())
}
