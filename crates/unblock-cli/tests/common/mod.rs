//! Shared harness for the `unblock-cli` integration suites (D27/T3.1, promoted for T3.2).
//!
//! Every case runs against an **isolated** temp workspace (its own `.unblock/`) so the suites are
//! hermetic and parallel-safe: no case reads the repo's own `.unblock/` (walk-up discovery is pinned
//! by passing an explicit `--dir`), and no case mutates process-global env (`std::env::set_var` is
//! `unsafe` under edition 2024 and forbidden — per-child `Command::env` is used instead).
//!
//! The `unblock` binary is located via `assert_cmd`'s cargo integration (`cargo_bin`), so the suites
//! drive the SAME artifact the shipped build produces.
//!
//! T3.2 promotes the `mcp` stdio harness (`McpClient`/`send_signal`/`wait_for`, previously private
//! to `mcp_lifecycle.rs`) here so the shutdown-reliability failure-injection suite
//! (`shutdown_failure_injection.rs`) can reuse it without duplicating the JSON-RPC framing code, and
//! adds the shared shutdown-case oracle (`reopen_and_check`) + the pinned bulk-markdown fixture
//! (`bulk_markdown`).

#![allow(dead_code)] // each test binary uses a subset of the harness.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use assert_cmd::cargo::CommandCargoExt as _;
use serde_json::{Value, json};
use tempfile::TempDir;
use unblock_config::{CliOverrides, open_with_storage_with_cli};
use unblock_engine::{Session, SessionConfig};
use unblock_model::ListFilters;

/// A freshly-scaffolded, isolated workspace: a tempdir whose `.unblock/` holds a migrated empty
/// `unblock.db` + a `config.toml`. The `TempDir` is retained so it outlives the case.
pub struct Workspace {
    /// The owning tempdir (the project root; `.unblock/` sits directly under it).
    pub root: TempDir,
}

impl Workspace {
    /// Scaffold a fresh workspace by running the real `unblock init` (FR-9 no-drift — the same code
    /// path `mcp`/`migrate`/`doctor` open). Panics on failure (a harness precondition, not the SUT).
    #[must_use]
    pub fn init() -> Self {
        Self::init_with_prefix(None)
    }

    /// Like [`Workspace::init`] but seeds an explicit `--prefix`.
    #[must_use]
    pub fn init_with_prefix(prefix: Option<&str>) -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        let mut cmd = unblock();
        cmd.current_dir(root.path()).arg("init");
        if let Some(prefix) = prefix {
            cmd.args(["--prefix", prefix]);
        }
        let out = cmd.output().expect("run init");
        assert!(
            out.status.success(),
            "init must succeed; stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        Self { root }
    }

    /// The project root (contains `.unblock/`).
    #[must_use]
    pub fn root(&self) -> &Path {
        self.root.path()
    }

    /// The `.unblock/` directory.
    #[must_use]
    pub fn unblock_dir(&self) -> PathBuf {
        self.root.path().join(".unblock")
    }

    /// The workspace database path.
    #[must_use]
    pub fn db_path(&self) -> PathBuf {
        self.unblock_dir().join("unblock.db")
    }

    /// The scaffolded `config.toml` path.
    #[must_use]
    pub fn config_path(&self) -> PathBuf {
        self.unblock_dir().join("config.toml")
    }

    /// A `Command` for the `unblock` binary with `current_dir` set to this workspace root (so
    /// walk-up discovery finds this `.unblock/`, not the repo's).
    #[must_use]
    pub fn cmd(&self) -> Command {
        let mut cmd = unblock();
        cmd.current_dir(self.root.path());
        cmd
    }
}

/// A bare `Command` for the `unblock` binary (no cwd set). Callers set `current_dir`/`--dir`.
///
/// **Every** child spawned by this suite funnels through here, so discovery-affecting env is scrubbed
/// at this ONE root: `CLAUDE_PROJECT_DIR` and `UNBLOCK_DIR` are host discovery inputs (D39/D10), and
/// `UNBLOCK_ACTOR` seeds the default actor (FORK-4). Left inherited, a dev running the suite inside a
/// dogfooded repo (a real `<repo>/.unblock` on disk) or under Claude Code (which injects
/// `CLAUDE_PROJECT_DIR`) would have workspace discovery bind ProjectDir/ExplicitDir instead of the
/// per-case tempdir — flipping the D39 startup-line tier assertion (and any actor-derived field) RED
/// purely from the host shell. Scrubbing them keeps every spawn hermetic (module doc invariant); a
/// case that WANTS one of these sets it back with `Command::env` (which wins over this removal).
///
/// `UNBLOCK_OUTPUT_FORMAT` (FR-13/D48) is scrubbed for the same reason and one worse one: it selects
/// the error-render FORMAT for every spawn, so a host shell exporting `plain` would make every D48
/// frame-only assertion pass VACUOUSLY (the human arm writes nothing to stdout in any case) while
/// turning the shipped `mcp_lifecycle.rs` payload assertion RED for a reason having nothing to do
/// with the code under test.
#[must_use]
pub fn unblock() -> Command {
    let mut cmd = Command::cargo_bin("unblock").expect("locate the `unblock` binary");
    cmd.env_remove("CLAUDE_PROJECT_DIR");
    cmd.env_remove("UNBLOCK_DIR");
    cmd.env_remove("UNBLOCK_ACTOR");
    // FR-13/D48: see the paragraph above — inherited, it makes the frame-only cells vacuous.
    cmd.env_remove("UNBLOCK_OUTPUT_FORMAT");
    cmd
}

/// The D48 oracle: the ONE stderr line that IS the `StructuredError` payload, parsed.
///
/// `mcp` stderr legitimately carries non-payload lines — the D39 startup binding line
/// (`commands/mcp.rs`) and `tracing` records on a `-vv` child — so the payload is located by SHAPE
/// (`code` + `retryable`, the pair this suite already uses to spell "a `StructuredError` blob"), never
/// by position. A whole-buffer `serde_json::from_str(stderr.trim())` would be red from the D39 line
/// alone. The render is COMPACT single-line JSON (`RenderOptions::default().pretty_json == false`),
/// so one line is the whole payload — pinned at the source by `exit.rs`'s U9 cell, which asserts the
/// payload is ONE line plus ONE terminator.
///
/// **Both halves of this claim have their OWN cells, because this function is the POSITIVE half of
/// every D48 cell in two files and no CALLER can notice it weakening — each runs against real
/// product output, which always carries both members:** the
/// SHAPE-not-position half is `mcp_stdout_channel.rs`'s H8 (a buffer whose first parseable JSON line
/// is a log record), and the two-member shape test is H7 (a degraded document carrying one member).
/// Weakened here, the oracle would start accepting a degraded payload with every caller still green.
#[must_use]
pub fn structured_error_on_stderr(stderr: &str) -> Option<Value> {
    stderr
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .find(|v| v.get("code").is_some() && v.get("retryable").is_some())
}

/// Assert every non-empty stdout line is JSON-RPC framing (NFR-14 / D48): valid JSON carrying a
/// `jsonrpc` member and NO `StructuredError` shape at the top level.
///
/// **This assertion is VACUOUSLY TRUE on empty stdout, which is the normal state for every
/// pre-run-loop failure.** It is therefore never sufficient alone: every cell calling it MUST also
/// assert the POSITIVE stderr landing via [`structured_error_on_stderr`], or the cell stays green
/// under a mutation that deletes the diagnostic outright.
///
/// **And for that same reason this function is DRIVEN DIRECTLY by a self-test**
/// (`mcp_stdout_channel.rs`): every production call site asserts stdout is EMPTY immediately
/// afterwards, so the loop below never sees a byte and replacing the whole body with `{}` would
/// leave the matrix green. A guard nobody drives is a guard that can be deleted.
///
/// The shape assertion below is a CONJUNCTION, and each half is driven by its own single-member
/// input in that self-test (H3): a line carrying BOTH `code` and `retryable` is rejected by either
/// half alone, so it can never show that both are still there.
pub fn assert_stdout_is_frame_only(stdout: &str, cell: &str) {
    for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
        let parsed: Value = serde_json::from_str(line.trim())
            .unwrap_or_else(|e| panic!("{cell}: stdout line is not JSON: `{line}`: {e}"));
        assert!(
            parsed.get("jsonrpc").is_some(),
            "{cell}: not JSON-RPC framing: {line}"
        );
        assert!(
            parsed.get("code").is_none() && parsed.get("retryable").is_none(),
            "{cell}: a StructuredError blob reached the JSON-RPC framing channel: {line}"
        );
    }
}

/// Is `value` a JSON-RPC frame (a `jsonrpc` member) rather than a `StructuredError` blob
/// (`code` + `retryable`)? The D48 hardening of the [`McpClient::read_response`] guard.
///
/// **All three membership tests are TOP-LEVEL, on the object's own keys — [`Value::get`], never a
/// recursive search.** A recursive reading would reject the shipped D47 `-32600` frame, whose `code`
/// member is nested under `error`; that frame is legitimate framing and this predicate must accept
/// it. That claim is pinned for ALL THREE tests, not only for `code`, by `mcp_stdout_channel.rs`'s
/// H6; and each of the two blob-shape clauses is pinned as INDEPENDENTLY load-bearing by H5, whose
/// inputs carry one blob member each — H1's inputs cannot discriminate them, so dropping either
/// clause was green until H5 existed.
#[must_use]
pub fn is_jsonrpc_framing(value: &Value) -> bool {
    value.get("jsonrpc").is_some()
        && value.get("code").is_none()
        && value.get("retryable").is_none()
}

/// A tiny current-thread runtime for the raw-libsql fixtures (the harness itself is sync).
#[must_use]
pub fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build a current-thread runtime")
}

/// Stamp `PRAGMA user_version = <version>` on the workspace DB via a raw libsql open (the same
/// bundled `SQLite` the backend uses). This makes the on-disk schema look NEWER than this build so
/// the next migrate rejects it with `SchemaMismatch` (D27/AF-2). The connection is dropped before
/// the CLI child opens the file, so there is no writer contention.
pub fn stamp_user_version(db: &Path, version: i64) {
    runtime().block_on(async {
        let database = libsql::Builder::new_local(db)
            .build()
            .await
            .expect("open the workspace db");
        let conn = database.connect().expect("connect");
        conn.execute(&format!("PRAGMA user_version = {version}"), ())
            .await
            .expect("stamp user_version");
    });
}

/// Corrupt the `SQLite` SCHEMA region (from byte 100 — after the file header, over `sqlite_master`)
/// so the very first `open_local`/`migrate()` read fails as `database disk image is malformed` →
/// `DATABASE_ERROR`, exit 2. Deterministic: fixed bytes at a fixed offset.
///
/// **NOT the same recipe as `migrate_doctor.rs`'s `corrupt_db`**, which starts at 4096 to keep page
/// 1 intact: that one is INVISIBLE to the `mcp` open path (measured exit 0, clean EOF, empty
/// stdout), so a cell built on it would pass today, pass after the fix, and pass under every
/// mutation.
pub fn corrupt_db_schema_page(db: &Path) {
    use std::io::{Seek, SeekFrom};

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(db)
        .expect("open the workspace db for corruption");
    file.seek(SeekFrom::Start(100))
        .expect("seek past the SQLite file header");
    file.write_all(&[0xAD; 16 * 1024])
        .expect("scribble over the schema region");
    file.flush().expect("flush the corruption");
}

/// A `Command` for the `unblock` binary anchored at `dir` (its cwd) — used when the case wants a
/// specific cwd but does NOT pre-scaffold a workspace (e.g. the no-workspace error paths).
#[must_use]
pub fn unblock_in(dir: &Path) -> Command {
    let mut cmd = unblock();
    cmd.current_dir(dir);
    cmd
}

// ----------------------------------------------------------------------------------------------
// `mcp` stdio harness (D27/AD-4, T3.1 — promoted here at T3.2 so `shutdown_failure_injection.rs`
// can reuse it without duplicating the JSON-RPC framing code).
// ----------------------------------------------------------------------------------------------

/// A hand-rolled newline-delimited JSON-RPC client over a spawned `unblock mcp` child.
pub struct McpClient {
    pub child: Child,
    /// `Some` while the pipe is open; `.take()` + drop closes it → the child reads EOF.
    stdin: Option<ChildStdin>,
    /// `Some` until [`write_without_reading`](Self::write_without_reading) moves it into a background
    /// drain thread (C2/C6 — a write-without-read case never reads a response on this client again).
    stdout: Option<BufReader<ChildStdout>>,
    next_id: i64,
    /// Every non-empty stdout LINE the server produced (each must be valid JSON — NFR-14 guard).
    /// Only populated while `stdout` is still owned by this client (i.e. `read_response` ran).
    pub seen_lines: Vec<String>,
    /// The child's STDERR, drained continuously by a background thread (T3.2.1/D38 — see
    /// [`stderr_snapshot`](Self::stderr_snapshot)).
    stderr: Arc<Mutex<String>>,
    /// The child's STDOUT once [`capture_stdout`](McpClient::capture_stdout) has moved it into a
    /// background capture thread (empty until then — the protocol cases own stdout themselves).
    stdout_capture: Arc<Mutex<String>>,
    /// The two RETAINED drain handles (D48). They were previously dropped on the spot, and
    /// [`wait_for`](Self::wait_for) returns the moment `try_wait` sees the child gone — it never
    /// joins and never reads to EOF. That was fail-LOUD while the stdout assertion was a POSITIVE (a
    /// lost race left the buffer short and the cell went RED); after the D48 inversion the same race
    /// is fail-SILENT, because "stdout is EMPTY" is exactly what an unread buffer looks like. So the
    /// headline channel-revert mutation could survive its own regression pin intermittently.
    /// [`join_drains`](Self::join_drains) closes that window. Both pipes reach EOF at child exit, so
    /// the joins cannot hang behind the existing deadline.
    ///
    /// Not reproduced — this is a read of the synchronisation, and it is rare precisely because the
    /// 25 ms poll granularity usually hides it. It is fixed rather than measured because the failure
    /// mode is a false GREEN.
    drains: Vec<std::thread::JoinHandle<()>>,
}

impl McpClient {
    /// Spawn `unblock mcp` in `root` with piped stdio + a background-drained stderr (kept off stdout).
    #[must_use]
    pub fn spawn(root: &Path) -> Self {
        Self::spawn_with_args(root, &[])
    }

    /// Spawn `unblock mcp` at DEBUG verbosity (`-vv`, `logging.rs`'s level map) — the T3.2.1/D38
    /// peer of [`spawn`](Self::spawn) for the cases that assert on the child's `tracing::debug!`
    /// output (the demoted cancellation-class diagnostic, and the `install()`-ordering markers).
    /// `-v` is a clap `global` flag, so it is accepted after the subcommand.
    #[must_use]
    pub fn spawn_verbose(root: &Path) -> Self {
        Self::spawn_with_args(root, &["-vv"])
    }

    /// Spawn `unblock mcp <args...>` in `root` with piped stdio + a background-drained stderr.
    #[must_use]
    pub fn spawn_with_args(root: &Path, args: &[&str]) -> Self {
        let mut child = unblock_in(root)
            .arg("mcp")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn `unblock mcp`");
        let stdin = child.stdin.take().expect("child stdin");
        let stdout = BufReader::new(child.stdout.take().expect("child stdout"));

        // T3.2.1/D38 diagnosability: the child's stderr was previously PIPED AND NEVER READ, so every
        // child-side diagnostic (`error[CODE]: message`, a panic, a tracing line) was destroyed —
        // including in CI, which is a large part of why the pre-handshake hang went unnoticed for
        // weeks behind a green suite. Drain it CONTINUOUSLY on a background thread (never at EOF only:
        // a hung child never reaches EOF, and its stderr must still be readable) into a shared buffer
        // the assertion messages can quote. Draining also guarantees a full stderr pipe can never
        // stall the child mid-shutdown, which would itself masquerade as the hang under test.
        let stderr = Arc::new(Mutex::new(String::new()));
        let mut drains = Vec::new();
        if let Some(pipe) = child.stderr.take() {
            let sink = Arc::clone(&stderr);
            // RETAINED, not detached (D48) — see the `drains` field.
            drains.push(std::thread::spawn(move || drain_into(pipe, &sink)));
        }

        Self {
            child,
            stdin: Some(stdin),
            stdout: Some(stdout),
            next_id: 1,
            seen_lines: Vec::new(),
            stderr,
            stdout_capture: Arc::new(Mutex::new(String::new())),
            drains,
        }
    }

    /// Wrap an ALREADY-SPAWNED child with piped stdio, wiring the same retained drain threads as
    /// [`spawn_with_args`](Self::spawn_with_args) — the TEST-ONLY constructor behind the harness
    /// self-test that proves the framing guard is actually INSTALLED inside
    /// [`read_response`](Self::read_response).
    ///
    /// The predicate having a self-test does not prove it is CALLED: with the `assert!` deleted, an
    /// orphaned `pub fn is_jsonrpc_framing` raises no dead-code warning (this module is
    /// `#![allow(dead_code)]`) and every one of the inherited `spawn*` sites silently loses its
    /// guard. Driving a FABRICATED child whose stdout emits a non-frame line is the only way to
    /// observe the call site.
    #[must_use]
    pub fn from_child(mut child: Child) -> Self {
        let stdin = child.stdin.take().expect("fabricated child stdin");
        let stdout = BufReader::new(child.stdout.take().expect("fabricated child stdout"));
        let stderr = Arc::new(Mutex::new(String::new()));
        let mut drains = Vec::new();
        if let Some(pipe) = child.stderr.take() {
            let sink = Arc::clone(&stderr);
            drains.push(std::thread::spawn(move || drain_into(pipe, &sink)));
        }
        Self {
            child,
            stdin: Some(stdin),
            stdout: Some(stdout),
            // The same seed as `spawn_with_args`: the guard-DELETED branch must be able to correlate
            // the fabricated child's `id=1` response, or it would hang to the deadline instead of
            // returning, and the self-test would stop discriminating between the two states.
            next_id: 1,
            seen_lines: Vec::new(),
            stderr,
            stdout_capture: Arc::new(Mutex::new(String::new())),
            drains,
        }
    }

    /// Everything the child has written to STDERR so far (T3.2.1/D38). Quote this in EVERY failure
    /// message of a case that drives a child: without it a child-side error is invisible and the case
    /// reports only a bare exit code / timeout (the diagnosability gap D38 was hidden by).
    #[must_use]
    pub fn stderr_snapshot(&self) -> String {
        snapshot(&self.stderr)
    }

    /// Move this client's STDOUT into a background CAPTURE thread, RETAINING every byte for
    /// [`stdout_snapshot`](Self::stdout_snapshot) — since D48, **the NEGATIVE-side oracle**: the
    /// buffer it fills must be FRAME-FREE (on the unsignalled `Err` path, EMPTY), because the
    /// structured error no longer goes anywhere near this stream. The POSITIVE now lives on stderr,
    /// via [`structured_error_on_stderr`]. It is the retaining peer of
    /// [`write_without_reading`](Self::write_without_reading), which DISCARDS what it drains.
    ///
    /// Like that method this consumes the stdout reader, so no further request/response traffic is
    /// possible on this client afterwards — call it only on a case that just signals/waits.
    ///
    /// A negative assertion over this buffer is only sound once the drain has been JOINED, which
    /// [`wait_for`](Self::wait_for) now does: an unread buffer is empty too.
    pub fn capture_stdout(&mut self) {
        if let Some(stdout) = self.stdout.take() {
            let sink = Arc::clone(&self.stdout_capture);
            // RETAINED, not detached (D48) — see the `drains` field.
            self.drains
                .push(std::thread::spawn(move || drain_into(stdout, &sink)));
        }
    }

    /// Join every retained drain thread, so both capture buffers are COMPLETE (D48).
    ///
    /// Called by [`wait_for`](Self::wait_for) once the child is gone — both pipes are then at EOF,
    /// so each drain returns promptly and no join can hang behind the caller's deadline. The
    /// deliberately-detached third drain (`drain_to_eof`, behind
    /// [`write_without_reading`](Self::write_without_reading)) is NOT retained: it discards every
    /// byte, so no assertion can ever read what it drained.
    fn join_drains(&mut self) {
        for handle in self.drains.drain(..) {
            let _ignored = handle.join();
        }
    }

    /// How many retained drain threads are still UNJOINED — the deterministic observation of
    /// [`join_drains`](Self::join_drains) having run.
    ///
    /// It exists because the alternative is a TIMING assertion. The defect this accessor guards
    /// against (deleting the `join_drains()` call in [`wait_for`](Self::wait_for)) is fail-SILENT
    /// and RARE — an unjoined drain usually finishes first, so a cell that asserted on buffer
    /// COMPLETENESS alone would pass under the mutation on nearly every run and then flake for real
    /// on a loaded machine. The count is exact, so the self-test (`mcp_stdout_channel.rs` H9) reads
    /// the synchronisation itself rather than gambling on its outcome.
    #[must_use]
    pub fn pending_drain_count(&self) -> usize {
        self.drains.len()
    }

    /// Everything the child has written to STDOUT since [`capture_stdout`](Self::capture_stdout)
    /// (empty if it was never called).
    #[must_use]
    pub fn stdout_snapshot(&self) -> String {
        snapshot(&self.stdout_capture)
    }

    /// Wait for the child to exit within `timeout`, returning its exit status — the [`wait_for`]
    /// peer that, on a TIMEOUT (i.e. the D38 hang), reports the child's captured STDERR instead of a
    /// bare "did not exit". Prefer this over [`wait_for`] whenever an `McpClient` owns the child.
    ///
    /// On a clean exit it JOINS the retained drains (D48) before returning, so a `stdout_snapshot()`
    /// read as a NEGATIVE — or a `stderr_snapshot()` read as a POSITIVE — sees the complete buffer
    /// rather than whatever the race left in it. The deadline panic below deliberately precedes any
    /// join: a HUNG child never reaches EOF, so joining first would replace a diagnosable timeout
    /// with a silent block.
    ///
    /// # Panics
    /// If the child does not exit within `timeout` (the no-hang invariant, spine §5b).
    pub fn wait_for(&mut self, timeout: Duration) -> std::process::ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait().expect("try_wait") {
                self.join_drains();
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "D38 no-hang invariant: the child did not exit within {timeout:?}. Child stderr:\n{}",
                self.stderr_snapshot()
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Send a JSON-RPC request and block for the matching-id response (asserting valid JSON framing).
    pub fn request(&mut self, method: &str, params: &Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.write_line(&req);
        self.read_response(id)
    }

    /// Send a JSON-RPC notification (no id, no response expected).
    pub fn notify(&mut self, method: &str, params: &Value) {
        let note = json!({"jsonrpc": "2.0", "method": method, "params": params});
        self.write_line(&note);
    }

    /// Send a request but do NOT read the response — spawns a background thread that drains this
    /// client's stdout to EOF (discarding every byte, no JSON assertions) so an unread response
    /// cannot fill the OS pipe buffer and stall the child's write around a signal (T3.2 C2/C6
    /// guardrail: never read a bulk response synchronously on a signal case). After this call no
    /// further request/response traffic is possible on this client — the stdout reader has moved
    /// into the drain thread — which is fine: C2/C6 only signal/wait on `child` afterward.
    pub fn write_without_reading(&mut self, method: &str, params: &Value) {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.write_line(&req);
        if let Some(stdout) = self.stdout.take() {
            std::thread::spawn(move || drain_to_eof(stdout));
        }
    }

    fn write_line(&mut self, value: &Value) {
        let mut line = serde_json::to_string(value).expect("serialize request");
        line.push('\n');
        let stdin = self.stdin.as_mut().expect("stdin still open");
        stdin
            .write_all(line.as_bytes())
            .expect("write to child stdin");
        stdin.flush().expect("flush child stdin");
    }

    /// Close the child's stdin pipe (EOF) so the MCP server shuts down cleanly (exit 0).
    pub fn close_stdin(&mut self) {
        // Dropping the `ChildStdin` closes the write end of the pipe → the child reads EOF.
        drop(self.stdin.take());
    }

    /// Read newline-delimited lines until the response with `id` arrives. EVERY line read must be
    /// JSON-RPC FRAMING — this is the NFR-14/D48 "stdout carries only MCP framing" guard.
    ///
    /// Merely PARSING as JSON is not enough, and that is not a hypothetical: the ub-og3
    /// `StructuredError` blob IS valid JSON, which is exactly how it passed this guard for the whole
    /// life of the suite while sitting on the framing channel. The predicate is
    /// [`is_jsonrpc_framing`]; this call site is what INSTALLS it for all the inherited `spawn*`
    /// sites, and it has its own self-test because deleting it here would be green and silent.
    fn read_response(&mut self, id: i64) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            assert!(
                Instant::now() < deadline,
                "timed out awaiting response id={id}"
            );
            let stdout = self
                .stdout
                .as_mut()
                .expect("stdout still owned (no write_without_reading call yet)");
            let mut line = String::new();
            let n = stdout.read_line(&mut line).expect("read child stdout line");
            assert!(n > 0, "child stdout closed before response id={id}");
            let trimmed = line.trim_end_matches(['\n', '\r']);
            if trimmed.is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(trimmed).unwrap_or_else(|e| {
                panic!("NFR-14: every stdout line must be JSON-RPC framing, got `{trimmed}`: {e}")
            });
            assert!(
                is_jsonrpc_framing(&value),
                "NFR-14/D48: every stdout line must be JSON-RPC framing — a line that merely \
                 PARSES as JSON is not enough, which is exactly how the ub-og3 StructuredError \
                 blob passed this guard for the whole life of the suite. Got `{trimmed}`"
            );
            self.seen_lines.push(trimmed.to_string());
            // Responses have an `id`; notifications from the server (no id) are skipped.
            if value.get("id").and_then(Value::as_i64) == Some(id) {
                return value;
            }
        }
    }

    /// **The DETERMINISTIC pre-handshake readiness barrier (T3.2.1/D38).** Block until the server is
    /// provably parked awaiting `initialize` — WITHOUT completing the handshake — by round-tripping a
    /// JSON-RPC `ping`.
    ///
    /// The MCP lifecycle spec permits `ping` BEFORE `initialize`, and rmcp 1.7 implements it: its
    /// handshake loop (`rmcp-1.7.0/src/service/server.rs` `serve_server_with_ct_inner`) answers a
    /// `PingRequest` with an `EmptyResult` and LOOPS BACK to `expect_next_message` — it does not
    /// break out to negotiate. (`VersionClampingTransport` passes it through untouched: the clamp
    /// only rewrites `InitializeRequest`.) So receiving the ping response proves, IN-BAND:
    /// 1. the run loop is up ⇒ `shutdown::install()` has ALREADY run (it precedes `run_mcp_server`),
    ///    so a signal sent now is RECORDED, never delivered to the default disposition; and
    /// 2. the stdio transport is bound and a FRESH blocking-pool `stdin` read is parked — the exact
    ///    precondition of the D38 runtime-drop hang; and
    /// 3. the `initialize` handshake is still INCOMPLETE — the `Err(Cancelled)` window under test.
    ///
    /// **This replaces a fixed 300ms sleep, which was FLAKY (measured RED 3/3 under CPU load: the
    /// window expired while the child was still opening/migrating the DB, so the signal landed before
    /// `install()` and killed it by default disposition → `left: None` = WIFSIGNALED, with an empty
    /// child stderr).** Lengthening that sleep would only have moved the race; widening the assertion
    /// to accept a signal-death would have been the prohibited vacuous fix. The predecessor's
    /// docstring claimed "there is no in-band signal for 'rmcp is now awaiting initialize'" — that is
    /// FALSE, and this is it.
    ///
    /// # Panics
    /// If the child dies or does not answer the ping (either is a REAL defect: the server must stay
    /// alive awaiting a client) — reporting the child's captured stderr.
    pub fn ping_barrier(&mut self) {
        assert!(
            self.child.try_wait().expect("try_wait").is_none(),
            "`unblock mcp` must stay alive awaiting `initialize`, but exited before the barrier. \
             Child stderr:\n{}",
            self.stderr_snapshot()
        );
        let resp = self.request("ping", &json!({}));
        assert!(
            resp.get("error").is_none(),
            "a pre-initialize `ping` must be answered (the MCP lifecycle spec permits it and rmcp \
             1.7 implements it) — got: {resp}. Child stderr:\n{}",
            self.stderr_snapshot()
        );
    }

    /// Block until the child's STDERR contains `marker`, returning the full snapshot (T3.2.1/D38).
    ///
    /// The readiness barrier for stderr lines emitted BEFORE the run loop exists (so
    /// [`ping_barrier`](Self::ping_barrier) cannot see them). Two marker CLASSES flow through here,
    /// with DIFFERENT verbosity requirements:
    /// - the D39 startup-visibility line (`unblock: workspace bound to …`) is an UNCONDITIONAL direct
    ///   stderr write — readable on a plain [`spawn`](Self::spawn) child, NO `-vv` needed; and
    /// - the `install()`-ordering markers (`mcp: shutdown signal handling installed` /
    ///   `mcp: workspace opened`) are `tracing::debug!`, so they require a `-vv` child
    ///   ([`spawn_verbose`](Self::spawn_verbose)).
    ///
    /// # Panics
    /// If `marker` does not appear within `timeout`, or the child dies first.
    pub fn wait_for_stderr(&mut self, marker: &str, timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        loop {
            let snapshot = self.stderr_snapshot();
            if snapshot.contains(marker) {
                return snapshot;
            }
            if let Some(status) = self.child.try_wait().expect("try_wait") {
                panic!(
                    "the child exited ({status:?}) before emitting `{marker}` on stderr. \
                     Child stderr:\n{snapshot}"
                );
            }
            assert!(
                Instant::now() < deadline,
                "`{marker}` did not appear on the child's stderr within {timeout:?} \
                 (the `install()`-ordering markers are debug-level — is the child `-vv`? — but the \
                 D39 `workspace bound to` line is unconditional). Child stderr:\n{snapshot}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Complete the MCP `initialize` handshake, returning the `InitializeResult`.
    pub fn initialize(&mut self) -> Value {
        let result = self.request(
            "initialize",
            &json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "unblock-cli-test", "version": "0.0.0"}
            }),
        );
        assert!(result.get("error").is_none(), "initialize failed: {result}");
        // Per the MCP spec the client sends `notifications/initialized` after a successful init.
        self.notify("notifications/initialized", &json!({}));
        result["result"].clone()
    }

    /// Call a tool and return the FULL JSON-RPC response envelope, error member included.
    ///
    /// [`Self::call_tool`] asserts the error member away, so it structurally cannot express the
    /// D42 channel invariant (`resp["error"].is_none()` — a malformed ARGUMENT must arrive in-band
    /// as `isError:true`, never as an out-of-band `-32602`). Use this for any cell that is about
    /// WHICH channel a failure took.
    pub fn call_tool_envelope(&mut self, name: &str, arguments: &Value) -> Value {
        self.request("tools/call", &json!({"name": name, "arguments": arguments}))
    }

    /// Send a raw `tools/call` with hand-built `params` (so a cell can omit `arguments` entirely,
    /// or set `_meta` / `task`, which [`Self::call_tool_envelope`] cannot).
    pub fn call_tool_raw_params(&mut self, params: &Value) -> Value {
        self.request("tools/call", params)
    }

    // -- D43 RAW-BYTES capability -------------------------------------------------------------
    //
    // Everything above round-trips through `serde_json::to_string(&Value)`, which builds a `Map`
    // first and therefore **structurally cannot emit a duplicate key** — the one input the D43
    // suite exists to test. `json!` cannot either. These four methods are the only way to put
    // arbitrary bytes on the wire.

    /// Allocate the next request id (the same counter the serde-built helpers use, so raw and
    /// normal traffic can be interleaved on one connection).
    pub fn next_request_id(&mut self) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Write EXACT bytes plus a newline. NO serde on this path.
    pub fn write_raw_line(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().expect("stdin still open");
        stdin
            .write_all(line.as_bytes())
            .expect("write raw frame to child stdin");
        stdin.write_all(b"\n").expect("write newline");
        stdin.flush().expect("flush child stdin");
    }

    /// Write a raw frame and block for the response with `id`.
    ///
    /// Routes through the same `read_response`, so the NFR-14 "every stdout line is JSON" guard
    /// still applies to whatever the server answers.
    pub fn request_raw(&mut self, id: i64, raw: &str) -> Value {
        self.write_raw_line(raw);
        self.read_response(id)
    }

    /// Was a response with `id` ever seen on this connection?
    ///
    /// The deterministic, TIMEOUT-FREE probe for "no response at all": send the frame, then a
    /// known-good SENTINEL request with a fresh id, read the sentinel's response, then ask this. No
    /// sleeps, no threads, no flake.
    #[must_use]
    pub fn saw_response_for(&self, id: i64) -> bool {
        self.seen_lines.iter().any(|line| {
            serde_json::from_str::<Value>(line)
                .ok()
                .and_then(|value| value.get("id").and_then(Value::as_i64))
                == Some(id)
        })
    }

    /// Call a tool, returning `(is_error, structured_content)`.
    pub fn call_tool(&mut self, name: &str, arguments: &Value) -> (bool, Value) {
        let resp = self.request("tools/call", &json!({"name": name, "arguments": arguments}));
        assert!(
            resp.get("error").is_none(),
            "tools/call transport error: {resp}"
        );
        let result = &resp["result"];
        let is_error = result["isError"].as_bool().unwrap_or(false);
        let structured = result
            .get("structuredContent")
            .cloned()
            .unwrap_or(Value::Null);
        (is_error, structured)
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        // Best-effort: kill the child if a test bailed before shutting it down cleanly.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Read a background-drained capture buffer, tolerating a poisoned lock (a panicking drain thread
/// must never turn a real assertion failure into a confusing second panic).
fn snapshot(buffer: &Mutex<String>) -> String {
    buffer.lock().map_or_else(
        |poisoned| poisoned.into_inner().clone(),
        |guard| guard.clone(),
    )
}

/// Drain `pipe` line-by-line into `sink` until EOF (T3.2.1/D38) — backs [`McpClient`]'s stderr
/// capture. Appends INCREMENTALLY (never `read_to_end`) so a still-running / HUNG child's stderr is
/// already readable when a case's deadline fires — the whole point of capturing it.
fn drain_into<R: std::io::Read>(pipe: R, sink: &Mutex<String>) {
    let mut reader = BufReader::new(pipe);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => match sink.lock() {
                Ok(mut guard) => guard.push_str(&line),
                Err(poisoned) => poisoned.into_inner().push_str(&line),
            },
        }
    }
}

/// Read `stdout` to EOF in a background thread, discarding every byte (no assertions) — the T3.2
/// C2/C6 guardrail helper backing [`McpClient::write_without_reading`].
fn drain_to_eof(mut stdout: BufReader<ChildStdout>) {
    let mut line = String::new();
    loop {
        line.clear();
        match stdout.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
}

/// Send `signal` to `pid` (unix) via a `libc`-free / `nix`-free path: `std` offers no safe `kill`, so
/// we shell out to the system `kill` command (a POSIX utility; NOT git, NOT network — allowed in a
/// TEST). This keeps the test unsafe-free while delivering a real signal to the child. STRICT: asserts
/// the `kill` invocation itself succeeded (the target must be alive) — used for every single-signal
/// case (C1/C3) and the FIRST signal of the escalation case (C6); a failed strict kill is a harness
/// bug, not an expected outcome.
pub fn send_signal(pid: u32, signal: &str) {
    let status = std::process::Command::new("kill")
        .args(["-s", signal, &pid.to_string()])
        .status()
        .expect("run kill");
    assert!(status.success(), "kill -s {signal} {pid} failed");
}

/// Like [`send_signal`], but tolerates an already-dead target (`kill`'s ESRCH "No such process",
/// exit code 1) — used ONLY for the SECOND signal in the escalation case (C6/T3.2): by the time it
/// is sent the child may already be mid-exit (or exited) from the first signal, and that is not a
/// test failure.
pub fn send_signal_tolerant(pid: u32, signal: &str) {
    let _ = std::process::Command::new("kill")
        .args(["-s", signal, &pid.to_string()])
        .status();
}

/// Wait for the child to exit within `timeout`, returning its exit status.
#[must_use]
pub fn wait_for(child: &mut Child, timeout: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "child did not exit within {timeout:?}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Extract an issue id from a create/show structured result (`{"id": "..."}` or `{"issue": {...}}`).
#[must_use]
pub fn issue_id(value: &Value) -> String {
    value
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .get("issue")
                .and_then(|i| i.get("id"))
                .and_then(Value::as_str)
        })
        .unwrap_or_else(|| panic!("no id in create result: {value}"))
        .to_string()
}

/// Collect the `id`s from a structured value that is (or contains) an array of issues.
#[must_use]
pub fn id_set(value: &Value) -> BTreeSet<String> {
    let array = value
        .as_array()
        .or_else(|| value.get("issues").and_then(Value::as_array))
        .cloned()
        .unwrap_or_default();
    array
        .iter()
        .filter_map(|i| i.get("id").and_then(Value::as_str).map(str::to_string))
        .collect()
}

/// Run a json-mode command in `ws` and parse its stdout as a JSON report (asserts success exit 0).
/// Promoted here (D27/AF-1/AF-2, originally `migrate_doctor.rs`-private) so T3.2's C-doctor case can
/// reuse it verbatim without inventing a second report-parsing shape.
pub fn json_report(ws: &Workspace, args: &[&str]) -> Value {
    let out = ws.cmd().args(args).output().expect("run command");
    assert_eq!(
        out.status.code(),
        Some(0),
        "command must succeed; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    serde_json::from_str(stdout.trim()).expect("valid JSON report on stdout")
}

/// Pull a finding's `detail` by `label` from a `DiagnosticReport`-shaped value. Promoted alongside
/// [`json_report`] (see its docs).
#[must_use]
pub fn detail<'a>(report: &'a Value, label: &str) -> Option<&'a str> {
    report["findings"]
        .as_array()?
        .iter()
        .find(|f| f["label"] == label)
        .and_then(|f| f["detail"].as_str())
}

// ----------------------------------------------------------------------------------------------
// T3.2 — shared shutdown-case oracle + the pinned bulk fixture.
// ----------------------------------------------------------------------------------------------

/// Reopen `root`'s workspace FRESH via the SAME config facade the CLI uses (so `open_local` runs its
/// normal WAL-recovery open — NEVER a raw file read) and return `(integrity_problems, count)` — the
/// shared shutdown-case oracle every CLI e2e case funnels through (C1/C2/C3/C6/C-doctor/C-neg, T3.2).
/// `count` covers the FULL corpus (`include_closed`+`include_deferred`) so a partial commit is
/// visible. The reopened `Session` is dropped before returning (never lingers past the caller's
/// `Workspace` `TempDir` cleanup).
///
/// # Panics
/// If the workspace cannot be reopened / read (a harness precondition, not the SUT under test).
pub async fn reopen_and_check(root: &Path) -> (Vec<String>, usize) {
    let overrides = CliOverrides::new().with_dir(root.join(".unblock"));
    let ctx = open_with_storage_with_cli(&overrides)
        .await
        .expect("reopen workspace via the config facade");
    let session = Session::open(ctx, SessionConfig::default())
        .await
        .expect("reopen session");
    let problems = session.integrity_check().await.expect("integrity_check");
    let filters = ListFilters {
        include_closed: true,
        include_deferred: true,
        ..ListFilters::default()
    };
    let count = session.list(&filters).await.expect("list").len();
    drop(session);
    (problems, count)
}

/// Build the pinned T3.2 bulk-markdown fixture: `n` title-only `## bulk-item-k` H2 blocks (no body,
/// no H3 sections). The SAME document shape is reused across C1/C2/C6 (T3.2 spec) — C1 is the
/// within-cap validity control: it awaits the success response and asserts `count == n`, proving the
/// document is accepted, so C2/C6's `count ∈ {0, n}` cannot pass vacuously via a
/// validation-rejected (non-signal) doc.
#[must_use]
pub fn bulk_markdown(n: usize) -> String {
    use std::fmt::Write as _;
    let mut doc = String::new();
    for k in 1..=n {
        let _ = writeln!(doc, "## bulk-item-{k}");
    }
    doc
}
