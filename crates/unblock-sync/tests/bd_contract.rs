//! bd one-shot import contract (FR-26/D16/D24) against REAL `unblock-storage` libsql.
//!
//! Drives `import_bd` over the synthesized byte-faithful `fixtures/bd_export.jsonl` (D24/F3 — NO
//! dev-dep on `temp/beads_rust-main`) and asserts:
//!
//! - **per-repair POST-import field values** (non-vacuity, SF-1): each assertion FAILS under a
//!   single-repair mutation — idempotency alone does NOT satisfy the gate (`content_hash` is
//!   `#[serde(skip)]`, so a repair skipped-in-both runs is still idempotent);
//! - the extended `ImportReport` counts (`imported`/`dependencies`/`comments`) + the `dropped_fields`
//!   list (unknown top-level keys, D24/F4);
//! - **idempotent rerun** (2nd import → `imported == 0`);
//! - **bd ids preserved verbatim** (the `bd-` prefix survives — no remap);
//! - FR-8 on the bd path: a conflict-marker file is rejected with ZERO writes; a `..`-traversal path
//!   is refused at preflight.

use std::io::Write;
use std::path::Path;

use unblock_model::{DependencyType, ListFilters, Status};
use unblock_storage::{LibsqlStorage, Storage};

use unblock_sync::import_bd;

async fn fresh_storage() -> LibsqlStorage {
    let storage = LibsqlStorage::open_in_memory().await.expect("open");
    storage.migrate().await.expect("migrate");
    storage
}

/// Copy the packaged fixture into a confined `.unblock/` dir and return `(tempdir, confine_root,
/// staged_path)`.
fn stage_fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join(".unblock");
    std::fs::create_dir_all(&dir).unwrap();
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/bd_export.jsonl"
    );
    let bytes = std::fs::read(fixture).expect("read fixture");
    let staged = dir.join("issues.jsonl");
    std::fs::write(&staged, &bytes).unwrap();
    (tmp, dir, staged)
}

async fn count_rows(storage: &LibsqlStorage) -> usize {
    storage
        .list_issues(&ListFilters {
            include_closed: true,
            include_deferred: true,
            include_tombstone: true,
            ..ListFilters::default()
        })
        .await
        .expect("list")
        .len()
}

#[tokio::test]
async fn bd_import_applies_every_repair_and_reports_counts() {
    let (_tmp, dir, staged) = stage_fixture();
    let storage = fresh_storage().await;

    let report = import_bd(&storage, &staged, &dir, "importer")
        .await
        .expect("bd import");

    // ---- counts ----
    assert_eq!(report.imported, 5, "5 issues migrated");
    assert_eq!(report.skipped, 0);
    // POST-dedup deps on bd-1: parent-child + waits-for + blocks(kept-latest of 2) = 3.
    assert_eq!(report.dependencies, 3, "deps counted POST-dedup");
    assert_eq!(report.comments, 2, "two comments on bd-1");
    // dropped_fields = unknown TOP-LEVEL keys, deduped, first-seen order.
    assert_eq!(
        report.dropped_fields,
        vec!["legacy_field".to_string(), "deprecated_col".to_string()],
    );
    assert_eq!(count_rows(&storage).await, 5);

    // ---- repair 3 + 5: status `done` → Closed AND closed_at set to updated_at ----
    let bd1 = storage.get_issue("bd-1").await.unwrap().unwrap();
    assert_eq!(bd1.status, Status::Closed, "terminal-status alias → Closed");
    assert_eq!(
        bd1.closed_at,
        Some(bd1.updated_at),
        "terminal + closed_at:None → updated_at (repair runs AFTER the alias, SF-2)"
    );

    // ---- repair 1: general underscore dep-types adopt their kebab variants ----
    let dep_type_for = |target: &str| -> DependencyType {
        bd1.dependencies
            .iter()
            .find(|d| d.depends_on_id == target)
            .unwrap_or_else(|| panic!("dep to {target} present"))
            .dep_type
            .clone()
    };
    assert_eq!(
        dep_type_for("bd-2"),
        DependencyType::ParentChild,
        "parent_child → parent-child"
    );
    assert_eq!(
        dep_type_for("bd-3"),
        DependencyType::WaitsFor,
        "waits_for → waits-for (general rule, not just parent_child)"
    );
    assert_eq!(dep_type_for("bd-4"), DependencyType::Blocks);

    // ---- repair 2: dup (bd-1 → bd-4 blocks) deduped to the LATEST created_at ----
    assert_eq!(
        bd1.dependencies
            .iter()
            .filter(|d| d.depends_on_id == "bd-4")
            .count(),
        1,
        "duplicate (issue_id, depends_on_id, dep_type) collapsed to 1 (N-1)"
    );

    // ---- repair 6: non-terminal bd-2 has its stale closed_at cleared ----
    let bd2 = storage.get_issue("bd-2").await.unwrap().unwrap();
    assert_eq!(bd2.status, Status::Open);
    assert_eq!(bd2.closed_at, None, "non-terminal → closed_at cleared");

    // ---- repair 7a: blank external_ref → None ----
    let bd3 = storage.get_issue("bd-3").await.unwrap().unwrap();
    assert_eq!(bd3.external_ref, None, "blank external_ref → None");

    // ---- repair 7b (SF-3): whitespace-padded external_ref → trimmed ----
    let bd4 = storage.get_issue("bd-4").await.unwrap().unwrap();
    assert_eq!(
        bd4.external_ref.as_deref(),
        Some("GH-42"),
        "padded external_ref → trimmed (the trim sub-branch is non-vacuous)"
    );

    // ---- repair 4: `-wisp-` id → ephemeral ----
    let wisp = storage.get_issue("bd-wisp-x1").await.unwrap().unwrap();
    assert!(wisp.ephemeral, "wisp id → ephemeral = true");

    // ---- bd ids preserved verbatim (no remap) ----
    for id in ["bd-1", "bd-2", "bd-3", "bd-4", "bd-wisp-x1"] {
        assert!(
            storage.get_issue(id).await.unwrap().is_some(),
            "id {id} preserved verbatim"
        );
    }
}

#[tokio::test]
async fn bd_import_rerun_is_idempotent() {
    let (_tmp, dir, staged) = stage_fixture();
    let storage = fresh_storage().await;

    let first = import_bd(&storage, &staged, &dir, "importer")
        .await
        .expect("first import");
    assert_eq!(first.imported, 5);

    // 2nd import into the SAME DB → every record already present → imported == 0 (all skipped).
    let second = import_bd(&storage, &staged, &dir, "importer")
        .await
        .expect("re-import");
    assert_eq!(second.imported, 0, "rerun is idempotent");
    assert_eq!(second.skipped, 5, "all 5 records skipped on rerun");
    assert_eq!(count_rows(&storage).await, 5, "no duplicate rows");
    // The relation counts are scoped to the APPLIED subset (bd's applied-subset scoping): a full-Skip
    // rerun inserts NOTHING, so deps/comments MUST be 0 — never re-counted over the Skipped records
    // (would over-report the FR-26 "migrated" count on every idempotent rerun).
    assert_eq!(
        second.dependencies, 0,
        "rerun applies nothing → dependencies=0 (applied-subset scoping)"
    );
    assert_eq!(
        second.comments, 0,
        "rerun applies nothing → comments=0 (applied-subset scoping)"
    );
}

#[tokio::test]
async fn bd_import_mixed_counts_only_the_newly_applied_subset() {
    // Applied-subset scoping proof (NOT a full-set tally): first import the fixture (5 records,
    // bd-1 carries 3 POST-dedup deps + 2 comments). The second import re-presents the SAME 5 records
    // (all Skipped) PLUS one NEW record (bd-6) carrying its OWN deps + comments. The second report's
    // relation counts MUST reflect ONLY bd-6's relations (the applied subset) — the Skipped records'
    // relations are NOT re-counted.
    let (tmp, dir, staged) = stage_fixture();
    let storage = fresh_storage().await;

    let first = import_bd(&storage, &staged, &dir, "importer")
        .await
        .expect("first import");
    assert_eq!(first.imported, 5);
    assert_eq!(first.dependencies, 3, "bd-1 POST-dedup deps");
    assert_eq!(first.comments, 2, "bd-1 comments");

    // Build the second file: the SAME 5 fixture lines (→ all Skip) + a NEW bd-6 with 2 deps + 1
    // comment. bd-6's relations are the ONLY applied-subset relations on the second import.
    let fixture_bytes = std::fs::read(&staged).expect("read staged fixture");
    let mut second_file = fixture_bytes;
    let bd6 = concat!(
        r#"{"id":"bd-6","title":"new record with relations","status":"open","priority":2,"#,
        r#""issue_type":"task","created_at":"2023-11-14T22:13:20Z",""#,
        r#"updated_at":"2023-11-14T22:13:20Z","#,
        r#""dependencies":["#,
        r#"{"issue_id":"bd-6","depends_on_id":"bd-1","type":"blocks","created_at":"2023-11-14T22:13:20Z"},"#,
        r#"{"issue_id":"bd-6","depends_on_id":"bd-2","type":"blocks","created_at":"2023-11-14T22:13:20Z"}"#,
        r#"],"comments":[{"id":9,"issue_id":"bd-6","author":"carol","text":"only comment","created_at":"2023-11-15T09:00:00Z"}]}"#,
        "\n"
    );
    second_file.extend_from_slice(bd6.as_bytes());
    let staged2 = dir.join("issues.jsonl");
    std::fs::write(&staged2, &second_file).expect("write second file");

    let second = import_bd(&storage, &staged2, &dir, "importer")
        .await
        .expect("second import");

    assert_eq!(second.imported, 1, "only bd-6 is new");
    assert_eq!(second.skipped, 5, "the 5 fixture records already exist");
    // Applied-subset scoping: ONLY bd-6's relations are counted (2 deps + 1 comment), NOT the
    // Skipped bd-1's 3 deps / 2 comments (a full-set tally would report 5 deps / 3 comments).
    assert_eq!(
        second.dependencies, 2,
        "count ONLY bd-6's 2 deps (applied subset), not the skipped bd-1's 3"
    );
    assert_eq!(
        second.comments, 1,
        "count ONLY bd-6's 1 comment (applied subset), not the skipped bd-1's 2"
    );
    assert_eq!(count_rows(&storage).await, 6, "5 + bd-6");
    drop(tmp);
}

#[tokio::test]
async fn bd_import_rejects_conflict_markers_zero_writes() {
    // FR-8 on the bd path: a merge-accident file is rejected at preflight, ZERO DB writes.
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join(".unblock");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("issues.jsonl");
    let mut f = std::fs::File::create(&path).unwrap();
    writeln!(f, "<<<<<<< HEAD").unwrap();
    writeln!(f, "=======").unwrap();
    writeln!(f, ">>>>>>> theirs").unwrap();
    drop(f);

    let storage = fresh_storage().await;
    let before = count_rows(&storage).await;
    let err = import_bd(&storage, &path, &dir, "importer")
        .await
        .expect_err("conflict markers");
    assert!(
        matches!(err, unblock_sync::SyncError::ConflictMarkers { .. }),
        "{err:?}"
    );
    assert_eq!(count_rows(&storage).await, before, "zero writes on reject");
}

#[tokio::test]
async fn bd_import_mapping_report_snapshot() {
    // insta (NFR-14): pin the mapping report (counts + dropped-field list) over the fixture.
    let (_tmp, dir, staged) = stage_fixture();
    let storage = fresh_storage().await;
    let report = import_bd(&storage, &staged, &dir, "importer")
        .await
        .expect("bd import");
    insta::assert_json_snapshot!(report);
}

#[tokio::test]
async fn bd_import_refuses_parent_traversal_path() {
    // FR-8 path confinement: a `..`-escaping path is refused at preflight (platform-independent —
    // rejected lexically, no /tmp-symlink dependence).
    let (_tmp, dir, _staged) = stage_fixture();
    let storage = fresh_storage().await;
    let escaping = Path::new("../../etc/issues.jsonl");
    let err = import_bd(&storage, escaping, &dir, "importer")
        .await
        .expect_err("parent traversal refused");
    assert!(
        matches!(err, unblock_sync::SyncError::PathTraversal { .. }),
        "{err:?}"
    );
}

// -------------------------------------------------------------------------------------------
// D43 — DUPLICATE JSON KEYS on the `bd` line parse.
//
// This is the SECOND instance of the root cause the MCP transport closes. `serde_json::from_str`
// collapses a duplicated key last-wins while building the `Value`, so a `bd` record whose text says
// one thing imports as another — and `dropped_fields` cannot see it BY CONSTRUCTION, because the
// key-diff runs over the already-collapsed map.
// -------------------------------------------------------------------------------------------

/// The hand-written duplicate-key case catalogue.
///
/// It is deliberately NOT an importable export: `//` lines carry the case names and the
/// never-regenerate warning, so the suite stages each case individually.
const DUP_FIXTURE: &str = include_str!("fixtures/bd_export_duplicate_keys.jsonl");

/// `(case name, json line)` for every non-comment line, in file order. The LAST entry is the clean
/// control.
fn duplicate_key_cases() -> Vec<(String, String)> {
    let mut cases = Vec::new();
    let mut pending = String::new();
    for line in DUP_FIXTURE.lines() {
        if let Some(rest) = line.strip_prefix("// CASE: ") {
            pending = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("// CLEAN CONTROL") {
            pending = format!("CLEAN CONTROL{rest}");
        } else if !line.starts_with("//") && !line.trim().is_empty() {
            cases.push((std::mem::take(&mut pending), line.to_string()));
        }
    }
    cases
}

/// Stage `lines` as a confined `.unblock/issues.jsonl` and return `(tempdir, dir, path)`.
fn stage_lines(lines: &[&str]) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join(".unblock");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("issues.jsonl");
    std::fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();
    (tmp, dir, path)
}

/// **The MECHANICAL never-regenerated guard.**
///
/// A header comment is not executable. If this fixture is ever round-tripped through a serializer
/// every duplicate collapses to ONE occurrence and the whole suite goes green against input it can
/// no longer express. This asserts the duplicates are still physically there.
#[test]
fn every_fixture_duplicate_line_carries_its_key_exactly_twice() {
    let expected_keys = [
        ("bd-d1", "id"),
        ("bd-d2", "status"),
        ("bd-d3", "depends_on_id"),
        ("bd-d4", "text"),
        ("bd-d5", "author"),
    ];
    let cases = duplicate_key_cases();
    assert_eq!(
        cases.len(),
        expected_keys.len() + 1,
        "the catalogue must hold one line per case plus the clean control: {cases:?}"
    );
    for (id, key) in expected_keys {
        let line = cases
            .iter()
            .find(|(_, text)| text.contains(&format!("\"{id}\"")))
            .unwrap_or_else(|| panic!("case {id} missing from the catalogue"))
            .1
            .clone();
        assert_eq!(
            line.matches(&format!("\"{key}\"")).count(),
            2,
            "case {id}: `{key}` must appear EXACTLY twice — a serializer round-trip would have \
             collapsed it to once and this fixture would silently stop testing anything: {line}"
        );
    }
    assert!(
        DUP_FIXTURE.contains("NEVER REGENERATE THIS FILE THROUGH A SERIALIZER"),
        "the fixture must keep its warning header"
    );
}

/// Every duplicate-key case is REJECTED, LINE-NUMBERED, with ZERO writes.
///
/// Each case is staged BEHIND a clean control line, so the reported line number is a real one (2) —
/// not the trivial 1 a single-line file would always produce — and the zero-writes assertion is a
/// real all-or-nothing claim: the control line ahead of it WOULD have imported.
#[tokio::test]
async fn bd_import_rejects_every_duplicate_key_line_with_zero_writes() {
    let cases = duplicate_key_cases();
    let (control_name, control) = cases.last().cloned().expect("clean control");
    assert!(
        control_name.starts_with("CLEAN CONTROL"),
        "the LAST catalogue entry must be the clean control, got `{control_name}`"
    );

    let expected_key = |id: &str| match id {
        "bd-d1" => "id",
        "bd-d2" => "status",
        "bd-d3" => "depends_on_id",
        "bd-d4" => "text",
        _ => "author",
    };

    for (name, line) in cases.iter().take(cases.len() - 1) {
        let (_tmp, dir, path) = stage_lines(&[&control, line]);
        let storage = fresh_storage().await;
        let err = import_bd(&storage, &path, &dir, "importer")
            .await
            .err()
            .unwrap_or_else(|| panic!("{name}: a duplicate-key line MUST be rejected"));
        match err {
            unblock_sync::SyncError::DuplicateKey {
                line: line_no,
                ref key,
                ..
            } => {
                assert_eq!(line_no, 2, "{name}: the REAL line number must be reported");
                let id = line.split('"').nth(3).unwrap_or_default().to_string();
                assert_eq!(
                    key,
                    expected_key(&id),
                    "{name}: the duplicated key is named"
                );
            }
            other => panic!("{name}: expected DuplicateKey, got {other:?}"),
        }
        assert_eq!(
            count_rows(&storage).await,
            0,
            "{name}: ZERO writes — the clean control line ahead of the duplicate must not land \
             either (all-or-nothing)"
        );
    }
}

/// A line the scanner cannot resolve is refused FAIL-CLOSED, with its own variant.
///
/// Reusing `DuplicateKey` with empty strings here would report a duplicate that may not exist —
/// telling the operator something false about their data.
#[tokio::test]
async fn bd_import_refuses_an_unscannable_line_fail_closed() {
    let cases = duplicate_key_cases();
    let (_, control) = cases.last().cloned().expect("clean control");
    // Nesting past serde_json's 128-level recursion limit: neither the scanner nor the parser can
    // resolve it, so the scan is INDETERMINATE and the line is refused rather than waved through.
    let deep = format!("{}{}", "[".repeat(200), "]".repeat(200));
    let (_tmp, dir, path) = stage_lines(&[&control, &deep]);
    let storage = fresh_storage().await;
    let err = import_bd(&storage, &path, &dir, "importer")
        .await
        .expect_err("an unscannable line must be refused");
    assert!(
        matches!(err, unblock_sync::SyncError::IndeterminateLine { line: 2 }),
        "{err:?}"
    );
    assert_eq!(count_rows(&storage).await, 0, "zero writes on reject");
}

/// The clean control still imports, with an unchanged `dropped_fields` report.
///
/// The ACCEPT half of the discipline: without it every rejection cell could pass for the wrong
/// reason (a scanner that refuses EVERYTHING) and prove nothing.
#[tokio::test]
async fn bd_import_still_accepts_the_clean_control_line() {
    let cases = duplicate_key_cases();
    let (_, control) = cases.last().cloned().expect("clean control");
    let (_tmp, dir, path) = stage_lines(&[&control]);
    let storage = fresh_storage().await;
    let report = import_bd(&storage, &path, &dir, "importer")
        .await
        .expect("the clean control must import");
    assert_eq!(report.imported, 1, "{report:?}");
    assert_eq!(
        report.dropped_fields,
        vec!["legacy_field".to_string()],
        "the advisory dropped-fields report is unchanged by D43: {report:?}"
    );
    assert_eq!(count_rows(&storage).await, 1);
}

/// **The generic-JSONL IMMUNITY PIN.**
///
/// `parse_issue_line` deserializes straight into `Issue`, a plain derived struct, so `serde_derive`'s
/// generated `visit_map` already errors with ``duplicate field `id` ``. That immunity is a DERIVE
/// ARTEFACT: adding a `#[serde(flatten)]` or a `serde_json::Value`-typed field to `Issue` would
/// destroy it SILENTLY, with no test failure anywhere — which is what this pin exists to prevent.
///
/// It drives the PRODUCTION entry points, deliberately never naming the internal `from_str::<Issue>`
/// call: a test written against that call independently re-implements it and therefore cannot
/// observe the refactor it claims to catch.
#[tokio::test]
async fn the_generic_jsonl_path_is_immune_to_duplicate_keys() {
    let duplicate = r#"{"id":"ub-a","title":"t","status":"open","priority":2,"issue_type":"task","created_at":"2023-11-14T22:13:20Z","updated_at":"2023-11-14T22:13:20Z","id":"ub-EVIL"}"#;
    assert_eq!(
        duplicate.matches("\"id\"").count(),
        2,
        "non-vacuity: the pin's own input must really carry the duplicate"
    );

    let err = unblock_sync::parse_issue_line(duplicate, 7)
        .expect_err("a duplicated field must be REJECTED, never last-wins collapsed");
    assert!(
        matches!(err, unblock_sync::SyncError::JsonlParse { line: 7, .. }),
        "{err:?}"
    );

    // ... and end-to-end through the import orchestrator, with zero writes.
    let clean = r#"{"id":"ub-ok","title":"ok","status":"open","priority":2,"issue_type":"task","created_at":"2023-11-14T22:13:20Z","updated_at":"2023-11-14T22:13:20Z"}"#;
    let (_tmp, dir, path) = stage_lines(&[clean, duplicate]);
    let storage = fresh_storage().await;
    let err = unblock_sync::import_jsonl(
        &storage,
        &path,
        &dir,
        "importer",
        &unblock_sync::ImportOptions::default(),
    )
    .await
    .expect_err("import_jsonl must reject the duplicate-key line");
    assert!(
        matches!(
            err,
            unblock_sync::SyncError::ValidationFailed { line: 2, .. }
        ),
        "{err:?}"
    );
    assert_eq!(count_rows(&storage).await, 0, "zero writes on reject");
}
