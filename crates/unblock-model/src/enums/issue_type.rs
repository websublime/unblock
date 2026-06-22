//! [`IssueType`] — issue category (spine §1.3).

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use unblock_error::ModelError;

/// Issue type category (spine §1.3).
///
/// An **open enum** like [`crate::Status`]: an unrecognised string deserializes into
/// [`IssueType::Custom`] (preserving its original case). `Serialize`/`Deserialize`/`JsonSchema`
/// are hand-rolled (string). `epic` participates in epic-status rollups (v1.1).
///
/// # Examples
///
/// ```
/// use unblock_model::IssueType;
/// use std::str::FromStr;
///
/// assert_eq!(IssueType::Bug.as_str(), "bug");
/// assert!(IssueType::Task.is_standard());
/// assert_eq!(IssueType::from_str("Odd_Type").unwrap(), IssueType::Custom("Odd_Type".to_string()));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum IssueType {
    /// A unit of work (the default).
    #[default]
    Task,
    /// A defect.
    Bug,
    /// A new capability.
    Feature,
    /// An epic that aggregates child issues.
    Epic,
    /// A maintenance chore.
    Chore,
    /// Documentation work.
    Docs,
    /// An open question.
    Question,
    /// An open-enum tail variant for any unrecognised type (original case preserved).
    Custom(String),
}

impl IssueType {
    /// Map a string to a known variant (case-insensitive), or `None` if unrecognised.
    fn known_value(value: &str) -> Option<Self> {
        Some(match value.to_lowercase().as_str() {
            "task" => Self::Task,
            "bug" => Self::Bug,
            "feature" => Self::Feature,
            "epic" => Self::Epic,
            "chore" => Self::Chore,
            "docs" => Self::Docs,
            "question" => Self::Question,
            _ => return None,
        })
    }

    /// The stable wire string for this type (`snake_case` for known variants).
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Task => "task",
            Self::Bug => "bug",
            Self::Feature => "feature",
            Self::Epic => "epic",
            Self::Chore => "chore",
            Self::Docs => "docs",
            Self::Question => "question",
            Self::Custom(value) => value,
        }
    }

    /// Whether this is a standard (non-`Custom`) issue type.
    #[must_use]
    pub const fn is_standard(&self) -> bool {
        !matches!(self, Self::Custom(_))
    }
}

impl fmt::Display for IssueType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for IssueType {
    type Err = ModelError;

    /// Parse an issue-type string. **Infallible** for an open enum: an unknown string becomes
    /// [`IssueType::Custom`]. The `Err` type is fixed by the spine; this never returns `Err`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::known_value(s).unwrap_or_else(|| Self::Custom(s.to_string())))
    }
}

impl Serialize for IssueType {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for IssueType {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Ok(Self::known_value(&value).unwrap_or(Self::Custom(value)))
    }
}

impl JsonSchema for IssueType {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("IssueType")
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        generator.subschema_for::<String>()
    }
}

#[cfg(test)]
mod tests {
    use super::IssueType;
    use std::str::FromStr;

    #[test]
    fn as_str_from_str_roundtrip_all_standard() {
        for ty in [
            IssueType::Task,
            IssueType::Bug,
            IssueType::Feature,
            IssueType::Epic,
            IssueType::Chore,
            IssueType::Docs,
            IssueType::Question,
        ] {
            assert_eq!(IssueType::from_str(ty.as_str()).unwrap(), ty);
            assert!(ty.is_standard());
        }
    }

    #[test]
    fn unknown_becomes_custom_preserving_case() {
        let ty = IssueType::from_str("Odd_Type").unwrap();
        assert_eq!(ty, IssueType::Custom("Odd_Type".to_string()));
        assert!(!ty.is_standard());
    }

    #[test]
    fn epic_variant_present() {
        assert_eq!(IssueType::from_str("epic").unwrap(), IssueType::Epic);
    }

    #[test]
    fn serde_snake_case_and_custom() {
        assert_eq!(serde_json::to_string(&IssueType::Bug).unwrap(), "\"bug\"");
        let ty: IssueType = serde_json::from_str("\"Odd_Type\"").unwrap();
        assert_eq!(ty, IssueType::Custom("Odd_Type".to_string()));
        assert_eq!(serde_json::to_string(&ty).unwrap(), "\"Odd_Type\"");
    }

    #[test]
    fn default_is_task() {
        assert_eq!(IssueType::default(), IssueType::Task);
    }
}
