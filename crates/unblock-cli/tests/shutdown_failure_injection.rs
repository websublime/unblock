//! T3.2 — cooperative-shutdown adversarial failure-injection over a REAL `unblock mcp` process
//! (FR-17/NFR-5). The AC (impl-plan T3.2 / PRD FR-17) has four clauses to PROVE: (1) SIGTERM mid-write
//! leaves no WAL corruption; (2) the in-flight write fully commits or fully rolls back (never
//! partial); (3) exit is `128+signo`; (4) a second signal escalates to an async-signal-safe exit.
//!
//! The cases here are **real-signal e2e** and therefore INVARIANT-ONLY / race-robust — they are
//! e2e-indistinguishable between the cooperative and escalation exit paths (both `128+signo`, no
//! hang) and are NEVER load-bearing for non-vacuity on their own. The DETERMINISTIC anchors that carry
//! the real proof are `unblock-engine/tests/shutdown_drain_barrier.rs` (C4 — the permit-drain
//! mechanism itself, no signal) and `unblock-storage/tests/shutdown_abandoned_tx.rs` (C5 — SIGKILL an
//! abandoned raw-libsql tx, no signal handler). Every case here funnels its post-shutdown assertions
//! through `common::reopen_and_check` (fresh reopen via the SAME config facade the CLI uses, so
//! `open_local`'s WAL-recovery open runs — never a raw file read).
//!
//! - **C1** — commit-durability (deterministic ON COMMIT): awaits the `create_bulk` success response
//!   (tx committed, permit released) BEFORE the signal. Also the within-cap VALIDITY CONTROL for
//!   C2/C6: the SAME `N = 100` title-only bulk-markdown doc (`common::bulk_markdown`, pinned <=
//!   `Quotas::max_batch`) is proven acceptable here, so C2/C6's `count ∈ {0, N}` can never pass
//!   vacuously via a validation-rejected (non-signal) doc.
//! - **C2** — mid-write atomicity (~5 bounded rounds; a `#[ignore]`d ~80-round soak): a SIGTERM raced
//!   against an UNREAD `create_bulk` response (background-drained, never read synchronously — a
//!   signal-case guardrail). `count ∈ {0, N}`, never partial, and the exit is exactly `143`.
//!   **T3.2.1/D38 — corrected:** `6e5b72b` had widened this to accept exit 1 on the (FALSE) rationale
//!   that the signal races a *blocked stdout pipe write*; `common::write_without_reading`
//!   background-drains stdout to EOF precisely so the pipe can never block, so no such race exists.
//!   The accepted exit 1 was the D38 defect itself. Re-tightened to `assert_eq!(Some(143))`.
//! - **C3** — signo-generic exit codes: SIGINT → 130, SIGHUP → 129 (SIGTERM → 143 is covered by C1 /
//!   `mcp_lifecycle.rs::sigterm_drives_clean_shutdown_with_exit_143`).
//! - **C6** — second-signal escalation (race-based, invariant-only): SIGTERM then an ESRCH-tolerant
//!   SIGINT ~1–2ms later. Asserts NO HANG and `code() ∈ {143, 130}` — NOT `Some(143)` alone: the two
//!   signals sent that close together can both be pending, and the lower signo (INT) can win delivery,
//!   so a CORRECT system may legitimately record 130. Both are `128+signo`, so this is a legitimate
//!   two-signal DELIVERY race and NOT to be "tightened" to an `assert_eq!` (that would be a false
//!   pin); the standing prohibition — no `||`-widened acceptance of a NON-signal code — is untouched
//!   by it. This case is what CAUGHT D38 (it flaked `Some(1)` in CI under load).
//!   **Accepted gap (T3.2.1/D38 — EXTENDED/SHARPENED, not falsified):** the claim below stays TRUE
//!   before and after the fix — C6 does not prove the escalation LINE itself (`shutdown.rs`'s
//!   second-signal `process::exit`), which is e2e-indistinguishable from the cooperative exit (a
//!   100-row bulk drains in ms) and is unit-covered by
//!   `shutdown.rs::first_signal_semantics_fire_both_sinks`. What it OMITTED is that until T3.2.1 that
//!   branch was the SOLE exit on the PRE-handshake signal path — i.e. load-bearing (the only thing
//!   rescuing the D38 hang), not the mere backstop the wording implied. That is why the gap was worth
//!   accepting for the *line* but never for the *behaviour*. Post-D38 it is a genuine backstop, and
//!   the first-signal pre-handshake exit is proven deterministically by
//!   `mcp_lifecycle.rs::a_signal_before_any_handshake_exits_128_plus_signo_and_never_hangs`.
//! - **C-doctor** — after a clean C1-style round, `unblock doctor --output json` reports a clean
//!   integrity header (reusing `migrate_doctor.rs`'s promoted `json_report`/`detail` helpers — no
//!   invented report shape).
//! - **C-neg** (gated `#[ignore]`) — a negative CONTROL proving `reopen_and_check`'s two oracle
//!   channels (integrity / count) are EACH independently capable of catching a real defect. This does
//!   NOT test anything under test being broken — it proves the harness itself is non-vacuous.
//!
//! Unix-only (`#![cfg(unix)]`) — the SIGTERM/exit-`128+signo` contract is a unix construct; Windows
//! `unblock mcp` is a no-op EOF path (NFR-11).
#![cfg(unix)]

mod common;

use std::time::Duration;

use common::{
    McpClient, Workspace, bulk_markdown, detail, id_set, json_report, reopen_and_check,
    send_signal, send_signal_tolerant,
};
use serde_json::{Value, json};

/// The pinned T3.2 bulk size: `<= Quotas::max_batch` (100, mcp/options.rs) so the doc is ACCEPTED
/// (101+ is rejected pre-mint) — the SAME document shape is reused across C1/C2/C6.
const N: usize = 100;

/// A cheap pseudo-jitter in `0..=max_ms` (no new `rand` dependency): varies the signal timing across
/// rounds/runs so different windows of the write get exercised, without pretending to be a
/// cryptographic RNG (a test-timing jitter has no such requirement).
fn jitter_ms(max_ms: u64) -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    u64::from(nanos) % (max_ms + 1)
}

// ------------------------------------------------------------------------------------------------
// C1 — commit-durability (deterministic on commit; the within-cap validity control for C2/C6).
// ------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn c1_sigterm_after_a_committed_bulk_is_durable_and_stdout_stays_json_only() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());
    client.initialize();

    let markdown = bulk_markdown(N);
    let (err, created) = client.call_tool(
        "issue",
        &json!({"action": "create_bulk", "markdown": markdown}),
    );
    assert!(
        !err,
        "the N={N} bulk must be accepted (within Quotas::max_batch): {created}"
    );
    assert_eq!(
        id_set(&created).len(),
        N,
        "C1 is the within-cap validity control: the doc must mint exactly N distinct ids"
    );

    let pid = client.child.id();
    send_signal(pid, "TERM");
    let status = client.wait_for(Duration::from_secs(20));
    assert_eq!(
        status.code(),
        Some(143),
        "SIGTERM after a committed write yields the conventional 128+15 exit. Child stderr:\n{}",
        client.stderr_snapshot()
    );

    // stdout-only-JSON (NFR-14): safe to assert here — C1 read every response synchronously, so no
    // line was ever left unread in the pipe.
    for line in client.seen_lines.clone() {
        serde_json::from_str::<Value>(&line)
            .unwrap_or_else(|e| panic!("stdout line not JSON framing: {line}: {e}"));
    }

    let (problems, count) = reopen_and_check(ws.root()).await;
    assert!(
        problems.is_empty(),
        "no WAL corruption after a commit-then-SIGTERM: {problems:?}"
    );
    assert_eq!(
        count, N,
        "the committed bulk is durable across the cooperative shutdown"
    );
}

// ------------------------------------------------------------------------------------------------
// C2 — mid-write atomicity (race-robust, bounded rounds; a #[ignore]d soak).
// ------------------------------------------------------------------------------------------------

/// One mid-write SIGTERM round: fresh workspace, an UNREAD `create_bulk` write raced against a
/// jittered SIGTERM. Never asserts "mid-tx was hit" — only the atomicity invariant.
async fn run_c2_round(round: usize) {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());
    client.initialize();

    let markdown = bulk_markdown(N);
    client.write_without_reading(
        "tools/call",
        &json!({"name": "issue", "arguments": {"action": "create_bulk", "markdown": markdown}}),
    );

    std::thread::sleep(Duration::from_millis(jitter_ms(15)));
    let pid = client.child.id();
    send_signal(pid, "TERM");
    let status = client.wait_for(Duration::from_secs(20));
    // A mid-write SIGTERM is DETERMINISTIC in its exit code: the recorded signal takes precedence over
    // whatever the MCP server run loop returned (D38 clause 1), so it is exactly 128+15 == 143.
    //
    // T3.2.1 corpus correction — this comment previously justified an `|| Some(1)` widening with "the
    // signal races a BLOCKED PIPE WRITE against the cooperative shutdown, and the blocked write errors
    // during cancellation". That rationale was FALSE: `common::write_without_reading`
    // (`tests/common/mod.rs`) deliberately spawns a background thread that drains stdout to EOF
    // precisely so an unread response can NEVER fill the OS pipe buffer or stall the child's write.
    // The pipe cannot block, so no such race exists. The exit 1 that widening accepted was really the
    // D38 defect (a cancel landing during/near the rmcp handshake → `Err(Cancelled)` → an early return
    // PAST the signal guard → `InternalError`), i.e. a wrong causal story that hid a GA-blocking hang
    // behind a green suite. Re-tightened to the exact code, never widened (CLAUDE.md: widening an
    // assertion as the fix is the prohibited simplification).
    assert_eq!(
        status.code(),
        Some(143),
        "round {round}: a mid-write SIGTERM exits exactly 128+15 == 143 — the recorded signal takes \
         precedence over the run loop's return (D38). Child stderr:\n{}",
        client.stderr_snapshot()
    );

    let (problems, count) = reopen_and_check(ws.root()).await;
    assert!(
        problems.is_empty(),
        "round {round}: no WAL corruption, got {problems:?}"
    );
    assert!(
        count == 0 || count == N,
        "round {round}: count must be 0 or N (never partial), got {count}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn c2_mid_write_sigterm_never_leaves_a_partial_batch() {
    const ROUNDS: usize = 5;
    for round in 0..ROUNDS {
        run_c2_round(round).await;
    }
}

/// The gated ~80-round soak (spec: ~50–100 rounds) — run explicitly with `-- --ignored`. The bounded
/// 5-round leg above stays in the default job; this is NOT how the atomicity invariant is hidden if
/// it flakes (a flake here would be hardened, not `#[ignore]`d away, per the T3.2 gate discipline).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "T3.2 C2 soak: ~80-round mid-write SIGTERM atomicity stress; run with -- --ignored"]
async fn c2_soak_mid_write_sigterm_never_leaves_a_partial_batch() {
    const SOAK_ROUNDS: usize = 80;
    for round in 0..SOAK_ROUNDS {
        run_c2_round(round).await;
    }
}

// ------------------------------------------------------------------------------------------------
// C3 — signo-generic exit codes (parametrized; TERM->143 is covered by C1/the existing baseline).
// ------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn c3_signo_generic_exit_codes_and_clean_integrity() {
    for (sig, code) in [("INT", 130), ("HUP", 129)] {
        let ws = Workspace::init();
        let mut client = McpClient::spawn(ws.root());
        client.initialize();
        let (err, _ready) = client.call_tool("query", &json!({"kind": "ready"}));
        assert!(!err, "sig {sig}: a call works before the signal");

        let pid = client.child.id();
        send_signal(pid, sig);
        let status = client.wait_for(Duration::from_secs(20));
        assert_eq!(
            status.code(),
            Some(code),
            "sig {sig}: exit must be the conventional 128+signo == {code}. Child stderr:\n{}",
            client.stderr_snapshot()
        );

        let (problems, _count) = reopen_and_check(ws.root()).await;
        assert!(
            problems.is_empty(),
            "sig {sig}: no WAL corruption, got {problems:?}"
        );
    }
}

// ------------------------------------------------------------------------------------------------
// C6 — second-signal escalation (race-based, invariant-only).
// ------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn c6_second_signal_escalation_never_hangs_and_keeps_a_valid_exit_code() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());
    client.initialize();

    let markdown = bulk_markdown(N);
    client.write_without_reading(
        "tools/call",
        &json!({"name": "issue", "arguments": {"action": "create_bulk", "markdown": markdown}}),
    );

    std::thread::sleep(Duration::from_millis(jitter_ms(15)));
    let pid = client.child.id();
    // Signal #1 — strict (the process must be alive to receive it).
    send_signal(pid, "TERM");
    std::thread::sleep(Duration::from_millis(2));
    // Signal #2 — ESRCH-tolerant: the child may already be mid-exit (or exited) from #1.
    send_signal_tolerant(pid, "INT");

    // A generous deadline proves there is NO HANG regardless of which signal's code got recorded.
    let status = client.wait_for(Duration::from_secs(20));
    let code = status.code();
    assert!(
        matches!(code, Some(143 | 130)),
        "the second signal must not corrupt the recorded exit code — both 143 (TERM) and 130 (INT) \
         are valid 128+signo outcomes of this race, got {code:?}. Child stderr:\n{}",
        client.stderr_snapshot()
    );

    // Hard-exit path guardrail: assert the exit code + the reopen invariant ONLY — NO stdout-only-JSON
    // assertion (the response was never read here, and `process::exit` may flush a partial line).
    let (problems, count) = reopen_and_check(ws.root()).await;
    assert!(
        problems.is_empty(),
        "no WAL corruption across the escalation race, got {problems:?}"
    );
    assert!(
        count == 0 || count == N,
        "count must be 0 or N (never partial) across the escalation race, got {count}"
    );
}

// ------------------------------------------------------------------------------------------------
// C-doctor — secondary integrity cross-check (default job).
// ------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn c_doctor_reports_clean_integrity_after_a_shutdown_round() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());
    client.initialize();

    let markdown = bulk_markdown(N);
    let (err, created) = client.call_tool(
        "issue",
        &json!({"action": "create_bulk", "markdown": markdown}),
    );
    assert!(!err, "the bulk must be accepted: {created}");

    let pid = client.child.id();
    send_signal(pid, "TERM");
    let status = client.wait_for(Duration::from_secs(20));
    assert_eq!(
        status.code(),
        Some(143),
        "the round must end on the conventional 128+15 exit. Child stderr:\n{}",
        client.stderr_snapshot()
    );
    drop(client);

    // Reuses `migrate_doctor.rs`'s promoted `json_report`/`detail` helpers — asserts exit 0 (inside
    // `json_report`) + the REAL `DoctorReport` integrity finding (no invented shape).
    let report = json_report(&ws, &["doctor", "--output", "json"]);
    assert_eq!(
        detail(&report, "integrity"),
        Some("ok"),
        "doctor must report a clean integrity header after the round: {report}"
    );
}

// ------------------------------------------------------------------------------------------------
// C-neg — negative control (gated `#[ignore]`): proves the shared oracle can actually FAIL.
// ------------------------------------------------------------------------------------------------

#[tokio::test]
#[ignore = "T3.2 C-neg: gated negative control proving reopen_and_check's two oracle channels are \
            each independently capable of catching a real defect (harness non-vacuity, not a SUT bug)"]
async fn c_neg_reopen_and_check_oracle_channels_can_both_fail() {
    // (1) Integrity channel: corrupt the DB (mirrors `migrate_doctor.rs::corrupt_db` /
    // `doctor_on_a_corrupt_db_exits_2`). NOTE (spec/code deviation, documented — see the PR body):
    // this exact page-overwrite corruption is severe enough that `integrity_check()` surfaces a hard
    // `StorageError` ("database disk image is malformed") rather than an `Ok` non-empty `Vec<String>`
    // — the SAME real behaviour `migrate_doctor.rs`'s own module doc admits ("the non-empty-
    // integrity_check → exit-2 mapping is unit-tested [with a synthetic vector], NOT via this
    // integration test"). `reopen_and_check` (used unchanged by every OTHER case, which never expects
    // a read failure) `.expect()`s the call, so it is not reused for this leg; a hard error is
    // equally a "the integrity channel detected the corruption" outcome, so both are accepted here.
    let ws_corrupt = Workspace::init();
    corrupt_db(&ws_corrupt.db_path());
    let detected = integrity_channel_detects_a_problem(ws_corrupt.root()).await;
    assert!(
        detected,
        "the integrity channel must be able to detect corruption (non-vacuity)"
    );

    // (2) Count channel: stage an out-of-band row directly via raw libsql (bypassing the engine) —
    // a stand-in "unexpected/partial write" a genuine `count == 0` expectation must be sensitive to.
    let ws_partial = Workspace::init();
    seed_one_row_out_of_band(&ws_partial.db_path()).await;
    let (problems, count) = reopen_and_check(ws_partial.root()).await;
    assert!(problems.is_empty(), "a mere extra row is not a corruption");
    assert_ne!(
        count, 0,
        "the count channel must be able to detect an unexpected row (non-vacuity)"
    );
}

/// Like `common::reopen_and_check`'s integrity leg, but TOLERATES a hard `integrity_check()` failure
/// (a `StorageError`, e.g. "database disk image is malformed") in addition to an `Ok` non-empty
/// `Vec<String>` — both are legitimate "the integrity channel detected the problem" outcomes. Used
/// ONLY by C-neg's integrity-channel leg (see its call site for why `common::reopen_and_check` itself
/// is not reused here: every OTHER case funnels through it expecting `integrity_check` to never fail).
async fn integrity_channel_detects_a_problem(root: &std::path::Path) -> bool {
    let overrides = unblock_config::CliOverrides::new().with_dir(root.join(".unblock"));
    let ctx = unblock_config::open_with_storage_with_cli(&overrides)
        .await
        .expect("reopen workspace via the config facade");
    let session = unblock_engine::Session::open(ctx, unblock_engine::SessionConfig::default())
        .await
        .expect("reopen session");
    match session.integrity_check().await {
        Ok(problems) => !problems.is_empty(),
        Err(_) => true,
    }
}

/// Overwrite a deep region of the `SQLite` file with garbage (mirrors
/// `migrate_doctor.rs::corrupt_db`) — a deterministic page-level corruption `PRAGMA integrity_check`
/// surfaces.
fn corrupt_db(db: &std::path::Path) {
    use std::io::{Seek, SeekFrom, Write};
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .open(db)
        .expect("open db for corruption");
    f.seek(SeekFrom::Start(4096)).expect("seek");
    let garbage = vec![0xADu8; 8 * 1024];
    f.write_all(&garbage).expect("corrupt");
    f.flush().expect("flush");
}

/// Insert ONE row directly via raw libsql (bypassing the engine entirely) — a stand-in "out-of-band
/// partial write" the count channel of [`reopen_and_check`] must be sensitive to (mirrors
/// `migrate_doctor.rs::stamp_user_version`'s raw-libsql idiom, adapted to an already-async caller).
async fn seed_one_row_out_of_band(db: &std::path::Path) {
    let database = libsql::Builder::new_local(db)
        .build()
        .await
        .expect("open the workspace db");
    let conn = database.connect().expect("connect");
    conn.execute(
        "INSERT INTO issues (id, title) VALUES ('c-neg-partial', 'partial')",
        (),
    )
    .await
    .expect("insert an out-of-band row");
}
