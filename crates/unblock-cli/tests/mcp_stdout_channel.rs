//! **D48 / `ub-og3` (v1.0.1) — the regression file for the STDOUT CHANNEL of `unblock mcp`.**
//!
//! Spawning end-to-end proof that `unblock mcp` puts NOTHING but JSON-RPC frames on stdout on every
//! REACHABLE failure route, and that the FULL structured payload lands on STDERR instead. Modelled
//! on `tests/error_channel.rs` (the D42 precedent: a channel is only observable by driving the
//! wire). What no in-process cell can reach is the classification's CALL SITE in `lib.rs` and the
//! wrapper's binding of the two real streams in `exit.rs` — a flipped literal or a swapped pair of
//! sink arguments compiles and survives every unit cell in `exit.rs::tests`. That is what this file
//! is for, which is why D48 clause (7) calls the spawning layer required rather than optional.
//!
//! **Every cell asserts a POSITIVE stderr landing, never only the frame-only negative.** On every
//! pre-run-loop failure stdout is legitimately EMPTY, so a negative-only assertion stays green under
//! a mutation that deletes the diagnostic outright — the exact vacuity class that hid this defect
//! for the whole life of the suite.
//!
//! **The non-regression half needs no cell here, and the shipped cells that carry it are NAMED so a
//! future reader does not add a redundant one.** The set below is MEASURED, never derived: the
//! "classify everything as a protocol channel" mutation was applied to `cli.rs`'s `stdout_role` and
//! `cargo test -p unblock-cli --no-fail-fast` was read off. EVERY cell it turned red is named here
//! with the command it drives, so the LIST is the rule and carries no count and no "n of m
//! commands" summary. An earlier version of this paragraph carried a COUNT ("eight shipped cells")
//! and its sibling sentence in `exit.rs` carried a COMMANDS summary ("the six report-channel
//! commands"); both were wrong, in different directions, which is why there is now one enumeration
//! and it lives here.
//!
//! - `exit_codes.rs` — `exit_2_not_initialized_no_workspace` and `exit_7_config_parse_error`
//!   (`migrate`); `exit_2_already_initialized_clobber_guard`,
//!   `exit_8_io_error_from_init_into_a_file_path` and `robot_mode_error_is_valid_json_on_stdout`
//!   (`init`); `exit_1_internal_error_from_update_unconfigured` (`update`).
//! - `init_agents.rs` — `agents_requires_a_workspace` (`agents`; the only cell in the crate that
//!   covers that command at all).
//! - `migrate_doctor.rs` — `migrate_on_a_future_schema_db_exits_2_with_schema_mismatch` and
//!   `a_lying_stamp_exits_2_with_a_hint_that_names_the_repair_and_the_missing_columns` (`migrate`);
//!   `doctor_on_a_corrupt_db_exits_2` (`doctor`).
//! - `update_verify.rs` — `update_refuses_a_receiptless_binary_and_swaps_nothing` and
//!   `update_rejects_a_tampered_download_and_swaps_nothing` (`update`).
//! - `cli.rs`'s unit cell `each_subcommand_declares_its_stdout_role` — the unit-layer half, and the
//!   only member of this set that does not spawn a process.
//!
//! `version` is covered by NONE of them, which is expected rather than a hole: `exit_codes.rs`'s
//! `version` cell is an exit-0 SUCCESS report rendered by `output::emit_report`, which D48 does not
//! touch (clause 6(iii)), so it stays green under that mutation and pins nothing here.
//!
//! **Two routes deliberately get no cell here.** The non-`initialize`-first-frame route is what the
//! two INVERTED cells in `tests/mcp_lifecycle.rs` provoke, and a third cell on the same frame would
//! extend a coupling that file already had to document. `unblock mcp --help` is EXCLUDED by decision
//! (D48 clause 5 — the server never starts, so the framing channel was never live) and is already
//! guarded by `tests/help_snapshots.rs`'s `mcp_help`, which asserts exit 0 AND non-empty stdout.
//!
//! Structurally unreachable routes get NO cell either — `Session::open` (its only failure is a
//! feature the CLI hard-codes off) and the teardown `Err` (a poisoned permit on a semaphore nothing
//! closes) — so an acceptance criterion phrased "covers all four `Err` sources" would be satisfiable
//! VACUOUSLY. The reachable enumeration is what this file carries.
#![cfg(unix)]

use std::process::{Command, Output, Stdio};

use serde_json::Value;

mod common;

/// Spawn `unblock mcp` with stdin already at EOF and collect both streams.
///
/// No `McpClient` and no deadline loop: every cell here returns BEFORE `run_mcp_server`, and
/// `Stdio::null()` guarantees that even under a mutation the run loop reads instant EOF and exits
/// rather than hanging. `Command::output()` reads both pipes to EOF by construction, so these cells
/// are unaffected by the drain-join question the `mcp_lifecycle.rs` inversions have to answer.
fn run_mcp(cmd: &mut Command) -> Output {
    cmd.arg("mcp")
        .stdin(Stdio::null())
        .output()
        .expect("run `unblock mcp`")
}

/// The whole D48 oracle for one pre-run-loop failure: exit code, frame-free stdout, payload on
/// stderr. Returns the payload so a cell can assert further members (E3's `hint`).
fn assert_diagnostic_on_stderr(out: &Output, cell: &str, exit: i32, code: &str) -> Value {
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(exit),
        "{cell}: exit code. D48 moves the CHANNEL and nothing else. stderr:\n{stderr}"
    );
    // NEGATIVE — vacuous alone (stdout is legitimately empty here); see the POSITIVE below.
    common::assert_stdout_is_frame_only(&stdout, cell);
    assert!(
        stdout.is_empty(),
        "{cell}: the server never framed anything, so stdout must be EMPTY: {stdout}"
    );
    // POSITIVE — the half that survives a "delete the diagnostic" mutation.
    let payload = common::structured_error_on_stderr(&stderr).unwrap_or_else(|| {
        panic!(
            "{cell}: the FULL structured payload must land on STDERR (D48) — an MCP host capturing \
             the child stderr is the only place this failure can now be read. stderr:\n{stderr}"
        )
    });
    assert_eq!(payload["code"], code, "{cell}: {payload}");
    assert!(
        payload.get("retryable").is_some(),
        "{cell}: it is the StructuredError, not a human line: {payload}"
    );
    assert!(
        !stderr.contains("error["),
        "{cell}: json/robot keeps the MACHINE payload; it is not degraded to the `error[CODE]` \
         line (D48 payload clause): {stderr}"
    );
    payload
}

/// **E1 — the most likely real hit.** A committed `.mcp.json` whose cwd and D39 tier both miss: the
/// workspace open fails BEFORE the D39 binding line is written, so stderr used to be EMPTY and the
/// only bytes the operator got were on the one stream that cannot carry them.
///
/// The exit code is `NOT_INITIALIZED`/2 — a walk-up MISS, not a config 7 (see E6 for the other
/// discovery route). The harness scrubs `UNBLOCK_DIR`/`CLAUDE_PROJECT_DIR`, so host discovery
/// cannot rescue this tempdir.
#[test]
fn no_workspace_reports_on_stderr_not_on_the_framing_channel() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = run_mcp(&mut common::unblock_in(dir.path()));
    assert_diagnostic_on_stderr(&out, "E1 no workspace", 2, "NOT_INITIALIZED");
}

/// **E2 — the self-referential case.** `pick_cli_format` read the SAME variable leniently and
/// resolved `Json`, so the format resolver's own failure is reported in the format it defaulted to.
/// Post-D48, on stderr. The per-child `env` beats the harness's `env_remove`, which is what makes
/// this cell possible at all — E6's second spelling
/// (`an_explicit_dir_at_a_non_workspace_reports_on_stderr`, which sets `UNBLOCK_DIR` back per child
/// and says so) relies on exactly the same ordering, so it is a shared property of the harness
/// rather than anything special to this cell.
#[test]
fn an_unparseable_output_format_env_reports_on_stderr() {
    let ws = common::Workspace::init();
    let mut cmd = ws.cmd();
    cmd.env("UNBLOCK_OUTPUT_FORMAT", "xml");
    let out = run_mcp(&mut cmd);
    let payload = assert_diagnostic_on_stderr(&out, "E2 bad output format", 7, "CONFIG_ERROR");
    let message = payload["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("UNBLOCK_OUTPUT_FORMAT"),
        "the message must NAME the offending variable: {payload}"
    );
}

/// **E3 — THE payload cell (D48 clause 3).** The D46 mixed-version upgrade scenario: a database
/// stamped NEWER than this build. The `hint` is the actionable half, and it is exactly what a
/// degrade-to-`error[CODE]`-line would drop — so it is asserted PRESENT and non-empty here, not
/// merely described.
#[test]
fn a_newer_schema_reports_on_stderr_with_its_hint() {
    let ws = common::Workspace::init();
    common::stamp_user_version(&ws.db_path(), 99);
    let out = run_mcp(&mut ws.cmd());
    let payload = assert_diagnostic_on_stderr(&out, "E3 newer schema", 2, "SCHEMA_MISMATCH");
    let hint = payload["hint"].as_str().unwrap_or_default();
    assert!(
        hint.contains("NEWER") && hint.contains("unblock update"),
        "the hint tells the operator what to DO, and the move must not cost it: {payload}"
    );
}

/// **E4 — a corrupt database.** The CODE is asserted and never the `SQLite` message text, which is
/// not ours to pin. The corruption starts at byte 100 (over `sqlite_master`, keeping the file
/// header): the shipped `migrate_doctor.rs` recipe starts at 4096 and is INVISIBLE to the `mcp`
/// open path, so a cell built on it would pass today, after the fix, and under every mutation.
#[test]
fn a_corrupt_database_reports_on_stderr() {
    let ws = common::Workspace::init();
    common::corrupt_db_schema_page(&ws.db_path());
    let out = run_mcp(&mut ws.cmd());
    assert_diagnostic_on_stderr(&out, "E4 corrupt database", 2, "DATABASE_ERROR");
}

/// **E5 — the control.** D48 moves the MACHINE arm only; the human arm was already on stderr and is
/// byte-unchanged. Without this cell, a "fix" that funnelled every format through one arm would go
/// unobserved.
#[test]
fn the_human_format_arm_is_untouched_by_d48() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut cmd = common::unblock_in(dir.path());
    cmd.args(["-o", "plain"]);
    let out = run_mcp(&mut cmd);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "stderr:\n{stderr}");
    assert!(
        stdout.is_empty(),
        "the human arm never wrote to stdout, before or after D48: {stdout}"
    );
    assert!(
        stderr.contains("error[NOT_INITIALIZED]: "),
        "the plain arm keeps the NFR-14 one-line shape: {stderr}"
    );
    assert!(
        common::structured_error_on_stderr(&stderr).is_none(),
        "and it does NOT emit the machine document — that is the json/robot arm's job: {stderr}"
    );
}

/// **E6 — the OTHER discovery route.** An EXPLICIT dir naming a non-workspace is
/// `ConfigError::InvalidValue` → `CONFIG_ERROR`/7, distinct from E1's walk-up MISS
/// (`NOT_INITIALIZED`/2). D48 clause (4) states both routes as measured, so without this cell that
/// enumeration would be aspirational rather than pinned.
///
/// **The two spellings below are ONE code path, not two, and saying so is the point.** `--dir` and
/// `UNBLOCK_DIR` are bound by a single clap `env` attribute (`cli.rs`'s `GlobalArgs::dir`), so both
/// arrive as the same `Option<PathBuf>`; the REAL distinction this cell draws is explicit-dir
/// `InvalidValue` versus E1's walk-up miss. Both are driven anyway because clause (4) names both,
/// and a reader who met only one could reasonably wonder whether the other differs.
#[test]
fn an_explicit_dir_at_a_non_workspace_reports_on_stderr() {
    let ws = common::Workspace::init();
    let empty = tempfile::tempdir().expect("tempdir");

    // Spelling 1: the flag. Anchored at a REAL workspace root, so only the explicit dir can be what
    // fails — a walk-up from here would have succeeded.
    let mut with_flag = common::unblock_in(ws.root());
    with_flag.args(["--dir", &empty.path().to_string_lossy()]);
    let out = run_mcp(&mut with_flag);
    assert_diagnostic_on_stderr(&out, "E6 --dir at a non-workspace", 7, "CONFIG_ERROR");

    // Spelling 2: the env var the harness scrubs, set back per-child.
    let mut with_env = common::unblock_in(ws.root());
    with_env.env("UNBLOCK_DIR", empty.path());
    let out = run_mcp(&mut with_env);
    assert_diagnostic_on_stderr(&out, "E6 UNBLOCK_DIR at a non-workspace", 7, "CONFIG_ERROR");
}

/// **E7 — the malformed `config.toml` route** (`CONFIG_PARSE_ERROR`/7), the same pre-run-loop route
/// `exit_codes.rs`'s `exit_7_config_parse_error` already pins for `migrate` on STDOUT.
///
/// It is here rather than recorded as an exclusion because it is genuinely REACHABLE on the `mcp`
/// open path and reaches it through a DIFFERENT `ConfigError` variant than E2 or E6 — so the
/// enumeration of what a broken workspace looks like on this channel would otherwise stop one route
/// short. The `.unblock/` is hand-built (a scaffolded one has a valid config), and a DB file must
/// exist or discovery classifies the directory as not-a-workspace and E1's route fires instead.
#[test]
fn a_malformed_config_reports_on_stderr() {
    let root = tempfile::tempdir().expect("tempdir");
    let unblock_dir = root.path().join(".unblock");
    std::fs::create_dir_all(&unblock_dir).expect("mkdir .unblock");
    std::fs::write(unblock_dir.join("config.toml"), "id_prefix = [unclosed\n").expect("write toml");
    std::fs::write(unblock_dir.join("unblock.db"), b"").expect("touch db");
    let out = run_mcp(&mut common::unblock_in(root.path()));
    assert_diagnostic_on_stderr(&out, "E7 malformed config.toml", 7, "CONFIG_PARSE_ERROR");
}

// -------------------------------------------------------------------------------------------------
// HARNESS SELF-TESTS — a guard nobody drives is a guard that can be deleted.
//
// The three guards this change adds to `common/mod.rs` are HARNESS, not SUT, so nothing in the
// product turns red when one is reverted. Each therefore gets a cell that drives it directly, and
// the predicate and its CALL SITE get SEPARATE cells: they are separately revertible, and a cell
// that calls the predicate by hand says nothing about whether `read_response` still does.
// -------------------------------------------------------------------------------------------------

/// The MEASURED ub-og3 blob — the real bytes `unblock mcp` used to write onto the framing channel,
/// not an invented shape. It is valid JSON, which is precisely why the pre-D48 guard passed it.
const UB_OG3_BLOB: &str = r#"{"code":"INTERNAL_ERROR","message":"mcp server error: failed to start the MCP server","retryable":false}"#;

const REAL_FRAME: &str = r#"{"jsonrpc":"2.0","id":1,"result":{}}"#;

/// **H1** — the framing PREDICATE itself. Kills a revert to the bare `serde_json::from_str`, and
/// only that: it calls the predicate by hand, so it says nothing about whether `read_response`
/// still installs it (H4).
#[test]
fn the_framing_guard_rejects_a_structured_error_blob() {
    let parse = |s: &str| serde_json::from_str::<Value>(s).expect("fixture parses");
    assert!(
        common::is_jsonrpc_framing(&parse(REAL_FRAME)),
        "a real frame must be accepted"
    );
    assert!(
        !common::is_jsonrpc_framing(&parse(UB_OG3_BLOB)),
        "the ub-og3 blob is valid JSON and is NOT framing — that gap is the whole defect"
    );
    assert!(
        !common::is_jsonrpc_framing(&parse(r#"{"jsonrpc":"2.0","code":"X","retryable":true}"#)),
        "belt and braces: a blob wearing a `jsonrpc` member is still not framing"
    );
    // The D47 `-32600` answer is legitimate framing whose `code` is nested UNDER `error`. The
    // membership tests are TOP-LEVEL for exactly this reason; a recursive reading would reject it.
    assert!(
        common::is_jsonrpc_framing(&parse(
            r#"{"jsonrpc":"2.0","id":90001,"error":{"code":-32600,"message":"Invalid request"}}"#
        )),
        "a nested `code` belongs to the frame's error object and must NOT be mistaken for a blob"
    );
}

/// **H2** — the env scrub. `Command::get_envs()` records an explicit `(key, None)` per
/// `env_remove`, which is the only way to pin a REMOVAL: mutating process env is `unsafe` under
/// edition 2024 and forbidden here. Pins the three pre-existing removals as well.
#[test]
fn the_harness_scrubs_every_discovery_and_format_env() {
    for key in [
        "CLAUDE_PROJECT_DIR",
        "UNBLOCK_DIR",
        "UNBLOCK_ACTOR",
        "UNBLOCK_OUTPUT_FORMAT",
    ] {
        assert!(
            common::unblock()
                .get_envs()
                .any(|(k, v)| k == key && v.is_none()),
            "every spawn must scrub `{key}`: inherited, it makes the frame-only cells vacuous \
             (UNBLOCK_OUTPUT_FORMAT) or binds the wrong workspace (the discovery three)"
        );
    }
}

/// **H3** — the frame-only ASSERTION helper. Kills replacing its body with `{}`, and nothing else
/// does: every production call site asserts stdout is EMPTY immediately afterwards, so its loop
/// never sees a byte and the gutted helper would leave the whole matrix green.
///
/// **All THREE of the helper's arms are driven, one input each, and each is matched on ITS OWN
/// message** — otherwise deleting a single `assert!` inside the helper would stay green while the
/// other two carried the cell. The arms fire in order, which is why the measured ub-og3 blob lands
/// on the MISSING-`jsonrpc` arm rather than the blob-shape one: it carries no `jsonrpc` member at
/// all. Driving the blob-shape arm therefore needs a line that HAS one, which is the hybrid below.
#[test]
fn the_frame_only_assertion_rejects_a_blob_and_a_bare_line() {
    use std::panic::catch_unwind;

    common::assert_stdout_is_frame_only(&format!("{REAL_FRAME}\n"), "H3 accepts a real frame");

    let bare = catch_unwind(|| common::assert_stdout_is_frame_only("not json at all", "H3 bare"))
        .expect_err("a bare non-JSON line on stdout must PANIC");
    let message = panic_message(&bare);
    assert!(
        message.contains("not JSON"),
        "arm 1: a non-JSON line fails on parsing, before either membership test: {message}"
    );

    let blob = catch_unwind(|| common::assert_stdout_is_frame_only(UB_OG3_BLOB, "H3 blob"))
        .expect_err("the ub-og3 blob on stdout must PANIC");
    let message = panic_message(&blob);
    assert!(
        message.contains("not JSON-RPC framing"),
        "arm 2: the measured blob parses but carries NO `jsonrpc` member: {message}"
    );

    let hybrid = catch_unwind(|| {
        common::assert_stdout_is_frame_only(
            r#"{"jsonrpc":"2.0","code":"X","retryable":true}"#,
            "H3 hybrid",
        );
    })
    .expect_err("a StructuredError wearing a `jsonrpc` member must PANIC too");
    let message = panic_message(&hybrid);
    assert!(
        message.contains("StructuredError blob"),
        "arm 3: the shape test is what catches a blob dressed as a frame: {message}"
    );

    // Arm 3 is a CONJUNCTION (`code.is_none() && retryable.is_none()`) and the hybrid above carries
    // BOTH members, so either half alone still rejects it — dropping one is invisible to it. These
    // two inputs carry ONE member each, so each half is independently load-bearing. Same defect
    // class as `is_jsonrpc_framing`'s (H5), in the sibling helper four cells call.
    for (line, half) in [
        (r#"{"jsonrpc":"2.0","code":"X"}"#, "code"),
        (r#"{"jsonrpc":"2.0","retryable":true}"#, "retryable"),
    ] {
        let caught = catch_unwind(|| common::assert_stdout_is_frame_only(line, "H3 half-blob"))
            .expect_err("arm 3 must reject a line carrying ONE of the two blob members");
        let message = panic_message(&caught);
        assert!(
            message.contains("StructuredError blob"),
            "arm 3's `{half}` half is load-bearing on its own — `{line}` must be rejected BY IT, \
             got: {message}"
        );
    }
}

/// **H4** — the guard's CALL SITE inside `read_response`, which H1 cannot reach.
///
/// Drives a FABRICATED child whose stdout emits the ub-og3 blob and THEN a real response, so the
/// two states are cleanly discriminated: with the guard installed the run dies on the blob line;
/// with it deleted the run succeeds and returns the second line's value. Asserting the panic
/// MESSAGE is mandatory rather than stylistic — without the guard a run can still end in a panic
/// eventually (`child stdout closed before response id=…`), which a bare "it panicked" could not
/// tell apart.
///
/// The child keeps its stdin open (`exec cat >/dev/null`) because `McpClient::request` WRITES
/// before it reads: a child that printed and exited would have closed the read end, and the write
/// would fail `EPIPE` with the wrong message in BOTH branches.
#[test]
fn the_read_response_guard_is_actually_installed() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let child = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "printf '%s\\n' '{UB_OG3_BLOB}' '{REAL_FRAME}'; exec cat >/dev/null"
        ))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the fabricated child");

    let mut client = common::McpClient::from_child(child);
    let caught = catch_unwind(AssertUnwindSafe(|| {
        client.request("ping", &serde_json::json!({}))
    }))
    .expect_err("a non-frame line on the framing channel must PANIC inside read_response");
    let message = panic_message(&caught);
    assert!(
        message.contains("must be JSON-RPC framing"),
        "the guard must be the thing that fired, not a downstream symptom: {message}"
    );
}

/// **H5 — each membership clause of the framing predicate is INDEPENDENTLY load-bearing.**
///
/// H1's three rejection inputs cannot tell the two blob-shape clauses apart: the measured ub-og3
/// blob carries no `jsonrpc` member at all (so the FIRST clause rejects it on its own), and the
/// hybrid carries BOTH `code` and `retryable` (so either clause rejects it on its own). Dropping
/// EITHER shape clause therefore left H1 green — measured, not supposed. These two inputs carry one
/// blob member each ALONGSIDE a `jsonrpc` member, so exactly one clause can be what rejects them.
///
/// The inputs are not exotic: a partially-populated `StructuredError`, or a future frame that grew
/// a top-level `code`, is precisely the line the guard exists to keep off the framing channel.
#[test]
fn each_blob_shape_clause_of_the_framing_predicate_is_load_bearing() {
    let parse = |s: &str| serde_json::from_str::<Value>(s).expect("fixture parses");
    assert!(
        !common::is_jsonrpc_framing(&parse(r#"{"jsonrpc":"2.0","retryable":true}"#)),
        "the `retryable` clause alone must reject this — no other clause can: `code` is absent \
         and `jsonrpc` is present"
    );
    assert!(
        !common::is_jsonrpc_framing(&parse(r#"{"jsonrpc":"2.0","code":"INTERNAL_ERROR"}"#)),
        "the mirror: the `code` clause alone must reject this one"
    );
}

/// **H6 — "all three membership tests are TOP-LEVEL", the predicate's docstring claim, pinned.**
///
/// H1 pins ONE third of it (the D47 `-32600` frame, whose `code` is nested under `error`, is
/// accepted). The other two thirds were prose. Both are reachable on the real wire:
/// - a `tools/call` answer carrying an in-band tool failure nests the WHOLE `StructuredError` —
///   `code` AND `retryable` — under `result.structuredContent`, and that is legitimate framing; a
///   recursive reading would reject the single most common error-carrying frame this server emits;
/// - a blob whose nested payload happens to carry a `jsonrpc` member must still be rejected — a
///   recursive reading of the FIRST clause would accept it.
#[test]
fn every_membership_test_of_the_framing_predicate_is_top_level() {
    let parse = |s: &str| serde_json::from_str::<Value>(s).expect("fixture parses");
    assert!(
        common::is_jsonrpc_framing(&parse(
            r#"{"jsonrpc":"2.0","id":7,"result":{"isError":true,"structuredContent":{"code":"VALIDATION_FAILED","retryable":false}}}"#
        )),
        "an in-band tool failure nests the StructuredError inside a REAL frame; rejecting it \
         would break the commonest error frame the server sends"
    );
    assert!(
        !common::is_jsonrpc_framing(&parse(r#"{"method":"x","params":{"jsonrpc":"2.0"}}"#)),
        "the `jsonrpc` test is top-level too: a nested one does not make a line into framing"
    );
}

/// **H7 — the stderr ORACLE's own shape test, driven directly.**
///
/// `structured_error_on_stderr` is the POSITIVE half of every spawning cell in this file (they all
/// funnel through `assert_diagnostic_on_stderr`) and of the two INVERTED `mcp_lifecycle.rs` cells, so
/// weakening its `.find` predicate weakens all of them at once. The callers that DO re-assert the
/// shape cannot notice: they run against the real product output, which always carries both members,
/// so a weakened oracle returns the same document to them either way. Only a DEGRADED input
/// discriminates, and nothing in the product produces one — which is why it has to be constructed
/// here rather than waited for.
///
/// Both halves are therefore driven with a document carrying ONE member each. A degraded payload is
/// exactly what a "fix" that routed the `error[CODE]` line's fields through the machine arm would
/// produce, which is the mutation the oracle exists to refuse.
#[test]
fn the_stderr_oracle_refuses_a_degraded_document() {
    const STARTUP: &str = "unblock: workspace bound to /ws/.unblock (source: cwd walk-up)\n";

    let full = format!("{STARTUP}{UB_OG3_BLOB}\n");
    let payload = common::structured_error_on_stderr(&full)
        .expect("the control: a COMPLETE StructuredError document is found");
    assert_eq!(payload["code"], "INTERNAL_ERROR");

    for degraded in [
        r#"{"code":"INTERNAL_ERROR","message":"mcp server error"}"#,
        r#"{"retryable":false,"message":"mcp server error"}"#,
    ] {
        let stderr = format!("{STARTUP}{degraded}\n");
        assert!(
            common::structured_error_on_stderr(&stderr).is_none(),
            "the oracle spells `a StructuredError blob` as BOTH members; half a document must not \
             satisfy it, or every cell built on it would accept a degraded payload: {degraded}"
        );
    }
}

/// **H8 — the oracle locates the payload by SHAPE, never by POSITION** (its docstring's claim).
///
/// The claim passes today only by accident: the D39 startup line, the one non-payload line the
/// shipped cells actually meet, is not JSON — so "the first line that parses" and "the line with
/// the payload shape" coincide, and replacing the shape search with a first-parseable-line search
/// survived the whole suite. This buffer breaks that coincidence: its FIRST parseable JSON line is
/// a JSON-formatted log record, the shape a `tracing` JSON subscriber or an MCP host's own wrapper
/// puts on the same stream, and the payload is the THIRD line.
#[test]
fn the_stderr_oracle_finds_the_payload_by_shape_not_by_position() {
    let stderr = format!(
        "{}\n{}{}\n",
        r#"{"timestamp":"2026-08-06T10:00:00Z","level":"DEBUG","fields":{"message":"mcp: workspace opened"}}"#,
        "unblock: workspace bound to /ws/.unblock (source: cwd walk-up)\n",
        UB_OG3_BLOB
    );
    let payload = common::structured_error_on_stderr(&stderr)
        .expect("the payload is on this buffer — it is simply not the first JSON line");
    assert_eq!(
        payload["code"], "INTERNAL_ERROR",
        "a positional search returns the LOG record instead, which is the whole difference \
         between locating by shape and locating by luck: {payload}"
    );
}

/// **H9 — `wait_for` JOINS the retained drains before it returns** (the D48 fail-silent fix in
/// `common/mod.rs`'s `drains` field, which nothing drove).
///
/// The two D48 buffer reads are a NEGATIVE on stdout and a POSITIVE on stderr, and both are read
/// AFTER `wait_for` returns. Without the join, `wait_for` returns the moment `try_wait` sees the
/// child gone — a still-draining pipe then leaves the stdout buffer SHORT, which is exactly what
/// "stdout is empty" looks like, so the headline channel-revert mutation could survive its own
/// regression pin intermittently.
///
/// **The discriminator is a HAPPENS-BEFORE the mutation cannot fabricate, and it had to be.** The
/// first version of this cell read `pending_drain_count()` and nothing else, which was measured to
/// be worthless: `join_drains` empties the handle vector with `drain(..)` whether or not it joins,
/// so replacing `handle.join()` with `drop(handle)` — the exact detached state this fix exists to
/// repair — left the count at 0 and this cell green. A count of handles observes that `join_drains`
/// was CALLED, never that a thread was JOINED.
///
/// So the fixture makes the two states physically distinguishable. The child writes one line to each
/// stream, hands both pipe write ends to a BACKGROUND grandchild, and exits immediately; the
/// grandchild writes the `-last` lines a second later and only then do the pipes reach EOF. A real
/// join therefore BLOCKS until those late bytes are in the buffers, and nothing that skips the join
/// can put them there — at the moment such a `wait_for` returns, those bytes have not been written
/// yet. The same fact is read twice, as content and as elapsed time.
///
/// **The child exits 1 on purpose.** Both cells this join protects
/// (`mcp_lifecycle.rs`'s `a_no_signal_run_loop_error_exits_1_and_never_hangs` and
/// `an_id_less_notification_before_initialize_still_exits_1`) drive a child that exits 1, so a join
/// gated on `status.success()` would be skipped on precisely the path that matters. An exit-0
/// fixture — which is what this cell used — cannot see that mutation at all.
///
/// `pending_drain_count()` is still read twice, but only as an anti-vacuity CONTROL: 2 before proves
/// there were drains to join at all, 0 after proves the call ran. Neither reading can tell a join
/// from a drop; that is what the late bytes are for.
#[test]
fn wait_for_joins_the_retained_drains_before_the_buffers_are_read() {
    use std::time::{Duration, Instant};

    // The `{ … } &` subshell inherits BOTH pipe write ends and outlives its parent, so EOF — and
    // therefore the end of any real join — comes a second AFTER the child itself is reaped.
    let child = Command::new("sh")
        .arg("-c")
        .arg(
            "printf 'out-first\\n'; printf 'err-first\\n' >&2; \
             { sleep 1; printf 'out-last\\n'; printf 'err-last\\n' >&2; } & \
             exit 1",
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the fabricated child");

    let mut client = common::McpClient::from_child(child);
    client.capture_stdout();
    assert_eq!(
        client.pending_drain_count(),
        2,
        "the control: both pipes are drained by RETAINED threads (stderr from `from_child`, \
         stdout from `capture_stdout`) — without this, the assertions below could pass vacuously \
         over a client that never had a drain to join"
    );

    let started = Instant::now();
    let status = client.wait_for(Duration::from_secs(20));
    let waited = started.elapsed();

    assert_eq!(
        status.code(),
        Some(1),
        "the fixture exits 1 DELIBERATELY — it is the exit code of both cells this join protects, \
         so a join gated on a SUCCESSFUL exit fails here instead of being silently skipped"
    );

    // THE DISCRIMINATOR. These bytes reach the pipes ~1s after the child is reaped, so they can be
    // in the buffers only if `wait_for` waited for the drains to read to EOF.
    let (stdout, stderr) = (client.stdout_snapshot(), client.stderr_snapshot());
    assert!(
        stdout.contains("out-last") && stderr.contains("err-last"),
        "every retained drain must be JOINED before `wait_for` returns, not merely dropped: these \
         bytes are written a second after the child exits, so an unjoined drain has not read them \
         yet — and a short stdout buffer is indistinguishable from the empty stdout the D48 \
         frame-only negative expects. stdout=`{stdout}` stderr=`{stderr}`"
    );
    // The same happens-before, read as a duration. ONE-SIDED by construction: a loaded machine can
    // only make this longer, so it can fail only when the synchronisation is absent.
    assert!(
        waited >= Duration::from_millis(500),
        "`wait_for` must BLOCK on the drains: it returned after {waited:?}, i.e. before the pipes \
         could possibly have reached EOF"
    );

    assert_eq!(
        client.pending_drain_count(),
        0,
        "the closing control: `join_drains` ran. It empties the vector whether or not it joins, so \
         this can never be the discriminator — the late bytes above are"
    );
}

/// Extract a panic payload's message (`&str` or `String`), so a self-test can assert WHICH
/// assertion fired rather than merely that something did.
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    payload.downcast_ref::<&str>().map_or_else(
        || {
            payload
                .downcast_ref::<String>()
                .cloned()
                .unwrap_or_else(|| "<non-string panic payload>".to_string())
        },
        |s| (*s).to_string(),
    )
}
