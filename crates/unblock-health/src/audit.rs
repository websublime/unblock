//! The NFR-13 audit record on the `unblock.reliability` tracing target.
//!
//! Every reliability emission carries the four D30 fields `operation`, `path`, `result` and
//! `reason`. Building a record is pure; [`ReliabilityAuditRecord::emit`] writes it.

use unblock_error::RELIABILITY_TARGET;

use crate::anomaly::AnomalyClass;
use crate::classify::WorkspaceClassification;

/// One NFR-13 audit record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReliabilityAuditRecord {
    /// The operation that produced the record (`doctor`, `recover`, for example).
    pub operation: String,
    /// The workspace path the record concerns; empty until the caller sets it.
    pub path: String,
    /// The outcome; for a classification, the level's label.
    pub result: String,
    /// Why; for a classification, the anomaly codes in report order, or `none`.
    pub reason: String,
}

impl ReliabilityAuditRecord {
    /// Emit the record as one INFO event on `unblock.reliability`.
    pub fn emit(&self) {
        tracing::info!(
            target: RELIABILITY_TARGET,
            operation = %self.operation,
            path = %self.path,
            result = %self.result,
            reason = %self.reason,
        );
    }
}

impl WorkspaceClassification {
    /// The audit record of this classification for `operation`.
    ///
    /// `result` is the level's label and `reason` lists the anomaly codes. The classification holds
    /// no path, so `path` is empty; the caller sets it before [`emit`](ReliabilityAuditRecord::emit).
    #[must_use]
    pub fn audit_record(&self, operation: &str) -> ReliabilityAuditRecord {
        let reason = if self.anomalies.is_empty() {
            "none".to_string()
        } else {
            self.anomalies
                .iter()
                .map(AnomalyClass::code)
                .collect::<Vec<_>>()
                .join(",")
        };
        ReliabilityAuditRecord {
            operation: operation.to_string(),
            path: String::new(),
            result: self.level.as_str().to_string(),
            reason,
        }
    }
}
