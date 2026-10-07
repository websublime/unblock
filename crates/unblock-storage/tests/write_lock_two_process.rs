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
//!
//! Neither half depends on the two processes overlapping in time (ub-fh5). Each process's first
//! mutation passes a stdio gate (see `tests/bin/write_lock_race.rs`), and the test releases nobody
//! from the `read → insert` window until both processes have reported. Without the lock both get in,
//! read the same `N` and collide. With it, one gets in, and the other's single `try_lock` is refused
//! because the first holds the lock until it is released from the window.

#![cfg(unix)]

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;

use chrono::Utc;
use unblock_model::Issue;
use unblock_storage::{DEFAULT_WRITE_LOCK_TIMEOUT_MS, LibsqlStorage, Storage};

/// Children each process attempts under the shared parent.
const COUNT: u32 = 15;
/// The shared parent id both processes mint children under.
const PARENT: &str = "ub-parent";
/// The two racing processes, by actor.
const ACTORS: [&str; 2] = ["proc-1", "proc-2"];

/// The outcome of one two-process scenario: the two gate reports, the set of DISTINCT child ids that
/// persisted across BOTH processes, and the total number of `IdCollision`s the two processes reported.
struct Outcome {
    /// Sorted gate reports. `["entered", "entered"]` means both processes got into the window
    /// together, and `["entered", "refused"]` means the lock kept one of them out.
    gates: Vec<String>,
    distinct: u32,
    collisions: u32,
}

/// Pre-migrate a fresh temp db and seed the shared parent (a SEPARATELY committed tx before either
/// child process opens the file), then run TWO `write_lock_race` processes concurrently in `mode`,
/// holding both at the first-mutation gate until both have reported.
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
    let mut children: Vec<Child> = ACTORS
        .iter()
        .map(|actor| {
            Command::new(bin)
                .arg(&db_path)
                .arg(PARENT)
                .arg(mode)
                .arg(COUNT.to_string())
                .arg(actor)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn write_lock_race")
        })
        .collect();

    let (gate_tx, gate_rx) = mpsc::channel();
    let readers: Vec<_> = children
        .iter_mut()
        .enumerate()
        .map(|(idx, child)| read_stdout(idx, child, gate_tx.clone()))
        .collect();
    // Only the readers may send: if one dies without reporting, `recv` fails instead of hanging.
    drop(gate_tx);

    // Release nobody until BOTH processes have reported, then send GO to every process parked inside
    // the window. Closing stdin afterwards also unblocks a process stuck on a broken protocol.
    let mut gates: Vec<Option<String>> = vec![None; children.len()];
    for _ in 0..children.len() {
        let (idx, gate) = gate_rx.recv().expect("each reader reports once");
        let gate = gate.unwrap_or_else(|| {
            panic!(
                "{} exited before its gate (stderr: {})",
                ACTORS[idx],
                stderr_of(&mut children[idx])
            )
        });
        gates[idx] = Some(gate);
    }
    for (child, gate) in children.iter_mut().zip(&gates) {
        if gate.as_deref() == Some("entered") {
            let stdin = child.stdin.as_mut().expect("piped stdin");
            writeln!(stdin, "GO").expect("release the gate");
        }
    }
    for child in &mut children {
        drop(child.stdin.take());
    }

    for (actor, child) in ACTORS.iter().zip(&mut children) {
        let status = child.wait().expect("wait write_lock_race");
        assert!(
            status.success(),
            "{actor} exited {status} (stderr: {})",
            stderr_of(child)
        );
    }

    let mut distinct: HashSet<String> = HashSet::new();
    let mut collisions = 0u32;
    for reader in readers {
        for line in reader.join().expect("stdout reader") {
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

    let mut gates: Vec<String> = gates.into_iter().flatten().collect();
    gates.sort_unstable();
    Outcome {
        gates,
        distinct: u32::try_from(distinct.len()).expect("distinct count fits u32"),
        collisions,
    }
}

/// Spawn the stdout reader for one child. It forwards the child's gate report the moment it arrives
/// (or `None` if stdout closes first) and returns every stdout line once the child exits.
fn read_stdout(
    idx: usize,
    child: &mut Child,
    gate_tx: mpsc::Sender<(usize, Option<String>)>,
) -> thread::JoinHandle<Vec<String>> {
    let stdout = child.stdout.take().expect("piped stdout");
    thread::spawn(move || {
        let mut lines = Vec::new();
        let mut reported = false;
        for line in BufReader::new(stdout).lines() {
            let line = line.expect("read child stdout");
            if let Some(gate) = line.strip_prefix("GATE=") {
                // The receiver is only gone if the test has already failed.
                let _ = gate_tx.send((idx, Some(gate.to_string())));
                reported = true;
            }
            lines.push(line);
        }
        if !reported {
            let _ = gate_tx.send((idx, None));
        }
        lines
    })
}

/// Everything the child wrote to stderr (read once it has exited).
fn stderr_of(child: &mut Child) -> String {
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    stderr
}

/// AC-1 headline: `.write.lock` active → the lock keeps a second process out of the window, ids are
/// distinct, no `IdCollision`; the no-lock control lets both in and REPRODUCES the collision
/// (non-vacuity).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cross_process_write_lock_prevents_id_collision() {
    // With the lock, the other process's try_lock is refused while one is parked inside the
    // read → insert window, and every one of the 2 * COUNT children gets a distinct `parent.N`.
    let locked = run_scenario("locked").await;
    assert_eq!(
        locked.gates,
        ["entered", "refused"],
        "with .write.lock active, a second process must be kept out of the read → insert window"
    );
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

    // The no-lock control proves non-vacuity. The SAME gate WITHOUT the lock lets both processes in,
    // and both read the same N before either inserts. So every run reproduces the IdCollision, with
    // fewer than 2 * COUNT distinct children persisted AND at least one IdCollision reported.
    let control = run_scenario("nolock").await;
    assert_eq!(
        control.gates,
        ["entered", "entered"],
        "without the lock, both processes must get into the read → insert window"
    );
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
