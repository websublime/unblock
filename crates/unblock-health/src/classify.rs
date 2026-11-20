//! The pure classifier (D58 clauses (1) and (4)).
//!
//! [`classify`] turns the four fact sets into anomalies in clause (1) class order, one planned
//! repair per anomaly, the advisories and the composite level (`max` over the anomaly severities).

use std::time::{Duration, SystemTime};

use crate::anomaly::{Advisory, AnomalyClass, PlannedRepair};
use crate::facts::{AdvisoryFacts, DbFacts, DbFileFact, ExportFacts, ExportScan, PreOpenFacts};
use crate::integrity::classify_integrity_row;
use crate::level::HealthLevel;

/// A `-wal` shorter than its header is truncated (D29 clause F5-A).
const WAL_HEADER_BYTES: u64 = 32;

/// A run directory older than this raises `stale_recovery_artifacts`.
const STALE_AFTER_DAYS: u64 = 30;

/// Seconds in one day.
const SECONDS_PER_DAY: u64 = 86_400;

/// One workspace's classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceClassification {
    /// The composite level: the worst anomaly severity, `Healthy` when there is none.
    pub level: HealthLevel,
    /// The anomalies, in D58 clause (1) class order.
    pub anomalies: Vec<AnomalyClass>,
    /// The advisories; they never raise the level.
    pub advisories: Vec<Advisory>,
    /// One planned repair per anomaly, in anomaly order.
    pub planned: Vec<PlannedRepair>,
}

impl WorkspaceClassification {
    /// Whether the workspace can still be operated on (`Healthy` or `Drifted`).
    #[must_use]
    pub const fn is_operable(&self) -> bool {
        self.level.is_operable()
    }

    /// Whether the workspace needs recovery (`Recoverable`).
    #[must_use]
    pub const fn needs_recovery(&self) -> bool {
        self.level.needs_recovery()
    }

    /// Whether an in-product repair can bring the workspace back (any level but `Unsafe`).
    #[must_use]
    pub const fn recovery_possible(&self) -> bool {
        !self.level.is_fatal()
    }
}

/// Classify a workspace from its facts.
///
/// Pre-open classes come from `pre`, database classes only when `db` is `Some`, and export classes
/// from `export`. With no database file, a sidecar class plans a manual step, because the exclusive
/// window needs a database file.
///
/// ```
/// use std::time::SystemTime;
/// use unblock_health::{
///     AdvisoryFacts, DbFileFact, ExportFacts, ExportScan, HealthLevel, PreOpenFacts, classify,
/// };
///
/// let pre = PreOpenFacts {
///     db: DbFileFact::SqliteHeader,
///     lost: false,
///     wal_len: None,
///     shm_present: true,
///     journal_present: false,
///     write_lock_held: false,
///     recovery_entries: Vec::new(),
/// };
/// let export = ExportFacts { path: None, scan: ExportScan::Absent };
/// let adv = AdvisoryFacts { now_utc: SystemTime::now() };
/// let class = classify(&pre, None, &export, &adv);
/// assert_eq!(class.level, HealthLevel::Recoverable);
/// assert_eq!(class.anomalies[0].code(), "sidecar_mismatch");
/// assert_eq!(class.planned[0].action.code(), "quarantine_sidecar");
/// ```
#[must_use]
pub fn classify(
    pre: &PreOpenFacts,
    db: Option<&DbFacts>,
    export: &ExportFacts,
    adv: &AdvisoryFacts,
) -> WorkspaceClassification {
    let mut anomalies = Vec::new();
    let mut advisories = Vec::new();

    let no_database_file = matches!(pre.db, DbFileFact::Absent | DbFileFact::Empty);
    if pre.lost {
        anomalies.push(AnomalyClass::DatabaseLost);
    }
    match &pre.db {
        DbFileFact::Unreadable { reason } => anomalies.push(AnomalyClass::DatabaseUnreadable {
            reason: reason.clone(),
        }),
        DbFileFact::NotSqlite { len } => {
            anomalies.push(AnomalyClass::DatabaseNotSqlite { len: *len });
        }
        DbFileFact::Absent | DbFileFact::Empty if !pre.lost => {
            advisories.push(Advisory::DatabaseNotCreated);
        }
        DbFileFact::Absent | DbFileFact::Empty | DbFileFact::SqliteHeader => {}
    }
    if pre.shm_present && pre.wal_len.is_none() {
        anomalies.push(AnomalyClass::SidecarMismatch);
    }
    if let Some(len) = pre
        .wal_len
        .filter(|len| (1..WAL_HEADER_BYTES).contains(len))
    {
        anomalies.push(AnomalyClass::TruncatedWal { len });
    }
    if pre.journal_present {
        anomalies.push(AnomalyClass::JournalSidecarPresent);
    }

    match &export.scan {
        ExportScan::Absent | ExportScan::Clean => {}
        ExportScan::ConflictMarkers { first_line } => {
            anomalies.push(AnomalyClass::JsonlConflictMarkers {
                first_line: *first_line,
            });
        }
        ExportScan::Unscannable { reason } => anomalies.push(AnomalyClass::JsonlUnscannable {
            reason: reason.clone(),
        }),
        // PRD §4 D5 amendment (`ub-c15`): a session-file export is reported, never scanned and never
        // regenerated, so it is an advisory (no level, no planned action).
        ExportScan::SessionFile => advisories.push(Advisory::ExportIsSessionFile),
    }

    if let Some(db) = db {
        push_database_classes(db, &mut anomalies);
    }

    // Stable, so rows keep their probe order inside one class.
    anomalies.sort_by_key(AnomalyClass::rank);

    if let Some(stale) = stale_advisory(&pre.recovery_entries, adv.now_utc) {
        advisories.push(stale);
    }
    if pre.write_lock_held {
        advisories.push(Advisory::WriteLockHeld);
    }

    let level = anomalies
        .iter()
        .map(AnomalyClass::severity)
        .max()
        .unwrap_or(HealthLevel::Healthy);
    let planned = anomalies
        .iter()
        .map(|anomaly| {
            if no_database_file {
                anomaly.planned_without_database()
            } else {
                anomaly.planned()
            }
        })
        .collect();

    WorkspaceClassification {
        level,
        anomalies,
        advisories,
        planned,
    }
}

/// Append the database classes: integrity rows, child-counter drift, duplicate metadata keys and
/// the schema stamp.
fn push_database_classes(db: &DbFacts, anomalies: &mut Vec<AnomalyClass>) {
    anomalies.extend(
        db.integrity_rows
            .iter()
            .map(|row| classify_integrity_row(row, &db.index_names)),
    );
    anomalies.extend(
        db.child_counter_drift
            .iter()
            .filter(|(_, stored, observed_max)| stored < observed_max)
            .map(
                |(parent_id, stored, observed_max)| AnomalyClass::ChildCounterDrift {
                    parent_id: parent_id.clone(),
                    stored: *stored,
                    observed_max: *observed_max,
                },
            ),
    );
    anomalies.extend(
        db.duplicate_metadata_keys
            .iter()
            .filter(|(_, rows)| *rows > 1)
            .map(|(key, rows)| AnomalyClass::DuplicateMetadataKey {
                key: key.clone(),
                rows: *rows,
            }),
    );
    let (observed, expected) = (db.schema_version, db.expected_schema_version);
    if observed < expected {
        anomalies.push(AnomalyClass::SchemaStampBehind { observed, expected });
    } else if observed > expected {
        anomalies.push(AnomalyClass::SchemaStampAhead { observed, expected });
    }
}

/// The `stale_recovery_artifacts` advisory: entries older than 30 days by their name's stamp, plus
/// every entry whose name does not parse.
fn stale_advisory(entries: &[crate::facts::RecoveryEntry], now: SystemTime) -> Option<Advisory> {
    let threshold = Duration::from_secs(STALE_AFTER_DAYS * SECONDS_PER_DAY);
    let mut count = 0usize;
    let mut oldest: Option<Duration> = None;
    for entry in entries {
        match entry.stamp_utc {
            None => count += 1,
            Some(stamp) => {
                let Ok(age) = now.duration_since(stamp) else {
                    continue;
                };
                if age > threshold {
                    count += 1;
                    oldest = oldest.max(Some(age));
                }
            }
        }
    }
    (count > 0).then(|| Advisory::StaleRecoveryArtifacts {
        count,
        oldest_days: oldest
            .map(|age| u32::try_from(age.as_secs() / SECONDS_PER_DAY).unwrap_or(u32::MAX)),
    })
}
