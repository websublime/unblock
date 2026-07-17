//! Plain-text backend (human, no color — D7).
//!
//! Line-oriented issue/list/tree/count/cycle/error/diagnostics views. ASCII labels only: the
//! original's unicode status icons + ANSI color depended on `crossterm`, which D7 removes, so the
//! status is rendered via its `as_str()` label. Every user-controlled string — `Issue` fields, the
//! open-enum `Status`/`IssueType`/`DependencyType` labels (incl. `Custom`), embedded ids, and every
//! `StructuredError.context` value — is routed through [`sanitize_inline`]; multi-line
//! description/notes use [`sanitize_text`] to keep line structure. Timestamps use [`fmt_ts`].

use unblock_error::StructuredError;
use unblock_model::{
    Comment, CountBucket, DepTree, DiagnosticReport, Issue, OutputFormat, Priority,
};

use crate::error::RenderError;
use crate::format::fmt_ts;
use crate::options::{ContentType, RenderOptions, RenderOutput};
use crate::renderer::Renderer;
use crate::sanitize::{sanitize_inline, sanitize_text};

/// The plain-text renderer.
pub(crate) struct PlainRenderer {
    opts: RenderOptions,
}

impl PlainRenderer {
    pub(crate) fn new(opts: RenderOptions) -> Self {
        Self { opts }
    }

    /// Render a single-line issue summary: `{id} [P{n}] [{type}] - {title}`.
    ///
    /// Honours [`RenderOptions::max_width`] by truncating the title (char-boundary safe). The
    /// status label is shown in the long view; the single line keeps the original's id/priority/
    /// type/title shape, ASCII-only (no icons/color).
    fn issue_line(&self, issue: &Issue) -> String {
        let id = sanitize_inline(&issue.id);
        let priority = format_priority(issue.priority);
        let issue_type = sanitize_inline(issue.issue_type.as_str());
        let prefix = format!("{id} [{priority}] [{issue_type}] - ");

        let title = if let Some(width) = self.opts.max_width {
            truncate_title(&issue.title, width.saturating_sub(prefix.chars().count()))
        } else {
            sanitize_inline(&issue.title).into_owned()
        };
        format!("{prefix}{title}")
    }

    /// Render the multi-line detail view for one issue.
    ///
    /// The long view is full-detail and does not truncate, so it does not consult `self.opts`.
    fn issue_long(issue: &Issue) -> String {
        let mut lines = Vec::new();
        lines.push(format!("ID: {}", sanitize_inline(&issue.id)));
        lines.push(format!("Title: {}", sanitize_inline(&issue.title)));
        lines.push(format!(
            "Status: {}",
            sanitize_inline(issue.status.as_str())
        ));
        lines.push(format!("Priority: {}", format_priority(issue.priority)));
        lines.push(format!(
            "Type: {}",
            sanitize_inline(issue.issue_type.as_str())
        ));
        if let Some(assignee) = issue.assignee.as_deref() {
            lines.push(format!("Assignee: {}", sanitize_inline(assignee)));
        }
        if let Some(owner) = issue.owner.as_deref() {
            lines.push(format!("Owner: {}", sanitize_inline(owner)));
        }
        if !issue.labels.is_empty() {
            let labels = issue
                .labels
                .iter()
                .map(|l| sanitize_inline(l).into_owned())
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("Labels: {labels}"));
        }
        lines.push(format!("Created: {}", fmt_ts(issue.created_at)));
        lines.push(format!("Updated: {}", fmt_ts(issue.updated_at)));
        if let Some(due) = issue.due_at {
            lines.push(format!("Due: {}", fmt_ts(due)));
        }
        if let Some(description) = issue.description.as_deref() {
            // Multi-line content: preserve `\n`/`\t`, escape the rest.
            lines.push(format!("Description:\n{}", sanitize_text(description)));
        }
        // Comments (FR-6/D37). A redacted comment renders the masked marker in place of its body.
        if !issue.comments.is_empty() {
            lines.push("Comments:".to_string());
            for comment in &issue.comments {
                lines.push(format!(
                    "  [{}] {} at {}",
                    comment.id,
                    sanitize_inline(&comment.author),
                    fmt_ts(comment.created_at),
                ));
                lines.push(format!("  {}", comment_body(comment)));
            }
        }
        lines.join("\n")
    }
}

/// Render a comment body, or the masked marker when the comment is redacted (D37/D-E).
///
/// The redact marker form `[redacted <ts>]` is **NORMATIVE and shared verbatim with the markdown
/// backend** (`backend/markdown.rs`) — it is renderer-generated CHROME, not user content, so it is
/// emitted as-is and never routed through `sanitize_text` (the masked `text` is `""`: there is
/// nothing to sanitize). The presence of `redacted_at` is the flag (spine §1.7).
pub(crate) fn comment_body(comment: &Comment) -> String {
    match comment.redacted_at {
        Some(redacted_at) => format!("[redacted {}]", fmt_ts(redacted_at)),
        None => sanitize_text(&comment.body).into_owned(),
    }
}

/// Format a priority as the ASCII `P{n}` label (matches the original `format_priority`).
fn format_priority(priority: Priority) -> String {
    format!("P{}", priority.0)
}

/// Truncate `title` to `max_len` characters (char-boundary safe, ASCII ellipsis), after
/// sanitizing. A faithful behavioural port of the original `truncate_title` without the
/// `unicode-width` dependency (D7 removed the rich stack); width is counted in chars.
fn truncate_title(title: &str, max_len: usize) -> String {
    let sanitized = sanitize_inline(title);
    let sanitized = sanitized.as_ref();

    if max_len == 0 {
        return String::new();
    }
    let char_count = sanitized.chars().count();
    if char_count <= max_len {
        return sanitized.to_string();
    }
    if max_len <= 3 {
        return sanitized.chars().take(max_len).collect();
    }
    let target = max_len - 3;
    let mut s: String = sanitized.chars().take(target).collect();
    s.push_str("...");
    s
}

impl Renderer for PlainRenderer {
    fn format(&self) -> OutputFormat {
        OutputFormat::Plain
    }

    fn issue(&self, value: &Issue, _opts: &RenderOptions) -> Result<RenderOutput, RenderError> {
        Ok(RenderOutput::new(
            Self::issue_long(value),
            ContentType::Text,
        ))
    }

    fn issues(&self, value: &[Issue], _opts: &RenderOptions) -> Result<RenderOutput, RenderError> {
        let body = if value.is_empty() {
            "(no issues)".to_string()
        } else {
            value
                .iter()
                .map(|issue| self.issue_line(issue))
                .collect::<Vec<_>>()
                .join("\n")
        };
        Ok(RenderOutput::new(body, ContentType::Text))
    }

    fn counts(
        &self,
        value: &[CountBucket],
        _opts: &RenderOptions,
    ) -> Result<RenderOutput, RenderError> {
        let body = if value.is_empty() {
            "(no counts)".to_string()
        } else {
            value
                .iter()
                .map(|bucket| format!("{}: {}", sanitize_inline(&bucket.key), bucket.count))
                .collect::<Vec<_>>()
                .join("\n")
        };
        Ok(RenderOutput::new(body, ContentType::Text))
    }

    fn dep_tree(
        &self,
        value: &DepTree,
        _opts: &RenderOptions,
    ) -> Result<RenderOutput, RenderError> {
        let mut lines = vec![format!("{}", sanitize_inline(&value.root))];
        // Preserve caller edge order (MF-5): never re-sort.
        for edge in &value.edges {
            lines.push(format!(
                "  {} -> {} ({})",
                sanitize_inline(&edge.from),
                sanitize_inline(&edge.to),
                sanitize_inline(edge.dep_type.as_str()),
            ));
        }
        Ok(RenderOutput::new(lines.join("\n"), ContentType::Text))
    }

    fn cycles(
        &self,
        value: &[Vec<String>],
        _opts: &RenderOptions,
    ) -> Result<RenderOutput, RenderError> {
        let body = if value.is_empty() {
            "(no cycles)".to_string()
        } else {
            value
                .iter()
                .map(|cycle| {
                    // Preserve caller path order (MF-5).
                    cycle
                        .iter()
                        .map(|id| sanitize_inline(id).into_owned())
                        .collect::<Vec<_>>()
                        .join(" -> ")
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        Ok(RenderOutput::new(body, ContentType::Text))
    }

    fn structured_error(
        &self,
        value: &StructuredError,
        _opts: &RenderOptions,
    ) -> Result<RenderOutput, RenderError> {
        let mut lines = Vec::new();
        // `code`/`message`/`hint` are pre-sanitized at L0 (spine §2.4 chokepoint) — pass through.
        lines.push(format!(
            "Error [{}]: {}",
            value.code.as_str(),
            value.message
        ));
        if let Some(hint) = &value.hint {
            lines.push(format!("Hint: {hint}"));
        }
        if !value.context.is_empty() {
            lines.push("Context:".to_string());
            // `context` keys/values are NOT pre-sanitized (JSON-safe only) — sanitize each here.
            for (key, val) in &value.context {
                let rendered = context_value_string(val);
                lines.push(format!(
                    "  {}: {}",
                    sanitize_inline(key),
                    sanitize_inline(&rendered),
                ));
            }
        }
        Ok(RenderOutput::new(lines.join("\n"), ContentType::Text))
    }

    fn diagnostics(
        &self,
        value: &DiagnosticReport,
        _opts: &RenderOptions,
    ) -> Result<RenderOutput, RenderError> {
        let mut lines = vec![format!("Diagnostics: {:?}", value.kind)];
        for finding in &value.findings {
            lines.push(format!(
                "  {}: {}",
                sanitize_inline(&finding.label),
                sanitize_inline(&finding.detail),
            ));
        }
        Ok(RenderOutput::new(lines.join("\n"), ContentType::Text))
    }
}

/// Render a JSON context value to a flat single-line string for text/markdown display.
///
/// Strings are taken verbatim (the caller sanitizes); other JSON values use their compact JSON
/// form. The result is always passed through a sanitizer by the caller before embedding.
pub(crate) fn context_value_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::PlainRenderer;
    use crate::options::RenderOptions;
    use crate::renderer::Renderer;
    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use unblock_error::{ErrorCode, StructuredError};
    use unblock_model::{Comment, Issue, Status};

    fn fixture() -> Issue {
        Issue {
            id: "ub-abc123".to_string(),
            title: "Hello world".to_string(),
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            ..Issue::default()
        }
    }

    #[test]
    fn issue_line_truncates_to_max_width() {
        let opts = RenderOptions::default().with_max_width(Some(40));
        let r = PlainRenderer::new(opts.clone());
        let out = r.issues(&[fixture()], &opts).unwrap();
        assert!(out.stdout.chars().count() <= 40, "{}", out.stdout);
    }

    #[test]
    fn empty_list_note() {
        let r = PlainRenderer::new(RenderOptions::default());
        let out = r.issues(&[], &RenderOptions::default()).unwrap();
        assert_eq!(out.stdout, "(no issues)");
    }

    #[test]
    fn title_esc_is_escaped() {
        let mut issue = fixture();
        issue.title = "danger\x1b[2Jwipe".to_string();
        let r = PlainRenderer::new(RenderOptions::default());
        let out = r.issue(&issue, &RenderOptions::default()).unwrap();
        assert!(out.stdout.contains("\\u{1b}"));
        assert!(!out.stdout.contains('\x1b'));
    }

    #[test]
    fn custom_status_esc_is_escaped() {
        let mut issue = fixture();
        issue.status = Status::Custom("state\x1b[2J".to_string());
        let r = PlainRenderer::new(RenderOptions::default());
        let out = r.issue(&issue, &RenderOptions::default()).unwrap();
        assert!(out.stdout.contains("\\u{1b}"));
        assert!(!out.stdout.contains('\x1b'));
    }

    #[test]
    fn context_value_esc_is_escaped() {
        let err = StructuredError::from_code(ErrorCode::ValidationFailed, "bad")
            .with_context("provided", json!("state\x1b[2J"));
        let r = PlainRenderer::new(RenderOptions::default());
        let out = r.structured_error(&err, &RenderOptions::default()).unwrap();
        assert!(out.stdout.contains("\\u{1b}"));
        assert!(!out.stdout.contains('\x1b'));
    }

    // --- comments (FR-6/D37) ---

    fn comment(body: &str) -> Comment {
        Comment {
            id: 1,
            issue_id: "ub-abc123".to_string(),
            author: "alice".to_string(),
            body: body.to_string(),
            created_at: Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 7).unwrap(),
            updated_at: None,
            redacted_at: None,
        }
    }

    fn issue_with(comments: Vec<Comment>) -> Issue {
        Issue {
            id: "ub-abc123".to_string(),
            title: "t".to_string(),
            created_at: Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 6).unwrap(),
            comments,
            ..Issue::default()
        }
    }

    /// NFR-18: a comment body carrying ESC/BEL is ESCAPED, never emitted raw into a terminal.
    #[test]
    fn comment_body_esc_is_escaped() {
        let issue = issue_with(vec![comment("evil\x1b[2Jbody\x07")]);
        let r = PlainRenderer::new(RenderOptions::default());
        let out = r.issue(&issue, &RenderOptions::default()).unwrap();
        assert!(
            out.stdout.contains("\\u{1b}"),
            "ESC must be escaped: {}",
            out.stdout
        );
        assert!(!out.stdout.contains('\x1b'), "no raw ESC may reach stdout");
        assert!(!out.stdout.contains('\x07'), "no raw BEL may reach stdout");
    }

    /// NFR-18: the comment AUTHOR is sanitized too (it is user-controlled on the import path).
    #[test]
    fn comment_author_esc_is_escaped() {
        let mut c = comment("body");
        c.author = "ali\x1b[2Jce".to_string();
        let issue = issue_with(vec![c]);
        let r = PlainRenderer::new(RenderOptions::default());
        let out = r.issue(&issue, &RenderOptions::default()).unwrap();
        assert!(!out.stdout.contains('\x1b'));
    }

    /// A multi-line comment body keeps its line structure (`sanitize_text`, not `sanitize_inline`).
    #[test]
    fn comment_body_is_multiline_sanitized() {
        let issue = issue_with(vec![comment("line one\nline two")]);
        let r = PlainRenderer::new(RenderOptions::default());
        let out = r.issue(&issue, &RenderOptions::default()).unwrap();
        assert!(out.stdout.contains("line one\nline two"), "{}", out.stdout);
    }

    /// D37/D-E: a redacted comment renders the masked marker, never its (masked) body.
    #[test]
    fn redacted_comment_renders_the_masked_marker() {
        let mut c = comment("");
        c.redacted_at = Some(Utc.with_ymd_and_hms(2026, 1, 3, 0, 0, 0).unwrap());
        let issue = issue_with(vec![c]);
        let r = PlainRenderer::new(RenderOptions::default());
        let out = r.issue(&issue, &RenderOptions::default()).unwrap();
        assert!(
            out.stdout.contains("[redacted 2026-01-03T00:00:00Z]"),
            "{}",
            out.stdout
        );
    }

    /// An issue with no comments emits NO Comments section (no empty header).
    #[test]
    fn no_comments_means_no_section() {
        let r = PlainRenderer::new(RenderOptions::default());
        let out = r
            .issue(&issue_with(Vec::new()), &RenderOptions::default())
            .unwrap();
        assert!(!out.stdout.contains("Comments:"));
    }
}
