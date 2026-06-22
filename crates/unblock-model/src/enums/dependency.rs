//! [`DependencyType`] — relationship kind between two issues (spine §1.4).

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use unblock_error::ModelError;

/// Dependency relationship type (spine §1.4).
///
/// An **open enum** with a kebab-case wire form. Unlike [`crate::Status`] / [`crate::IssueType`],
/// an unrecognised value is **lowercased** before being stored in [`DependencyType::Custom`]
/// (asymmetry ported verbatim from the original). The four gating types — `Blocks`,
/// `ParentChild`, `ConditionalBlocks`, `WaitsFor` — are the set that affects ready-work.
///
/// # Examples
///
/// ```
/// use unblock_model::DependencyType;
/// use std::str::FromStr;
///
/// assert_eq!(DependencyType::ParentChild.as_str(), "parent-child");
/// assert!(DependencyType::Blocks.affects_ready_work());
/// assert!(!DependencyType::Related.affects_ready_work());
/// assert_eq!(
///     DependencyType::from_str("Mentions").unwrap(),
///     DependencyType::Custom("mentions".to_string()),
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DependencyType {
    /// The source is blocked by the target (gating).
    Blocks,
    /// The source is a child of the target (gating).
    ParentChild,
    /// The source is conditionally blocked by the target (gating).
    ConditionalBlocks,
    /// The source waits for the target (gating).
    WaitsFor,
    /// A loose relation.
    Related,
    /// The source was discovered while working the target (the agent flywheel edge).
    DiscoveredFrom,
    /// The source replies to the target.
    RepliesTo,
    /// The source relates to the target.
    RelatesTo,
    /// The source duplicates the target.
    Duplicates,
    /// The source supersedes the target.
    Supersedes,
    /// The source was caused by the target.
    CausedBy,
    /// An open-enum tail variant for any unrecognised type (stored **lowercased**).
    Custom(String),
}

impl DependencyType {
    /// The stable kebab-case wire string for this type.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Blocks => "blocks",
            Self::ParentChild => "parent-child",
            Self::ConditionalBlocks => "conditional-blocks",
            Self::WaitsFor => "waits-for",
            Self::Related => "related",
            Self::DiscoveredFrom => "discovered-from",
            Self::RepliesTo => "replies-to",
            Self::RelatesTo => "relates-to",
            Self::Duplicates => "duplicates",
            Self::Supersedes => "supersedes",
            Self::CausedBy => "caused-by",
            Self::Custom(value) => value,
        }
    }

    /// Parse a (already case-folded) kebab-case value into a known variant, lowercasing the input
    /// before matching and storing any unrecognised value lowercased in [`DependencyType::Custom`].
    fn parse_lowercased(value: &str) -> Self {
        match value.to_lowercase().as_str() {
            "blocks" => Self::Blocks,
            "parent-child" => Self::ParentChild,
            "conditional-blocks" => Self::ConditionalBlocks,
            "waits-for" => Self::WaitsFor,
            "related" => Self::Related,
            "discovered-from" => Self::DiscoveredFrom,
            "replies-to" => Self::RepliesTo,
            "relates-to" => Self::RelatesTo,
            "duplicates" => Self::Duplicates,
            "supersedes" => Self::Supersedes,
            "caused-by" => Self::CausedBy,
            lowered => Self::Custom(lowered.to_string()),
        }
    }

    /// Whether this type gates ready-work (`Blocks` / `ParentChild` / `ConditionalBlocks` /
    /// `WaitsFor`).
    #[must_use]
    pub const fn affects_ready_work(&self) -> bool {
        matches!(
            self,
            Self::Blocks | Self::ParentChild | Self::ConditionalBlocks | Self::WaitsFor
        )
    }

    /// Whether this type is blocking — the same set as [`DependencyType::affects_ready_work`].
    #[must_use]
    pub const fn is_blocking(&self) -> bool {
        matches!(
            self,
            Self::Blocks | Self::ParentChild | Self::ConditionalBlocks | Self::WaitsFor
        )
    }
}

impl fmt::Display for DependencyType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DependencyType {
    type Err = ModelError;

    /// Parse a dependency-type string. **Infallible** for an open enum: an unknown string becomes
    /// [`DependencyType::Custom`] (lowercased). The `Err` type is fixed by the spine.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::parse_lowercased(s))
    }
}

impl Serialize for DependencyType {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for DependencyType {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Ok(Self::parse_lowercased(&value))
    }
}

impl JsonSchema for DependencyType {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("DependencyType")
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        generator.subschema_for::<String>()
    }
}

#[cfg(test)]
mod tests {
    use super::DependencyType;
    use std::str::FromStr;

    const ALL_KNOWN: [DependencyType; 11] = [
        DependencyType::Blocks,
        DependencyType::ParentChild,
        DependencyType::ConditionalBlocks,
        DependencyType::WaitsFor,
        DependencyType::Related,
        DependencyType::DiscoveredFrom,
        DependencyType::RepliesTo,
        DependencyType::RelatesTo,
        DependencyType::Duplicates,
        DependencyType::Supersedes,
        DependencyType::CausedBy,
    ];

    #[test]
    fn kebab_roundtrip_all_known() {
        for dep in ALL_KNOWN {
            assert_eq!(DependencyType::from_str(dep.as_str()).unwrap(), dep);
        }
        assert_eq!(DependencyType::ParentChild.as_str(), "parent-child");
    }

    #[test]
    fn gating_predicates_agree_and_cover_exactly_four() {
        let gating = [
            DependencyType::Blocks,
            DependencyType::ParentChild,
            DependencyType::ConditionalBlocks,
            DependencyType::WaitsFor,
        ];
        for dep in ALL_KNOWN {
            assert_eq!(dep.affects_ready_work(), dep.is_blocking());
            assert_eq!(dep.affects_ready_work(), gating.contains(&dep));
        }
        assert!(!DependencyType::DiscoveredFrom.affects_ready_work());
    }

    #[test]
    fn unknown_becomes_custom_lowercased() {
        assert_eq!(
            DependencyType::from_str("Mentions").unwrap(),
            DependencyType::Custom("mentions".to_string())
        );
        let dep: DependencyType = serde_json::from_str("\"Mentions\"").unwrap();
        assert_eq!(dep, DependencyType::Custom("mentions".to_string()));
    }
}
