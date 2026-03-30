//! Create tool — creates a new GitHub Issue with all metadata.
//!
//! Builds the issue via REST, adds it to the Projects V2 project, sets custom
//! fields (Priority, `IssueType`, `StoryPoints`, `DeferUntil`, Status=Backlog,
//! ReadyState=Ready), optionally adds blocking relationships and parent linkage.
//! Uses `execute_write_tool()` to rebuild the cache after all mutations complete.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Input parameters for the `create` MCP tool.
///
/// Only `title` is required. All other fields are optional and have sensible
/// defaults: `issue_type` defaults to `Task`, `priority` defaults to `P2`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateParams {
    /// Issue title (required).
    pub title: String,
    /// Issue type: Task, Bug, Feature, Epic, Chore, Spike. Defaults to Task.
    pub issue_type: Option<String>,
    /// Priority: P0, P1, P2, P3, P4. Defaults to P2.
    pub priority: Option<String>,
    /// Issue body in markdown. If omitted, a `BodySections` template is generated.
    pub body: Option<String>,
    /// Labels to attach. Labels that do not exist on the repo are created.
    pub labels: Option<Vec<String>>,
    /// Milestone title. Currently accepted but not resolved to a milestone ID
    /// (milestone resolution requires a separate API call not yet implemented).
    pub milestone: Option<String>,
    /// Issues that block this new issue. Accepts local numbers (`42`) or
    /// cross-repo references (`owner/repo#42`).
    pub blocked_by: Option<Vec<String>>,
    /// Parent issue number — makes this issue a sub-issue of the parent.
    pub parent: Option<u64>,
    /// Story points estimate (number field on the project).
    pub story_points: Option<f64>,
    /// Date until which this issue is deferred (ISO 8601: `YYYY-MM-DD`).
    pub defer_until: Option<String>,
}

/// Result returned by the `create` MCP tool.
///
/// Contains the created issue number, URL, and a summary of what was set.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CreateResult {
    /// The created issue number.
    pub number: u64,
    /// The created issue URL.
    pub url: String,
    /// The issue title.
    pub title: String,
    /// The issue type that was set.
    pub issue_type: String,
    /// The priority that was set.
    pub priority: String,
    /// Whether the issue was added to a project.
    pub added_to_project: bool,
    /// Whether project fields were set successfully.
    pub fields_set: bool,
    /// Number of blocking relationships created.
    pub blockers_added: u32,
    /// Whether a parent relationship was created.
    pub parent_set: bool,
    /// Hint message for next steps.
    pub hint: String,
}
