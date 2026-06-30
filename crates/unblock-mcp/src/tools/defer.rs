//! Tool **#3 `defer`** — defer/undefer an issue (spine §5.1/§5.2, FR-3).
//!
//! Maps to `Session::{defer, undefer}`. `defer` sets `defer_until`; `undefer` clears it.

use chrono::{DateTime, Utc};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::tool;
use serde::{Deserialize, Serialize};

use crate::server::UnblockServer;
use crate::tools::dto::Attribution;
use crate::tools::{engine_err_json, err_json, ok_json};

/// The `defer` tool input (spine §5.2 — EXACT shape).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub(crate) enum DeferInput {
    /// Defer the issue until a future timestamp.
    Defer {
        /// The issue id.
        id: String,
        /// The defer-until timestamp.
        until: DateTime<Utc>,
        #[serde(flatten)]
        attribution: Attribution,
    },
    /// Undefer the issue (clear `defer_until`).
    Undefer {
        /// The issue id.
        id: String,
        #[serde(flatten)]
        attribution: Attribution,
    },
}

#[rmcp::tool_router(router = defer_router, vis = "pub(crate)")]
impl UnblockServer {
    /// Defer or undefer an issue (FR-3).
    #[tool(
        name = "defer",
        description = "Defer an issue until a future timestamp, or undefer it."
    )]
    pub(crate) async fn defer(&self, Parameters(input): Parameters<DeferInput>) -> CallToolResult {
        if let Err(structured) = self.preflight(&input) {
            return err_json(&structured);
        }
        let result = match input {
            DeferInput::Defer {
                id,
                until,
                attribution: _,
            } => self.session.defer(&id, until).await,
            DeferInput::Undefer { id, attribution: _ } => self.session.undefer(&id).await,
        };
        match result {
            Ok(issue) => ok_json(&issue),
            Err(err) => engine_err_json(&err),
        }
    }
}
