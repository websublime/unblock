//! The workspace health severity ladder ([`HealthLevel`]), defined by repair path (PRD §4 D58).
//!
//! `Ord` is severity order (`Healthy < Drifted < Recoverable < Unsafe`), so the composite level over a
//! set of anomalies is their `max`.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The four-state workspace health ladder (PRD §7/§12.4; D58 clause (1)).
///
/// Variant order is severity order, so `max` yields the worst level in a set.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum HealthLevel {
    /// No anomaly.
    Healthy,
    /// Derived state inside the database disagrees with the base rows it derives from; the
    /// workspace stays operable and an online recompute, dedupe or migrate fixes it.
    Drifted,
    /// Damage or an inconsistent on-disk state for which an in-product repair exists.
    Recoverable,
    /// No in-product repair exists; the report names the manual path.
    Unsafe,
}

impl HealthLevel {
    /// The stable snake-case label (`"healthy"`/`"drifted"`/`"recoverable"`/`"unsafe"`).
    ///
    /// This is the single source of the string form; [`Display`](Self::fmt) and the serde
    /// representation both agree with it (contract stability, NFR-14).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Drifted => "drifted",
            Self::Recoverable => "recoverable",
            Self::Unsafe => "unsafe",
        }
    }

    /// Whether the workspace can still be operated on (`Healthy` or `Drifted`).
    #[must_use]
    pub const fn is_operable(self) -> bool {
        matches!(self, Self::Healthy | Self::Drifted)
    }

    /// Whether the workspace needs recovery (`Recoverable`).
    #[must_use]
    pub const fn needs_recovery(self) -> bool {
        matches!(self, Self::Recoverable)
    }

    /// Whether the workspace is in a fatal state (`Unsafe`).
    #[must_use]
    pub const fn is_fatal(self) -> bool {
        matches!(self, Self::Unsafe)
    }
}

impl fmt::Display for HealthLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::HealthLevel;

    #[test]
    fn as_str_covers_every_variant() {
        assert_eq!(HealthLevel::Healthy.as_str(), "healthy");
        assert_eq!(HealthLevel::Drifted.as_str(), "drifted");
        assert_eq!(HealthLevel::Recoverable.as_str(), "recoverable");
        assert_eq!(HealthLevel::Unsafe.as_str(), "unsafe");
    }

    #[test]
    fn display_matches_as_str() {
        for level in [
            HealthLevel::Healthy,
            HealthLevel::Drifted,
            HealthLevel::Recoverable,
            HealthLevel::Unsafe,
        ] {
            assert_eq!(level.to_string(), level.as_str());
        }
    }

    #[test]
    fn ord_is_severity_order() {
        assert!(HealthLevel::Healthy < HealthLevel::Drifted);
        assert!(HealthLevel::Drifted < HealthLevel::Recoverable);
        assert!(HealthLevel::Recoverable < HealthLevel::Unsafe);
    }

    #[test]
    fn max_of_a_set_is_the_worst() {
        let set = [
            HealthLevel::Healthy,
            HealthLevel::Unsafe,
            HealthLevel::Recoverable,
        ];
        assert_eq!(set.into_iter().max(), Some(HealthLevel::Unsafe));
    }

    #[test]
    fn predicates_are_disjoint_and_correct() {
        assert!(HealthLevel::Healthy.is_operable());
        assert!(HealthLevel::Drifted.is_operable());
        assert!(!HealthLevel::Recoverable.is_operable());
        assert!(HealthLevel::Recoverable.needs_recovery());
        assert!(HealthLevel::Unsafe.is_fatal());
        assert!(!HealthLevel::Unsafe.needs_recovery());
    }

    #[test]
    fn serde_json_pins_snake_case_strings() {
        insta::assert_json_snapshot!(
            "health_level_variants",
            [
                HealthLevel::Healthy,
                HealthLevel::Drifted,
                HealthLevel::Recoverable,
                HealthLevel::Unsafe,
            ]
        );
    }
}
