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
    /// `[Backlog, Ready, In Progress, Blocked, Deferred, Closed]`
    /// (`TitleCase`, sourced from `Status::option_name`; see spec §5.7).
    /// Empty when every existing single-select required field already
    /// matched the spec exactly. See bead unblock-aa2 for the auto-heal
    /// contract and bead unblock-1zj for the rename + Backlog default.
    pub fields_healed: Vec<String>,
    /// Canonical names of fields that already existed and matched the
    /// spec — no mutation issued.
    pub fields_existing: Vec<String>,
    /// Canonical names of fields that are missing and would be created by a
    /// non-dry-run call. Always empty when `dry_run` is `false` (the fields
    /// were already created).
    pub fields_missing: Vec<String>,
    /// Canonical names of org-level GitHub `IssueType`s that were
    /// CREATED (not pre-existing) by this `setup` run, e.g.
    /// `["Spike", "Epic", "Chore", "Refactor", "Docs"]`. Sourced from
    /// `IssueType::canonical_name` (spec §2.6).
    ///
    /// Empty vector when:
    /// - All eight canonical types already existed on the org, OR
    /// - The repo owner is a `User` (GitHub's native issue types are
    ///   org-level only — `setup_fields` skips the step for User
    ///   accounts and emits an info-level log line).
    ///
    /// Mirrors the `fields_created` / `fields_healed` / `fields_existing`
    /// buckets above. Introduced by `unblock-wgj` per spec §8.10 + §5.7.
    pub issue_types_created: Vec<String>,
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

/// The `://ready` view filter: `"Status":"Ready"`.
///
/// Sourced from [`unblock_core::types::Status::option_name`] per spec
/// §5.8. The filter wraps the helper's `Ready` output in the Projects
/// V2 saved-view filter syntax via a const concatenation so no
/// hand-rolled `"Ready"` literal exists in this module. The §5.7 ↔
/// §5.8 round-trip is closed by the unit test
/// `ready_view_filter_matches_status_helper` below.
const READY_VIEW_FILTER: &str = const_concat_ready_view_filter();

const fn const_concat_ready_view_filter() -> &'static str {
    // `concat!` requires literal `&str` arguments and cannot reference
    // a `const fn` call result; instead we emit the bytes via a
    // hand-rolled byte-array assembly that operates at compile time.
    // `Status::Ready.option_name()` is a `const fn`, so the splice
    // happens entirely at const eval. The helper-name unit test
    // `ready_view_filter_matches_status_helper` (see below) verifies
    // the runtime result still equals the expected wire string —
    // catching any future drift in `option_name`.
    const PREFIX: &[u8] = b"\"Status\":\"";
    const SUFFIX: &[u8] = b"\"";
    const NAME: &[u8] = unblock_core::types::Status::Ready.option_name().as_bytes();
    const LEN: usize = PREFIX.len() + NAME.len() + SUFFIX.len();
    const fn build() -> [u8; LEN] {
        let mut out = [0u8; LEN];
        let mut i = 0;
        let mut j = 0;
        while j < PREFIX.len() {
            out[i] = PREFIX[j];
            i += 1;
            j += 1;
        }
        let mut j = 0;
        while j < NAME.len() {
            out[i] = NAME[j];
            i += 1;
            j += 1;
        }
        let mut j = 0;
        while j < SUFFIX.len() {
            out[i] = SUFFIX[j];
            i += 1;
            j += 1;
        }
        out
    }
    const FILTER_BYTES: [u8; LEN] = build();
    // `from_utf8` is const-stable; the inputs are pure ASCII so this is
    // infallible — `unwrap()` in const context.
    match std::str::from_utf8(&FILTER_BYTES) {
        Ok(s) => s,
        Err(_) => panic!("READY_VIEW_FILTER bytes are not valid UTF-8 — bug"),
    }
}

/// The 5 pre-configured views required by the setup tool.
///
/// Each view follows the naming convention `://name` to distinguish
/// unblock-managed views from user-created ones.
pub const REQUIRED_VIEWS: &[ViewSpec] = &[
    ViewSpec {
        name: "://ready",
        layout: ViewLayout::Board,
        // Filter sourced from `Status::option_name` per spec §5.8 —
        // see [`READY_VIEW_FILTER`] for the round-trip closure.
        filter: Some(READY_VIEW_FILTER),
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
