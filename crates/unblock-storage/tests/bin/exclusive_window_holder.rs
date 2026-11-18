//! PRD §4 D58 criteria (f), (g) and (n) test-only helper `[[bin]]`. It is the SECOND process that
//! holds a database, or its `.write.lock`, while `tests/exclusive_window.rs` runs the exclusive
//! window, and it plays the processes of the (n) descriptor-close lock-loss cells (`ub-2vp`).
//!
//! Run it as `exclusive_window_holder <mode> <db-path>`, or as `exclusive_window_holder window
//! <db-path> <run-dir>`.
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
//! - `lock-probe` (process A) opens the database through `LibsqlStorage::open_local`, serves one
//!   read so it holds `SQLite`'s SHARED lock, prints `READY`, then answers one line per stdin
//!   command (`PROBE-V1`, `NOOP`, `SQLITE-REOPEN`, `READ`, `LIVE`, `PRIMITIVES`, `WRITE <id>`,
//!   `GET <id>`, `VERSION`, `QUIT`). `PROBE-V1` runs the v1.0.x header probe, copied verbatim.
//! - `observe-exclusive` (observer O) runs `PRAGMA locking_mode=EXCLUSIVE`, `BEGIN EXCLUSIVE` and
//!   `COMMIT` with `busy_timeout = 0`, then prints `LOCKED`, `BUSY <raw code>` or `FAILED <error>`.
//! - `read-and-close` (process B) opens through `open_local`, serves one read, closes, and prints
//!   `CLOSED`.
//! - `window` (process W, `testkit` only) runs the real exclusive window with the two fd7
//!   observation hooks, each of which prints a line and waits for the parent's next line.
//!
//! The raw-connection modes (`reader`, `hot-journal`, `begin-exclusive`, `observe-exclusive`) open
//! at most one raw libsql connection in a single-threaded process, so libsql's double close at exit
//! (ub-q1u) has no other handle to hit. The other modes open only close-once connections.

#![forbid(unsafe_code)]

use std::fs::OpenOptions;
use std::io::{BufRead as _, Write as _};
use std::path::Path;
use std::time::Duration;

use chrono::Utc;
use libsql::{Builder, Connection, Value};
use unblock_model::Issue;
use unblock_storage::{
    DEFAULT_WRITE_LOCK_TIMEOUT_MS, LibsqlStorage, Storage, live_connections_to, lost_database,
    probe_write_lock,
};

/// Rows the `hot-journal` transaction inserts. With [`DESCRIPTION_LEN`] bytes each they overflow
/// the small page cache, so `SQLite` syncs the journal header and writes pages into the database
/// file before the kill.
const ROWS: usize = 200;

/// Per-row `description` length in bytes.
const DESCRIPTION_LEN: usize = 4_000;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arity = if args.get(1).map(String::as_str) == Some("window") {
        4
    } else {
        3
    };
    assert!(
        args.len() == arity,
        "usage: exclusive_window_holder <reader|write-lock|hot-journal|begin-exclusive|lock-probe|\
         observe-exclusive|read-and-close> <db> | exclusive_window_holder window <db> <run-dir>"
    );
    let db = Path::new(&args[2]);
    match args[1].as_str() {
        "reader" => reader(db).await,
        "write-lock" => write_lock(db),
        "hot-journal" => hot_journal(db).await,
        "begin-exclusive" => begin_exclusive(db).await,
        "lock-probe" => lock_probe(db).await,
        "observe-exclusive" => observe_exclusive(db).await,
        "read-and-close" => read_and_close(db).await,
        #[cfg(feature = "testkit")]
        "window" => window(db, Path::new(&args[3])).await,
        #[cfg(not(feature = "testkit"))]
        "window" => panic!("the window mode needs --features testkit"),
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

// ---- (n) descriptor-close lock loss (ub-2vp) ----

/// The 16-byte `SQLite` magic header a valid database file starts with.
///
/// Copied VERBATIM, with [`db_header_mismatches_sqlite`], from tag v1.0.2 (commit
/// `842cdff3909ac356ac73f116d85516839167afba`), `crates/unblock-health/src/file_state.rs` (blob
/// `77b62bb1328c69545ed87319655243e579d3b4c5`, identical at v1.0.0 and v1.0.1), lines `:31` and
/// `:203-211`. Its caller there, `classify_file_state`, checks `db.is_file()` first.
const SQLITE_MAGIC: [u8; 16] = *b"SQLite format 3\0";

/// Whether the db opens but its first 16 bytes are not the `SQLite` magic (or are shorter than 16).
///
/// An OPEN failure returns `false` (not flagged) — faithful to the original's conservative
/// `if let Ok(file) = File::open(..)` guard.
fn db_header_mismatches_sqlite(db: &Path) -> bool {
    use std::io::Read as _;

    let Ok(mut file) = std::fs::File::open(db) else {
        return false;
    };
    let mut header = [0_u8; 16];
    file.read_exact(&mut header).is_err() || header != SQLITE_MAGIC
}

/// Write one reply line and flush it.
fn reply(line: &str) {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{line}").expect("write the reply");
    stdout.flush().expect("flush the reply");
}

/// The next stdin line without its line ending, or `None` at EOF.
fn next_command() -> Option<String> {
    let mut line = String::new();
    let read = std::io::stdin()
        .lock()
        .read_line(&mut line)
        .expect("read a command");
    (read > 0).then(|| line.trim_end().to_owned())
}

/// Open `db` the way production does, with two close-once connections, the production pragmas
/// and the registry.
async fn open_store(db: &Path) -> LibsqlStorage {
    LibsqlStorage::open_local(db, DEFAULT_WRITE_LOCK_TIMEOUT_MS)
        .await
        .expect("open_local")
}

/// Process A. The store's first read takes `SQLite`'s SHARED lock, which a WAL connection keeps.
async fn lock_probe(db: &Path) {
    let store = open_store(db).await;
    store.schema_version().await.expect("the first read");
    ready();
    while let Some(command) = next_command() {
        let (verb, arg) = command
            .split_once(' ')
            .map_or((command.as_str(), ""), |(verb, arg)| (verb, arg));
        match verb {
            "PROBE-V1" => {
                // This is the v1.0.x call site, `db_is_file && db_header_mismatches_sqlite(db)`.
                // The `File` is closed before the reply.
                let mismatch = db.is_file() && db_header_mismatches_sqlite(db);
                reply(&format!("PROBED mismatch={mismatch}"));
            }
            "NOOP" => reply("OK"),
            "SQLITE-REOPEN" => {
                let second = open_store(db).await;
                second.schema_version().await.expect("the reopened read");
                drop(second);
                reply("REOPENED");
            }
            "READ" => match store.schema_version().await {
                Ok(version) => reply(&format!("READ {version}")),
                Err(error) => reply(&format!("READ-FAILED {error:?}")),
            },
            "LIVE" => {
                let live = live_connections_to(db).expect("live_connections_to");
                reply(&format!("LIVE {live}"));
            }
            "PRIMITIVES" => {
                let lost = lost_database(db).expect("lost_database");
                let live = live_connections_to(db).expect("live_connections_to");
                let write_lock = probe_write_lock(db).expect("probe_write_lock");
                reply(&format!(
                    "PRIMITIVES lost={lost} live={live} write_lock={write_lock}"
                ));
            }
            "WRITE" => {
                let now = Utc::now();
                let issue = Issue {
                    id: arg.to_owned(),
                    title: format!("ub-2vp {arg}"),
                    created_at: now,
                    updated_at: now,
                    ..Issue::default()
                };
                match store.create_issue(&issue, "ub-2vp").await {
                    Ok(id) => reply(&format!("WROTE {id}")),
                    Err(error) => reply(&format!("WRITE-FAILED {error:?}")),
                }
            }
            "GET" => match store.get_issue(arg).await {
                Ok(Some(_)) => reply("FOUND"),
                Ok(None) => reply("ABSENT"),
                Err(error) => reply(&format!("GET-FAILED {error:?}")),
            },
            "VERSION" => reply(&format!("SQLITE {}", libsql::version())),
            "QUIT" => {
                reply("BYE");
                break;
            }
            other => panic!("unknown lock-probe command {other:?}"),
        }
    }
    drop(store);
}

/// Observer O is a fresh process that tries the window's exclusivity proof once.
async fn observe_exclusive(db: &Path) {
    let conn = connect(db).await;
    conn.busy_timeout(Duration::ZERO).expect("busy_timeout");
    let result = async {
        let mut rows = conn.query("PRAGMA locking_mode=EXCLUSIVE", ()).await?;
        while rows.next().await?.is_some() {}
        drop(rows);
        conn.execute("BEGIN EXCLUSIVE", ()).await?;
        conn.execute("COMMIT", ()).await?;
        Ok::<(), libsql::Error>(())
    }
    .await;
    match result {
        Ok(()) => println!("LOCKED"),
        Err(libsql::Error::SqliteFailure(code, message)) => {
            if code & 0xff == 5 {
                println!("BUSY {code}");
            } else {
                println!("FAILED {code} {message}");
            }
        }
        Err(other) => println!("FAILED {other}"),
    }
    std::io::stdout().flush().expect("flush the result");
}

/// Process B is an ordinary process whose last close may checkpoint and delete the sidecars.
async fn read_and_close(db: &Path) {
    let store = open_store(db).await;
    store.schema_version().await.expect("one read");
    drop(store);
    println!("CLOSED");
    std::io::stdout().flush().expect("flush CLOSED");
}

/// Print `line`, then block until the parent sends `NEXT`.
#[cfg(feature = "testkit")]
fn pause(line: &str) {
    reply(line);
    let next = next_command();
    assert_eq!(next.as_deref(), Some("NEXT"), "the window waits for NEXT");
}

/// Process W runs the real exclusive window, observed at P1 (after step (c)'s lock, before the
/// evidence check) and P2 (after the evidence check). The hooks block the current-thread runtime,
/// which is safe because no other task needs progress.
#[cfg(feature = "testkit")]
async fn window(db: &Path, run_dir: &Path) {
    let request = unblock_storage::ExclusiveRequest::new(run_dir.to_path_buf())
        .quarantine_shm()
        .checkpoint_truncate()
        .with_after_exclusive_lock(Box::new(|_db: &Path| pause("EXCLUSIVE-HELD")))
        .with_after_evidence_check(Box::new(|_db: &Path| pause("EVIDENCE-CHECKED")));
    ready();
    let go = next_command();
    assert_eq!(go.as_deref(), Some("GO"), "the window waits for GO");
    let outcome = unblock_storage::exclusive_window(db, &request)
        .await
        .expect("exclusive_window");
    reply(&format!(
        "OUTCOME refused={:?} checkpoint_truncated={} renamed={}",
        outcome.refused,
        outcome.checkpoint_truncated,
        outcome.renamed.len()
    ));
    wait_for_parent();
}
