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
//! Three contract properties are not reachable through the public surface alone:
//!
//! - [`Storage::detect_cycles`](crate::Storage::detect_cycles)' **positive** path — the public
//!   [`add_dependency`](crate::Storage::add_dependency) *rejects* a gating-cycle edge with
//!   `CycleDetected`, so a stored cycle can only be planted by bypassing that guard (since D45 the
//!   same seam is what plants a DANGLING edge, which every write path now refuses);
//! - the **id child-counter high-water mark**, an internal allocation invariant; and
//! - the **SQL half of the `external:` predicate** (D45) — the write guard calls the Rust
//!   [`unblock_model::is_external_target`] while the ready/blocked queries carry
//!   `NOT LIKE 'external:%'`, and only the backend can answer for its own half.
//!
//! [`StorageTestkit`] exposes exactly those seams, gated behind
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
    Comment, CountGroupBy, Dependency, DependencyType, Issue, IssueType, IssueValidator,
    ListFilters, Priority, Status,
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

    /// Ask the BACKEND ITSELF whether `probe` matches the SQL external-target twin — for `SQLite`,
    /// literally `SELECT ?1 LIKE 'external:%'` (D45, spine §1.9 invariant 3).
    ///
    /// The `external:` predicate has TWO homes by construction: the Rust
    /// [`unblock_model::is_external_target`] the write guard calls, and the `NOT LIKE 'external:%'`
    /// predicates the ready/blocked queries carry — because SQL cannot call Rust. They agree only BY
    /// CONTRACT, and this seam is what turns that contract into a red test: the suite asserts the two
    /// verdicts are equal over a probe corpus pinned in the spine. Without it the suite could only
    /// assert the Rust half against itself, which proves nothing about the divergence D45 exists to
    /// abolish. A backend whose external-target SQL is spelled differently answers with ITS OWN
    /// predicate — the obligation is the equivalence, not the syntax.
    async fn testkit_sql_matches_external_prefix(&self, probe: &str) -> Result<bool, StorageError>;

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
    // ub-lp9.27 — the label diff base is the issue's ACTUAL label set (see the block comment above
    // `contract_label_remove_then_read`).
    contract_label_remove_then_read(factory().await).await;
    contract_label_add_existing_is_idempotent(factory().await).await;
    contract_labels_set_overlap_replaces(factory().await).await;
    contract_label_change_updated_at_semantics(factory().await).await;
    contract_label_diff_base_is_per_issue(factory().await).await;
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

    // Comments (FR-6, D37).
    contract_add_comment(factory().await).await;
    contract_list_comments_order(factory().await).await;
    contract_update_comment_provenance(factory().await).await;
    contract_delete_comment_soft_redact(factory().await).await;
    contract_seed_comments_round_trip_state(factory().await).await;

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

    // Dependency edge fields (D42).
    contract_add_dependency_persists_metadata(factory().await).await;
    contract_create_issue_persists_dep_metadata(factory().await).await;
    contract_dep_metadata_null_vs_empty_object(factory().await).await;
    contract_dep_thread_id_round_trip(factory().await).await;

    // D44 — the create-with-deps contract: implicit edge ownership, the create-specific guards,
    // and the BULK/IMPORT non-regression that proves those guards did NOT move into the shared body.
    contract_create_issue_reanchors_a_foreign_edge_source(factory().await).await;
    contract_create_issue_rejects_a_duplicate_declared_edge(factory().await).await;
    contract_create_issue_rejects_a_declared_gating_cycle(factory().await).await;
    contract_create_issues_still_dedups_and_still_admits_a_cycle(factory().await).await;

    // D45 — the dependency-TARGET existence guard, on every L2 edge-writing body, plus the
    // batch-awareness the import leg depends on, the external/tombstone carve-outs, the published
    // precedence chain, the L2 read the `dangling` diagnostic composes, and the §1.9 Rust/SQL
    // equivalence obligation.
    contract_create_issue_refuses_a_phantom_target(factory().await).await;
    contract_create_issues_refuses_a_phantom_target_whole_batch(factory().await).await;
    contract_create_issues_accepts_an_intra_batch_reference_in_both_directions(factory().await)
        .await;
    contract_add_dependency_guards_both_endpoints(factory().await).await;
    contract_reparent_refuses_a_phantom_parent(factory().await).await;
    contract_external_target_is_accepted_on_every_write_path(factory().await).await;
    contract_tombstoned_target_counts_as_existing(factory().await).await;
    contract_blocker_not_found_precedence(factory().await).await;
    contract_dangling_duplicate_edge_reports_the_missing_target(factory().await).await;
    contract_dangling_edges_survive_the_whole_graph_read(factory().await).await;
    contract_dangling_dependencies_lists_only_absent_targets(factory().await).await;
    contract_external_predicate_matches_the_sql_twin(factory().await).await;

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

/// A comment fixture at the fixed epoch (a LIVE comment: never edited, never redacted).
fn comment(id: i64, issue_id: &str, author: &str, body: &str) -> Comment {
    Comment {
        id,
        issue_id: issue_id.to_string(),
        author: author.to_string(),
        body: body.to_string(),
        created_at: ts(2026, 1, 1),
        updated_at: None,
        redacted_at: None,
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

/// Collect the `(event_type, old_value, new_value)` triples for an issue, oldest first.
///
/// The type-only [`event_types`] cannot tell WHICH label an event carries — the label cases need the
/// payload to prove the event announces the actual delta and not merely "something labelly happened".
async fn event_triples<S: Storage>(
    storage: &S,
    id: &str,
) -> Vec<(String, Option<String>, Option<String>)> {
    storage
        .list_events(id)
        .await
        .expect("list_events")
        .into_iter()
        .map(|e| (e.event_type.as_str().to_string(), e.old_value, e.new_value))
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
///
/// A row-column change is **not** the only trigger for the `updated_at` stamp: a real RELATION change
/// stamps it too — a reparent (FR-1b) and, since ub-lp9.27, a real label add/remove/set (spine §3.2.1
/// `update_issue`). The label half lives in the four `contract_label_*` cases below; the empty-diff
/// **full skip** (no `SET`, no `updated_at`, no `Event`) is unchanged and is pinned by
/// [`contract_noop_update_writes_no_event`] and by
/// [`contract_label_change_updated_at_semantics`]' second half.
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

// --- ub-lp9.27 — the label diff base is the issue's ACTUAL label set ------------------------------
//
// `update_issue` diffs the requested label ops against the labels the issue REALLY carries (the
// update transaction reads them from the labels relation before diffing). A backend that diffs
// against an empty base passes none of the cases below: a remove silently no-ops, an add of an
// already-present label re-inserts a row that already exists, and a `labels_set` overlapping the
// current set fails the same way instead of replacing.
//
// The base is the ISSUE's label set, not the workspace's — `contract_label_diff_base_is_per_issue`
// is the case that grades that scoping, and it is the only one whose fixture carries a second
// labelled issue while the label ops run (a workspace-wide base passes all the others).

/// `labels_remove` on a label the issue ACTUALLY carries removes it — from the returned issue AND
/// from a fresh read — and announces it with exactly one `LabelRemoved` carrying that label.
///
/// The fresh re-read is the load-bearing half: the response alone cannot distinguish a real removal
/// from a silent skip, because `update_issue` re-hydrates the issue after the transaction, so a
/// skipped patch still returns a perfectly well-formed (stale) label set with `isError:false`.
pub async fn contract_label_remove_then_read<S: Storage>(storage: S) {
    let mut seeded = issue("ub-1", "labelled");
    seeded.labels = vec!["alpha".to_string(), "beta".to_string()];
    storage.create_issue(&seeded, "a").await.expect("create");

    let after = storage
        .update_issue(
            "ub-1",
            &IssuePatch {
                labels_remove: vec!["alpha".to_string()],
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .expect("labels_remove of a present label");
    assert_eq!(
        after.labels,
        vec!["beta".to_string()],
        "labels_remove must drop the label from the returned issue"
    );

    let reread = storage
        .get_issue("ub-1")
        .await
        .expect("get_issue")
        .expect("present");
    assert_eq!(
        reread.labels,
        vec!["beta".to_string()],
        "the removed label must be GONE from a FRESH read, not just from the response"
    );

    // Exactly one LabelRemoved, carrying the removed label in `old_value` (the create emitted the two
    // LabelAdded events; a single removal keeps this slice order-independent by construction).
    let removed: Vec<Option<String>> = event_triples(&storage, "ub-1")
        .await
        .into_iter()
        .filter(|(kind, _, _)| kind == "label_removed")
        .map(|(_, old, _)| old)
        .collect();
    assert_eq!(
        removed,
        vec![Some("alpha".to_string())],
        "exactly one LabelRemoved, naming the label actually removed"
    );
}

/// `labels_add` of an **already-present** label is an idempotent no-op: `Ok`, no event, and
/// `updated_at` does not move.
///
/// Against an empty diff base the label is seen as new, so the reconcile re-INSERTs a row that
/// already exists and the `labels` uniqueness constraint turns a harmless idempotent call into a
/// backend error surfaced to the caller.
pub async fn contract_label_add_existing_is_idempotent<S: Storage>(storage: S) {
    let mut seeded = issue("ub-1", "labelled");
    seeded.labels = vec!["alpha".to_string()];
    storage.create_issue(&seeded, "a").await.expect("create");

    let before = storage
        .get_issue("ub-1")
        .await
        .expect("get_issue")
        .expect("present");
    let events_before = event_types(&storage, "ub-1").await;

    let after = storage
        .update_issue(
            "ub-1",
            &IssuePatch {
                labels_add: vec!["alpha".to_string()],
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .expect("adding an ALREADY-PRESENT label must be an idempotent Ok, never a backend error");

    assert_eq!(
        after.labels,
        vec!["alpha".to_string()],
        "the label set is unchanged (never duplicated)"
    );
    assert_eq!(
        after.updated_at, before.updated_at,
        "an idempotent label add changes nothing, so it must NOT stamp updated_at"
    );
    assert_eq!(
        event_types(&storage, "ub-1").await,
        events_before,
        "an idempotent label add writes NO LabelAdded event"
    );
}

/// `labels_set` over a set that OVERLAPS the current one performs the replacement, and emits events
/// for the actual deltas only — the label the two sets share is neither re-added nor re-announced.
///
/// Second half: a combined label + scalar patch, which pins the ONE ordering the contract promises —
/// the label events are appended while the relation is reconciled, so they precede the scalar
/// per-field events. **What this case deliberately does NOT pin is the order of the label events
/// among THEMSELVES** (spine §3.2.1 `update_issue`: the diff is a set, so that relative order is
/// explicitly not guaranteed). Both halves therefore compare the label events as a SET. The libsql
/// backend's own deterministic order — removals reconciled before additions — is a statement about
/// one backend, pinned in `unblock-storage/tests/behaviour.rs`
/// (`labels_reconcile_removals_before_additions`), never here: a conformance suite that graded it
/// would fail a fully conforming backend that reconciled additions first.
pub async fn contract_labels_set_overlap_replaces<S: Storage>(storage: S) {
    let mut seeded = issue("ub-1", "labelled");
    seeded.labels = vec!["alpha".to_string(), "beta".to_string()];
    storage.create_issue(&seeded, "a").await.expect("create");
    let events_before = event_triples(&storage, "ub-1").await.len();

    let after = storage
        .update_issue(
            "ub-1",
            &IssuePatch {
                labels_set: Some(vec!["beta".to_string(), "gamma".to_string()]),
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .expect("labels_set over an OVERLAPPING set");
    assert_eq!(
        after.labels,
        vec!["beta".to_string(), "gamma".to_string()],
        "labels_set replaces the whole set (sorted), keeping the shared label"
    );
    let reread = storage
        .get_issue("ub-1")
        .await
        .expect("get_issue")
        .expect("present");
    assert_eq!(
        reread.labels,
        vec!["beta".to_string(), "gamma".to_string()],
        "the replacement is durable, not just reflected in the response"
    );

    // Exactly ONE delta per side (alpha out, gamma in). `beta` — present before and after — is
    // silent. Compared as a SET (canonicalised by sorting) rather than as a sequence: the contract
    // does not order the label events of one patch relative to each other. The comparison stays
    // exact in every other respect — the LENGTH, both event types and both payloads are pinned, so a
    // spurious extra event or a re-announced `beta` still fails.
    let mut new_events: Vec<(String, Option<String>, Option<String>)> =
        event_triples(&storage, "ub-1")
            .await
            .split_off(events_before);
    new_events.sort();
    assert_eq!(
        new_events,
        vec![
            ("label_added".to_string(), None, Some("gamma".to_string())),
            ("label_removed".to_string(), Some("alpha".to_string()), None),
        ],
        "labels_set announces the DELTAS only — the shared label is not re-announced"
    );

    // Combined label + scalar patch: the label events precede the scalar `Updated`.
    let mut combined = issue("ub-2", "before");
    combined.labels = vec!["keep".to_string()];
    storage.create_issue(&combined, "a").await.expect("create");
    let events_before = event_types(&storage, "ub-2").await.len();
    storage
        .update_issue(
            "ub-2",
            &IssuePatch {
                title: Some("after".to_string()),
                labels_add: vec!["added".to_string()],
                labels_remove: vec!["keep".to_string()],
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .expect("combined label + scalar patch");
    let new_events: Vec<String> = event_types(&storage, "ub-2").await.split_off(events_before);
    assert_eq!(
        new_events.len(),
        3,
        "one LabelRemoved + one LabelAdded + one Updated, and nothing else"
    );
    // The SCALAR event is pinned LAST — THAT half of the order IS guaranteed (label events are
    // appended with the relation reconcile, before the scalar per-field events).
    assert_eq!(
        new_events[2], "updated",
        "the scalar field event is LAST: label events are appended with the relation reconcile, \
         BEFORE the scalar field events"
    );
    // The two LABEL events are compared as a SET — their relative order is not guaranteed.
    let mut label_half = new_events[..2].to_vec();
    label_half.sort();
    assert_eq!(
        label_half,
        vec!["label_added".to_string(), "label_removed".to_string()],
        "both label deltas are announced before the scalar event; their order among themselves is \
         deliberately NOT graded here (the diff is a set — spine §3.2.1)"
    );
}

/// `updated_at` semantics for labels: a **real** label-only change advances it (the relation is a
/// genuine modification of the issue, exactly as a reparent is — spine §3.2.1 `update_issue`); a
/// label **no-op** patch changes nothing and must leave it untouched.
///
/// `content_hash` is unaffected either way — spine §1.8 excludes relations from the hash — so the
/// stored hash stays byte-identical across a label change while `updated_at` moves.
pub async fn contract_label_change_updated_at_semantics<S: Storage>(storage: S) {
    let mut seeded = issue("ub-1", "labelled");
    seeded.labels = vec!["alpha".to_string()];
    storage.create_issue(&seeded, "a").await.expect("create");
    let before = storage
        .get_issue("ub-1")
        .await
        .expect("get_issue")
        .expect("present");
    // Event count AFTER the create, so the assertions below speak only about the two patches (the
    // create's own event order is a different case's business).
    let events_after_create = event_types(&storage, "ub-1").await.len();

    // (a) A genuine label-only change: no row column in the patch at all, yet updated_at advances.
    let changed = storage
        .update_issue(
            "ub-1",
            &IssuePatch {
                labels_add: vec!["beta".to_string()],
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .expect("label-only add");
    assert_eq!(
        changed.labels,
        vec!["alpha".to_string(), "beta".to_string()],
        "the label landed"
    );
    assert!(
        changed.updated_at > before.updated_at,
        "a REAL label-only change advances updated_at"
    );
    assert_eq!(
        changed.content_hash, before.content_hash,
        "labels are excluded from content_hash (spine §1.8), so the hash must NOT move"
    );
    assert_eq!(
        changed.content_hash.as_deref(),
        Some(changed.compute_content_hash().as_str()),
        "the stored hash still matches the recompute after the label change"
    );
    assert_eq!(
        event_types(&storage, "ub-1")
            .await
            .split_off(events_after_create),
        vec!["label_added".to_string()],
        "the real label-only change emits exactly one LabelAdded"
    );
    let events_after_change = event_types(&storage, "ub-1").await.len();

    // (b) A label NO-OP patch (re-add a present label, remove an absent one) is the empty-diff full
    // skip: EXACT equality, because a `>=` cannot detect a spurious stamp.
    let noop = storage
        .update_issue(
            "ub-1",
            &IssuePatch {
                labels_add: vec!["beta".to_string()],
                labels_remove: vec!["never-present".to_string()],
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .expect("label no-op patch");
    assert_eq!(
        noop.updated_at, changed.updated_at,
        "a label no-op patch must NOT move updated_at (empty diff = full skip)"
    );
    assert_eq!(
        noop.labels,
        vec!["alpha".to_string(), "beta".to_string()],
        "a label no-op patch leaves the set alone"
    );
    assert_eq!(
        event_types(&storage, "ub-1").await.len(),
        events_after_change,
        "a label no-op patch writes NO event"
    );
}

/// The label diff base is scoped to the issue being patched: **another issue's labels never enter
/// it**, in either direction.
///
/// This is the only case here whose fixture holds a SECOND labelled issue at the moment of the label
/// ops, and that is the whole point. The other cases each hold a single labelled issue when their
/// patch runs, so they cannot tell a per-issue diff base from a workspace-wide one — a base built
/// from every label row in the database passes every one of them. A polluted base is not a cosmetic defect
/// either; it re-creates exactly the failure this ub-lp9.27 cell set exists to close:
///
/// - a `labels_add` of a label some OTHER issue carries diffs **equal**, so the op is skipped and
///   returned as a success with the label absent (the silent no-op again); and
/// - a `labels_remove` of a label only some OTHER issue carries diffs as a **real** removal, so the
///   patch invents a `LabelRemoved` event and an `updated_at` stamp for a label the target never had.
///
/// Both directions are exercised below, which is why the polluting issue carries two labels rather
/// than one.
pub async fn contract_label_diff_base_is_per_issue<S: Storage>(storage: S) {
    // Pollute FIRST — before any label op runs — with an issue carrying labels the target does NOT.
    let mut other = issue("ub-2", "the other issue");
    other.labels = vec!["foreign-add".to_string(), "foreign-remove".to_string()];
    storage
        .create_issue(&other, "a")
        .await
        .expect("create the polluting issue");
    let other_events_before = event_types(&storage, "ub-2").await.len();

    let mut target = issue("ub-1", "the target");
    target.labels = vec!["own".to_string()];
    storage
        .create_issue(&target, "a")
        .await
        .expect("create the target");
    let events_after_create = event_types(&storage, "ub-1").await.len();

    // (a) ADD a label the OTHER issue carries and the target does not: it must LAND on the target.
    let added = storage
        .update_issue(
            "ub-1",
            &IssuePatch {
                labels_add: vec!["foreign-add".to_string()],
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .expect("labels_add of a label another issue happens to carry");
    assert_eq!(
        added.labels,
        vec!["foreign-add".to_string(), "own".to_string()],
        "the add must LAND: another issue carrying that label says nothing about THIS issue's diff"
    );
    let reread = storage
        .get_issue("ub-1")
        .await
        .expect("get_issue")
        .expect("present");
    assert_eq!(
        reread.labels,
        vec!["foreign-add".to_string(), "own".to_string()],
        "the add is durable, not just reflected in the response"
    );
    assert_eq!(
        event_types(&storage, "ub-1")
            .await
            .split_off(events_after_create),
        vec!["label_added".to_string()],
        "exactly one LabelAdded, for the add that really happened"
    );
    let events_after_add = event_types(&storage, "ub-1").await.len();

    // (b) REMOVE a label ONLY the other issue carries: the target never had it, so this is the
    // empty-diff full skip — no stamp, no event, no phantom LabelRemoved.
    let removed = storage
        .update_issue(
            "ub-1",
            &IssuePatch {
                labels_remove: vec!["foreign-remove".to_string()],
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .expect("labels_remove of a label the target does not carry");
    assert_eq!(
        removed.labels,
        vec!["foreign-add".to_string(), "own".to_string()],
        "removing a label the target never carried leaves its set alone"
    );
    assert_eq!(
        removed.updated_at, added.updated_at,
        "removing a label the target never carried changes nothing, so it must NOT stamp updated_at"
    );
    assert_eq!(
        event_types(&storage, "ub-1").await.len(),
        events_after_add,
        "and it must NOT invent a LabelRemoved event"
    );

    // (c) The polluting issue is untouched throughout — the scoping holds for WRITES too, not only
    // for the read that seeds the base.
    let other_after = storage
        .get_issue("ub-2")
        .await
        .expect("get_issue")
        .expect("present");
    assert_eq!(
        other_after.labels,
        vec!["foreign-add".to_string(), "foreign-remove".to_string()],
        "the other issue keeps BOTH labels — neither the add nor the remove reached it"
    );
    assert_eq!(
        event_types(&storage, "ub-2").await.len(),
        other_events_before,
        "and no event was written against the other issue"
    );
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
/// # Invariant: file-backed storage for large `n` / high-concurrency seeding (T3.5.1, unblock-storage.md §5 OQ-8)
///
/// Callers **MUST** pass a **file-backed** (`open_local`) storage when `n` is large or several tasks
/// seed concurrently — never the shared-cache `open_in_memory` path. `open_in_memory`'s docs explain
/// why: its `memory_open_lock` serializes only the open-vs-open race, and a large concurrent
/// shared-cache transaction (or a concurrent store close) can still aggravate the same `SQLite`
/// process-global shared-cache registry race under heavy load — the intermittent flake observed at
/// T3.5.1. `benches/storage.rs` and `tests/scale.rs` already honor this (both seed a file-backed
/// store); `tests/heavy_corpus_stress.rs` is the dedicated regression proof for the boundary.
///
/// This function deliberately carries **no runtime guard** rejecting an in-memory `storage`: it is
/// generic over `S: Storage` and cannot cleanly detect file-vs-memory from the trait alone, and adding
/// a detection seam would either widen the production `create_issues` path or land somewhere not
/// cleanly reachable from here — either a genuine (and unwarranted) production-behaviour change for a
/// test/bench-only invariant. The boundary is enforced by this doc, the `tests/heavy_corpus_stress.rs`
/// regression, and code review instead — a deliberate omission, not an oversight.
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

// --------------------------------------------------------------------------------------------------
// Comments (FR-6, D37 — spine §3.2/§3.2.1)
// --------------------------------------------------------------------------------------------------

/// `add_comment`: the FORK-3 existence guard, `Event(Commented)`, MUST-1 (`updated_at` stays NULL),
/// the FORK-S1 `issues.updated_at` bump, and the FR-26 invariant that `content_hash` NEVER moves.
pub async fn contract_add_comment<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-a", "a"), "x")
        .await
        .unwrap();

    let before = storage.get_issue("ub-a").await.unwrap().expect("issue");
    let hash_before = before.content_hash.clone();

    let added = storage
        .add_comment("ub-a", "alice", "first thought", "alice")
        .await
        .expect("add_comment");
    assert_eq!(added.issue_id, "ub-a");
    assert_eq!(added.author, "alice");
    assert_eq!(added.body, "first thought");
    // MUST-1: the create path is create-time only — ONLY `update` ever sets `updated_at`.
    assert!(
        added.updated_at.is_none(),
        "add_comment must leave updated_at NULL (MUST-1), got {:?}",
        added.updated_at
    );
    assert!(added.redacted_at.is_none(), "a new comment is live");

    // Event(Commented) written in the SAME tx.
    assert!(
        event_types(&storage, "ub-a")
            .await
            .contains(&"commented".to_string()),
        "add_comment must write Event(Commented)"
    );

    let after = storage.get_issue("ub-a").await.unwrap().expect("issue");
    // FORK-S1: the owning issue's updated_at is bumped (feeds `stale`).
    assert!(
        after.updated_at > before.updated_at,
        "add_comment must bump issues.updated_at (FORK-S1)"
    );
    // FR-26 / spine §1.8: comments are EXCLUDED from content_hash — it must not move.
    assert_eq!(
        after.content_hash, hash_before,
        "a comment add must NEVER move content_hash (FR-26)"
    );

    // FORK-3 existence guard: an unknown issue id is IssueNotFound (the code is REUSED).
    let missing = storage.add_comment("ub-nope", "alice", "hi", "alice").await;
    assert!(
        matches!(missing, Err(StorageError::IssueNotFound { .. })),
        "add_comment on a missing issue must be IssueNotFound, got {missing:?}"
    );

    // FORK-3: a CLOSED issue is ALLOWED (post-mortem commentary).
    let mut closed = issue("ub-closed", "closed");
    closed.status = Status::Closed;
    closed.closed_at = Some(ts(2026, 1, 2));
    storage.create_issue(&closed, "x").await.unwrap();
    storage
        .add_comment("ub-closed", "alice", "post-mortem", "alice")
        .await
        .expect("a closed issue accepts comments (post-mortem)");

    // FORK-3: a TOMBSTONED issue is rejected.
    storage
        .create_issue(&issue("ub-dead", "dead"), "x")
        .await
        .unwrap();
    storage
        .delete_issue(
            &crate::filters::DeletePlan {
                mode: crate::filters::DeleteMode::Tombstone,
                targets: vec!["ub-dead".to_string()],
                cascade_children: Vec::new(),
            },
            "x",
        )
        .await
        .expect("tombstone");
    let on_tombstone = storage.add_comment("ub-dead", "alice", "hi", "alice").await;
    assert!(
        matches!(on_tombstone, Err(StorageError::IssueNotFound { .. })),
        "add_comment on a tombstone must be IssueNotFound, got {on_tombstone:?}"
    );
}

/// `list_comments` returns the canonical order (`created_at ASC, id ASC`) and hydration populates
/// `Issue.comments` in the SAME order on both the single and the batch read paths.
pub async fn contract_list_comments_order<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-a", "a"), "x")
        .await
        .unwrap();
    assert!(
        storage.list_comments("ub-a").await.unwrap().is_empty(),
        "no comments yet"
    );

    for body in ["one", "two", "three"] {
        storage
            .add_comment("ub-a", "alice", body, "alice")
            .await
            .unwrap();
    }

    let listed = storage.list_comments("ub-a").await.unwrap();
    let bodies: Vec<&str> = listed.iter().map(|c| c.body.as_str()).collect();
    assert_eq!(
        bodies,
        ["one", "two", "three"],
        "list_comments is ordered created_at ASC, id ASC"
    );

    // Hydration — the single read path (get_issue).
    let hydrated = storage.get_issue("ub-a").await.unwrap().expect("issue");
    let hydrated_bodies: Vec<&str> = hydrated.comments.iter().map(|c| c.body.as_str()).collect();
    assert_eq!(
        hydrated_bodies,
        ["one", "two", "three"],
        "get_issue must hydrate Issue.comments in canonical order"
    );

    // Hydration — the BATCH read path (get_issues / hydrate_ids), incl. an issue with none.
    storage
        .create_issue(&issue("ub-b", "b"), "x")
        .await
        .unwrap();
    let batch = storage
        .get_issues(&["ub-a".to_string(), "ub-b".to_string()])
        .await
        .unwrap();
    assert_eq!(batch.len(), 2);
    assert_eq!(
        batch[0]
            .comments
            .iter()
            .map(|c| c.body.as_str())
            .collect::<Vec<_>>(),
        ["one", "two", "three"],
        "the batch hydration path must populate Issue.comments identically"
    );
    assert!(
        batch[1].comments.is_empty(),
        "an issue with no comments hydrates an EMPTY vec, never a placeholder"
    );

    // Every collect_hydrated read path hydrates comments (list is the representative).
    let listed_issues = storage.list_issues(&ListFilters::default()).await.unwrap();
    let a = listed_issues
        .iter()
        .find(|i| i.id == "ub-a")
        .expect("ub-a listed");
    assert_eq!(
        a.comments.len(),
        3,
        "the list read path must hydrate Issue.comments"
    );
}

/// `update_comment` is PROVENANCE-PRESERVING (D-D): it sets `updated_at = now` and writes
/// `Event(CommentEdited)` carrying old + new bodies. A missing row is `CommentNotFound`.
pub async fn contract_update_comment_provenance<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-a", "a"), "x")
        .await
        .unwrap();
    let added = storage
        .add_comment("ub-a", "alice", "original", "alice")
        .await
        .unwrap();
    let hash_before = storage
        .get_issue("ub-a")
        .await
        .unwrap()
        .expect("issue")
        .content_hash;

    let edited = storage
        .update_comment(added.id, "revised", "bob")
        .await
        .expect("update_comment");
    assert_eq!(edited.id, added.id);
    assert_eq!(edited.body, "revised");
    assert!(
        edited.updated_at.is_some(),
        "update_comment MUST set updated_at — the bump IS the provenance (in-place-replace-\
         without-provenance is forbidden)"
    );
    assert!(edited.redacted_at.is_none(), "an edit does not redact");

    // The 2nd half of the provenance: Event(CommentEdited). Reading it back through the storage
    // event mapper also proves `parse_event_type` gained its arm rather than silently yielding
    // Custom("comment_edited").
    let events = storage.list_events("ub-a").await.expect("list_events");
    let edit = events
        .iter()
        .find(|e| e.event_type == unblock_model::EventType::CommentEdited)
        .expect("Event(CommentEdited) must be written in the same tx");
    assert_eq!(edit.old_value.as_deref(), Some("original"));
    assert_eq!(edit.new_value.as_deref(), Some("revised"));
    assert!(
        event_types(&storage, "ub-a")
            .await
            .contains(&"comment_edited".to_string()),
        "the wire string must be comment_edited, never a Custom fallthrough"
    );

    // FR-26: an edit never moves content_hash.
    assert_eq!(
        storage
            .get_issue("ub-a")
            .await
            .unwrap()
            .expect("issue")
            .content_hash,
        hash_before,
        "a comment edit must NEVER move content_hash (FR-26)"
    );

    // A missing comment row → CommentNotFound (which maps to ErrorCode::IssueNotFound at L7).
    let missing = storage.update_comment(999_999, "x", "bob").await;
    assert!(
        matches!(missing, Err(StorageError::CommentNotFound { id }) if id == 999_999),
        "update_comment on a missing row must be CommentNotFound, got {missing:?}"
    );
}

/// `delete_comment` is a SOFT-REDACT (D-E): the row is KEPT, `text` masked to `""`, `redacted_at`
/// set, and `Event(CommentRedacted)` RETAINS the original body. Idempotent when already redacted.
pub async fn contract_delete_comment_soft_redact<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-a", "a"), "x")
        .await
        .unwrap();
    let added = storage
        .add_comment("ub-a", "alice", "sensitive", "alice")
        .await
        .unwrap();
    let hash_before = storage
        .get_issue("ub-a")
        .await
        .unwrap()
        .expect("issue")
        .content_hash;

    let redacted = storage
        .delete_comment(added.id, "bob")
        .await
        .expect("delete_comment");
    assert_eq!(redacted.id, added.id);
    assert_eq!(redacted.body, "", "the body is MASKED to the empty string");
    assert!(
        redacted.redacted_at.is_some(),
        "the PRESENCE of redacted_at is the is-redacted flag"
    );

    // The row is KEPT — this is a soft-redact, NOT a hard delete.
    let listed = storage.list_comments("ub-a").await.unwrap();
    assert_eq!(listed.len(), 1, "soft-redact KEEPS the row");
    assert_eq!(listed[0].body, "");
    assert!(listed[0].redacted_at.is_some());

    // Event(CommentRedacted) RETAINS the original body (provenance — FORK-redact-wire), and its
    // wire string round-trips through parse_event_type rather than degrading to Custom.
    let events = storage.list_events("ub-a").await.expect("list_events");
    let redact = events
        .iter()
        .find(|e| e.event_type == unblock_model::EventType::CommentRedacted)
        .expect("Event(CommentRedacted) must be written in the same tx");
    assert_eq!(
        redact.old_value.as_deref(),
        Some("sensitive"),
        "the audit event RETAINS the original body (provenance)"
    );
    assert_eq!(redact.new_value, None);
    assert!(
        event_types(&storage, "ub-a")
            .await
            .contains(&"comment_redacted".to_string()),
        "the wire string must be comment_redacted, never a Custom fallthrough"
    );

    // FR-26: a redact never moves content_hash.
    assert_eq!(
        storage
            .get_issue("ub-a")
            .await
            .unwrap()
            .expect("issue")
            .content_hash,
        hash_before,
        "a comment redact must NEVER move content_hash (FR-26)"
    );

    // Idempotent: redacting an ALREADY-redacted comment is a no-op (no second event).
    let again = storage
        .delete_comment(added.id, "bob")
        .await
        .expect("delete_comment is idempotent on an already-redacted comment");
    assert_eq!(
        again.redacted_at, redacted.redacted_at,
        "the instant is kept"
    );
    let redact_count = event_types(&storage, "ub-a")
        .await
        .iter()
        .filter(|e| *e == "comment_redacted")
        .count();
    assert_eq!(
        redact_count, 1,
        "an idempotent redact writes NO second event"
    );

    // A missing comment row → CommentNotFound.
    let missing = storage.delete_comment(999_999, "bob").await;
    assert!(
        matches!(missing, Err(StorageError::CommentNotFound { id }) if id == 999_999),
        "delete_comment on a missing row must be CommentNotFound, got {missing:?}"
    );
}

/// The create/bulk/**import** SEED path persists caller-supplied `Comment` state VERBATIM — both
/// `updated_at` and `redacted_at` survive the round-trip (spine §3.2.1 MUST-1 SCOPE).
///
/// **This case exists because nothing else reaches the seed `INSERT INTO comments`**
/// (`crud::insert_issue_in_tx`, shared by `create_issue` AND `create_issues`): the sync fakes never
/// execute it and every other seeded fixture is comment-less. It FAILS against the 4-column INSERT
/// that MUST-1, over-applied beyond `add_comment`, would leave in place — under which a redacted
/// comment imports back UN-REDACTED.
pub async fn contract_seed_comments_round_trip_state<S: Storage>(storage: S) {
    let edited_at = ts(2026, 2, 1);
    let redacted_at = ts(2026, 3, 1);

    let mut live = comment(0, "ub-a", "alice", "still here");
    let mut edited = comment(0, "ub-a", "alice", "was revised");
    edited.created_at = ts(2026, 1, 2);
    edited.updated_at = Some(edited_at);
    let mut gone = comment(0, "ub-a", "alice", String::new().as_str());
    gone.created_at = ts(2026, 1, 3);
    gone.redacted_at = Some(redacted_at);
    live.created_at = ts(2026, 1, 1);

    // (1) the single create path.
    let mut seeded = issue("ub-a", "a");
    seeded.comments = vec![live.clone(), edited.clone(), gone.clone()];
    storage.create_issue(&seeded, "importer").await.unwrap();

    let read_back = storage.get_issue("ub-a").await.unwrap().expect("issue");
    assert_eq!(read_back.comments.len(), 3, "all seeded comments persist");
    assert_eq!(
        read_back.comments[0].updated_at, None,
        "a live comment stays un-edited"
    );
    assert_eq!(
        read_back.comments[0].redacted_at, None,
        "a live comment stays live"
    );
    assert_eq!(
        read_back.comments[1].updated_at,
        Some(edited_at),
        "the seed path must PRESERVE updated_at (it REPLAYS an existing comment; MUST-1 is scoped \
         to add_comment ONLY)"
    );
    assert_eq!(
        read_back.comments[2].redacted_at,
        Some(redacted_at),
        "the seed path must PRESERVE redacted_at — otherwise a redacted comment imports back \
         UN-REDACTED"
    );
    assert_eq!(
        read_back.comments[2].body, "",
        "the masked body round-trips"
    );

    // (2) the ATOMIC BULK path (create_issues) — the real import apply path.
    let mut bulk = issue("ub-b", "b");
    let mut bulk_redacted = comment(0, "ub-b", "alice", String::new().as_str());
    bulk_redacted.redacted_at = Some(redacted_at);
    let mut bulk_edited = comment(0, "ub-b", "alice", "bulk revised");
    bulk_edited.created_at = ts(2026, 1, 2);
    bulk_edited.updated_at = Some(edited_at);
    bulk.comments = vec![bulk_redacted, bulk_edited];
    storage
        .create_issues(&[bulk], "importer")
        .await
        .expect("bulk create with comments");

    let bulk_back = storage.get_issue("ub-b").await.unwrap().expect("issue");
    assert_eq!(bulk_back.comments.len(), 2);
    assert_eq!(
        bulk_back.comments[0].redacted_at,
        Some(redacted_at),
        "create_issues shares insert_issue_in_tx — redacted_at must survive the bulk path too"
    );
    assert_eq!(bulk_back.comments[1].updated_at, Some(edited_at));

    // (3) sync_equals: a seeded issue round-trips its comment state semantically (FORK-M2 puts
    //     redacted_at INTO the compare, so a dropped column would break this too).
    let mut expected = seeded.clone();
    // Ids are storage-minted; align them with what was read back before comparing.
    for (want, got) in expected.comments.iter_mut().zip(read_back.comments.iter()) {
        want.id = got.id;
    }
    assert!(
        expected.sync_equals(&read_back),
        "the seeded issue must round-trip its comment state under sync_equals"
    );
}

// --------------------------------------------------------------------------------------------------
// D42 — `Dependency.metadata` / `.thread_id` round-trip (NFR-16).
//
// These are the ONLY thing making the L2 bind non-vacuous. Before D42 the write side bound 5 columns
// while the read side projected 7, so both fields were accepted, typed, schema-published — and
// DISCARDED. It survived to GA because the defect is DOUBLY masked: `DEFAULT '{}'` writes `'{}'` for
// an unbound value and the read filter coerces `'{}'` back to `None`, making "never bound"
// indistinguishable from "explicitly absent" even under direct SQL inspection. Every pre-existing
// storage test constructs `metadata: None`, so the field had NEVER been round-trip tested.
//
// The two INSERT sites are tested INDEPENDENTLY on purpose: a partial fix (binding `deps.rs` but not
// `crud.rs`) still looks correct through the `dep` tool while the JSONL import leg stays lossy. One
// end-to-end test is NOT sufficient.
// --------------------------------------------------------------------------------------------------

/// A dependency edge carrying metadata.
fn dep_with_metadata(from: &str, to: &str, metadata: Option<&str>) -> Dependency {
    Dependency {
        metadata: metadata.map(ToString::to_string),
        ..dep(from, to, DependencyType::Blocks)
    }
}

/// `add_dependency` (the `dep` tool leg) PERSISTS `metadata`. Fails against the 5-column INSERT.
pub async fn contract_add_dependency_persists_metadata<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-a", "a"), "x")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-b", "b"), "x")
        .await
        .unwrap();

    storage
        .add_dependency(
            &dep_with_metadata("ub-a", "ub-b", Some("{\"why\":\"KEEP\"}")),
            "x",
        )
        .await
        .expect("add edge");

    let deps = storage.list_dependencies("ub-a").await.expect("list");
    assert_eq!(deps.len(), 1);
    assert_eq!(
        deps[0].metadata.as_deref(),
        Some("{\"why\":\"KEEP\"}"),
        "`add_dependency` must PERSIST `metadata` — a 5-column INSERT silently drops it and the \
         `DEFAULT '{{}}'` + read-filter pair makes the loss invisible to direct SQL inspection"
    );
}

/// `create_issue` with a PRE-BUILT `Issue.dependencies` (the JSONL / bd-import leg) persists
/// `metadata`, on BOTH the single and the batch hydrate path. Fails against the 5-column INSERT.
pub async fn contract_create_issue_persists_dep_metadata<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-b", "b"), "x")
        .await
        .unwrap();
    let mut carrier = issue("ub-a", "a");
    carrier.dependencies = vec![dep_with_metadata(
        "ub-a",
        "ub-b",
        Some("{\"src\":\"jsonl\"}"),
    )];
    storage.create_issue(&carrier, "x").await.expect("create");

    let hydrated = storage
        .get_issue("ub-a")
        .await
        .expect("get")
        .expect("present");
    assert_eq!(
        hydrated.dependencies[0].metadata.as_deref(),
        Some("{\"src\":\"jsonl\"}"),
        "the create path must persist dep `metadata` — otherwise `export -> import -> export` is \
         NOT a fixed point for a metadata-carrying dep (an FR-26/D5 fidelity bug)"
    );

    // Second cell: the BATCH hydrate is a distinct read projection.
    let batch = storage
        .get_issues(&["ub-a".to_string()])
        .await
        .expect("batch");
    assert_eq!(
        batch[0].dependencies[0].metadata.as_deref(),
        Some("{\"src\":\"jsonl\"}")
    );
}

/// NULL vs `'{}'` discrimination. Guards against "fixing" the bind by writing `'{}'` instead of
/// SQL NULL, and against deleting the deliberate legacy `!= "{}"` read coercion.
pub async fn contract_dep_metadata_null_vs_empty_object<S: Storage>(storage: S) {
    for (idx, (input, expected)) in [
        (None, None),
        // The documented legacy tolerance: a stored `'{}'` reads back as absent.
        (Some("{}"), None),
        (Some("{\"a\":1}"), Some("{\"a\":1}")),
    ]
    .into_iter()
    .enumerate()
    {
        let from = format!("ub-s{idx}");
        let to = format!("ub-t{idx}");
        storage.create_issue(&issue(&from, "s"), "x").await.unwrap();
        storage.create_issue(&issue(&to, "t"), "x").await.unwrap();
        storage
            .add_dependency(&dep_with_metadata(&from, &to, input), "x")
            .await
            .expect("add edge");
        let deps = storage.list_dependencies(&from).await.expect("list");
        assert_eq!(
            deps[0].metadata.as_deref(),
            expected,
            "metadata {input:?} must read back as {expected:?}"
        );
    }
}

/// `thread_id` round-trips at L2. It is NOT wire-reachable (no MCP input carries such a field, and
/// the `dep` tool's `build_add_dependency` — the L7 construction site — hardcodes `None`), so this
/// case is the ONLY thing that makes the `thread_id` bind verifiable rather than dead code.
pub async fn contract_dep_thread_id_round_trip<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-a", "a"), "x")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-b", "b"), "x")
        .await
        .unwrap();
    let edge = Dependency {
        thread_id: Some("thread-7".to_string()),
        ..dep("ub-a", "ub-b", DependencyType::Blocks)
    };
    storage.add_dependency(&edge, "x").await.expect("add edge");

    let deps = storage.list_dependencies("ub-a").await.expect("list");
    assert_eq!(deps[0].thread_id.as_deref(), Some("thread-7"));
}

// --------------------------------------------------------------------------------------------------
// D44 — `create_issue` with seeded `Issue.dependencies` (spine §3.2.1)
// --------------------------------------------------------------------------------------------------

/// A `create_issue` whose `dependencies[0].issue_id` deliberately names ANOTHER EXISTING issue lands
/// the edge on `issue.id` and leaves that other issue byte-unchanged.
///
/// The spine makes this an explicit NFR-16 obligation: the re-anchoring is "the property that makes
/// the single create path safe, and it MUST be pinned by the contract suite".
///
/// MUTANT KILLED: an INSERT that binds `dep.issue_id` instead of `issue.id` as the edge source. That
/// is the misattachment defect itself, live-reproduced at GA over the JSON-RPC wire: the edge lands
/// on the third party, which silently drops out of the ready set while its `updated_at` does not
/// move, so no staleness or change-detection query ever signals it. Both halves are asserted here —
/// where the edge DID land, and that the victim graph is untouched — because either alone is
/// satisfied by an implementation that simply drops the edge.
pub async fn contract_create_issue_reanchors_a_foreign_edge_source<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-victim", "an unrelated third party"), "x")
        .await
        .expect("victim");
    storage
        .create_issue(&issue("ub-target", "the blocker"), "x")
        .await
        .expect("target");

    let victim_before = storage
        .get_issue("ub-victim")
        .await
        .expect("get")
        .expect("exists");

    let mut carrier = issue("ub-new", "the issue being created");
    carrier.dependencies = vec![dep("ub-victim", "ub-target", DependencyType::Blocks)];
    storage.create_issue(&carrier, "x").await.expect("create");

    let landed = storage.list_dependencies("ub-new").await.expect("list new");
    assert_eq!(landed.len(), 1, "the seeded edge is persisted");
    assert_eq!(
        landed[0].issue_id, "ub-new",
        "the edge MUST be anchored on the row being created, never on the id carried by the object"
    );
    assert_eq!(landed[0].depends_on_id, "ub-target");

    assert!(
        storage
            .list_dependencies("ub-victim")
            .await
            .expect("list victim")
            .is_empty(),
        "the named third party must gain NO edge at all"
    );
    let victim_after = storage
        .get_issue("ub-victim")
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(
        victim_before, victim_after,
        "the third party row must be byte-unchanged, `updated_at` and `content_hash` included"
    );
}

/// `create_issue` REJECTS a `depends_on_id` repeated inside the ONE declared list, and persists
/// nothing. The two elements carry different `dep_type`s: the key is the `(source, target)` pair,
/// type-INSENSITIVE, matching `add_dependency`.
///
/// MUTANT KILLED: dropping the create-specific duplicate guard and letting the shared per-record
/// body handle it. That body `continue`s past a repeated target, so the create would return `Ok`
/// having written ONE edge where two were declared — the exact silent-skip this guard restores.
pub async fn contract_create_issue_rejects_a_duplicate_declared_edge<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-target", "target"), "x")
        .await
        .expect("target");

    let mut carrier = issue("ub-dup", "declares one target twice");
    carrier.dependencies = vec![
        dep("ub-dup", "ub-target", DependencyType::Blocks),
        dep("ub-dup", "ub-target", DependencyType::WaitsFor),
    ];
    let err = storage
        .create_issue(&carrier, "x")
        .await
        .expect_err("a repeated target is a duplicate edge");
    assert!(
        matches!(err, StorageError::DuplicateDependency),
        "expected DuplicateDependency, got {err:?}"
    );
    assert!(
        storage.get_issue("ub-dup").await.expect("get").is_none(),
        "the rejected create persists ZERO rows — not even an edgeless issue"
    );
}

/// `create_issue` REJECTS declared gating edges that close a cycle, and persists nothing.
///
/// The fixture is the mixed-orientation case (spine §3.2.1 guard (b)): a `parent-child` element
/// gives the new node an IN-edge and a `blocks` element an OUT-edge, and only together do they close
/// `new -> far -> parent -> new`.
///
/// MUTANT KILLED: dropping the create-specific gating guard, which the seeded path never had — the
/// create would then commit a genuine ready-gating cycle that `dep {action:"add"}` refuses to build
/// one edge at a time.
pub async fn contract_create_issue_rejects_a_declared_gating_cycle<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-p", "parent"), "x")
        .await
        .expect("p");
    let mut far = issue("ub-x", "far");
    far.dependencies = vec![dep("ub-x", "ub-p", DependencyType::Blocks)];
    storage.create_issue(&far, "x").await.expect("x");

    let mut carrier = issue("ub-n", "closes the cycle");
    carrier.dependencies = vec![
        dep("ub-n", "ub-p", DependencyType::ParentChild),
        dep("ub-n", "ub-x", DependencyType::Blocks),
    ];
    let err = storage
        .create_issue(&carrier, "x")
        .await
        .expect_err("the declared edges close a gating cycle");
    match err {
        StorageError::CycleDetected { ref path } => {
            for node in ["ub-n", "ub-x", "ub-p"] {
                assert!(
                    path.contains(node),
                    "the cycle path must name every node ({node} missing): {path}"
                );
            }
        }
        other => panic!("expected CycleDetected, got {other:?}"),
    }
    assert!(
        storage.get_issue("ub-n").await.expect("get").is_none(),
        "the rejected create persists ZERO rows"
    );
}

/// THE SILENT-REGRESSION CATCHER. `create_issues` — the shared bulk/import body — keeps BOTH of the
/// behaviours `create_issue` now refuses: it still DEDUPS a repeated target with a silent skip, and
/// it still ADMITS a genuine mutual gating cycle, atomically.
///
/// This is the one cell in the suite whose job is to fail when a future edit tidies the two
/// create-specific guards into `insert_issue_in_tx`. That edit is invisible everywhere else — every
/// D44 cell above would stay green, because moving a guard INWARD only makes the create stricter.
/// What it silently breaks is the D5 import leg, which enters through this method
/// (`unblock-sync`, `import.rs`): an already-exported record carrying either shape would stop being
/// importable, so a data-integrity tool would have made its own committed record un-restorable.
/// PRD D44 clause (3) states the constraint; this asserts it.
///
/// MUTANT KILLED: moving `reject_duplicate_declared_edges` or `reject_declared_gating_cycles` from
/// the `create_issue` wrapper into the shared per-record body. The first assertion below goes red on
/// the duplicate move, the second on the cycle move.
pub async fn contract_create_issues_still_dedups_and_still_admits_a_cycle<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-bulk-t", "bulk target"), "x")
        .await
        .expect("target");

    // (1) A repeated target inside ONE record: deduped-and-continued, NOT rejected.
    let mut dup = issue("ub-bulk-dup", "repeats one target");
    dup.dependencies = vec![
        dep("ub-bulk-dup", "ub-bulk-t", DependencyType::Blocks),
        dep("ub-bulk-dup", "ub-bulk-t", DependencyType::WaitsFor),
    ];
    storage.create_issues(&[dup], "x").await.expect(
        "bulk must still accept a repeated target (dedup-and-continue is D5 import policy)",
    );
    assert_eq!(
        storage
            .list_dependencies("ub-bulk-dup")
            .await
            .expect("list")
            .len(),
        1,
        "the shared body writes the first copy and skips the second — unchanged by D44"
    );

    // (2) A genuine MUTUAL gating cycle across two records of one batch: committed atomically.
    let mut a = issue("ub-bulk-a", "a");
    a.dependencies = vec![dep("ub-bulk-a", "ub-bulk-b", DependencyType::Blocks)];
    let mut b = issue("ub-bulk-b", "b");
    b.dependencies = vec![dep("ub-bulk-b", "ub-bulk-a", DependencyType::Blocks)];
    storage.create_issues(&[a, b], "x").await.expect(
        "bulk must still commit a mutual gating cycle — moving the guard here could make an \
                 ALREADY-EXPORTED D5 record un-importable",
    );

    let cycles = storage.detect_cycles(true).await.expect("detect");
    assert!(
        !cycles.is_empty(),
        "the cycle really was persisted, so this cell is not vacuous: {cycles:?}"
    );
}

// --------------------------------------------------------------------------------------------------
// D45 (v1.0.1) — the dependency-TARGET existence guard (spine §1.9 / §3.2.1)
// --------------------------------------------------------------------------------------------------
//
// `dependencies.depends_on_id` carries NO foreign key — deliberately, because an `external:` target
// is a legitimate blocker no row can ever satisfy — so an edge naming an issue that does not exist
// used to be written with `isError:false`, planting a blocker that can never be created and therefore
// never closed. D45 closes the class application-level, in-transaction, on EVERY edge-writing path.
// The cells below cover the three L2 write bodies (the shared per-record insert body, `add_dependency`
// and `apply_reparent`), the batch-awareness the import leg depends on, the external/tombstone
// carve-outs, the published precedence chain, and the §1.9 Rust/SQL equivalence obligation.

/// `create_issue` REFUSES a declared target that names no row, with `BlockerNotFound` carrying BOTH
/// ids, and persists ZERO rows.
///
/// MUTANT KILLED: deleting the target-existence pass from the shared per-record insert body — the
/// create then returns `Ok` and the issue is born with a blocker that can never be created and
/// therefore never closed (the D45 defect itself). The zero-rows half is asserted separately because
/// an implementation that rejected AFTER committing the row would satisfy the code assertion alone.
pub async fn contract_create_issue_refuses_a_phantom_target<S: Storage>(storage: S) {
    let mut carrier = issue("ub-dep", "names a blocker that does not exist");
    carrier.dependencies = vec![dep("ub-dep", "ub-ghost", DependencyType::Blocks)];

    let err = storage
        .create_issue(&carrier, "x")
        .await
        .expect_err("a target that names no row is refused");
    match err {
        StorageError::BlockerNotFound {
            ref issue_id,
            ref depends_on_id,
        } => {
            assert_eq!(issue_id, "ub-dep", "the DECLARING record must be named");
            assert_eq!(
                depends_on_id, "ub-ghost",
                "the phantom target must be named"
            );
        }
        other => panic!("expected BlockerNotFound, got {other:?}"),
    }
    assert!(
        storage.get_issue("ub-dep").await.expect("get").is_none(),
        "the rejected create persists ZERO rows — not even an edgeless issue"
    );
}

/// `create_issues` — the shared bulk/import body — REFUSES the WHOLE batch when any record declares a
/// target present neither in the batch nor in storage, leaving ZERO rows.
///
/// This is the NFR-16 obligation the spine names explicitly, and it is the storage-level proof of the
/// guard on the bulk/import leg: the `create_bulk` path rejects an unknown REFERENCE at L5 with
/// `VALIDATION_FAILED` and never reaches this body, so without this cell the shared-body guard would
/// have no direct coverage at all.
///
/// MUTANT KILLED: bodying the guard in the `create_issue` WRAPPER instead of the shared body. Every
/// single-create cell would stay green while bulk and the JSONL/`bd` import leg — the paths that
/// actually ingest foreign data — kept writing phantom blockers.
pub async fn contract_create_issues_refuses_a_phantom_target_whole_batch<S: Storage>(storage: S) {
    let clean = issue("ub-batch-ok", "a perfectly good record");
    let mut broken = issue("ub-batch-bad", "names a phantom");
    broken.dependencies = vec![dep("ub-batch-bad", "ub-ghost", DependencyType::Blocks)];

    let err = storage
        .create_issues(&[clean, broken], "x")
        .await
        .expect_err("a phantom target refuses the batch");
    assert!(
        matches!(err, StorageError::BlockerNotFound { .. }),
        "expected BlockerNotFound, got {err:?}"
    );

    for id in ["ub-batch-ok", "ub-batch-bad"] {
        assert!(
            storage.get_issue(id).await.expect("get").is_none(),
            "the whole batch rolls back — {id} must not survive the refusal"
        );
    }
    assert_eq!(
        count_all(&storage).await,
        0,
        "ZERO rows persist: not even the record staged BEFORE the offending one"
    );
}

/// The guard is BATCH-AWARE in BOTH directions: a record may name a sibling staged EARLIER in the
/// same batch (backward) **and** one appearing LATER in it (forward).
///
/// MUTANT KILLED: a naive per-record in-transaction `SELECT` with no batch arm. It accepts the
/// BACKWARD reference and refuses the FORWARD one, so the suite would still look half-green — an
/// ordering-dependent refusal of a legal import that no file author can avoid and no caller can
/// predict. The forward assertion is the one that kills it; the backward one keeps the cell honest
/// about what already worked.
pub async fn contract_create_issues_accepts_an_intra_batch_reference_in_both_directions<
    S: Storage,
>(
    storage: S,
) {
    // Record A names Z, which appears LATER in the same slice (the forward reference); record C
    // names A, which appeared EARLIER (the backward one).
    let mut a = issue("ub-fwd-a", "names a record later in the file");
    a.dependencies = vec![dep("ub-fwd-a", "ub-fwd-z", DependencyType::Blocks)];
    let z = issue("ub-fwd-z", "the later record");
    let mut c = issue("ub-fwd-c", "names a record earlier in the file");
    c.dependencies = vec![dep("ub-fwd-c", "ub-fwd-a", DependencyType::Blocks)];

    storage
        .create_issues(&[a, z, c], "x")
        .await
        .expect("an intra-batch reference is legal in BOTH directions, whatever the record order");

    assert_eq!(
        storage
            .list_dependencies("ub-fwd-a")
            .await
            .expect("list")
            .len(),
        1,
        "the FORWARD edge is persisted"
    );
    assert_eq!(
        storage
            .list_dependencies("ub-fwd-c")
            .await
            .expect("list")
            .len(),
        1,
        "the BACKWARD edge is persisted"
    );
}

/// `add_dependency` guards BOTH endpoints in-transaction, with the published codes: a missing SOURCE
/// is the EXISTING `IssueNotFound` (the missing thing genuinely IS the addressed issue), a missing
/// TARGET is `BlockerNotFound`, and when both are missing the SOURCE wins.
///
/// MUTANTS KILLED: (a) dropping the target probe — the phantom blocker lands with `isError:false`;
/// (b) dropping the source probe — a typo in the source field returns the schema's opaque foreign-key
/// failure (`DatabaseError`, exit 2) while the same typo in the target field returns exit 3, so ONE
/// call with ONE typo yields two codes depending on which FIELD carried it; (c) minting a new variant
/// for the source instead of reusing `IssueNotFound`, which the third assertion pins by CODE.
pub async fn contract_add_dependency_guards_both_endpoints<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-real", "a real row"), "x")
        .await
        .expect("real");

    // Missing TARGET.
    let err = storage
        .add_dependency(&dep("ub-real", "ub-ghost", DependencyType::Blocks), "x")
        .await
        .expect_err("a target that names no row is refused");
    match err {
        StorageError::BlockerNotFound {
            ref issue_id,
            ref depends_on_id,
        } => {
            assert_eq!(issue_id, "ub-real");
            assert_eq!(depends_on_id, "ub-ghost");
        }
        other => panic!("expected BlockerNotFound for a missing target, got {other:?}"),
    }
    assert!(
        storage
            .list_dependencies("ub-real")
            .await
            .expect("list")
            .is_empty(),
        "the refused edge is not written"
    );

    // Missing SOURCE (target exists) -> the EXISTING IssueNotFound, never a minted variant.
    let err = storage
        .add_dependency(&dep("ub-nobody", "ub-real", DependencyType::Blocks), "x")
        .await
        .expect_err("a source that names no row is refused");
    match err {
        StorageError::IssueNotFound { ref id } => assert_eq!(id, "ub-nobody"),
        other => panic!("expected IssueNotFound for a missing source, got {other:?}"),
    }

    // BOTH missing -> the SOURCE wins (a row must exist before its edges mean anything).
    let err = storage
        .add_dependency(&dep("ub-nobody", "ub-ghost", DependencyType::Blocks), "x")
        .await
        .expect_err("both endpoints missing is still refused");
    assert!(
        matches!(err, StorageError::IssueNotFound { ref id } if id == "ub-nobody"),
        "the SOURCE probe precedes the TARGET probe, got {err:?}"
    );
}

/// The reparent path (`update_issue { parent }`) carries the same guard: a parent that names no row
/// is `BlockerNotFound`, and the whole update rolls back.
///
/// MUTANT KILLED: leaving `apply_reparent` unguarded because a dangling `parent-child` edge does not
/// produce the never-ready symptom. It is still hydrated onto the issue, still exported, still
/// written with `isError:false`, and still LISTED by the `dangling` diagnostic — so an unguarded
/// reparent lets the tool that reports the defect be used to create the defects it reports.
pub async fn contract_reparent_refuses_a_phantom_parent<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-child", "a child looking for a parent"), "x")
        .await
        .expect("child");
    let before = storage
        .get_issue("ub-child")
        .await
        .expect("get")
        .expect("exists");

    let patch = IssuePatch {
        parent: Some(Some("ub-ghost-parent".to_string())),
        ..IssuePatch::default()
    };
    let err = storage
        .update_issue("ub-child", &patch, "x")
        .await
        .expect_err("a parent that names no row is refused");
    match err {
        StorageError::BlockerNotFound {
            ref issue_id,
            ref depends_on_id,
        } => {
            assert_eq!(issue_id, "ub-child");
            assert_eq!(depends_on_id, "ub-ghost-parent");
        }
        other => panic!("expected BlockerNotFound, got {other:?}"),
    }

    assert!(
        storage
            .list_dependencies("ub-child")
            .await
            .expect("list")
            .is_empty(),
        "no parent-child edge is written"
    );
    assert_eq!(
        storage.get_issue("ub-child").await.expect("get"),
        Some(before),
        "the whole update rolls back — the row is byte-unchanged"
    );
}

/// An `external:` target is ACCEPTED on every L2 write path, in BOTH ASCII spellings, INCLUDING as a
/// PARENT.
///
/// An external target names a ticket in another system: it is a legitimate blocker that no row can
/// ever satisfy, which is exactly why the column carries no foreign key and why the guard has to be
/// application-level.
///
/// The AC (`docs/plans/implementation-plan.md`, T1.6 D45 item (3)) is stated **per PATH**, not per
/// suite: each of the four L2 legs below carries BOTH the `external:` and the `EXTERNAL:` spelling,
/// so no leg is covered by a sibling leg's spelling.
///
/// MUTANTS KILLED: (a) a guard with no carve-out at all — every external blocker becomes unwritable,
/// breaking a GA-shipped shape; (b) a case-SENSITIVE `starts_with`, the shipped engine dialect, under
/// which `EXTERNAL:jira-1` is refused by the write guard while the ready/blocked SQL already treats
/// it as external — the write guard would be STRICTER than the read side (spine §1.9 invariant 4);
/// (c) a carve-out applied per-EDGE-TYPE, which the `parent` leg catches by refusing an external
/// PARENT that is legal today. Mutant (b) kills the UPPER-spelled cell on every leg *independently*
/// — proven by a ladder that lowered each earlier leg's upper spelling in turn and read the offending
/// id out of the panic, so no leg's cell is shadowed by an earlier leg's failure.
pub async fn contract_external_target_is_accepted_on_every_write_path<S: Storage>(storage: S) {
    // (1) The shared insert body, via the single create — both spellings, on one record.
    let mut carrier = issue("ub-ext", "blocked by two foreign tickets");
    carrier.dependencies = vec![
        dep("ub-ext", "external:jira-1", DependencyType::Blocks),
        dep("ub-ext", "EXTERNAL:jira-2", DependencyType::Blocks),
    ];
    storage
        .create_issue(&carrier, "x")
        .await
        .expect("an external target is legal on create, in either ASCII case");
    assert_eq!(
        storage
            .list_dependencies("ub-ext")
            .await
            .expect("list")
            .len(),
        2
    );

    // (2) The shared insert body, via the bulk/import leg — BOTH spellings on this path too (the
    // AC is per-PATH, not per-suite), plus the mixed spelling that only an ASCII-case-INSENSITIVE
    // predicate accepts.
    let mut bulk = issue("ub-ext-bulk", "imported with a foreign blocker");
    bulk.dependencies = vec![
        dep("ub-ext-bulk", "external:jira-3", DependencyType::WaitsFor),
        dep("ub-ext-bulk", "EXTERNAL:jira-3b", DependencyType::WaitsFor),
        dep("ub-ext-bulk", "ExTeRnAl:jira-3c", DependencyType::WaitsFor),
    ];
    storage
        .create_issues(&[bulk], "x")
        .await
        .expect("an external target is legal on the bulk/import leg, in either ASCII case");
    assert_eq!(
        storage
            .list_dependencies("ub-ext-bulk")
            .await
            .expect("list")
            .len(),
        3,
        "every external spelling is persisted on the bulk/import leg"
    );

    // (3) `add_dependency` — both spellings.
    storage
        .add_dependency(
            &dep("ub-ext", "external:jira-4", DependencyType::Blocks),
            "x",
        )
        .await
        .expect("an external target is legal on dep add");
    storage
        .add_dependency(
            &dep("ub-ext", "EXTERNAL:jira-4b", DependencyType::Blocks),
            "x",
        )
        .await
        .expect("an external target is legal on dep add in the UPPER spelling too");

    // (4) Reparent — an `external:` PARENT stays legal: the carve-out is per-TARGET, NEVER
    // per-edge-type (spine §1.9 invariant 6). Both spellings, one reparent each (the second
    // replaces the first parent edge).
    let lower_patch = IssuePatch {
        parent: Some(Some("external:jira-5-lower".to_string())),
        ..IssuePatch::default()
    };
    storage
        .update_issue("ub-ext-bulk", &lower_patch, "x")
        .await
        .expect("an external PARENT is legal in the LOWER spelling");
    assert!(
        storage
            .list_dependencies("ub-ext-bulk")
            .await
            .expect("list")
            .iter()
            .any(|d| d.depends_on_id == "external:jira-5-lower"
                && d.dep_type == DependencyType::ParentChild),
        "the lower-spelled external parent edge is persisted verbatim"
    );
    let patch = IssuePatch {
        parent: Some(Some("EXTERNAL:jira-5".to_string())),
        ..IssuePatch::default()
    };
    storage
        .update_issue("ub-ext-bulk", &patch, "x")
        .await
        .expect("an external PARENT stays representable exactly as it is today");
    assert!(
        storage
            .list_dependencies("ub-ext-bulk")
            .await
            .expect("list")
            .iter()
            .any(|d| d.depends_on_id == "EXTERNAL:jira-5"
                && d.dep_type == DependencyType::ParentChild),
        "the external parent edge is persisted verbatim"
    );
}

/// A TOMBSTONED target COUNTS AS EXISTING — deliberately, and deliberately UNLIKE `add_comment`'s
/// FORK-3 rule, which rejects a tombstoned issue.
///
/// The export corpus includes tombstones, so an edge to a tombstoned blocker is a normal,
/// round-trippable fact. Refusing it would make a conforming export un-importable — the precise
/// failure D45 also addresses on the export side.
///
/// MUTANT KILLED: a status-aware existence query (`… AND status != 'tombstone'`, or reusing the
/// default-visibility filter). Every cell above stays green under it; only this one goes red, and the
/// damage it does is silent until someone re-imports their own export.
pub async fn contract_tombstoned_target_counts_as_existing<S: Storage>(storage: S) {
    storage
        .create_issue(&issue("ub-dead", "about to be tombstoned"), "x")
        .await
        .expect("target");
    let plan = DeletePlan {
        mode: DeleteMode::Tombstone,
        targets: vec!["ub-dead".to_string()],
        cascade_children: Vec::new(),
    };
    storage.delete_issue(&plan, "x").await.expect("tombstone");

    // Via the shared insert body.
    let mut carrier = issue("ub-live", "depends on a tombstoned blocker");
    carrier.dependencies = vec![dep("ub-live", "ub-dead", DependencyType::Blocks)];
    storage
        .create_issue(&carrier, "x")
        .await
        .expect("an edge to a TOMBSTONED blocker is a normal, round-trippable fact");

    // Via `add_dependency`.
    storage
        .create_issue(&issue("ub-live2", "another dependent"), "x")
        .await
        .expect("live2");
    storage
        .add_dependency(&dep("ub-live2", "ub-dead", DependencyType::WaitsFor), "x")
        .await
        .expect("the existence query is STATUS-AGNOSTIC on dep add too");
}

/// The PUBLISHED precedence chain is ASSERTED, not assumed:
/// `SelfDependency` → `BlockerNotFound` → `DuplicateDependency` → `CycleDetected`.
///
/// Precedence is OBSERVABLE BEHAVIOUR, so every neighbouring pair is exercised with an input that
/// violates both rules at once.
///
/// MUTANTS KILLED: (a) interleaving the target-existence check INTO the staging loop instead of
/// running it as ONE post-staging pass — element 1's missing target would then beat element 2's
/// self-dependency, which the first case detects by deliberately putting the phantom FIRST in the
/// declared list; (b) running the existence pass after the wrapper's duplicate guard, which the
/// second case detects; (c) running it after the cycle guard, which the third detects — a cycle
/// witness naming a phantom node reports a derived defect while hiding the primary one.
pub async fn contract_blocker_not_found_precedence<S: Storage>(storage: S) {
    // (a) `SelfDependency` beats `BlockerNotFound`, even with the phantom declared FIRST.
    let mut selfish = issue("ub-prec-self", "names a phantom, then itself");
    selfish.dependencies = vec![
        dep("ub-prec-self", "ub-ghost", DependencyType::Blocks),
        dep("ub-prec-self", "ub-prec-self", DependencyType::Blocks),
    ];
    let err = storage
        .create_issue(&selfish, "x")
        .await
        .expect_err("refused");
    assert!(
        matches!(err, StorageError::SelfDependency),
        "SelfDependency fires during staging and still wins when the phantom is declared FIRST — \
         the existence check is ONE POST-staging pass, not an interleaved one. Got {err:?}"
    );

    // (b) `BlockerNotFound` beats `DuplicateDependency` (the rank the shared-body placement forces).
    let mut dup = issue("ub-prec-dup", "repeats one phantom target");
    dup.dependencies = vec![
        dep("ub-prec-dup", "ub-ghost", DependencyType::Blocks),
        dep("ub-prec-dup", "ub-ghost", DependencyType::WaitsFor),
    ];
    let err = storage.create_issue(&dup, "x").await.expect_err("refused");
    assert!(
        matches!(err, StorageError::BlockerNotFound { .. }),
        "a missing target NECESSARILY precedes a duplicate on the create path, got {err:?}"
    );

    // (c) `BlockerNotFound` beats `CycleDetected`: endpoint existence is prior to any relational
    // question about the graph.
    storage
        .create_issue(&issue("ub-prec-p", "parent"), "x")
        .await
        .expect("p");
    let mut far = issue("ub-prec-x", "far");
    far.dependencies = vec![dep("ub-prec-x", "ub-prec-p", DependencyType::Blocks)];
    storage.create_issue(&far, "x").await.expect("x");

    let mut cyclic = issue("ub-prec-n", "closes a cycle AND names a phantom");
    cyclic.dependencies = vec![
        dep("ub-prec-n", "ub-prec-p", DependencyType::ParentChild),
        dep("ub-prec-n", "ub-prec-x", DependencyType::Blocks),
        dep("ub-prec-n", "ub-ghost", DependencyType::Blocks),
    ];
    let err = storage
        .create_issue(&cyclic, "x")
        .await
        .expect_err("refused");
    assert!(
        matches!(err, StorageError::BlockerNotFound { .. }),
        "a target that does not exist cannot participate in a cycle, got {err:?}"
    );
}

/// THE ONE SHIPPED REJECTION D45 CHANGES, pinned rather than discovered later: on `add_dependency`,
/// re-adding an edge that ALREADY exists and whose target is DANGLING now returns `BlockerNotFound`
/// (`ISSUE_NOT_FOUND`) where GA returned `DuplicateDependency`.
///
/// It is reachable only on already-corrupt data — exactly the population D45 exists to surface — and
/// it is the observable consequence of moving the target probe AHEAD of the duplicate query so that
/// ONE published chain describes every path.
///
/// MUTANT KILLED: leaving the target probe AFTER the duplicate query on this path. Every other cell
/// stays green (the duplicate query only fires on an edge that already exists), while `add_dependency`
/// and `create_issue` would publish contradictory precedence for the same pair of rules.
pub async fn contract_dangling_duplicate_edge_reports_the_missing_target<S>(storage: S)
where
    S: Storage + StorageTestkit,
{
    storage
        .create_issue(
            &issue("ub-corrupt", "carries a pre-existing dangling edge"),
            "x",
        )
        .await
        .expect("source");
    // Plant the dangling edge through the seam — every guarded write path now refuses to create one,
    // which is the point of D45; the corrupt state can only pre-date the guard.
    storage
        .testkit_insert_raw_edge(&dep("ub-corrupt", "ub-ghost", DependencyType::Blocks))
        .await
        .expect("plant the pre-existing dangling edge");

    let err = storage
        .add_dependency(&dep("ub-corrupt", "ub-ghost", DependencyType::Blocks), "x")
        .await
        .expect_err("re-adding the dangling edge is refused");
    assert!(
        matches!(err, StorageError::BlockerNotFound { .. }),
        "the TARGET probe runs BEFORE the duplicate query, so the primary defect is reported, \
         not the derived one. Got {err:?}"
    );
}

/// A DANGLING edge SURVIVES into the whole-graph read (`dependency_graph(&[])`).
///
/// The survival is not incidental: the loader is a bare `SELECT issue_id, depends_on_id, type FROM
/// dependencies` with NO join, which is precisely why a broken edge reaches the caller.
///
/// **Its stated PURPOSE changed on 2026-08-02 and the docstring moves with it rather than becoming
/// false prose in a green suite.** This cell was written as "the L2 half of the `dangling`
/// diagnostic, which the engine composes by differencing this edge set against a fully-inclusive
/// `list_issues` id set — which is also why the diagnostic needs NO new `Storage` method". That
/// composition is SUPERSEDED (it cost 10.72s at 250k rows): the diagnostic now has its own read,
/// `dangling_dependencies`, pinned by the cell below. What survives here is a property of the
/// `dep graph` SURFACE itself: the whole-graph read reports the edges a workspace actually holds, and
/// a broken one is a fact about that workspace, not a row to be quietly resolved away.
///
/// MUTANT KILLED: "tidying" the whole-graph loader into a join against `issues` (or filtering out
/// unresolvable targets). Every other graph cell stays green — every edge in them resolves — while
/// `dep {action:"graph"}` silently under-reports a corrupt workspace's real shape.
pub async fn contract_dangling_edges_survive_the_whole_graph_read<S>(storage: S)
where
    S: Storage + StorageTestkit,
{
    storage
        .create_issue(&issue("ub-g-a", "a"), "x")
        .await
        .expect("a");
    storage
        .create_issue(&issue("ub-g-b", "b"), "x")
        .await
        .expect("b");
    // One resolvable edge, one dangling edge, one external edge.
    storage
        .add_dependency(&dep("ub-g-a", "ub-g-b", DependencyType::Blocks), "x")
        .await
        .expect("resolvable");
    storage
        .testkit_insert_raw_edge(&dep("ub-g-a", "ub-ghost", DependencyType::Blocks))
        .await
        .expect("plant a dangling edge");
    storage
        .add_dependency(
            &dep("ub-g-b", "external:jira-9", DependencyType::Blocks),
            "x",
        )
        .await
        .expect("external");

    let graph = storage.dependency_graph(&[]).await.expect("whole graph");
    let targets: Vec<&str> = graph.edges.iter().map(|e| e.to.as_str()).collect();
    assert!(
        targets.contains(&"ub-ghost"),
        "the whole-graph read must NOT resolve away a dangling edge — the `dangling` diagnostic \
         differences this set against the id set. Got {targets:?}"
    );
    assert!(
        targets.contains(&"external:jira-9"),
        "an external target also survives the read; classifying it is the engine's job, not the \
         loader's. Got {targets:?}"
    );
    assert!(targets.contains(&"ub-g-b"), "resolvable edges survive too");
}

/// **The D45 `dangling` view's ONE read** (`dangling_dependencies`, spine §3.2 / §3.2.1 as AMENDED
/// 2026-08-02): a LEFT JOIN returning ONLY the edges whose target row is ABSENT.
///
/// The amendment replaced a two-read engine composition that cost 10.72s at 250k rows and took
/// `doctor()` past the 15s `scale` guard. Its correctness rules did NOT change shape-for-shape, so
/// this cell asserts all four at once, on one corpus:
///
/// MUTANTS KILLED:
/// 1. **A STATUS-AWARE JOIN** — the single easiest way to ship a query that lies, and the SQL form of
///    the trap the retired wording carried ("the id set MUST come from FULLY-INCLUSIVE filters, or
///    every CLOSED blocker is reported as dangling"). Adding `AND i.status NOT IN
///    ('closed','tombstone')` to the `ON`/`WHERE` makes the CLOSED and TOMBSTONED blockers below
///    surface as false findings. Both are here, plus a DEFERRED one, because a mutant that excludes
///    only one status class must still go red.
/// 2. **Dropping `NOT LIKE 'external:%'`** — the two `external:` edges become false findings; the
///    UPPERCASE spelling additionally kills a case-SENSITIVE narrowing of that predicate (`SQLite`'s
///    `LIKE` folds ASCII, and §1.9 invariant 4 forbids the write guard being stricter than the read).
/// 3. **Returning every edge** (dropping `WHERE i.id IS NULL`, i.e. an inner-join/no-filter read) —
///    the resolvable edge appears.
/// 4. **Losing the `ORDER BY`, or ordering on the wrong columns.** The planted set is chosen so the
///    pinned `(issue_id, type, depends_on_id)` order DISAGREES with `(issue_id, depends_on_id)`:
///    `ub-d-a` carries `parent-child -> ub-aaa` and `blocks -> ub-zzz`, so target-order would put
///    `ub-aaa` first while the pinned order puts `blocks` first.
#[allow(clippy::too_many_lines)] // one corpus, four mutants: splitting it would split the pins
pub async fn contract_dangling_dependencies_lists_only_absent_targets<S>(storage: S)
where
    S: Storage + StorageTestkit,
{
    for (id, title) in [
        ("ub-d-a", "dependent"),
        ("ub-d-b", "live blocker"),
        ("ub-d-closed", "closed blocker"),
        ("ub-d-deferred", "deferred blocker"),
        ("ub-d-dead", "tombstoned blocker"),
    ] {
        storage
            .create_issue(&issue(id, title), "x")
            .await
            .expect(id);
    }

    // Legitimate edges: a live, a CLOSED, a DEFERRED and a TOMBSTONED blocker — every one of those
    // rows EXISTS, so none of these is dangling.
    for (target, ty) in [
        ("ub-d-b", DependencyType::Blocks),
        ("ub-d-closed", DependencyType::Blocks),
        ("ub-d-deferred", DependencyType::Blocks),
        ("ub-d-dead", DependencyType::Blocks),
    ] {
        storage
            .add_dependency(&dep("ub-d-a", target, ty), "x")
            .await
            .expect("an edge to an EXISTING row");
    }
    // External targets, in BOTH ASCII cases — legitimate blockers outside this workspace.
    storage
        .add_dependency(
            &dep("ub-d-a", "external:jira-1", DependencyType::WaitsFor),
            "x",
        )
        .await
        .expect("external lower");
    storage
        .add_dependency(
            &dep("ub-d-b", "EXTERNAL:jira-2", DependencyType::WaitsFor),
            "x",
        )
        .await
        .expect("external upper");

    // Now make three of those targets non-active — WITHOUT deleting their rows.
    storage
        .update_issue(
            "ub-d-closed",
            &IssuePatch {
                status: Some(Status::Closed),
                ..IssuePatch::default()
            },
            "x",
        )
        .await
        .expect("close");
    storage
        .update_issue(
            "ub-d-deferred",
            &IssuePatch {
                status: Some(Status::Deferred),
                ..IssuePatch::default()
            },
            "x",
        )
        .await
        .expect("defer");
    storage
        .delete_issue(
            &DeletePlan {
                mode: DeleteMode::Tombstone,
                targets: vec!["ub-d-dead".to_string()],
                cascade_children: Vec::new(),
            },
            "x",
        )
        .await
        .expect("tombstone");

    // The genuinely broken edges — plantable only through the testkit seam, since the D45 write
    // guard refuses to create them through every public path.
    storage
        .testkit_insert_raw_edge(&dep("ub-d-a", "ub-zzz", DependencyType::Blocks))
        .await
        .expect("plant");
    storage
        .testkit_insert_raw_edge(&dep("ub-d-a", "ub-aaa", DependencyType::ParentChild))
        .await
        .expect("plant");
    storage
        .testkit_insert_raw_edge(&dep("ub-d-b", "ub-mmm", DependencyType::WaitsFor))
        .await
        .expect("plant");

    let found: Vec<(String, String, String)> = storage
        .dangling_dependencies()
        .await
        .expect("dangling_dependencies")
        .into_iter()
        .map(|e| (e.from, e.dep_type.as_str().to_string(), e.to))
        .collect();

    assert_eq!(
        found,
        vec![
            (
                "ub-d-a".to_string(),
                "blocks".to_string(),
                "ub-zzz".to_string()
            ),
            (
                "ub-d-a".to_string(),
                "parent-child".to_string(),
                "ub-aaa".to_string()
            ),
            (
                "ub-d-b".to_string(),
                "waits-for".to_string(),
                "ub-mmm".to_string()
            ),
        ],
        "ONLY the absent-target edges, in the pinned (issue_id, dep_type, depends_on_id) order. A \
         closed / deferred / tombstoned blocker EXISTS and is NOT dangling; an `external:` target \
         never is, in either case"
    );
}

/// The §1.9 EQUIVALENCE obligation: the Rust `is_external_target` and the SQL twin
/// `LIKE 'external:%'` accept precisely the same strings, over a probe corpus PINNED in the spine so
/// it cannot silently shrink.
///
/// The `external:` predicate has two homes by construction — SQL cannot call Rust — and they agree
/// only BY CONTRACT. This cell is that contract's teeth: it asks the DATABASE ITSELF for its verdict
/// and compares. The corpus deliberately includes both ASCII cases, a partial prefix, a
/// leading-space form, an embedded form and two non-ASCII near-misses (fullwidth letters, the dotless
/// `ı`) — `SQLite`'s `LIKE` folds ASCII ONLY.
///
/// MUTANTS KILLED: a case-SENSITIVE Rust predicate (the two uppercase probes go red — and that is
/// the exact asymmetry the two shipped dialects had, where the write guard would be STRICTER than
/// the read side); a Unicode case-FOLDING predicate (the two non-ASCII near-misses go red); and any
/// future narrowing of either half.
pub async fn contract_external_predicate_matches_the_sql_twin<S>(storage: S)
where
    S: Storage + StorageTestkit,
{
    // The corpus is pinned in spine §1.9 invariant (3).
    let corpus = [
        "",
        "external",
        "external:",
        "external:jira-1",
        "EXTERNAL:jira-1",
        "ExTeRnAl:jira-1",
        "externally:x",
        "externaL:",
        " external:x",
        "ub-external:x",
        "ＥＸＴＥＲＮＡＬ:x",
        "externaı:x",
    ];

    for probe in corpus {
        let rust = unblock_model::is_external_target(probe);
        let sql = storage
            .testkit_sql_matches_external_prefix(probe)
            .await
            .expect("the backend answers its own external-target predicate");
        assert_eq!(
            rust, sql,
            "the Rust write-guard predicate and the SQL read-side twin disagree on {probe:?} \
             (rust={rust}, sql={sql}) — a write the store is happy to serve would be refused, or \
             a phantom blocker would slip past the guard"
        );
    }

    // Non-vacuity: the corpus must exercise BOTH verdicts, or an always-false predicate would pass.
    assert!(
        corpus.iter().any(|p| unblock_model::is_external_target(p)),
        "the corpus must contain at least one external target"
    );
    assert!(
        corpus.iter().any(|p| !unblock_model::is_external_target(p)),
        "the corpus must contain at least one non-external target"
    );
}
