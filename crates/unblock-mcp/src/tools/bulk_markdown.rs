//! The pure bulk-markdown parser (T2.3/D22) — a **byte-faithful port** of
//! `temp/beads_rust-main/src/util/markdown_import.rs::parse_markdown_content` (NOT the file-reading
//! `parse_markdown_file`: the MCP surface takes INLINE content, so the extension / path-traversal /
//! symlink / size file guards are EXCLUDED — file ingestion + path confinement are a T3.1 CLI concern).
//!
//! **DELIBERATE DEVIATIONS FROM THE PORT (D42, v1.0.1).** The original accepted-and-discarded four
//! classes of malformed input in silence; unblock's safe-import discipline (NFR-8) rejects them
//! instead, in-band, before any write. D22 already set the precedent for overriding the port on this
//! axis (its own row records the best-effort-`continue` deviation). The four are: an unrecognized
//! `### ` section, an EMPTY `### ` header, a `### ` section before the first `## `, and — at the
//! `issue.rs` mapping step — an invalid `### Priority` value.
//!
//! **A FIFTH rejection is NOT of that class and must not be filed with them.** The UNTERMINATED
//! fence (below) rejects documents GA v1.0.0 **ACCEPTED AND IMPORTED** — GA's parser carried no
//! fence tracking at all — so it deviates from SHIPPED GA BEHAVIOUR, not merely from `CommonMark`:
//! a behavioural break shipping in a PATCH release, RATIFIED at PRD D42 clause 4(iii).
//!
//! # Grammar (authoritative — do NOT reduce)
//!
//! - Each issue starts with an H2 line `## Issue Title`.
//! - Per-issue sections are H3 lines `### Section Name` (case-insensitive set: ID, Parent, Priority,
//!   Type, Description, Design, Acceptance Criteria / Acceptance, Assignee, Labels, Dependencies /
//!   Deps, Agent Context / agent-context / `agent_context`). **An unrecognized `### Section` REJECTS
//!   the whole document** (`ValidationFailed`, zero writes) — as does an EMPTY `### ` header, with a
//!   distinct message. Note `### ` always starts a NEW section: use `#### ` for a sub-heading inside
//!   a section body (`"#### Sub".strip_prefix("### ")` is `None`, so H4 stays plain content).
//! - **CODE BLOCKS ARE OPAQUE.** `## ` / `### ` lines inside a FENCED code block are CONTENT, never
//!   headers ([`fence_delimiter`]). Without this the D42 rejections above would be a false positive
//!   on any document embedding a markdown code sample — an author controls their own headings but
//!   NOT the bytes of a code example — and, worse, a *known* section name inside a fence would tear
//!   the fence in half and relocate the sample's bytes into another field with `isError:false`.
//!   **INDENTED code blocks need no tracking**: `strip_prefix("### ")` already fails on an indented
//!   line, so their content was never mistaken for a header (pinned by
//!   `an_indented_code_block_h3_is_already_content`).
//! - An **UNTERMINATED fence REJECTS** (`kind = "unterminated_code_fence"`, naming the OPENING
//!   line). This is a DELIBERATE deviation from `CommonMark`, which lets an unclosed fence run to the
//!   end of the document: that reading would silently swallow every later `## `/`### ` into one
//!   section — the exact silent-drop class D42 closes. **It is ALSO a deviation from SHIPPED GA
//!   BEHAVIOUR**, which is the stronger and more important statement: GA v1.0.0 ACCEPTED such
//!   documents, so this is a behavioural break in a PATCH release, not a silent-drop closure.
//!   Ratified at PRD D42 clause 4(iii) and PUBLISHED in the `create_bulk` + `markdown` wire
//!   descriptions (`issue.rs`) — do not let those two descriptions drift from this grammar.
//! - A `### ` section appearing **before the first `## `** REJECTS the document. Previously it was
//!   consumed and discarded along with its body.
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
}

/// The closed set of accepted `### Section` spellings, in the order [`Section::from_header`] matches
/// them. Published on the wire ONLY through the rejection `hint` and the `markdown` field
/// description — there is no other place a client can discover it, which is why the enumerating hint
/// is load-bearing rather than cosmetic.
const ACCEPTED_SECTIONS: &str = "id, parent, priority, type, description, design, \
     acceptance criteria, acceptance, assignee, labels, dependencies, deps, agent context, \
     agent-context, agent_context";

/// The maximum number of characters of an echoed `### ` header text.
///
/// A header is otherwise bounded only by `Quotas::max_string_len` (the whole markdown document is
/// ONE string), so an unbounded echo would amplify attacker-controlled text into the error payload.
/// `StructuredError::from_code` sanitizes but does NOT truncate, and `with_context` does neither.
const MAX_ECHOED_HEADER_CHARS: usize = 80;

/// Truncate an echoed header on a char boundary. Sanitization happens downstream, in
/// `StructuredError`'s constructors — truncate FIRST, so a cut can never land inside an escape.
fn clip_header(header: &str) -> String {
    if header.chars().count() <= MAX_ECHOED_HEADER_CHARS {
        return header.to_string();
    }
    let kept: String = header.chars().take(MAX_ECHOED_HEADER_CHARS).collect();
    format!("{kept}…[truncated]")
}

impl Section {
    /// Map an H3 header to its [`Section`], or REJECT.
    ///
    /// # Errors
    ///
    /// Returns a `ValidationFailed` [`StructuredError`] for an EMPTY header (`kind =
    /// "empty_section_header"`) or an unrecognized one (`kind = "unknown_section"`). Before D42 both
    /// mapped to a `Section::Unknown` variant whose content was silently discarded.
    ///
    /// The two kinds are deliberately DISTINCT: an unnamed section and an unrecognized section are
    /// different user errors, and only the second can be fixed by consulting the accepted-name list.
    fn from_header(header: &str, line_no: usize) -> Result<Self, StructuredError> {
        let normalized = header.trim().to_lowercase();
        match normalized.as_str() {
            "id" => Ok(Self::Id),
            "parent" => Ok(Self::Parent),
            "priority" => Ok(Self::Priority),
            "type" => Ok(Self::Type),
            "description" => Ok(Self::Description),
            "design" => Ok(Self::Design),
            "acceptance criteria" | "acceptance" => Ok(Self::AcceptanceCriteria),
            "assignee" => Ok(Self::Assignee),
            "labels" => Ok(Self::Labels),
            "dependencies" | "deps" => Ok(Self::Dependencies),
            "agent context" | "agent-context" | "agent_context" => Ok(Self::AgentContext),
            "" => Err(empty_section_header(line_no)),
            _ => Err(unknown_section(header, line_no)),
        }
    }
}

/// An EMPTY `### ` header (H3 marker, no name).
///
/// Before D42 `Section::from_header("")` yielded `Unknown`, i.e. `### ` was an *ignored* section —
/// the same silent-drop class every other D42 rejection closes. Rejecting it keeps the grammar
/// consistent; treating it as content would add a second special case whose only purpose is to keep
/// a silent drop alive.
fn empty_section_header(line_no: usize) -> StructuredError {
    StructuredError::from_code(
        ErrorCode::ValidationFailed,
        format!("line {line_no}: an empty `### ` section header"),
    )
    .with_hint(format!(
        "name the section — one of: {ACCEPTED_SECTIONS} (case-insensitive). \
         `### ` always starts a NEW section; use `#### ` for a sub-heading inside a section body."
    ))
    .with_context("field", serde_json::json!("markdown"))
    .with_context("kind", serde_json::json!("empty_section_header"))
    .with_context("line", serde_json::json!(line_no))
}

/// An unrecognized `### Section` header. **Rejects the whole document (D42, SUPERSEDES D22 clause 2).**
fn unknown_section(header: &str, line_no: usize) -> StructuredError {
    let header = clip_header(header);
    StructuredError::from_code(
        ErrorCode::ValidationFailed,
        format!("line {line_no}: unrecognized `### {header}` section"),
    )
    .with_hint(format!(
        "accepted sections are: {ACCEPTED_SECTIONS} (case-insensitive). \
         `### ` always starts a NEW section; use `#### ` for a sub-heading inside a section body, \
         or wrap a code sample in a ``` fence — headers inside a fence are content."
    ))
    .with_context("field", serde_json::json!("markdown"))
    .with_context("kind", serde_json::json!("unknown_section"))
    .with_context("section", serde_json::json!(header))
    .with_context("line", serde_json::json!(line_no))
}

/// An OPEN fenced code block, carried as parser state through the single pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OpenFence {
    /// The delimiter character — a backtick or a tilde. A tilde fence is NOT closed by backticks.
    marker: char,
    /// The delimiter run length of the OPENING fence. A closing run must be at least this long, so
    /// a shorter run appearing inside the block (the usual way a sample nests a fence) is content.
    run_len: usize,
    /// The 1-based line the fence opened on — the actionable line for `unterminated_code_fence`.
    line_no: usize,
}

/// Classify a line as a code-fence delimiter: `(marker, run length, info string)`.
///
/// Follows `CommonMark`'s fenced-code-block opener rules to the extent that they decide whether a line
/// is a DELIMITER at all: up to 3 leading spaces of indentation, then a run of **at least 3**
/// backticks or tildes, then an optional info string. Returns `None` for anything else.
fn fence_delimiter(line: &str) -> Option<(char, usize, &str)> {
    let rest = line.trim_start_matches(' ');
    if line.len() - rest.len() > 3 {
        // 4+ spaces makes it an INDENTED code block line, not a fence delimiter.
        return None;
    }
    let marker = match rest.chars().next()? {
        '`' => '`',
        '~' => '~',
        _ => return None,
    };
    let run_len = rest.chars().take_while(|c| *c == marker).count();
    if run_len < 3 {
        return None;
    }
    Some((marker, run_len, rest[run_len..].trim()))
}

/// Whether `line` OPENS a fenced code block, and with what delimiter.
///
/// `CommonMark` forbids a backtick in the info string of a backtick fence (it would be ambiguous with
/// inline code), so such a line is ordinary content — pinned by
/// `a_backtick_in_a_backtick_info_string_does_not_open_a_fence`.
fn fence_opener(line: &str) -> Option<(char, usize)> {
    let (marker, run_len, info) = fence_delimiter(line)?;
    if marker == '`' && info.contains('`') {
        return None;
    }
    Some((marker, run_len))
}

/// Whether `line` CLOSES `open`: the same marker, a run at least as long, and NO info string.
fn closes_fence(line: &str, open: OpenFence) -> bool {
    matches!(
        fence_delimiter(line),
        Some((marker, run_len, info))
            if marker == open.marker && run_len >= open.run_len && info.is_empty()
    )
}

/// A fenced code block opened and never closed.
///
/// `CommonMark` would let it run to EOF; unblock REJECTS instead, because that reading silently
/// swallows every later `## `/`### ` into one section — the silent-drop class D42 closes. The
/// reported line is the OPENING one: EOF is where the problem is detected, not where it is fixed.
fn unterminated_code_fence(open: OpenFence) -> StructuredError {
    let delimiter: String = std::iter::repeat_n(open.marker, open.run_len).collect();
    StructuredError::from_code(
        ErrorCode::ValidationFailed,
        format!(
            "line {}: unterminated `{delimiter}` code fence",
            open.line_no
        ),
    )
    .with_hint(format!(
        "close the fence with a line of at least {} `{}` characters and nothing else. \
         Inside a fence, ``` `## ` ``` and ``` `### ` ``` are content, not headers — an unclosed \
         fence would silently swallow every later header into one section.",
        open.run_len, open.marker
    ))
    .with_context("field", serde_json::json!("markdown"))
    .with_context("kind", serde_json::json!("unterminated_code_fence"))
    .with_context("fence", serde_json::json!(delimiter))
    .with_context("line", serde_json::json!(open.line_no))
}

/// A `### Section` appearing before the first `## Issue Title`.
///
/// Before D42 the H3 branch was gated on `if let Some(issue) = current_issue.as_mut()` and
/// `continue`d regardless, so such a header AND its whole body were consumed and discarded while the
/// parse returned `Ok`.
fn section_before_first_issue(header: &str, line_no: usize) -> StructuredError {
    let header = clip_header(header);
    StructuredError::from_code(
        ErrorCode::ValidationFailed,
        format!("line {line_no}: `### {header}` appears before the first `## Issue Title`"),
    )
    .with_hint("a `### Section` must follow a `## Issue Title`; add the H2 header above it")
    .with_context("field", serde_json::json!("markdown"))
    .with_context("kind", serde_json::json!("section_before_issue"))
    .with_context("section", serde_json::json!(header))
    .with_context("line", serde_json::json!(line_no))
}

/// Parse bulk-markdown content into a list of [`ParsedIssue`]s (a byte-faithful port of
/// `parse_markdown_content`).
///
/// # Errors
///
/// Returns a `ValidationFailed` [`StructuredError`], all-or-nothing and before any write, when:
///
/// - the content has non-whitespace text but no `## Title` header (`kind = "no_issues"`; faithful to
///   the original's "no issues found" rejection);
/// - a `### Section` appears before the first `## ` (`kind = "section_before_issue"`, D42);
/// - a `### ` header is EMPTY (`kind = "empty_section_header"`, D42);
/// - a `### Section` header is unrecognized (`kind = "unknown_section"`, D42 — SUPERSEDES D22
///   clause 2, which specified that unknown H3 sections be ignored).
///
/// The pre-D42 wording — *"otherwise the parse is total (it never rejects an individual record)"* —
/// is no longer true and has been removed rather than softened. The all-or-nothing batch rejection
/// on bad *references* remains the ENGINE's job at `create_bulk`.
pub(crate) fn parse_bulk_markdown(content: &str) -> Result<Vec<ParsedIssue>, StructuredError> {
    let has_non_whitespace_content = content.lines().any(|line| !line.trim().is_empty());
    let mut issues = Vec::new();
    let mut current_issue: Option<ParsedIssue> = None;
    let mut current_section = Section::BeforeH3;
    let mut section_lines: Vec<String> = Vec::new();
    let mut captured_implicit_desc = false;
    let mut open_fence: Option<OpenFence> = None;

    for (index, line) in content.lines().enumerate() {
        let line_no = index + 1;

        // Fenced code blocks are OPAQUE: while one is open, NOTHING is a header. This runs before
        // the H2/H3 checks precisely so `### ` inside a code sample stays content on both arms —
        // neither rejected as an unknown section nor (worse) silently honoured as a known one.
        if let Some(fence) = open_fence {
            if closes_fence(line, fence) {
                open_fence = None;
            }
            // The delimiter lines themselves are part of the body, verbatim.
            collect_content_line(
                line,
                current_issue.is_some(),
                current_section,
                &mut section_lines,
                &mut captured_implicit_desc,
            );
            continue;
        }
        if let Some((marker, run_len)) = fence_opener(line) {
            open_fence = Some(OpenFence {
                marker,
                run_len,
                line_no,
            });
            collect_content_line(
                line,
                current_issue.is_some(),
                current_section,
                &mut section_lines,
                &mut captured_implicit_desc,
            );
            continue;
        }

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
            let header = stripped.trim();
            // D42: the STRUCTURAL check must precede `from_header`, so `### Bogus` before the first
            // H2 reports the actionable error (the missing H2), not the unknown-section one.
            let Some(issue) = current_issue.as_mut() else {
                return Err(section_before_first_issue(header, line_no));
            };
            // Apply the previous section.
            apply_section_to_issue(issue, current_section, &section_lines);

            // Start the new section. `?` fires at the offending line during the SINGLE pass — no
            // pre-scan is needed, and the existing call order (parse -> batch quota ->
            // Session::create_bulk) already guarantees zero writes.
            current_section = Section::from_header(header, line_no)?;
            section_lines.clear();
            continue;
        }

        // Collect content for the current section.
        collect_content_line(
            line,
            current_issue.is_some(),
            current_section,
            &mut section_lines,
            &mut captured_implicit_desc,
        );
    }

    // A fence that never closed. Checked BEFORE the "no issues found" arm so the actionable cause
    // (the unclosed fence) wins over its symptom (everything after it became one section's body).
    if let Some(fence) = open_fence {
        return Err(unterminated_code_fence(fence));
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
        .with_hint("each issue starts with an H2 line: `## Issue Title`")
        .with_context("field", serde_json::json!("markdown"))
        .with_context("kind", serde_json::json!("no_issues")));
    }

    Ok(issues)
}

/// Append a non-header line to the current section's buffer.
///
/// Extracted from the parse loop because the fence arms need the identical behaviour: a line inside
/// a code block is collected exactly as any other content line, INCLUDING under the
/// implicit-description quirk (`BeforeH3` keeps only the first non-empty line).
fn collect_content_line(
    line: &str,
    in_issue: bool,
    section: Section,
    section_lines: &mut Vec<String>,
    captured_implicit_desc: &mut bool,
) {
    if !in_issue {
        return;
    }
    if section == Section::BeforeH3 {
        if !*captured_implicit_desc && !line.trim().is_empty() {
            section_lines.push(line.to_string());
            *captured_implicit_desc = true;
        }
    } else {
        section_lines.push(line.to_string());
    }
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
        // D42: there is no `Section::Unknown` arm to write — the variant is DELETED, so the literal
        // drop site cannot be re-created here by accident. That closes the ACCIDENTAL path only; the
        // GUARANTEE is the `unknown_section_rejected` test, which turns RED under any re-introduction
        // however it is spelled (including `.unwrap_or(Section::Description)` at the call site, which
        // clippy pedantic does not ban).
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
    use super::{MAX_ECHOED_HEADER_CHARS, ParsedIssue, parse_bulk_markdown};

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

    /// **THE INVERTED TEST (D42, R1-i).** This was `unknown_sections_ignored`, which asserted the
    /// silent drop as CORRECT. It is now the load-bearing guarantee that the drop is gone.
    ///
    /// Deleting the `Section::Unknown` variant closes the ACCIDENTAL path — there is no arm left to
    /// write, and no `None` branch for a future editor to fill with a default. That is a real
    /// reduction in risk; it is NOT a proof. Nothing in the type system stops someone re-adding the
    /// variant, and `Result` does not stop someone writing `.unwrap_or(Section::Description)` at the
    /// call site — clippy pedantic does not ban `unwrap_or`, so CI would not catch that spelling.
    /// **This test is the guarantee**: it turns RED under ANY re-introduction, however spelled.
    #[test]
    fn unknown_section_rejected() {
        let err =
            parse_bulk_markdown("## T\n### Unknown Section\nignored?\n\n### Description\nreal\n")
                .expect_err("an unrecognized `### ` section must REJECT the whole document");
        assert_eq!(err.code, unblock_error::ErrorCode::ValidationFailed);
        assert!(err.retryable, "VALIDATION_FAILED is retryable");
        assert_eq!(err.context["kind"], "unknown_section");
        assert_eq!(err.context["field"], "markdown");
        assert_eq!(
            err.context["section"], "Unknown Section",
            "the ORIGINAL case is preserved in the echoed header"
        );
        assert_eq!(err.context["line"], 2);
        let hint = err.hint.as_deref().expect("an enumerating hint");
        assert!(hint.contains("acceptance criteria"), "{hint}");
        assert!(
            hint.contains("#### "),
            "the H4 rule must be in the hint: {hint}"
        );
    }

    /// Non-vacuity for `unknown_section_rejected`: every accepted spelling still parses, so the
    /// rejection cannot over-fire, and the alias set is pinned against accidental narrowing.
    #[test]
    fn every_accepted_section_spelling_still_parses() {
        /// `(header spelling, the field it must populate)`.
        type Case = (&'static str, fn(&ParsedIssue) -> bool);
        let cases: &[Case] = &[
            ("ID", |i| i.stand_in_id.is_some()),
            ("Parent", |i| i.parent.is_some()),
            ("PRIORITY", |i| i.priority.is_some()),
            ("Type", |i| i.issue_type.is_some()),
            ("Description", |i| i.description.is_some()),
            ("Design", |i| i.design.is_some()),
            ("Acceptance Criteria", |i| i.acceptance_criteria.is_some()),
            ("Acceptance", |i| i.acceptance_criteria.is_some()),
            ("ASSIGNEE", |i| i.assignee.is_some()),
            ("Labels", |i| !i.labels.is_empty()),
            ("Dependencies", |i| !i.dependencies.is_empty()),
            ("Deps", |i| !i.dependencies.is_empty()),
            ("Agent Context", |i| i.agent_context.is_some()),
            ("agent-context", |i| i.agent_context.is_some()),
            ("agent_context", |i| i.agent_context.is_some()),
        ];
        for (header, populated) in cases {
            let content = format!("## T\n### {header}\nvalue\n");
            let issues = parse_bulk_markdown(&content)
                .unwrap_or_else(|e| panic!("`### {header}` must still parse: {e:?}"));
            assert!(
                populated(&issues[0]),
                "`### {header}` parsed but populated no field"
            );
        }
    }

    /// R4(ii): a KNOWN H3 before the first `## ` was consumed and discarded along with its body,
    /// and the parse returned `Ok`. This fixture lost TWO complete sections before D42.
    #[test]
    fn known_section_before_the_first_issue_rejected() {
        let err = parse_bulk_markdown(
            "### ID\nstand-in-1\n### Priority\n0\n## Real Title\n### Type\ntask\n",
        )
        .expect_err("a `### ` before the first `## ` must REJECT");
        assert_eq!(err.code, unblock_error::ErrorCode::ValidationFailed);
        assert_eq!(err.context["kind"], "section_before_issue");
        assert_eq!(err.context["section"], "ID");
        assert_eq!(err.context["line"], 1);
    }

    /// The STRUCTURAL error must win over the unknown-section one: `### Bogus` before the first H2
    /// reports the missing H2 (actionable), not "unrecognized section". Pins the ordering rule.
    #[test]
    fn structural_error_wins_over_unknown_section() {
        let err = parse_bulk_markdown("### Bogus\nx\n## T\n").expect_err("must reject");
        assert_eq!(
            err.context["kind"], "section_before_issue",
            "the `current_issue.is_none()` check MUST precede `Section::from_header`"
        );
    }

    /// The empty `### ` header. Before D42 `Section::from_header("")` yielded `Unknown`, so `### `
    /// was an IGNORED section — the same silent-drop class D42 closes. It is now rejected, with a
    /// kind DISTINCT from the unknown-section one: an unnamed section and an unrecognized section
    /// are different user errors and only the second is fixed by consulting the accepted-name list.
    ///
    /// `arbitrary_utf8_never_panics` covers this exact input but only asserts no-panic, so it stays
    /// green under either behaviour — this test is the ONLY thing in the repo that can tell the two
    /// apart. Deleting it silently evaporates the decision.
    #[test]
    fn empty_section_header_rejected_with_a_distinct_kind() {
        let err = parse_bulk_markdown("## t\n### \n").expect_err("an empty `### ` must REJECT");
        assert_eq!(err.code, unblock_error::ErrorCode::ValidationFailed);
        assert_eq!(err.context["kind"], "empty_section_header");
        assert_ne!(err.context["kind"], "unknown_section");
        assert_eq!(err.context["line"], 2);
    }

    /// An over-long header is TRUNCATED before it enters `message`/`context.section`. The whole
    /// markdown document is ONE string, so a header is otherwise bounded only by `max_string_len`.
    #[test]
    fn echoed_unknown_header_is_truncated() {
        let header = "Z".repeat(4000);
        let err = parse_bulk_markdown(&format!("## t\n### {header}\n")).expect_err("must reject");
        let echoed = err.context["section"].as_str().expect("section");
        assert!(
            echoed.chars().count() <= MAX_ECHOED_HEADER_CHARS + 16,
            "{}",
            echoed.len()
        );
        assert!(echoed.ends_with("…[truncated]"));
        assert!(err.message.len() < 200, "message len {}", err.message.len());
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

    /// STRENGTHENED (D42). As written before, this passed under BOTH the old and the new code —
    /// it only asserted the `ErrorCode`, which both paths share. It was therefore not evidence of
    /// anything. Asserting `context.kind` is what makes it a test: this fixture now trips the
    /// `section_before_issue` rejection, EARLIER than the "no issues found" path it used to reach.
    #[test]
    fn non_empty_without_header_rejects() {
        let err = parse_bulk_markdown("### Description\nNo issue header here.\n")
            .expect_err("must reject");
        assert_eq!(err.code, unblock_error::ErrorCode::ValidationFailed);
        assert_eq!(err.context["kind"], "section_before_issue");
    }

    /// The pre-existing "no issues found" path is still reachable and still rejects — via content
    /// that is neither an H2 nor an H3.
    #[test]
    fn prose_without_any_header_still_reports_no_issues() {
        let err = parse_bulk_markdown("just some prose\n").expect_err("must reject");
        assert_eq!(err.code, unblock_error::ErrorCode::ValidationFailed);
        assert_eq!(err.context["kind"], "no_issues");
    }

    // --- MF-1: fenced code blocks -----------------------------------------------------------

    /// **MF-1 arm (a) — the FALSE POSITIVE.** A section body containing a fenced code block whose
    /// content happens to include a `### ` line was hard-REJECTED by the first D42 cut, with zero
    /// writes, and the emitted hint ("use `#### `") was unactionable: the author does not control
    /// the bytes of a code example. On `main` this document was ACCEPTED. It must be accepted
    /// again, with the fence content INTACT.
    #[test]
    fn a_fenced_unknown_h3_is_content_not_a_section() {
        let content = "## T\n### Design\nexample:\n```\n### Bogus Section\nbody\n```\ntail\n";
        let issues = parse_bulk_markdown(content)
            .expect("an H3 inside a fence is CONTENT — rejecting it is a false positive");
        assert_eq!(issues.len(), 1);
        let design = issues[0].design.as_deref().expect("design");
        assert!(
            design.contains("### Bogus Section"),
            "the fenced H3 must survive verbatim: {design:?}"
        );
        assert!(design.contains("body"), "{design:?}");
        assert!(design.ends_with("tail"), "{design:?}");
    }

    /// **MF-1 arm (b) — the SILENT CORRUPTION.** A *known* section name inside a fence used to tear
    /// the fence in half and relocate the code sample's bytes into another field, with
    /// `isError:false`. Executed against the pre-fix parser this produced `design` ending
    /// `"example:\n```"` and `description == "INSIDE-FENCE\n```"`.
    #[test]
    fn a_fenced_known_h3_does_not_tear_the_fence_apart() {
        let content = "## T\n### Design\nexample:\n```\n### Description\nINSIDE-FENCE\n```\n";
        let issues = parse_bulk_markdown(content).expect("parse");
        assert_eq!(issues.len(), 1);
        assert_eq!(
            issues[0].description, None,
            "the fenced `### Description` must NOT relocate content into `description`"
        );
        let design = issues[0].design.as_deref().expect("design");
        assert_eq!(design, "example:\n```\n### Description\nINSIDE-FENCE\n```");
    }

    /// An H2 inside a fence is content too — otherwise a code sample containing `## ` would silently
    /// split one record into two.
    #[test]
    fn a_fenced_h2_does_not_start_a_new_issue() {
        let issues =
            parse_bulk_markdown("## T\n### Design\n```\n## Not A Title\n```\n").expect("parse");
        assert_eq!(issues.len(), 1, "a fenced `## ` must not split the record");
        assert_eq!(issues[0].title, "T");
    }

    /// Tilde fences, longer runs and info strings are all real `CommonMark` spellings.
    #[test]
    fn tilde_and_long_and_info_string_fences_are_all_tracked() {
        for open in [
            "~~~",
            "~~~~~",
            "```rust",
            "````",
            "```rust,ignore",
            "   ```",
        ] {
            let close = if open.trim_start().starts_with('~') {
                open.trim_start()
            } else {
                "```````"
            };
            let content = format!("## T\n### Design\n{open}\n### Bogus\n{close}\n");
            let issues = parse_bulk_markdown(&content)
                .unwrap_or_else(|e| panic!("fence `{open}` must open a code block: {e:?}"));
            assert!(
                issues[0]
                    .design
                    .as_deref()
                    .is_some_and(|d| d.contains("### Bogus")),
                "fence `{open}` did not protect its body"
            );
        }
    }

    /// A closing run must be at least as long as the opening one and carry no info string — so a
    /// SHORTER run, or one with trailing text, does not close the block ("nesting" in the sense
    /// `CommonMark` gives it).
    #[test]
    fn a_shorter_or_annotated_run_does_not_close_the_fence() {
        let content = "## T\n### Design\n````\n```\n### Bogus\n``` still open\n````\n";
        let issues = parse_bulk_markdown(content).expect("parse");
        let design = issues[0].design.as_deref().expect("design");
        assert!(design.contains("### Bogus"), "{design:?}");
        assert!(design.contains("``` still open"), "{design:?}");
    }

    /// A backtick opening fence's info string may not itself contain a backtick (`CommonMark`), so
    /// such a line is ordinary content and does NOT open a block.
    #[test]
    fn a_backtick_in_a_backtick_info_string_does_not_open_a_fence() {
        // `### Bogus` here is a REAL unknown section: no fence was ever opened.
        let err = parse_bulk_markdown("## T\n### Design\n``` a`b\n### Bogus\n")
            .expect_err("no fence opened, so `### Bogus` is a genuine unknown section");
        assert_eq!(err.context["kind"], "unknown_section");
    }

    /// An UNTERMINATED fence REJECTS. The alternative (`CommonMark`'s "runs to end of document") would
    /// silently swallow every later `## `/`### ` into one section — exactly the silent-drop class
    /// D42 exists to close. The rejection names the OPENING line, which is the actionable one.
    #[test]
    fn an_unterminated_fence_is_rejected_naming_the_opening_line() {
        let err = parse_bulk_markdown("## T\n### Design\n```\ncode\n## Another\n### Type\ntask\n")
            .expect_err("an unterminated fence must REJECT, never swallow the rest silently");
        assert_eq!(err.code, unblock_error::ErrorCode::ValidationFailed);
        assert_eq!(err.context["kind"], "unterminated_code_fence");
        assert_eq!(err.context["field"], "markdown");
        assert_eq!(err.context["line"], 3, "the OPENING line, not EOF");
        assert!(err.hint.is_some_and(|h| h.contains("```")));
    }

    /// An indented code block needs no tracking: `strip_prefix("### ")` already fails on an indented
    /// line, so its `### ` is content today and stays content. Pinned so a future `trim_start()` at
    /// the header check cannot re-open the hole without turning this RED.
    #[test]
    fn an_indented_code_block_h3_is_already_content() {
        for indent in ["    ", "\t", "      "] {
            let content = format!("## T\n### Design\ntext\n\n{indent}### Bogus\n{indent}body\n");
            let issues = parse_bulk_markdown(&content)
                .unwrap_or_else(|e| panic!("indent {indent:?} must stay content: {e:?}"));
            assert!(
                issues[0]
                    .design
                    .as_deref()
                    .is_some_and(|d| d.contains("### Bogus")),
                "indent {indent:?} lost the indented-code H3"
            );
        }
    }

    /// A 4+-space-INDENTED delimiter is an indented-code-block line, not a fence opener. Without the
    /// indent guard this document would open a fence that never closes and be REJECTED.
    /// (Mutation M1: `fence_delimiter`'s `> 3` guard.)
    #[test]
    fn an_indented_delimiter_does_not_open_a_fence() {
        let issues = parse_bulk_markdown("## T\n### Design\n    ```\nplain text\n")
            .expect("an indented ``` is code-block CONTENT, not a fence opener");
        let design = issues[0].design.as_deref().expect("design");
        assert!(design.contains("```"), "{design:?}");
        assert!(design.ends_with("plain text"), "{design:?}");
    }

    /// A run SHORTER than 3 is not a fence delimiter at all — so a lone or doubled backtick line
    /// leaves a following `### ` a REAL unknown section. (Mutation M2: the `run_len < 3` guard.)
    #[test]
    fn a_run_shorter_than_three_is_not_a_fence() {
        for short in ["`", "``", "~", "~~"] {
            let err = parse_bulk_markdown(&format!("## T\n### Design\n{short}\n### Bogus\nx\n"))
                .expect_err("`### Bogus` is a real header: no fence was opened");
            assert_eq!(
                err.context["kind"], "unknown_section",
                "`{short}` must not open a fence (an opened one would report \
                 `unterminated_code_fence` instead)"
            );
        }
    }

    /// A TILDE fence is not closed by backticks, and vice versa. Without the marker check a
    /// backtick delimiter inside a `~~~` block would close it and expose the following `### ` as a
    /// header.
    /// (Mutation M5: `closes_fence`'s `marker == open.marker`.)
    #[test]
    fn a_fence_is_only_closed_by_its_own_marker() {
        let issues = parse_bulk_markdown("## T\n### Design\n~~~\n```\n### Bogus\n```\n~~~\n")
            .expect("a ``` line must not close a ~~~ fence");
        let design = issues[0].design.as_deref().expect("design");
        assert!(design.contains("### Bogus"), "{design:?}");
        assert!(design.ends_with("~~~"), "{design:?}");
    }

    /// NON-VACUITY for every fence cell above: an unknown `### ` OUTSIDE any fence still rejects.
    /// Without this the fence work could silently degrade into "never reject anything".
    #[test]
    fn fence_tracking_does_not_disable_the_unknown_section_rejection() {
        let err = parse_bulk_markdown("## T\n### Design\n```\ncode\n```\n### Bogus\nx\n")
            .expect_err("a CLOSED fence must not suppress a later real unknown section");
        assert_eq!(err.context["kind"], "unknown_section");
        assert_eq!(err.context["line"], 6);
    }

    #[test]
    fn arbitrary_utf8_never_panics() {
        // A property-ish smoke: pathological inputs must not panic.
        for content in ["", "##", "###", "## \n###\n", "## t\n### \n", "\u{0}## x"] {
            let _ = parse_bulk_markdown(content);
        }
    }
}
