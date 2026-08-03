//! FR-9 no-drift parity: a lifecycle op routed through the CLI (`unblock migrate`/`doctor`) yields
//! the SAME engine-level outcome as the identical op called on `Session` directly. The CLI is a thin
//! adapter over the single mutation home (D14) — it must not compute anything the engine doesn't.
//!
//! Both sides run against the SAME on-disk workspace (opened through the SAME config facade the CLI
//! uses, `open_with_storage_with_cli`), so any drift between the CLI's rendered report and the engine
//! truth fails here.

mod common;

use common::Workspace;
use serde_json::Value;
use unblock_config::{CliOverrides, open_with_storage_with_cli};
use unblock_engine::{Session, SessionConfig};

/// Open a `Session` directly over `ws`'s workspace via the SAME facade the CLI dispatches through.
async fn open_session(ws: &Workspace) -> Session {
    let overrides = CliOverrides::new().with_dir(ws.unblock_dir());
    let ctx = open_with_storage_with_cli(&overrides)
        .await
        .expect("open workspace via the config facade");
    Session::open(
        ctx,
        SessionConfig {
            import_on_open: false,
            ..SessionConfig::default()
        },
    )
    .await
    .expect("open session")
}

/// Pull a CLI JSON report's finding detail by label.
fn detail<'a>(report: &'a Value, label: &str) -> Option<&'a str> {
    report["findings"]
        .as_array()?
        .iter()
        .find(|f| f["label"] == label)
        .and_then(|f| f["detail"].as_str())
}

/// **FR-9 parity for `migrate`, RE-AIMED at D46 clause (10).**
///
/// The shipped cell asserted all THREE rendered values against `MigrateOutcome`. That surplus stops
/// being true by construction: since D46 `schema_from`/`applied` come from the PRE-OPEN stamp the
/// config facade records, so the three-way equality holds only on an already-current workspace —
/// i.e. it would stay GREEN while pinning nothing. `schema_to` keeps the verbatim engine parity (it
/// still comes straight from `MigrateOutcome.to`); `schema_from`/`applied` get their own assertion
/// against `WorkspaceContext::schema_version_before_migrate` on a workspace where the pre-open and
/// post-migration stamps genuinely DIFFER.
///
/// **That second half needs a SECOND, IDENTICALLY SEEDED workspace, and stating why is the whole
/// specification of the cell:** ONE workspace cannot show it in EITHER order. The CLI runs as a CHILD
/// process and the engine side opens the same dir IN-PROCESS, and BOTH opens go through the config
/// facade, which migrates — so whichever runs first advances the stamp and the other reads an
/// already-current database with the delta gone.
///
/// MUTANT KILLED: sourcing `schema_from` from `MigrateOutcome.from` again (the pre-ruling code) — on
/// the never-migrated pair the CLI would render `2` instead of the captured `0`.
///
/// MUTANT KILLED: computing `applied` from `outcome.applied` — it is `false` post-facade, so the
/// second assertion goes red.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn migrate_cli_matches_session_directly() {
    // Half 1 — the verbatim engine parity, on the already-current workspace this cell always built.
    let ws = Workspace::init();
    let cli_report = run_migrate_json(&ws);
    let cli_to = detail(&cli_report, "schema_to").expect("schema_to");

    let session = open_session(&ws).await;
    let outcome = session.migrate().await.expect("Session::migrate");
    assert_eq!(
        cli_to,
        outcome.to.to_string(),
        "schema_to parity — still verbatim from MigrateOutcome"
    );
    drop(session);

    // Half 2 — the PRE-OPEN delta, on TWO identically seeded never-migrated workspaces (pre-open
    // stamp `0`), because a single workspace cannot exhibit it to both observers.
    let cli_ws = never_migrated_workspace();
    let engine_ws = never_migrated_workspace();

    let cli_report = run_migrate_json(&cli_ws);
    let cli_from = detail(&cli_report, "schema_from").expect("schema_from");
    let cli_applied = detail(&cli_report, "applied").expect("applied");

    let overrides = CliOverrides::new().with_dir(engine_ws.unblock_dir());
    let ctx = open_with_storage_with_cli(&overrides)
        .await
        .expect("open workspace via the config facade");
    let pre_open = ctx.schema_version_before_migrate;
    let session = Session::open(ctx, SessionConfig::default())
        .await
        .expect("open session");
    let outcome = session.migrate().await.expect("Session::migrate");
    drop(session);

    assert_eq!(
        pre_open, 0,
        "the identically seeded fixture really is never-migrated"
    );
    assert_eq!(
        cli_from,
        pre_open.to_string(),
        "the CLI renders the PRE-OPEN stamp the facade recorded, not the engine's post-open `from`"
    );
    assert_eq!(
        cli_applied,
        (pre_open != outcome.to).to_string(),
        "`applied` is `the pre-open stamp differs from MigrateOutcome.to`"
    );
    assert_ne!(
        pre_open, outcome.to,
        "the two genuinely differ here — this is the workspace shape the shipped cell could not build"
    );
}

/// Run `unblock migrate --output json` against `ws` and parse the report (exit 0 asserted).
fn run_migrate_json(ws: &Workspace) -> Value {
    let out = ws
        .cmd()
        .args(["migrate", "--output", "json"])
        .output()
        .expect("run migrate");
    assert_eq!(
        out.status.code(),
        Some(0),
        "migrate must succeed; stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    serde_json::from_slice(&out.stdout).expect("valid JSON migrate report")
}

/// A `.unblock/` carrying `config.toml` and NO `unblock.db`, so the facade's own `open_local` creates
/// the file at stamp `0` — the never-migrated fixture, in the pre-migration state D46 repairs.
fn never_migrated_workspace() -> Workspace {
    let ws = Workspace::init();
    let db = ws.db_path();
    for suffix in ["", "-wal", "-shm"] {
        let mut path = db.clone().into_os_string();
        path.push(suffix);
        let _ = std::fs::remove_file(std::path::PathBuf::from(path));
    }
    assert!(!db.exists(), "the never-migrated fixture has no unblock.db");
    ws
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_cli_renders_session_doctor_verbatim() {
    // T3.3 (D29/F4): the cli RENDERS the wired `Session::doctor()` `DiagnosticReport` directly (no
    // transformation), so the rendered findings match the engine's report exactly — the FR-9 no-drift
    // guarantee for the doctor path.
    let ws = Workspace::init();

    // CLI side: `unblock doctor --output json` (child process; it closes its session on exit).
    let out = ws
        .cmd()
        .args(["doctor", "--output", "json"])
        .output()
        .expect("run doctor");
    assert_eq!(out.status.code(), Some(0));
    let cli_report: Value = serde_json::from_slice(&out.stdout).expect("valid JSON doctor report");

    // Engine side: `Session::doctor()` over the SAME workspace (after the child exited — single-MCP-server).
    let session = open_session(&ws).await;
    let engine_report = session.doctor().await.expect("Session::doctor");

    // Kind parity + verbatim finding parity (label + detail, in order): the cli adds nothing.
    assert_eq!(
        cli_report["kind"], "info",
        "doctor reuses DiagnosticKind::Info"
    );
    let cli_findings = cli_report["findings"].as_array().expect("findings array");
    assert_eq!(
        cli_findings.len(),
        engine_report.findings.len(),
        "no cli-added findings (verbatim render); cli: {cli_report}"
    );
    for (cli_finding, engine_finding) in cli_findings.iter().zip(&engine_report.findings) {
        assert_eq!(cli_finding["label"], engine_finding.label, "label parity");
        assert_eq!(
            cli_finding["detail"], engine_finding.detail,
            "detail parity"
        );
    }
    // A clean workspace is healthy with clean integrity (sanity on the shared content).
    assert_eq!(detail(&cli_report, "health"), Some("healthy"));
    assert_eq!(detail(&cli_report, "integrity"), Some("ok"));
}
