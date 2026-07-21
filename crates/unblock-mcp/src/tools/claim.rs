//! Tool **#2 `claim`** — atomically claim an issue for an assignee (spine §5.1/§5.2, FR-2).
//!
//! Maps to `Session::claim(id, assignee)`. The loser of a concurrent claim surfaces
//! `ErrorCode::AlreadyClaimed` (retryable) via the in-band error channel.

// D42 SEAM: this is the CRATE-LOCAL `Parameters` (`crate::tools::args`), NOT rmcp's. It defers
// deserialization so argument errors reach the FR-11 in-band channel instead of an out-of-band
// `-32602`. The NAME IS LOAD-BEARING (rmcp-macros matches the ident `Parameters` to pick the
// published inputSchema) — see `tools/args.rs`. Do NOT "fix" this back to rmcp's wrapper.
use crate::tools::args::{Parameters, parse_args};
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::tool;
use serde::{Deserialize, Serialize};

use crate::server::UnblockServer;
use crate::tools::dto::Attribution;
use crate::tools::{engine_err_json, err_json, ok_json};

/// The one-line `claim` tool description.
///
/// **CONTRACT BYTES, declared ONCE.** These bytes ship in TWO places — the `#[tool(description)]`
/// attribute below (the live `tools/list` wire; rmcp requires a LITERAL there) and the
/// `capabilities()` tool descriptor (`resources/capabilities.rs`, which `CONTRACT_HASH` digests).
/// The two copies DID diverge (the descriptor carried a truncated form while the wire carried this
/// one); `contract_suite::live_list_tools_equals_the_builder_eight` now compares
/// `(name, description)` pairs, so a future divergence fails.
pub(crate) const CLAIM_TOOL_DESCRIPTION: &str =
    "Atomically claim an issue for an assignee; the loser of a race is reported.";

/// The `claim` tool input (spine §5.2 — EXACT shape).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
// D42: `#[serde(deny_unknown_fields)]` — an unknown/misspelled argument is REJECTED in-band
// instead of being silently dropped. NOT recursive and inert on a flatten TARGET: every nested
// container needs its OWN attribute (see `tools/args.rs` + the CHECK-3 container guard).
#[serde(deny_unknown_fields)]
pub(crate) struct ClaimInput {
    /// The issue id to claim.
    pub id: String,
    /// The assignee taking the claim.
    pub assignee: String,
    /// Capture-only attribution (never enforced).
    #[serde(flatten)]
    pub attribution: Attribution,
}

#[rmcp::tool_router(router = claim_router, vis = "pub(crate)")]
impl UnblockServer {
    /// Atomically claim an issue for an assignee (FR-2).
    #[tool(
        name = "claim",
        description = "Atomically claim an issue for an assignee; the loser of a race is reported."
    )]
    pub(crate) async fn claim(&self, Parameters(raw, _): Parameters<ClaimInput>) -> CallToolResult {
        // D42 PROLOGUE: the ONLY deserialization of tool arguments. The NFR-18 quota already
        // ran once in `call_tool` over the whole `params`. `ClaimInput` carries
        // `#[serde(deny_unknown_fields)]`, so an unknown/misspelled argument is REJECTED here,
        // in-band, instead of being silently discarded.
        let input: ClaimInput = match parse_args(raw) {
            Ok(input) => input,
            Err(structured) => return err_json(&structured),
        };
        match self.session.claim(&input.id, &input.assignee).await {
            Ok(issue) => ok_json(&issue),
            Err(err) => engine_err_json(&err),
        }
    }
}
