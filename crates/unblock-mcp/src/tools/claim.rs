//! Claim tool — atomically claims an issue for an agent.
//!
//! Validates the issue is open, unblocked, not deferred, and not already claimed,
//! then updates Projects V2 fields (Status=In Progress, Agent=name,
//! ReadyState=Not Ready) and posts a claim comment. Uses `execute_write_tool()`
//! to rebuild the cache after all mutations complete.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Input parameters for the `claim` MCP tool.
///
/// Only `id` is required. If `agent` is not provided, falls back to
/// `Config.agent` (from `UNBLOCK_AGENT`), then `"unknown"`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ClaimParams {
    /// Issue number to claim (required).
    pub id: u64,
    /// Agent name claiming the issue. Defaults to the configured agent name.
    pub agent: Option<String>,
}

/// Result returned by the `claim` MCP tool.
///
/// Contains the claimed issue number, the resolved agent name, and the
/// timestamp when the claim was recorded.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ClaimResult {
    /// The claimed issue number.
    pub issue_number: u64,
    /// The agent name that claimed the issue.
    pub agent: String,
    /// Timestamp when the claim was recorded (ISO 8601).
    pub claimed_at: DateTime<Utc>,
}
