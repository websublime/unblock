//! `unblock mcp` end-to-end over piped stdio (FR-9/FR-20/FR-17, D27/AD-4).
//!
//! Drives the REAL `unblock mcp` binary as a child process, speaking the MCP JSON-RPC protocol over
//! its stdin/stdout directly (rmcp's `transport-io` framing is **newline-delimited JSON** — verified
//! against `rmcp::transport::async_rw`). Proves:
//! - the MCP `initialize` handshake succeeds and advertises the `unblock` identity;
//! - a `issue{create}` → `query{ready}` → `claim` → `issue{close, suggest_next}` smoke runs over
//!   stdio, and closing the blocker surfaces the newly-unblocked dependent (CLI↔MCP wiring, FR-9/20);
//! - **stdout carries ONLY MCP framing** — every non-empty stdout line parses as JSON-RPC (NFR-14: no
//!   log line ever pollutes stdout);
//! - SIGTERM mid-run drives a CLEAN cooperative shutdown: the process exits `128 + 15 == 143` and
//!   stdout still holds only MCP framing (FR-17; the adversarial WAL-corruption/mid-write-atomicity
//!   proof is `tests/shutdown_failure_injection.rs` (T3.2 — cases C1/C2/C3/C6) plus the deterministic
//!   drain-to-commit barrier `unblock-engine/tests/shutdown_drain_barrier.rs` (C4) and the SIGKILL
//!   abandoned-tx recovery proof `unblock-storage/tests/shutdown_abandoned_tx.rs` (C5));
//! - **T3.2.1/D38 — the two DETERMINISTIC shutdown cases** (unlike the race-robust real-signal e2e
//!   cases in `shutdown_failure_injection.rs`, these are deterministic rather than invariant-only):
//!   - a signal delivered **BEFORE any client handshake** exits exactly `128+signo` and never hangs
//!     (`a_signal_before_any_handshake_exits_128_plus_signo_and_never_hangs` — D38 clause 1+2, the
//!     window where rmcp returns `Err(Cancelled)` rather than `Ok`). **This is the load-bearing
//!     regression case: it HANGS against the pre-fix binary**, and it is RED under BOTH D38
//!     mutations — removing the signal-precedence guard (→ exit 1) and restoring the blocking
//!     runtime drop (→ hang);
//!   - an **unsignalled** genuine run-loop `Err` still exits `1` and never hangs
//!     (`a_no_signal_run_loop_error_exits_1_and_never_hangs`) — the OTHER half of the precedence:
//!     the fix must not swallow unsignalled failures. It passes pre-fix (see its docs for the
//!     measured reason); it guards against over-reach, and does not carry the no-hang proof.
//!
//! The `mcp` stdio harness (`McpClient`/`send_signal`/`wait_for`) lives in `tests/common/mod.rs`
//! (promoted there at T3.2 so the failure-injection suite can reuse it without duplication). Cases
//! that own a child use `McpClient::wait_for` (not the free `common::wait_for`), so a blown deadline
//! reports the child's captured STDERR (T3.2.1/D38 diagnosability) instead of a bare "did not exit".
//!
//! These are unix-only (the SIGTERM/exit-`128+signo` contract is a unix construct; Windows `unblock mcp` is a
//! no-op EOF path, NFR-11) — gated with `#![cfg(unix)]`.
#![cfg(unix)]

mod common;

use std::time::{Duration, Instant};

use common::{McpClient, Workspace, id_set, issue_id, send_signal};
use serde_json::{Value, json};

#[test]
fn initialize_handshake_advertises_unblock_identity() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());
    let init = client.initialize();
    assert_eq!(
        init["serverInfo"]["name"], "unblock",
        "server identity must be `unblock`"
    );
    assert!(
        init.get("capabilities").is_some(),
        "initialize advertises capabilities"
    );

    // Clean shutdown: closing stdin (EOF) returns `run_mcp_server` cleanly (exit 0). Drop stdin explicitly.
    client.close_stdin();
    let status = client.wait_for(Duration::from_secs(20));
    assert_eq!(status.code(), Some(0), "EOF drives a clean exit 0");
}

#[test]
fn ready_claim_close_smoke_over_stdio() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());
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
    let status = client.wait_for(Duration::from_secs(20));
    assert_eq!(status.code(), Some(0), "clean EOF exit 0 after the smoke");
}

#[test]
fn sigterm_drives_clean_shutdown_with_exit_143() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());
    // Complete a handshake so the server is fully up + serving before the signal.
    client.initialize();
    let (err, _ready) = client.call_tool("query", &json!({"kind": "ready"}));
    assert!(!err, "a call works before the signal");

    // SIGTERM the child (FR-17): the signal cancels the token + sets the engine flag → `run_mcp_server` returns
    // Ok → `session.shutdown()` → the process exits `128 + 15 == 143`.
    let pid = client.child.id();
    send_signal(pid, "TERM");

    let status = client.wait_for(Duration::from_secs(20));
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

// ------------------------------------------------------------------------------------------------
// T3.2.1 / D38 — the PRE-handshake signal path + the no-signal Err path (the two proven defects).
// ------------------------------------------------------------------------------------------------

/// Block until the child is parked awaiting the `initialize` request, WITHOUT sending one.
///
/// There is no in-band signal for "rmcp is now awaiting initialize" (the server writes nothing on
/// stdout before a request — asserting on stdout here would deadlock), so this polls the child's
/// liveness for a short settle window instead. Deliberately conservative: if the child were to exit
/// on its own during the window that is a REAL defect (the server must wait for a client), so the
/// case fails loudly here rather than silently degrading into a post-mortem signal that proves
/// nothing.
fn settle_before_handshake(client: &mut McpClient) {
    let settle = Duration::from_millis(300);
    let deadline = Instant::now() + settle;
    while Instant::now() < deadline {
        assert!(
            client.child.try_wait().expect("try_wait").is_none(),
            "`unblock mcp` must stay alive awaiting `initialize`, but exited before the signal. \
             Child stderr:\n{}",
            client.stderr_snapshot()
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// **T3.2.1/D38 AC(1) — the load-bearing regression case; it FAILS (HANGS) against the pre-fix
/// binary.** A signal delivered BEFORE any client completes the MCP `initialize` handshake must
/// still exit exactly `128+signo`, within a hard deadline.
///
/// Why this window is special (the defect chain — PRD §4/D38): rmcp 1.7's `serve_server_with_ct`
/// wraps the WHOLE handshake in a `select!` against the cancellation token, so a cancel landing here
/// returns `Err(ServerInitializeError::Cancelled)` — NOT `Ok` (spine §0.1: BOTH are normal
/// cooperative-shutdown outcomes). Pre-fix, `commands/mcp.rs` `?`-propagated that `Err` PAST the
/// `signal_exit_code()` guard (which sat only on the Ok path) → exit 1 → and the `#[tokio::main]`
/// runtime drop then blocked FOREVER in `BlockingPool::shutdown` on the parked `tokio::io::stdin()`
/// blocking read. So pre-fix this case HANGS to the deadline; the second-signal escalation (the only
/// thing that used to rescue it) is deliberately NOT triggered here — exactly ONE signal is sent, so
/// the FIRST-signal path is what is under test.
///
/// Every signo (`TERM`/`INT`/`HUP`) is covered: the precedence fix must be signo-generic, not
/// SIGTERM-special.
#[test]
fn a_signal_before_any_handshake_exits_128_plus_signo_and_never_hangs() {
    for (sig, expected) in [("TERM", 143), ("INT", 130), ("HUP", 129)] {
        let ws = Workspace::init();
        let mut client = McpClient::spawn(ws.root());

        // NO `initialize` is ever sent — stdin stays OPEN (so the blocking stdin read stays PARKED,
        // which is what made the pre-fix runtime drop block) and the server is parked mid-handshake.
        settle_before_handshake(&mut client);

        let pid = client.child.id();
        send_signal(pid, sig);

        let status = client.wait_for(Duration::from_secs(20));
        assert_eq!(
            status.code(),
            Some(expected),
            "sig {sig}: a signal delivered BEFORE any handshake must exit the conventional \
             128+signo == {expected} (D38 clause 1: the recorded signal takes precedence over the \
             run loop's Err(Cancelled)), and must not hang (D38 clause 2). Child stderr:\n{}",
            client.stderr_snapshot()
        );
    }
}

/// **T3.2.1/D38 AC(3)** — a genuine, NO-signal `Err` from the run loop still exits `1` and still
/// TERMINATES. Its load-bearing role is the OTHER half of the D38 precedence: with no signal
/// recorded, a genuine error must keep its spine §2.3 0–8 code — the fix must not over-reach and
/// swallow unsignalled failures into a signal exit (or into exit 0). It is also a standing no-hang
/// guard on the `Err` path.
///
/// The `Err` is provoked in-protocol, with NO fault injection: rmcp's handshake loop rejects a
/// NOTIFICATION arriving where the `initialize` REQUEST is expected with
/// `Err(ServerInitializeError::ExpectedInitializeRequest)` (`rmcp-1.7.0/src/service/server.rs`) →
/// `McpServerError::Transport` → `CliError::Mcp` → `ErrorCode::InternalError` → exit 1 (D27/AF-4).
///
/// **Measured scope — this case does NOT hang pre-fix, and that is stated rather than assumed.**
/// Verified against both the pre-fix binary and a "restore the blocking runtime drop" mutation: it
/// PASSES under both. Unlike the `Cancelled` window, rmcp reaches this `Err` after having consumed a
/// COMPLETE message and returns without issuing another `receive()`, so no `tokio::io::stdin()`
/// blocking-pool read is left parked and `Runtime::drop` has nothing to block on. The no-hang
/// non-vacuity of D38 clause (2) is therefore carried by
/// `a_signal_before_any_handshake_exits_128_plus_signo_and_never_hangs` (which HANGS under that same
/// mutation), not by this case. Recording the measurement instead of inheriting the plausible-but-
/// unverified "every Err path parks a read" story is the D38 discipline: the defect shipped behind a
/// comment whose causal claim nobody had measured.
///
/// This is ALSO the FR-11 anchor D38 scopes: the unsignalled `Err` path is the ONE `mcp` path that
/// still renders the structured error to stdout (the default error-render format is `json`), so the
/// payload is asserted to be valid JSON carrying `INTERNAL_ERROR`.
#[test]
fn a_no_signal_run_loop_error_exits_1_and_never_hangs() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());

    // A notification where rmcp expects the `initialize` request → a genuine handshake Err.
    client.notify("notifications/initialized", &json!({}));
    // Retain stdout so the FR-11 payload survives (and so no unread pipe can stall the child).
    client.capture_stdout();

    let status = client.wait_for(Duration::from_secs(20));
    assert_eq!(
        status.code(),
        Some(1),
        "an UNSIGNALLED genuine run-loop Err keeps its 0-8 code (InternalError → exit 1, D27/AF-4) \
         and must still terminate (D38 clause 2 — no signal is sent here, so nothing signal-\
         conditional can rescue the runtime drop). Child stderr:\n{}",
        client.stderr_snapshot()
    );

    // FR-11 (scoped to the unsignalled Err path by D38): stdout is still always-valid JSON, and it
    // NAMES the failure — the error is surfaced, never swallowed.
    let stdout = client.stdout_snapshot();
    let payload: Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "FR-11: the unsignalled Err path must render always-valid JSON on stdout, got \
             `{stdout}`: {e}. Child stderr:\n{}",
            client.stderr_snapshot()
        )
    });
    assert_eq!(
        payload["code"], "INTERNAL_ERROR",
        "the run-loop failure surfaces as INTERNAL_ERROR (D27/AF-4), never swallowed: {payload}"
    );
}
