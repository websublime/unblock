//! PRD §4 D58 criteria (f) and (g) test-only helper `[[bin]]`. It is the SECOND process that holds
//! a database, or its `.write.lock`, while `tests/exclusive_window.rs` runs the exclusive window.
//!
//! Usage: `exclusive_window_holder <mode> <db-path>`.
//!
//! - `reader` opens a raw libsql connection, serves one read, prints `READY`, and keeps the
//!   connection open until its stdin yields a line or closes.
//! - `write-lock` takes `<db dir>/.write.lock` with a blocking exclusive lock, prints `READY`, and
//!   holds it until its stdin yields a line or closes.
//! - `hot-journal` switches the database to `journal_mode=DELETE` and opens a write transaction
//!   large enough to spill pages into the database file. It prints `READY` and parks, so the parent
//!   can SIGKILL it and keep the hot `-journal` it leaves.
//! - `begin-exclusive` runs `BEGIN EXCLUSIVE` and `COMMIT` with `busy_timeout = 0` and WITHOUT
//!   `locking_mode=EXCLUSIVE`, then prints `COMMITTED` or `FAILED <code>`. It is the criterion (f5)
//!   control.
//!
//! Each mode opens at most one raw libsql connection in a single-threaded process, so libsql's
//! double close at exit (ub-q1u) has no other handle to hit.

#![forbid(unsafe_code)]

use std::fs::OpenOptions;
use std::io::{BufRead as _, Write as _};
use std::path::Path;
use std::time::Duration;

use libsql::{Builder, Connection, Value};

/// Rows the `hot-journal` transaction inserts. With [`DESCRIPTION_LEN`] bytes each they overflow
/// the small page cache, so `SQLite` syncs the journal header and writes pages into the database
/// file before the kill.
const ROWS: usize = 200;

/// Per-row `description` length in bytes.
const DESCRIPTION_LEN: usize = 4_000;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    assert!(
        args.len() == 3,
        "usage: exclusive_window_holder <reader|write-lock|hot-journal|begin-exclusive> <db>"
    );
    let db = Path::new(&args[2]);
    match args[1].as_str() {
        "reader" => reader(db).await,
        "write-lock" => write_lock(db),
        "hot-journal" => hot_journal(db).await,
        "begin-exclusive" => begin_exclusive(db).await,
        other => panic!("unknown mode {other:?}"),
    }
}

/// A raw connection to `db`.
async fn connect(db: &Path) -> Connection {
    let database = Builder::new_local(db)
        .build()
        .await
        .expect("open the database");
    database.connect().expect("connect")
}

/// Run a statement that returns rows, drain them, and return each row's first column.
async fn query(conn: &Connection, sql: &str) -> Vec<Value> {
    let mut rows = conn.query(sql, ()).await.expect(sql);
    let mut values = Vec::new();
    while let Some(row) = rows.next().await.expect(sql) {
        values.push(row.get_value(0).expect("column 0"));
    }
    values
}

/// Tell the parent the hold is in place.
fn ready() {
    println!("READY");
    std::io::stdout().flush().expect("flush READY");
}

/// Block until the parent writes a line or closes stdin.
fn wait_for_parent() {
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
}

async fn reader(db: &Path) {
    let conn = connect(db).await;
    let tables = query(&conn, "SELECT count(*) FROM sqlite_master").await;
    assert_eq!(tables.len(), 1, "one read served");
    ready();
    wait_for_parent();
    drop(conn);
}

fn write_lock(db: &Path) {
    let lock_path = db
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(".write.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .expect("open .write.lock");
    file.lock().expect("lock .write.lock");
    ready();
    wait_for_parent();
    drop(file);
}

async fn hot_journal(db: &Path) {
    let conn = connect(db).await;
    let mode = query(&conn, "PRAGMA journal_mode=DELETE").await;
    assert_eq!(
        mode,
        [Value::Text("delete".into())],
        "rollback-journal mode"
    );
    query(&conn, "PRAGMA cache_size=10").await;
    conn.execute("BEGIN IMMEDIATE", ())
        .await
        .expect("BEGIN IMMEDIATE");
    let description = "x".repeat(DESCRIPTION_LEN);
    for i in 0..ROWS {
        conn.execute(
            "INSERT INTO issues (id, title, description) VALUES (?1, ?2, ?3)",
            libsql::params![format!("hot-{i:05}"), "hot", description.as_str()],
        )
        .await
        .expect("insert (uncommitted)");
    }
    ready();
    // The parent SIGKILLs this process while the transaction is open.
    tokio::time::sleep(Duration::from_mins(2)).await;
}

async fn begin_exclusive(db: &Path) {
    let conn = connect(db).await;
    conn.busy_timeout(Duration::ZERO).expect("busy_timeout");
    let result = match conn.execute("BEGIN EXCLUSIVE", ()).await {
        Ok(_) => conn.execute("COMMIT", ()).await,
        Err(error) => Err(error),
    };
    match result {
        Ok(_) => println!("COMMITTED"),
        Err(libsql::Error::SqliteFailure(code, _)) => println!("FAILED {code}"),
        Err(other) => println!("FAILED {other}"),
    }
    std::io::stdout().flush().expect("flush the result");
}
