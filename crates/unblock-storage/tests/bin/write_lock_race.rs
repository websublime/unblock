//! AC-1 helper `[[bin]]` (D31/T3.4.1) — one OS process in the cross-process write-lock race.
//!
//! Reproduces the engine's WHOLE-MUTATION id-allocation path against a shared file DB: for each of
//! `count` children it reads `next_child_number(parent)` (the allocation READ), mints
//! `child_id(parent, N)`, and inserts it via `create_issue` (the write). In **`locked`** mode it holds
//! the cross-process advisory `.write.lock` (`Storage::acquire_write_lock`) across that READ + insert —
//! exactly the span the engine holds it (spine §4.2). In **`nolock`** mode it skips the lock (AC-1's
//! bypass control), so two concurrent processes race the shared `parent.N` namespace.
//!
//! **The gate (ub-fh5).** The FIRST mutation is a rendezvous with the parent test over stdio, so the
//! AC-1 outcome does not depend on the two processes overlapping in time. Under host load one process
//! can finish its whole loop before the other has opened the db.
//!
//! - A process that is INSIDE the `read → insert` window prints `GATE=entered` after its READ and
//!   parks until the parent writes `GO` on its stdin. The parent releases nobody until BOTH processes
//!   have reported, so in `nolock` mode both read the same `N` before either inserts and the
//!   collision is certain.
//! - In `locked` mode the first acquire is a single `try_lock` (a second handle opened with
//!   `lock_timeout_ms = 0`). The winner holds the lock from that try until the parent releases it
//!   from the window, so the other one's try is REFUSED. That process prints `GATE=refused` and then
//!   queues on the ordinary acquire. One `entered` + one `refused` is the witness that the lock kept a
//!   second process out of the window.
//!
//! The remaining `count - 1` mutations run ungated and race freely.
//!
//! Usage: `write_lock_race <db-path> <parent-id> <locked|nolock> <count> <actor>`. Emits one
//! `GATE=<entered|refused>` line, one `MINTED=<id>` line per committed child and a final
//! `COLLISIONS=<n>` line to stdout; exits 3 on any unexpected storage error.

#![forbid(unsafe_code)]

use std::io::BufRead;
use std::path::Path;
use std::time::Duration;

use chrono::Utc;
use unblock_model::{Issue, child_id};
use unblock_storage::{DEFAULT_WRITE_LOCK_TIMEOUT_MS, LibsqlStorage, Storage, StorageError};

/// Widen the `read → insert` window so the ungated mutations after the gate overlap more often. The
/// AC-1 outcome rests on the gate alone.
const RACE_WINDOW: Duration = Duration::from_millis(3);

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    assert!(
        args.len() == 6,
        "usage: write_lock_race <db> <parent> <locked|nolock> <count> <actor>"
    );
    let db = Path::new(&args[1]);
    let parent = &args[2];
    let locked = match args[3].as_str() {
        "locked" => true,
        "nolock" => false,
        other => panic!("mode must be `locked` or `nolock`, got {other:?}"),
    };
    let count: u32 = args[4].parse().expect("count is a u32");
    let actor = &args[5];

    let storage = LibsqlStorage::open_local(db, DEFAULT_WRITE_LOCK_TIMEOUT_MS)
        .await
        .expect("open_local the shared db");
    // In `locked` mode the gate enters through a second handle with `lock_timeout_ms = 0`, whose
    // `acquire_write_lock` is a single `try_lock`.
    let gate_store = if locked {
        Some(
            LibsqlStorage::open_local(db, 0)
                .await
                .expect("open_local the gate handle"),
        )
    } else {
        None
    };

    let mut collisions = 0u32;
    for i in 0..count {
        let gated = i == 0;
        let mut entered = gated;
        // The whole-mutation lock spans the READ + the insert (locked mode only) — exactly what the
        // engine does. Held for the iteration, released at its end.
        let _guard = match (&gate_store, gated) {
            (None, _) => None,
            (Some(gate_store), true) => match gate_store.acquire_write_lock().await {
                Ok(guard) => guard,
                Err(StorageError::DatabaseLocked) => {
                    println!("GATE=refused");
                    entered = false;
                    storage
                        .acquire_write_lock()
                        .await
                        .expect("acquire_write_lock")
                }
                Err(err) => unexpected(&err),
            },
            (Some(_), false) => storage
                .acquire_write_lock()
                .await
                .expect("acquire_write_lock"),
        };

        let n = storage
            .next_child_number(parent)
            .await
            .expect("next_child_number");
        let id = child_id(parent, n);

        if entered {
            println!("GATE=entered");
            wait_for_go();
        }
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
            Err(err) => unexpected(&err),
        }
    }

    println!("COLLISIONS={collisions}");
}

/// Park inside the `read → insert` window until the parent writes `GO` on stdin.
fn wait_for_go() {
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .expect("read the parent's GO");
    assert_eq!(
        line.trim_end(),
        "GO",
        "the parent releases the gate with GO"
    );
}

/// Report an unexpected storage error and exit 3.
fn unexpected(err: &StorageError) -> ! {
    eprintln!("UNEXPECTED={err:?}");
    std::process::exit(3);
}
