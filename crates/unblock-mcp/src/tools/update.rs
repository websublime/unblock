//! Update tool — updates issue metadata and Projects V2 fields.
//!
//! Supports selective updates: priority, status, labels (add/remove), milestone,
//! story points, defer until, and body section edits. Only fields that are
//! provided in the input are modified — omitted fields are left unchanged.
//!
//! Body section updates use `BodySections::from_markdown()` and `to_markdown()`
//! for a read-modify-write cycle on the issue body.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The section of the issue body to update.
///
/// Maps to the three recognized sections in [`BodySections`]:
/// - `Description` -> `BodySections::description`
/// - `Acceptance` -> `BodySections::acceptance_criteria`
/// - `Design` -> `BodySections::design_notes`
///
/// [`BodySections`]: unblock_core::types::BodySections
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub enum SectionName {
    /// The `## Description` section.
    Description,
    /// The `## Acceptance Criteria` section.
    Acceptance,
    /// The `## Design Notes` section.
    Design,
}

/// A body section update — specifies which section to modify and the new content.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct BodySectionUpdate {
    /// Which section to update.
    pub section: SectionName,
    /// The new content for the section (markdown).
    pub content: String,
}

/// Input parameters for the `update` MCP tool.
///
/// All fields except `id` are optional. Only provided fields are updated;
/// omitted fields are left unchanged on the issue.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateParams {
    /// Issue number to update (required).
    pub id: u64,
    /// New priority: P0, P1, P2, P3, P4.
    pub priority: Option<String>,
    /// New status: Backlog, Ready, In Progress, Blocked, Deferred, Closed
    /// (canonical `TitleCase` Projects V2 option names sourced from
    /// `unblock_core::types::Status::option_name`; spec §13.3, §2.3).
    pub status: Option<String>,
    /// Labels to add to the issue.
    pub labels_add: Option<Vec<String>>,
    /// Labels to remove from the issue.
    pub labels_remove: Option<Vec<String>>,
    /// GitHub usernames to add as assignees.
    pub assignees_add: Option<Vec<String>>,
    /// GitHub usernames to remove from assignees.
    pub assignees_remove: Option<Vec<String>>,
    /// Update a specific section of the issue body.
    pub body_section: Option<BodySectionUpdate>,
    /// Milestone title to set (resolved to milestone number via REST).
    pub milestone: Option<String>,
    /// Story points estimate (number field on the project).
    pub story_points: Option<f64>,
    /// Date until which this issue is deferred (ISO 8601: `YYYY-MM-DD`).
    pub defer_until: Option<String>,
}

/// Result returned by the `update` MCP tool.
///
/// Summarizes what was updated and returns the refreshed issue number and URL.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct UpdateResult {
    /// The updated issue number.
    pub number: u64,
    /// The issue URL.
    pub url: String,
    /// List of fields that were successfully updated.
    pub fields_updated: Vec<String>,
    /// Hint message for next steps.
    pub hint: String,
}
