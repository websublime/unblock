//! Tool **#3 `defer`** — defer/undefer an issue (spine §5.1/§5.2, FR-3).
//!
//! Maps to `Session::{defer, undefer}`. `defer` sets `defer_until`; `undefer` clears it.

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

use crate::server::UnblockServer;
use crate::tools::dto::Attribution;
use crate::tools::{engine_err_json, err_json, ok_json};

/// The `defer` tool input (spine §5.2 — EXACT shape).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
// §5.2a (CD-1): inject the root `"type": "object"` (the tagged-enum `oneOf` root omits it, which
// strict MCP clients reject) — the union is preserved verbatim.
#[schemars(extend("type" = "object"))]
// D42: `#[serde(deny_unknown_fields)]` — an unknown/misspelled argument is REJECTED in-band
// instead of being silently dropped. NOT recursive and inert on a flatten TARGET: every nested
// container needs its OWN attribute (see `tools/args.rs` + the CHECK-3 container guard).
#[serde(deny_unknown_fields)]
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
    pub(crate) async fn defer(&self, Parameters(raw, _): Parameters<DeferInput>) -> CallToolResult {
        // D42 PROLOGUE: the ONLY deserialization of tool arguments. The NFR-18 quota already
        // ran once in `call_tool` over the whole `params`. `DeferInput` carries
        // `#[serde(deny_unknown_fields)]`, so an unknown/misspelled argument is REJECTED here,
        // in-band, instead of being silently discarded.
        let input: DeferInput = match parse_args(raw) {
            Ok(input) => input,
            Err(structured) => return err_json(&structured),
        };
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
