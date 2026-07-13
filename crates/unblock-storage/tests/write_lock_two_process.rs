//! AC-1 (D31/T3.4.1) — the HEADLINE two-process cross-process write-lock proof.
//!
//! Two **separate OS processes** (the `write_lock_race` `[[bin]]`) concurrently `create_issue` under
//! the SAME parent on the SAME file DB, mirroring the child-per-client stdio topology (multiple
//! MCP servers (`unblock mcp`) on one `unblock.db`, PRD §8.2). The engine mints `parent.N` from a pre-tx
//! `next_child_number` READ, so two processes that both read `N` before either commits would both mint
//! `parent.N` — a cross-process `IdCollision` a tx-scoped lock cannot close.
//!
//! - **With `.write.lock` ACTIVE (`locked`):** ids are DISTINCT and every child commits — ZERO
//!   `IdCollision` (the whole-mutation cross-process lock serializes the READ + insert across
//!   processes).
//! - **With the lock BYPASSED (`nolock`, the control):** the SAME test REPRODUCES the `IdCollision`
//!   (fewer than `2 * COUNT` distinct children persist) — proving the lock is load-bearing AND covers
//!   the whole mutation, not just the tx.

#![cfg(unix)]

use std::collections::HashSet;
use std::process::Command;

use chrono::Utc;
use unblock_model::Issue;
use unblock_storage::{DEFAULT_WRITE_LOCK_TIMEOUT_MS, LibsqlStorage, Storage};

/// Children each process attempts under the shared parent.
const COUNT: u32 = 15;
/// The shared parent id both processes mint children under.
const PARENT: &str = "ub-parent";

/// The outcome of one two-process scenario: the set of DISTINCT child ids that persisted across BOTH
/// processes, and the total number of `IdCollision`s the two processes reported.
struct Outcome {
    distinct: u32,
    collisions: u32,
}

/// Pre-migrate a fresh temp db and seed the shared parent (a SEPARATELY committed tx before either
/// child process opens the file), then run TWO `write_lock_race` processes concurrently in `mode`.
async fn run_scenario(mode: &str) -> Outcome {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("unblock.db");

    // Pre-migrate + seed the parent via the PUBLIC open path; drop the store so its connections close
    // before the child processes open the same file.
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

    let bin = env!("CARGO_BIN_EXE_write_lock_race");
    let spawn = |actor: &str| {
        Command::new(bin)
            .arg(&db_path)
            .arg(PARENT)
            .arg(mode)
            .arg(COUNT.to_string())
            .arg(actor)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn write_lock_race")
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

/// AC-1 headline: `.write.lock` active → distinct ids / no `IdCollision`; the no-lock control
/// REPRODUCES the collision (non-vacuity).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cross_process_write_lock_prevents_id_collision() {
    // With the lock: every one of the 2 * COUNT children gets a distinct `parent.N`, no collision.
    let locked = run_scenario("locked").await;
    assert_eq!(
        locked.collisions, 0,
        "with .write.lock active, two processes must not collide on parent.N"
    );
    assert_eq!(
        locked.distinct,
        2 * COUNT,
        "with the lock, all {} children get distinct ids",
        2 * COUNT
    );

    // Non-vacuity: the SAME race WITHOUT the lock reproduces the IdCollision — fewer than 2 * COUNT
    // distinct children persist AND at least one process reported an IdCollision.
    let control = run_scenario("nolock").await;
    assert!(
        control.collisions > 0,
        "the no-lock control must REPRODUCE the IdCollision (got 0 — the test would be vacuous)"
    );
    assert!(
        control.distinct < 2 * COUNT,
        "the no-lock control must lose children to collisions (distinct {} should be < {})",
        control.distinct,
        2 * COUNT
    );
}
