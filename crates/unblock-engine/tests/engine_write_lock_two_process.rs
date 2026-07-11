//! AC-1 / M1 (D31/T3.4.1) — the ENGINE-level two-process cross-process write-lock proof.
//!
//! The storage-level proof (`unblock-storage/tests/write_lock_two_process.rs`) calls
//! `Storage::acquire_write_lock` DIRECTLY, so it stays GREEN even if the lock is dropped from the
//! production engine `Session::acquire()`. This test closes that gap: two **separate OS processes**
//! drive the REAL engine `Session::create_issue` (the MINTING path) concurrently under the SAME parent
//! on the SAME file DB — the child-per-client stdio topology (multiple `unblock serve` on one
//! `unblock.db`, PRD §8.2).
//!
//! The engine mints `parent.N` from a pre-tx `next_child_number` READ held under the D31 `.write.lock`
//! (acquired inside `Session::acquire`). With the lock wired, the two processes serialize the whole
//! mutation across processes, so every child gets a DISTINCT `parent.N` and there is ZERO
//! `IdCollision`. **Deleting `storage.acquire_write_lock()` from `Session::acquire()` (write.rs) turns
//! this RED** — both processes read the same `N` inside the (decorator-widened) read→insert window and
//! the second insert reproduces the cross-process `IdCollision`. That mutation is the M1 non-vacuity.

#![cfg(unix)]

use std::collections::HashSet;
use std::process::Command;

use chrono::Utc;
use unblock_model::Issue;
use unblock_storage::{DEFAULT_WRITE_LOCK_TIMEOUT_MS, LibsqlStorage, Storage};

/// Children each process attempts under the shared parent.
const COUNT: u32 = 15;
/// The shared parent id both processes mint children under. A canonical root id, so the engine's
/// `IssueValidator` accepts the minted child ids `ub-abc123.N` (`is_valid_id_format`).
const PARENT: &str = "ub-abc123";

/// The outcome of the two-process scenario: the set of DISTINCT child ids that persisted across BOTH
/// processes, and the total number of `IdCollision`s the two processes reported.
struct Outcome {
    distinct: u32,
    collisions: u32,
}

/// Pre-migrate a fresh temp db and seed the shared parent (a SEPARATELY committed row before either
/// child process opens the file), then run TWO `engine_write_lock_race` processes concurrently.
async fn run() -> Outcome {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("unblock.db");

    // Pre-migrate + seed the parent via the PUBLIC storage open path (the import/create path preserves
    // the caller id); drop the store so its connections close before the child processes open the file.
    // The child processes NEVER migrate (migrate fail-fasts under a held lock, MF2) — the harness owns
    // the one-time migration here.
    {
        let storage = LibsqlStorage::open_local(&db_path, DEFAULT_WRITE_LOCK_TIMEOUT_MS)
            .await
            .expect("open_local");
        storage.migrate().await.expect("migrate");
        let now = Utc::now();
        let parent = Issue {
            id: PARENT.to_string(),
            title: "parent".to_string(),
            created_at: now,
            updated_at: now,
            ..Issue::default()
        };
        storage
            .create_issue(&parent, "seed")
            .await
            .expect("seed the parent");
    }

    let bin = env!("CARGO_BIN_EXE_engine_write_lock_race");
    let spawn = |actor: &str| {
        Command::new(bin)
            .arg(&db_path)
            .arg(PARENT)
            .arg(COUNT.to_string())
            .arg(actor)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn engine_write_lock_race")
    };

    // Spawn BOTH first so they run concurrently, then collect both outputs.
    let c1 = spawn("proc-1");
    let c2 = spawn("proc-2");
    let out1 = c1.wait_with_output().expect("wait proc-1");
    let out2 = c2.wait_with_output().expect("wait proc-2");

    for (label, out) in [("proc-1", &out1), ("proc-2", &out2)] {
        assert!(
            out.status.success(),
            "{label} exited non-zero (stderr: {})",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let mut distinct: HashSet<String> = HashSet::new();
    let mut collisions = 0u32;
    for out in [&out1, &out2] {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if let Some(id) = line.strip_prefix("MINTED=") {
                assert!(
                    distinct.insert(id.to_string()),
                    "id {id} persisted twice across processes — a lost cross-process collision"
                );
            } else if let Some(n) = line.strip_prefix("COLLISIONS=") {
                collisions += n.parse::<u32>().expect("collisions count");
            }
        }
    }

    Outcome {
        distinct: u32::try_from(distinct.len()).expect("distinct count fits u32"),
        collisions,
    }
}

/// M1 headline: two OS processes driving the REAL engine `Session::create_issue` under one parent must
/// mint DISTINCT `parent.N` ids with ZERO `IdCollision` — because `Session::acquire` holds the D31
/// `.write.lock` across the whole mutation. Deleting that acquire (write.rs) reproduces the collision
/// (the non-vacuity, checked by mutation).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn engine_session_two_process_write_lock_prevents_id_collision() {
    let outcome = run().await;
    assert_eq!(
        outcome.collisions, 0,
        "with the engine-wired .write.lock, two Session processes must not collide on parent.N"
    );
    assert_eq!(
        outcome.distinct,
        2 * COUNT,
        "with the lock, all {} children get distinct ids",
        2 * COUNT
    );
}
