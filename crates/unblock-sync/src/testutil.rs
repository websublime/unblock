//! Backend-free test doubles (SH-5) — a FULL object-safe `#[async_trait] Storage` impl.
//!
//! Only the methods `unblock-sync` actually drives are meaningful (`list_issues` honours
//! `include_tombstone`/`include_closed`, `get_issue` RETURNS tombstoned rows, `create_issues`
//! records the batch); every other method is `unimplemented!()` so a mis-wired call is a loud test
//! failure, never a silent stub. Call counters let the tombstone/idempotency/atomicity tests assert
//! "ONE `create_issues`, zero per-record `create_issue`" and "zero writes on reject".

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use unblock_model::{
    CountBucket, CountGroupBy, DepTree, Dependency, DependencyType, Event, Issue, ListFilters,
    Status,
};
use unblock_storage::{DeletePlan, IssuePatch, Storage, StorageError};

/// A deterministic sample issue with `created_at == updated_at` (second precision so `sync_equals`
/// holds across a serialize→parse round-trip).
#[must_use]
pub fn sample_issue(id: &str) -> Issue {
    let ts = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    Issue {
        id: id.to_string(),
        title: format!("issue {id}"),
        status: Status::Open,
        created_at: ts,
        updated_at: ts,
        ..Issue::default()
    }
}

/// A tombstoned sample issue for the given id (via the model tombstone transition).
#[must_use]
pub fn tombstone_of(id: &str) -> Issue {
    let now: DateTime<Utc> = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
    sample_issue(id).into_tombstone(Some("admin".into()), Some("gone".into()), now)
}

/// A backend-free `Storage` double for the sync unit tests.
pub struct FakeStorage {
    /// The current row set keyed by id (mutated by `create_issues`).
    rows: Mutex<HashMap<String, Issue>>,
    create_issues_calls: AtomicUsize,
    create_issue_calls: AtomicUsize,
}

impl FakeStorage {
    /// Build a double pre-seeded with `issues`.
    #[must_use]
    pub fn with_issues(issues: Vec<Issue>) -> Self {
        let rows = issues.into_iter().map(|i| (i.id.clone(), i)).collect();
        Self {
            rows: Mutex::new(rows),
            create_issues_calls: AtomicUsize::new(0),
            create_issue_calls: AtomicUsize::new(0),
        }
    }

    /// How many atomic `create_issues` (bulk) calls were made.
    #[must_use]
    pub fn create_issues_calls(&self) -> usize {
        self.create_issues_calls.load(Ordering::SeqCst)
    }

    /// How many single `create_issue` calls were made (must be 0 — the apply path is atomic-bulk).
    #[must_use]
    pub fn create_issue_calls(&self) -> usize {
        self.create_issue_calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Storage for FakeStorage {
    async fn list_issues(&self, filters: &ListFilters) -> Result<Vec<Issue>, StorageError> {
        let rows = self.rows.lock().expect("rows lock");
        let mut out: Vec<Issue> = rows
            .values()
            .filter(|i| {
                // Honour the visibility flags the export path uses (SH-5): default excludes
                // closed + tombstone + deferred; the widening flags include them.
                match i.status {
                    Status::Tombstone => filters.include_tombstone,
                    Status::Closed => filters.include_closed || filters.include_tombstone,
                    Status::Deferred => {
                        filters.include_deferred
                            || filters.include_closed
                            || filters.include_tombstone
                    }
                    _ => true,
                }
            })
            .cloned()
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    async fn get_issue(&self, id: &str) -> Result<Option<Issue>, StorageError> {
        // RETURNS tombstoned rows (the import anti-resurrection guard depends on it, SH-5).
        Ok(self.rows.lock().expect("rows lock").get(id).cloned())
    }

    async fn create_issues(&self, issues: &[Issue], _actor: &str) -> Result<(), StorageError> {
        self.create_issues_calls.fetch_add(1, Ordering::SeqCst);
        let mut rows = self.rows.lock().expect("rows lock");
        // Atomic: any in-batch id collision with an existing row rolls the WHOLE batch back (mirrors
        // the real `create_issues` all-or-nothing contract) — nothing is inserted on failure.
        for issue in issues {
            if rows.contains_key(&issue.id) {
                return Err(StorageError::IdCollision {
                    id: issue.id.clone(),
                });
            }
        }
        for issue in issues {
            rows.insert(issue.id.clone(), issue.clone());
        }
        Ok(())
    }

    async fn create_issue(&self, _issue: &Issue, _actor: &str) -> Result<String, StorageError> {
        // The atomic import apply MUST NOT use the per-record create path — track it so a regression
        // (a per-record loop) is caught by the call-count assertions.
        self.create_issue_calls.fetch_add(1, Ordering::SeqCst);
        unimplemented!("sync never uses per-record create_issue for import apply")
    }

    async fn update_issue(
        &self,
        _id: &str,
        _patch: &IssuePatch,
        _actor: &str,
    ) -> Result<Issue, StorageError> {
        unimplemented!("sync import apply is create-only; update is not on the atomic path")
    }

    // ---- everything below is genuinely unused by sync: loud stubs, never silent. ----

    async fn migrate(&self) -> Result<(), StorageError> {
        unimplemented!("FakeStorage::migrate")
    }
    async fn integrity_check(&self) -> Result<Vec<String>, StorageError> {
        unimplemented!("FakeStorage::integrity_check")
    }
    async fn schema_version(&self) -> Result<i64, StorageError> {
        unimplemented!("FakeStorage::schema_version")
    }
    async fn acquire_write_lock(
        &self,
    ) -> Result<Option<unblock_storage::WriteLockGuard>, StorageError> {
        // A file-less fake store has no cross-process workspace lock.
        Ok(None)
    }
    async fn get_issues(&self, _ids: &[String]) -> Result<Vec<Issue>, StorageError> {
        unimplemented!("FakeStorage::get_issues")
    }
    async fn delete_issue(
        &self,
        _plan: &DeletePlan,
        _actor: &str,
    ) -> Result<DeletePlan, StorageError> {
        unimplemented!("FakeStorage::delete_issue")
    }
    async fn restore_issue(&self, _id: &str, _actor: &str) -> Result<Issue, StorageError> {
        unimplemented!("FakeStorage::restore_issue")
    }
    async fn claim_issue(
        &self,
        _id: &str,
        _assignee: &str,
        _actor: &str,
    ) -> Result<Issue, StorageError> {
        unimplemented!("FakeStorage::claim_issue")
    }
    async fn defer_issue(
        &self,
        _id: &str,
        _until: DateTime<Utc>,
        _actor: &str,
    ) -> Result<Issue, StorageError> {
        unimplemented!("FakeStorage::defer_issue")
    }
    async fn undefer_issue(&self, _id: &str, _actor: &str) -> Result<Issue, StorageError> {
        unimplemented!("FakeStorage::undefer_issue")
    }
    async fn ready_issues(&self, _filters: &ListFilters) -> Result<Vec<Issue>, StorageError> {
        unimplemented!("FakeStorage::ready_issues")
    }
    async fn blocked_issues(&self, _filters: &ListFilters) -> Result<Vec<Issue>, StorageError> {
        unimplemented!("FakeStorage::blocked_issues")
    }
    async fn search_issues(
        &self,
        _query: &str,
        _filters: &ListFilters,
    ) -> Result<Vec<Issue>, StorageError> {
        unimplemented!("FakeStorage::search_issues")
    }
    async fn count_issues(
        &self,
        _filters: &ListFilters,
        _group_by: Option<CountGroupBy>,
    ) -> Result<Vec<CountBucket>, StorageError> {
        unimplemented!("FakeStorage::count_issues")
    }
    async fn stale_issues(
        &self,
        _older_than: DateTime<Utc>,
        _filters: &ListFilters,
    ) -> Result<Vec<Issue>, StorageError> {
        unimplemented!("FakeStorage::stale_issues")
    }
    async fn add_dependency(&self, _dep: &Dependency, _actor: &str) -> Result<(), StorageError> {
        unimplemented!("FakeStorage::add_dependency")
    }
    async fn remove_dependency(
        &self,
        _issue_id: &str,
        _depends_on_id: &str,
        _dep_type: &DependencyType,
        _actor: &str,
    ) -> Result<(), StorageError> {
        unimplemented!("FakeStorage::remove_dependency")
    }
    async fn list_dependencies(&self, _id: &str) -> Result<Vec<Dependency>, StorageError> {
        unimplemented!("FakeStorage::list_dependencies")
    }
    async fn next_child_number(&self, _parent_id: &str) -> Result<u32, StorageError> {
        unimplemented!("FakeStorage::next_child_number")
    }
    async fn dependency_tree(&self, _id: &str) -> Result<DepTree, StorageError> {
        unimplemented!("FakeStorage::dependency_tree")
    }
    async fn dependency_graph(&self, _roots: &[String]) -> Result<DepTree, StorageError> {
        unimplemented!("FakeStorage::dependency_graph")
    }
    async fn detect_cycles(&self, _blocking_only: bool) -> Result<Vec<Vec<String>>, StorageError> {
        unimplemented!("FakeStorage::detect_cycles")
    }
    async fn list_events(&self, _issue_id: &str) -> Result<Vec<Event>, StorageError> {
        unimplemented!("FakeStorage::list_events")
    }
    async fn epic_child_rollup(&self) -> Result<Vec<(String, (usize, usize))>, StorageError> {
        unimplemented!("FakeStorage::epic_child_rollup")
    }
    async fn closed_since(
        &self,
        _since: Option<DateTime<Utc>>,
    ) -> Result<Vec<Issue>, StorageError> {
        unimplemented!("FakeStorage::closed_since")
    }
    async fn orphan_candidates(&self) -> Result<Vec<Issue>, StorageError> {
        unimplemented!("FakeStorage::orphan_candidates")
    }
}
