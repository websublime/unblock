//! PRD §4 D5 amendment (`ub-c15`, v1.1), on the shipped surface: the real `unblock mcp` server refuses
//! a `sync` target that is its own database (or a sidecar), `dry_run` included, and keeps its `SQLite`
//! locks and the database.
//!
//! The server is the holder: `LibsqlStorage` keeps its connections open for the server's lifetime, so
//! the idle server holds SHARED and the `-shm` DMS lock. It is driven through `common::McpClient`
//! (env scrub, `spawn_locked`); its pid is `client.child.id()`. The lock instrument is storage's
//! `posix_locks.rs` and the observer the engine's `lock_observer.rs`, both included by `#[path]`.
//! Every cell asserts the baseline (SHARED held + a fresh observer BUSY) before the sync call.
#![cfg(unix)]

mod common;

#[path = "../../unblock-engine/tests/common/lock_observer.rs"]
#[allow(dead_code)] // the cli cells use a subset of the engine's observer
mod lock_observer;
#[path = "../../unblock-storage/tests/common/posix_locks.rs"]
#[allow(dead_code)] // the cli cells use a subset of the one lock instrument
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
#[ignore = "observer process entry point; spawned by the lock cells"]
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

fn assert_shm_dms_held(cell: &str, point: &str, shm: &Path, pid: i32) {
    let kind = posix_locks::shm_dms(shm, pid);
    eprintln!(
        "UB2VP-MEASURE leg={} cell={cell} point={point} holder_pid={pid} shm_dms={kind:?}",
        posix_locks::leg()
    );
    assert_eq!(
        kind,
        Some(LockKind::Read),
        "{cell} at {point}: the server lost its -shm DMS lock — {MUTANT}"
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

/// The three import calls of the `sync` tool on `path`.
fn imports(path: &str) -> [(&'static str, Value); 3] {
    [
        ("import", json!({"action": "import", "path": path})),
        (
            "import-dry-run",
            json!({"action": "import", "path": path, "dry_run": true}),
        ),
        ("import_bd", json!({"action": "import_bd", "path": path})),
    ]
}

/// (M0) A `sync import` (with and without `dry_run`) and `import_bd` of a hard link to the live
/// database are refused, and the server keeps its SHARED lock.
#[test]
fn w1j3_mcp_refused_import_keeps_the_shared_lock() {
    let ws = Workspace::init();
    let db = ws.db_path();
    let mut client = ready_server("w1j3", &ws, &db);
    let pid = server_pid(&client);
    let alias = ws.unblock_dir().join("alias.jsonl");
    std::fs::hard_link(&db, &alias).expect("hard link");
    let alias = alias.to_str().expect("utf-8 path").to_owned();
    for (what, args) in imports(&alias) {
        let (is_error, structured) = client.call_tool("sync", &args);
        assert_path_traversal(is_error, &structured, what);
        assert_shared_and_busy("w1j3", &format!("after-{what}"), &db, pid);
    }
    client.close_stdin();
}

/// (M1) With `db_filename = "issues.jsonl"`, a path-less `sync export` (the tool's default target,
/// `.unblock/issues.jsonl`) is refused and never replaces the live database.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn w1j4_mcp_default_export_never_replaces_the_database() {
    let ws = Workspace::init();
    // The migrated database itself takes the export's default name (a missing configured
    // database is refused on main, D58), then the config points at it.
    std::fs::rename(ws.db_path(), ws.unblock_dir().join("issues.jsonl")).expect("rename the db");
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

/// (M2) A SYMLINKED database (`.unblock/unblock.db` → `.unblock/store/unblock.db`): the server keeps
/// the live `-shm` beside the TARGET. A JSONL-named hard link to that `-shm` is refused by `import`,
/// `import {dry_run}`, `import_bd` and `export {path}`, and a hard link to the target database by
/// `import`; the target's `-shm` DMS lock and SHARED are kept.
#[test]
fn c15m0_mcp_symlinked_database_refuses_target_aliases_and_keeps_the_locks() {
    let ws = Workspace::init();
    let link = ws.db_path();
    let store = ws.unblock_dir().join("store");
    std::fs::create_dir(&store).expect("mkdir store");
    let real = store.join("unblock.db");
    std::fs::rename(&link, &real).expect("move the db");
    std::os::unix::fs::symlink(&real, &link).expect("symlink the db");
    let mut client = ready_server("c15m0", &ws, &real);
    let pid = server_pid(&client);
    let shm = store.join("unblock.db-shm");
    assert!(
        shm.exists(),
        "precondition: the server keeps the -shm beside the symlink's target"
    );
    assert_shm_dms_held("c15m0", "baseline", &shm, pid);

    let alias = ws.unblock_dir().join("alias.jsonl");
    std::fs::hard_link(&shm, &alias).expect("hard link to the target's -shm");
    let alias_str = alias.to_str().expect("utf-8 path").to_owned();
    let mut calls = imports(&alias_str).to_vec();
    calls.push((
        "export",
        json!({"action": "export", "path": alias_str.clone()}),
    ));
    for (what, args) in calls {
        let (is_error, structured) = client.call_tool("sync", &args);
        assert_path_traversal(
            is_error,
            &structured,
            &format!("{what} of the target's -shm"),
        );
        assert_shm_dms_held("c15m0", &format!("after-{what}"), &shm, pid);
        assert_shared_and_busy("c15m0", &format!("after-{what}"), &real, pid);
    }

    std::fs::remove_file(&alias).expect("remove the -shm alias");
    std::fs::hard_link(&real, &alias).expect("hard link to the target database");
    let (is_error, structured) = client.call_tool(
        "sync",
        &json!({"action": "import", "path": alias_str.clone()}),
    );
    assert_path_traversal(is_error, &structured, "import of the target database");
    assert_shm_dms_held("c15m0", "after-db-import", &shm, pid);
    assert_shared_and_busy("c15m0", "after-db-import", &real, pid);
    client.close_stdin();
}
