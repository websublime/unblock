//! Tool **#2 `claim`** — atomically claim an issue for an assignee (spine §5.1/§5.2, FR-2).
//!
//! Maps to `Session::claim(id, assignee)`. The loser of a concurrent claim surfaces
//! `ErrorCode::AlreadyClaimed` (retryable) via the in-band error channel.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::tool;
use serde::{Deserialize, Serialize};

use crate::server::UnblockServer;
use crate::tools::dto::Attribution;
use crate::tools::{engine_err_json, err_json, ok_json};

/// The `claim` tool input (spine §5.2 — EXACT shape).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
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
    pub(crate) async fn claim(&self, Parameters(input): Parameters<ClaimInput>) -> CallToolResult {
        if let Err(structured) = self.preflight(&input) {
            return err_json(&structured);
        }
        match self.session.claim(&input.id, &input.assignee).await {
            Ok(issue) => ok_json(&issue),
            Err(err) => engine_err_json(&err),
        }
    }
}
