//! Property tests for `IssueValidator` invariants (NFR-16).
//!
//! - a valid generator always passes;
//! - targeted invalid generators always fail with the expected `FieldError` field;
//! - boundary values (title 500/501, labels 64/65, custom variant 50/51 multibyte).

use chrono::{TimeZone, Utc};
use proptest::prelude::*;
use unblock_error::ModelError;
use unblock_model::{Issue, IssueType, IssueValidator, Priority, Status};

fn ts() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
}

/// A generator that always produces a VALID issue.
fn arb_valid_issue() -> impl Strategy<Value = Issue> {
    (
        "[a-z0-9]{1,8}",
        // A non-blank title: at least one visible char, never all-whitespace (which the validator
        // correctly rejects). `chars().count()` stays <= 500 so the length rule also passes.
        "[a-zA-Z0-9][a-zA-Z0-9 ]{0,99}",
        0i32..=4,
        // Valid labels: ASCII alphanumerics only (charset rule), 1..=10 chars.
        prop::collection::vec("[a-z0-9]{1,10}", 0..64),
    )
        .prop_map(|(hash, title, prio, labels)| Issue {
            id: format!("ub-{hash}"),
            title,
            priority: Priority(prio),
            labels,
            created_at: ts(),
            updated_at: ts(),
            ..Issue::default()
        })
}

fn failing_fields(issue: &Issue) -> Vec<String> {
    match IssueValidator::validate(issue) {
        Err(ModelError::ValidationFailed { fields }) => {
            fields.into_iter().map(|f| f.field).collect()
        }
        Ok(()) => Vec::new(),
        other => panic!("unexpected error variant: {other:?}"),
    }
}

proptest! {
    #[test]
    fn valid_generator_always_passes(issue in arb_valid_issue()) {
        prop_assert!(IssueValidator::validate(&issue).is_ok());
    }

    #[test]
    fn empty_or_whitespace_title_always_fails_as_title(ws in r"\s{0,5}") {
        let mut issue = Issue {
            id: "ub-abc123".to_string(),
            title: ws,
            created_at: ts(),
            updated_at: ts(),
            ..Issue::default()
        };
        // Empty/whitespace title -> a FieldError row, NEVER scalar RequiredField.
        match IssueValidator::validate(&issue) {
            Err(ModelError::ValidationFailed { fields }) => {
                prop_assert!(fields.iter().any(|f| f.field == "title"));
            }
            Err(ModelError::RequiredField { .. }) => {
                prop_assert!(false, "validate() must not emit scalar RequiredField");
            }
            other => prop_assert!(false, "expected ValidationFailed: {other:?}"),
        }
        issue.title = "ok".to_string();
        prop_assert!(IssueValidator::validate(&issue).is_ok());
    }

    #[test]
    fn out_of_range_priority_always_fails(p in prop_oneof![i32::MIN..0, 5..i32::MAX]) {
        let issue = Issue {
            id: "ub-abc123".to_string(),
            title: "ok".to_string(),
            priority: Priority(p),
            created_at: ts(),
            updated_at: ts(),
            ..Issue::default()
        };
        prop_assert!(failing_fields(&issue).contains(&"priority".to_string()));
    }
}

#[test]
fn title_boundary_500_passes_501_fails() {
    let mut issue = Issue {
        id: "ub-abc123".to_string(),
        title: "a".repeat(500),
        created_at: ts(),
        updated_at: ts(),
        ..Issue::default()
    };
    assert!(IssueValidator::validate(&issue).is_ok());
    issue.title = "a".repeat(501);
    assert!(failing_fields(&issue).contains(&"title".to_string()));
}

#[test]
fn labels_boundary_64_passes_65_fails() {
    let mut issue = Issue {
        id: "ub-abc123".to_string(),
        title: "ok".to_string(),
        labels: (0..64).map(|i| format!("l{i}")).collect(),
        created_at: ts(),
        updated_at: ts(),
        ..Issue::default()
    };
    assert!(IssueValidator::validate(&issue).is_ok());
    issue.labels = (0..65).map(|i| format!("l{i}")).collect();
    assert!(failing_fields(&issue).contains(&"labels".to_string()));
}

#[test]
fn custom_variant_boundary_50_multibyte_passes_51_fails() {
    // 50 multibyte chars pass; 51 fail (counted in chars, not bytes).
    let mut issue = Issue {
        id: "ub-abc123".to_string(),
        title: "ok".to_string(),
        status: Status::Custom("\u{1f980}".repeat(50)),
        issue_type: IssueType::Custom("\u{1f980}".repeat(50)),
        created_at: ts(),
        updated_at: ts(),
        ..Issue::default()
    };
    assert!(IssueValidator::validate(&issue).is_ok());

    issue.status = Status::Custom("\u{1f980}".repeat(51));
    assert!(failing_fields(&issue).contains(&"status".to_string()));

    issue.status = Status::Open;
    issue.issue_type = IssueType::Custom("\u{1f980}".repeat(51));
    assert!(failing_fields(&issue).contains(&"issue_type".to_string()));
}
