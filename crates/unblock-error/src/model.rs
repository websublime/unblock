//! [`ModelError`] — the one concrete per-crate error enum that `unblock-model` returns (spine
//! §1.1/§1.2/§1.9), plus the [`FieldError`] carrier for the aggregate validation variant (D-E1).
//!
//! It lives here (not in `unblock-model`) so the dependency arrow stays `model → error`: the model
//! crate's `FromStr`/`validate` return `unblock_error::ModelError` without depending on a sibling
//! for its `ErrorCode` mapping. The scalar variants back the single-field `FromStr` paths; the
//! [`ModelError::ValidationFailed`] aggregate carries every failure an `IssueValidator::validate`
//! run found, so the boundary still emits exactly one [`ErrorCode`] while preserving per-field
//! detail (surfaced as `context["fields"]`, spine §2.4).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use snafu::Snafu;

use crate::code::ErrorCode;
use crate::coded::CodedError;
use crate::hints::{PRIORITY_DETAIL_HINT, VALID_STATUS_HINT, VALID_TYPE_HINT};

/// A single field-level validation failure (D-E1).
///
/// `FieldError`s are collected into [`ModelError::ValidationFailed`] and surface, when bridged to a
/// [`crate::StructuredError`], as the `context["fields"]` array of `{ field, reason }` objects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FieldError {
    /// The offending field name (e.g. `"title"`).
    pub field: String,
    /// A human-readable reason (e.g. `"exceeds 500 characters"`).
    pub reason: String,
}

impl FieldError {
    /// Construct a [`FieldError`] from a field name and reason.
    #[must_use]
    pub fn new(field: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            reason: reason.into(),
        }
    }
}

/// The concrete error type `unblock-model` returns (spine §1.1/§1.2/§1.9).
///
/// Scalar variants back the single-field `FromStr` paths; [`ModelError::ValidationFailed`] is the
/// aggregate carrier for a full `IssueValidator::validate` run (D-E1). There is intentionally **no**
/// `TitleTooLong` variant — an empty/whitespace title maps to [`ErrorCode::RequiredField`], and a
/// title longer than 500 `char`s is one [`FieldError`] inside `ValidationFailed`.
#[derive(Debug, Clone, PartialEq, Eq, Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum ModelError {
    /// A priority value could not be parsed or is out of the 0..=4 range.
    #[snafu(display("invalid priority: {value}"))]
    InvalidPriority {
        /// The rejected priority text.
        value: String,
    },

    /// A status value could not be parsed.
    #[snafu(display("invalid status: {value}"))]
    InvalidStatus {
        /// The rejected status text.
        value: String,
    },

    /// An issue-type value could not be parsed.
    #[snafu(display("invalid issue type: {value}"))]
    InvalidType {
        /// The rejected issue-type text.
        value: String,
    },

    /// An issue id has an invalid format.
    #[snafu(display("invalid id: {id}"))]
    InvalidId {
        /// The rejected id.
        id: String,
    },

    /// A required field was empty or missing.
    #[snafu(display("required field missing: {field}"))]
    RequiredField {
        /// The name of the missing field.
        field: &'static str,
    },

    /// Reparenting would create a dependency cycle.
    #[snafu(display("reparent would create a cycle: {path}"))]
    ReparentCycle {
        /// The detected cycle path.
        path: String,
    },

    /// One or more field validations failed (aggregate; D-E1).
    #[snafu(display("validation failed: {} field(s)", fields.len()))]
    ValidationFailed {
        /// Every field-level failure found in one validation run.
        fields: Vec<FieldError>,
    },
}

impl CodedError for ModelError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::InvalidPriority { .. } => ErrorCode::InvalidPriority,
            Self::InvalidStatus { .. } => ErrorCode::InvalidStatus,
            Self::InvalidType { .. } => ErrorCode::InvalidType,
            Self::InvalidId { .. } => ErrorCode::InvalidId,
            Self::RequiredField { .. } => ErrorCode::RequiredField,
            Self::ReparentCycle { .. } => ErrorCode::CycleDetected,
            Self::ValidationFailed { .. } => ErrorCode::ValidationFailed,
        }
    }

    fn hint(&self) -> Option<String> {
        match self {
            Self::InvalidPriority { .. } => Some(PRIORITY_DETAIL_HINT.to_string()),
            Self::InvalidStatus { .. } => Some(VALID_STATUS_HINT.to_string()),
            Self::InvalidType { .. } => Some(VALID_TYPE_HINT.to_string()),
            Self::InvalidId { .. }
            | Self::RequiredField { .. }
            | Self::ReparentCycle { .. }
            | Self::ValidationFailed { .. } => None,
        }
    }

    fn context(&self) -> Map<String, Value> {
        let mut map = Map::new();
        match self {
            Self::InvalidPriority { value }
            | Self::InvalidStatus { value }
            | Self::InvalidType { value } => {
                map.insert("provided".to_string(), Value::String(value.clone()));
            }
            Self::InvalidId { id } => {
                map.insert("id".to_string(), Value::String(id.clone()));
            }
            Self::RequiredField { field } => {
                map.insert("field".to_string(), Value::String((*field).to_string()));
            }
            Self::ReparentCycle { path } => {
                map.insert("cycle_path".to_string(), Value::String(path.clone()));
            }
            Self::ValidationFailed { fields } => {
                let array = fields
                    .iter()
                    .map(|f| json!({ "field": f.field, "reason": f.reason }))
                    .collect();
                map.insert("fields".to_string(), Value::Array(array));
            }
        }
        map
    }
}

#[cfg(test)]
mod tests {
    use super::{FieldError, ModelError};
    use crate::code::ErrorCode;
    use crate::coded::CodedError;
    use crate::structured::StructuredError;

    #[test]
    fn scalar_variants_map_to_codes() {
        assert_eq!(
            ModelError::InvalidPriority { value: "9".into() }.code(),
            ErrorCode::InvalidPriority
        );
        assert_eq!(
            ModelError::InvalidStatus { value: "x".into() }.code(),
            ErrorCode::InvalidStatus
        );
        assert_eq!(
            ModelError::InvalidType { value: "x".into() }.code(),
            ErrorCode::InvalidType
        );
        assert_eq!(
            ModelError::InvalidId { id: "bad".into() }.code(),
            ErrorCode::InvalidId
        );
        assert_eq!(
            ModelError::RequiredField { field: "title" }.code(),
            ErrorCode::RequiredField
        );
        assert_eq!(
            ModelError::ReparentCycle { path: "a -> b -> a".into() }.code(),
            ErrorCode::CycleDetected
        );
    }

    #[test]
    fn validation_failed_maps_and_carries_fields() {
        let err = ModelError::ValidationFailed {
            fields: vec![
                FieldError::new("title", "cannot be empty"),
                FieldError::new("priority", "must be 0-4"),
            ],
        };
        assert_eq!(err.code(), ErrorCode::ValidationFailed);

        let structured: StructuredError = (&err).into();
        let fields = structured.context["fields"].as_array().unwrap();
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0]["field"], "title");
        assert_eq!(fields[0]["reason"], "cannot be empty");
        assert_eq!(fields[1]["field"], "priority");
    }

    #[test]
    fn invalid_priority_hint_and_context() {
        let err = ModelError::InvalidPriority { value: "high".into() };
        let structured = StructuredError::from_coded(&err);
        assert!(structured.hint.is_some());
        assert_eq!(structured.context["provided"], "high");
        assert!(structured.retryable);
    }

    #[test]
    fn display_is_non_empty() {
        let err = ModelError::InvalidId { id: "BAD".into() };
        assert!(!err.to_string().is_empty());
    }
}
