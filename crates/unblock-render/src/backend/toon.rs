//! TOON backend — **cfg-empty placeholder** (A.OQ-C / crate plan §5 item 4).
//!
//! This whole module is `#[cfg(feature = "toon")]`-gated. In v1 there is **no** `toon`/`toon_rust`
//! encoder dependency: the `ToonRenderer` exists only so the feature-matrix CI proves the gated
//! surface compiles, and every method errs [`RenderError::UnsupportedFormat`] until the encoder
//! lands in v1.1. The default build (`--no-default-features`) has zero TOON surface (NFR-10).

use unblock_error::StructuredError;
use unblock_model::{CountBucket, DepTree, DiagnosticReport, Issue, OutputFormat};

use crate::error::RenderError;
use crate::options::{RenderOptions, RenderOutput};
use crate::renderer::Renderer;

/// v1.1 TOON renderer — present (under `--features toon`) but encoderless in v1.
pub(crate) struct ToonRenderer {
    #[allow(dead_code)]
    opts: RenderOptions,
}

impl ToonRenderer {
    pub(crate) fn new(opts: RenderOptions) -> Self {
        Self { opts }
    }
}

/// Until the v1.1 encoder lands, every TOON method errs `UnsupportedFormat`.
fn unsupported() -> Result<RenderOutput, RenderError> {
    Err(RenderError::UnsupportedFormat {
        format: OutputFormat::Toon,
    })
}

impl Renderer for ToonRenderer {
    fn format(&self) -> OutputFormat {
        OutputFormat::Toon
    }

    fn issue(&self, _value: &Issue, _opts: &RenderOptions) -> Result<RenderOutput, RenderError> {
        unsupported()
    }

    fn issues(&self, _value: &[Issue], _opts: &RenderOptions) -> Result<RenderOutput, RenderError> {
        unsupported()
    }

    fn counts(
        &self,
        _value: &[CountBucket],
        _opts: &RenderOptions,
    ) -> Result<RenderOutput, RenderError> {
        unsupported()
    }

    fn dep_tree(
        &self,
        _value: &DepTree,
        _opts: &RenderOptions,
    ) -> Result<RenderOutput, RenderError> {
        unsupported()
    }

    fn cycles(
        &self,
        _value: &[Vec<String>],
        _opts: &RenderOptions,
    ) -> Result<RenderOutput, RenderError> {
        unsupported()
    }

    fn structured_error(
        &self,
        _value: &StructuredError,
        _opts: &RenderOptions,
    ) -> Result<RenderOutput, RenderError> {
        unsupported()
    }

    fn diagnostics(
        &self,
        _value: &DiagnosticReport,
        _opts: &RenderOptions,
    ) -> Result<RenderOutput, RenderError> {
        unsupported()
    }
}
