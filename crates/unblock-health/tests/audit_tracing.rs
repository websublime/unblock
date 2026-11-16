//! The NFR-13 audit record: `WorkspaceClassification::audit_record` builds the four D30 fields and
//! `ReliabilityAuditRecord::emit` writes ONE INFO event on `unblock.reliability` carrying their
//! values.
//!
//! Capture is the D30 dep-free approach `unblock-sync`'s `reliability_tracing.rs` uses: a scoped
//! `tracing_subscriber::Layer` plus a `field::Visit` that records each field VALUE.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;
use unblock_health::{
    AdvisoryFacts, DbFacts, DbFileFact, ExportFacts, ExportScan, PreOpenFacts,
    ReliabilityAuditRecord, classify,
};

#[derive(Debug, Clone)]
struct Captured {
    level: Level,
    target: String,
    fields: HashMap<String, String>,
}

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

struct CaptureLayer(Arc<Mutex<Vec<Captured>>>);

impl<S> Layer<S> for CaptureLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let mut grab = FieldGrab(HashMap::new());
        event.record(&mut grab);
        if let Ok(mut events) = self.0.lock() {
            events.push(Captured {
                level: *meta.level(),
                target: meta.target().to_string(),
                fields: grab.0,
            });
        }
    }
}

fn index_damaged_classification() -> unblock_health::WorkspaceClassification {
    let pre = PreOpenFacts {
        db: DbFileFact::SqliteHeader,
        lost: false,
        wal_len: None,
        shm_present: false,
        journal_present: true,
        write_lock_held: false,
        recovery_entries: Vec::new(),
    };
    let db = DbFacts {
        integrity_rows: vec!["wrong # of entries in index idx_a".to_string()],
        index_names: vec!["idx_a".to_string()],
        ..DbFacts::default()
    };
    let export = ExportFacts {
        path: None,
        scan: ExportScan::Absent,
    };
    classify(
        &pre,
        Some(&db),
        &export,
        &AdvisoryFacts {
            now_utc: SystemTime::now(),
        },
    )
}

#[test]
fn audit_record_carries_level_and_anomaly_codes() {
    let record = index_damaged_classification().audit_record("doctor");
    assert_eq!(
        record,
        ReliabilityAuditRecord {
            operation: "doctor".to_string(),
            path: String::new(),
            result: "recoverable".to_string(),
            reason: "journal_sidecar_present,index_corruption".to_string(),
        }
    );
}

#[test]
fn emit_writes_one_info_event_on_unblock_reliability_with_four_field_values() {
    let captured: Arc<Mutex<Vec<Captured>>> = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::registry().with(CaptureLayer(captured.clone()));
    let guard = tracing::subscriber::set_default(subscriber);

    let mut record = index_damaged_classification().audit_record("doctor");
    record.path = "/ws/.unblock/unblock.db".to_string();
    record.emit();

    drop(guard);
    let events = captured.lock().unwrap();
    let reliability: Vec<&Captured> = events
        .iter()
        .filter(|c| c.target == unblock_error::RELIABILITY_TARGET)
        .collect();
    assert_eq!(reliability.len(), 1, "{events:?}");
    let event = reliability[0];
    assert_eq!(event.target, "unblock.reliability");
    assert_eq!(event.level, Level::INFO);
    let field = |name: &str| event.fields.get(name).map(String::as_str);
    assert_eq!(field("operation"), Some("doctor"));
    assert_eq!(field("path"), Some("/ws/.unblock/unblock.db"));
    assert_eq!(field("result"), Some("recoverable"));
    assert_eq!(
        field("reason"),
        Some("journal_sidecar_present,index_corruption")
    );
}

#[test]
fn a_healthy_classification_audits_reason_none() {
    let pre = PreOpenFacts {
        db: DbFileFact::SqliteHeader,
        lost: false,
        wal_len: None,
        shm_present: false,
        journal_present: false,
        write_lock_held: false,
        recovery_entries: Vec::new(),
    };
    let export = ExportFacts {
        path: None,
        scan: ExportScan::Absent,
    };
    let record = classify(
        &pre,
        None,
        &export,
        &AdvisoryFacts {
            now_utc: SystemTime::now(),
        },
    )
    .audit_record("recover");
    assert_eq!(record.result, "healthy");
    assert_eq!(record.reason, "none");
}
