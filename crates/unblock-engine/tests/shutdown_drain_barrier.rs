//! T3.2/C4 — the engine drain-to-commit barrier (FR-17/NFR-5), the DETERMINISTIC anchor for AC
//! clauses (a) no WAL corruption and (b) an in-flight write fully commits or fully rolls back (never
//! partial), over the permit-drain mechanism itself (NO signal, NO real serve process).
//!
//! `Session::shutdown()` (lifecycle.rs) sets the shutdown flag, then `acquire_owned()`s the SAME
//! single write permit (D14, `WRITE_PERMITS = 1`) a mutation holds for its ENTIRE body — so a
//! `shutdown()` racing an in-flight `create_bulk` cannot return until that bulk's tx has committed
//! (or rolled back on its own error, never on the shutdown). This test makes that race deterministic
//! by parking `create_bulk`'s `storage.create_issues` call on a controllable gate
//! (`common::parked::ParkedStorage::new_gated_bulk`, wrapping a REAL on-disk `LibsqlStorage` — no
//! `Storage` mock) instead of relying on real signal timing.
//!
//! Built over `common::session_over_in_dir` (the same path the FR-9 concurrency tests use, e.g.
//! `linearizable.rs`) with a REAL `tempfile::tempdir()` + `LibsqlStorage::open_local(db).migrate()` —
//! NOT `session_over` (hardcoded `/tmp/unblock-test-ws`, a macOS symlink hazard) and NOT
//! `open_in_memory` (shared-cache, no real WAL/reopen).

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use common::parked::ParkedStorage;
use common::session_over_in_dir;
use unblock_engine::{NewIssue, SessionConfig};
use unblock_model::ListFilters;
use unblock_storage::{LibsqlStorage, Storage};

/// The bulk size — small and title-only (no intra-batch deps/parents), so the pre-insert mint/probe
/// steps (which run BEFORE the gated `storage.create_issues` call) are near-instant.
const N: usize = 5;

#[tokio::test]
async fn shutdown_parks_while_the_bulk_tx_is_held_then_drains_to_commit() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let workspace_dir: PathBuf = tmp.path().to_path_buf();
    let unblock_dir = workspace_dir.join(".unblock");
    std::fs::create_dir_all(&unblock_dir).expect("create .unblock");
    let db_path = unblock_dir.join("unblock.db");

    let inner = LibsqlStorage::open_local(&db_path, unblock_storage::DEFAULT_WRITE_LOCK_TIMEOUT_MS)
        .await
        .expect("open_local");
    inner.migrate().await.expect("migrate");
    let inner: Arc<dyn Storage> = Arc::new(inner);

    // The T3.2/C4 barrier: gates the BULK `create_issues` path (opt-in — the existing single-create
    // gate other tests rely on stays disarmed, see `common::parked` module docs).
    let parked = ParkedStorage::new_gated_bulk(inner);
    let storage: Arc<dyn Storage> = parked.clone();
    let session = Arc::new(
        session_over_in_dir(
            storage,
            SessionConfig::default(),
            workspace_dir.clone(),
            unblock_dir.clone(),
        )
        .await,
    );

    let records: Vec<NewIssue> = (0..N)
        .map(|k| NewIssue {
            title: format!("bulk-item-{k}"),
            ..NewIssue::default()
        })
        .collect();

    // Drive `create_bulk` in a spawned task: it acquires the single write permit up front
    // (write.rs:231) and holds it for its WHOLE body, so by the time the barrier signals "entered"
    // the permit is held.
    let writer_session = Arc::clone(&session);
    let writer = tokio::spawn(async move { writer_session.create_bulk(records).await });

    // Wait for "entered the gated bulk tx" — the permit is now held mid-tx.
    parked.wait_until_parked().await;

    // While the barrier holds the tx, `shutdown()`'s permit-drain must NOT return: a bounded timeout
    // proves it PARKS (not a vacuous "it happened to be fast" race).
    let parked_shutdown =
        tokio::time::timeout(Duration::from_millis(300), session.shutdown()).await;
    assert!(
        parked_shutdown.is_err(),
        "shutdown() must PARK while the barrier holds the write permit (D14 drain, spine §4.2) — \
         a lone cooperative shutdown never rolls back a started tx"
    );

    // Release the barrier: the bulk tx proceeds to commit, freeing the permit.
    parked.release();

    // Negative-timing leg: rely on `shutdown()` IDEMPOTENCY rather than re-polling the timed-out
    // future above (which was already dropped by the `timeout` — cancel-safe, spine §4.2). A FRESH
    // `shutdown()` call now drains cleanly. Both this drain AND the writer join are bounded by a
    // generous timeout so a drain-to-commit DEADLOCK regression fails FAST here (with a clear message)
    // instead of hanging until the CI job-level timeout.
    tokio::time::timeout(Duration::from_secs(5), session.shutdown())
        .await
        .expect("a fresh shutdown() must not deadlock after the barrier releases")
        .expect("a fresh shutdown() drains to commit after the barrier releases");

    let created = tokio::time::timeout(Duration::from_secs(5), writer)
        .await
        .expect("the create_bulk writer must not deadlock after the drain")
        .expect("writer task joins")
        .expect("create_bulk commits — a lone shutdown drains-to-commit, never rolls back");
    assert_eq!(
        created.len(),
        N,
        "the whole batch committed (no partial write)"
    );

    // Drop the barrier Session (+ its storage handle) BEFORE the fresh reopen.
    drop(session);
    drop(parked);

    // A fresh reopen (via `open_local`, the SAME production open path) sees exactly N committed rows.
    let reopened =
        LibsqlStorage::open_local(&db_path, unblock_storage::DEFAULT_WRITE_LOCK_TIMEOUT_MS)
            .await
            .expect("fresh reopen");
    let filters = ListFilters {
        include_closed: true,
        include_deferred: true,
        ..ListFilters::default()
    };
    let count = reopened.list_issues(&filters).await.expect("list").len();
    assert_eq!(count, N, "a fresh reopen sees exactly the committed N rows");
}
