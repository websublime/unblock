//! Shared integration-test harness: a real-storage [`Session`] over an in-memory libsql workspace
//! (NOT a `Storage` mock — the MCP adapter's contract is "identical behaviour through one path",
//! FR-9). Mirrors the engine test harness (`crates/unblock-engine/tests/common/mod.rs`).

#![allow(dead_code)] // each test binary uses a subset of the harness.

use std::path::PathBuf;
use std::sync::Arc;

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::service::{RoleClient, RoleServer, RunningService};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;
use unblock_config::{ConfigPaths, ResolvedConfig, WorkspaceContext};
use unblock_engine::{Session, SessionConfig};
use unblock_mcp::{Quotas, UnblockServer, mcp_server_duplex_for_test};
use unblock_storage::{LibsqlStorage, Storage};

/// Build an `Arc<Session>` over a fresh in-memory libsql backend (migrated), wired into a synthetic
/// `WorkspaceContext` — the same shape `unblock-config` builds in production, but in-memory.
pub async fn session() -> Arc<Session> {
    let storage = LibsqlStorage::open_in_memory()
        .await
        .expect("open in-memory");
    storage.migrate().await.expect("migrate");
    let storage: Arc<dyn Storage> = Arc::new(storage);

    let workspace_dir = PathBuf::from("/tmp/unblock-mcp-test-ws");
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
    Arc::new(
        Session::open(ctx, SessionConfig::default())
            .await
            .expect("open session"),
    )
}

/// Build an `Arc<Session>` over a [`recording::RecordingStorage`] spy (NFR-18 AC): the spy wraps a real
/// in-memory backend and COUNTS every mutating `Storage` call, so a test can assert an over-quota
/// input was rejected at the preflight with ZERO `Session`/`Storage` calls. Returns both the session
/// and the spy handle.
pub async fn session_recording() -> (Arc<Session>, Arc<recording::RecordingStorage>) {
    let inner = LibsqlStorage::open_in_memory()
        .await
        .expect("open in-memory");
    inner.migrate().await.expect("migrate");
    let inner: Arc<dyn Storage> = Arc::new(inner);
    let spy = recording::RecordingStorage::new(inner);
    let spy_dyn: Arc<dyn Storage> = spy.clone();

    let workspace_dir = PathBuf::from("/tmp/unblock-mcp-test-ws");
    let unblock_dir = workspace_dir.join(".unblock");
    let config = ResolvedConfig::default();
    let paths = ConfigPaths {
        db_path: unblock_dir.join(&config.db_filename),
        jsonl_path: unblock_dir.join(&config.jsonl_filename),
        unblock_dir,
    };
    let ctx = WorkspaceContext {
        storage: spy_dyn,
        workspace_dir,
        actor: "tester".to_string(),
        config,
        paths,
    };
    let session = Arc::new(
        Session::open(ctx, SessionConfig::default())
            .await
            .expect("open session"),
    );
    (session, spy)
}

/// Spin up the real server over an in-memory duplex transport and return an initialized client peer
/// plus the server handle + cancellation token (so a test can drive a cancel).
///
/// The MCP initialize handshake is symmetric: `mcp_server_duplex_for_test` awaits the client's initialize
/// before returning, and the client awaits the server's response — so the two MUST run concurrently.
/// The server-serve future is spawned, the client initializes on this task, then the server is joined.
pub async fn connect(
    session: Arc<Session>,
) -> (
    RunningService<RoleClient, ()>,
    RunningService<RoleServer, UnblockServer>,
    CancellationToken,
) {
    connect_with_instructions(session, None).await
}

/// Like [`connect`], but advertises the given `instructions` (drives the `McpServerOptions::instructions`
/// → `get_info().instructions` wiring through the same real server path the CLI uses).
pub async fn connect_with_instructions(
    session: Arc<Session>,
    instructions: Option<String>,
) -> (
    RunningService<RoleClient, ()>,
    RunningService<RoleServer, UnblockServer>,
    CancellationToken,
) {
    connect_with_quotas(session, Quotas::default(), instructions).await
}

/// Like [`connect`], but with caller-supplied [`Quotas`] (the NFR-18 untrusted-input AC suite injects
/// tightened limits so an over-quota input is rejected at the preflight — before any `Session` call).
pub async fn connect_with_quotas(
    session: Arc<Session>,
    quotas: Quotas,
    instructions: Option<String>,
) -> (
    RunningService<RoleClient, ()>,
    RunningService<RoleServer, UnblockServer>,
    CancellationToken,
) {
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let cancel = CancellationToken::new();
    let server_task = tokio::spawn(mcp_server_duplex_for_test(
        session,
        quotas,
        instructions,
        server_io,
        cancel.clone(),
    ));
    let client = ().serve(client_io).await.expect("client initializes");
    let server = server_task
        .await
        .expect("server task joins")
        .expect("server starts over duplex");
    (client, server, cancel)
}

/// Call a tool by name with JSON arguments; return `(is_error, structured_content)`.
pub async fn call_tool(
    client: &RunningService<RoleClient, ()>,
    name: &'static str,
    args: Value,
) -> (bool, Value) {
    let arguments: Map<String, Value> = match args {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    let result = client
        .call_tool(CallToolRequestParams::new(name).with_arguments(arguments))
        .await
        .expect("tool call round-trips");
    let is_error = result.is_error.unwrap_or(false);
    let structured = result.structured_content.unwrap_or(Value::Null);
    (is_error, structured)
}

/// A `Storage` decorator that COUNTS every mutating call (NFR-18 spy). It wraps a real inner backend
/// and delegates everything; the only added effect is a per-mutation counter so an over-quota AC test
/// can assert the preflight rejected an input BEFORE it reached any `Storage` mutation (zero calls).
pub mod recording {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use chrono::{DateTime, Utc};
    use unblock_model::{
        Comment, CountBucket, CountGroupBy, DepTree, Dependency, DependencyType, Event, Issue,
    };
    use unblock_storage::{DeletePlan, IssuePatch, ListFilters, Storage, StorageError};

    /// Wraps an inner `Storage`, counting every mutating call.
    pub struct RecordingStorage {
        inner: Arc<dyn Storage>,
        mutations: AtomicUsize,
    }

    impl RecordingStorage {
        /// Wrap `inner`.
        #[must_use]
        pub fn new(inner: Arc<dyn Storage>) -> Arc<Self> {
            Arc::new(Self {
                inner,
                mutations: AtomicUsize::new(0),
            })
        }

        /// The total number of mutating `Storage` calls observed.
        #[must_use]
        pub fn mutation_count(&self) -> usize {
            self.mutations.load(Ordering::SeqCst)
        }

        fn record(&self) {
            self.mutations.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl Storage for RecordingStorage {
        async fn create_issue(&self, issue: &Issue, actor: &str) -> Result<String, StorageError> {
            self.record();
            self.inner.create_issue(issue, actor).await
        }
        async fn create_issues(&self, issues: &[Issue], actor: &str) -> Result<(), StorageError> {
            self.record();
            self.inner.create_issues(issues, actor).await
        }
        async fn update_issue(
            &self,
            id: &str,
            patch: &IssuePatch,
            actor: &str,
        ) -> Result<Issue, StorageError> {
            self.record();
            self.inner.update_issue(id, patch, actor).await
        }
        async fn delete_issue(
            &self,
            plan: &DeletePlan,
            actor: &str,
        ) -> Result<DeletePlan, StorageError> {
            self.record();
            self.inner.delete_issue(plan, actor).await
        }
        async fn restore_issue(&self, id: &str, actor: &str) -> Result<Issue, StorageError> {
            self.record();
            self.inner.restore_issue(id, actor).await
        }
        async fn claim_issue(
            &self,
            id: &str,
            assignee: &str,
            actor: &str,
        ) -> Result<Issue, StorageError> {
            self.record();
            self.inner.claim_issue(id, assignee, actor).await
        }
        async fn defer_issue(
            &self,
            id: &str,
            until: DateTime<Utc>,
            actor: &str,
        ) -> Result<Issue, StorageError> {
            self.record();
            self.inner.defer_issue(id, until, actor).await
        }
        async fn undefer_issue(&self, id: &str, actor: &str) -> Result<Issue, StorageError> {
            self.record();
            self.inner.undefer_issue(id, actor).await
        }
        async fn add_dependency(&self, dep: &Dependency, actor: &str) -> Result<(), StorageError> {
            self.record();
            self.inner.add_dependency(dep, actor).await
        }
        async fn remove_dependency(
            &self,
            issue_id: &str,
            depends_on_id: &str,
            dep_type: &DependencyType,
            actor: &str,
        ) -> Result<(), StorageError> {
            self.record();
            self.inner
                .remove_dependency(issue_id, depends_on_id, dep_type, actor)
                .await
        }

        // --- non-mutating delegates (not counted) ---
        async fn migrate(&self) -> Result<(), StorageError> {
            self.inner.migrate().await
        }
        async fn integrity_check(&self) -> Result<Vec<String>, StorageError> {
            self.inner.integrity_check().await
        }
        async fn schema_version(&self) -> Result<i64, StorageError> {
            self.inner.schema_version().await
        }
        async fn acquire_write_lock(
            &self,
        ) -> Result<Option<unblock_storage::WriteLockGuard>, StorageError> {
            self.inner.acquire_write_lock().await
        }
        async fn get_issue(&self, id: &str) -> Result<Option<Issue>, StorageError> {
            self.inner.get_issue(id).await
        }
        async fn get_issues(&self, ids: &[String]) -> Result<Vec<Issue>, StorageError> {
            self.inner.get_issues(ids).await
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
        async fn delete_comment(
            &self,
            comment_id: i64,
            actor: &str,
        ) -> Result<Comment, StorageError> {
            self.inner.delete_comment(comment_id, actor).await
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
        async fn epic_child_rollup(&self) -> Result<Vec<(String, (usize, usize))>, StorageError> {
            self.inner.epic_child_rollup().await
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

/// Build an `Arc<Session>` over a [`gated::GateStorage`] double whose `list_issues` BLOCKS every call
/// in-flight on a shared [`gated::Gate`] until the test releases (the NFR-18 rate-limit AC seam): a
/// test holds `n` tool calls occupying `n` rate-limit permits, then proves the `(n + 1)`th is rejected.
/// The release barrier has `n + 1` parties (the `n` in-flight reads + the test). Returns the session
/// and the gate handle.
pub async fn session_gated(n: usize) -> (Arc<Session>, gated::Gate) {
    let inner = LibsqlStorage::open_in_memory()
        .await
        .expect("open in-memory");
    inner.migrate().await.expect("migrate");
    let inner: Arc<dyn Storage> = Arc::new(inner);
    let gate = gated::Gate::new(n);
    let storage: Arc<dyn Storage> = Arc::new(gated::GateStorage::new(inner, gate.clone()));

    let workspace_dir = PathBuf::from("/tmp/unblock-mcp-test-ws");
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
    let session = Arc::new(
        Session::open(ctx, SessionConfig::default())
            .await
            .expect("open session"),
    );
    (session, gate)
}

/// A `Storage` decorator that BLOCKS every `list_issues` call in-flight on a shared [`gated::Gate`]
/// (a REAL barrier/semaphore handoff — never a `sleep`) so the NFR-18 rate-limit AC can hold exactly
/// N reads occupying N permits simultaneously, then prove the (N+1)th is rejected. Every other call
/// delegates to a real inner backend.
pub mod gated {
    use std::sync::Arc;

    use async_trait::async_trait;
    use chrono::{DateTime, Utc};
    use tokio::sync::{Barrier, Semaphore};
    use unblock_model::{
        Comment, CountBucket, CountGroupBy, DepTree, Dependency, DependencyType, Event, Issue,
    };
    use unblock_storage::{DeletePlan, IssuePatch, ListFilters, Storage, StorageError};

    /// The shared concurrency gate for [`GateStorage`]. A `Semaphore` signals "a gated read entered
    /// (its permit is held)"; a `Barrier` releases the in-flight reads only after the test has arrived.
    #[derive(Clone)]
    pub struct Gate {
        /// Counts gated reads that ENTERED (permit held) — the test `acquire_many(n)`s to learn all `n`
        /// calls are simultaneously in-flight before it fires the `(n + 1)`th.
        entered: Arc<Semaphore>,
        /// Releases the in-flight reads: `n` reads + the test = `n + 1` parties, so a read proceeds only
        /// AFTER the test has observed the over-cap rejection. A `Barrier` handoff is lost-wakeup-free.
        release: Arc<Barrier>,
    }

    impl Gate {
        /// Build a gate expecting `n` in-flight gated reads (release barrier = `n + 1` parties).
        #[must_use]
        pub fn new(n: usize) -> Self {
            Self {
                entered: Arc::new(Semaphore::new(0)),
                release: Arc::new(Barrier::new(n + 1)),
            }
        }

        /// Block until all `n` gated reads have entered (all `n` rate-limit permits are held).
        pub async fn await_all_entered(&self, n: usize) {
            let permits = u32::try_from(n).expect("n fits u32");
            self.entered
                .acquire_many(permits)
                .await
                .expect("entered semaphore is not closed")
                .forget();
        }

        /// Release the in-flight reads — the caller is the `(n + 1)`th barrier party.
        pub async fn release(&self) {
            self.release.wait().await;
        }
    }

    /// Wraps an inner `Storage`, blocking every `list_issues` on the shared [`Gate`].
    pub struct GateStorage {
        inner: Arc<dyn Storage>,
        gate: Gate,
    }

    impl GateStorage {
        /// Wrap `inner`, gating `list_issues` on `gate`.
        #[must_use]
        pub fn new(inner: Arc<dyn Storage>, gate: Gate) -> Self {
            Self { inner, gate }
        }
    }

    #[async_trait]
    impl Storage for GateStorage {
        async fn list_issues(&self, filters: &ListFilters) -> Result<Vec<Issue>, StorageError> {
            // Signal "in-flight" (this call now holds a rate-limit permit), then block until the test
            // releases us (the barrier's `n + 1`th party) — so all `n` permits stay held while the
            // test fires the over-cap `(n + 1)`th call.
            self.gate.entered.add_permits(1);
            self.gate.release.wait().await;
            self.inner.list_issues(filters).await
        }

        // --- everything else delegates to the real inner backend ---
        async fn migrate(&self) -> Result<(), StorageError> {
            self.inner.migrate().await
        }
        async fn integrity_check(&self) -> Result<Vec<String>, StorageError> {
            self.inner.integrity_check().await
        }
        async fn schema_version(&self) -> Result<i64, StorageError> {
            self.inner.schema_version().await
        }
        async fn acquire_write_lock(
            &self,
        ) -> Result<Option<unblock_storage::WriteLockGuard>, StorageError> {
            self.inner.acquire_write_lock().await
        }
        async fn create_issue(&self, issue: &Issue, actor: &str) -> Result<String, StorageError> {
            self.inner.create_issue(issue, actor).await
        }
        async fn create_issues(&self, issues: &[Issue], actor: &str) -> Result<(), StorageError> {
            self.inner.create_issues(issues, actor).await
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
        async fn get_issue(&self, id: &str) -> Result<Option<Issue>, StorageError> {
            self.inner.get_issue(id).await
        }
        async fn get_issues(&self, ids: &[String]) -> Result<Vec<Issue>, StorageError> {
            self.inner.get_issues(ids).await
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
        async fn delete_comment(
            &self,
            comment_id: i64,
            actor: &str,
        ) -> Result<Comment, StorageError> {
            self.inner.delete_comment(comment_id, actor).await
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
        async fn epic_child_rollup(&self) -> Result<Vec<(String, (usize, usize))>, StorageError> {
            self.inner.epic_child_rollup().await
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

/// Build an `Arc<Session>` over a storage that FAILS `list_issues` (the not-found suggestion-scan
/// path, T2.6/D25/FORK-3A): the resource must SURFACE the scan error, not the not-found (faithful to
/// the original `issue_not_found_resource_surfaces_id_scan_failure`).
pub async fn session_failing_list() -> Arc<Session> {
    let inner = LibsqlStorage::open_in_memory()
        .await
        .expect("open in-memory");
    inner.migrate().await.expect("migrate");
    let inner: Arc<dyn Storage> = Arc::new(inner);
    let storage: Arc<dyn Storage> = Arc::new(failing::FailListStorage { inner });

    let workspace_dir = PathBuf::from("/tmp/unblock-mcp-test-ws");
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
    Arc::new(
        Session::open(ctx, SessionConfig::default())
            .await
            .expect("open session"),
    )
}

/// A `Storage` decorator that FAILS `list_issues` (returns `IntegrityFailed`) and delegates everything
/// else — the fault-injection seam for the FORK-3A scan-failure-surfacing test.
pub mod failing {
    use std::sync::Arc;

    use async_trait::async_trait;
    use chrono::{DateTime, Utc};
    use unblock_model::{
        Comment, CountBucket, CountGroupBy, DepTree, Dependency, DependencyType, Event, Issue,
    };
    use unblock_storage::{DeletePlan, IssuePatch, ListFilters, Storage, StorageError};

    /// Wraps an inner `Storage`, failing only `list_issues`.
    pub struct FailListStorage {
        pub inner: Arc<dyn Storage>,
    }

    #[async_trait]
    impl Storage for FailListStorage {
        async fn list_issues(&self, _filters: &ListFilters) -> Result<Vec<Issue>, StorageError> {
            Err(StorageError::IntegrityFailed {
                messages: vec!["injected scan failure".to_string()],
            })
        }

        // --- everything else delegates to the real inner backend ---
        async fn migrate(&self) -> Result<(), StorageError> {
            self.inner.migrate().await
        }
        async fn integrity_check(&self) -> Result<Vec<String>, StorageError> {
            self.inner.integrity_check().await
        }
        async fn schema_version(&self) -> Result<i64, StorageError> {
            self.inner.schema_version().await
        }
        async fn acquire_write_lock(
            &self,
        ) -> Result<Option<unblock_storage::WriteLockGuard>, StorageError> {
            self.inner.acquire_write_lock().await
        }
        async fn create_issue(&self, issue: &Issue, actor: &str) -> Result<String, StorageError> {
            self.inner.create_issue(issue, actor).await
        }
        async fn create_issues(&self, issues: &[Issue], actor: &str) -> Result<(), StorageError> {
            self.inner.create_issues(issues, actor).await
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
        async fn get_issue(&self, id: &str) -> Result<Option<Issue>, StorageError> {
            self.inner.get_issue(id).await
        }
        async fn get_issues(&self, ids: &[String]) -> Result<Vec<Issue>, StorageError> {
            self.inner.get_issues(ids).await
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
        async fn delete_comment(
            &self,
            comment_id: i64,
            actor: &str,
        ) -> Result<Comment, StorageError> {
            self.inner.delete_comment(comment_id, actor).await
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
        async fn epic_child_rollup(&self) -> Result<Vec<(String, (usize, usize))>, StorageError> {
            self.inner.epic_child_rollup().await
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
