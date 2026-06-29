//! Markdown backend — **new views built here + the ported `escape_markdown`**.
//!
//! The original `format/markdown.rs` is only `render_markdown`/`strip_markdown`/`escape_markdown`
//! for description content; it has **no** issue-detail / GFM-table / dep-tree / error / diagnostics
//! renderers to copy, so those views are authored here (drop syntax highlighting, D7). User strings
//! are sanitized (then markdown-escaped) before embedding: `Issue` fields, the open-enum labels
//! (incl. `Custom`), embedded ids, and **every `StructuredError.context` value** (spine §2.4:503).
//! Timestamps use [`fmt_ts`].

use unblock_error::StructuredError;
use unblock_model::{CountBucket, DepTree, DiagnosticReport, Issue, OutputFormat, Priority};

use crate::backend::plain::context_value_string;
use crate::error::RenderError;
use crate::format::fmt_ts;
use crate::options::{ContentType, RenderOptions, RenderOutput};
use crate::renderer::Renderer;
use crate::sanitize::sanitize_inline;

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
    use unblock_model::Issue;

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
}
