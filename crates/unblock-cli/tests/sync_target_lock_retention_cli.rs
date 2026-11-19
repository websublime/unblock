//! PRD §4 D5 amendment (`ub-w1j`, v1.0.3), on the shipped surface: the real `unblock mcp` server
//! refuses a `sync` target that is its own database (or a sidecar) and keeps its `SQLite` locks and
//! the database.
//!
//! The server is the holder: `LibsqlStorage` keeps its connections open for the server's lifetime, so
//! the idle server holds SHARED. It is driven through `common::McpClient` (env scrub, `spawn_locked`);
//! the lock instrument is the engine's, included by `#[path]`. Every cell asserts the baseline
//! (SHARED held + a fresh observer BUSY) before the sync call.
#![cfg(unix)]

mod common;

#[path = "../../unblock-engine/tests/common/lock_observer.rs"]
#[allow(dead_code)]
// the cli cell uses a subset of the engine's lock instrument (engine tests/common/mod.rs:5 precedent)
mod lock_observer;
#[path = "../../unblock-engine/tests/common/posix_locks.rs"]
#[allow(dead_code)]
// the cli cell uses a subset of the engine's lock instrument (engine tests/common/mod.rs:5 precedent)
mod posix_locks;

use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

use common::{McpClient, Workspace};
use lock_observer::Observed;
use posix_locks::LockKind;
use serde_json::{Value, json};
use unblock_config::{CliOverrides, open_workspace_with_cli};

/// This binary's observer entry.
const ENTRY: &str = "kpe_observer_entry";

const MUTANT: &str =
    "the MCP sync action closed a descriptor on the live database or replaced it (D5 amendment)";

/// The observer process body (see the engine's `common::lock_observer`).
#[test]
#[ignore = "observer process entry point; spawned by the w1j cells"]
fn kpe_observer_entry() {
    lock_observer::observer_main();
}

fn server_pid(client: &McpClient) -> i32 {
    i32::try_from(client.child.id()).expect("a pid fits i32")
}

fn assert_shared_and_busy(cell: &str, point: &str, db: &Path, pid: i32) {
    let observation = posix_locks::shared_range(db, pid);
    let (stdout, stderr) = common::observe(ENTRY, db);
    let observed = Observed::parse(&stdout, &stderr);
    eprintln!(
        "{}",
        posix_locks::measure_line(cell, point, pid, &observation, Some(&observed.line), None)
    );
    assert_eq!(
        observation.kind(),
        Some(LockKind::Read),
        "{cell} at {point}: the server lost SHARED — {MUTANT}"
    );
    assert!(
        observed.is_busy(),
        "{cell} at {point}: the observer must be BUSY, got {:?} (stderr {}) — {MUTANT}",
        observed.line,
        observed.stderr
    );
}

/// Spawn the server on `ws`, initialize, serve one read, and assert the baseline.
fn ready_server(cell: &str, ws: &Workspace, db: &Path) -> McpClient {
    let unblock = ws.unblock_dir();
    let unblock = unblock.to_str().expect("utf-8 path");
    let mut client = McpClient::spawn_with_args(ws.root(), &["--dir", unblock]);
    client.initialize();
    let (is_error, _) = client.call_tool("query", &json!({"kind": "list"}));
    assert!(!is_error, "{cell}: the read call succeeds");
    assert_shared_and_busy(cell, "baseline", db, server_pid(&client));
    client
}

fn assert_path_traversal(is_error: bool, structured: &Value, what: &str) {
    assert!(is_error, "{what} must be refused, got {structured}");
    assert!(
        structured.to_string().contains("PATH_TRAVERSAL"),
        "{what} must be refused as PATH_TRAVERSAL, got {structured}"
    );
    assert!(
        structured.to_string().contains("SessionDatabase"),
        "{what} must be refused for the SessionDatabase reason, got {structured}"
    );
}

/// (w1j3) A `sync import` / `import_bd` of a hard link to the live database is refused, and the server
/// keeps its SHARED lock.
#[test]
fn w1j3_mcp_refused_import_keeps_the_shared_lock() {
    let ws = Workspace::init();
    let db = ws.db_path();
    let mut client = ready_server("w1j3", &ws, &db);
    let pid = server_pid(&client);
    let alias = ws.unblock_dir().join("alias.jsonl");
    std::fs::hard_link(&db, &alias).expect("hard link");
    let alias = alias.to_str().expect("utf-8 path").to_owned();
    for action in ["import", "import_bd"] {
        let (is_error, structured) =
            client.call_tool("sync", &json!({"action": action, "path": alias}));
        assert_path_traversal(is_error, &structured, action);
        assert_shared_and_busy("w1j3", &format!("after-{action}"), &db, pid);
    }
    client.close_stdin();
}

/// (w1j4) With `db_filename = "issues.jsonl"`, a path-less `sync export` (the tool's default target)
/// is refused and never replaces the live database.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn w1j4_mcp_default_export_never_replaces_the_database() {
    let ws = Workspace::init();
    let config = std::fs::read_to_string(ws.config_path()).expect("read config.toml");
    std::fs::write(
        ws.config_path(),
        format!("db_filename = \"issues.jsonl\"\n{config}"),
    )
    .expect("write config.toml");
    let resolved = open_workspace_with_cli(&CliOverrides::new().with_dir(ws.unblock_dir()))
        .await
        .expect("resolve");
    let db = resolved.paths.db_path.clone();
    assert!(
        db.ends_with("issues.jsonl"),
        "precondition: the database is named issues.jsonl ({})",
        db.display()
    );
    let mut client = ready_server("w1j4", &ws, &db);
    let pid = server_pid(&client);
    let meta = std::fs::metadata(&db).expect("stat db");
    let before = (
        (meta.dev(), meta.ino()),
        std::fs::read(&db).expect("read db"),
    );
    let (is_error, structured) = client.call_tool("sync", &json!({"action": "export"}));
    assert_path_traversal(is_error, &structured, "the default export");
    let meta = std::fs::metadata(&db).expect("stat db");
    assert_eq!(
        (
            (meta.dev(), meta.ino()),
            std::fs::read(&db).expect("read db")
        ),
        before,
        "the database was replaced — {MUTANT}"
    );
    assert_shared_and_busy("w1j4", "after-export", &db, pid);
    client.close_stdin();
}
