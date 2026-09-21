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
//!     (`a_no_signal_run_loop_error_exits_1_and_never_hangs`) — the OTHER half of the precedence,
//!     so the fix must not swallow unsignalled failures. Since D50 it provokes that `Err` with a
//!     broken pipe on the pre-handshake `ping` reply, the one unsignalled run-loop `Err` still
//!     reachable from the wire; it guards against over-reach and does not carry the no-hang proof.
//! - **T3.2.1 follow-up (b) / D40 — the unsignalled pre-`initialize` client disconnect exits 0** (NOT
//!   the pre-fix exit 1): a bare pre-`initialize` stdin close yields exit 0 via the same barrier
//!   (`a_pre_handshake_client_disconnect_exits_0`, RED against the pre-fix binary — no `error[CODE]`
//!   line at the default level), and its `-vv` peer proves the demoted `ConnectionClosed` disconnect
//!   is still RECORDED at debug (`a_pre_handshake_client_disconnect_records_the_disconnect_at_debug_level`).
//! - **v1.0.1/D50 — the PRE-HANDSHAKE FRAME GATE**: a first frame that is neither `initialize` nor
//!   `ping` no longer kills the server. A premature Request is answered `-32600` on its own id and
//!   dropped, a premature Notification, Response or Error frame is dropped with no reply, and the
//!   handshake still completes in every case (the `D50-E1`..`D50-E6` cells below). The gate opens on
//!   the server's own `InitializeResult` and never waits for `notifications/initialized`.
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

/// **T3.2.1/D38 AC(3), repointed by D50** — a genuine, NO-signal `Err` from the run loop still
/// exits `1` and still TERMINATES. It carries the OTHER half of the D38 precedence. With no signal
/// recorded a genuine error must keep its spine §2.3 0–8 code, so the signal fix must not over-reach
/// and swallow an unsignalled failure into a signal exit or into exit 0. It is also a standing
/// no-hang guard on the `Err` path.
///
/// **The provocation is a BROKEN PIPE on the pre-handshake `ping` reply.** The cell closes its own
/// read end of the child's stdout, then sends a `ping` before any `initialize`. rmcp answers that
/// `ping` itself (`rmcp-1.7.0/src/service/server.rs:175-189`), the reply write hits the closed pipe,
/// and rmcp wraps the `BrokenPipe` as `ServerInitializeError::TransportError` with the context
/// `sending pre-init ping response`, which reaches `McpServerError::Transport` →
/// `ErrorCode::InternalError` → exit 1 (D27/AF-4). Stdin stays OPEN throughout, so EOF is never the
/// reason the child exits and the D40 disconnect carve-out (exit 0) is never entered.
///
/// **That transport failure is the ONE unsignalled run-loop `Err` still reachable from the wire**
/// (D50 clause 11). The gate D50 installs makes `ExpectedInitializeRequest` unreachable from the
/// wire; `ExpectedInitializedNotification` is never constructed; `UnexpectedInitializeResponse` and
/// `InitializeFailed` both need an `initialize` override `UnblockServer` does not declare;
/// `UnsupportedProtocolVersion` needs a `partial_cmp` that returns `None`, and `ProtocolVersion`'s
/// is total; `ConnectionClosed` is D40-delegated to exit 0; and `Cancelled` is the signal path. The
/// gate's own failed reply write cannot carry this witness either, because it returns `None` from
/// `receive()` and lands on that same D40 exit 0.
///
/// **The frame-free-stdout half is gone BY CONSTRUCTION.** The provocation closes the very stream
/// that assertion would read, so no read of it could mean anything here. D48's channel claim keeps
/// its pins on the pre-run-loop routes in `crates/unblock-cli/tests/mcp_stdout_channel.rs`. The
/// exit-1 half and the stderr-payload half are unchanged and are what this cell asserts.
///
/// The no-hang non-vacuity of D38 clause (2) is carried by
/// `a_signal_before_any_handshake_exits_128_plus_signo_and_never_hangs`, which HANGS under the
/// restored-blocking-runtime-drop mutation, and never by this cell.
#[test]
fn a_no_signal_run_loop_error_exits_1_and_never_hangs() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());

    // Close the read end FIRST, so the child's ping reply already has nowhere to go when rmcp
    // writes it. `write_raw_line` is the one send path that touches stdin alone — every other one
    // either reads the response or moves the stdout handle this cell has just dropped.
    client.close_stdout();
    client.write_raw_line(r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{}}"#);

    // Stdin stays open, because EOF would route through D40 to exit 0 and hide the transport error.
    let status = client.wait_for(Duration::from_secs(20));
    assert_eq!(
        status.code(),
        Some(1),
        "an UNSIGNALLED genuine run-loop Err keeps its 0-8 code (InternalError → exit 1, D27/AF-4) \
         and must still terminate (D38 clause 2 — no signal is sent here, so nothing signal-\
         conditional can rescue the runtime drop). Child stderr:\n{}",
        client.stderr_snapshot()
    );

    // The POSITIVE half. Without it this cell stays GREEN under a mutation that deletes the
    // diagnostic entirely, because an exit code alone says nothing about what was reported.
    let stderr = client.stderr_snapshot();
    let payload = common::structured_error_on_stderr(&stderr).unwrap_or_else(|| {
        panic!(
            "D48: the run-loop failure must be REPORTED on stderr, never swallowed. \
             Child stderr:\n{stderr}"
        )
    });
    assert_eq!(
        payload["code"], "INTERNAL_ERROR",
        "the run-loop failure surfaces as INTERNAL_ERROR (D27/AF-4), never swallowed: {payload}"
    );
    assert!(
        payload.get("retryable").is_some(),
        "the FULL structured payload moves (D48), not a degraded `error[CODE]` line: {payload}"
    );
    // ANTI-VACUITY: exit 1 plus INTERNAL_ERROR alone would also fit several other initialize-time
    // variants. D49's dedicated `TransportError` arm is the only one that renders this prefix
    // (`crates/unblock-mcp/src/error.rs`), so reading it pins WHICH failure ran the cell.
    let message = payload["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("a transport error while"),
        "the exit-1 must come from the pre-handshake ping reply failing to write, and no other \
         initialize-time variant renders through that arm: {payload}"
    );
}

// ------------------------------------------------------------------------------------------------
// [v1.0.1/D47] — an un-decodable envelope `id` BEFORE `initialize` no longer kills the server.
//
// This is the half of the defect nobody had connected to it. rmcp's `serve_server_with_ct_inner`
// loops on `expect_next_message`, whose `other =>` arm returns `ExpectedInitializeRequest` for ANY
// non-Request message — a Notification included. So a class frame DELIVERED in the initialize slot
// terminated the server with exit 1, while the same frame ANSWERED-AND-DROPPED is never handed to
// rmcp at all and the connection stays parked waiting for a real `initialize`.
//
// OBSERVATION CHANNEL: `client.seen_lines`, NOT `stdout_snapshot()`. `capture_stdout()` CONSUMES
// the stdout reader and `read_response` panics once it is gone, so it cannot be called before the
// `ping_barrier()`/`initialize()` these cells must perform. A cell written the other way — calling
// `capture_stdout()` up front and reading nothing — would push every line of interest into
// `seen_lines` while `stdout_snapshot()` came back EMPTY, and the two negatives below would then
// pass over an empty collection while proving nothing. Hence the non-emptiness guard that opens
// each of them.
// ------------------------------------------------------------------------------------------------

/// **L-P1** — a class frame before `initialize` no longer kills the server, and stdout stays clean.
///
/// The negatives are the entire pin on the requirement that the pre-handshake fatal variant is
/// gone: no `INTERNAL_ERROR`, and no line carrying the `StructuredError` shape (`code`/`retryable`
/// at the top level) which is NOT JSON-RPC framing and has no business on this channel.
///
/// Mutant: answer-AND-deliver (`continue` replaced by `return Some(message)`), which restores
/// exactly the `ExpectedInitializeRequest` death this cell exists to prove is gone.
#[test]
fn a_class_frame_before_initialize_no_longer_kills_the_server() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());

    // BEFORE the handshake: a duplicated envelope id with EQUAL values.
    client.write_raw_line(r#"{"jsonrpc":"2.0","id":90001,"id":90001,"method":"ping","params":{}}"#);
    // The server must still be parked awaiting `initialize` — and must still answer a ping.
    client.ping_barrier();
    client.initialize();
    client.close_stdin();

    let status = client.wait_for(Duration::from_secs(20));
    assert_eq!(
        status.code(),
        Some(0),
        "a class frame before `initialize` must NOT kill the server (it did before D47: exit 1 \
         with ExpectedInitializeRequest). Child stderr:\n{}",
        client.stderr_snapshot()
    );

    // NON-EMPTINESS FIRST: a negative quantified over an empty collection proves nothing.
    assert!(
        !client.seen_lines.is_empty(),
        "nothing was observed on stdout at all — the negatives below would be vacuous"
    );
    for line in &client.seen_lines {
        let parsed: Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("every stdout line must be JSON: `{line}`: {e}"));
        assert!(
            parsed.get("jsonrpc").is_some(),
            "every stdout line must be JSON-RPC framing (NFR-14): {line}"
        );
        assert!(
            parsed.get("code").is_none() && parsed.get("retryable").is_none(),
            "a StructuredError blob must never reach the JSON-RPC framing channel: {line}"
        );
        assert!(
            !line.contains("INTERNAL_ERROR"),
            "the pre-handshake death is gone, so nothing may report it: {line}"
        );
    }
    assert!(
        client.seen_lines.iter().any(|l| l.contains("-32600")),
        "and the class frame must itself have been ANSWERED: {:?}",
        client.seen_lines
    );
}

/// **L-P2** — the pre-handshake answer carries the RECOVERED id.
///
/// Mutant: passing `None` instead of `Some(id)` on the recovered arm.
#[test]
fn the_pre_handshake_answer_carries_the_recovered_id() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());

    client.write_raw_line(r#"{"jsonrpc":"2.0","id":90001,"id":90001,"method":"ping","params":{}}"#);
    client.ping_barrier();
    client.initialize();
    client.close_stdin();
    let status = client.wait_for(Duration::from_secs(20));
    assert_eq!(status.code(), Some(0));

    assert!(
        !client.seen_lines.is_empty(),
        "nothing was observed on stdout at all — the assertion below would be vacuous"
    );
    let answer = client
        .seen_lines
        .iter()
        .find(|l| l.contains("-32600"))
        .unwrap_or_else(|| panic!("no -32600 was emitted: {:?}", client.seen_lines));
    let parsed: Value = serde_json::from_str(answer).expect("the answer is JSON");
    assert_eq!(
        parsed["id"], 90001,
        "the answer must ride the RECOVERED id — an id-less error is DROPPED by an rmcp client, \
         so only this spelling actually releases a waiting peer: {parsed}"
    );
}

/// **L-N1** — the control: a clean pre-`initialize` request still works, exactly as before D47.
///
/// Mutant: re-keying the predicate onto the `Request` variant, which would answer the barrier ping.
#[test]
fn a_clean_pre_initialize_request_still_works() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());

    client.ping_barrier();
    client.initialize();
    client.close_stdin();

    let status = client.wait_for(Duration::from_secs(20));
    assert_eq!(
        status.code(),
        Some(0),
        "today's behaviour, which D47 must not move. Child stderr:\n{}",
        client.stderr_snapshot()
    );
}

/// **L-N2, inverted by D50** — an id-LESS notification before `initialize` is DROPPED, and the
/// handshake still completes.
///
/// The frame is a genuine JSON-RPC Notification, so D47's class excludes it by decision and D47
/// leaves it untouched. The D50 gate covers all four frame shapes, so this one is dropped with NO
/// reply before rmcp ever sees it, and the connection goes on to a normal handshake and a clean EOF
/// exit 0. JSON-RPC 2.0 section 4.1 is why the drop is silent — a Notification gets no reply.
///
/// It stays PARTLY REDUNDANT with `a_no_signal_run_loop_error_exits_1_and_never_hangs`, which is
/// the point. That cell no longer provokes its `Err` with this frame, because this frame is no
/// longer fatal, and reading the two together shows the same class from both sides.
///
/// Mutant: answering the Notification arm instead of dropping it, which the no-reply assertion
/// below turns red.
#[test]
fn an_id_less_notification_before_initialize_is_dropped_and_the_handshake_still_completes() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());

    client.notify("notifications/initialized", &json!({}));
    // The barrier reads past the dropped frame, so `seen_lines` below holds a COMPLETE record of
    // what the server wrote rather than racing it.
    client.ping_barrier();
    let result = client.initialize();
    client.close_stdin();

    let status = client.wait_for(Duration::from_secs(20));
    assert_eq!(
        status.code(),
        Some(0),
        "the gate drops the frame, so EOF is the only event left and D40 delegates it to a clean \
         teardown. Child stderr:\n{}",
        client.stderr_snapshot()
    );

    // The POSITIVE landing: the handshake the dropped frame used to prevent actually completed.
    assert!(
        result.get("serverInfo").is_some() && result.get("protocolVersion").is_some(),
        "the handshake must complete after the drop, not merely fail to kill the server: {result}"
    );

    // NON-EMPTINESS FIRST: a negative quantified over an empty collection proves nothing.
    assert!(
        !client.seen_lines.is_empty(),
        "nothing was observed on stdout at all — the negative below would be vacuous"
    );
    assert!(
        !client.seen_lines.iter().any(|l| l.contains("-32600")),
        "a Notification gets NO reply (JSON-RPC 2.0 section 4.1), so nothing may answer it: {:?}",
        client.seen_lines
    );
    let stderr = client.stderr_snapshot();
    assert!(
        common::structured_error_on_stderr(&stderr).is_none(),
        "and no failure is reported at all, since none occurred. Child stderr:\n{stderr}"
    );
}

// ------------------------------------------------------------------------------------------------
// [v1.0.1/D50] — the PRE-HANDSHAKE FRAME GATE, end to end over the real binary.
//
// Before the server has answered `initialize`, the transport stack passes only `initialize` and
// `ping` upward. A premature Request is answered `-32600` on its OWN id and dropped; a premature
// Notification, Response or Error frame is dropped with no reply. Every cell here asserts the
// POSITIVE outcome — the answer riding the expected id, or the handshake the frame used to prevent
// actually completing — because "the child did not die" is satisfied by a server that hangs.
//
// The cells that SEARCH stdout read `client.seen_lines` after a sentinel request has round-tripped,
// and no cell here reads `stdout_snapshot()`. `capture_stdout()` consumes the stdout reader, and
// these cells must keep reading responses.
//
// The gate's own unit surface lives in `crates/unblock-mcp/src/pre_handshake.rs`; these cells prove
// the gate is COMPOSED INTO the shipped stdio stack, which no in-module cell can see.
// ------------------------------------------------------------------------------------------------

/// The compile-time `-32600` message, byte-identical to `PRE_HANDSHAKE_REJECTION_MESSAGE`
/// (`crates/unblock-mcp/src/pre_handshake.rs`). A reply that merely carries the CODE would stay
/// green under a mutation that rewrote the text, so the cells read both.
const PRE_HANDSHAKE_REJECTION: &str = "the server has not completed the initialize handshake and accepts only initialize and ping until it has";

/// Find the gate's `-32600` among the lines the server wrote, then assert it whole.
///
/// The caller round-trips a SENTINEL request first, so `seen_lines` already holds every line the
/// server produced and this read is deterministic and timeout-free. Blocking on the answer's own id
/// instead would HANG under the mutation that drops that id, because `read_response` checks its
/// deadline only between lines that actually arrive, and after an id-less reply none does.
fn assert_gate_rejected(client: &McpClient, id: i64) {
    assert!(
        !client.seen_lines.is_empty(),
        "nothing was observed on stdout at all — the search below would be vacuous"
    );
    let answer = client
        .seen_lines
        .iter()
        .find(|line| line.contains("-32600"))
        .unwrap_or_else(|| {
            panic!(
                "the premature frame must be ANSWERED, and nothing answered it: {:?}",
                client.seen_lines
            )
        });
    let reply: Value = serde_json::from_str(answer).expect("the answer is JSON");
    assert_gate_rejection(&reply, id);
}

/// Assert one gate reply — the `-32600`, the message, the id it rides, and the absent `data`.
fn assert_gate_rejection(reply: &Value, id: i64) {
    assert_eq!(
        reply["error"]["code"], -32600,
        "a premature request is answered Invalid Request: {reply}"
    );
    assert_eq!(
        reply["error"]["message"], PRE_HANDSHAKE_REJECTION,
        "and it carries the compile-time constant: {reply}"
    );
    assert_eq!(
        reply["id"], id,
        "riding the frame's OWN id — an id-less error is DROPPED by an rmcp client, so only this \
         spelling releases a waiting peer: {reply}"
    );
    assert!(
        reply["error"].get("data").is_none(),
        "`data` is absent (D50 clause 2), so nothing of the frame is echoed back but the id asserted \
         above: {reply}"
    );
}

/// Complete the handshake WITHOUT sending `notifications/initialized`, returning the result.
///
/// [`McpClient::initialize`] sends that notification for us, which is right for every other cell and
/// wrong for the one that asks what happens in the window before it.
fn initialize_without_the_notification(client: &mut McpClient) -> Value {
    let response = client.request(
        "initialize",
        &json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "unblock-cli-test", "version": "0.0.0"}
        }),
    );
    assert!(
        response.get("error").is_none(),
        "initialize failed: {response}"
    );
    response["result"].clone()
}

/// **D50-E1** — a premature `tools/list` is answered `-32600` on its own id, and the handshake then
/// completes.
///
/// This is the one shape that writes bytes. Answering it is forced by rmcp's client rather than
/// chosen — that client awaits untimed, so a server that dropped silently would turn the old fast
/// death into an unbounded hang.
///
/// This cell goes red on a mutant that passes `None` as the reply id, and on one that drops the
/// gate from the composed stack at `crates/unblock-mcp/src/server.rs`, which leaves nothing to
/// answer at all.
#[test]
fn a_premature_request_before_initialize_is_answered_on_its_own_id() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());

    client.write_raw_line(r#"{"jsonrpc":"2.0","id":90101,"method":"tools/list","params":{}}"#);
    // The barrier reads past the answer, so `seen_lines` holds it before anything is asserted.
    client.ping_barrier();
    assert_gate_rejected(&client, 90101);

    // The POSITIVE landing: the server is still there and still handshakes.
    let result = client.initialize();
    assert!(
        result.get("serverInfo").is_some(),
        "the handshake must complete after the rejection: {result}"
    );
    client.close_stdin();

    let status = client.wait_for(Duration::from_secs(20));
    assert_eq!(
        status.code(),
        Some(0),
        "EOF after a completed handshake exits 0. Child stderr:\n{}",
        client.stderr_snapshot()
    );
}

/// **D50-E2** — a premature RESPONSE frame is dropped with no reply, and the handshake still
/// completes.
///
/// A reply to a reply is meaningless, so the gate writes nothing. `saw_response_for` is the
/// timeout-free probe for that — the barrier and the handshake read every line the server wrote, so
/// asking afterwards reads a COMPLETE record rather than racing one.
///
/// This cell goes red on a mutant that answers the `Response` arm instead of dropping it, on the
/// frame's own id or with no id at all.
#[test]
fn a_premature_response_frame_before_initialize_is_dropped_with_no_reply() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());

    client.write_raw_line(r#"{"jsonrpc":"2.0","id":90102,"result":{}}"#);
    client.ping_barrier();
    let result = client.initialize();
    client.close_stdin();

    let status = client.wait_for(Duration::from_secs(20));
    assert_eq!(
        status.code(),
        Some(0),
        "a premature Response frame must not kill the server. Child stderr:\n{}",
        client.stderr_snapshot()
    );
    assert!(
        result.get("serverInfo").is_some(),
        "and the handshake completes: {result}"
    );

    assert!(
        !client.seen_lines.is_empty(),
        "nothing was observed on stdout at all — the negative below would be vacuous"
    );
    assert!(
        !client.saw_response_for(90102),
        "a Response frame gets NO reply: {:?}",
        client.seen_lines
    );
    // `saw_response_for` looks for a line carrying the id, so an answer sent with no id at all
    // passes it, and this id-less negative is what refuses that reply too.
    assert!(
        !client.seen_lines.iter().any(|l| l.contains("-32600")),
        "and nothing answers it id-lessly either: {:?}",
        client.seen_lines
    );
}

/// **D50-E3** — a premature ERROR frame is dropped with no reply, and the handshake still completes.
///
/// The Error shape is its own cell because the classifier names it in its own match arm, so a
/// mutation that answered only this arm would survive every other cell here.
///
/// This cell goes red on a mutant that answers the `Error` arm instead of dropping it, on the
/// frame's own id or with no id at all.
#[test]
fn a_premature_error_frame_before_initialize_is_dropped_with_no_reply() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());

    client.write_raw_line(r#"{"jsonrpc":"2.0","id":90103,"error":{"code":-1,"message":"nope"}}"#);
    client.ping_barrier();
    let result = client.initialize();
    client.close_stdin();

    let status = client.wait_for(Duration::from_secs(20));
    assert_eq!(
        status.code(),
        Some(0),
        "a premature Error frame must not kill the server. Child stderr:\n{}",
        client.stderr_snapshot()
    );
    assert!(
        result.get("serverInfo").is_some(),
        "and the handshake completes: {result}"
    );

    assert!(
        !client.seen_lines.is_empty(),
        "nothing was observed on stdout at all — the negative below would be vacuous"
    );
    assert!(
        !client.saw_response_for(90103),
        "an Error frame gets NO reply: {:?}",
        client.seen_lines
    );
    // This id-less negative is here for the same reason the Response cell carries one, because an
    // answer with no id survives an id-keyed probe.
    assert!(
        !client.seen_lines.iter().any(|l| l.contains("-32600")),
        "and nothing answers it id-lessly either: {:?}",
        client.seen_lines
    );
}

/// **D50-E4** — a `ping` before `initialize` leaves the gate SHUT, and the handshake still completes.
///
/// rmcp answers a pre-handshake `ping` with `EmptyResult`, which must not open the latch. The second
/// premature frame is what proves it stayed shut — a latch keyed on any Response rather than on the
/// `InitializeResult` variant would pass that frame upward into rmcp's initialize slot and kill the
/// server.
///
/// Mutant: opening the latch on the `ping` reply.
#[test]
fn a_ping_before_initialize_leaves_the_gate_shut_and_the_handshake_still_completes() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());

    client.ping_barrier();

    // STILL pre-handshake, so this frame must STILL be rejected. The handshake below is the
    // sentinel that reads past the answer.
    client.write_raw_line(r#"{"jsonrpc":"2.0","id":90104,"method":"tools/list","params":{}}"#);

    let result = client.initialize();
    assert_gate_rejected(&client, 90104);
    assert!(
        result.get("serverInfo").is_some(),
        "`ping` then `initialize` still completes a handshake: {result}"
    );
    client.close_stdin();

    let status = client.wait_for(Duration::from_secs(20));
    assert_eq!(
        status.code(),
        Some(0),
        "and EOF then exits 0. Child stderr:\n{}",
        client.stderr_snapshot()
    );
}

/// **D50-E5** — a `tools/list` sent AFTER the initialize response but BEFORE
/// `notifications/initialized` is served normally.
///
/// The latch opens on the server's own `InitializeResult` and never waits for the client's
/// `notifications/initialized`. rmcp does not wait for it either, and this transport must not be
/// stricter than the server it decorates.
///
/// Mutant: opening the latch on `notifications/initialized`, which leaves this `tools/list`
/// rejected `-32600` instead of served.
#[test]
fn a_request_before_the_initialized_notification_is_served_normally() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());

    let result = initialize_without_the_notification(&mut client);
    assert!(
        result.get("serverInfo").is_some(),
        "the handshake response arrives first: {result}"
    );

    // The window under test: the response has landed, the notification has NOT been sent.
    let listed = client.request("tools/list", &json!({}));
    assert!(
        listed.get("error").is_none(),
        "a request in the post-response, pre-initialized window is SERVED, never rejected: {listed}"
    );
    let tools = listed["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools/list returns a tool array: {listed}"));
    assert!(
        !tools.is_empty(),
        "and it really served the request: {listed}"
    );

    client.notify("notifications/initialized", &json!({}));
    client.close_stdin();
    let status = client.wait_for(Duration::from_secs(20));
    assert_eq!(
        status.code(),
        Some(0),
        "EOF after a completed handshake exits 0. Child stderr:\n{}",
        client.stderr_snapshot()
    );
}

/// **D50-E6** — a frame spelled `"method":"initialize"` whose `params` do not type is answered
/// `-32600` and dropped.
///
/// `ClientRequest` is `#[serde(untagged)]` and ends in `CustomRequest`, so this frame decodes as
/// `CustomRequest` rather than `InitializeRequest`. A gate keyed on `ClientRequest::method()` would
/// read the string `initialize` and pass it upward into the very death the gate exists to remove.
/// This cell is what tells a VARIANT-matched classifier from a method-string one.
///
/// Mutant: keying the pass arm on `ClientRequest::method()`.
#[test]
fn a_method_initialize_frame_with_untypeable_params_is_rejected_over_stdio() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());

    // `capabilities` and `clientInfo` are missing, so the typed variant fails and the untagged
    // union falls through to `CustomRequest`. The barrier reads past the answer.
    client.write_raw_line(
        r#"{"jsonrpc":"2.0","id":90105,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
    );
    client.ping_barrier();
    assert_gate_rejected(&client, 90105);

    // The POSITIVE landing: a well-formed `initialize` still gets through.
    let result = client.initialize();
    assert!(
        result.get("serverInfo").is_some(),
        "a real initialize still completes the handshake: {result}"
    );
    client.close_stdin();

    let status = client.wait_for(Duration::from_secs(20));
    assert_eq!(
        status.code(),
        Some(0),
        "and EOF then exits 0. Child stderr:\n{}",
        client.stderr_snapshot()
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
