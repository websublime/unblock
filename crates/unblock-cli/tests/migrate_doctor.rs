//! `unblock migrate` (D27/AF-2) + `unblock doctor` (HEALTH-LITE, D29/F4) end-to-end.
//!
//! - migrate: a fresh workspace is already migrated on open, so the FIRST `migrate` reports the real
//!   schema (`schema_from == schema_to == 1`, `applied == false` — the honest idempotent signal, not
//!   a phantom applied-list), and a SECOND run reports identically (idempotent). Report shape pinned.
//! - doctor (T3.3): the cli ROUTES through the wired `Session::doctor()` — a clean DB → `health:
//!   healthy` + `integrity: ok` (no file-state anomalies), exit 0. A corrupted DB → exit 2 (db bucket)
//!   — the corruption is surfaced as a `DATABASE_ERROR` structured error (the deterministic corruption
//!   a page-overwrite yields; the non-empty-`integrity_check` → exit-2 mapping is unit-tested in
//!   `commands/doctor.rs::database_error_exit_is_two`).

mod common;

use common::{Workspace, detail, json_report};
use serde_json::Value;

#[test]
fn migrate_fresh_reports_current_schema_and_is_idempotent() {
    let ws = Workspace::init();

    // First migrate: the config facade already migrated on open, so this reports the current schema
    // with `applied == false` (honest — nothing was advanced by THIS call).
    let first = json_report(&ws, &["migrate", "--output", "json"]);
    assert_eq!(
        first["kind"], "info",
        "migrate report reuses DiagnosticKind::Info"
    );
    assert_eq!(
        detail(&first, "schema_from"),
        Some("1"),
        "on-disk schema before"
    );
    assert_eq!(
        detail(&first, "schema_to"),
        Some("1"),
        "on-disk schema after"
    );
    assert_eq!(
        detail(&first, "applied"),
        Some("false"),
        "no advance post-open"
    );
    // The database finding names the workspace DB (a path — assert it ends with the db filename).
    assert!(
        detail(&first, "database").is_some_and(|d| d.ends_with("unblock.db")),
        "migrate report names the workspace db"
    );

    // Second migrate: idempotent — identical from/to/applied.
    let second = json_report(&ws, &["migrate", "--output", "json"]);
    assert_eq!(detail(&second, "schema_from"), Some("1"));
    assert_eq!(detail(&second, "schema_to"), Some("1"));
    assert_eq!(detail(&second, "applied"), Some("false"));
}

#[test]
fn migrate_report_shape_is_snapshot_pinned() {
    let ws = Workspace::init();
    let out = ws
        .cmd()
        .args(["migrate", "--output", "json"])
        .output()
        .expect("run migrate");
    assert_eq!(out.status.code(), Some(0));
    let mut report: Value = serde_json::from_slice(&out.stdout).expect("valid JSON migrate report");
    // The `database` detail is an absolute tempdir path — redact it to a stable token so the snapshot
    // pins the SHAPE (kind + finding labels + schema/applied values) without the volatile path.
    if let Some(findings) = report["findings"].as_array_mut() {
        for f in findings {
            if f["label"] == "database" {
                f["detail"] = Value::String("<db-path>".to_string());
            }
        }
    }
    insta::assert_json_snapshot!("migrate_report_fresh", report);
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
