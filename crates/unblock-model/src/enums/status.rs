//! [`Status`] — issue lifecycle state (spine §1.1).

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use unblock_error::ModelError;

/// Issue lifecycle status (spine §1.1).
///
/// An **open enum**: any string that is not a known variant deserializes into [`Status::Custom`]
/// (preserving its original case) rather than failing. The wire form is the `snake_case`
/// [`Status::as_str`] string; `Serialize`/`Deserialize`/`JsonSchema` are hand-rolled (string).
///
/// # Examples
///
/// ```
/// use unblock_model::Status;
/// use std::str::FromStr;
///
/// assert_eq!(Status::InProgress.as_str(), "in_progress");
/// assert_eq!(Status::from_str("IN_PROGRESS").unwrap(), Status::InProgress);
/// assert_eq!(Status::from_str("QaReview").unwrap(), Status::Custom("QaReview".to_string()));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum Status {
    /// Open and ready to be worked on.
    #[default]
    Open,
    /// Actively in progress.
    InProgress,
    /// Blocked by a dependency.
    Blocked,
    /// Deferred until a later time.
    Deferred,
    /// A draft, not yet ready for execution.
    Draft,
    /// Closed (terminal).
    Closed,
    /// Soft-deleted (terminal); see the tombstone semantics in [`crate::Issue`].
    Tombstone,
    /// Pinned for visibility.
    Pinned,
    /// An open-enum tail variant for any unrecognised status (original case preserved).
    Custom(String),
}

impl Status {
    /// Map a string to a known variant (case-insensitive), or `None` if unrecognised.
    fn known_value(value: &str) -> Option<Self> {
        Some(match value.to_lowercase().as_str() {
            "open" => Self::Open,
            "in_progress" | "inprogress" => Self::InProgress,
            "blocked" => Self::Blocked,
            "deferred" => Self::Deferred,
            "draft" => Self::Draft,
            "closed" => Self::Closed,
            "tombstone" => Self::Tombstone,
            "pinned" => Self::Pinned,
            _ => return None,
        })
    }

    /// The stable wire string for this status (`snake_case` for known variants).
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Open => "open",
            Self::InProgress => "in_progress",
            Self::Blocked => "blocked",
            Self::Deferred => "deferred",
            Self::Draft => "draft",
            Self::Closed => "closed",
            Self::Tombstone => "tombstone",
            Self::Pinned => "pinned",
            Self::Custom(value) => value,
        }
    }

    /// Whether this is a terminal status (`Closed` or `Tombstone`).
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Closed | Self::Tombstone)
    }

    /// Whether this is an active status (`Open` or `InProgress`).
    #[must_use]
    pub const fn is_active(&self) -> bool {
        matches!(self, Self::Open | Self::InProgress)
    }

    /// Whether this is the draft status.
    #[must_use]
    pub const fn is_draft(&self) -> bool {
        matches!(self, Self::Draft)
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Status {
    type Err = ModelError;

    /// Parse a status string. **Infallible** for an open enum: an unknown string becomes
    /// [`Status::Custom`]. The `Err` type is fixed by the spine; this never returns `Err`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::known_value(s).unwrap_or_else(|| Self::Custom(s.to_string())))
    }
}

impl Serialize for Status {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Status {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Ok(Self::known_value(&value).unwrap_or(Self::Custom(value)))
    }
}

impl JsonSchema for Status {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("Status")
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        // Serialized as a string (see the hand-rolled Serialize/Deserialize above).
        generator.subschema_for::<String>()
    }
}

#[cfg(test)]
mod tests {
    use super::Status;
    use std::str::FromStr;

    #[test]
    fn as_str_from_str_roundtrip_all_known() {
        for status in [
            Status::Open,
            Status::InProgress,
            Status::Blocked,
            Status::Deferred,
            Status::Draft,
            Status::Closed,
            Status::Tombstone,
            Status::Pinned,
        ] {
            let parsed = Status::from_str(status.as_str()).unwrap();
            assert_eq!(parsed, status);
        }
    }

    #[test]
    fn from_str_is_case_insensitive() {
        assert_eq!(Status::from_str("IN_PROGRESS").unwrap(), Status::InProgress);
        assert_eq!(Status::from_str("InProgress").unwrap(), Status::InProgress);
        assert_eq!(Status::from_str("OPEN").unwrap(), Status::Open);
    }

    #[test]
    fn unknown_becomes_custom_preserving_case() {
        assert_eq!(
            Status::from_str("QaReview").unwrap(),
            Status::Custom("QaReview".to_string())
        );
    }

    #[test]
    fn predicate_truth_table() {
        assert!(Status::Closed.is_terminal());
        assert!(Status::Tombstone.is_terminal());
        assert!(!Status::Open.is_terminal());

        assert!(Status::Open.is_active());
        assert!(Status::InProgress.is_active());
        assert!(!Status::Blocked.is_active());

        assert!(Status::Draft.is_draft());
        assert!(!Status::Open.is_draft());
    }

    #[test]
    fn serde_wire_string() {
        let json = serde_json::to_string(&Status::InProgress).unwrap();
        assert_eq!(json, "\"in_progress\"");

        let status: Status = serde_json::from_str("\"custom_status\"").unwrap();
        assert_eq!(status, Status::Custom("custom_status".to_string()));
        // Original case preserved on deserialize.
        let mixed: Status = serde_json::from_str("\"QaReview\"").unwrap();
        assert_eq!(mixed, Status::Custom("QaReview".to_string()));
        // Custom serializes back as its raw string.
        assert_eq!(serde_json::to_string(&mixed).unwrap(), "\"QaReview\"");
    }

    #[test]
    fn default_is_open() {
        assert_eq!(Status::default(), Status::Open);
    }
}
