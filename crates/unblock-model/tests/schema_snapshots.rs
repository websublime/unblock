//! `JsonSchema` stability snapshots for every public type (FR-12 schema resource).
//!
//! Snapshotted under the DEFAULT (no-`toon`) feature set. A diff here signals a contract change
//! that downstream `contract_version` bumps must account for. Each type gets its own `#[test]` so a
//! single run regenerates every `.snap` independently (no early stop on the first diff).

use schemars::schema_for;

macro_rules! schema_test {
    ($test:ident, $name:literal, $ty:ty) => {
        #[test]
        fn $test() {
            let schema = schema_for!($ty);
            let json = serde_json::to_string_pretty(&schema).unwrap();
            insta::assert_snapshot!($name, json);
        }
    };
}

// The `output_format` baseline is pinned to the DEFAULT (no-`toon`) feature set: with `toon` the
// `Toon` variant is present and the schema legitimately differs. Gate that one snapshot off so the
// `--features toon` build does not diff against the no-`toon` golden.
macro_rules! schema_test_default_features {
    ($test:ident, $name:literal, $ty:ty) => {
        #[cfg(not(feature = "toon"))]
        #[test]
        fn $test() {
            let schema = schema_for!($ty);
            let json = serde_json::to_string_pretty(&schema).unwrap();
            insta::assert_snapshot!($name, json);
        }
    };
}

schema_test!(status, "status", unblock_model::Status);
schema_test!(priority, "priority", unblock_model::Priority);
schema_test!(issue_type, "issue_type", unblock_model::IssueType);
schema_test!(
    dependency_type,
    "dependency_type",
    unblock_model::DependencyType
);
schema_test!(event_type, "event_type", unblock_model::EventType);

schema_test!(issue, "issue", unblock_model::Issue);
schema_test!(dependency, "dependency", unblock_model::Dependency);
schema_test!(comment, "comment", unblock_model::Comment);
schema_test!(event, "event", unblock_model::Event);
schema_test!(epic_status, "epic_status", unblock_model::EpicStatus);

schema_test!(list_filters, "list_filters", unblock_model::ListFilters);
schema_test!(
    count_group_by,
    "count_group_by",
    unblock_model::CountGroupBy
);
schema_test_default_features!(output_format, "output_format", unblock_model::OutputFormat);
schema_test!(count_bucket, "count_bucket", unblock_model::CountBucket);
schema_test!(graph_edge, "graph_edge", unblock_model::GraphEdge);
schema_test!(dep_tree, "dep_tree", unblock_model::DepTree);
schema_test!(close_outcome, "close_outcome", unblock_model::CloseOutcome);
schema_test!(import_report, "import_report", unblock_model::ImportReport);
schema_test!(export_report, "export_report", unblock_model::ExportReport);
schema_test!(
    diagnostic_kind,
    "diagnostic_kind",
    unblock_model::DiagnosticKind
);
schema_test!(
    diagnostic_report,
    "diagnostic_report",
    unblock_model::DiagnosticReport
);
schema_test!(
    diagnostic_finding,
    "diagnostic_finding",
    unblock_model::DiagnosticFinding
);
