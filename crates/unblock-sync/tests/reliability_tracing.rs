//! NFR-13/D30 — `tracing_capture_reliability`: the REACHABLE conflict-marker-rejection guard emits
//! EXACTLY ONE INFO event on `unblock.reliability` carrying all four field VALUES.
//!
//! Capture is the D30 dep-free FALLBACK (not `tracing-capture`, not `tracing-test`): a hand-rolled
//! `tracing_subscriber::Layer` + `field::Visit` records the four structured field VALUES into a map
//! and asserts each — `tracing-test`'s `logs_contain` would only match the FORMATTED string, not the
//! field VALUES the D30 AC requires. The layer is installed via `set_default` (SCOPED, thread-local),
//! so it never touches the global `init_tracing`.
//!
//! Non-vacuous: removing the `reliability_guard!` from `import_jsonl`'s conflict-marker arm drops the
//! event count to 0, failing `reliability.len() == 1`.

#![allow(missing_docs)] // internal test-support layer; not a public API surface.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;

use unblock_sync::{ImportOptions, SyncError, import_jsonl};

mod fake;
use fake::FakeStorage;

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
