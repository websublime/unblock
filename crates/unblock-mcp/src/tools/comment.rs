//! Comment tool — posts a comment on an issue.
//!
//! This is a read tool from the graph perspective: comments do not affect
//! the dependency graph or ready set, so no cache invalidation is needed.
//! Uses `execute_read_tool()` despite being a write to GitHub.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Input parameters for the `comment` MCP tool.
///
/// Requires both an issue number and a non-empty comment body.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CommentParams {
    /// Issue number to comment on (required).
    pub id: u64,
    /// Comment body text (required, must not be empty or whitespace-only).
    pub body: String,
}

/// Result returned by the `comment` MCP tool.
///
/// Contains the issue number and a URL pointing to the newly created comment.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CommentResult {
    /// The issue number the comment was posted on.
    pub issue_number: u64,
    /// URL of the newly created comment on GitHub.
    pub comment_url: String,
}

// TODO(unblock-45a.12): Add integration tests for comment tool (existing issue,
// non-existent issue, empty body) as part of the E2E workflow integration test.
