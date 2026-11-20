//! The D58 class table and the classifier's invariants (criteria (a), (i) and (m)).
//!
//! The golden snapshot pins every anomaly code with its severity, planned action and path, and
//! every advisory code. Proptests over random fact sets pin composite = `max`, monotonicity, the
//! `Drifted` → online rule and the "no `Unsafe` class plans an executable action" rule. The class
//! list and the class strategy come from exhaustive matches over `AnomalyClass`, so a new variant
//! fails to compile here until it joins them.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use proptest::prelude::*;
use serde_json::{Value, json};
use unblock_health::{
    Advisory, AdvisoryFacts, AnomalyClass, DbFacts, DbFileFact, ExportFacts, ExportScan,
    HealthLevel, PlannedAction, PreOpenFacts, RecoveryEntry, RepairPath, WorkspaceClassification,
    classify,
};

/// Criterion (m): `classify` is a plain, non-async `fn`.
const _: fn(
    &PreOpenFacts,
    Option<&DbFacts>,
    &ExportFacts,
    &AdvisoryFacts,
) -> WorkspaceClassification = classify;

const DAY: Duration = Duration::from_hours(24);

fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_794_573_296)
}

fn adv() -> AdvisoryFacts {
    AdvisoryFacts { now_utc: now() }
}

fn healthy_pre() -> PreOpenFacts {
    PreOpenFacts {
        db: DbFileFact::SqliteHeader,
        lost: false,
        wal_len: None,
        shm_present: false,
        journal_present: false,
        write_lock_held: false,
        recovery_entries: Vec::new(),
    }
}

fn no_export() -> ExportFacts {
    ExportFacts {
        path: None,
        scan: ExportScan::Absent,
    }
}

fn db(version: i64) -> DbFacts {
    DbFacts {
        schema_version: version,
        expected_schema_version: version,
        index_names: vec!["idx_issues_status".to_string()],
        ..DbFacts::default()
    }
}

/// One instance of every class, in clause (1) order, walked through [`next_class`].
fn every_class() -> Vec<AnomalyClass> {
    let mut classes = Vec::new();
    let mut next = Some(AnomalyClass::DatabaseLost);
    while let Some(class) = next {
        next = next_class(&class);
        classes.push(class);
    }
    classes
}

/// The class after `class` in clause (1) order, `None` after the last. The match is exhaustive
/// with no wildcard arm, so a new `AnomalyClass` variant fails to compile until it is chained in.
fn next_class(class: &AnomalyClass) -> Option<AnomalyClass> {
    Some(match class {
        AnomalyClass::DatabaseLost => AnomalyClass::DatabaseUnreadable {
            reason: "permission denied".to_string(),
        },
        AnomalyClass::DatabaseUnreadable { .. } => AnomalyClass::DatabaseNotSqlite { len: 5 },
        AnomalyClass::DatabaseNotSqlite { .. } => AnomalyClass::SidecarMismatch,
        AnomalyClass::SidecarMismatch => AnomalyClass::TruncatedWal { len: 5 },
        AnomalyClass::TruncatedWal { .. } => AnomalyClass::JournalSidecarPresent,
        AnomalyClass::JournalSidecarPresent => AnomalyClass::JsonlConflictMarkers { first_line: 3 },
        AnomalyClass::JsonlConflictMarkers { .. } => AnomalyClass::JsonlUnscannable {
            reason: "line too long".to_string(),
        },
        AnomalyClass::JsonlUnscannable { .. } => AnomalyClass::IndexCorruption {
            index: "idx_issues_status".to_string(),
            row: "row 7 missing from index idx_issues_status".to_string(),
        },
        AnomalyClass::IndexCorruption { .. } => AnomalyClass::PageCorruption {
            row: "Page 7: never used".to_string(),
        },
        AnomalyClass::PageCorruption { .. } => AnomalyClass::ChildCounterDrift {
            parent_id: "ub-1".to_string(),
            stored: 2,
            observed_max: 5,
        },
        AnomalyClass::ChildCounterDrift { .. } => AnomalyClass::DuplicateMetadataKey {
            key: "k".to_string(),
            rows: 2,
        },
        AnomalyClass::DuplicateMetadataKey { .. } => AnomalyClass::SchemaStampBehind {
            observed: 3,
            expected: 4,
        },
        AnomalyClass::SchemaStampBehind { .. } => AnomalyClass::SchemaStampAhead {
            observed: 5,
            expected: 4,
        },
        AnomalyClass::SchemaStampAhead { .. } => return None,
    })
}

/// Random instances of the variant `class` names. Exhaustive like [`next_class`].
fn variant_strategy(class: &AnomalyClass) -> BoxedStrategy<AnomalyClass> {
    match class {
        AnomalyClass::DatabaseLost => Just(AnomalyClass::DatabaseLost).boxed(),
        AnomalyClass::DatabaseUnreadable { .. } => ".*"
            .prop_map(|reason| AnomalyClass::DatabaseUnreadable { reason })
            .boxed(),
        AnomalyClass::DatabaseNotSqlite { .. } => (1u64..)
            .prop_map(|len| AnomalyClass::DatabaseNotSqlite { len })
            .boxed(),
        AnomalyClass::SidecarMismatch => Just(AnomalyClass::SidecarMismatch).boxed(),
        AnomalyClass::TruncatedWal { .. } => (1u64..32)
            .prop_map(|len| AnomalyClass::TruncatedWal { len })
            .boxed(),
        AnomalyClass::JournalSidecarPresent => Just(AnomalyClass::JournalSidecarPresent).boxed(),
        AnomalyClass::JsonlConflictMarkers { .. } => (1usize..)
            .prop_map(|first_line| AnomalyClass::JsonlConflictMarkers { first_line })
            .boxed(),
        AnomalyClass::JsonlUnscannable { .. } => ".*"
            .prop_map(|reason| AnomalyClass::JsonlUnscannable { reason })
            .boxed(),
        AnomalyClass::IndexCorruption { .. } => ("[a-z_]{1,12}", ".*")
            .prop_map(|(index, row)| AnomalyClass::IndexCorruption { index, row })
            .boxed(),
        AnomalyClass::PageCorruption { .. } => ".*"
            .prop_map(|row| AnomalyClass::PageCorruption { row })
            .boxed(),
        AnomalyClass::ChildCounterDrift { .. } => ("[a-z0-9.-]{1,12}", any::<u32>(), any::<u32>())
            .prop_map(
                |(parent_id, stored, observed_max)| AnomalyClass::ChildCounterDrift {
                    parent_id,
                    stored,
                    observed_max,
                },
            )
            .boxed(),
        AnomalyClass::DuplicateMetadataKey { .. } => (".*", 2u32..)
            .prop_map(|(key, rows)| AnomalyClass::DuplicateMetadataKey { key, rows })
            .boxed(),
        AnomalyClass::SchemaStampBehind { .. } => (any::<i64>(), any::<i64>())
            .prop_map(|(observed, expected)| AnomalyClass::SchemaStampBehind { observed, expected })
            .boxed(),
        AnomalyClass::SchemaStampAhead { .. } => (any::<i64>(), any::<i64>())
            .prop_map(|(observed, expected)| AnomalyClass::SchemaStampAhead { observed, expected })
            .boxed(),
    }
}

#[test]
fn class_table_golden() {
    let classes: Vec<Value> = every_class()
        .iter()
        .map(|class| {
            let planned = class.planned();
            let text = match planned.action {
                PlannedAction::Manual { text } => Some(text),
                _ => None,
            };
            json!({
                "code": class.code(),
                "severity": class.severity().as_str(),
                "action": planned.action.code(),
                "target": planned.action.target(),
                "path": planned.path.as_str(),
                "manual_text": text,
            })
        })
        .collect();
    let advisories: Vec<&str> = [
        Advisory::DatabaseNotCreated,
        Advisory::ExportIsSessionFile,
        Advisory::StaleRecoveryArtifacts {
            count: 1,
            oldest_days: Some(31),
        },
        Advisory::WriteLockHeld,
    ]
    .iter()
    .map(Advisory::code)
    .collect();
    insta::assert_json_snapshot!(
        "class_table",
        json!({ "anomalies": classes, "advisories": advisories })
    );
}

#[test]
fn codes_are_unique_and_planned_names_its_class() {
    let classes = every_class();
    let codes: std::collections::BTreeSet<&str> = classes.iter().map(AnomalyClass::code).collect();
    for class in &classes {
        assert_eq!(class.planned().anomaly, class.code());
        assert!(!class.to_string().is_empty());
    }
    assert_eq!(codes.len(), 14);
    assert_eq!(
        classes.len(),
        codes.len(),
        "the chain visits each class once"
    );
}

#[test]
fn healthy_facts_classify_healthy() {
    let class = classify(&healthy_pre(), Some(&db(4)), &no_export(), &adv());
    assert_eq!(class.level, HealthLevel::Healthy);
    assert!(class.anomalies.is_empty());
    assert!(class.advisories.is_empty());
    assert!(class.planned.is_empty());
    assert!(class.is_operable());
    assert!(class.recovery_possible());
}

#[test]
fn database_not_created_is_an_advisory_with_level_healthy() {
    for fact in [DbFileFact::Absent, DbFileFact::Empty] {
        let pre = PreOpenFacts {
            db: fact,
            ..healthy_pre()
        };
        let class = classify(&pre, None, &no_export(), &adv());
        assert_eq!(class.level, HealthLevel::Healthy);
        assert!(class.anomalies.is_empty());
        assert_eq!(class.advisories, vec![Advisory::DatabaseNotCreated]);
    }
}

#[test]
fn database_lost_plans_no_preopen_exclusive_action() {
    let pre = PreOpenFacts {
        db: DbFileFact::Absent,
        lost: true,
        wal_len: Some(5),
        shm_present: true,
        journal_present: true,
        ..healthy_pre()
    };
    let class = classify(&pre, None, &no_export(), &adv());
    assert_eq!(class.level, HealthLevel::Unsafe);
    assert_eq!(class.anomalies[0], AnomalyClass::DatabaseLost);
    assert!(!class.advisories.contains(&Advisory::DatabaseNotCreated));
    assert_eq!(class.planned.len(), class.anomalies.len());
    assert!(
        class
            .planned
            .iter()
            .all(|planned| planned.path != RepairPath::PreopenExclusive),
        "{:?}",
        class.planned
    );
    assert!(!class.recovery_possible());
}

#[test]
fn database_unreadable_and_not_sqlite_are_unsafe_without_db_facts() {
    let pre = PreOpenFacts {
        db: DbFileFact::Unreadable {
            reason: "permission denied".to_string(),
        },
        ..healthy_pre()
    };
    let class = classify(&pre, None, &no_export(), &adv());
    assert_eq!(class.level, HealthLevel::Unsafe);
    assert_eq!(class.planned[0].path, RepairPath::Manual);

    let pre = PreOpenFacts {
        db: DbFileFact::NotSqlite { len: 3 },
        ..healthy_pre()
    };
    let class = classify(&pre, None, &no_export(), &adv());
    assert_eq!(class.level, HealthLevel::Unsafe);
    assert_eq!(class.planned[0].path, RepairPath::Manual);
    match class.planned[0].action {
        PlannedAction::Manual { text } => {
            for needle in [
                "Stop every unblock process",
                "-wal, -shm or -journal",
                ".unblock/.recovery/",
                "unblock init",
                "`sync` tool",
            ] {
                assert!(text.contains(needle), "{needle:?} missing from {text}");
            }
        }
        ref other => panic!("expected a manual action, got {other:?}"),
    }
}

#[test]
fn truncated_wal_is_strictly_between_zero_and_thirty_two_bytes() {
    for (len, truncated) in [
        (0, false),
        (1, true),
        (31, true),
        (32, false),
        (4096, false),
    ] {
        let pre = PreOpenFacts {
            wal_len: Some(len),
            ..healthy_pre()
        };
        let class = classify(&pre, None, &no_export(), &adv());
        assert_eq!(
            class
                .anomalies
                .contains(&AnomalyClass::TruncatedWal { len }),
            truncated,
            "wal_len {len}"
        );
    }
}

#[test]
fn sidecar_classes_plan_preopen_exclusive_actions() {
    let pre = PreOpenFacts {
        shm_present: true,
        journal_present: true,
        ..healthy_pre()
    };
    let class = classify(&pre, None, &no_export(), &adv());
    let actions: Vec<(&str, RepairPath)> = class
        .planned
        .iter()
        .map(|planned| (planned.action.code(), planned.path))
        .collect();
    assert_eq!(
        actions,
        vec![
            ("quarantine_sidecar", RepairPath::PreopenExclusive),
            ("quarantine_journal", RepairPath::PreopenExclusive),
        ]
    );
    assert_eq!(class.level, HealthLevel::Recoverable);
}

#[test]
fn drift_classes_are_drifted_and_plan_online_repairs() {
    let facts = DbFacts {
        schema_version: 3,
        expected_schema_version: 4,
        child_counter_drift: vec![("ub-1".to_string(), 2, 5), ("ub-2".to_string(), 9, 4)],
        duplicate_metadata_keys: vec![("k".to_string(), 2)],
        ..DbFacts::default()
    };
    let class = classify(&healthy_pre(), Some(&facts), &no_export(), &adv());
    assert_eq!(class.level, HealthLevel::Drifted);
    assert!(class.is_operable());
    let actions: Vec<PlannedAction> = class.planned.iter().map(|p| p.action.clone()).collect();
    assert_eq!(
        actions,
        vec![
            PlannedAction::RaiseChildCounter {
                parent_id: "ub-1".to_string(),
                to: 5
            },
            PlannedAction::DedupeMetadataKey {
                key: "k".to_string()
            },
            PlannedAction::Migrate,
        ]
    );
    assert!(class.planned.iter().all(|p| p.path == RepairPath::Online));
}

#[test]
fn schema_stamp_ahead_is_unsafe_and_names_unblock_update() {
    let class = classify(
        &healthy_pre(),
        Some(&DbFacts {
            schema_version: 5,
            expected_schema_version: 4,
            ..DbFacts::default()
        }),
        &no_export(),
        &adv(),
    );
    assert_eq!(class.level, HealthLevel::Unsafe);
    match class.planned[0].action {
        PlannedAction::Manual { text } => assert!(text.contains("unblock update"), "{text}"),
        ref other => panic!("expected a manual action, got {other:?}"),
    }
}

#[test]
fn anomalies_follow_clause_one_class_order() {
    let facts = DbFacts {
        integrity_rows: vec![
            "Page 7: never used".to_string(),
            "row 1 missing from index idx_issues_status".to_string(),
        ],
        index_names: vec!["idx_issues_status".to_string()],
        schema_version: 3,
        expected_schema_version: 4,
        ..DbFacts::default()
    };
    let pre = PreOpenFacts {
        journal_present: true,
        ..healthy_pre()
    };
    let export = ExportFacts {
        path: None,
        scan: ExportScan::ConflictMarkers { first_line: 2 },
    };
    let class = classify(&pre, Some(&facts), &export, &adv());
    let codes: Vec<&str> = class.anomalies.iter().map(AnomalyClass::code).collect();
    assert_eq!(
        codes,
        vec![
            "journal_sidecar_present",
            "jsonl_conflict_markers",
            "index_corruption",
            "page_corruption",
            "schema_stamp_behind",
        ]
    );
    let planned: Vec<&str> = class.planned.iter().map(|p| p.anomaly).collect();
    assert_eq!(planned, codes);
}

#[test]
fn advisories_never_raise_the_level() {
    let pre = PreOpenFacts {
        write_lock_held: true,
        recovery_entries: vec![RecoveryEntry {
            name: "moved-by-hand".to_string(),
            stamp_utc: None,
        }],
        ..healthy_pre()
    };
    let class = classify(&pre, Some(&db(4)), &no_export(), &adv());
    assert_eq!(class.level, HealthLevel::Healthy);
    assert_eq!(
        class.advisories,
        vec![
            Advisory::StaleRecoveryArtifacts {
                count: 1,
                oldest_days: None
            },
            Advisory::WriteLockHeld,
        ]
    );
}

#[test]
fn stale_recovery_advisory_31_days_raises_and_29_days_does_not() {
    let entry = |age: Duration| RecoveryEntry {
        name: "run".to_string(),
        stamp_utc: Some(now() - age),
    };
    let pre = PreOpenFacts {
        recovery_entries: vec![entry(DAY * 31)],
        ..healthy_pre()
    };
    let class = classify(&pre, Some(&db(4)), &no_export(), &adv());
    assert_eq!(class.level, HealthLevel::Healthy);
    assert_eq!(
        class.advisories,
        vec![Advisory::StaleRecoveryArtifacts {
            count: 1,
            oldest_days: Some(31)
        }]
    );

    let pre = PreOpenFacts {
        recovery_entries: vec![entry(DAY * 29), entry(DAY * 30)],
        ..healthy_pre()
    };
    let class = classify(&pre, Some(&db(4)), &no_export(), &adv());
    assert!(class.advisories.is_empty(), "{:?}", class.advisories);
}

/// One random instance of any class, each variant drawn from [`variant_strategy`].
fn class_strategy() -> impl Strategy<Value = AnomalyClass> {
    proptest::strategy::Union::new(every_class().iter().map(variant_strategy))
}

fn db_file_strategy() -> impl Strategy<Value = DbFileFact> {
    prop_oneof![
        Just(DbFileFact::Absent),
        Just(DbFileFact::Empty),
        "[a-z ]{0,16}".prop_map(|reason| DbFileFact::Unreadable { reason }),
        (1u64..1024).prop_map(|len| DbFileFact::NotSqlite { len }),
        Just(DbFileFact::SqliteHeader),
    ]
}

fn pre_strategy() -> impl Strategy<Value = PreOpenFacts> {
    (
        db_file_strategy(),
        any::<bool>(),
        proptest::option::of(0u64..64),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        proptest::collection::vec(proptest::option::of(0u64..90), 0..4),
    )
        .prop_map(
            |(db, lost, wal_len, shm_present, journal_present, write_lock_held, ages)| {
                PreOpenFacts {
                    lost: lost && matches!(db, DbFileFact::Absent | DbFileFact::Empty),
                    db,
                    wal_len,
                    shm_present,
                    journal_present,
                    write_lock_held,
                    recovery_entries: ages
                        .into_iter()
                        .map(|age| RecoveryEntry {
                            name: "run".to_string(),
                            stamp_utc: age.map(|days| now() - DAY * u32::try_from(days).unwrap()),
                        })
                        .collect(),
                }
            },
        )
}

fn db_strategy() -> impl Strategy<Value = Option<DbFacts>> {
    proptest::option::of(
        (
            proptest::collection::vec(
                prop_oneof![
                    (0u64..100).prop_map(|n| format!("row {n} missing from index idx_a")),
                    (0u64..100).prop_map(|n| format!("row {n} missing from index idx_gone")),
                    ".{0,40}",
                ],
                0..4,
            ),
            1i64..6,
            1i64..6,
            proptest::collection::vec(("[a-z]{1,4}", any::<u32>(), any::<u32>()), 0..3),
            proptest::collection::vec(("[a-z]{1,4}", 0u32..5), 0..3),
        )
            .prop_map(
                |(integrity_rows, schema_version, expected_schema_version, drift, dups)| DbFacts {
                    integrity_rows,
                    schema_version,
                    expected_schema_version,
                    index_names: vec!["idx_a".to_string()],
                    child_counter_drift: drift,
                    duplicate_metadata_keys: dups,
                },
            ),
    )
}

fn export_strategy() -> impl Strategy<Value = ExportFacts> {
    prop_oneof![
        Just(ExportScan::Absent),
        Just(ExportScan::Clean),
        (1usize..100).prop_map(|first_line| ExportScan::ConflictMarkers { first_line }),
        "[a-z ]{0,16}".prop_map(|reason| ExportScan::Unscannable { reason }),
        Just(ExportScan::SessionFile),
    ]
    .prop_map(|scan| ExportFacts { path: None, scan })
}

/// A fact change that adds exactly one anomaly and removes none.
#[derive(Debug, Clone)]
enum Addition {
    Journal,
    IntegrityRow(String),
    Drift(u32),
    DuplicateKey(u32),
    ConflictMarkers,
}

fn addition_strategy() -> impl Strategy<Value = Addition> {
    prop_oneof![
        Just(Addition::Journal),
        ".{0,40}".prop_map(Addition::IntegrityRow),
        (0u32..1000).prop_map(Addition::Drift),
        (2u32..10).prop_map(Addition::DuplicateKey),
        Just(Addition::ConflictMarkers),
    ]
}

fn apply(
    addition: &Addition,
    pre: &mut PreOpenFacts,
    db: &mut Option<DbFacts>,
    export: &mut ExportFacts,
) {
    match addition {
        Addition::Journal => pre.journal_present = true,
        Addition::IntegrityRow(row) => db
            .get_or_insert_with(DbFacts::default)
            .integrity_rows
            .push(row.clone()),
        Addition::Drift(stored) => db
            .get_or_insert_with(DbFacts::default)
            .child_counter_drift
            .push(("added".to_string(), *stored, stored + 1)),
        Addition::DuplicateKey(rows) => db
            .get_or_insert_with(DbFacts::default)
            .duplicate_metadata_keys
            .push(("added".to_string(), *rows)),
        Addition::ConflictMarkers => {
            if matches!(export.scan, ExportScan::Absent | ExportScan::Clean) {
                export.scan = ExportScan::ConflictMarkers { first_line: 1 };
            }
        }
    }
}

proptest! {
    #[test]
    fn composite_is_the_max_over_anomaly_severities(
        pre in pre_strategy(),
        db in db_strategy(),
        export in export_strategy(),
    ) {
        let class = classify(&pre, db.as_ref(), &export, &adv());
        let max = class
            .anomalies
            .iter()
            .map(AnomalyClass::severity)
            .max()
            .unwrap_or(HealthLevel::Healthy);
        prop_assert_eq!(class.level, max);
        prop_assert_eq!(class.planned.len(), class.anomalies.len());
        for (anomaly, planned) in class.anomalies.iter().zip(&class.planned) {
            prop_assert_eq!(planned.anomaly, anomaly.code());
            if anomaly.severity() == HealthLevel::Drifted {
                prop_assert_eq!(planned.path, RepairPath::Online);
            }
            if anomaly.severity() == HealthLevel::Unsafe {
                let manual = matches!(planned.action, PlannedAction::Manual { .. });
                prop_assert!(manual, "{}", planned.action.code());
                prop_assert_eq!(planned.path, RepairPath::Manual);
            }
        }
        if pre.lost {
            prop_assert!(class.planned.iter().all(|p| p.path != RepairPath::PreopenExclusive));
        }
    }

    #[test]
    fn adding_a_class_never_lowers_the_level(
        pre in pre_strategy(),
        db in db_strategy(),
        export in export_strategy(),
        addition in addition_strategy(),
    ) {
        let before = classify(&pre, db.as_ref(), &export, &adv());
        let (mut pre2, mut db2, mut export2) = (pre, db, export);
        apply(&addition, &mut pre2, &mut db2, &mut export2);
        let after = classify(&pre2, db2.as_ref(), &export2, &adv());
        prop_assert!(after.level >= before.level, "{:?} -> {:?}", before.level, after.level);
        prop_assert!(after.anomalies.len() >= before.anomalies.len());
    }

    #[test]
    fn every_drifted_class_plans_online(class in class_strategy()) {
        if class.severity() == HealthLevel::Drifted {
            prop_assert_eq!(class.planned().path, RepairPath::Online);
        }
    }

    #[test]
    fn no_unsafe_class_plans_an_executable_action(class in class_strategy()) {
        if class.severity() == HealthLevel::Unsafe {
            let planned = class.planned();
            let manual = matches!(planned.action, PlannedAction::Manual { .. });
            prop_assert!(manual, "{} plans {}", class.code(), planned.action.code());
            prop_assert!(
                !matches!(planned.path, RepairPath::PreopenExclusive | RepairPath::Online),
                "{} plans a {} action", class.code(), planned.path
            );
        }
    }
}
