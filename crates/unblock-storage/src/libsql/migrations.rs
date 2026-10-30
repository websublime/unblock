//! `PRAGMA user_version`-based forward migrations (crate plan §3.3).
//!
//! **No in-place edits** — forward steps only. **D46 (v1.0.1) VINDICATES that rule rather than
//! retiring it: D46 is what finally makes it executable**, after `schema.rs` shipped exactly the
//! in-place edit it forbids (D37 added `comments.updated_at`/`redacted_at` to the baseline
//! `CREATE TABLE`, leaving every database written before 2026-07-17 stamped `1` — indistinguishable
//! by `user_version` from a GA one — with a five-column `comments` and a failing hydrated read).
//!
//! **THE FROZEN-BASELINE DISCIPLINE governs this file (D46 clause (1)).** `SCHEMA_SQL` is frozen at
//! [`BASELINE_SCHEMA_VERSION`]; every post-baseline element is a STEP; a fresh database is created at
//! the baseline, stamped there, and FALLS THROUGH the ladder like any database found on disk — so
//! there is exactly ONE path to the current shape and every fresh install exercises every step. That
//! is the point: this defect survived because the fresh path worked while the ladder path was never
//! exercised once. A FRESH INITIALISATION THEREFORE APPLIES THE STEPS AND REPORTS A MIGRATION
//! (`user_version = 0` ends at [`CURRENT_SCHEMA_VERSION`] having run steps 2 and 3).
//!
//! The full normative contract — the invariant, the one-time exception, atomicity, when a step may
//! run implicitly, the lying-stamp sentinel and its hint, and both ends of the version range — is on
//! [`crate::Storage::migrate`]'s doc comment (spine §3.2).

use libsql::{Connection, TransactionBehavior};

use crate::error::{SchemaGap, StorageError, describe_gaps, map_libsql_err};

use super::schema::{BASELINE_SCHEMA_VERSION, CURRENT_SCHEMA_VERSION, SCHEMA_SQL};

/// The `comments` columns step 2 reconciles, in the order it appends them.
///
/// Appending after `created_at` in this order reproduces the ordinal sequence the pre-D46 inline
/// `CREATE TABLE` produced, so the `PRAGMA table_info(comments)` golden and the POSITIONAL ordinals
/// `5`/`6` that `mappers::comment_from_row` reads are unchanged for fresh and migrated databases
/// alike (they now reach that shape by the SAME `ALTER`).
pub(crate) const COMMENTS_STEP_COLUMNS: &[(&str, &str)] =
    &[("updated_at", "DATETIME"), ("redacted_at", "DATETIME")];

/// The schema version whose step adds [`COMMENTS_STEP_COLUMNS`] (step 2, D46) — named so the
/// sentinel's per-gap reason names the STEP that adds each missing piece, never
/// [`CURRENT_SCHEMA_VERSION`] for all of them (D57 clause (9)).
pub(crate) const COMMENTS_STEP_VERSION: i32 = 2;

/// The schema version whose step creates the `config` table (step 3, D57 clause (9)).
pub(crate) const CONFIG_STEP_VERSION: i32 = 3;

/// The `config` table's columns step 3 creates — the sentinel's step-3 half (D57 clause (9)).
pub(crate) const CONFIG_STEP_COLUMNS: &[(&str, &str)] = &[("key", "TEXT"), ("value", "TEXT")];

/// What a [`Migration`] step DOES — the step-KIND discriminant (D46).
///
/// A step carries a KIND rather than a static SQL field because the one-time historical step must
/// SENSE the database before it acts, which static SQL cannot express. A function-pointer body was
/// REJECTED: it makes the ladder unhashable and kills the D46 clause (6) content pin.
#[derive(Clone, Copy)]
pub(crate) enum MigrationKind {
    /// The ordinary kind: apply `sql` UNCONDITIONALLY. **Every step from 3 onward is this kind** —
    /// the version it advances FROM denotes exactly one physical shape (spine §3.2 clause (i)), so a
    /// step must not inspect the database to decide what to do. Its SQL TEXT is what the content
    /// pin hashes. First constructed by step 3 (D57 clause (9)).
    Sql(&'static str),

    /// **THE ONE-TIME SHAPE-SENSING EXCEPTION — step 2, and NO other step, ever (D46 clause (2)).**
    ///
    /// Stamp `1` covers TWO physical `comments` shapes (five columns before 2026-07-17; seven from
    /// `v1.0.0-rc.4` on), so this step reads `PRAGMA table_info(comments)` ONCE and adds only the
    /// columns actually ABSENT. **The sensing is load-bearing on the LARGEST half of the population,
    /// not a historical courtesy:** an existing GA database carries all seven columns and is stamped
    /// `1`, so under the frozen-baseline discipline it falls through the ladder and reaches this step
    /// with both columns ALREADY PRESENT — an unconditional `ALTER` there is the measured
    /// `duplicate column name` hard error, on every database created since 2026-07-17.
    ///
    /// It is UNPARAMETERISED and single-purpose precisely so that no later step can physically reuse
    /// it: **copying its shape into a step 3 or later is a contract violation, not a style choice.**
    CommentsColumnsReconcile,
}

impl MigrationKind {
    /// The stable discriminant byte the D46 clause (6) content pin hashes.
    ///
    /// Hand-written rather than derived so it is `const`-evaluable and so re-ordering the variants
    /// cannot silently change the digest's meaning.
    pub(crate) const fn discriminant(self) -> u8 {
        match self {
            Self::Sql(_) => 0,
            Self::CommentsColumnsReconcile => 1,
        }
    }
}

/// One forward migration step: bring a database from `version - 1` to `version`.
pub(crate) struct Migration {
    /// The `user_version` this step stamps on success.
    pub(crate) version: i32,
    /// What the step does (D46 — a KIND, no longer a static-SQL field).
    pub(crate) kind: MigrationKind,
}

/// The ordered forward steps, lowest first.
///
/// **Two steps since D57 (v1.1):** step `2`, the one-time `comments`-columns reconcile (D46), and
/// step `3`, the first ORDINARY step — the FR-13-full `config` table (D57 clause (9)), ADDITIVE and
/// so implicit on open (spine §3.2 clause (iv)). The covered range is `BASELINE_SCHEMA_VERSION + 1
/// ..= CURRENT_SCHEMA_VERSION` — asserted contiguous and non-empty by the `const` block below, so
/// bumping the version without a step and adding a step without bumping are BOTH compile errors.
///
/// Step 3's `IF NOT EXISTS` is the STATEMENT's own idempotence — not a Rust-side probe choosing which
/// DDL runs, so spine §3.2 clause (i) holds — and it is REQUIRED: the lying-stamp recovery resets
/// the stamp and replays the ladder, and a plain `CREATE` would then fail `table config already
/// exists` on a database that already has the table, a recovery that loops.
pub(crate) const MIGRATIONS: &[Migration] = &[
    Migration {
        version: COMMENTS_STEP_VERSION,
        kind: MigrationKind::CommentsColumnsReconcile,
    },
    Migration {
        version: CONFIG_STEP_VERSION,
        kind: MigrationKind::Sql(
            "CREATE TABLE IF NOT EXISTS config (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL)",
        ),
    },
];

// D46 clause (6) — THE LADDER-CONTIGUITY ASSERTION. A `const` block, so it fires under
// `cargo check`/`clippy`/`test` alike and no annotation can silence it.
const _: () = {
    assert!(
        BASELINE_SCHEMA_VERSION < CURRENT_SCHEMA_VERSION,
        "D46: the ladder must cover a NON-EMPTY range (BASELINE + 1 ..= CURRENT)"
    );
    // Walked with an i32 counter rather than an index cast: the versions must be CONTIGUOUS and
    // ASCENDING from BASELINE + 1, and the last one must land exactly on CURRENT.
    let mut expected = BASELINE_SCHEMA_VERSION + 1;
    let mut i = 0;
    while i < MIGRATIONS.len() {
        assert!(
            MIGRATIONS[i].version == expected,
            "D46 clause (6): the MIGRATIONS versions must be contiguous and ascending from \
             BASELINE_SCHEMA_VERSION + 1"
        );
        expected += 1;
        i += 1;
    }
    assert!(
        expected - 1 == CURRENT_SCHEMA_VERSION,
        "D46 clause (6): MIGRATIONS must cover BASELINE_SCHEMA_VERSION + 1 ..= \
         CURRENT_SCHEMA_VERSION with no gap — bumping the version without adding a step (or adding \
         a step without bumping) is a compile error, never a field failure"
    );
};

// THE SENTINEL'S SUBJECT, bound to the ladder rather than asserted in prose (Verify gate,
// 2026-08-03). The `Storage::migrate` contract (spine §3.2 clause (v)) promises the sentinel
// witnesses the NEWEST step's OWN postcondition; this assertion makes that a FACT for every tree
// that compiles, because a tree in which it stops holding does not compile.
//
// [D57] STEP 3 TOOK THE "EXTEND THE SENTINEL" BRANCH (PRD §4 D57 clause (9)). `witness_newest_step`
// witnesses the UNION of step 2's `COMMENTS_STEP_COLUMNS` and step 3's `CONFIG_STEP_COLUMNS`, so
// the newest step it witnesses is the version-3 `Sql` step that creates `config` — and this
// assertion now names THAT step: its version is `CONFIG_STEP_VERSION` and its kind is `Sql`. A step 4
// must again EITHER extend the sentinel to witness its own postcondition OR amend the contract —
// deliberately, here.
//
// The alternative — a per-step witness DESCRIPTOR (a table name + a column list on every step) — was
// REJECTED: no non-column step (an index, a backfill) can populate it, so it degrades silently to a
// no-op sentinel for that version, and it would add a per-step surface the clause (6) content pin
// does NOT hash. A const assertion adds no surface, cannot degrade, and forces the choice at the
// exact moment a new step is written.
const _: () = {
    let newest = &MIGRATIONS[MIGRATIONS.len() - 1];
    assert!(
        newest.version == CONFIG_STEP_VERSION
            && newest.kind.discriminant() == MigrationKind::Sql("").discriminant(),
        "D46 clause (v) / D57 clause (9): `witness_newest_step` witnesses COMMENTS_STEP_COLUMNS and \
         CONFIG_STEP_COLUMNS, whose newest half IS the newest step's own postcondition only while the \
         newest step is the version-3 `config` Sql step. A new newest step must EITHER extend the \
         sentinel to witness its own postcondition OR amend the `Storage::migrate` contract that \
         promises the newest step's columns — deliberately, here, not by discovering in the field \
         that the sentinel witnesses a step two versions old."
    );
};

/// Read the database's `PRAGMA user_version`.
///
/// A fresh, unstamped database reports `0`.
pub(crate) async fn current_user_version(conn: &Connection) -> Result<i32, StorageError> {
    let mut rows = conn
        .query("PRAGMA user_version", ())
        .await
        .map_err(map_libsql_err)?;
    let row = rows
        .next()
        .await
        .map_err(map_libsql_err)?
        .ok_or(StorageError::NotInitialized)?;
    let value = row.get_value(0).map_err(map_libsql_err)?;
    Ok(value
        .as_integer()
        .copied()
        .and_then(|v| i32::try_from(v).ok())
        .unwrap_or(0))
}

/// Bring the database at `conn` to [`CURRENT_SCHEMA_VERSION`] — the ONE path to the current shape.
///
/// - A database at a version **greater** than current is rejected with
///   [`StorageError::SchemaMismatch`] (spine §3.2 clause (vi) — a REACHABLE direction since D46).
/// - A **fresh** database (`user_version == 0`) — whether truly empty or one that already carries
///   tables from a crash between the `SCHEMA_SQL` apply and its stamp — has `SCHEMA_SQL` applied, is
///   stamped [`BASELINE_SCHEMA_VERSION`], and then **falls through the ladder**. It is NEVER stamped
///   at the current version directly: that would assert a shape nobody verified and put the database
///   beyond the reach of the very step that repairs it.
/// - Otherwise the ordered [`MIGRATIONS`] steps above the found version run, each committing its DDL
///   and its stamp TOGETHER in one `BEGIN IMMEDIATE`.
/// - On **every** exit path, including the already-at-current early return, the newest step's own
///   columns are witnessed: a stamp that LIES is [`StorageError::Migration`] naming what is missing,
///   never a silent read failure downstream (spine §3.2 clause (v)).
pub(crate) async fn run_migrations(conn: &Connection) -> Result<(), StorageError> {
    let found = current_user_version(conn).await?;

    if found > CURRENT_SCHEMA_VERSION {
        return Err(StorageError::SchemaMismatch {
            found,
            expected: CURRENT_SCHEMA_VERSION,
        });
    }

    if found < CURRENT_SCHEMA_VERSION {
        let mut at = found;
        if at == 0 {
            bootstrap_baseline(conn).await?;
            at = BASELINE_SCHEMA_VERSION;
        }
        for step in MIGRATIONS.iter().filter(|m| m.version > at) {
            apply_step(conn, step).await?;
        }
        // A step that silently failed to stamp would leave the database at a version that lies in
        // the OTHER direction, so the ladder's own outcome is asserted before the sentinel runs.
        let stamped = current_user_version(conn).await?;
        if stamped != CURRENT_SCHEMA_VERSION {
            return Err(StorageError::Migration {
                from: found,
                to: CURRENT_SCHEMA_VERSION,
                reason: format!(
                    "the forward steps ended stamped at user_version {stamped}, not \
                     {CURRENT_SCHEMA_VERSION}"
                ),
                gaps: Vec::new(),
            });
        }
    }

    witness_newest_step(conn, found).await
}

/// **The lying-stamp SENTINEL (spine §3.2 clause (v)).** Witness the NEWEST step's own
/// postcondition — and, since D57, step 2's as well.
///
/// A bounded per-step POSTCONDITION: its result never decides which DDL to run (only step 2 ever
/// decides anything from a probe), and it is NOT a conformance comparison of the live schema against
/// `SCHEMA_SQL` — that is deliberately out of scope (PRD §4, D46 clause (4)). It runs on EVERY exit
/// path because a STALE database IS at the current stamp and the ladder does NOT run on it: "only
/// where the ladder ran" is precisely the complement of the defect.
///
/// `found` is the stamp observed on entry, so the returned error names what the caller actually met.
///
/// **The subject is the UNION of [`COMMENTS_STEP_COLUMNS`] (on `comments`) and
/// [`CONFIG_STEP_COLUMNS`] (on `config`; an absent table reads as zero columns and is ONE
/// [`SchemaGap::ConfigTable`] gap) — D57 clause (9)'s literal "EXTEND".** The `const` block beside
/// [`MIGRATIONS`] refuses to build a ladder whose newest step is not the version-3 `config` step,
/// so this function cannot silently fall behind the ladder, and keeping the `comments` half means a
/// stamp that lies about the `comments` columns is still refused after step 3 lands.
///
/// EVERY SENTINEL FAILURE IS A LYING STAMP, whatever `found` is: the error carries NON-EMPTY `gaps`,
/// which is what the hint dispatches on (a stamp-2 database lying about `comments` first runs step 3
/// and arrives here with `from = 2`, `to = 3`).
async fn witness_newest_step(conn: &Connection, found: i32) -> Result<(), StorageError> {
    let mut gaps = Vec::new();

    let comments = table_columns(conn, "comments").await?;
    gaps.extend(
        COMMENTS_STEP_COLUMNS
            .iter()
            .map(|(name, _)| *name)
            .filter(|name| !comments.iter().any(|have| have.as_str() == *name))
            .map(SchemaGap::CommentsColumn),
    );

    let config = table_columns(conn, "config").await?;
    if config.is_empty() {
        gaps.push(SchemaGap::ConfigTable);
    } else {
        gaps.extend(
            CONFIG_STEP_COLUMNS
                .iter()
                .map(|(name, _)| *name)
                .filter(|name| !config.iter().any(|have| have.as_str() == *name))
                .map(SchemaGap::ConfigColumn),
        );
    }

    if gaps.is_empty() {
        return Ok(());
    }

    Err(StorageError::Migration {
        from: found,
        to: CURRENT_SCHEMA_VERSION,
        reason: describe_gaps(&gaps),
        gaps,
    })
}

/// Bootstrap a fresh (`user_version == 0`) database at the BASELINE: apply the canonical schema,
/// stamp [`BASELINE_SCHEMA_VERSION`], and truncate-checkpoint the WAL so the freshly written pages do
/// not linger in the log.
///
/// The caller then runs the ladder over it — `SCHEMA_SQL` is `CREATE … IF NOT EXISTS` throughout, so
/// this is also the safe repair for a stamped-`0` database that already carries tables.
async fn bootstrap_baseline(conn: &Connection) -> Result<(), StorageError> {
    conn.execute_batch(SCHEMA_SQL)
        .await
        .map_err(|e| StorageError::Migration {
            from: 0,
            to: BASELINE_SCHEMA_VERSION,
            reason: e.to_string(),
            gaps: Vec::new(),
        })?;
    stamp_version(conn, BASELINE_SCHEMA_VERSION).await?;
    // Manual truncate checkpoint (wal_autocheckpoint is disabled — see `apply_pragmas`). A query is
    // used because `wal_checkpoint` returns a result row; a non-WAL DB simply yields no rows.
    let _ = conn.query("PRAGMA wal_checkpoint(TRUNCATE)", ()).await;
    Ok(())
}

/// Apply one forward [`Migration`] step and stamp its version — **in ONE `BEGIN IMMEDIATE`**.
///
/// D46 clause (iii): the DDL and the `PRAGMA user_version` write commit TOGETHER. A step that
/// applied DDL and stamped separately could crash between them and manufacture a third physical
/// shape, which is exactly the ambiguity the stamped-version-implies-a-known-shape invariant exists
/// to forbid. (The pre-D46 applier's doc comment already claimed a transaction; its body opened none.)
async fn apply_step(conn: &Connection, step: &Migration) -> Result<(), StorageError> {
    let to_error = |reason: String| StorageError::Migration {
        from: step.version - 1,
        to: step.version,
        reason,
        gaps: Vec::new(),
    };

    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await
        .map_err(map_libsql_err)?;

    match step.kind {
        MigrationKind::Sql(sql) => {
            tx.execute_batch(sql)
                .await
                .map_err(|e| to_error(e.to_string()))?;
        }
        MigrationKind::CommentsColumnsReconcile => {
            reconcile_comments_columns(&tx).await.map_err(|e| match e {
                // Keep a typed cause typed; re-label only the opaque backend text with this step's
                // from→to so the hint can name the version pair the caller actually met.
                StorageError::Backend { source } => to_error(source.to_string()),
                other => other,
            })?;
        }
    }

    // The stamp rides the SAME transaction as the DDL above.
    stamp_version(&tx, step.version).await?;
    tx.commit().await.map_err(map_libsql_err)
}

/// The one-time `comments`-columns reconcile (step 2's body).
///
/// Reads `PRAGMA table_info(comments)` ONCE and `ALTER TABLE ADD COLUMN`s only the columns actually
/// absent, in [`COMMENTS_STEP_COLUMNS`] order. Both are **nullable with NO `DEFAULT`** — a default
/// would make a migrated database disagree with a fresh one on inserts that omit the column.
async fn reconcile_comments_columns(conn: &Connection) -> Result<(), StorageError> {
    let present = table_columns(conn, "comments").await?;
    for &(name, sql_type) in COMMENTS_STEP_COLUMNS {
        if present.iter().any(|have| have.as_str() == name) {
            continue;
        }
        // The column name and type are this crate's own constants, never user input.
        let _ = conn
            .query(
                &format!("ALTER TABLE comments ADD COLUMN {name} {sql_type}"),
                (),
            )
            .await
            .map_err(map_libsql_err)?;
    }
    Ok(())
}

/// The current column names of `table`, in ordinal order (`PRAGMA table_info`). An ABSENT table
/// reads as zero columns. `table` is always one of this crate's own constant names (`comments`,
/// `config`), never user input — `PRAGMA` does not accept a bound table name.
async fn table_columns(conn: &Connection, table: &str) -> Result<Vec<String>, StorageError> {
    let mut rows = conn
        .query(&format!("PRAGMA table_info({table})"), ())
        .await
        .map_err(map_libsql_err)?;
    let mut columns = Vec::new();
    while let Some(row) = rows.next().await.map_err(map_libsql_err)? {
        // table_info columns: cid, name, type, notnull, dflt_value, pk.
        if let libsql::Value::Text(name) = row.get_value(1).map_err(map_libsql_err)? {
            columns.push(name);
        }
    }
    Ok(columns)
}

/// Stamp `PRAGMA user_version`. `PRAGMA` does not accept bound parameters, so the version (a
/// validated `i32` from this crate's own constants, never user input) is formatted inline.
async fn stamp_version(conn: &Connection, version: i32) -> Result<(), StorageError> {
    // `query` (not `execute`): the PRAGMA setter form may surface a row in libsql, which `execute`
    // rejects with `ExecuteReturnedRows`.
    let _ = conn
        .query(&format!("PRAGMA user_version = {version}"), ())
        .await
        .map_err(map_libsql_err)?;
    Ok(())
}
