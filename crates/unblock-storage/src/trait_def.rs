//! The backend-agnostic async [`Storage`] trait (spine §3.2) — the contract every backend
//! implements. This file is **pure declaration + doc-comments**: no backend, no I/O. The libsql
//! implementation lands at T0.6; the backend-independent contract suite that *verifies* these
//! preconditions lands at T0.7.
//!
//! The trait is **object-safe** (`Arc<dyn Storage>` is the shape `unblock-config` builds and
//! `unblock-engine` consumes, spine §4): every method takes `&self`, all are `async fn` lowered by
//! `#[async_trait]` to `Pin<Box<dyn Future>>`, and there are no generic methods.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use unblock_model::{
    CountBucket, CountGroupBy, DepTree, Dependency, DependencyType, Event, Issue, ListFilters,
};

use crate::error::StorageError;
use crate::filters::{DeletePlan, IssuePatch};

// NOTE: the CF-E reserved seams (`read_config`/`diagnostic_probe`/`diagnostic_probes`) reference
// `DiagnosticKind`/`DiagnosticReport`. They are kept **commented** below per spine §3.2 (reserved
// for v1.1, NOT live default methods), so those DTOs are intentionally NOT imported here — importing
// them would trip the unused-import lint.

/// The backend-agnostic storage contract (spine §3.2).
///
/// Async throughout (`#[async_trait]`); `Send + Sync` so it can be shared as `Arc<dyn Storage>`
/// across tokio tasks. The only backend-aware implementation is libsql (T0.6); a future backend
/// reuses the T0.7 contract suite. **No backend type appears in any signature** — failures surface
/// as [`StorageError`] (spine §6 rule 2).
///
/// # General invariants (honoured by the T0.6 impl, verified by the T0.7 suite)
///
/// - **Transactional audit (FR-9):** every mutation writes its [`Event`](unblock_model::Event)(s)
///   in the **same transaction** as the row change — rows and audit commit together or not at all.
/// - **No git, no network (NFR-6):** no method shells to git or links a git library; reads are
///   plain WAL reads. The `remote` path (T0.6+, non-default) is the only network surface.
/// - **Reads never serialize:** the write-serialization permit lives in `unblock-engine` (D14);
///   storage reads run concurrently against WAL readers (FR-10).
/// - **Storage never imports policy (CF-11):** ready/blocked ordering is deterministic for stable
///   snapshots, but the hybrid re-rank is applied by the engine via policy.
#[async_trait]
pub trait Storage: Send + Sync {
    // ---------------------------------------------------------------------------------------------
    // lifecycle
    // ---------------------------------------------------------------------------------------------

    /// Apply schema migrations to bring the database to the current version.
    ///
    /// Idempotent: re-running on an up-to-date database is a no-op. A database at a **newer**
    /// version than this build is rejected with [`StorageError::SchemaMismatch`]; a failed step
    /// surfaces [`StorageError::Migration`].
    async fn migrate(&self) -> Result<(), StorageError>;

    /// Run `PRAGMA integrity_check`, returning the raw problem rows.
    ///
    /// A healthy database returns an empty `Vec` (the `"ok"` sentinel is normalized away). Any
    /// returned strings are integrity problems to surface to the operator.
    async fn integrity_check(&self) -> Result<Vec<String>, StorageError>;

    // ---------------------------------------------------------------------------------------------
    // issue CRUD (mutations carry the actor + optional Tier-1 attribution; write Event(s) in-tx)
    // ---------------------------------------------------------------------------------------------

    /// Create an issue, returning its allocated id.
    ///
    /// Validates via the model `IssueValidator`, allocates the id, dedups by `content_hash`
    /// (FR-26 idempotency), inserts the row, and writes an `Event(Created)` in the same
    /// transaction. `actor` is the attributed author.
    async fn create_issue(&self, issue: &Issue, actor: &str) -> Result<String, StorageError>;

    /// Fetch a single issue by id, hydrated with its labels and dependencies.
    ///
    /// Returns `Ok(None)` when no issue matches (a missing issue is **not** an error here; callers
    /// that require existence map `None` to [`StorageError::IssueNotFound`] themselves).
    async fn get_issue(&self, id: &str) -> Result<Option<Issue>, StorageError>;

    /// Fetch multiple issues by id (hydrated). Unknown ids are simply absent from the result.
    async fn get_issues(&self, ids: &[String]) -> Result<Vec<Issue>, StorageError>;

    /// Apply an [`IssuePatch`] to an issue, returning the updated issue.
    ///
    /// Writes **one `Event` per changed field** in the same transaction (so the audit log records
    /// exactly what changed). A **no-op update** (a patch that changes nothing) writes **no
    /// `Event`** and leaves `updated_at` unchanged. A `parent` change is cycle-checked (rejected
    /// with [`StorageError::CycleDetected`] carrying the path).
    async fn update_issue(
        &self,
        id: &str,
        patch: &IssuePatch,
        actor: &str,
    ) -> Result<Issue, StorageError>;

    /// Execute (or, for [`DeleteMode::DryRun`](crate::DeleteMode), plan) a delete.
    ///
    /// Returns the **resolved** [`DeletePlan`] (with `cascade_children` populated for every mode).
    /// Semantics by mode:
    /// - **`DryRun`** mutates nothing and returns the plan (the full blast radius).
    /// - **`Tombstone`** sets `status = Tombstone` + the `deleted_*` fields and **preserves
    ///   `original_type`**.
    /// - **`Cascade`** tombstones the targets and their children.
    /// - **`Hard`** permanently deletes the rows.
    ///
    /// Every non-`DryRun` mode writes an `Event(Deleted)` per affected issue in the same
    /// transaction.
    async fn delete_issue(
        &self,
        plan: &DeletePlan,
        actor: &str,
    ) -> Result<DeletePlan, StorageError>;

    // ---------------------------------------------------------------------------------------------
    // atomic claim (FR-2)
    // ---------------------------------------------------------------------------------------------

    /// Atomically claim an issue for `assignee` (sets assignee + `in_progress`), with no race
    /// window (FR-2).
    ///
    /// The claim is a single conditional `UPDATE` so concurrent claimers cannot both win. There are
    /// exactly **three** outcomes:
    /// - **Unassigned** → succeeds: sets `assignee` + `status = in_progress` and writes a
    ///   transactional `Event`.
    /// - **Held by a *different* actor** → fails with [`StorageError::AlreadyClaimed`] whose `by`
    ///   field is the current holder, **re-read within the same transaction** (so the loser learns
    ///   who won).
    /// - **Re-claimed by the *same* assignee** → **idempotent `Ok`** (NOT an error): re-claiming
    ///   what you already hold returns the issue unchanged.
    async fn claim_issue(
        &self,
        id: &str,
        assignee: &str,
        actor: &str,
    ) -> Result<Issue, StorageError>;

    // ---------------------------------------------------------------------------------------------
    // defer / undefer (FR-3)
    // ---------------------------------------------------------------------------------------------

    /// Defer an issue until `until` (sets `defer_until`), writing a transactional `Event`.
    ///
    /// A deferred issue is excluded from [`ready_issues`](Storage::ready_issues) until `until`
    /// passes (or it is undeferred).
    async fn defer_issue(
        &self,
        id: &str,
        until: DateTime<Utc>,
        actor: &str,
    ) -> Result<Issue, StorageError>;

    /// Undefer an issue (clears `defer_until`), writing a transactional `Event`. The issue becomes
    /// ready-eligible again immediately (subject to its gating dependencies).
    async fn undefer_issue(&self, id: &str, actor: &str) -> Result<Issue, StorageError>;

    // ---------------------------------------------------------------------------------------------
    // queries (FR-4)
    // ---------------------------------------------------------------------------------------------

    /// List issues matching `filters` (status/type OR within, labels AND/OR, priority range, text
    /// LIKE, include-deferred/closed, limit/offset).
    async fn list_issues(&self, filters: &ListFilters) -> Result<Vec<Issue>, StorageError>;

    /// Return the **ready** candidate set: open, undeferred, and not blocked by any unresolved
    /// gating dependency.
    ///
    /// The set is **default-complete** (unlimited unless `filters.limit` is set) and returned in a
    /// **deterministic order** — `priority` ASC, then `created_at` ASC, then `id` ASC — so output
    /// snapshots are stable (NFR-14). Storage does **not** import policy (CF-11): the engine
    /// re-ranks this candidate set with the hybrid sort; storage only guarantees the stable order.
    async fn ready_issues(&self, filters: &ListFilters) -> Result<Vec<Issue>, StorageError>;

    /// Return the **blocked** set: non-terminal issues (`status NOT IN ('closed','tombstone')`,
    /// deferred-INCLUSIVE) with **at least one unresolved gating edge** (a
    /// `blocks`/`parent-child`/`conditional-blocks`/`waits-for` dependency on a not-yet-closed
    /// issue).
    ///
    /// `filters` **compose** (D18, spine §3.2.1): the same narrowing facets `list_issues` applies
    /// (status-OR, `issue_type`-OR, priority range, `assignee`, `labels_all`/`labels_any`,
    /// `text_contains`) narrow the candidate rows before the live membership test. The baseline is
    /// deferred-inclusive and does NOT inherit `list`'s default visibility, so
    /// `include_closed`/`include_deferred` are **no-ops** here.
    ///
    /// Ready and blocked are **disjoint** but not jointly exhaustive (a closed issue is neither; a
    /// deferred issue is blocked only if it has an unresolved gating edge).
    async fn blocked_issues(&self, filters: &ListFilters) -> Result<Vec<Issue>, StorageError>;

    /// Full-text-ish search over `query` honouring `filters`.
    ///
    /// v1 uses a `LIKE` scan over `title` + `description`, **`ESCAPE`-guarded** so `%`/`_`/the
    /// escape char in `query` are matched literally (no injection, no accidental wildcards). Honours
    /// `filters.limit`; the engine applies the default cap of 50 when no limit is set.
    async fn search_issues(
        &self,
        query: &str,
        filters: &ListFilters,
    ) -> Result<Vec<Issue>, StorageError>;

    /// Count issues matching `filters`, optionally grouped (by status/type/assignee/priority/label).
    ///
    /// With `group_by = None`, returns a single bucket with the total count. For
    /// `Status`/`Type`/`Assignee`/`Priority` the per-group counts **sum to the ungrouped total** over
    /// the same filter (each issue lands in exactly one bucket). **`Label` is the exception:** an
    /// issue is counted **once per label it carries** (the label JOIN), so the `Label` group sum
    /// equals the number of `(issue, label)` pairs among the matching issues — which can be greater
    /// than the total (a multi-label issue) **or** less than it (label-less issues contribute zero).
    /// It is therefore **not** related to the total by a simple `==` or `>=`.
    async fn count_issues(
        &self,
        filters: &ListFilters,
        group_by: Option<CountGroupBy>,
    ) -> Result<Vec<CountBucket>, StorageError>;

    /// Return issues not updated since `older_than` (i.e. `updated_at < older_than`) that match
    /// `filters`.
    async fn stale_issues(
        &self,
        older_than: DateTime<Utc>,
        filters: &ListFilters,
    ) -> Result<Vec<Issue>, StorageError>;

    // ---------------------------------------------------------------------------------------------
    // dependencies (FR-5)
    // ---------------------------------------------------------------------------------------------

    /// Add a dependency edge, writing a transactional `Event(DependencyAdded)`.
    ///
    /// Rejects [`StorageError::SelfDependency`] and [`StorageError::DuplicateDependency`]. Cycle
    /// gating uses **exactly** `DependencyType::affects_ready_work`
    /// (`Blocks` | `ParentChild` | `ConditionalBlocks` | `WaitsFor`); a new edge that would close a
    /// cycle over that gating set is rejected with [`StorageError::CycleDetected`] carrying the
    /// concrete `path`. A non-gating edge (e.g. `Related`) never creates a ready-gating cycle.
    async fn add_dependency(&self, dep: &Dependency, actor: &str) -> Result<(), StorageError>;

    /// Remove a dependency edge, writing a transactional `Event(DependencyRemoved)`.
    async fn remove_dependency(
        &self,
        issue_id: &str,
        depends_on_id: &str,
        dep_type: &DependencyType,
        actor: &str,
    ) -> Result<(), StorageError>;

    /// List the dependencies declared *by* `id`.
    async fn list_dependencies(&self, id: &str) -> Result<Vec<Dependency>, StorageError>;

    /// Return the dependency subtree **rooted at `id`** as a [`DepTree`].
    async fn dependency_tree(&self, id: &str) -> Result<DepTree, StorageError>;

    /// Return the dependency graph for a **root set** as a [`DepTree`].
    ///
    /// Backs the `dep graph` action. An **empty `roots`** slice means the **whole graph** (every
    /// edge); a non-empty `roots` returns the union of the subgraphs reachable from those roots.
    async fn dependency_graph(&self, roots: &[String]) -> Result<DepTree, StorageError>;

    /// Detect every dependency cycle, returning each as an **ordered traversal witness**: a
    /// multi-node cycle is `[start, …, start]` (the start repeated at the end), a self-loop is
    /// `[node, node]`; an acyclic graph returns `[]`. The outer `Vec` is deterministically ordered
    /// (NFR-14). NOT a sorted SCC node set (spine §3.2.1, D3).
    ///
    /// `blocking_only=true` restricts the cycle graph to the 4 gating types
    /// (`DependencyType::affects_ready_work`) — the ready-work view; `=false` considers **all**
    /// dependency types — the integrity/lint view. `parent-child` is inserted reversed regardless
    /// (D4/D19). The trait takes a bare `bool`; the default-TRUE (gating-only) is a wire-only
    /// contract on the MCP `Cycles` input (spine §5.2).
    async fn detect_cycles(&self, blocking_only: bool) -> Result<Vec<Vec<String>>, StorageError>;

    // ---------------------------------------------------------------------------------------------
    // events (audit; append-only)
    // ---------------------------------------------------------------------------------------------

    /// List the append-only audit events for `issue_id`, oldest first.
    async fn list_events(&self, issue_id: &str) -> Result<Vec<Event>, StorageError>;

    // ---------------------------------------------------------------------------------------------
    // diagnostics support (FR-15, pure-DB; no git, no network — NFR-6)
    // ---------------------------------------------------------------------------------------------

    /// Return issues closed since `since` (or all closed issues when `since` is `None`), by
    /// `closed_at` — the changelog source. Pure-DB; **never** shells to git (NFR-6).
    async fn closed_since(&self, since: Option<DateTime<Utc>>) -> Result<Vec<Issue>, StorageError>;

    /// Return orphan candidates: issues whose `external_ref` matches the commit-hash pattern.
    ///
    /// The pattern match runs in SQL/Rust — it **never** invokes git or the network (NFR-6); the
    /// caller (health/diagnostics) decides what to do with the candidates.
    async fn orphan_candidates(&self) -> Result<Vec<Issue>, StorageError>;

    // ---------------------------------------------------------------------------------------------
    // [v1.1] reserved seams (CF-E; spine §3.2) — additive, depended on by config db-layer +
    //         health full-taxonomy. Kept COMMENTED (not live default methods) so the seam is
    //         reserved without v1 behaviour and this file does not `use` the diagnostics DTOs.
    // ---------------------------------------------------------------------------------------------
    //
    // [v1.1] async fn read_config(&self) -> Result<Vec<(String, String)>, StorageError>;
    // [v1.1] async fn diagnostic_probe(&self, kind: DiagnosticKind) -> Result<DiagnosticReport, StorageError>;
    // [v1.1] async fn diagnostic_probes(&self) -> Result<Vec<DiagnosticReport>, StorageError>;
}

/// Object-safety guard: this signature only compiles if [`Storage`] is object-safe (i.e. usable as
/// `dyn Storage`). It is never called.
#[cfg(test)]
fn _assert_object_safe(_: &dyn Storage) {}

#[cfg(test)]
mod tests {
    use super::Storage;
    use crate::error::StorageError;
    use crate::filters::{DeletePlan, IssuePatch};
    use async_trait::async_trait;
    use chrono::{DateTime, Utc};
    use std::sync::Arc;
    use unblock_model::{
        CountBucket, CountGroupBy, DepTree, Dependency, DependencyType, Event, Issue, ListFilters,
    };

    /// A backend-free [`Storage`] used only to prove the trait is implementable and object-safe.
    ///
    /// Every method returns an explicitly-constructed value or `Err(StorageError::NotInitialized)`
    /// — never `Default::default()` on a type that has none (`DeletePlan`/`Issue`/`DepTree`).
    struct NoopStorage;

    #[async_trait]
    impl Storage for NoopStorage {
        async fn migrate(&self) -> Result<(), StorageError> {
            Err(StorageError::NotInitialized)
        }

        async fn integrity_check(&self) -> Result<Vec<String>, StorageError> {
            Ok(Vec::new())
        }

        async fn create_issue(&self, _issue: &Issue, _actor: &str) -> Result<String, StorageError> {
            Err(StorageError::NotInitialized)
        }

        async fn get_issue(&self, _id: &str) -> Result<Option<Issue>, StorageError> {
            Ok(None)
        }

        async fn get_issues(&self, _ids: &[String]) -> Result<Vec<Issue>, StorageError> {
            Ok(Vec::new())
        }

        async fn update_issue(
            &self,
            _id: &str,
            _patch: &IssuePatch,
            _actor: &str,
        ) -> Result<Issue, StorageError> {
            Err(StorageError::NotInitialized)
        }

        async fn delete_issue(
            &self,
            plan: &DeletePlan,
            _actor: &str,
        ) -> Result<DeletePlan, StorageError> {
            Ok(plan.clone())
        }

        async fn claim_issue(
            &self,
            _id: &str,
            _assignee: &str,
            _actor: &str,
        ) -> Result<Issue, StorageError> {
            Err(StorageError::NotInitialized)
        }

        async fn defer_issue(
            &self,
            _id: &str,
            _until: DateTime<Utc>,
            _actor: &str,
        ) -> Result<Issue, StorageError> {
            Err(StorageError::NotInitialized)
        }

        async fn undefer_issue(&self, _id: &str, _actor: &str) -> Result<Issue, StorageError> {
            Err(StorageError::NotInitialized)
        }

        async fn list_issues(&self, _filters: &ListFilters) -> Result<Vec<Issue>, StorageError> {
            Ok(Vec::new())
        }

        async fn ready_issues(&self, _filters: &ListFilters) -> Result<Vec<Issue>, StorageError> {
            Ok(Vec::new())
        }

        async fn blocked_issues(&self, _filters: &ListFilters) -> Result<Vec<Issue>, StorageError> {
            Ok(Vec::new())
        }

        async fn search_issues(
            &self,
            _query: &str,
            _filters: &ListFilters,
        ) -> Result<Vec<Issue>, StorageError> {
            Ok(Vec::new())
        }

        async fn count_issues(
            &self,
            _filters: &ListFilters,
            _group_by: Option<CountGroupBy>,
        ) -> Result<Vec<CountBucket>, StorageError> {
            Ok(Vec::new())
        }

        async fn stale_issues(
            &self,
            _older_than: DateTime<Utc>,
            _filters: &ListFilters,
        ) -> Result<Vec<Issue>, StorageError> {
            Ok(Vec::new())
        }

        async fn add_dependency(
            &self,
            _dep: &Dependency,
            _actor: &str,
        ) -> Result<(), StorageError> {
            Err(StorageError::NotInitialized)
        }

        async fn remove_dependency(
            &self,
            _issue_id: &str,
            _depends_on_id: &str,
            _dep_type: &DependencyType,
            _actor: &str,
        ) -> Result<(), StorageError> {
            Err(StorageError::NotInitialized)
        }

        async fn list_dependencies(&self, _id: &str) -> Result<Vec<Dependency>, StorageError> {
            Ok(Vec::new())
        }

        async fn dependency_tree(&self, id: &str) -> Result<DepTree, StorageError> {
            Ok(DepTree {
                root: id.to_string(),
                edges: Vec::new(),
            })
        }

        async fn dependency_graph(&self, roots: &[String]) -> Result<DepTree, StorageError> {
            Ok(DepTree {
                root: roots.first().cloned().unwrap_or_default(),
                edges: Vec::new(),
            })
        }

        async fn detect_cycles(
            &self,
            _blocking_only: bool,
        ) -> Result<Vec<Vec<String>>, StorageError> {
            Ok(Vec::new())
        }

        async fn list_events(&self, _issue_id: &str) -> Result<Vec<Event>, StorageError> {
            Ok(Vec::new())
        }

        async fn closed_since(
            &self,
            _since: Option<DateTime<Utc>>,
        ) -> Result<Vec<Issue>, StorageError> {
            Ok(Vec::new())
        }

        async fn orphan_candidates(&self) -> Result<Vec<Issue>, StorageError> {
            Ok(Vec::new())
        }
    }

    /// Drive the `Arc<dyn Storage>` coercion: this only compiles/runs if `Storage` is object-safe
    /// and every method is implementable through a trait object.
    #[tokio::test]
    async fn arc_dyn_storage_coercion() {
        let storage: Arc<dyn Storage> = Arc::new(NoopStorage);

        // A read path returns an explicitly-constructed value.
        assert!(storage.integrity_check().await.expect("ok").is_empty());
        assert!(storage.get_issue("ub-1").await.expect("ok").is_none());

        // The DryRun plan round-trips through the trait object unchanged.
        let plan = DeletePlan {
            mode: crate::DeleteMode::DryRun,
            targets: vec!["ub-1".to_string()],
            cascade_children: Vec::new(),
        };
        let returned = storage.delete_issue(&plan, "tester").await.expect("ok");
        assert_eq!(returned.targets, plan.targets);

        // dependency_graph([]) over the whole graph is reachable through the trait object.
        let tree = storage.dependency_graph(&[]).await.expect("ok");
        assert!(tree.edges.is_empty());

        // An error path maps to the typed error (not a panic).
        assert!(matches!(
            storage.migrate().await,
            Err(StorageError::NotInitialized)
        ));
    }
}
