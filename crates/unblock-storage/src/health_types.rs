//! Health facts and repair requests (D58, spine §3.1) — the plain values
//! [`Storage::health_facts`](crate::Storage::health_facts) and
//! [`Storage::repair`](crate::Storage::repair) speak.
//!
//! Storage-owned and never on the wire: no `JsonSchema`, no `CONTRACT_HASH` impact. The engine maps
//! [`DbHealthFacts`] into `unblock-health`'s own `DbFacts`, so no storage type crosses into health
//! (D29 clause F3).

use std::path::PathBuf;

/// The database-internal facts one health classification reads (D58 clause (5)).
///
/// Every list is ordered by its key in SQL (NFR-14).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DbHealthFacts {
    /// The rows of the ONE `PRAGMA integrity_check`, with `ok` normalized to no rows.
    pub integrity_rows: Vec<String>,
    /// `PRAGMA user_version`.
    pub schema_version: i64,
    /// Every `type = 'index'` name in `sqlite_schema`, ordered by name.
    pub index_names: Vec<String>,
    /// Every `child_counters` row below its parent's greatest direct child, ordered by `parent_id`.
    pub child_counter_drift: Vec<ChildCounterDrift>,
    /// Every `metadata` key held by two or more rows, ordered by key.
    pub duplicate_metadata_keys: Vec<DuplicateKey>,
}

/// A `child_counters` row whose `last_child` is below the greatest direct-child number among the
/// `issues` ids `<parent_id>.<N>`, tombstones included. Reported only when `stored < observed_max`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildCounterDrift {
    /// The parent id the counter row names.
    pub parent_id: String,
    /// The stored `last_child`.
    pub stored: u32,
    /// The greatest direct-child number found under `parent_id`.
    pub observed_max: u32,
}

/// A `metadata` key held by two or more rows (the table has no key constraint).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateKey {
    /// The duplicated key.
    pub key: String,
    /// How many rows hold it (`>= 2`).
    pub rows: u32,
}

/// One [`Storage::repair`](crate::Storage::repair) call: the evidence snapshot, the optional
/// migrate, and the SQL actions run in one transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairRequest {
    /// The `VACUUM INTO` target. It must not exist and must lie under `<db dir>/.recovery/`.
    pub snapshot: PathBuf,
    /// Run the D46 migration ladder after the snapshot and before the actions.
    pub migrate: bool,
    /// The actions, applied in order inside ONE `BEGIN IMMEDIATE`.
    pub actions: Vec<RepairAction>,
}

/// One online SQL repair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepairAction {
    /// `REINDEX "<index>"`; the name is re-checked against `sqlite_schema` first.
    Reindex {
        /// The index to rebuild.
        index: String,
    },
    /// `last_child = MAX(last_child, to)`; never lowers a counter (D21).
    RaiseChildCounter {
        /// The counter row's parent id.
        parent_id: String,
        /// The child number the counter must reach.
        to: u32,
    },
    /// Keep the row with the GREATEST rowid for `key` and delete the rest.
    DedupeMetadataKey {
        /// The duplicated key.
        key: String,
    },
}

/// What a successful [`Storage::repair`](crate::Storage::repair) did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairOutcome {
    /// The snapshot written by `VACUUM INTO`.
    pub snapshot: PathBuf,
    /// The snapshot's size in bytes.
    pub snapshot_bytes: u64,
    /// `(from, to)` schema versions when the request ran the migration ladder.
    pub migrated: Option<(i64, i64)>,
    /// The actions applied, in order.
    pub applied: Vec<RepairAction>,
}
