//! Export-determinism golden (NFR-14, MF-8): a fixed corpus — including an issue whose nested
//! `dependencies[].created_at` carries SUB-SECOND precision, and (D37) one whose nested
//! `comments[].updated_at`/`redacted_at` do — serializes to canonical `…Z` bytes. This pins the
//! recursive timestamp canonicalizer (MF-6) as non-vacuous.
//!
//! **Sub-second precision is deliberate HERE and nowhere else** (D37): `canonicalize_ts_in_value`
//! recurses field-name-blind, so only a sub-second nested value proves it actually reaches the new
//! comment fields. The `sync_equals`-compared fixtures (`tests/contract.rs`,
//! `tests/roundtrip_proptest.rs`) are SECOND-truncated instead — `serialize_issue_line` renders at
//! second precision and FORK-M2 puts `redacted_at` INTO the comparator, so a sub-second
//! `redacted_at` there would fail the round-trip identity.

use chrono::{TimeZone, Utc};
use unblock_model::{Comment, Dependency, DependencyType, Issue, Status};
use unblock_sync::serialize_issue_line;

fn base(id: &str) -> Issue {
    Issue {
        id: id.to_string(),
        title: format!("issue {id}"),
        status: Status::Open,
        created_at: Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap(),
        updated_at: Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap(),
        ..Issue::default()
    }
}

#[test]
fn export_bytes_are_canonical_and_deterministic() {
    // ub-1: plain. ub-2: a nested dependency whose created_at has SUB-SECOND precision. ub-3: plain.
    let mut ub2 = base("ub-2");
    let sub = Utc.timestamp_opt(1_767_326_645, 123_456_789).unwrap(); // sub-second nested ts.
    ub2.dependencies = vec![Dependency {
        issue_id: "ub-2".to_string(),
        depends_on_id: "ub-1".to_string(),
        dep_type: DependencyType::Blocks,
        created_at: sub,
        created_by: None,
        metadata: None,
        thread_id: None,
    }];

    // ub-4 (D37): comments carrying SUB-SECOND updated_at/redacted_at — a LIVE one and a REDACTED
    // one. Without this the export golden could not diff on comments at all, so a re-bless would
    // certify ZERO comment coverage.
    let mut ub4 = base("ub-4");
    let sub_edit = Utc.timestamp_opt(1_767_326_646, 987_654_321).unwrap();
    let sub_redact = Utc.timestamp_opt(1_767_326_647, 111_222_333).unwrap();
    ub4.comments = vec![
        Comment {
            id: 1,
            issue_id: "ub-4".to_string(),
            author: "alice".to_string(),
            body: "a live, edited comment".to_string(),
            created_at: Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap(),
            updated_at: Some(sub_edit),
            redacted_at: None,
        },
        Comment {
            id: 2,
            issue_id: "ub-4".to_string(),
            author: "bob".to_string(),
            // The redact wire form: body masked to "" + redacted_at present.
            body: String::new(),
            created_at: Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 6).unwrap(),
            updated_at: None,
            redacted_at: Some(sub_redact),
        },
    ];

    let corpus = [base("ub-1"), ub2, base("ub-3"), ub4];
    let lines: Vec<String> = corpus
        .iter()
        .map(|i| serialize_issue_line(i).expect("serialize"))
        .collect();
    let joined = lines.join("\n");

    // No sub-second component leaks into the nested timestamp (MF-6/MF-8 non-vacuity guard).
    assert!(
        !joined.contains(".123456"),
        "nested sub-second must be canonicalized: {joined}"
    );
    // D37: the canonicalizer recurses field-name-blind, so it must reach the new comment fields
    // too. These asserts FAIL if `comments[].updated_at`/`redacted_at` escape canonicalization.
    assert!(
        !joined.contains(".987654"),
        "nested comment updated_at sub-second must be canonicalized: {joined}"
    );
    assert!(
        !joined.contains(".111222"),
        "nested comment redacted_at sub-second must be canonicalized: {joined}"
    );
    insta::assert_snapshot!(joined);
}
