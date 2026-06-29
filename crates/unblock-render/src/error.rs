//! The per-crate [`RenderError`] enum (D4) and its single-valued [`RenderError::code`]
//! mapping into the shared [`unblock_error::ErrorCode`] taxonomy (spine §6.5).
//!
//! There are exactly **three** variants: the CSV write path is hand-rolled over an in-memory
//! `String` (infallible — no `csv` crate, no I/O), so there is **no** `Csv`/`IoError` path. The
//! enum, its variants, and `code()` are all `pub` (cli/mcp match on them at the L7 boundary); only
//! the snafu context selectors are crate-internal.

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
}

impl RenderError {
    /// The stable [`ErrorCode`] for this error (spine §6.5: a total, single-valued map).
    ///
    /// `Serialize` is the JSON-serialization family (`JsonError`, exit 8); `UnsupportedFormat`
    /// and `FieldUnknown` are validation failures (`ValidationFailed`, exit 4).
    #[must_use]
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Serialize { .. } => ErrorCode::JsonError,
            Self::UnsupportedFormat { .. } | Self::FieldUnknown { .. } => {
                ErrorCode::ValidationFailed
            }
        }
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
}
