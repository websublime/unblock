//! Issue CRUD (crate plan §3.3, spine §3.2.1). Every mutation runs inside one `BEGIN IMMEDIATE`
//! transaction (rows + audit events commit together, FR-9).

use std::collections::HashSet;

use chrono::Utc;
use libsql::{Connection, Value, params_from_iter};

use unblock_model::{Issue, IssueValidator, parse_id};

use crate::error::{StorageError, map_libsql_err};
use crate::filters::{DeleteMode, DeletePlan};

use super::events::append_event_in_tx;
use super::ids::update_child_counter_in_tx;
use super::mappers::{ISSUE_COLUMNS, bind_issue, dependency_from_row, issue_from_row};
use super::{WriteHook, with_immediate_tx};

/// Create an issue: validate, guard against `id/external_ref` collisions, insert the row + relations,
/// write `Event(Created)` (+ per-relation events) — all in one tx. Returns the allocated id.
///
/// There is **no** content-hash dedup (spine §3.2.1): the hash is computed and stored, never used to
/// short-circuit. FR-26 import idempotency lives in `unblock-sync`, not here.
#[allow(clippy::too_many_lines)] // one cohesive transaction: row + labels + deps + comments + events
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
        // Guard 1: id collision.
        if row_exists(&tx, "SELECT 1 FROM issues WHERE id = ?1 LIMIT 1", &issue.id).await? {
            return Err(StorageError::IdCollision { id: issue.id });
        }

        // Guard 2: external_ref collision.
        if let Some(ext_ref) = issue.external_ref.as_deref()
            && row_exists(
                &tx,
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
        let params = bind_issue(&issue, &content_hash);
        tx.execute(&columns, params_from_iter(params))
            .await
            .map_err(map_libsql_err)?;

        // Maintain the child counter for a hierarchical id.
        if let Ok(parsed) = parse_id(&issue.id)
            && !parsed.is_root()
            && let (Some(parent), Some(&child)) = (parsed.parent(), parsed.child_path.last())
        {
            update_child_counter_in_tx(&tx, &parent, child).await?;
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
                &tx,
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
                &tx,
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
        for comment in &issue.comments {
            tx.execute(
                "INSERT INTO comments (issue_id, author, text, created_at) VALUES (?1, ?2, ?3, ?4)",
                libsql::params![
                    issue.id.as_str(),
                    comment.author.as_str(),
                    comment.body.as_str(),
                    comment.created_at.to_rfc3339(),
                ],
            )
            .await
            .map_err(map_libsql_err)?;
            append_event_in_tx(
                &tx,
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
            &tx,
            &issue.id,
            &unblock_model::EventType::Created,
            actor,
            None,
            None,
            Some(&format!("Created issue: {}", issue.title)),
        )
        .await?;

        let id = issue.id.clone();
        Ok((id, tx))
    })
    .await
}

/// Fetch a single issue (hydrated with labels + dependencies), or `None` if absent.
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

/// Fetch multiple issues by id (hydrated). Unknown ids are simply absent from the result.
pub(super) async fn get_issues(
    conn: &Connection,
    ids: &[String],
) -> Result<Vec<Issue>, StorageError> {
    let mut out = Vec::new();
    for id in ids {
        if let Some(issue) = get_issue(conn, id).await? {
            out.push(issue);
        }
    }
    Ok(out)
}

/// Hydrate an issue's `labels` (sorted) and `dependencies` from their tables.
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
    Ok(())
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
        let parent_changed = apply_reparent(&tx, &id_owned, &patch).await?;

        // Empty diff: nothing in the row, no relation change -> full skip (no updated_at, no event).
        if builder.is_empty() && !label_changed && !parent_changed {
            return Ok((issue, tx));
        }

        // A row column changed -> advance updated_at + recompute content_hash.
        if !builder.is_empty() {
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

/// Apply a reparent (`parent`): set/clear the `parent-child` dependency edge, cycle-checked. Returns
/// whether the parent **changed** — a reparent to the issue's current parent (or a detach when there
/// is no parent edge) is a no-op (returns `false`, so it does not, on its own, advance `updated_at`).
async fn apply_reparent(
    tx: &libsql::Transaction,
    id: &str,
    patch: &crate::filters::IssuePatch,
) -> Result<bool, StorageError> {
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

    if let Some(parent_id) = parent {
        if parent_id == id {
            return Err(StorageError::SelfDependency);
        }
        // Cycle check over the gating graph (a parent-child edge gates ready work).
        if super::deps::would_cycle_in_tx(tx, id, &parent_id).await? {
            return Err(StorageError::CycleDetected {
                path: format!("{id} -> {parent_id} -> … -> {id}"),
            });
        }
        tx.execute(
            "INSERT INTO dependencies (issue_id, depends_on_id, type, created_at, created_by) \
             VALUES (?1, ?2, 'parent-child', ?3, '')",
            libsql::params![id, parent_id.as_str(), Utc::now().to_rfc3339()],
        )
        .await
        .map_err(map_libsql_err)?;
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
