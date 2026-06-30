//! Tool **#6 `sync`** — JSONL/bd interchange (spine §5.1/§5.2, FR-7/8/26).
//!
//! Maps `SyncInput{action}` to `Session::{export_jsonl, import_jsonl, import_bd}`. The export path
//! defaults to `.unblock/issues.jsonl`; path-confinement + conflict-marker rejection are enforced in
//! `unblock-sync`/the engine, surfaced here as `PathTraversal`/`ConflictMarkers`/`JsonlParseError`.
//!
//! **v1 seam:** the engine sync methods return `EngineError::FeatureNotWired{"sync"}` until T2.4 — so
//! this tool surfaces that as a clean in-band structured error today; T2.4 lands the real reports
//! without any change to this adapter (the `SyncOutput` wrapping is already wired).

use std::path::PathBuf;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::tool;
use serde::{Deserialize, Serialize};
use unblock_engine::ImportOptions;

use crate::server::UnblockServer;
use crate::tools::output::SyncOutput;
use crate::tools::{engine_err_json, err_json, ok_json};

/// The `sync` tool input (spine §5.2 — EXACT shape).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub(crate) enum SyncInput {
    /// Export the store to JSONL (default path `.unblock/issues.jsonl`).
    Export {
        /// Optional export path (path-confined to the workspace).
        #[serde(default)]
        path: Option<String>,
    },
    /// Import JSONL.
    Import {
        /// The JSONL path to import.
        path: String,
        /// Plan-only: report the import without writing.
        #[serde(default)]
        dry_run: bool,
    },
    /// One-shot, idempotent `bd` import (D16).
    ImportBd {
        /// The `bd` export path to import.
        path: String,
    },
}

#[rmcp::tool_router(router = sync_router, vis = "pub(crate)")]
impl UnblockServer {
    /// JSONL/bd interchange (FR-7/8/26).
    #[tool(
        name = "sync",
        description = "Export/import the issue store as JSONL, or one-shot import a bd export."
    )]
    pub(crate) async fn sync(&self, Parameters(input): Parameters<SyncInput>) -> CallToolResult {
        if let Err(structured) = self.preflight(&input) {
            return err_json(&structured);
        }
        match input {
            SyncInput::Export { path } => {
                let path = self.resolve_jsonl_path(path);
                match self.session.export_jsonl(&path).await {
                    Ok(report) => ok_json(&SyncOutput::Export(report)),
                    Err(err) => engine_err_json(&err),
                }
            }
            SyncInput::Import { path, dry_run } => {
                let opts = ImportOptions { dry_run };
                match self.session.import_jsonl(&PathBuf::from(path), opts).await {
                    Ok(report) => ok_json(&SyncOutput::Import(report)),
                    Err(err) => engine_err_json(&err),
                }
            }
            SyncInput::ImportBd { path } => {
                match self.session.import_bd(&PathBuf::from(path)).await {
                    Ok(report) => ok_json(&SyncOutput::Import(report)),
                    Err(err) => engine_err_json(&err),
                }
            }
        }
    }

    /// Resolve the export path: an explicit path, or the default `<workspace>/.unblock/issues.jsonl`.
    ///
    /// Path-confinement is enforced downstream in `unblock-sync`/the engine (surfaced as
    /// `PathTraversal`), so this only computes the default — it does not validate confinement.
    fn resolve_jsonl_path(&self, path: Option<String>) -> PathBuf {
        match path {
            Some(p) => PathBuf::from(p),
            None => self
                .session
                .workspace_dir()
                .join(".unblock")
                .join("issues.jsonl"),
        }
    }
}
