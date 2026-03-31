//! Show tool — fetches full detail for a single issue.
//!
//! Returns the complete issue with parsed body sections, blocking/blocked-by
//! relationships, an optional dependency tree (from the cached graph), and
//! optional comments. This is a read tool — uses `execute_read_tool()`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Input parameters for the `show` MCP tool.
///
/// Requires the issue number. Optional flags control whether comments
/// and the dependency tree are included in the response.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ShowParams {
    /// GitHub issue number to fetch (e.g. `42`).
    pub id: u64,
    /// Whether to include comments in the response. Defaults to `true`.
    pub include_comments: Option<bool>,
    /// Whether to include the dependency tree in the response. Defaults to `true`.
    pub include_deps: Option<bool>,
}

/// Result returned by the `show` MCP tool.
///
/// Contains the full issue detail, parsed body sections, blocking
/// relationships, an optional dependency tree, and optional comments.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ShowResult {
    /// Full issue data serialized as JSON. Contains all fields from the
    /// GitHub issue including Projects V2 custom fields.
    pub issue: ShowIssue,
    /// Parsed body sections (description, design notes, acceptance criteria).
    pub body_sections: ShowBodySections,
    /// Issues that this issue blocks (downstream dependents).
    pub blocking: Vec<ShowRelatedIssue>,
    /// Issues that block this issue (upstream blockers).
    pub blocked_by: Vec<ShowRelatedIssue>,
    /// Parent issue if this is a sub-issue.
    pub parent: Option<ShowRelatedIssue>,
    /// Sub-issues of this issue.
    pub sub_issues: Vec<ShowRelatedIssue>,
    /// Dependency tree from the cached graph, up to depth 3.
    /// Each entry is `(issue_number, depth)`. `None` if `include_deps`
    /// is `false` or the graph cache is empty.
    pub dependency_tree: Option<Vec<DependencyTreeEntry>>,
    /// Comments on the issue. `None` if `include_comments` is `false`.
    pub comments: Option<Vec<ShowComment>>,
}

/// Full issue detail for the show result.
///
/// Re-declared from `unblock_core::types::Issue` with `JsonSchema` derive,
/// since core types do not depend on `schemars`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ShowIssue {
    /// GitHub issue number.
    pub number: u64,
    /// GitHub GraphQL node ID.
    pub node_id: String,
    /// Issue title.
    pub title: String,
    /// Issue type classification (e.g. "Task", "Bug").
    pub issue_type: Option<String>,
    /// Workflow status from Projects V2.
    pub status: String,
    /// Priority level from Projects V2.
    pub priority: String,
    /// Agent name if claimed.
    pub agent: Option<String>,
    /// Timestamp when the issue was claimed by an agent.
    pub claimed_at: Option<String>,
    /// Ready state from Projects V2.
    pub ready_state: String,
    /// Story points estimate.
    pub story_points: Option<i32>,
    /// Date until which the issue is deferred (ISO 8601 date).
    pub defer_until: Option<String>,
    /// Labels attached to the issue.
    pub labels: Vec<String>,
    /// Milestone title.
    pub milestone: Option<String>,
    /// GitHub usernames of assignees.
    pub assignees: Vec<String>,
    /// GitHub native issue state: "Open" or "Closed".
    pub state: String,
    /// Full markdown body of the issue.
    pub body: Option<String>,
    /// Timestamp when the issue was created.
    pub created_at: String,
    /// Timestamp when the issue was last updated.
    pub updated_at: String,
    /// HTML URL for linking back to GitHub.
    pub url: String,
}

/// Parsed body sections for the show result.
///
/// Re-declared from `unblock_core::types::BodySections` with `JsonSchema`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ShowBodySections {
    /// Content under the `## Description` header.
    pub description: Option<String>,
    /// Content under the `## Design Notes` header.
    pub design_notes: Option<String>,
    /// Content under the `## Acceptance Criteria` header.
    pub acceptance_criteria: Option<String>,
}

/// A lightweight reference to a related issue for the show result.
///
/// Re-declared from `unblock_core::types::RelatedIssue` with `JsonSchema`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ShowRelatedIssue {
    /// GitHub issue number.
    pub number: u64,
    /// Issue title.
    pub title: String,
    /// GitHub native issue state: "Open" or "Closed".
    pub state: String,
}

/// A single entry in the dependency tree.
///
/// Wraps the `(issue_number, depth)` tuple from `DependencyGraph::dependency_tree()`
/// with named fields and `JsonSchema` support.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DependencyTreeEntry {
    /// GitHub issue number of the dependency.
    pub issue_number: u64,
    /// BFS depth from the root issue (1 = direct dependency).
    pub depth: usize,
}

/// A comment on the issue for the show result.
///
/// Re-declared from `unblock_core::types::IssueComment` with `JsonSchema`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ShowComment {
    /// GitHub login of the comment author.
    pub author: String,
    /// Full markdown body of the comment.
    pub body: String,
    /// Timestamp when the comment was created (ISO 8601 / RFC 3339).
    pub created_at: String,
}
