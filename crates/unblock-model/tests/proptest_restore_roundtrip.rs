//! Round-trip property test for the tombstone delete/restore inverse (spine §1.8, D20).
//!
//! `Issue::restore_from_tombstone` is the pure inverse of `Issue::into_tombstone`. Over arbitrary
//! issues (any starting status, any `issue_type` including `Custom`, any `closed_at`):
//! `issue.into_tombstone(by, reason, now).restore_from_tombstone()` must land:
//! - `status == Closed` iff the issue carried a `closed_at` before deletion, else `Open` (best-effort
//!   via `closed_at` — D20 DECISION 1);
//! - `issue_type` **unchanged** (restore never touches it — D20 DECISION 3);
//! - `original_type == None` and every tombstone field (`deleted_at`/`deleted_by`/`delete_reason`)
//!   cleared;
//! - `closed_at` preserved on the Closed branch, `None` on the Open branch (the CHECK-constraint
//!   satisfier).

use chrono::{TimeZone, Utc};
use proptest::prelude::*;
use unblock_model::{Issue, IssueType, Status};

/// An arbitrary issue spanning the pre-delete states restore must round-trip (open enums include a
/// `Custom` arm with arbitrary strings; `closed_at` is present/absent to exercise both branches).
fn arb_issue() -> impl Strategy<Value = Issue> {
    let arb_status = prop_oneof![
        Just(Status::Open),
        Just(Status::InProgress),
        Just(Status::Blocked),
        Just(Status::Deferred),
        Just(Status::Closed),
        ".*".prop_map(Status::Custom),
    ];
    let arb_type = prop_oneof![
        Just(IssueType::Task),
        Just(IssueType::Bug),
        Just(IssueType::Epic),
        ".*".prop_map(IssueType::Custom),
    ];
    (
        ".*",                       // id
        ".*",                       // title
        arb_status,                 // pre-delete status
        arb_type,                   // issue_type (must be preserved across the round-trip)
        prop::option::of(Just(())), // whether closed_at is present
        prop::option::of(".*"),     // pre-existing original_type (cleared on restore)
    )
        .prop_map(
            |(id, title, status, issue_type, has_closed, original_type)| {
                let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
                Issue {
                    id,
                    title,
                    status,
                    issue_type,
                    original_type,
                    closed_at: has_closed
                        .map(|()| Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap()),
                    created_at: base,
                    updated_at: base,
                    ..Issue::default()
                }
            },
        )
}

proptest! {
    /// `into_tombstone(...).restore_from_tombstone()` round-trips status (via the `closed_at` signal),
    /// preserves `issue_type`, and clears `original_type` + every tombstone field.
    #[test]
    fn tombstone_then_restore_round_trips(issue in arb_issue()) {
        let issue_type = issue.issue_type.clone();
        // The was-Closed signal is `closed_at` being set on the pre-delete issue (into_tombstone never
        // touches closed_at, so the tombstone carries the same closed_at).
        let was_closed_signal = issue.closed_at;

        let now = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let restored = issue
            .into_tombstone(Some("by".to_string()), Some("reason".to_string()), now)
            .restore_from_tombstone();

        prop_assert!(!restored.is_tombstone());
        if was_closed_signal.is_some() {
            prop_assert_eq!(&restored.status, &Status::Closed);
            prop_assert_eq!(restored.closed_at, was_closed_signal);
        } else {
            prop_assert_eq!(&restored.status, &Status::Open);
            prop_assert_eq!(restored.closed_at, None);
        }

        prop_assert_eq!(&restored.issue_type, &issue_type, "issue_type must be untouched");
        prop_assert_eq!(restored.original_type.as_deref(), None);
        prop_assert_eq!(restored.deleted_at, None);
        prop_assert_eq!(restored.deleted_by.as_deref(), None);
        prop_assert_eq!(restored.delete_reason.as_deref(), None);
    }
}
