//! PRD §4 D29 clause F6 (`ub-kpe`, v1.0.3): the cli's own doctor open order keeps the session's
//! `SQLite` SHARED lock.
//!
//! The HOLDER is THIS test binary re-executed at `kpe_cli_holder_entry` (no `[[bin]]`: `dist` would
//! ship it). It opens through the PRODUCTION `unblock_cli::open_session_for_doctor` (resolve →
//! `DoctorPreflight::capture` → `open_resolved_with_storage` → `Session::open`), reads once, and runs
//! `session.doctor(preflight)` on command, so a reordering mutant in production goes red here (the
//! engine holder can only copy the order). The parent asserts SHARED kept (`F_GETLK`, plus
//! `/proc/locks` on Linux) and a fresh observer BUSY. The lock instrument is the engine's, included by
//! `#[path]`; every spawn goes through `common::spawn_locked` / `common::output_locked`
//! (`tests/spawn_lock_gate.rs`).
#![cfg(unix)]

mod common;

#[path = "../../unblock-engine/tests/common/lock_observer.rs"]
#[allow(dead_code)]
// the cli cell uses a subset of the engine's lock instrument (engine tests/common/mod.rs:5 precedent)
mod lock_observer;
#[path = "../../unblock-engine/tests/common/posix_locks.rs"]
#[allow(dead_code)]
// the cli cell uses a subset of the engine's lock instrument (engine tests/common/mod.rs:5 precedent)
mod posix_locks;

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::Path;
use std::process::{ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use common::{Workspace, spawn_locked};
use lock_observer::Observed;
use posix_locks::LockKind;
use unblock_config::CliOverrides;

/// This binary's observer entry.
const OBSERVER_ENTRY: &str = "kpe_observer_entry";
/// This binary's holder entry.
const HOLDER_ENTRY: &str = "kpe_cli_holder_entry";
/// The env var that turns `kpe_cli_holder_entry` into the holder.
const HOLD_ENV: &str = "UBKPE_HOLD_DIR";
/// The token every holder reply carries (searched anywhere in a line: libtest may prefix it).
const REPLY_TOKEN: &str = "UBKPE-REPLY ";
/// How long one holder reply may take.
const REPLY_TIMEOUT: Duration = Duration::from_mins(1);

const MUTANT: &str = "the cli doctor open order let a descriptor on unblock.db close inside the session (D29 clause F6)";

/// The observer process body (see the engine's `common::lock_observer`).
#[test]
#[ignore = "observer process entry point; spawned by the kpe cells"]
fn kpe_observer_entry() {
    lock_observer::observer_main();
}

fn holder_reply(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = write!(out, "\n{REPLY_TOKEN}{line}\n");
    let _ = out.flush();
}

/// The holder process body: a no-op unless `UBKPE_HOLD_DIR` is set.
#[test]
#[ignore = "holder process entry point; spawned by kpe5"]
fn kpe_cli_holder_entry() {
    let Some(dir) = std::env::var_os(HOLD_ENV) else {
        return;
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("holder runtime");
    runtime.block_on(async move {
        let (session, preflight, _fmt) =
            unblock_cli::open_session_for_doctor(&CliOverrides::new().with_dir(dir))
                .await
                .expect("open_session_for_doctor");
        session.integrity_check().await.expect("one read");
        holder_reply("READY");
        let mut preflight = Some(preflight);
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            let answer = match line.trim() {
                "NOOP" => "OK".to_owned(),
                "DOCTOR" => match preflight.take() {
                    None => "DOCTOR-SPENT".to_owned(),
                    Some(preflight) => match session.doctor(preflight).await {
                        Ok(report) => format!(
                            "DOCTORED {}",
                            report
                                .findings
                                .iter()
                                .map(|f| f.label.as_str())
                                .collect::<Vec<_>>()
                                .join(",")
                        ),
                        Err(err) => format!("DOCTOR-FAILED {err}"),
                    },
                },
                "READ" => match session.integrity_check().await {
                    Ok(rows) => format!("READ {}", rows.len()),
                    Err(err) => format!("READ-FAILED {err}"),
                },
                "QUIT" => break,
                other => format!("UNKNOWN {other}"),
            };
            holder_reply(&answer);
        }
        holder_reply("BYE");
        drop(session);
    });
}

/// The re-executed holder, seen from the parent.
struct Holder {
    child: std::process::Child,
    stdin: Option<ChildStdin>,
    replies: Receiver<String>,
}

impl Holder {
    fn spawn(unblock_dir: &Path) -> Self {
        let exe = std::env::current_exe().expect("current_exe");
        let mut cmd = Command::new(exe);
        cmd.args([
            HOLDER_ENTRY,
            "--exact",
            "--include-ignored",
            "--nocapture",
            "--test-threads=1",
            "-q",
        ])
        .env(HOLD_ENV, unblock_dir)
        .env_remove("CLAUDE_PROJECT_DIR")
        .env_remove("UNBLOCK_DIR")
        .env_remove("UNBLOCK_ACTOR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
        let mut child = spawn_locked(&mut cmd, "spawn the cli holder");
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("holder stdout");
        let (tx, replies) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                // Skip libtest's own lines (`running 1 test`, blanks, `.`, `test result:`).
                if let Some((_, reply)) = line.split_once(REPLY_TOKEN)
                    && tx.send(reply.trim().to_owned()).is_err()
                {
                    break;
                }
            }
        });
        let mut holder = Self {
            child,
            stdin,
            replies,
        };
        let ready = holder.read_reply();
        assert_eq!(ready, "READY");
        holder
    }

    fn read_reply(&mut self) -> String {
        match self.replies.recv_timeout(REPLY_TIMEOUT) {
            Ok(reply) => reply,
            Err(mpsc::RecvTimeoutError::Disconnected) => panic!(
                "the holder closed stdout before replying: the libtest filter matched nothing or \
                 the holder panicked (see its stderr above)"
            ),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("no holder reply within {REPLY_TIMEOUT:?}")
            }
        }
    }

    fn send(&mut self, line: &str) -> String {
        let stdin = self.stdin.as_mut().expect("holder stdin open");
        writeln!(stdin, "{line}").expect("write to the holder");
        stdin.flush().expect("flush the holder's stdin");
        self.read_reply()
    }

    fn pid(&self) -> i32 {
        i32::try_from(self.child.id()).expect("a pid fits i32")
    }

    fn quit(mut self) {
        assert_eq!(self.send("QUIT"), "BYE");
        drop(self.stdin.take());
        let status = self.child.wait().expect("reap the holder");
        assert!(status.success(), "the cli holder exits 0");
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Assert SHARED held by the holder AND a fresh observer BUSY, printing the evidence line.
fn assert_shared_and_busy(point: &str, db: &Path, holder: &Holder) {
    let pid = holder.pid();
    let observation = posix_locks::shared_range(db, pid);
    let (stdout, stderr) = common::observe(OBSERVER_ENTRY, db);
    let observed = Observed::parse(&stdout, &stderr);
    eprintln!(
        "{}",
        posix_locks::measure_line("kpe5", point, pid, &observation, Some(&observed.line), None)
    );
    assert_eq!(
        observation.kind(),
        Some(LockKind::Read),
        "kpe5 at {point}: SHARED lost — {MUTANT}"
    );
    assert!(
        observed.is_busy(),
        "kpe5 at {point}: the observer must be BUSY, got {:?} (stderr {}) — {MUTANT}",
        observed.line,
        observed.stderr
    );
}

/// (kpe5) The production cli open order (`open_session_for_doctor`) keeps SHARED through `doctor`.
#[test]
fn kpe5_cli_doctor_open_order_keeps_the_shared_lock() {
    let ws = Workspace::init();
    let db = ws.db_path();
    let mut holder = Holder::spawn(&ws.unblock_dir());
    assert_shared_and_busy("baseline", &db, &holder);
    assert_eq!(
        holder.send("DOCTOR"),
        "DOCTORED health,integrity,schema_version,schema_expected"
    );
    assert_shared_and_busy("after-doctor", &db, &holder);
    assert!(holder.send("READ").starts_with("READ "));
    assert_shared_and_busy("after-read", &db, &holder);
    holder.quit();
}
