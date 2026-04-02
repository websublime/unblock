//! Domain types for the unblock system.
//!
//! Defines the core data structures: `QualifiedId`, `Issue`, `IssueComment`,
//! `RelatedIssue`, `IssueState`, `Status`, `Priority`, `ReadyState`, `IssueType`,
//! `BlockingEdge`, `IssueSummary`, and `BodySections`.
//!
//! All types are backend-agnostic — the GitHub client handles mapping from
//! GitHub-specific field names. The graph engine works identically regardless
//! of data source.

use std::fmt;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

/// A fully qualified issue identifier: `owner/repo#number`.
///
/// Used as the canonical node key in the dependency graph to prevent silent
/// node collision when cross-repo dependencies reference issues with the
/// same number (ARCH §5.5).
///
/// # Display format
///
/// `owner/repo#number` — e.g., `"websublime/unblock#42"`.
///
/// # Parsing
///
/// The [`FromStr`](std::str::FromStr) implementation accepts the display format:
/// `"owner/repo#number"` → `QualifiedId { owner: "owner", repo: "repo", number: 42 }`.
///
/// Serialized as a flat string in `owner/repo#number` format.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct QualifiedId {
    /// Repository owner (e.g., `"websublime"`).
    pub owner: String,
    /// Repository name (e.g., `"unblock"`).
    pub repo: String,
    /// Issue number within the repository.
    pub number: u64,
}

impl QualifiedId {
    /// Create a new `QualifiedId` from explicit parts.
    #[must_use]
    pub fn new(owner: impl Into<String>, repo: impl Into<String>, number: u64) -> Self {
        Self {
            owner: owner.into(),
            repo: repo.into(),
            number,
        }
    }
}

impl fmt::Display for QualifiedId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}#{}", self.owner, self.repo, self.number)
    }
}

impl std::str::FromStr for QualifiedId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let Some((prefix, num_str)) = s.split_once('#') else {
            return Err(format!("missing '#' in qualified id: {s}"));
        };
        let Some((owner, repo)) = prefix.split_once('/') else {
            return Err(format!("missing '/' in qualified id prefix: {s}"));
        };
        if owner.is_empty() {
            return Err(format!("empty owner in qualified id: {s}"));
        }
        if repo.is_empty() {
            return Err(format!("empty repo in qualified id: {s}"));
        }
        let number = num_str
            .parse::<u64>()
            .map_err(|_| format!("invalid number in qualified id: {s}"))?;
        Ok(Self {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            number,
        })
    }
}

impl Serialize for QualifiedId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for QualifiedId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// A comment on a GitHub issue.
///
/// Parsed from the GraphQL `comments` connection on a single-issue fetch.
/// Only included in [`Issue`] when fetched via `fetch_issue()` — the bulk
/// `fetch_graph_data()` omits comments for performance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IssueComment {
    /// GitHub login of the comment author.
    pub author: String,
    /// Full markdown body of the comment.
    pub body: String,
    /// Timestamp when the comment was created.
    pub created_at: DateTime<Utc>,
}

/// An issue in the dependency graph.
///
/// Mapped from GitHub Issue + Projects V2 field values. Contains both
/// GitHub-native fields (`state`, `number`) and Projects V2 custom fields
/// (`status`, `priority`, `agent`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Issue {
    /// Fully qualified issue identifier (`owner/repo#number`).
    ///
    /// Canonical key for the graph engine. Two issues with the same `number`
    /// but different `owner/repo` are distinct nodes.
    pub qualified_id: QualifiedId,
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
    /// Comments on the issue (populated by `fetch_issue()`, empty for bulk fetches).
    pub comments: Vec<IssueComment>,
    /// Issues that block this issue (populated by `fetch_issue()` only).
    pub blocked_by: Vec<RelatedIssue>,
    /// Issues that this issue blocks (populated by `fetch_issue()` only).
    pub blocking: Vec<RelatedIssue>,
    /// Parent issue if this is a sub-issue (populated by `fetch_issue()` only).
    pub parent: Option<RelatedIssue>,
    /// Sub-issues of this issue (populated by `fetch_issue()` only).
    pub sub_issues: Vec<RelatedIssue>,
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

impl fmt::Display for IssueType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Task => write!(f, "Task"),
            Self::Bug => write!(f, "Bug"),
            Self::Feature => write!(f, "Feature"),
            Self::Epic => write!(f, "Epic"),
            Self::Chore => write!(f, "Chore"),
            Self::Spike => write!(f, "Spike"),
        }
    }
}

/// A lightweight reference to a related issue.
///
/// Used for dependency relationships (`blockedBy`, `blocking`),
/// parent issues, and sub-issues returned by `fetch_issue()`.
/// Contains only the fields available from nested GraphQL fragments.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RelatedIssue {
    /// GitHub issue number.
    pub number: u64,
    /// Issue title.
    pub title: String,
    /// GitHub native issue state.
    pub state: IssueState,
}

/// A blocking edge in the dependency graph.
///
/// Mapped from GitHub's native `blockedBy` relationship.
/// The edge direction is: `source` is blocked by `target`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlockingEdge {
    /// Qualified ID of the issue that is blocked.
    pub source: QualifiedId,
    /// Qualified ID of the issue that blocks `source`.
    pub target: QualifiedId,
}

/// Lightweight summary of an issue for list and ready-set responses.
///
/// Contains only the fields needed for display and sorting, avoiding the
/// full weight of [`Issue`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueSummary {
    /// Fully qualified issue identifier (`owner/repo#number`).
    pub qualified_id: QualifiedId,
    /// GitHub issue number (convenience accessor, same as `qualified_id.number`).
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
    /// Date until which the issue is deferred.
    ///
    /// Issues with `defer_until > today` are excluded from the default
    /// ready set. Populated from [`Issue::defer_until`].
    pub defer_until: Option<NaiveDate>,
    /// Labels attached to the issue.
    pub labels: Vec<String>,
    /// Timestamp when the issue was created.
    pub created_at: DateTime<Utc>,
    /// HTML URL for linking back to GitHub.
    pub url: String,
}

/// A reference to an issue, either local (same repo) or cross-repo.
///
/// Used by the `create` tool's `blocked_by` parameter to accept both local
/// issue numbers and cross-repo references in `owner/repo#number` format.
///
/// # Parsing
///
/// The [`FromStr`](std::str::FromStr) implementation accepts:
/// - `"42"` -> `IssueRef::Local(42)`
/// - `"owner/repo#42"` -> `IssueRef::CrossRepo { owner: "owner", repo: "repo", number: 42 }`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IssueRef {
    /// A local issue number in the same repository.
    Local(u64),
    /// A cross-repo reference: `owner/repo#number`.
    CrossRepo {
        /// Repository owner (e.g. `"websublime"`).
        owner: String,
        /// Repository name (e.g. `"unblock"`).
        repo: String,
        /// Issue number in the target repository.
        number: u64,
    },
}

impl IssueRef {
    /// Resolve this reference to a fully qualified ID using the given repo context.
    ///
    /// - `Local(n)` resolves to `owner/repo#n` using the provided `owner` and `repo`.
    /// - `CrossRepo { owner, repo, number }` is already fully qualified and ignores
    ///   the context parameters.
    #[must_use]
    pub fn resolve(&self, owner: &str, repo: &str) -> QualifiedId {
        match self {
            Self::Local(n) => QualifiedId::new(owner, repo, *n),
            Self::CrossRepo {
                owner,
                repo,
                number,
            } => QualifiedId::new(owner.clone(), repo.clone(), *number),
        }
    }
}

impl std::fmt::Display for IssueRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local(n) => write!(f, "#{n}"),
            Self::CrossRepo {
                owner,
                repo,
                number,
            } => write!(f, "{owner}/{repo}#{number}"),
        }
    }
}

impl std::str::FromStr for IssueRef {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();

        // Try cross-repo format: owner/repo#number
        if let Some((prefix, num_str)) = s.split_once('#') {
            if let Some((owner, repo)) = prefix.split_once('/')
                && !owner.is_empty()
                && !repo.is_empty()
            {
                let number = num_str
                    .parse::<u64>()
                    .map_err(|_| format!("invalid issue number in cross-repo ref: {s}"))?;
                return Ok(Self::CrossRepo {
                    owner: owner.to_owned(),
                    repo: repo.to_owned(),
                    number,
                });
            }
            // Has # but no valid owner/repo prefix
            if prefix.is_empty() {
                // Bare "#number" shorthand for local ref
                let number = num_str
                    .parse::<u64>()
                    .map_err(|_| format!("invalid issue reference: {s}"))?;
                return Ok(Self::Local(number));
            }
            // Malformed cross-repo ref (e.g. "/repo#42", "owner/#42")
            return Err(format!("invalid issue reference: {s}"));
        }

        // Plain number
        let number = s
            .parse::<u64>()
            .map_err(|_| format!("invalid issue reference: {s}"))?;
        Ok(Self::Local(number))
    }
}

/// Direction for dependency tree traversal.
///
/// Controls which edges are followed during BFS traversal of the
/// dependency graph. Used by [`DependencyGraph::dependency_tree()`]
/// to walk upstream (blockers), downstream (blocked issues), or both
/// directions simultaneously.
///
/// [`DependencyGraph::dependency_tree()`]: crate::graph::DependencyGraph::dependency_tree
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TraversalDirection {
    /// Follow outgoing edges — walk toward blockers (who blocks this issue?).
    Upstream,
    /// Follow incoming edges — walk toward dependents (what does this issue block?).
    Downstream,
    /// Follow both directions — upstream blockers and downstream dependents.
    Both,
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

impl From<&str> for BodySections {
    /// Create `BodySections` by parsing a markdown body.
    ///
    /// Equivalent to [`BodySections::from_markdown()`]. Enables idiomatic
    /// conversion via `BodySections::from("## Description\n\n...")` and
    /// `.into()` in generic contexts.
    fn from(body: &str) -> Self {
        Self::from_markdown(body)
    }
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

    // ── QualifiedId ────────────────────────────────────────────────────

    #[test]
    fn qualified_id_display() {
        let qid = QualifiedId::new("acme", "widgets", 42);
        assert_eq!(qid.to_string(), "acme/widgets#42");
    }

    #[test]
    fn qualified_id_parse_roundtrip() {
        let original = QualifiedId::new("acme", "widgets", 42);
        let s = original.to_string();
        let parsed: QualifiedId = s.parse().unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn qualified_id_parse_invalid_no_hash() {
        assert!("acme/widgets".parse::<QualifiedId>().is_err());
    }

    #[test]
    fn qualified_id_parse_invalid_no_slash() {
        assert!("acme#42".parse::<QualifiedId>().is_err());
    }

    #[test]
    fn qualified_id_parse_invalid_empty_owner() {
        assert!("/widgets#42".parse::<QualifiedId>().is_err());
    }

    #[test]
    fn qualified_id_parse_invalid_empty_repo() {
        assert!("acme/#42".parse::<QualifiedId>().is_err());
    }

    #[test]
    fn qualified_id_parse_invalid_number() {
        assert!("acme/widgets#abc".parse::<QualifiedId>().is_err());
    }

    #[test]
    fn qualified_id_serde_roundtrip() {
        let qid = QualifiedId::new("acme", "widgets", 42);
        let json = serde_json::to_string(&qid).expect("serialize");
        assert_eq!(json, "\"acme/widgets#42\"");
        let back: QualifiedId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(qid, back);
    }

    #[test]
    fn qualified_id_hash_eq_distinct_repos() {
        use std::collections::HashSet;
        let a = QualifiedId::new("acme", "widgets", 42);
        let b = QualifiedId::new("acme", "gadgets", 42);
        assert_ne!(a, b);
        // Verify they hash differently (with high probability).
        let mut set = HashSet::new();
        set.insert(a.clone());
        set.insert(b.clone());
        assert_eq!(set.len(), 2);
    }

    // ── IssueRef::resolve ──────────────────────────────────────────────

    #[test]
    fn issue_ref_resolve_local() {
        let r = IssueRef::Local(42);
        let qid = r.resolve("acme", "widgets");
        assert_eq!(qid, QualifiedId::new("acme", "widgets", 42));
    }

    #[test]
    fn issue_ref_resolve_cross_repo_ignores_context() {
        let r = IssueRef::CrossRepo {
            owner: "other".to_owned(),
            repo: "stuff".to_owned(),
            number: 7,
        };
        let qid = r.resolve("acme", "widgets");
        assert_eq!(qid, QualifiedId::new("other", "stuff", 7));
    }

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

    // ── From<&str> for BodySections ─────────────────────────────────────

    #[test]
    fn from_str_delegates_to_from_markdown() {
        let body = "\
## Description

A description.

## Design Notes

Some notes.";

        let from_trait: BodySections = body.into();
        let from_method = BodySections::from_markdown(body);
        assert_eq!(from_trait, from_method);
    }

    #[test]
    fn from_str_empty_body() {
        let sections = BodySections::from("");
        assert_eq!(sections, BodySections::default());
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

    // ── IssueRef ────────────────────────────────────────────────────────

    #[test]
    fn issue_ref_parse_local_number() {
        let r: IssueRef = "42".parse().unwrap();
        assert_eq!(r, IssueRef::Local(42));
    }

    #[test]
    fn issue_ref_parse_hash_prefix() {
        let r: IssueRef = "#42".parse().unwrap();
        assert_eq!(r, IssueRef::Local(42));
    }

    #[test]
    fn issue_ref_parse_cross_repo() {
        let r: IssueRef = "acme/widgets#99".parse().unwrap();
        assert_eq!(
            r,
            IssueRef::CrossRepo {
                owner: "acme".to_owned(),
                repo: "widgets".to_owned(),
                number: 99,
            }
        );
    }

    #[test]
    fn issue_ref_display_local() {
        assert_eq!(IssueRef::Local(42).to_string(), "#42");
    }

    #[test]
    fn issue_ref_display_cross_repo() {
        assert_eq!(
            IssueRef::CrossRepo {
                owner: "acme".to_owned(),
                repo: "widgets".to_owned(),
                number: 99,
            }
            .to_string(),
            "acme/widgets#99"
        );
    }

    #[test]
    fn issue_ref_parse_invalid_returns_error() {
        assert!("not-a-number".parse::<IssueRef>().is_err());
    }

    #[test]
    fn issue_ref_parse_whitespace_trimmed() {
        let r: IssueRef = "  42  ".parse().unwrap();
        assert_eq!(r, IssueRef::Local(42));
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
