//! Query-input contract types (CF-C, spine §1.10).
//!
//! Owned here so `unblock-policy` can fingerprint filters without depending on `unblock-storage`;
//! re-exported (never redefined) by `unblock-storage`/`unblock-engine`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::enums::{IssueType, Priority, Status};

/// Filters for list/ready/blocked/search/count/stale queries (spine §1.10).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct ListFilters {
    /// Status filter (OR within).
    pub status: Vec<Status>,
    /// Issue-type filter (OR within).
    pub issue_type: Vec<IssueType>,
    /// Assignee filter.
    pub assignee: Option<String>,
    /// Labels that must ALL be present (AND).
    pub labels_all: Vec<String>,
    /// Labels of which ANY may be present (OR).
    pub labels_any: Vec<String>,
    /// Minimum priority (inclusive).
    pub priority_min: Option<Priority>,
    /// Maximum priority (inclusive).
    pub priority_max: Option<Priority>,
    /// Free-text contains filter.
    pub text_contains: Option<String>,
    /// Include deferred issues.
    pub include_deferred: bool,
    /// Include closed issues.
    pub include_closed: bool,
    /// Include `status = 'tombstone'` (soft-deleted) rows. Default `false` — the default-visibility,
    /// `include_deferred`, and `include_closed` branches all EXCLUDE tombstones, so a caller must opt
    /// in explicitly. Set `true` ONLY by the `unblock-sync` full-corpus export (FORK-1/D23):
    /// tombstones must be exported so import-side tombstone-non-resurrection (FR-8, spine §1.8) is
    /// round-trippable. Orthogonal to `include_closed`: export sets BOTH `true`. list/ready/blocked/
    /// search/count/stale keep it `false` (agent query surfaces never set it).
    pub include_tombstone: bool,
    /// Result cap (`None` = unlimited; ready is default-complete).
    pub limit: Option<usize>,
    /// Result offset.
    pub offset: Option<usize>,
}

/// The grouping dimension for a count query (spine §1.10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CountGroupBy {
    /// Group by status.
    Status,
    /// Group by issue type.
    Type,
    /// Group by assignee.
    Assignee,
    /// Group by priority.
    Priority,
    /// Group by label.
    Label,
}

#[cfg(test)]
mod tests {
    use super::{CountGroupBy, ListFilters};

    #[test]
    fn default_is_empty_filter() {
        let f = ListFilters::default();
        assert!(f.status.is_empty());
        assert!(f.issue_type.is_empty());
        assert!(f.assignee.is_none());
        assert!(f.limit.is_none());
        assert!(!f.include_closed);
        // `include_tombstone` defaults to `false` via `#[derive(Default)]` (export-only opt-in, D23).
        assert!(!f.include_tombstone);
        // Stable empty filter feeds the policy fingerprint determinism.
        assert_eq!(f, ListFilters::default());
    }

    #[test]
    fn count_group_by_serde_snake_case() {
        assert_eq!(
            serde_json::to_string(&CountGroupBy::Type).unwrap(),
            "\"type\""
        );
        let g: CountGroupBy = serde_json::from_str("\"assignee\"").unwrap();
        assert_eq!(g, CountGroupBy::Assignee);
    }

    #[test]
    fn count_group_by_is_copy() {
        let g = CountGroupBy::Status;
        let copy = g; // `Copy`, so `g` is still usable below.
        assert_eq!(g, copy);
    }
}
