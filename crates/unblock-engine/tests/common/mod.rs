//! Shared integration-test harness: a real-storage [`Session`] over an in-memory libsql workspace
//! (NOT a `Storage` mock — the engine's contract is "identical behaviour through one path", FR-9),
//! corpus builders, and a "park mid-tx" write helper for the read-during-write / cancel-safety tests.

#![allow(dead_code)] // each test binary uses a subset of the harness.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use unblock_config::{ConfigPaths, ResolvedConfig, WorkspaceContext};
use unblock_engine::{Session, SessionConfig};
use unblock_model::{Dependency, DependencyType, Issue, Priority, Status};
use unblock_storage::{LibsqlStorage, Storage};

/// Build a `Session` over a fresh in-memory libsql backend (migrated), wired into a synthetic
/// `WorkspaceContext` — the same shape `unblock-config` builds in production, but with an in-memory
/// DB so tests are fast and isolated. Real storage, no mock.
pub async fn session() -> Session {
    session_with(SessionConfig::default()).await
}

/// Like [`session`], but with explicit [`SessionConfig`] knobs.
pub async fn session_with(cfg: SessionConfig) -> Session {
    let storage = LibsqlStorage::open_in_memory()
        .await
        .expect("open in-memory");
    storage.migrate().await.expect("migrate");
    let storage: Arc<dyn Storage> = Arc::new(storage);
    session_over(storage, cfg).await
}

/// Build a `Session` over an already-built `Arc<dyn Storage>` (so two sessions can share one DB for
/// the dual-callsite identity test).
pub async fn session_over(storage: Arc<dyn Storage>, cfg: SessionConfig) -> Session {
    let workspace_dir = PathBuf::from("/tmp/unblock-test-ws");
    let unblock_dir = workspace_dir.join(".unblock");
    let config = ResolvedConfig::default();
    let paths = ConfigPaths {
        db_path: unblock_dir.join(&config.db_filename),
        jsonl_path: unblock_dir.join(&config.jsonl_filename),
        unblock_dir,
    };
    let ctx = WorkspaceContext {
        storage,
        workspace_dir,
        actor: "tester".to_string(),
        config,
        paths,
    };
    Session::open(ctx, cfg).await.expect("open session")
}

/// A fixed reference timestamp the corpus builders use (deterministic snapshots).
#[must_use]
pub fn t(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(secs, 0).single().expect("valid ts")
}

/// Build a minimal valid [`Issue`] with a valid id, title, and `created_at == updated_at`.
#[must_use]
pub fn issue(id: &str, priority: Priority, created_secs: i64) -> Issue {
    Issue {
        id: id.to_string(),
        title: format!("issue {id}"),
        priority,
        status: Status::Open,
        created_at: t(created_secs),
        updated_at: t(created_secs),
        ..Issue::default()
    }
}

/// Create `n` open issues `ub-0001..ub-{n}` through the engine, each P2 at a distinct `created_at`.
pub async fn seed_open(session: &Session, n: usize) -> Vec<String> {
    let mut ids = Vec::with_capacity(n);
    for i in 0..n {
        let id = format!("ub-{i:04}");
        let issue = issue(
            &id,
            Priority::MEDIUM,
            1_000 + i64::try_from(i).unwrap_or(i64::MAX),
        );
        let created = session.create(&issue).await.expect("create");
        ids.push(created);
    }
    ids
}

/// Seed the 6-issue authoritative Cascade/Hard/DryRun corpus (T1.4 LOCKED spec §2).
///
/// Dotted ids are required because `resolve_cascade_children` keys on the `{target}.%` dotted-id
/// prefix (crud.rs:835), NOT on parent-child edges. The reparent corpus uses flat ids; these two
/// corpora are intentionally separate (D-DRIFT-A in the LOCKED spec).
///
/// Corpus:
/// - `ub-1`   — delete target (root, Open)
/// - `ub-1.1` — child depth 1 (Open), non-terminal cascade member
/// - `ub-1.1.1` — grandchild depth 2 (Open), proves recursive prefix match
/// - `ub-1.2` — sibling child (Closed/terminal), proves Deleted-event guard
/// - `ub-10`  — dot-boundary decoy (shares "ub-1" prefix WITHOUT a dot — must be UNTOUCHED)
/// - `ub-2`   — unrelated root, bounded blast-radius witness
///
/// `resolve_cascade_children(["ub-1"]) == ["ub-1.1","ub-1.1.1","ub-1.2"]` (sorted).
pub async fn seed_hierarchy(session: &Session) {
    use unblock_engine::IssuePatch;
    use unblock_model::Status;

    session
        .create(&issue("ub-1", Priority::MEDIUM, 100))
        .await
        .expect("create ub-1");
    session
        .create(&issue("ub-1.1", Priority::MEDIUM, 101))
        .await
        .expect("create ub-1.1");
    session
        .create(&issue("ub-1.1.1", Priority::MEDIUM, 102))
        .await
        .expect("create ub-1.1.1");
    session
        .create(&issue("ub-1.2", Priority::MEDIUM, 103))
        .await
        .expect("create ub-1.2");
    // Close ub-1.2 (terminal) to prove the Deleted-event guard.
    session
        .update(
            "ub-1.2",
            &IssuePatch {
                status: Some(Status::Closed),
                ..IssuePatch::default()
            },
        )
        .await
        .expect("close ub-1.2");
    session
        .create(&issue("ub-10", Priority::MEDIUM, 104))
        .await
        .expect("create ub-10");
    session
        .create(&issue("ub-2", Priority::MEDIUM, 105))
        .await
        .expect("create ub-2");
}

/// Build a `Dependency` edge `from -> on` of the given type (a test fixture).
pub fn dep(from: &str, on: &str, dep_type: DependencyType) -> Dependency {
    Dependency {
        issue_id: from.to_string(),
        depends_on_id: on.to_string(),
        dep_type,
        created_at: Utc::now(),
        created_by: Some("tester".to_string()),
        metadata: None,
        thread_id: None,
    }
}

/// Add a `Blocks` dependency `from -> on` (from depends on / is blocked by `on`).
pub async fn add_blocks(session: &Session, from: &str, on: &str) {
    session
        .add_dep(&dep(from, on, DependencyType::Blocks))
        .await
        .expect("add_dep");
}

/// A `Storage` decorator that gates a chosen mutation on a [`tokio::sync::Notify`], so a write
/// driven through the engine **holds the engine's write permit** while parked inside the storage
/// transaction — the precise condition the FR-10 (read-during-write) and D14 (cancel-safety) tests
/// need. It delegates **every** call to a real inner `LibsqlStorage` (no behaviour mock): the only
/// added effect is a controllable pause at the start of the gated mutation, inside the engine's
/// permit-holding window.
pub mod parked {
    use super::{Arc, Storage};
    use async_trait::async_trait;
    use chrono::{DateTime, Utc};
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::Notify;
    use unblock_model::{
        CountBucket, CountGroupBy, DepTree, Dependency, DependencyType, Event, Issue,
    };
    use unblock_storage::{DeletePlan, IssuePatch, ListFilters, StorageError};

    /// A storage wrapper that parks the next `create_issue` until [`release`](ParkedStorage::release)
    /// is called, while delegating everything to the inner real storage.
    pub struct ParkedStorage {
        inner: Arc<dyn Storage>,
        gate: Notify,
        /// Set once the parked write has entered (so the test can wait for "the write is mid-tx").
        entered: Notify,
        armed: AtomicBool,
    }

    impl ParkedStorage {
        /// Wrap an inner storage; the first `create_issue` parks until released.
        #[must_use]
        pub fn new(inner: Arc<dyn Storage>) -> Arc<Self> {
            Arc::new(Self {
                inner,
                gate: Notify::new(),
                entered: Notify::new(),
                armed: AtomicBool::new(true),
            })
        }

        /// Wait until the parked write has entered the gated mutation (it is now holding the
        /// engine's write permit).
        pub async fn wait_until_parked(&self) {
            self.entered.notified().await;
        }

        /// Release the parked write so it completes its transaction.
        pub fn release(&self) {
            self.gate.notify_one();
        }
    }

    #[async_trait]
    impl Storage for ParkedStorage {
        async fn create_issue(&self, issue: &Issue, actor: &str) -> Result<String, StorageError> {
            if self.armed.swap(false, Ordering::SeqCst) {
                // Signal we are inside the gated mutation (engine permit held), then park.
                self.entered.notify_one();
                self.gate.notified().await;
            }
            self.inner.create_issue(issue, actor).await
        }

        async fn migrate(&self) -> Result<(), StorageError> {
            self.inner.migrate().await
        }
        async fn integrity_check(&self) -> Result<Vec<String>, StorageError> {
            self.inner.integrity_check().await
        }
        async fn get_issue(&self, id: &str) -> Result<Option<Issue>, StorageError> {
            self.inner.get_issue(id).await
        }
        async fn get_issues(&self, ids: &[String]) -> Result<Vec<Issue>, StorageError> {
            self.inner.get_issues(ids).await
        }
        async fn update_issue(
            &self,
            id: &str,
            patch: &IssuePatch,
            actor: &str,
        ) -> Result<Issue, StorageError> {
            self.inner.update_issue(id, patch, actor).await
        }
        async fn delete_issue(
            &self,
            plan: &DeletePlan,
            actor: &str,
        ) -> Result<DeletePlan, StorageError> {
            self.inner.delete_issue(plan, actor).await
        }
        async fn restore_issue(&self, id: &str, actor: &str) -> Result<Issue, StorageError> {
            self.inner.restore_issue(id, actor).await
        }
        async fn claim_issue(
            &self,
            id: &str,
            assignee: &str,
            actor: &str,
        ) -> Result<Issue, StorageError> {
            self.inner.claim_issue(id, assignee, actor).await
        }
        async fn defer_issue(
            &self,
            id: &str,
            until: DateTime<Utc>,
            actor: &str,
        ) -> Result<Issue, StorageError> {
            self.inner.defer_issue(id, until, actor).await
        }
        async fn undefer_issue(&self, id: &str, actor: &str) -> Result<Issue, StorageError> {
            self.inner.undefer_issue(id, actor).await
        }
        async fn list_issues(&self, filters: &ListFilters) -> Result<Vec<Issue>, StorageError> {
            self.inner.list_issues(filters).await
        }
        async fn ready_issues(&self, filters: &ListFilters) -> Result<Vec<Issue>, StorageError> {
            self.inner.ready_issues(filters).await
        }
        async fn blocked_issues(&self, filters: &ListFilters) -> Result<Vec<Issue>, StorageError> {
            self.inner.blocked_issues(filters).await
        }
        async fn search_issues(
            &self,
            query: &str,
            filters: &ListFilters,
        ) -> Result<Vec<Issue>, StorageError> {
            self.inner.search_issues(query, filters).await
        }
        async fn count_issues(
            &self,
            filters: &ListFilters,
            group_by: Option<CountGroupBy>,
        ) -> Result<Vec<CountBucket>, StorageError> {
            self.inner.count_issues(filters, group_by).await
        }
        async fn stale_issues(
            &self,
            older_than: DateTime<Utc>,
            filters: &ListFilters,
        ) -> Result<Vec<Issue>, StorageError> {
            self.inner.stale_issues(older_than, filters).await
        }
        async fn add_dependency(&self, dep: &Dependency, actor: &str) -> Result<(), StorageError> {
            self.inner.add_dependency(dep, actor).await
        }
        async fn remove_dependency(
            &self,
            issue_id: &str,
            depends_on_id: &str,
            dep_type: &DependencyType,
            actor: &str,
        ) -> Result<(), StorageError> {
            self.inner
                .remove_dependency(issue_id, depends_on_id, dep_type, actor)
                .await
        }
        async fn list_dependencies(&self, id: &str) -> Result<Vec<Dependency>, StorageError> {
            self.inner.list_dependencies(id).await
        }
        async fn next_child_number(&self, parent_id: &str) -> Result<u32, StorageError> {
            self.inner.next_child_number(parent_id).await
        }
        async fn dependency_tree(&self, id: &str) -> Result<DepTree, StorageError> {
            self.inner.dependency_tree(id).await
        }
        async fn dependency_graph(&self, roots: &[String]) -> Result<DepTree, StorageError> {
            self.inner.dependency_graph(roots).await
        }
        async fn detect_cycles(
            &self,
            blocking_only: bool,
        ) -> Result<Vec<Vec<String>>, StorageError> {
            self.inner.detect_cycles(blocking_only).await
        }
        async fn list_events(&self, issue_id: &str) -> Result<Vec<Event>, StorageError> {
            self.inner.list_events(issue_id).await
        }
        async fn closed_since(
            &self,
            since: Option<DateTime<Utc>>,
        ) -> Result<Vec<Issue>, StorageError> {
            self.inner.closed_since(since).await
        }
        async fn orphan_candidates(&self) -> Result<Vec<Issue>, StorageError> {
            self.inner.orphan_candidates().await
        }
    }
}

/// A `Storage` decorator that **forces the root-hash collision ladder to extend the hash length**.
///
/// The engine allocator (`session/ids.rs`) tries nonces `0..10` at the adaptive base length, then —
/// only if all ten collide — grows the length by one and retries (`ids.rs:93`, the EXTENSION branch).
/// To exercise that branch deterministically over real storage, this wrapper makes every root
/// candidate whose **hash segment is exactly `base_len` characters** appear already-occupied (it
/// returns a synthetic `Some(Issue)` from `get_issue` for those ids, recording each distinct one),
/// while delegating everything else to the inner real `LibsqlStorage`. Once the allocator extends to
/// `base_len + 1`, candidates stop being shadowed, so the real insert + re-read for the longer id pass
/// straight through. Every other call is a pure delegate (no behaviour mock).
pub mod collide {
    use super::{Arc, Storage};
    use async_trait::async_trait;
    use chrono::{DateTime, Utc};
    use std::collections::BTreeSet;
    use std::sync::Mutex;
    use unblock_model::{
        CountBucket, CountGroupBy, DepTree, Dependency, DependencyType, Event, Issue, parse_id,
    };
    use unblock_storage::{DeletePlan, IssuePatch, ListFilters, StorageError};

    /// Forces the hash-length-extension branch by shadowing every root candidate whose hash segment
    /// is exactly `base_len` chars.
    pub struct CollisionForcer {
        inner: Arc<dyn Storage>,
        /// The base adaptive hash length to shadow (= `optimal_hash_length(issue_count)`).
        base_len: usize,
        /// The distinct base-length root candidate ids the allocator probed (and we shadowed).
        shadowed: Mutex<BTreeSet<String>>,
    }

    impl CollisionForcer {
        /// Wrap `inner`; shadow every root candidate `*-<hash>` with `hash.len() == base_len`.
        #[must_use]
        pub fn new(inner: Arc<dyn Storage>, base_len: usize) -> Arc<Self> {
            Arc::new(Self {
                inner,
                base_len,
                shadowed: Mutex::new(BTreeSet::new()),
            })
        }

        /// How many DISTINCT base-length root candidates were probed (and reported occupied).
        #[must_use]
        pub fn shadowed_count(&self) -> usize {
            self.shadowed.lock().expect("forcer lock").len()
        }

        /// A root candidate to shadow = a parseable ROOT id (no child path) whose hash segment is
        /// exactly `base_len` characters. Slug/longer/child candidates are NOT shadowed.
        fn should_shadow(&self, id: &str) -> bool {
            match parse_id(id) {
                Ok(parsed) => parsed.is_root() && parsed.hash.len() == self.base_len,
                Err(_) => false,
            }
        }
    }

    #[async_trait]
    impl Storage for CollisionForcer {
        async fn get_issue(&self, id: &str) -> Result<Option<Issue>, StorageError> {
            if self.should_shadow(id) {
                // Report this base-length candidate as occupied so the allocator must move on; record
                // it so the test can assert all ten base-length nonces were exhausted.
                self.shadowed
                    .lock()
                    .expect("forcer lock")
                    .insert(id.to_string());
                let occupied = Issue {
                    id: id.to_string(),
                    title: "synthetic occupant".to_string(),
                    ..Issue::default()
                };
                return Ok(Some(occupied));
            }
            self.inner.get_issue(id).await
        }

        async fn create_issue(&self, issue: &Issue, actor: &str) -> Result<String, StorageError> {
            self.inner.create_issue(issue, actor).await
        }
        async fn migrate(&self) -> Result<(), StorageError> {
            self.inner.migrate().await
        }
        async fn integrity_check(&self) -> Result<Vec<String>, StorageError> {
            self.inner.integrity_check().await
        }
        async fn get_issues(&self, ids: &[String]) -> Result<Vec<Issue>, StorageError> {
            self.inner.get_issues(ids).await
        }
        async fn update_issue(
            &self,
            id: &str,
            patch: &IssuePatch,
            actor: &str,
        ) -> Result<Issue, StorageError> {
            self.inner.update_issue(id, patch, actor).await
        }
        async fn delete_issue(
            &self,
            plan: &DeletePlan,
            actor: &str,
        ) -> Result<DeletePlan, StorageError> {
            self.inner.delete_issue(plan, actor).await
        }
        async fn restore_issue(&self, id: &str, actor: &str) -> Result<Issue, StorageError> {
            self.inner.restore_issue(id, actor).await
        }
        async fn claim_issue(
            &self,
            id: &str,
            assignee: &str,
            actor: &str,
        ) -> Result<Issue, StorageError> {
            self.inner.claim_issue(id, assignee, actor).await
        }
        async fn defer_issue(
            &self,
            id: &str,
            until: DateTime<Utc>,
            actor: &str,
        ) -> Result<Issue, StorageError> {
            self.inner.defer_issue(id, until, actor).await
        }
        async fn undefer_issue(&self, id: &str, actor: &str) -> Result<Issue, StorageError> {
            self.inner.undefer_issue(id, actor).await
        }
        async fn list_issues(&self, filters: &ListFilters) -> Result<Vec<Issue>, StorageError> {
            self.inner.list_issues(filters).await
        }
        async fn ready_issues(&self, filters: &ListFilters) -> Result<Vec<Issue>, StorageError> {
            self.inner.ready_issues(filters).await
        }
        async fn blocked_issues(&self, filters: &ListFilters) -> Result<Vec<Issue>, StorageError> {
            self.inner.blocked_issues(filters).await
        }
        async fn search_issues(
            &self,
            query: &str,
            filters: &ListFilters,
        ) -> Result<Vec<Issue>, StorageError> {
            self.inner.search_issues(query, filters).await
        }
        async fn count_issues(
            &self,
            filters: &ListFilters,
            group_by: Option<CountGroupBy>,
        ) -> Result<Vec<CountBucket>, StorageError> {
            self.inner.count_issues(filters, group_by).await
        }
        async fn stale_issues(
            &self,
            older_than: DateTime<Utc>,
            filters: &ListFilters,
        ) -> Result<Vec<Issue>, StorageError> {
            self.inner.stale_issues(older_than, filters).await
        }
        async fn add_dependency(&self, dep: &Dependency, actor: &str) -> Result<(), StorageError> {
            self.inner.add_dependency(dep, actor).await
        }
        async fn remove_dependency(
            &self,
            issue_id: &str,
            depends_on_id: &str,
            dep_type: &DependencyType,
            actor: &str,
        ) -> Result<(), StorageError> {
            self.inner
                .remove_dependency(issue_id, depends_on_id, dep_type, actor)
                .await
        }
        async fn list_dependencies(&self, id: &str) -> Result<Vec<Dependency>, StorageError> {
            self.inner.list_dependencies(id).await
        }
        async fn next_child_number(&self, parent_id: &str) -> Result<u32, StorageError> {
            self.inner.next_child_number(parent_id).await
        }
        async fn dependency_tree(&self, id: &str) -> Result<DepTree, StorageError> {
            self.inner.dependency_tree(id).await
        }
        async fn dependency_graph(&self, roots: &[String]) -> Result<DepTree, StorageError> {
            self.inner.dependency_graph(roots).await
        }
        async fn detect_cycles(
            &self,
            blocking_only: bool,
        ) -> Result<Vec<Vec<String>>, StorageError> {
            self.inner.detect_cycles(blocking_only).await
        }
        async fn list_events(&self, issue_id: &str) -> Result<Vec<Event>, StorageError> {
            self.inner.list_events(issue_id).await
        }
        async fn closed_since(
            &self,
            since: Option<DateTime<Utc>>,
        ) -> Result<Vec<Issue>, StorageError> {
            self.inner.closed_since(since).await
        }
        async fn orphan_candidates(&self) -> Result<Vec<Issue>, StorageError> {
            self.inner.orphan_candidates().await
        }
    }
}
