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

    /// Restore this issue from a tombstone — the **pure inverse** of [`Issue::into_tombstone`] (spine
    /// §1.8, D20).
    ///
    /// No clock: like `into_tombstone`, this sets **no timestamp** — storage bumps `updated_at` and
    /// recomputes `content_hash` after calling it (mirroring how `tombstone_one` finishes the soft
    /// delete). It:
    /// - sets `status` **best-effort via `closed_at`** — `Status::Closed` when `closed_at.is_some()`,
    ///   else `Status::Open`. The pre-delete status is **not** preserved (only `original_type`
    ///   survives a tombstone); `closed_at` being set is the *only* signal the issue was Closed.
    ///   Open/Closed round-trip exactly; InProgress/Blocked/Deferred collapse to Open (lost, by
    ///   design).
    /// - leaves `issue_type` **UNTOUCHED** — `into_tombstone` only *snapshots* `original_type` from
    ///   the live `issue_type`, never mutates it, so the live value on a tombstone is already correct
    ///   (writing `original_type → issue_type` would corrupt imported rows whose serde-carried
    ///   `original_type` diverges from `issue_type`).
    /// - **clears `original_type` → `None`**, returning a clean active issue.
    /// - **clears the tombstone fields** — `deleted_at`/`deleted_by`/`delete_reason` → `None`.
    /// - **`closed_at`**: the Open branch ensures `None`; the Closed branch **keeps** it (it is both
    ///   the was-Closed signal and what satisfies the issues-table CHECK constraint for
    ///   `status='closed'`).
    ///
    /// Defensive: a non-tombstone is returned unchanged (storage guards before calling, but the
    /// helper stays total).
    #[must_use]
    pub fn restore_from_tombstone(mut self) -> Self {
        if !self.is_tombstone() {
            return self;
        }
        if self.closed_at.is_some() {
            self.status = Status::Closed;
            // Keep `closed_at` — the was-Closed signal AND the CHECK satisfier for status='closed'.
        } else {
            self.status = Status::Open;
            self.closed_at = None; // defensive — a tombstone's closed_at is already None.
        }
        self.original_type = None;
        self.deleted_at = None;
        self.deleted_by = None;
        self.delete_reason = None;
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
    fn restore_from_was_open_tombstone_lands_open_and_clears_fields() {
        // A was-Open issue tombstoned (no closed_at), then restored: Open, type preserved,
        // original_type + deleted_* cleared.
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let tomb = base().into_tombstone(Some("admin".to_string()), Some("dup".to_string()), now);
        assert!(tomb.is_tombstone());

        let restored = tomb.restore_from_tombstone();
        assert_eq!(restored.status, Status::Open);
        assert_eq!(restored.closed_at, None);
        assert_eq!(restored.original_type, None);
        assert_eq!(restored.deleted_at, None);
        assert_eq!(restored.deleted_by, None);
        assert_eq!(restored.delete_reason, None);
        // issue_type is UNTOUCHED by restore (and untouched by tombstone).
        assert_eq!(restored.issue_type, IssueType::Bug);
    }

    #[test]
    fn restore_from_was_closed_tombstone_lands_closed_and_keeps_closed_at() {
        // A was-Closed issue: closed_at set BEFORE deletion is the only was-Closed signal.
        let closed = Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let mut issue = base();
        issue.status = Status::Closed;
        issue.closed_at = Some(closed);
        let tomb = issue.into_tombstone(Some("admin".to_string()), None, now);
        // Tombstone keeps closed_at (into_tombstone never touches it).
        assert_eq!(tomb.closed_at, Some(closed));

        let restored = tomb.restore_from_tombstone();
        assert_eq!(restored.status, Status::Closed);
        assert_eq!(
            restored.closed_at,
            Some(closed),
            "closed_at preserved — the was-Closed signal AND the CHECK satisfier"
        );
        assert_eq!(restored.original_type, None);
        assert_eq!(restored.deleted_at, None);
        assert_eq!(restored.deleted_by, None);
        assert_eq!(restored.delete_reason, None);
        assert_eq!(restored.issue_type, IssueType::Bug);
    }

    #[test]
    fn restore_round_trip_preserves_issue_type_and_status_open() {
        // into_tombstone(...).restore_from_tombstone() round-trip for an originally-Open issue.
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let original = base();
        let original_type = original.issue_type.clone();

        let restored = original
            .into_tombstone(Some("x".to_string()), Some("r".to_string()), now)
            .restore_from_tombstone();
        assert_eq!(restored.status, Status::Open);
        assert_eq!(restored.issue_type, original_type);
        assert_eq!(restored.original_type, None);
        assert_eq!(restored.deleted_at, None);
        assert_eq!(restored.deleted_by, None);
        assert_eq!(restored.delete_reason, None);
    }

    #[test]
    fn restore_round_trip_preserves_status_closed() {
        // Originally-Closed round-trip: tombstone then restore returns Closed (via closed_at signal).
        let closed = Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let mut original = base();
        original.status = Status::Closed;
        original.closed_at = Some(closed);

        let restored = original
            .into_tombstone(Some("x".to_string()), None, now)
            .restore_from_tombstone();
        assert_eq!(restored.status, Status::Closed);
        assert_eq!(restored.closed_at, Some(closed));
        assert_eq!(restored.original_type, None);
    }

    #[test]
    fn restore_on_non_tombstone_is_unchanged() {
        // The helper is total: a non-tombstone is returned unchanged (storage guards before calling).
        let active = base();
        let restored = active.clone().restore_from_tombstone();
        assert_eq!(restored, active);
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
