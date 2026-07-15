//! File-backed heavy-corpus parallel stress regression (T3.5.1 Verify follow-up; unblock-storage.md
//! §5 OQ-8).
//!
//! **Un-gated** (unlike `tests/scale.rs`, which needs `--features testkit` for its
//! `testkit::seed_corpus` import) so this runs in the always-on `cargo test --workspace` set — the
//! exact suite under which the T3.5.1 residual flake surfaced. Being un-gated, it builds its own heavy
//! batch inline rather than importing the testkit-gated `seed_corpus`.
//!
//! This is the **file analogue of `open_in_memory_parallel_first_write_stress`** (`src/libsql/mod.rs`)
//! but with the *heavy* bulk workload that surfaced the flake on the in-memory path: [`TASKS`] parallel
//! tasks each open their own [`LibsqlStorage::open_local`]-backed store, migrate it, insert one
//! [`HEAVY_ROWS`]-issue batch via a single [`Storage::create_issues`] call, and read every row back.
//! Every task must succeed — zero failures.
//!
//! # Why in-memory can't carry this load
//!
//! [`LibsqlStorage::open_in_memory`]'s docs explain the boundary this test pins in place: opening a
//! shared-cache `:memory:` URI mutates `SQLite`'s process-global shared-cache registry, and
//! `memory_open_lock` serializes only the **open-vs-open** race (T0.9). It cannot cover the window
//! where an open races a concurrent *heavy* shared-cache transaction (or a store close/Drop) on another
//! in-memory instance — the residual window behind the T3.5.1 flake (~2/15 under full-workspace
//! parallel load; not reproducible in a dedicated single-process harness, so it is not a bug an
//! in-memory-only fix could safely target). The file path sidesteps the whole class: `open_local` opens
//! a private file with no shared cache, so there is no global registry to race. This test proves the
//! file path carries the identical heavy load cleanly — pinning the boundary `open_in_memory`'s docs now
//! prescribe: heavy or high-concurrency corpus work belongs on `open_local`, never `open_in_memory`.

use chrono::{DateTime, TimeZone, Utc};

use unblock_model::Issue;
use unblock_storage::{DEFAULT_WRITE_LOCK_TIMEOUT_MS, LibsqlStorage, Storage, StorageError};

/// The T3.5.1 heavy-batch row count — the size of the single `create_issues` batch (per task) that
/// surfaced the residual `open_in_memory` shared-cache flake under full-workspace parallel load.
const HEAVY_ROWS: usize = 902;

/// Parallel tasks, each driving its own file-backed store through one [`HEAVY_ROWS`]-issue batch — the
/// file analogue of `open_in_memory_parallel_first_write_stress`'s 32 tasks (halved here since every
/// task does a heavy bulk insert rather than a single row, keeping total runtime bounded).
const TASKS: usize = 16;

fn ts(year: i32, month: u32, day: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(year, month, day, 0, 0, 0).unwrap()
}

/// Build the `i`-th synthetic issue for `task`'s heavy batch: a unique `ub-h<task>-<i>` id (zero-padded
/// so every id is syntactically valid) at a fixed epoch. Mirrors
/// `open_in_memory_parallel_first_write_stress`'s `Issue { ..Issue::default() }` build (`src/libsql/mod.rs`)
/// and `testkit::seed_corpus`'s `seed_issue` shape (`src/testkit.rs`).
fn heavy_issue(task: usize, i: usize, created: DateTime<Utc>) -> Issue {
    Issue {
        id: format!("ub-h{task}-{i:04}"),
        title: format!("heavy stress task {task} issue {i}"),
        created_at: created,
        updated_at: created,
        ..Issue::default()
    }
}

/// The file-backed heavy-corpus analogue of `open_in_memory_parallel_first_write_stress`: [`TASKS`]
/// parallel tasks each `open_local` their own tempdir-backed store, migrate it, insert one
/// [`HEAVY_ROWS`]-issue batch via a single `create_issues` call, then read every row back. Every task
/// must succeed — proving the file path carries the heavy load the in-memory path cannot (T3.5.1,
/// unblock-storage.md §5 OQ-8).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn file_backed_heavy_corpus_parallel_stress() {
    let created = ts(2026, 1, 1);
    let mut handles = Vec::new();
    for task in 0..TASKS {
        handles.push(tokio::spawn(async move {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("unblock.db");
            let storage = LibsqlStorage::open_local(&path, DEFAULT_WRITE_LOCK_TIMEOUT_MS).await?;
            storage.migrate().await?;

            let issues: Vec<Issue> = (0..HEAVY_ROWS)
                .map(|i| heavy_issue(task, i, created))
                .collect();
            storage.create_issues(&issues, "heavy-stress").await?;

            // Read every inserted row back through the public API — the non-vacuous proof the whole
            // heavy batch landed (not merely that `create_issues` returned `Ok`).
            let ids: Vec<String> = issues.iter().map(|issue| issue.id.clone()).collect();
            let fetched = storage.get_issues(&ids).await?;
            assert_eq!(
                fetched.len(),
                HEAVY_ROWS,
                "task {task}: every row in the heavy batch must land"
            );

            drop(storage);
            drop(dir);
            Ok::<(), StorageError>(())
        }));
    }

    for (task, handle) in handles.into_iter().enumerate() {
        handle
            .await
            .expect("join")
            .unwrap_or_else(|e| panic!("task {task} failed: {e:?}"));
    }
}
