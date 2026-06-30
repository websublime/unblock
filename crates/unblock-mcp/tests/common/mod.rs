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
use unblock_mcp::{Quotas, UnblockServer, serve_duplex_for_test};
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
/// The MCP initialize handshake is symmetric: `serve_duplex_for_test` awaits the client's initialize
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

/// Like [`connect`], but advertises the given `instructions` (drives the `ServeOptions::instructions`
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
    let server_task = tokio::spawn(serve_duplex_for_test(
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
        CountBucket, CountGroupBy, DepTree, Dependency, DependencyType, Event, Issue,
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
