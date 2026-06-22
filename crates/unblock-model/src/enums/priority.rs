//! [`Priority`] — issue priority newtype (spine §1.2).

use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use unblock_error::ModelError;

/// Issue priority (spine §1.2): a transparent newtype over `i32`, valid range `0..=4`.
///
/// Ordering is numeric, so `CRITICAL < HIGH < MEDIUM < LOW < BACKLOG` — this is the order the
/// hybrid ready-sort relies on. `Default` is [`Priority::MEDIUM`]. The wire form is the bare
/// integer (`#[serde(transparent)]`), and `Display` renders `"P{n}"`.
///
/// # Examples
///
/// ```
/// use unblock_model::Priority;
/// use std::str::FromStr;
///
/// assert_eq!(Priority::default(), Priority::MEDIUM);
/// assert_eq!(Priority::CRITICAL.to_string(), "P0");
/// assert_eq!(Priority::from_str("p1").unwrap(), Priority::HIGH);
/// assert!(Priority::from_str("P5").is_err());
/// assert!(Priority::CRITICAL < Priority::BACKLOG);
/// ```
#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema,
)]
#[serde(transparent)]
pub struct Priority(pub i32);

impl Priority {
    /// Highest priority (`0`).
    pub const CRITICAL: Self = Self(0);
    /// High priority (`1`).
    pub const HIGH: Self = Self(1);
    /// Medium priority (`2`) — the default.
    pub const MEDIUM: Self = Self(2);
    /// Low priority (`3`).
    pub const LOW: Self = Self(3);
    /// Lowest priority (`4`).
    pub const BACKLOG: Self = Self(4);
}

impl Default for Priority {
    fn default() -> Self {
        Self::MEDIUM
    }
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "P{}", self.0)
    }
}

impl FromStr for Priority {
    type Err = ModelError;

    /// Parse `"P0".."P4"` or `"0".."4"` (case-insensitive). An out-of-range or non-numeric value
    /// yields [`ModelError::InvalidPriority`].
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let upper = s.trim().to_uppercase();
        let val = upper.strip_prefix('P').unwrap_or(&upper);

        match val.parse::<i32>() {
            Ok(p) if (0..=4).contains(&p) => Ok(Self(p)),
            Ok(p) => Err(ModelError::InvalidPriority {
                value: p.to_string(),
            }),
            Err(_) => Err(ModelError::InvalidPriority {
                value: val.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Priority;
    use std::str::FromStr;

    #[test]
    fn const_values() {
        assert_eq!(Priority::CRITICAL, Priority(0));
        assert_eq!(Priority::HIGH, Priority(1));
        assert_eq!(Priority::MEDIUM, Priority(2));
        assert_eq!(Priority::LOW, Priority(3));
        assert_eq!(Priority::BACKLOG, Priority(4));
    }

    #[test]
    fn default_is_medium() {
        assert_eq!(Priority::default(), Priority::MEDIUM);
    }

    #[test]
    fn display_renders_p_prefix() {
        assert_eq!(Priority::MEDIUM.to_string(), "P2");
        assert_eq!(Priority::CRITICAL.to_string(), "P0");
    }

    #[test]
    fn parse_valid_forms() {
        assert_eq!(Priority::from_str("p0").unwrap(), Priority::CRITICAL);
        assert_eq!(Priority::from_str("P4").unwrap(), Priority::BACKLOG);
        assert_eq!(Priority::from_str("4").unwrap(), Priority::BACKLOG);
        assert_eq!(Priority::from_str("  2  ").unwrap(), Priority::MEDIUM);
    }

    #[test]
    fn parse_rejects_invalid() {
        assert!(Priority::from_str("P5").is_err());
        assert!(Priority::from_str("-1").is_err());
        assert!(Priority::from_str("x").is_err());
    }

    #[test]
    fn ordering_is_numeric() {
        assert!(Priority::CRITICAL < Priority::HIGH);
        assert!(Priority::HIGH < Priority::MEDIUM);
        assert!(Priority::MEDIUM < Priority::LOW);
        assert!(Priority::LOW < Priority::BACKLOG);
    }

    #[test]
    fn transparent_serde() {
        assert_eq!(serde_json::to_string(&Priority(2)).unwrap(), "2");
        let p: Priority = serde_json::from_str("2").unwrap();
        assert_eq!(p, Priority(2));
    }
}
