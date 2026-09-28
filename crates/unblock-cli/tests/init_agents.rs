//! `unblock init` + `unblock agents` bootstrap (FR-14, D27/AF-3).
//!
//! - init: scaffolds EXACTLY `.unblock/config.toml` + a migrated empty `unblock.db` — nothing else
//!   (NO `.gitignore`/`metadata.json`/`issues.jsonl`, D13/NFR-6/model-B). Idempotent + clobber-guarded
//!   (`--force` overwrites); `--prefix` is normalized on disk; the scaffolded config round-trips
//!   through a real workspace open (FR-9 no-drift — `migrate` succeeds against it).
//! - agents: writes a managed `AGENTS.md` block delimited by markers; a re-run updates ONLY the block
//!   (idempotent) and preserves surrounding content; the block is snapshot-pinned.
//! - The target probe, the binds-first check and the sibling guard pick and refuse `init`'s target.
//! - `init --agents` (v1.1) writes the same block at the root of the workspace it scaffolded. A bare
//!   `init` prints a one-line stderr hint instead. The hint, a failed `AGENTS.md` write and an
//!   `AlreadyInitialized` refusal under `--agents` end with the same retry text, and the cells run
//!   that text through `sh`.

mod common;

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Output;

use common::{Workspace, unblock};
use serde_json::Value;

#[test]
fn init_scaffolds_only_config_and_db() {
    let ws = Workspace::init();
    assert!(ws.config_path().exists(), "config.toml created");
    assert!(ws.db_path().exists(), "unblock.db created + migrated");

    // The `.unblock/` dir holds EXACTLY these entries — no extras (AF-3, D13/NFR-6/model-B). The
    // `.write.lock` is the D31 cross-process advisory write lock (a pure `File::try_lock` target, no
    // content), created when migrate's fresh-bootstrap takes the exclusive lock; it is a documented
    // `.unblock/` artifact (PRD §7 on-disk-artifacts), distinct from the vestigial `.unblock.lock`.
    let entries: BTreeSet<String> = std::fs::read_dir(ws.unblock_dir())
        .expect("read .unblock")
        .map(|e| {
            e.expect("dir entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let expected: BTreeSet<String> = ["config.toml", "unblock.db", ".write.lock"]
        .into_iter()
        .map(str::to_string)
        .collect();
    assert_eq!(
        entries, expected,
        "init must scaffold config.toml + unblock.db + the D31 .write.lock; no .gitignore/metadata.json/issues.jsonl"
    );
    // A bare `init` (no `--agents`) leaves AGENTS.md absent.
    assert!(
        !ws.root().join("AGENTS.md").exists(),
        "a bare init must leave AGENTS.md absent"
    );
    // No issues.jsonl at the workspace root either.
    assert!(
        !ws.root().join("issues.jsonl").exists(),
        "no seeded issues.jsonl"
    );
}

/// `-o` reaches `init`'s open, so each human format renders the scaffold report on success. json and
/// robot render identical compact bytes under default options, so a robot leg would prove nothing.
/// Markdown escapes the underscore in the `id_prefix` label.
#[test]
fn init_honors_the_output_flag() {
    for (format, opening, prefix_label) in [
        ("plain", "Diagnostics: ", "id_prefix"),
        ("markdown", "## Diagnostics: ", r"id\_prefix"),
        ("csv", "label,detail\n", "id_prefix"),
    ] {
        let root = tempfile::tempdir().expect("tempdir");
        let out = unblock()
            .current_dir(root.path())
            .args(["init", "--output", format])
            .output()
            .expect("run init");
        assert_eq!(
            out.status.code(),
            Some(0),
            "init -o {format} must exit 0; stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            serde_json::from_slice::<Value>(&out.stdout).is_err(),
            "init -o {format} must not render JSON: {stdout}"
        );
        assert!(
            stdout.starts_with(opening),
            "init -o {format} renders its own format: {stdout}"
        );
        assert!(
            stdout.contains(prefix_label),
            "the report names the prefix: {stdout}"
        );
        if format == "csv" {
            assert_eq!(
                common::csv_report(&out.stdout).len(),
                5,
                "the five scaffold records: {stdout}"
            );
        }
    }
}

/// `init -o csv` renders the scaffold report as csv, in the adapter's order, and under `--agents`
/// the `agents_path` row comes last.
#[test]
fn init_csv_reports_the_scaffold() {
    let root = tempfile::tempdir().expect("tempdir");
    let out = unblock()
        .current_dir(root.path())
        .args(["init", "--output", "csv"])
        .output()
        .expect("run init");
    assert_eq!(
        out.status.code(),
        Some(0),
        "init -o csv must exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let records = common::csv_report(&out.stdout);
    assert_eq!(
        common::csv_labels(&records),
        [
            "workspace_dir",
            "unblock_dir",
            "db_path",
            "config_path",
            "id_prefix"
        ]
    );

    let agents_root = tempfile::tempdir().expect("tempdir");
    let out = unblock()
        .current_dir(agents_root.path())
        .args(["init", "--agents", "--output", "csv"])
        .output()
        .expect("run init --agents");
    assert_eq!(
        out.status.code(),
        Some(0),
        "init --agents -o csv must exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let records = common::csv_report(&out.stdout);
    assert_eq!(
        common::csv_labels(&records),
        [
            "workspace_dir",
            "unblock_dir",
            "db_path",
            "config_path",
            "id_prefix",
            "agents_path"
        ]
    );
}

/// `--actor` reaches `init`'s open, so an actor over the length bound fails there with exit 7.
/// `config.toml` is already written and `unblock.db` is not. A plain re-run then meets the clobber
/// guard, and `init --force` without the bad actor recovers.
#[test]
fn init_rejects_an_invalid_actor_after_writing_config_and_force_recovers() {
    let root = tempfile::tempdir().expect("tempdir");
    let unblock_dir = root.path().join(".unblock");
    let long_actor = "a".repeat(201);

    let rejected = unblock()
        .current_dir(root.path())
        .args(["init", "--actor", &long_actor, "--output", "json"])
        .output()
        .expect("run init --actor");
    let stderr = String::from_utf8_lossy(&rejected.stderr);
    assert_eq!(
        rejected.status.code(),
        Some(7),
        "an invalid actor fails the open; stderr: {stderr}"
    );
    let error: Value =
        serde_json::from_slice(&rejected.stdout).expect("a JSON error document on stdout");
    assert_eq!(error["code"], "CONFIG_ERROR", "error: {error}");
    assert!(
        unblock_dir.join("config.toml").exists(),
        "config.toml is written before the open"
    );
    assert!(
        !unblock_dir.join("unblock.db").exists(),
        "the open fails before it creates the database"
    );
    assert!(
        !stderr.lines().any(|line| line.starts_with("hint: ")),
        "a failed init prints no hint: {stderr}"
    );

    let rerun = unblock()
        .current_dir(root.path())
        .arg("init")
        .output()
        .expect("run init again");
    assert_eq!(
        rerun.status.code(),
        Some(2),
        "the clobber guard refuses the half scaffold; stderr: {}",
        String::from_utf8_lossy(&rerun.stderr)
    );

    let forced = unblock()
        .current_dir(root.path())
        .args(["init", "--force"])
        .output()
        .expect("run init --force");
    assert_eq!(
        forced.status.code(),
        Some(0),
        "init --force recovers; stderr: {}",
        String::from_utf8_lossy(&forced.stderr)
    );
    assert!(
        unblock_dir.join("unblock.db").exists(),
        "the recovery creates the database"
    );
}

/// `UNBLOCK_OUTPUT_FORMAT=csv` reaches `init` through config's env layer, so the scaffold report
/// renders as csv and the run exits 0.
#[test]
fn init_csv_via_env_reports_the_scaffold() {
    let root = tempfile::tempdir().expect("tempdir");
    let out = unblock()
        .current_dir(root.path())
        .env("UNBLOCK_OUTPUT_FORMAT", "csv")
        .arg("init")
        .output()
        .expect("run init");
    assert_eq!(
        out.status.code(),
        Some(0),
        "init under UNBLOCK_OUTPUT_FORMAT=csv must exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let records = common::csv_report(&out.stdout);
    assert_eq!(
        common::csv_labels(&records),
        [
            "workspace_dir",
            "unblock_dir",
            "db_path",
            "config_path",
            "id_prefix"
        ]
    );
}

#[test]
fn init_is_clobber_guarded_and_force_overwrites() {
    let ws = Workspace::init();

    // A second bare init refuses (exit 2, AlreadyInitialized).
    let refused = ws.cmd().arg("init").output().expect("run init again");
    assert_eq!(refused.status.code(), Some(2), "clobber guard fires");

    // `--force` overwrites the scaffold (exit 0).
    let forced = ws
        .cmd()
        .args(["init", "--force", "--output", "json"])
        .output()
        .expect("run init --force");
    assert_eq!(
        forced.status.code(),
        Some(0),
        "--force overrides the clobber guard; stderr: {}",
        String::from_utf8_lossy(&forced.stderr)
    );
}

#[test]
fn init_prefix_is_normalized_on_disk() {
    // `--prefix Weird!!` → `normalize_prefix` → lowercase-alnum "weird" (D21).
    let ws = Workspace::init_with_prefix(Some("Weird!!"));
    let config_text = std::fs::read_to_string(ws.config_path()).expect("read config.toml");
    assert!(
        config_text.contains("id_prefix = \"weird\""),
        "the seeded prefix must be normalized on disk, got:\n{config_text}"
    );
    // And the init JSON report echoes the normalized prefix.
    let out = ws
        .cmd()
        .args(["init", "--force", "--prefix", "Weird!!", "--output", "json"])
        .output()
        .expect("re-init json");
    assert_eq!(out.status.code(), Some(0));
    let report: Value = serde_json::from_slice(&out.stdout).expect("valid JSON init report");
    let prefix = report["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .find(|f| f["label"] == "id_prefix")
        .and_then(|f| f["detail"].as_str());
    assert_eq!(prefix, Some("weird"), "report echoes the normalized prefix");
}

#[test]
fn scaffolded_config_round_trips_through_a_real_open() {
    // The scaffolded config.toml must open cleanly through the SAME facade the runtime uses — proven
    // by a `migrate` (which opens the workspace) succeeding on the freshly-init'd dir (FR-9 no-drift).
    let ws = Workspace::init_with_prefix(Some("proj"));
    let out = ws
        .cmd()
        .args(["migrate", "--output", "json"])
        .output()
        .expect("run migrate on the scaffold");
    assert_eq!(
        out.status.code(),
        Some(0),
        "the scaffolded config must open+migrate cleanly; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn agents_writes_a_managed_block_and_is_idempotent() {
    let ws = Workspace::init();
    let agents_path = ws.root().join("AGENTS.md");

    // First run creates AGENTS.md with the managed block.
    let first = ws.cmd().arg("agents").output().expect("run agents");
    assert_eq!(
        first.status.code(),
        Some(0),
        "agents succeeds; stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(agents_path.exists(), "AGENTS.md written");
    let after_first = std::fs::read_to_string(&agents_path).expect("read AGENTS.md");
    assert!(
        after_first.contains("<!-- BEGIN unblock -->"),
        "managed block markers present"
    );
    assert!(after_first.contains("<!-- END unblock -->"));
    assert!(
        after_first.contains("unblock mcp"),
        "block describes MCP wiring"
    );

    // The terse "wrote X" note goes to STDERR (NFR-14), not stdout.
    assert!(first.stdout.is_empty(), "agents writes no report to stdout");
    assert!(
        String::from_utf8_lossy(&first.stderr).contains("wrote"),
        "agents writes a note to stderr"
    );

    // Second run is idempotent — the file bytes are identical (exactly one managed block).
    let second = ws.cmd().arg("agents").output().expect("run agents again");
    assert_eq!(second.status.code(), Some(0));
    let after_second = std::fs::read_to_string(&agents_path).expect("re-read AGENTS.md");
    assert_eq!(
        after_first, after_second,
        "a re-run yields identical bytes (idempotent)"
    );
    assert_eq!(
        after_second.matches("<!-- BEGIN unblock -->").count(),
        1,
        "one block only"
    );
}

#[test]
fn agents_preserves_surrounding_content() {
    let ws = Workspace::init();
    let agents_path = ws.root().join("AGENTS.md");
    // Pre-existing content the managed merge must preserve.
    std::fs::write(&agents_path, "# My Project\n\nHand-written notes.\n").expect("seed AGENTS.md");

    let out = ws.cmd().arg("agents").output().expect("run agents");
    assert_eq!(out.status.code(), Some(0));
    let merged = std::fs::read_to_string(&agents_path).expect("read AGENTS.md");
    assert!(
        merged.starts_with("# My Project"),
        "pre-existing content preserved"
    );
    assert!(
        merged.contains("Hand-written notes."),
        "hand notes preserved"
    );
    assert!(
        merged.contains("<!-- BEGIN unblock -->"),
        "managed block appended"
    );
}

#[test]
fn agents_managed_block_is_snapshot_pinned() {
    // Snapshot the generated managed block (marker-delimited, deterministic — the contract version is
    // a fixed const) so a drift in the wiring text is a deliberate re-bless.
    let ws = Workspace::init();
    ws.cmd().arg("agents").output().expect("run agents");
    let content = std::fs::read_to_string(ws.root().join("AGENTS.md")).expect("read AGENTS.md");
    insta::assert_snapshot!("agents_managed_block", content);
}

/// A stderr whose reader has exited fails the "wrote X" note, and `agents` still exits 0.
#[cfg(unix)]
#[test]
fn agents_with_a_broken_stderr_still_exits_0() {
    let ws = Workspace::init();
    let (reader, writer) = std::io::pipe().expect("create a pipe");
    drop(reader);

    let out = ws
        .cmd()
        .arg("agents")
        .stderr(writer)
        .output()
        .expect("run agents");
    assert_eq!(
        out.status.code(),
        Some(0),
        "a failed note must not change the exit code"
    );
    assert!(
        ws.root().join("AGENTS.md").is_file(),
        "agents writes AGENTS.md before the note"
    );
}

/// An `AGENTS.md` that `agents` cannot read keeps the `Io` error and its message (exit 8).
#[test]
fn agents_io_failure_keeps_its_message() {
    let ws = Workspace::init();
    std::fs::create_dir(ws.root().join("AGENTS.md")).expect("mkdir AGENTS.md");

    let out = ws
        .cmd()
        .args(["agents", "--output", "json"])
        .output()
        .expect("run agents");
    assert_eq!(
        out.status.code(),
        Some(8),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let error: Value =
        serde_json::from_slice(&out.stdout).expect("one JSON error document on stdout");
    assert_eq!(error["code"], "IO_ERROR", "error: {error}");
    let message = error["message"].as_str().expect("a string message");
    assert!(
        message.starts_with("file operation failed:"),
        "the agents I/O message: {message}"
    );
}

#[test]
fn agents_requires_a_workspace() {
    // `agents` opens resolve-only — no workspace → NotInitialized (exit 2), so AGENTS.md sits next to
    // a real `.unblock/`.
    let empty = tempfile::tempdir().expect("tempdir");
    let out = unblock()
        .current_dir(empty.path())
        .args(["agents", "--output", "json"])
        .output()
        .expect("run agents");
    assert_eq!(
        out.status.code(),
        Some(2),
        "agents requires an existing workspace"
    );
    let value: Value = serde_json::from_slice(&out.stdout).expect("valid JSON error");
    assert_eq!(value["code"], "NOT_INITIALIZED");
}

// -- The target probe (ub-lp9.14) -----------------------------------------------------------------

/// Runs `init` from `cwd` with `flags` and `--dir dir`, under `--output json`.
fn init_dir(cwd: &Path, dir: &Path, flags: &[&str]) -> Output {
    common::unblock_in(cwd)
        .arg("init")
        .args(flags)
        .arg("--dir")
        .arg(dir)
        .args(["--output", "json"])
        .output()
        .expect("run init --dir")
}

/// Asserts that a json-mode `init` exited 0 and returns its one report document.
fn scaffold_report(out: &Output) -> Value {
    assert_eq!(
        out.status.code(),
        Some(0),
        "init must exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("one JSON report on stdout")
}

/// Asserts that a json-mode `init` was refused with `ALREADY_INITIALIZED` (exit 2) in one JSON
/// document without a hint, and returns its message.
fn refusal_message(out: &Output) -> String {
    assert_eq!(
        out.status.code(),
        Some(2),
        "init must be refused; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let error: Value =
        serde_json::from_slice(&out.stdout).expect("one JSON error document on stdout");
    assert_eq!(error["code"], "ALREADY_INITIALIZED", "error: {error}");
    assert!(
        error["hint"].is_null(),
        "the refusal carries no hint: {error}"
    );
    error["message"]
        .as_str()
        .expect("a string message")
        .to_string()
}

/// The `AlreadyInitialized` message naming `dir` as `init` formed it.
fn already_initialized(dir: &Path) -> String {
    format!("workspace already initialized at {}", dir.display())
}

/// Asserts that `reported` is the canonical form of `expected`.
///
/// The CLI canonicalizes through `dunce`, which agrees with `std::fs::canonicalize` on unix. On
/// Windows `std::fs::canonicalize` adds the verbatim prefix, so there the helper compares the
/// canonical forms of both paths.
fn assert_canonical(reported: &Path, expected: &Path, context: &str) {
    let canonical = std::fs::canonicalize(expected).expect("canonicalize the expected path");
    if cfg!(unix) {
        assert_eq!(reported, canonical, "{context}");
    } else {
        let reported = std::fs::canonicalize(reported).expect("canonicalize the reported path");
        assert_eq!(reported, canonical, "{context}");
    }
}

/// Asserts that the report's `unblock_dir` is the canonical form of `dir`.
fn assert_reports_unblock_dir(report: &Value, dir: &Path) {
    let reported = Path::new(common::detail(report, "unblock_dir").expect("an unblock_dir row"));
    assert_canonical(
        reported,
        dir,
        &format!("the report names the canonical target: {report}"),
    );
}

/// `init --dir <root>` scaffolds the root's `.unblock/` and writes nothing at the root itself.
#[test]
fn init_dir_naming_a_project_root_scaffolds_its_dot_unblock() {
    let root = tempfile::tempdir().expect("tempdir");
    let elsewhere = tempfile::tempdir().expect("tempdir");

    let report = scaffold_report(&init_dir(elsewhere.path(), root.path(), &[]));
    let unblock_dir = root.path().join(".unblock");
    assert!(
        unblock_dir.join("config.toml").exists(),
        "the scaffold's config.toml"
    );
    assert!(
        unblock_dir.join("unblock.db").exists(),
        "the scaffold's database"
    );
    assert!(
        !root.path().join("config.toml").exists(),
        "init writes nothing at the root itself"
    );
    assert_reports_unblock_dir(&report, &unblock_dir);
}

/// `init --dir <root>` over an initialized root is refused, naming the root's `.unblock/`, and
/// writes nothing.
#[test]
fn init_dir_naming_an_initialized_root_is_refused() {
    let ws = Workspace::init();
    let saved = std::fs::read(ws.config_path()).expect("read config.toml");
    let elsewhere = tempfile::tempdir().expect("tempdir");

    let message = refusal_message(&init_dir(elsewhere.path(), ws.root(), &[]));
    assert_eq!(message, already_initialized(&ws.root().join(".unblock")));
    assert!(
        !ws.root().join("config.toml").exists(),
        "init writes nothing at the root itself"
    );
    assert_eq!(
        std::fs::read(ws.config_path()).expect("re-read config.toml"),
        saved,
        "the workspace's config.toml is untouched"
    );
}

/// `init --dir <root>/_unblock` scaffolds that directory as given.
#[test]
fn init_dir_naming_a_workspace_dir_is_used_as_given() {
    let root = tempfile::tempdir().expect("tempdir");
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let underscore = root.path().join("_unblock");

    scaffold_report(&init_dir(elsewhere.path(), &underscore, &[]));
    assert!(
        underscore.join("config.toml").exists(),
        "the scaffold lands in _unblock/"
    );
    assert!(
        !underscore.join(".unblock").exists(),
        "a directory named _unblock is the target, not a root"
    );
}

/// `init --dir <root>` finds the root's `_unblock/` workspace through the probe and refuses it,
/// naming the directory as joined, so the `sub/..` of the given root survives in the message.
#[test]
fn init_dir_naming_a_root_with_an_underscore_workspace_is_refused() {
    let root = tempfile::tempdir().expect("tempdir");
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let underscore = root.path().join("_unblock");
    scaffold_report(&init_dir(elsewhere.path(), &underscore, &[]));
    let saved = std::fs::read(underscore.join("config.toml")).expect("read config.toml");
    std::fs::create_dir(root.path().join("sub")).expect("mkdir sub");

    let via_sub = root.path().join("sub").join("..");
    let message = refusal_message(&init_dir(elsewhere.path(), &via_sub, &[]));
    assert_eq!(message, already_initialized(&via_sub.join("_unblock")));
    assert!(
        !root.path().join(".unblock").exists(),
        "no .unblock/ appears to hide the _unblock/ workspace"
    );
    assert_eq!(
        std::fs::read(underscore.join("config.toml")).expect("re-read config.toml"),
        saved,
        "the _unblock/ workspace is untouched"
    );
}

/// A bare `init` in a root holding only an `_unblock/` workspace is refused, naming it.
#[test]
fn bare_init_in_a_root_with_an_underscore_workspace_is_refused() {
    let root = tempfile::tempdir().expect("tempdir");
    let elsewhere = tempfile::tempdir().expect("tempdir");
    scaffold_report(&init_dir(
        elsewhere.path(),
        &root.path().join("_unblock"),
        &[],
    ));

    let out = common::unblock_in(root.path())
        .args(["init", "--output", "json"])
        .output()
        .expect("run init");
    let message = refusal_message(&out);
    assert!(
        message.ends_with("_unblock"),
        "the refusal names the _unblock/ workspace: {message}"
    );
    assert!(
        !root.path().join(".unblock").exists(),
        "no .unblock/ appears to hide the _unblock/ workspace"
    );

    // The child reads its cwd as the physical path, which `std::fs::canonicalize` matches on unix.
    #[cfg(unix)]
    {
        let canonical_root = std::fs::canonicalize(root.path()).expect("canonicalize the root");
        assert_eq!(
            message,
            already_initialized(&canonical_root.join("_unblock"))
        );
    }
}

/// `init --force` run in a root holding only an `_unblock/` workspace re-scaffolds it in place.
#[test]
fn bare_init_force_in_a_root_with_an_underscore_workspace_rescaffolds_it_in_place() {
    let root = tempfile::tempdir().expect("tempdir");
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let underscore = root.path().join("_unblock");
    scaffold_report(&init_dir(
        elsewhere.path(),
        &underscore,
        &["--prefix", "proj"],
    ));

    let out = common::unblock_in(root.path())
        .args(["init", "--force", "--output", "json"])
        .output()
        .expect("run init --force");
    let report = scaffold_report(&out);
    let config = std::fs::read_to_string(underscore.join("config.toml")).expect("read config.toml");
    assert!(
        config.contains("id_prefix = \"ub\""),
        "the forced scaffold replaces the _unblock/ config: {config}"
    );
    assert!(
        !root.path().join(".unblock").exists(),
        "no .unblock/ appears beside the _unblock/ workspace"
    );
    assert_reports_unblock_dir(&report, &underscore);
}

/// A bare `init` run in a root holding an empty `_unblock/` scaffolds inside it, which discovery at
/// the root binds.
#[test]
fn bare_init_in_a_root_with_an_empty_underscore_scaffolds_inside_it() {
    let root = tempfile::tempdir().expect("tempdir");
    let underscore = root.path().join("_unblock");
    std::fs::create_dir(&underscore).expect("mkdir _unblock");

    let out = common::unblock_in(root.path())
        .args(["init", "--output", "json"])
        .output()
        .expect("run init");
    let report = scaffold_report(&out);
    assert!(
        underscore.join("config.toml").exists(),
        "the scaffold lands in _unblock/"
    );
    assert!(
        !root.path().join(".unblock").exists(),
        "no .unblock/ appears beside the empty _unblock/"
    );
    assert_reports_unblock_dir(&report, &underscore);
}

/// `init --force --dir <root>` re-scaffolds the root's `_unblock/` workspace in place.
#[test]
fn init_force_on_an_underscore_root_rescaffolds_it_in_place() {
    let root = tempfile::tempdir().expect("tempdir");
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let underscore = root.path().join("_unblock");
    scaffold_report(&init_dir(
        elsewhere.path(),
        &underscore,
        &["--prefix", "proj"],
    ));

    let report = scaffold_report(&init_dir(elsewhere.path(), root.path(), &["--force"]));
    let config = std::fs::read_to_string(underscore.join("config.toml")).expect("read config.toml");
    assert!(
        config.contains("id_prefix = \"ub\""),
        "the forced scaffold replaces the _unblock/ config: {config}"
    );
    assert!(
        !root.path().join(".unblock").exists(),
        "no .unblock/ appears beside the _unblock/ workspace"
    );
    assert_reports_unblock_dir(&report, &underscore);
}

/// `init --dir <root>` scaffolds inside the root's empty `_unblock/`, which discovery at the root
/// binds, and its hint names that `_unblock/`.
#[test]
fn init_dir_naming_a_root_with_an_empty_underscore_scaffolds_inside_it() {
    let root = tempfile::tempdir().expect("tempdir");
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let underscore = root.path().join("_unblock");
    std::fs::create_dir(&underscore).expect("mkdir _unblock");

    let out = init_dir(elsewhere.path(), root.path(), &[]);
    let report = scaffold_report(&out);
    assert!(
        underscore.join("config.toml").exists(),
        "the scaffold lands in _unblock/"
    );
    assert!(
        !root.path().join(".unblock").exists(),
        "no .unblock/ appears beside the empty _unblock/"
    );
    assert_reports_unblock_dir(&report, &underscore);

    #[cfg(unix)]
    {
        let canonical_root = std::fs::canonicalize(root.path()).expect("canonicalize the root");
        assert_eq!(
            Path::new(quoted_dir(&printed_retry(&out))),
            canonical_root.join("_unblock"),
            "the hint names the canonical _unblock/"
        );
    }
}

// -- The binds-first check and the sibling guard (ub-lp9.14) --------------------------------------

/// The flag sets a refusal must hold under, because `--force` overrides only the clobber guard.
const WITH_AND_WITHOUT_FORCE: [&[&str]; 2] = [&[], &["--force"]];

/// The binds-first message for the pair's `.unblock` and `_unblock` directories as `init` formed
/// them.
fn binds_first(bound: &Path, hidden: &Path) -> String {
    format!(
        "{} already exists, and discovery binds it before {}",
        bound.display(),
        hidden.display()
    )
}

/// Asserts that `dir` is a directory with no entries.
fn assert_empty_dir(dir: &Path) {
    let mut entries = std::fs::read_dir(dir).expect("read the directory");
    assert!(entries.next().is_none(), "{} stays empty", dir.display());
}

/// `init --dir <root>/.unblock` beside an initialized `_unblock/` is refused, naming the sibling.
#[test]
fn init_beside_an_initialized_underscore_sibling_is_refused() {
    let root = tempfile::tempdir().expect("tempdir");
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let underscore = root.path().join("_unblock");
    scaffold_report(&init_dir(elsewhere.path(), &underscore, &[]));
    let saved = std::fs::read(underscore.join("config.toml")).expect("read config.toml");

    // The `--dir` runs through `sub/..`, so a guard that canonicalizes the sibling it names changes
    // the message on every host.
    std::fs::create_dir(root.path().join("sub")).expect("mkdir sub");
    let via_sub = root.path().join("sub").join("..");
    let message = refusal_message(&init_dir(elsewhere.path(), &via_sub.join(".unblock"), &[]));
    assert_eq!(message, already_initialized(&via_sub.join("_unblock")));
    assert!(
        !root.path().join(".unblock").exists(),
        "no .unblock/ appears to hide the _unblock/ workspace"
    );
    assert_eq!(
        std::fs::read(underscore.join("config.toml")).expect("re-read config.toml"),
        saved,
        "the _unblock/ workspace is untouched"
    );
}

/// `init --dir <root>/_unblock` beside an initialized `.unblock/` is refused, naming the sibling.
#[test]
fn init_underscore_beside_an_initialized_dot_unblock_sibling_is_refused() {
    let root = tempfile::tempdir().expect("tempdir");
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let dot = root.path().join(".unblock");
    scaffold_report(&init_dir(elsewhere.path(), &dot, &[]));

    let underscore = root.path().join("_unblock");
    let message = refusal_message(&init_dir(elsewhere.path(), &underscore, &[]));
    assert_eq!(message, already_initialized(&root.path().join(".unblock")));
    assert!(
        !underscore.exists(),
        "no _unblock/ appears beside the .unblock/ workspace"
    );
}

/// `--force` replaces only the target's own scaffold, so the sibling guard still refuses.
#[test]
fn init_force_beside_an_initialized_sibling_is_still_refused() {
    let root = tempfile::tempdir().expect("tempdir");
    let elsewhere = tempfile::tempdir().expect("tempdir");
    scaffold_report(&init_dir(
        elsewhere.path(),
        &root.path().join("_unblock"),
        &[],
    ));

    let dot = root.path().join(".unblock");
    let message = refusal_message(&init_dir(elsewhere.path(), &dot, &["--force"]));
    assert_eq!(message, already_initialized(&root.path().join("_unblock")));
    assert!(
        !dot.exists(),
        "no .unblock/ appears to hide the _unblock/ workspace"
    );
}

/// Layout L holds an initialized `_unblock/` beside an empty `.unblock/`. A bare `init` and
/// `init --force` at its root, and `init --dir <root>/.unblock`, are each refused, naming the
/// `.unblock/` that discovery binds first.
#[test]
fn bare_init_on_an_empty_dot_unblock_beside_an_initialized_underscore_is_refused() {
    let root = tempfile::tempdir().expect("tempdir");
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let underscore = root.path().join("_unblock");
    scaffold_report(&init_dir(elsewhere.path(), &underscore, &[]));
    let saved = std::fs::read(underscore.join("config.toml")).expect("read config.toml");
    let dot = root.path().join(".unblock");
    std::fs::create_dir(&dot).expect("mkdir .unblock");

    for flags in WITH_AND_WITHOUT_FORCE {
        let out = common::unblock_in(root.path())
            .arg("init")
            .args(flags)
            .args(["--output", "json"])
            .output()
            .expect("run init");
        let message = refusal_message(&out);
        assert!(
            message.contains("already exists, and discovery binds it before"),
            "{flags:?}: the binds-first refusal: {message}"
        );

        // The child reads its cwd as the physical path, which `std::fs::canonicalize` matches on
        // unix.
        #[cfg(unix)]
        {
            let canonical_root = std::fs::canonicalize(root.path()).expect("canonicalize the root");
            assert_eq!(
                message,
                binds_first(
                    &canonical_root.join(".unblock"),
                    &canonical_root.join("_unblock")
                ),
                "{flags:?}"
            );
        }
    }

    let message = refusal_message(&init_dir(elsewhere.path(), &dot, &[]));
    assert_eq!(message, binds_first(&dot, &underscore));
    assert_empty_dir(&dot);
    assert_eq!(
        std::fs::read(underscore.join("config.toml")).expect("re-read config.toml"),
        saved,
        "the _unblock/ workspace is untouched"
    );
}

/// An empty sibling directory never blocks a `.unblock` target, whether `--dir` names it or the
/// probe finds it.
#[test]
fn init_beside_an_empty_sibling_directory_proceeds() {
    let root = tempfile::tempdir().expect("tempdir");
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let underscore = root.path().join("_unblock");
    std::fs::create_dir(&underscore).expect("mkdir _unblock");

    scaffold_report(&init_dir(
        elsewhere.path(),
        &root.path().join(".unblock"),
        &[],
    ));
    assert!(
        root.path().join(".unblock").join("config.toml").exists(),
        "the scaffold lands in .unblock/"
    );
    assert_empty_dir(&underscore);

    let both_empty = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(both_empty.path().join(".unblock")).expect("mkdir .unblock");
    std::fs::create_dir(both_empty.path().join("_unblock")).expect("mkdir _unblock");
    scaffold_report(&init_dir(elsewhere.path(), both_empty.path(), &[]));
    assert!(
        both_empty
            .path()
            .join(".unblock")
            .join("config.toml")
            .exists(),
        "the probed .unblock/ receives the scaffold"
    );
    assert_empty_dir(&both_empty.path().join("_unblock"));
}

/// A root whose two workspace dirs both hold a scaffold meets the clobber guard first, naming the
/// probed `.unblock/`. Under `--force` the sibling guard refuses it, naming the `_unblock/`.
#[test]
fn a_root_with_two_initialized_workspace_dirs_is_refused_by_each_guard() {
    let root = tempfile::tempdir().expect("tempdir");
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let underscore = root.path().join("_unblock");
    scaffold_report(&init_dir(elsewhere.path(), &underscore, &[]));
    let dot = root.path().join(".unblock");
    std::fs::create_dir(&dot).expect("mkdir .unblock");
    std::fs::write(dot.join("config.toml"), "id_prefix = \"ub\"\n").expect("write config.toml");
    let saved_dot = std::fs::read(dot.join("config.toml")).expect("read .unblock/config.toml");
    let saved_underscore =
        std::fs::read(underscore.join("config.toml")).expect("read _unblock/config.toml");

    let message = refusal_message(&init_dir(elsewhere.path(), root.path(), &[]));
    assert_eq!(
        message,
        already_initialized(&root.path().join(".unblock")),
        "the clobber guard names the probed .unblock/"
    );
    let message = refusal_message(&init_dir(elsewhere.path(), root.path(), &["--force"]));
    assert_eq!(
        message,
        already_initialized(&root.path().join("_unblock")),
        "under --force the sibling guard names the _unblock/"
    );
    assert_eq!(
        std::fs::read(dot.join("config.toml")).expect("re-read .unblock/config.toml"),
        saved_dot,
        "the .unblock/ scaffold is untouched"
    );
    assert_eq!(
        std::fs::read(underscore.join("config.toml")).expect("re-read _unblock/config.toml"),
        saved_underscore,
        "the _unblock/ scaffold is untouched"
    );
}

/// An `_unblock` target beside a `.unblock` directory holding no scaffold is refused with or
/// without `--force`, naming that `.unblock/`. A regular `.unblock` file never blocks. A `.unblock`
/// symlink to a directory blocks, and so does a `.unblock/` holding only an export.
#[test]
fn init_underscore_beside_an_empty_dot_unblock_is_refused() {
    let root = tempfile::tempdir().expect("tempdir");
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let dot = root.path().join(".unblock");
    std::fs::create_dir(&dot).expect("mkdir .unblock");
    let underscore = root.path().join("_unblock");
    for flags in WITH_AND_WITHOUT_FORCE {
        let message = refusal_message(&init_dir(elsewhere.path(), &underscore, flags));
        assert_eq!(message, binds_first(&dot, &underscore), "{flags:?}");
    }
    assert!(!underscore.exists(), "the refused target is never created");
    assert_empty_dir(&dot);

    let file_root = tempfile::tempdir().expect("tempdir");
    std::fs::write(file_root.path().join(".unblock"), b"not a dir").expect("write .unblock file");
    let file_underscore = file_root.path().join("_unblock");
    scaffold_report(&init_dir(elsewhere.path(), &file_underscore, &[]));
    assert!(
        file_underscore.join("config.toml").exists(),
        "a .unblock file is no directory, so the _unblock/ receives the scaffold"
    );

    #[cfg(unix)]
    {
        let link_root = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(link_root.path().join("X")).expect("mkdir X");
        std::os::unix::fs::symlink("X", link_root.path().join(".unblock"))
            .expect("symlink .unblock to X");
        let link_underscore = link_root.path().join("_unblock");
        let message = refusal_message(&init_dir(elsewhere.path(), &link_underscore, &[]));
        assert_eq!(
            message,
            binds_first(&link_root.path().join(".unblock"), &link_underscore),
            "a .unblock symlink to a directory is a directory to discovery"
        );
    }

    let export_root = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(export_root.path().join(".unblock")).expect("mkdir .unblock");
    std::fs::write(
        export_root.path().join(".unblock").join("issues.jsonl"),
        b"",
    )
    .expect("write issues.jsonl");
    let export_underscore = export_root.path().join("_unblock");
    let message = refusal_message(&init_dir(elsewhere.path(), &export_underscore, &[]));
    assert_eq!(
        message,
        binds_first(&export_root.path().join(".unblock"), &export_underscore),
        "an export alone is no scaffold"
    );
}

/// A `.unblock` symlink to the `_unblock/` beside it is the same workspace, so `init --force`
/// re-scaffolds it through the link.
#[cfg(unix)]
#[test]
fn init_force_through_a_symlinked_sibling_rescaffolds_in_place() {
    let root = tempfile::tempdir().expect("tempdir");
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let underscore = root.path().join("_unblock");
    scaffold_report(&init_dir(
        elsewhere.path(),
        &underscore,
        &["--prefix", "proj"],
    ));
    let link = root.path().join(".unblock");
    std::os::unix::fs::symlink("_unblock", &link).expect("symlink .unblock to _unblock");

    let report = scaffold_report(&init_dir(elsewhere.path(), root.path(), &["--force"]));
    let config = std::fs::read_to_string(underscore.join("config.toml")).expect("read config.toml");
    assert!(
        config.contains("id_prefix = \"ub\""),
        "the forced scaffold replaces the _unblock/ config: {config}"
    );
    assert!(
        std::fs::symlink_metadata(&link)
            .expect("stat the .unblock link")
            .file_type()
            .is_symlink(),
        "the .unblock link stays a symlink"
    );
    assert_reports_unblock_dir(&report, &underscore);
}

/// An `_unblock` target beside a `.unblock` symlink to itself proceeds, because the pair is one
/// directory.
#[cfg(unix)]
#[test]
fn init_underscore_beside_a_dot_unblock_link_to_itself_proceeds() {
    let root = tempfile::tempdir().expect("tempdir");
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let underscore = root.path().join("_unblock");
    std::fs::create_dir(&underscore).expect("mkdir _unblock");
    std::os::unix::fs::symlink("_unblock", root.path().join(".unblock"))
        .expect("symlink .unblock to _unblock");

    let report = scaffold_report(&init_dir(elsewhere.path(), &underscore, &[]));
    assert!(
        underscore.join("config.toml").exists(),
        "the scaffold lands in _unblock/"
    );
    assert_reports_unblock_dir(&report, &underscore);
}

/// In layout L, `init --dir <root>/_unblock` meets the binds-first check before the clobber guard,
/// so it is refused naming the `.unblock/` with or without `--force`. Under `--force --agents` the
/// message carries no retry text, and no `AGENTS.md` appears.
#[test]
fn an_initialized_underscore_beside_an_empty_dot_unblock_is_refused_naming_the_dot_unblock() {
    let root = tempfile::tempdir().expect("tempdir");
    let elsewhere = tempfile::tempdir().expect("tempdir");

    // Every path runs through `sub/..`, so a check that canonicalizes a path it names changes the
    // message on every host.
    std::fs::create_dir(root.path().join("sub")).expect("mkdir sub");
    let via_sub = root.path().join("sub").join("..");
    let underscore = via_sub.join("_unblock");
    scaffold_report(&init_dir(elsewhere.path(), &underscore, &[]));
    let saved = std::fs::read(underscore.join("config.toml")).expect("read config.toml");
    let dot = via_sub.join(".unblock");
    std::fs::create_dir(&dot).expect("mkdir .unblock");

    let runs: [&[&str]; 3] = [&[], &["--force"], &["--force", "--agents"]];
    for flags in runs {
        let message = refusal_message(&init_dir(elsewhere.path(), &underscore, flags));
        assert_eq!(message, binds_first(&dot, &underscore), "{flags:?}");
    }
    assert_eq!(
        std::fs::read(underscore.join("config.toml")).expect("re-read config.toml"),
        saved,
        "the _unblock/ workspace is untouched"
    );
    assert_empty_dir(&dot);
    assert!(
        !root.path().join("AGENTS.md").exists(),
        "a refused init writes no AGENTS.md"
    );
}

// -- `init --agents` and the next-step hint (ub-lp9.14) -------------------------------------------

/// The managed-block start marker `agents` writes.
const BEGIN_MARKER: &str = "<!-- BEGIN unblock -->";

/// The hint a bare `init` prints, up to its retry text.
#[cfg(unix)]
const HINT_PREFIX: &str = "hint: to write the AGENTS.md block for this workspace, run ";

/// The text an `AlreadyInitialized` refusal under `--agents` appends, up to its retry text.
#[cfg(unix)]
const REFUSAL_RETRY_PREFIX: &str = "; to write its AGENTS.md block, run ";

/// Runs `init` with `args` from `cwd`.
fn init_in(cwd: &Path, args: &[&str]) -> Output {
    common::unblock_in(cwd)
        .arg("init")
        .args(args)
        .output()
        .expect("run init")
}

/// Asserts that `out` exited with `code`, quoting its stderr on failure.
fn assert_exit(out: &Output, code: i32) {
    assert_eq!(
        out.status.code(),
        Some(code),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Returns the lines of `stderr` that are next-step hints.
fn hint_lines(stderr: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(stderr)
        .lines()
        .filter(|line| line.starts_with("hint: "))
        .map(str::to_string)
        .collect()
}

/// Returns the retry text of the one hint line a successful bare `init` printed.
#[cfg(unix)]
fn printed_retry(out: &Output) -> String {
    let hints = hint_lines(&out.stderr);
    assert_eq!(hints.len(), 1, "exactly one hint line: {hints:?}");
    hints[0]
        .strip_prefix(HINT_PREFIX)
        .expect("the hint wording")
        .to_string()
}

/// Returns the directory a retry text names, read from between its single quotes. It holds for a
/// path without a single quote, which the retry text leaves unescaped.
fn quoted_dir(retry: &str) -> &str {
    retry
        .strip_prefix("unblock agents --dir '")
        .and_then(|rest| rest.strip_suffix('\''))
        .unwrap_or_else(|| panic!("a single-quoted retry text: {retry}"))
}

/// Runs a printed retry command through `sh -c` from `cwd`, as a user pasting it would. The
/// leading `unblock` becomes this build's binary, `CLAUDE_PROJECT_DIR` names `project_dir`, and no
/// `UNBLOCK_*` variable is set.
#[cfg(unix)]
fn run_retry_in_sh(retry: &str, cwd: &Path, project_dir: &Path) -> Output {
    let args = retry
        .strip_prefix("unblock ")
        .unwrap_or_else(|| panic!("the retry text runs unblock: {retry}"));
    let binary = assert_cmd::cargo::cargo_bin("unblock");
    let script = format!("{} {args}", posix_quote(&binary.display().to_string()));
    std::process::Command::new("sh")
        .arg("-c")
        .arg(&script)
        .current_dir(cwd)
        .env("CLAUDE_PROJECT_DIR", project_dir)
        .env_remove("UNBLOCK_DIR")
        .env_remove("UNBLOCK_ACTOR")
        .env_remove("UNBLOCK_OUTPUT_FORMAT")
        .output()
        .expect("run sh -c")
}

/// Single-quotes `text` for a POSIX shell.
#[cfg(unix)]
fn posix_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// `init --agents` writes the bytes that `init` followed by `agents` writes, and a following
/// `agents` leaves them unchanged.
#[test]
fn init_agents_writes_the_same_bytes_as_unblock_agents() {
    let chained = tempfile::tempdir().expect("tempdir");
    assert_exit(&init_in(chained.path(), &["--agents"]), 0);
    let agents_md = chained.path().join("AGENTS.md");
    let from_init = std::fs::read(&agents_md).expect("init --agents writes AGENTS.md");

    let rerun = common::unblock_in(chained.path())
        .arg("agents")
        .output()
        .expect("run agents");
    assert_exit(&rerun, 0);
    assert_eq!(
        std::fs::read(&agents_md).expect("re-read AGENTS.md"),
        from_init,
        "a following agents leaves the bytes identical"
    );

    let separate = Workspace::init();
    assert_exit(
        &separate.cmd().arg("agents").output().expect("run agents"),
        0,
    );
    let from_agents =
        std::fs::read(separate.root().join("AGENTS.md")).expect("agents writes AGENTS.md");
    assert_eq!(
        from_init, from_agents,
        "init --agents writes the bytes agents writes"
    );
}

/// `init --agents` keeps hand-written `AGENTS.md` text and adds one managed block.
#[test]
fn init_agents_preserves_hand_written_agents_md() {
    let root = tempfile::tempdir().expect("tempdir");
    let agents_md = root.path().join("AGENTS.md");
    std::fs::write(&agents_md, "# Mine\n\nnotes\n").expect("seed AGENTS.md");

    assert_exit(&init_in(root.path(), &["--agents"]), 0);
    let text = std::fs::read_to_string(&agents_md).expect("read AGENTS.md");
    assert!(
        text.starts_with("# Mine\n\nnotes\n"),
        "the hand-written text survives: {text}"
    );
    assert_eq!(
        text.matches(BEGIN_MARKER).count(),
        1,
        "one managed block: {text}"
    );
}

/// `init --force --agents` re-scaffolds the workspace and replaces a stale block in place, keeping
/// the text around it.
#[test]
fn init_force_agents_rescaffolds_and_refreshes_one_block() {
    let ws = Workspace::init_with_prefix(Some("proj"));
    let agents_md = ws.root().join("AGENTS.md");
    std::fs::write(
        &agents_md,
        "top\n<!-- BEGIN unblock -->\nSTALE BLOCK\n<!-- END unblock -->\nbottom\n",
    )
    .expect("seed AGENTS.md");

    let out = ws
        .cmd()
        .args(["init", "--force", "--agents"])
        .output()
        .expect("run init --force --agents");
    assert_exit(&out, 0);
    let text = std::fs::read_to_string(&agents_md).expect("read AGENTS.md");
    assert_eq!(
        text.matches(BEGIN_MARKER).count(),
        1,
        "one managed block: {text}"
    );
    assert!(
        !text.contains("STALE BLOCK"),
        "the stale block is replaced: {text}"
    );
    assert!(text.starts_with("top\n"), "the text above survives: {text}");
    assert!(
        text.ends_with("bottom\n"),
        "the text below survives: {text}"
    );
    let config = std::fs::read_to_string(ws.config_path()).expect("read config.toml");
    assert!(
        config.contains("id_prefix = \"ub\""),
        "the forced scaffold replaces config.toml: {config}"
    );
}

/// `init --agents` writes `AGENTS.md` beside the workspace it scaffolded, even when
/// `CLAUDE_PROJECT_DIR` names another initialized workspace.
#[test]
fn init_agents_ignores_an_ambient_claude_project_dir() {
    let other = Workspace::init();
    let fresh = tempfile::tempdir().expect("tempdir");

    let out = common::unblock_in(fresh.path())
        .env("CLAUDE_PROJECT_DIR", other.root())
        .args(["init", "--agents"])
        .output()
        .expect("run init --agents");
    assert_exit(&out, 0);
    assert!(
        fresh.path().join("AGENTS.md").is_file(),
        "AGENTS.md lands beside the new workspace"
    );
    assert!(
        !other.root().join("AGENTS.md").exists(),
        "the ambient workspace is untouched"
    );
}

/// `init --agents --dir <root>/.unblock` writes `AGENTS.md` beside that `.unblock/`, not in the
/// cwd.
#[test]
fn init_agents_with_explicit_dir_writes_next_to_that_dot_unblock() {
    let cwd = tempfile::tempdir().expect("tempdir");
    let target_root = tempfile::tempdir().expect("tempdir");

    let out = common::unblock_in(cwd.path())
        .args(["init", "--agents", "--dir"])
        .arg(target_root.path().join(".unblock"))
        .output()
        .expect("run init --agents --dir");
    assert_exit(&out, 0);
    assert!(
        target_root.path().join("AGENTS.md").is_file(),
        "AGENTS.md lands beside the --dir workspace"
    );
    assert!(
        !cwd.path().join("AGENTS.md").exists(),
        "the cwd gets no AGENTS.md"
    );
}

/// `init --agents` scaffolds the same workspace-dir entries as a bare `init`, and `AGENTS.md` sits
/// at the root beside them.
#[test]
fn init_agents_scaffold_set_is_unchanged() {
    let root = tempfile::tempdir().expect("tempdir");
    assert_exit(&init_in(root.path(), &["--agents"]), 0);

    let entries: BTreeSet<String> = std::fs::read_dir(root.path().join(".unblock"))
        .expect("read .unblock")
        .map(|entry| {
            entry
                .expect("dir entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let expected: BTreeSet<String> = ["config.toml", "unblock.db", ".write.lock"]
        .into_iter()
        .map(str::to_string)
        .collect();
    assert_eq!(
        entries, expected,
        "AGENTS.md stays out of the workspace dir"
    );
    assert!(
        root.path().join("AGENTS.md").is_file(),
        "AGENTS.md sits at the workspace root"
    );
}

/// The report's last row is `agents_path`, which names `AGENTS.md` under the canonical root, not
/// under the symlink `--dir` went through.
#[cfg(unix)]
#[test]
fn init_agents_reports_agents_path_last() {
    let real = tempfile::tempdir().expect("tempdir");
    let links = tempfile::tempdir().expect("tempdir");
    let link = links.path().join("L");
    std::os::unix::fs::symlink(real.path(), &link).expect("symlink L to the real root");

    let report = scaffold_report(&init_dir(
        links.path(),
        &link.join(".unblock"),
        &["--agents"],
    ));
    let last = report["findings"]
        .as_array()
        .and_then(|findings| findings.last())
        .expect("a findings array");
    assert_eq!(last["label"], "agents_path", "{report}");
    let canonical = std::fs::canonicalize(real.path()).expect("canonicalize the real root");
    assert_eq!(
        last["detail"],
        canonical.join("AGENTS.md").display().to_string(),
        "{report}"
    );
}

/// `init --agents` prints the shared "wrote X" note and no hint.
#[test]
fn init_agents_prints_no_hint() {
    let root = tempfile::tempdir().expect("tempdir");
    let out = init_in(root.path(), &["--agents"]);
    assert_exit(&out, 0);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.lines().any(|line| line.starts_with("wrote ")),
        "the shared note: {stderr}"
    );
    assert!(
        hint_lines(&out.stderr).is_empty(),
        "--agents prints no hint: {stderr}"
    );
}

/// `-q` does not silence the shared "wrote X" note, because `output::diag` ignores `-q`. The
/// follow-up that makes `diag` honour `-q` flips this cell on purpose.
#[test]
fn init_agents_quiet_still_prints_the_wrote_note() {
    let root = tempfile::tempdir().expect("tempdir");
    let out = init_in(root.path(), &["--agents", "-q"]);
    assert_exit(&out, 0);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.lines().any(|line| line.starts_with("wrote ")),
        "the note ignores -q: {stderr}"
    );
}

/// A successful bare `init` prints exactly one hint line on stderr in every format, stdout stays
/// the report alone, and no `AGENTS.md` appears.
#[test]
fn bare_init_hints_unblock_agents_on_stderr_only() {
    for format in ["json", "robot", "plain", "markdown", "csv"] {
        let root = tempfile::tempdir().expect("tempdir");
        let out = init_in(root.path(), &["--output", format]);
        assert_exit(&out, 0);

        let hints = hint_lines(&out.stderr);
        assert_eq!(hints.len(), 1, "{format}: exactly one hint line: {hints:?}");
        assert!(
            hints[0].contains("unblock agents --dir '"),
            "{format}: the hint names the command: {}",
            hints[0]
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            !stdout.contains("hint:"),
            "{format}: the hint stays off stdout: {stdout}"
        );
        match format {
            "json" | "robot" => {
                serde_json::from_slice::<Value>(&out.stdout)
                    .unwrap_or_else(|e| panic!("{format}: one JSON document: {e}: {stdout}"));
            }
            "csv" => assert_eq!(
                common::csv_report(&out.stdout).len(),
                5,
                "{format}: the five scaffold records: {stdout}"
            ),
            _ => {}
        }
        assert!(
            !root.path().join("AGENTS.md").exists(),
            "{format}: a bare init writes no AGENTS.md"
        );
    }
}

/// The hint names the canonical workspace dir, even for a relative `--dir`.
#[cfg(unix)]
#[test]
fn bare_init_hint_path_is_the_canonical_dot_unblock() {
    let cwd = tempfile::tempdir().expect("tempdir");
    let out = init_in(cwd.path(), &["--dir", "sub/.unblock"]);
    assert_exit(&out, 0);

    let retry = printed_retry(&out);
    let canonical = std::fs::canonicalize(cwd.path()).expect("canonicalize the cwd");
    assert_eq!(
        Path::new(quoted_dir(&retry)),
        canonical.join("sub").join(".unblock"),
        "the hint names the canonical workspace dir: {retry}"
    );
}

/// The printed retry command runs verbatim in `sh` for a root whose name holds a space, `$`, a
/// backtick, double quotes, a backslash, a single quote and `!#`. It binds the workspace it names,
/// although `CLAUDE_PROJECT_DIR` names another one.
#[cfg(unix)]
#[test]
fn bare_init_hint_runs_verbatim_in_sh() {
    let base = tempfile::tempdir().expect("tempdir");
    let root = base
        .path()
        .join("sp ace $HOME `id` \"dq\" back\\slash it's !#");
    std::fs::create_dir(&root).expect("mkdir the root");
    let other = Workspace::init();

    let out = init_in(&root, &[]);
    assert_exit(&out, 0);
    let retry = printed_retry(&out);

    let elsewhere = tempfile::tempdir().expect("tempdir");
    let ran = run_retry_in_sh(&retry, elsewhere.path(), other.root());
    assert_exit(&ran, 0);
    assert!(
        root.join("AGENTS.md").is_file(),
        "the retry writes AGENTS.md beside the workspace it names: {retry}"
    );
    assert!(
        !other.root().join("AGENTS.md").exists(),
        "the CLAUDE_PROJECT_DIR workspace is untouched: {retry}"
    );
}

/// `-q` suppresses the hint.
#[test]
fn quiet_init_prints_no_hint() {
    let root = tempfile::tempdir().expect("tempdir");
    let out = init_in(root.path(), &["-q"]);
    assert_exit(&out, 0);
    assert!(
        hint_lines(&out.stderr).is_empty(),
        "-q suppresses the hint: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A refused `init --agents` ends its message with the retry text for the refused directory,
/// canonicalized, while the message head keeps the path as given. It writes nothing and prints no
/// hint, in json and in plain.
#[cfg(unix)]
#[test]
fn refused_init_agents_names_the_retry_text_and_writes_nothing() {
    let real = tempfile::tempdir().expect("tempdir");
    let links = tempfile::tempdir().expect("tempdir");
    let link = links.path().join("L");
    std::os::unix::fs::symlink(real.path(), &link).expect("symlink L to the real root");
    let dot = link.join(".unblock");
    scaffold_report(&init_dir(links.path(), &dot, &[]));

    let canonical_dot = std::fs::canonicalize(real.path())
        .expect("canonicalize the real root")
        .join(".unblock");
    let expected = format!(
        "{}{REFUSAL_RETRY_PREFIX}unblock agents --dir '{}'",
        already_initialized(&dot),
        canonical_dot.display()
    );

    let refused = init_dir(links.path(), &dot, &["--agents"]);
    assert_eq!(refusal_message(&refused), expected);
    assert!(
        hint_lines(&refused.stderr).is_empty(),
        "a refusal prints no hint"
    );

    let plain = common::unblock_in(links.path())
        .args(["init", "--agents", "--output", "plain", "--dir"])
        .arg(&dot)
        .output()
        .expect("run init --agents -o plain");
    assert_exit(&plain, 2);
    assert!(plain.stdout.is_empty(), "the plain error goes to stderr");
    let stderr = String::from_utf8_lossy(&plain.stderr);
    let error_line = format!("error[ALREADY_INITIALIZED]: {expected}");
    assert!(
        stderr.lines().any(|line| line == error_line),
        "the plain error line: {stderr}"
    );
    assert!(
        hint_lines(&plain.stderr).is_empty(),
        "a refusal prints no hint: {stderr}"
    );
    assert!(
        !real.path().join("AGENTS.md").exists(),
        "a refused init writes no AGENTS.md"
    );
}

/// A refused `init --agents` at a root holding an `_unblock/` workspace names that `_unblock/`,
/// and its retry text, run through `sh`, writes `AGENTS.md` beside it.
#[cfg(unix)]
#[test]
fn refused_init_agents_on_an_underscore_root_names_its_canonical_dir() {
    let root = tempfile::tempdir().expect("tempdir");
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let underscore = root.path().join("_unblock");
    scaffold_report(&init_dir(elsewhere.path(), &underscore, &[]));

    let message = refusal_message(&init_dir(elsewhere.path(), root.path(), &["--agents"]));
    let canonical = std::fs::canonicalize(root.path())
        .expect("canonicalize the root")
        .join("_unblock");
    assert_eq!(
        message,
        format!(
            "{}{REFUSAL_RETRY_PREFIX}unblock agents --dir '{}'",
            already_initialized(&underscore),
            canonical.display()
        )
    );

    let (_, retry) = message
        .split_once(REFUSAL_RETRY_PREFIX)
        .expect("the retry text");
    let other = Workspace::init();
    let ran = run_retry_in_sh(retry, elsewhere.path(), other.root());
    assert_exit(&ran, 0);
    assert!(
        root.path().join("AGENTS.md").is_file(),
        "the retry writes AGENTS.md beside the _unblock/ workspace"
    );
    assert!(
        !other.root().join("AGENTS.md").exists(),
        "the CLAUDE_PROJECT_DIR workspace is untouched"
    );
}

/// A refused `init --agents` beside an initialized sibling names the sibling, and its retry text
/// names the sibling's canonical dir.
#[cfg(unix)]
#[test]
fn refused_init_agents_beside_an_initialized_sibling_names_the_sibling() {
    let root = tempfile::tempdir().expect("tempdir");
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let underscore = root.path().join("_unblock");
    scaffold_report(&init_dir(elsewhere.path(), &underscore, &[]));

    let dot = root.path().join(".unblock");
    let message = refusal_message(&init_dir(elsewhere.path(), &dot, &["--agents"]));
    let canonical = std::fs::canonicalize(root.path())
        .expect("canonicalize the root")
        .join("_unblock");
    assert_eq!(
        message,
        format!(
            "{}{REFUSAL_RETRY_PREFIX}unblock agents --dir '{}'",
            already_initialized(&underscore),
            canonical.display()
        )
    );
    assert!(
        !dot.exists(),
        "no .unblock/ appears to hide the _unblock/ workspace"
    );
    assert!(
        !root.path().join("AGENTS.md").exists(),
        "a refused init writes no AGENTS.md"
    );
}

/// A binds-first refusal under `--agents` carries no retry text, because the `.unblock/` it names
/// holds no workspace.
#[test]
fn refused_init_agents_beside_an_empty_dot_unblock_carries_no_retry_text() {
    let root = tempfile::tempdir().expect("tempdir");
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let dot = root.path().join(".unblock");
    std::fs::create_dir(&dot).expect("mkdir .unblock");
    let underscore = root.path().join("_unblock");

    let message = refusal_message(&init_dir(elsewhere.path(), &underscore, &["--agents"]));
    assert_eq!(message, binds_first(&dot, &underscore));
    assert!(!underscore.exists(), "the refused target is never created");
    assert!(
        !root.path().join("AGENTS.md").exists(),
        "a refused init writes no AGENTS.md"
    );
}

/// In layout L, a bare `init --agents` at the root is refused naming the empty `.unblock/`, with no
/// retry text, although the `_unblock/` beside it holds a workspace.
#[test]
fn refused_init_agents_at_a_root_whose_empty_dot_unblock_binds_first_carries_no_retry_text() {
    let root = tempfile::tempdir().expect("tempdir");
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let underscore = root.path().join("_unblock");
    scaffold_report(&init_dir(elsewhere.path(), &underscore, &[]));
    let saved = std::fs::read(underscore.join("config.toml")).expect("read config.toml");
    let dot = root.path().join(".unblock");
    std::fs::create_dir(&dot).expect("mkdir .unblock");

    let out = common::unblock_in(root.path())
        .args(["init", "--agents", "--output", "json"])
        .output()
        .expect("run init --agents");
    let message = refusal_message(&out);
    assert!(
        message.contains("already exists, and discovery binds it before"),
        "the binds-first refusal: {message}"
    );
    assert!(!message.contains("; to write"), "no retry text: {message}");

    // The child reads its cwd as the physical path, which `std::fs::canonicalize` matches on unix.
    #[cfg(unix)]
    {
        let canonical_root = std::fs::canonicalize(root.path()).expect("canonicalize the root");
        assert_eq!(
            message,
            binds_first(
                &canonical_root.join(".unblock"),
                &canonical_root.join("_unblock")
            )
        );
    }
    assert_empty_dir(&dot);
    assert_eq!(
        std::fs::read(underscore.join("config.toml")).expect("re-read config.toml"),
        saved,
        "the _unblock/ workspace is untouched"
    );
    assert!(
        !root.path().join("AGENTS.md").exists(),
        "a refused init writes no AGENTS.md"
    );
}

/// A refused bare `init` keeps the message `workspace already initialized at <path>`, with the
/// `--dir` argument exactly as given, and prints no hint.
#[test]
fn refused_bare_init_prints_no_hint_and_keeps_its_message() {
    let root = tempfile::tempdir().expect("tempdir");
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let dot = root.path().join(".unblock");
    scaffold_report(&init_dir(elsewhere.path(), &dot, &[]));

    let refused = init_dir(elsewhere.path(), &dot, &[]);
    assert_eq!(refusal_message(&refused), already_initialized(&dot));
    assert!(
        hint_lines(&refused.stderr).is_empty(),
        "a refusal prints no hint: {}",
        String::from_utf8_lossy(&refused.stderr)
    );
}

/// A failed `AGENTS.md` write after a good scaffold exits 8 with one JSON error and keeps the
/// scaffold, in a `.unblock` and an `_unblock` workspace dir alike. The message names the canonical
/// workspace dir, the `AGENTS.md` path and, last, the retry text, which finishes the job from
/// another directory once the cause is gone.
#[test]
fn init_agents_write_failure_keeps_the_scaffold_and_exits_8() {
    for name in [".unblock", "_unblock"] {
        let cwd = tempfile::tempdir().expect("tempdir");
        let sub = cwd.path().join("sub");
        std::fs::create_dir_all(sub.join("AGENTS.md")).expect("mkdir sub/AGENTS.md");
        let dir = format!("sub/{name}");

        let out = init_in(cwd.path(), &["--agents", "--dir", &dir, "--output", "json"]);
        assert_exit(&out, 8);
        let error: Value =
            serde_json::from_slice(&out.stdout).expect("ONE JSON error document on stdout");
        assert_eq!(error["code"], "IO_ERROR", "{name}: {error}");
        assert!(
            error.get("findings").is_none(),
            "{name}: no report renders: {error}"
        );

        let message = error["message"].as_str().expect("a string message");
        let rest = message
            .strip_prefix("workspace initialized at ")
            .unwrap_or_else(|| panic!("the message head: {message}"));
        let (unblock_dir, rest) = rest
            .split_once(", but could not update ")
            .unwrap_or_else(|| panic!("the could-not-update clause: {message}"));
        let (agents_md, rest) = rest
            .split_once(": ")
            .unwrap_or_else(|| panic!("the OS error: {message}"));
        let (_os_error, retry) = rest
            .split_once("; to finish, run ")
            .unwrap_or_else(|| panic!("the retry text: {message}"));
        assert_canonical(
            Path::new(unblock_dir),
            &sub.join(name),
            &format!("the message names the canonical workspace dir: {message}"),
        );
        assert_canonical(
            Path::new(agents_md),
            &sub.join("AGENTS.md"),
            &format!("the message names the canonical AGENTS.md path: {message}"),
        );
        assert_eq!(
            retry,
            format!("unblock agents --dir '{unblock_dir}'"),
            "the message ends with the retry text for that dir: {message}"
        );

        let scaffold = sub.join(name);
        assert!(
            scaffold.join("config.toml").exists() && scaffold.join("unblock.db").exists(),
            "{name}: the scaffold stays"
        );
        assert_exit(&init_in(cwd.path(), &["--dir", &dir]), 2);

        std::fs::remove_dir(sub.join("AGENTS.md")).expect("remove the AGENTS.md directory");
        let elsewhere = tempfile::tempdir().expect("tempdir");
        let finished = common::unblock_in(elsewhere.path())
            .args(["agents", "--dir"])
            .arg(quoted_dir(retry))
            .output()
            .expect("run the retry");
        assert_exit(&finished, 0);
        let text = std::fs::read_to_string(sub.join("AGENTS.md")).expect("AGENTS.md is a file now");
        assert_eq!(
            text.matches(BEGIN_MARKER).count(),
            1,
            "{name}: one managed block: {text}"
        );
    }
}

/// A scaffold failure under `--agents` keeps its own exit code and writes no `AGENTS.md`.
#[test]
fn init_agents_scaffold_failure_writes_no_agents_md() {
    let ws = Workspace::init();
    std::fs::remove_file(ws.config_path()).expect("remove config.toml");
    std::fs::create_dir(ws.config_path()).expect("mkdir config.toml");

    let out = ws
        .cmd()
        .args(["init", "--force", "--agents", "--output", "json"])
        .output()
        .expect("run init --force --agents");
    assert_exit(&out, 8);
    assert!(
        !ws.root().join("AGENTS.md").exists(),
        "a failed scaffold writes no AGENTS.md"
    );
}

/// An open failure under `--agents` keeps its own exit code and writes no `AGENTS.md`.
#[test]
fn init_agents_open_failure_writes_no_agents_md() {
    let root = tempfile::tempdir().expect("tempdir");
    let out = common::unblock_in(root.path())
        .env("UNBLOCK_OUTPUT_FORMAT", "xml")
        .args(["init", "--agents"])
        .output()
        .expect("run init --agents");
    assert_exit(&out, 7);
    assert!(
        !root.path().join("AGENTS.md").exists(),
        "a failed open writes no AGENTS.md"
    );
}

/// Runs `init` with `flags` under `-o json` in a fresh root and returns its report with the root
/// written as `<root>`, so a snapshot pins the shape and not a tempdir path.
///
/// It canonicalizes each path detail, strips the canonical root and joins the remaining components
/// with `/`, so the snapshot bytes match on every host.
fn redacted_init_report(flags: &[&str]) -> Value {
    let root = tempfile::tempdir().expect("tempdir");
    let args = [flags, &["--output", "json"]].concat();
    let mut report = scaffold_report(&init_in(root.path(), &args));
    let canonical_root = std::fs::canonicalize(root.path()).expect("canonicalize the root");
    for finding in report["findings"].as_array_mut().expect("a findings array") {
        let detail = finding["detail"].as_str().expect("a string detail");
        if Path::new(detail).is_absolute() {
            let redacted = redact_root(Path::new(detail), &canonical_root);
            finding["detail"] = Value::String(redacted);
        }
    }
    report
}

/// Returns `<root>` followed by the components of canonical `path` below `canonical_root`, each
/// after a `/`.
fn redact_root(path: &Path, canonical_root: &Path) -> String {
    let canonical = std::fs::canonicalize(path).expect("canonicalize a path detail");
    let below = canonical
        .strip_prefix(canonical_root)
        .unwrap_or_else(|_| panic!("{} lies under the root", path.display()));
    let mut redacted = String::from("<root>");
    for component in below.components() {
        redacted.push('/');
        redacted.push_str(component.as_os_str().to_str().expect("a UTF-8 component"));
    }
    redacted
}

#[test]
fn init_report_default_is_snapshot_pinned() {
    insta::assert_json_snapshot!("init_report_default", redacted_init_report(&[]));
}

#[test]
fn init_report_with_agents_is_snapshot_pinned() {
    insta::assert_json_snapshot!(
        "init_report_with_agents",
        redacted_init_report(&["--agents"])
    );
}

/// A stdout whose reader has exited fails the report write with exit 8, and no hint prints,
/// because the hint follows a successful report.
#[cfg(unix)]
#[test]
fn bare_init_with_a_broken_stdout_exits_8_and_prints_no_hint() {
    let root = tempfile::tempdir().expect("tempdir");
    let (reader, writer) = std::io::pipe().expect("create a pipe");
    drop(reader);

    let out = common::unblock_in(root.path())
        .args(["init", "--output", "plain"])
        .stdout(writer)
        .output()
        .expect("run init");
    assert_exit(&out, 8);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.starts_with("error[IO_ERROR]"),
        "the report write fails with IO_ERROR: {stderr}"
    );
    assert!(
        hint_lines(&out.stderr).is_empty(),
        "a failed report prints no hint: {stderr}"
    );
    let scaffold = root.path().join(".unblock");
    assert!(
        scaffold.join("config.toml").exists() && scaffold.join("unblock.db").exists(),
        "the scaffold stays"
    );
}

/// A stderr whose reader has exited fails the hint write, and a completed bare `init` still exits
/// 0.
#[cfg(unix)]
#[test]
fn bare_init_with_a_broken_stderr_still_exits_0() {
    let root = tempfile::tempdir().expect("tempdir");
    let (reader, writer) = std::io::pipe().expect("create a pipe");
    drop(reader);

    let out = common::unblock_in(root.path())
        .arg("init")
        .stderr(writer)
        .output()
        .expect("run init");
    assert_eq!(
        out.status.code(),
        Some(0),
        "a failed hint must not change the exit code"
    );
    let scaffold = root.path().join(".unblock");
    assert!(
        scaffold.join("config.toml").exists() && scaffold.join("unblock.db").exists(),
        "the scaffold exists"
    );
}
