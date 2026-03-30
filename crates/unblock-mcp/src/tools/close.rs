//! Close tool — closes an issue and triggers cascade unblock.
//!
//! Validates the issue is open, closes it via the GitHub API, updates Projects V2
//! fields (Status=Done, ReadyState=Not Ready), rebuilds the cache, then computes
//! the unblock cascade. For each newly unblocked issue, updates its Projects V2
//! fields (ReadyState=Ready, Status=Backlog if not already `InProgress`) and posts
//! an unblock comment.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Input parameters for the `close` MCP tool.
///
/// Only `id` is required. An optional `reason` can be provided, which is added
/// as a comment on the issue before closing.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CloseParams {
    /// Issue number to close (required).
    pub id: u64,
    /// Optional reason for closing. If provided, a comment with this text is
    /// added to the issue before it is closed.
    pub reason: Option<String>,
}

/// Result returned by the `close` MCP tool.
///
/// Contains the closed issue number and the list of issue numbers that were
/// fully unblocked by this close (the cascade).
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CloseResult {
    /// The closed issue number.
    pub issue_number: u64,
    /// Issue numbers that were fully unblocked by closing this issue.
    /// Only includes issues where ALL blockers are now closed.
    pub unblocked: Vec<u64>,
}

// TODO(unblock-45a.12): Add integration tests for close tool (cascade, co-blocking,
// already-closed paths) as part of the E2E workflow integration test.
