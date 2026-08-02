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
    /// The dependency edges whose target denotes nothing (D45).
    //
    // D45 — the `///` doc comment ABOVE is CONTRACT BYTES (spine §1.10): schemars lifts it into the
    // variant `description` that rides `schema_bundle()`, which `CONTRACT_HASH` digests. Re-wording
    // it, even harmlessly, RE-CUTS the hash and is a contract change, never a comment tidy-up.
    //
    // D45 — `Dangling` is APPENDED, never inserted mid-list: schemars emits the variants in
    // DECLARATION order and `CONTRACT_HASH` digests those bytes, so a mid-list insertion would move
    // the digest for a reason unrelated to the new kind. §5.2's `DiagnosticsInput` gains its arm LAST
    // for the same reason, keeping the two mirrored. The variant is MINTED rather than reusing
    // `Lint`: both options bump the contract anyway, and reusing `Lint` would make
    // `DiagnosticReport.kind` DECLARE a kind the report is not — a lie on a published field.
    Dangling,
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

    /// D45 — the `DiagnosticKind` taxonomy is EIGHT kinds, `Dangling` is LAST, and its `description`
    /// carries the exact contract bytes.
    ///
    /// Asserted over the GENERATED SCHEMA, not over a hand-written list: schemars emits the variants
    /// in DECLARATION order into the `oneOf` array, and that array is what `CONTRACT_HASH` digests
    /// downstream. A name-SET assertion (or a re-blessed snapshot) would not catch a mid-list
    /// insertion; reading the generated order does.
    ///
    /// MUTANTS KILLED: (a) declaring `Dangling` anywhere but last (e.g. beside `Lint`) — the
    /// position assertion goes red while every name-set check stays green; (b) re-wording the
    /// variant's `///` doc comment, which is contract bytes lifted into `description`; (c) changing
    /// the wire spelling away from the plain noun `dangling`.
    #[test]
    fn diagnostic_kind_taxonomy_is_eight_with_dangling_last() {
        let schema = serde_json::to_value(schemars::schema_for!(DiagnosticKind)).unwrap();
        let variants = schema["oneOf"]
            .as_array()
            .expect("a unit-only enum schema is a `oneOf` array");
        assert_eq!(
            variants.len(),
            8,
            "the spine §5.2 taxonomy is EIGHT kinds since D45"
        );

        let spellings: Vec<&str> = variants
            .iter()
            .map(|v| v["const"].as_str().expect("each arm has a const spelling"))
            .collect();
        assert_eq!(
            spellings,
            [
                "stats",
                "info",
                "where",
                "version",
                "lint",
                "changelog",
                "orphans",
                "dangling",
            ],
            "declaration order is hash-visible: `dangling` is APPENDED last, never inserted"
        );

        let last = variants.last().expect("eight arms");
        assert_eq!(
            last["description"].as_str(),
            Some("The dependency edges whose target denotes nothing (D45)."),
            "the `Dangling` doc comment is CONTRACT BYTES — re-wording it re-cuts CONTRACT_HASH"
        );

        // The Rust value and the wire spelling agree in both directions.
        assert_eq!(
            serde_json::to_string(&DiagnosticKind::Dangling).unwrap(),
            "\"dangling\""
        );
        assert_eq!(
            serde_json::from_str::<DiagnosticKind>("\"dangling\"").unwrap(),
            DiagnosticKind::Dangling
        );
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
