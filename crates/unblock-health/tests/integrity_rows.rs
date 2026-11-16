//! Criterion (b): the D58 clause (3) split of `integrity_check` rows, table-driven.
//!
//! The four index-only shapes, with and without the `*** in database main ***` line, naming an index
//! in `sqlite_schema`, are `index_corruption` with a `reindex` of that index. Every other row is
//! `page_corruption` and `Unsafe`.

use std::time::SystemTime;

use proptest::prelude::*;
use unblock_health::{
    AdvisoryFacts, AnomalyClass, DbFacts, DbFileFact, ExportFacts, ExportScan, HealthLevel,
    PlannedAction, PreOpenFacts, RepairPath, classify, classify_integrity_row,
};

const PREFIX: &str = "*** in database main ***\n";

fn names() -> Vec<String> {
    vec![
        "idx_issues_status".to_string(),
        "sqlite_autoindex_metadata_1".to_string(),
    ]
}

fn index_only_rows(index: &str) -> Vec<String> {
    vec![
        format!("row 12 missing from index {index}"),
        format!("rowid not at end-of-record for row 3 of index {index}"),
        format!("row 44 values differ from index {index}"),
        format!("wrong # of entries in index {index}"),
    ]
}

fn classify_db(rows: &[&str]) -> unblock_health::WorkspaceClassification {
    let pre = PreOpenFacts {
        db: DbFileFact::SqliteHeader,
        lost: false,
        wal_len: None,
        shm_present: false,
        journal_present: false,
        write_lock_held: false,
        recovery_entries: Vec::new(),
    };
    let db = DbFacts {
        integrity_rows: rows.iter().map(ToString::to_string).collect(),
        index_names: names(),
        ..DbFacts::default()
    };
    let export = ExportFacts {
        path: None,
        scan: ExportScan::Absent,
    };
    classify(
        &pre,
        Some(&db),
        &export,
        &AdvisoryFacts {
            now_utc: SystemTime::now(),
        },
    )
}

#[test]
fn each_index_only_shape_with_and_without_prefix_is_index_corruption() {
    for index in names() {
        for message in index_only_rows(&index) {
            for row in [message.clone(), format!("{PREFIX}{message}")] {
                let class = classify_integrity_row(&row, &names());
                assert_eq!(
                    class,
                    AnomalyClass::IndexCorruption {
                        index: index.clone(),
                        row: message.clone(),
                    },
                    "row {row:?}"
                );
                assert_eq!(class.severity(), HealthLevel::Recoverable);
                let planned = class.planned();
                assert_eq!(
                    planned.action,
                    PlannedAction::Reindex {
                        index: index.clone()
                    }
                );
                assert_eq!(planned.path, RepairPath::Online);
            }
        }
    }
}

#[test]
fn every_other_row_is_page_corruption_and_unsafe() {
    let rows = [
        "non-unique entry in index idx_issues_status",
        "NULL value in issues.title",
        "NUMERIC value in issues.priority",
        "TEXT value in issues.created_at",
        "CHECK constraint failed in issues",
        "Tree 3 page 4 cell 0: Rowid 9 out of order",
        "Tree 3 page 4: btreeInitPage() returns error code 11",
        "Page 7: never used",
        "Tree 2 page 5: 2nd reference to page 5",
        "Freelist: size is 3 but should be 2",
        "Rowid 9 out of order",
        "Multiple uses for byte 12 of page 3",
        "Fragmentation of 4 bytes reported as 0 on page 3",
        "Child page depth differs",
        "max rootpage (5) disagrees with header (4)",
        "something SQLite has never said",
        "",
        "*** in database main ***",
        // Near misses of the index-only shapes.
        "row 12 missing from index idx_issues_status extra",
        "row  missing from index idx_issues_status",
        "row -3 missing from index idx_issues_status",
        "row 12 missing from index ",
        "wrong # of entries in index",
        "*** in database main ***\n*** in database main ***\nrow 1 missing from index idx_issues_status",
        "row 12 missing from index idx_issues_status\nPage 7: never used",
    ];
    for row in rows {
        let class = classify_integrity_row(row, &names());
        assert_eq!(class.code(), "page_corruption", "row {row:?}");
        assert_eq!(class.severity(), HealthLevel::Unsafe, "row {row:?}");
        assert_eq!(class.planned().path, RepairPath::Manual, "row {row:?}");
        assert!(matches!(
            class.planned().action,
            PlannedAction::Manual { .. }
        ));
    }
    let prefixed = classify_integrity_row(&format!("{PREFIX}Page 7: never used"), &names());
    assert_eq!(
        prefixed,
        AnomalyClass::PageCorruption {
            row: "Page 7: never used".to_string()
        }
    );
}

#[test]
fn an_index_only_row_naming_an_absent_index_is_unsafe() {
    for message in index_only_rows("idx_not_in_schema") {
        let class = classify_integrity_row(&message, &names());
        assert_eq!(class.code(), "page_corruption", "row {message:?}");
        assert_eq!(class.severity(), HealthLevel::Unsafe);
    }
    let class = classify_db(&["wrong # of entries in index idx_not_in_schema"]);
    assert_eq!(class.level, HealthLevel::Unsafe);
}

#[test]
fn index_only_rows_alone_are_recoverable_with_a_reindex_each() {
    let class = classify_db(&[
        "row 12 missing from index idx_issues_status",
        "wrong # of entries in index sqlite_autoindex_metadata_1",
    ]);
    assert_eq!(class.level, HealthLevel::Recoverable);
    let targets: Vec<Option<String>> = class.planned.iter().map(|p| p.action.target()).collect();
    assert_eq!(
        targets,
        vec![
            Some("idx_issues_status".to_string()),
            Some("sqlite_autoindex_metadata_1".to_string()),
        ]
    );
}

#[test]
fn index_only_rows_mixed_with_one_unrecognised_row_are_unsafe() {
    let class = classify_db(&[
        "row 12 missing from index idx_issues_status",
        "row 13 missing from index idx_issues_status",
        "an unrecognised message",
    ]);
    assert_eq!(class.level, HealthLevel::Unsafe);
    assert!(!class.recovery_possible());
    let codes: Vec<&str> = class.anomalies.iter().map(AnomalyClass::code).collect();
    assert_eq!(
        codes,
        vec!["index_corruption", "index_corruption", "page_corruption"]
    );
}

proptest! {
    #[test]
    fn arbitrary_rows_never_panic_and_unknown_rows_are_page_corruption(row in ".*") {
        let class = classify_integrity_row(&row, &[]);
        prop_assert_eq!(class.code(), "page_corruption");
        let class = classify_integrity_row(&format!("unknown: {row}"), &names());
        prop_assert_eq!(class.code(), "page_corruption");
        let class = classify_integrity_row(&row, &names());
        let known = matches!(
            class,
            AnomalyClass::IndexCorruption { .. } | AnomalyClass::PageCorruption { .. }
        );
        prop_assert!(known);
    }

    #[test]
    fn well_formed_shapes_with_any_row_number_are_index_corruption(
        n in any::<u64>(),
        index in "[A-Za-z_][A-Za-z0-9_]{0,24}",
        shape in 0usize..4,
        prefixed in any::<bool>(),
    ) {
        let message = match shape {
            0 => format!("row {n} missing from index {index}"),
            1 => format!("rowid not at end-of-record for row {n} of index {index}"),
            2 => format!("row {n} values differ from index {index}"),
            _ => format!("wrong # of entries in index {index}"),
        };
        let row = if prefixed { format!("{PREFIX}{message}") } else { message.clone() };
        let class = classify_integrity_row(&row, std::slice::from_ref(&index));
        prop_assert_eq!(class, AnomalyClass::IndexCorruption { index, row: message });
    }
}
