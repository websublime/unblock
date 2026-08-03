//! `unblock migrate` (D27/AF-2) + `unblock doctor` (HEALTH-LITE, D29/F4) end-to-end.
//!
//! - migrate (D46 clause (10)): the command reports the stamp observed BEFORE its own open, so all
//!   THREE cases are pinned here and get the SAME treatment. An `unblock init`-built workspace is
//!   ALREADY-CURRENT (`init` opened through the same facade, which migrated), so it reports
//!   `schema_from == schema_to == 2`, `applied == false` — `applied: false` means "the stamp did not
//!   move across THIS run's own open", NOT "nothing was wrong". A NEVER-MIGRATED workspace
//!   (`.unblock/config.toml`, no `unblock.db`) reports `0` -> `2`, `applied: true`. A STALE workspace
//!   (five-column `comments`, stamped `1`) reports `1` -> `2`, `applied: true`. Report shape pinned.
//! - doctor (T3.3): the cli ROUTES through the wired `Session::doctor()` — a clean DB → `health:
//!   healthy` + `integrity: ok` (no file-state anomalies), exit 0. A corrupted DB → exit 2 (db bucket)
//!   — the corruption is surfaced as a `DATABASE_ERROR` structured error (the deterministic corruption
//!   a page-overwrite yields; the non-empty-`integrity_check` → exit-2 mapping is unit-tested in
//!   `commands/doctor.rs::database_error_exit_is_two`).

mod common;

use common::{Workspace, detail, json_report};
use serde_json::Value;

/// An `init`-built workspace is ALREADY CURRENT — `init` opened it through the same config facade,
/// which migrated it — so the stamp observed BEFORE this run's own open is already `2` and nothing
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
        Some("2"),
        "the stamp observed BEFORE this run's own open — already current"
    );
    assert_eq!(
        detail(&first, "schema_to"),
        Some("2"),
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
    assert_eq!(detail(&second, "schema_from"), Some("2"));
    assert_eq!(detail(&second, "schema_to"), Some("2"));
    assert_eq!(detail(&second, "applied"), Some("false"));
}

/// Run `unblock migrate --output json` and return the report with the volatile `database` path
/// redacted to a stable token, so a snapshot pins the SHAPE (kind + finding labels + schema/applied
/// values) rather than a tempdir path.
fn migrate_report_json(ws: &Workspace) -> Value {
    let out = ws
        .cmd()
        .args(["migrate", "--output", "json"])
        .output()
        .expect("run migrate");
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
/// database now reports `0` -> `2` `applied: true`, pinned by the cell below).
#[test]
fn migrate_report_shape_on_an_already_current_workspace_is_snapshot_pinned() {
    let ws = Workspace::init();
    insta::assert_json_snapshot!("migrate_report_already_current", migrate_report_json(&ws));
}

/// **D46 clause (10) — the NEVER-MIGRATED direction, which this command could not show AT ALL before.**
///
/// The fixture needs no libsql: a `.unblock/` carrying `config.toml` and NO `unblock.db`, so the
/// facade's own `open_local` creates the file at stamp `0`, the ladder then runs (a fresh database
/// reaches the current shape THROUGH step 2 under the frozen-baseline discipline), and the command
/// reports the pre-open stamp it captured.
///
/// MUTANT KILLED: sourcing `schema_from` from `MigrateOutcome.from` again — which is exactly the
/// pre-ruling code. `Session::migrate` runs AFTER the facade migrated, so it would observe `2` and
/// this cell would read `2` -> `2` `applied: false`.
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
    assert_eq!(detail(&report, "schema_to"), Some("2"));
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
/// read. `unblock migrate` repairs it and reports `1` -> `2` `applied: true` (exit 0), the
/// pre-existing comment row survives with both new columns NULL, and the same workspace then serves
/// the hydrated read that failed before.
///
/// MUTANT KILLED: emptying the ladder (`MIGRATIONS: &[]`) — the stale database stays five-column, the
/// hydrated read keeps failing and `schema_to` never reaches `2`.
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
    assert_eq!(detail(&report, "schema_to"), Some("2"));
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

#[test]
fn migrate_on_a_future_schema_db_exits_2_with_schema_mismatch() {
    // D27/AF-2 newer-DB path, proven END-TO-END at the CLI boundary. The rejection is proven at the
    // storage layer (`libsql::migrate_rejects_future_version`), but NOT via the cli — this closes that
    // gap. Stamp the workspace DB to a FUTURE `PRAGMA user_version` (99), then run `unblock migrate`:
    // the config facade migrates on open, `migrate()` finds found=99 > expected → `SchemaMismatch`,
    // which surfaces transparently as exit 2 + a `SCHEMA_MISMATCH` structured error on stdout (FR-11).
    let ws = Workspace::init();
    stamp_user_version(&ws.db_path(), 99);

    let out = ws
        .cmd()
        .args(["migrate", "--output", "json"])
        .output()
        .expect("run migrate on a future-schema db");
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
#[test]
fn a_lying_stamp_exits_2_with_a_hint_that_names_the_repair_and_the_missing_columns() {
    let ws = Workspace::init();
    // Current stamp (2) + stale shape (5 columns) — the state the stamp alone cannot detect.
    drop_comment_columns(&ws);
    assert_eq!(
        stamped_user_version(&ws),
        2,
        "the stamp still claims current"
    );

    let out = ws
        .cmd()
        .args(["migrate", "--output", "json"])
        .output()
        .expect("run migrate on a lying-stamp db");
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
        hint.contains("unblock migrate"),
        "the hint must name the ONE command that repairs it: {hint}"
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

/// Delete the workspace database (and its WAL sidecars), leaving `.unblock/config.toml` behind — the
/// NEVER-MIGRATED fixture. The facade's own `open_local` recreates the file at stamp `0`.
fn remove_database(ws: &Workspace) {
    let db = ws.db_path();
    for suffix in ["", "-wal", "-shm"] {
        let mut path = db.clone().into_os_string();
        path.push(suffix);
        let _ = std::fs::remove_file(std::path::PathBuf::from(path));
    }
    assert!(!db.exists(), "the never-migrated fixture has no unblock.db");
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
        let database = libsql::Builder::new_local(ws.db_path())
            .build()
            .await
            .expect("open the workspace db");
        let conn = database.connect().expect("connect");
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
        let database = libsql::Builder::new_local(ws.db_path())
            .build()
            .await
            .expect("open the workspace db");
        let conn = database.connect().expect("connect");
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
        let database = libsql::Builder::new_local(ws.db_path())
            .build()
            .await
            .expect("open the workspace db");
        let conn = database.connect().expect("connect");
        let mut rows = conn.query("PRAGMA user_version", ()).await.expect("uv");
        let row = rows.next().await.expect("row").expect("present");
        row.get_value(0)
            .expect("value")
            .as_integer()
            .copied()
            .expect("integer")
    })
}

/// A tiny current-thread runtime for the raw-libsql fixtures (the harness itself is sync).
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build a current-thread runtime")
}

/// Stamp `PRAGMA user_version = <version>` on the workspace DB via a raw libsql open (the same bundled
/// `SQLite` the backend uses). This makes the on-disk schema look NEWER than this build so the next
/// migrate rejects it with `SchemaMismatch` (D27/AF-2). The connection is dropped before the CLI child
/// opens the file, so there is no writer contention.
fn stamp_user_version(db: &std::path::Path, version: i64) {
    // A tiny current-thread runtime just to drive the async libsql open/exec (the harness is sync).
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build a current-thread runtime");
    rt.block_on(async {
        let database = libsql::Builder::new_local(db)
            .build()
            .await
            .expect("open the workspace db");
        let conn = database.connect().expect("connect");
        conn.execute(&format!("PRAGMA user_version = {version}"), ())
            .await
            .expect("stamp user_version");
    });
}

#[test]
fn doctor_healthy_routes_through_session_doctor_and_exits_0() {
    // T3.3 (D29/F4): the cli routes through the wired `Session::doctor()`. On a clean workspace the
    // health report is `health: healthy` + `integrity: ok` with NO file-state anomalies, exit 0
    // (`json_report` asserts success). The T3.1 Stats/Lint/Info composition is SUPERSEDED for the
    // doctor output.
    let ws = Workspace::init();
    let report = json_report(&ws, &["doctor", "--output", "json"]);
    assert_eq!(
        report["kind"], "info",
        "doctor report reuses DiagnosticKind::Info (F2 — no new model variant)"
    );
    assert_eq!(
        detail(&report, "health"),
        Some("healthy"),
        "a clean workspace is healthy (the live 0-byte WAL is not a truncation, D29 refinement); report: {report}"
    );
    assert_eq!(
        detail(&report, "integrity"),
        Some("ok"),
        "clean DB integrity"
    );
    let labels: Vec<&str> = report["findings"]
        .as_array()
        .expect("findings array")
        .iter()
        .map(|f| f["label"].as_str().unwrap_or(""))
        .collect();
    assert!(
        !labels.contains(&"integrity_problem"),
        "a clean DB has no integrity problems"
    );
    // The T3.1 diagnostics composition (stats./info. prefixes) is SUPERSEDED at T3.3 — the doctor
    // output is the health-lite report (integrity + file-state), NOT Stats/Lint/Info.
    assert!(
        !labels
            .iter()
            .any(|l| l.starts_with("stats.") || l.starts_with("info.")),
        "no diagnostics-composition sections at T3.3; labels: {labels:?}"
    );
}

#[test]
fn doctor_with_advisory_jsonl_conflict_renders_the_anomaly_yet_exits_0() {
    // SF3 — the headline F4 boundary: an ADVISORY file-state anomaly (a merge conflict left in the
    // JSONL export → health=unsafe) with CLEAN integrity is RENDERED in the report AND exits 0.
    // Advisory findings NEVER flip the exit; exit 2 is corruption-only (D27/AF-1, D29/F4).
    let ws = Workspace::init();
    // The engine inspects `<.unblock>/issues.jsonl` (config's default jsonl path). `init` seeds no
    // jsonl (AF-3), so materialize one carrying git conflict markers.
    let jsonl = ws.unblock_dir().join("issues.jsonl");
    std::fs::write(
        &jsonl,
        "<<<<<<< HEAD\n{\"id\":\"ub-1\"}\n=======\n{\"id\":\"ub-2\"}\n>>>>>>> branch\n",
    )
    .expect("write a conflicted jsonl export");

    // `json_report` asserts exit 0 — so a passing call already proves the advisory anomaly did NOT
    // flip the exit despite health=unsafe.
    let report = json_report(&ws, &["doctor", "--output", "json"]);
    assert_eq!(report["kind"], "info");
    assert_eq!(
        detail(&report, "integrity"),
        Some("ok"),
        "integrity is clean — only the JSONL is conflicted"
    );
    assert_eq!(
        detail(&report, "health"),
        Some("unsafe"),
        "a JSONL merge conflict is Unsafe (advisory), yet the exit stays 0; report: {report}"
    );
    assert_eq!(
        detail(&report, "jsonl_conflict_markers"),
        Some("JSONL contains merge conflict markers"),
        "the advisory anomaly must be surfaced as a rendered finding row"
    );
}

// SF4/SF5 (v1.1 follow-up): an e2e fixture driving a readable-but-integrity-DIRTY libsql DB to exit 2
// (and `Session::doctor()` over genuinely corrupt integrity rows) is impractical to synthesize
// reliably; `doctor_exit(&[String])` non-empty→exit-2 is unit + mutation-proven in `commands/doctor.rs`
// and the corrupt-DB error path is covered by `doctor_on_a_corrupt_db_exits_2` below (see the health
// crate plan's "deferred should-fixes" note).
#[test]
fn doctor_on_a_corrupt_db_exits_2() {
    // Corrupt the DB deterministically (overwrite a large b-tree region past the header page) so the
    // read fails as a malformed-image DatabaseError → exit 2 (the db bucket). This proves `doctor`
    // yields a non-zero db-bucket exit on a corrupt workspace (AF-1: non-zero only on corruption).
    let ws = Workspace::init();
    corrupt_db(&ws.db_path());
    let out = ws
        .cmd()
        .args(["doctor", "--output", "json"])
        .output()
        .expect("run doctor on corrupt db");
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
        detail(&report, "integrity"),
        Some("ok"),
        "a dangling edge is a DATA fact, not corruption — integrity is still clean; report: {report}"
    );
}

/// **D46 — `doctor` reports BOTH schema numbers, and its exit rule is byte-identical.**
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
/// NO "flips the exit on a stamp mismatch" mutant is claimed, and the reason is stated so it is not
/// re-added as an oversight: a stamp mismatch is UNREACHABLE from this command by construction (the
/// facade migrates on open, and a stamp ABOVE the build's is refused at open before `doctor` runs at
/// all). A mutant that cannot fire proves nothing; the exit rule's protection stays where it already
/// is — `doctor_exit`'s own mutation pin.
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
    use unblock_config::{CliOverrides, open_with_storage_with_cli};
    use unblock_engine::{Session, SessionConfig};
    use unblock_model::{Issue, Status};

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build a current-thread runtime");
    rt.block_on(async {
        let overrides = CliOverrides::new().with_dir(ws.unblock_dir());
        let ctx = open_with_storage_with_cli(&overrides)
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

/// Open a `Session` over `ws` through the SAME config facade the CLI dispatches through. The caller
/// drops it before spawning any CLI child, so no connection outlives the call.
async fn open_session(ws: &Workspace) -> unblock_engine::Session {
    use unblock_config::{CliOverrides, open_with_storage_with_cli};
    use unblock_engine::{Session, SessionConfig};

    let overrides = CliOverrides::new().with_dir(ws.unblock_dir());
    let ctx = open_with_storage_with_cli(&overrides)
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
        let database = libsql::Builder::new_local(ws.db_path())
            .build()
            .await
            .expect("open the workspace db");
        let conn = database.connect().expect("connect");
        conn.execute(
            "INSERT INTO dependencies (issue_id, depends_on_id, type, created_at, created_by) \
             VALUES (?1, ?2, ?3, '2026-08-01T00:00:00Z', 'planted')",
            libsql::params![source, target, dep_type],
        )
        .await
        .expect("plant the dangling edge");
    });
}
