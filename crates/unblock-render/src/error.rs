//! The per-crate [`RenderError`] enum (D4) and its single-valued [`RenderError::code`]
//! mapping into the shared [`unblock_error::ErrorCode`] taxonomy (spine §6.5).
//!
//! There are exactly **four** variants: the CSV write path is hand-rolled over an in-memory
//! `String` (infallible — no `csv` crate, no I/O), so there is **no** `Csv`/`IoError` path. The
//! enum, its variants, and `code()` are all `pub` (cli/mcp match on them at the L7 boundary); only
//! the snafu context selectors are crate-internal.
//!
//! `RenderError` also implements [`unblock_error::CodedError`] (D27/AF-4, T3.1 — delegating to the
//! inherent [`RenderError::code`]) so the uniform `(&err).into()` L7 bridge covers it like every
//! other per-crate error enum.

use snafu::Snafu;
use unblock_error::ErrorCode;
use unblock_model::OutputFormat;

/// The error type every [`crate::Renderer`] method returns (spine §6.5: one error → one code).
///
/// # Examples
///
/// ```
/// use unblock_render::RenderError;
/// use unblock_model::OutputFormat;
/// use unblock_error::ErrorCode;
///
/// let err = RenderError::UnsupportedFormat { format: OutputFormat::Csv };
/// assert_eq!(err.code(), ErrorCode::ValidationFailed);
///
/// // The 4th variant (D27/AF-4) — an unknown format name from `parse_format` — is the same family.
/// let unknown = RenderError::UnknownFormat { name: "xml".to_string() };
/// assert_eq!(unknown.code(), ErrorCode::ValidationFailed);
/// ```
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum RenderError {
    /// JSON/robot serialization failed.
    #[snafu(display("failed to serialize output as JSON: {source}"))]
    Serialize {
        /// The underlying `serde_json` error.
        source: serde_json::Error,
    },

    /// The requested format does not support this result kind (e.g. CSV cannot render a
    /// dependency tree).
    #[snafu(display("format {format:?} does not support this result kind"))]
    UnsupportedFormat {
        /// The format that was asked to render an unsupported kind.
        format: OutputFormat,
    },

    /// An unknown CSV field name was requested via `RenderOptions::csv_fields`.
    #[snafu(display("unknown CSV field: {field}"))]
    FieldUnknown {
        /// The unrecognised field name.
        field: String,
    },

    /// The requested format name is not a known format (from
    /// [`parse_format`](crate::parse_format)) — D27/AF-4 (T3.1).
    ///
    /// Distinct from [`UnsupportedFormat`](RenderError::UnsupportedFormat) (a KNOWN format that
    /// cannot render a particular result kind): this carries the raw offending name so the boundary
    /// can echo exactly what the caller typed.
    #[snafu(display("unknown output format: {name}"))]
    UnknownFormat {
        /// The raw, unrecognised format name the caller passed.
        name: String,
    },
}

impl RenderError {
    /// The stable [`ErrorCode`] for this error (spine §6.5: a total, single-valued map).
    ///
    /// `Serialize` is the JSON-serialization family (`JsonError`, exit 8); `UnsupportedFormat`,
    /// `FieldUnknown`, and `UnknownFormat` are validation failures (`ValidationFailed`, exit 4).
    #[must_use]
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Serialize { .. } => ErrorCode::JsonError,
            Self::UnsupportedFormat { .. }
            | Self::FieldUnknown { .. }
            | Self::UnknownFormat { .. } => ErrorCode::ValidationFailed,
        }
    }
}

impl unblock_error::CodedError for RenderError {
    /// Delegate to the inherent [`RenderError::code`] (D27/AF-4, spine §6.5: one error → one code) so
    /// the uniform L7 `(&err).into()` bridge (`StructuredError: From<&RenderError>`) covers
    /// `RenderError` like every other per-crate error enum. `hint`/`retryable`/`context` use the
    /// trait defaults (`retryable` tracks `code().is_retryable()`).
    fn code(&self) -> ErrorCode {
        self.code()
    }
}

#[cfg(test)]
mod tests {
    use super::RenderError;
    use unblock_error::ErrorCode;
    use unblock_model::OutputFormat;

    #[test]
    fn serialize_maps_to_json_error_exit_8() {
        // Force a real serde_json error: serializing a map with a non-string key fails.
        let bad = serde_json::to_string(&std::collections::BTreeMap::from([(vec![1u8], 2u8)]))
            .expect_err("non-string map key must fail to serialize");
        let err = RenderError::Serialize { source: bad };
        assert_eq!(err.code(), ErrorCode::JsonError);
        assert_eq!(err.code().exit_code(), 8);
    }

    #[test]
    fn unsupported_format_maps_to_validation_failed_exit_4() {
        let err = RenderError::UnsupportedFormat {
            format: OutputFormat::Csv,
        };
        assert_eq!(err.code(), ErrorCode::ValidationFailed);
        assert_eq!(err.code().exit_code(), 4);
    }

    #[test]
    fn field_unknown_maps_to_validation_failed_exit_4() {
        let err = RenderError::FieldUnknown {
            field: "nope".to_string(),
        };
        assert_eq!(err.code(), ErrorCode::ValidationFailed);
        assert_eq!(err.code().exit_code(), 4);
    }

    #[test]
    fn display_messages_are_stable() {
        let err = RenderError::FieldUnknown {
            field: "bogus".to_string(),
        };
        assert_eq!(err.to_string(), "unknown CSV field: bogus");
    }

    #[test]
    fn unknown_format_maps_to_validation_failed_exit_4() {
        let err = RenderError::UnknownFormat {
            name: "xml".to_string(),
        };
        assert_eq!(err.code(), ErrorCode::ValidationFailed);
        assert_eq!(err.code().exit_code(), 4);
        assert_eq!(err.to_string(), "unknown output format: xml");
    }

    #[test]
    fn coded_error_bridge_round_trips_to_validation_failed() {
        use unblock_error::{CodedError, StructuredError};
        // D27/AF-4: `impl CodedError for RenderError` lets the uniform L7 bridge cover RenderError.
        let err = RenderError::UnknownFormat {
            name: "xml".to_string(),
        };
        // Via the `&dyn CodedError` object path.
        let dyn_code = (&err as &dyn CodedError).code();
        assert_eq!(dyn_code, ErrorCode::ValidationFailed);
        // Via the blanket `From<&E: CodedError + Error>` into StructuredError (what the cli exit uses).
        let structured: StructuredError = (&err).into();
        assert_eq!(structured.code, ErrorCode::ValidationFailed);
        assert_eq!(structured.message, "unknown output format: xml");
        // `retryable` tracks `code().is_retryable()` (the default) — `ValidationFailed` is retryable
        // per spine §2.2, so the bridge reflects that.
        assert_eq!(
            structured.retryable,
            ErrorCode::ValidationFailed.is_retryable()
        );
    }
}
