//! Display/result DTOs and diagnostics (CF-A/CF-B, spine §1.10).
//!
//! Owned here so `unblock-render` (model + error only) and `unblock-mcp` can format engine results
//! without depending on `unblock-engine`/`unblock-storage`; re-exported (never redefined) by those
//! crates.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::enums::DependencyType;
use crate::issue::Issue;

/// A single count bucket from a count query.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct CountBucket {
    /// The bucket key (status/type/assignee/priority/label value).
    pub key: String,
    /// The count in this bucket.
    pub count: usize,
}

/// A directed dependency edge.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct GraphEdge {
    /// The source issue id.
    pub from: String,
    /// The target issue id.
    pub to: String,
    /// The dependency type.
    pub dep_type: DependencyType,
}

/// A dependency tree/graph rooted at one or more issues.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct DepTree {
    /// The root issue id.
    pub root: String,
    /// The edges of the tree/graph.
    pub edges: Vec<GraphEdge>,
}

/// The outcome of a close-with-suggestions operation (FR-11).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct CloseOutcome {
    /// The issue that was closed.
    pub closed: Issue,
    /// Issues that became unblocked as a result.
    pub newly_unblocked: Vec<Issue>,
}

/// The report of a JSONL/bd import operation (FR-8/FR-26).
///
/// `dependencies`/`comments` count the relations/comments of the issues ACTUALLY inserted by the
/// one-shot `bd` import (the applied subset — D24/F1), matching bd's applied-subset scoping (its
/// `record_imported_relation_counts` runs only on applied Insert/Update records, never on a Skip). So
/// an idempotent rerun (all records Skipped) reports `dependencies=0, comments=0`. Both stay `0` on
/// the generic `import_jsonl` path (it never tallies them).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct ImportReport {
    /// Number of issues imported.
    pub imported: usize,
    /// Number of lines skipped (no-ops / rejected).
    pub skipped: usize,
    /// Dependency edges on the applied (actually-inserted) subset (`0` on a full-Skip rerun and on
    /// the generic `import_jsonl` path).
    pub dependencies: usize,
    /// Comments on the applied (actually-inserted) subset (`0` on a full-Skip rerun and on the
    /// generic `import_jsonl` path).
    pub comments: usize,
    /// Fields dropped during import.
    pub dropped_fields: Vec<String>,
}

/// The report of a JSONL export operation (FR-7).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct ExportReport {
    /// Number of issues written.
    pub written: usize,
    /// The path written to (serialized as a string).
    pub path: PathBuf,
}

/// The kind of a diagnostic probe (CF-B; mirrors the spine §5.2 `DiagnosticsInput` kinds).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticKind {
    /// Aggregate statistics.
    Stats,
    /// General workspace info.
    Info,
    /// Where the workspace lives.
    Where,
    /// Version information.
    Version,
    /// Lint findings.
    Lint,
    /// The changelog of closed issues.
    Changelog,
    /// Orphan candidates (FR-15).
    Orphans,
}

/// A diagnostic report — a kind plus a list of findings (CF-B).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct DiagnosticReport {
    /// The kind of diagnostic.
    pub kind: DiagnosticKind,
    /// The findings.
    pub findings: Vec<DiagnosticFinding>,
}

/// A single generic key/value diagnostic finding row (CF-B).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct DiagnosticFinding {
    /// The finding label.
    pub label: String,
    /// The finding detail.
    pub detail: String,
}

#[cfg(test)]
mod tests {
    use super::{
        CloseOutcome, CountBucket, DepTree, DiagnosticFinding, DiagnosticKind, DiagnosticReport,
        ExportReport, GraphEdge, ImportReport,
    };
    use crate::enums::DependencyType;
    use std::path::PathBuf;

    #[test]
    fn count_bucket_roundtrip() {
        let b = CountBucket {
            key: "open".to_string(),
            count: 3,
        };
        let json = serde_json::to_string(&b).unwrap();
        let back: CountBucket = serde_json::from_str(&json).unwrap();
        assert_eq!(b, back);
    }

    #[test]
    fn graph_edge_and_dep_tree() {
        let tree = DepTree {
            root: "ub-a".to_string(),
            edges: vec![GraphEdge {
                from: "ub-a".to_string(),
                to: "ub-b".to_string(),
                dep_type: DependencyType::Blocks,
            }],
        };
        let json = serde_json::to_string(&tree).unwrap();
        let back: DepTree = serde_json::from_str(&json).unwrap();
        assert_eq!(tree, back);
    }

    #[test]
    fn close_outcome_default_empty_unblocked() {
        let outcome = CloseOutcome {
            closed: crate::issue::Issue::default(),
            newly_unblocked: Vec::new(),
        };
        assert!(outcome.newly_unblocked.is_empty());
    }

    #[test]
    fn import_export_reports_roundtrip() {
        let imp = ImportReport {
            imported: 5,
            skipped: 1,
            dependencies: 3,
            comments: 2,
            dropped_fields: vec!["x".to_string()],
        };
        let json = serde_json::to_string(&imp).unwrap();
        assert_eq!(imp, serde_json::from_str::<ImportReport>(&json).unwrap());

        let exp = ExportReport {
            written: 5,
            path: PathBuf::from("/tmp/issues.jsonl"),
        };
        let json = serde_json::to_string(&exp).unwrap();
        assert_eq!(exp, serde_json::from_str::<ExportReport>(&json).unwrap());
    }

    #[test]
    fn diagnostic_kind_snake_case_and_copy() {
        assert_eq!(
            serde_json::to_string(&DiagnosticKind::Changelog).unwrap(),
            "\"changelog\""
        );
        let k = DiagnosticKind::Stats;
        let copy = k; // `Copy`, so `k` is still usable below.
        assert_eq!(k, copy);
    }

    #[test]
    fn diagnostic_report_roundtrip() {
        let report = DiagnosticReport {
            kind: DiagnosticKind::Info,
            findings: vec![DiagnosticFinding {
                label: "k".to_string(),
                detail: "v".to_string(),
            }],
        };
        let json = serde_json::to_string(&report).unwrap();
        assert_eq!(
            report,
            serde_json::from_str::<DiagnosticReport>(&json).unwrap()
        );
    }
}
