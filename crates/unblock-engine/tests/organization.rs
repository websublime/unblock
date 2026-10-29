//! FR-6 remainder through the engine (PRD §4 D56, spine §4.1 organization block): the label catalog,
//! the label rename, the epic rollup read and the explicit close of every eligible epic.
//!
//! Real libsql storage throughout (no mock, FR-9). The guard cells run over a FILE-backed store so
//! the D31 `.write.lock` is the genuine flock: a second handle on the same database, opened with a
//! short lock timeout, observes `DatabaseLocked` exactly while an organization mutation holds the
//! guard, and a concurrent engine writer waits on the in-process permit. The parking seam is the
//! `ParkedStorage` organization gate (`tests/common`), which parks the first `epic_child_rollup` /
//! `rename_label` call — both run INSIDE the guard on the mutation paths.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::parked::ParkedStorage;
use common::{dep, issue, session, session_over, t};
use unblock_engine::{
    CountGroupBy, DeleteMode, DeletePlan, DiagnosticKind, EngineError, ListFilters, Session,
    SessionConfig,
};
use unblock_error::{CodedError, ErrorCode, ModelError};
use unblock_model::{DependencyType, Issue, IssueType, Priority, Status};
use unblock_storage::{DEFAULT_WRITE_LOCK_TIMEOUT_MS, LibsqlStorage, Storage, StorageError};

// --------------------------------------------------------------------------------------------------
// Fixtures
// --------------------------------------------------------------------------------------------------

/// A minimal issue of `issue_type` at a fixed epoch.
fn typed(id: &str, issue_type: IssueType) -> Issue {
    Issue {
        issue_type,
        ..issue(id, Priority::MEDIUM, 1_000)
    }
}

/// Create an epic.
async fn epic(session: &Session, id: &str) {
    session
        .create(&typed(id, IssueType::Epic))
        .await
        .expect("create epic");
}

/// Create a task child of `parent` with `status` (a closed child carries `closed_at`).
async fn child(session: &Session, id: &str, parent: &str, status: Status) {
    let mut row = typed(id, IssueType::Task);
    if status == Status::Closed {
        row.closed_at = Some(t(2_000));
    }
    row.status = status;
    session.create(&row).await.expect("create child");
    link(session, id, parent).await;
}

/// Add the `parent-child` edge `child → parent`.
async fn link(session: &Session, child_id: &str, parent: &str) {
    session
        .add_dep(&dep(child_id, parent, DependencyType::ParentChild))
        .await
        .expect("parent-child edge");
}

/// Tombstone `id` through the engine soft-delete path.
async fn tombstone(session: &Session, id: &str) {
    session
        .delete(&DeletePlan {
            mode: DeleteMode::Tombstone,
            targets: vec![id.to_string()],
            cascade_children: Vec::new(),
        })
        .await
        .expect("tombstone");
}

/// Create an issue carrying `labels`.
async fn labelled(session: &Session, mut row: Issue, labels: &[&str]) {
    row.labels = labels.iter().map(ToString::to_string).collect();
    session.create(&row).await.expect("create labelled");
}

/// The `(event_type, old, new)` triples of `id`, oldest first.
async fn events(storage: &dyn Storage, id: &str) -> Vec<(String, Option<String>, Option<String>)> {
    storage
        .list_events(id)
        .await
        .expect("list_events")
        .into_iter()
        .map(|e| (e.event_type.as_str().to_string(), e.old_value, e.new_value))
        .collect()
}

/// `stats.epics_eligible` as a number.
async fn stats_epics_eligible(session: &Session) -> usize {
    session
        .diagnostics(DiagnosticKind::Stats, None)
        .await
        .expect("stats")
        .findings
        .into_iter()
        .find(|f| f.label == "epics_eligible")
        .expect("epics_eligible row")
        .detail
        .parse()
        .expect("a count")
}

/// The ids of an epic read.
async fn epic_ids(session: &Session, id: Option<&str>, eligible_only: bool) -> Vec<String> {
    session
        .epic_statuses(id, eligible_only)
        .await
        .expect("epic_statuses")
        .into_iter()
        .map(|s| s.epic.id)
        .collect()
}

/// D56 (j): `stats.epics_eligible` equals the number of eligible entries the single-pass epic list
/// returns — on every fixture that calls it.
async fn assert_stats_match_the_epic_list(session: &Session) {
    let listed = session
        .epic_statuses(None, true)
        .await
        .expect("eligible epic list")
        .len();
    assert_eq!(
        stats_epics_eligible(session).await,
        listed,
        "stats.epics_eligible == the single-pass eligible list"
    );
}

/// A FILE-backed workspace: the session over a pass-through [`ParkedStorage`] (re-opened behind an
/// armed gate by [`gated`]) and the database path (for a second handle).
struct FileWorkspace {
    session: Arc<Session>,
    parked: Arc<ParkedStorage>,
    db: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

async fn file_workspace() -> FileWorkspace {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("unblock.db");
    let inner = LibsqlStorage::open_local(&db, DEFAULT_WRITE_LOCK_TIMEOUT_MS)
        .await
        .expect("open_local");
    inner.migrate().await.expect("migrate");
    let parked = ParkedStorage::new_passthrough(Arc::new(inner));
    let storage: Arc<dyn Storage> = parked.clone();
    let session = Arc::new(session_over(storage, SessionConfig::default()).await);
    FileWorkspace {
        session,
        parked,
        db,
        _dir: dir,
    }
}

/// Re-open `ws`'s database behind a [`ParkedStorage`] built by `gate` (same file, fresh session).
async fn gated(
    ws: &FileWorkspace,
    gate: fn(Arc<dyn Storage>) -> Arc<ParkedStorage>,
) -> (Arc<Session>, Arc<ParkedStorage>) {
    let inner = LibsqlStorage::open_local(&ws.db, DEFAULT_WRITE_LOCK_TIMEOUT_MS)
        .await
        .expect("open_local");
    let parked = gate(Arc::new(inner));
    let storage: Arc<dyn Storage> = parked.clone();
    let session = Arc::new(session_over(storage, SessionConfig::default()).await);
    (session, parked)
}

/// Whether ANOTHER handle on `db` can take the `.write.lock` right now (50 ms timeout).
async fn write_lock_is_free(db: &std::path::Path) -> bool {
    let probe = LibsqlStorage::open_local(db, 50).await.expect("probe open");
    match probe.acquire_write_lock().await {
        Ok(guard) => {
            drop(guard);
            true
        }
        Err(StorageError::DatabaseLocked) => false,
        Err(other) => panic!("unexpected lock probe failure: {other:?}"),
    }
}

/// Assert, while an organization mutation is parked INSIDE its guard, that the `.write.lock` is held
/// and that a concurrent engine writer waits on the permit; then release and return the writer.
async fn assert_guard_held(
    session: &Arc<Session>,
    parked: &ParkedStorage,
    db: &std::path::Path,
) -> tokio::task::JoinHandle<unblock_engine::Result<String>> {
    parked.wait_until_parked().await;
    assert!(
        !write_lock_is_free(db).await,
        "the `.write.lock` is held for the whole operation"
    );
    let writer_session = session.clone();
    let writer = tokio::spawn(async move {
        writer_session
            .create(&typed("ub-writer", IssueType::Task))
            .await
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        !writer.is_finished(),
        "a concurrent writer waits for the write permit"
    );
    parked.release();
    writer
}

// --------------------------------------------------------------------------------------------------
// Label catalog (D56 (g))
// --------------------------------------------------------------------------------------------------

/// D56 (g) CATALOG: labels on open, in-progress, deferred, closed, template and ephemeral issues
/// are counted; a label carried only by tombstoned issues is absent; entries are byte-ordered; and
/// `count group_by: label` is UNCHANGED — it keeps default visibility (closed/deferred hidden).
#[tokio::test]
async fn organization_catalog_counts_every_live_status_and_leaves_count_unchanged() {
    let session = session().await;
    labelled(
        &session,
        typed("ub-open", IssueType::Task),
        &["shared", "Zeta"],
    )
    .await;
    let mut prog = typed("ub-prog", IssueType::Task);
    prog.status = Status::InProgress;
    labelled(&session, prog, &["shared"]).await;
    let mut deferred = typed("ub-def", IssueType::Task);
    deferred.status = Status::Deferred;
    deferred.defer_until = Some(t(4_000_000_000));
    labelled(&session, deferred, &["shared", "beta"]).await;
    let mut closed = typed("ub-closed", IssueType::Task);
    closed.status = Status::Closed;
    closed.closed_at = Some(t(2_000));
    labelled(&session, closed, &["shared"]).await;
    let mut template = typed("ub-tmpl", IssueType::Task);
    template.is_template = true;
    labelled(&session, template, &["shared"]).await;
    let mut ephemeral = typed("ub-eph", IssueType::Task);
    ephemeral.ephemeral = true;
    labelled(&session, ephemeral, &["shared"]).await;
    labelled(
        &session,
        typed("ub-dead", IssueType::Task),
        &["shared", "gone"],
    )
    .await;
    tombstone(&session, "ub-dead").await;

    let catalog: Vec<(String, usize)> = session
        .list_all_labels()
        .await
        .expect("catalog")
        .into_iter()
        .map(|row| (row.label, row.count))
        .collect();
    assert_eq!(
        catalog,
        vec![
            ("Zeta".to_string(), 1),
            ("beta".to_string(), 1),
            ("shared".to_string(), 6),
        ]
    );

    // The visibility-scoped count is unchanged: closed and deferred stay hidden there.
    let counts: Vec<(String, usize)> = session
        .count(&ListFilters::default(), Some(CountGroupBy::Label))
        .await
        .expect("count by label")
        .into_iter()
        .map(|b| (b.key, b.count))
        .collect();
    assert_eq!(
        counts,
        vec![("Zeta".to_string(), 1), ("shared".to_string(), 4)],
        "`count group_by: label` keeps default visibility"
    );
}

// --------------------------------------------------------------------------------------------------
// Label rename (D56 (a)–(f), (m))
// --------------------------------------------------------------------------------------------------

/// D56 (e) VALIDATION: an invalid `old_name` or `new_name` is `VALIDATION_FAILED` naming THAT field,
/// with zero writes — both fields aggregated into one error when both are bad.
#[tokio::test]
async fn organization_rename_validates_both_names_with_zero_writes() {
    let session = session().await;
    labelled(&session, typed("ub-1", IssueType::Task), &["good"]).await;
    let before = session.get("ub-1").await.unwrap().unwrap();
    let too_long = "x".repeat(51);

    for (old, new, expected) in [
        ("bad name", "fine", vec!["old_name"]),
        ("good", "", vec!["new_name"]),
        ("bad!", too_long.as_str(), vec!["old_name", "new_name"]),
    ] {
        let err = session
            .rename_label(old, new)
            .await
            .expect_err("invalid names are rejected");
        assert_eq!(err.code(), ErrorCode::ValidationFailed);
        let EngineError::Model {
            source: ModelError::ValidationFailed { fields },
        } = &err
        else {
            panic!("expected the ValidationFailed aggregate, got {err:?}");
        };
        let named: Vec<&str> = fields.iter().map(|f| f.field.as_str()).collect();
        assert_eq!(
            named, expected,
            "the failing field(s) for ({old:?}, {new:?})"
        );
    }

    let after = session.get("ub-1").await.unwrap().unwrap();
    assert_eq!(after, before, "zero writes");
}

/// D56 (d) NO-OP: `old_name == new_name` returns `affected_issues: 0` WITHOUT taking the write
/// permit (it completes while a parked write holds it), and a label no live issue carries returns
/// `0`; neither changes a row or writes an event.
#[tokio::test]
async fn organization_rename_same_name_takes_no_permit() {
    let inner = LibsqlStorage::open_in_memory().await.expect("open");
    inner.migrate().await.expect("migrate");
    let parked = ParkedStorage::new(Arc::new(inner));
    let storage: Arc<dyn Storage> = parked.clone();
    let session = Arc::new(session_over(storage.clone(), SessionConfig::default()).await);

    // Fixtures go in through the inner path BEFORE the create gate is consumed: the first
    // `create` parks, so seed with `create_issues`.
    let mut row = typed("ub-1", IssueType::Task);
    row.labels = vec!["same".to_string()];
    storage.create_issues(&[row], "tester").await.expect("seed");
    let events_before = events(storage.as_ref(), "ub-1").await;

    let writer_session = session.clone();
    let writer = tokio::spawn(async move {
        writer_session
            .create(&typed("ub-parked", IssueType::Task))
            .await
    });
    parked.wait_until_parked().await;

    let rename = tokio::time::timeout(Duration::from_secs(2), session.rename_label("same", "same"))
        .await
        .expect("old == new must not wait for the write permit")
        .expect("no-op rename");
    assert_eq!(rename.affected_issues, 0);
    assert_eq!(rename.old_name, "same");
    assert_eq!(rename.new_name, "same");

    parked.release();
    writer.await.expect("join").expect("parked write completes");

    let missing = session.rename_label("absent", "new").await.expect("rename");
    assert_eq!(missing.affected_issues, 0);
    assert_eq!(events(storage.as_ref(), "ub-1").await, events_before);
    assert_eq!(
        session.get("ub-1").await.unwrap().unwrap().labels,
        vec!["same".to_string()]
    );
}

/// D56 (m) LOCKING + (a)/(b): a rename holds the write permit AND the `.write.lock` for the whole
/// operation, and returns the merged + renamed count.
#[tokio::test]
async fn organization_rename_holds_permit_and_lock() {
    let ws = file_workspace().await;
    labelled(&ws.session, typed("ub-1", IssueType::Task), &["old"]).await;
    labelled(&ws.session, typed("ub-2", IssueType::Task), &["new", "old"]).await;

    let (session, parked) = gated(&ws, ParkedStorage::new_gated_organization).await;
    let renamer = tokio::spawn({
        let session = session.clone();
        async move { session.rename_label("old", "new").await }
    });
    let writer = assert_guard_held(&session, &parked, &ws.db).await;

    let rename = renamer.await.expect("join").expect("rename");
    assert_eq!(rename.affected_issues, 2, "merged and renamed both counted");
    writer
        .await
        .expect("join")
        .expect("the waiting writer completes");
    assert!(write_lock_is_free(&ws.db).await, "the guard is released");
    for id in ["ub-1", "ub-2"] {
        assert_eq!(
            session.get(id).await.unwrap().unwrap().labels,
            vec!["new".to_string()]
        );
    }
}

// --------------------------------------------------------------------------------------------------
// Epic rollup read (D56 (h), (i))
// --------------------------------------------------------------------------------------------------

/// D56 (h) EPIC LIST: without `id` the read returns EXACTLY the non-closed, non-tombstoned,
/// non-template epics in id order — deferred, blocked-status and ephemeral epics included, a
/// childless epic included as ineligible — each with its full `Issue`; `eligible_only` keeps
/// exactly the eligible ones.
#[tokio::test]
async fn organization_epic_list_returns_open_non_template_epics_in_id_order() {
    let session = session().await;
    // ub-e1: deferred, eligible.
    let mut e1 = typed("ub-e1", IssueType::Epic);
    e1.status = Status::Deferred;
    e1.defer_until = Some(t(4_000_000_000));
    session.create(&e1).await.unwrap();
    child(&session, "ub-e1c", "ub-e1", Status::Closed).await;
    // ub-e2: ephemeral, eligible.
    let mut e2 = typed("ub-e2", IssueType::Epic);
    e2.ephemeral = true;
    session.create(&e2).await.unwrap();
    child(&session, "ub-e2c", "ub-e2", Status::Closed).await;
    // ub-e3: open, no child → listed, ineligible.
    epic(&session, "ub-e3").await;
    // ub-e4: manual `blocked` status with an open child → listed, ineligible.
    let mut e4 = typed("ub-e4", IssueType::Epic);
    e4.status = Status::Blocked;
    session.create(&e4).await.unwrap();
    child(&session, "ub-e4c", "ub-e4", Status::Open).await;
    // Omitted: a closed epic, a template epic, a tombstoned epic, a task.
    let mut closed = typed("ub-x1", IssueType::Epic);
    closed.status = Status::Closed;
    closed.closed_at = Some(t(2_000));
    session.create(&closed).await.unwrap();
    let mut template = typed("ub-x2", IssueType::Epic);
    template.is_template = true;
    session.create(&template).await.unwrap();
    epic(&session, "ub-x3").await;
    tombstone(&session, "ub-x3").await;
    session
        .create(&typed("ub-x4", IssueType::Task))
        .await
        .unwrap();

    let statuses = session.epic_statuses(None, false).await.expect("list");
    let summary: Vec<(&str, usize, usize, bool)> = statuses
        .iter()
        .map(|s| {
            (
                s.epic.id.as_str(),
                s.total_children,
                s.closed_children,
                s.eligible_for_close,
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            ("ub-e1", 1, 1, true),
            ("ub-e2", 1, 1, true),
            ("ub-e3", 0, 0, false),
            ("ub-e4", 1, 0, false),
        ]
    );
    for status in &statuses {
        let full = session.get(&status.epic.id).await.unwrap().unwrap();
        assert_eq!(status.epic, full, "the FULL hydrated issue is embedded");
    }
    assert_eq!(
        epic_ids(&session, None, true).await,
        vec!["ub-e1".to_string(), "ub-e2".to_string()]
    );
    assert_stats_match_the_epic_list(&session).await;
}

/// D56 (i) EPIC BY ID: a closed epic AND a template epic are each read as a one-element list with
/// `eligible_for_close: false` even when all their children are closed; `eligible_only` on such an
/// epic returns `[]`; an eligible open epic reads `true`.
#[tokio::test]
async fn organization_epic_by_id_reaches_closed_and_template_epics() {
    let session = session().await;
    let mut closed = typed("ub-closed", IssueType::Epic);
    closed.status = Status::Closed;
    closed.closed_at = Some(t(2_000));
    session.create(&closed).await.unwrap();
    child(&session, "ub-c1", "ub-closed", Status::Closed).await;
    let mut template = typed("ub-tmpl", IssueType::Epic);
    template.is_template = true;
    session.create(&template).await.unwrap();
    child(&session, "ub-c2", "ub-tmpl", Status::Closed).await;
    epic(&session, "ub-open").await;
    child(&session, "ub-c3", "ub-open", Status::Closed).await;

    for id in ["ub-closed", "ub-tmpl"] {
        let read = session.epic_statuses(Some(id), false).await.expect("by id");
        assert_eq!(read.len(), 1, "{id}: one-element list");
        assert_eq!(read[0].epic.id, id);
        assert_eq!((read[0].total_children, read[0].closed_children), (1, 1));
        assert!(
            !read[0].eligible_for_close,
            "{id}: excluded by the predicate"
        );
        assert!(
            session
                .epic_statuses(Some(id), true)
                .await
                .unwrap()
                .is_empty(),
            "{id}: eligible_only on an ineligible epic is []"
        );
    }
    let open = session.epic_statuses(Some("ub-open"), true).await.unwrap();
    assert_eq!(open.len(), 1);
    assert!(open[0].eligible_for_close);
    // The closed and template epics are excluded from both the list and `stats`.
    assert_stats_match_the_epic_list(&session).await;
}

/// D56 (i): an unknown or TOMBSTONED id is `ISSUE_NOT_FOUND`; an id naming another type is
/// `VALIDATION_FAILED` with one `FieldError` on `id` reading `issue <id> is a <type>, not an epic`.
#[tokio::test]
async fn organization_epic_by_id_rejects_unknown_tombstoned_and_non_epic() {
    let session = session().await;
    epic(&session, "ub-dead").await;
    tombstone(&session, "ub-dead").await;
    session
        .create(&typed("ub-task", IssueType::Task))
        .await
        .unwrap();

    for id in ["ub-nope", "ub-dead"] {
        let err = session.epic_statuses(Some(id), false).await.unwrap_err();
        assert_eq!(err.code(), ErrorCode::IssueNotFound, "{id}");
    }
    let err = session
        .epic_statuses(Some("ub-task"), false)
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ValidationFailed);
    let EngineError::Model {
        source: ModelError::ValidationFailed { fields },
    } = &err
    else {
        panic!("expected ValidationFailed, got {err:?}");
    };
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].field, "id");
    assert_eq!(fields[0].reason, "issue ub-task is a task, not an epic");
}

/// D56 (j) ONE PREDICATE: `closed_children` counts closed AND tombstoned non-template children;
/// template children count nowhere; an epic whose only child is a template is ineligible; an epic
/// whose children are all tombstoned is eligible; an eligible epic with an open `blocks` blocker of
/// its own stays eligible (and blocked); and `stats.epics_eligible` equals the eligible entries.
#[tokio::test]
async fn organization_one_predicate_matches_stats_epics_eligible() {
    let session = session().await;
    // ub-a: one closed + one tombstoned child + an OPEN template child → eligible, (2, 2).
    epic(&session, "ub-a").await;
    child(&session, "ub-a1", "ub-a", Status::Closed).await;
    child(&session, "ub-a2", "ub-a", Status::Open).await;
    tombstone(&session, "ub-a2").await;
    let mut tmpl = typed("ub-a3", IssueType::Task);
    tmpl.is_template = true;
    session.create(&tmpl).await.unwrap();
    link(&session, "ub-a3", "ub-a").await;
    // ub-b: only a (closed) template child → ineligible, (0, 0).
    epic(&session, "ub-b").await;
    let mut tmpl = typed("ub-b1", IssueType::Task);
    tmpl.is_template = true;
    tmpl.status = Status::Closed;
    tmpl.closed_at = Some(t(2_000));
    session.create(&tmpl).await.unwrap();
    link(&session, "ub-b1", "ub-b").await;
    // ub-c: every child tombstoned → eligible.
    epic(&session, "ub-c").await;
    child(&session, "ub-c1", "ub-c", Status::Open).await;
    tombstone(&session, "ub-c1").await;
    // ub-d: eligible, but blocked by an open blocker of its own.
    epic(&session, "ub-d").await;
    child(&session, "ub-d1", "ub-d", Status::Closed).await;
    session
        .create(&typed("ub-blocker", IssueType::Task))
        .await
        .unwrap();
    session
        .add_dep(&dep("ub-d", "ub-blocker", DependencyType::Blocks))
        .await
        .unwrap();

    let statuses = session.epic_statuses(None, false).await.unwrap();
    let summary: Vec<(&str, usize, usize, bool)> = statuses
        .iter()
        .map(|s| {
            (
                s.epic.id.as_str(),
                s.total_children,
                s.closed_children,
                s.eligible_for_close,
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            ("ub-a", 2, 2, true),
            ("ub-b", 0, 0, false),
            ("ub-c", 1, 1, true),
            ("ub-d", 1, 1, true),
        ]
    );
    let blocked: Vec<String> = session
        .blocked(&ListFilters::default())
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert!(
        blocked.contains(&"ub-d".to_string()),
        "eligible AND blocked by its own blocker"
    );
    let eligible = statuses.iter().filter(|s| s.eligible_for_close).count();
    assert_eq!(stats_epics_eligible(&session).await, eligible);
    assert_stats_match_the_epic_list(&session).await;
    assert_eq!(eligible, 3);
}

// --------------------------------------------------------------------------------------------------
// close_eligible (D56 (k), (l), (m), (o))
// --------------------------------------------------------------------------------------------------

/// The A → B → leaf fixture: epic A's only child is epic B, B's only child is a closed leaf.
async fn cascade_fixture(session: &Session) {
    epic(session, "ub-a").await;
    epic(session, "ub-b").await;
    link(session, "ub-b", "ub-a").await;
    child(session, "ub-leaf", "ub-b", Status::Closed).await;
    // An ineligible bystander: an epic with an open child.
    epic(session, "ub-z").await;
    child(session, "ub-z1", "ub-z", Status::Open).await;
}

/// D56 (k) DRY RUN: `close_eligible{dry_run:true}` holds the write permit AND the `.write.lock`
/// (a concurrent writer waits), changes no row, writes no event and opens no transaction, and
/// returns the cascade set in close order — exactly the ids a real run on the same state closes.
#[tokio::test]
async fn organization_dry_run_holds_permit_and_lock_and_writes_nothing() {
    let ws = file_workspace().await;
    cascade_fixture(&ws.session).await;
    let storage: Arc<dyn Storage> = ws.parked.clone();
    let mut before = Vec::new();
    for id in ["ub-a", "ub-b", "ub-z"] {
        before.push((
            ws.session.get(id).await.unwrap().unwrap(),
            events(storage.as_ref(), id).await,
        ));
    }

    let (session, parked) = gated(&ws, ParkedStorage::new_gated_organization).await;
    let dry = tokio::spawn({
        let session = session.clone();
        async move { session.close_eligible(true).await }
    });
    let writer = assert_guard_held(&session, &parked, &ws.db).await;
    let outcome = dry.await.expect("join").expect("dry run");
    writer.await.expect("join").expect("writer completes");

    assert_eq!(
        outcome.epic_ids,
        vec!["ub-b".to_string(), "ub-a".to_string()]
    );
    assert_eq!(outcome.count, 2);
    assert!(outcome.dry_run);
    assert_eq!(
        parked.close_calls(),
        0,
        "a dry run opens no close transaction"
    );
    for (row, row_events) in before {
        assert_eq!(ws.session.get(&row.id).await.unwrap().unwrap(), row);
        assert_eq!(events(storage.as_ref(), &row.id).await, row_events);
    }

    let real = ws.session.close_eligible(false).await.expect("real run");
    assert_eq!(
        real.epic_ids, outcome.epic_ids,
        "the dry run previews the real close"
    );
    assert!(!real.dry_run);
}

/// D56 (o) CASCADE: with A → B → closed leaf, the single-pass reads see only B (`query epics
/// eligible_only` returns `[B]`, `stats.epics_eligible` is 1), the dry run returns `[B, A]`, and ONE
/// real run closes `[B, A]` in ONE `close_issues` call (one transaction), child before parent, each
/// with `StatusChanged` then `Closed`. Two independent eligible epics in one round come out in id
/// order, after which the round-2 parent follows.
#[tokio::test]
async fn organization_cascade_closes_child_before_parent_in_one_run() {
    let inner = LibsqlStorage::open_in_memory().await.expect("open");
    inner.migrate().await.expect("migrate");
    let parked = ParkedStorage::new_passthrough(Arc::new(inner));
    let storage: Arc<dyn Storage> = parked.clone();
    let session = session_over(storage.clone(), SessionConfig::default()).await;
    cascade_fixture(&session).await;

    assert_eq!(
        epic_ids(&session, None, true).await,
        vec!["ub-b".to_string()]
    );
    assert_eq!(stats_epics_eligible(&session).await, 1, "single-pass stats");
    assert_stats_match_the_epic_list(&session).await;
    assert_eq!(
        session.close_eligible(true).await.unwrap().epic_ids,
        vec!["ub-b".to_string(), "ub-a".to_string()]
    );

    let seen_a = events(storage.as_ref(), "ub-a").await.len();
    let seen_b = events(storage.as_ref(), "ub-b").await.len();
    let outcome = session.close_eligible(false).await.expect("real run");
    assert_eq!(
        outcome.epic_ids,
        vec!["ub-b".to_string(), "ub-a".to_string()]
    );
    assert_eq!(outcome.count, 2);
    assert_eq!(
        parked.close_calls(),
        1,
        "the whole cascade is ONE close transaction"
    );
    for (id, seen) in [("ub-a", seen_a), ("ub-b", seen_b)] {
        assert_eq!(
            session.get(id).await.unwrap().unwrap().status,
            Status::Closed
        );
        let tail: Vec<String> = events(storage.as_ref(), id)
            .await
            .into_iter()
            .skip(seen)
            .map(|(kind, _, _)| kind)
            .collect();
        assert_eq!(tail, vec!["status_changed", "closed"], "{id}");
    }
    assert_eq!(
        session.get("ub-z").await.unwrap().unwrap().status,
        Status::Open,
        "the ineligible bystander is untouched"
    );

    // Two independent eligible epics share round 1 (id order); their parent closes in round 2.
    let session = common::session().await;
    epic(&session, "ub-p").await;
    epic(&session, "ub-y").await;
    epic(&session, "ub-x").await;
    link(&session, "ub-y", "ub-p").await;
    link(&session, "ub-x", "ub-p").await;
    child(&session, "ub-y1", "ub-y", Status::Closed).await;
    child(&session, "ub-x1", "ub-x", Status::Closed).await;
    assert_eq!(
        session.close_eligible(false).await.unwrap().epic_ids,
        vec!["ub-x".to_string(), "ub-y".to_string(), "ub-p".to_string()]
    );
}

/// D56 (l) REAL CLOSE + (m) LOCKING: the real run still holds the permit and the `.write.lock`
/// while `close_issues` runs (the gate parks INSIDE the close call, after the snapshot read — the
/// dry-run cell covers the read); every closed epic ends `closed` with `closed_at`, the reason "All
/// children completed" and an advanced `updated_at`; ineligible epics are untouched; the output
/// lists the closed ids in close order with `dry_run: false`.
#[tokio::test]
async fn organization_real_close_holds_permit_and_lock() {
    let ws = file_workspace().await;
    cascade_fixture(&ws.session).await;
    let z_before = ws.session.get("ub-z").await.unwrap().unwrap();

    let (session, parked) = gated(&ws, ParkedStorage::new_gated_close).await;
    let closer = tokio::spawn({
        let session = session.clone();
        async move { session.close_eligible(false).await }
    });
    let writer = assert_guard_held(&session, &parked, &ws.db).await;
    let outcome = closer.await.expect("join").expect("real run");
    writer.await.expect("join").expect("writer completes");

    assert_eq!(
        outcome.epic_ids,
        vec!["ub-b".to_string(), "ub-a".to_string()]
    );
    assert_eq!(outcome.count, 2);
    assert!(!outcome.dry_run);
    assert_eq!(parked.close_calls(), 1);
    for id in ["ub-a", "ub-b"] {
        let row = session.get(id).await.unwrap().unwrap();
        assert_eq!(row.status, Status::Closed, "{id}");
        assert!(row.closed_at.is_some(), "{id}");
        assert_eq!(row.close_reason.as_deref(), Some("All children completed"));
        assert!(row.updated_at > t(1_000), "{id}: updated_at advanced");
    }
    assert_eq!(session.get("ub-z").await.unwrap().unwrap(), z_before);
}

/// D56 (l): with nothing eligible a real run returns `{"epic_ids":[],"count":0,"dry_run":false}`
/// and opens no transaction.
#[tokio::test]
async fn organization_close_eligible_nothing_eligible_is_empty() {
    let inner = LibsqlStorage::open_in_memory().await.expect("open");
    inner.migrate().await.expect("migrate");
    let parked = ParkedStorage::new_passthrough(Arc::new(inner));
    let storage: Arc<dyn Storage> = parked.clone();
    let session = session_over(storage, SessionConfig::default()).await;
    epic(&session, "ub-e").await;
    child(&session, "ub-e1", "ub-e", Status::Open).await;

    let outcome = session.close_eligible(false).await.expect("run");
    assert!(outcome.epic_ids.is_empty());
    assert_eq!(outcome.count, 0);
    assert!(!outcome.dry_run);
    assert_eq!(parked.close_calls(), 0, "no transaction opens");
    assert_eq!(
        serde_json::to_value(&outcome).unwrap(),
        serde_json::json!({"epic_ids": [], "count": 0, "dry_run": false})
    );
}

/// D56 (m): `query labels` and `query epics` are READS — they complete while a parked write holds
/// the write permit, and while another handle holds the `.write.lock`.
#[tokio::test]
async fn organization_reads_take_no_permit() {
    let ws = file_workspace().await;
    labelled(&ws.session, typed("ub-1", IssueType::Task), &["l"]).await;
    epic(&ws.session, "ub-e").await;

    // Another handle holds the `.write.lock`.
    let holder = LibsqlStorage::open_local(&ws.db, DEFAULT_WRITE_LOCK_TIMEOUT_MS)
        .await
        .expect("holder");
    let lock = holder.acquire_write_lock().await.expect("lock");
    assert!(lock.is_some(), "a file-backed store takes a real lock");

    // And a parked write holds the in-process permit.
    let inner = LibsqlStorage::open_local(&ws.db, DEFAULT_WRITE_LOCK_TIMEOUT_MS)
        .await
        .expect("open");
    let parked = ParkedStorage::new(Arc::new(inner));
    let storage: Arc<dyn Storage> = parked.clone();
    let session = Arc::new(session_over(storage, SessionConfig::default()).await);
    // The parked create blocks on the `.write.lock` held above, which it reaches only AFTER taking
    // the permit — so while it waits, the permit is held.
    let writer = tokio::spawn({
        let session = session.clone();
        async move { session.create(&typed("ub-w", IssueType::Task)).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !writer.is_finished(),
        "the writer holds the permit, waiting on the lock"
    );

    let labels = tokio::time::timeout(Duration::from_secs(2), session.list_all_labels())
        .await
        .expect("query labels never waits on the permit or the lock")
        .expect("labels");
    assert_eq!(labels.len(), 1);
    let epics = tokio::time::timeout(Duration::from_secs(2), session.epic_statuses(None, false))
        .await
        .expect("query epics never waits on the permit or the lock")
        .expect("epics");
    assert_eq!(epics.len(), 1);

    drop(lock);
    parked.wait_until_parked().await;
    parked.release();
    writer.await.expect("join").expect("writer completes");
}
