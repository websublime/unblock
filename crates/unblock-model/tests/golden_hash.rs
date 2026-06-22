//! Golden snapshot locking the byte-compat of `content_hash` against a fixed canonical issue.
//!
//! FR-26 `bd` import idempotency depends on this hash being stable byte-for-byte, including the
//! frozen 17-field Go-bd zero-value padding tail (spine §1.8, Q4 = KEEP — cross-refs the ported
//! `temp/beads_rust-main/src/util/hash.rs` L94–129). A diff here means the canonical byte stream
//! changed and `bd` re-import dedup would break — re-bless only on a deliberate spine amendment.

use chrono::{TimeZone, Utc};
use unblock_model::{Issue, IssueType, Priority, Status};

/// A canonical issue with every hash-INCLUDED field set to a known, non-default value.
fn canonical_issue() -> Issue {
    Issue {
        id: "ub-canonical".to_string(),
        title: "Canonical title".to_string(),
        description: Some("Canonical description".to_string()),
        design: Some("Canonical design".to_string()),
        acceptance_criteria: Some("Canonical acceptance".to_string()),
        notes: Some("Canonical notes".to_string()),
        status: Status::InProgress,
        priority: Priority::HIGH,
        issue_type: IssueType::Bug,
        assignee: Some("alice".to_string()),
        owner: Some("bob".to_string()),
        created_by: Some("carol".to_string()),
        external_ref: Some("JIRA-123".to_string()),
        source_system: Some("jira".to_string()),
        pinned: true,
        is_template: true,
        // Hash-EXCLUDED fields below — present to prove they do not affect the hash.
        estimated_minutes: Some(42),
        created_at: Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap(),
        updated_at: Utc.with_ymd_and_hms(2026, 6, 7, 8, 9, 10).unwrap(),
        labels: vec!["x".to_string(), "y".to_string()],
        ..Issue::default()
    }
}

#[test]
fn golden_content_hash() {
    let hash = canonical_issue().compute_content_hash();
    assert_eq!(hash.len(), 64, "SHA-256 hex must be 64 chars");
    insta::assert_snapshot!("canonical_content_hash", hash);
}

#[test]
fn golden_hash_invariant_to_volatile_timestamps() {
    let base = canonical_issue().compute_content_hash();

    let mut moved = canonical_issue();
    moved.created_at = Utc.with_ymd_and_hms(1999, 1, 1, 0, 0, 0).unwrap();
    moved.updated_at = Utc.with_ymd_and_hms(2100, 12, 31, 23, 59, 59).unwrap();
    assert_eq!(base, moved.compute_content_hash());
}
