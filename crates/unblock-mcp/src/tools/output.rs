//! MCP-owned OUTPUT types (spine §5.3 per-tool decomposition, T2.6/D25/FORK-1B).
//!
//! The tool success surface is a family of REAL, mcp-owned types — the single output authority, not
//! documentation. Each tool body constructs its structured success payload AS an arm of its tool's
//! [`serde(untagged)`] union (or as the tool's single output type), so the published
//! `schema_bundle()` output schemas are true BY CONSTRUCTION (never schema-only types). All unions are
//! `#[serde(untagged)]` ⇒ the wire bytes are IDENTICAL to serializing the arm's value directly, so
//! materializing them changed NO wire byte (only the schema bundle). `Box` is serde- and
//! schemars-transparent (wire bytes + published schema unchanged); the boxed arms
//! ([`IssueOutput::Issue`], [`IssueOutput::Close`]) keep `clippy::large_enum_variant` clean.
//!
//! [`IdOnly`] and [`SyncOutput`] are declared **here** (mcp-owned, F-4); they are NOT re-exported from
//! `unblock-model`. The report DTOs [`SyncOutput`] wraps (`ExportReport`/`ImportReport`), the
//! [`Issue`]/[`CloseOutcome`]/[`CountBucket`]/[`DepTree`]/[`Dependency`] the unions carry, ARE model
//! §1.10 types sourced from `unblock-model`.
//!
//! The in-band ERROR output is NOT an arm of any union: every tool may return a `StructuredError` with
//! `is_error = true` (FR-11). It is published ONCE, bundle-level, as `SchemaBundle.error`
//! (`resources/schema.rs`) — the rmcp `is_error` flag is the channel discriminator (spine §5.6).

use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use unblock_engine::DeleteMode;
use unblock_model::{
    CloseOutcome, CountBucket, DepTree, Dependency, ExportReport, ImportReport, Issue,
};

/// The quick-create output: the minted id only (spine §5.3).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct IdOnly {
    /// The minted issue id.
    pub id: String,
}

/// The `issue` tool success union — the 5 success shapes across the 8 actions (spine §5.3, D25).
///
/// `#[serde(untagged)]` ⇒ each arm serializes as its inner value directly (wire-identical to the
/// pre-D25 ad-hoc values). The large arms are boxed to keep `clippy::large_enum_variant` clean.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(untagged)]
pub(crate) enum IssueOutput {
    /// Quick-create: the minted id only.
    Id(IdOnly),
    /// A single issue (`create` / `show` / `reopen` / `restore`).
    Issue(Box<Issue>),
    /// Multiple issues (multi-id `update`; ALSO `create_bulk` — the N created issues).
    Issues(Vec<Issue>),
    /// The `close` outcome (`suggest_next` → `newly_unblocked`, FR-11).
    Close(Box<CloseOutcome>),
    /// The resolved delete plan (was the ad-hoc `delete_plan_json`).
    Delete(DeletePlanOutput),
}

/// The resolved `delete` plan output (spine §5.3, D25) — was the ad-hoc `delete_plan_json`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct DeletePlanOutput {
    /// The delete mode that was resolved.
    pub mode: DeleteModeOutput,
    /// The explicitly requested target ids.
    pub targets: Vec<String>,
    /// The child ids the chosen mode will also affect.
    pub cascade_children: Vec<String>,
}

/// The delete mode on the output wire (spine §5.3, D25) — mcp-owned, from the RESOLVED model
/// [`DeleteMode`] (an OUTPUT type, distinct from the input `DeleteModeInput`).
#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DeleteModeOutput {
    /// Soft-delete: tombstone the targets.
    Tombstone,
    /// Tombstone the targets and their children.
    Cascade,
    /// Permanently delete the rows.
    Hard,
    /// Compute the plan only; mutate nothing.
    DryRun,
}

impl From<DeleteMode> for DeleteModeOutput {
    fn from(mode: DeleteMode) -> Self {
        match mode {
            DeleteMode::Tombstone => Self::Tombstone,
            DeleteMode::Cascade => Self::Cascade,
            DeleteMode::Hard => Self::Hard,
            DeleteMode::DryRun => Self::DryRun,
        }
    }
}

/// The `query` tool success union (spine §5.3, D25). `#[serde(untagged)]` ⇒ wire-identical.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(untagged)]
pub(crate) enum QueryOutput {
    /// The `list`/`ready`/`blocked`/`search`/`stale` result set.
    Issues(Vec<Issue>),
    /// The `count` buckets.
    Counts(Vec<CountBucket>),
}

/// The `dep` tool success union (spine §5.3, D25). `#[serde(untagged)]` ⇒ wire-identical to the
/// pre-D25 ad-hoc `json!` values.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(untagged)]
pub(crate) enum DepOutput {
    /// `add` acknowledgement (`{"added":true}`).
    Added(DepAdded),
    /// `remove` acknowledgement (`{"removed":true}`).
    Removed(DepRemoved),
    /// The direct edges declared by an issue (`list`).
    Deps(Vec<Dependency>),
    /// The dependency `tree` OR `graph` (`Session::dependency_graph` returns a `DepTree`).
    Tree(DepTree),
    /// The ordered cycle-path witnesses (`cycles`, §3.2.1/D19).
    Cycles(Vec<Vec<String>>),
}

/// The `dep add` acknowledgement (`{"added":true}`) — a typed shape for the pre-D25 ad-hoc value.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct DepAdded {
    /// Always `true` on a successful add.
    pub added: bool,
}

/// The `dep remove` acknowledgement (`{"removed":true}`) — a typed shape for the pre-D25 ad-hoc value.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct DepRemoved {
    /// Always `true` on a successful remove.
    pub removed: bool,
}

/// The `sync` tool output: an export or import report (spine §5.3, G-23a).
///
/// An mcp-owned wrapper over the two model report DTOs (re-exported from `unblock-model`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SyncOutput {
    /// A JSONL export report.
    Export(ExportReport),
    /// A JSONL/bd import report.
    Import(ImportReport),
}

#[cfg(test)]
mod tests {
    use super::{
        DeleteModeOutput, DeletePlanOutput, DepAdded, DepOutput, DepRemoved, IdOnly, IssueOutput,
        SyncOutput,
    };
    use std::path::PathBuf;
    use unblock_engine::DeleteMode;
    use unblock_model::{ExportReport, ImportReport};

    #[test]
    fn id_only_serializes_to_id_field() {
        let value = serde_json::to_value(IdOnly {
            id: "ub-1".to_string(),
        })
        .unwrap();
        assert_eq!(value["id"], "ub-1");
    }

    #[test]
    fn issue_output_id_is_wire_identical_to_id_only() {
        // `#[serde(untagged)]` ⇒ the `Id` arm serializes exactly as the inner `IdOnly`.
        let via_union = serde_json::to_value(IssueOutput::Id(IdOnly {
            id: "ub-1".to_string(),
        }))
        .unwrap();
        let direct = serde_json::to_value(IdOnly {
            id: "ub-1".to_string(),
        })
        .unwrap();
        assert_eq!(via_union, direct);
    }

    #[test]
    fn issue_output_delete_matches_the_old_delete_plan_json() {
        // `to_value` is the wire arbiter (`ok_json` routes every payload through it). The typed
        // `DeletePlanOutput` must Value-equal the old ad-hoc `delete_plan_json` object, incl. every
        // mode string.
        for (mode, expected_mode) in [
            (DeleteMode::Tombstone, "tombstone"),
            (DeleteMode::Cascade, "cascade"),
            (DeleteMode::Hard, "hard"),
            (DeleteMode::DryRun, "dry_run"),
        ] {
            let typed = serde_json::to_value(IssueOutput::Delete(DeletePlanOutput {
                mode: DeleteModeOutput::from(mode),
                targets: vec!["ub-a".to_string()],
                cascade_children: vec!["ub-b".to_string()],
            }))
            .unwrap();
            let old = serde_json::json!({
                "mode": expected_mode,
                "targets": ["ub-a"],
                "cascade_children": ["ub-b"],
            });
            assert_eq!(typed, old, "mode {expected_mode} must be wire-identical");
        }
    }

    #[test]
    fn dep_output_added_and_removed_match_the_old_json() {
        let added = serde_json::to_value(DepOutput::Added(DepAdded { added: true })).unwrap();
        assert_eq!(added, serde_json::json!({ "added": true }));

        let removed =
            serde_json::to_value(DepOutput::Removed(DepRemoved { removed: true })).unwrap();
        assert_eq!(removed, serde_json::json!({ "removed": true }));
    }

    #[test]
    fn sync_output_export_tags_snake_case() {
        let value = serde_json::to_value(SyncOutput::Export(ExportReport {
            written: 3,
            path: PathBuf::from("/tmp/x.jsonl"),
        }))
        .unwrap();
        assert_eq!(value["export"]["written"], 3);
    }

    #[test]
    fn sync_output_import_tags_snake_case() {
        let value = serde_json::to_value(SyncOutput::Import(ImportReport {
            imported: 2,
            skipped: 1,
            dependencies: 0,
            comments: 0,
            dropped_fields: vec![],
        }))
        .unwrap();
        assert_eq!(value["import"]["imported"], 2);
    }
}
