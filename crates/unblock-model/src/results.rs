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

// --- FR-6 remainder (D56, v1.1) — label catalog / rename / close-eligible results (spine §1.10).
//     All three reach L7 (spine §5.3) through the `query`/`issue` output unions, so every `///` line
//     below is CONTRACT BYTES (a schema `description` digested by `CONTRACT_HASH`).

/// One label-catalog row (`query kind: labels`, FR-6 / D56).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct LabelCount {
    /// The label value.
    pub label: String,
    /// The number of non-tombstoned issues carrying the label, whatever their status.
    pub count: usize,
}

/// The outcome of a label rename (`issue action: relabel`, FR-6 / D56).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct LabelRename {
    /// The label that was renamed.
    pub old_name: String,
    /// The label it was renamed to.
    pub new_name: String,
    /// The number of non-tombstoned issues changed (renamed and merged alike).
    pub affected_issues: usize,
}

/// The outcome of closing every eligible epic (`issue action: close_eligible`, FR-6 / D56).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct CloseEligibleOutcome {
    /// The epic ids closed (or, on a dry run, that would close), in close order: by cascade round,
    /// then by id ascending, so every child epic precedes its parent.
    //
    // D56 clause (4): the key is `epic_ids`, NEVER `closed` — `issue close` already returns `closed`
    // as an `Issue` inside the same untagged `IssueOutput` union, and one key carrying two types
    // would make the wire ambiguous.
    pub epic_ids: Vec<String>,
    /// The number of entries in `epic_ids`.
    pub count: usize,
    /// Whether this was a dry run (nothing written).
    pub dry_run: bool,
}

// --- FR-13 full (D57, v1.1) — the `issue action: config` result row and the DB-layer fault types
//     (spine §1.10). `ConfigEntry` reaches L7 (spine §5.3) through the `issue` output union, so every
//     `///` line on it and on the types it names is CONTRACT BYTES (a schema `description` digested
//     by `CONTRACT_HASH`).

/// The layer that supplied a resolved runtime configuration value, highest precedence first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConfigSource {
    /// A command-line flag.
    Cli,
    /// An `UNBLOCK_*` environment variable.
    Env,
    /// The project file, `.unblock/config.toml`.
    Project,
    /// The user file, `config.toml` under the user configuration directory.
    User,
    /// A row of the database `config` table.
    Db,
    /// The operating-system user name (the actor's `$USER` fallback).
    OsUser,
    /// The built-in default.
    Default,
}

/// Why a database `config` row contributes no value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConfigFaultKind {
    /// The row names a startup key, which only the command line, the environment and the project
    /// file may set. Blocking.
    StartupKey,
    /// The row's value cannot be decoded for its key. Blocking.
    InvalidValue,
    /// The row names a key this build does not know. Not blocking.
    UnknownKey,
}

impl ConfigFaultKind {
    /// Whether a row with this fault fail-closes the workspace (D57 clause (2a)) — the ONE blocking
    /// predicate: the engine's fail-closed gate, the cli `mcp` startup line and `unblock init`'s
    /// check all call it. `startup_key` and `invalid_value` block; `unknown_key` does not (SF-4
    /// forward compatibility).
    #[must_use]
    pub const fn is_blocking(self) -> bool {
        matches!(self, Self::StartupKey | Self::InvalidValue)
    }
}

/// A database `config` row's fault.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct ConfigFault {
    /// What is wrong with the row.
    pub kind: ConfigFaultKind,
    /// A human description of the fault.
    pub reason: String,
}

/// One faulty database `config` row, as read (D57 clause (2a)).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct DbConfigFault {
    /// The row's key.
    pub key: String,
    /// The row's value, as stored.
    pub value: String,
    /// Why the row contributes no value.
    pub fault: ConfigFault,
}

/// One runtime configuration entry (`issue action: config`, FR-13 / D57).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct ConfigEntry {
    /// The configuration key.
    pub key: String,
    /// The database `config` row's value, or null when no row holds the key.
    pub value: Option<String>,
    /// The canonical value this session resolved, or null when the key is not a runtime layer key.
    pub effective: Option<String>,
    /// The layer that supplied `effective`, or null when the key is not a runtime layer key.
    pub source: Option<ConfigSource>,
    /// The row's fault, or null for a sound or absent row.
    pub fault: Option<ConfigFault>,
}

#[cfg(test)]
mod tests {
    use super::{
        CloseOutcome, ConfigEntry, ConfigFault, ConfigFaultKind, ConfigSource, CountBucket,
        DepTree, DiagnosticFinding, DiagnosticKind, DiagnosticReport, ExportReport, GraphEdge,
        ImportReport,
    };
    use crate::enums::DependencyType;
    use std::path::PathBuf;

    /// D57 clause (2a): `is_blocking` is the ONE blocking predicate — `startup_key` and
    /// `invalid_value` block, `unknown_key` does not (SF-4 forward compatibility).
    #[test]
    fn config_fault_kind_blocking_is_startup_and_invalid_only() {
        assert!(ConfigFaultKind::StartupKey.is_blocking());
        assert!(ConfigFaultKind::InvalidValue.is_blocking());
        assert!(!ConfigFaultKind::UnknownKey.is_blocking());
    }

    /// The `ConfigEntry` wire shape: `snake_case` sources and fault kinds, nulls spelled out.
    #[test]
    fn config_entry_wire_shape() {
        let entry = ConfigEntry {
            key: "actor".to_string(),
            value: Some("bob".to_string()),
            effective: Some("alice".to_string()),
            source: Some(ConfigSource::OsUser),
            fault: Some(ConfigFault {
                kind: ConfigFaultKind::InvalidValue,
                reason: "r".to_string(),
            }),
        };
        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "key": "actor",
                "value": "bob",
                "effective": "alice",
                "source": "os_user",
                "fault": { "kind": "invalid_value", "reason": "r" }
            })
        );
        let empty = ConfigEntry {
            key: "db_filename".to_string(),
            value: None,
            effective: None,
            source: None,
            fault: None,
        };
        assert_eq!(
            serde_json::to_value(&empty).unwrap(),
            serde_json::json!({
                "key": "db_filename", "value": null, "effective": null, "source": null, "fault": null
            })
        );
    }

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
