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
    CountGroupBy, Dependency, DependencyType, Issue, IssueType, ListFilters, Priority, Status,
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

    // CRUD.
    contract_create_issue(factory().await).await;
    contract_get_issue(factory().await).await;
    contract_get_issues(factory().await).await;
    contract_update_issue(factory().await).await;
    contract_delete_issue(factory().await).await;

    // Claim / defer.
    contract_claim_issue_three_outcomes(factory().await).await;
    contract_claim_concurrent_exactly_one_winner(Arc::new(factory().await)).await;
    contract_defer_issue(factory().await).await;
    contract_undefer_issue(factory().await).await;

    // Queries.
    contract_list_issues(factory().await).await;
    contract_ready_issues(factory().await).await;
    contract_blocked_issues(factory().await).await;
    contract_ready_blocked_disjoint(factory().await).await;
    contract_search_issues(factory().await).await;
    contract_search_escape_guard(factory().await).await;
    contract_count_issues_sum_consistency(factory().await).await;
    contract_stale_issues(factory().await).await;

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
    contract_closed_since(factory().await).await;
    contract_orphan_candidates(factory().await).await;

    // Cross-cutting invariants.
    contract_noop_update_writes_no_event(factory().await).await;
    contract_dry_run_mutates_nothing(factory().await).await;
    contract_tombstone_preserves_type_and_event_rule(factory().await).await;
    contract_transactional_audit_atomicity(factory().await).await;

    // Seam-backed: id child-counter high-water mark.
    contract_child_counter_high_water(factory().await).await;
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

    // Acyclic graph → no cycles (and the call terminates).
    let cycles = storage.detect_cycles().await.unwrap();
    assert!(cycles.is_empty(), "acyclic graph yields no cycles");

    // The reverse gating edge would close a cycle → rejected with a path.
    match storage
        .add_dependency(&dep("ub-b", "ub-a", DependencyType::Blocks), "x")
        .await
    {
        Err(StorageError::CycleDetected { path }) => {
            assert!(path.contains("ub-b"), "cycle path names a node: {path}");
        }
        other => panic!("expected CycleDetected, got {other:?}"),
    }

    // A non-gating edge (Related) never gates ready work — the reverse edge is fine.
    storage
        .add_dependency(&dep("ub-b", "ub-a", DependencyType::Related), "x")
        .await
        .expect("related edges never cycle");
    assert!(
        storage.detect_cycles().await.unwrap().is_empty(),
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

    let cycles = storage.detect_cycles().await.unwrap();
    assert!(!cycles.is_empty(), "the stored gating cycle is detected");
    let names: std::collections::HashSet<&str> = cycles
        .iter()
        .flat_map(|path| path.iter().map(String::as_str))
        .collect();
    assert!(
        names.contains("ub-a") && names.contains("ub-b"),
        "the detected cycle path contains both nodes: {cycles:?}"
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
