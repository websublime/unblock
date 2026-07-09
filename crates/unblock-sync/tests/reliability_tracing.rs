//! NFR-13/D30 reliability-emission capture tests over `unblock.reliability`, asserting the structured
//! field VALUES (not just the formatted string) each SSOT arm carries:
//! - `tracing_capture_reliability`: the REACHABLE conflict-marker-rejection guard (`reliability_guard!`
//!   INFO) emits EXACTLY ONE event carrying all four field VALUES;
//! - `tracing_capture_reliability_detail_debug`: the REACHABLE per-skip import detail
//!   (`reliability_detail!` DEBUG) emits EXACTLY ONE event carrying all four field VALUES — the DEBUG
//!   arm that only the INFO arm previously had a capturing test for.
//!
//! Capture is the D30 dep-free FALLBACK (not `tracing-capture`, not `tracing-test`): a hand-rolled
//! `tracing_subscriber::Layer` + `field::Visit` records the four structured field VALUES into a map
//! and asserts each — `tracing-test`'s `logs_contain` would only match the FORMATTED string, not the
//! field VALUES the D30 AC requires. The layer is installed via `set_default` (SCOPED, thread-local),
//! so it never touches the global `init_tracing`. `CaptureLayer` carries NO level filter, so it
//! records EVERY level (INFO and DEBUG alike) — no INFO-only gate to lift.
//!
//! Non-vacuous: removing the `reliability_guard!` from `import_jsonl`'s conflict-marker arm drops the
//! INFO event count to 0; dropping the `reason` field from the `reliability_detail!` macro body drops
//! the DEBUG `reason` VALUE — each failing its test's four-VALUE assertion.

#![allow(missing_docs)] // internal test-support layer; not a public API surface.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;

use unblock_sync::{ImportOptions, SyncError, import_jsonl, serialize_issue_line};

mod fake;
use fake::{FakeStorage, sample_issue, tombstone_of};

/// One captured event: its level, target, and the field name -> VALUE map.
#[derive(Debug, Clone)]
struct Captured {
    level: Level,
    target: String,
    fields: HashMap<String, String>,
}

/// A field visitor recording every field's VALUE as a string.
///
/// `%value` (Display) is recorded by `tracing` via `record_debug` over a `DisplayValue` whose `Debug`
/// forwards to `Display`, so `{value:?}` yields the field VALUE with no surrounding quotes.
struct FieldGrab(HashMap<String, String>);

impl Visit for FieldGrab {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .insert(field.name().to_string(), format!("{value:?}"));
    }
}

/// A scoped capture layer collecting every event into a shared vec.
struct CaptureLayer(Arc<Mutex<Vec<Captured>>>);

impl<S> Layer<S> for CaptureLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let mut grab = FieldGrab(HashMap::new());
        event.record(&mut grab);
        let captured = Captured {
            level: *meta.level(),
            target: meta.target().to_string(),
            fields: grab.0,
        };
        if let Ok(mut events) = self.0.lock() {
            events.push(captured);
        }
    }
}

#[tokio::test]
async fn tracing_capture_reliability() {
    let captured: Arc<Mutex<Vec<Captured>>> = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::registry().with(CaptureLayer(captured.clone()));

    // SCOPED (thread-local) install — does NOT touch the global `init_tracing`.
    let guard = tracing::subscriber::set_default(subscriber);

    // Drive the REACHABLE conflict-marker-rejection guard: import a CONFINED file with git markers.
    let dir = tempfile::tempdir().expect("tempdir");
    let unblock = dir.path().join(".unblock");
    std::fs::create_dir_all(&unblock).expect("create .unblock");
    let path = unblock.join("issues.jsonl");
    std::fs::write(&path, "<<<<<<< HEAD\n=======\n").expect("write markers");

    let storage = FakeStorage::with_issues(vec![]);
    let err = import_jsonl(
        &storage,
        &path,
        &unblock,
        "tester",
        &ImportOptions::default(),
    )
    .await
    .expect_err("conflict markers must be rejected");
    assert!(
        matches!(err, SyncError::ConflictMarkers { .. }),
        "expected ConflictMarkers, got {err:?}"
    );

    // Stop capturing before inspecting.
    drop(guard);

    let events = captured.lock().expect("capture lock");
    let reliability: Vec<&Captured> = events
        .iter()
        .filter(|c| c.target == unblock_error::RELIABILITY_TARGET)
        .collect();

    assert_eq!(
        reliability.len(),
        1,
        "exactly one reliability event expected, got {reliability:?}"
    );
    let ev = reliability[0];
    assert_eq!(ev.level, Level::INFO, "a guard activation is INFO");

    // All FOUR field VALUES (D30 AC) — not just the formatted string.
    assert_eq!(
        ev.fields.get("operation").map(String::as_str),
        Some("import"),
        "operation VALUE"
    );
    assert_eq!(
        ev.fields.get("result").map(String::as_str),
        Some("conflict-markers-rejected"),
        "result VALUE"
    );
    assert!(
        ev.fields
            .get("path")
            .is_some_and(|p| p.ends_with("issues.jsonl")),
        "path VALUE names the confined file, got {:?}",
        ev.fields.get("path")
    );
    assert!(
        ev.fields
            .get("reason")
            .is_some_and(|r| r.contains("line 1")),
        "reason VALUE carries the marker preview, got {:?}",
        ev.fields.get("reason")
    );
}

#[tokio::test]
async fn tracing_capture_reliability_detail_debug() {
    let captured: Arc<Mutex<Vec<Captured>>> = Arc::new(Mutex::new(Vec::new()));
    // Same hand-rolled capture layer — NO level filter, so it records DEBUG as well as INFO.
    let subscriber = tracing_subscriber::registry().with(CaptureLayer(captured.clone()));

    // SCOPED (thread-local) install — does NOT touch the global `init_tracing`.
    let guard = tracing::subscriber::set_default(subscriber);

    // Drive the REACHABLE per-skip import DEBUG (`reliability_detail!`): a NON-tombstone incoming line
    // for a DB-tombstoned id is SKIPPED ("tombstone protection", deterministic — no hash dependence),
    // which is the ONLY reliability emission on this clean import path.
    let dir = tempfile::tempdir().expect("tempdir");
    let unblock = dir.path().join(".unblock");
    std::fs::create_dir_all(&unblock).expect("create .unblock");
    let path = unblock.join("issues.jsonl");
    let line = serialize_issue_line(&sample_issue("ub-skip-1")).expect("serialize line");
    std::fs::write(&path, format!("{line}\n")).expect("write jsonl");

    // Existing tombstoned row for the same id → the incoming non-tombstone line is skipped.
    let storage = FakeStorage::with_issues(vec![tombstone_of("ub-skip-1")]);
    let report = import_jsonl(
        &storage,
        &path,
        &unblock,
        "tester",
        &ImportOptions::default(),
    )
    .await
    .expect("import must succeed with the record skipped");
    assert_eq!(report.imported, 0, "the tombstone must not be resurrected");
    assert_eq!(report.skipped, 1, "exactly one record skipped");

    // Stop capturing before inspecting.
    drop(guard);

    let events = captured.lock().expect("capture lock");
    let reliability: Vec<&Captured> = events
        .iter()
        .filter(|c| c.target == unblock_error::RELIABILITY_TARGET)
        .collect();

    assert_eq!(
        reliability.len(),
        1,
        "exactly one reliability event expected, got {reliability:?}"
    );
    let ev = reliability[0];
    assert_eq!(ev.level, Level::DEBUG, "a per-skip detail is DEBUG");

    // All FOUR field VALUES (D30 AC) — the record id rides the uniform `path` field, `reason` the
    // skip cause. Dropping any field from `reliability_detail!` fails one of these.
    assert_eq!(
        ev.fields.get("operation").map(String::as_str),
        Some("import"),
        "operation VALUE (import_jsonl path)"
    );
    assert_eq!(
        ev.fields.get("path").map(String::as_str),
        Some("ub-skip-1"),
        "path VALUE carries the record id"
    );
    assert_eq!(
        ev.fields.get("result").map(String::as_str),
        Some("skip"),
        "result VALUE"
    );
    assert_eq!(
        ev.fields.get("reason").map(String::as_str),
        Some("tombstone protection"),
        "reason VALUE carries the skip cause"
    );
}
