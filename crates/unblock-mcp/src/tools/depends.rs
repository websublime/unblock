//! Depends tool — adds a blocking dependency between two issues.
//!
//! Validates both issues exist, checks for cycles and duplicates, then creates
//! the blocking relationship via the GitHub API. Updates Projects V2 fields on
//! the source issue (Status=Blocked) when the source is local to the
//! configured project; skips field updates for cross-repo sources. Rebuilds
//! the cache so the ready set reflects the new dependency.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Input parameters for the `depends` MCP tool.
///
/// The `source` issue will become blocked by the `target` issue. Both sides
/// accept an [`IssueRef`](unblock_core::types::IssueRef)-compatible string:
///
/// - a bare number for a local issue (`"42"`),
/// - a hash-prefixed local number (`"#42"`), or
/// - a cross-repo reference (`"owner/repo#42"`).
///
/// Per spec §8.4, `source != target` is required. Cross-repo sources are
/// outside the configured project, so the Projects V2 field update on the
/// source (Status=Blocked) is skipped for them — GitHub still records the
/// dependency edge itself cross-repo.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DependsParams {
    /// Issue that will be blocked. Accepts `42`, `#42`, or `owner/repo#42`.
    pub source: String,
    /// Issue that blocks `source`. Accepts `42`, `#42`, or `owner/repo#42`.
    pub target: String,
}

/// Result returned by the `depends` MCP tool.
///
/// Confirms the blocking relationship was created, including a `created` flag
/// (spec §8.4), the resolved source and target references as strings, and a
/// human-readable message.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DependsResult {
    /// `true` when a new blocking edge was created by this call.
    pub created: bool,
    /// The source issue reference as resolved (local `#n` or `owner/repo#n`).
    pub source: String,
    /// The target issue reference as resolved (local `#n` or `owner/repo#n`).
    pub target: String,
    /// Human-readable confirmation message.
    pub message: String,
}
