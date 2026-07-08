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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn migrate_cli_matches_session_directly() {
    let ws = Workspace::init();

    // CLI side: `unblock migrate --output json` (dispatch → Session::migrate → report).
    let out = ws
        .cmd()
        .args(["migrate", "--output", "json"])
        .output()
        .expect("run migrate");
    assert_eq!(out.status.code(), Some(0));
    let cli_report: Value = serde_json::from_slice(&out.stdout).expect("valid JSON migrate report");
    let cli_from = detail(&cli_report, "schema_from").expect("schema_from");
    let cli_to = detail(&cli_report, "schema_to").expect("schema_to");
    let cli_applied = detail(&cli_report, "applied").expect("applied");

    // Engine side: `Session::migrate()` over the SAME workspace.
    let session = open_session(&ws).await;
    let outcome = session.migrate().await.expect("Session::migrate");

    // Parity: the CLI's rendered from/to/applied are exactly the engine outcome (no CLI drift, FR-9).
    assert_eq!(cli_from, outcome.from.to_string(), "schema_from parity");
    assert_eq!(cli_to, outcome.to.to_string(), "schema_to parity");
    assert_eq!(cli_applied, outcome.applied.to_string(), "applied parity");
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

    // Engine side: `Session::doctor()` over the SAME workspace (after the child exited — single-serve).
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
