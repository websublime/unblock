//! The parent side of the PRD §4 D5 amendment and D58 amendment (`ub-c15`) lock-retention cells: a
//! migrated [`Workspace`], the [`Holder`] process (`session_lock_holder`), the fresh observer
//! ([`observe`]) and the `UB2VP-MEASURE` evidence lines.
//!
//! **`SPAWN_LOCK` (the ub-vcp pattern).** On macOS std creates a pipe and only then marks it
//! close-on-exec, so a child spawned by another test thread in that window inherits the pipe's ends:
//! a long-lived holder could keep a short-lived observer's stdout open. Every spawn here holds the one
//! process-wide `SPAWN_LOCK` across `Command::spawn` ONLY, never across a wait or a read.
//!
//! Every in-process open uses the explicit `with_dir` tier, so a `CLAUDE_PROJECT_DIR` in a developer's
//! environment (D39) never binds the developer's real database. Sidecar paths come from the owner,
//! `unblock_sync::sidecar`.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tempfile::TempDir;
use unblock_config::{CliOverrides, open_with_storage_with_cli};

use super::lock_observer::{self, Observed};
use super::posix_locks::{self, LockKind, Observation};

/// The holder `[[bin]]`.
pub const HOLDER: &str = env!("CARGO_BIN_EXE_session_lock_holder");

/// How long one holder reply may take before the cell fails.
const REPLY_TIMEOUT: Duration = Duration::from_mins(1);

/// The one process-wide spawn lock (held across `spawn` only).
static SPAWN_LOCK: Mutex<()> = Mutex::new(());

/// Spawn `cmd` while holding [`SPAWN_LOCK`] (poison-tolerant).
///
/// # Panics
///
/// When the spawn fails.
pub fn spawn_locked(cmd: &mut Command, what: &str) -> Child {
    let _guard = SPAWN_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    cmd.spawn()
        .unwrap_or_else(|error| panic!("{what}: {error}"))
}

/// A migrated workspace under a tempdir: `<root>/.unblock/` with a closed, migrated database.
pub struct Workspace {
    /// The owning tempdir.
    pub root: TempDir,
    /// The canonical `.unblock/` directory.
    pub unblock: PathBuf,
    /// The database path the holder opens.
    pub db: PathBuf,
    /// The explicit `--db` the holder is given, if any.
    pub db_override: Option<PathBuf>,
}

impl Workspace {
    /// A migrated workspace with the default database name.
    pub async fn migrated() -> Self {
        Self::migrated_with(None, None).await
    }

    /// A migrated workspace with an optional `config.toml` body and an optional database file NAME
    /// passed as `--db <unblock>/<name>`. The migrate connection is closed and proven closed: the
    /// `-wal` and `-shm` files are gone, so the parent holds no connection.
    pub async fn migrated_with(config: Option<&str>, db_name: Option<&str>) -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        let unblock = root.path().join(".unblock");
        std::fs::create_dir(&unblock).expect("mkdir .unblock");
        let unblock = std::fs::canonicalize(&unblock).expect("canonicalize .unblock");
        if let Some(config) = config {
            std::fs::write(unblock.join("config.toml"), config).expect("write config.toml");
        }
        let db_override = db_name.map(|name| unblock.join(name));
        let overrides = Self::overrides_for(&unblock, db_override.as_deref());
        let ctx = open_with_storage_with_cli(&overrides)
            .await
            .expect("open + migrate the workspace");
        let db = ctx.paths.db_path.clone();
        drop(ctx);
        let ws = Self {
            root,
            unblock,
            db,
            db_override,
        };
        assert!(
            !ws.sidecar(unblock_sync::WAL_SUFFIX).exists()
                && !ws.sidecar(unblock_sync::SHM_SUFFIX).exists(),
            "the parent's migrate connection must be closed (its -wal/-shm gone)"
        );
        ws
    }

    /// The overrides the holder and in-process opens use: `--dir`, plus `--db` when given.
    fn overrides_for(unblock: &Path, db: Option<&Path>) -> CliOverrides {
        let overrides = CliOverrides::new().with_dir(unblock);
        match db {
            Some(db) => overrides.with_db(db),
            None => overrides,
        }
    }

    /// The in-process overrides of this workspace.
    #[must_use]
    pub fn overrides(&self) -> CliOverrides {
        Self::overrides_for(&self.unblock, self.db_override.as_deref())
    }

    /// `<db><suffix>`, through the owner `unblock_sync::sidecar`.
    #[must_use]
    pub fn sidecar(&self, suffix: &str) -> PathBuf {
        unblock_sync::sidecar(&self.db, suffix)
    }
}

/// A running `session_lock_holder`. Dropping it kills and reaps the process.
pub struct Holder {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    stderr: Arc<Mutex<String>>,
}

impl Holder {
    /// Spawn the holder on `ws` in `mode` (`sync`, `doctor` or `recover`) and wait for `READY`.
    ///
    /// # Panics
    ///
    /// When the holder does not reply `READY`.
    #[must_use]
    pub fn spawn(ws: &Workspace, mode: &str) -> Self {
        let mut cmd = Command::new(HOLDER);
        cmd.arg(&ws.unblock);
        if let Some(db) = &ws.db_override {
            cmd.arg("--db").arg(db);
        }
        cmd.arg("--mode")
            .arg(mode)
            .env_remove("CLAUDE_PROJECT_DIR")
            .env_remove("UNBLOCK_DIR")
            .env_remove("UNBLOCK_ACTOR")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = spawn_locked(&mut cmd, "spawn session_lock_holder");
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("holder stdout");
        let mut stderr_pipe = child.stderr.take().expect("holder stderr");
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(String::new()));
        let sink = Arc::clone(&stderr);
        std::thread::spawn(move || {
            let mut buf = [0_u8; 4096];
            while let Ok(n) = stderr_pipe.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let chunk = String::from_utf8_lossy(buf.get(..n).unwrap_or_default());
                sink.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push_str(&chunk);
            }
        });
        let mut holder = Self {
            child,
            stdin,
            lines,
            stderr,
        };
        let ready = holder.read_line();
        assert_eq!(
            ready,
            "READY",
            "the holder never got ready: {}",
            holder.stderr()
        );
        holder
    }

    /// The holder's pid.
    #[must_use]
    pub fn pid(&self) -> i32 {
        i32::try_from(self.child.id()).expect("a pid fits i32")
    }

    /// The holder's stderr so far.
    #[must_use]
    pub fn stderr(&self) -> String {
        self.stderr
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// One reply line, bounded by a 60 s timeout.
    fn read_line(&mut self) -> String {
        self.lines
            .recv_timeout(REPLY_TIMEOUT)
            .unwrap_or_else(|error| {
                panic!(
                    "no holder reply within {REPLY_TIMEOUT:?} ({error}); holder stderr: {}",
                    self.stderr()
                )
            })
    }

    /// Send one command line and read exactly one reply.
    pub fn send(&mut self, line: &str) -> String {
        let stdin = self.stdin.as_mut().expect("the holder's stdin is open");
        writeln!(stdin, "{line}").expect("write to the holder");
        stdin.flush().expect("flush the holder's stdin");
        self.read_line()
    }

    /// Send `QUIT`, expect `BYE`, and reap the holder.
    pub fn quit(mut self) {
        let bye = self.send("QUIT");
        assert_eq!(bye, "BYE");
        drop(self.stdin.take());
        let status = self.child.wait().expect("reap the holder");
        assert!(status.success(), "the holder exits 0: {}", self.stderr());
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Run a fresh observer on `db`: this test binary re-executed at `entry` with the observer env set.
///
/// # Panics
///
/// When the observer cannot run or exits non-zero.
#[must_use]
pub fn observe(entry: &str, db: &Path) -> Observed {
    let exe = std::env::current_exe().expect("current_exe");
    let mut cmd = Command::new(exe);
    cmd.args(lock_observer::entry_args(entry))
        .env(lock_observer::OBSERVE_ENV, db)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = spawn_locked(&mut cmd, "spawn the observer");
    let output = wait_bounded(child, "the observer");
    let observed = Observed::parse(
        &String::from_utf8_lossy(&output.stdout),
        &String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        output.status.success(),
        "the observer exits 0 (line {:?}, stderr {})",
        observed.line,
        observed.stderr
    );
    observed
}

/// Wait for a short-lived child for at most [`REPLY_TIMEOUT`]; on a timeout kill it and fail the
/// cell (its output is small, so the pipes cannot fill while it runs).
///
/// # Panics
///
/// When the child does not exit in time or cannot be reaped.
pub fn wait_bounded(mut child: Child, what: &str) -> std::process::Output {
    let deadline = std::time::Instant::now() + REPLY_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let output = child.wait_with_output().expect("reap the timed-out child");
                panic!(
                    "{what} did not exit within {REPLY_TIMEOUT:?} (stdout {:?}, stderr {:?})",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            Err(error) => panic!("{what}: try_wait failed: {error}"),
        }
    }
    child
        .wait_with_output()
        .expect("collect the child's output")
}

/// Print the `UB2VP-MEASURE` line of one observation and return it.
pub fn measure(
    cell: &str,
    point: &str,
    holder_pid: i32,
    observation: Observation,
    observer: Option<&Observed>,
) -> Observation {
    eprintln!(
        "{}",
        posix_locks::measure_line(
            cell,
            point,
            holder_pid,
            &observation,
            observer.map(|observed| observed.line.as_str()),
            None,
        )
    );
    observation
}

/// Assert the holder still holds SHARED on `db` (`F_GETLK`, and `/proc/locks` on Linux) AND a fresh
/// observer is BUSY; `why` names the mutant a failure means.
pub fn assert_shared_and_busy(
    entry: &str,
    cell: &str,
    point: &str,
    db: &Path,
    holder: &Holder,
    why: &str,
) {
    let pid = holder.pid();
    let observation = posix_locks::shared_range(db, pid);
    let observed = observe(entry, db);
    let observation = measure(cell, point, pid, observation, Some(&observed));
    assert_eq!(
        observation.kind(),
        Some(LockKind::Read),
        "{cell} at {point}: the holder lost SQLite's SHARED lock — {why}"
    );
    assert!(
        observed.is_busy(),
        "{cell} at {point}: a fresh observer must be BUSY, got {:?} (stderr {}) — {why}",
        observed.line,
        observed.stderr
    );
}

/// Assert the holder still holds its read lock on the `-shm` DMS byte; `why` names the mutant.
pub fn assert_shm_dms_held(cell: &str, point: &str, shm: &Path, holder: &Holder, why: &str) {
    let kind = posix_locks::shm_dms(shm, holder.pid());
    eprintln!(
        "UB2VP-MEASURE leg={} cell={cell} point={point} holder_pid={} shm_dms={kind:?}",
        posix_locks::leg(),
        holder.pid()
    );
    assert_eq!(
        kind,
        Some(LockKind::Read),
        "{cell} at {point}: the holder lost its -shm DMS lock — {why}"
    );
}
