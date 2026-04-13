//! Depends tool — adds a blocking dependency between two issues.
//!
//! Validates both issues exist, checks for cycles and duplicates, then creates
//! the blocking relationship via the GitHub API. Updates Projects V2 fields on
//! the source issue (Status=Blocked), and rebuilds the
//! cache so the ready set reflects the new dependency.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Input parameters for the `depends` MCP tool.
///
/// The `source` issue will become blocked by the `target` issue. The source
/// must be in the configured repository. The target can be a local issue
/// number or a cross-repo reference in `owner/repo#number` format.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DependsParams {
    /// Issue number that will be blocked (must be in the configured repo).
    pub source: u64,
    /// The issue that blocks source. Accepts a plain integer for local issues
    /// (e.g. `"42"`) or `owner/repo#number` for cross-repo (e.g.
    /// `"websublime/other-repo#7"`).
    pub target: String,
}

/// Result returned by the `depends` MCP tool.
///
/// Confirms the blocking relationship was created, including the source issue
/// number, the resolved target reference, and a human-readable message.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DependsResult {
    /// The source issue number (the one that is now blocked).
    pub source: u64,
    /// The target reference as provided (local number or cross-repo ref).
    pub target: String,
    /// Human-readable confirmation message.
    pub message: String,
}

// TODO(unblock-45a.12): Add integration tests for depends tool (local dep,
// cross-repo dep, cycle detection, duplicate detection, non-existent issue)
// as part of the E2E workflow integration test.
