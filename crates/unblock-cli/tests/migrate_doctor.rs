//! `unblock migrate` (D27/AF-2) and `unblock doctor` / `doctor --repair` (FR-16 full, PRD §4 D58)
//! end-to-end.
//!
//! - migrate (D46 clause (10)): the command reports the stamp observed BEFORE its own open, so all
//!   THREE cases are pinned here and get the SAME treatment. An `unblock init`-built workspace is
//!   ALREADY-CURRENT (`init` opened through the same facade, which migrated), so it reports
//!   `schema_from == schema_to == 3`, `applied == false` — `applied: false` means "the stamp did not
//!   move across THIS run's own open", NOT "nothing was wrong". A NEVER-MIGRATED workspace
//!   (`.unblock/config.toml`, no `unblock.db`) reports `0` -> `3`, `applied: true`. A STALE workspace
//!   (five-column `comments`, stamped `1`) reports `1` -> `3`, `applied: true`. A stamp-2 workspace
//!   (every GA, v1.0.1 and v1.0.2 database — no `config` table) reports `2` -> `3`, `applied: true`
//!   (D57 clause (9)). Report shape pinned.
//! - doctor (D58 clause (8)): plain `doctor` classifies read-only and exits by the level, 0 for
//!   `Healthy`/`Drifted` and 2 for `Recoverable`/`Unsafe`. It changes no database content, takes no
//!   `.write.lock` and creates no `.recovery/` entry (ruling 390: `SQLite`'s read-only open may create
//!   an empty `-wal` and a `-shm`). `doctor --repair` captures the anomalous sidecars, stops at the
//!   Unsafe gate, runs the pre-open window, then `recover()`, then re-classifies, and renders one
//!   document. The cells below carry the cli half of criteria
//!   (b), (c), (d), (f), (g), (h), (i) and (j).
//! - csv (`ub-q3k`): `migrate` and a healthy `doctor` render one `label,detail` document and exit 0.
//!   A readable database whose `integrity_check` reports a problem renders the report and exits 2.

mod common;

use common::{Workspace, csv_labels, csv_report, detail, json_report, output_locked};
use serde_json::Value;

/// An `init`-built workspace is ALREADY CURRENT — `init` opened it through the same config facade,
/// which migrated it — so the stamp observed BEFORE this run's own open is already `3` and nothing
/// moves (D46 clause (10); the pre-ruling reason, "the facade outran `Session::migrate`", is retired
/// and is now false).
#[test]
fn migrate_on_an_already_current_workspace_reports_no_move_and_is_idempotent() {
    let ws = Workspace::init();

    let first = json_report(&ws, &["migrate", "--output", "json"]);
    assert_eq!(
        first["kind"], "info",
        "migrate report reuses DiagnosticKind::Info"
    );
    assert_eq!(
        detail(&first, "schema_from"),
        Some("3"),
        "the stamp observed BEFORE this run's own open — already current"
    );
    assert_eq!(
        detail(&first, "schema_to"),
        Some("3"),
        "on-disk schema after"
    );
    assert_eq!(
        detail(&first, "applied"),
        Some("false"),
        "the stamp did not move across this run's own open"
    );
    // The database finding names the workspace DB (a path — assert it ends with the db filename).
    assert!(
        detail(&first, "database").is_some_and(|d| d.ends_with("unblock.db")),
        "migrate report names the workspace db"
    );

    // Second migrate: idempotent — identical from/to/applied.
    let second = json_report(&ws, &["migrate", "--output", "json"]);
    assert_eq!(detail(&second, "schema_from"), Some("3"));
    assert_eq!(detail(&second, "schema_to"), Some("3"));
    assert_eq!(detail(&second, "applied"), Some("false"));
}

/// Run `unblock migrate --output json` and return the report with the volatile `database` path
/// redacted to a stable token, so a snapshot pins the SHAPE (kind + finding labels + schema/applied
/// values) rather than a tempdir path.
fn migrate_report_json(ws: &Workspace) -> Value {
    let out = output_locked(
        ws.cmd().args(["migrate", "--output", "json"]),
        "run migrate",
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "migrate must succeed; stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let mut report: Value = serde_json::from_slice(&out.stdout).expect("valid JSON migrate report");
    if let Some(findings) = report["findings"].as_array_mut() {
        for f in findings {
            if f["label"] == "database" {
                f["detail"] = Value::String("<db-path>".to_string());
            }
        }
    }
    report
}

/// The ALREADY-CURRENT report shape (D46 clause (3) item (a) — RENAMED from
/// `…__migrate_report_fresh.snap`, because D46 gives "fresh" the OPPOSITE meaning: a never-migrated
/// database now reports `0` -> `3` `applied: true`, pinned by the cell below).
#[test]
fn migrate_report_shape_on_an_already_current_workspace_is_snapshot_pinned() {
    let ws = Workspace::init();
    insta::assert_json_snapshot!("migrate_report_already_current", migrate_report_json(&ws));
}

/// `migrate -o csv` migrates, renders the report in the adapter's order and keeps exit 0.
#[test]
fn migrate_csv_reports_the_delta_and_exits_0() {
    let ws = Workspace::init();
    let out = output_locked(ws.cmd().args(["migrate", "--output", "csv"]), "run migrate");
    assert_eq!(
        out.status.code(),
        Some(0),
        "migrate -o csv must exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let records = csv_report(&out.stdout);
    assert_eq!(
        csv_labels(&records),
        ["database", "schema_from", "schema_to", "applied"]
    );
    assert_eq!(
        records[2].1,
        unblock_storage::CURRENT_SCHEMA_VERSION.to_string(),
        "schema_to is the current schema version"
    );
    assert_eq!(
        records[3].1, "false",
        "init already migrated this workspace"
    );
}

/// **D46 clause (10) — the NEVER-MIGRATED direction, which this command could not show AT ALL before.**
///
/// The fixture needs no libsql: a `.unblock/` carrying `config.toml` and NO `unblock.db`, so the
/// facade's own `open_local` creates the file at stamp `0`, the ladder then runs (a fresh database
/// reaches the current shape THROUGH step 2 under the frozen-baseline discipline), and the command
/// reports the pre-open stamp it captured.
///
/// MUTANT KILLED: sourcing `schema_from` from `MigrateOutcome.from` again — which is exactly the
/// pre-ruling code. `Session::migrate` runs AFTER the facade migrated, so it would observe `3` and
/// this cell would read `3` -> `3` `applied: false`.
#[test]
fn migrate_on_a_never_migrated_workspace_reports_zero_to_current_applied() {
    let ws = Workspace::init();
    remove_database(&ws);

    let report = migrate_report_json(&ws);
    assert_eq!(
        detail(&report, "schema_from"),
        Some("0"),
        "an unstamped, not-yet-created database reads 0 BEFORE the facade migrates; report: {report}"
    );
    assert_eq!(detail(&report, "schema_to"), Some("3"));
    assert_eq!(
        detail(&report, "applied"),
        Some("true"),
        "the stamp genuinely moved across this run's own open; report: {report}"
    );
    insta::assert_json_snapshot!("migrate_report_never_migrated", report);
}

/// **D46 — the STALE direction: the case this whole decision exists for.**
///
/// A workspace whose `comments` table is the five-column pre-D37 shape while its stamp says `1` —
/// indistinguishable by `user_version` from a GA database, and unable to serve a single hydrated
/// read. `unblock migrate` repairs it and reports `1` -> `3` `applied: true` (exit 0), the
/// pre-existing comment row survives with both new columns NULL, and the same workspace then serves
/// the hydrated read that failed before.
///
/// MUTANT KILLED: emptying the ladder (`MIGRATIONS: &[]`) — the stale database stays five-column, the
/// hydrated read keeps failing and `schema_to` never reaches `3`.
///
/// MUTANT KILLED: making step 2 unconditional (a version-keyed `ALTER` with no sensing) — this cell
/// still passes, but every ALREADY-CURRENT cell above hard-errors `duplicate column name`, which is
/// the measured behaviour on every database created since 2026-07-17.
#[test]
fn migrate_on_a_stale_workspace_repairs_it_and_reports_the_real_delta() {
    let ws = Workspace::init();
    seed_issue(&ws, "ub-stale");
    seed_comment(
        &ws,
        "ub-stale",
        "a comment written before the columns existed",
    );
    make_comments_table_stale(&ws);

    // Precondition: the fixture really is the broken state (five columns, stamped 1).
    assert_eq!(
        comments_columns(&ws).len(),
        5,
        "the fixture is the 5-column shape"
    );
    assert_eq!(stamped_user_version(&ws), 1, "…while the stamp claims 1");

    let report = migrate_report_json(&ws);
    assert_eq!(
        detail(&report, "schema_from"),
        Some("1"),
        "the pre-open stamp — the stale database's own lie; report: {report}"
    );
    assert_eq!(detail(&report, "schema_to"), Some("3"));
    assert_eq!(
        detail(&report, "applied"),
        Some("true"),
        "the ladder genuinely ran; report: {report}"
    );

    // The repair is additive: the pre-existing row survives, with both new columns NULL.
    assert_eq!(comments_columns(&ws).len(), 7, "the two columns are back");
    let comments = read_comments(&ws, "ub-stale");
    assert_eq!(comments.len(), 1, "the pre-existing comment row survived");
    assert_eq!(
        comments[0].body,
        "a comment written before the columns existed"
    );
    assert!(
        comments[0].updated_at.is_none() && comments[0].redacted_at.is_none(),
        "an ADD COLUMN leaves the existing row NULL — no recreate-and-copy, which would be data loss"
    );
}

/// **D57 clause (9), criterion (l) — a stamp-2 workspace reports `2` -> `3` `applied: true`.** Every
/// GA, v1.0.1 and v1.0.2 database is stamped `2` with no `config` table; `unblock migrate` (whose
/// facade runs step 3 implicitly on open) reports the real delta and the table exists afterwards.
///
/// MUTANT KILLED: dropping step 3 from the ladder (with `CURRENT_SCHEMA_VERSION` back at 2) — the
/// report reads `2` -> `2` `applied: false` and the table never appears.
#[test]
fn migrate_on_a_stamp_two_workspace_reports_two_to_three_applied() {
    let ws = Workspace::init();
    let rt = runtime();
    rt.block_on(async {
        let conn = common::raw_connection(&ws.db_path()).await;
        conn.execute("DROP TABLE config", ())
            .await
            .expect("revert to the stamp-2 shape: no config table");
    });
    stamp_user_version(&ws.db_path(), 2);

    let report = migrate_report_json(&ws);
    assert_eq!(
        detail(&report, "schema_from"),
        Some("2"),
        "report: {report}"
    );
    assert_eq!(detail(&report, "schema_to"), Some("3"), "report: {report}");
    assert_eq!(detail(&report, "applied"), Some("true"), "report: {report}");
    assert_eq!(stamped_user_version(&ws), 3);
}

#[test]
fn migrate_on_a_future_schema_db_exits_2_with_schema_mismatch() {
    // D27/AF-2 newer-DB path, proven END-TO-END at the CLI boundary. The rejection is proven at the
    // storage layer (`libsql::migrate_rejects_future_version`), but NOT via the cli — this closes that
    // gap. Stamp the workspace DB to a FUTURE `PRAGMA user_version` (99), then run `unblock migrate`:
    // the config facade migrates on open, `migrate()` finds found=99 > expected → `SchemaMismatch`,
    // which surfaces transparently as exit 2 + a `SCHEMA_MISMATCH` structured error on stdout (FR-11).
    let ws = Workspace::init();
    stamp_user_version(&ws.db_path(), 99);

    let out = output_locked(
        ws.cmd().args(["migrate", "--output", "json"]),
        "run migrate on a future-schema db",
    );
    assert_eq!(
        out.status.code(),
        Some(2),
        "a newer-than-build DB yields a db-bucket (exit 2) error; stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let value: Value =
        serde_json::from_slice(&out.stdout).expect("valid JSON structured error on stdout (FR-11)");
    assert_eq!(
        value["code"], "SCHEMA_MISMATCH",
        "a future user_version maps to the SCHEMA_MISMATCH code (D27/AF-2)"
    );
}

/// **D46 — the LYING-STAMP path: a database AT the current version whose shape is not the current
/// shape.** The sentinel turns what used to be an opaque `DATABASE_ERROR` on every hydrated read
/// (with `migrate` reporting `applied:false` exit 0 and `doctor` reporting `healthy` exit 0 — three
/// lies) into a typed exit-2 refusal that NAMES the missing columns and carries a self-correction
/// HINT.
///
/// **This is the cell that proves the hint survives the IMPLICIT-ON-OPEN boundary**, not merely that
/// `StorageError` composes one: the failure happens inside the config facade's own `migrate()` at
/// open, so the text reaching stdout has passed through `ConfigError`.
///
/// MUTANT KILLED: deleting the sentinel (`witness_newest_step`) — the command exits 0 claiming
/// success on a database that cannot serve a read.
///
/// MUTANT KILLED: deleting the `ConfigError::hint()` forwarding arm — the trait default
/// `hint() -> None` swallows it and `hint` arrives `null`, which is the contract publishing
/// `contextual_text` with nothing behind it.
///
/// MUTANT KILLED (2026-08-03 ruling): collapsing `StorageError`'s two `Migration` states back onto
/// the single shipped text, which on THIS state advises `unblock migrate` alone — a command that
/// re-emits this identical error forever, because the ladder is skipped at this stamp. The recovery
/// assertions below name the stamp reset, which that text does not carry.
#[test]
fn a_lying_stamp_exits_2_with_a_hint_that_names_the_repair_and_the_missing_columns() {
    let ws = Workspace::init();
    // Current stamp (3) + stale shape (5 columns) — the state the stamp alone cannot detect.
    drop_comment_columns(&ws);
    assert_eq!(
        stamped_user_version(&ws),
        3,
        "the stamp still claims current"
    );

    let out = output_locked(
        ws.cmd().args(["migrate", "--output", "json"]),
        "run migrate on a lying-stamp db",
    );
    assert_eq!(
        out.status.code(),
        Some(2),
        "a shape fault is never reported as a green delta; stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let value: Value =
        serde_json::from_slice(&out.stdout).expect("valid JSON structured error on stdout (FR-11)");
    assert_eq!(
        value["code"], "SCHEMA_MISMATCH",
        "onto the EXISTING code — D46 mints none"
    );
    let hint = value["hint"]
        .as_str()
        .unwrap_or_else(|| panic!("the failure must carry a hint; payload: {value}"));
    assert!(
        hint.contains("PRAGMA user_version = 1") && hint.contains("unblock migrate"),
        "the recovery that WORKS reaches the user end-to-end: reset the lying stamp to the \
         baseline, then migrate (Miguel's ruling, 2026-08-03): {hint}"
    );
    assert!(
        hint.contains("STAMP is what is wrong"),
        "…and it names the state truthfully rather than reporting a delta between two identical \
         numbers: {hint}"
    );
    assert!(
        hint.contains("updated_at") && hint.contains("redacted_at"),
        "the hint must name the columns actually missing: {hint}"
    );
    let lowered = hint.to_lowercase();
    assert!(
        !lowered.contains("re-import") && !lowered.contains("reimport"),
        "`sync export` is itself broken by a stale `comments` table — that advice is invalid: {hint}"
    );
}

/// Delete the workspace database, its WAL sidecars and the `.write.lock` marker, leaving
/// `.unblock/config.toml` behind — the NEVER-MIGRATED fixture (a fresh clone). The facade's own
/// `open_local` recreates the file at stamp `0`. Leaving the marker would make the workspace read as
/// a LOST database, which every storage-bearing open refuses (PRD §4 D58 clause (7)).
fn remove_database(ws: &Workspace) {
    let db = ws.db_path();
    for suffix in ["", "-wal", "-shm"] {
        let mut path = db.clone().into_os_string();
        path.push(suffix);
        let _ = std::fs::remove_file(std::path::PathBuf::from(path));
    }
    let marker = db
        .parent()
        .expect("the database lives in .unblock/")
        .join(".write.lock");
    let _ = std::fs::remove_file(&marker);
    assert!(!db.exists(), "the never-migrated fixture has no unblock.db");
    assert!(
        !marker.exists(),
        "the never-migrated fixture has no .write.lock"
    );
}

/// Revert the `comments` table to its five-column pre-D37 shape AND stamp `user_version = 1` — the
/// STALE fixture: exactly what a build before 2026-07-17 left on disk.
///
/// **Why the columns are DROPPED from a real `init`-built database rather than re-created from a
/// duplicated historical DDL const:** the workspace this cell drives must be a genuine product
/// workspace (every table, index and `config.toml` as `unblock init` writes them), and a second copy
/// of the 38-column baseline DDL living in the cli crate could drift into testing a fiction. The
/// FROZEN HISTORICAL DDL const lives in exactly ONE place, `unblock-storage/tests/migrations.rs`,
/// which is the cell its crate plan assigns it to. Dropping the two post-baseline columns cannot
/// silently become "create a fresh one" — the mutation this construction guards against — because a
/// no-op step 2 leaves them dropped and every assertion above goes red.
fn make_comments_table_stale(ws: &Workspace) {
    drop_comment_columns(ws);
    stamp_user_version(&ws.db_path(), 1);
}

/// Drop the two post-baseline `comments` columns via raw libsql, leaving the stamp untouched.
fn drop_comment_columns(ws: &Workspace) {
    let rt = runtime();
    rt.block_on(async {
        let conn = common::raw_connection(&ws.db_path()).await;
        for column in ["updated_at", "redacted_at"] {
            conn.execute(&format!("ALTER TABLE comments DROP COLUMN {column}"), ())
                .await
                .unwrap_or_else(|e| panic!("drop {column}: {e}"));
        }
    });
}

/// The `comments` column names, in ordinal order, read straight off the file.
fn comments_columns(ws: &Workspace) -> Vec<String> {
    let rt = runtime();
    rt.block_on(async {
        let conn = common::raw_connection(&ws.db_path()).await;
        let mut rows = conn
            .query("PRAGMA table_info(comments)", ())
            .await
            .expect("table_info");
        let mut columns = Vec::new();
        while let Some(row) = rows.next().await.expect("row") {
            if let libsql::Value::Text(name) = row.get_value(1).expect("name") {
                columns.push(name);
            }
        }
        columns
    })
}

/// The `PRAGMA user_version` currently on the file.
fn stamped_user_version(ws: &Workspace) -> i64 {
    let rt = runtime();
    rt.block_on(async {
        let conn = common::raw_connection(&ws.db_path()).await;
        let mut rows = conn.query("PRAGMA user_version", ()).await.expect("uv");
        let row = rows.next().await.expect("row").expect("present");
        row.get_value(0)
            .expect("value")
            .as_integer()
            .copied()
            .expect("integer")
    })
}

// `runtime()` and `stamp_user_version()` were PROMOTED into `common/mod.rs` (D48): the `mcp`
// channel suite provokes the same future-`user_version` case, and two copies of a fixture is how
// two provocations drift apart. Both are re-exported here under their original names so every
// call site below reads as it did.
use common::{runtime, stamp_user_version};

/// Criterion (d): a clean workspace reports `health: healthy` first and exits 0.
#[test]
fn doctor_healthy_exits_0() {
    let ws = Workspace::init();
    let (code, report) = doctor(&ws, false);
    assert_eq!(code, Some(0), "report: {report}");
    assert_eq!(
        report["kind"], "info",
        "doctor reuses DiagnosticKind::Info; report: {report}"
    );
    assert_eq!(
        report["findings"][0]["label"], "health",
        "the level row leads; report: {report}"
    );
    assert_eq!(
        detail(&report, "health"),
        Some("healthy"),
        "report: {report}"
    );
    let labels = labels(&report);
    assert!(
        !labels.iter().any(|l| l == "repair_planned"),
        "a healthy workspace plans nothing; report: {report}"
    );
    assert!(
        !labels
            .iter()
            .any(|l| l == "integrity" || l == "integrity_problem"),
        "the v1 integrity rows are gone; report: {report}"
    );
}

/// Criterion (d): two `metadata` rows sharing a key report `Drifted` and exit 0, and plain `doctor`
/// lists the `dedupe_metadata_key` repair on the online path.
#[test]
fn doctor_drifted_exits_0() {
    let ws = Workspace::init();
    duplicate_metadata_key(&ws, "k");
    let (code, report) = doctor(&ws, false);
    assert_eq!(code, Some(0), "Drifted exits 0; report: {report}");
    assert_eq!(
        detail(&report, "health"),
        Some("drifted"),
        "report: {report}"
    );
    assert!(
        has_label(&report, "duplicate_metadata_key"),
        "report: {report}"
    );
    assert!(
        planned(&report)
            .iter()
            .any(|p| p == "dedupe_metadata_key k (online)"),
        "report: {report}"
    );
}

/// Criterion (d): a lone `-shm` reports `Recoverable` and exits 2.
#[test]
fn doctor_recoverable_exits_2() {
    let ws = Workspace::init();
    lone_shm(&ws);
    let (code, report) = doctor(&ws, false);
    assert_eq!(code, Some(2), "Recoverable exits 2; report: {report}");
    assert_eq!(
        detail(&report, "health"),
        Some("recoverable"),
        "report: {report}"
    );
    assert!(has_label(&report, "sidecar_mismatch"), "report: {report}");
    assert!(
        planned(&report)
            .iter()
            .any(|p| p == "quarantine_sidecar (preopen-exclusive)"),
        "report: {report}"
    );
}

/// Criterion (d): a database stamped above this build reports `schema_stamp_ahead`, `Unsafe`, and
/// exits 2. The read-only open does not refuse the stamp, so `doctor` can classify it.
#[test]
fn doctor_unsafe_exits_2() {
    let ws = Workspace::init();
    stamp_user_version(&ws.db_path(), 99);
    let (code, report) = doctor(&ws, false);
    assert_eq!(code, Some(2), "Unsafe exits 2; report: {report}");
    assert_eq!(
        detail(&report, "health"),
        Some("unsafe"),
        "report: {report}"
    );
    assert!(has_label(&report, "schema_stamp_ahead"), "report: {report}");
}

/// Criteria (d) and (j): an export carrying git conflict markers reports `jsonl_conflict_markers`,
/// `Recoverable`, and exits 2. D29 clause F5 called it advisory `Unsafe` with exit 0. D58 makes it
/// `Recoverable`, because the database regenerates the export.
#[test]
fn doctor_with_jsonl_conflict_markers_reports_recoverable_and_exits_2() {
    let ws = Workspace::init();
    conflicted_export(&ws);
    let (code, report) = doctor(&ws, false);
    assert_eq!(code, Some(2), "report: {report}");
    assert_eq!(
        detail(&report, "health"),
        Some("recoverable"),
        "report: {report}"
    );
    assert!(
        has_label(&report, "jsonl_conflict_markers"),
        "report: {report}"
    );
    assert!(
        planned(&report)
            .iter()
            .any(|p| p.starts_with("regenerate_export") && p.ends_with("(online)")),
        "report: {report}"
    );
}

/// A corrupt database exits 2 in the db bucket.
#[test]
fn doctor_on_a_corrupt_db_exits_2() {
    // Overwrite a large b-tree region past the header page.
    let ws = Workspace::init();
    corrupt_db(&ws.db_path());
    let out = output_locked(
        ws.cmd().args(["doctor", "--output", "json"]),
        "run doctor on corrupt db",
    );
    assert_eq!(
        out.status.code(),
        Some(2),
        "a corrupt DB yields a db-bucket (exit 2) error; stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let value: Value =
        serde_json::from_slice(&out.stdout).expect("valid JSON structured error on stdout (FR-11)");
    assert_eq!(
        value["code"], "DATABASE_ERROR",
        "corruption maps to the db-bucket DATABASE_ERROR code (spine §2.3 unchanged)"
    );
}

/// `doctor -o csv` on a clean workspace renders the health report and keeps exit 0.
#[test]
fn doctor_csv_on_a_healthy_workspace_exits_0() {
    let ws = Workspace::init();
    let out = output_locked(ws.cmd().args(["doctor", "--output", "csv"]), "run doctor");
    assert_eq!(
        out.status.code(),
        Some(0),
        "doctor -o csv must exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let records = csv_report(&out.stdout);
    let labels = csv_labels(&records);
    assert_eq!(
        records.first(),
        Some(&("health".to_string(), "healthy".to_string())),
        "labels: {labels:?}"
    );
    assert!(labels.contains(&"schema_version"), "labels: {labels:?}");
    assert!(labels.contains(&"schema_expected"), "labels: {labels:?}");
}

/// A corrupt database under `-o csv` keeps the human error on stderr, stdout empty and exit 2.
#[test]
fn doctor_csv_on_a_corrupt_db_reports_on_stderr_and_exits_2() {
    let ws = Workspace::init();
    corrupt_db(&ws.db_path());
    let out = output_locked(
        ws.cmd().args(["doctor", "--output", "csv"]),
        "run doctor on corrupt db",
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "exit code; stderr: {stderr}");
    assert!(
        out.stdout.is_empty(),
        "a csv error never reaches stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        stderr.starts_with("error[DATABASE_ERROR]"),
        "the human error line opens stderr: {stderr}"
    );
}

/// A readable database whose `integrity_check` reports an orphaned page renders the report under
/// `-o csv` and keeps exit 2.
#[test]
fn doctor_csv_on_a_readable_integrity_dirty_db_reports_and_exits_2() {
    let ws = Workspace::init();
    orphan_a_page(&ws.db_path());
    let csv = output_locked(
        ws.cmd().args(["doctor", "--output", "csv"]),
        "run doctor -o csv",
    );
    assert_eq!(
        csv.status.code(),
        Some(2),
        "the csv render keeps doctor's exit 2; stderr: {}",
        String::from_utf8_lossy(&csv.stderr)
    );
    let records = csv_report(&csv.stdout);
    assert!(
        records
            .iter()
            .any(|(label, problem)| label == "page_corruption" && problem.contains("never used")),
        "the csv report carries the orphaned page: {records:?}"
    );
}

/// Append one zeroed page to the database file and count it in the header, so `integrity_check`
/// reports the page as never used while every table stays readable.
///
/// The in-header page count is authoritative only while no WAL frame is pending. `SQLite` checkpoints
/// the WAL when `init`'s last connection closes, and the fixture asserts that no frame remains.
fn orphan_a_page(db: &std::path::Path) {
    use std::io::{Read, Seek, SeekFrom, Write};

    let wal = db.with_extension("db-wal");
    assert!(
        !std::fs::metadata(&wal).is_ok_and(|meta| meta.len() > 0),
        "the fixture needs a checkpointed database, but {} holds frames",
        wal.display()
    );
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(db)
        .expect("open db");
    let mut header = [0u8; 100];
    file.read_exact(&mut header).expect("read the db header");
    let page_size = match u16::from_be_bytes([header[16], header[17]]) {
        1 => 65_536,
        size => usize::from(size),
    };
    let page_count = u32::from_be_bytes([header[28], header[29], header[30], header[31]]);
    file.seek(SeekFrom::End(0)).expect("seek to the end");
    file.write_all(&vec![0u8; page_size])
        .expect("append a page");
    file.seek(SeekFrom::Start(28))
        .expect("seek to the page count");
    file.write_all(&(page_count + 1).to_be_bytes())
        .expect("count the new page");
    file.flush().expect("flush");
}

/// Overwrite a deep region of the `SQLite` file with garbage (keeping the `100`-byte header + page 1
/// so the DB still opens) — a page-level corruption that a read surfaces as `database disk image is
/// malformed`. Deterministic across runs.
fn corrupt_db(db: &std::path::Path) {
    use std::io::{Seek, SeekFrom, Write};
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .open(db)
        .expect("open db for corruption");
    // Start of page 2 (default page size 4096); overwrite a large contiguous run of b-tree pages.
    f.seek(SeekFrom::Start(4096)).expect("seek");
    let garbage = vec![0xADu8; 8 * 1024];
    f.write_all(&garbage).expect("corrupt");
    f.flush().expect("flush");
}

/// **D45 — the `doctor` FOLD, at the CLI boundary.** A workspace carrying a dangling dependency edge
/// LISTS it in the `doctor` report, with the pinned finding shape, and STILL exits 0.
///
/// The shape is a decision, not decoration, because it is what a human reads and what a snapshot
/// pins: `label` = the DEPENDENT issue id, `detail` = `"<dep_type> -> <missing target id>"`. The
/// edge TYPE is in there because it is what distinguishes a permanently-stuck issue (a `blocks`
/// target that will never close) from a merely phantom parent.
///
/// The exit stays 0 because the findings are ADVISORY: a dangling edge is a repairable DATA fact,
/// not database corruption, and flipping the exit would change GA-frozen CLI behaviour in a patch
/// release. `json_report` asserts exit 0, so a passing call already proves that.
///
/// **How the edge is planted, and why not through the product.** Since D45 there is no supported
/// path that writes one — that is the entire point of the change — so the corrupt state has to be
/// reached under the guard. This uses the raw-libsql precedent already established in this file by
/// `stamp_user_version`: the ISSUE is created through the real engine (nothing about the row is
/// fabricated), and only the EDGE is inserted directly. The engine-side cell for the same
/// composition lives in `crates/unblock-engine/tests/dangling.rs` behind the `testkit` feature and
/// runs in the required `storage-testkit` CI job; this one is its CLI-boundary twin, and because it
/// needs no feature gate it runs in the ordinary workspace test job.
///
/// MUTANT KILLED: deleting the dangling fold from `Session::doctor()`
/// (`crates/unblock-engine/src/session/lifecycle.rs`) — the finding disappears and the label/detail
/// assertion goes red.
///
/// MUTANT KILLED: flipping the fold to a non-advisory exit — `json_report` fails on the exit code.
#[test]
fn doctor_lists_a_planted_dangling_edge_yet_exits_0() {
    let ws = Workspace::init();
    seed_issue(&ws, "ub-dangler");
    plant_dangling_edge(&ws, "ub-dangler", "ub-ghost", "blocks");

    let report = json_report(&ws, &["doctor", "--output", "json"]);
    assert_eq!(
        report["kind"], "info",
        "the fold REUSES Info — the `Dangling` KIND exists for the `diagnostics` tool arm, where \
         the response must declare what it is; report: {report}"
    );
    assert_eq!(
        detail(&report, "ub-dangler"),
        Some("blocks -> ub-ghost"),
        "the pinned finding shape: label = the DEPENDENT, detail = `<dep_type> -> <missing \
         target>`; report: {report}"
    );
    assert_eq!(
        detail(&report, "health"),
        Some("healthy"),
        "a dangling edge is a DATA fact outside the class table; report: {report}"
    );
}

/// **D46 — `doctor` reports BOTH schema numbers.**
///
/// The two findings are asserted BY VALUE on a workspace this test just created, so both integers are
/// known — **but NOT against `unblock_storage::CURRENT_SCHEMA_VERSION`.** The cell writes the CLI's
/// OWN observable instead: it runs `unblock migrate --output json` against the same workspace and
/// asserts both doctor findings equal that report's `schema_to`, and equal each other. The version is
/// then asserted once and read twice, and the only literal in play is the one the re-blessed migrate
/// snapshot in this same test binary already pins.
///
/// MUTANT KILLED: dropping either finding from the engine fold — the lookup returns `None`.
///
/// The stamp comparison itself (D58 clause (1)) is pinned by
/// `doctor_schema_stamp_behind_is_reported_and_stamp_unchanged` and `doctor_unsafe_exits_2`.
#[test]
fn doctor_reports_the_observed_and_expected_schema_versions_and_still_exits_0() {
    let ws = Workspace::init();

    let migrate = migrate_report_json(&ws);
    let expected = detail(&migrate, "schema_to").expect("the migrate report names schema_to");

    // `json_report` asserts exit 0, so a passing call already proves these findings are ADVISORY.
    let report = json_report(&ws, &["doctor", "--output", "json"]);
    assert_eq!(
        detail(&report, "schema_version"),
        Some(expected),
        "the stamp OBSERVED on disk; report: {report}"
    );
    assert_eq!(
        detail(&report, "schema_expected"),
        Some(expected),
        "the version THIS BUILD expects; report: {report}"
    );
}

/// Create one real issue through the engine (the same `Session` the CLI opens), so the only
/// fabricated thing in the workspace is the edge planted next.
fn seed_issue(ws: &Workspace, id: &str) {
    use unblock_config::{CliOverrides, open_with_storage_with_env};
    use unblock_engine::{Session, SessionConfig};
    use unblock_model::{Issue, Status};

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build a current-thread runtime");
    rt.block_on(async {
        let overrides = CliOverrides::new().with_dir(ws.unblock_dir());
        let ctx = open_with_storage_with_env(None, &overrides, &common::MapEnv::empty())
            .await
            .expect("open the workspace via the config facade");
        let session = Session::open(ctx, SessionConfig::default())
            .await
            .expect("open a session");
        // `Issue::default()` stamps both timestamps at `Utc::now()`, so this row needs no clock of
        // its own (the cli crate carries no `chrono` dependency).
        let issue = Issue {
            id: id.to_string(),
            title: format!("issue {id}"),
            status: Status::Open,
            ..Issue::default()
        };
        session.create(&issue).await.expect("create the issue");
        drop(session);
    });
}

/// Open a `Session` over `ws` through the SAME config facade body the CLI dispatches through (its
/// `open_with_storage_with_env` test entry over an EMPTY environment, D57/NFR-16). The caller drops
/// it before spawning any CLI child, so no connection outlives the call.
async fn open_session(ws: &Workspace) -> unblock_engine::Session {
    use unblock_config::{CliOverrides, open_with_storage_with_env};
    use unblock_engine::{Session, SessionConfig};

    let overrides = CliOverrides::new().with_dir(ws.unblock_dir());
    let ctx = open_with_storage_with_env(None, &overrides, &common::MapEnv::empty())
        .await
        .expect("open the workspace via the config facade");
    Session::open(ctx, SessionConfig::default())
        .await
        .expect("open a session")
}

/// Add one comment through the real engine (so nothing about the row is fabricated).
fn seed_comment(ws: &Workspace, issue_id: &str, body: &str) {
    runtime().block_on(async {
        let session = open_session(ws).await;
        session
            .add_comment(issue_id, body)
            .await
            .expect("add the comment");
        drop(session);
    });
}

/// The HYDRATED comment read — the read a five-column `comments` table breaks, run through the real
/// engine over the real workspace.
fn read_comments(ws: &Workspace, issue_id: &str) -> Vec<unblock_model::Comment> {
    runtime().block_on(async {
        let session = open_session(ws).await;
        let comments = session
            .list_comments(issue_id)
            .await
            .expect("the hydrated comment read must succeed after the repair");
        drop(session);
        comments
    })
}

/// Insert a dependency row DIRECTLY, bypassing every guard — the only way to reach the
/// already-corrupt state the `dangling` view exists to enumerate (see the cell's docstring). The
/// connection is dropped before the CLI child opens the file, so there is no writer contention.
fn plant_dangling_edge(ws: &Workspace, source: &str, target: &str, dep_type: &str) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build a current-thread runtime");
    rt.block_on(async {
        let conn = common::raw_connection(&ws.db_path()).await;
        conn.execute(
            "INSERT INTO dependencies (issue_id, depends_on_id, type, created_at, created_by) \
             VALUES (?1, ?2, ?3, '2026-08-01T00:00:00Z', 'planted')",
            libsql::params![source, target, dep_type],
        )
        .await
        .expect("plant the dangling edge");
    });
}

/// **D57 criterion (d), the CLI half — "`unblock migrate` and `unblock doctor` exit 7 with that
/// hint".** Over a workspace whose `config` table holds a blocking row (a startup key, and an
/// undecodable layer-key value), both commands exit 7 with a `CONFIG_ERROR` document whose
/// `context.keys` names the row and whose hint names its exact recovery call; after the repair
/// (the row removed) both exit 0 again.
#[test]
fn config_migrate_and_doctor_exit_7_with_the_hint_over_a_blocking_row() {
    for (key, value, recovery) in [
        (
            "db_filename",
            "x.db",
            r#"issue {"action":"config","op":"unset","key":"db_filename"}"#,
        ),
        (
            "output_format",
            "xml",
            r#"issue {"action":"config","op":"get","key":"output_format"}"#,
        ),
    ] {
        let ws = Workspace::init();
        common::plant_config_row(&ws.db_path(), key, value);
        for command in ["migrate", "doctor"] {
            let out = output_locked(
                ws.cmd().args([command, "--output", "json"]),
                "run a gated lifecycle command",
            );
            assert_eq!(
                out.status.code(),
                Some(7),
                "{command} over a blocking `{key}` row; stderr: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            let error: Value = serde_json::from_slice(&out.stdout).expect("a JSON error document");
            assert_eq!(error["code"], "CONFIG_ERROR", "{command}: {error}");
            assert_eq!(
                error["context"]["keys"],
                serde_json::json!([key]),
                "{command}: {error}"
            );
            let hint = error["hint"].as_str().unwrap_or_default();
            assert!(
                hint.contains(recovery),
                "{command}: names `{recovery}`: {hint}"
            );
        }
        common::runtime().block_on(async {
            let conn = common::raw_connection(&ws.db_path()).await;
            conn.execute("DELETE FROM config", ())
                .await
                .expect("repair: remove the row");
        });
        for command in ["migrate", "doctor"] {
            let out = output_locked(ws.cmd().arg(command), "run the command after the repair");
            assert_eq!(out.status.code(), Some(0), "{command} after the repair");
        }
    }
}

// ----------------------------------------------------------------------------------------------
// D58 — `doctor` and `doctor --repair` (PRD §4 D58 clauses (6) to (9), criteria (b) to (j)).
// ----------------------------------------------------------------------------------------------

/// Criterion (b): one entry removed from an index b-tree reports `index_corruption`, `Recoverable`,
/// and exits 2. `doctor --repair` reindexes it, `integrity_check` is clean afterwards, and the run
/// exits 0.
#[test]
fn doctor_index_entry_removed_reports_recoverable_exit_2_and_repair_leaves_integrity_clean_exit_0()
{
    let ws = Workspace::init();
    drop_an_index_entry(&ws, "idx_metadata_key");

    let (code, report) = doctor(&ws, false);
    assert_eq!(code, Some(2), "report: {report}");
    assert_eq!(
        detail(&report, "health"),
        Some("recoverable"),
        "report: {report}"
    );
    assert!(has_label(&report, "index_corruption"), "report: {report}");
    assert!(
        !has_label(&report, "page_corruption"),
        "every row is index-only; report: {report}"
    );
    assert!(
        planned(&report)
            .iter()
            .any(|p| p == "reindex idx_metadata_key (online)"),
        "report: {report}"
    );

    let (code, report) = doctor(&ws, true);
    assert_eq!(code, Some(0), "report: {report}");
    assert_eq!(
        detail(&report, "health"),
        Some("healthy"),
        "report: {report}"
    );
    assert!(
        repairs(&report)
            .iter()
            .any(|r| r.starts_with("reindex idx_metadata_key") && r.ends_with(": applied")),
        "report: {report}"
    );
    assert!(integrity_rows(&ws).is_empty(), "integrity_check is clean");
}

/// Criterion (b): an orphaned page reports `page_corruption`, `Unsafe`, and exits 2. `--repair`
/// runs no action, creates no run directory, and still exits 2.
#[test]
fn doctor_orphan_a_page_reports_page_corruption_exit_2_and_repair_runs_nothing_creates_no_run_dir()
{
    let ws = Workspace::init();
    orphan_a_page(&ws.db_path());

    let (code, report) = doctor(&ws, false);
    assert_eq!(code, Some(2), "report: {report}");
    assert_eq!(
        detail(&report, "health"),
        Some("unsafe"),
        "report: {report}"
    );
    assert!(
        detail(&report, "page_corruption").is_some_and(|row| row.contains("never used")),
        "report: {report}"
    );

    let (code, report) = doctor(&ws, true);
    assert_eq!(code, Some(2), "report: {report}");
    assert_eq!(
        detail(&report, "health"),
        Some("unsafe"),
        "report: {report}"
    );
    assert!(
        repairs(&report).is_empty(),
        "no action ran; report: {report}"
    );
    assert!(
        !has_label(&report, "evidence"),
        "no evidence; report: {report}"
    );
    assert!(run_dirs(&ws).is_empty(), "no run directory");
}

/// The Unsafe gate (spec §5b (i-b), ruling 389): an orphaned page beside a lowered `child_counters`
/// row and a lone `-shm` makes `--repair` repair nothing. It renders plain `doctor`'s `Unsafe`
/// report, exits 2, leaves the counter lowered and the `-shm` in place, and creates no run directory.
/// Without the gate the window would quarantine the `-shm` into a new run directory.
#[test]
fn repair_with_page_corruption_and_drift_repairs_nothing_exit_2_no_run_dir() {
    let ws = Workspace::init();
    seed_issue(&ws, "ub-p");
    assert_eq!(create_children(&ws, "ub-p", 2), ["ub-p.1", "ub-p.2"]);
    set_child_counter(&ws, "ub-p", 1);
    orphan_a_page(&ws.db_path());
    lone_shm(&ws);

    assert_gate_repairs_nothing(&ws, "page_corruption");
}

/// The Unsafe gate (spec §5b (i-b), ruling 389): a `PRAGMA user_version` above this build's beside a
/// lowered `child_counters` row and a lone `-shm` makes `--repair` repair nothing, exit 2, and
/// create no run directory. Without the gate the window would quarantine the `-shm`.
#[test]
fn repair_with_schema_stamp_ahead_and_drift_repairs_nothing_exit_2_no_run_dir() {
    let ws = Workspace::init();
    seed_issue(&ws, "ub-p");
    assert_eq!(create_children(&ws, "ub-p", 2), ["ub-p.1", "ub-p.2"]);
    set_child_counter(&ws, "ub-p", 1);
    stamp_user_version(&ws.db_path(), unblock_storage::CURRENT_SCHEMA_VERSION + 1);
    lone_shm(&ws);

    assert_gate_repairs_nothing(&ws, "schema_stamp_ahead");
}

/// Run `doctor --repair` on a workspace whose `ub-p` counter is lowered to 1, a lone `-shm` sits
/// beside the database, and `unsafe_class` makes it `Unsafe`. Asserts the gate's report and that
/// nothing moved.
fn assert_gate_repairs_nothing(ws: &Workspace, unsafe_class: &str) {
    let shm = sidecar(ws, "-shm");
    let (code, report) = doctor(ws, true);
    assert_eq!(code, Some(2), "report: {report}");
    assert_eq!(
        detail(&report, "health"),
        Some("unsafe"),
        "report: {report}"
    );
    assert!(has_label(&report, unsafe_class), "report: {report}");
    assert!(
        repairs(&report).is_empty(),
        "no repair ran; report: {report}"
    );
    assert!(
        !has_label(&report, "evidence"),
        "no evidence; report: {report}"
    );
    // The `-shm` check comes first: the counter read's own read-write connection removes the `-wal`
    // and `-shm` when it closes.
    assert!(shm.exists(), "the -shm stays in place");
    assert_eq!(child_counter(ws, "ub-p"), 1, "the counter stays lowered");
    assert!(
        !ws.unblock_dir().join(".recovery").exists(),
        "no .unblock/.recovery is created"
    );
}

/// Criterion (c): a `child_counters` row lowered below its parent's greatest child reports
/// `child_counter_drift`, `Drifted`, exit 0, and plain `doctor` lists `raise_child_counter`.
/// `--repair` raises it to that child number, and the next create under the parent mints the
/// following number.
#[test]
fn doctor_lists_raise_child_counter_and_repair_raises_it() {
    let ws = Workspace::init();
    seed_issue(&ws, "ub-p");
    let children = create_children(&ws, "ub-p", 3);
    assert_eq!(children, ["ub-p.1", "ub-p.2", "ub-p.3"]);
    set_child_counter(&ws, "ub-p", 1);

    let (code, report) = doctor(&ws, false);
    assert_eq!(code, Some(0), "Drifted exits 0; report: {report}");
    assert_eq!(
        detail(&report, "health"),
        Some("drifted"),
        "report: {report}"
    );
    assert!(
        has_label(&report, "child_counter_drift"),
        "report: {report}"
    );
    assert!(
        planned(&report)
            .iter()
            .any(|p| p.starts_with("raise_child_counter") && p.ends_with("(online)")),
        "report: {report}"
    );
    assert_eq!(
        child_counter(&ws, "ub-p"),
        1,
        "plain doctor changes no database content"
    );

    let (code, report) = doctor(&ws, true);
    assert_eq!(code, Some(0), "report: {report}");
    assert_eq!(
        detail(&report, "health"),
        Some("healthy"),
        "report: {report}"
    );
    assert_eq!(
        child_counter(&ws, "ub-p"),
        3,
        "raised to the greatest child"
    );
    assert_eq!(create_children(&ws, "ub-p", 1), ["ub-p.4"]);
}

/// Criterion (c): a database stamped one step behind reports `schema_stamp_behind`, `Drifted`,
/// exit 0, and plain `doctor` leaves the stamp and the schema unchanged.
#[test]
fn doctor_schema_stamp_behind_is_reported_and_stamp_unchanged() {
    let ws = Workspace::init();
    runtime().block_on(async {
        let conn = common::raw_connection(&ws.db_path()).await;
        conn.execute("DROP TABLE config", ())
            .await
            .expect("revert to the stamp-2 shape: no config table");
    });
    stamp_user_version(&ws.db_path(), 2);

    let (code, report) = doctor(&ws, false);
    assert_eq!(code, Some(0), "Drifted exits 0; report: {report}");
    assert_eq!(
        detail(&report, "health"),
        Some("drifted"),
        "report: {report}"
    );
    assert!(
        has_label(&report, "schema_stamp_behind"),
        "report: {report}"
    );
    assert!(
        planned(&report).iter().any(|p| p == "migrate (online)"),
        "report: {report}"
    );
    assert_eq!(detail(&report, "schema_version"), Some("2"), "{report}");
    assert_eq!(stamped_user_version(&ws), 2, "the stamp is unchanged");
    assert!(!has_table(&ws, "config"), "the schema is unchanged");
}

/// Criterion (g) and ruling 390 (capture before touch): a lone `-shm` is renamed into `quarantine/`
/// with its SHA-256 in the manifest, and the final level is `Healthy`. The gate's read-only open
/// rebuilds the `-shm` as a derived wal-index, so the quarantined copy is the rebuilt one. The
/// ORIGINAL bytes, captured before that open, are under `original/` and the manifest's
/// `originals` records their SHA-256 and the rebuild. A capture taken after the gate open would
/// hash the rebuilt bytes and fail the `originals` assertion.
#[test]
fn repair_lone_shm_quarantined_final_healthy() {
    let ws = Workspace::init();
    let shm = lone_shm(&ws);

    let (code, report) = doctor(&ws, true);
    assert_eq!(code, Some(0), "report: {report}");
    assert_eq!(
        detail(&report, "health"),
        Some("healthy"),
        "report: {report}"
    );
    assert!(
        repairs(&report)
            .iter()
            .any(|r| r.starts_with("quarantine_sidecar") && r.ends_with(": applied")),
        "report: {report}"
    );
    let run = only_run_dir(&ws);
    assert!(
        detail(&report, "evidence")
            .is_some_and(|dir| Path::new(dir).ends_with(run.file_name().expect("name"))),
        "the evidence row names the run directory; report: {report}"
    );
    let name = file_name(&ws, "-shm");
    let moved = run.join("quarantine").join(&name);
    let rebuilt = std::fs::read(&moved).expect("quarantined -shm");
    assert_ne!(
        rebuilt, shm,
        "the gate's read-only open rebuilt the -shm before the window moved it"
    );
    let manifest = manifest(&run);
    assert_eq!(
        manifest_sha(&manifest, &format!("quarantine/{name}")),
        sha256_hex(&rebuilt),
        "manifest: {manifest}"
    );
    let original = manifest_original(&manifest, &format!("original/{name}"));
    assert_eq!(
        original["sha256"].as_str(),
        Some(sha256_hex(&shm).as_str()),
        "the original's SHA-256 is the pre-test bytes'; manifest: {manifest}"
    );
    assert_eq!(
        original["bytes"].as_u64(),
        Some(u64::try_from(shm.len()).expect("length")),
        "manifest: {manifest}"
    );
    assert!(
        original["note"]
            .as_str()
            .is_some_and(|note| note.contains("rebuilt")),
        "the original records the rebuild; manifest: {manifest}"
    );
    assert_eq!(
        std::fs::read(run.join("original").join(&name)).expect("the original -shm copy"),
        shm,
        "the original bytes are preserved"
    );
}

/// Criterion (g): a 5-byte `-wal` is byte-preserved under `raw/`, `SQLite` removes it at the window's
/// close, and the final level is `Healthy`.
///
/// The later phases open the database in WAL mode again, so a fresh, empty `-wal` sits beside it
/// when the command ends. A hard link taken before the run tells the two files apart: once the
/// window's close unlinks the 5-byte `-wal`, the link is that inode's only name.
#[cfg(unix)]
#[test]
fn repair_five_byte_wal_preserved_in_raw_absent_after_final_healthy() {
    use std::os::unix::fs::MetadataExt as _;

    let ws = Workspace::init();
    let wal = five_byte_wal(&ws);
    let witness = ws.root().join("wal-witness");
    std::fs::hard_link(sidecar(&ws, "-wal"), &witness).expect("hard-link the -wal");

    let (code, report) = doctor(&ws, true);
    assert_eq!(code, Some(0), "report: {report}");
    assert_eq!(
        detail(&report, "health"),
        Some("healthy"),
        "report: {report}"
    );
    assert!(
        repairs(&report)
            .iter()
            .any(|r| r.starts_with("checkpoint_truncate") && r.ends_with(": applied")),
        "report: {report}"
    );
    let run = only_run_dir(&ws);
    let raw = run.join("raw").join(file_name(&ws, "-wal"));
    assert_eq!(std::fs::read(&raw).expect("raw -wal copy"), wal);
    let manifest = manifest(&run);
    assert_eq!(
        manifest_sha(&manifest, &format!("raw/{}", file_name(&ws, "-wal"))),
        sha256_hex(&wal),
        "manifest: {manifest}"
    );
    let witness_meta = std::fs::metadata(&witness).expect("the witness link");
    assert_eq!(
        witness_meta.nlink(),
        1,
        "the 5-byte -wal's name is gone; the witness is its only link"
    );
    if let Ok(live) = std::fs::metadata(sidecar(&ws, "-wal")) {
        assert_ne!(live.ino(), witness_meta.ino(), "a later phase's fresh -wal");
        assert_eq!(live.len(), 0, "the later phases leave an empty -wal");
    }
}

/// Criterion (g): plain `doctor` on a hot `-journal` renders the session-less pre-open report
/// (`journal_sidecar_present`, exit 2) instead of failing with `SQLITE_READONLY_ROLLBACK`, and
/// changes neither file.
#[test]
fn doctor_hot_journal_renders_session_less_report_exit_2() {
    let ws = Workspace::init();
    let journal = hot_journal(&ws);
    let db = std::fs::read(ws.db_path()).expect("read db");

    let (code, report) = doctor(&ws, false);
    assert_eq!(code, Some(2), "report: {report}");
    assert_eq!(
        detail(&report, "health"),
        Some("recoverable"),
        "report: {report}"
    );
    assert!(
        has_label(&report, "journal_sidecar_present"),
        "report: {report}"
    );
    assert!(
        planned(&report)
            .iter()
            .any(|p| p == "quarantine_journal (preopen-exclusive)"),
        "report: {report}"
    );
    assert!(
        !has_label(&report, "schema_version"),
        "the session-less report reads no database row; report: {report}"
    );
    assert!(
        details(&report, "journal_sidecar_present").iter().any(|d| {
            d.ends_with(
            "the database classes are read only after `unblock doctor --repair` lets SQLite roll \
             the journal back"
        )
        }),
        "clause (8) names when the database classes are read; report: {report}"
    );
    assert_eq!(
        std::fs::read(sidecar(&ws, "-journal")).expect("journal"),
        journal
    );
    assert_eq!(std::fs::read(ws.db_path()).expect("db"), db);
}

/// Criterion (g), as amended (PRD §4 D58, measured 2026-11-13): `--repair` lets `SQLite` roll a hot
/// `-journal` back in the window, and that rollback IS the repair. The pre-rollback bytes are the
/// `raw/` evidence with their SHA-256 in the manifest, the report says the journal was rolled back,
/// nothing is quarantined, the `-journal` is absent afterwards (like the `-wal`), and the final
/// level is `Healthy`.
#[test]
fn repair_hot_journal_is_rolled_back_by_sqlite_with_raw_evidence_and_absent_after() {
    let ws = Workspace::init();
    let journal = hot_journal(&ws);

    let (code, report) = doctor(&ws, true);
    assert_eq!(code, Some(0), "report: {report}");
    assert_eq!(
        detail(&report, "health"),
        Some("healthy"),
        "report: {report}"
    );
    let run = only_run_dir(&ws);
    let name = file_name(&ws, "-journal");
    assert_eq!(
        std::fs::read(run.join("raw").join(&name)).expect("raw journal copy"),
        journal,
        "the pre-rollback bytes are preserved"
    );
    assert!(
        !sidecar(&ws, "-journal").exists(),
        "no live journal remains; tree: {:#?}",
        file_tree(&ws.unblock_dir()).keys().collect::<Vec<_>>()
    );
    assert_eq!(
        manifest_sha(&manifest(&run), &format!("raw/{name}")),
        sha256_hex(&journal),
        "the manifest hashes the pre-rollback raw/ copy"
    );
    assert!(
        !run.join("quarantine").join(&name).exists(),
        "the rolled-back journal is never renamed"
    );
    assert!(
        repairs(&report).iter().any(|r| r
            == "quarantine_journal: applied (SQLite rolled the hot journal back; the pre-rollback copy is under raw/)"),
        "report: {report}"
    );
    assert!(
        integrity_rows(&ws).is_empty(),
        "the rolled-back database is clean"
    );
}

/// Criterion (g): no file is ever unlinked by unblock. Every file present before `--repair` is still
/// in the workspace, in place or under the run's `quarantine/`. A `raw/` copy is evidence, not a
/// kept file, so it only accounts for the `-wal` and the rolled-back `-journal` `SQLite` itself
/// removes. In place means the same file: a `-shm` or `-wal` that a later open recreated under the
/// same name does not count (same inode on unix, same bytes elsewhere).
#[test]
fn repair_never_unlinks_a_file() {
    type Fixture = (&'static str, fn(&Workspace));
    let fixtures: [Fixture; 4] = [
        ("lone -shm", |ws| {
            lone_shm(ws);
        }),
        ("5-byte -wal", |ws| {
            five_byte_wal(ws);
        }),
        ("hot -journal", |ws| {
            hot_journal(ws);
        }),
        ("not SQLite", |ws| {
            not_sqlite(ws);
        }),
    ];
    for (what, fixture) in fixtures {
        let ws = Workspace::init();
        fixture(&ws);
        let before = top_level_files(&ws.unblock_dir());
        let identities: BTreeMap<String, FileIdentity> = before
            .iter()
            .map(|name| (name.clone(), FileIdentity::of(&ws.unblock_dir().join(name))))
            .collect();
        let (_, report) = doctor(&ws, true);
        let runs = run_dirs(&ws);
        let sqlite_removes = [file_name(&ws, "-wal"), file_name(&ws, "-journal")];
        for name in before {
            let live = ws.unblock_dir().join(&name);
            let same_in_place = live.exists() && FileIdentity::of(&live) == identities[&name];
            let kept = same_in_place
                || runs
                    .iter()
                    .any(|run| run.join("quarantine").join(&name).exists());
            let evidence = runs.iter().any(|run| run.join("raw").join(&name).exists());
            assert!(
                kept || (sqlite_removes.contains(&name) && evidence),
                "{what}: {name} vanished; report: {report}"
            );
        }
    }
}

/// Which file a path names: its inode on unix, its bytes elsewhere.
#[derive(Debug, PartialEq, Eq)]
enum FileIdentity {
    #[cfg(unix)]
    Inode(u64, u64),
    #[cfg(not(unix))]
    Bytes(Vec<u8>),
}

impl FileIdentity {
    fn of(path: &Path) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            let meta = std::fs::metadata(path).expect("stat");
            Self::Inode(meta.dev(), meta.ino())
        }
        #[cfg(not(unix))]
        {
            Self::Bytes(std::fs::read(path).expect("read"))
        }
    }
}

/// A named fixture of the capture-refusal cell.
type CaptureFixture = (&'static str, Box<dyn Fn(&Workspace)>);

/// Ruling 390 as amended (tracker comment 393): a sidecar `--repair` cannot capture is a refusal on
/// `DATABASE_ERROR`, exit 2, before the gate and before anything is written: a lone `-shm` above
/// the 64 MiB capture cap (sparse, so the cell writes no data) and, on unix, a symlinked `-shm`.
#[test]
fn repair_refuses_a_sidecar_it_cannot_capture_exit_2_and_writes_nothing() {
    let over_the_cap = |ws: &Workspace| {
        lone_shm(ws);
        std::fs::OpenOptions::new()
            .write(true)
            .open(sidecar(ws, "-shm"))
            .and_then(|file| file.set_len(64 * 1024 * 1024 + 1))
            .expect("a sparse -shm above the cap");
    };
    let mut fixtures: Vec<CaptureFixture> = vec![("over the cap", Box::new(over_the_cap))];
    #[cfg(unix)]
    fixtures.push((
        "symlinked",
        Box::new(|ws: &Workspace| {
            remove_empty_sidecar(ws, "-wal");
            let target = ws.root().join("elsewhere-shm");
            std::fs::write(&target, vec![0x5A; 32 * 1024]).expect("symlink target");
            std::os::unix::fs::symlink(&target, sidecar(ws, "-shm")).expect("symlink the -shm");
        }),
    ));
    for (what, fixture) in fixtures {
        let ws = Workspace::init();
        fixture(&ws);
        let before = file_tree(&ws.unblock_dir());
        let (code, report) = doctor(&ws, true);
        assert_eq!(code, Some(2), "{what}: {report}");
        assert_eq!(
            report["code"].as_str(),
            Some("DATABASE_ERROR"),
            "{what}: the refusal rides DATABASE_ERROR; {report}"
        );
        assert!(
            !ws.unblock_dir().join(".recovery").exists(),
            "{what}: no evidence is written"
        );
        assert_eq!(
            file_tree(&ws.unblock_dir()),
            before,
            "{what}: nothing in .unblock/ changes"
        );
    }
}

/// The hot-journal exception to the Unsafe gate (tracker comment 393): a hot `-journal` beside a
/// page-corrupt database with a lowered counter. The journal hides the database classes, so the
/// window runs: its `raw/` copy keeps the pre-rollback bytes and `SQLite` rolls the journal back.
/// `recover()`'s gate then sees `page_corruption` and runs NO SQL repair (the counter stays
/// lowered), and `--repair` exits 2.
#[test]
fn repair_hot_journal_beside_page_corruption_rolls_back_and_repairs_no_sql_exit_2() {
    let ws = Workspace::init();
    seed_issue(&ws, "ub-p");
    assert_eq!(create_children(&ws, "ub-p", 2), ["ub-p.1", "ub-p.2"]);
    set_child_counter(&ws, "ub-p", 1);
    orphan_a_page(&ws.db_path());
    let journal = hot_journal(&ws);

    let (code, report) = doctor(&ws, true);
    assert_eq!(code, Some(2), "report: {report}");
    assert_eq!(
        detail(&report, "health"),
        Some("unsafe"),
        "report: {report}"
    );
    assert!(has_label(&report, "page_corruption"), "report: {report}");
    let run = only_run_dir(&ws);
    let name = file_name(&ws, "-journal");
    assert_eq!(
        std::fs::read(run.join("raw").join(&name)).expect("raw journal copy"),
        journal,
        "the pre-rollback bytes are the evidence"
    );
    assert!(!sidecar(&ws, "-journal").exists(), "SQLite rolled it back");
    assert!(
        repairs(&report)
            .iter()
            .all(|r| r.starts_with("quarantine_journal")),
        "no SQL repair ran; report: {report}"
    );
    assert_eq!(child_counter(&ws, "ub-p"), 1, "the counter stays lowered");
}

/// Criterion (h): with `.write.lock` present and the database absent (and again 0 bytes), plain
/// `doctor` reports `database_lost`/`Unsafe`, exits 2, opens no `SQLite` connection (nothing in the
/// workspace changes) and names the recovery path.
#[test]
fn doctor_lost_database_reports_unsafe_exit_2_opens_nothing_names_recovery_path() {
    for empty in [false, true] {
        let ws = Workspace::init();
        remove_database_keep_marker(&ws);
        if empty {
            std::fs::write(ws.db_path(), b"").expect("a 0-byte database");
        }
        assert!(
            ws.unblock_dir().join(".write.lock").exists(),
            "the marker exists"
        );
        let before = file_tree(ws.root());

        let (code, report) = doctor(&ws, false);
        assert_eq!(code, Some(2), "empty={empty}; report: {report}");
        assert_eq!(
            detail(&report, "health"),
            Some("unsafe"),
            "report: {report}"
        );
        assert!(has_label(&report, "database_lost"), "report: {report}");
        assert!(
            planned(&report)
                .iter()
                .any(|p| p.starts_with("manual (manual): ") && p.contains("unblock init")),
            "the manual row names the recovery path; report: {report}"
        );
        assert_eq!(
            file_tree(ws.root()),
            before,
            "empty={empty}: nothing changed"
        );
    }
}

/// Criterion (h): a fresh clone (no database, no `.write.lock`) reports the `database_not_created`
/// advisory and exits 0. Neither the database nor `.write.lock` exists afterwards, and
/// `unblock migrate` then creates the database.
#[test]
fn doctor_fresh_clone_reports_database_not_created_exit_0_creates_nothing() {
    let ws = Workspace::init();
    remove_database(&ws);
    let marker = ws.unblock_dir().join(".write.lock");

    let (code, report) = doctor(&ws, false);
    assert_eq!(code, Some(0), "report: {report}");
    assert_eq!(
        detail(&report, "health"),
        Some("healthy"),
        "an advisory never raises the level; report: {report}"
    );
    assert!(
        has_label(&report, "database_not_created"),
        "report: {report}"
    );
    assert!(!ws.db_path().exists(), "no database is created");
    assert!(!marker.exists(), "no .write.lock is created");

    let out = output_locked(ws.cmd().arg("migrate"), "run migrate");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(ws.db_path().exists(), "migrate creates the database");
}

/// Criterion (h): an unreadable database reports `database_unreadable`/`Unsafe`, exits 2, and
/// `--repair` moves nothing.
///
/// A process that reads a mode-000 file (root) cannot build this fixture. It asserts instead that
/// the mode bits alone never classify the file unreadable: plain `doctor` reads it and reports
/// `healthy`, exit 0. The unreadable path is then covered by
/// `unblock-health/tests/classify_props.rs::database_unreadable_and_not_sqlite_are_unsafe_without_db_facts`
/// (the class is `Unsafe`), this crate's `doctor::tests::only_unsafe_stops_repair_at_the_gate` and
/// `only_a_sqlite_header_reaches_the_window` (an `Unsafe` or unreadable database repairs nothing),
/// and `doctor_not_sqlite_is_unsafe_and_repair_moves_nothing_exit_2` end to end.
#[cfg(unix)]
#[test]
fn doctor_unreadable_database_reports_unsafe_and_repair_moves_nothing() {
    use std::os::unix::fs::PermissionsExt as _;

    let ws = Workspace::init();
    let db = ws.db_path();
    std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o000)).expect("chmod 000");
    if std::fs::read(&db).is_ok() {
        let (code, report) = doctor(&ws, false);
        std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o644)).expect("restore");
        assert_eq!(code, Some(0), "root reads the file; report: {report}");
        assert_eq!(
            detail(&report, "health"),
            Some("healthy"),
            "report: {report}"
        );
        assert!(
            !has_label(&report, "database_unreadable"),
            "a readable mode-000 file is not unreadable; report: {report}"
        );
        return;
    }
    let before = file_tree(ws.root());

    let (code, report) = doctor(&ws, false);
    assert_eq!(code, Some(2), "report: {report}");
    assert_eq!(
        detail(&report, "health"),
        Some("unsafe"),
        "report: {report}"
    );
    assert!(
        has_label(&report, "database_unreadable"),
        "report: {report}"
    );

    let (code, report) = doctor(&ws, true);
    assert_eq!(code, Some(2), "report: {report}");
    assert!(
        repairs(&report).is_empty(),
        "no action ran; report: {report}"
    );
    assert_eq!(
        detail(&report, "health"),
        Some("unsafe"),
        "report: {report}"
    );
    let after = file_tree(ws.root());
    std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o644)).expect("restore");
    assert_eq!(after, before, "--repair moved nothing");
}

/// Criterion (h) and the Unsafe gate (ruling 389): a non-SQLite database reports
/// `database_not_sqlite`/`Unsafe` without a `SQLite` open, and `--repair` moves nothing. The file
/// bytes stay unchanged, no `.recovery/` is created, the run exits 2, and the report names the
/// manual path: move the file aside by hand, `unblock init`, then import an export.
#[test]
fn doctor_not_sqlite_is_unsafe_and_repair_moves_nothing_exit_2() {
    let ws = Workspace::init();
    let bytes = not_sqlite(&ws);

    let (code, report) = doctor(&ws, false);
    assert_eq!(code, Some(2), "report: {report}");
    assert_eq!(
        detail(&report, "health"),
        Some("unsafe"),
        "report: {report}"
    );
    assert!(
        has_label(&report, "database_not_sqlite"),
        "report: {report}"
    );
    assert_eq!(
        std::fs::read(ws.db_path()).expect("db"),
        bytes,
        "plain doctor moves nothing"
    );
    let before = file_tree(ws.root());

    let (code, report) = doctor(&ws, true);
    assert_eq!(code, Some(2), "report: {report}");
    assert_eq!(
        detail(&report, "health"),
        Some("unsafe"),
        "report: {report}"
    );
    assert!(
        has_label(&report, "database_not_sqlite"),
        "report: {report}"
    );
    assert!(
        repairs(&report).is_empty(),
        "no repair ran; report: {report}"
    );
    assert!(
        planned(&report)
            .iter()
            .any(|p| p.starts_with("manual (manual): ")
                && p.contains("by hand into a new directory under .unblock/.recovery/")
                && p.contains("`unblock init`")
                && p.contains("import the last export")),
        "the manual row names the manual path; report: {report}"
    );
    assert_eq!(
        std::fs::read(ws.db_path()).expect("db"),
        bytes,
        "the file bytes are unchanged"
    );
    assert!(
        !ws.unblock_dir().join(".recovery").exists(),
        "no .unblock/.recovery is created"
    );
    assert_eq!(file_tree(ws.root()), before, "--repair moved nothing");
}

/// Ruling 390: plain `doctor` changes no database content, but `SQLite`'s read-only open of a WAL
/// database creates its housekeeping files. On a clean database with no sidecar it leaves an empty
/// `-wal` and a `-shm` beside the unchanged database file, and the report stays `healthy`.
#[test]
fn plain_doctor_on_a_clean_db_creates_empty_wal_and_shm_and_leaves_db_bytes_unchanged() {
    let ws = Workspace::init();
    remove_empty_sidecar(&ws, "-wal");
    remove_empty_sidecar(&ws, "-shm");
    let db = std::fs::read(ws.db_path()).expect("db");

    let (code, report) = doctor(&ws, false);
    assert_eq!(code, Some(0), "report: {report}");
    assert_eq!(
        detail(&report, "health"),
        Some("healthy"),
        "report: {report}"
    );
    assert_eq!(
        std::fs::read(ws.db_path()).expect("db"),
        db,
        "the database bytes are unchanged"
    );
    assert_eq!(
        std::fs::metadata(sidecar(&ws, "-wal"))
            .expect("the read-only open creates a -wal")
            .len(),
        0,
        "the -wal is empty"
    );
    assert!(
        sidecar(&ws, "-shm").exists(),
        "the read-only open creates a -shm"
    );
    assert!(
        !ws.unblock_dir().join(".recovery").exists(),
        "no .recovery/ entry"
    );
}

/// Criterion (i): plain `doctor` creates no `.recovery/` entry and leaves the database bytes and
/// `.write.lock` untouched, even with a repair planned. On a fresh clone `.write.lock` stays ABSENT.
#[test]
fn plain_doctor_creates_no_recovery_entry_and_leaves_db_and_write_lock_untouched() {
    let ws = Workspace::init();
    duplicate_metadata_key(&ws, "k");
    let marker = ws.unblock_dir().join(".write.lock");
    let db = std::fs::read(ws.db_path()).expect("db");
    let lock = std::fs::read(&marker).expect(".write.lock");
    let lock_mtime = std::fs::metadata(&marker)
        .and_then(|m| m.modified())
        .expect("mtime");

    let (code, report) = doctor(&ws, false);
    assert_eq!(code, Some(0), "report: {report}");
    assert!(
        !planned(&report).is_empty(),
        "a repair is planned; report: {report}"
    );
    assert!(
        !ws.unblock_dir().join(".recovery").exists(),
        "no .recovery/ entry"
    );
    assert_eq!(
        std::fs::read(ws.db_path()).expect("db"),
        db,
        "database bytes"
    );
    assert_eq!(std::fs::read(&marker).expect(".write.lock"), lock);
    assert_eq!(
        std::fs::metadata(&marker)
            .and_then(|m| m.modified())
            .expect("mtime"),
        lock_mtime,
        ".write.lock untouched"
    );

    let fresh = Workspace::init();
    remove_database(&fresh);
    let fresh_marker = fresh.unblock_dir().join(".write.lock");
    let (code, report) = doctor(&fresh, false);
    assert_eq!(code, Some(0), "report: {report}");
    assert!(
        !fresh_marker.exists(),
        "fresh clone: .write.lock stays absent"
    );
    assert!(!fresh.unblock_dir().join(".recovery").exists());
}

/// Criterion (i): a file-tree diff around `--repair` shows no write outside `.unblock/.recovery/`
/// other than the repair targets, and no `.gitignore` write.
#[test]
fn repair_file_tree_diff_shows_writes_only_under_recovery_and_repair_targets() {
    let ws = Workspace::init();
    lone_shm(&ws);
    duplicate_metadata_key(&ws, "k");
    conflicted_export(&ws);
    let before = file_tree(ws.root());

    let (code, report) = doctor(&ws, true);
    assert_eq!(code, Some(0), "report: {report}");
    let after = file_tree(ws.root());

    let unblock = PathBuf::from(".unblock");
    let targets: Vec<PathBuf> = ["", "-wal", "-shm", "-journal"]
        .iter()
        .map(|suffix| unblock.join(file_name(&ws, suffix)))
        .chain([unblock.join(".write.lock"), unblock.join("issues.jsonl")])
        .collect();
    let changed: std::collections::BTreeSet<&PathBuf> = before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .collect();
    for path in changed {
        assert!(
            path.starts_with(unblock.join(".recovery")) || targets.contains(path),
            "write outside .recovery/ and the repair targets: {}",
            path.display()
        );
    }
    assert!(
        !after
            .keys()
            .any(|path| path.file_name().is_some_and(|n| n == ".gitignore")),
        "no .gitignore is written"
    );
}

/// Criterion (j): `--repair` copies a conflicted export to `export/issues.jsonl`, regenerates it from
/// the database, says rows only in the conflicted file were not imported, and the final level is
/// `Healthy`.
#[test]
fn repair_conflicted_export_regenerates_and_final_healthy() {
    let ws = Workspace::init();
    let conflicted = conflicted_export(&ws);

    let (code, report) = doctor(&ws, true);
    assert_eq!(code, Some(0), "report: {report}");
    assert_eq!(
        detail(&report, "health"),
        Some("healthy"),
        "report: {report}"
    );
    assert!(
        has_label(&report, "export_not_imported"),
        "report: {report}"
    );
    let run = only_run_dir(&ws);
    assert_eq!(
        std::fs::read(run.join("export").join("issues.jsonl")).expect("export copy"),
        conflicted
    );
    let regenerated =
        std::fs::read_to_string(ws.unblock_dir().join("issues.jsonl")).expect("regenerated");
    assert!(
        !regenerated.contains("<<<<<<<") && !regenerated.contains(">>>>>>>"),
        "the regenerated export has no markers: {regenerated}"
    );
}

/// Criterion (f), cli end to end: while an `unblock mcp` child that has served one request holds the
/// database open, `doctor --repair` refuses the pre-open window with `database_open_elsewhere`,
/// renames nothing, and exits 2. The planned action is `quarantine_journal` over an inert
/// (zero-headed, so never hot) `-journal`, because the live child keeps its own `-wal` beside the
/// database, so a lone `-shm` cannot exist while it runs.
#[test]
fn repair_refuses_database_open_elsewhere_while_an_mcp_child_is_live_exit_2() {
    let ws = Workspace::init();
    let mut client = common::McpClient::spawn(ws.root());
    client.initialize();
    let (is_error, listed) = client.call_tool("query", &serde_json::json!({"kind": "list"}));
    assert!(!is_error, "the child served one read: {listed}");

    let journal = sidecar(&ws, "-journal");
    std::fs::write(&journal, vec![0u8; 512]).expect("write an inert -journal");

    let (code, report) = doctor(&ws, true);
    assert_eq!(code, Some(2), "report: {report}");
    assert!(
        repairs(&report)
            .iter()
            .any(|r| r.starts_with("quarantine_journal")
                && r.contains("refused")
                && r.contains("database_open_elsewhere")),
        "report: {report}"
    );
    assert_eq!(
        std::fs::read(&journal).expect("the journal stays in place"),
        vec![0u8; 512]
    );
    for run in run_dirs(&ws) {
        let quarantine = run.join("quarantine");
        assert!(
            !quarantine.exists()
                || std::fs::read_dir(&quarantine)
                    .expect("read quarantine")
                    .next()
                    .is_none(),
            "nothing renamed into {}",
            quarantine.display()
        );
    }

    std::fs::remove_file(&journal).expect("remove the inert journal");
    client.close_stdin();
    let status = client.wait_for(std::time::Duration::from_secs(20));
    assert!(status.success(), "the mcp child exits cleanly: {status:?}");
}

// --- D58 helpers ------------------------------------------------------------------------------

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Run `unblock doctor [--repair] --output json` and parse its one stdout document.
fn doctor(ws: &Workspace, repair: bool) -> (Option<i32>, Value) {
    let mut cmd = ws.cmd();
    cmd.args(["doctor", "--output", "json"]);
    if repair {
        cmd.arg("--repair");
    }
    let out = output_locked(&mut cmd, "run doctor");
    let report = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "doctor prints one JSON document ({e}); stdout: {}; stderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (out.status.code(), report)
}

/// Every finding label, in order.
fn labels(report: &Value) -> Vec<String> {
    report["findings"]
        .as_array()
        .expect("findings array")
        .iter()
        .filter_map(|f| f["label"].as_str().map(str::to_string))
        .collect()
}

fn has_label(report: &Value, label: &str) -> bool {
    labels(report).iter().any(|l| l == label)
}

/// The details of every finding labelled `label`, in order.
fn details(report: &Value, label: &str) -> Vec<String> {
    report["findings"]
        .as_array()
        .expect("findings array")
        .iter()
        .filter(|f| f["label"] == label)
        .filter_map(|f| f["detail"].as_str().map(str::to_string))
        .collect()
}

/// The `repair_planned` rows (`"<action>[ <target>] (<path>)"`).
fn planned(report: &Value) -> Vec<String> {
    details(report, "repair_planned")
}

/// The `repair` rows of a `--repair` document (`"<action>[ <target>]: <outcome>"`).
fn repairs(report: &Value) -> Vec<String> {
    details(report, "repair")
}

/// The file name of the database path plus `suffix` (`""`, `-wal`, `-shm`, `-journal`).
fn file_name(ws: &Workspace, suffix: &str) -> String {
    format!(
        "{}{suffix}",
        ws.db_path()
            .file_name()
            .expect("db file name")
            .to_string_lossy()
    )
}

/// The database path plus `suffix`.
fn sidecar(ws: &Workspace, suffix: &str) -> PathBuf {
    ws.unblock_dir().join(file_name(ws, suffix))
}

/// Remove a 0-byte sidecar `init` left behind, refusing one that holds bytes.
fn remove_empty_sidecar(ws: &Workspace, suffix: &str) {
    let path = sidecar(ws, suffix);
    if let Ok(meta) = std::fs::metadata(&path) {
        assert_eq!(meta.len(), 0, "{} holds bytes", path.display());
        std::fs::remove_file(&path).expect("remove an empty sidecar");
    }
}

/// Delete the database and its WAL sidecars but keep `.write.lock`, the lost-database state
/// (PRD §4 D58 clause (7)).
fn remove_database_keep_marker(ws: &Workspace) {
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(sidecar(ws, suffix));
    }
    assert!(!ws.db_path().exists());
}

/// A `-shm` with no `-wal` beside the database (`sidecar_mismatch`). Returns its bytes.
fn lone_shm(ws: &Workspace) -> Vec<u8> {
    remove_empty_sidecar(ws, "-wal");
    let bytes = vec![0x5A; 32 * 1024];
    std::fs::write(sidecar(ws, "-shm"), &bytes).expect("write a lone -shm");
    bytes
}

/// A 5-byte `-wal` and no `-shm` (`truncated_wal`). Returns its bytes.
fn five_byte_wal(ws: &Workspace) -> Vec<u8> {
    let path = sidecar(ws, "-shm");
    if path.exists() {
        std::fs::remove_file(&path).expect("remove -shm");
    }
    let bytes = vec![1, 2, 3, 4, 5];
    std::fs::write(sidecar(ws, "-wal"), &bytes).expect("write a 5-byte -wal");
    bytes
}

/// A hot rollback `-journal` (`journal_sidecar_present`). The database switches to rollback-journal
/// mode, a write transaction captures the journal `SQLite` writes for it, the transaction rolls back,
/// and the captured bytes are written back once no connection remains. Returns the journal bytes.
fn hot_journal(ws: &Workspace) -> Vec<u8> {
    let path = sidecar(ws, "-journal");
    let bytes = runtime().block_on(async {
        let conn = common::raw_connection(&ws.db_path()).await;
        let mut rows = conn
            .query("PRAGMA journal_mode=DELETE", ())
            .await
            .expect("leave WAL mode");
        let mode: String = rows
            .next()
            .await
            .expect("row")
            .expect("present")
            .get(0)
            .expect("mode");
        assert_eq!(mode, "delete");
        drop(rows);
        // With `synchronous=OFF` the pager writes the journal magic at once. Otherwise the header
        // stays zeroed until the journal is synced at commit, and a zero first byte is not hot.
        conn.execute("PRAGMA synchronous=OFF", ())
            .await
            .expect("synchronous off");
        conn.execute("BEGIN IMMEDIATE", ()).await.expect("begin");
        conn.execute(
            "INSERT INTO metadata (key, value) VALUES ('hot', 'journal')",
            (),
        )
        .await
        .expect("write a page");
        let bytes = std::fs::read(&path).expect("the live journal");
        conn.execute("ROLLBACK", ()).await.expect("rollback");
        bytes
    });
    assert!(
        bytes.first().is_some_and(|b| *b != 0),
        "the captured journal carries a header"
    );
    std::fs::write(&path, &bytes).expect("write the hot journal back");
    bytes
}

/// A database file that is not `SQLite` (`database_not_sqlite`), with no sidecar. Returns its bytes.
fn not_sqlite(ws: &Workspace) -> Vec<u8> {
    remove_empty_sidecar(ws, "-wal");
    remove_empty_sidecar(ws, "-shm");
    let bytes = b"this file is not a SQLite database".to_vec();
    std::fs::write(ws.db_path(), &bytes).expect("overwrite the database");
    bytes
}

/// Two `metadata` rows sharing `key` (`duplicate_metadata_key`).
fn duplicate_metadata_key(ws: &Workspace, key: &str) {
    runtime().block_on(async {
        let conn = common::raw_connection(&ws.db_path()).await;
        for value in ["v1", "v2"] {
            conn.execute(
                "INSERT INTO metadata (key, value) VALUES (?1, ?2)",
                libsql::params![key, value],
            )
            .await
            .expect("insert a metadata row");
        }
    });
}

/// An `issues.jsonl` export carrying git conflict markers. Returns its bytes.
fn conflicted_export(ws: &Workspace) -> Vec<u8> {
    let bytes =
        b"<<<<<<< HEAD\n{\"id\":\"ub-1\"}\n=======\n{\"id\":\"ub-2\"}\n>>>>>>> branch\n".to_vec();
    std::fs::write(ws.unblock_dir().join("issues.jsonl"), &bytes).expect("write the export");
    bytes
}

/// Remove one table row's entry from the b-tree of `index`. The index's stored SQL becomes a
/// `WHERE 0` partial index, a new `metadata` row is inserted (so the index skips it), and the
/// original SQL is restored. Asserts that `integrity_check` now reports the missing entry.
fn drop_an_index_entry(ws: &Workspace, index: &str) {
    let db = ws.db_path();
    let original: String = runtime().block_on(async {
        let conn = common::raw_connection(&db).await;
        let mut rows = conn
            .query(
                "SELECT sql FROM sqlite_schema WHERE type = 'index' AND name = ?1",
                libsql::params![index],
            )
            .await
            .expect("read the index sql");
        rows.next()
            .await
            .expect("row")
            .expect("the index exists")
            .get(0)
            .expect("sql")
    });
    rewrite_index_sql(&db, index, &format!("{original} WHERE 0"));
    runtime().block_on(async {
        let conn = common::raw_connection(&db).await;
        conn.execute(
            "INSERT INTO metadata (key, value) VALUES ('unindexed', 'row')",
            (),
        )
        .await
        .expect("insert a row the partial index skips");
    });
    rewrite_index_sql(&db, index, &original);
    let rows = integrity_rows(ws);
    assert!(
        rows.iter()
            .any(|row| row.contains(&format!("missing from index {index}"))),
        "the fixture removed an index entry: {rows:?}"
    );
}

/// Overwrite the stored SQL of `index` in `sqlite_schema` (`writable_schema`).
fn rewrite_index_sql(db: &Path, index: &str, sql: &str) {
    runtime().block_on(async {
        let conn = common::raw_connection(db).await;
        conn.execute("PRAGMA writable_schema=ON", ())
            .await
            .expect("writable_schema");
        conn.execute(
            "UPDATE sqlite_schema SET sql = ?1 WHERE type = 'index' AND name = ?2",
            libsql::params![sql, index],
        )
        .await
        .expect("rewrite the index sql");
        conn.execute("PRAGMA writable_schema=OFF", ())
            .await
            .expect("writable_schema off");
    });
}

/// The rows of `PRAGMA integrity_check`, with `ok` normalized to no rows.
fn integrity_rows(ws: &Workspace) -> Vec<String> {
    runtime().block_on(async {
        let conn = common::raw_connection(&ws.db_path()).await;
        let mut rows = conn
            .query("PRAGMA integrity_check", ())
            .await
            .expect("integrity_check");
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.expect("row") {
            let text: String = row.get(0).expect("text");
            if text != "ok" {
                out.push(text);
            }
        }
        out
    })
}

/// Whether the database has a table named `name`.
fn has_table(ws: &Workspace, name: &str) -> bool {
    runtime().block_on(async {
        let conn = common::raw_connection(&ws.db_path()).await;
        let mut rows = conn
            .query(
                "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1",
                libsql::params![name],
            )
            .await
            .expect("read sqlite_schema");
        rows.next().await.expect("row").is_some()
    })
}

/// Create `n` children of `parent` through the engine, which mints their `parent.N` ids.
fn create_children(ws: &Workspace, parent: &str, n: usize) -> Vec<String> {
    use unblock_engine::NewIssue;

    runtime().block_on(async {
        let session = open_session(ws).await;
        let mut ids = Vec::new();
        for k in 0..n {
            let issue = session
                .create_issue(NewIssue {
                    title: format!("child {k} of {parent}"),
                    parent: Some(parent.to_string()),
                    ..NewIssue::default()
                })
                .await
                .expect("create a child");
            ids.push(issue.id);
        }
        drop(session);
        ids
    })
}

/// The `child_counters.last_child` of `parent`.
fn child_counter(ws: &Workspace, parent: &str) -> i64 {
    runtime().block_on(async {
        let conn = common::raw_connection(&ws.db_path()).await;
        let mut rows = conn
            .query(
                "SELECT last_child FROM child_counters WHERE parent_id = ?1",
                libsql::params![parent],
            )
            .await
            .expect("read the counter");
        rows.next()
            .await
            .expect("row")
            .expect("the counter exists")
            .get(0)
            .expect("last_child")
    })
}

/// Lower (or set) the `child_counters.last_child` of `parent` directly.
fn set_child_counter(ws: &Workspace, parent: &str, last_child: i64) {
    runtime().block_on(async {
        let conn = common::raw_connection(&ws.db_path()).await;
        conn.execute(
            "UPDATE child_counters SET last_child = ?2 WHERE parent_id = ?1",
            libsql::params![parent, last_child],
        )
        .await
        .expect("set the counter");
    });
}

/// The run directories under `.unblock/.recovery/`, sorted.
fn run_dirs(ws: &Workspace) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(ws.unblock_dir().join(".recovery")) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .map(|entry| entry.expect("dir entry").path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs
}

/// The one run directory a single-phase `--repair` created.
fn only_run_dir(ws: &Workspace) -> PathBuf {
    let dirs = run_dirs(ws);
    assert_eq!(dirs.len(), 1, "exactly one run directory: {dirs:?}");
    dirs.into_iter().next().expect("one run directory")
}

/// The parsed `manifest.json` of `run`.
fn manifest(run: &Path) -> Value {
    let text = std::fs::read(run.join("manifest.json")).expect("read manifest.json");
    serde_json::from_slice(&text).expect("manifest.json is JSON")
}

/// The `sha256` the manifest records for the run-relative `path`.
fn manifest_sha(manifest: &Value, path: &str) -> String {
    manifest["files"]
        .as_array()
        .expect("files array")
        .iter()
        .find(|f| f["path"] == path)
        .and_then(|f| f["sha256"].as_str())
        .unwrap_or_else(|| panic!("the manifest lists {path}: {manifest}"))
        .to_string()
}

/// The `originals` entry the manifest records for the run-relative `path` (ruling 390).
fn manifest_original<'a>(manifest: &'a Value, path: &str) -> &'a Value {
    manifest["originals"]
        .as_array()
        .expect("originals array")
        .iter()
        .find(|f| f["path"] == path)
        .unwrap_or_else(|| panic!("the manifest's originals list {path}: {manifest}"))
}

/// Lowercase hex SHA-256 of `bytes`.
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    use std::fmt::Write as _;

    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// The names of the regular files directly under `dir`.
fn top_level_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("read dir")
        .map(|entry| entry.expect("dir entry").path())
        .filter(|path| path.is_file())
        .map(|path| {
            path.file_name()
                .expect("file name")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

/// One node of a [`file_tree`].
#[derive(Debug, PartialEq, Eq)]
enum Node {
    Dir,
    File(Vec<u8>),
    Unreadable,
}

/// Every path under `root` (relative to it) with its contents.
fn file_tree(root: &Path) -> BTreeMap<PathBuf, Node> {
    fn walk(root: &Path, dir: &Path, tree: &mut BTreeMap<PathBuf, Node>) {
        for entry in std::fs::read_dir(dir).expect("read dir") {
            let path = entry.expect("dir entry").path();
            let rel = path.strip_prefix(root).expect("under root").to_path_buf();
            if path.is_dir() {
                tree.insert(rel, Node::Dir);
                walk(root, &path, tree);
            } else {
                let node = std::fs::read(&path).map_or(Node::Unreadable, Node::File);
                tree.insert(rel, node);
            }
        }
    }
    let mut tree = BTreeMap::new();
    walk(root, root, &mut tree);
    tree
}

// ------------------------------------------------------------------------------------------------
// PRD §4 D58 amendment (`ub-c15`): an export that IS the database is skipped, reported, never
// regenerated
// ------------------------------------------------------------------------------------------------

/// The two shapes of the export resolving to the database: `db_filename = "issues.jsonl"` (the
/// data-loss shape: the default export path IS the database) and, on unix, a hard link
/// `issues.jsonl` → `unblock.db`. Each is seeded with an issue whose description holds a
/// `<<<<<<< HEAD` line, and the database FILE bytes are asserted to carry `\n<<<<<<< ` (so a scan
/// that read the database would report `jsonl_conflict_markers` and `--repair` would regenerate
/// over it). Returns `(shape, workspace, database path)`.
fn export_is_the_database_shapes() -> Vec<(&'static str, Workspace, PathBuf)> {
    let mut shapes = Vec::new();

    let ws = Workspace::init();
    std::fs::rename(ws.db_path(), ws.unblock_dir().join("issues.jsonl")).expect("rename the db");
    let config = std::fs::read_to_string(ws.config_path()).expect("read config.toml");
    std::fs::write(
        ws.config_path(),
        format!("db_filename = \"issues.jsonl\"\n{config}"),
    )
    .expect("write config.toml");
    let db = ws.unblock_dir().join("issues.jsonl");
    shapes.push(("same-name", ws, db));

    #[cfg(unix)]
    {
        let ws = Workspace::init();
        let db = ws.db_path();
        std::fs::hard_link(&db, ws.unblock_dir().join("issues.jsonl")).expect("hard link");
        shapes.push(("hard-link", ws, db));
    }

    for (shape, ws, db) in &shapes {
        seed_marker_issue(ws);
        let bytes = std::fs::read(db).expect("read the database file");
        assert!(
            bytes.windows(9).any(|w| w == b"\n<<<<<<< "),
            "{shape}: precondition: the database file carries a conflict-marker line"
        );
    }
    shapes
}

/// Create one issue whose description holds a `<<<<<<< HEAD` line, through the real engine.
fn seed_marker_issue(ws: &Workspace) {
    runtime().block_on(async {
        let session = open_session(ws).await;
        session
            .create_issue(unblock_engine::NewIssue {
                title: "carries a marker line".to_string(),
                description: Some("before\n<<<<<<< HEAD\nafter".to_string()),
                ..unblock_engine::NewIssue::default()
            })
            .await
            .expect("create the marker issue");
        drop(session);
    });
}

/// The database's SHA-256 and (unix) inode.
fn db_fingerprint(db: &Path) -> (String, Option<u64>) {
    #[cfg(unix)]
    let ino = {
        use std::os::unix::fs::MetadataExt as _;
        Some(std::fs::metadata(db).expect("stat the database").ino())
    };
    #[cfg(not(unix))]
    let ino = None;
    (
        sha256_hex(&std::fs::read(db).expect("read the database")),
        ino,
    )
}

/// (C0) Plain `doctor` with the export resolving to the database exits by the level (`Healthy`, 0:
/// the skip is an advisory), reports `export_is_session_file` and no scan finding, and writes
/// nothing: the database's SHA-256 and inode are unchanged and no `.unblock/.recovery/` exists.
#[test]
fn c15e0_doctor_reports_an_export_that_is_the_database_and_writes_nothing() {
    for (shape, ws, db) in export_is_the_database_shapes() {
        let before = db_fingerprint(&db);
        let (code, report) = doctor(&ws, false);
        assert_eq!(code, Some(0), "{shape}: report: {report}");
        assert_eq!(
            detail(&report, "health"),
            Some("healthy"),
            "{shape}: report: {report}"
        );
        assert!(
            has_label(&report, "export_is_session_file"),
            "{shape}: report: {report}"
        );
        assert!(
            !has_label(&report, "jsonl_conflict_markers")
                && !has_label(&report, "jsonl_unscannable"),
            "{shape}: the database was scanned as the export; report: {report}"
        );
        assert_eq!(
            db_fingerprint(&db),
            before,
            "{shape}: doctor changed the database"
        );
        assert!(
            !ws.unblock_dir().join(".recovery").exists(),
            "{shape}: doctor created a .recovery/ entry"
        );
    }
}

/// (C1) `doctor --repair` with the export resolving to the database regenerates nothing. In
/// document order: (1) the `recover` / `nothing to repair online` row; (2) `export_is_session_file`
/// exactly once, after it; (3) no `repair_planned`, `regenerate_export` or `evidence` row; (4) exit
/// 0; (5) no `.unblock/.recovery/`; (6) the database's SHA-256 and inode unchanged.
#[test]
fn c15e1_repair_with_an_export_that_is_the_database_regenerates_nothing() {
    for (shape, ws, db) in export_is_the_database_shapes() {
        let before = db_fingerprint(&db);
        let (code, report) = doctor(&ws, true);
        let labels = labels(&report);
        let recover = labels
            .iter()
            .position(|l| l == "recover")
            .unwrap_or_else(|| panic!("{shape}: no recover row; report: {report}"));
        assert_eq!(
            details(&report, "recover"),
            vec!["nothing to repair online".to_string()],
            "{shape}: report: {report}"
        );
        let skips: Vec<usize> = labels
            .iter()
            .enumerate()
            .filter(|(_, l)| *l == "export_is_session_file")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(skips.len(), 1, "{shape}: report: {report}");
        assert!(skips[0] > recover, "{shape}: report: {report}");
        assert!(
            !labels
                .iter()
                .any(|l| l == "repair_planned" || l == "regenerate_export" || l == "evidence"),
            "{shape}: report: {report}"
        );
        assert!(
            !report.to_string().contains("regenerate_export"),
            "{shape}: report: {report}"
        );
        assert_eq!(code, Some(0), "{shape}: report: {report}");
        assert!(
            !ws.unblock_dir().join(".recovery").exists(),
            "{shape}: --repair created a .recovery/ entry"
        );
        assert_eq!(
            db_fingerprint(&db),
            before,
            "{shape}: --repair changed or replaced the database"
        );
    }
}
