//! Setup tool — creates required Projects V2 custom fields (idempotent).
//!
//! This is typically the first tool an agent calls on a fresh repository.
//! It ensures the 7 required Projects V2 fields exist on the configured project,
//! and reports which fields were created vs. skipped.
//!
//! Supports a `dry_run` mode that queries field presence without mutating.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Input parameters for the `setup` MCP tool.
///
/// Both fields are optional: `project` overrides the configured project number,
/// and `dry_run` controls whether fields are actually created or just inspected.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetupParams {
    /// Optional project number override. If omitted, uses the configured
    /// `UNBLOCK_PROJECT` value.
    ///
    /// **Note:** This parameter is accepted but not yet wired — the configured
    /// project number is always used. If provided, a warning is logged.
    pub project: Option<u32>,
    /// If `true`, report which fields exist and which are missing without
    /// creating anything. Defaults to `false`.
    pub dry_run: Option<bool>,
}

/// Result returned by the `setup` MCP tool.
///
/// Contains the canonical names of fields that were created, skipped, and
/// (in dry-run mode) missing, plus the project's GraphQL node ID for reference.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SetupResult {
    /// Canonical names of fields that were newly created (e.g. `["Agent", "DeferUntil"]`).
    pub fields_created: Vec<String>,
    /// Canonical names of fields that already existed and were skipped.
    pub fields_skipped: Vec<String>,
    /// Canonical names of fields that are missing and would be created by a
    /// non-dry-run call. Always empty when `dry_run` is `false` (the fields
    /// were already created).
    pub fields_missing: Vec<String>,
    /// The GraphQL node ID of the project.
    pub project_id: String,
}
