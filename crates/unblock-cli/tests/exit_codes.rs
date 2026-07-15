//! The **golden 0–8 exit-code contract** (FR-11, spine §2.3 / conformance rule 6.5), driven
//! end-to-end over the real `unblock` binary (`main` → `run_with`).
//!
//! Two lock-stepped layers:
//! 1. **CLI-reachable categories** exercised end-to-end (0 success, 1 internal, 2 db, 7 config, 8 io):
//!    each asserts the numeric exit AND that the json-mode error payload is VALID JSON on **stdout**
//!    (FR-11 always-valid-JSON on error) with the human diagnostic on **stderr** in plain mode
//!    (NFR-14). The lifecycle CLI surface (D3) cannot itself produce the issue/validation/dependency/
//!    sync codes (those are MCP-only domain ops) — so:
//! 2. **The full 0–8 table dual-pin** over [`unblock_error::ErrorCode::ALL`] asserts every code maps
//!    to a value in `0..=8` per the exit bucket, in LOCK-STEP with `unblock-error`'s own golden table
//!    (a deliberate dual-pin — a divergence fails CI in both crates, spine §2.3).

mod common;

use std::process::Output;

use common::{Workspace, unblock};
use serde_json::Value;
use unblock_error::ErrorCode;

/// Assert a json-mode command exits with `expected_exit`, emits VALID JSON carrying `expected_code`
/// on **stdout** (FR-11), and writes nothing to stdout that is not that payload.
fn assert_json_error(out: &Output, expected_exit: i32, expected_code: &str) {
    assert_eq!(
        out.status.code(),
        Some(expected_exit),
        "exit code; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let value: Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("stdout must be valid JSON on error (FR-11): {e}; got: {stdout}")
    });
    assert_eq!(
        value["code"], expected_code,
        "structured error code on stdout"
    );
    // The structured error carries a `retryable` flag (spine §2.4) — proves it is the real payload.
    assert!(
        value.get("retryable").is_some(),
        "payload is a StructuredError"
    );
}

#[test]
fn exit_0_success_emits_valid_json_on_stdout() {
    // `version` runs with no workspace; json mode → the report renders to stdout, exit 0.
    let out = unblock()
        .args(["version", "--output", "json"])
        .output()
        .expect("run version");
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let value: Value = serde_json::from_str(stdout.trim()).expect("valid JSON report on stdout");
    assert_eq!(value["kind"], "version", "the version report shape");
}

#[test]
fn exit_1_internal_error_from_update_unconfigured() {
    // `update` outside a dist install refuses (the release source is not configured/verifiable) →
    // `McpServerError`-family InternalError (exit 1). The self-update posture: an unconfigured/
    // unverifiable release source is refused before any swap (see tests/update_verify.rs). Behind the
    // default-on `self-update`.
    let out = unblock()
        .args(["update", "--dry-run", "--output", "json"])
        .output()
        .expect("run update");
    assert_json_error(&out, 1, "INTERNAL_ERROR");
}

#[test]
fn exit_2_already_initialized_clobber_guard() {
    // A second `init` over a scaffolded workspace → CLI-local AlreadyInitialized (exit 2).
    let ws = Workspace::init();
    let out = ws
        .cmd()
        .args(["init", "--output", "json"])
        .output()
        .expect("run init again");
    assert_json_error(&out, 2, "ALREADY_INITIALIZED");
}

#[test]
fn exit_2_not_initialized_no_workspace() {
    // `migrate` from a cwd with no `.unblock/` above it → config NotInitialized (exit 2, db bucket).
    let empty = tempfile::tempdir().expect("tempdir");
    let out = common::unblock_in(empty.path())
        .args(["migrate", "--output", "json"])
        .output()
        .expect("run migrate");
    assert_json_error(&out, 2, "NOT_INITIALIZED");
}

#[test]
fn exit_7_config_parse_error() {
    // A malformed `.unblock/config.toml` → config ConfigParseError (exit 7).
    let root = tempfile::tempdir().expect("tempdir");
    let unblock_dir = root.path().join(".unblock");
    std::fs::create_dir_all(&unblock_dir).expect("mkdir .unblock");
    std::fs::write(unblock_dir.join("config.toml"), "id_prefix = [unclosed\n").expect("write toml");
    // A DB must be present so discovery treats this as a workspace (else it is a not-a-workspace).
    std::fs::write(unblock_dir.join("unblock.db"), b"").expect("touch db");
    let out = common::unblock_in(root.path())
        .args(["migrate", "--output", "json"])
        .output()
        .expect("run migrate");
    assert_json_error(&out, 7, "CONFIG_PARSE_ERROR");
}

#[test]
fn exit_8_io_error_from_init_into_a_file_path() {
    // `init --dir <file>/.unblock` → `create_dir_all` fails (a file is in the path) → CLI-local
    // IoError (exit 8). Portable (no perms dependency — works under root too).
    let root = tempfile::tempdir().expect("tempdir");
    let file = root.path().join("a-file");
    std::fs::write(&file, b"not a dir").expect("write file");
    let target = file.join(".unblock"); // a path THROUGH a regular file → ENOTDIR
    let out = unblock()
        .args(["init", "--dir"])
        .arg(&target)
        .args(["--output", "json"])
        .output()
        .expect("run init");
    assert_json_error(&out, 8, "IO_ERROR");
}

#[test]
fn plain_mode_error_goes_to_stderr_not_stdout() {
    // NFR-14: in plain mode the human `error[CODE]: message` diagnostic goes to STDERR; stdout stays
    // empty (no JSON payload, no diagnostic on stdout).
    let ws = Workspace::init();
    let out = ws
        .cmd()
        .args(["init", "--output", "plain"])
        .output()
        .expect("run init again (plain)");
    assert_eq!(out.status.code(), Some(2));
    assert!(
        out.stdout.is_empty(),
        "plain-mode error must NOT pollute stdout, got: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("error[ALREADY_INITIALIZED]"),
        "human diagnostic on stderr, got: {stderr}"
    );
}

#[test]
fn robot_mode_error_is_valid_json_on_stdout() {
    // FR-11: the `robot` machine format also emits a valid-JSON structured error to stdout.
    let ws = Workspace::init();
    let out = ws
        .cmd()
        .args(["init", "--output", "robot"])
        .output()
        .expect("run init again (robot)");
    assert_json_error(&out, 2, "ALREADY_INITIALIZED");
}

#[test]
fn clap_usage_error_exits_2() {
    // A domain verb (`create`) is not a lifecycle subcommand → a clap usage error (exit 2), printed
    // by clap itself (not the structured boundary). Dual-checks the `run_with` clap-error path.
    let out = unblock().arg("create").output().expect("run create");
    assert_eq!(out.status.code(), Some(2), "clap usage error exit");
    assert!(
        !out.stderr.is_empty(),
        "clap prints its usage error to stderr"
    );
}

#[test]
fn full_exit_code_table_is_dual_pinned_with_unblock_error() {
    // The lifecycle CLI (D3) cannot itself surface the issue(3)/validation(4)/dependency(5)/sync(6)
    // codes — those are MCP-only domain ops. This layer pins the WHOLE 0–8 table in lock-step with
    // `unblock-error`'s golden (spine §2.3): every ErrorCode maps into the correct 0–8 bucket, so a
    // divergence fails here AND in `unblock-error`'s own `exit_code_table` golden.
    for code in ErrorCode::ALL {
        let exit = code.exit_code();
        assert!(
            (1..=8).contains(&exit),
            "{code:?} exit {exit} is outside the 1..=8 error range (0 is success-only)"
        );
        let expected = match code {
            // exit 2 — Database
            ErrorCode::DatabaseNotFound
            | ErrorCode::DatabaseLocked
            | ErrorCode::SchemaMismatch
            | ErrorCode::DatabaseError
            | ErrorCode::NotInitialized
            | ErrorCode::AlreadyInitialized
            | ErrorCode::RateLimited => 2,
            // exit 3 — Issue / operational
            ErrorCode::IssueNotFound
            | ErrorCode::AmbiguousId
            | ErrorCode::IdCollision
            | ErrorCode::InvalidId
            | ErrorCode::NothingToDo
            | ErrorCode::AlreadyClaimed => 3,
            // exit 4 — Validation / policy
            ErrorCode::ValidationFailed
            | ErrorCode::InvalidStatus
            | ErrorCode::InvalidType
            | ErrorCode::InvalidPriority
            | ErrorCode::RequiredField
            | ErrorCode::PolicyViolation => 4,
            // exit 5 — Dependency
            ErrorCode::CycleDetected
            | ErrorCode::DependencyNotFound
            | ErrorCode::HasDependents
            | ErrorCode::SelfDependency
            | ErrorCode::DuplicateDependency => 5,
            // exit 6 — Sync / JSONL
            ErrorCode::JsonlParseError
            | ErrorCode::PrefixMismatch
            | ErrorCode::ImportCollision
            | ErrorCode::SyncConflict
            | ErrorCode::ConflictMarkers
            | ErrorCode::PathTraversal => 6,
            // exit 7 — Config
            ErrorCode::ConfigError | ErrorCode::ConfigNotFound | ErrorCode::ConfigParseError => 7,
            // exit 8 — I/O
            ErrorCode::IoError | ErrorCode::JsonError => 8,
            // exit 1 — Internal
            ErrorCode::InternalError => 1,
        };
        assert_eq!(exit, expected, "{code:?} must map to exit {expected}");
    }
}
