//! The D58 clause (1) class table — the one place that maps each anomaly class to its code, its
//! severity, its planned action and that action's repair path.
//!
//! The levels are defined by repair path. `Drifted` classes plan an online recompute, `Recoverable`
//! classes plan an in-product repair, and every `Unsafe` class plans a manual step and nothing
//! `--repair` runs.

use std::fmt;

use crate::level::HealthLevel;

/// The manual path for a lost database (D58 clause (7)).
const LOST_DATABASE_PATH: &str = "Stop every unblock process. Restore the database file from a \
backup or from the newest .unblock/.recovery/<run>/snapshot.db by copying it to the configured \
database path; the snapshot is a complete VACUUM INTO copy carrying its schema stamp. Before \
restoring, with every unblock process stopped, move any -wal, -shm or -journal file left beside \
the database path into a new directory under .unblock/.recovery/, because a stale -wal is replayed \
into a restored file on its first open. Otherwise, after moving those files aside the same way, run \
`unblock init`, whose database-only mode creates an empty database under the configured \
db_filename, keeps config.toml and refuses while any such file remains, then import the last export \
through an MCP client's `sync` tool `import` action; the CLI has no import command.";

/// The manual path for a database file that is not `SQLite` (D58 clause (1), tracker comment 389).
const NOT_SQLITE_PATH: &str = "No in-product repair exists, and `unblock doctor --repair` moves \
nothing. Stop every unblock process, every `unblock mcp` server and every CLI command. Move the \
invalid database file and any -wal, -shm or -journal file beside it by hand into a new directory \
under .unblock/.recovery/. Then run `unblock init`, whose database-only mode creates an empty \
database under the configured db_filename and keeps config.toml, and import the last export \
through an MCP client's `sync` tool `import` action; the CLI has no import command.";

/// The manual path for a database file unblock cannot read.
const UNREADABLE_DATABASE_PATH: &str = "Fix the database file's permissions or ownership so \
unblock can read it, then run `unblock doctor` again. unblock never moves an unreadable database \
file.";

/// The manual path for page-level corruption (D58 clause (3)).
const PAGE_CORRUPTION_PATH: &str = "No in-product repair exists. Stop every unblock process and \
copy the database file aside. Salvage it outside unblock with the sqlite3 shell's `.recover` \
command, or restore it from a backup. Otherwise move the database file and any -wal, -shm or \
-journal file beside it into a new directory under .unblock/.recovery/, run `unblock init` to \
create an empty database, and import the last export through an MCP client's `sync` tool `import` \
action.";

/// The manual path for a database a newer binary migrated.
const SCHEMA_AHEAD_PATH: &str = "A newer unblock binary migrated this database. Run `unblock \
update` to install a binary that reads this schema.";

/// The manual path for a sidecar beside a missing database, where no exclusive window can run.
const SIDECAR_WITHOUT_DATABASE_PATH: &str = "No database file exists, so no exclusive repair can \
run. Stop every unblock process, then move this file into a new directory under \
.unblock/.recovery/.";

/// One anomaly class, in D58 clause (1) report order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnomalyClass {
    /// The database file is absent or 0 bytes while `.unblock/.write.lock` exists.
    DatabaseLost,
    /// The database file exists but its first bytes cannot be read.
    DatabaseUnreadable {
        /// The I/O error text.
        reason: String,
    },
    /// The database file holds bytes that do not start with the `SQLite` magic.
    DatabaseNotSqlite {
        /// The file length in bytes.
        len: u64,
    },
    /// A `-shm` sidecar exists without a `-wal`.
    SidecarMismatch,
    /// A `-wal` sidecar is shorter than its 32-byte header.
    TruncatedWal {
        /// The `-wal` length in bytes (`0 < len < 32`).
        len: u64,
    },
    /// A rollback `-journal` sidecar exists.
    JournalSidecarPresent,
    /// The export carries git conflict markers.
    JsonlConflictMarkers {
        /// The 1-based line of the first marker.
        first_line: usize,
    },
    /// The conflict-marker scanner refused the export.
    JsonlUnscannable {
        /// The scanner's error text.
        reason: String,
    },
    /// An index-only `integrity_check` row.
    IndexCorruption {
        /// The index the row names.
        index: String,
        /// The row, without its `*** in database main ***` line.
        row: String,
    },
    /// Any other `integrity_check` row.
    PageCorruption {
        /// The row, without its `*** in database main ***` line.
        row: String,
    },
    /// A `child_counters` row below its parent's greatest direct-child number.
    ChildCounterDrift {
        /// The parent issue id.
        parent_id: String,
        /// The stored `last_child`.
        stored: u32,
        /// The greatest direct-child number among the parent's children, tombstones included.
        observed_max: u32,
    },
    /// Two or more `metadata` rows share a key.
    DuplicateMetadataKey {
        /// The shared key.
        key: String,
        /// How many rows hold it.
        rows: u32,
    },
    /// The schema stamp is below the version this build expects.
    SchemaStampBehind {
        /// The stamp on disk.
        observed: i64,
        /// The version this build expects.
        expected: i64,
    },
    /// The schema stamp is above the version this build expects.
    SchemaStampAhead {
        /// The stamp on disk.
        observed: i64,
        /// The version this build expects.
        expected: i64,
    },
}

impl AnomalyClass {
    /// The stable snake-case code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::DatabaseLost => "database_lost",
            Self::DatabaseUnreadable { .. } => "database_unreadable",
            Self::DatabaseNotSqlite { .. } => "database_not_sqlite",
            Self::SidecarMismatch => "sidecar_mismatch",
            Self::TruncatedWal { .. } => "truncated_wal",
            Self::JournalSidecarPresent => "journal_sidecar_present",
            Self::JsonlConflictMarkers { .. } => "jsonl_conflict_markers",
            Self::JsonlUnscannable { .. } => "jsonl_unscannable",
            Self::IndexCorruption { .. } => "index_corruption",
            Self::PageCorruption { .. } => "page_corruption",
            Self::ChildCounterDrift { .. } => "child_counter_drift",
            Self::DuplicateMetadataKey { .. } => "duplicate_metadata_key",
            Self::SchemaStampBehind { .. } => "schema_stamp_behind",
            Self::SchemaStampAhead { .. } => "schema_stamp_ahead",
        }
    }

    /// The class's position in D58 clause (1) report order.
    pub(crate) fn rank(&self) -> u8 {
        match self {
            Self::DatabaseLost => 0,
            Self::DatabaseUnreadable { .. } => 1,
            Self::DatabaseNotSqlite { .. } => 2,
            Self::SidecarMismatch => 3,
            Self::TruncatedWal { .. } => 4,
            Self::JournalSidecarPresent => 5,
            Self::JsonlConflictMarkers { .. } => 6,
            Self::JsonlUnscannable { .. } => 7,
            Self::IndexCorruption { .. } => 8,
            Self::PageCorruption { .. } => 9,
            Self::ChildCounterDrift { .. } => 10,
            Self::DuplicateMetadataKey { .. } => 11,
            Self::SchemaStampBehind { .. } => 12,
            Self::SchemaStampAhead { .. } => 13,
        }
    }

    /// The level this class alone puts the workspace in.
    #[must_use]
    pub fn severity(&self) -> HealthLevel {
        match self {
            Self::DatabaseLost
            | Self::DatabaseUnreadable { .. }
            | Self::DatabaseNotSqlite { .. }
            | Self::PageCorruption { .. }
            | Self::SchemaStampAhead { .. } => HealthLevel::Unsafe,
            Self::SidecarMismatch
            | Self::TruncatedWal { .. }
            | Self::JournalSidecarPresent
            | Self::JsonlConflictMarkers { .. }
            | Self::JsonlUnscannable { .. }
            | Self::IndexCorruption { .. } => HealthLevel::Recoverable,
            Self::ChildCounterDrift { .. }
            | Self::DuplicateMetadataKey { .. }
            | Self::SchemaStampBehind { .. } => HealthLevel::Drifted,
        }
    }

    /// The repair this class plans.
    #[must_use]
    pub fn planned(&self) -> PlannedRepair {
        let (action, path) = match self {
            Self::DatabaseLost => (
                PlannedAction::Manual {
                    text: LOST_DATABASE_PATH,
                },
                RepairPath::Manual,
            ),
            Self::DatabaseUnreadable { .. } => (
                PlannedAction::Manual {
                    text: UNREADABLE_DATABASE_PATH,
                },
                RepairPath::Manual,
            ),
            Self::DatabaseNotSqlite { .. } => (
                PlannedAction::Manual {
                    text: NOT_SQLITE_PATH,
                },
                RepairPath::Manual,
            ),
            Self::SidecarMismatch => (
                PlannedAction::QuarantineSidecar,
                RepairPath::PreopenExclusive,
            ),
            Self::TruncatedWal { .. } => (
                PlannedAction::CheckpointTruncate,
                RepairPath::PreopenExclusive,
            ),
            Self::JournalSidecarPresent => (
                PlannedAction::QuarantineJournal,
                RepairPath::PreopenExclusive,
            ),
            Self::JsonlConflictMarkers { .. } | Self::JsonlUnscannable { .. } => {
                (PlannedAction::RegenerateExport, RepairPath::Online)
            }
            Self::IndexCorruption { index, .. } => (
                PlannedAction::Reindex {
                    index: index.clone(),
                },
                RepairPath::Online,
            ),
            Self::PageCorruption { .. } => (
                PlannedAction::Manual {
                    text: PAGE_CORRUPTION_PATH,
                },
                RepairPath::Manual,
            ),
            Self::ChildCounterDrift {
                parent_id,
                observed_max,
                ..
            } => (
                PlannedAction::RaiseChildCounter {
                    parent_id: parent_id.clone(),
                    to: *observed_max,
                },
                RepairPath::Online,
            ),
            Self::DuplicateMetadataKey { key, .. } => (
                PlannedAction::DedupeMetadataKey { key: key.clone() },
                RepairPath::Online,
            ),
            Self::SchemaStampBehind { .. } => (PlannedAction::Migrate, RepairPath::Online),
            Self::SchemaStampAhead { .. } => (
                PlannedAction::Manual {
                    text: SCHEMA_AHEAD_PATH,
                },
                RepairPath::Manual,
            ),
        };
        PlannedRepair {
            anomaly: self.code(),
            action,
            path,
        }
    }

    /// The repair planned when no database file exists. An exclusive action needs a database file,
    /// so a sidecar class falls back to moving the file aside by hand.
    pub(crate) fn planned_without_database(&self) -> PlannedRepair {
        let planned = self.planned();
        if planned.path == RepairPath::PreopenExclusive {
            PlannedRepair {
                anomaly: planned.anomaly,
                action: PlannedAction::Manual {
                    text: SIDECAR_WITHOUT_DATABASE_PATH,
                },
                path: RepairPath::Manual,
            }
        } else {
            planned
        }
    }
}

impl fmt::Display for AnomalyClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DatabaseLost => f.write_str(
                "the database file is absent or empty while .unblock/.write.lock records that one \
                 existed",
            ),
            Self::DatabaseUnreadable { reason } => {
                write!(f, "the database file cannot be read: {reason}")
            }
            Self::DatabaseNotSqlite { len } => write!(
                f,
                "the database file ({len} bytes) does not start with the SQLite header"
            ),
            Self::SidecarMismatch => f.write_str("a -shm file exists without a -wal file"),
            Self::TruncatedWal { len } => write!(
                f,
                "the -wal file holds {len} bytes, fewer than its 32-byte header"
            ),
            Self::JournalSidecarPresent => f.write_str(
                "a rollback -journal file exists; the database classes are read only after \
                 `unblock doctor --repair` lets SQLite roll the journal back",
            ),
            Self::JsonlConflictMarkers { first_line } => write!(
                f,
                "the JSONL export carries git conflict markers, the first at line {first_line}"
            ),
            Self::JsonlUnscannable { reason } => {
                write!(f, "the JSONL export cannot be scanned: {reason}")
            }
            Self::IndexCorruption { index, row } => {
                write!(f, "index {index} disagrees with its table: {row}")
            }
            Self::PageCorruption { row } => write!(f, "integrity_check reports: {row}"),
            Self::ChildCounterDrift {
                parent_id,
                stored,
                observed_max,
            } => write!(
                f,
                "the child counter of {parent_id} is {stored}, below its greatest child number \
                 {observed_max}"
            ),
            Self::DuplicateMetadataKey { key, rows } => {
                write!(f, "the metadata key {key} is held by {rows} rows")
            }
            Self::SchemaStampBehind { observed, expected } => write!(
                f,
                "the schema stamp {observed} is behind this build's {expected}"
            ),
            Self::SchemaStampAhead { observed, expected } => write!(
                f,
                "the schema stamp {observed} is ahead of this build's {expected}"
            ),
        }
    }
}

/// A finding that is reported but never raises the level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Advisory {
    /// No database file (or a 0-byte one) and no `.unblock/.write.lock`: a fresh clone or worktree.
    DatabaseNotCreated,
    /// The JSONL export path is the session database or one of its `-wal`/`-shm`/`-journal` sidecars
    /// (PRD §4 D5 amendment, `ub-c15`): it was not scanned, and `doctor --repair` plans no
    /// regeneration of it.
    ExportIsSessionFile,
    /// Run directories under `.unblock/.recovery/` older than 30 days, or whose name does not parse.
    StaleRecoveryArtifacts {
        /// How many run directories are stale.
        count: usize,
        /// The age in whole days of the oldest one whose name parses, or `None` when none parses.
        oldest_days: Option<u32>,
    },
    /// Another process holds `.unblock/.write.lock` right now.
    WriteLockHeld,
}

impl Advisory {
    /// The stable snake-case code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::DatabaseNotCreated => "database_not_created",
            Self::ExportIsSessionFile => "export_is_session_file",
            Self::StaleRecoveryArtifacts { .. } => "stale_recovery_artifacts",
            Self::WriteLockHeld => "write_lock_held",
        }
    }
}

impl fmt::Display for Advisory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DatabaseNotCreated => f.write_str(
                "no database file and no .unblock/.write.lock exist (a fresh clone or worktree); \
                 the next ordinary command creates the database",
            ),
            Self::ExportIsSessionFile => f.write_str(
                "the JSONL export path is the session database or one of its -wal/-shm/-journal \
                 sidecars; it was not scanned, and doctor --repair does not regenerate it; point \
                 jsonl_export_filename at a file that is not the database, a sidecar, or a link to \
                 either",
            ),
            Self::StaleRecoveryArtifacts { count, oldest_days } => {
                write!(
                    f,
                    "{count} run directories under .unblock/.recovery/ are older than 30 days or \
                     carry no date"
                )?;
                match oldest_days {
                    Some(days) => write!(f, "; the oldest dated one is {days} days old"),
                    None => Ok(()),
                }
            }
            Self::WriteLockHeld => {
                f.write_str("another process holds .unblock/.write.lock right now")
            }
        }
    }
}

/// The repair one anomaly plans.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedRepair {
    /// The anomaly's code.
    pub anomaly: &'static str,
    /// The action that repairs it.
    pub action: PlannedAction,
    /// Where the action runs.
    pub path: RepairPath,
}

/// A repair action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedAction {
    /// Move a `-shm` without a `-wal` into `quarantine/`.
    QuarantineSidecar,
    /// Let `SQLite` roll a hot `-journal` back. The rollback is the repair and the `raw/` copy is
    /// its evidence; nothing is renamed.
    QuarantineJournal,
    /// Run `PRAGMA wal_checkpoint(TRUNCATE)` under exclusive locking.
    CheckpointTruncate,
    /// `REINDEX` one index.
    Reindex {
        /// The index name.
        index: String,
    },
    /// Raise a `child_counters` row; never lowers it.
    RaiseChildCounter {
        /// The parent issue id.
        parent_id: String,
        /// The new floor for `last_child`.
        to: u32,
    },
    /// Keep the greatest-rowid row of a duplicated `metadata` key.
    DedupeMetadataKey {
        /// The duplicated key.
        key: String,
    },
    /// Run the schema migration ladder after the snapshot.
    Migrate,
    /// Regenerate the JSONL export from the database.
    RegenerateExport,
    /// No in-product repair exists; `text` names the manual path.
    Manual {
        /// The manual recovery path.
        text: &'static str,
    },
}

impl PlannedAction {
    /// The stable snake-case code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::QuarantineSidecar => "quarantine_sidecar",
            Self::QuarantineJournal => "quarantine_journal",
            Self::CheckpointTruncate => "checkpoint_truncate",
            Self::Reindex { .. } => "reindex",
            Self::RaiseChildCounter { .. } => "raise_child_counter",
            Self::DedupeMetadataKey { .. } => "dedupe_metadata_key",
            Self::Migrate => "migrate",
            Self::RegenerateExport => "regenerate_export",
            Self::Manual { .. } => "manual",
        }
    }

    /// The object the action targets, when it names one.
    #[must_use]
    pub fn target(&self) -> Option<String> {
        match self {
            Self::Reindex { index } => Some(index.clone()),
            Self::RaiseChildCounter { parent_id, .. } => Some(parent_id.clone()),
            Self::DedupeMetadataKey { key } => Some(key.clone()),
            Self::QuarantineSidecar
            | Self::QuarantineJournal
            | Self::CheckpointTruncate
            | Self::Migrate
            | Self::RegenerateExport
            | Self::Manual { .. } => None,
        }
    }
}

/// Where a repair action runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RepairPath {
    /// Inside the pre-open exclusive window, before any session exists.
    PreopenExclusive,
    /// Inside `Session::recover()`, under the permit and `.write.lock`.
    Online,
    /// Outside unblock, by hand.
    Manual,
}

impl RepairPath {
    /// The stable label (`preopen-exclusive`, `online` or `manual`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PreopenExclusive => "preopen-exclusive",
            Self::Online => "online",
            Self::Manual => "manual",
        }
    }
}

impl fmt::Display for RepairPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
