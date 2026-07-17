//! Markdown backend — **new views built here + the ported `escape_markdown`**.
//!
//! The original `format/markdown.rs` is only `render_markdown`/`strip_markdown`/`escape_markdown`
//! for description content; it has **no** issue-detail / GFM-table / dep-tree / error / diagnostics
//! renderers to copy, so those views are authored here (drop syntax highlighting, D7). User strings
//! are sanitized (then markdown-escaped) before embedding: `Issue` fields, the open-enum labels
//! (incl. `Custom`), embedded ids, and **every `StructuredError.context` value** (spine §2.4:503).
//! Timestamps use [`fmt_ts`].

use unblock_error::StructuredError;
use unblock_model::{
    Comment, CountBucket, DepTree, DiagnosticReport, Issue, OutputFormat, Priority,
};

use crate::backend::plain::context_value_string;
use crate::error::RenderError;
use crate::format::fmt_ts;
use crate::options::{ContentType, RenderOptions, RenderOutput};
use crate::renderer::Renderer;
use crate::sanitize::{sanitize_inline, sanitize_text};

/// Escape markdown special characters — ported with the exact original char set
/// (`temp/beads_rust-main/src/format/markdown.rs:365-380`).
///
/// Backslash-escapes `\` `` ` `` `* _ { } [ ] ( ) # + - . ! | ~ >`; all other characters pass
/// through.
#[must_use]
pub(crate) fn escape_markdown(content: &str) -> String {
    let mut result = String::with_capacity(content.len() * 2);
    for c in content.chars() {
        match c {
            '\\' | '`' | '*' | '_' | '{' | '}' | '[' | ']' | '(' | ')' | '#' | '+' | '-' | '.'
            | '!' | '|' | '~' | '>' => {
                result.push('\\');
                result.push(c);
            }
            _ => result.push(c),
        }
    }
    result
}

/// Sanitize (terminal-control) then markdown-escape an untrusted string for safe embedding.
fn safe_md(value: &str) -> String {
    escape_markdown(sanitize_inline(value).as_ref())
}

/// The MULTI-LINE sibling of [`safe_md`] (D37): sanitize with [`sanitize_text`] — which preserves
/// `\n`/`\t` — then markdown-escape.
///
/// `safe_md` cannot be used for a comment body: it sanitizes with `sanitize_inline`, which escapes
/// `\n` and would flatten a multi-line comment into a literal `\n` soup.
fn safe_md_multiline(value: &str) -> String {
    escape_markdown(sanitize_text(value).as_ref())
}

/// Render a comment body, or the masked marker when the comment is redacted (D37/D-E).
///
/// The `[redacted <ts>]` marker is the SAME literal form the plain backend emits (normative, one
/// form across both backends). It is renderer-generated CHROME, not user content, so it is emitted
/// VERBATIM — never through `escape_markdown`, whose bracket-escaping would mangle it.
fn comment_body(comment: &Comment) -> String {
    match comment.redacted_at {
        Some(redacted_at) => format!("[redacted {}]", fmt_ts(redacted_at)),
        None => safe_md_multiline(&comment.body),
    }
}

/// The markdown renderer.
///
/// `opts` is retained for forward-compatibility (markdown gains width/option-driven views in
/// v1.1); the v1 markdown views are option-independent, so the field is currently unread.
pub(crate) struct MarkdownRenderer {
    #[allow(dead_code)]
    opts: RenderOptions,
}

impl MarkdownRenderer {
    pub(crate) fn new(opts: RenderOptions) -> Self {
        Self { opts }
    }
}

/// Render the markdown detail section for one issue (option-independent in v1).
fn issue_detail(issue: &Issue) -> String {
    let mut lines = vec![
        format!("## {}", safe_md(&issue.id)),
        String::new(),
        format!("- **Title:** {}", safe_md(&issue.title)),
        format!("- **Status:** {}", safe_md(issue.status.as_str())),
        format!("- **Priority:** {}", format_priority(issue.priority)),
        format!("- **Type:** {}", safe_md(issue.issue_type.as_str())),
    ];
    if let Some(assignee) = issue.assignee.as_deref() {
        lines.push(format!("- **Assignee:** {}", safe_md(assignee)));
    }
    lines.push(format!("- **Created:** {}", fmt_ts(issue.created_at)));
    lines.push(format!("- **Updated:** {}", fmt_ts(issue.updated_at)));
    if let Some(description) = issue.description.as_deref() {
        lines.push(String::new());
        lines.push(safe_md(description));
    }
    // Comments (FR-6/D37).
    if !issue.comments.is_empty() {
        lines.push(String::new());
        lines.push("### Comments".to_string());
        for comment in &issue.comments {
            lines.push(String::new());
            lines.push(format!(
                "- **{}** at {}",
                safe_md(&comment.author),
                fmt_ts(comment.created_at),
            ));
            lines.push(String::new());
            lines.push(comment_body(comment));
        }
    }
    lines.join("\n")
}

/// `P{n}` priority label (digits only — no markdown-special chars to escape).
fn format_priority(priority: Priority) -> String {
    format!("P{}", priority.0)
}

impl Renderer for MarkdownRenderer {
    fn format(&self) -> OutputFormat {
        OutputFormat::Markdown
    }

    fn issue(&self, value: &Issue, _opts: &RenderOptions) -> Result<RenderOutput, RenderError> {
        Ok(RenderOutput::new(
            issue_detail(value),
            ContentType::Markdown,
        ))
    }

    fn issues(&self, value: &[Issue], _opts: &RenderOptions) -> Result<RenderOutput, RenderError> {
        if value.is_empty() {
            return Ok(RenderOutput::new(
                "_no issues_".to_string(),
                ContentType::Markdown,
            ));
        }
        let mut lines = vec![
            "| id | title | status | priority | type |".to_string(),
            "| --- | --- | --- | --- | --- |".to_string(),
        ];
        for issue in value {
            lines.push(format!(
                "| {} | {} | {} | {} | {} |",
                safe_md(&issue.id),
                safe_md(&issue.title),
                safe_md(issue.status.as_str()),
                format_priority(issue.priority),
                safe_md(issue.issue_type.as_str()),
            ));
        }
        Ok(RenderOutput::new(lines.join("\n"), ContentType::Markdown))
    }

    fn counts(
        &self,
        value: &[CountBucket],
        _opts: &RenderOptions,
    ) -> Result<RenderOutput, RenderError> {
        if value.is_empty() {
            return Ok(RenderOutput::new(
                "_no counts_".to_string(),
                ContentType::Markdown,
            ));
        }
        let mut lines = vec!["| key | count |".to_string(), "| --- | --- |".to_string()];
        for bucket in value {
            lines.push(format!("| {} | {} |", safe_md(&bucket.key), bucket.count));
        }
        Ok(RenderOutput::new(lines.join("\n"), ContentType::Markdown))
    }

    fn dep_tree(
        &self,
        value: &DepTree,
        _opts: &RenderOptions,
    ) -> Result<RenderOutput, RenderError> {
        let mut lines = vec![format!("- {}", safe_md(&value.root))];
        // Preserve caller edge order (MF-5).
        for edge in &value.edges {
            lines.push(format!(
                "  - {} → {} ({})",
                safe_md(&edge.from),
                safe_md(&edge.to),
                safe_md(edge.dep_type.as_str()),
            ));
        }
        Ok(RenderOutput::new(lines.join("\n"), ContentType::Markdown))
    }

    fn cycles(
        &self,
        value: &[Vec<String>],
        _opts: &RenderOptions,
    ) -> Result<RenderOutput, RenderError> {
        if value.is_empty() {
            return Ok(RenderOutput::new(
                "_no cycles_".to_string(),
                ContentType::Markdown,
            ));
        }
        let lines = value
            .iter()
            .map(|cycle| {
                // Preserve caller path order (MF-5).
                let path = cycle
                    .iter()
                    .map(|id| safe_md(id))
                    .collect::<Vec<_>>()
                    .join(" → ");
                format!("- {path}")
            })
            .collect::<Vec<_>>();
        Ok(RenderOutput::new(lines.join("\n"), ContentType::Markdown))
    }

    fn structured_error(
        &self,
        value: &StructuredError,
        _opts: &RenderOptions,
    ) -> Result<RenderOutput, RenderError> {
        let mut lines = Vec::new();
        // `message`/`hint` are pre-sanitized at L0, but still markdown-escape for safe embedding.
        lines.push(format!(
            "**Error** `{}`: {}",
            escape_markdown(value.code.as_str()),
            escape_markdown(&value.message),
        ));
        if let Some(hint) = &value.hint {
            lines.push(format!("- _Hint:_ {}", escape_markdown(hint)));
        }
        if !value.context.is_empty() {
            lines.push(String::new());
            lines.push("| key | value |".to_string());
            lines.push("| --- | --- |".to_string());
            // context keys/values are NOT pre-sanitized — sanitize + escape each here.
            for (key, val) in &value.context {
                let rendered = context_value_string(val);
                lines.push(format!("| {} | {} |", safe_md(key), safe_md(&rendered)));
            }
        }
        Ok(RenderOutput::new(lines.join("\n"), ContentType::Markdown))
    }

    fn diagnostics(
        &self,
        value: &DiagnosticReport,
        _opts: &RenderOptions,
    ) -> Result<RenderOutput, RenderError> {
        let mut lines = vec![
            format!("## Diagnostics: {:?}", value.kind),
            String::new(),
            "| label | detail |".to_string(),
            "| --- | --- |".to_string(),
        ];
        for finding in &value.findings {
            lines.push(format!(
                "| {} | {} |",
                safe_md(&finding.label),
                safe_md(&finding.detail),
            ));
        }
        Ok(RenderOutput::new(lines.join("\n"), ContentType::Markdown))
    }
}

#[cfg(test)]
mod tests {
    use super::{MarkdownRenderer, escape_markdown};
    use crate::options::RenderOptions;
    use crate::renderer::Renderer;
    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use unblock_error::{ErrorCode, StructuredError};
    use unblock_model::{Comment, Issue};

    fn fixture() -> Issue {
        Issue {
            id: "ub-abc123".to_string(),
            title: "Fix | bug `now`".to_string(),
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            ..Issue::default()
        }
    }

    #[test]
    fn escape_markdown_exact_char_set() {
        assert_eq!(escape_markdown("a|b`c"), "a\\|b\\`c");
        assert_eq!(escape_markdown("[x](y)"), "\\[x\\]\\(y\\)");
        assert_eq!(
            escape_markdown("# h + 1 - 2 . !"),
            "\\# h \\+ 1 \\- 2 \\. \\!"
        );
        assert_eq!(escape_markdown("plain text"), "plain text");
    }

    #[test]
    fn pipe_and_backtick_escaped_in_table() {
        let r = MarkdownRenderer::new(RenderOptions::default());
        let out = r.issues(&[fixture()], &RenderOptions::default()).unwrap();
        assert!(out.stdout.contains("\\|"));
        assert!(out.stdout.contains("\\`"));
    }

    #[test]
    fn empty_list_note() {
        let r = MarkdownRenderer::new(RenderOptions::default());
        let out = r.issues(&[], &RenderOptions::default()).unwrap();
        assert_eq!(out.stdout, "_no issues_");
    }

    #[test]
    fn context_value_esc_is_escaped() {
        let err = StructuredError::from_code(ErrorCode::ValidationFailed, "bad")
            .with_context("provided", json!("state\x1b[2J"));
        let r = MarkdownRenderer::new(RenderOptions::default());
        let out = r.structured_error(&err, &RenderOptions::default()).unwrap();
        // The raw ESC byte must be gone (the security property); sanitize_inline turns it into the
        // visible `\u{1b}` escape, then markdown-escaping doubles its backslash, so the cell shows
        // the `u{1b}` token rather than a literal control byte.
        assert!(!out.stdout.contains('\x1b'));
        assert!(out.stdout.contains("u\\{1b\\}"), "{}", out.stdout);
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

    fn fixture_with(comments: Vec<Comment>) -> Issue {
        let mut issue = fixture();
        issue.comments = comments;
        issue
    }

    /// NFR-18: a comment body carrying ESC is escaped, never emitted raw.
    #[test]
    fn comment_body_esc_is_escaped() {
        let r = MarkdownRenderer::new(RenderOptions::default());
        let out = r
            .issue(
                &fixture_with(vec![comment("evil\x1b[2Jbody\x07")]),
                &RenderOptions::default(),
            )
            .unwrap();
        assert!(!out.stdout.contains('\x1b'), "no raw ESC may reach stdout");
        assert!(!out.stdout.contains('\x07'), "no raw BEL may reach stdout");
    }

    /// `safe_md_multiline` PRESERVES the newlines `safe_md`/`sanitize_inline` would escape, while
    /// still markdown-escaping the body. This test fails if the body is routed through `safe_md`.
    #[test]
    fn comment_body_keeps_newlines_and_is_markdown_escaped() {
        let r = MarkdownRenderer::new(RenderOptions::default());
        let out = r
            .issue(
                &fixture_with(vec![comment("pipe | one\nback `tick`")]),
                &RenderOptions::default(),
            )
            .unwrap();
        assert!(
            out.stdout.contains("pipe \\| one\nback \\`tick\\`"),
            "the body must keep its newline AND be markdown-escaped: {}",
            out.stdout
        );
    }

    /// D37/D-E: a redacted comment renders the marker VERBATIM — it is renderer chrome, so its
    /// brackets must NOT be markdown-escaped (the plain backend emits the identical literal).
    #[test]
    fn redacted_comment_renders_the_unescaped_masked_marker() {
        let mut c = comment("");
        c.redacted_at = Some(Utc.with_ymd_and_hms(2026, 1, 3, 0, 0, 0).unwrap());
        let r = MarkdownRenderer::new(RenderOptions::default());
        let out = r
            .issue(&fixture_with(vec![c]), &RenderOptions::default())
            .unwrap();
        assert!(
            out.stdout.contains("[redacted 2026-01-03T00:00:00Z]"),
            "{}",
            out.stdout
        );
        assert!(
            !out.stdout.contains("\\[redacted"),
            "the marker is chrome — it must not be markdown-escaped: {}",
            out.stdout
        );
    }

    #[test]
    fn no_comments_means_no_section() {
        let r = MarkdownRenderer::new(RenderOptions::default());
        let out = r
            .issue(&fixture_with(Vec::new()), &RenderOptions::default())
            .unwrap();
        assert!(!out.stdout.contains("### Comments"));
    }
}
