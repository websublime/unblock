//! Render configuration value types — all **render-private, L6-internal** (spine §1.10 / §6.1:
//! they carry no contract-DTO derive obligation).
//!
//! [`RenderOptions`] is a pure value type (`Clone, Debug, Default`) so one opts set can be reused
//! across many [`crate::renderer_for`] calls (MF-4). There is **no** color / TTY field (D7 — render
//! is always plain and never detects a terminal; the caller decides).

/// The content type of a [`RenderOutput`] payload (a hint for the caller's MIME/header handling).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    /// `application/json` (json + robot).
    Json,
    /// `text/plain`.
    Text,
    /// `text/csv`.
    Csv,
    /// `text/markdown`.
    Markdown,
    /// TOON (v1.1; only present under `--features toon`).
    #[cfg(feature = "toon")]
    Toon,
}

/// Pure rendering configuration (no color/TTY — D7).
///
/// All fields are plain values; the builder setters return `self` for chaining. `Default` is the
/// neutral config: compact JSON, no width cap, default CSV field set, and second-precision
/// timestamps (the only timestamp form render emits — see [`crate::fmt_ts`]).
#[derive(Clone, Debug, Default)]
pub struct RenderOptions {
    /// Pretty-print JSON (the `Json` format). `Robot` always emits compact JSON regardless.
    pub pretty_json: bool,
    /// Maximum visible width for single-line plain output (title truncation). `None` = no cap.
    pub max_width: Option<usize>,
    /// Explicit CSV field selection. `None` = the default 8-column set.
    pub csv_fields: Option<Vec<String>>,
    /// Render timestamps at second precision (the only supported form; reserved for future
    /// sub-second toggles). Currently informational — [`crate::fmt_ts`] is always second-precision.
    pub timestamp_secs_only: bool,
}

impl RenderOptions {
    /// Set [`RenderOptions::pretty_json`] (builder).
    #[must_use]
    pub fn with_pretty_json(mut self, pretty: bool) -> Self {
        self.pretty_json = pretty;
        self
    }

    /// Set [`RenderOptions::max_width`] (builder).
    #[must_use]
    pub fn with_max_width(mut self, width: Option<usize>) -> Self {
        self.max_width = width;
        self
    }

    /// Set [`RenderOptions::csv_fields`] (builder).
    #[must_use]
    pub fn with_csv_fields(mut self, fields: Option<Vec<String>>) -> Self {
        self.csv_fields = fields;
        self
    }

    /// Set [`RenderOptions::timestamp_secs_only`] (builder).
    #[must_use]
    pub fn with_timestamp_secs_only(mut self, secs_only: bool) -> Self {
        self.timestamp_secs_only = secs_only;
        self
    }
}

/// A rendered payload plus its content type (pure value — the caller routes it to stdout, with
/// diagnostics to stderr, NFR-14). This crate never writes to a stream or file itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderOutput {
    /// The rendered bytes (UTF-8 `String`) destined for stdout.
    pub stdout: String,
    /// The content type of [`RenderOutput::stdout`].
    pub content_type: ContentType,
}

impl RenderOutput {
    /// Construct a [`RenderOutput`] from a payload and its content type.
    #[must_use]
    pub fn new(stdout: String, content_type: ContentType) -> Self {
        Self {
            stdout,
            content_type,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ContentType, RenderOptions, RenderOutput};

    #[test]
    fn default_is_neutral() {
        let opts = RenderOptions::default();
        assert!(!opts.pretty_json);
        assert!(opts.max_width.is_none());
        assert!(opts.csv_fields.is_none());
        assert!(!opts.timestamp_secs_only);
    }

    #[test]
    fn builders_mutate_expected_field() {
        let opts = RenderOptions::default()
            .with_pretty_json(true)
            .with_max_width(Some(72))
            .with_csv_fields(Some(vec!["id".to_string(), "title".to_string()]))
            .with_timestamp_secs_only(true);
        assert!(opts.pretty_json);
        assert_eq!(opts.max_width, Some(72));
        assert_eq!(
            opts.csv_fields,
            Some(vec!["id".to_string(), "title".to_string()])
        );
        assert!(opts.timestamp_secs_only);
    }

    #[test]
    fn options_clone_is_independent() {
        let opts = RenderOptions::default().with_max_width(Some(40));
        let clone = opts.clone();
        assert_eq!(opts.max_width, clone.max_width);
    }

    #[test]
    fn render_output_new() {
        let out = RenderOutput::new("body".to_string(), ContentType::Text);
        assert_eq!(out.stdout, "body");
        assert_eq!(out.content_type, ContentType::Text);
    }
}
