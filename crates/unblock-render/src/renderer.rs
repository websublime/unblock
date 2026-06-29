//! The object-safe [`Renderer`] trait and the [`renderer_for`] factory — the crate's contract.
//!
//! One method per renderable kind (`issue`/`issues`/`counts`/`dep_tree`/`cycles`/
//! `structured_error`/`diagnostics`), each `&self` + a typed reference + `&RenderOptions` →
//! `Result<RenderOutput, RenderError>`, plus `format()`. Every method takes `&self` and returns a
//! concrete type, so `Box<dyn Renderer>` is object-safe.
//!
//! A backend that cannot represent a kind (e.g. CSV has no dependency tree) returns
//! [`RenderError::UnsupportedFormat`] — it never drops the method (object safety + completeness).

use unblock_error::StructuredError;
use unblock_model::{CountBucket, DepTree, DiagnosticReport, Issue, OutputFormat};

use crate::backend::{
    csv_fmt::CsvRenderer, json::JsonRenderer, markdown::MarkdownRenderer, plain::PlainRenderer,
};
use crate::error::RenderError;
use crate::options::{RenderOptions, RenderOutput};

/// Object-safe format dispatcher. One impl per backend (`json` + `robot` share one).
///
/// Every method renders one §1.10 result kind into a [`RenderOutput`]. The `cycles` input is a
/// slice of ordered cycle paths (spine §3.2.1 / D3): render preserves the caller's `Vec` order and
/// never re-sorts (MF-5).
pub trait Renderer {
    /// The format this renderer emits (factory round-trip check).
    fn format(&self) -> OutputFormat;

    /// Render a single [`Issue`].
    ///
    /// # Errors
    /// Returns [`RenderError`] if serialization fails or the format cannot represent an issue.
    fn issue(&self, value: &Issue, opts: &RenderOptions) -> Result<RenderOutput, RenderError>;

    /// Render a list of [`Issue`]s (caller order preserved).
    ///
    /// # Errors
    /// Returns [`RenderError`] if serialization fails or a requested CSV field is unknown.
    fn issues(&self, value: &[Issue], opts: &RenderOptions) -> Result<RenderOutput, RenderError>;

    /// Render count buckets (caller order preserved).
    ///
    /// # Errors
    /// Returns [`RenderError::UnsupportedFormat`] for formats that cannot represent counts (CSV).
    fn counts(
        &self,
        value: &[CountBucket],
        opts: &RenderOptions,
    ) -> Result<RenderOutput, RenderError>;

    /// Render a dependency tree (edges in caller order).
    ///
    /// # Errors
    /// Returns [`RenderError::UnsupportedFormat`] for formats that cannot represent a tree (CSV).
    fn dep_tree(&self, value: &DepTree, opts: &RenderOptions) -> Result<RenderOutput, RenderError>;

    /// Render cycle witnesses: each inner `Vec` is one ordered cycle path (caller order preserved).
    ///
    /// # Errors
    /// Returns [`RenderError::UnsupportedFormat`] for formats that cannot represent cycles (CSV).
    fn cycles(
        &self,
        value: &[Vec<String>],
        opts: &RenderOptions,
    ) -> Result<RenderOutput, RenderError>;

    /// Render a [`StructuredError`] (always valid output even on error, FR-11).
    ///
    /// # Errors
    /// Returns [`RenderError::UnsupportedFormat`] for formats that cannot represent an error (CSV).
    fn structured_error(
        &self,
        value: &StructuredError,
        opts: &RenderOptions,
    ) -> Result<RenderOutput, RenderError>;

    /// Render a [`DiagnosticReport`].
    ///
    /// # Errors
    /// Returns [`RenderError::UnsupportedFormat`] for formats that cannot represent diagnostics (CSV).
    fn diagnostics(
        &self,
        value: &DiagnosticReport,
        opts: &RenderOptions,
    ) -> Result<RenderOutput, RenderError>;
}

/// Construct the [`Renderer`] for `fmt`, capturing `opts`.
///
/// The match is exhaustive in both feature states: the `OutputFormat::Toon` arm is itself
/// `#[cfg(feature = "toon")]`-gated, so it only exists (and must be matched) when the feature is on.
#[must_use]
pub fn renderer_for(fmt: OutputFormat, opts: RenderOptions) -> Box<dyn Renderer> {
    match fmt {
        OutputFormat::Json => Box::new(JsonRenderer::new(false, opts)),
        OutputFormat::Robot => Box::new(JsonRenderer::new(true, opts)),
        OutputFormat::Plain => Box::new(PlainRenderer::new(opts)),
        OutputFormat::Csv => Box::new(CsvRenderer::new(opts)),
        OutputFormat::Markdown => Box::new(MarkdownRenderer::new(opts)),
        #[cfg(feature = "toon")]
        OutputFormat::Toon => Box::new(crate::backend::toon::ToonRenderer::new(opts)),
    }
}
