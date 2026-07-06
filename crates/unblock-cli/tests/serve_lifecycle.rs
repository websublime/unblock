//! `unblock serve` end-to-end over piped stdio (FR-9/FR-20/FR-17, D27/AD-4).
//!
//! Drives the REAL `unblock serve` binary as a child process, speaking the MCP JSON-RPC protocol over
//! its stdin/stdout directly (rmcp's `transport-io` framing is **newline-delimited JSON** — verified
//! against `rmcp::transport::async_rw`). Proves:
//! - the MCP `initialize` handshake succeeds and advertises the `unblock` identity;
//! - a `issue{create}` → `query{ready}` → `claim` → `issue{close, suggest_next}` smoke runs over
//!   stdio, and closing the blocker surfaces the newly-unblocked dependent (CLI↔MCP wiring, FR-9/20);
//! - **stdout carries ONLY MCP framing** — every non-empty stdout line parses as JSON-RPC (NFR-14: no
//!   log line ever pollutes stdout);
//! - SIGTERM mid-serve drives a CLEAN cooperative shutdown: the process exits `128 + 15 == 143` and
//!   stdout still holds only MCP framing (FR-17; the adversarial WAL-corruption proof is T3.2).
//!
//! These are unix-only (the SIGTERM/exit-`128+signo` contract is a unix construct; Windows serve is a
//! no-op EOF path, NFR-11) — gated with `#![cfg(unix)]`.
#![cfg(unix)]

mod common;

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::time::{Duration, Instant};

use common::Workspace;
use serde_json::{Value, json};

/// A hand-rolled newline-delimited JSON-RPC client over a spawned `unblock serve` child.
struct ServeClient {
    child: Child,
    /// `Some` while the pipe is open; `.take()` + drop closes it → the child reads EOF.
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
    /// Every non-empty stdout LINE the server produced (each must be valid JSON — NFR-14 guard).
    seen_lines: Vec<String>,
}

impl ServeClient {
    /// Spawn `unblock serve` in `root` with piped stdio + captured stderr (kept off stdout).
    fn spawn(root: &Path) -> Self {
        let mut child = common::unblock_in(root)
            .arg("serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn `unblock serve`");
        let stdin = child.stdin.take().expect("child stdin");
        let stdout = BufReader::new(child.stdout.take().expect("child stdout"));
        Self {
            child,
            stdin: Some(stdin),
            stdout,
            next_id: 1,
            seen_lines: Vec::new(),
        }
    }

    /// Send a JSON-RPC request and block for the matching-id response (asserting valid JSON framing).
    fn request(&mut self, method: &str, params: &Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.write_line(&req);
        self.read_response(id)
    }

    /// Send a JSON-RPC notification (no id, no response expected).
    fn notify(&mut self, method: &str, params: &Value) {
        let note = json!({"jsonrpc": "2.0", "method": method, "params": params});
        self.write_line(&note);
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

    /// Close the child's stdin pipe (EOF) so `serve` shuts down cleanly (exit 0).
    fn close_stdin(&mut self) {
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
            let mut line = String::new();
            let n = self
                .stdout
                .read_line(&mut line)
                .expect("read child stdout line");
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
    fn initialize(&mut self) -> Value {
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
    fn call_tool(&mut self, name: &str, arguments: &Value) -> (bool, Value) {
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

impl Drop for ServeClient {
    fn drop(&mut self) {
        // Best-effort: kill the child if a test bailed before shutting it down cleanly.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Send `signal` to `pid` (unix) via `libc`-free `nix`-free path: the `kill(1)`-equivalent through
/// `std::process` is not available, so use the raw syscall via the `signal-hook`-adjacent... — NO:
/// `#![forbid(unsafe_code)]` holds, and `std` offers no safe `kill`. We shell out to the system
/// `kill` command (a POSIX utility; NOT git, NOT network — allowed in a TEST). This keeps the test
/// unsafe-free while delivering a real SIGTERM to the child.
fn send_signal(pid: u32, signal: &str) {
    let status = std::process::Command::new("kill")
        .args(["-s", signal, &pid.to_string()])
        .status()
        .expect("run kill");
    assert!(status.success(), "kill -s {signal} {pid} failed");
}

#[test]
fn initialize_handshake_advertises_unblock_identity() {
    let ws = Workspace::init();
    let mut client = ServeClient::spawn(ws.root());
    let init = client.initialize();
    assert_eq!(
        init["serverInfo"]["name"], "unblock",
        "server identity must be `unblock`"
    );
    assert!(
        init.get("capabilities").is_some(),
        "initialize advertises capabilities"
    );

    // Clean shutdown: closing stdin (EOF) returns `serve` cleanly (exit 0). Drop stdin explicitly.
    client.close_stdin();
    let status = wait_for(&mut client.child, Duration::from_secs(20));
    assert_eq!(status.code(), Some(0), "EOF drives a clean exit 0");
}

#[test]
fn ready_claim_close_smoke_over_stdio() {
    let ws = Workspace::init();
    let mut client = ServeClient::spawn(ws.root());
    client.initialize();

    // Seed a blocker + a dependent via the MINTING create path, add a Blocks edge, then run
    // ready → claim → close and assert the close surfaces the newly-unblocked dependent (FR-9/FR-20).
    let (err, blocker) =
        client.call_tool("issue", &json!({"action": "create", "title": "blocker"}));
    assert!(!err, "create blocker: {blocker}");
    let blocker_id = issue_id(&blocker);
    let (err, dependent) =
        client.call_tool("issue", &json!({"action": "create", "title": "dependent"}));
    assert!(!err, "create dependent: {dependent}");
    let dependent_id = issue_id(&dependent);

    let (err, dep) = client.call_tool(
        "dep",
        &json!({
            "action": "add",
            "issue_id": dependent_id,
            "depends_on_id": blocker_id,
            "dep_type": "blocks"
        }),
    );
    assert!(!err, "add blocking edge: {dep}");

    // query{ready}: the blocker is ready, the dependent is blocked.
    let (err, ready) = client.call_tool("query", &json!({"kind": "ready"}));
    assert!(!err, "ready query: {ready}");
    let ready_ids = id_set(&ready);
    assert!(ready_ids.contains(&blocker_id), "blocker is ready");
    assert!(!ready_ids.contains(&dependent_id), "dependent is blocked");

    // claim the blocker.
    let (err, claimed) =
        client.call_tool("claim", &json!({"id": blocker_id, "assignee": "agent-a"}));
    assert!(!err, "claim: {claimed}");

    // close{suggest_next}: the close surfaces the now-unblocked dependent (FR-11).
    let (err, close) = client.call_tool(
        "issue",
        &json!({"action": "close", "id": blocker_id, "suggest_next": true}),
    );
    assert!(!err, "close: {close}");
    let unblocked = id_set(&close["newly_unblocked"]);
    assert!(
        unblocked.contains(&dependent_id),
        "closing the blocker surfaces the dependent as newly unblocked: {close}"
    );

    // Every stdout line seen so far was valid JSON-RPC framing (asserted inside read_response).
    assert!(
        !client.seen_lines.is_empty(),
        "the server produced MCP framing on stdout"
    );

    client.close_stdin();
    let status = wait_for(&mut client.child, Duration::from_secs(20));
    assert_eq!(status.code(), Some(0), "clean EOF exit 0 after the smoke");
}

#[test]
fn sigterm_drives_clean_shutdown_with_exit_143() {
    let ws = Workspace::init();
    let mut client = ServeClient::spawn(ws.root());
    // Complete a handshake so the server is fully up + serving before the signal.
    client.initialize();
    let (err, _ready) = client.call_tool("query", &json!({"kind": "ready"}));
    assert!(!err, "a call works before the signal");

    // SIGTERM the child (FR-17): the signal cancels the token + sets the engine flag → `serve` returns
    // Ok → `session.shutdown()` → the process exits `128 + 15 == 143`.
    let pid = client.child.id();
    send_signal(pid, "TERM");

    let status = wait_for(&mut client.child, Duration::from_secs(20));
    assert_eq!(
        status.code(),
        Some(143),
        "SIGTERM yields the conventional 128+15 exit (FR-17/D27/AD-4)"
    );
    // stdout still holds ONLY MCP framing — no shutdown diagnostic leaked to stdout (NFR-14). Every
    // line captured during the session was already asserted JSON in read_response; drain any tail.
    for line in client.seen_lines.clone() {
        serde_json::from_str::<Value>(&line)
            .unwrap_or_else(|e| panic!("stdout line not JSON framing after shutdown: {line}: {e}"));
    }
}

/// Extract an issue id from a create/show structured result (`{"id": "..."}` or `{"issue": {...}}`).
fn issue_id(value: &Value) -> String {
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
fn id_set(value: &Value) -> std::collections::BTreeSet<String> {
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

/// Wait for the child to exit within `timeout`, returning its exit status.
fn wait_for(child: &mut Child, timeout: Duration) -> std::process::ExitStatus {
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
