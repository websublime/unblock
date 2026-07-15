//! Backend-independent **`Storage` contract suite** (NFR-16) + the gated [`StorageTestkit`] seam.
//!
//! This is the reusable proof a future backend (roadmap v2+) reuses to demonstrate it honours the
//! [`Storage`](crate::Storage) contract: it is **generic over a storage factory** and exercises
//! **every** trait method plus the cross-cutting invariants spelled out in spine §3.2.1. The libsql
//! self-test (`tests/contract.rs`) runs it against `LibsqlStorage::open_in_memory` and a temp-file
//! `open_local`.
//!
//! # Why a seam trait
//!
//! Two contract properties are not reachable through the public surface alone:
//!
//! - [`Storage::detect_cycles`](crate::Storage::detect_cycles)' **positive** path — the public
//!   [`add_dependency`](crate::Storage::add_dependency) *rejects* a gating-cycle edge with
//!   `CycleDetected`, so a stored cycle can only be planted by bypassing that guard; and
//! - the **id child-counter high-water mark**, an internal allocation invariant.
//!
//! [`StorageTestkit`] exposes exactly those two seams, gated behind
//! `#[cfg(any(test, feature = "testkit"))]`, so they never widen the production surface.
//!
//! # Structure
//!
//! Each `contract_*` case is its own `pub async fn`, so a future backend can run a subset. The
//! entry point [`run_storage_contract_suite`] drives every case in turn, requesting a **fresh,
//! migrated** store from the factory per case so no state leaks across cases.

// Every `contract_*` case is a **test-harness assertion**: it panics (via `assert!`/`unwrap`) on a
// contract violation, by design. Documenting a `# Panics` section on each would be noise — the whole
// module exists to panic on failure — so the pedantic `missing_panics_doc` lint is scoped off here.
#![allow(clippy::missing_panics_doc)]

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};

use unblock_model::{
    CountGroupBy, Dependency, DependencyType, Issue, IssueType, IssueValidator, ListFilters,
    Priority, Status,
};

use crate::error::StorageError;
use crate::filters::{DeleteMode, DeletePlan, IssuePatch};
use crate::trait_def::Storage;

/// The two backend-independent **testkit seams** a backend must provide so the NFR-16 contract suite
/// can reach paths the public [`Storage`] surface deliberately blocks (spine §3.2.1).
///
/// Gated (`#[cfg(any(test, feature = "testkit"))]`); never part of the default public surface.
#[async_trait]
pub trait StorageTestkit: Storage {
    /// Insert a dependency edge **bypassing the cycle guard** (and the duplicate guard, and the
    /// audit event) so a stored gating cycle can be planted.
    ///
    /// The public [`Storage::add_dependency`] rejects a would-be gating cycle with
    /// [`StorageError::CycleDetected`]; this seam is the only way to drive
    /// [`Storage::detect_cycles`]' positive path. Use it **only** from the contract suite.
    async fn testkit_insert_raw_edge(&self, dep: &Dependency) -> Result<(), StorageError>;

    /// Read the child-counter **high-water mark** for `parent_id` (the max child segment allocated
    /// under it), or `None` if no child has been allocated.
    ///
    /// Lets the suite assert the id child-counter advances monotonically past the hierarchical
    /// children created through the public [`Storage::create_issue`].
    async fn testkit_child_high_water(&self, parent_id: &str) -> Result<Option<u32>, StorageError>;

    // --- T0.8 contention-lab instrumentation seams (RK-1 / NFR-3) ---------------------------------
    //
    // The counters + toggles below exist solely so the M0 contention-lab gate
    // (`tests/contention_lab.rs`) can prove, from outside the crate, that (a) contention actually
    // materialized (the busy-retry witness is > 0 under contention and == 0 without) and (b) the
    // passive WAL checkpoint keeps the sidecar bounded — without ever widening the production surface.

    /// The number of **witnessed write-lock contention events** since open.
    ///
    /// A contended `BEGIN IMMEDIATE` (another writer held the file write-lock) is counted here when
    /// the busy-witness probe is enabled, or once per spin in the forced-spin control. The contention
    /// lab asserts this is `> 0` in the contended leg and `== 0` in the baseline leg — the
    /// deterministic proof that contention materialized (never a silent pass).
    async fn testkit_busy_retry_count(&self) -> u64;

    /// The number of **passive WAL checkpoints** fired by the periodic cadence since open.
    async fn testkit_checkpoint_count(&self) -> u64;

    /// The number of **committed mutations** since open (every `BEGIN IMMEDIATE` that committed).
    async fn testkit_mutation_count(&self) -> u64;

    /// Set the passive-checkpoint cadence: fire one passive checkpoint every `n` committed mutations
    /// (`0` disables it). The lab sets `0` inside its timed CPU-ratio brackets so checkpoint CPU
    /// never enters the ratio, and restores the production cadence for the WAL-bound sub-phase.
    async fn testkit_set_checkpoint_interval(&self, n: u64);

    /// Enable/disable the **zero-timeout busy-witness probe** on the write path.
    ///
    /// libsql exposes no busy-handler callback and the native `busy_timeout` resolves contention by
    /// *blocking silently* (no error surfaces), so without this probe contention is invisible from
    /// safe Rust. With it on, each mutating `BEGIN IMMEDIATE` first tries to acquire the write lock
    /// with a zero timeout: if another writer holds it, that is recorded as one busy-retry and the
    /// write then proceeds with the real (blocking) begin — the blocking semantics the gate measures
    /// are unchanged. Off in production.
    async fn testkit_set_busy_witness(&self, on: bool);
}

// --------------------------------------------------------------------------------------------------
// Suite entry point
// --------------------------------------------------------------------------------------------------

/// Run the **full** backend-independent `Storage` contract suite (NFR-16) against a backend produced
/// by `factory`.
///
/// `factory` must yield a **fresh, migrated** store on each call (no cross-case state). The
/// concurrency case wraps the produced store in `Arc<S>` and spawns tokio tasks, hence the
/// `'static` / `Send` bounds.
///
/// # Panics
///
/// Panics (via `assert!`) on the first contract violation — it is a test harness.
pub async fn run_storage_contract_suite<S, F, Fut>(factory: F)
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = S> + Send + 'static,
    S: Storage + StorageTestkit + 'static,
{
    // Lifecycle.
    contract_migrate_idempotent(factory().await).await;
    contract_integrity_check_clean(factory().await).await;
    contract_schema_version(factory().await).await;

    // CRUD.
    contract_create_issue(factory().await).await;
    contract_create_issues_atomic(factory().await).await;
    contract_get_issue(factory().await).await;
    contract_get_issues(factory().await).await;
    contract_get_issues_skip_absent_order(factory().await).await;
    contract_update_issue(factory().await).await;
    contract_delete_issue(factory().await).await;
    contract_restore_issue(factory().await).await;

    // Claim / defer.
    contract_claim_issue_three_outcomes(factory().await).await;
    contract_claim_concurrent_exactly_one_winner(Arc::new(factory().await)).await;
    contract_defer_issue(factory().await).await;
    contract_undefer_issue(factory().await).await;

    // Queries.
    contract_list_issues(factory().await).await;
    contract_ready_issues(factory().await).await;
    contract_blocked_issues(factory().await).await;
    contract_blocked_facets(factory().await).await;
    contract_ready_blocked_disjoint(factory().await).await;
    contract_search_issues(factory().await).await;
    contract_search_escape_guard(factory().await).await;
    contract_list_filters_compose(factory().await).await;
    contract_priority_range_inclusive(factory().await).await;
    contract_label_and_or(factory().await).await;
    contract_count_issues_sum_consistency(factory().await).await;
    contract_stale_issues(factory().await).await;
    contract_batch_hydration_determinism(factory().await).await;

    // Dependencies.
    contract_add_dependency(factory().await).await;
    contract_remove_dependency(factory().await).await;
    contract_list_dependencies(factory().await).await;
    contract_dependency_tree(factory().await).await;
    contract_dependency_graph_whole(factory().await).await;
    contract_detect_cycles_generic(factory().await).await;
    contract_detect_cycles_positive(factory().await).await;

    // Events.
    contract_list_events_order_oracle(factory().await).await;

    // Diagnostics.
    contract_epic_child_rollup(factory().await).await;
    contract_closed_since(factory().await).await;
    contract_orphan_candidates(factory().await).await;

    // Cross-cutting invariants.
    contract_noop_update_writes_no_event(factory().await).await;
    contract_dry_run_mutates_nothing(factory().await).await;
    contract_tombstone_preserves_type_and_event_rule(factory().await).await;
    contract_transactional_audit_atomicity(factory().await).await;

    // Delete modes — cascade + hard (FR-1c, never-run production paths).
    contract_cascade_delete(factory().await).await;
    contract_hard_delete(factory().await).await;

    // Seam-backed: id child-counter high-water mark.
    contract_child_counter_high_water(factory().await).await;

    // Production trait read-half (D21): next_child_number advances as children are created.
    contract_next_child_number(factory().await).await;
}

// --------------------------------------------------------------------------------------------------
// Fixtures
// --------------------------------------------------------------------------------------------------

/// A fixed instant for deterministic snapshots.
fn ts(year: i32, month: u32, day: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(year, month, day, 0, 0, 0)
        .single()
        .unwrap_or_else(Utc::now)
}

/// A minimal valid issue at the fixed epoch.
fn issue(id: &str, title: &str) -> Issue {
    Issue {
        id: id.to_string(),
        title: title.to_string(),
        created_at: ts(2026, 1, 1),
        updated_at: ts(2026, 1, 1),
        ..Issue::default()
    }
}

/// A dependency edge between two issues.
fn dep(from: &str, to: &str, dep_type: DependencyType) -> Dependency {
    Dependency {
        issue_id: from.to_string(),
        depends_on_id: to.to_string(),
        dep_type,
        created_at: ts(2026, 1, 1),
        created_by: None,
        metadata: None,
        thread_id: None,
    }
}

/// Collect the `event_type` strings for an issue, oldest first.
async fn event_types<S: Storage>(storage: &S, id: &str) -> Vec<String> {
    storage
        .list_events(id)
        .await
        .expect("list_events")
        .into_iter()
        .map(|e| e.event_type.as_str().to_string())
        .collect()
}

/// The total row count (active + closed + tombstone) via the public list read path.
async fn count_all<S: Storage>(storage: &S) -> usize {
    let filters = ListFilters {
        include_closed: true,
        include_deferred: true,
        ..ListFilters::default()
    };
    storage
        .list_issues(&filters)
        .await
        .expect("list_issues")
        .len()
}

// --------------------------------------------------------------------------------------------------
// Lifecycle
// --------------------------------------------------------------------------------------------------

/// `migrate` is idempotent: re-running on an already-migrated store is a no-op `Ok`.
pub async fn contract_migrate_idempotent<S: Storage>(storage: S) {
    // The factory already migrated; a second migrate must still succeed (no-op).
    storage.migrate().await.expect("re-migrate is a no-op Ok");
}

/// `integrity_check` returns an empty `Vec` on a healthy database (the `"ok"` sentinel normalized).
pub async fn contract_integrity_check_clean<S: Storage>(storage: S) {
    let problems = storage.integrity_check().await.expect("integrity_check");
    assert!(problems.is_empty(), "healthy DB returns no problems");
}

/// `schema_version` reports the stamped baseline on a migrated store (D27/AF-2, T3.1).
///
/// The factory yields a **migrated** store, so its on-disk `PRAGMA user_version` is the current
/// baseline: a positive value (a fresh/unstamped DB would report `0`). This is a backend-agnostic
/// assertion (the concrete baseline constant is a libsql-internal detail); it proves the read
/// surfaces a real stamped version and re-reads consistently (a pure read, no side-effect).
pub async fn contract_schema_version<S: Storage>(storage: S) {
    let version = storage.schema_version().await.expect("schema_version");
    assert!(
        version >= 1,
        "a migrated store reports its stamped baseline (>= 1), not the unstamped 0; got {version}"
    );
    // Pure read: re-reading yields the same version (no migration side-effect).
    let again = storage
        .schema_version()
        .await
        .expect("schema_version re-read");
    assert_eq!(version, again, "schema_version is a stable pure read");
}

// --------------------------------------------------------------------------------------------------
// CRUD
// --------------------------------------------------------------------------------------------------

/// `create_issue` returns the id, writes `Event(Created)`, does **not** dedup on content, and
/// rejects an id collision.
pub async fn contract_create_issue<S: Storage>(storage: S) {
    let id = storage
        .create_issue(&issue("ub-1", "first"), "alice")
        .await
        .expect("create");
    assert_eq!(id, "ub-1");
    assert_eq!(event_types(&storage, "ub-1").await, vec!["created"]);

    // Same content, different id → NOT deduped.
    let id2 = storage
        .create_issue(&issue("ub-2", "first"), "alice")
        .await
        .expect("create dup content");
    assert_eq!(id2, "ub-2");

    // Same id → IdCollision.
    let collision = storage.create_issue(&issue("ub-1", "x"), "alice").await;
    assert!(
        matches!(collision, Err(StorageError::IdCollision { .. })),
        "duplicate id must collide"
    );
}

/// `create_issues` (D22/T2.3) is the ATOMIC bulk INSERT: a clean batch inserts every row + a
/// sibling-to-sibling edge in ONE tx, AND a mid-batch failure rolls back the WHOLE batch (ZERO rows
/// persist — the all-or-nothing proof, spine §3.2.1).
pub async fn contract_create_issues_atomic<S: Storage>(storage: S) {
    // (1) A clean N-record batch with an intra-batch sibling edge round-trips.
    let mut b = issue("ub-b", "beta");
    b.dependencies = vec![dep("ub-b", "ub-a", DependencyType::Blocks)];
    let batch = vec![issue("ub-a", "alpha"), b, issue("ub-c", "gamma")];
    storage
        .create_issues(&batch, "alice")
        .await
        .expect("clean bulk create");
    let loaded = storage
        .get_issues(&["ub-a".to_string(), "ub-b".to_string(), "ub-c".to_string()])
        .await
        .expect("get_issues");
    assert_eq!(loaded.len(), 3, "all rows persisted");
    let edges = storage.list_dependencies("ub-b").await.expect("deps");
    assert_eq!(edges.len(), 1, "sibling edge persisted");
    assert_eq!(edges[0].depends_on_id, "ub-a");
    assert_eq!(event_types(&storage, "ub-a").await, vec!["created"]);

    // (2) FAULT-INJECTION ROLLBACK: a second batch whose record #k collides with a committed id
    //     fails and leaves ZERO new rows — records staged before #k are discarded.
    let count_before = count_all(&storage).await;
    let collide_batch = vec![
        issue("ub-x", "x"),
        issue("ub-y", "y"),
        issue("ub-a", "collides"), // duplicates the committed ub-a
    ];
    let err = storage
        .create_issues(&collide_batch, "alice")
        .await
        .expect_err("mid-batch collision must fail the whole batch");
    assert!(
        matches!(err, StorageError::IdCollision { id } if id == "ub-a"),
        "the failure is the IdCollision on record #k",
    );
    assert_eq!(
        count_all(&storage).await,
        count_before,
        "ZERO rows from the failed batch persist (no partial commit)",
    );
    assert!(
        storage.get_issue("ub-x").await.expect("get").is_none(),
        "record staged before the failure must NOT persist",
    );
    assert!(
        storage.get_issue("ub-y").await.expect("get").is_none(),
        "record staged before the failure must NOT persist",
    );
}

/// `get_issue` hydrates labels + deps; a missing id is `Ok(None)` (not an error).
pub async fn contract_get_issue<S: Storage>(storage: S) {
    let mut seeded = issue("ub-1", "with relations");
    seeded.labels = vec!["beta".to_string(), "alpha".to_string()];
    storage.create_issue(&seeded, "a").await.expect("create");

    let fetched = storage
        .get_issue("ub-1")
        .await
        .expect("get_issue")
        .expect("present");
    assert_eq!(fetched.id, "ub-1");
    // Labels hydrate sorted.
    assert_eq!(
        fetched.labels,
        vec!["alpha".to_string(), "beta".to_string()]
    );
    // content_hash is recomputed on load (never trusted from the column).
    assert_eq!(
        fetched.content_hash.as_deref(),
        Some(fetched.compute_content_hash().as_str())
    );

    // Missing id → Ok(None).
    assert!(
        storage
            .get_issue("ub-nope")
            .await
            .expect("get_issue")
            .is_none(),
        "missing id is Ok(None), not an error"
    );
}

/// `get_issues` returns the hydrated subset; unknown ids are simply absent.
pub async fn contract_get_issues<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-1", "a"), "a")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-2", "b"), "a")
        .await
        .unwrap();

    let got = storage
        .get_issues(&[
            "ub-1".to_string(),
            "ub-missing".to_string(),
            "ub-2".to_string(),
        ])
        .await
        .expect("get_issues");
    let ids: Vec<String> = got.into_iter().map(|i| i.id).collect();
    assert_eq!(ids.len(), 2, "unknown ids are absent");
    assert!(ids.contains(&"ub-1".to_string()));
    assert!(ids.contains(&"ub-2".to_string()));

    // Ids are a lookup SET: a duplicate id yields AT MOST ONE result (the batch-hydration path
    // dedups; the trait contract makes no duplicate-preservation guarantee — T3.5.1).
    let deduped = storage
        .get_issues(&["ub-1".to_string(), "ub-1".to_string()])
        .await
        .expect("get_issues dedups a duplicate id");
    assert_eq!(deduped.len(), 1, "a duplicate id yields at most one result");
}

/// `update_issue` applies a patch, advances `updated_at` and recomputes `content_hash` when a row
/// column changes, and emits the per-field events.
pub async fn contract_update_issue<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-1", "old"), "a")
        .await
        .unwrap();
    let before = storage.get_issue("ub-1").await.unwrap().unwrap();

    let patch = IssuePatch {
        title: Some("new".to_string()),
        ..IssuePatch::default()
    };
    let after = storage.update_issue("ub-1", &patch, "a").await.unwrap();
    assert_eq!(after.title, "new");
    assert!(after.updated_at >= before.updated_at, "updated_at advances");
    // content_hash recomputed.
    assert_eq!(
        after.content_hash.as_deref(),
        Some(after.compute_content_hash().as_str())
    );
    // Title change → exactly one Updated event after Created.
    assert_eq!(
        event_types(&storage, "ub-1").await,
        vec!["created", "updated"]
    );

    // Updating a missing issue → IssueNotFound.
    let missing = storage.update_issue("ub-nope", &patch, "a").await;
    assert!(matches!(missing, Err(StorageError::IssueNotFound { .. })));
}

/// `delete_issue` (Tombstone mode) soft-deletes and returns the resolved plan.
pub async fn contract_delete_issue<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-1", "t"), "a")
        .await
        .unwrap();

    let plan = DeletePlan {
        mode: DeleteMode::Tombstone,
        targets: vec!["ub-1".to_string()],
        cascade_children: Vec::new(),
    };
    let resolved = storage.delete_issue(&plan, "admin").await.expect("delete");
    assert_eq!(resolved.targets, vec!["ub-1".to_string()]);

    let fetched = storage.get_issue("ub-1").await.unwrap().unwrap();
    assert_eq!(fetched.status, Status::Tombstone);
}

/// `restore_issue` (D20) — the audited live inverse of a soft delete (spine §3.2.1):
/// - a tombstone restores to active, **clears `original_type`**, **preserves `issue_type`**, and
///   emits exactly one `Event(Restored)`;
/// - an already-active issue is an **idempotent no-op `Ok`** (no new event);
/// - a missing/hard-deleted id → `IssueNotFound`.
pub async fn contract_restore_issue<S: Storage>(storage: S) {
    // create → delete(Tombstone) → restore.
    let mut bug = issue("ub-1", "t");
    bug.issue_type = IssueType::Bug;
    storage.create_issue(&bug, "a").await.unwrap();

    let plan = DeletePlan {
        mode: DeleteMode::Tombstone,
        targets: vec!["ub-1".to_string()],
        cascade_children: Vec::new(),
    };
    storage.delete_issue(&plan, "admin").await.expect("delete");
    let tombstoned = storage.get_issue("ub-1").await.unwrap().unwrap();
    assert_eq!(tombstoned.status, Status::Tombstone);
    assert_eq!(tombstoned.original_type.as_deref(), Some("bug"));

    let restored = storage
        .restore_issue("ub-1", "admin")
        .await
        .expect("restore");
    // Active again (was-Open → Open), original_type cleared, issue_type preserved.
    assert_eq!(restored.status, Status::Open);
    assert_eq!(
        restored.original_type, None,
        "original_type cleared on restore"
    );
    assert_eq!(
        restored.issue_type,
        IssueType::Bug,
        "issue_type preserved across restore"
    );
    assert!(restored.deleted_at.is_none());
    assert!(restored.deleted_by.is_none());

    // Exactly one Restored event (created → deleted → restored), never StatusChanged/Reopened.
    assert_eq!(
        event_types(&storage, "ub-1").await,
        vec![
            "created".to_string(),
            "deleted".to_string(),
            "restored".to_string()
        ],
        "restore emits a single Restored event, never StatusChanged/Reopened"
    );

    // Already-active → idempotent no-op Ok: NO event AND NO row write. Capture updated_at +
    // content_hash BEFORE so the no-op contract is non-vacuous — a stray `UPDATE … SET updated_at`
    // (which would still emit no event) must be caught. Mirrors
    // `noop_update_writes_no_event_and_leaves_updated_at` (behaviour.rs:100). The ==-assert on
    // updated_at/content_hash is the load-bearing guard here (the event check alone is vacuous).
    let before_events = event_types(&storage, "ub-1").await;
    let before = storage.get_issue("ub-1").await.unwrap().unwrap();
    let again = storage
        .restore_issue("ub-1", "admin")
        .await
        .expect("restore of an active issue is an idempotent Ok");
    assert_eq!(again.status, Status::Open);
    assert_eq!(
        event_types(&storage, "ub-1").await,
        before_events,
        "restore of an already-active issue writes no event"
    );
    let after = storage.get_issue("ub-1").await.unwrap().unwrap();
    assert_eq!(
        after.updated_at, before.updated_at,
        "no-op restore must NOT bump updated_at (byte-for-byte unchanged)"
    );
    assert_eq!(
        after.content_hash, before.content_hash,
        "no-op restore must NOT rewrite the row (content_hash byte-for-byte unchanged)"
    );

    // Missing id → IssueNotFound.
    match storage.restore_issue("ub-missing", "admin").await {
        Err(StorageError::IssueNotFound { id }) => assert_eq!(id, "ub-missing"),
        other => panic!("expected IssueNotFound for a missing id, got {other:?}"),
    }
}

// --------------------------------------------------------------------------------------------------
// Claim / defer
// --------------------------------------------------------------------------------------------------

/// `claim_issue` has exactly three outcomes: unassigned succeeds; same-actor re-claim is an
/// idempotent `Ok` with no new event; a different actor loses with `AlreadyClaimed{by}`.
pub async fn contract_claim_issue_three_outcomes<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-1", "t"), "a")
        .await
        .unwrap();

    // (1) unassigned → succeeds; events AssigneeChanged + StatusChanged (the §3.2.1 order).
    storage.claim_issue("ub-1", "alice", "alice").await.unwrap();
    let events = event_types(&storage, "ub-1").await;
    assert_eq!(
        events,
        vec!["created", "assignee_changed", "status_changed"],
        "won claim emits AssigneeChanged then StatusChanged"
    );

    // (2) same-actor re-claim → idempotent Ok, no new event.
    storage.claim_issue("ub-1", "alice", "alice").await.unwrap();
    assert_eq!(
        event_types(&storage, "ub-1").await,
        events,
        "same-actor re-claim writes no event"
    );

    // (3) different actor → AlreadyClaimed{by = alice}.
    match storage.claim_issue("ub-1", "bob", "bob").await {
        Err(StorageError::AlreadyClaimed { by, .. }) => assert_eq!(by, "alice"),
        other => panic!("expected AlreadyClaimed, got {other:?}"),
    }
}

/// N concurrent claimers on the same issue: **exactly one** wins, the rest get `AlreadyClaimed`.
pub async fn contract_claim_concurrent_exactly_one_winner<S>(storage: Arc<S>)
where
    S: Storage + 'static,
{
    storage
        .create_issue(&issue("ub-1", "t"), "a")
        .await
        .unwrap();

    let mut handles = Vec::new();
    for n in 0..8 {
        let storage = Arc::clone(&storage);
        handles.push(tokio::spawn(async move {
            let actor = format!("agent-{n}");
            storage.claim_issue("ub-1", &actor, &actor).await
        }));
    }

    let mut wins = 0;
    let mut already = 0;
    for handle in handles {
        match handle.await.expect("join") {
            Ok(_) => wins += 1,
            Err(StorageError::AlreadyClaimed { .. }) => already += 1,
            Err(other) => panic!("unexpected error: {other:?}"),
        }
    }
    assert_eq!(wins, 1, "exactly one claim must win");
    assert_eq!(already, 7, "the rest must lose with AlreadyClaimed");
}

/// `defer_issue` sets `defer_until` (excluding the issue from `ready`) and writes `Event(Updated)`.
pub async fn contract_defer_issue<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-1", "t"), "a")
        .await
        .unwrap();
    storage
        .defer_issue("ub-1", ts(2099, 1, 1), "a")
        .await
        .expect("defer");

    let fetched = storage.get_issue("ub-1").await.unwrap().unwrap();
    assert!(fetched.defer_until.is_some(), "defer_until set");
    assert_eq!(
        event_types(&storage, "ub-1").await,
        vec!["created", "updated"],
        "defer writes Updated"
    );

    // The deferred issue is excluded from ready.
    let ready: Vec<String> = storage
        .ready_issues(&ListFilters::default())
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert!(!ready.contains(&"ub-1".to_string()), "deferred excluded");
}

/// `undefer_issue` clears `defer_until` (the issue becomes ready-eligible again) + `Event(Updated)`.
pub async fn contract_undefer_issue<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-1", "t"), "a")
        .await
        .unwrap();
    storage
        .defer_issue("ub-1", ts(2099, 1, 1), "a")
        .await
        .unwrap();
    storage.undefer_issue("ub-1", "a").await.expect("undefer");

    let fetched = storage.get_issue("ub-1").await.unwrap().unwrap();
    assert!(fetched.defer_until.is_none(), "defer_until cleared");

    let ready: Vec<String> = storage
        .ready_issues(&ListFilters::default())
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert!(
        ready.contains(&"ub-1".to_string()),
        "undeferred issue is ready again"
    );
    assert_eq!(
        event_types(&storage, "ub-1").await,
        vec!["created", "updated", "updated"],
        "undefer writes a second Updated"
    );
}

// --------------------------------------------------------------------------------------------------
// Queries
// --------------------------------------------------------------------------------------------------

/// `list_issues` honours the status facet and excludes closed/tombstone by default.
pub async fn contract_list_issues<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-open", "open"), "a")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-closed", "closed"), "a")
        .await
        .unwrap();
    storage
        .update_issue(
            "ub-closed",
            &IssuePatch {
                status: Some(Status::Closed),
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .unwrap();

    // Default filter excludes closed.
    let all: Vec<String> = storage
        .list_issues(&ListFilters::default())
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert!(all.contains(&"ub-open".to_string()));
    assert!(
        !all.contains(&"ub-closed".to_string()),
        "closed excluded by default"
    );

    // include_closed surfaces it.
    let with_closed: Vec<String> = storage
        .list_issues(&ListFilters {
            include_closed: true,
            ..ListFilters::default()
        })
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert!(with_closed.contains(&"ub-closed".to_string()));
}

/// `ready_issues` excludes blocked, deferred, and closed issues; is deterministically ordered.
pub async fn contract_ready_issues<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-open", "open"), "a")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-blocker", "blocker"), "a")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-blocked", "blocked"), "a")
        .await
        .unwrap();
    storage
        .add_dependency(
            &dep("ub-blocked", "ub-blocker", DependencyType::Blocks),
            "a",
        )
        .await
        .unwrap();

    let ready: Vec<String> = storage
        .ready_issues(&ListFilters::default())
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert!(ready.contains(&"ub-open".to_string()));
    assert!(ready.contains(&"ub-blocker".to_string()));
    assert!(
        !ready.contains(&"ub-blocked".to_string()),
        "blocked excluded from ready"
    );
}

/// `blocked_issues` returns issues with an unresolved gating edge (includes `in_progress`).
pub async fn contract_blocked_issues<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-blocker", "blocker"), "a")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-wip", "wip"), "a")
        .await
        .unwrap();
    storage
        .add_dependency(&dep("ub-wip", "ub-blocker", DependencyType::Blocks), "a")
        .await
        .unwrap();
    storage.claim_issue("ub-wip", "bob", "bob").await.unwrap();

    let blocked: Vec<String> = storage
        .blocked_issues(&ListFilters::default())
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert!(
        blocked.contains(&"ub-wip".to_string()),
        "in_progress blocked issue appears"
    );
}

/// **`blocked_issues` composes the `list` narrowing facets AND preserves the deferred-blocked set**
/// (D18, spine §3.2.1; the cross-backend guard for the T1.5 production change — A.7).
///
/// This is the ONLY contract leg that protects future backends against the most likely
/// implementation error: reusing `list`'s default visibility branch in `blocked` (which strips
/// `deferred`-status rows). It therefore asserts both the facet narrowing and — crucially — that a
/// `deferred`-status blocked issue STILL appears under a default filter and is NOT dropped by
/// `include_deferred=false`.
// The comprehensive corpus (5 issues + a blocker) plus the seven a–g sub-assertions exceed the
// pedantic line cap; splitting the single coherent A.7 guard would obscure the invariant it pins.
#[allow(clippy::too_many_lines)]
pub async fn contract_blocked_facets<S: Storage>(storage: S) {
    // A shared blocker so each candidate has an unresolved gating edge.
    storage
        .create_issue(&issue("ub-blocker", "blocker"), "a")
        .await
        .unwrap();

    // ub-bug-blocked: Bug, P0, label {a}, blocked + in_progress (claimed).
    let mut bug = issue("ub-bug-blocked", "bug blocked");
    bug.issue_type = IssueType::Bug;
    bug.priority = Priority::CRITICAL; // P0 — the LOWEST priority NUMBER
    bug.labels = vec!["a".to_string()];
    storage.create_issue(&bug, "a").await.unwrap();
    storage
        .add_dependency(
            &dep("ub-bug-blocked", "ub-blocker", DependencyType::Blocks),
            "a",
        )
        .await
        .unwrap();
    storage
        .claim_issue("ub-bug-blocked", "bob", "bob")
        .await
        .unwrap();

    // ub-defer-blocked: Task, P2, label {b}, blocked, then set status = deferred (the regression pin).
    let mut deferred = issue("ub-defer-blocked", "deferred blocked");
    deferred.issue_type = IssueType::Task;
    deferred.priority = Priority::MEDIUM; // P2
    deferred.labels = vec!["b".to_string()];
    storage.create_issue(&deferred, "a").await.unwrap();
    storage
        .add_dependency(
            &dep("ub-defer-blocked", "ub-blocker", DependencyType::Blocks),
            "a",
        )
        .await
        .unwrap();
    // Set the STATUS to deferred (defer_until alone does NOT change status; only a deferred *status*
    // exercises list's `NOT IN ('deferred')` strip — the precise regression A.7 guards).
    storage
        .update_issue(
            "ub-defer-blocked",
            &IssuePatch {
                status: Some(Status::Deferred),
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .unwrap();

    // ub-closed-blocked: was blocked, then closed (must NEVER be blocked-visible).
    storage
        .create_issue(&issue("ub-closed-blocked", "closed blocked"), "a")
        .await
        .unwrap();
    storage
        .add_dependency(
            &dep("ub-closed-blocked", "ub-blocker", DependencyType::Blocks),
            "a",
        )
        .await
        .unwrap();
    storage
        .update_issue(
            "ub-closed-blocked",
            &IssuePatch {
                status: Some(Status::Closed),
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .unwrap();

    // ub-free: open, no gating edge — never blocked.
    storage
        .create_issue(&issue("ub-free", "free"), "a")
        .await
        .unwrap();

    let blocked_ids = |f: ListFilters| {
        let storage = &storage;
        async move {
            storage
                .blocked_issues(&f)
                .await
                .unwrap()
                .into_iter()
                .map(|i| i.id)
                .collect::<std::collections::HashSet<String>>()
        }
    };

    // (a) default: both the in_progress AND the deferred-status blocked issue appear; closed excluded;
    //     ub-free (no edge) excluded.
    let default = blocked_ids(ListFilters::default()).await;
    assert!(
        default.contains("ub-bug-blocked"),
        "in_progress blocked issue appears under default"
    );
    // (b) REQUIRED visibility-preserved pin (A.7): the DEFERRED-status blocked issue is still present
    //     — proves blocked did NOT inherit list's `NOT IN ('deferred')` visibility branch.
    assert!(
        default.contains("ub-defer-blocked"),
        "deferred-status blocked issue MUST survive a default filter (deferred-inclusive baseline)"
    );
    assert!(
        !default.contains("ub-closed-blocked"),
        "closed is never blocked-visible"
    );
    assert!(!default.contains("ub-free"), "an unblocked issue is absent");

    // (c) facet narrows: labels_all=[a] keeps only the Bug-blocked (its sole {a} carrier).
    let by_label = blocked_ids(ListFilters {
        labels_all: vec!["a".to_string()],
        ..ListFilters::default()
    })
    .await;
    assert_eq!(
        by_label,
        std::collections::HashSet::from(["ub-bug-blocked".to_string()]),
        "labels_all=[a] narrows blocked to the single {{a}} carrier"
    );

    // (d) priority facet narrows non-vacuously: ub-bug-blocked=P0, ub-defer-blocked=P2 →
    //     priority_max=CRITICAL keeps ONLY the P0.
    let by_prio = blocked_ids(ListFilters {
        priority_max: Some(Priority::CRITICAL),
        ..ListFilters::default()
    })
    .await;
    assert_eq!(
        by_prio,
        std::collections::HashSet::from(["ub-bug-blocked".to_string()]),
        "priority_max=CRITICAL keeps only the P0 blocked issue (non-vacuous)"
    );

    // (e) REQUIRED no-op pins (A.7): include_deferred=false does NOT drop the deferred-blocked issue;
    //     include_closed=true does NOT add the closed-blocked issue.
    let no_defer = blocked_ids(ListFilters {
        include_deferred: false,
        ..ListFilters::default()
    })
    .await;
    assert!(
        no_defer.contains("ub-defer-blocked"),
        "include_deferred is a no-op on blocked — the deferred-blocked issue stays"
    );
    let with_closed = blocked_ids(ListFilters {
        include_closed: true,
        ..ListFilters::default()
    })
    .await;
    assert!(
        !with_closed.contains("ub-closed-blocked"),
        "include_closed is a no-op on blocked — closed never becomes blocked-visible"
    );

    // (g) status facet vs blocked base: status=[Closed] ∩ (status NOT IN closed/tombstone) = ∅.
    let status_closed = blocked_ids(ListFilters {
        status: vec![Status::Closed],
        ..ListFilters::default()
    })
    .await;
    assert!(
        status_closed.is_empty(),
        "blocked(status=[Closed]) is empty (closed is excluded by the deferred-inclusive base)"
    );
}

/// (Optional, mirrors engine #1 at the `Storage` layer.) `list_issues` facets compose as an
/// intersection: a multi-facet filter matches exactly the one row satisfying every facet.
pub async fn contract_list_filters_compose<S: Storage>(storage: S) {
    let mut hit = issue("ub-hit", "fix the parser");
    hit.issue_type = IssueType::Task;
    hit.priority = Priority::HIGH;
    hit.assignee = Some("alice".to_string());
    hit.labels = vec!["api".to_string()];
    storage.create_issue(&hit, "a").await.unwrap();

    // Decoys, each differing on exactly one facet.
    let mut wrong_type = issue("ub-wrongtype", "fix the parser");
    wrong_type.issue_type = IssueType::Bug;
    wrong_type.priority = Priority::HIGH;
    wrong_type.assignee = Some("alice".to_string());
    wrong_type.labels = vec!["api".to_string()];
    storage.create_issue(&wrong_type, "a").await.unwrap();

    let mut wrong_label = issue("ub-wronglabel", "fix the parser");
    wrong_label.issue_type = IssueType::Task;
    wrong_label.priority = Priority::HIGH;
    wrong_label.assignee = Some("alice".to_string());
    wrong_label.labels = vec!["ui".to_string()];
    storage.create_issue(&wrong_label, "a").await.unwrap();

    let combined = ListFilters {
        issue_type: vec![IssueType::Task],
        priority_min: Some(Priority::HIGH),
        priority_max: Some(Priority::HIGH),
        assignee: Some("alice".to_string()),
        labels_all: vec!["api".to_string()],
        text_contains: Some("parser".to_string()),
        ..ListFilters::default()
    };
    let ids: Vec<String> = storage
        .list_issues(&combined)
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert_eq!(
        ids,
        vec!["ub-hit".to_string()],
        "composed facets intersect to the single matching row"
    );

    // Non-vacuity: dropping the label facet admits the wrong-label decoy.
    let drop_label = ListFilters {
        labels_all: Vec::new(),
        ..combined
    };
    let ids: std::collections::HashSet<String> = storage
        .list_issues(&drop_label)
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert!(
        ids.contains("ub-wronglabel"),
        "relaxing labels_all admits the wrong-label decoy (the facet was load-bearing)"
    );
}

/// (Optional, mirrors engine #3.) The `priority` range is inclusive on both ends and direction-pinned
/// (CRITICAL=0 is the LOWEST number; a min/max swap cannot pass).
pub async fn contract_priority_range_inclusive<S: Storage>(storage: S) {
    for (id, prio) in [
        ("ub-p0", Priority::CRITICAL),
        ("ub-p1", Priority::HIGH),
        ("ub-p2", Priority::MEDIUM),
        ("ub-p3", Priority::LOW),
        ("ub-p4", Priority::BACKLOG),
    ] {
        let mut i = issue(id, id);
        i.priority = prio;
        storage.create_issue(&i, "a").await.unwrap();
    }

    let range: std::collections::HashSet<String> = storage
        .list_issues(&ListFilters {
            priority_min: Some(Priority::HIGH),
            priority_max: Some(Priority::MEDIUM),
            ..ListFilters::default()
        })
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert_eq!(
        range,
        std::collections::HashSet::from(["ub-p1".to_string(), "ub-p2".to_string()]),
        "priority [HIGH,MEDIUM] is inclusive on both ends and excludes the lower-numbered P0"
    );

    let only_p0: Vec<String> = storage
        .list_issues(&ListFilters {
            priority_max: Some(Priority::CRITICAL),
            ..ListFilters::default()
        })
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert_eq!(
        only_p0,
        vec!["ub-p0".to_string()],
        "priority_max=CRITICAL keeps only P0 (the lowest number)"
    );
}

/// (Optional, mirrors engine #2.) `labels_all` (AND) and `labels_any` (OR) discriminate, and both
/// together intersect (AND ∩ OR), with a `{c}` witness in neither.
pub async fn contract_label_and_or<S: Storage>(storage: S) {
    for (id, labels) in [
        ("ub-a", vec!["a"]),
        ("ub-b", vec!["b"]),
        ("ub-ab", vec!["a", "b"]),
        ("ub-c", vec!["c"]),
    ] {
        let mut i = issue(id, id);
        i.labels = labels.into_iter().map(str::to_string).collect();
        storage.create_issue(&i, "a").await.unwrap();
    }

    let all: std::collections::HashSet<String> = storage
        .list_issues(&ListFilters {
            labels_all: vec!["a".to_string(), "b".to_string()],
            ..ListFilters::default()
        })
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert_eq!(
        all,
        std::collections::HashSet::from(["ub-ab".to_string()]),
        "labels_all=[a,b] (AND) matches only the issue carrying BOTH"
    );

    let any: std::collections::HashSet<String> = storage
        .list_issues(&ListFilters {
            labels_any: vec!["a".to_string(), "b".to_string()],
            ..ListFilters::default()
        })
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert_eq!(
        any,
        std::collections::HashSet::from([
            "ub-a".to_string(),
            "ub-b".to_string(),
            "ub-ab".to_string()
        ]),
        "labels_any=[a,b] (OR) matches any carrier; ub-c (neither) is absent"
    );
    assert_ne!(
        all, any,
        "AND and OR over the same labels are distinct sets"
    );
}

/// Ready and blocked are **disjoint**; a deferred or closed issue is in neither.
pub async fn contract_ready_blocked_disjoint<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-ready", "ready"), "a")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-blocker", "blocker"), "a")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-blocked", "blocked"), "a")
        .await
        .unwrap();
    storage
        .add_dependency(
            &dep("ub-blocked", "ub-blocker", DependencyType::Blocks),
            "a",
        )
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-deferred", "deferred"), "a")
        .await
        .unwrap();
    storage
        .defer_issue("ub-deferred", ts(2099, 1, 1), "a")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-closed", "closed"), "a")
        .await
        .unwrap();
    storage
        .update_issue(
            "ub-closed",
            &IssuePatch {
                status: Some(Status::Closed),
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .unwrap();

    let ready: std::collections::HashSet<String> = storage
        .ready_issues(&ListFilters::default())
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    let blocked: std::collections::HashSet<String> = storage
        .blocked_issues(&ListFilters::default())
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();

    assert!(
        ready.is_disjoint(&blocked),
        "ready and blocked must be disjoint"
    );
    // Deferred and closed are in neither.
    for id in ["ub-deferred", "ub-closed"] {
        assert!(!ready.contains(id), "{id} not ready");
        assert!(!blocked.contains(id), "{id} not blocked");
    }
}

/// `search_issues` matches a substring over title + description + id, case-insensitively.
pub async fn contract_search_issues<S: Storage>(storage: S) {
    let mut needle = issue("ub-needle", "Parser bug");
    needle.description = Some("fix the lexer".to_string());
    storage.create_issue(&needle, "a").await.unwrap();
    storage
        .create_issue(&issue("ub-other", "Unrelated"), "a")
        .await
        .unwrap();

    let by_title = storage
        .search_issues("parser", &ListFilters::default())
        .await
        .unwrap();
    assert_eq!(by_title.len(), 1);
    assert_eq!(by_title[0].id, "ub-needle");

    let by_desc = storage
        .search_issues("LEXER", &ListFilters::default())
        .await
        .unwrap();
    assert_eq!(by_desc.len(), 1, "description substring, case-insensitive");

    let by_id = storage
        .search_issues("needle", &ListFilters::default())
        .await
        .unwrap();
    assert_eq!(by_id.len(), 1, "id substring");
}

/// `search`'s `text_contains` FILTER is `ESCAPE`-guarded: a literal `%` matches `%`, not everything.
pub async fn contract_search_escape_guard<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-pct", "50% done"), "a")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-plain", "all done"), "a")
        .await
        .unwrap();

    // text_contains "%" must be matched LITERALLY (ESCAPE guard) — only "50% done" contains a "%".
    let filtered = storage
        .list_issues(&ListFilters {
            text_contains: Some("%".to_string()),
            ..ListFilters::default()
        })
        .await
        .unwrap();
    let ids: Vec<String> = filtered.into_iter().map(|i| i.id).collect();
    assert_eq!(
        ids,
        vec!["ub-pct".to_string()],
        "literal % must not act as a wildcard (ESCAPE guard)"
    );
}

/// `count_issues` group buckets sum to the ungrouped total for Status/Type/Assignee/Priority; for
/// **Label** the sum equals the number of `(issue, label)` pairs among the matching issues (an issue
/// counts once per label — the trait-doc Label exception).
pub async fn contract_count_issues_sum_consistency<S: Storage>(storage: S) {
    // Two open issues; one closed (excluded by the default filter on both legs).
    storage
        .create_issue(&issue("ub-1", "one"), "a")
        .await
        .unwrap();
    let mut two = issue("ub-2", "two");
    two.assignee = Some("alice".to_string());
    two.priority = Priority::HIGH;
    two.labels = vec!["x".to_string(), "y".to_string()]; // multi-label
    storage.create_issue(&two, "a").await.unwrap();
    storage
        .create_issue(&issue("ub-3", "three"), "a")
        .await
        .unwrap();
    storage
        .update_issue(
            "ub-3",
            &IssuePatch {
                status: Some(Status::Closed),
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .unwrap();

    let filters = ListFilters::default();
    let total = total_count(&storage, &filters).await;
    assert_eq!(total, 2, "two open issues under the default filter");

    // Exact-sum group-bys.
    for group in [
        CountGroupBy::Status,
        CountGroupBy::Type,
        CountGroupBy::Assignee,
        CountGroupBy::Priority,
    ] {
        let sum: usize = storage
            .count_issues(&filters, Some(group))
            .await
            .unwrap()
            .into_iter()
            .map(|b| b.count)
            .sum();
        assert_eq!(sum, total, "{group:?} buckets must sum to the total");
    }

    // Label is the exception (trait doc): an issue is counted once PER label, so the Label group sum
    // equals the number of (issue, label) pairs among the matching issues — NOT simply `== total` or
    // `>= total` (a label-less issue contributes 0; a multi-label issue contributes >1). Derive the
    // expected pair count independently from the hydrated label lists of `list_issues`.
    let label_sum: usize = storage
        .count_issues(&filters, Some(CountGroupBy::Label))
        .await
        .unwrap()
        .into_iter()
        .map(|b| b.count)
        .sum();
    let pairs: usize = storage
        .list_issues(&filters)
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.labels.len())
        .sum();
    assert_eq!(
        label_sum, pairs,
        "Label group sum ({label_sum}) must equal the (issue, label) pair count ({pairs})"
    );
    // Concretely: ub-1 has 0 labels, ub-2 has {x, y} → 2 pairs; ub-3 is closed (excluded).
    assert_eq!(label_sum, 2, "x + y from the one multi-label open issue");
}

/// Sum the ungrouped total (the `group_by = None` single-bucket count).
async fn total_count<S: Storage>(storage: &S, filters: &ListFilters) -> usize {
    storage
        .count_issues(filters, None)
        .await
        .unwrap()
        .into_iter()
        .map(|b| b.count)
        .sum()
}

/// `stale_issues` returns issues whose `updated_at < older_than`.
pub async fn contract_stale_issues<S: Storage>(storage: S) {
    // ub-old keeps its fixed 2026-01-01 updated_at; ub-fresh is touched, which stamps `updated_at`
    // to the real wall clock (well after the cutoff below).
    storage
        .create_issue(&issue("ub-old", "old"), "a")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-fresh", "fresh"), "a")
        .await
        .unwrap();
    storage
        .update_issue(
            "ub-fresh",
            &IssuePatch {
                title: Some("touched".to_string()),
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .unwrap();

    // older_than sits just after ub-old's fixed timestamp (2026-01-01) and well before the
    // wall-clock `updated_at` the touch stamped on ub-fresh: ub-old is stale, ub-fresh is not.
    let cutoff = ts(2026, 1, 2);
    let stale: Vec<String> = storage
        .stale_issues(cutoff, &ListFilters::default())
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert!(stale.contains(&"ub-old".to_string()), "old issue is stale");
    assert!(
        !stale.contains(&"ub-fresh".to_string()),
        "freshly-updated issue is not stale"
    );
}

/// **Batch-hydration determinism (T3.5.1):** every read path attaches EACH issue's OWN labels +
/// dependencies (never a cross-attach) and preserves the query order (never the `IN`-result row
/// order), with `label ASC` + `depends_on_id ASC` intra-issue sorting — the anti-mis-grouping /
/// anti-reorder guard for the batched read path (`hydrate_ids`).
///
/// Three issues with **DISTINCT priorities** (so the result order is priority-driven and differs
/// from the id/insertion order — a reconstruction that used the `IN`-row order would reorder them),
/// **DISTINCT label sets** (one multi-label, inserted out of `label ASC` order), and **DISTINCT
/// dependency sets** (`blocks` edges to `external:*` targets, which never gate readiness and never
/// materialize as their own rows), driven through `list`, `ready`, AND `search`.
pub async fn contract_batch_hydration_determinism<S: Storage>(storage: S) {
    // ub-a: priority 2, two labels inserted UNSORTED, two external deps inserted UNSORTED.
    let mut a = issue("ub-a", "alpha widget");
    a.priority = Priority(2);
    a.labels = vec!["m-label".to_string(), "a-label".to_string()];
    a.dependencies = vec![
        dep("ub-a", "external:z2", DependencyType::Blocks),
        dep("ub-a", "external:z1", DependencyType::Blocks),
    ];
    // ub-b: priority 0, one label, one external dep.
    let mut b = issue("ub-b", "beta widget");
    b.priority = Priority(0);
    b.labels = vec!["b-label".to_string()];
    b.dependencies = vec![dep("ub-b", "external:z3", DependencyType::Blocks)];
    // ub-c: priority 1, no labels, no deps.
    let mut c = issue("ub-c", "gamma widget");
    c.priority = Priority(1);

    for i in [&a, &b, &c] {
        storage.create_issue(i, "seed").await.expect("create");
    }

    // Expected read order: priority ASC → [ub-b(0), ub-c(1), ub-a(2)] — NOT the id/insert order.
    let assert_shape = |issues: &[Issue], path: &str| {
        let ids: Vec<&str> = issues.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(
            ids,
            ["ub-b", "ub-c", "ub-a"],
            "{path}: priority order preserved"
        );

        // ub-b (index 0): its own single label + single dep.
        assert_eq!(
            issues[0].labels,
            vec!["b-label".to_string()],
            "{path}: ub-b labels"
        );
        let b_deps: Vec<&str> = issues[0]
            .dependencies
            .iter()
            .map(|d| d.depends_on_id.as_str())
            .collect();
        assert_eq!(b_deps, ["external:z3"], "{path}: ub-b deps");

        // ub-c (index 1): empty relations.
        assert!(issues[1].labels.is_empty(), "{path}: ub-c has no labels");
        assert!(
            issues[1].dependencies.is_empty(),
            "{path}: ub-c has no deps"
        );

        // ub-a (index 2): its two labels SORTED (label ASC), its two deps SORTED (depends_on_id ASC).
        assert_eq!(
            issues[2].labels,
            vec!["a-label".to_string(), "m-label".to_string()],
            "{path}: ub-a labels sorted label ASC"
        );
        let a_deps: Vec<&str> = issues[2]
            .dependencies
            .iter()
            .map(|d| d.depends_on_id.as_str())
            .collect();
        assert_eq!(
            a_deps,
            ["external:z1", "external:z2"],
            "{path}: ub-a deps sorted depends_on_id ASC"
        );
    };

    let listed = storage
        .list_issues(&ListFilters::default())
        .await
        .expect("list");
    assert_shape(&listed, "list");

    let ready = storage
        .ready_issues(&ListFilters::default())
        .await
        .expect("ready");
    assert_shape(&ready, "ready");

    let searched = storage
        .search_issues("widget", &ListFilters::default())
        .await
        .expect("search");
    assert_shape(&searched, "search");
}

/// **Batch-hydration skip-absent + order (T3.5.1):** `get_issues` (which now routes through the
/// batched `hydrate_ids`) drops an id whose row is absent and keeps the CALLER's order — not the DB
/// row order (a reconstruction from the `IN`-result rows would reorder them) and never a panic /
/// null hole for the missing id.
pub async fn contract_get_issues_skip_absent_order<S: Storage>(storage: S) {
    for id in ["ub-1", "ub-2", "ub-3"] {
        storage
            .create_issue(&issue(id, id), "a")
            .await
            .expect("create");
    }

    // Caller order deliberately NON-ascending, with a missing id interleaved.
    let got = storage
        .get_issues(&[
            "ub-3".to_string(),
            "ub-missing".to_string(),
            "ub-1".to_string(),
            "ub-2".to_string(),
        ])
        .await
        .expect("get_issues");
    let ids: Vec<&str> = got.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(
        ids,
        ["ub-3", "ub-1", "ub-2"],
        "missing id skipped; survivors keep the caller's order (not the DB row order)"
    );
}

// --------------------------------------------------------------------------------------------------
// Dependencies
// --------------------------------------------------------------------------------------------------

/// `add_dependency` writes `Event(DependencyAdded)`, rejects self/duplicate edges.
pub async fn contract_add_dependency<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-a", "a"), "x")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-b", "b"), "x")
        .await
        .unwrap();

    storage
        .add_dependency(&dep("ub-a", "ub-b", DependencyType::Blocks), "x")
        .await
        .expect("add edge");
    assert!(
        event_types(&storage, "ub-a")
            .await
            .contains(&"dependency_added".to_string())
    );

    // Self-dependency rejected.
    assert!(matches!(
        storage
            .add_dependency(&dep("ub-a", "ub-a", DependencyType::Blocks), "x")
            .await,
        Err(StorageError::SelfDependency)
    ));
    // Duplicate edge rejected.
    assert!(matches!(
        storage
            .add_dependency(&dep("ub-a", "ub-b", DependencyType::Blocks), "x")
            .await,
        Err(StorageError::DuplicateDependency)
    ));
}

/// `remove_dependency` deletes the edge + writes `Event(DependencyRemoved)`; a missing edge errors.
pub async fn contract_remove_dependency<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-a", "a"), "x")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-b", "b"), "x")
        .await
        .unwrap();
    storage
        .add_dependency(&dep("ub-a", "ub-b", DependencyType::Blocks), "x")
        .await
        .unwrap();

    storage
        .remove_dependency("ub-a", "ub-b", &DependencyType::Blocks, "x")
        .await
        .expect("remove edge");
    assert!(
        storage.list_dependencies("ub-a").await.unwrap().is_empty(),
        "edge removed"
    );
    assert!(
        event_types(&storage, "ub-a")
            .await
            .contains(&"dependency_removed".to_string())
    );

    // Removing a non-existent edge errors.
    assert!(matches!(
        storage
            .remove_dependency("ub-a", "ub-b", &DependencyType::Blocks, "x")
            .await,
        Err(StorageError::DependencyNotFound)
    ));
}

/// `list_dependencies` returns the edges declared *by* an id.
pub async fn contract_list_dependencies<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-a", "a"), "x")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-b", "b"), "x")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-c", "c"), "x")
        .await
        .unwrap();
    storage
        .add_dependency(&dep("ub-a", "ub-b", DependencyType::Blocks), "x")
        .await
        .unwrap();
    storage
        .add_dependency(&dep("ub-a", "ub-c", DependencyType::Related), "x")
        .await
        .unwrap();

    let deps = storage.list_dependencies("ub-a").await.unwrap();
    assert_eq!(deps.len(), 2);
    let targets: Vec<String> = deps.into_iter().map(|d| d.depends_on_id).collect();
    assert!(targets.contains(&"ub-b".to_string()));
    assert!(targets.contains(&"ub-c".to_string()));
}

/// `dependency_tree` returns the subtree rooted at the requested id.
pub async fn contract_dependency_tree<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-a", "a"), "x")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-b", "b"), "x")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-c", "c"), "x")
        .await
        .unwrap();
    storage
        .add_dependency(&dep("ub-a", "ub-b", DependencyType::Blocks), "x")
        .await
        .unwrap();
    storage
        .add_dependency(&dep("ub-b", "ub-c", DependencyType::Blocks), "x")
        .await
        .unwrap();

    let tree = storage.dependency_tree("ub-a").await.unwrap();
    assert_eq!(tree.root, "ub-a");
    // Reaches a -> b and b -> c transitively.
    assert!(
        tree.edges
            .iter()
            .any(|e| e.from == "ub-a" && e.to == "ub-b")
    );
    assert!(
        tree.edges
            .iter()
            .any(|e| e.from == "ub-b" && e.to == "ub-c")
    );
}

/// `dependency_graph(&[])` returns the **whole** graph (every edge).
pub async fn contract_dependency_graph_whole<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-a", "a"), "x")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-b", "b"), "x")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-c", "c"), "x")
        .await
        .unwrap();
    storage
        .add_dependency(&dep("ub-a", "ub-b", DependencyType::Blocks), "x")
        .await
        .unwrap();
    storage
        .add_dependency(&dep("ub-c", "ub-b", DependencyType::Related), "x")
        .await
        .unwrap();

    let whole = storage.dependency_graph(&[]).await.unwrap();
    assert_eq!(whole.edges.len(), 2, "empty roots = whole graph");
    assert!(
        whole
            .edges
            .iter()
            .any(|e| e.from == "ub-a" && e.to == "ub-b")
    );
    assert!(
        whole
            .edges
            .iter()
            .any(|e| e.from == "ub-c" && e.to == "ub-b")
    );
}

/// `detect_cycles` (generic, non-seam): `[]` on an acyclic graph; `add_dependency` rejects a
/// would-be gating cycle with a path. The **positive stored-cycle** path is the seam case.
pub async fn contract_detect_cycles_generic<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-a", "a"), "x")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-b", "b"), "x")
        .await
        .unwrap();
    storage
        .add_dependency(&dep("ub-a", "ub-b", DependencyType::Blocks), "x")
        .await
        .unwrap();

    // Acyclic graph → no cycles (and the call terminates), for both blocking-only views.
    assert!(
        storage.detect_cycles(true).await.unwrap().is_empty(),
        "acyclic gating graph yields no cycles"
    );
    assert!(
        storage.detect_cycles(false).await.unwrap().is_empty(),
        "acyclic all-types graph yields no cycles"
    );

    // The reverse gating edge would close a cycle → rejected with the REAL ordered path naming both
    // endpoints (`ub-b -> ub-a -> ub-b`), NOT a synthetic placeholder.
    match storage
        .add_dependency(&dep("ub-b", "ub-a", DependencyType::Blocks), "x")
        .await
    {
        Err(StorageError::CycleDetected { path }) => {
            assert!(
                path.contains("ub-a") && path.contains("ub-b"),
                "cycle path names both nodes: {path}"
            );
            let nodes: Vec<&str> = path.split(" -> ").collect();
            assert!(
                nodes.len() >= 3 && nodes.first() == nodes.last(),
                "the path is an ordered cycle `[start, …, start]`: {path}"
            );
        }
        other => panic!("expected CycleDetected, got {other:?}"),
    }

    // A non-gating edge (Related) never gates ready work — the reverse edge is fine.
    storage
        .add_dependency(&dep("ub-b", "ub-a", DependencyType::Related), "x")
        .await
        .expect("related edges never cycle");
    assert!(
        storage.detect_cycles(true).await.unwrap().is_empty(),
        "a related back-edge does not create a gating cycle"
    );
}

/// `detect_cycles` **positive** path (seam-backed): plant a raw gating cycle via the testkit seam
/// (bypassing the public cycle guard), then assert `detect_cycles` returns a path containing the
/// nodes.
pub async fn contract_detect_cycles_positive<S: Storage + StorageTestkit>(storage: S) {
    storage
        .create_issue(&issue("ub-a", "a"), "x")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-b", "b"), "x")
        .await
        .unwrap();

    // a -> b through the public API (valid, acyclic so far).
    storage
        .add_dependency(&dep("ub-a", "ub-b", DependencyType::Blocks), "x")
        .await
        .unwrap();
    // b -> a planted RAW (the public add_dependency would reject this with CycleDetected).
    storage
        .testkit_insert_raw_edge(&dep("ub-b", "ub-a", DependencyType::Blocks))
        .await
        .expect("raw edge planted");

    let cycles = storage.detect_cycles(true).await.unwrap();
    assert!(!cycles.is_empty(), "the stored gating cycle is detected");
    let names: std::collections::HashSet<&str> = cycles
        .iter()
        .flat_map(|path| path.iter().map(String::as_str))
        .collect();
    assert!(
        names.contains("ub-a") && names.contains("ub-b"),
        "the detected cycle path contains both nodes: {cycles:?}"
    );
    // The witness is an ordered cycle `[start, …, start]` (the start repeated at the end), NOT a
    // sorted node set: for this 2-cycle it is `[a, b, a]` (D3).
    assert!(
        cycles.iter().any(|w| w.len() == 3 && w.first() == w.last()),
        "the witness is an ordered `[start, …, start]` cycle: {cycles:?}"
    );
}

// --------------------------------------------------------------------------------------------------
// Events — the §3.2.1 ORDER oracle
// --------------------------------------------------------------------------------------------------

/// `list_events` is the §3.2.1 `EventType` **ORDER** oracle: a create + `update(title)` +
/// `update(status -> closed)` yields exactly `[Created, Updated, StatusChanged, Closed]`,
/// oldest-first.
pub async fn contract_list_events_order_oracle<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-1", "old title"), "a")
        .await
        .unwrap();
    storage
        .update_issue(
            "ub-1",
            &IssuePatch {
                title: Some("new title".to_string()),
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .unwrap();
    storage
        .update_issue(
            "ub-1",
            &IssuePatch {
                status: Some(Status::Closed),
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .unwrap();

    assert_eq!(
        event_types(&storage, "ub-1").await,
        vec!["created", "updated", "status_changed", "closed"],
        "the §3.2.1 EventType order oracle"
    );
}

// --------------------------------------------------------------------------------------------------
// Diagnostics
// --------------------------------------------------------------------------------------------------

/// `closed_since(None)` returns all closed issues by `closed_at`.
pub async fn contract_closed_since<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-1", "t"), "a")
        .await
        .unwrap();
    storage
        .update_issue(
            "ub-1",
            &IssuePatch {
                status: Some(Status::Closed),
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-2", "open"), "a")
        .await
        .unwrap();

    let closed = storage.closed_since(None).await.unwrap();
    let ids: Vec<String> = closed.into_iter().map(|i| i.id).collect();
    assert_eq!(ids, vec!["ub-1".to_string()], "only the closed issue");
}

/// `orphan_candidates` returns issues whose `external_ref` matches the commit-hash pattern.
pub async fn contract_orphan_candidates<S: Storage>(storage: S) {
    let mut commit = issue("ub-commit", "has commit ref");
    commit.external_ref = Some("a1b2c3d4e5f6".to_string());
    storage.create_issue(&commit, "a").await.unwrap();
    let mut jira = issue("ub-jira", "has jira ref");
    jira.external_ref = Some("jira-1234".to_string());
    storage.create_issue(&jira, "a").await.unwrap();

    let orphans: Vec<String> = storage
        .orphan_candidates()
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert!(orphans.contains(&"ub-commit".to_string()));
    assert!(!orphans.contains(&"ub-jira".to_string()));
}

/// `epic_child_rollup` (D26/T2.7) returns per-epic `(child_total, child_closed_or_tombstone)` over
/// `parent-child` edges — non-template children only, non-`parent-child` edges ignored, id-sorted.
pub async fn contract_epic_child_rollup<S: Storage>(storage: S) {
    // An empty store yields an empty rollup.
    assert!(
        storage.epic_child_rollup().await.unwrap().is_empty(),
        "empty store → empty rollup"
    );

    let mut epic = issue("ub-epic", "epic");
    epic.issue_type = IssueType::Epic;
    storage.create_issue(&epic, "a").await.unwrap();

    // 3 real children: 2 closed / 1 open.
    for (id, closed) in [("ub-c1", true), ("ub-c2", true), ("ub-c3", false)] {
        let mut child = issue(id, "child");
        if closed {
            child.status = Status::Closed;
            child.closed_at = Some(ts(2026, 1, 2));
        }
        storage.create_issue(&child, "a").await.unwrap();
        storage
            .add_dependency(&dep(id, "ub-epic", DependencyType::ParentChild), "a")
            .await
            .unwrap();
    }
    // A TEMPLATE child — EXCLUDED from the rollup (bd's is_template guard).
    let mut tmpl = issue("ub-tmpl", "template child");
    tmpl.is_template = true;
    storage.create_issue(&tmpl, "a").await.unwrap();
    storage
        .add_dependency(&dep("ub-tmpl", "ub-epic", DependencyType::ParentChild), "a")
        .await
        .unwrap();
    // A NON-parent-child edge — IGNORED by the rollup.
    let mut blk = issue("ub-blk", "blocker, wrong edge type");
    blk.status = Status::Closed;
    blk.closed_at = Some(ts(2026, 1, 2));
    storage.create_issue(&blk, "a").await.unwrap();
    storage
        .add_dependency(&dep("ub-blk", "ub-epic", DependencyType::Blocks), "a")
        .await
        .unwrap();

    let rollup = storage.epic_child_rollup().await.unwrap();
    assert_eq!(
        rollup,
        vec![("ub-epic".to_string(), (3, 2))],
        "3 non-template parent-child children, 2 closed (template + Blocks ignored)"
    );
}

// --------------------------------------------------------------------------------------------------
// Cross-cutting invariants
// --------------------------------------------------------------------------------------------------

/// A no-op update (a patch that changes nothing) writes **no `Event`** and leaves `updated_at`.
pub async fn contract_noop_update_writes_no_event<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-1", "same"), "a")
        .await
        .unwrap();
    let before = storage.get_issue("ub-1").await.unwrap().unwrap();

    // Patch the title to its current value: nothing changes.
    let after = storage
        .update_issue(
            "ub-1",
            &IssuePatch {
                title: Some("same".to_string()),
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .unwrap();
    assert_eq!(
        after.updated_at, before.updated_at,
        "no-op update must not move updated_at"
    );
    assert_eq!(
        event_types(&storage, "ub-1").await,
        vec!["created"],
        "no-op update writes no event"
    );
}

/// `DeleteMode::DryRun` mutates nothing and returns the resolved plan (full blast radius).
pub async fn contract_dry_run_mutates_nothing<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-1", "t"), "a")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-1.1", "child"), "a")
        .await
        .unwrap();

    let plan = DeletePlan {
        mode: DeleteMode::DryRun,
        targets: vec!["ub-1".to_string()],
        cascade_children: Vec::new(),
    };
    let resolved = storage.delete_issue(&plan, "admin").await.unwrap();
    assert_eq!(resolved.targets, vec!["ub-1".to_string()]);
    assert!(
        resolved.cascade_children.contains(&"ub-1.1".to_string()),
        "DryRun resolves the cascade children (blast radius)"
    );

    // Nothing mutated.
    let fetched = storage.get_issue("ub-1").await.unwrap().unwrap();
    assert_eq!(fetched.status, Status::Open, "DryRun must not mutate");
    assert_eq!(
        event_types(&storage, "ub-1").await,
        vec!["created"],
        "DryRun writes no event"
    );
}

/// Tombstone preserves `original_type` and writes `Deleted` **only** from a non-terminal status —
/// not from a closed/already-tombstone target.
pub async fn contract_tombstone_preserves_type_and_event_rule<S: Storage>(storage: S) {
    // (a) Tombstone from open (non-terminal): preserves original_type + emits Deleted.
    let mut bug = issue("ub-bug", "a bug");
    bug.issue_type = IssueType::Bug;
    storage.create_issue(&bug, "a").await.unwrap();
    let plan = DeletePlan {
        mode: DeleteMode::Tombstone,
        targets: vec!["ub-bug".to_string()],
        cascade_children: Vec::new(),
    };
    storage.delete_issue(&plan, "admin").await.unwrap();
    let fetched = storage.get_issue("ub-bug").await.unwrap().unwrap();
    assert_eq!(fetched.status, Status::Tombstone);
    assert_eq!(
        fetched.original_type.as_deref(),
        Some("bug"),
        "original_type preserved across tombstone"
    );
    assert!(
        event_types(&storage, "ub-bug")
            .await
            .contains(&"deleted".to_string()),
        "tombstone from non-terminal emits Deleted"
    );

    // (b) Tombstone from CLOSED (terminal): NO Deleted event.
    storage
        .create_issue(&issue("ub-closed", "t"), "a")
        .await
        .unwrap();
    storage
        .update_issue(
            "ub-closed",
            &IssuePatch {
                status: Some(Status::Closed),
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .unwrap();
    let plan = DeletePlan {
        mode: DeleteMode::Tombstone,
        targets: vec!["ub-closed".to_string()],
        cascade_children: Vec::new(),
    };
    storage.delete_issue(&plan, "admin").await.unwrap();
    let deleted_count = event_types(&storage, "ub-closed")
        .await
        .into_iter()
        .filter(|e| e == "deleted")
        .count();
    assert_eq!(deleted_count, 0, "no Deleted event from a terminal status");

    // (c) Re-tombstoning an already-tombstone target is a no-op (no extra Deleted event).
    let before = event_types(&storage, "ub-bug").await;
    let plan = DeletePlan {
        mode: DeleteMode::Tombstone,
        targets: vec!["ub-bug".to_string()],
        cascade_children: Vec::new(),
    };
    storage.delete_issue(&plan, "admin").await.unwrap();
    assert_eq!(
        event_types(&storage, "ub-bug").await,
        before,
        "re-tombstone of an already-tombstone is a no-op (no extra event)"
    );
}

/// Transactional-audit atomicity (FR-9): a mutation that **fails** leaves rows *and* events
/// unchanged — a rejected cycle-forming dependency adds neither the edge nor a `DependencyAdded`
/// event.
pub async fn contract_transactional_audit_atomicity<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-a", "a"), "x")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-b", "b"), "x")
        .await
        .unwrap();
    storage
        .add_dependency(&dep("ub-a", "ub-b", DependencyType::Blocks), "x")
        .await
        .unwrap();

    let events_before = event_types(&storage, "ub-b").await;
    let edges_on_a = storage.list_dependencies("ub-a").await.unwrap().len();
    let edges_on_b = storage.list_dependencies("ub-b").await.unwrap().len();

    // This add would close a gating cycle → rejected; nothing must commit.
    let rejected = storage
        .add_dependency(&dep("ub-b", "ub-a", DependencyType::Blocks), "x")
        .await;
    assert!(matches!(rejected, Err(StorageError::CycleDetected { .. })));

    assert_eq!(
        event_types(&storage, "ub-b").await,
        events_before,
        "the failed mutation wrote no event"
    );
    assert_eq!(
        storage.list_dependencies("ub-a").await.unwrap().len(),
        edges_on_a,
        "no edge added on ub-a"
    );
    assert_eq!(
        storage.list_dependencies("ub-b").await.unwrap().len(),
        edges_on_b,
        "no edge added on ub-b (the rejected direction)"
    );
}

/// `DeleteMode::Cascade` tombstones the target and all dotted-id-prefix descendants (FR-1c).
///
/// The corpus: `ub-1` (target), `ub-1.1`/`ub-1.1.1` (prefix children, Open),
/// `ub-1.2` (prefix child, Closed/terminal), `ub-10` (dot-boundary decoy — shares `ub-1` prefix
/// WITHOUT a dot, must be UNTOUCHED), `ub-2` (unrelated, must be UNTOUCHED).
///
/// Cascade keys on the `{target}.%` dotted-id LIKE pattern (`crud.rs:835`), NOT on edges —
/// so this corpus uses dotted ids without any parent-child edges.
pub async fn contract_cascade_delete<S: Storage>(storage: S) {
    // Build corpus.
    storage
        .create_issue(&issue("ub-1", "root"), "alice")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-1.1", "child"), "alice")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-1.1.1", "grandchild"), "alice")
        .await
        .unwrap();
    // ub-1.2 is Closed (terminal) — tombstone_one must set deleted_by but must NOT emit Deleted.
    storage
        .create_issue(&issue("ub-1.2", "closed-child"), "alice")
        .await
        .unwrap();
    storage
        .update_issue(
            "ub-1.2",
            &IssuePatch {
                status: Some(Status::Closed),
                ..IssuePatch::default()
            },
            "alice",
        )
        .await
        .unwrap();
    // Dot-boundary decoy: shares the "ub-1" string prefix but has no dot after "ub-1".
    storage
        .create_issue(&issue("ub-10", "decoy"), "alice")
        .await
        .unwrap();
    // Unrelated root.
    storage
        .create_issue(&issue("ub-2", "unrelated"), "alice")
        .await
        .unwrap();

    let plan = DeletePlan {
        mode: DeleteMode::Cascade,
        targets: vec!["ub-1".to_string()],
        cascade_children: Vec::new(), // storage resolves this
    };
    let resolved = storage.delete_issue(&plan, "admin").await.unwrap();

    // Resolved plan shows the full blast radius (sorted, non-empty).
    assert_eq!(resolved.mode, DeleteMode::Cascade);
    assert_eq!(resolved.targets, vec!["ub-1".to_string()]);
    assert_eq!(
        resolved.cascade_children,
        vec![
            "ub-1.1".to_string(),
            "ub-1.1.1".to_string(),
            "ub-1.2".to_string()
        ],
        "cascade_children: sorted prefix-descendants; ub-10 excluded (no dot); ub-2 excluded"
    );

    // All four affected issues are Tombstone with original_type preserved.
    for id in ["ub-1", "ub-1.1", "ub-1.1.1", "ub-1.2"] {
        let fetched = storage.get_issue(id).await.unwrap().unwrap();
        assert_eq!(fetched.status, Status::Tombstone, "{id} must be Tombstone");
        assert!(
            fetched.original_type.is_some(),
            "{id} original_type must be preserved"
        );
        assert_eq!(
            fetched.deleted_by.as_deref(),
            Some("admin"),
            "{id} deleted_by must be the actor (including the terminal ub-1.2)"
        );
    }

    // Deleted-event count: 3 non-terminal members each emit one; ub-1.2 (was Closed) emits zero.
    for id in ["ub-1", "ub-1.1", "ub-1.1.1"] {
        assert!(
            event_types(&storage, id)
                .await
                .contains(&"deleted".to_string()),
            "{id} (non-terminal) must have a Deleted event"
        );
    }
    let ub12_deleted = event_types(&storage, "ub-1.2")
        .await
        .into_iter()
        .filter(|e| e == "deleted")
        .count();
    assert_eq!(
        ub12_deleted, 0,
        "ub-1.2 (was Closed/terminal) must have zero Deleted events"
    );

    // Bounded blast radius: dot-boundary decoy and unrelated root are UNTOUCHED.
    let decoy = storage.get_issue("ub-10").await.unwrap().unwrap();
    assert_eq!(
        decoy.status,
        Status::Open,
        "ub-10 (dot-boundary decoy) must be untouched"
    );
    assert!(decoy.deleted_by.is_none());

    let unrelated = storage.get_issue("ub-2").await.unwrap().unwrap();
    assert_eq!(
        unrelated.status,
        Status::Open,
        "ub-2 (unrelated) must be untouched"
    );
    assert!(unrelated.deleted_by.is_none());
}

/// `DeleteMode::Hard` permanently removes the target row + FK-child rows + inbound dep rows —
/// but child *issue rows* identified by the dotted-id prefix SURVIVE (Hard ≠ Cascade).
///
/// `issues` has NO self-referential parent FK (`schema.rs:34-78`). The `Hard` arm in `crud.rs:731`
/// takes `_ => targets.clone()` — it does NOT extend with `cascade_children`. Only the target's own
/// child rows (labels/deps/events via `issue_id` FK CASCADE) + inbound `depends_on_id` dep rows
/// (explicit DELETE, `crud.rs:741-746`) are removed.
///
/// `PRAGMA foreign_keys = ON` is verified by `pragmas_readback_in_memory` (mod.rs:702-727) and
/// `foreign_keys_enforced` (mod.rs:853); the FK CASCADE assertion here relies on that.
pub async fn contract_hard_delete<S: Storage>(storage: S) {
    // ub-1: target; ub-1.1: child issue (dotted id — must survive); ub-2: unrelated.
    storage
        .create_issue(&issue("ub-1", "target"), "alice")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-1.1", "child"), "alice")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-2", "unrelated"), "alice")
        .await
        .unwrap();

    // Outbound edge: ub-1 --blocks--> ub-3 (ub-1's issue_id dep row — FK CASCADE removes it).
    storage
        .create_issue(&issue("ub-3", "out-target"), "alice")
        .await
        .unwrap();
    storage
        .add_dependency(&dep("ub-1", "ub-3", DependencyType::Blocks), "alice")
        .await
        .unwrap();

    // Inbound edge: ub-4 --blocks--> ub-1 (depends_on_id = "ub-1", NO FK — explicit DELETE).
    storage
        .create_issue(&issue("ub-4", "in-blocker"), "alice")
        .await
        .unwrap();
    storage
        .add_dependency(&dep("ub-4", "ub-1", DependencyType::Blocks), "alice")
        .await
        .unwrap();

    let plan = DeletePlan {
        mode: DeleteMode::Hard,
        targets: vec!["ub-1".to_string()],
        cascade_children: Vec::new(),
    };
    storage.delete_issue(&plan, "admin").await.unwrap();

    // (1) ub-1 row is gone.
    assert!(
        storage.get_issue("ub-1").await.unwrap().is_none(),
        "Hard delete must remove the target row"
    );

    // (2) Inbound orphan cleaned: ub-4's dep list has no depends_on_id="ub-1".
    let ub4_deps = storage.list_dependencies("ub-4").await.unwrap();
    assert!(
        !ub4_deps.iter().any(|d| d.depends_on_id == "ub-1"),
        "inbound dep from ub-4 to ub-1 must be cleaned by explicit DELETE"
    );

    // (3) FK CASCADE: ub-1's OWN dep rows (issue_id="ub-1") are gone.
    let ub1_deps = storage.list_dependencies("ub-1").await.unwrap();
    assert!(
        ub1_deps.is_empty(),
        "ub-1's own dep rows gone via FK CASCADE"
    );

    // (4) Global no-orphan: whole graph has no edge mentioning ub-1.
    let graph = storage.dependency_graph(&[]).await.unwrap();
    assert!(
        !graph
            .edges
            .iter()
            .any(|e| e.from == "ub-1" || e.to == "ub-1"),
        "no edge may reference the deleted ub-1"
    );

    // (5) Hard ≠ Cascade: child *issue rows* survive (issues table has NO self-parent FK).
    assert!(
        storage.get_issue("ub-1.1").await.unwrap().is_some(),
        "ub-1.1 (child issue row) SURVIVES Hard delete — no self-parent FK"
    );
    assert!(
        storage.get_issue("ub-2").await.unwrap().is_some(),
        "ub-2 (unrelated) SURVIVES"
    );
    assert!(
        storage.get_issue("ub-3").await.unwrap().is_some(),
        "ub-3 (outbound target) SURVIVES"
    );
    assert!(
        storage.get_issue("ub-4").await.unwrap().is_some(),
        "ub-4 (inbound blocker) SURVIVES"
    );

    // (6) Hard writes no spurious Deleted event on a SURVIVING issue (S9): tombstone_one is never
    //     called (Hard takes the row-delete path). ub-1's OWN events are unobservable post-delete
    //     (its events rows are gone via FK CASCADE, not by writing a Deleted event), so we only
    //     assert what is checkable: ub-4 (which survives) has no "deleted" event from this Hard op.
    assert!(
        !event_types(&storage, "ub-4")
            .await
            .contains(&"deleted".to_string()),
        "Hard delete must NOT emit a Deleted event on any surviving issue"
    );
}

/// Seam-backed: the id child-counter high-water mark advances monotonically past the max child
/// segment created through the public `create_issue`.
pub async fn contract_child_counter_high_water<S: Storage + StorageTestkit>(storage: S) {
    storage
        .create_issue(&issue("ub-root", "root"), "a")
        .await
        .unwrap();

    // No child yet → high-water None.
    assert_eq!(
        storage.testkit_child_high_water("ub-root").await.unwrap(),
        None,
        "no child allocated yet"
    );

    // Create hierarchical children 1, 2, 3 through the public API.
    let mut last_hw = 0u32;
    for n in 1..=3u32 {
        storage
            .create_issue(&issue(&format!("ub-root.{n}"), &format!("child {n}")), "a")
            .await
            .unwrap();
        let hw = storage
            .testkit_child_high_water("ub-root")
            .await
            .unwrap()
            .expect("a child now exists");
        assert!(
            hw >= n,
            "high-water ({hw}) must reach the max child segment created ({n})"
        );
        assert!(hw >= last_hw, "high-water advances monotonically");
        last_hw = hw;
    }
    assert!(
        last_hw >= 3,
        "high-water ({last_hw}) advanced past the max child segment (3)"
    );
}

/// `next_child_number` (the PRODUCTION trait read-half the engine allocator consumes, D21) returns
/// the high-water mark + 1, advancing as children are created through the public `create_issue`.
///
/// Distinct from the testkit-only `testkit_child_high_water` seam: this reaches the same body via the
/// public trait surface (so a backend's production wiring is exercised, not just the test seam).
pub async fn contract_next_child_number<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-root", "root"), "a")
        .await
        .unwrap();

    // No child yet → the first child number is 1.
    assert_eq!(
        storage.next_child_number("ub-root").await.unwrap(),
        1,
        "the first child of a fresh parent is number 1"
    );

    // Create child .1, then .2 through the public API; next_child_number advances past each.
    storage
        .create_issue(&issue("ub-root.1", "child 1"), "a")
        .await
        .unwrap();
    assert_eq!(
        storage.next_child_number("ub-root").await.unwrap(),
        2,
        "after child .1, the next free child number is 2"
    );

    storage
        .create_issue(&issue("ub-root.2", "child 2"), "a")
        .await
        .unwrap();
    assert_eq!(
        storage.next_child_number("ub-root").await.unwrap(),
        3,
        "after child .2, the next free child number is 3"
    );

    // An unknown parent has no counter → its first child is 1 (never panics on a missing parent).
    assert_eq!(
        storage.next_child_number("ub-unknown").await.unwrap(),
        1,
        "an unseen parent's first child number is 1"
    );
}

// --------------------------------------------------------------------------------------------------
// Corpus seeder (T3.5/D34 — the shared perf/scale fixture)
// --------------------------------------------------------------------------------------------------

/// Issues inserted per atomic [`Storage::create_issues`] transaction while seeding a large corpus.
///
/// One `BEGIN IMMEDIATE` tx per chunk (spine §3.2.1). ~1k rows/tx keeps each transaction bounded
/// while amortising the per-tx overhead across the 250k-issue NFR-2 corpus (D34).
const SEED_CHUNK: usize = 1_000;

/// Seed `n` synthetic-but-valid issues via batched atomic [`Storage::create_issues`] (~1k rows/tx).
///
/// The **one** corpus seeder shared by `benches/storage.rs`, `tests/scale.rs`, and the
/// `unblock-engine` scale/bench suites (T3.5/D34). It is the **storage-direct, validated-but-
/// non-minted** NFR-2 path Miguel sanctioned under the never-simplify rule (D34/F-2): each issue is
/// built with a unique adaptive id (`ub-<i>`, all-digit hash — [`unblock_model::parse_id`] valid) at
/// a fixed epoch and passed through [`IssueValidator::validate`] **before** the batch, because the
/// atomic bulk primitive deliberately does no validation/mint of its own (the engine
/// `Session::create_bulk` normally validates first). Bypassing the O(N²) engine mint validates the
/// same rows without the quadratic build cost.
///
/// Insertion flows through the production [`Storage::create_issues`] write path, so for the libsql
/// backend the passive WAL-checkpoint cadence (`CHECKPOINT_EVERY_N_MUTATIONS`) fires on the held
/// write connection as the committed chunk-txs accumulate, keeping the `-wal` sidecar bounded across
/// the whole seed (the contention-lab precedent — no unbounded WAL growth even at 250k).
///
/// # Errors
///
/// Propagates any [`StorageError`] from the underlying [`Storage::create_issues`] (e.g. a backend
/// failure). A seeded id never collides because ids are a dense unique sequence.
///
/// # Panics
///
/// Panics if a generated issue fails [`IssueValidator::validate`] — a seeder-construction invariant
/// that can never trip at runtime (this module is a test/bench harness), so surfacing it as a panic
/// keeps the `-> Result<_, StorageError>` signature reserved for genuine backend errors.
pub async fn seed_corpus<S: Storage>(storage: &S, n: usize) -> Result<(), StorageError> {
    let created = ts(2026, 1, 1);
    let mut chunk: Vec<Issue> = Vec::with_capacity(SEED_CHUNK.min(n));
    let mut start = 0usize;
    while start < n {
        let end = (start + SEED_CHUNK).min(n);
        chunk.clear();
        for i in start..end {
            let issue = seed_issue(i, created);
            IssueValidator::validate(&issue).expect("seed_corpus builds only valid issues");
            chunk.push(issue);
        }
        storage.create_issues(&chunk, "seed").await?;
        start = end;
    }
    Ok(())
}

/// Build the `i`-th synthetic seed issue: a unique `ub-<i>` id (zero-padded so the all-digit hash is
/// a syntactically valid [`unblock_model::parse_id`] id) at the fixed seed epoch.
fn seed_issue(i: usize, created: DateTime<Utc>) -> Issue {
    Issue {
        id: format!("ub-{i:07}"),
        title: format!("seed issue {i}"),
        created_at: created,
        updated_at: created,
        ..Issue::default()
    }
}
