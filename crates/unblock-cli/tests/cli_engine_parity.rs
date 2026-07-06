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
use unblock_engine::{DiagnosticKind, Session, SessionConfig};

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
async fn doctor_cli_matches_session_diagnostics_and_integrity() {
    let ws = Workspace::init();

    // CLI side: `unblock doctor --output json`.
    let out = ws
        .cmd()
        .args(["doctor", "--output", "json"])
        .output()
        .expect("run doctor");
    assert_eq!(out.status.code(), Some(0));
    let cli_report: Value = serde_json::from_slice(&out.stdout).expect("valid JSON doctor report");

    // Engine side: the SAME composition the CLI performs (AF-1) — integrity + Stats/Lint/Info.
    let session = open_session(&ws).await;
    let integrity = session.integrity_check().await.expect("integrity_check");
    let stats = session
        .diagnostics(DiagnosticKind::Stats, None)
        .await
        .expect("stats");
    let info = session
        .diagnostics(DiagnosticKind::Info, None)
        .await
        .expect("info");

    // Integrity parity: a clean DB → engine reports empty → CLI header is `ok`.
    assert!(integrity.is_empty(), "clean DB integrity is empty");
    assert_eq!(
        detail(&cli_report, "integrity"),
        Some("ok"),
        "CLI integrity header parity"
    );

    // Stats parity: every engine Stats finding is rendered as a `stats.<label>` row with the same
    // detail (the CLI does no transformation beyond the label prefix, AF-1/AD-2).
    for finding in &stats.findings {
        let cli_label = format!("stats.{}", finding.label);
        assert_eq!(
            detail(&cli_report, &cli_label),
            Some(finding.detail.as_str()),
            "stats parity for `{}`",
            finding.label
        );
    }
    // Info parity likewise (the `info.actor`/`info.workspace_dir`/... rows).
    for finding in &info.findings {
        let cli_label = format!("info.{}", finding.label);
        assert_eq!(
            detail(&cli_report, &cli_label),
            Some(finding.detail.as_str()),
            "info parity for `{}`",
            finding.label
        );
    }
}
