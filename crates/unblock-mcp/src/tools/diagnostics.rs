//! Tool **#7 `diagnostics`** — the **EIGHT**-kind read-path diagnostics (spine §5.1/§5.2, FR-15;
//! seven at GA, the eighth — `dangling` — appended by D45/v1.0.1).
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
//! - `dangling` (D45) lists the dependency edges whose target denotes nothing — the read view of
//!   the class the D45 write guard now refuses. It is an ACTION ARM on THIS tool, **never a ninth
//!   tool**: the RK-3 budget (spine §6.6) is FULL at 8 and stays there. Nothing is computed here —
//!   the findings are composed in the ENGINE (`unblock_engine::diagnostics::dangling_findings`),
//!   the ONE home the CLI `doctor` fold reads too.
//! - No git (FR-15/NFR-6) — `diagnostics` is pure-DB.

use chrono::{DateTime, Utc};
// D42 SEAM: this is the CRATE-LOCAL `Parameters` (`crate::tools::args`), NOT rmcp's. It defers
// deserialization so argument errors reach the FR-11 in-band channel instead of an out-of-band
// `-32602`. The NAME IS LOAD-BEARING (rmcp-macros matches the ident `Parameters` to pick the
// published inputSchema) — see `tools/args.rs`. Do NOT "fix" this back to rmcp's wrapper.
use crate::tools::args::{Parameters, parse_args};
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::tool;
use serde::{Deserialize, Serialize};
use unblock_model::{DiagnosticFinding, DiagnosticKind, DiagnosticReport};

use crate::options::CONTRACT_VERSION;
use crate::server::UnblockServer;
use crate::tools::{engine_err_json, err_json, ok_json};

/// **CONTRACT BYTES, declared ONCE (spine §5.2 D45 note).** These bytes ship in TWO places — the
/// `#[tool(description)]` attribute below (the live `tools/list` wire; rmcp requires a LITERAL
/// there, which is why this const cannot be used in the attribute itself) and the `capabilities()`
/// tool descriptor (`resources/capabilities.rs`, which `CONTRACT_HASH` digests).
///
/// D45 REWROTE these bytes to name the new `dangling` action; that rewrite is one of the two axes
/// forcing the `unblock.mcp.v1.8` bump (the other is `schema_bundle()`'s new `oneOf` arm +
/// `DiagnosticKind` enum member). The `claim`/`comment` precedent is followed exactly: the constant
/// is the single declaration, `contract_suite::live_list_tools_equals_the_builder_eight` compares
/// `(name, description)` pairs so wire-vs-builder divergence fails, and
/// `contract_suite::the_diagnostics_tool_description_names_all_eight_kinds` pins these exact bytes
/// so a change that moved BOTH copies in step still has to be a deliberate contract act.
pub(crate) const DIAGNOSTICS_TOOL_DESCRIPTION: &str =
    "Diagnostics: stats, info, where, version, lint, changelog, orphans, or dangling.";

/// The `diagnostics` tool input (spine §5.2 — EXACT shape; mirrors [`DiagnosticKind`]).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
// §5.2a (CD-1): inject the root `"type": "object"` (the tagged-enum `oneOf` root omits it, which
// strict MCP clients reject) — the union is preserved verbatim.
#[schemars(extend("type" = "object"))]
// D42: `#[serde(deny_unknown_fields)]` — an unknown/misspelled argument is REJECTED in-band
// instead of being silently dropped. NOT recursive and inert on a flatten TARGET: every nested
// container needs its OWN attribute (see `tools/args.rs` + the CHECK-3 container guard).
#[serde(deny_unknown_fields)]
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
    /// The dependency edges whose target denotes nothing (D45).
    //
    // D45 — the `///` doc comment ABOVE is CONTRACT BYTES (spine §5.2): schemars lifts a variant
    // doc comment into the arm's `description`, which rides `schema_bundle()`, which
    // `CONTRACT_HASH` digests. Re-wording it, even harmlessly, RE-CUTS the hash and is a contract
    // change, never a comment tidy-up. The bytes are IDENTICAL to `DiagnosticKind::Dangling`'s
    // (spine §1.10) — pinned on both sides, so the wire kind and the model kind cannot drift into
    // describing the same thing two ways.
    //
    // D45 — the arm is APPENDED LAST, never inserted mid-list: schemars emits variants in
    // DECLARATION order and `CONTRACT_HASH` digests those bytes, so a mid-list insertion would move
    // the digest for a reason unrelated to the new kind. `DiagnosticKind` appends for the same
    // reason, keeping the two mirrored.
    //
    // D45 — NO parameters: the report is always workspace-wide. An empty-brace arm publishes
    // `{"kind":"dangling"}` with no properties, which is what the spine pins.
    Dangling {},
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
            // D45 — the eighth kind. `since` is `None`: the dangling report is workspace-wide and
            // takes no window (the arm carries no parameters at all).
            Self::Dangling {} => (DiagnosticKind::Dangling, None),
        }
    }
}

#[rmcp::tool_router(router = diagnostics_router, vis = "pub(crate)")]
impl UnblockServer {
    /// Read-path diagnostics (FR-15) — pure-DB, no git.
    // The `description` literal below MUST carry the SAME bytes as
    // [`DIAGNOSTICS_TOOL_DESCRIPTION`] — rmcp requires a literal here, so the constant cannot be
    // interpolated; the pair-compare in `contract_suite` is what keeps them equal.
    #[tool(
        name = "diagnostics",
        description = "Diagnostics: stats, info, where, version, lint, changelog, orphans, or dangling."
    )]
    pub(crate) async fn diagnostics(
        &self,
        Parameters(raw, _): Parameters<DiagnosticsInput>,
    ) -> CallToolResult {
        // D42 PROLOGUE: the ONLY deserialization of tool arguments. The NFR-18 quota already
        // ran once in `call_tool` over the whole `params`. `DiagnosticsInput` carries
        // `#[serde(deny_unknown_fields)]`, so an unknown/misspelled argument is REJECTED here,
        // in-band, instead of being silently discarded.
        let input: DiagnosticsInput = match parse_args(raw) {
            Ok(input) => input,
            Err(structured) => return err_json(&structured),
        };
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

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::{DiagnosticKind, DiagnosticsInput};

    /// The wire→model discriminator map is TOTAL and each arm lands on ITS OWN kind — eight arms
    /// since D45 appended `dangling`. Written as an exhaustive table rather than a spot check,
    /// because the failure this guards is a silent MIS-mapping (an arm routed to a neighbouring
    /// kind still returns a well-formed report, just one that answers a different question).
    ///
    /// `since` is pinned in the same table: it is threaded ONLY by `changelog` (D26/OQ-1), so every
    /// other arm — `dangling` included, whose report is always workspace-wide — passes `None`.
    ///
    /// MUTANT KILLED: mapping `Dangling {}` to any other kind (e.g. reusing `Orphans` or `Lint`),
    /// or threading a window into it.
    #[test]
    fn every_wire_kind_maps_to_its_own_model_kind_and_only_changelog_carries_a_window() {
        let since = Utc
            .with_ymd_and_hms(2026, 8, 1, 0, 0, 0)
            .single()
            .expect("ts");
        let table = [
            (DiagnosticsInput::Stats {}, DiagnosticKind::Stats, None),
            (DiagnosticsInput::Info {}, DiagnosticKind::Info, None),
            (DiagnosticsInput::Where {}, DiagnosticKind::Where, None),
            (DiagnosticsInput::Version {}, DiagnosticKind::Version, None),
            (DiagnosticsInput::Lint {}, DiagnosticKind::Lint, None),
            (
                DiagnosticsInput::Changelog { since: Some(since) },
                DiagnosticKind::Changelog,
                Some(since),
            ),
            (DiagnosticsInput::Orphans {}, DiagnosticKind::Orphans, None),
            // D45 — the eighth arm.
            (
                DiagnosticsInput::Dangling {},
                DiagnosticKind::Dangling,
                None,
            ),
        ];
        assert_eq!(table.len(), 8, "EIGHT wire kinds since D45");
        for (input, expected_kind, expected_since) in table {
            let (kind, window) = input.to_kind_and_since();
            assert_eq!(kind, expected_kind, "wire arm mapped to the wrong kind");
            assert_eq!(window, expected_since, "only `changelog` carries a window");
        }
    }

    /// The `dangling` wire spelling is `"dangling"` — the plain-noun form its seven siblings use
    /// (spine §5.2). The tag is published in `schema_bundle()` and digested by `CONTRACT_HASH`, so a
    /// rename is a contract act; this cell pins the DESERIALIZER side of it (the schema side is
    /// pinned in `contract_suite`).
    ///
    /// MUTANT KILLED: renaming the variant without its `rename_all = "snake_case"` spelling staying
    /// `dangling` (e.g. a `DanglingDeps` variant, which would publish `dangling_deps`).
    #[test]
    fn the_dangling_arm_deserializes_from_the_plain_noun_tag() {
        let input: DiagnosticsInput =
            serde_json::from_value(serde_json::json!({ "kind": "dangling" }))
                .expect("`{\"kind\":\"dangling\"}` is the published wire shape");
        assert_eq!(input.to_kind_and_since(), (DiagnosticKind::Dangling, None));
    }
}
