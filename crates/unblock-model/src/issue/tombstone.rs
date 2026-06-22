//! Tombstone delete semantics and TTL (spine §1.8).
//!
//! The **no-resurrection invariant** — "import NEVER resurrects a tombstone" — is enforced at the
//! sync layer (`unblock-sync`); this module owns only the pure predicates and the constructor it
//! relies on. A non-tombstone JSONL line for an id that is tombstoned in the DB is rejected/skipped
//! by sync, not applied.

use chrono::{DateTime, Duration, Utc};

use super::Issue;
use crate::enums::Status;

/// Clamp ceiling for tombstone retention, in days (~1000 years).
///
/// `chrono::Duration::days` can hold far more, but an issue-tracker TTL never legitimately exceeds
/// this; clamping here keeps [`Issue::is_expired_tombstone`] panic-free for absurd inputs without
/// a bare `unwrap` on the `u64 -> i64` conversion.
pub const MAX_SAFE_TOMBSTONE_DAYS: u64 = 365 * 1000;

impl Issue {
    /// Whether this issue is a tombstone (`status == Status::Tombstone`).
    #[must_use]
    pub fn is_tombstone(&self) -> bool {
        self.status == Status::Tombstone
    }

    /// Transition this issue into a tombstone (net-new; spine §1.8).
    ///
    /// Sets `status = Status::Tombstone` and `deleted_at = Some(now)` (the clock is **injected** —
    /// this stays pure, no `Utc::now()` inside), records `deleted_by`/`delete_reason`, and captures
    /// `original_type` from the current `issue_type` **only when it was `None`** (an already-set
    /// `original_type` is preserved). `created_at` / `updated_at` are **not** touched.
    #[must_use]
    pub fn into_tombstone(
        mut self,
        deleted_by: Option<String>,
        reason: Option<String>,
        now: DateTime<Utc>,
    ) -> Self {
        if self.original_type.is_none() {
            self.original_type = Some(self.issue_type.as_str().to_string());
        }
        self.status = Status::Tombstone;
        self.deleted_at = Some(now);
        self.deleted_by = deleted_by;
        self.delete_reason = reason;
        self
    }

    /// Whether this tombstone has exceeded its TTL (spine §1.8).
    ///
    /// Returns `false` for a non-tombstone, for `retention_days` of `None` or `0`, or when
    /// `deleted_at` is unknown. The retention window is clamped at [`MAX_SAFE_TOMBSTONE_DAYS`].
    ///
    /// **Panic-free for any input.** Both the `u64 → i64` clamp *and* the `deleted_at + retention`
    /// calendar addition are overflow-safe: the addition uses `checked_add_signed`, and an overflow
    /// (a `deleted_at` so near [`chrono::DateTime::<Utc>::MAX_UTC`] that adding the retention escapes
    /// the calendar) yields `false` — by definition such an expiry is astronomically far in the
    /// future, so the tombstone is **not yet** expired.
    #[must_use]
    pub fn is_expired_tombstone(&self, retention_days: Option<u64>) -> bool {
        if self.status != Status::Tombstone {
            return false;
        }

        let Some(days) = retention_days else {
            return false;
        };

        if days == 0 {
            return false;
        }

        let Some(deleted_at) = self.deleted_at else {
            return false;
        };

        let clamped = days.min(MAX_SAFE_TOMBSTONE_DAYS);
        // `clamped <= MAX_SAFE_TOMBSTONE_DAYS` always fits i64; the fallback is defensive only.
        let days_i64 = i64::try_from(clamped)
            .unwrap_or_else(|_| i64::try_from(MAX_SAFE_TOMBSTONE_DAYS).unwrap_or(i64::MAX));

        // `deleted_at + Duration::days(..)` can overflow the calendar for a near-MAX `deleted_at`;
        // `checked_add_signed` returns `None` instead of panicking. An expiry that overflows the
        // calendar is unreachably far in the future -> treat the tombstone as not yet expired.
        match deleted_at.checked_add_signed(Duration::days(days_i64)) {
            Some(expiration) => Utc::now() > expiration,
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MAX_SAFE_TOMBSTONE_DAYS;
    use crate::enums::{IssueType, Status};
    use crate::issue::Issue;
    use chrono::{DateTime, Duration, TimeZone, Utc};

    fn base() -> Issue {
        Issue {
            id: "ub-abc123".to_string(),
            title: "Test".to_string(),
            issue_type: IssueType::Bug,
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            ..Issue::default()
        }
    }

    #[test]
    fn into_tombstone_sets_fields_and_captures_type() {
        let issue = base();
        let created = issue.created_at;
        let updated = issue.updated_at;
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();

        let tomb = issue.into_tombstone(Some("admin".to_string()), Some("dup".to_string()), now);
        assert_eq!(tomb.status, Status::Tombstone);
        assert_eq!(tomb.deleted_at, Some(now));
        assert_eq!(tomb.deleted_by.as_deref(), Some("admin"));
        assert_eq!(tomb.delete_reason.as_deref(), Some("dup"));
        assert_eq!(tomb.original_type.as_deref(), Some("bug"));
        // created_at / updated_at untouched.
        assert_eq!(tomb.created_at, created);
        assert_eq!(tomb.updated_at, updated);
        assert!(tomb.is_tombstone());
    }

    #[test]
    fn into_tombstone_preserves_existing_original_type() {
        let mut issue = base();
        issue.original_type = Some("feature".to_string());
        let tomb = issue.into_tombstone(None, None, Utc::now());
        assert_eq!(tomb.original_type.as_deref(), Some("feature"));
    }

    #[test]
    fn non_tombstone_never_expired() {
        assert!(!base().is_expired_tombstone(Some(1)));
    }

    #[test]
    fn none_or_zero_retention_never_expired() {
        let tomb = base().into_tombstone(None, None, Utc::now());
        assert!(!tomb.is_expired_tombstone(None));
        assert!(!tomb.is_expired_tombstone(Some(0)));
    }

    #[test]
    fn missing_deleted_at_never_expired() {
        let mut issue = base();
        issue.status = Status::Tombstone;
        issue.deleted_at = None;
        assert!(!issue.is_expired_tombstone(Some(1)));
    }

    #[test]
    fn expired_vs_not_yet_around_boundary() {
        let mut issue = base();
        issue.status = Status::Tombstone;
        // Deleted 100 days ago.
        issue.deleted_at = Some(Utc::now() - Duration::days(100));
        assert!(issue.is_expired_tombstone(Some(30)));
        assert!(!issue.is_expired_tombstone(Some(365)));
    }

    #[test]
    fn huge_retention_does_not_panic() {
        let mut issue = base();
        issue.status = Status::Tombstone;
        issue.deleted_at = Some(Utc::now() - Duration::days(1));
        // u64::MAX clamps to MAX_SAFE_TOMBSTONE_DAYS — no panic, not expired.
        assert!(!issue.is_expired_tombstone(Some(u64::MAX)));
        assert!(!issue.is_expired_tombstone(Some(MAX_SAFE_TOMBSTONE_DAYS)));
    }

    #[test]
    fn near_max_deleted_at_with_large_retention_does_not_panic() {
        // A `deleted_at` near the calendar ceiling: `deleted_at + retention` overflows
        // `DateTime<Utc>`. The old `deleted_at + Duration::days(..)` would panic here; the
        // `checked_add_signed` path returns `None` -> not yet expired (no panic).
        let mut issue = base();
        issue.status = Status::Tombstone;
        issue.deleted_at = Some(DateTime::<Utc>::MAX_UTC);
        assert!(!issue.is_expired_tombstone(Some(MAX_SAFE_TOMBSTONE_DAYS)));
        assert!(!issue.is_expired_tombstone(Some(u64::MAX)));
        assert!(!issue.is_expired_tombstone(Some(1)));
    }
}
