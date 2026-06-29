//! CSV backend (RFC-4180) — hand-rolled, byte-faithful port of the original `format/csv.rs`.
//!
//! There is **no** `csv` runtime dependency (decision #2): `escape_field` is ported verbatim
//! (manual double-quote wrapping + quote-doubling + the formula-injection guard) and rows are
//! written via `writeln!` into an in-memory `String` (infallible — no I/O, no `IoError` path). Only
//! `issue`/`issues` produce CSV; every other kind returns [`RenderError::UnsupportedFormat`].
//!
//! Untrusted columns (`id`, `title`, `description`, `notes`, `assignee`, `owner`, `external_ref`,
//! and the `status`/`issue_type` open-enum labels incl. their `Custom` arms) are routed through
//! [`crate::sanitize::sanitize_inline`] **before** `escape_field` (sanitize → escape; NFR-18 /
//! MF-2). Machine-generated columns (timestamps via [`crate::fmt_ts`], the bare-int `priority`) are
//! control-free by construction and are not sanitized.

use std::fmt::Write as _;

use unblock_model::Issue;

use crate::error::RenderError;
use crate::format::fmt_ts;
use crate::options::{ContentType, RenderOptions, RenderOutput};
use crate::renderer::Renderer;
use crate::sanitize::sanitize_inline;

/// Default CSV fields (8, exact original order — `temp/beads_rust-main/src/format/csv.rs:10`).
pub(crate) const DEFAULT_FIELDS: &[&str] = &[
    "id",
    "title",
    "status",
    "priority",
    "issue_type",
    "assignee",
    "created_at",
    "updated_at",
];

/// All curated CSV fields (15, exact original order — `csv.rs:22`; decision #3, faithful port).
///
/// CSV is a clean human/spreadsheet view; `json`/`robot` carry the full `Issue` field set. This is
/// an export-view curation, not a simplification of the data model.
pub(crate) const ALL_FIELDS: &[&str] = &[
    "id",
    "title",
    "description",
    "status",
    "priority",
    "issue_type",
    "assignee",
    "owner",
    "created_at",
    "updated_at",
    "closed_at",
    "due_at",
    "defer_until",
    "notes",
    "external_ref",
];

/// Escape a CSV field value — **verbatim** port of the original (`csv.rs:46-64`).
///
/// Wraps in double quotes when the value contains a comma, quote, `\n`, or `\r`, doubling any
/// existing quote. Mitigates spreadsheet formula injection: a value starting with `= + - @ \t \r
/// \n` is prefixed with a single quote and wrapped (`"'<escaped>"`).
#[must_use]
pub(crate) fn escape_field(value: &str) -> String {
    // Mitigate CSV formula injection: prefix dangerous characters with a single-quote so
    // spreadsheets treat the cell as a literal string.
    if value.starts_with(['=', '+', '-', '@', '\t', '\r', '\n']) {
        let escaped = value.replace('"', "\"\"");
        return format!("\"'{escaped}\"");
    }

    let needs_quoting =
        value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r');

    if needs_quoting {
        let escaped = value.replace('"', "\"\"");
        format!("\"{escaped}\"")
    } else {
        value.to_string()
    }
}

/// Whether `field` is one of the 15 curated [`ALL_FIELDS`].
fn is_known_field(field: &str) -> bool {
    ALL_FIELDS.contains(&field)
}

/// Get a CSV cell value for `issue`/`field`, applying [`sanitize_inline`] to untrusted columns and
/// [`fmt_ts`] to timestamps (sanitize → the caller then escapes).
///
/// Mirrors the original `get_field_value` (`csv.rs:68-91`) over the curated 15, with two pinned
/// rules from the spine: `priority` is the bare integer (`priority.0`, NOT `P{n}`), and the
/// `status`/`issue_type` open-enum labels pass through `sanitize_inline` (their `Custom` arms carry
/// untrusted strings — MF-2).
fn get_field_value(issue: &Issue, field: &str) -> Option<String> {
    let value = match field {
        "id" => sanitize_inline(&issue.id).into_owned(),
        "title" => sanitize_inline(&issue.title).into_owned(),
        "description" => optional_user(issue.description.as_deref()),
        "status" => sanitize_inline(issue.status.as_str()).into_owned(),
        "priority" => issue.priority.0.to_string(),
        "issue_type" => sanitize_inline(issue.issue_type.as_str()).into_owned(),
        "assignee" => optional_user(issue.assignee.as_deref()),
        "owner" => optional_user(issue.owner.as_deref()),
        "created_at" => fmt_ts(issue.created_at),
        "updated_at" => fmt_ts(issue.updated_at),
        "closed_at" => issue.closed_at.map(fmt_ts).unwrap_or_default(),
        "due_at" => issue.due_at.map(fmt_ts).unwrap_or_default(),
        "defer_until" => issue.defer_until.map(fmt_ts).unwrap_or_default(),
        "notes" => optional_user(issue.notes.as_deref()),
        "external_ref" => optional_user(issue.external_ref.as_deref()),
        _ => return None,
    };
    Some(value)
}

/// Sanitize an optional user-controlled string field; `None` → empty cell.
fn optional_user(value: Option<&str>) -> String {
    value
        .map(|s| sanitize_inline(s).into_owned())
        .unwrap_or_default()
}

/// The CSV renderer. `fields` is resolved at construction (default or validated selection).
pub(crate) struct CsvRenderer {
    opts: RenderOptions,
}

impl CsvRenderer {
    pub(crate) fn new(opts: RenderOptions) -> Self {
        Self { opts }
    }

    /// Resolve the effective field list from `RenderOptions::csv_fields`.
    ///
    /// `None` → [`DEFAULT_FIELDS`]. An explicit selection is validated against [`ALL_FIELDS`]; an
    /// unknown name is a [`RenderError::FieldUnknown`] (a deliberate behaviour change from the
    /// original's silent filter, justified by the typed-error contract).
    fn resolve_fields(&self) -> Result<Vec<String>, RenderError> {
        match &self.opts.csv_fields {
            None => Ok(DEFAULT_FIELDS.iter().map(|f| (*f).to_string()).collect()),
            Some(fields) => {
                for field in fields {
                    if !is_known_field(field) {
                        return Err(RenderError::FieldUnknown {
                            field: field.clone(),
                        });
                    }
                }
                Ok(fields.clone())
            }
        }
    }

    /// Render `issues` as CSV (header + one row per issue) into an in-memory `String`.
    fn render_rows(&self, issues: &[Issue]) -> Result<RenderOutput, RenderError> {
        let fields = self.resolve_fields()?;
        let mut out = String::new();

        // Header (field names are static identifiers — no escaping needed, but stay consistent).
        let header = fields
            .iter()
            .map(|f| escape_field(f))
            .collect::<Vec<_>>()
            .join(",");
        // Writing to a String is infallible; the `_ =` keeps clippy happy without an expect/unwrap.
        let _ = writeln!(out, "{header}");

        for issue in issues {
            let row = fields
                .iter()
                .map(|field| {
                    // `field` came from `resolve_fields`, which validated against ALL_FIELDS, so
                    // `get_field_value` always returns Some here.
                    let cell = get_field_value(issue, field).unwrap_or_default();
                    escape_field(&cell)
                })
                .collect::<Vec<_>>()
                .join(",");
            let _ = writeln!(out, "{row}");
        }

        Ok(RenderOutput::new(out, ContentType::Csv))
    }
}

impl Renderer for CsvRenderer {
    fn format(&self) -> unblock_model::OutputFormat {
        unblock_model::OutputFormat::Csv
    }

    fn issue(&self, value: &Issue, _opts: &RenderOptions) -> Result<RenderOutput, RenderError> {
        self.render_rows(std::slice::from_ref(value))
    }

    fn issues(&self, value: &[Issue], _opts: &RenderOptions) -> Result<RenderOutput, RenderError> {
        self.render_rows(value)
    }

    fn counts(
        &self,
        _value: &[unblock_model::CountBucket],
        _opts: &RenderOptions,
    ) -> Result<RenderOutput, RenderError> {
        Err(RenderError::UnsupportedFormat {
            format: unblock_model::OutputFormat::Csv,
        })
    }

    fn dep_tree(
        &self,
        _value: &unblock_model::DepTree,
        _opts: &RenderOptions,
    ) -> Result<RenderOutput, RenderError> {
        Err(RenderError::UnsupportedFormat {
            format: unblock_model::OutputFormat::Csv,
        })
    }

    fn cycles(
        &self,
        _value: &[Vec<String>],
        _opts: &RenderOptions,
    ) -> Result<RenderOutput, RenderError> {
        Err(RenderError::UnsupportedFormat {
            format: unblock_model::OutputFormat::Csv,
        })
    }

    fn structured_error(
        &self,
        _value: &unblock_error::StructuredError,
        _opts: &RenderOptions,
    ) -> Result<RenderOutput, RenderError> {
        Err(RenderError::UnsupportedFormat {
            format: unblock_model::OutputFormat::Csv,
        })
    }

    fn diagnostics(
        &self,
        _value: &unblock_model::DiagnosticReport,
        _opts: &RenderOptions,
    ) -> Result<RenderOutput, RenderError> {
        Err(RenderError::UnsupportedFormat {
            format: unblock_model::OutputFormat::Csv,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ALL_FIELDS, CsvRenderer, DEFAULT_FIELDS, escape_field, get_field_value};
    use crate::options::RenderOptions;
    use crate::renderer::Renderer;
    use chrono::{TimeZone, Utc};
    use unblock_model::{Issue, Status};

    fn fixture(id: &str, title: &str) -> Issue {
        Issue {
            id: id.to_string(),
            title: title.to_string(),
            created_at: Utc.with_ymd_and_hms(2025, 1, 15, 12, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2025, 1, 15, 14, 30, 0).unwrap(),
            ..Issue::default()
        }
    }

    #[test]
    fn escape_field_plain_and_quoting() {
        assert_eq!(escape_field("simple"), "simple");
        assert_eq!(escape_field("hello, world"), "\"hello, world\"");
        assert_eq!(escape_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(escape_field("line1\nline2"), "\"line1\nline2\"");
    }

    #[test]
    fn escape_field_formula_injection_guard() {
        assert_eq!(escape_field("=1+1"), "\"'=1+1\"");
        assert_eq!(
            escape_field("+cmd|' /C calc'!A0"),
            "\"'+cmd|' /C calc'!A0\""
        );
        assert_eq!(escape_field("@SUM(A:A)"), "\"'@SUM(A:A)\"");
        assert_eq!(escape_field("-3 items"), "\"'-3 items\"");
        assert_eq!(escape_field("\n=1+1"), "\"'\n=1+1\"");
    }

    #[test]
    fn priority_is_bare_int() {
        let issue = fixture("ub-1", "x");
        assert_eq!(get_field_value(&issue, "priority").unwrap(), "2");
    }

    #[test]
    fn status_label_is_sanitized() {
        let mut issue = fixture("ub-1", "x");
        issue.status = Status::Custom("state\x1b[2J".to_string());
        let cell = get_field_value(&issue, "status").unwrap();
        assert!(cell.contains("\\u{1b}"));
        assert!(!cell.contains('\x1b'));
    }

    #[test]
    fn timestamps_use_fmt_ts() {
        let issue = fixture("ub-1", "x");
        assert_eq!(
            get_field_value(&issue, "created_at").unwrap(),
            "2025-01-15T12:00:00Z"
        );
    }

    #[test]
    fn default_fields_header() {
        let r = CsvRenderer::new(RenderOptions::default());
        let out = r
            .issues(&[fixture("ub-1", "First")], &RenderOptions::default())
            .unwrap();
        let first_line = out.stdout.lines().next().unwrap();
        assert_eq!(first_line, DEFAULT_FIELDS.join(","));
    }

    #[test]
    fn all_fields_has_15_columns() {
        assert_eq!(ALL_FIELDS.len(), 15);
        let opts = RenderOptions::default()
            .with_csv_fields(Some(ALL_FIELDS.iter().map(|f| (*f).to_string()).collect()));
        let r = CsvRenderer::new(opts.clone());
        let out = r.issues(&[fixture("ub-1", "x")], &opts).unwrap();
        let header = out.stdout.lines().next().unwrap();
        assert_eq!(header.split(',').count(), 15);
    }

    #[test]
    fn unknown_field_errors() {
        let opts = RenderOptions::default().with_csv_fields(Some(vec!["bogus".to_string()]));
        let r = CsvRenderer::new(opts.clone());
        let err = r.issues(&[fixture("ub-1", "x")], &opts).unwrap_err();
        assert!(matches!(err, super::RenderError::FieldUnknown { .. }));
    }

    #[test]
    fn empty_list_is_header_only() {
        let r = CsvRenderer::new(RenderOptions::default());
        let out = r.issues(&[], &RenderOptions::default()).unwrap();
        assert_eq!(out.stdout.lines().count(), 1);
    }

    #[test]
    fn unsupported_kinds_error() {
        let r = CsvRenderer::new(RenderOptions::default());
        assert!(r.counts(&[], &RenderOptions::default()).is_err());
    }
}
