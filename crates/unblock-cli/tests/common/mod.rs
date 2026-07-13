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
#[must_use]
pub fn unblock() -> Command {
    Command::cargo_bin("unblock").expect("locate the `unblock` binary")
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
}

impl McpClient {
    /// Spawn `unblock mcp` in `root` with piped stdio + captured stderr (kept off stdout).
    #[must_use]
    pub fn spawn(root: &Path) -> Self {
        let mut child = unblock_in(root)
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn `unblock mcp`");
        let stdin = child.stdin.take().expect("child stdin");
        let stdout = BufReader::new(child.stdout.take().expect("child stdout"));
        Self {
            child,
            stdin: Some(stdin),
            stdout: Some(stdout),
            next_id: 1,
            seen_lines: Vec::new(),
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
    /// valid JSON — this is the NFR-14 "stdout carries only MCP framing" guard.
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
            self.seen_lines.push(trimmed.to_string());
            // Responses have an `id`; notifications from the server (no id) are skipped.
            if value.get("id").and_then(Value::as_i64) == Some(id) {
                return value;
            }
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
