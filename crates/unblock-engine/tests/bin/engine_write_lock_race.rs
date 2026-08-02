//! AC-1 / M1 (D31/T3.4.1) helper `[[bin]]` — one OS process driving the REAL engine `Session`.
//!
//! Unlike the storage-level `write_lock_race` bin (which calls `Storage::acquire_write_lock` DIRECTLY),
//! this drives the production `Session::create_issue` MINTING path: each create runs
//! `Session::acquire()` (the in-process permit **and** the D31 `.write.lock`) → `allocate_id`
//! (`next_child_number(parent)` READ) → build/validate → `storage.create_issue` INSERT. So this test
//! exercises the lock **as the engine wires it** — deleting `storage.acquire_write_lock()` from
//! `Session::acquire()` (write.rs) makes two processes race the shared `parent.N` namespace and
//! reproduces the cross-process `IdCollision` (the M1 non-vacuity).
//!
//! To make the inherent (narrow) read→insert race DETERMINISTIC, the store is wrapped in a
//! [`WidenedStorage`] decorator that sleeps briefly inside `next_child_number` (widening the window
//! between the allocation READ and the INSERT) while forwarding **every** call — crucially
//! `acquire_write_lock` — to the inner real `LibsqlStorage`. Under the engine lock the whole
//! mutation (incl. the sleep) is serialized across processes → no collision; with the lock removed the
//! widened window makes both processes read the same `N` → the second insert collides.
//!
//! Usage: `engine_write_lock_race <db-path> <parent-id> <count> <actor>`. Emits one `MINTED=<id>` line
//! per committed child and a final `COLLISIONS=<n>` line; exits 3 on any unexpected engine error.

#![forbid(unsafe_code)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use unblock_config::{ConfigPaths, ResolvedConfig, WorkspaceContext, WorkspaceSource};
use unblock_engine::{EngineError, NewIssue, Session, SessionConfig};
use unblock_model::{
    Comment, CountBucket, CountGroupBy, DepTree, Dependency, DependencyType, Event, GraphEdge,
    Issue, ListFilters,
};
use unblock_storage::{
    DEFAULT_WRITE_LOCK_TIMEOUT_MS, DeletePlan, IssuePatch, LibsqlStorage, Storage, StorageError,
    WriteLockGuard,
};

/// Widen the allocation-READ → INSERT window so the (inherent) cross-process race is deterministic:
/// under the engine lock the two processes serialize (no collision); with the lock removed both read
/// the same `parent.N` inside this window and the second insert collides.
const RACE_WINDOW: Duration = Duration::from_millis(3);

/// A `Storage` decorator that widens the `next_child_number` window and delegates everything else to
/// an inner real `LibsqlStorage` — **including `acquire_write_lock`**, so the engine's D31 lock is the
/// genuine cross-process flock (not a mock). The ONLY added effect is a short sleep after the child
/// counter READ, which makes the engine's read→insert race deterministic.
struct WidenedStorage {
    inner: Arc<dyn Storage>,
}

#[async_trait]
impl Storage for WidenedStorage {
    async fn next_child_number(&self, parent_id: &str) -> Result<u32, StorageError> {
        let n = self.inner.next_child_number(parent_id).await?;
        // Widen the window between this allocation READ and the engine's subsequent INSERT.
        tokio::time::sleep(RACE_WINDOW).await;
        Ok(n)
    }

    async fn acquire_write_lock(&self) -> Result<Option<WriteLockGuard>, StorageError> {
        // Forward to the real store: the engine's D31 lock must be the genuine cross-process flock.
        self.inner.acquire_write_lock().await
    }

    async fn migrate(&self) -> Result<(), StorageError> {
        self.inner.migrate().await
    }
    async fn integrity_check(&self) -> Result<Vec<String>, StorageError> {
        self.inner.integrity_check().await
    }
    async fn schema_version(&self) -> Result<i64, StorageError> {
        self.inner.schema_version().await
    }
    async fn create_issue(&self, issue: &Issue, actor: &str) -> Result<String, StorageError> {
        self.inner.create_issue(issue, actor).await
    }
    async fn create_issues(&self, issues: &[Issue], actor: &str) -> Result<(), StorageError> {
        self.inner.create_issues(issues, actor).await
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
    // --- comments (FR-6, D37) — DELEGATE: this double decorates a real `Storage`, exactly as it
    // already does for `list_dependencies`/`next_child_number`. A stub here would silently
    // decouple the decorated behaviour from the real one.
    async fn add_comment(
        &self,
        issue_id: &str,
        author: &str,
        body: &str,
        actor: &str,
    ) -> Result<Comment, StorageError> {
        self.inner.add_comment(issue_id, author, body, actor).await
    }
    async fn list_comments(&self, issue_id: &str) -> Result<Vec<Comment>, StorageError> {
        self.inner.list_comments(issue_id).await
    }
    async fn update_comment(
        &self,
        comment_id: i64,
        body: &str,
        actor: &str,
    ) -> Result<Comment, StorageError> {
        self.inner.update_comment(comment_id, body, actor).await
    }
    async fn delete_comment(&self, comment_id: i64, actor: &str) -> Result<Comment, StorageError> {
        self.inner.delete_comment(comment_id, actor).await
    }
    async fn dependency_tree(&self, id: &str) -> Result<DepTree, StorageError> {
        self.inner.dependency_tree(id).await
    }
    async fn dependency_graph(&self, roots: &[String]) -> Result<DepTree, StorageError> {
        self.inner.dependency_graph(roots).await
    }
    async fn detect_cycles(&self, blocking_only: bool) -> Result<Vec<Vec<String>>, StorageError> {
        self.inner.detect_cycles(blocking_only).await
    }
    async fn list_events(&self, issue_id: &str) -> Result<Vec<Event>, StorageError> {
        self.inner.list_events(issue_id).await
    }
    async fn epic_child_rollup(&self) -> Result<Vec<(String, (usize, usize))>, StorageError> {
        self.inner.epic_child_rollup().await
    }
    async fn closed_since(&self, since: Option<DateTime<Utc>>) -> Result<Vec<Issue>, StorageError> {
        self.inner.closed_since(since).await
    }
    async fn orphan_candidates(&self) -> Result<Vec<Issue>, StorageError> {
        self.inner.orphan_candidates().await
    }

    // D45 (amended 2026-08-02): a non-gated read — delegate, like every other one.
    async fn dangling_dependencies(&self) -> Result<Vec<GraphEdge>, StorageError> {
        self.inner.dangling_dependencies().await
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    assert!(
        args.len() == 5,
        "usage: engine_write_lock_race <db> <parent> <count> <actor>"
    );
    let db_path = Path::new(&args[1]);
    let parent = &args[2];
    let count: u32 = args[3].parse().expect("count is a u32");
    let actor = args[4].clone();

    // File-backed store (the shared cross-process DB) → the D31 `.write.lock` is live. The harness
    // pre-migrated + seeded the parent, so this process only opens + creates (it never migrates —
    // migrate now fail-fasts under a held lock, MF2). Wrap it so the read→insert window is widened.
    let libsql = LibsqlStorage::open_local(db_path, DEFAULT_WRITE_LOCK_TIMEOUT_MS)
        .await
        .expect("open_local the shared db");
    let inner: Arc<dyn Storage> = Arc::new(libsql);
    let storage: Arc<dyn Storage> = Arc::new(WidenedStorage { inner });

    // Build the same storage-bearing WorkspaceContext `unblock-config` builds in production, then open
    // the REAL engine `Session` over it.
    let unblock_dir = db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let workspace_dir = unblock_dir
        .parent()
        .map_or_else(|| unblock_dir.clone(), Path::to_path_buf);
    let config = ResolvedConfig::default();
    let paths = ConfigPaths {
        db_path: db_path.to_path_buf(),
        jsonl_path: unblock_dir.join(&config.jsonl_filename),
        unblock_dir,
    };
    let ctx = WorkspaceContext {
        storage,
        workspace_dir,
        actor,
        config,
        paths,
        source: WorkspaceSource::WalkUp,
    };
    let session = Session::open(ctx, SessionConfig::default())
        .await
        .expect("open session");

    let mut collisions = 0u32;
    for i in 0..count {
        let new = NewIssue {
            title: format!("child {i}"),
            parent: Some(parent.clone()),
            ..NewIssue::default()
        };
        match session.create_issue(new).await {
            Ok(issue) => println!("MINTED={}", issue.id),
            // The cross-process collision the engine-wired `.write.lock` exists to prevent.
            Err(EngineError::Storage {
                source: StorageError::IdCollision { .. },
            }) => collisions += 1,
            Err(err) => {
                eprintln!("UNEXPECTED={err:?}");
                std::process::exit(3);
            }
        }
    }

    println!("COLLISIONS={collisions}");
}
