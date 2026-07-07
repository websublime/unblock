//! T3.2/C5 test-only helper binary (FR-17/NFR-5) — drives RAW libsql (already a private dep of
//! `unblock-storage`, `Cargo.toml:27` — NO new dep) to hold an UNCOMMITTED `BEGIN IMMEDIATE`
//! transaction open on a PRE-MIGRATED workspace database, so `tests/shutdown_abandoned_tx.rs` can
//! `Child::kill()` (SIGKILL) it and prove WAL-recovery discards the abandoned tx on reopen (never a
//! partial commit) — the exact no-destructors semantics of the shutdown.rs second-signal
//! `process::exit` path.
//!
//! `LibsqlStorage::create_issues` is ONE atomic `with_immediate_tx` (crud.rs) and cannot be paused
//! mid-flight, and its `WriteHook` seam is `pub(super)` — so this helper cannot reuse it and instead
//! opens its OWN raw libsql connection directly against the same on-disk file the parent pre-migrated
//! via the public `LibsqlStorage::open_local(db).migrate()`.
//!
//! Usage: `c5_abandoned_tx <path-to-pre-migrated-db>`. Once the uncommitted INSERT loop has run it
//! prints the `READY-IN-TX` marker to stdout (flushed), then sleeps — the parent kills it while the
//! tx is open (it never lets `main` return normally, which would drop `tx` and roll it back via the
//! ordinary libsql `Drop` path — the whole point here is the abandoned-tx crash-recovery path
//! instead, not a cooperative rollback).

#![forbid(unsafe_code)]

use std::io::Write as _;
use std::time::Duration;

use libsql::{Builder, TransactionBehavior};

/// The number of rows the uncommitted tx inserts. Each row's `description` is large enough (see
/// [`DESCRIPTION_LEN`]) that the total dirty-page volume exceeds the small `cache_size` set below,
/// forcing `SQLite`'s cache-spill (ON by default) to write dirty frames to the `-wal` sidecar BEFORE
/// commit — so the crash-recovery discard the integration test proves is genuinely non-vacuous (real
/// frames existed in the WAL at kill time, not just an in-memory page cache).
const ROWS: usize = 200;

/// Per-row `description` length (bytes). `title` carries `CHECK(length(title) <= 500)`
/// (`schema.rs`) — `description` has no such CHECK, so the padding lives there.
const DESCRIPTION_LEN: usize = 4_000;

#[tokio::main]
async fn main() {
    let db_path = std::env::args()
        .nth(1)
        .expect("usage: c5_abandoned_tx <path-to-pre-migrated-db>");

    let database = Builder::new_local(&db_path)
        .build()
        .await
        .expect("open the pre-migrated workspace db");
    let conn = database.connect().expect("connect");

    // A tiny page cache forces SQLite's cache-spill to write dirty pages to the WAL before commit.
    let _ = conn
        .query("PRAGMA cache_size = 10", ())
        .await
        .expect("set a small cache_size");

    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await
        .expect("BEGIN IMMEDIATE");

    let description = "x".repeat(DESCRIPTION_LEN);
    for i in 0..ROWS {
        let id = format!("c5-abandoned-{i:05}");
        tx.execute(
            "INSERT INTO issues (id, title, description) VALUES (?1, ?2, ?3)",
            libsql::params![id, "abandoned", description.as_str()],
        )
        .await
        .expect("insert (uncommitted)");
    }

    // The marker: printed AFTER every insert has run, so the parent only observes it once the tx is
    // genuinely open with ROWS uncommitted rows (some of which have spilled to the WAL).
    println!("READY-IN-TX");
    std::io::stdout().flush().expect("flush the marker");

    // Sleep well past any reasonable parent timeout — the parent SIGKILLs this process while the tx
    // is open. `tx` stays alive (never dropped, never committed) for as long as this process runs.
    tokio::time::sleep(Duration::from_mins(2)).await;
}
