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
        after_first.contains("unblock serve"),
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
