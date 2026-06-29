//! JSON + Robot backend (one impl, two modes).
//!
//! `Json` = `serde_json::to_string_pretty` when `RenderOptions::pretty_json`, else compact; `Robot`
//! = always compact `serde_json::to_string`. Every kind serializes its §1.10 DTO / `StructuredError`
//! directly, so JSON output is always valid even on error (FR-11). serde escaping covers untrusted
//! strings, so no extra sanitization is applied here.

use serde::Serialize;
use snafu::ResultExt;
use unblock_error::StructuredError;
use unblock_model::{CountBucket, DepTree, DiagnosticReport, Issue, OutputFormat};

use crate::error::{RenderError, SerializeSnafu};
use crate::options::{ContentType, RenderOptions, RenderOutput};
use crate::renderer::Renderer;

/// The JSON/robot renderer. `robot = true` forces compact output (machine-parse line);
/// `robot = false` honours [`RenderOptions::pretty_json`].
pub(crate) struct JsonRenderer {
    robot: bool,
    opts: RenderOptions,
}

impl JsonRenderer {
    pub(crate) fn new(robot: bool, opts: RenderOptions) -> Self {
        Self { robot, opts }
    }

    /// Serialize `value` to a [`RenderOutput`] using the configured mode.
    fn render<T: Serialize>(&self, value: &T) -> Result<RenderOutput, RenderError> {
        let body = if self.robot || !self.opts.pretty_json {
            serde_json::to_string(value).context(SerializeSnafu)?
        } else {
            serde_json::to_string_pretty(value).context(SerializeSnafu)?
        };
        Ok(RenderOutput::new(body, ContentType::Json))
    }

    fn format(&self) -> OutputFormat {
        if self.robot {
            OutputFormat::Robot
        } else {
            OutputFormat::Json
        }
    }
}

impl Renderer for JsonRenderer {
    fn format(&self) -> OutputFormat {
        JsonRenderer::format(self)
    }

    fn issue(&self, value: &Issue, _opts: &RenderOptions) -> Result<RenderOutput, RenderError> {
        self.render(value)
    }

    fn issues(&self, value: &[Issue], _opts: &RenderOptions) -> Result<RenderOutput, RenderError> {
        self.render(&value)
    }

    fn counts(
        &self,
        value: &[CountBucket],
        _opts: &RenderOptions,
    ) -> Result<RenderOutput, RenderError> {
        self.render(&value)
    }

    fn dep_tree(
        &self,
        value: &DepTree,
        _opts: &RenderOptions,
    ) -> Result<RenderOutput, RenderError> {
        self.render(value)
    }

    fn cycles(
        &self,
        value: &[Vec<String>],
        _opts: &RenderOptions,
    ) -> Result<RenderOutput, RenderError> {
        self.render(&value)
    }

    fn structured_error(
        &self,
        value: &StructuredError,
        _opts: &RenderOptions,
    ) -> Result<RenderOutput, RenderError> {
        self.render(value)
    }

    fn diagnostics(
        &self,
        value: &DiagnosticReport,
        _opts: &RenderOptions,
    ) -> Result<RenderOutput, RenderError> {
        self.render(value)
    }
}

#[cfg(test)]
mod tests {
    use super::JsonRenderer;
    use crate::options::RenderOptions;
    use crate::renderer::Renderer;
    use unblock_model::{Issue, OutputFormat};

    fn fixture() -> Issue {
        use chrono::{TimeZone, Utc};
        Issue {
            id: "ub-abc123".to_string(),
            title: "Hello".to_string(),
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            ..Issue::default()
        }
    }

    #[test]
    fn robot_is_compact_json_parses_back() {
        let r = JsonRenderer::new(true, RenderOptions::default());
        let out = r.issue(&fixture(), &RenderOptions::default()).unwrap();
        assert_eq!(r.format(), OutputFormat::Robot);
        assert!(!out.stdout.contains('\n'), "robot must be a single line");
        let back: Issue = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(back.id, "ub-abc123");
    }

    #[test]
    fn pretty_json_multiline_when_requested() {
        let opts = RenderOptions::default().with_pretty_json(true);
        let r = JsonRenderer::new(false, opts.clone());
        let out = r.issue(&fixture(), &opts).unwrap();
        assert_eq!(r.format(), OutputFormat::Json);
        assert!(out.stdout.contains('\n'), "pretty json must be multi-line");
    }

    #[test]
    fn default_json_is_compact() {
        let opts = RenderOptions::default();
        let r = JsonRenderer::new(false, opts.clone());
        let out = r.issue(&fixture(), &opts).unwrap();
        assert!(!out.stdout.contains('\n'));
    }
}
