//! Output-format selector (CF-J, spine §1.10).
//!
//! Owned here once so `unblock-render` and `unblock-config` share a single enum (no lock-step drift
//! on the Csv/Markdown/Toon variants); both re-export it. The model crate has **no** encoder — the
//! `toon` feature is a bare marker that only reserves the variant.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The output format for rendered command output (spine §1.10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    /// Structured JSON to stdout (the default).
    #[default]
    Json,
    /// A stable machine-parse line format.
    Robot,
    /// Human-readable terminal output.
    Plain,
    /// Comma-separated values.
    Csv,
    /// Markdown.
    Markdown,
    /// TOON (v1.1; behind the `toon` Cargo feature).
    #[cfg(feature = "toon")]
    Toon,
}

#[cfg(test)]
mod tests {
    use super::OutputFormat;

    #[test]
    fn default_is_json() {
        assert_eq!(OutputFormat::default(), OutputFormat::Json);
    }

    #[test]
    fn serde_snake_case() {
        assert_eq!(
            serde_json::to_string(&OutputFormat::Json).unwrap(),
            "\"json\""
        );
        assert_eq!(
            serde_json::to_string(&OutputFormat::Markdown).unwrap(),
            "\"markdown\""
        );
        let f: OutputFormat = serde_json::from_str("\"robot\"").unwrap();
        assert_eq!(f, OutputFormat::Robot);
    }

    #[cfg(feature = "toon")]
    #[test]
    fn toon_present_with_feature() {
        assert_eq!(
            serde_json::to_string(&OutputFormat::Toon).unwrap(),
            "\"toon\""
        );
    }
}
