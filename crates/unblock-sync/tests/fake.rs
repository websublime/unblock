//! Shared backend-free `Storage` double for the sync integration tests (SH-5).
//!
//! A FULL object-safe `#[async_trait] Storage` impl: only the methods sync drives are meaningful
//! (`list_issues` honours `include_tombstone`/`include_closed`, `get_issue` RETURNS tombstoned rows,
//! `create_issues` records the batch atomically); every other method is `unimplemented!()`.

#![allow(dead_code)] // each integration test binary uses a subset.
#![allow(missing_docs)] // internal test-support double; not a public API surface.

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

/// A deterministic second-precision sample issue.
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

/// A tombstoned sample issue for `id`.
#[must_use]
pub fn tombstone_of(id: &str) -> Issue {
    let now: DateTime<Utc> = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
    sample_issue(id).into_tombstone(Some("admin".into()), Some("gone".into()), now)
}

/// A backend-free `Storage` double.
pub struct FakeStorage {
    rows: Mutex<HashMap<String, Issue>>,
    create_issues_calls: AtomicUsize,
    create_issue_calls: AtomicUsize,
}

impl FakeStorage {
    #[must_use]
    pub fn with_issues(issues: Vec<Issue>) -> Self {
        let rows = issues.into_iter().map(|i| (i.id.clone(), i)).collect();
        Self {
            rows: Mutex::new(rows),
            create_issues_calls: AtomicUsize::new(0),
            create_issue_calls: AtomicUsize::new(0),
        }
    }

    #[must_use]
    pub fn create_issues_calls(&self) -> usize {
        self.create_issues_calls.load(Ordering::SeqCst)
    }

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
            .filter(|i| match i.status {
                Status::Tombstone => filters.include_tombstone,
                Status::Closed => filters.include_closed || filters.include_tombstone,
                Status::Deferred => {
                    filters.include_deferred || filters.include_closed || filters.include_tombstone
                }
                _ => true,
            })
            .cloned()
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    async fn get_issue(&self, id: &str) -> Result<Option<Issue>, StorageError> {
        Ok(self.rows.lock().expect("rows lock").get(id).cloned())
    }

    async fn create_issues(&self, issues: &[Issue], _actor: &str) -> Result<(), StorageError> {
        self.create_issues_calls.fetch_add(1, Ordering::SeqCst);
        let mut rows = self.rows.lock().expect("rows lock");
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
        self.create_issue_calls.fetch_add(1, Ordering::SeqCst);
        unimplemented!("sync never uses per-record create_issue for import apply")
    }

    async fn update_issue(
        &self,
        _id: &str,
        _patch: &IssuePatch,
        _actor: &str,
    ) -> Result<Issue, StorageError> {
        unimplemented!("FakeStorage::update_issue")
    }
    async fn migrate(&self) -> Result<(), StorageError> {
        unimplemented!("FakeStorage::migrate")
    }
    async fn integrity_check(&self) -> Result<Vec<String>, StorageError> {
        unimplemented!("FakeStorage::integrity_check")
    }
    async fn schema_version(&self) -> Result<i64, StorageError> {
        unimplemented!("FakeStorage::schema_version")
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
