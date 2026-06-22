//! Panic-safety property tests for the untrusted-input chain (NFR-16).
//!
//! The cargo-fuzz ingestion targets landed at T0.7 (nightly/libFuzzer, excluded from the stable
//! workspace). These proptests close the coverage gap cheaply on stable: over arbitrary bytes /
//! arbitrary strings the full untrusted-input chain must NEVER panic and must stay well-formed
//! (`compute_content_hash` is always exactly 64 lowercase-hex chars).
//!
//! Parallel fuzz-side expression: `unblock_fuzz::run_content_hash_case`,
//! `run_issue_ingest_case`, `run_parse_id_case`, and `run_enum_deserialize_case` exercise the same
//! invariants under libFuzzer. `unblock-model` (L0) does **not** depend on `unblock-fuzz` (that
//! would invert the layering); these are deliberately independent mirrors.

use chrono::{TimeZone, Utc};
use proptest::prelude::*;
use unblock_model::{
    DependencyType, EventType, Issue, IssueType, IssueValidator, Priority, Status,
    is_valid_id_format, parse_id,
};

/// Assert the standard well-formedness + no-panic surface for any `Issue`, however obtained.
fn assert_issue_surface_well_formed(issue: &Issue) {
    // validate never panics (Ok or Err — either is fine here).
    let _ = IssueValidator::validate(issue);

    // content_hash is always exactly 64 lowercase hex chars.
    let hash = issue.compute_content_hash();
    assert_eq!(hash.len(), 64, "content_hash must be 64 chars: {hash:?}");
    assert!(
        hash.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "content_hash must be lowercase hex: {hash:?}"
    );

    // sync_equals against self must hold and not panic.
    assert!(issue.sync_equals(issue));

    // is_expired_tombstone never panics for any retention, including absurd ones.
    let _ = issue.is_expired_tombstone(None);
    let _ = issue.is_expired_tombstone(Some(0));
    let _ = issue.is_expired_tombstone(Some(1));
    let _ = issue.is_expired_tombstone(Some(u64::MAX));
}

/// A structurally-valid arbitrary issue (open enums include `Custom` arms with arbitrary strings).
fn arb_issue() -> impl Strategy<Value = Issue> {
    let arb_status = prop_oneof![
        Just(Status::Open),
        Just(Status::Closed),
        Just(Status::Tombstone),
        ".*".prop_map(Status::Custom),
    ];
    let arb_type = prop_oneof![
        Just(IssueType::Task),
        Just(IssueType::Epic),
        ".*".prop_map(IssueType::Custom),
    ];
    (
        ".*",
        ".*",
        prop::option::of(".*"),
        arb_status,
        arb_type,
        any::<i32>(),
        prop::option::of(any::<i32>()),
    )
        .prop_map(
            |(id, title, description, status, issue_type, prio, est)| Issue {
                id,
                title,
                description,
                status,
                issue_type,
                priority: Priority(prio),
                estimated_minutes: est,
                created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
                updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
                ..Issue::default()
            },
        )
}

proptest! {
    /// Arbitrary bytes through `from_slice::<Issue>`: Ok or Err, never a panic. Any Issue that DOES
    /// deserialize must then survive the full read-side surface without panicking.
    #[test]
    fn from_slice_issue_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        if let Ok(issue) = serde_json::from_slice::<Issue>(&bytes) {
            assert_issue_surface_well_formed(&issue);
        }
    }

    /// Same, but over arbitrary UTF-8 strings (exercises the JSON lexer's text path).
    #[test]
    fn from_str_issue_never_panics(s in ".*") {
        if let Ok(issue) = serde_json::from_str::<Issue>(&s) {
            assert_issue_surface_well_formed(&issue);
        }
    }

    /// Generated (structurally-valid) issues — incl. arbitrary `Custom` strings and any-`i32`
    /// priority/estimated_minutes — survive the full read-side surface without panicking.
    #[test]
    fn generated_issue_surface_never_panics(issue in arb_issue()) {
        assert_issue_surface_well_formed(&issue);
    }

    /// Id parsing/validation over arbitrary UTF-8 never panics (Ok or Err only).
    #[test]
    fn id_parsing_never_panics(s in ".*") {
        let _ = is_valid_id_format(&s);
        let _ = parse_id(&s);
        // The two must agree.
        prop_assert_eq!(is_valid_id_format(&s), parse_id(&s).is_ok());
    }

    /// Hand-rolled enum `Deserialize` over arbitrary strings never panics and round-trips its wire
    /// form (open enums fold any unknown string into `Custom`).
    #[test]
    fn enum_deserialize_never_panics(s in ".*") {
        let value = serde_json::Value::String(s);

        let status: Status = serde_json::from_value(value.clone()).unwrap();
        prop_assert_eq!(&status, &serde_json::from_value::<Status>(serde_json::to_value(&status).unwrap()).unwrap());

        let ty: IssueType = serde_json::from_value(value.clone()).unwrap();
        prop_assert_eq!(&ty, &serde_json::from_value::<IssueType>(serde_json::to_value(&ty).unwrap()).unwrap());

        let dep: DependencyType = serde_json::from_value(value.clone()).unwrap();
        prop_assert_eq!(&dep, &serde_json::from_value::<DependencyType>(serde_json::to_value(&dep).unwrap()).unwrap());

        let ev: EventType = serde_json::from_value(value).unwrap();
        prop_assert_eq!(&ev, &serde_json::from_value::<EventType>(serde_json::to_value(&ev).unwrap()).unwrap());
    }
}
