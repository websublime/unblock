//! `unblock init` + `unblock agents` bootstrap (FR-14, D27/AF-3).
//!
//! - init: scaffolds EXACTLY `.unblock/config.toml` + a migrated empty `unblock.db` — nothing else
//!   (NO `.gitignore`/`metadata.json`/`issues.jsonl`, D13/NFR-6/model-B). Idempotent + clobber-guarded
//!   (`--force` overwrites); `--prefix` is normalized on disk; the scaffolded config round-trips
//!   through a real workspace open (FR-9 no-drift — `migrate` succeeds against it).
//! - agents: writes a managed `AGENTS.md` block delimited by markers; a re-run updates ONLY the block
//!   (idempotent) and preserves surrounding content; the block is snapshot-pinned.

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
    // No AGENTS.md is written by init (agents is a SEPARATE command).
    assert!(
        !ws.root().join("AGENTS.md").exists(),
        "init does not write AGENTS.md"
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

/// `init -o csv` renders the scaffold report as csv, in the adapter's order.
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

/// Asserts that the report's `unblock_dir` is the canonical form of `dir`.
///
/// The CLI canonicalizes through `dunce`, which agrees with `std::fs::canonicalize` on unix. On
/// Windows `std::fs::canonicalize` adds the verbatim prefix, so there the cell compares the
/// canonical forms of both paths.
fn assert_reports_unblock_dir(report: &Value, dir: &Path) {
    let reported = Path::new(common::detail(report, "unblock_dir").expect("an unblock_dir row"));
    let canonical = std::fs::canonicalize(dir).expect("canonicalize the expected dir");
    if cfg!(unix) {
        assert_eq!(
            reported, canonical,
            "the report names the canonical target: {report}"
        );
    } else {
        let reported = std::fs::canonicalize(reported).expect("canonicalize the reported dir");
        assert_eq!(reported, canonical, "the report names the target: {report}");
    }
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
/// binds.
#[test]
fn init_dir_naming_a_root_with_an_empty_underscore_scaffolds_inside_it() {
    let root = tempfile::tempdir().expect("tempdir");
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let underscore = root.path().join("_unblock");
    std::fs::create_dir(&underscore).expect("mkdir _unblock");

    let report = scaffold_report(&init_dir(elsewhere.path(), root.path(), &[]));
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
/// so it is refused naming the `.unblock/` with or without `--force`.
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

    for flags in WITH_AND_WITHOUT_FORCE {
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
