//! AC-1 helper `[[bin]]` (D31/T3.4.1) — one OS process in the cross-process write-lock race.
//!
//! Reproduces the engine's WHOLE-MUTATION id-allocation path against a shared file DB: for each of
//! `count` children it reads `next_child_number(parent)` (the allocation READ), mints
//! `child_id(parent, N)`, and inserts it via `create_issue` (the write). In **`locked`** mode it holds
//! the cross-process advisory `.write.lock` (`Storage::acquire_write_lock`) across that READ + insert —
//! exactly the span the engine holds it (spine §4.2). In **`nolock`** mode it skips the lock (AC-1's
//! bypass control), so two concurrent processes race the shared `parent.N` namespace.
//!
//! A small `read → insert` sleep makes the inherent race **deterministic**: inside the lock (locked
//! mode) the two processes serialize and never collide; in the racy window (nolock mode) both read the
//! same `N` and both mint `parent.N`, so the second insert hits the storage id-collision guard.
//!
//! Usage: `write_lock_race <db-path> <parent-id> <locked|nolock> <count> <actor>`. Emits one
//! `MINTED=<id>` line per committed child and a final `COLLISIONS=<n>` line to stdout; exits 3 on any
//! unexpected storage error.

#![forbid(unsafe_code)]

use std::path::Path;
use std::time::Duration;

use chrono::Utc;
use unblock_model::{Issue, child_id};
use unblock_storage::{DEFAULT_WRITE_LOCK_TIMEOUT_MS, LibsqlStorage, Storage, StorageError};

/// Widen the `read → insert` window so the (inherent) cross-process race is deterministic — inside
/// the lock in `locked` mode (serialized, no collision), in the racy window in `nolock` mode.
const RACE_WINDOW: Duration = Duration::from_millis(3);

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    assert!(
        args.len() == 6,
        "usage: write_lock_race <db> <parent> <locked|nolock> <count> <actor>"
    );
    let db = &args[1];
    let parent = &args[2];
    let locked = match args[3].as_str() {
        "locked" => true,
        "nolock" => false,
        other => panic!("mode must be `locked` or `nolock`, got {other:?}"),
    };
    let count: u32 = args[4].parse().expect("count is a u32");
    let actor = &args[5];

    let storage = LibsqlStorage::open_local(Path::new(db), DEFAULT_WRITE_LOCK_TIMEOUT_MS)
        .await
        .expect("open_local the shared db");

    let mut collisions = 0u32;
    for _ in 0..count {
        // The whole-mutation lock spans the READ + the insert (locked mode only) — exactly what the
        // engine does. Held for the iteration, released at its end.
        let _guard = if locked {
            storage
                .acquire_write_lock()
                .await
                .expect("acquire_write_lock")
        } else {
            None
        };

        let n = storage
            .next_child_number(parent)
            .await
            .expect("next_child_number");
        let id = child_id(parent, n);

        tokio::time::sleep(RACE_WINDOW).await;

        let now = Utc::now();
        let issue = Issue {
            id: id.clone(),
            title: format!("child {id}"),
            created_at: now,
            updated_at: now,
            ..Issue::default()
        };
        match storage.create_issue(&issue, actor).await {
            Ok(_) => println!("MINTED={id}"),
            // The cross-process collision the lock exists to prevent.
            Err(StorageError::IdCollision { .. }) => collisions += 1,
            Err(err) => {
                eprintln!("UNEXPECTED={err:?}");
                std::process::exit(3);
            }
        }
    }

    println!("COLLISIONS={collisions}");
}
