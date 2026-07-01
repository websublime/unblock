//! `Session::create_bulk` (D22/T2.3) — the ATOMIC bulk MINTING create path, over a real in-memory
//! libsql `Session` (NOT a mock; the engine's contract is "identical behaviour through one path",
//! FR-9).
//!
//! Covers (per the engine crate plan T2.3 AC): a clean N-record batch with an intra-batch dep edge;
//! the in-batch per-parent child-counter (two same-parent records mint DISTINCT `parent.1`/`parent.2`
//! — MF-3 non-vacuity, would collide under a naive `next_child_number`-only mint); topological mint
//! order (a child placed BEFORE its parent in file order still mints); the whole-batch rejection set
//! (parent cycle, self-dependency, self-parent, ambiguous, unresolved, marker-only) each →
//! `ValidationFailed` + ZERO writes; the `blocked-by`→`blocks` alias flip at the edge step; the D22
//! create-surface field fidelity on `create_issue`; the fault-injection rollback (an out-of-band
//! racer → whole-batch rollback → ZERO persisted); and that single `create_issue`/`create(&Issue)`
//! are unchanged.

mod common;

use std::sync::Arc;

use unblock_engine::{EngineError, NewIssue, Session, SessionConfig};
use unblock_error::{CodedError, ErrorCode};
use unblock_model::{DependencyType, ListFilters, parse_id};
use unblock_storage::{LibsqlStorage, Storage};

use common::race::RaceInjector;
use common::session;

/// A bare `NewIssue` with just a title (bulk-shape; carriers default empty).
fn bulk_record(title: &str) -> NewIssue {
    NewIssue {
        title: title.to_string(),
        ..NewIssue::default()
    }
}

/// Count all issues (active + closed + deferred) via the engine list read.
async fn count_all(session: &Session) -> usize {
    let filters = ListFilters {
        include_closed: true,
        include_deferred: true,
        ..ListFilters::default()
    };
    session.list(&filters).await.expect("list").len()
}

// --------------------------------------------------------------------------------------------------
// Happy path
// --------------------------------------------------------------------------------------------------

/// A clean N-record batch round-trips all N created issues, in FILE order, with each minting a
/// parseable, MUTUALLY-DISTINCT root id.
///
/// Timing-independent by construction: the three records carry DISTINCT titles, so each feeds a
/// different id seed even though the whole batch shares one `created_at` instant (the mint stamps all
/// records with one `now`). The minted root ids are therefore deterministic for the batch, and the
/// explicit distinct-id check below proves the in-batch dedup never returns a colliding id.
#[tokio::test]
async fn clean_batch_round_trips_all_records_in_file_order() {
    let s = session().await;
    let created = s
        .create_bulk(vec![
            bulk_record("alpha"),
            bulk_record("beta"),
            bulk_record("gamma"),
        ])
        .await
        .expect("bulk create");

    assert_eq!(created.len(), 3);
    assert_eq!(created[0].title, "alpha");
    assert_eq!(created[1].title, "beta");
    assert_eq!(created[2].title, "gamma");
    let mut ids = std::collections::HashSet::new();
    for issue in &created {
        assert!(parse_id(&issue.id).expect("parses").is_root());
        assert!(
            ids.insert(issue.id.clone()),
            "every batch record mints a distinct root id (got a duplicate: {})",
            issue.id
        );
    }
    assert_eq!(count_all(&s).await, 3);
}

/// An intra-batch dep edge by TITLE (record B's `dep_refs` references record A's title) resolves to
/// A's minted id and persists.
#[tokio::test]
async fn intra_batch_title_dep_resolves_and_persists() {
    let s = session().await;
    let mut b = bulk_record("Build API");
    b.dep_refs = vec!["Build Database Schema".to_string()];
    let created = s
        .create_bulk(vec![bulk_record("Build Database Schema"), b])
        .await
        .expect("bulk create");

    let a_id = created[0].id.clone();
    let b_id = created[1].id.clone();
    let deps = s.list_dependencies(&b_id).await.expect("deps");
    assert_eq!(deps.len(), 1);
    assert_eq!(
        deps[0].depends_on_id, a_id,
        "title ref resolved to A's minted id"
    );
    assert_eq!(deps[0].dep_type, DependencyType::Blocks);
}

/// An intra-batch dep edge by STAND-IN id (`### ID` handle) resolves.
#[tokio::test]
async fn intra_batch_standin_dep_resolves() {
    let s = session().await;
    let mut a = bulk_record("Build Database Schema");
    a.stand_in_id = Some("db-1".to_string());
    let mut b = bulk_record("Build API");
    b.dep_refs = vec!["db-1".to_string()];
    let created = s.create_bulk(vec![a, b]).await.expect("bulk create");

    let deps = s.list_dependencies(&created[1].id).await.expect("deps");
    assert_eq!(deps.len(), 1);
    assert_eq!(deps[0].depends_on_id, created[0].id);
}

// --------------------------------------------------------------------------------------------------
// In-batch per-parent child counter (MF-3 — non-vacuous)
// --------------------------------------------------------------------------------------------------

/// TWO records with the SAME intra-batch parent (each `### Parent` → the same stand-in) mint DISTINCT
/// `parent.1` / `parent.2`. Non-vacuous: with a naive per-sibling `next_child_number`-only mint (which
/// reads only committed state) BOTH would mint the same `parent.N` → an in-tx `IdCollision` →
/// whole-batch rollback (the bulk would be UNBUILDABLE for any 2+ same-parent children).
#[tokio::test]
async fn two_same_parent_children_mint_distinct_child_numbers() {
    let s = session().await;
    let mut parent = bulk_record("Epic");
    parent.stand_in_id = Some("epic".to_string());
    let mut c1 = bulk_record("Child One");
    c1.parent = Some("epic".to_string());
    let mut c2 = bulk_record("Child Two");
    c2.parent = Some("epic".to_string());

    let created = s
        .create_bulk(vec![parent, c1, c2])
        .await
        .expect("bulk create with two same-parent children");

    let parent_id = created[0].id.clone();
    let c1_id = &created[1].id;
    let c2_id = &created[2].id;
    assert_eq!(*c1_id, format!("{parent_id}.1"));
    assert_eq!(*c2_id, format!("{parent_id}.2"));
    assert_ne!(c1_id, c2_id, "siblings mint distinct child numbers");
}

/// Topological mint order: a child placed BEFORE its intra-batch parent in FILE order still mints
/// `parent.N` correctly (the parent's id is minted first).
#[tokio::test]
async fn child_before_parent_in_file_order_still_mints() {
    let s = session().await;
    let mut child = bulk_record("Child");
    child.parent = Some("the-parent".to_string());
    let mut parent = bulk_record("Parent");
    parent.stand_in_id = Some("the-parent".to_string());

    // Child FIRST in file order; the parent is the 2nd record.
    let created = s
        .create_bulk(vec![child, parent])
        .await
        .expect("bulk create child-before-parent");

    // The response is FILE order: [child, parent].
    let child_id = &created[0].id;
    let parent_id = &created[1].id;
    assert_eq!(
        *child_id,
        format!("{parent_id}.1"),
        "child minted under its parent"
    );
}

// --------------------------------------------------------------------------------------------------
// Whole-batch rejection set (ValidationFailed + ZERO writes)
// --------------------------------------------------------------------------------------------------

/// Assert a `create_bulk` rejects the WHOLE batch with `ValidationFailed` and persists ZERO rows.
async fn assert_rejected_zero_writes(records: Vec<NewIssue>) {
    let s = session().await;
    let err = s.create_bulk(records).await.expect_err("must reject");
    assert_eq!(
        err.code(),
        ErrorCode::ValidationFailed,
        "the whole batch is rejected as ValidationFailed",
    );
    assert_eq!(count_all(&s).await, 0, "ZERO writes on a rejected batch");
}

#[tokio::test]
async fn rejects_parent_cycle() {
    let mut a = bulk_record("A");
    a.stand_in_id = Some("a".to_string());
    a.parent = Some("b".to_string());
    let mut b = bulk_record("B");
    b.stand_in_id = Some("b".to_string());
    b.parent = Some("a".to_string());
    assert_rejected_zero_writes(vec![a, b]).await;
}

#[tokio::test]
async fn rejects_self_parent() {
    let mut a = bulk_record("A");
    a.stand_in_id = Some("a".to_string());
    a.parent = Some("a".to_string());
    assert_rejected_zero_writes(vec![a]).await;
}

#[tokio::test]
async fn rejects_self_dependency() {
    let mut a = bulk_record("A");
    a.stand_in_id = Some("a".to_string());
    a.dep_refs = vec!["a".to_string()]; // depends on its own stand-in
    assert_rejected_zero_writes(vec![a]).await;
}

#[tokio::test]
async fn rejects_ambiguous_reference() {
    // Two records share a title; a third references it → ambiguous.
    let mut c = bulk_record("Consumer");
    c.dep_refs = vec!["Shared".to_string()];
    assert_rejected_zero_writes(vec![bulk_record("Shared"), bulk_record("Shared"), c]).await;
}

#[tokio::test]
async fn rejects_unresolved_reference() {
    let mut a = bulk_record("A");
    a.dep_refs = vec!["ub-does-not-exist".to_string()];
    assert_rejected_zero_writes(vec![a]).await;
}

#[tokio::test]
async fn rejects_marker_only_reference() {
    // A marker-only token that survives to resolution (most are dropped by the parser; the engine is
    // the backstop). A bare `-` is a marker-only dep id.
    let mut a = bulk_record("A");
    a.dep_refs = vec!["-".to_string()];
    assert_rejected_zero_writes(vec![a]).await;
}

// --------------------------------------------------------------------------------------------------
// blocked-by alias flip (the engine edge step, NOT the parser)
// --------------------------------------------------------------------------------------------------

/// A `blocked-by:<ref>` dep resolves to a `blocks` edge at the engine resolution step (the built
/// edge's `dep_type` is `Blocks`; the parser kept the verbatim `blocked-by` type string).
#[tokio::test]
async fn blocked_by_alias_flips_to_blocks_at_edge_build() {
    let s = session().await;
    let mut a = bulk_record("Blocker");
    a.stand_in_id = Some("blk".to_string());
    let mut b = bulk_record("Blocked");
    b.dep_refs = vec!["blocked-by:blk".to_string()];
    let created = s.create_bulk(vec![a, b]).await.expect("bulk create");

    let deps = s.list_dependencies(&created[1].id).await.expect("deps");
    assert_eq!(deps.len(), 1);
    assert_eq!(deps[0].depends_on_id, created[0].id);
    assert_eq!(
        deps[0].dep_type,
        DependencyType::Blocks,
        "blocked-by flips to blocks at the engine edge-build step",
    );
}

// --------------------------------------------------------------------------------------------------
// D22 create-surface field fidelity (single create_issue)
// --------------------------------------------------------------------------------------------------

/// A `create_issue` carrying the 4 D22 markdown-captured fields round-trips them onto the built
/// `Issue`; a `show` of the new id surfaces them.
#[tokio::test]
async fn create_issue_round_trips_d22_fields() {
    let s = session().await;
    let created = s
        .create_issue(NewIssue {
            title: "with d22 fields".to_string(),
            design: Some("the design".to_string()),
            acceptance_criteria: Some("the AC".to_string()),
            assignee: Some("alice".to_string()),
            agent_context: Some("the context".to_string()),
            ..NewIssue::default()
        })
        .await
        .expect("create_issue");

    let fetched = s.get(&created.id).await.expect("get").expect("present");
    assert_eq!(fetched.design.as_deref(), Some("the design"));
    assert_eq!(fetched.acceptance_criteria.as_deref(), Some("the AC"));
    assert_eq!(fetched.assignee.as_deref(), Some("alice"));
    assert_eq!(fetched.agent_context.as_deref(), Some("the context"));
}

/// The bulk path carries the D22 fields per record too.
#[tokio::test]
async fn create_bulk_round_trips_d22_fields() {
    let s = session().await;
    let created = s
        .create_bulk(vec![NewIssue {
            title: "bulk d22".to_string(),
            design: Some("d".to_string()),
            acceptance_criteria: Some("ac".to_string()),
            assignee: Some("bob".to_string()),
            agent_context: Some("ctx".to_string()),
            ..NewIssue::default()
        }])
        .await
        .expect("bulk create");

    let fetched = s.get(&created[0].id).await.expect("get").expect("present");
    assert_eq!(fetched.design.as_deref(), Some("d"));
    assert_eq!(fetched.acceptance_criteria.as_deref(), Some("ac"));
    assert_eq!(fetched.assignee.as_deref(), Some("bob"));
    assert_eq!(fetched.agent_context.as_deref(), Some("ctx"));
}

// --------------------------------------------------------------------------------------------------
// Fault-injection rollback (the engine-level all-or-nothing proof, non-vacuous)
// --------------------------------------------------------------------------------------------------

/// A batch whose insert hits an out-of-band racer (a `RaceInjector` commits a colliding row right
/// before the one-tx insert) rolls back the WHOLE tx: `create_bulk` returns `Err`, the engine routed
/// through exactly ONE `create_issues` call (NOT N `create_issue` calls), and ZERO of the batch's
/// intended rows persist (only the racer's own row remains).
///
/// Deterministic setup: pre-seed a committed parent via a SLUG (so its id is stable), then a batch of
/// two children under it. The children mint `parent.1` / `parent.2` deterministically. Arm the racer
/// to collide with `parent.2` so the in-tx insert of record #2 fails AFTER record #1 has staged —
/// proving the staged `parent.1` rolls back (non-vacuous: without one-tx atomicity, `parent.1` would
/// have committed independently).
#[tokio::test]
async fn out_of_band_racer_rolls_back_whole_batch() {
    let inner = LibsqlStorage::open_in_memory().await.expect("open");
    inner.migrate().await.expect("migrate");
    let inner: Arc<dyn Storage> = Arc::new(inner);

    // Pre-seed a committed parent with a stable slug-derived id.
    let parent_session = common::session_over(inner.clone(), SessionConfig::default()).await;
    let parent = parent_session
        .create_issue(NewIssue {
            title: "epic".to_string(),
            slug: Some("epic".to_string()),
            ..NewIssue::default()
        })
        .await
        .expect("create parent");
    let parent_id = parent.id.clone();
    let race_id = format!("{parent_id}.2"); // collide with the 2nd-minted child.

    let racing: Arc<RaceInjector> = RaceInjector::new(inner.clone(), race_id);
    let racing_dyn: Arc<dyn Storage> = racing.clone();
    let s = common::session_over(racing_dyn, SessionConfig::default()).await;

    let mut c1 = bulk_record("Child One");
    c1.parent = Some(parent_id.clone());
    let mut c2 = bulk_record("Child Two");
    c2.parent = Some(parent_id.clone());

    let count_before = count_all(&s).await; // 1 (the parent).
    let err = s
        .create_bulk(vec![c1, c2])
        .await
        .expect_err("out-of-band collision must fail the whole batch");
    assert!(
        matches!(&err, EngineError::Storage { .. }),
        "the failure is the storage-layer collision: {err:?}",
    );

    // ONE atomic create_issues call (the bulk primitive), NOT a loop of create_issue.
    assert_eq!(
        racing.bulk_calls(),
        1,
        "exactly one atomic create_issues call"
    );
    assert_eq!(
        racing.single_calls(),
        0,
        "the bulk path never loops create_issue"
    );

    // ZERO of the batch's children persist beyond the racer's own committed row. The store has the
    // parent + the racer's `parent.2` row = 2; `parent.1` (record #1, staged then rolled back) is ABSENT.
    assert_eq!(
        count_all(&s).await,
        count_before + 1,
        "only the out-of-band racer row was added; the staged batch rows rolled back",
    );
    assert!(
        s.get(&format!("{parent_id}.1"))
            .await
            .expect("get")
            .is_none(),
        "the first staged child (parent.1) rolled back — no partial batch",
    );
}

// --------------------------------------------------------------------------------------------------
// Single-create paths unchanged
// --------------------------------------------------------------------------------------------------

/// `create_issue` (single) and `create(&Issue)` (import) still work — the bulk path is additive.
#[tokio::test]
async fn single_create_paths_unchanged() {
    let s = session().await;
    let minted = s
        .create_issue(bulk_record("single"))
        .await
        .expect("create_issue");
    assert!(parse_id(&minted.id).expect("parses").is_root());

    let imported = unblock_model::Issue {
        id: "ub-import-1".to_string(),
        title: "imported".to_string(),
        ..unblock_model::Issue::default()
    };
    let id = s.create(&imported).await.expect("create import");
    assert_eq!(id, "ub-import-1", "import path preserves the caller id");
}

/// An empty bulk batch is a no-op `Ok` returning an empty Vec.
#[tokio::test]
async fn empty_batch_is_noop_ok() {
    let s = session().await;
    let created = s.create_bulk(vec![]).await.expect("empty batch Ok");
    assert!(created.is_empty());
    assert_eq!(count_all(&s).await, 0);
}
