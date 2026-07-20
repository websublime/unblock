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
//! - **T3.2.1/D38 — the DETERMINISTIC shutdown cases** (unlike the race-robust real-signal e2e
//!   cases in `shutdown_failure_injection.rs`, these are deterministic rather than invariant-only;
//!   every one of them is barrier-driven, with NO sleeps — see `McpClient::ping_barrier`):
//!   - a signal delivered **BEFORE any client handshake** exits exactly `128+signo` and never hangs
//!     (`a_signal_before_any_handshake_exits_128_plus_signo_and_never_hangs` — D38 clause 1+2, the
//!     window where rmcp returns `Err(Cancelled)` rather than `Ok`). **This is the load-bearing
//!     regression case: it HANGS against the pre-fix binary**, and it is RED under BOTH D38
//!     mutations — removing the signal-precedence guard (→ exit 1) and restoring the blocking
//!     runtime drop (→ hang). It also pins the D38 labelling clause's quiet half (no `error[CODE]`
//!     line for a routine signal);
//!   - its `-vv` peer proves the demoted diagnostic is still RECORDED
//!     (`a_pre_handshake_signal_records_the_cancellation_at_debug_level`) — demoted, never dropped;
//!   - `shutdown::install()` precedes the workspace open
//!     (`shutdown_signal_handling_is_installed_before_the_workspace_opens`, by marker ORDER) and a
//!     signal racing that open still exits cleanly
//!     (`a_signal_during_the_workspace_open_exits_128_plus_signo_cleanly`) — FR-17 "unwinds
//!     cleanly": no hard kill mid-`migrate()`;
//!   - an **unsignalled** genuine run-loop `Err` still exits `1` and never hangs
//!     (`a_no_signal_run_loop_error_exits_1_and_never_hangs`) — the OTHER half of the precedence:
//!     the fix must not swallow unsignalled failures. It passes pre-fix (see its docs for the
//!     measured reason); it guards against over-reach, and does not carry the no-hang proof.
//! - **T3.2.1 follow-up (b) / D40 — the unsignalled pre-`initialize` client disconnect exits 0** (NOT
//!   the pre-fix exit 1): a bare pre-`initialize` stdin close yields exit 0 via the same barrier
//!   (`a_pre_handshake_client_disconnect_exits_0`, RED against the pre-fix binary — no `error[CODE]`
//!   line at the default level), and its `-vv` peer proves the demoted `ConnectionClosed` disconnect
//!   is still RECORDED at debug (`a_pre_handshake_client_disconnect_records_the_disconnect_at_debug_level`).
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

use std::time::Duration;

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

/// D39 (3) — startup VISIBILITY: `unblock mcp` reports the bound workspace dir AND the winning
/// discovery tier on STDERR at startup, ALWAYS (no `-v` needed). Here the workspace is found by the
/// cwd walk-up (the client spawns with `cwd = ws.root()`, no `--dir`), so the line names that tier.
/// The line must NOT read as a fault (`error[`) — a routine binding is not an error.
#[test]
fn startup_reports_the_bound_workspace_on_stderr() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());
    // The startup line is emitted before any handshake — wait for it on the drained child stderr.
    let stderr = client.wait_for_stderr("workspace bound to", Duration::from_secs(20));
    assert!(
        stderr.contains("unblock: workspace bound to"),
        "the D39 startup line must name the bound dir: {stderr}"
    );
    assert!(
        stderr.contains("(via walk-up from cwd)"),
        "and the winning discovery tier (walk-up here): {stderr}"
    );
    assert!(
        !stderr.contains("error["),
        "a routine binding must not read as a fault: {stderr}"
    );

    // Complete the handshake before EOF so the clean exit-0 path is exercised (a pre-`initialize`
    // EOF is the separate exit-1 case, not what this test is about).
    client.initialize();
    client.close_stdin();
    let status = client.wait_for(Duration::from_secs(20));
    assert_eq!(
        status.code(),
        Some(0),
        "EOF after a completed handshake drives a clean exit 0"
    );
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
///
/// Determinism comes from `McpClient::ping_barrier` (a pre-`initialize` `ping` round-trip), NOT a
/// sleep — see its docs for why the previous 300ms settle window was measurably flaky.
#[test]
fn a_signal_before_any_handshake_exits_128_plus_signo_and_never_hangs() {
    for (sig, expected) in [("TERM", 143), ("INT", 130), ("HUP", 129)] {
        let ws = Workspace::init();
        let mut client = McpClient::spawn(ws.root());

        // NO `initialize` is ever sent — stdin stays OPEN (so the blocking stdin read stays PARKED,
        // which is what made the pre-fix runtime drop block) and the server is parked mid-handshake.
        client.ping_barrier();

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

        // D38 labelling clause: a ROUTINE signal must not blame unblock for obeying. At the DEFAULT
        // level the demoted cancellation diagnostic is filtered out entirely, so a clean SIGTERM is
        // SILENT. Reverting the demotion (routing the cancellation back to the stderr line) → RED.
        let stderr = client.stderr_snapshot();
        assert!(
            !stderr.contains("error["),
            "sig {sig}: a routine pre-handshake signal must print NO `error[CODE]` line — the \
             cancellation is the cooperative shutdown SUCCEEDING (D38 labelling clause), not a \
             fault. Child stderr:\n{stderr}"
        );
    }
}

/// **T3.2.1/D38 labelling clause (Miguel, 2026-07-17) — the demoted diagnostic is DEMOTED, not
/// DROPPED.** The peer of the assertion above: at `-vv` the post-signal cancellation MUST still be
/// recorded (via `tracing::debug!`), naming the underlying rmcp outcome.
///
/// Together the two pin both halves of "never swallowed, never shouted": gutting the `Debug` arm of
/// `commands/mcp.rs::report` (or the routing that reaches it) turns THIS red, while routing the
/// cancellation back to `error[CODE]` turns its default-level peer red. Neither can be satisfied by
/// deleting the diagnostic.
#[test]
fn a_pre_handshake_signal_records_the_cancellation_at_debug_level() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn_verbose(ws.root());
    client.ping_barrier();

    send_signal(client.child.id(), "TERM");
    let status = client.wait_for(Duration::from_secs(20));
    assert_eq!(status.code(), Some(143), "still the conventional 128+15");

    let stderr = client.stderr_snapshot();
    assert!(
        stderr.contains("cooperative shutdown"),
        "the demoted cancellation must still be RECORDED at debug level (D38: reported, never \
         swallowed — only quieter). Child stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("Cancelled"),
        "and it must NAME the rmcp outcome it demoted, so the shutdown stays diagnosable. Child \
         stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("error["),
        "even at -vv it is a debug record, NOT an `error[CODE]` line. Child stderr:\n{stderr}"
    );
}

/// **T3.2.1/D38 — `shutdown::install()` runs BEFORE the workspace opens (FR-17 "unwinds cleanly").**
///
/// `open_with_storage_with_cli` does discovery + `LibsqlStorage::open_local` (taking the D31
/// `.write.lock`) + `migrate()`. With the handler installed AFTER it, a signal anywhere in that
/// window hit the DEFAULT disposition and hard-killed the process MID-MIGRATE — an integrity risk of
/// exactly the class T3.2 exists to close, and the mechanical cause of the old settle-window flake
/// (the child was still in this phase when the sleep expired).
///
/// Pinned by ORDER rather than by racing a signal into a millisecond-wide window: the two markers
/// are emitted at the two sites, so swapping the calls back makes them appear in the opposite order
/// and turns this RED. (The `ping_barrier` cases cannot catch that swap — `install()` preceded
/// `run_mcp_server` both before and after this fix; only the CONFIG open moved across it.)
#[test]
fn shutdown_signal_handling_is_installed_before_the_workspace_opens() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn_verbose(ws.root());

    // Both markers precede the run loop, so wait on the LATER one; the snapshot then holds both.
    let stderr = client.wait_for_stderr("mcp: workspace opened", Duration::from_secs(20));

    let installed = stderr
        .find("mcp: shutdown signal handling installed")
        .unwrap_or_else(|| panic!("the install marker must be emitted. Child stderr:\n{stderr}"));
    let opened = stderr
        .find("mcp: workspace opened")
        .unwrap_or_else(|| panic!("the open marker must be emitted. Child stderr:\n{stderr}"));
    assert!(
        installed < opened,
        "FR-17: signal handling must be installed BEFORE the workspace open (discovery + \
         open_local + migrate), so a signal in that window is RECORDED and migrate() is never \
         hard-killed mid-flight. Child stderr:\n{stderr}"
    );

    // Shut the child down through a COMPLETED handshake (the clean exit-0 path). NOTE: closing stdin
    // BEFORE `initialize` (the unsignalled pre-handshake disconnect) ALSO exits 0 now — rmcp reports
    // it as `ConnectionClosed(_)` and D40 (T3.2.1 follow-up (b)) intercepts it in `resolve_mcp_exit`,
    // delegating the exit code to the clean teardown → exit 0 (unifying with the post-handshake EOF).
    // The deferred "GA CLI-surface question (D35)" of whether that NORMAL event should exit 0 is now
    // ANSWERED by D40. This case deliberately keeps a COMPLETED handshake to keep proving the
    // POST-handshake path; the bare pre-`initialize` disconnect → exit 0 is proven by
    // `a_pre_handshake_client_disconnect_exits_0` below.
    client.initialize();
    client.close_stdin();
    let status = client.wait_for(Duration::from_secs(20));
    assert_eq!(status.code(), Some(0), "EOF still drives a clean exit 0");
}

/// **T3.2.1/D38 — a signal arriving DURING the workspace open exits cleanly, never hard-killed.**
///
/// The semantic peer of the ordering test: it signals as early as the fix makes safe (the instant
/// signal handling is armed, while discovery/`open_local`/`migrate()` may still be running) and
/// demands a CLEAN `128+signo`.
///
/// `Some(143)` — not `None` — is the whole point: `None` means the process died BY the signal
/// (WIFSIGNALED, the default disposition) rather than unwinding, which is what FR-17 forbids and
/// what a hard kill mid-`migrate()` looks like. It also exercises the "token already cancelled
/// before the run loop starts" path: `migrate()` is NOT interrupted, it completes, and rmcp's
/// `select!` then returns `Err(Cancelled)` at once → the normal teardown → 143. The open SUCCEEDS,
/// so the D38 scope boundary (a pre-run-loop `Err` keeps its own 0–8 code) is not engaged here.
#[test]
fn a_signal_during_the_workspace_open_exits_128_plus_signo_cleanly() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn_verbose(ws.root());

    // The earliest moment a signal is guaranteed to be RECORDED rather than fatal.
    client.wait_for_stderr(
        "mcp: shutdown signal handling installed",
        Duration::from_secs(20),
    );
    send_signal(client.child.id(), "TERM");

    let status = client.wait_for(Duration::from_secs(20));
    assert_eq!(
        status.code(),
        Some(143),
        "a SIGTERM racing the workspace open must still UNWIND to the conventional 128+15 — \
         `None` here would mean the child was hard-killed by the default disposition (possibly \
         mid-migrate), and `Some(1)` would mean the signal lost to the cancellation error. Child \
         stderr:\n{}",
        client.stderr_snapshot()
    );
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

// ------------------------------------------------------------------------------------------------
// T3.2.1 follow-up (b) / D40 — the unsignalled pre-`initialize` client disconnect exits 0.
// ------------------------------------------------------------------------------------------------

/// **D40 (T3.2.1 follow-up (b)) — a bare pre-`initialize` client disconnect exits 0; it FAILS (exit 1)
/// against the pre-fix binary.** A client that connects, proves the server is up (a pre-`initialize`
/// `ping`), then closes stdin WITHOUT ever sending `initialize` is a routine lifecycle event, not a
/// fault: rmcp returns `Err(ServerInitializeError::ConnectionClosed(_))` (its `expect_next_message`
/// maps the transport's `receive() == None` to `ConnectionClosed`), and D40's `resolve_mcp_exit`
/// intercepts it (NO signal recorded) and DELEGATES the exit code to the clean teardown → exit 0,
/// unifying with the already-blessed post-handshake EOF (`initialize_handshake_advertises_unblock_identity`).
///
/// Determinism comes from `McpClient::ping_barrier` (a pre-`initialize` `ping` round-trip proving the
/// stdin read is PARKED mid-handshake), NOT a sleep — a sleep-based test would be vacuous. At the
/// DEFAULT level the demoted disconnect is filtered out, so stderr carries NO `error[CODE]` line (a
/// routine disconnect is not a fault). Routing the disconnect back to the stderr line turns the
/// `error[` assertion RED; the blanket-`Ok(None)` vs teardown-delegation distinction is pinned by the
/// `resolve_mcp_exit` unit tests in `commands/mcp.rs`.
#[test]
fn a_pre_handshake_client_disconnect_exits_0() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());

    // Prove the server is parked awaiting `initialize` (stdin read parked, handshake incomplete)
    // WITHOUT completing the handshake, then close stdin so rmcp reads EOF pre-`initialize`.
    client.ping_barrier();
    client.close_stdin();

    let status = client.wait_for(Duration::from_secs(20));
    assert_eq!(
        status.code(),
        Some(0),
        "a pre-`initialize` client disconnect (no signal) must exit 0 (D40 — the unsignalled \
         ConnectionClosed is intercepted and the code delegated to the clean teardown), NOT the \
         pre-fix exit 1. Child stderr:\n{}",
        client.stderr_snapshot()
    );

    let stderr = client.stderr_snapshot();
    assert!(
        !stderr.contains("error["),
        "a routine pre-`initialize` disconnect must print NO `error[CODE]` line — it is the \
         cooperative shutdown, not a fault (D40, demoted to -vv debug). Child stderr:\n{stderr}"
    );
}

/// **D40 peer — the demoted disconnect is DEMOTED, not DROPPED.** The peer of the assertion above: at
/// `-vv` the unsignalled pre-`initialize` disconnect MUST still be RECORDED (via `tracing::debug!`),
/// naming the underlying rmcp `ConnectionClosed` outcome. Together the two pin both halves of "never
/// swallowed, never shouted": gutting the `Debug` arm of `commands/mcp.rs::report` (or the routing/
/// reporting that reaches it) turns THIS red, while routing the disconnect back to `error[CODE]` turns
/// its default-level peer red.
#[test]
fn a_pre_handshake_client_disconnect_records_the_disconnect_at_debug_level() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn_verbose(ws.root());
    client.ping_barrier();
    client.close_stdin();

    let status = client.wait_for(Duration::from_secs(20));
    assert_eq!(status.code(), Some(0), "still a clean exit 0 (D40)");

    let stderr = client.stderr_snapshot();
    assert!(
        stderr.contains("cooperative shutdown"),
        "the demoted disconnect must still be RECORDED at debug level (D40: reported, never \
         swallowed — only quieter). Child stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("ConnectionClosed"),
        "and it must NAME the rmcp outcome it demoted, so the shutdown stays diagnosable. Child \
         stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("error["),
        "even at -vv it is a debug record, NOT an `error[CODE]` line. Child stderr:\n{stderr}"
    );
}
