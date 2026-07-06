//! Tool **#7 `diagnostics`** — the 7-kind read-path diagnostics (spine §5.1/§5.2, FR-15).
//!
//! Maps `DiagnosticsInput{kind}` → the model [`DiagnosticKind`] + the changelog `since` window →
//! `Session::diagnostics(kind, since)` (the BUILD-now, pure-DB read path) returning a
//! [`unblock_model::DiagnosticReport`]. It does NOT route through `doctor()`/`recover()` (the T3.3
//! health seam) — see the spine §4.1 precision note.
//!
//! - `version` embeds [`crate::CONTRACT_VERSION`] in the report (the mcp `contract_version` SSOT).
//! - `changelog{since}` THREADS the wire `since` to `Session::diagnostics(kind, Some(since))`
//!   (D26/OQ-1 — the D19 `detect_cycles(blocking_only)` precedent: the wire default lives on the
//!   `#[serde(default)] since` field; every other kind passes `None`). No schema change → no bump.
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
    // NOTE: this doc comment + the `since` field's below are captured by `#[derive(JsonSchema)]` and
    // therefore land in the hashed schema bundle (D25/FR-12). They are kept BYTE-IDENTICAL to the
    // T2.6 pin so threading `since` into the engine (T2.7) does NOT move `CONTRACT_HASH` (no bump —
    // the wire SHAPE is unchanged; only the engine consumes `since` now). Re-wording them would move
    // the digest and force a `CONTRACT_VERSION` bump.
    Changelog {
        /// Optional since-window (accepted but not yet threaded — T2.7).
        #[serde(default)]
        since: Option<DateTime<Utc>>,
    },
    /// Orphan candidates (FR-15).
    Orphans {},
}

impl DiagnosticsInput {
    /// Map the wire discriminator to the model [`DiagnosticKind`] and the changelog `since` window
    /// (total). `since` is threaded ONLY for `Changelog`; every other kind passes `None`
    /// (D26/OQ-1 — the bare-arg + wire-default asymmetry, the D19 precedent).
    fn to_kind_and_since(&self) -> (DiagnosticKind, Option<DateTime<Utc>>) {
        match self {
            Self::Stats {} => (DiagnosticKind::Stats, None),
            Self::Info {} => (DiagnosticKind::Info, None),
            Self::Where {} => (DiagnosticKind::Where, None),
            Self::Version {} => (DiagnosticKind::Version, None),
            Self::Lint {} => (DiagnosticKind::Lint, None),
            Self::Changelog { since } => (DiagnosticKind::Changelog, *since),
            Self::Orphans {} => (DiagnosticKind::Orphans, None),
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
        let (kind, since) = input.to_kind_and_since();
        match self.session.diagnostics(kind, since).await {
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
