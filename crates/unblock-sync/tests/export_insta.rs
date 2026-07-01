//! Export-determinism golden (NFR-14, MF-8): a fixed corpus — including an issue whose nested
//! `dependencies[].created_at` carries SUB-SECOND precision — serializes to canonical `…Z` bytes.
//! This pins the recursive timestamp canonicalizer (MF-6) as non-vacuous.

use chrono::{TimeZone, Utc};
use unblock_model::{Dependency, DependencyType, Issue, Status};
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

    let corpus = [base("ub-1"), ub2, base("ub-3")];
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
    insta::assert_snapshot!(joined);
}
