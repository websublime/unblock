//! Domain types for the unblock system.
//!
//! Defines the core data structures: `Issue`, `IssueState`, `Status`,
//! `Priority`, `ReadyState`, `IssueType`, `BlockingEdge`,
//! `IssueSummary`, and `BodySections`.
//!
//! All types are backend-agnostic — the GitHub client handles mapping from
//! GitHub-specific field names. The graph engine works identically regardless
//! of data source.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

/// An issue in the dependency graph.
///
/// Mapped from GitHub Issue + Projects V2 field values. Contains both
/// GitHub-native fields (`state`, `number`) and Projects V2 custom fields
/// (`status`, `priority`, `agent`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Issue {
    /// GitHub issue number (e.g. `#42`).
    pub number: u64,
    /// GitHub GraphQL node ID (opaque, used for mutations).
    pub node_id: String,
    /// Issue title.
    pub title: String,
    /// Issue type classification from Projects V2.
    pub issue_type: Option<IssueType>,
    /// Workflow status from Projects V2 custom field.
    pub status: Status,
    /// Priority from Projects V2 custom field.
    pub priority: Priority,
    /// Agent name from Projects V2 custom field (free text).
    pub agent: Option<String>,
    /// Timestamp when the issue was claimed by an agent.
    pub claimed_at: Option<DateTime<Utc>>,
    /// Ready state from Projects V2 custom field (MCP writes, never reads for logic).
    pub ready_state: ReadyState,
    /// Story points from Projects V2 custom field.
    pub story_points: Option<i32>,
    /// Date until which the issue is deferred.
    pub defer_until: Option<NaiveDate>,
    /// Labels attached to the issue.
    pub labels: Vec<String>,
    /// Milestone title (epic equivalent).
    pub milestone: Option<String>,
    /// GitHub usernames of assignees (human assignment).
    pub assignees: Vec<String>,
    /// GitHub native issue state: Open or Closed.
    pub state: IssueState,
    /// Full markdown body of the issue.
    pub body: Option<String>,
    /// Timestamp when the issue was created.
    pub created_at: DateTime<Utc>,
    /// Timestamp when the issue was last updated.
    pub updated_at: DateTime<Utc>,
    /// HTML URL for linking back to GitHub.
    pub url: String,
}

/// GitHub native issue state.
///
/// Separate from our workflow [`Status`] — GitHub only tracks Open/Closed,
/// while `Status` provides finer-grained workflow states.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum IssueState {
    /// The issue is open and active.
    Open,
    /// The issue has been closed.
    Closed,
}

/// Workflow status stored as a Projects V2 single-select field.
///
/// Finer-grained than GitHub's binary Open/Closed. Used by the graph engine
/// and MCP tools for workflow logic.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Status {
    /// Issue is open and waiting to be picked up.
    Open,
    /// Issue is actively being worked on.
    InProgress,
    /// Issue is blocked by one or more dependencies.
    Blocked,
    /// Issue is deferred until a future date.
    Deferred,
    /// Issue is completed.
    Closed,
}

/// Issue priority levels.
///
/// P0 is the highest priority, P4 is the lowest. Used for sorting
/// the ready set so agents pick up the most important work first.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Priority {
    /// Critical — drop everything.
    P0,
    /// High — do next.
    P1,
    /// Medium — normal queue.
    P2,
    /// Low — when convenient.
    P3,
    /// Backlog — nice to have.
    P4,
}

impl Priority {
    /// Sort key for priority ordering (P0=0, P4=4).
    ///
    /// Lower values indicate higher priority, suitable for ascending sort.
    #[must_use]
    pub fn as_sort_key(&self) -> u8 {
        match self {
            Self::P0 => 0,
            Self::P1 => 1,
            Self::P2 => 2,
            Self::P3 => 3,
            Self::P4 => 4,
        }
    }
}

/// Ready state for an issue, stored as a Projects V2 custom field.
///
/// The MCP server writes this field to reflect computed readiness.
/// The graph engine does **not** read this field for logic — it computes
/// readiness from the dependency graph directly.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReadyState {
    /// All blockers resolved — issue can be picked up.
    Ready,
    /// Issue has active blockers.
    Blocked,
    /// Issue is not ready for other reasons (e.g., deferred).
    NotReady,
    /// Issue is closed.
    Closed,
}

/// Classification of issue types.
///
/// Used for filtering and reporting. Mapped from GitHub's issue type
/// or a Projects V2 custom field.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum IssueType {
    /// A concrete unit of work.
    Task,
    /// A defect to be fixed.
    Bug,
    /// A new feature request.
    Feature,
    /// A collection of related issues.
    Epic,
    /// Maintenance or housekeeping work.
    Chore,
    /// A time-boxed investigation.
    Spike,
}

/// A blocking edge in the dependency graph.
///
/// Mapped from GitHub's native `blockedBy` relationship.
/// The edge direction is: `source` is blocked by `target`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BlockingEdge {
    /// Issue number that is blocked.
    pub source: u64,
    /// Issue number that blocks `source`.
    pub target: u64,
}

/// Lightweight summary of an issue for list and ready-set responses.
///
/// Contains only the fields needed for display and sorting, avoiding the
/// full weight of [`Issue`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueSummary {
    /// GitHub issue number.
    pub number: u64,
    /// Issue title.
    pub title: String,
    /// Issue type classification.
    pub issue_type: Option<IssueType>,
    /// Workflow status.
    pub status: Status,
    /// Priority level.
    pub priority: Priority,
    /// Agent name if claimed.
    pub agent: Option<String>,
    /// Milestone title.
    pub milestone: Option<String>,
    /// Story points estimate.
    pub story_points: Option<i32>,
    /// Labels attached to the issue.
    pub labels: Vec<String>,
    /// Timestamp when the issue was created.
    pub created_at: DateTime<Utc>,
    /// HTML URL for linking back to GitHub.
    pub url: String,
}

/// Parsed sections from the issue body markdown.
///
/// Three sections only — each data type lives in the correct GitHub primitive.
/// Parsed from `## Description`, `## Design Notes`, and `## Acceptance Criteria`
/// headers. Missing sections are represented as `None`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BodySections {
    /// Content under the `## Description` header.
    pub description: Option<String>,
    /// Content under the `## Design Notes` header.
    pub design_notes: Option<String>,
    /// Content under the `## Acceptance Criteria` header.
    pub acceptance_criteria: Option<String>,
}

impl BodySections {
    /// Parse structured sections from a markdown body.
    ///
    /// Looks for `## Description`, `## Design Notes`, and `## Acceptance Criteria`
    /// headers. Content before any recognized header is treated as the description.
    /// Unknown headers are ignored. Missing sections result in `None`.
    /// An empty or whitespace-only body returns the default (all `None`).
    #[must_use]
    pub fn from_markdown(body: &str) -> Self {
        if body.trim().is_empty() {
            return Self::default();
        }

        let mut description: Option<String> = None;
        let mut design_notes: Option<String> = None;
        let mut acceptance_criteria: Option<String> = None;

        // Track which section we're currently collecting into.
        // None means we're collecting into the "preamble" (treated as description
        // if no explicit ## Description header is found).
        let mut current_section: Option<&str> = None;
        let mut current_content = String::new();
        let mut has_description_header = false;
        let mut preamble = String::new();

        for line in body.lines() {
            if let Some(header) = line.strip_prefix("## ") {
                // Flush current section before starting a new one.
                flush_section(
                    current_section,
                    &current_content,
                    &mut description,
                    &mut design_notes,
                    &mut acceptance_criteria,
                );
                current_content = String::new();

                let header_trimmed = header.trim();
                match header_trimmed {
                    "Description" => {
                        has_description_header = true;
                        current_section = Some("description");
                    }
                    "Design Notes" => {
                        current_section = Some("design_notes");
                    }
                    "Acceptance Criteria" => {
                        current_section = Some("acceptance_criteria");
                    }
                    _ => {
                        // Unknown header — ignore its content.
                        current_section = Some("unknown");
                    }
                }
            } else if current_section.is_some() {
                if !current_content.is_empty() {
                    current_content.push('\n');
                }
                current_content.push_str(line);
            } else {
                // Before any recognized header — preamble.
                if !preamble.is_empty() {
                    preamble.push('\n');
                }
                preamble.push_str(line);
            }
        }

        // Flush the last section.
        flush_section(
            current_section,
            &current_content,
            &mut description,
            &mut design_notes,
            &mut acceptance_criteria,
        );

        // If no explicit ## Description header was found, treat preamble as description.
        if !has_description_header && !preamble.trim().is_empty() {
            description = Some(preamble.trim().to_owned());
        }

        // Trim all sections — None if empty after trimming.
        Self {
            description: description.as_deref().and_then(non_empty_trimmed),
            design_notes: design_notes.as_deref().and_then(non_empty_trimmed),
            acceptance_criteria: acceptance_criteria.as_deref().and_then(non_empty_trimmed),
        }
    }

    /// Render sections back to a markdown body.
    ///
    /// Only sections with content are included. Each section is preceded by
    /// its `##` header. Sections are separated by blank lines.
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut parts = Vec::new();

        if let Some(ref desc) = self.description {
            parts.push(format!("## Description\n\n{desc}"));
        }

        if let Some(ref notes) = self.design_notes {
            parts.push(format!("## Design Notes\n\n{notes}"));
        }

        if let Some(ref criteria) = self.acceptance_criteria {
            parts.push(format!("## Acceptance Criteria\n\n{criteria}"));
        }

        parts.join("\n\n")
    }
}

/// Flush accumulated content into the appropriate section field.
fn flush_section(
    section: Option<&str>,
    content: &str,
    description: &mut Option<String>,
    design_notes: &mut Option<String>,
    acceptance_criteria: &mut Option<String>,
) {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return;
    }
    match section {
        Some("description") => *description = Some(trimmed.to_owned()),
        Some("design_notes") => *design_notes = Some(trimmed.to_owned()),
        Some("acceptance_criteria") => *acceptance_criteria = Some(trimmed.to_owned()),
        _ => {} // unknown or None — discard
    }
}

/// Return `Some(trimmed)` if the string is non-empty after trimming, else `None`.
fn non_empty_trimmed(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Priority::as_sort_key ───────────────────────────────────────────

    #[test]
    fn priority_sort_keys_are_in_order() {
        assert_eq!(Priority::P0.as_sort_key(), 0);
        assert_eq!(Priority::P1.as_sort_key(), 1);
        assert_eq!(Priority::P2.as_sort_key(), 2);
        assert_eq!(Priority::P3.as_sort_key(), 3);
        assert_eq!(Priority::P4.as_sort_key(), 4);
    }

    #[test]
    fn priority_sort_key_range() {
        for p in &[
            Priority::P0,
            Priority::P1,
            Priority::P2,
            Priority::P3,
            Priority::P4,
        ] {
            assert!(p.as_sort_key() <= 4);
        }
    }

    // ── BodySections::from_markdown ─────────────────────────────────────

    #[test]
    fn parse_empty_body() {
        let sections = BodySections::from_markdown("");
        assert_eq!(sections, BodySections::default());
    }

    #[test]
    fn parse_whitespace_only_body() {
        let sections = BodySections::from_markdown("   \n  \n  ");
        assert_eq!(sections, BodySections::default());
    }

    #[test]
    fn parse_all_three_sections() {
        let body = "\
## Description

This is the description.

## Design Notes

These are design notes.

## Acceptance Criteria

- [ ] Criterion one
- [ ] Criterion two";

        let sections = BodySections::from_markdown(body);
        assert_eq!(
            sections.description.as_deref(),
            Some("This is the description.")
        );
        assert_eq!(
            sections.design_notes.as_deref(),
            Some("These are design notes.")
        );
        assert_eq!(
            sections.acceptance_criteria.as_deref(),
            Some("- [ ] Criterion one\n- [ ] Criterion two")
        );
    }

    #[test]
    fn parse_missing_sections() {
        let body = "## Description\n\nOnly a description here.";
        let sections = BodySections::from_markdown(body);
        assert_eq!(
            sections.description.as_deref(),
            Some("Only a description here.")
        );
        assert!(sections.design_notes.is_none());
        assert!(sections.acceptance_criteria.is_none());
    }

    #[test]
    fn parse_preamble_as_description() {
        let body = "Some text before any header.\n\nMore text.";
        let sections = BodySections::from_markdown(body);
        assert_eq!(
            sections.description.as_deref(),
            Some("Some text before any header.\n\nMore text.")
        );
    }

    #[test]
    fn parse_unknown_headers_ignored() {
        let body = "\
## Description

Description text.

## Random Header

This should be ignored.

## Design Notes

Design text.";

        let sections = BodySections::from_markdown(body);
        assert_eq!(sections.description.as_deref(), Some("Description text."));
        assert_eq!(sections.design_notes.as_deref(), Some("Design text."));
        assert!(sections.acceptance_criteria.is_none());
    }

    #[test]
    fn parse_sections_in_any_order() {
        let body = "\
## Acceptance Criteria

- [ ] Done

## Description

Desc here.

## Design Notes

Notes here.";

        let sections = BodySections::from_markdown(body);
        assert_eq!(sections.description.as_deref(), Some("Desc here."));
        assert_eq!(sections.design_notes.as_deref(), Some("Notes here."));
        assert_eq!(sections.acceptance_criteria.as_deref(), Some("- [ ] Done"));
    }

    #[test]
    fn parse_empty_section_becomes_none() {
        let body = "## Description\n\n## Design Notes\n\nActual content.";
        let sections = BodySections::from_markdown(body);
        assert!(sections.description.is_none());
        assert_eq!(sections.design_notes.as_deref(), Some("Actual content."));
    }

    // ── BodySections::to_markdown ───────────────────────────────────────

    #[test]
    fn render_all_sections() {
        let sections = BodySections {
            description: Some("Desc.".to_owned()),
            design_notes: Some("Notes.".to_owned()),
            acceptance_criteria: Some("- [ ] Done".to_owned()),
        };
        let md = sections.to_markdown();
        assert!(md.contains("## Description\n\nDesc."));
        assert!(md.contains("## Design Notes\n\nNotes."));
        assert!(md.contains("## Acceptance Criteria\n\n- [ ] Done"));
    }

    #[test]
    fn render_skips_none_sections() {
        let sections = BodySections {
            description: Some("Desc.".to_owned()),
            design_notes: None,
            acceptance_criteria: None,
        };
        let md = sections.to_markdown();
        assert!(md.contains("## Description\n\nDesc."));
        assert!(!md.contains("Design Notes"));
        assert!(!md.contains("Acceptance Criteria"));
    }

    #[test]
    fn render_empty_sections_is_empty_string() {
        let sections = BodySections::default();
        assert!(sections.to_markdown().is_empty());
    }

    // ── Roundtrip ───────────────────────────────────────────────────────

    #[test]
    fn roundtrip_all_sections() {
        let original = BodySections {
            description: Some("A description.".to_owned()),
            design_notes: Some("Some design notes.".to_owned()),
            acceptance_criteria: Some("- [ ] First\n- [ ] Second".to_owned()),
        };
        let rendered = original.to_markdown();
        let parsed = BodySections::from_markdown(&rendered);
        assert_eq!(original, parsed);
    }

    #[test]
    fn roundtrip_partial_sections() {
        let original = BodySections {
            description: Some("Only desc.".to_owned()),
            design_notes: None,
            acceptance_criteria: Some("- [x] Done".to_owned()),
        };
        let rendered = original.to_markdown();
        let parsed = BodySections::from_markdown(&rendered);
        assert_eq!(original, parsed);
    }

    #[test]
    fn roundtrip_empty() {
        let original = BodySections::default();
        let rendered = original.to_markdown();
        let parsed = BodySections::from_markdown(&rendered);
        assert_eq!(original, parsed);
    }

    // ── Serde roundtrip ─────────────────────────────────────────────────

    #[test]
    fn serde_roundtrip_priority() {
        for p in &[
            Priority::P0,
            Priority::P1,
            Priority::P2,
            Priority::P3,
            Priority::P4,
        ] {
            let json = serde_json::to_string(p).expect("serialize");
            let back: Priority = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*p, back);
        }
    }

    #[test]
    fn serde_roundtrip_issue_state() {
        for s in &[IssueState::Open, IssueState::Closed] {
            let json = serde_json::to_string(s).expect("serialize");
            let back: IssueState = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*s, back);
        }
    }

    #[test]
    fn serde_roundtrip_status() {
        for s in &[
            Status::Open,
            Status::InProgress,
            Status::Blocked,
            Status::Deferred,
            Status::Closed,
        ] {
            let json = serde_json::to_string(s).expect("serialize");
            let back: Status = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*s, back);
        }
    }

    // ── Proptest ────────────────────────────────────────────────────────

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        /// Strategy that generates strings without `## ` at the start of any line.
        /// This ensures the roundtrip property holds — generated content cannot
        /// be confused with markdown section headers.
        fn safe_section_content() -> impl Strategy<Value = String> {
            "[a-zA-Z0-9 .,;:!?\\-_\\n]{0,200}"
                .prop_filter("must not contain ## at line start", |s| {
                    !s.lines().any(|line| line.starts_with("## "))
                })
        }

        fn optional_safe_content() -> impl Strategy<Value = Option<String>> {
            prop_oneof![
                Just(None),
                safe_section_content().prop_map(|s| {
                    let trimmed = s.trim().to_owned();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed)
                    }
                }),
            ]
        }

        proptest! {
            #[test]
            fn body_sections_roundtrip(
                desc in optional_safe_content(),
                notes in optional_safe_content(),
                criteria in optional_safe_content(),
            ) {
                let original = BodySections {
                    description: desc,
                    design_notes: notes,
                    acceptance_criteria: criteria,
                };
                let rendered = original.to_markdown();
                let parsed = BodySections::from_markdown(&rendered);
                prop_assert_eq!(original, parsed);
            }
        }
    }
}
