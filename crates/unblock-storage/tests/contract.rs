//! NFR-16 gate: run the backend-independent `Storage` contract suite against `LibsqlStorage`.
//!
//! The suite (`unblock_storage::run_storage_contract_suite`) is generic over a storage factory; this
//! file binds it to the two libsql constructors — `open_in_memory` (shared-cache) and `open_local`
//! (a temp-file WAL DB) — so the contract is proven on **both** the in-memory and the file-backed
//! paths. Each factory call yields a **fresh, migrated** store (no cross-case state), the temp-file
//! leg using a unique filename per call.
//!
//! This is the reusable proof the v2+ pluggable-backend seam relies on: a future backend supplies a
//! factory and reuses the exact same suite.
//!
//! An **integration** test compiles the library **without** its `#[cfg(test)]` items, so the suite +
//! seam are only reachable here through the `testkit` **feature**. The whole file is therefore gated
//! on `feature = "testkit"`: run it via `cargo test -p unblock-storage --features testkit`. Without
//! the feature this file is empty (so a plain `cargo test -p unblock-storage` still compiles + runs
//! `behaviour.rs` and the in-crate `libsql::testkit`-backed unit tests).

#![cfg(feature = "testkit")]

use std::sync::atomic::{AtomicU64, Ordering};

use unblock_storage::{LibsqlStorage, Storage};

/// A fresh migrated in-memory store.
async fn fresh_in_memory() -> LibsqlStorage {
    let storage = LibsqlStorage::open_in_memory()
        .await
        .expect("open_in_memory");
    storage.migrate().await.expect("migrate");
    storage
}

/// Monotonic counter giving each temp-file factory call a unique filename within the shared `TempDir`.
static FILE_DB_SEQ: AtomicU64 = AtomicU64::new(0);

#[tokio::test]
async fn contract_suite_in_memory() {
    unblock_storage::run_storage_contract_suite(fresh_in_memory).await;
}

#[tokio::test]
async fn contract_suite_temp_file() {
    // One TempDir for the whole suite run; each factory call gets a UNIQUE filename inside it, so the
    // fresh-per-case guarantee holds (no DB file is reused across cases). The TempDir is owned for
    // the duration of the suite and cleaned up on drop.
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path().to_path_buf();

    let factory = move || {
        let base = base.clone();
        async move {
            let seq = FILE_DB_SEQ.fetch_add(1, Ordering::Relaxed);
            let path = base.join(format!("contract-{seq}.db"));
            let storage = LibsqlStorage::open_local(&path).await.expect("open_local");
            storage.migrate().await.expect("migrate");
            storage
        }
    };

    unblock_storage::run_storage_contract_suite(factory).await;
    // `dir` drops here, removing every per-case DB file.
}
