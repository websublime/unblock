//! Setup tool — configures Projects V2 fields and views (idempotent).
//!
//! This is typically the first tool an agent calls on a fresh repository
//! (after `init`). It ensures the 7 required Projects V2 fields and 5
//! pre-configured views exist on the configured project, and reports which
//! were created vs. already present.
//!
//! Supports a `dry_run` mode that queries field/view presence without mutating.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use unblock_github::projects::ViewLayout;

/// Input parameters for the `setup` MCP tool.
///
/// Both fields are optional: `project` overrides the configured project number,
/// and `dry_run` controls whether fields/views are actually created or just
/// inspected.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetupParams {
    /// Optional project number override. If omitted, uses the configured
    /// `UNBLOCK_PROJECT` value.
    ///
    /// **Note:** This parameter is accepted but not yet wired — the configured
    /// project number is always used. If provided, a warning is logged.
    pub project: Option<u64>,
    /// If `true`, report which fields/views exist and which are missing without
    /// creating anything. Defaults to `false`.
    pub dry_run: Option<bool>,
}

/// Result returned by the `setup` MCP tool.
///
/// Contains the canonical names of fields and views that were created,
/// healed (option-set reconciled in place), or already existed, plus
/// (in dry-run mode) which fields are missing and the project number.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SetupResult {
    /// Canonical names of fields that were newly created (e.g. `["Agent", "DeferUntil"]`).
    pub fields_created: Vec<String>,
    /// Canonical names of single-select required fields whose option set
    /// diverged from the spec and was reconciled in place via
    /// `updateProjectV2Field`. Most commonly this is the GitHub-default
    /// built-in `Status` field on a fresh project: its options
    /// `[Todo, In Progress, Done]` get rewritten to the spec's canonical
    /// `[ready, in_progress, blocked, deferred, closed]`. Empty when
    /// every existing single-select required field already matched the
    /// spec exactly. See bead unblock-aa2 for the auto-heal contract.
    pub fields_healed: Vec<String>,
    /// Canonical names of fields that already existed and matched the
    /// spec — no mutation issued.
    pub fields_existing: Vec<String>,
    /// Canonical names of fields that are missing and would be created by a
    /// non-dry-run call. Always empty when `dry_run` is `false` (the fields
    /// were already created).
    pub fields_missing: Vec<String>,
    /// Names of views that were newly created (e.g. `["://ready", "://team"]`).
    pub views_created: Vec<String>,
    /// Names of views that already existed and were skipped.
    pub views_existing: Vec<String>,
    /// The project number.
    pub project_number: u64,
    /// Whether this was a dry-run (no mutations were performed).
    pub dry_run: bool,
}

/// Specification for a required project view.
///
/// Defines the name, layout, and optional filter for each view that the
/// setup tool creates on a project. The 5 required views are defined in
/// [`REQUIRED_VIEWS`].
#[derive(Debug, Clone)]
pub struct ViewSpec {
    /// View display name (e.g. `"://ready"`).
    pub name: &'static str,
    /// View layout type.
    pub layout: ViewLayout,
    /// Optional filter query string.
    pub filter: Option<&'static str>,
}

/// The 5 pre-configured views required by the setup tool.
///
/// Each view follows the naming convention `://name` to distinguish
/// unblock-managed views from user-created ones.
pub const REQUIRED_VIEWS: &[ViewSpec] = &[
    ViewSpec {
        name: "://ready",
        layout: ViewLayout::Board,
        filter: Some("\"Status\":\"ready\""),
    },
    ViewSpec {
        name: "://team",
        layout: ViewLayout::Board,
        filter: None,
    },
    ViewSpec {
        name: "://pipeline",
        layout: ViewLayout::Table,
        filter: None,
    },
    ViewSpec {
        name: "://roadmap",
        layout: ViewLayout::Roadmap,
        filter: None,
    },
    ViewSpec {
        name: "://timeline",
        layout: ViewLayout::Roadmap,
        filter: None,
    },
];
