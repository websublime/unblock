//! Show tool — fetches full detail for a single issue.
//!
//! Returns the complete issue with parsed body sections, blocking/blocked-by
//! relationships, an optional dependency tree (from the cached graph), and
//! optional comments. This is a read tool — uses `execute_read_tool()`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use unblock_core::types::RelatedIssue;

/// Input parameters for the `show` MCP tool.
///
/// Requires an [`IssueRef`](unblock_core::types::IssueRef)-compatible
/// string identifying the target issue. Accepts:
///
/// - a bare number for a local issue (`"42"`),
/// - a hash-prefixed local number (`"#42"`), or
/// - a cross-repo reference (`"owner/repo#42"`).
///
/// The field is typed as `String` because `IssueRef` does not derive
/// `JsonSchema`; the handler parses the string into an `IssueRef` and
/// resolves it via the GitHub client, mirroring the `depends` tool.
///
/// Optional flags control whether comments and the dependency tree are
/// included in the response.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ShowParams {
    /// Issue reference to fetch. Accepts `42`, `#42`, or `owner/repo#42`.
    pub issue: String,
    /// Whether to include comments in the response. Defaults to `true`.
    pub include_comments: Option<bool>,
    /// Whether to include the dependency tree in the response. Defaults to `true`.
    pub include_deps: Option<bool>,
}

/// Result returned by the `show` MCP tool.
///
/// Contains the full issue detail, parsed body sections, blocking
/// relationships, optional upstream/downstream dependency trees, and
/// optional comments.
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
    /// Upstream dependency tree (what this issue depends on).
    /// `None` if `include_deps` is `false` or the graph cache is empty.
    pub upstream: Option<Vec<ShowTreeNode>>,
    /// Downstream dependency tree (what depends on this issue).
    /// `None` if `include_deps` is `false` or the graph cache is empty.
    pub downstream: Option<Vec<ShowTreeNode>>,
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
    /// Pipeline stage from Projects V2.
    pub pipeline_stage: Option<String>,
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

impl From<&RelatedIssue> for ShowRelatedIssue {
    /// Convert a core `RelatedIssue` into the schemars-friendly wire type.
    ///
    /// The `state` field is rendered via `IssueState`'s `Display`
    /// impl, which is locked byte-for-byte against the historical
    /// `Debug` shape by unit tests in `unblock-core::types`. This
    /// preserves the exact MCP wire format while decoupling the public
    /// contract from Rust's `Debug` formatting.
    fn from(r: &RelatedIssue) -> Self {
        Self {
            number: r.number,
            title: r.title.clone(),
            state: r.state.to_string(),
        }
    }
}

/// A node in a dependency tree for the show result.
///
/// Re-declared from `unblock_core::types::TreeNode` with `JsonSchema`
/// derive and string-rendered enum fields for stable MCP wire format.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ShowTreeNode {
    /// Fully qualified issue identifier (`owner/repo#number`).
    pub id: String,
    /// Workflow status at graph-build time.
    pub status: String,
    /// GitHub native issue state at graph-build time.
    pub state: String,
    /// BFS depth from the root issue (1 = direct dependency).
    pub depth: usize,
    /// Child nodes at deeper levels.
    pub children: Vec<ShowTreeNode>,
}

impl ShowTreeNode {
    /// Convert a core [`TreeNode`](unblock_core::types::TreeNode) into
    /// the schemars-friendly wire type recursively.
    pub fn from_core(node: &unblock_core::types::TreeNode) -> Self {
        Self {
            id: node.id.to_string(),
            status: node.status.to_string(),
            state: node.state.to_string(),
            depth: node.depth,
            children: node.children.iter().map(Self::from_core).collect(),
        }
    }
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

#[cfg(test)]
mod tests {
    use unblock_core::types::{IssueState, RelatedIssue};

    use super::ShowRelatedIssue;

    /// Helper to build a same-repo `RelatedIssue` with the given state.
    ///
    /// Uses [`RelatedIssue::local`] — `repo_owner` / `repo_name` stay
    /// `None` (the "same-repo-as-enclosing" convention per
    /// `RelatedIssue` docs).
    fn related_issue(state: IssueState) -> RelatedIssue {
        RelatedIssue::local(1, "test", state)
    }

    /// The `From<&RelatedIssue>` conversion writes `IssueState::Open` as
    /// the literal string `"Open"` into the MCP wire response. This test
    /// locks that value so any future change to `IssueState`'s `Display`
    /// impl is caught before reaching MCP consumers.
    #[test]
    fn related_issue_state_string_open() {
        let show = ShowRelatedIssue::from(&related_issue(IssueState::Open));
        assert_eq!(show.state, "Open");
    }

    /// Same as [`related_issue_state_string_open`] but for the `Closed`
    /// variant.
    #[test]
    fn related_issue_state_string_closed() {
        let show = ShowRelatedIssue::from(&related_issue(IssueState::Closed));
        assert_eq!(show.state, "Closed");
    }

    /// Verify that the conversion copies `number` and `title` unchanged.
    #[test]
    fn related_issue_copies_number_and_title() {
        let ri = RelatedIssue::local(42, "Important task", IssueState::Open);
        let show = ShowRelatedIssue::from(&ri);
        assert_eq!(show.number, 42);
        assert_eq!(show.title, "Important task");
    }
}
