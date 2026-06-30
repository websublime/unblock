//! The pure bulk-markdown parser (T2.3/D22) — a **byte-faithful port** of
//! `temp/beads_rust-main/src/util/markdown_import.rs::parse_markdown_content` (NOT the file-reading
//! `parse_markdown_file`: the MCP surface takes INLINE content, so the extension / path-traversal /
//! symlink / size file guards are EXCLUDED — file ingestion + path confinement are a T3.1 CLI concern).
//!
//! # Grammar (authoritative — do NOT reduce)
//!
//! - Each issue starts with an H2 line `## Issue Title`.
//! - Per-issue sections are H3 lines `### Section Name` (case-insensitive set: ID, Parent, Priority,
//!   Type, Description, Design, Acceptance Criteria / Acceptance, Assignee, Labels, Dependencies /
//!   Deps, Agent Context / agent-context / `agent_context`). Unknown sections are ignored.
//! - The **implicit-description quirk**: lines after the H2 before any H3 are description, but **only
//!   the first non-empty line** is captured; an explicit `### Description` overrides it.
//! - Dep encoding `type:id` / bare (→ `blocks`) / `external:…`, with an invalid-type prefix treated
//!   as part of the id (a title with a colon). The `blocked-by` type is PRESERVED verbatim here — the
//!   `blocked-by`→`blocks` alias flip is the ENGINE's job at the edge-build step, NOT the parser's.
//! - Bulleted / checkbox (`- ` / `* ` / `+ `, `[ ]` / `[x]`) list items are stripped.
//! - The `### ID` stand-in is a symbolic intra-file handle (NOT the minted id).
//!
//! Pure (no `Session`, no I/O) so it is unit-/proptest-/fuzz-testable in isolation. The `issue.rs`
//! adapter owns the pre-mutation all-or-nothing parse + the `ParsedIssue → NewIssue` mapping, then
//! hands the `Vec<NewIssue>` (carrying the symbolic refs verbatim) to the ATOMIC `Session::create_bulk`.

use unblock_error::{ErrorCode, StructuredError};

/// A parsed issue from the markdown document (mcp-owned; the `issue.rs` adapter maps it to a
/// `NewIssue`). Mirrors the original `markdown_import.rs::ParsedIssue` field set.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ParsedIssue {
    /// The H2 title text.
    pub title: String,
    /// The optional `### ID` stand-in (a symbolic intra-file handle; NOT the minted id).
    pub stand_in_id: Option<String>,
    /// The optional `### Parent` reference (a title / stand-in / pre-existing id).
    pub parent: Option<String>,
    /// The optional `### Priority` string (e.g. `"0"`, `"P1"`, `"2"`).
    pub priority: Option<String>,
    /// The optional `### Type` string (e.g. `"task"`, `"bug"`).
    pub issue_type: Option<String>,
    /// The description (implicit first pre-H3 line, or an explicit `### Description`).
    pub description: Option<String>,
    /// The `### Design` content.
    pub design: Option<String>,
    /// The `### Acceptance Criteria` / `### Acceptance` content.
    pub acceptance_criteria: Option<String>,
    /// The `### Assignee` content.
    pub assignee: Option<String>,
    /// The `### Labels` (split on commas / whitespace, list-prefix stripped).
    pub labels: Vec<String>,
    /// The verbatim `### Dependencies` / `### Deps` reference strings.
    pub dependencies: Vec<String>,
    /// The `### Agent Context` / `agent-context` / `agent_context` opaque content.
    pub agent_context: Option<String>,
}

/// The H3 section types recognized in the markdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    /// Before any H3, capturing the implicit description.
    BeforeH3,
    Id,
    Parent,
    Priority,
    Type,
    Description,
    Design,
    AcceptanceCriteria,
    Assignee,
    Labels,
    Dependencies,
    AgentContext,
    Unknown,
}

impl Section {
    fn from_header(header: &str) -> Self {
        let normalized = header.trim().to_lowercase();
        match normalized.as_str() {
            "id" => Self::Id,
            "parent" => Self::Parent,
            "priority" => Self::Priority,
            "type" => Self::Type,
            "description" => Self::Description,
            "design" => Self::Design,
            "acceptance criteria" | "acceptance" => Self::AcceptanceCriteria,
            "assignee" => Self::Assignee,
            "labels" => Self::Labels,
            "dependencies" | "deps" => Self::Dependencies,
            "agent context" | "agent-context" | "agent_context" => Self::AgentContext,
            _ => Self::Unknown,
        }
    }
}

/// Parse bulk-markdown content into a list of [`ParsedIssue`]s (a byte-faithful port of
/// `parse_markdown_content`).
///
/// # Errors
///
/// Returns a `ValidationFailed` [`StructuredError`] when the content has non-whitespace text but no
/// `## Title` header (faithful to the original's "no issues found" rejection). Otherwise the parse is
/// total (it never rejects an individual record — the all-or-nothing batch rejection on bad
/// references is the ENGINE's job at `create_bulk`).
pub(crate) fn parse_bulk_markdown(content: &str) -> Result<Vec<ParsedIssue>, StructuredError> {
    let has_non_whitespace_content = content.lines().any(|line| !line.trim().is_empty());
    let mut issues = Vec::new();
    let mut current_issue: Option<ParsedIssue> = None;
    let mut current_section = Section::BeforeH3;
    let mut section_lines: Vec<String> = Vec::new();
    let mut captured_implicit_desc = false;

    for line in content.lines() {
        // Check for H2 (new issue).
        if let Some(stripped) = line.strip_prefix("## ") {
            // Save the previous issue.
            if let Some(mut issue) = current_issue.take() {
                apply_section_to_issue(&mut issue, current_section, &section_lines);
                issues.push(issue);
            }

            // Start a new issue.
            let title = stripped.trim().to_string();
            current_issue = Some(ParsedIssue {
                title,
                ..Default::default()
            });
            current_section = Section::BeforeH3;
            section_lines.clear();
            captured_implicit_desc = false;
            continue;
        }

        // Check for H3 (section header).
        if let Some(stripped) = line.strip_prefix("### ") {
            if let Some(issue) = current_issue.as_mut() {
                // Apply the previous section.
                apply_section_to_issue(issue, current_section, &section_lines);

                // Start the new section.
                let header = stripped.trim();
                current_section = Section::from_header(header);
                section_lines.clear();
            }
            continue;
        }

        // Collect content for the current section.
        if current_issue.is_some() {
            if current_section == Section::BeforeH3 {
                if !captured_implicit_desc && !line.trim().is_empty() {
                    section_lines.push(line.to_string());
                    captured_implicit_desc = true;
                }
            } else {
                section_lines.push(line.to_string());
            }
        }
    }

    // The last issue.
    if let Some(mut issue) = current_issue {
        apply_section_to_issue(&mut issue, current_section, &section_lines);
        issues.push(issue);
    }

    if issues.is_empty() && has_non_whitespace_content {
        return Err(StructuredError::from_code(
            ErrorCode::ValidationFailed,
            "no issues found; expected '## Title' headers",
        )
        .with_hint("each issue starts with an H2 line: `## Issue Title`"));
    }

    Ok(issues)
}

/// Apply the collected section content to an issue (faithful to `apply_section_to_issue`).
fn apply_section_to_issue(issue: &mut ParsedIssue, section: Section, lines: &[String]) {
    let content = lines.join("\n").trim().to_string();
    if content.is_empty() {
        return;
    }

    match section {
        Section::BeforeH3 => {
            // Implicit description (first non-empty line only).
            if issue.description.is_none() {
                issue.description = Some(content);
            }
        }
        Section::Id => issue.stand_in_id = Some(content),
        Section::Parent => issue.parent = Some(content),
        Section::Priority => issue.priority = Some(content),
        Section::Type => issue.issue_type = Some(content),
        Section::Description => issue.description = Some(content),
        Section::Design => issue.design = Some(content),
        Section::AcceptanceCriteria => issue.acceptance_criteria = Some(content),
        Section::Assignee => issue.assignee = Some(content),
        Section::Labels => issue.labels = split_list_content(&content),
        Section::Dependencies => issue.dependencies = split_dependency_content(&content),
        Section::AgentContext => issue.agent_context = Some(content),
        Section::Unknown => {} // ignore unknown sections.
    }
}

/// Split dependency content, preserving bulleted lines as whole items (faithful to
/// `split_dependency_content`).
fn split_dependency_content(content: &str) -> Vec<String> {
    let mut result = Vec::new();
    for raw_line in content.lines() {
        let trimmed = raw_line.trim_start();
        let is_bulleted =
            trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("+ ");
        let line = strip_markdown_list_prefix(raw_line).trim();
        if line.is_empty() || is_marker_only_token(line) {
            continue;
        }
        if is_bulleted {
            // Treat the whole stripped line as a single dependency reference (title-based refs).
            result.push(line.to_string());
        } else if line.contains(',') {
            result.extend(
                line.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty() && !is_marker_only_token(s)),
            );
        } else {
            result.extend(split_whitespace_items_preserving_colon_pairs(line));
        }
    }
    result
}

/// Split content on commas or whitespace for labels (faithful to `split_list_content`).
fn split_list_content(content: &str) -> Vec<String> {
    let mut result = Vec::new();
    for raw_line in content.lines() {
        let line = strip_markdown_list_prefix(raw_line).trim();
        if line.is_empty() || is_marker_only_token(line) {
            continue;
        }
        if line.contains(',') {
            result.extend(
                line.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty() && !is_marker_only_token(s)),
            );
        } else {
            result.extend(split_whitespace_items_preserving_colon_pairs(line));
        }
    }
    result
}

/// Split on whitespace, preserving `type:` + value as one item (faithful to the original).
fn split_whitespace_items_preserving_colon_pairs(line: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut pending_colon_prefix: Option<String> = None;

    for token in line
        .split_whitespace()
        .filter(|token| !token.is_empty() && !is_marker_only_token(token))
    {
        if let Some(mut prefix) = pending_colon_prefix.take() {
            prefix.push(' ');
            prefix.push_str(token);
            items.push(prefix);
            continue;
        }

        if token.ends_with(':') && token != ":" {
            pending_colon_prefix = Some(token.to_string());
        } else {
            items.push(token.to_string());
        }
    }

    if let Some(prefix) = pending_colon_prefix {
        items.push(prefix);
    }

    items
}

/// Strip a leading markdown list marker (`- ` / `* ` / `+ `) then a checkbox prefix.
fn strip_markdown_list_prefix(line: &str) -> &str {
    let trimmed = line.trim_start();
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return strip_markdown_checkbox_prefix(rest);
        }
    }
    strip_markdown_checkbox_prefix(trimmed)
}

/// Strip a leading checkbox prefix (`[ ] ` / `[x] ` / `[X] `).
fn strip_markdown_checkbox_prefix(line: &str) -> &str {
    let trimmed = line.trim_start();
    for marker in ["[ ] ", "[x] ", "[X] "] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return rest;
        }
    }
    trimmed
}

/// Whether a token is a bare list marker (`-` / `*` / `+`).
fn is_marker_only_token(token: &str) -> bool {
    matches!(token.trim(), "-" | "*" | "+")
}

#[cfg(test)]
mod tests {
    use super::{ParsedIssue, parse_bulk_markdown};

    fn parse(content: &str) -> Vec<ParsedIssue> {
        parse_bulk_markdown(content).expect("parse")
    }

    #[test]
    fn parse_simple_issue() {
        let content = "## My First Issue\n### Parent\nproj-abc123\n\n### Description\nThis is the description.\n\n### Priority\n1\n\n### Type\nbug\n";
        let issues = parse(content);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].title, "My First Issue");
        assert_eq!(issues[0].parent.as_deref(), Some("proj-abc123"));
        assert_eq!(
            issues[0].description.as_deref(),
            Some("This is the description.")
        );
        assert_eq!(issues[0].priority.as_deref(), Some("1"));
        assert_eq!(issues[0].issue_type.as_deref(), Some("bug"));
    }

    #[test]
    fn parse_multiple_issues() {
        let content = "## Issue One\n### Type\ntask\n\n## Issue Two\n### Type\nfeature\n\n## Issue Three\n### Type\nbug\n";
        let issues = parse(content);
        assert_eq!(issues.len(), 3);
        assert_eq!(issues[0].title, "Issue One");
        assert_eq!(issues[1].title, "Issue Two");
        assert_eq!(issues[2].title, "Issue Three");
    }

    #[test]
    fn implicit_description_quirk_first_line_only() {
        let content = "## Issue Title\nFirst line becomes description\nThis line is ignored\nAnd this one too\n\n### Priority\n2\n";
        let issues = parse(content);
        assert_eq!(
            issues[0].description.as_deref(),
            Some("First line becomes description")
        );
    }

    #[test]
    fn explicit_description_overrides_implicit() {
        let content = "## Test Issue\nImplicit description line\n\n### Description\nExplicit description content\n";
        let issues = parse(content);
        assert_eq!(
            issues[0].description.as_deref(),
            Some("Explicit description content")
        );
    }

    #[test]
    fn labels_comma_separated() {
        let issues = parse("## Test Issue\n### Labels\nbug, urgent, frontend\n");
        assert_eq!(issues[0].labels, vec!["bug", "urgent", "frontend"]);
    }

    #[test]
    fn labels_whitespace_separated() {
        let issues = parse("## Test Issue\n### Labels\nbug urgent frontend\n");
        assert_eq!(issues[0].labels, vec!["bug", "urgent", "frontend"]);
    }

    #[test]
    fn dependencies_parsing() {
        let issues =
            parse("## Test Issue\n### Dependencies\nblocks:bd-123, bd-456, related:bd-789\n");
        assert_eq!(
            issues[0].dependencies,
            vec!["blocks:bd-123", "bd-456", "related:bd-789"]
        );
    }

    #[test]
    fn dependencies_bullets_and_checkboxes_stripped() {
        let content = "## Test Issue\n### Dependencies\n- bd-123\n- [ ] related:bd-456\n* external:github#123\n";
        let issues = parse(content);
        assert_eq!(
            issues[0].dependencies,
            vec!["bd-123", "related:bd-456", "external:github#123"]
        );
    }

    #[test]
    fn dependencies_whitespace_typed_tokens() {
        let issues = parse(
            "## Test Issue\n### Dependencies\nblocks: bd-123 related:bd-456 external:github#123\n",
        );
        assert_eq!(
            issues[0].dependencies,
            vec!["blocks: bd-123", "related:bd-456", "external:github#123"]
        );
    }

    #[test]
    fn non_bulleted_deps_split_on_whitespace() {
        let issues = parse("## Test Issue\n### Dependencies\nbd-123 bd-456\n");
        assert_eq!(issues[0].dependencies, vec!["bd-123", "bd-456"]);
    }

    #[test]
    fn acceptance_criteria_alias() {
        let issues =
            parse("## Test Issue\n### Acceptance\n- [ ] First criterion\n- [ ] Second criterion\n");
        let ac = issues[0].acceptance_criteria.as_deref().expect("ac");
        assert!(ac.contains("First criterion"));
    }

    #[test]
    fn case_insensitive_sections() {
        let content =
            "## Test Issue\n### PRIORITY\n1\n\n### description\nTest desc\n\n### TYPE\ntask\n";
        let issues = parse(content);
        assert_eq!(issues[0].priority.as_deref(), Some("1"));
        assert_eq!(issues[0].description.as_deref(), Some("Test desc"));
        assert_eq!(issues[0].issue_type.as_deref(), Some("task"));
    }

    #[test]
    fn unknown_sections_ignored() {
        let content = "## Test Issue\n### Unknown Section\nThis content should be ignored.\n\n### Description\nThis is the actual description.\n";
        let issues = parse(content);
        assert_eq!(
            issues[0].description.as_deref(),
            Some("This is the actual description.")
        );
    }

    #[test]
    fn stand_in_id_section() {
        let content = "## Build Database Schema\n### ID\ndb-1\n### Type\ntask\n\n## Build API Endpoints\n### Type\nfeature\n### Dependencies\ndb-1\n";
        let issues = parse(content);
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].stand_in_id.as_deref(), Some("db-1"));
        assert_eq!(issues[1].dependencies, vec!["db-1"]);
    }

    #[test]
    fn title_based_dependencies_bulleted() {
        let content = "## Build API Endpoints\n### Type\nfeature\n### Dependencies\n- Build Database Schema\n\n## Build Database Schema\n### Type\ntask\n";
        let issues = parse(content);
        assert_eq!(issues[0].dependencies, vec!["Build Database Schema"]);
    }

    #[test]
    fn design_section() {
        let issues = parse("## Test Issue\n### Design\nDesign notes here.\nMulti-line content.\n");
        let design = issues[0].design.as_deref().expect("design");
        assert!(design.contains("Design notes"));
    }

    #[test]
    fn agent_context_aliases() {
        for header in [
            "Agent Context",
            "agent-context",
            "AGENT CONTEXT",
            "agent_context",
        ] {
            let content = format!("## Issue\n### {header}\nopaque body\n");
            let issues = parse(&content);
            assert_eq!(
                issues[0].agent_context.as_deref(),
                Some("opaque body"),
                "header `{header}` should map to agent_context"
            );
        }
    }

    #[test]
    fn agent_context_opaque_multiline() {
        let content = "## Issue\n### Agent Context\n{\"skills\": [\"porting-to-rust\"]}\nsecond line of opaque text\n";
        let issues = parse(content);
        let ctx = issues[0].agent_context.as_deref().expect("ctx");
        assert!(ctx.contains("porting-to-rust"));
        assert!(ctx.contains("second line of opaque text"));
    }

    #[test]
    fn assignee_section() {
        let issues = parse("## Test Issue\n### Assignee\nalice\n");
        assert_eq!(issues[0].assignee.as_deref(), Some("alice"));
    }

    #[test]
    fn blocked_by_dep_preserved_verbatim() {
        // The parser PRESERVES the `blocked-by` type string; the flip to `blocks` is the engine's job.
        let issues = parse("## Test Issue\n### Dependencies\nblocked-by:ub-1\n");
        assert_eq!(issues[0].dependencies, vec!["blocked-by:ub-1"]);
    }

    #[test]
    fn empty_content_is_ok_empty() {
        let issues = parse("   \n\n  ");
        assert!(issues.is_empty());
    }

    #[test]
    fn non_empty_without_header_rejects() {
        let err = parse_bulk_markdown("### Description\nNo issue header here.\n")
            .expect_err("must reject");
        assert_eq!(err.code, unblock_error::ErrorCode::ValidationFailed);
    }

    #[test]
    fn arbitrary_utf8_never_panics() {
        // A property-ish smoke: pathological inputs must not panic.
        for content in ["", "##", "###", "## \n###\n", "## t\n### \n", "\u{0}## x"] {
            let _ = parse_bulk_markdown(content);
        }
    }
}
