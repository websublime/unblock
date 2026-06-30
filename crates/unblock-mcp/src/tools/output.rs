//! MCP-owned output wrapper types (spine §5.3 — F-4: they exist NOWHERE else).
//!
//! [`IdOnly`] and [`SyncOutput`] are declared **here** (mcp-owned); they are NOT re-exported from
//! `unblock-model`. The report DTOs they wrap (`ExportReport`/`ImportReport`) ARE model §1.10 types,
//! sourced from `unblock-model`.

use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use unblock_model::{ExportReport, ImportReport};

/// The quick-create output: the minted id only (spine §5.3 `ToolOutput::Id`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct IdOnly {
    /// The minted issue id.
    pub id: String,
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
    use super::{IdOnly, SyncOutput};
    use std::path::PathBuf;
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
            dropped_fields: vec![],
        }))
        .unwrap();
        assert_eq!(value["import"]["imported"], 2);
    }
}
