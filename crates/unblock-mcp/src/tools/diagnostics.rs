//! Tool **#7 `diagnostics`** — the 7-kind read-path diagnostics (spine §5.1/§5.2, FR-15).
//!
//! Maps `DiagnosticsInput{kind}` → the model [`DiagnosticKind`] → `Session::diagnostics(kind)` (the
//! BUILD-now, pure-DB read path) returning a [`unblock_model::DiagnosticReport`]. It does NOT route
//! through `doctor()`/`recover()` (the T3.3 health seam) — see the spine §4.1 precision note.
//!
//! - `version` embeds [`crate::CONTRACT_VERSION`] in the report (the mcp `contract_version` SSOT).
//! - `changelog{since}` accepts the wire `since` but DROPS it pre-call — there is no `Session`
//!   parameter for it at T2.2 (the `since` window threading is T2.7); the report uses the full window.
//! - No git (FR-15/NFR-6) — `diagnostics` is pure-DB.

use chrono::{DateTime, Utc};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::tool;
use serde::{Deserialize, Serialize};
use unblock_model::{DiagnosticFinding, DiagnosticKind, DiagnosticReport};

use crate::options::CONTRACT_VERSION;
use crate::server::UnblockServer;
use crate::tools::{engine_err_json, err_json, ok_json};

/// The `diagnostics` tool input (spine §5.2 — EXACT shape; mirrors [`DiagnosticKind`]).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum DiagnosticsInput {
    /// Aggregate statistics.
    Stats {},
    /// General workspace info.
    Info {},
    /// Where the workspace lives.
    Where {},
    /// Version information (embeds the contract version).
    Version {},
    /// Lint findings.
    Lint {},
    /// The changelog of closed issues (the `since` window is T2.7; dropped here).
    Changelog {
        /// Optional since-window (accepted but not yet threaded — T2.7).
        #[serde(default)]
        since: Option<DateTime<Utc>>,
    },
    /// Orphan candidates (FR-15).
    Orphans {},
}

impl DiagnosticsInput {
    /// Map the wire discriminator to the model [`DiagnosticKind`] (total).
    fn to_diagnostic_kind(&self) -> DiagnosticKind {
        match self {
            Self::Stats {} => DiagnosticKind::Stats,
            Self::Info {} => DiagnosticKind::Info,
            Self::Where {} => DiagnosticKind::Where,
            Self::Version {} => DiagnosticKind::Version,
            Self::Lint {} => DiagnosticKind::Lint,
            // `since` is intentionally dropped pre-call (no Session parameter at T2.2 — T2.7).
            Self::Changelog { since: _ } => DiagnosticKind::Changelog,
            Self::Orphans {} => DiagnosticKind::Orphans,
        }
    }
}

#[rmcp::tool_router(router = diagnostics_router, vis = "pub(crate)")]
impl UnblockServer {
    /// Read-path diagnostics (FR-15) — pure-DB, no git.
    #[tool(
        name = "diagnostics",
        description = "Diagnostics: stats, info, where, version, lint, changelog, or orphans."
    )]
    pub(crate) async fn diagnostics(
        &self,
        Parameters(input): Parameters<DiagnosticsInput>,
    ) -> CallToolResult {
        if let Err(structured) = self.preflight(&input) {
            return err_json(&structured);
        }
        let kind = input.to_diagnostic_kind();
        match self.session.diagnostics(kind).await {
            Ok(report) => ok_json(&with_contract_version(kind, report)),
            Err(err) => engine_err_json(&err),
        }
    }
}

/// For the `version` kind, append the mcp contract version as a finding (the mcp `contract_version`
/// SSOT, F-5). Other kinds pass through unchanged.
fn with_contract_version(kind: DiagnosticKind, mut report: DiagnosticReport) -> DiagnosticReport {
    if matches!(kind, DiagnosticKind::Version) {
        report.findings.push(DiagnosticFinding {
            label: "mcp_contract_version".to_string(),
            detail: CONTRACT_VERSION.to_string(),
        });
    }
    report
}
