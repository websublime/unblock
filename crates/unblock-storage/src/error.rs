//! [`StorageError`] — the backend-agnostic error the [`crate::Storage`] trait returns (spine §2.1,
//! §3.1), plus the opaque [`BackendOpaque`] wrapper that absorbs backend (libsql) failures.
//!
//! `StorageError` implements [`unblock_error::CodedError`] (NOT a bespoke inherent `code()`) —
//! mirroring [`unblock_error::ModelError`] — so the L7 boundary bridges it to a
//! [`unblock_error::StructuredError`] uniformly via the blanket `From<&E>` / `from_coded` path.
//!
//! No backend type ever appears in any public signature (spine §6 rule 2): a libsql error is
//! absorbed into [`BackendOpaque`], whose message is sanitized **at construction** and whose inner
//! `String` is never publicly reachable. The `From<libsql::Error> for BackendOpaque` conversion
//! (added at T0.6) is the only place that names a libsql type; the crate-internal
//! [`map_libsql_err`] routes the two retryable busy/locked codes to
//! [`StorageError::DatabaseLocked`] and everything else to [`StorageError::Backend`].

use std::fmt;

use serde_json::{Map, Value};
use snafu::Snafu;
use unblock_error::{CodedError, ErrorCode, sanitize_message};

use crate::libsql::{BASELINE_SCHEMA_VERSION, COMMENTS_STEP_VERSION, CONFIG_STEP_VERSION};

/// One piece of schema the migration SENTINEL found missing (D57 clause (9)) — the structured
/// carrier of the lying-stamp dispatch on [`StorageError::Migration`]'s `gaps`.
///
/// Each gap names the piece and, through [`SchemaGap::step_version`], the forward step that adds it:
/// the `comments` columns are schema version 2's (D46), the `config` table and its columns version
/// 3's (D57).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaGap {
    /// A `comments` column step 2 adds is absent.
    CommentsColumn(&'static str),
    /// The `config` table step 3 creates is absent.
    ConfigTable,
    /// The `config` table exists but lacks a column step 3 creates.
    ConfigColumn(&'static str),
}

impl SchemaGap {
    /// The schema version whose forward step adds this piece.
    #[must_use]
    pub const fn step_version(self) -> i32 {
        match self {
            Self::CommentsColumn(_) => COMMENTS_STEP_VERSION,
            Self::ConfigTable | Self::ConfigColumn(_) => CONFIG_STEP_VERSION,
        }
    }

    /// The stamp the lying-stamp recovery resets to for this gap: the version BELOW the step that
    /// adds it, so replaying the ladder re-runs that step (D57 clause (9)).
    #[must_use]
    pub const fn reset_stamp(self) -> i32 {
        self.step_version() - 1
    }

    /// The gap's stable `context()` spelling: `comments.<column>`, `config`, `config.<column>`.
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::CommentsColumn(name) => format!("comments.{name}"),
            Self::ConfigTable => "config".to_string(),
            Self::ConfigColumn(name) => format!("config.{name}"),
        }
    }
}

/// Render the sentinel's `reason` from its gaps, naming each gap by the STEP that adds it — version
/// 2 for the `comments` columns, version 3 for the `config` table and columns (D57 clause (9)),
/// never [`crate::CURRENT_SCHEMA_VERSION`] for all of them.
#[must_use]
pub(crate) fn describe_gaps(gaps: &[SchemaGap]) -> String {
    let comments: Vec<&str> = gaps
        .iter()
        .filter_map(|g| match g {
            SchemaGap::CommentsColumn(name) => Some(*name),
            _ => None,
        })
        .collect();
    let config_columns: Vec<&str> = gaps
        .iter()
        .filter_map(|g| match g {
            SchemaGap::ConfigColumn(name) => Some(*name),
            _ => None,
        })
        .collect();
    let mut parts = Vec::new();
    if !comments.is_empty() {
        parts.push(format!(
            "the `comments` table is missing the column(s) {} that schema version \
             {COMMENTS_STEP_VERSION} adds",
            comments.join(", ")
        ));
    }
    if gaps.contains(&SchemaGap::ConfigTable) {
        parts.push(format!(
            "the `config` table that schema version {CONFIG_STEP_VERSION} creates is absent"
        ));
    }
    if !config_columns.is_empty() {
        parts.push(format!(
            "the `config` table is missing the column(s) {} that schema version \
             {CONFIG_STEP_VERSION} creates",
            config_columns.join(", ")
        ));
    }
    parts.join("; ")
}

/// The lying-stamp recovery for `gaps` (D57 clause (9)): one branch per gap kind, each of which
/// terminates — `comments` columns missing → reset the stamp to the BASELINE (`1`); `config` table
/// ABSENT → reset to `2`; `config` table PRESENT but missing a column → `DROP TABLE config` (its rows
/// are unreadable to every binary and are LOST), then reset to `2`. When several hold they combine in
/// that order with the LOWEST stamp, then `unblock migrate`.
fn lying_stamp_recovery(gaps: &[SchemaGap]) -> String {
    let drop_config = gaps.iter().any(|g| matches!(g, SchemaGap::ConfigColumn(_)));
    let reset_to = gaps
        .iter()
        .map(|g| g.reset_stamp())
        .min()
        .unwrap_or(BASELINE_SCHEMA_VERSION);
    let mut steps = Vec::new();
    if drop_config {
        steps.push(
            "drop the incomplete table with one `DROP TABLE config` write (its rows cannot be \
             read by any `unblock` build and are LOST)"
                .to_string(),
        );
    }
    steps.push(format!(
        "reset the stamp with one `PRAGMA user_version = {reset_to}` write against this database \
         file"
    ));
    steps.push("run `unblock migrate` in this workspace".to_string());
    steps.join(", then ")
}

/// The backend-agnostic error returned by every [`crate::Storage`] method (spine §2.1/§3.1).
///
/// Each variant maps to exactly one [`ErrorCode`] via the [`CodedError`] impl below (the L7
/// boundary turns that into a [`unblock_error::StructuredError`] and a 0–8 exit code). Backend
/// (libsql) failures are absorbed into [`StorageError::Backend`] as an opaque [`BackendOpaque`];
/// no backend type is ever public.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum StorageError {
    /// No issue exists for the given id.
    #[snafu(display("issue not found: {id}"))]
    IssueNotFound {
        /// The id that matched no issue.
        id: String,
    },

    /// No comment exists for the given comment id (FR-6/D37).
    ///
    /// This StorageError-level variant maps ONTO the EXISTING [`ErrorCode::IssueNotFound`] — the
    /// two levels are deliberately NOT 1:1 here (spine §3.1). FORK-E1 constrains the `ErrorCode`
    /// taxonomy (it stays at 36, so no exit-code-table or error-golden movement), not the internal
    /// enum; reusing `IssueNotFound { id }` here would render `issue 42 not found` when it was
    /// **comment** 42 that was missing — actively misleading in an agent-first tracker.
    #[snafu(display("comment not found: {id}"))]
    CommentNotFound {
        /// The comment id that matched no row (matches `Comment.id`'s own type).
        id: i64,
    },

    /// A partial id matched more than one issue (disambiguate and retry).
    #[snafu(display("ambiguous id: {id}"))]
    AmbiguousId {
        /// The ambiguous id prefix.
        id: String,
    },

    /// An id collided with an existing issue on create.
    #[snafu(display("id collision: {id}"))]
    IdCollision {
        /// The colliding id.
        id: String,
    },

    /// An id has an invalid format.
    #[snafu(display("invalid id: {id}"))]
    InvalidId {
        /// The rejected id.
        id: String,
    },

    /// The database is locked by another writer (retryable).
    #[snafu(display("database locked"))]
    DatabaseLocked,

    /// The on-disk schema version does not match the expected version.
    #[snafu(display("schema mismatch: found user_version {found}, expected {expected}"))]
    SchemaMismatch {
        /// The `PRAGMA user_version` found on disk.
        found: i32,
        /// The schema version this build expects.
        expected: i32,
    },

    /// The workspace database has not been initialized (`migrate` not run).
    #[snafu(display("workspace not initialized"))]
    NotInitialized,

    /// The workspace database is already initialized.
    #[snafu(display("workspace already initialized"))]
    AlreadyInitialized,

    /// An atomic claim lost the race: the issue is already held by another actor (FR-2; retryable).
    ///
    /// `by` is the **current holder**, re-read within the same transaction so the loser learns who
    /// won. It surfaces as `context["holder"]` for agent self-correction.
    #[snafu(display("issue {id} already claimed by {by}"))]
    AlreadyClaimed {
        /// The issue that could not be claimed.
        id: String,
        /// The actor currently holding the issue.
        by: String,
    },

    /// Adding the dependency would create a cycle in the ready-gating graph.
    ///
    /// `path` is the concrete cycle path (e.g. `"a -> b -> a"`); it surfaces as
    /// `context["cycle_path"]`.
    #[snafu(display("dependency cycle: {path}"))]
    CycleDetected {
        /// The detected cycle path.
        path: String,
    },

    /// The dependency target was not found.
    #[snafu(display("dependency target not found"))]
    DependencyNotFound,

    /// A declared dependency TARGET names no issue and is not an `external:` target (D45).
    ///
    /// The SECOND StorageError-level variant that maps ONTO the EXISTING
    /// [`ErrorCode::IssueNotFound`], for the same reason [`StorageError::CommentNotFound`] does
    /// (spine §3.1): FORK-E1 constrains the `ErrorCode` taxonomy (it stays at 36 — no exit-code-table
    /// re-bless, no error-golden churn), not the internal enum. `DependencyNotFound` is deliberately
    /// NOT reused: its published meaning is already "the EDGE does not exist" (from
    /// `remove_dependency`), and one code meaning both would leave an agent no way to tell them
    /// apart. `ValidationFailed` is not reused either — it is published as RETRYABLE, which would be
    /// a lie here (retrying the identical call cannot succeed).
    ///
    /// BOTH ids are carried because both are load-bearing on a batch path: on an import of 500
    /// records, `depends_on_id` alone would name the phantom without naming which record declared it.
    /// The `Display` text is deliberately NEUTRAL about the edge KIND — the guard runs over the
    /// distinct target set of EVERY declared dependency type and over the reparent path, whose target
    /// is a PARENT, so rendering "blocker" there would be misleading in exactly the way the
    /// `CommentNotFound` rationale rejects.
    #[snafu(display(
        "issue {issue_id} declares a dependency target that does not exist: {depends_on_id}"
    ))]
    BlockerNotFound {
        /// The issue that declared the edge (the dependent).
        issue_id: String,
        /// The dependency target that matched no row.
        depends_on_id: String,
    },

    /// The issue cannot be removed/modified because other issues depend on it.
    #[snafu(display("issue {id} has dependents"))]
    HasDependents {
        /// The issue that still has dependents.
        id: String,
    },

    /// An issue cannot depend on itself.
    #[snafu(display("an issue cannot depend on itself"))]
    SelfDependency,

    /// The dependency edge already exists.
    #[snafu(display("duplicate dependency"))]
    DuplicateDependency,

    /// A backend (libsql) operation failed; the cause is absorbed opaquely.
    #[snafu(display("backend failure: {source}"))]
    Backend {
        /// The sanitized, opaque backend cause (no backend type is public).
        source: BackendOpaque,
    },

    /// A schema migration failed.
    ///
    /// `from`/`to` are `PRAGMA user_version` values (`i32` to match the schema-version type);
    /// `reason` is a sanitized human description. Maps to [`ErrorCode::SchemaMismatch`].
    #[snafu(display("migration {from} -> {to} failed: {reason}"))]
    Migration {
        /// The starting `user_version`.
        from: i32,
        /// The target `user_version`.
        to: i32,
        /// Why the migration failed.
        reason: String,
        /// The schema pieces the lying-stamp SENTINEL found missing (D57 clause (9)): EMPTY for a
        /// genuine step failure, NON-EMPTY exactly when the sentinel raised this error. The hint
        /// dispatches on it, never on `from == to`.
        gaps: Vec<SchemaGap>,
    },

    /// `PRAGMA integrity_check` reported one or more problems.
    #[snafu(display("integrity check failed: {} message(s)", messages.len()))]
    IntegrityFailed {
        /// The integrity-check failure messages.
        messages: Vec<String>,
    },
}

impl CodedError for StorageError {
    fn code(&self) -> ErrorCode {
        match self {
            // D37/FORK-E1: `CommentNotFound` REUSES the issue-not-found code — the taxonomy does
            // not grow; only the StorageError level names the missing entity honestly. D45 adds
            // `BlockerNotFound` as the SECOND variant of that shape, for the same reason: no
            // `ErrorCode` is minted, so the 36-code map and the 0–8 exit table are byte-unchanged.
            Self::IssueNotFound { .. }
            | Self::CommentNotFound { .. }
            | Self::BlockerNotFound { .. } => ErrorCode::IssueNotFound,
            Self::AmbiguousId { .. } => ErrorCode::AmbiguousId,
            Self::IdCollision { .. } => ErrorCode::IdCollision,
            Self::InvalidId { .. } => ErrorCode::InvalidId,
            Self::DatabaseLocked => ErrorCode::DatabaseLocked,
            // `Migration` surfaces as `SchemaMismatch` (pinned in spine §3.1 + the crate plan).
            Self::SchemaMismatch { .. } | Self::Migration { .. } => ErrorCode::SchemaMismatch,
            Self::NotInitialized => ErrorCode::NotInitialized,
            Self::AlreadyInitialized => ErrorCode::AlreadyInitialized,
            Self::AlreadyClaimed { .. } => ErrorCode::AlreadyClaimed,
            Self::CycleDetected { .. } => ErrorCode::CycleDetected,
            Self::DependencyNotFound => ErrorCode::DependencyNotFound,
            Self::HasDependents { .. } => ErrorCode::HasDependents,
            Self::SelfDependency => ErrorCode::SelfDependency,
            Self::DuplicateDependency => ErrorCode::DuplicateDependency,
            // `Backend` and `IntegrityFailed` both surface as the generic `DatabaseError` code.
            Self::Backend { .. } | Self::IntegrityFailed { .. } => ErrorCode::DatabaseError,
        }
    }

    /// **D46 (v1.0.1) — THE HINT COMPOSITION SITE for the stale-schema failure (spine §3.2 clause
    /// (v), §2.2).** "Actionable" is delivered literally: the failure says WHAT HAPPENED and WHAT TO
    /// RUN, rendered from the FIELDS of the failing variant (which `src/libsql/migrations.rs`
    /// populates — the fields already exist and already feed `context()`, so this adds a rendering,
    /// not a data path).
    ///
    /// **ONE code serves TWO OPPOSITE directions, which is why the published shape is
    /// `HintShape::ContextualText` and not a per-code constant:** a database BELOW this build (or a
    /// stamp that lies about its shape) must be told to run `unblock migrate`; one ABOVE it must be
    /// told the opposite — upgrade the binary, do NOT migrate. A fixed per-code text would be wrong
    /// for one of them.
    ///
    /// **It may NOT advise a recovery the product cannot perform in that state:** "export and
    /// re-import" is invalid while the shape is stale, because `sync export` is itself among the
    /// tools a stale `comments` table breaks.
    ///
    /// **`Migration` SERVES TWO STATES AND THE TEXT BRANCHES ON THEM (Miguel's ruling, 2026-08-03,
    /// after the Verify gate; the DISPATCH TEST moved by D57 clause (9)).** On a GENUINE step failure
    /// `gaps` is EMPTY and `unblock migrate` IS the remedy — that arm is unchanged. On the
    /// LYING-STAMP path (`src/libsql/migrations.rs`'s sentinel) `gaps` is NON-EMPTY, and advising
    /// `unblock migrate` there would advise a command that CANNOT repair the state: the ladder has
    /// already run up to the stamp the database now carries, so a re-run re-emits this identical
    /// error forever. Until D57 the dispatch read `from == to`, a faithful proxy for "the sentinel
    /// fired" only while no step followed the lied-about stamp; once step 3 exists, a stamp-2
    /// database lying about `comments` runs step 3 FIRST and reaches the sentinel with `from = 2`,
    /// `to = 3`, so the proxy would hand it the stale-direction text. The dispatch is therefore
    /// `!gaps.is_empty()`, never `from == to`. The lying-stamp text says (i) WHAT IS ACTUALLY WRONG —
    /// the STAMP claims a shape the tables do not have, naming each missing piece by the step that
    /// adds it; (ii) THE RECOVERY THAT WORKS, branched per gap ([`SchemaGap::reset_stamp`]) and
    /// combined with the LOWEST stamp, then `unblock migrate`; and (iii) WHY THAT IS SAFE — step 2
    /// INSPECTS the table before acting and adds only the columns actually absent, and step 3 creates
    /// its table only if absent, leaving every existing row untouched. **There is deliberately no
    /// in-product rescue verb for this state**: the export path is broken by the same stale shape
    /// (`sync export` reads `comments`), so an honest, executable instruction is the entire remedy the
    /// user gets — which is exactly why it may not be replaced by a command that returns this error
    /// again.
    ///
    /// Every other variant keeps the trait default (`None`), so no other code's published
    /// `hint_shape` moves. **It only reaches a user because `unblock-config` FORWARDS it** on
    /// `DbOpenFailed`/`MigrationFailed` — the D46 step runs implicitly on open, and that boundary
    /// would otherwise drop it.
    fn hint(&self) -> Option<String> {
        match self {
            Self::Migration {
                from,
                to,
                reason,
                gaps,
            } => Some(if gaps.is_empty() {
                format!(
                    "the workspace database is at schema version {from}, but this build expects \
                     {to} ({reason}). Run `unblock migrate` in this workspace to bring it forward; \
                     the repair is additive and keeps every existing row."
                )
            } else {
                // THE LYING STAMP. The ladder already ran up to `to`, so `unblock migrate` alone is
                // skipped wholesale at this stamp and would re-emit this same error forever.
                let recovery = lying_stamp_recovery(gaps);
                format!(
                    "the workspace database is STAMPED at schema version {to} — the version this \
                     build expects — but its tables lack what the forward steps add ({reason}). \
                     The STAMP is what is wrong, not the migration ladder, so running `unblock \
                     migrate` on its own CANNOT repair it: every forward step is skipped at this \
                     stamp and this same error is returned again. To recover: {recovery}. That is \
                     safe — step {COMMENTS_STEP_VERSION} INSPECTS the `comments` table before it \
                     acts and adds only the columns actually missing, and step \
                     {CONFIG_STEP_VERSION} creates the `config` table only if it is absent, so \
                     replaying them leaves every existing row untouched."
                )
            }),
            Self::SchemaMismatch { found, expected } => Some(format!(
                "the workspace database is at schema version {found}, which is NEWER than the {expected} \
                 this build understands — it was written by a newer `unblock`. Upgrade this binary \
                 (`unblock update`) and retry; do NOT downgrade the database."
            )),
            // Enumerated rather than a catch-all `_`: a future variant must DECLARE whether it
            // carries a hint, exactly as it must declare its `code()`. A catch-all would silently
            // publish `None` for a variant whose failure genuinely needs guidance.
            Self::IssueNotFound { .. }
            | Self::CommentNotFound { .. }
            | Self::BlockerNotFound { .. }
            | Self::AmbiguousId { .. }
            | Self::IdCollision { .. }
            | Self::InvalidId { .. }
            | Self::DatabaseLocked
            | Self::NotInitialized
            | Self::AlreadyInitialized
            | Self::AlreadyClaimed { .. }
            | Self::CycleDetected { .. }
            | Self::DependencyNotFound
            | Self::HasDependents { .. }
            | Self::SelfDependency
            | Self::DuplicateDependency
            | Self::Backend { .. }
            | Self::IntegrityFailed { .. } => None,
        }
    }

    // `retryable()` is intentionally the default (`code().is_retryable()`): the retryable storage
    // codes are exactly {DatabaseLocked, AlreadyClaimed, AmbiguousId} per unblock-error's pinned
    // set, and the unit tests assert that mapping holds.

    fn context(&self) -> Map<String, Value> {
        let mut map = Map::new();
        match self {
            Self::IssueNotFound { id }
            | Self::AmbiguousId { id }
            | Self::IdCollision { id }
            | Self::InvalidId { id }
            | Self::HasDependents { id } => {
                map.insert("id".to_string(), Value::String(id.clone()));
            }
            Self::AlreadyClaimed { id, by } => {
                map.insert("id".to_string(), Value::String(id.clone()));
                map.insert("holder".to_string(), Value::String(by.clone()));
            }
            Self::CommentNotFound { id } => {
                // The code() is IssueNotFound, but the context key stays honest about WHICH
                // entity was missing (spine §3.1).
                map.insert("comment_id".to_string(), Value::from(*id));
            }
            Self::BlockerNotFound {
                issue_id,
                depends_on_id,
            } => {
                // D45 — the same discipline `CommentNotFound` uses: code() is IssueNotFound, but the
                // keys stay honest about WHICH entity was missing. The key is `blocker_id`, NOT
                // `id`, and `issue_id` names the record that DECLARED the edge (load-bearing on a
                // batch path). Adding context KEYS moves no schema byte — `context` is a free-form
                // map (spine §2.4).
                map.insert("issue_id".to_string(), Value::String(issue_id.clone()));
                map.insert(
                    "blocker_id".to_string(),
                    Value::String(depends_on_id.clone()),
                );
            }
            Self::CycleDetected { path } => {
                map.insert("cycle_path".to_string(), Value::String(path.clone()));
            }
            Self::SchemaMismatch { found, expected } => {
                map.insert("found".to_string(), Value::from(*found));
                map.insert("expected".to_string(), Value::from(*expected));
            }
            Self::Migration {
                from,
                to,
                reason,
                gaps,
            } => {
                map.insert("from".to_string(), Value::from(*from));
                map.insert("to".to_string(), Value::from(*to));
                map.insert("reason".to_string(), Value::String(reason.clone()));
                // D57 clause (9): the sentinel's gaps, by their stable `context()` spelling. Absent
                // on a genuine step failure (empty `gaps`), so its context is unchanged.
                if !gaps.is_empty() {
                    let array = gaps.iter().map(|g| Value::String(g.label())).collect();
                    map.insert("gaps".to_string(), Value::Array(array));
                }
            }
            Self::IntegrityFailed { messages } => {
                let array = messages.iter().map(|m| Value::String(m.clone())).collect();
                map.insert("messages".to_string(), Value::Array(array));
            }
            // No structured payload beyond the (already sanitized) Display message.
            Self::DatabaseLocked
            | Self::NotInitialized
            | Self::AlreadyInitialized
            | Self::DependencyNotFound
            | Self::SelfDependency
            | Self::DuplicateDependency
            | Self::Backend { .. } => {}
        }
        map
    }
}

/// An opaque, terminal-sanitized wrapper around a backend (libsql) failure.
///
/// The inner message is sanitized **at construction** via [`unblock_error::sanitize_message`] and
/// the inner `String` is never publicly reachable — so neither the backend's concrete type nor any
/// raw control byte from a backend message can escape through the public API (spine §6 rule 2,
/// NFR-14). Only `Debug`, `Display`, and [`std::error::Error`] are exposed.
///
/// The [`From<libsql::Error>`] conversion below — the single place that names a libsql type —
/// runs the same sanitize-at-construction path via [`BackendOpaque::from_message`].
#[derive(Debug)]
pub struct BackendOpaque(String);

impl BackendOpaque {
    /// Build an opaque backend error from a message, sanitizing it at construction.
    ///
    /// The message is routed through [`unblock_error::sanitize_message`] immediately, so a stored
    /// `BackendOpaque` can never carry raw control bytes regardless of how it is later rendered.
    pub(crate) fn from_message(message: impl Into<String>) -> Self {
        Self(sanitize_message(&message.into()).into_owned())
    }
}

impl fmt::Display for BackendOpaque {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Already sanitized at construction; emit the safe text verbatim.
        f.write_str(&self.0)
    }
}

impl std::error::Error for BackendOpaque {}

impl From<libsql::Error> for BackendOpaque {
    /// Absorb a libsql error opaquely: render it via `Display` and sanitize at construction.
    ///
    /// This is the **only** place that names a libsql type. The concrete `libsql::Error` value is
    /// consumed and reduced to a sanitized `String`; neither the backend type nor any raw control
    /// byte from its message can escape through the public API (spine §6 rule 2, NFR-14).
    fn from(error: libsql::Error) -> Self {
        Self::from_message(error.to_string())
    }
}

/// `SQLite` primary result code for a busy database (`SQLITE_BUSY`).
const SQLITE_BUSY: i32 = 5;
/// `SQLite` primary result code for a locked table/database (`SQLITE_LOCKED`).
const SQLITE_LOCKED: i32 = 6;

/// Map a backend (libsql) error to the backend-agnostic [`StorageError`].
///
/// The two retryable concurrency codes — `SQLITE_BUSY` (5) and `SQLITE_LOCKED` (6), matched on the
/// low byte of the (possibly extended) result code — surface as the retryable
/// [`StorageError::DatabaseLocked`]; every other libsql failure is absorbed opaquely into
/// [`StorageError::Backend`]. The catch-all arm is **required**: `libsql::Error` is
/// `#[non_exhaustive]`, so new variants must still compile.
pub(crate) fn map_libsql_err(error: libsql::Error) -> StorageError {
    match error {
        libsql::Error::SqliteFailure(code, _)
            if (code & 0xff) == SQLITE_BUSY || (code & 0xff) == SQLITE_LOCKED =>
        {
            StorageError::DatabaseLocked
        }
        other => StorageError::Backend {
            source: BackendOpaque::from(other),
        },
    }
}

/// Whether a libsql error is the retryable busy/locked concurrency signal (`SQLITE_BUSY` /
/// `SQLITE_LOCKED`, matched on the low byte of the possibly-extended result code).
///
/// Used by the libsql write path to witness write-lock contention without consuming the error: the
/// T0.8 contention lab's busy-witness probe and forced-spin control both branch on this. It mirrors
/// the exact code set [`map_libsql_err`] routes to [`StorageError::DatabaseLocked`].
pub(crate) fn is_busy_locked(error: &libsql::Error) -> bool {
    matches!(
        error,
        libsql::Error::SqliteFailure(code, _)
            if (code & 0xff) == SQLITE_BUSY || (code & 0xff) == SQLITE_LOCKED
    )
}

#[cfg(test)]
mod tests {
    use super::{BackendOpaque, SchemaGap, StorageError};
    use unblock_error::{CodedError, ErrorCode, StructuredError};

    /// Golden `StorageError` -> `ErrorCode` table (mirrors the `unblock-error` golden). Every
    /// variant is constructed explicitly so adding a variant forces this table to be extended.
    #[test]
    fn every_variant_maps_to_expected_code() {
        let cases: Vec<(StorageError, ErrorCode)> = vec![
            (
                StorageError::IssueNotFound { id: "ub-1".into() },
                ErrorCode::IssueNotFound,
            ),
            (
                StorageError::AmbiguousId { id: "ub".into() },
                ErrorCode::AmbiguousId,
            ),
            (
                StorageError::IdCollision { id: "ub-1".into() },
                ErrorCode::IdCollision,
            ),
            (
                StorageError::InvalidId { id: "!!".into() },
                ErrorCode::InvalidId,
            ),
            (StorageError::DatabaseLocked, ErrorCode::DatabaseLocked),
            (
                StorageError::SchemaMismatch {
                    found: 1,
                    expected: 2,
                },
                ErrorCode::SchemaMismatch,
            ),
            (StorageError::NotInitialized, ErrorCode::NotInitialized),
            (
                StorageError::AlreadyInitialized,
                ErrorCode::AlreadyInitialized,
            ),
            (
                StorageError::AlreadyClaimed {
                    id: "ub-1".into(),
                    by: "alice".into(),
                },
                ErrorCode::AlreadyClaimed,
            ),
            (
                StorageError::CycleDetected {
                    path: "a -> b -> a".into(),
                },
                ErrorCode::CycleDetected,
            ),
            (
                StorageError::DependencyNotFound,
                ErrorCode::DependencyNotFound,
            ),
            (
                StorageError::CommentNotFound { id: 42 },
                ErrorCode::IssueNotFound,
            ),
            (
                StorageError::BlockerNotFound {
                    issue_id: "ub-1".into(),
                    depends_on_id: "ub-ghost".into(),
                },
                ErrorCode::IssueNotFound,
            ),
            (
                StorageError::HasDependents { id: "ub-1".into() },
                ErrorCode::HasDependents,
            ),
            (StorageError::SelfDependency, ErrorCode::SelfDependency),
            (
                StorageError::DuplicateDependency,
                ErrorCode::DuplicateDependency,
            ),
            (
                StorageError::Backend {
                    source: BackendOpaque::from_message("boom"),
                },
                ErrorCode::DatabaseError,
            ),
            (
                StorageError::Migration {
                    from: 1,
                    to: 2,
                    reason: "bad step".into(),
                    gaps: Vec::new(),
                },
                ErrorCode::SchemaMismatch,
            ),
            (
                StorageError::IntegrityFailed {
                    messages: vec!["corrupt page".into()],
                },
                ErrorCode::DatabaseError,
            ),
        ];
        for (err, expected) in cases {
            assert_eq!(err.code(), expected, "wrong code for {err:?}");
        }
    }

    /// The exact retryable set: {`DatabaseLocked`, `AlreadyClaimed`, `AmbiguousId`} -> true; rest false.
    #[test]
    fn retryable_set_is_pinned() {
        let retryable: Vec<StorageError> = vec![
            StorageError::DatabaseLocked,
            StorageError::AlreadyClaimed {
                id: "ub-1".into(),
                by: "bob".into(),
            },
            StorageError::AmbiguousId { id: "ub".into() },
        ];
        for err in &retryable {
            assert!(err.retryable(), "{err:?} should be retryable");
        }

        let non_retryable: Vec<StorageError> = vec![
            StorageError::IssueNotFound { id: "ub-1".into() },
            StorageError::IdCollision { id: "ub-1".into() },
            StorageError::InvalidId { id: "x".into() },
            StorageError::SchemaMismatch {
                found: 1,
                expected: 2,
            },
            StorageError::NotInitialized,
            StorageError::AlreadyInitialized,
            StorageError::CycleDetected { path: "a".into() },
            StorageError::DependencyNotFound,
            StorageError::HasDependents { id: "ub-1".into() },
            StorageError::SelfDependency,
            StorageError::DuplicateDependency,
            StorageError::BlockerNotFound {
                issue_id: "ub-1".into(),
                depends_on_id: "ub-ghost".into(),
            },
            StorageError::Backend {
                source: BackendOpaque::from_message("x"),
            },
            StorageError::Migration {
                from: 1,
                to: 2,
                reason: "x".into(),
                gaps: Vec::new(),
            },
            StorageError::IntegrityFailed {
                messages: vec!["x".into()],
            },
        ];
        for err in &non_retryable {
            assert!(!err.retryable(), "{err:?} should NOT be retryable");
        }
    }

    /// `AlreadyClaimed{by}`'s holder survives the bridge into a `StructuredError`'s `context`.
    #[test]
    fn already_claimed_holder_survives_into_structured() {
        let err = StorageError::AlreadyClaimed {
            id: "ub-42".into(),
            by: "winner".into(),
        };
        let structured: StructuredError = (&err).into();
        assert_eq!(structured.code, ErrorCode::AlreadyClaimed);
        assert!(structured.retryable);
        assert_eq!(structured.context["holder"], "winner");
        assert_eq!(structured.context["id"], "ub-42");
    }

    /// D45 — `BlockerNotFound` rides `IssueNotFound` (no `ErrorCode` minted), is NOT retryable, and
    /// its `context` keys stay honest: `blocker_id` for the phantom target (NEVER `id`) plus
    /// `issue_id` for the record that declared the edge. The `Display` text is edge-kind-NEUTRAL.
    ///
    /// MUTANTS KILLED: mapping the variant onto `DependencyNotFound` (whose published meaning is
    /// "the EDGE does not exist") or onto the RETRYABLE `ValidationFailed`; surfacing the target
    /// under the key `id`, which would claim the ADDRESSED issue was the missing one; dropping
    /// `issue_id`, which on a 500-record import leaves the phantom unattributed; and rendering
    /// "blocker" in the message, which lies on the reparent path where the target is a PARENT.
    #[test]
    fn blocker_not_found_rides_issue_not_found_with_honest_context() {
        let err = StorageError::BlockerNotFound {
            issue_id: "ub-dependent".into(),
            depends_on_id: "ub-ghost".into(),
        };
        assert_eq!(err.code(), ErrorCode::IssueNotFound);
        assert!(!err.retryable(), "nothing here is transient");

        let structured: StructuredError = (&err).into();
        assert_eq!(structured.code, ErrorCode::IssueNotFound);
        assert_eq!(structured.context["blocker_id"], "ub-ghost");
        assert_eq!(structured.context["issue_id"], "ub-dependent");
        assert!(
            !structured.context.contains_key("id"),
            "the key must NOT be `id` — that would name the addressed issue, not the phantom"
        );

        assert_eq!(
            err.to_string(),
            "issue ub-dependent declares a dependency target that does not exist: ub-ghost",
            "the user-visible string is NEUTRAL about the edge kind (the guard also covers reparent)"
        );
    }

    /// **D46 — the stale-schema hint is composed HERE, and it is the one that must reach the user.**
    ///
    /// `Migration` serves TWO states and the text BRANCHES on its `gaps` (Miguel's ruling,
    /// 2026-08-03; the dispatch test moved from `from == to` to `!gaps.is_empty()` by D57 clause
    /// (9)): a GENUINE step failure (empty `gaps`) is told to run `unblock migrate`, while the
    /// LYING-STAMP state (non-empty `gaps`, the sentinel's own error) is told the truth — the STAMP
    /// is wrong, `unblock migrate` alone is skipped at that stamp and cannot repair it, so the
    /// recovery is to reset `PRAGMA user_version` FIRST and migrate after. `SchemaMismatch` (the
    /// newer-than-build direction) carries the OPPOSITE instruction and never names the migrate
    /// command; every text DIFFERS; and the hint survives the bridge into `StructuredError`.
    ///
    /// The stale fixture is the D46 seed AFTER step 3 exists: stamped 2, `comments` columns absent,
    /// so it reaches the sentinel with `from = 2`, `to = 3` — and the genuine step failure below
    /// carries the SAME `from`/`to`, so only `gaps` can tell them apart.
    ///
    /// MUTANTS KILLED: deleting the `Migration` arm; **restoring the `from == to` dispatch** (the
    /// stale fixture has `from != to`, so it would get the stale-direction text and the lying-stamp
    /// assertions fire); swapping the branch condition (the genuine-failure assertions fire on a
    /// text telling a mid-ladder failure to hand-edit its stamp); collapsing the two DIRECTIONS onto
    /// one constant text (the `assert_ne` fires); dropping `reason` from the rendering (the
    /// missing-column assertion fires); advising "export and re-import", which a stale `comments`
    /// table itself breaks.
    #[test]
    #[allow(clippy::too_many_lines)] // one variant's two states and the opposite direction, side by side.
    fn schema_hints_are_composed_and_carry_opposite_remedies() {
        let gaps = vec![
            SchemaGap::CommentsColumn("updated_at"),
            SchemaGap::CommentsColumn("redacted_at"),
        ];
        let stale = StorageError::Migration {
            from: 2,
            to: 3,
            reason: super::describe_gaps(&gaps),
            gaps,
        };
        let stale_hint = stale
            .hint()
            .expect("the stale-schema failure carries a hint");
        assert!(
            stale_hint.contains("PRAGMA user_version = 1"),
            "the lying-stamp recovery for a `comments` gap is a stamp RESET to the baseline, and it \
             must be executable as written: {stale_hint}"
        );
        assert!(
            stale_hint.contains("unblock migrate"),
            "…followed by the migrate that then has work to do: {stale_hint}"
        );
        assert!(
            stale_hint.contains("STAMP is what is wrong"),
            "the state must be named truthfully — the stamp lies, the ladder did not fail: \
             {stale_hint}"
        );
        assert!(
            stale_hint.contains("INSPECTS"),
            "…and why the reset is safe: the step senses the table and adds only what is missing: \
             {stale_hint}"
        );
        assert!(
            !stale_hint.contains("but this build expects"),
            "the stale-direction text advises a command that cannot repair this state: \
             {stale_hint}"
        );
        assert!(
            stale_hint.contains("updated_at")
                && stale_hint.contains("redacted_at")
                && stale_hint.contains("schema version 2 adds"),
            "the hint must name the columns actually missing and the step that adds them: \
             {stale_hint}"
        );
        assert!(
            !stale_hint.contains("DROP TABLE"),
            "a `comments` gap alone never drops the config table: {stale_hint}"
        );

        // The OTHER state of the SAME variant, with the SAME versions: a genuine mid-ladder step
        // failure (empty `gaps`), where `unblock migrate` IS the remedy.
        let step_failure = StorageError::Migration {
            from: 2,
            to: 3,
            reason: "the forward steps ended stamped at user_version 2, not 3".to_string(),
            gaps: Vec::new(),
        };
        let step_hint = step_failure
            .hint()
            .expect("a step failure carries a hint too");
        assert!(
            step_hint.contains("Run `unblock migrate`") && !step_hint.contains("PRAGMA"),
            "a REAL step failure keeps its existing advice — the stamp is honest there, so hand-\
             editing it would be wrong: {step_hint}"
        );
        assert_ne!(
            stale_hint, step_hint,
            "one variant, two states — the text branches on `gaps`, not on `from == to`"
        );

        let newer = StorageError::SchemaMismatch {
            found: 3,
            expected: 2,
        };
        let newer_hint = newer
            .hint()
            .expect("the newer-than-build failure carries a hint");
        assert!(
            !newer_hint.contains("unblock migrate"),
            "migrating cannot help a database written by a NEWER build: {newer_hint}"
        );
        assert!(
            newer_hint.contains("unblock update"),
            "the opposite direction's remedy is to upgrade the binary: {newer_hint}"
        );
        assert_ne!(
            stale_hint, newer_hint,
            "one code, two opposite remedies — which is why the published shape is ContextualText"
        );

        for hint in [&stale_hint, &step_hint, &newer_hint] {
            let lowered = hint.to_lowercase();
            assert!(
                !lowered.contains("re-import") && !lowered.contains("reimport"),
                "the hint may not advise a recovery the product cannot perform in this state \
                 (`sync export` is itself broken by a stale `comments` table): {hint}"
            );
        }

        // The hint survives the bridge the L7 boundary actually builds.
        let structured: StructuredError = (&stale).into();
        assert_eq!(structured.code, ErrorCode::SchemaMismatch);
        assert_eq!(structured.hint.as_deref(), Some(stale_hint.as_str()));

        // Every other variant keeps the trait default — no other code's published shape moves.
        for quiet in [
            StorageError::IssueNotFound { id: "ub-1".into() },
            StorageError::DatabaseLocked,
            StorageError::NotInitialized,
            StorageError::Backend {
                source: BackendOpaque::from_message("boom"),
            },
            StorageError::IntegrityFailed {
                messages: vec!["corrupt page".into()],
            },
        ] {
            assert_eq!(quiet.hint(), None, "{quiet:?} must carry no hint");
        }
    }

    /// **D57 clause (9) — the lying-stamp hint BRANCHES ON THE GAPS, and every branch terminates.**
    /// `comments` columns → reset to `1`; `config` table ABSENT → reset to `2`; `config` table
    /// PRESENT missing a column → `DROP TABLE config` (rows LOST, disclosed), then reset to `2`;
    /// several → combined in that order with the LOWEST stamp. Each gap's reason names the step that
    /// adds it, and `context()` surfaces the gaps by label.
    ///
    /// MUTANTS KILLED: one reset stamp for every gap (the config-only branches would reset to `1`,
    /// or the comments branch to `2`); a missing-column branch that skips the `DROP` (step 3's `IF
    /// NOT EXISTS` would then keep the broken table and loop); taking the HIGHEST stamp when gaps
    /// combine (the `comments` step would never re-run).
    #[test]
    fn lying_stamp_hint_branches_per_gap() {
        fn hint_for(gaps: Vec<SchemaGap>) -> (String, StructuredError) {
            let err = StorageError::Migration {
                from: 3,
                to: 3,
                reason: super::describe_gaps(&gaps),
                gaps,
            };
            (err.hint().expect("hint"), (&err).into())
        }

        // …and equal versions with EMPTY gaps still get the stale-direction text: the dispatch is
        // the gaps, never the versions.
        let equal_versions = StorageError::Migration {
            from: 3,
            to: 3,
            reason: "r".to_string(),
            gaps: Vec::new(),
        };
        assert!(
            equal_versions
                .hint()
                .expect("hint")
                .contains("Run `unblock migrate`"),
            "empty gaps → the stale-direction text, whatever from/to say"
        );
        let (absent, structured) = hint_for(vec![SchemaGap::ConfigTable]);
        assert!(absent.contains("PRAGMA user_version = 2"), "{absent}");
        assert!(!absent.contains("DROP TABLE"), "{absent}");
        assert!(
            absent.contains("`config` table that schema version 3 creates is absent"),
            "{absent}"
        );
        assert_eq!(structured.context["gaps"], serde_json::json!(["config"]));

        let (column, structured) = hint_for(vec![SchemaGap::ConfigColumn("value")]);
        assert!(column.contains("DROP TABLE config"), "{column}");
        assert!(
            column.contains("LOST"),
            "the row loss is disclosed: {column}"
        );
        assert!(column.contains("PRAGMA user_version = 2"), "{column}");
        let drop_at = column.find("DROP TABLE config").expect("drop");
        let reset_at = column.find("PRAGMA user_version").expect("reset");
        assert!(drop_at < reset_at, "drop first, then reset: {column}");
        assert_eq!(
            structured.context["gaps"],
            serde_json::json!(["config.value"])
        );

        let (combined, structured) = hint_for(vec![
            SchemaGap::CommentsColumn("redacted_at"),
            SchemaGap::ConfigColumn("key"),
        ]);
        assert!(
            combined.contains("PRAGMA user_version = 1"),
            "the LOWEST stamp wins: {combined}"
        );
        assert!(!combined.contains("PRAGMA user_version = 2"), "{combined}");
        assert!(combined.contains("DROP TABLE config"), "{combined}");
        assert!(
            combined.contains("schema version 2 adds") && combined.contains("schema version 3"),
            "each gap names the step that adds it: {combined}"
        );
        assert_eq!(
            structured.context["gaps"],
            serde_json::json!(["comments.redacted_at", "config.key"])
        );
    }

    /// `CycleDetected{path}` surfaces as `context["cycle_path"]`.
    #[test]
    fn cycle_path_surfaces_in_context() {
        let err = StorageError::CycleDetected {
            path: "a -> b -> a".into(),
        };
        let structured: StructuredError = (&err).into();
        assert_eq!(structured.context["cycle_path"], "a -> b -> a");
    }

    /// `BackendOpaque` Display is sanitized at construction: ESC/BEL and SQL-looking text yield no
    /// raw control byte, and the inner `String` is unreachable through the public API.
    #[test]
    fn backend_opaque_display_is_sanitized() {
        let opaque = BackendOpaque::from_message(
            "near \"SELECT\": syntax error\x1b[2K\x07; DROP TABLE issues",
        );
        let shown = opaque.to_string();
        assert!(
            !shown
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\t')),
            "raw control byte leaked: {shown:?}"
        );
        assert!(shown.contains("\\u{1b}[2K"));
        assert!(shown.contains("\\u{7}"));
        // The SQL-looking text is preserved (it is not a control byte) but harmlessly escaped of
        // control sequences — the point is no terminal-control byte survives.
        assert!(shown.contains("DROP TABLE issues"));
    }

    /// `From<libsql::Error>` absorbs a backend error opaquely and sanitizes its message at
    /// construction (no raw control byte from the backend message survives).
    #[test]
    fn from_libsql_error_sanitizes() {
        let backend: BackendOpaque =
            libsql::Error::SqliteFailure(1, "boom\x1b[2K\x07near \"x\"".to_string()).into();
        let shown = backend.to_string();
        assert!(
            !shown
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\t')),
            "raw control byte leaked through From<libsql::Error>: {shown:?}"
        );
        assert!(shown.contains("boom"));
    }

    /// `map_libsql_err` routes `SQLITE_BUSY` (5) and `SQLITE_LOCKED` (6) — including their extended
    /// forms — to the retryable `DatabaseLocked`; every other failure is absorbed into `Backend`.
    #[test]
    fn map_libsql_err_busy_locked_to_database_locked() {
        // Primary codes.
        for code in [super::SQLITE_BUSY, super::SQLITE_LOCKED] {
            let mapped = super::map_libsql_err(libsql::Error::SqliteFailure(code, "x".into()));
            assert!(
                matches!(mapped, StorageError::DatabaseLocked),
                "code {code}"
            );
            assert!(mapped.retryable());
        }
        // Extended codes share the low byte (e.g. SQLITE_BUSY_SNAPSHOT = 5 | (11<<8) = 0x0B05).
        let extended_busy = super::SQLITE_BUSY | (11 << 8);
        assert!(matches!(
            super::map_libsql_err(libsql::Error::SqliteFailure(extended_busy, "x".into())),
            StorageError::DatabaseLocked
        ));
        // A non-busy/locked sqlite failure is absorbed opaquely.
        let other = super::map_libsql_err(libsql::Error::SqliteFailure(1, "syntax".into()));
        assert!(matches!(other, StorageError::Backend { .. }));
        assert!(!other.retryable());
        // A non-SqliteFailure variant is also absorbed opaquely.
        let connect = super::map_libsql_err(libsql::Error::ConnectionFailed("nope".into()));
        assert!(matches!(connect, StorageError::Backend { .. }));
    }
}
