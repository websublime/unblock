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
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let cancel = CancellationToken::new();
    let server_task = tokio::spawn(serve_duplex_for_test(
        session,
        Quotas::default(),
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
