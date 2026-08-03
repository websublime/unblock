//! **Forward-migration & drift integration (D46, v1.0.1) — the cell that DISCHARGES `NFR-19`.**
//!
//! NFR-19 (PRD §6): *a released binary MUST open a database written by any earlier released binary,
//! by migrating it forward.* Nothing in the tree stated that before D46, which is the root cause of
//! the defect CLASS rather than of its instance — and this file is what proves it. **A green suite
//! that never opens a database it did not itself create does NOT satisfy NFR-19**, which is exactly
//! why this file — prescribed by the `unblock-storage` crate plan from the start and never written —
//! is part of the D46 fix and not follow-up.
//!
//! It seeds by RAW libsql, applying [`HISTORICAL_BASELINE_SQL`] below: no checked-in `.db` fixture
//! (the root `.gitignore` blanket-ignores `*.db`) and no `testkit` feature (the required workspace
//! `test` job does not enable one, so a gated cell here would be green by NON-EXECUTION).
//!
//! Every "this case pins X" claim below is proven by APPLYING the mutation and observing red, never
//! by reading the test — see each cell's MUTANT KILLED notes.

use libsql::{Connection, Database};
use unblock_error::CodedError as _;
use unblock_storage::{LibsqlStorage, Storage, StorageError};

/// **FROZEN HISTORY — the baseline DDL as it stood when the migration ladder began. THE PAST CANNOT
/// DRIFT, so this constant must NEVER be "updated" to track `SCHEMA_SQL`.**
///
/// Updating it would silently delete the only fixture that can produce the stale five-column
/// `comments` shape, and this file would quietly stop testing the thing it exists for.
///
/// **D46's frozen-baseline discipline makes it byte-equivalent to the production baseline TODAY, and
/// it must STILL stay a separate constant:** pointing the fixture at `SCHEMA_SQL` (even if that were
/// reachable — it is `pub(crate)`) would couple the PAST to a constant a future change may still
/// legitimately touch, and would quietly turn "seed an earlier state" back into "create a fresh one"
/// — the very thing this file exists to stop being the only thing tested.
///
/// It is reproduced IN FULL — every table and every index — rather than trimmed to what the step
/// touches: the stamped-`0` case re-applies the production `SCHEMA_SQL` over this database, and an
/// abbreviated `issues` table would fail there on an index referencing a column the trim dropped.
/// Reproducing it whole also lets the parity cell compare the FULL index list, not a subset.
const HISTORICAL_BASELINE_SQL: &str = r"
    -- Issues table.
    -- Column order is FROZEN to match the original bd schema (model-B trims applied): the
    -- PRAGMA table_info(issues) ordinal sequence is golden-pinned (insta) so a fresh and a
    -- migrated DB stay column-compatible.
    -- TEXT body columns use DEFAULT '' (the mapper coalesces '' -> None on load).
    CREATE TABLE IF NOT EXISTS issues (
        id TEXT PRIMARY KEY,
        content_hash TEXT,
        title TEXT NOT NULL CHECK(length(title) <= 500),
        description TEXT NOT NULL DEFAULT '',
        design TEXT NOT NULL DEFAULT '',
        acceptance_criteria TEXT NOT NULL DEFAULT '',
        notes TEXT NOT NULL DEFAULT '',
        status TEXT NOT NULL DEFAULT 'open',
        priority INTEGER NOT NULL DEFAULT 2 CHECK(priority >= 0 AND priority <= 4),
        issue_type TEXT NOT NULL DEFAULT 'task',
        assignee TEXT,
        owner TEXT DEFAULT '',
        estimated_minutes INTEGER,
        created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
        created_by TEXT DEFAULT '',
        updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
        closed_at DATETIME,
        close_reason TEXT DEFAULT '',
        closed_by_session TEXT DEFAULT '',
        due_at DATETIME,
        defer_until DATETIME,
        external_ref TEXT,
        source_system TEXT DEFAULT '',
        source_repo TEXT DEFAULT '',
        deleted_at DATETIME,
        deleted_by TEXT DEFAULT '',
        delete_reason TEXT DEFAULT '',
        original_type TEXT DEFAULT '',
        compaction_level INTEGER DEFAULT 0,
        compacted_at DATETIME,
        compacted_at_commit TEXT,
        original_size INTEGER,
        sender TEXT DEFAULT '',
        ephemeral INTEGER NOT NULL DEFAULT 0,
        pinned INTEGER NOT NULL DEFAULT 0,
        is_template INTEGER NOT NULL DEFAULT 0,
        source_repo_path TEXT,
        agent_context TEXT,
        CHECK (
            (status = 'closed' AND closed_at IS NOT NULL) OR
            (status = 'tombstone') OR
            (status NOT IN ('closed', 'tombstone') AND closed_at IS NULL)
        )
    );

    -- Primary access patterns.
    CREATE INDEX IF NOT EXISTS idx_issues_status ON issues(status);
    CREATE INDEX IF NOT EXISTS idx_issues_priority ON issues(priority);
    CREATE INDEX IF NOT EXISTS idx_issues_issue_type ON issues(issue_type);
    CREATE INDEX IF NOT EXISTS idx_issues_assignee ON issues(assignee) WHERE assignee IS NOT NULL;
    CREATE INDEX IF NOT EXISTS idx_issues_created_at ON issues(created_at);
    CREATE INDEX IF NOT EXISTS idx_issues_updated_at ON issues(updated_at);

    -- Export/sync patterns.
    CREATE INDEX IF NOT EXISTS idx_issues_content_hash ON issues(content_hash);
    CREATE UNIQUE INDEX IF NOT EXISTS idx_issues_external_ref_unique ON issues(external_ref) WHERE external_ref IS NOT NULL;

    -- Special states.
    CREATE INDEX IF NOT EXISTS idx_issues_ephemeral ON issues(ephemeral) WHERE ephemeral = 1;
    CREATE INDEX IF NOT EXISTS idx_issues_pinned ON issues(pinned) WHERE pinned = 1;
    CREATE INDEX IF NOT EXISTS idx_issues_tombstone ON issues(status) WHERE status = 'tombstone';

    -- Time-based.
    CREATE INDEX IF NOT EXISTS idx_issues_due_at ON issues(due_at) WHERE due_at IS NOT NULL;
    CREATE INDEX IF NOT EXISTS idx_issues_defer_until ON issues(defer_until) WHERE defer_until IS NOT NULL;

    -- Ready-work composite index (most important for performance; copied verbatim, NFR-1).
    CREATE INDEX IF NOT EXISTS idx_issues_ready
        ON issues(status, priority, created_at)
        WHERE status = 'open'
        AND ephemeral = 0
        AND pinned = 0
        AND is_template = 0;

    -- Common active-list path: non-terminal issues ordered by priority/created_at.
    CREATE INDEX IF NOT EXISTS idx_issues_list_active_order
        ON issues(priority, created_at)
        WHERE status NOT IN ('closed', 'tombstone')
        AND (is_template = 0 OR is_template IS NULL);

    -- Dependencies. issue_id CASCADE; depends_on_id has NO FK (external refs allowed).
    CREATE TABLE IF NOT EXISTS dependencies (
        issue_id TEXT NOT NULL,
        depends_on_id TEXT NOT NULL,
        type TEXT NOT NULL DEFAULT 'blocks',
        created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
        created_by TEXT NOT NULL DEFAULT '',
        metadata TEXT DEFAULT '{}',
        thread_id TEXT DEFAULT '',
        PRIMARY KEY (issue_id, depends_on_id),
        FOREIGN KEY (issue_id) REFERENCES issues(id) ON DELETE CASCADE
    );
    CREATE INDEX IF NOT EXISTS idx_dependencies_issue ON dependencies(issue_id);
    CREATE INDEX IF NOT EXISTS idx_dependencies_depends_on ON dependencies(depends_on_id);
    CREATE INDEX IF NOT EXISTS idx_dependencies_type ON dependencies(type);
    CREATE INDEX IF NOT EXISTS idx_dependencies_depends_on_type ON dependencies(depends_on_id, type);
    CREATE INDEX IF NOT EXISTS idx_dependencies_blocking
        ON dependencies(depends_on_id, issue_id)
        WHERE (type = 'blocks' OR type = 'parent-child' OR type = 'conditional-blocks' OR type = 'waits-for');

    -- Labels.
    CREATE TABLE IF NOT EXISTS labels (
        issue_id TEXT NOT NULL,
        label TEXT NOT NULL,
        PRIMARY KEY (issue_id, label),
        FOREIGN KEY (issue_id) REFERENCES issues(id) ON DELETE CASCADE
    );
    CREATE INDEX IF NOT EXISTS idx_labels_label ON labels(label);
    CREATE INDEX IF NOT EXISTS idx_labels_issue ON labels(issue_id);

    -- The five-column `comments` table every build before 2026-07-17 wrote. THIS is the shape D37
    -- edited IN PLACE (adding `updated_at`/`redacted_at`) without shipping a step, which is what left
    -- field databases stamped `1` and unable to serve a single hydrated comment read.
    CREATE TABLE IF NOT EXISTS comments (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        issue_id TEXT NOT NULL,
        author TEXT NOT NULL,
        text TEXT NOT NULL,
        created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
        FOREIGN KEY (issue_id) REFERENCES issues(id) ON DELETE CASCADE
    );
    CREATE INDEX IF NOT EXISTS idx_comments_issue ON comments(issue_id);
    CREATE INDEX IF NOT EXISTS idx_comments_created_at ON comments(created_at);

    -- Events (append-only audit + Tier-1 attribution, capture-only).
    CREATE TABLE IF NOT EXISTS events (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        issue_id TEXT NOT NULL,
        event_type TEXT NOT NULL,
        actor TEXT NOT NULL DEFAULT '',
        old_value TEXT,
        new_value TEXT,
        comment TEXT,
        created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
        agent_name TEXT,
        harness TEXT,
        model TEXT,
        FOREIGN KEY (issue_id) REFERENCES issues(id) ON DELETE CASCADE
    );
    CREATE INDEX IF NOT EXISTS idx_events_issue ON events(issue_id);
    CREATE INDEX IF NOT EXISTS idx_events_type ON events(event_type);
    CREATE INDEX IF NOT EXISTS idx_events_created_at ON events(created_at);
    CREATE INDEX IF NOT EXISTS idx_events_actor ON events(actor) WHERE actor != '';

    -- Metadata (key/value; application enforces key replacement).
    CREATE TABLE IF NOT EXISTS metadata (
        key TEXT NOT NULL,
        value TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_metadata_key ON metadata(key);

    -- Child counters (hierarchical ids like ub-abc.1, ub-abc.2).
    CREATE TABLE IF NOT EXISTS child_counters (
        parent_id TEXT PRIMARY KEY,
        last_child INTEGER NOT NULL DEFAULT 0,
        FOREIGN KEY (parent_id) REFERENCES issues(id) ON DELETE CASCADE
    );
";

/// The two columns step 2 reconciles, as the production step appends them.
const STEP_TWO_COLUMNS: [&str; 2] = ["updated_at", "redacted_at"];

/// A raw libsql handle on `path` (the same bundled `SQLite` the backend uses).
async fn raw(path: &std::path::Path) -> (Database, Connection) {
    let db = libsql::Builder::new_local(path)
        .build()
        .await
        .expect("open the db");
    let conn = db.connect().expect("connect");
    (db, conn)
}

/// Seed a database at the HISTORICAL baseline and stamp it `stamp`.
///
/// `stamp` is a parameter because two DISTINCT field states share this shape: a database stamped `1`
/// (an ordinary pre-2026-07-17 install) and one stamped `0` (a crash between the DDL apply and the
/// stamp). Both must reach the current shape, and neither may be stamped at the current version
/// without running the ladder.
async fn seed_historical(path: &std::path::Path, stamp: i32) {
    let (_db, conn) = raw(path).await;
    conn.execute_batch(HISTORICAL_BASELINE_SQL)
        .await
        .expect("apply the historical baseline");
    conn.query(&format!("PRAGMA user_version = {stamp}"), ())
        .await
        .expect("stamp");
}

/// Insert one comment row through the 4-column historical INSERT (what an old build wrote).
async fn seed_historical_comment(path: &std::path::Path, issue_id: &str, text: &str) {
    let (_db, conn) = raw(path).await;
    conn.execute(
        "INSERT INTO issues (id, title, created_at, updated_at) \
         VALUES (?1, 'seeded', '2026-07-01T00:00:00Z', '2026-07-01T00:00:00Z')",
        libsql::params![issue_id],
    )
    .await
    .expect("seed the issue");
    conn.execute(
        "INSERT INTO comments (issue_id, author, text, created_at) \
         VALUES (?1, 'historic', ?2, '2026-07-01T00:00:00Z')",
        libsql::params![issue_id, text],
    )
    .await
    .expect("seed the comment");
}

/// `PRAGMA table_info(<table>)` column names, in ordinal order.
async fn columns(conn: &Connection, table: &str) -> Vec<String> {
    let mut rows = conn
        .query(&format!("PRAGMA table_info({table})"), ())
        .await
        .expect("table_info");
    let mut names = Vec::new();
    while let Some(row) = rows.next().await.expect("row") {
        if let libsql::Value::Text(name) = row.get_value(1).expect("name") {
            names.push(name);
        }
    }
    names
}

/// The `idx_%` index names, sorted.
async fn indexes(conn: &Connection) -> Vec<String> {
    let mut rows = conn
        .query(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name LIKE 'idx_%' \
             ORDER BY name ASC",
            (),
        )
        .await
        .expect("index list");
    let mut names = Vec::new();
    while let Some(row) = rows.next().await.expect("row") {
        if let libsql::Value::Text(name) = row.get_value(0).expect("name") {
            names.push(name);
        }
    }
    names
}

/// Open the workspace database through the PRODUCTION path and migrate it.
async fn open_and_migrate(path: &std::path::Path) -> LibsqlStorage {
    let storage = LibsqlStorage::open_local(path, unblock_storage::DEFAULT_WRITE_LOCK_TIMEOUT_MS)
        .await
        .expect("open_local");
    storage.migrate().await.expect("migrate");
    storage
}

/// **NFR-19, case 1 — the STALE shape: five-column `comments`, stamped `1`.**
///
/// The database a build before 2026-07-17 wrote: indistinguishable from a GA one by `user_version`,
/// and unable to serve a single hydrated comment read. It must reach the seven-column shape stamped
/// `2`, with the pre-existing row intact and both new columns NULL.
///
/// MUTANT KILLED: emptying `MIGRATIONS` — the columns never arrive and the shape assertion goes red.
///
/// MUTANT KILLED: a recreate-and-copy step instead of `ALTER TABLE ADD COLUMN` — the pre-existing row
/// assertion catches the data loss.
#[tokio::test]
async fn a_five_column_database_stamped_one_reaches_the_current_shape() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db = tmp.path().join("unblock.db");
    seed_historical(&db, 1).await;
    seed_historical_comment(&db, "ub-old", "written before the columns existed").await;

    let storage = open_and_migrate(&db).await;
    assert_eq!(
        storage.schema_version().await.expect("schema_version"),
        2,
        "the ladder advanced the stale database to the current stamp"
    );

    let (_db, conn) = raw(&db).await;
    let cols = columns(&conn, "comments").await;
    assert_eq!(
        cols,
        vec![
            "id",
            "issue_id",
            "author",
            "text",
            "created_at",
            "updated_at",
            "redacted_at"
        ],
        "the two step-2 columns are APPENDED, reproducing the shipped ordinal sequence"
    );

    let mut rows = conn
        .query(
            "SELECT text, updated_at, redacted_at FROM comments WHERE issue_id = 'ub-old'",
            (),
        )
        .await
        .expect("read the migrated comment");
    let row = rows.next().await.expect("row").expect("the row survived");
    assert_eq!(
        row.get_value(0).expect("text"),
        libsql::Value::Text("written before the columns existed".to_string()),
        "the pre-existing comment row survived the migration verbatim"
    );
    assert_eq!(
        row.get_value(1).expect("updated_at"),
        libsql::Value::Null,
        "ADD COLUMN leaves the existing row NULL"
    );
    assert_eq!(
        row.get_value(2).expect("redacted_at"),
        libsql::Value::Null,
        "ADD COLUMN leaves the existing row NULL"
    );
}

/// **NFR-19, case 2 — the GA shape: SEVEN columns, still stamped `1`.**
///
/// The LARGEST half of the population. It falls through the ladder and reaches step 2 with both
/// columns ALREADY PRESENT, which is why step 2 senses the shape rather than applying unconditionally.
///
/// MUTANT KILLED: making step 2 a version-keyed UNCONDITIONAL `ALTER` (dropping the
/// `PRAGMA table_info` probe) — this cell hard-errors `duplicate column name: updated_at`, which is
/// the measured behaviour on every database created since 2026-07-17.
#[tokio::test]
async fn a_seven_column_database_stamped_one_succeeds_with_no_ddl() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db = tmp.path().join("unblock.db");
    seed_historical(&db, 1).await;
    {
        let (_db, conn) = raw(&db).await;
        for column in STEP_TWO_COLUMNS {
            conn.query(
                &format!("ALTER TABLE comments ADD COLUMN {column} DATETIME"),
                (),
            )
            .await
            .expect("pre-add the D37 column (the GA shape)");
        }
    }

    let storage = open_and_migrate(&db).await;
    assert_eq!(
        storage.schema_version().await.expect("schema_version"),
        2,
        "an already-shaped database is still ADVANCED to the current stamp"
    );

    let (_db, conn) = raw(&db).await;
    let cols = columns(&conn, "comments").await;
    assert_eq!(cols.len(), 7, "no column was added twice");
}

/// **NFR-19, case 3 — a stamped-`0` database that ALREADY carries tables.**
///
/// The crash-between-apply-and-stamp state. It must take the BASELINE stamp and then fall through
/// the ladder — never the current stamp directly, which would assert a shape nobody verified and put
/// the database beyond the reach of the very step that repairs it.
///
/// MUTANT KILLED: stamping `CURRENT_SCHEMA_VERSION` in the fresh arm instead of
/// `BASELINE_SCHEMA_VERSION` (the naive port) — the ladder is bypassed, the five-column table keeps
/// its shape and the column assertion goes red while the stamp lies about it.
#[tokio::test]
async fn a_stamped_zero_database_with_tables_falls_through_the_ladder() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db = tmp.path().join("unblock.db");
    seed_historical(&db, 0).await;

    let storage = open_and_migrate(&db).await;
    assert_eq!(storage.schema_version().await.expect("schema_version"), 2);

    let (_db, conn) = raw(&db).await;
    assert_eq!(columns(&conn, "comments").await.len(), 7);
}

/// **The FRESH path genuinely RUNS the ladder (D46 clause (1) — the discipline's whole point).**
///
/// Under the frozen-baseline discipline the fresh arm is no longer a separate route to the current
/// shape: a brand-new database is created at the BASELINE, stamped there, and falls through step 2
/// like any database found on disk — so there is exactly ONE path to the current shape and every
/// fresh install exercises every step. This defect survived precisely because the fresh path worked
/// while the ladder path was never exercised once.
///
/// MUTANT KILLED: making step 2 a no-op — a FRESH database is then left at FIVE columns instead of
/// silently passing, which is what proves the fresh path really exercises the ladder rather than a
/// `CREATE TABLE` that already had the columns.
///
/// MUTANT KILLED: re-adding the two columns to `SCHEMA_SQL` (the D37 in-place edit) — the const
/// content pin reddens the BUILD first; were it removed, step 2's `ALTER` would then hard-error
/// `duplicate column name` on this very cell.
#[tokio::test]
async fn a_fresh_database_reaches_the_current_shape_by_running_the_ladder() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db = tmp.path().join("unblock.db");

    let storage = open_and_migrate(&db).await;
    assert_eq!(
        storage.schema_version().await.expect("schema_version"),
        2,
        "0 -> 2 is a REAL migration even on a brand-new file"
    );

    let (_db, conn) = raw(&db).await;
    let cols = columns(&conn, "comments").await;
    assert_eq!(
        cols,
        vec![
            "id",
            "issue_id",
            "author",
            "text",
            "created_at",
            "updated_at",
            "redacted_at"
        ],
        "a fresh database reaches the SEVEN-column shape through step 2, not through the DDL"
    );
}

/// **SCHEMA PARITY — a migrated historical database and a freshly created one agree.**
///
/// Under the frozen baseline both reached the shape through the SAME step, which is what makes this
/// parity STRUCTURAL rather than coincidental.
///
/// MUTANT KILLED: giving step 2 a different column order, type or `DEFAULT` from the DDL a fresh
/// database would have produced — the two `table_info` lists diverge.
#[tokio::test]
async fn a_migrated_database_and_a_fresh_one_agree_on_shape() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let migrated_path = tmp.path().join("migrated.db");
    seed_historical(&migrated_path, 1).await;
    let _migrated = open_and_migrate(&migrated_path).await;

    let fresh_path = tmp.path().join("fresh.db");
    let _fresh = open_and_migrate(&fresh_path).await;

    let (_a, migrated_conn) = raw(&migrated_path).await;
    let (_b, fresh_conn) = raw(&fresh_path).await;

    for table in [
        "issues",
        "dependencies",
        "labels",
        "comments",
        "events",
        "metadata",
        "child_counters",
    ] {
        assert_eq!(
            columns(&migrated_conn, table).await,
            columns(&fresh_conn, table).await,
            "`{table}` must agree between a migrated and a fresh database"
        );
    }

    assert_eq!(
        indexes(&migrated_conn).await,
        indexes(&fresh_conn).await,
        "the index lists agree"
    );
}

/// **Re-migrating is a no-op, and a FUTURE stamp is still refused.**
///
/// MUTANT KILLED: dropping the `found > CURRENT_SCHEMA_VERSION` guard — the future-stamped database
/// would be silently accepted and read with a shape nobody verified.
#[tokio::test]
async fn re_migrating_is_a_noop_and_a_future_stamp_is_refused() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db = tmp.path().join("unblock.db");
    seed_historical(&db, 1).await;

    let storage = open_and_migrate(&db).await;
    storage.migrate().await.expect("re-migrate is a no-op");
    assert_eq!(storage.schema_version().await.expect("schema_version"), 2);
    drop(storage);

    {
        let (_db, conn) = raw(&db).await;
        conn.query("PRAGMA user_version = 99", ())
            .await
            .expect("stamp a future version");
    }
    let storage = LibsqlStorage::open_local(&db, unblock_storage::DEFAULT_WRITE_LOCK_TIMEOUT_MS)
        .await
        .expect("open_local");
    let err = storage
        .migrate()
        .await
        .expect_err("a newer-than-build database is refused");
    assert!(
        matches!(
            err,
            StorageError::SchemaMismatch {
                found: 99,
                expected: 2
            }
        ),
        "got {err:?}"
    );
}

/// **THE SENTINEL — a CURRENT-stamped database missing a newest-step column is an ERROR, never a
/// silent read failure (spine §3.2 clause (v)).**
///
/// This is the lying-stamp state: the ladder does NOT run on it (the stamp is already current), which
/// is why the witness must sit on EVERY exit path including the already-at-current early return.
/// "Only where the ladder ran" is precisely the complement of the defect.
///
/// MUTANT KILLED: deleting the sentinel — `migrate` returns `Ok(())` on a database that cannot serve
/// a hydrated read, which is the exact false green D46 exists to end.
///
/// MUTANT KILLED: running the sentinel only when the ladder ran — this cell's database is at the
/// current stamp, so the ladder is skipped and the error never fires.
#[tokio::test]
async fn a_lying_stamp_is_refused_naming_the_missing_column() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db = tmp.path().join("unblock.db");
    // The stale SHAPE with the CURRENT stamp: nothing in `user_version` can tell them apart.
    seed_historical(&db, 2).await;

    let storage = LibsqlStorage::open_local(&db, unblock_storage::DEFAULT_WRITE_LOCK_TIMEOUT_MS)
        .await
        .expect("open_local");
    let err = storage
        .migrate()
        .await
        .expect_err("a stamp that lies about the shape is an error");

    let StorageError::Migration { reason, .. } = &err else {
        panic!("expected StorageError::Migration, got {err:?}");
    };
    for column in STEP_TWO_COLUMNS {
        assert!(
            reason.contains(column),
            "the error must NAME what is missing; reason: {reason}"
        );
    }

    // And it carries the self-correction hint (D46 clause (7)) — composed from these same fields.
    let hint = err.hint().expect("the failure carries a hint");
    assert!(
        hint.contains("unblock migrate"),
        "the hint names the ONE command that repairs it: {hint}"
    );
}
