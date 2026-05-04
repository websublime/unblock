//! Projects V2 field and view management.
//!
//! - `resolve_project()` — find linked project by repo
//! - `setup_fields()` — create 7 custom fields (idempotent), returns [`SetupReport`](crate::projects::SetupReport)
//! - `query_setup_status()` — check which fields exist without mutating (for dry-run)
//! - `update_field()` — update a single field value on an issue
//! - `detect_owner_type()` — determine if the repo owner is an org or a user
//! - `list_rest_fields()` — list project fields via REST (integer IDs for view configuration)
//! - `create_view()` — create a project view via REST
//! - `list_views()` — list project views via GraphQL

use std::collections::HashMap;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use snafu::ResultExt as _;
use tracing::{debug, instrument, warn};
use unblock_core::types::{IssueType, Status};

use crate::client::GitHubClient;
use crate::errors::{self, Error};
use crate::graphql::check_rest_response;

/// Information about a resolved GitHub Projects V2 project.
#[derive(Debug, Clone)]
pub struct ProjectInfo {
    /// The GraphQL global node ID for the project.
    pub id: String,
    /// The project number (visible in the GitHub UI).
    pub number: u32,
}

/// Cached IDs for Projects V2 custom fields.
///
/// Resolved once at startup or on first `setup_fields()` call, then cached
/// on [`GitHubClient`] to avoid repeated GraphQL lookups.
#[derive(Debug, Clone)]
pub struct ProjectFieldIds {
    /// Status field — single select with the 6 canonical `TitleCase` options
    /// in board order: `Backlog`, `Ready`, `In Progress`, `Blocked`,
    /// `Deferred`, `Closed`. Sourced from `Status::option_name` per spec §5.7.
    pub status: FieldMeta,
    /// Priority field — single select: P0 - Critical, P1 - High, P2 - Medium, P3 - Low, P4 - Backlog.
    pub priority: FieldMeta,
    /// `PipelineStage` field — single select: investigation, implementation, review, refactoring, qa, done.
    pub pipeline_stage: FieldMeta,
    /// Agent field — text field (node ID only, no options).
    pub agent: String,
    /// `ClaimedAt` field — date field (node ID only).
    pub claimed_at: String,
    /// `StoryPoints` field — number field (node ID only).
    pub story_points: String,
    /// `DeferUntil` field — date field (node ID only).
    pub defer_until: String,
}

/// Metadata for a single-select Projects V2 field.
///
/// Contains the field's node ID and a map from option display name to option
/// node ID, enabling the caller to resolve an option name (e.g. `"P1 - High"`)
/// to the GraphQL ID required by `updateProjectV2ItemFieldValue`.
///
/// `option_colors` carries the GraphQL `ProjectV2SingleSelectFieldOptionColor`
/// value (e.g. `"BLUE"`, `"YELLOW"`, `"GRAY"`) for each option whose color
/// the surrounding API surface produced. It exists primarily so that the
/// auto-heal path in [`GitHubClient::setup_fields`] can preserve operator-
/// chosen colours through an `updateProjectV2Field` rewrite instead of
/// flattening every option to `GRAY`. Callers that don't care about colour
/// can ignore this map — it is populated on a best-effort basis (empty for
/// plain Text/Number/Date fields, partially populated when the upstream
/// response omits a colour for some option).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct FieldMeta {
    /// GraphQL node ID for the field.
    pub field_id: String,
    /// Map of option display name to option node ID.
    pub options: HashMap<String, String>,
    /// Map of option display name to its GraphQL colour enum value
    /// (e.g. `"BLUE"`, `"GRAY"`). Empty for non-single-select fields and
    /// for options whose colour the upstream response did not surface.
    pub option_colors: HashMap<String, String>,
}

impl FieldMeta {
    /// Constructs a `FieldMeta` from just the field ID and option name → ID
    /// map. Initialises `option_colors` to an empty map.
    ///
    /// This is the primary constructor for downstream crates because
    /// `FieldMeta` is `#[non_exhaustive]` and cannot be built via a struct
    /// literal from outside `unblock-github`. Use
    /// [`with_option_colors`](Self::with_option_colors) if colour metadata
    /// is also available.
    #[must_use]
    pub fn new(field_id: String, options: HashMap<String, String>) -> Self {
        Self {
            field_id,
            options,
            option_colors: HashMap::new(),
        }
    }

    /// Builder-style setter that attaches an option name → colour map.
    ///
    /// The colour map is consumed by the auto-heal path in
    /// [`GitHubClient::setup_fields`] when reconciling a single-select
    /// required field's option set, so operator-chosen colours survive
    /// idempotent re-runs (bead unblock-aa2 finding S1).
    #[must_use]
    pub fn with_option_colors(mut self, colors: HashMap<String, String>) -> Self {
        self.option_colors = colors;
        self
    }

    /// Looks up an option ID by exact name, falling back to prefix match.
    ///
    /// This enables callers to pass short codes like `"P0"` which match
    /// the full option name `"P0 - Critical"` in the Projects V2 field.
    /// Returns `None` if no match is found.
    #[must_use]
    pub fn option_id_by_prefix(&self, prefix: &str) -> Option<&String> {
        self.options.get(prefix).or_else(|| {
            // Prefix match: find the first option whose name starts with the
            // given prefix and is strictly longer (e.g. "P0" matches "P0 - Critical").
            self.options
                .iter()
                .find(|(name, _)| name.starts_with(prefix) && name.len() > prefix.len())
                .map(|(_, id)| id)
        })
    }
}

/// A value to set on a Projects V2 field.
///
/// Each variant maps to a different input shape in the
/// `updateProjectV2ItemFieldValue` GraphQL mutation.
#[derive(Debug, Clone)]
pub enum FieldValue {
    /// A single-select option, identified by its GraphQL option ID.
    SingleSelectOption(String),
    /// A free-text value.
    Text(String),
    /// A numeric value.
    Number(f64),
    /// A date value (serialized as ISO 8601 YYYY-MM-DD for the GraphQL API).
    Date(NaiveDate),
}

/// The canonical names of the 7 required Projects V2 custom fields.
///
/// Used by the setup tool to report which fields were created vs. skipped,
/// and by the dry-run mode to check field presence without mutating.
///
/// Derived at compile time from `REQUIRED_FIELDS` (private) so the two lists cannot
/// drift: adding, removing, or renaming a field in `REQUIRED_FIELDS`
/// automatically updates this slice.
pub const REQUIRED_FIELD_NAMES: &[&str] = &required_field_names();

/// Compile-time derivation of the canonical field name list from
/// `REQUIRED_FIELDS` (private). Kept private — callers use `REQUIRED_FIELD_NAMES`.
const fn required_field_names() -> [&'static str; REQUIRED_FIELDS.len()] {
    let mut names = [""; REQUIRED_FIELDS.len()];
    let mut i = 0;
    while i < REQUIRED_FIELDS.len() {
        names[i] = REQUIRED_FIELDS[i].name;
        i += 1;
    }
    names
}

/// Result of a `setup_fields()` call, including which fields were created,
/// healed (option set reconciled in place), and skipped (already existed
/// and matched the spec).
///
/// This is the enriched return type that enables the MCP setup tool to report
/// per-field setup status to the agent.
///
/// **Buckets are mutually exclusive.** Each canonical field name appears in
/// exactly one of `created`, `healed`, or `skipped` — `created` for fields
/// that did not exist (fresh `createProjectV2Field`), `healed` for fields
/// whose option set diverged from the spec and was reconciled in place via
/// `updateProjectV2Field` (single-select required fields only), and
/// `skipped` for fields that already matched the spec (no mutation issued).
///
/// Per CLAUDE.md `#[non_exhaustive]` discipline: marked `non_exhaustive` so a
/// future bucket (e.g. `renamed` for option-rename heuristics) can be added
/// without coordinating with downstream consumers. Construct via the
/// fully-qualified struct literal — pre-1.0, no users, internal-only.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SetupReport {
    /// The resolved field IDs for all 7 required fields.
    pub field_ids: ProjectFieldIds,
    /// Canonical names of fields that were newly created via
    /// `createProjectV2Field` (the field did not exist on the project).
    pub created: Vec<String>,
    /// Canonical names of fields whose option set was reconciled in place
    /// via `updateProjectV2Field` because the existing options diverged
    /// from the spec (e.g. the GitHub-default built-in `Status` field with
    /// options `[Todo, In Progress, Done]` was healed to the spec's
    /// canonical `[ready, in_progress, blocked, deferred, closed]`). Only
    /// single-select required fields are eligible for the heal path —
    /// plain Text/Number/Date fields have no option drift to reconcile and
    /// always land in `skipped` when present.
    pub healed: Vec<String>,
    /// Canonical names of fields that already existed and matched the
    /// spec (no mutation issued — pure idempotent hit on the existing
    /// field). For single-select fields this means the option set was
    /// already an exact match; for plain fields this means the field
    /// existed with the right `dataType`.
    pub skipped: Vec<String>,
    /// Canonical names of org-level GitHub `IssueTypes` that were
    /// CREATED (not pre-existing) by the `IssueType` ensure-and-heal
    /// step in [`GitHubClient::setup_fields`] (spec §5.7 step 3).
    ///
    /// Empty vector when:
    /// - All eight canonical types (`Task`, `Bug`, `Feature`, `Spike`,
    ///   `Epic`, `Chore`, `Refactor`, `Docs`) already existed on the
    ///   org, OR
    /// - The repo owner is a `User` (GitHub's native issue types are
    ///   org-level only — the step is a no-op for user-owned repos
    ///   and `setup_fields` emits an info-level log line in that
    ///   branch).
    ///
    /// Mirrors the `created` / `skipped` / `healed` buckets above so
    /// downstream tooling (`SetupResult` in `unblock-mcp::tools::setup`)
    /// can disclose the `IssueType` ensure-and-heal outcome distinctly
    /// from Projects V2 field creation.
    ///
    /// Introduced by `unblock-wgj` — additive `pub` API change in the
    /// `unblock-github` crate.
    pub issue_types_created: Vec<String>,
}

/// Status of the 7 required fields on a project, without mutating anything.
///
/// Returned by [`GitHubClient::query_setup_status`] for dry-run inspection.
#[derive(Debug, Clone)]
pub struct SetupStatus {
    /// Field names that already exist on the project.
    pub existing: Vec<String>,
    /// Field names that are missing and would be created by `setup_fields()`.
    pub missing: Vec<String>,
}

/// Whether the repository owner is a GitHub organization or a personal user.
///
/// Determines the REST API URL path segment (`/orgs/{owner}/` vs
/// `/users/{owner}/`) for Projects V2 view and field endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerType {
    /// A GitHub organization account.
    Org,
    /// A personal GitHub user account.
    User,
}

impl OwnerType {
    /// Returns the REST API path prefix for this owner type.
    ///
    /// - [`OwnerType::Org`] returns `"orgs"`
    /// - [`OwnerType::User`] returns `"users"`
    #[must_use]
    pub fn path_segment(self) -> &'static str {
        match self {
            Self::Org => "orgs",
            Self::User => "users",
        }
    }
}

/// Layout type for a Projects V2 view.
///
/// Maps to the REST API `layout` field values: `"board"`, `"table"`, `"roadmap"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewLayout {
    /// Kanban board layout.
    Board,
    /// Spreadsheet table layout.
    Table,
    /// Timeline/roadmap layout.
    Roadmap,
}

impl std::fmt::Display for ViewLayout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Board => write!(f, "board"),
            Self::Table => write!(f, "table"),
            Self::Roadmap => write!(f, "roadmap"),
        }
    }
}

/// An option value for a single-select REST field.
///
/// The REST API (version 2026-03-10) returns `options[].name` as a nested
/// object `{"raw": "...", "html": "..."}` rather than a plain string.
/// This struct captures the raw display name after parsing.
#[derive(Debug, Clone)]
pub struct RestFieldOption {
    /// The raw display name of the option (extracted from `name.raw`).
    pub name: String,
    /// The option color (e.g. `"RED"`, `"BLUE"`).
    pub color: String,
    /// The option description.
    pub description: String,
}

/// A Projects V2 field as returned by the REST API.
///
/// Unlike GraphQL field nodes which use string node IDs (`PVTF_...`), REST
/// fields use integer `id` values. These integer IDs are required for the
/// `visible_fields` parameter when creating views.
#[derive(Debug, Clone)]
pub struct RestField {
    /// Integer field ID (used in `visible_fields` for view creation).
    pub id: u64,
    /// Display name of the field.
    pub name: String,
    /// Data type string (e.g. `"single_select"`, `"text"`, `"number"`, `"date"`,
    /// `"title"`, `"assignees"`, etc.).
    pub data_type: String,
    /// Options for single-select fields. Empty for other field types.
    pub options: Vec<RestFieldOption>,
}

/// Parameters for creating a Projects V2 view via the REST API.
///
/// Corresponds to the request body for `POST /orgs|users/{owner}/projectsV2/{n}/views`.
#[derive(Debug, Clone, Serialize)]
pub struct CreateViewParams {
    /// View name (displayed in the project tab bar).
    pub name: String,
    /// View layout type.
    pub layout: ViewLayout,
    /// Optional filter query string (same syntax as the GitHub Projects UI filter bar).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    /// Optional list of integer field IDs to show as visible columns.
    /// Not supported for roadmap layout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visible_fields: Option<Vec<u64>>,
}

/// A Projects V2 view, returned by both REST (create) and GraphQL (list).
///
/// Contains the view metadata needed for idempotency checks and reporting.
#[derive(Debug, Clone)]
pub struct ProjectView {
    /// Integer view ID (from REST).
    pub id: Option<u64>,
    /// View number within the project.
    pub number: u64,
    /// View display name.
    pub name: String,
    /// View layout type.
    pub layout: ViewLayout,
    /// GraphQL global node ID (e.g. `"PVV_..."`).
    pub node_id: Option<String>,
    /// Optional filter query string.
    pub filter: Option<String>,
    /// Visible field integer IDs (from REST response).
    pub visible_fields: Vec<u64>,
}

/// A Projects V2 project summary returned by [`GitHubClient::list_owner_projects`].
#[derive(Debug, Clone)]
pub struct OwnerProject {
    /// The project number (visible in the GitHub UI URL).
    pub number: u64,
    /// The project title.
    pub title: String,
    /// The project URL (e.g. `https://github.com/orgs/acme/projects/1`).
    pub url: String,
}

/// Result of a successful [`GitHubClient::create_project`] call.
#[derive(Debug, Clone)]
pub struct CreatedProject {
    /// The project number (visible in the GitHub UI URL).
    pub number: u64,
    /// The project URL.
    pub url: String,
}

/// The GitHub REST API version header value required by several modern REST
/// surfaces.
///
/// This version is newer than the default `2022-11-28` and must be sent as
/// a per-request header override for the following endpoints:
///
/// - `/projectsV2/*/views` (list/create views)
/// - `/projectsV2/*/fields` (list fields)
/// - `/orgs/{org}/issue-types` (list/create org-level issue types)
///
/// Empirically, the `/orgs/{org}/issue-types` endpoint returns HTTP 403
/// "Resource not accessible by personal access token" when called with the
/// default `2022-11-28` header even on tokens with `Issue Types: R/W` /
/// `admin:org`; explicitly sending `X-GitHub-Api-Version: 2026-03-10`
/// returns the expected 200 response.
///
/// The name `VIEWS_API_VERSION` is preserved for callsite stability — the
/// constant now serves both Projects V2 view/field endpoints and the
/// org-level issue-types endpoints.
const VIEWS_API_VERSION: &str = "2026-03-10";

/// Specification for a required Projects V2 field.
///
/// Used internally by [`setup_fields`](GitHubClient::setup_fields) to describe
/// the 7 required fields and their expected types and options.
struct FieldSpec {
    /// Display name of the field in the project board.
    name: &'static str,
    /// GraphQL `ProjectV2CustomFieldType` value.
    data_type: &'static str,
    /// For single-select fields, the required option names in display order.
    /// Empty for text, number, and date fields.
    options: &'static [&'static str],
}

/// Normalise a Projects V2 single-select option name for the auto-heal
/// matcher (spec §5.7).
///
/// Pipeline: trim outer whitespace → lowercase → replace each `_` with
/// a single space → collapse runs of internal whitespace to one space.
/// The result is a comparable canonical key used by
/// [`GitHubClient::heal_select_field_options`] to reuse existing option
/// GraphQL IDs across the `unblock-1zj` lowercase → `TitleCase` rename.
///
/// Examples:
///
/// - `"in_progress"` → `"in progress"`
/// - `"In Progress"` → `"in progress"`
/// - `"IN_PROGRESS"` → `"in progress"`
/// - `"  Backlog  "` → `"backlog"`
/// - `"ready"` → `"ready"`
fn normalize_option_name(raw: &str) -> String {
    let lowered = raw.trim().to_ascii_lowercase();
    let with_spaces = lowered.replace('_', " ");
    // Collapse internal whitespace runs to a single space without an
    // extra dependency. The spec ASCII-only canonical strings keep this
    // implementation byte-oriented.
    with_spaces.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Canonical Projects V2 Status option names in board order.
///
/// **Single source of truth — generated from [`Status::ALL`].** Per spec
/// §5.7 the `REQUIRED_FIELDS` Status spec MUST be derived from the
/// `Status` enum (no duplicated literal list). `Status::option_name` is
/// a `const fn`, so this `const` array is materialized at compile time
/// and remains in lock-step with the enum: adding a new `Status` variant
/// updates this list automatically.
///
/// The board order mirrors `Status::ALL` and is the contract consumed by
/// the `REQUIRED_FIELDS` Status entry, the `UNBLOCK://ready` view filter,
/// and the auto-heal matcher in [`GitHubClient::heal_select_field_options`].
const STATUS_OPTION_NAMES: [&str; Status::ALL.len()] = {
    let mut out: [&str; Status::ALL.len()] = [""; Status::ALL.len()];
    let mut i = 0;
    while i < Status::ALL.len() {
        out[i] = Status::ALL[i].option_name();
        i += 1;
    }
    out
};

/// Specification for one canonical org-level GitHub `IssueType`.
///
/// Drives the `IssueType` ensure-and-heal step in
/// [`GitHubClient::setup_fields`](super::client::GitHubClient::setup_fields).
/// Each entry pairs a canonical name with a color and description sourced
/// directly from the [`IssueType`] enum's compile-time helpers (spec §2.6).
///
/// The struct is private — callers consume the const array
/// [`REQUIRED_ISSUE_TYPES`] which is generated from the [`IssueType`] enum
/// at compile time. See spec §5.7 + Appendix B.1 Decision 2 for the
/// single-source-of-truth discipline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IssueTypeSpec {
    /// Canonical `TitleCase` name (e.g. `"Task"`, `"Refactor"`).
    name: &'static str,
    /// GitHub REST `issue-types` color (lowercase, e.g. `"yellow"`,
    /// `"pink"`); see [`IssueType::canonical_color`] for the closed set.
    color: &'static str,
    /// Human-readable short description.
    description: &'static str,
}

/// Canonical org-level GitHub `IssueType` taxonomy.
///
/// **Single source of truth — generated from `IssueType::ALL`.** Per spec
/// §5.7 the ensure-and-heal loop in `setup_fields` MUST iterate this
/// constant in declared order. The array is materialised at compile
/// time from [`IssueType::ALL`] paired with the §2.6 helpers
/// [`IssueType::canonical_name`], [`IssueType::canonical_color`], and
/// [`IssueType::canonical_description`] — adding a new variant to
/// [`IssueType`] (allowed by `#[non_exhaustive]`) automatically extends
/// this list with no second-list bookkeeping.
///
/// Order mirrors [`IssueType::ALL`] (Task, Bug, Feature, Spike, Epic,
/// Chore, Refactor, Docs) and is the contract consumed by
/// `setup_fields`'s declared-order create loop.
///
/// **Discipline (Invariant 17, §14).** No literal `IssueType` name, color,
/// or description string is permitted in the workspace outside the
/// `IssueType::canonical_*` definition site and its unit tests; every
/// consumer (this constant, the `create`/`update` validators, the
/// GraphQL deserialiser) routes through the helpers.
const REQUIRED_ISSUE_TYPES: [IssueTypeSpec; IssueType::ALL.len()] = {
    let mut out: [IssueTypeSpec; IssueType::ALL.len()] = [IssueTypeSpec {
        name: "",
        color: "",
        description: "",
    }; IssueType::ALL.len()];
    let mut i = 0;
    while i < IssueType::ALL.len() {
        let variant = IssueType::ALL[i];
        out[i] = IssueTypeSpec {
            name: variant.canonical_name(),
            color: variant.canonical_color(),
            description: variant.canonical_description(),
        };
        i += 1;
    }
    out
};

/// The 7 required Projects V2 custom fields per spec §5.
const REQUIRED_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "Status",
        data_type: "SINGLE_SELECT",
        // Sourced from `Status::option_name` via `STATUS_OPTION_NAMES`
        // (spec §5.7 single-source-of-truth requirement).
        options: &STATUS_OPTION_NAMES,
    },
    FieldSpec {
        name: "Priority",
        data_type: "SINGLE_SELECT",
        options: &[
            "P0 - Critical",
            "P1 - High",
            "P2 - Medium",
            "P3 - Low",
            "P4 - Backlog",
        ],
    },
    FieldSpec {
        name: "PipelineStage",
        data_type: "SINGLE_SELECT",
        options: &[
            "investigation",
            "implementation",
            "review",
            "refactoring",
            "qa",
            "done",
        ],
    },
    FieldSpec {
        name: "Agent",
        data_type: "TEXT",
        options: &[],
    },
    FieldSpec {
        name: "ClaimedAt",
        data_type: "DATE",
        options: &[],
    },
    FieldSpec {
        name: "StoryPoints",
        data_type: "NUMBER",
        options: &[],
    },
    FieldSpec {
        name: "DeferUntil",
        data_type: "DATE",
        options: &[],
    },
];

/// Deserialization helper for querying existing project fields.
#[derive(Debug, Deserialize)]
struct FieldNode {
    id: String,
    name: String,
    /// Preserved for future field-type validation (e.g. verifying an existing
    /// "Status" field is actually `SINGLE_SELECT` and not `TEXT`).
    #[serde(default)]
    #[serde(rename = "dataType")]
    #[allow(dead_code)]
    data_type: Option<String>,
    #[serde(default)]
    options: Option<Vec<OptionNode>>,
}

/// Deserialization helper for single-select option nodes.
///
/// `color` is captured so that the auto-heal path can preserve operator-
/// chosen colours through an `updateProjectV2Field` rewrite (bead
/// unblock-aa2 finding S1). Older queries that don't surface `color` will
/// yield `None` here; the heal path treats `None` as "use a sensible
/// default" rather than "force GRAY".
#[derive(Debug, Deserialize)]
struct OptionNode {
    id: String,
    name: String,
    #[serde(default)]
    color: Option<String>,
}

/// Removes a single-select [`FieldMeta`] from the map, returning a GraphQL
/// error if the field was not resolved.
fn remove_field(map: &mut HashMap<String, FieldMeta>, name: &str) -> Result<FieldMeta, Error> {
    map.remove(name).ok_or_else(|| {
        errors::GitHubGraphQLSnafu {
            errors: vec![format!("Required field '{name}' was not resolved").into()],
        }
        .build()
    })
}

/// Removes a plain field ID from the map, returning a GraphQL error if the
/// field was not resolved.
fn remove_plain_field(map: &mut HashMap<String, String>, name: &str) -> Result<String, Error> {
    map.remove(name).ok_or_else(|| {
        errors::GitHubGraphQLSnafu {
            errors: vec![format!("Required field '{name}' was not resolved").into()],
        }
        .build()
    })
}

impl GitHubClient {
    /// Resolves the linked GitHub Projects V2 project for the configured repository.
    ///
    /// Queries the GitHub GraphQL API for the project matching the configured
    /// `project_number`. Returns [`ProjectInfo`] containing the project's global
    /// node ID and number.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ProjectNotConfigured`] if no project number is set.
    /// Returns [`Error::GitHubGraphQL`] if the project cannot be found.
    #[instrument(skip(self), fields(owner = %self.owner(), repo = %self.repo()))]
    pub async fn resolve_project_info(&self) -> Result<ProjectInfo, Error> {
        let project_number = self
            .project_number()
            .ok_or_else(|| errors::ProjectNotConfiguredSnafu.build())?;

        let query = "
            query FindProject($owner: String!, $repo: String!, $projectNumber: Int!) {
                repository(owner: $owner, name: $repo) {
                    projectV2(number: $projectNumber) {
                        id
                    }
                }
            }
        ";

        let variables = serde_json::json!({
            "owner": self.owner(),
            "repo": self.repo(),
            "projectNumber": project_number,
        });

        let response = self.graphql(query, variables).await?;
        let project_id = response["data"]["repository"]["projectV2"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_owned();

        if project_id.is_empty() {
            return Err(errors::GitHubGraphQLSnafu {
                errors: vec![
                    format!(
                        "Project V2 #{project_number} not found on {}/{}",
                        self.owner(),
                        self.repo()
                    )
                    .into(),
                ],
            }
            .build());
        }

        debug!(project_number, project_id = %project_id, "Resolved project V2");

        let number = u32::try_from(project_number).map_err(|_| {
            errors::GitHubGraphQLSnafu {
                errors: vec![format!("Project number {project_number} exceeds u32::MAX").into()],
            }
            .build()
        })?;

        Ok(ProjectInfo {
            id: project_id,
            number,
        })
    }

    /// Creates the 7 required custom fields on a Projects V2 project (idempotent).
    ///
    /// Queries existing fields first, then creates only those that are missing.
    /// For single-select fields, options are created as part of the field creation.
    /// Returns a [`SetupReport`] containing the resolved [`ProjectFieldIds`] along
    /// with lists of which fields were created and which were skipped.
    ///
    /// The caller should cache the field IDs via [`set_field_ids`](Self::set_field_ids)
    /// so subsequent calls to [`update_field`](Self::update_field) can resolve
    /// field/option IDs without another round-trip. This method does **not**
    /// cache automatically.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ProjectNotConfigured`] if no project number is set.
    /// Returns [`Error::GitHubGraphQL`] for GraphQL API errors.
    #[instrument(skip(self), fields(owner = %self.owner(), repo = %self.repo()))]
    pub async fn setup_fields(&self, project_id: &str) -> Result<SetupReport, Error> {
        // Step 1: Query existing fields on the project.
        let existing = self.fetch_existing_fields(project_id).await?;

        // Step 2: For each required field, either resolve existing or create new.
        let mut resolved: HashMap<String, FieldMeta> = HashMap::new();
        let mut resolved_plain: HashMap<String, String> = HashMap::new();
        let mut created: Vec<String> = Vec::new();
        let mut skipped: Vec<String> = Vec::new();

        let mut healed: Vec<String> = Vec::new();

        for spec in REQUIRED_FIELDS {
            if let Some(existing_field) = existing.get(spec.name) {
                debug!(field = spec.name, "Field already exists on the project");
                if spec.options.is_empty() {
                    // Plain field (Text/Number/Date) — no option set to
                    // reconcile, fall straight through to `skipped`.
                    skipped.push(spec.name.to_owned());
                    resolved_plain.insert(spec.name.to_owned(), existing_field.field_id.clone());
                } else {
                    // Auto-heal: when a single-select required field exists
                    // with a mismatched option set (e.g. the GitHub-default
                    // built-in `Status` field with options
                    // [Todo, In Progress, Done] vs. the spec's canonical
                    // [ready, in_progress, blocked, deferred, closed]),
                    // reconcile the options in place via
                    // `updateProjectV2Field`. Per empirical verification
                    // against the live GraphQL API (bead unblock-aa2,
                    // 2026-04-30), `updateProjectV2Field` preserves option
                    // IDs that match by name and allocates fresh IDs for
                    // options that don't — see the doc-comment on
                    // [`heal_select_field_options`] for the exact ID
                    // preservation contract.
                    //
                    // Options NOT in the input list are deleted by GitHub.
                    // This is safe by construction: `setup_fields` runs
                    // before any items are placed on the board (the
                    // canonical empty-project bootstrap path), so deleted
                    // options have no item assignments to invalidate.
                    //
                    // The helper returns whether a mutation was actually
                    // dispatched: fast-path (existing options already
                    // match the spec) lands in `skipped`; mutation path
                    // (GraphQL `updateProjectV2Field` issued) lands in
                    // `healed`. Buckets are mutually exclusive and the
                    // distinction is reflected in [`SetupReport`] so the
                    // MCP layer can surface what actually changed to the
                    // calling agent.
                    let (meta, mutated) =
                        self.heal_select_field_options(existing_field, spec).await?;
                    if mutated {
                        healed.push(spec.name.to_owned());
                    } else {
                        skipped.push(spec.name.to_owned());
                    }
                    resolved.insert(spec.name.to_owned(), meta);
                }
            } else {
                debug!(
                    field = spec.name,
                    data_type = spec.data_type,
                    "Creating field"
                );
                let meta = self.create_field(project_id, spec).await?;
                created.push(spec.name.to_owned());
                if spec.options.is_empty() {
                    resolved_plain.insert(spec.name.to_owned(), meta.field_id.clone());
                } else {
                    resolved.insert(spec.name.to_owned(), meta);
                }
            }
        }

        let field_ids = ProjectFieldIds {
            status: remove_field(&mut resolved, "Status")?,
            priority: remove_field(&mut resolved, "Priority")?,
            pipeline_stage: remove_field(&mut resolved, "PipelineStage")?,
            agent: remove_plain_field(&mut resolved_plain, "Agent")?,
            claimed_at: remove_plain_field(&mut resolved_plain, "ClaimedAt")?,
            story_points: remove_plain_field(&mut resolved_plain, "StoryPoints")?,
            defer_until: remove_plain_field(&mut resolved_plain, "DeferUntil")?,
        };

        // Step 3 (spec §5.7): IssueType ensure-and-heal at the org level.
        // GitHub's native issue types are org-only; for user-owned
        // repos this step is a no-op with an info-level log line and
        // an empty `issue_types_created` bucket. Introduced by
        // `unblock-wgj`.
        let owner_type = self.detect_owner_type().await?;
        let issue_types_created = match owner_type {
            OwnerType::Org => {
                let owner = self.owner().to_owned();
                self.ensure_issue_types(&owner).await?
            }
            OwnerType::User => {
                tracing::info!(
                    owner = %self.owner(),
                    "Skipping IssueType ensure-and-heal — owner is a User, not an Organization (GitHub native issue types are org-level only, spec §5.7)"
                );
                Vec::new()
            }
        };

        debug!(
            created_count = created.len(),
            healed_count = healed.len(),
            skipped_count = skipped.len(),
            issue_types_created_count = issue_types_created.len(),
            "All 7 project fields resolved + IssueType ensure-and-heal complete"
        );
        Ok(SetupReport {
            field_ids,
            created,
            healed,
            skipped,
            issue_types_created,
        })
    }

    /// Queries the setup status of required fields without mutating the project.
    ///
    /// Returns a [`SetupStatus`] indicating which of the 7 required fields
    /// already exist on the project and which are missing. This is used by the
    /// MCP setup tool's dry-run mode to report what `setup_fields()` would do
    /// without actually creating anything.
    ///
    /// # Errors
    ///
    /// Returns [`Error::GitHubGraphQL`] for GraphQL API errors.
    #[instrument(skip(self), fields(owner = %self.owner(), repo = %self.repo()))]
    pub async fn query_setup_status(&self, project_id: &str) -> Result<SetupStatus, Error> {
        let existing = self.fetch_existing_fields(project_id).await?;

        let mut existing_names = Vec::new();
        let mut missing_names = Vec::new();

        for name in REQUIRED_FIELD_NAMES {
            if existing.contains_key(*name) {
                existing_names.push((*name).to_owned());
            } else {
                missing_names.push((*name).to_owned());
            }
        }

        debug!(
            existing = existing_names.len(),
            missing = missing_names.len(),
            "Queried setup status"
        );

        Ok(SetupStatus {
            existing: existing_names,
            missing: missing_names,
        })
    }

    /// Updates a single field value on a Projects V2 item.
    ///
    /// Sends the `updateProjectV2ItemFieldValue` GraphQL mutation with the
    /// appropriate value input shape determined by the [`FieldValue`] variant.
    ///
    /// The `item_id` is the `ProjectV2Item` node ID (not the issue node ID).
    /// The `field_id` is the field's node ID from [`ProjectFieldIds`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::GitHubGraphQL`] for GraphQL API errors.
    #[instrument(skip(self, value))]
    pub async fn update_field(
        &self,
        project_id: &str,
        item_id: &str,
        field_id: &str,
        value: &FieldValue,
    ) -> Result<(), Error> {
        let value_input = match value {
            FieldValue::Text(t) => serde_json::json!({ "text": t }),
            FieldValue::Number(n) => serde_json::json!({ "number": n }),
            FieldValue::Date(d) => serde_json::json!({ "date": d.to_string() }),
            FieldValue::SingleSelectOption(option_id) => {
                serde_json::json!({ "singleSelectOptionId": option_id })
            }
        };

        let mutation = "
            mutation UpdateField($projectId: ID!, $itemId: ID!, $fieldId: ID!, $value: ProjectV2FieldValue!) {
                updateProjectV2ItemFieldValue(input: {
                    projectId: $projectId,
                    itemId: $itemId,
                    fieldId: $fieldId,
                    value: $value
                }) {
                    projectV2Item {
                        id
                    }
                }
            }
        ";

        let variables = serde_json::json!({
            "projectId": project_id,
            "itemId": item_id,
            "fieldId": field_id,
            "value": value_input,
        });

        self.graphql(mutation, variables).await?;

        debug!(item_id, field_id, "Updated project field");
        Ok(())
    }

    /// Fetches existing fields from a Projects V2 project and returns them as
    /// a map of field name to [`FieldMeta`].
    ///
    /// Uses cursor-based pagination to traverse all pages, so projects with
    /// more than 50 custom fields are handled correctly.
    #[allow(clippy::too_many_lines)]
    async fn fetch_existing_fields(
        &self,
        project_id: &str,
    ) -> Result<HashMap<String, FieldMeta>, Error> {
        let query = "
            query ProjectFields($projectId: ID!, $cursor: String) {
                node(id: $projectId) {
                    ... on ProjectV2 {
                        fields(first: 50, after: $cursor) {
                            pageInfo { endCursor hasNextPage }
                            nodes {
                                ... on ProjectV2SingleSelectField {
                                    id
                                    name
                                    dataType
                                    options {
                                        id
                                        name
                                        color
                                    }
                                }
                                ... on ProjectV2Field {
                                    id
                                    name
                                    dataType
                                }
                            }
                        }
                    }
                }
            }
        ";

        let mut fields: HashMap<String, FieldMeta> = HashMap::new();
        let mut cursor: Option<String> = None;

        loop {
            let variables = serde_json::json!({
                "projectId": project_id,
                "cursor": cursor,
            });

            let mut response = self.graphql(query, variables).await?;
            let fields_connection = &mut response["data"]["node"]["fields"];

            // Parse field nodes from this page. Take ownership of the nodes
            // array so individual node values can be moved into
            // `serde_json::from_value` without cloning.
            if let Some(serde_json::Value::Array(nodes)) = fields_connection
                .get_mut("nodes")
                .map(serde_json::Value::take)
            {
                for node_value in nodes {
                    // Skip null entries that can appear from union types.
                    if node_value.is_null() {
                        continue;
                    }

                    let field: FieldNode = match serde_json::from_value(node_value) {
                        Ok(f) => f,
                        Err(e) => {
                            warn!(error = %e, "Skipping unparseable field node");
                            continue;
                        }
                    };

                    let FieldNode {
                        id, name, options, ..
                    } = field;

                    let mut option_map = HashMap::new();
                    let mut color_map = HashMap::new();
                    for opt in options.unwrap_or_default() {
                        if let Some(color) = opt.color {
                            color_map.insert(opt.name.clone(), color);
                        }
                        option_map.insert(opt.name, opt.id);
                    }

                    fields.insert(
                        name,
                        FieldMeta {
                            field_id: id,
                            options: option_map,
                            option_colors: color_map,
                        },
                    );
                }
            } else {
                debug!(
                    "fields.nodes absent or not an array in GraphQL response — \
                     returning accumulated fields"
                );
                break;
            }

            // Check pagination: advance cursor or break.
            let page_info = &fields_connection["pageInfo"];
            let has_next_page = page_info
                .get("hasNextPage")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);

            if !has_next_page {
                break;
            }

            let next_cursor = page_info
                .get("endCursor")
                .and_then(serde_json::Value::as_str)
                .map(String::from);

            // Guard against infinite loop: if GitHub returns hasNextPage=true
            // but endCursor is null, cursor would reset to None and re-fetch
            // the first page forever. Break to prevent this.
            if next_cursor.is_none() {
                warn!(
                    "GitHub API returned hasNextPage=true but endCursor=null; \
                     stopping pagination to avoid infinite loop"
                );
                break;
            }

            cursor = next_cursor;
        }

        debug!(count = fields.len(), "Fetched existing project fields");
        Ok(fields)
    }

    /// Creates a single Projects V2 custom field via GraphQL.
    ///
    /// Dispatches to [`create_plain_field`](Self::create_plain_field) or
    /// [`create_select_field`](Self::create_select_field) based on whether the
    /// spec has options.
    async fn create_field(&self, project_id: &str, spec: &FieldSpec) -> Result<FieldMeta, Error> {
        if spec.options.is_empty() {
            self.create_plain_field(project_id, spec).await
        } else {
            self.create_select_field(project_id, spec).await
        }
    }

    /// Creates a plain (text, number, or date) Projects V2 field.
    async fn create_plain_field(
        &self,
        project_id: &str,
        spec: &FieldSpec,
    ) -> Result<FieldMeta, Error> {
        let mutation = "
            mutation CreateField($projectId: ID!, $name: String!, $dataType: ProjectV2CustomFieldType!) {
                createProjectV2Field(input: {
                    projectId: $projectId,
                    name: $name,
                    dataType: $dataType
                }) {
                    projectV2Field {
                        ... on ProjectV2Field {
                            id
                            name
                        }
                    }
                }
            }
        ";

        let variables = serde_json::json!({
            "projectId": project_id,
            "name": spec.name,
            "dataType": spec.data_type,
        });

        let response = self.graphql(mutation, variables).await?;
        let field_id = response["data"]["createProjectV2Field"]["projectV2Field"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_owned();

        if field_id.is_empty() {
            return Err(errors::GitHubGraphQLSnafu {
                errors: vec![format!("Failed to create field '{}'", spec.name).into()],
            }
            .build());
        }

        debug!(field = spec.name, field_id = %field_id, "Created non-select field");

        Ok(FieldMeta {
            field_id,
            options: HashMap::new(),
            option_colors: HashMap::new(),
        })
    }

    /// Creates a single-select Projects V2 field with the specified options.
    async fn create_select_field(
        &self,
        project_id: &str,
        spec: &FieldSpec,
    ) -> Result<FieldMeta, Error> {
        let mutation = "
            mutation CreateSelectField($projectId: ID!, $name: String!, $dataType: ProjectV2CustomFieldType!, $options: [ProjectV2SingleSelectFieldOptionInput!]!) {
                createProjectV2Field(input: {
                    projectId: $projectId,
                    name: $name,
                    dataType: $dataType,
                    singleSelectOptions: $options
                }) {
                    projectV2Field {
                        ... on ProjectV2SingleSelectField {
                            id
                            name
                            options {
                                id
                                name
                                color
                            }
                        }
                    }
                }
            }
        ";

        let options_input: Vec<serde_json::Value> = spec
            .options
            .iter()
            .map(|name| {
                serde_json::json!({
                    "name": name,
                    "description": "",
                    "color": "GRAY",
                })
            })
            .collect();

        let variables = serde_json::json!({
            "projectId": project_id,
            "name": spec.name,
            "dataType": spec.data_type,
            "options": options_input,
        });

        let response = self.graphql(mutation, variables).await?;
        let field_data = &response["data"]["createProjectV2Field"]["projectV2Field"];
        let field_id = field_data["id"].as_str().unwrap_or_default().to_owned();

        if field_id.is_empty() {
            return Err(errors::GitHubGraphQLSnafu {
                errors: vec![
                    format!("Failed to create single-select field '{}'", spec.name).into(),
                ],
            }
            .build());
        }

        let mut option_map = HashMap::new();
        let mut color_map = HashMap::new();
        if let Some(opts) = field_data["options"].as_array() {
            for opt in opts {
                if let (Some(id), Some(name)) = (opt["id"].as_str(), opt["name"].as_str()) {
                    option_map.insert(name.to_owned(), id.to_owned());
                    if let Some(color) = opt["color"].as_str() {
                        color_map.insert(name.to_owned(), color.to_owned());
                    }
                }
            }
        }

        debug!(
            field = spec.name,
            field_id = %field_id,
            options = ?option_map.keys().collect::<Vec<_>>(),
            "Created single-select field with options"
        );

        Ok(FieldMeta {
            field_id,
            options: option_map,
            option_colors: color_map,
        })
    }

    /// Reconciles a pre-existing single-select field's option set with the
    /// canonical [`FieldSpec`] options.
    ///
    /// Compares the current option names against the spec. When they match
    /// exactly (set equality, order-insensitive) the existing field is
    /// returned verbatim and the second tuple element is `false` — no
    /// GraphQL round-trip is issued. When they differ (e.g. the
    /// GitHub-default built-in `Status` field with options
    /// `[Todo, In Progress, Done]` vs. the spec's
    /// `[ready, in_progress, blocked, deferred, closed]`), this method
    /// issues `updateProjectV2Field` with the full canonical option set
    /// and the second tuple element is `true`.
    ///
    /// # Option ID preservation contract
    ///
    /// `updateProjectV2Field` accepts a `singleSelectOptions` array where
    /// each entry is either:
    ///
    /// - keyed by `id` — GitHub recognises the existing option and rewrites
    ///   its `name`, `color`, and `description` in place; the option ID
    ///   survives the mutation.
    /// - bare (no `id`) — GitHub allocates a fresh option ID.
    ///
    /// Options NOT present in the input array are **deleted** by GitHub
    /// (see Miguel's empirical verification in bead unblock-aa2,
    /// 2026-04-30).
    ///
    /// **Auto-heal matcher (post-`unblock-1zj`, spec §5.7).** This
    /// implementation reuses an existing option's GraphQL ID by a
    /// **normalised** name match — trim → lowercase → `_` → space →
    /// collapse internal whitespace. The matcher iterates the spec
    /// options in declared (board) order and consumes the first
    /// unconsumed existing option whose normalised name matches; any
    /// remaining unmatched existing options fall through to GitHub's
    /// standard "options not in the input list get deleted" behaviour.
    ///
    /// Examples (`normalize_option_name`):
    ///
    /// - `"in_progress"` → `"in progress"`
    /// - `"In Progress"` → `"in progress"`
    /// - `"IN_PROGRESS"` → `"in progress"`
    /// - `"  Backlog  "` → `"backlog"`
    ///
    /// **Migration path (`unblock-1zj`).** A board bootstrapped before
    /// `unblock-1zj` carries options
    /// `[ready, in_progress, blocked, deferred, closed]` (lowercase /
    /// `snake_case`). After this matcher upgrade, running `setup`
    /// against that board:
    ///
    /// - Reuses all 5 existing option IDs (each normalises to its
    ///   `TitleCase` counterpart), renaming them in place to `Ready`,
    ///   `In Progress`, `Blocked`, `Deferred`, `Closed`.
    /// - Allocates 1 fresh option ID for the new `Backlog` entry.
    /// - Reports the field in the `healed` bucket of `SetupReport`.
    ///
    /// No item assignments are lost — the rename is in-place per the
    /// GitHub `updateProjectV2Field` contract. The previous
    /// byte-exact-name matcher is REMOVED. The behaviour is safe
    /// because:
    ///
    /// - **Empty-project precondition.** `setup_fields` runs before any
    ///   items are placed on the board, so deleted options have no item
    ///   assignments to invalidate.
    /// - **Diagnostic loudness.** If this contract is ever violated (heal
    ///   runs against a populated project), the caller surfaces the
    ///   `healed` bucket in [`SetupReport`] rather than silently passing.
    ///
    /// # Color preservation
    ///
    /// Per bead unblock-aa2 finding S1, when the existing field already
    /// carried an option with the same name as a spec entry, the option's
    /// previous colour (read from
    /// [`FieldMeta::option_colors`]) is forwarded into the heal mutation
    /// instead of being flattened to `GRAY`. Brand-new spec options
    /// without a matching existing entry default to `GRAY`. This preserves
    /// operator-chosen colours through idempotent re-runs.
    ///
    /// # Display order
    ///
    /// The returned `singleSelectOptions` ordering matches the spec — the
    /// canonical option ordering is restored on each heal pass.
    ///
    /// # Errors
    ///
    /// Returns [`Error::FieldOptionHealFailed`] if the
    /// `updateProjectV2Field` mutation does not return the expected option
    /// set (i.e. GitHub responded but the response shape is unparseable or
    /// the post-heal options do not match the spec).
    ///
    /// Returns [`Error::GitHubGraphQL`] for transport-level GraphQL errors
    /// (network, schema, permission). Heal-specific failures bubble through
    /// the more specific [`FieldOptionHealFailed`](Error::FieldOptionHealFailed)
    /// variant for diagnostic clarity.
    #[allow(clippy::too_many_lines)]
    async fn heal_select_field_options(
        &self,
        existing: &FieldMeta,
        spec: &FieldSpec,
    ) -> Result<(FieldMeta, bool), Error> {
        // Fast path: option set already matches the spec by exact name —
        // no mutation. (We still defer to the normalised matcher below
        // for any deviation, so e.g. casing differences trigger a heal
        // rather than a silent skip.)
        let existing_names: std::collections::HashSet<&str> =
            existing.options.keys().map(String::as_str).collect();
        let spec_names: std::collections::HashSet<&str> = spec.options.iter().copied().collect();

        if existing_names == spec_names {
            debug!(
                field = spec.name,
                "Single-select field options already match spec — no heal needed"
            );
            return Ok((existing.clone(), false));
        }

        debug!(
            field = spec.name,
            existing = ?existing_names,
            expected = ?spec_names,
            "Auto-healing single-select field options to match spec"
        );

        // Build a (normalised_name, original_name) lookup over the
        // existing options so we can match the spec entries (in board
        // order) by normalised key while preserving the original name's
        // GraphQL ID. The match is one-to-one: each existing option may
        // be consumed at most once, in the spec's iteration order.
        // Trim / lowercase / underscore-to-space / collapse-whitespace
        // is the §5.7 normalisation contract.
        //
        // We pre-compute (existing_name → normalised_key) once because
        // `existing.options` is a `HashMap` and iteration is repeated
        // both in the matching loop below and (indirectly) when looking
        // up colours.
        let mut existing_normalised: Vec<(String, &str)> = existing
            .options
            .keys()
            .map(|name| (normalize_option_name(name), name.as_str()))
            .collect();
        // Stable iteration order is not required for correctness — the
        // §5.7 matching rule says "first unconsumed match" against an
        // existing iteration order — but we sort for deterministic
        // behaviour across HashMap implementations. Sort is by the
        // pre-existing original name so identical normalised keys
        // (which would only happen if a project carries two options
        // with normalised collision, e.g. `ready` and `Ready`
        // simultaneously — an edge case GitHub forbids on creation but
        // we tolerate gracefully) resolve in a predictable order.
        existing_normalised.sort_by(|a, b| a.1.cmp(b.1));

        let mutation = "
            mutation HealSelectField($fieldId: ID!, $name: String!, $options: [ProjectV2SingleSelectFieldOptionInput!]!) {
                updateProjectV2Field(input: {
                    fieldId: $fieldId,
                    name: $name,
                    singleSelectOptions: $options
                }) {
                    projectV2Field {
                        ... on ProjectV2SingleSelectField {
                            id
                            name
                            options {
                                id
                                name
                                color
                            }
                        }
                    }
                }
            }
        ";

        // Build the input: spec order, reusing existing option IDs and
        // colours by **normalised name match** so the `unblock-1zj`
        // lowercase → `TitleCase` rename preserves option IDs. Each
        // existing option may match at most one spec entry; consumed
        // entries are masked so a later spec entry cannot reuse them.
        // Options without a normalised match are inserted without `id`
        // (GitHub allocates a fresh option ID) and default to `GRAY`.
        // See the doc-comment above for the exact preservation contract.
        let mut consumed = vec![false; existing_normalised.len()];
        let options_input: Vec<serde_json::Value> = spec
            .options
            .iter()
            .map(|name| {
                let spec_key = normalize_option_name(name);

                // Find the first unconsumed existing option whose
                // normalised key matches.
                let matched_existing = existing_normalised
                    .iter()
                    .enumerate()
                    .find(|(idx, (key, _))| !consumed[*idx] && *key == spec_key)
                    .map(|(idx, (_, original))| (idx, *original));

                let mut opt = serde_json::Map::new();
                if let Some((idx, original_name)) = matched_existing {
                    consumed[idx] = true;
                    if let Some(existing_id) = existing.options.get(original_name) {
                        opt.insert(
                            "id".to_owned(),
                            serde_json::Value::String(existing_id.clone()),
                        );
                    }
                    opt.insert(
                        "name".to_owned(),
                        serde_json::Value::String((*name).to_owned()),
                    );
                    // Color preservation (bead unblock-aa2 S1; extended
                    // by `unblock-1zj` to follow the normalised match):
                    // forward the matched existing option's colour
                    // through the rename so operator-chosen colours
                    // survive the lowercase → `TitleCase` migration.
                    let color = existing
                        .option_colors
                        .get(original_name)
                        .map_or("GRAY", String::as_str)
                        .to_owned();
                    opt.insert("color".to_owned(), serde_json::Value::String(color));
                } else {
                    // No normalised match — fresh option ID allocated
                    // by GitHub. Default colour `GRAY` per the
                    // pre-existing color-preservation rule for new
                    // options.
                    opt.insert(
                        "name".to_owned(),
                        serde_json::Value::String((*name).to_owned()),
                    );
                    opt.insert(
                        "color".to_owned(),
                        serde_json::Value::String("GRAY".to_owned()),
                    );
                }
                opt.insert(
                    "description".to_owned(),
                    serde_json::Value::String(String::new()),
                );
                serde_json::Value::Object(opt)
            })
            .collect();

        let variables = serde_json::json!({
            "fieldId": existing.field_id,
            "name": spec.name,
            "options": options_input,
        });

        let response = self.graphql(mutation, variables).await?;
        let field_data = &response["data"]["updateProjectV2Field"]["projectV2Field"];
        let field_id = field_data["id"].as_str().unwrap_or_default().to_owned();

        if field_id.is_empty() {
            return Err(errors::FieldOptionHealFailedSnafu {
                field: spec.name.to_owned(),
                reason: "updateProjectV2Field response missing field id".to_owned(),
            }
            .build());
        }

        let mut option_map = HashMap::new();
        let mut color_map = HashMap::new();
        if let Some(opts) = field_data["options"].as_array() {
            for opt in opts {
                if let (Some(id), Some(name)) = (opt["id"].as_str(), opt["name"].as_str()) {
                    option_map.insert(name.to_owned(), id.to_owned());
                    if let Some(color) = opt["color"].as_str() {
                        color_map.insert(name.to_owned(), color.to_owned());
                    }
                }
            }
        }

        // Verify the heal actually produced the spec's option set.
        let healed_names: std::collections::HashSet<&str> =
            option_map.keys().map(String::as_str).collect();
        if healed_names != spec_names {
            return Err(errors::FieldOptionHealFailedSnafu {
                field: spec.name.to_owned(),
                reason: format!(
                    "post-heal options {healed_names:?} do not match spec {spec_names:?}"
                ),
            }
            .build());
        }

        debug!(
            field = spec.name,
            field_id = %field_id,
            options = ?option_map.keys().collect::<Vec<_>>(),
            "Healed single-select field — option set now matches spec"
        );

        Ok((
            FieldMeta {
                field_id,
                options: option_map,
                option_colors: color_map,
            },
            true,
        ))
    }
}

// ── REST API: views and fields ──────────────────────────────────────────

/// Deserialization helper for the REST field response.
///
/// The REST API (version 2026-03-10) returns fields with integer IDs and
/// a `data_type` string. Single-select fields include an `options` array
/// where each option's `name` is a nested object `{"raw": "...", "html": "..."}`.
#[derive(Debug, Deserialize)]
struct RestFieldResponse {
    id: u64,
    name: String,
    data_type: String,
    #[serde(default)]
    options: Vec<RestFieldOptionRaw>,
}

/// Raw option as returned by the REST API.
///
/// The `name` field is a nested object with `raw` and `html` sub-fields,
/// not a plain string.
#[derive(Debug, Deserialize)]
struct RestFieldOptionRaw {
    name: RestFieldOptionName,
    #[serde(default)]
    color: String,
    #[serde(default)]
    description: String,
}

/// Nested name object for REST field options.
///
/// The REST API (version 2026-03-10) returns `options[].name` as
/// `{"raw": "string", "html": "string"}` rather than a plain string.
#[derive(Debug, Deserialize)]
struct RestFieldOptionName {
    raw: String,
    #[allow(dead_code)]
    html: String,
}

/// Deserialization helper for the REST view creation response.
#[derive(Debug, Deserialize)]
struct RestViewResponse {
    id: u64,
    number: u64,
    name: String,
    layout: ViewLayout,
    #[serde(default)]
    node_id: Option<String>,
    #[serde(default)]
    filter: Option<String>,
    #[serde(default)]
    visible_fields: Vec<u64>,
}

/// Deserialization helper for the REST user/org type detection.
#[derive(Debug, Deserialize)]
struct UserTypeResponse {
    /// The `type` field: `"Organization"` or `"User"`.
    #[serde(rename = "type")]
    account_type: String,
}

/// Deserialisation helper for `GET /orgs/{org}/issue-types`.
///
/// Each entry is one of the org's issue types. Only `name` is consumed
/// — the `id`, `color`, `description`, and any other fields GitHub
/// returns are intentionally ignored because the ensure-and-heal step
/// (spec §5.7 step 3) MUST NOT overwrite operator-edited
/// color/description on existing types.
#[derive(Debug, Deserialize)]
struct OrgIssueTypeRaw {
    name: String,
}

/// Request body for `POST /orgs/{org}/issue-types`.
///
/// Wire format per GitHub REST API. `is_enabled` defaults to `true` —
/// org-level types must be enabled to be assignable to issues.
#[derive(Debug, Serialize)]
struct CreateOrgIssueTypeBody {
    name: &'static str,
    color: &'static str,
    description: &'static str,
    is_enabled: bool,
}

impl GitHubClient {
    /// Detects whether the repository owner is a GitHub organization or a
    /// personal user account.
    ///
    /// Sends `GET /users/{owner}` and inspects the `type` field in the
    /// response. Organizations return `"Organization"`, personal accounts
    /// return `"User"`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RateLimited`] for HTTP 429 responses.
    /// Returns [`Error::GitHubApi`] for other non-2xx responses.
    /// Returns [`Error::GitHubUnavailable`] for network failures.
    // TODO(unblock-45a.14): Cache the result in a OnceCell/Mutex on GitHubClient —
    // owner type does not change during a session and the setup tool calls this
    // before each REST call, adding unnecessary API round-trips.
    #[instrument(skip(self), fields(owner = %self.owner()))]
    pub async fn detect_owner_type(&self) -> Result<OwnerType, Error> {
        let url = self.rest_url(&format!("/users/{}", self.owner()));

        let response = self
            .http()
            .get(&url)
            .send()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        let response = check_rest_response(response).await?;

        let user_info: UserTypeResponse = response
            .json()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        let owner_type = match user_info.account_type.as_str() {
            "Organization" => OwnerType::Org,
            "User" => OwnerType::User,
            _ => {
                return errors::UnknownOwnerTypeSnafu {
                    owner: self.owner().to_owned(),
                    account_type: user_info.account_type.clone(),
                }
                .fail();
            }
        };

        debug!(
            owner = %self.owner(),
            account_type = %user_info.account_type,
            owner_type = ?owner_type,
            "Detected owner type"
        );

        Ok(owner_type)
    }

    /// Ensures all eight canonical org-level GitHub `IssueType`s
    /// (`Task`, `Bug`, `Feature`, `Spike`, `Epic`, `Chore`, `Refactor`,
    /// `Docs`) exist on the configured organisation, creating any that
    /// are missing.
    ///
    /// Per spec §5.7 step 3 / Appendix B Decision 3: idempotent
    /// ensure-and-heal — list existing org issue types, match
    /// case-insensitively + byte-trim against the canonical taxonomy
    /// (mirrors `heal_select_field_options` semantics, §5.7), SKIP
    /// existing types (color/description on the org side are
    /// user-editable and `setup` MUST NOT overwrite them), and POST any
    /// missing types using the canonical name/color/description from
    /// `IssueType::canonical_*` helpers (§2.6).
    ///
    /// Sends both the GET (via `diff_org_issue_types`) and the
    /// `POST /orgs/{org}/issue-types` create call with the
    /// `X-GitHub-Api-Version: 2026-03-10` header. The default
    /// `2022-11-28` version returns HTTP 403 "Resource not accessible by
    /// personal access token" against `/orgs/{org}/issue-types` even on
    /// tokens with `Issue Types: R/W`; the newer header is required.
    ///
    /// Returns the list of canonical names that were CREATED (not
    /// pre-existing) by this call, in the declared
    /// [`IssueType::ALL`](unblock_core::types::IssueType::ALL) order.
    /// An empty vector means all eight types already existed; this is
    /// the steady-state outcome of repeated `setup` runs.
    ///
    /// # Errors
    ///
    /// Returns [`Error::IssueTypeManagementForbidden`] when GitHub
    /// returns HTTP 403 from either the GET or the POST
    /// (the configured token lacks `admin:org` to manage org-level
    /// issue types). The error message points operators at upgrading
    /// the token (`admin:org` scope).
    ///
    /// Returns [`Error::RateLimited`] for HTTP 429 responses.
    /// Returns [`Error::GitHubApi`] for other non-2xx responses.
    /// Returns [`Error::GitHubUnavailable`] for network failures.
    ///
    /// # Panics
    ///
    /// Unreachable in practice: the internal diff helper sources every
    /// returned name directly from `REQUIRED_ISSUE_TYPES` (the private
    /// canonical taxonomy), so the reverse lookup always succeeds. A
    /// panic here would indicate a programmer error in
    /// `diff_org_issue_types` — not a runtime surface that callers
    /// can encounter.
    ///
    /// # Org-only scope
    ///
    /// The caller (typically `setup_fields`) is responsible for gating
    /// this call on `OwnerType::Org` — GitHub's native issue types are
    /// an org-level feature only. Calling against a user account would
    /// surface a 404 from GitHub; this method does not pre-validate.
    #[instrument(skip(self), fields(org = %org))]
    pub async fn ensure_issue_types(&self, org: &str) -> Result<Vec<String>, Error> {
        // Step 1: GET existing types and compute the canonical-order
        // diff (shared with the dry-run path). Any 403 surfaces as
        // `IssueTypeManagementForbidden` here.
        let missing = self.diff_org_issue_types(org).await?;

        let mut created: Vec<String> = Vec::new();

        // Step 2: POST each missing type in the diff order (already
        // `IssueType::ALL`-declared). Iteration matches the dry-run
        // ordering exactly so two independent runs against an empty
        // org produce byte-identical creation sequences (Invariant 18,
        // §14).
        for name in &missing {
            // Recover the spec entry by canonical name. The diff
            // returns `IssueType::ALL`-ordered names, so the matching
            // spec must exist; the panic is unreachable in practice
            // and the lookup is O(8).
            let spec = REQUIRED_ISSUE_TYPES
                .iter()
                .find(|s| s.name == name.as_str())
                .expect("missing name was sourced from REQUIRED_ISSUE_TYPES");

            let body = CreateOrgIssueTypeBody {
                name: spec.name,
                color: spec.color,
                description: spec.description,
                is_enabled: true,
            };

            let create_url = self.rest_url(&format!("/orgs/{org}/issue-types"));
            let response = self
                .http()
                .post(&create_url)
                .header("X-GitHub-Api-Version", VIEWS_API_VERSION)
                .json(&body)
                .send()
                .await
                .context(errors::GitHubUnavailableSnafu)?;

            if response.status().as_u16() == 403 {
                return errors::IssueTypeManagementForbiddenSnafu {
                    org: org.to_owned(),
                }
                .fail();
            }

            let _response = check_rest_response(response).await?;

            debug!(
                name = spec.name,
                color = spec.color,
                "Created org-level issue type"
            );
            created.push(spec.name.to_owned());
        }

        Ok(created)
    }

    /// Reads the org's existing issue types and returns the canonical
    /// names that `ensure_issue_types` WOULD create on a write-path
    /// run, in the declared
    /// [`IssueType::ALL`](unblock_core::types::IssueType::ALL) order.
    ///
    /// This is the read-only sibling of [`Self::ensure_issue_types`] —
    /// `setup --dry-run` calls this so operators see the diff between
    /// the canonical taxonomy and the org's current state without
    /// dispatching any POSTs. The method is idempotent and side
    /// effect-free.
    ///
    /// Per spec §5.7 step 3 / Appendix B Decision 3: matching is
    /// case-insensitive + byte-trim via the same `normalize_option_name`
    /// matcher used by `heal_select_field_options` so an org that
    /// already has `task` / `BUG` / `Feature` does not surface `Task`
    /// / `Bug` / `Feature` in the diff.
    ///
    /// # Errors
    ///
    /// Returns [`Error::IssueTypeManagementForbidden`] when GitHub
    /// returns HTTP 403 from the listing call (the configured token
    /// lacks `read:org` to enumerate org-level issue types). The error
    /// message points operators at upgrading the token.
    ///
    /// Returns [`Error::RateLimited`] for HTTP 429 responses.
    /// Returns [`Error::GitHubApi`] for other non-2xx responses.
    /// Returns [`Error::GitHubUnavailable`] for network failures.
    ///
    /// # Org-only scope
    ///
    /// As with [`Self::ensure_issue_types`], the caller is responsible
    /// for gating on `OwnerType::Org`. Calling against a user account
    /// would surface a 404 from GitHub.
    #[instrument(skip(self), fields(org = %org))]
    pub async fn query_issue_types_status(&self, org: &str) -> Result<Vec<String>, Error> {
        self.diff_org_issue_types(org).await
    }

    /// Internal: GETs `/orgs/{org}/issue-types` and computes the
    /// canonical-name diff against [`REQUIRED_ISSUE_TYPES`].
    ///
    /// Returned names follow [`IssueType::ALL`] declared order so both
    /// the write path (`ensure_issue_types`) and the dry-run path
    /// (`query_issue_types_status`) emit identical sequences. Any 403
    /// from the GET surfaces as
    /// [`Error::IssueTypeManagementForbidden`].
    ///
    /// Sets the `X-GitHub-Api-Version: 2026-03-10` header — the default
    /// `2022-11-28` version returns 403 on `/orgs/{org}/issue-types`
    /// even with sufficient token scope.
    async fn diff_org_issue_types(&self, org: &str) -> Result<Vec<String>, Error> {
        let list_url = self.rest_url(&format!("/orgs/{org}/issue-types"));

        let list_response = self
            .http()
            .get(&list_url)
            .header("X-GitHub-Api-Version", VIEWS_API_VERSION)
            .send()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        if list_response.status().as_u16() == 403 {
            return errors::IssueTypeManagementForbiddenSnafu {
                org: org.to_owned(),
            }
            .fail();
        }

        let list_response = check_rest_response(list_response).await?;
        let existing_raw: Vec<OrgIssueTypeRaw> = list_response
            .json()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        // Normalise existing names with `normalize_option_name` so the
        // lookup mirrors the §5.7 case-insensitive + byte-trim matcher
        // applied to Projects V2 single-select fields.
        let existing_normalised: std::collections::HashSet<String> = existing_raw
            .iter()
            .map(|raw| normalize_option_name(&raw.name))
            .collect();

        let mut missing: Vec<String> = Vec::new();
        for spec in &REQUIRED_ISSUE_TYPES {
            let key = normalize_option_name(spec.name);
            if existing_normalised.contains(&key) {
                debug!(name = spec.name, "Issue type already exists — skipping");
                continue;
            }
            missing.push(spec.name.to_owned());
        }

        Ok(missing)
    }

    /// Lists all fields on a Projects V2 project via the REST API.
    ///
    /// Sends `GET /{orgs|users}/{owner}/projectsV2/{n}/fields` with the
    /// `X-GitHub-Api-Version: 2026-03-10` header. Returns fields with integer
    /// IDs suitable for use in the `visible_fields` parameter of
    /// [`create_view()`](Self::create_view).
    ///
    /// For single-select fields, the `options[].name` nested object is
    /// parsed to extract the raw display name.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ProjectNotConfigured`] if no project number is set.
    /// Returns [`Error::RateLimited`] for HTTP 429 responses.
    /// Returns [`Error::GitHubApi`] for other non-2xx responses.
    /// Returns [`Error::GitHubUnavailable`] for network failures.
    #[instrument(skip(self), fields(owner = %self.owner()))]
    pub async fn list_rest_fields(&self, owner_type: OwnerType) -> Result<Vec<RestField>, Error> {
        let project_number = self
            .project_number()
            .ok_or_else(|| errors::ProjectNotConfiguredSnafu.build())?;

        let url = self.rest_url(&format!(
            "/{}/{}/projectsV2/{project_number}/fields",
            owner_type.path_segment(),
            self.owner()
        ));

        let response = self
            .http()
            .get(&url)
            .header("X-GitHub-Api-Version", VIEWS_API_VERSION)
            .send()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        let response = check_rest_response(response).await?;

        let raw_fields: Vec<RestFieldResponse> = response
            .json()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        let fields: Vec<RestField> = raw_fields
            .into_iter()
            .map(|f| {
                let options = f
                    .options
                    .into_iter()
                    .map(|o| RestFieldOption {
                        name: o.name.raw,
                        color: o.color,
                        description: o.description,
                    })
                    .collect();

                RestField {
                    id: f.id,
                    name: f.name,
                    data_type: f.data_type,
                    options,
                }
            })
            .collect();

        debug!(count = fields.len(), "Listed REST fields");
        Ok(fields)
    }

    /// Creates a new view on a Projects V2 project via the REST API.
    ///
    /// Sends `POST /{orgs|users}/{owner}/projectsV2/{n}/views` with the
    /// `X-GitHub-Api-Version: 2026-03-10` header. Returns the created
    /// [`ProjectView`] with integer IDs and layout metadata.
    ///
    /// The caller is responsible for idempotency — use [`list_views()`](Self::list_views)
    /// to check for existing views before creating.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ProjectNotConfigured`] if no project number is set.
    /// Returns [`Error::RateLimited`] for HTTP 429 responses.
    /// Returns [`Error::GitHubApi`] for other non-2xx responses.
    /// Returns [`Error::GitHubUnavailable`] for network failures.
    #[instrument(skip(self, params), fields(owner = %self.owner(), view_name = %params.name))]
    pub async fn create_view(
        &self,
        owner_type: OwnerType,
        params: &CreateViewParams,
    ) -> Result<ProjectView, Error> {
        let project_number = self
            .project_number()
            .ok_or_else(|| errors::ProjectNotConfiguredSnafu.build())?;

        let url = self.rest_url(&format!(
            "/{}/{}/projectsV2/{project_number}/views",
            owner_type.path_segment(),
            self.owner()
        ));

        let response = self
            .http()
            .post(&url)
            .header("X-GitHub-Api-Version", VIEWS_API_VERSION)
            .json(params)
            .send()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        let response = check_rest_response(response).await?;

        let view: RestViewResponse = response
            .json()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        let project_view = ProjectView {
            id: Some(view.id),
            number: view.number,
            name: view.name,
            layout: view.layout,
            node_id: view.node_id,
            filter: view.filter,
            visible_fields: view.visible_fields,
        };

        debug!(
            view_name = %project_view.name,
            view_number = project_view.number,
            layout = %project_view.layout,
            "Created project view"
        );

        Ok(project_view)
    }

    /// Lists all views on a Projects V2 project via the GraphQL API.
    ///
    /// The REST API does not provide a `GET /views` endpoint, so this method
    /// uses GraphQL to query existing views. The query routes through either
    /// `organization` or `user` based on the `owner_type` parameter.
    ///
    /// Returns up to 50 views. For projects with more views, pagination
    /// would need to be added (unlikely for typical use).
    ///
    /// # Errors
    ///
    /// Returns [`Error::ProjectNotConfigured`] if no project number is set.
    /// Returns [`Error::GitHubGraphQL`] for GraphQL API errors.
    /// Returns [`Error::GitHubUnavailable`] for network failures.
    #[instrument(skip(self), fields(owner = %self.owner()))]
    pub async fn list_views(&self, owner_type: OwnerType) -> Result<Vec<ProjectView>, Error> {
        let project_number = self
            .project_number()
            .ok_or_else(|| errors::ProjectNotConfiguredSnafu.build())?;

        // Route through organization or user based on owner type.
        // The `viewer { projectV2(number:) }` query does not work for
        // org-owned projects per research findings. Both queries share an
        // identical shape — only the root field differs — so we use two
        // const query strings to avoid a heap allocation on every call.
        const ORG_QUERY: &str = "
            query ListViews($login: String!, $projectNumber: Int!) {
                organization(login: $login) {
                    projectV2(number: $projectNumber) {
                        views(first: 50) {
                            nodes {
                                id
                                name
                                number
                                layout
                                filter
                            }
                        }
                    }
                }
            }
        ";
        const USER_QUERY: &str = "
            query ListViews($login: String!, $projectNumber: Int!) {
                user(login: $login) {
                    projectV2(number: $projectNumber) {
                        views(first: 50) {
                            nodes {
                                id
                                name
                                number
                                layout
                                filter
                            }
                        }
                    }
                }
            }
        ";

        let (query, owner_key) = match owner_type {
            OwnerType::Org => (ORG_QUERY, "organization"),
            OwnerType::User => (USER_QUERY, "user"),
        };

        let variables = serde_json::json!({
            "login": self.owner(),
            "projectNumber": project_number,
        });

        let response = self.graphql(query, variables).await?;

        let nodes = response["data"][owner_key]["projectV2"]["views"]["nodes"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        let views: Vec<ProjectView> = nodes
            .iter()
            .filter(|n| !n.is_null())
            .filter_map(|node| {
                let name = node["name"].as_str()?.to_owned();
                let number = node["number"].as_u64()?;
                let layout_str = node["layout"].as_str().unwrap_or("TABLE_LAYOUT");
                let layout = match layout_str {
                    "BOARD_LAYOUT" => ViewLayout::Board,
                    "ROADMAP_LAYOUT" => ViewLayout::Roadmap,
                    _ => ViewLayout::Table,
                };
                let node_id = node["id"].as_str().map(String::from);
                let filter = node["filter"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(String::from);

                Some(ProjectView {
                    id: None, // GraphQL does not return integer IDs
                    number,
                    name,
                    layout,
                    node_id,
                    filter,
                    visible_fields: Vec::new(), // Not available from this query
                })
            })
            .collect();

        debug!(count = views.len(), "Listed project views via GraphQL");
        Ok(views)
    }

    /// Resolves the GraphQL global node ID for the repository owner.
    ///
    /// Queries `organization(login:) { id }` or `user(login:) { id }` depending
    /// on the provided [`OwnerType`]. The returned node ID is required by the
    /// `createProjectV2` mutation's `ownerId` input.
    ///
    /// # Errors
    ///
    /// Returns [`Error::GitHubGraphQL`] if the owner cannot be found.
    /// Returns [`Error::GitHubUnavailable`] for network failures.
    #[instrument(skip(self), fields(owner = %self.owner(), owner_type = ?owner_type))]
    pub async fn resolve_owner_node_id(&self, owner_type: OwnerType) -> Result<String, Error> {
        let (query, data_key) = match owner_type {
            OwnerType::Org => (
                "query ResolveOrgId($login: String!) { organization(login: $login) { id } }",
                "organization",
            ),
            OwnerType::User => (
                "query ResolveUserId($login: String!) { user(login: $login) { id } }",
                "user",
            ),
        };

        let variables = serde_json::json!({
            "login": self.owner(),
        });

        let response = self.graphql(query, variables).await?;
        let node_id = response["data"][data_key]["id"]
            .as_str()
            .unwrap_or_default()
            .to_owned();

        if node_id.is_empty() {
            return Err(errors::GitHubGraphQLSnafu {
                errors: vec![
                    format!(
                        "Could not resolve node ID for {} '{}' — check the owner name",
                        match owner_type {
                            OwnerType::Org => "organization",
                            OwnerType::User => "user",
                        },
                        self.owner()
                    )
                    .into(),
                ],
            }
            .build());
        }

        debug!(owner = %self.owner(), node_id = %node_id, "Resolved owner node ID");
        Ok(node_id)
    }

    /// Lists all Projects V2 projects owned by the repository owner.
    ///
    /// Queries `organization(login:) { projectsV2(first: 100) { ... } }` or
    /// `user(login:) { projectsV2(first: 100) { ... } }` depending on the
    /// provided [`OwnerType`]. Uses cursor-based pagination to traverse all
    /// pages, returning the complete list of projects.
    ///
    /// # Errors
    ///
    /// Returns [`Error::GitHubGraphQL`] for GraphQL API errors.
    /// Returns [`Error::GitHubUnavailable`] for network failures.
    #[instrument(skip(self), fields(owner = %self.owner(), owner_type = ?owner_type))]
    pub async fn list_owner_projects(
        &self,
        owner_type: OwnerType,
    ) -> Result<Vec<OwnerProject>, Error> {
        let (query, data_key) = match owner_type {
            OwnerType::Org => (
                "
                query ListOrgProjects($login: String!, $cursor: String) {
                    organization(login: $login) {
                        projectsV2(first: 100, after: $cursor) {
                            pageInfo { endCursor hasNextPage }
                            nodes { number title url }
                        }
                    }
                }
                ",
                "organization",
            ),
            OwnerType::User => (
                "
                query ListUserProjects($login: String!, $cursor: String) {
                    user(login: $login) {
                        projectsV2(first: 100, after: $cursor) {
                            pageInfo { endCursor hasNextPage }
                            nodes { number title url }
                        }
                    }
                }
                ",
                "user",
            ),
        };

        let mut all_projects = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let variables = serde_json::json!({
                "login": self.owner(),
                "cursor": cursor,
            });

            let response = self.graphql(query, variables).await?;
            let projects_v2 = &response["data"][data_key]["projectsV2"];

            // Parse project nodes from this page.
            if let Some(nodes) = projects_v2
                .get("nodes")
                .and_then(serde_json::Value::as_array)
            {
                for node in nodes {
                    if node.is_null() {
                        continue;
                    }
                    if let (Some(number), Some(title), Some(url)) = (
                        node["number"].as_u64(),
                        node["title"].as_str(),
                        node["url"].as_str(),
                    ) {
                        all_projects.push(OwnerProject {
                            number,
                            title: title.to_owned(),
                            url: url.to_owned(),
                        });
                    }
                }
            } else {
                debug!(
                    data_key,
                    "projectsV2.nodes absent or not an array in GraphQL response — returning empty list"
                );
                break;
            }

            // Check pagination: advance cursor or break.
            let page_info = &projects_v2["pageInfo"];
            let has_next_page = page_info
                .get("hasNextPage")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);

            if !has_next_page {
                break;
            }

            let next_cursor = page_info
                .get("endCursor")
                .and_then(serde_json::Value::as_str)
                .map(String::from);

            // Guard against infinite loop: if GitHub returns hasNextPage=true
            // but endCursor is null, cursor would reset to None and re-fetch
            // the first page forever. Break to prevent this.
            if next_cursor.is_none() {
                warn!(
                    "GitHub API returned hasNextPage=true but endCursor=null; \
                     stopping pagination to avoid infinite loop"
                );
                break;
            }

            cursor = next_cursor;
        }

        debug!(count = all_projects.len(), "Listed owner projects");
        Ok(all_projects)
    }

    /// Creates a new GitHub Projects V2 project.
    ///
    /// Uses the `createProjectV2` GraphQL mutation with the given owner node ID
    /// and title. Returns the created project's number and URL.
    ///
    /// **Note:** The `description` field is not supported by the `createProjectV2`
    /// mutation input — it is accepted here for forward-compatibility but is not
    /// sent to the API. The description can be set separately via a project update
    /// mutation after creation.
    ///
    /// # Errors
    ///
    /// Returns [`Error::GitHubGraphQL`] if the mutation fails (e.g. insufficient
    /// permissions, invalid owner ID).
    /// Returns [`Error::GitHubUnavailable`] for network failures.
    #[instrument(skip(self), fields(owner = %self.owner(), title = %title))]
    pub async fn create_project(
        &self,
        owner_node_id: &str,
        title: &str,
    ) -> Result<CreatedProject, Error> {
        let mutation = "
            mutation CreateProject($ownerId: ID!, $title: String!) {
                createProjectV2(input: { ownerId: $ownerId, title: $title }) {
                    projectV2 {
                        number
                        url
                    }
                }
            }
        ";

        let variables = serde_json::json!({
            "ownerId": owner_node_id,
            "title": title,
        });

        let response = self.graphql(mutation, variables).await?;
        let project = &response["data"]["createProjectV2"]["projectV2"];
        let number = project["number"].as_u64().unwrap_or_default();
        let url = project["url"].as_str().unwrap_or_default().to_owned();

        if number == 0 || url.is_empty() {
            return Err(errors::GitHubGraphQLSnafu {
                errors: vec![
                    format!(
                        "createProjectV2 mutation returned empty project data for title '{title}'"
                    )
                    .into(),
                ],
            }
            .build());
        }

        debug!(number, url = %url, "Created project V2");
        Ok(CreatedProject { number, url })
    }
}

#[cfg(test)]
mod tests {
    use super::{FieldMeta, FieldSpec, OwnerType, REQUIRED_ISSUE_TYPES, STATUS_OPTION_NAMES};
    use crate::client::GitHubClient;
    use crate::errors::Error;
    use std::collections::HashMap;
    use unblock_core::types::IssueType;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_priority_field_meta() -> FieldMeta {
        let mut options = HashMap::new();
        options.insert("P0 - Critical".to_string(), "id-p0".to_string());
        options.insert("P1 - High".to_string(), "id-p1".to_string());
        options.insert("P2 - Medium".to_string(), "id-p2".to_string());
        options.insert("P3 - Low".to_string(), "id-p3".to_string());
        options.insert("P4 - Backlog".to_string(), "id-p4".to_string());
        FieldMeta {
            field_id: "field-priority".to_string(),
            options,
            option_colors: HashMap::new(),
        }
    }

    #[test]
    fn option_id_by_prefix_exact_match() {
        let meta = make_priority_field_meta();
        assert_eq!(
            meta.option_id_by_prefix("P0 - Critical"),
            Some(&"id-p0".to_string()),
        );
    }

    #[test]
    fn option_id_by_prefix_short_code_match() {
        let meta = make_priority_field_meta();
        assert_eq!(meta.option_id_by_prefix("P1"), Some(&"id-p1".to_string()),);
    }

    #[test]
    fn option_id_by_prefix_no_match() {
        let meta = make_priority_field_meta();
        assert_eq!(meta.option_id_by_prefix("P9"), None);
    }

    #[test]
    fn option_id_by_prefix_empty_prefix_matches_any() {
        // An empty prefix matches any option (all names start with "" and are longer).
        // This documents the current behavior — callers should not pass empty strings.
        let meta = make_priority_field_meta();
        assert!(meta.option_id_by_prefix("").is_some());
    }

    #[test]
    fn option_id_by_prefix_full_name_not_prefix() {
        // When the prefix equals a full option name exactly, exact match wins.
        let meta = make_priority_field_meta();
        assert_eq!(
            meta.option_id_by_prefix("P2 - Medium"),
            Some(&"id-p2".to_string()),
        );
    }

    #[test]
    fn option_id_by_prefix_empty_options() {
        let meta = FieldMeta {
            field_id: "field-empty".to_string(),
            options: HashMap::new(),
            option_colors: HashMap::new(),
        };
        assert_eq!(meta.option_id_by_prefix("P0"), None);
    }

    #[test]
    fn option_id_by_prefix_prefers_exact_over_prefix() {
        // If "P0" is both an exact key AND a prefix of another key,
        // exact match wins deterministically.
        let mut options = HashMap::new();
        options.insert("P0".to_string(), "id-exact".to_string());
        options.insert("P0 - Critical".to_string(), "id-prefix".to_string());
        let meta = FieldMeta {
            field_id: "field-mixed".to_string(),
            options,
            option_colors: HashMap::new(),
        };
        assert_eq!(
            meta.option_id_by_prefix("P0"),
            Some(&"id-exact".to_string()),
        );
    }

    #[tokio::test]
    async fn detect_owner_type_returns_org_for_organization_account() {
        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        Mock::given(method("GET"))
            .and(path("/users/test-owner"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "login": "test-owner",
                "type": "Organization"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let owner_type = client
            .detect_owner_type()
            .await
            .expect("detect_owner_type should succeed");
        assert_eq!(owner_type, OwnerType::Org);
    }

    #[tokio::test]
    async fn detect_owner_type_returns_user_for_user_account() {
        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        Mock::given(method("GET"))
            .and(path("/users/test-owner"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "login": "test-owner",
                "type": "User"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let owner_type = client
            .detect_owner_type()
            .await
            .expect("detect_owner_type should succeed");
        assert_eq!(owner_type, OwnerType::User);
    }

    #[tokio::test]
    async fn detect_owner_type_errors_on_unknown_account_type() {
        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        Mock::given(method("GET"))
            .and(path("/users/test-owner"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "login": "test-owner",
                "type": "Bot"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let err = client
            .detect_owner_type()
            .await
            .expect_err("detect_owner_type should fail for unknown account type");

        match err {
            Error::UnknownOwnerType {
                owner,
                account_type,
            } => {
                assert_eq!(owner, "test-owner");
                assert_eq!(account_type, "Bot");
            }
            other => panic!("expected UnknownOwnerType, got {other:?}"),
        }
    }

    // ── normalize_option_name (unblock-1zj §5.7 auto-heal matcher) ──────

    #[test]
    fn normalize_option_name_lowercases_and_collapses_underscores() {
        // Spec §5.7 examples: trim → lowercase → underscore-to-space →
        // collapse internal whitespace.
        assert_eq!(super::normalize_option_name("in_progress"), "in progress");
        assert_eq!(super::normalize_option_name("In Progress"), "in progress");
        assert_eq!(super::normalize_option_name("IN_PROGRESS"), "in progress");
        assert_eq!(super::normalize_option_name("Backlog"), "backlog");
        assert_eq!(super::normalize_option_name("ready"), "ready");
        assert_eq!(super::normalize_option_name("  Backlog  "), "backlog");
        // Internal whitespace runs collapse to a single space.
        assert_eq!(super::normalize_option_name("In   Progress"), "in progress");
        // Empty / whitespace-only normalises to empty.
        assert_eq!(super::normalize_option_name(""), "");
        assert_eq!(super::normalize_option_name("   "), "");
    }

    #[test]
    fn normalize_option_name_idempotent_on_canonical_keys() {
        // Re-normalising the helper output yields the same key — used
        // by the heal matcher to compare spec entries against existing
        // option names without round-tripping through the original.
        for s in ["in progress", "ready", "backlog", "blocked"] {
            assert_eq!(super::normalize_option_name(s), s);
        }
    }

    #[test]
    fn status_option_names_normalise_to_distinct_keys() {
        // Spec §5.7 / Invariant 16: every `Status::option_name` output
        // normalises to a unique key. This is the precondition for the
        // auto-heal matcher's first-unconsumed-match rule to be sound.
        let mut keys: Vec<String> = unblock_core::types::Status::ALL
            .iter()
            .map(|s| super::normalize_option_name(s.option_name()))
            .collect();
        keys.sort();
        let dedup_len = {
            let mut copy = keys.clone();
            copy.dedup();
            copy.len()
        };
        assert_eq!(
            keys.len(),
            dedup_len,
            "Status::option_name outputs collide under normalisation: {keys:?}"
        );
    }

    // ── heal_select_field_options: ID preservation across rename ────────

    #[tokio::test]
    async fn heal_status_field_preserves_existing_option_ids_across_rename() {
        // Spec §5.7 + Appendix A.3 obligation 3: a board carrying the
        // legacy lowercase / `snake_case` Status options heals to the
        // canonical `TitleCase` set with the 5 existing option IDs
        // preserved (one fresh ID for `Backlog`). The auto-heal matcher
        // is the only path that achieves this; this test pins it.
        let server = MockServer::start().await;

        // Mock the `updateProjectV2Field` mutation. The mutation echoes
        // back a synthesized option set that mirrors the spec's
        // canonical 6 entries while reusing IDs for the legacy 5.
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "updateProjectV2Field": {
                        "projectV2Field": {
                            "id": "STATUS_FIELD_ID",
                            "name": "Status",
                            "options": [
                                {"id": "OPT_BACKLOG_NEW", "name": "Backlog", "color": "GRAY"},
                                {"id": "OPT_READY", "name": "Ready", "color": "GREEN"},
                                {"id": "OPT_IN_PROGRESS", "name": "In Progress", "color": "YELLOW"},
                                {"id": "OPT_BLOCKED", "name": "Blocked", "color": "RED"},
                                {"id": "OPT_DEFERRED", "name": "Deferred", "color": "BLUE"},
                                {"id": "OPT_CLOSED", "name": "Closed", "color": "PURPLE"},
                            ]
                        }
                    }
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = GitHubClient::new_for_test(&server.uri());

        // Existing options match the pre-`unblock-1zj` lowercase
        // / `snake_case` set, with deliberate operator-chosen colours
        // to verify color preservation across the rename.
        let mut existing_options = HashMap::new();
        existing_options.insert("ready".to_owned(), "OPT_READY".to_owned());
        existing_options.insert("in_progress".to_owned(), "OPT_IN_PROGRESS".to_owned());
        existing_options.insert("blocked".to_owned(), "OPT_BLOCKED".to_owned());
        existing_options.insert("deferred".to_owned(), "OPT_DEFERRED".to_owned());
        existing_options.insert("closed".to_owned(), "OPT_CLOSED".to_owned());

        let mut existing_colors = HashMap::new();
        existing_colors.insert("ready".to_owned(), "GREEN".to_owned());
        existing_colors.insert("in_progress".to_owned(), "YELLOW".to_owned());
        existing_colors.insert("blocked".to_owned(), "RED".to_owned());
        existing_colors.insert("deferred".to_owned(), "BLUE".to_owned());
        existing_colors.insert("closed".to_owned(), "PURPLE".to_owned());

        let existing = FieldMeta::new("STATUS_FIELD_ID".to_owned(), existing_options)
            .with_option_colors(existing_colors);

        let spec_options: Vec<&'static str> = STATUS_OPTION_NAMES.to_vec();
        let spec = FieldSpec {
            name: "Status",
            data_type: "SINGLE_SELECT",
            options: Box::leak(spec_options.into_boxed_slice()),
        };

        let (healed, did_mutate) = client
            .heal_select_field_options(&existing, &spec)
            .await
            .expect("heal_select_field_options must succeed");

        // The heal MUST have mutated (option set differs by name even
        // though all 5 normalise to the same TitleCase keys).
        assert!(did_mutate, "heal must run when option names change");

        // Post-heal options carry the canonical TitleCase names.
        assert_eq!(
            healed.options.len(),
            unblock_core::types::Status::ALL.len(),
            "heal must converge on 6 options"
        );
        for variant in unblock_core::types::Status::ALL {
            assert!(
                healed.options.contains_key(variant.option_name()),
                "post-heal options missing {:?}",
                variant.option_name()
            );
        }

        // ID preservation: each renamed option carries its legacy ID.
        // `Backlog` is the only fresh allocation (no normalised match
        // in the existing set).
        assert_eq!(healed.options.get("Ready"), Some(&"OPT_READY".to_owned()));
        assert_eq!(
            healed.options.get("In Progress"),
            Some(&"OPT_IN_PROGRESS".to_owned())
        );
        assert_eq!(
            healed.options.get("Blocked"),
            Some(&"OPT_BLOCKED".to_owned())
        );
        assert_eq!(
            healed.options.get("Deferred"),
            Some(&"OPT_DEFERRED".to_owned())
        );
        assert_eq!(healed.options.get("Closed"), Some(&"OPT_CLOSED".to_owned()));
        assert_eq!(
            healed.options.get("Backlog"),
            Some(&"OPT_BACKLOG_NEW".to_owned()),
            "Backlog is freshly allocated — gets a new ID from GitHub"
        );

        // Color preservation: legacy colours forward through the rename.
        assert_eq!(healed.option_colors.get("Ready"), Some(&"GREEN".to_owned()));
        assert_eq!(
            healed.option_colors.get("In Progress"),
            Some(&"YELLOW".to_owned())
        );
    }

    // ── REQUIRED_ISSUE_TYPES (unblock-wgj.14) ───────────────────────────

    #[test]
    fn required_issue_types_derived_from_enum_in_declared_order() {
        // unblock-wgj.14: REQUIRED_ISSUE_TYPES MUST be generated from
        // IssueType::ALL at compile time — adding a new variant is the
        // single edit site for the canonical taxonomy.
        assert_eq!(REQUIRED_ISSUE_TYPES.len(), IssueType::ALL.len());
        assert_eq!(REQUIRED_ISSUE_TYPES.len(), 8);

        for (i, variant) in IssueType::ALL.iter().enumerate() {
            assert_eq!(REQUIRED_ISSUE_TYPES[i].name, variant.canonical_name());
            assert_eq!(REQUIRED_ISSUE_TYPES[i].color, variant.canonical_color());
            assert_eq!(
                REQUIRED_ISSUE_TYPES[i].description,
                variant.canonical_description()
            );
        }
    }

    #[test]
    fn required_issue_types_color_palette_matches_spec() {
        // Spec §2.6: color palette pinned byte-for-byte. Mirrors the
        // `Status` discipline — adding a literal here would violate
        // Invariant 17.
        let by_name: HashMap<&str, &str> = REQUIRED_ISSUE_TYPES
            .iter()
            .map(|spec| (spec.name, spec.color))
            .collect();
        assert_eq!(by_name.get("Task"), Some(&"yellow"));
        assert_eq!(by_name.get("Bug"), Some(&"red"));
        assert_eq!(by_name.get("Feature"), Some(&"blue"));
        assert_eq!(by_name.get("Spike"), Some(&"purple"));
        assert_eq!(by_name.get("Epic"), Some(&"green"));
        assert_eq!(by_name.get("Chore"), Some(&"gray"));
        assert_eq!(by_name.get("Refactor"), Some(&"orange"));
        assert_eq!(by_name.get("Docs"), Some(&"pink"));
    }

    // ── ensure_issue_types (unblock-wgj.16) ─────────────────────────────

    #[tokio::test]
    async fn ensure_issue_types_creates_only_missing_types() {
        // Org has three pre-existing types (mixed case) — must be matched
        // case-insensitively (mirrors §5.7 `normalize_option_name`) and
        // skipped. The remaining five are POST'd in declared order.
        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        Mock::given(method("GET"))
            .and(path("/orgs/test-owner/issue-types"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "id": 1, "name": "task", "color": "yellow", "description": "" },
                { "id": 2, "name": "BUG", "color": "red", "description": "" },
                { "id": 3, "name": "Feature", "color": "blue", "description": "" }
            ])))
            .expect(1)
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/orgs/test-owner/issue-types"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "id": 99,
                "name": "<placeholder>"
            })))
            .expect(5)
            .mount(&server)
            .await;

        let created = client
            .ensure_issue_types("test-owner")
            .await
            .expect("ensure_issue_types should succeed");

        // Five missing types created in IssueType::ALL declared order.
        // The first three (Task, Bug, Feature) were pre-existing.
        assert_eq!(
            created,
            vec![
                "Spike".to_owned(),
                "Epic".to_owned(),
                "Chore".to_owned(),
                "Refactor".to_owned(),
                "Docs".to_owned(),
            ]
        );
    }

    #[tokio::test]
    async fn ensure_issue_types_returns_empty_when_all_eight_already_exist() {
        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        Mock::given(method("GET"))
            .and(path("/orgs/test-owner/issue-types"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "id": 1, "name": "Task" },
                { "id": 2, "name": "Bug" },
                { "id": 3, "name": "Feature" },
                { "id": 4, "name": "Spike" },
                { "id": 5, "name": "Epic" },
                { "id": 6, "name": "Chore" },
                { "id": 7, "name": "Refactor" },
                { "id": 8, "name": "Docs" }
            ])))
            .expect(1)
            .mount(&server)
            .await;

        // No POST mocks — any creation attempt would error out the
        // request and the test would fail.

        let created = client
            .ensure_issue_types("test-owner")
            .await
            .expect("ensure_issue_types should succeed");

        assert!(created.is_empty(), "got: {created:?}");
    }

    #[tokio::test]
    async fn ensure_issue_types_surfaces_403_as_management_forbidden() {
        // Spec §12 / §13.3: token without `admin:org` → HTTP 403 →
        // `IssueTypeManagementForbidden { org }`. Remediation hint
        // points at upgrading the token (unblock-wgj.21).
        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        Mock::given(method("GET"))
            .and(path("/orgs/test-owner/issue-types"))
            .respond_with(ResponseTemplate::new(403).set_body_string("Forbidden"))
            .expect(1)
            .mount(&server)
            .await;

        let err = client
            .ensure_issue_types("test-owner")
            .await
            .expect_err("403 should surface as IssueTypeManagementForbidden");

        match err {
            Error::IssueTypeManagementForbidden { org } => {
                assert_eq!(org, "test-owner");
            }
            other => panic!("expected IssueTypeManagementForbidden, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn ensure_issue_types_403_on_post_surfaces_as_management_forbidden() {
        // The GET succeeds (`read:org` is enough to list) but the POST
        // fails 403 because the token lacks `admin:org`. Same error
        // surfaces.
        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        Mock::given(method("GET"))
            .and(path("/orgs/test-owner/issue-types"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .expect(1)
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/orgs/test-owner/issue-types"))
            .respond_with(ResponseTemplate::new(403).set_body_string("Forbidden"))
            .expect(1)
            .mount(&server)
            .await;

        let err = client
            .ensure_issue_types("test-owner")
            .await
            .expect_err("POST 403 should surface as IssueTypeManagementForbidden");

        match err {
            Error::IssueTypeManagementForbidden { org } => {
                assert_eq!(org, "test-owner");
            }
            other => panic!("expected IssueTypeManagementForbidden, got: {other:?}"),
        }
    }

    // ── query_issue_types_status (parent bead unblock-wgj WARNING 2) ───

    #[tokio::test]
    async fn query_issue_types_status_returns_canonical_diff_without_posting() {
        // Mirrors `ensure_issue_types_creates_only_missing_types` but
        // exercises the dry-run path: GET only, no POST. The five
        // missing canonical names are surfaced in `IssueType::ALL`
        // declared order so write-path and dry-run output sequences
        // match byte-for-byte (Invariant 18, §14).
        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        Mock::given(method("GET"))
            .and(path("/orgs/test-owner/issue-types"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "id": 1, "name": "task", "color": "yellow", "description": "" },
                { "id": 2, "name": "BUG", "color": "red", "description": "" },
                { "id": 3, "name": "Feature", "color": "blue", "description": "" }
            ])))
            .expect(1)
            .mount(&server)
            .await;

        // No POST mock — the dry-run path MUST NOT POST. If it does
        // wiremock will return 404 and the test will fail.

        let missing = client
            .query_issue_types_status("test-owner")
            .await
            .expect("query_issue_types_status should succeed");

        assert_eq!(
            missing,
            vec![
                "Spike".to_owned(),
                "Epic".to_owned(),
                "Chore".to_owned(),
                "Refactor".to_owned(),
                "Docs".to_owned(),
            ]
        );
    }

    #[tokio::test]
    async fn query_issue_types_status_returns_empty_when_all_eight_already_exist() {
        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        Mock::given(method("GET"))
            .and(path("/orgs/test-owner/issue-types"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "id": 1, "name": "Task" },
                { "id": 2, "name": "Bug" },
                { "id": 3, "name": "Feature" },
                { "id": 4, "name": "Spike" },
                { "id": 5, "name": "Epic" },
                { "id": 6, "name": "Chore" },
                { "id": 7, "name": "Refactor" },
                { "id": 8, "name": "Docs" }
            ])))
            .expect(1)
            .mount(&server)
            .await;

        let missing = client
            .query_issue_types_status("test-owner")
            .await
            .expect("query_issue_types_status should succeed");

        assert!(missing.is_empty(), "got: {missing:?}");
    }

    #[tokio::test]
    async fn query_issue_types_status_surfaces_403_as_management_forbidden() {
        // The dry-run path requires the same `read:org` scope as the
        // write path's GET. A 403 surfaces typed so operators see the
        // same actionable error in `setup --dry-run` as in `setup`.
        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        Mock::given(method("GET"))
            .and(path("/orgs/test-owner/issue-types"))
            .respond_with(ResponseTemplate::new(403).set_body_string("Forbidden"))
            .expect(1)
            .mount(&server)
            .await;

        let err = client
            .query_issue_types_status("test-owner")
            .await
            .expect_err("403 should surface as IssueTypeManagementForbidden");

        match err {
            Error::IssueTypeManagementForbidden { org } => {
                assert_eq!(org, "test-owner");
            }
            other => panic!("expected IssueTypeManagementForbidden, got: {other:?}"),
        }
    }

    // ── setup_fields User-owned no-op (parent bead unblock-wgj WARNING 3,
    //     Appendix B.3 obligation #2 second bullet) ──────────────────────

    /// User-owned repo: the `setup_fields` `IssueType` ensure-and-heal
    /// step is a no-op. The org-level `/orgs/{org}/issue-types` REST
    /// surface does NOT exist for users, so the call MUST short-circuit
    /// on the `OwnerType::User` branch (projects.rs §5.7 step 3, gated
    /// at the top of `setup_fields`). The returned
    /// `SetupReport.issue_types_created` MUST be `vec![]`.
    ///
    /// The test mocks:
    ///   1. POST /graphql → returns all 7 required fields with options
    ///      that match the spec exactly. This drives `setup_fields`
    ///      down the fast-path (no heal, no create) so the only
    ///      remaining decision is the `IssueType` branch.
    ///   2. GET /users/test-owner → returns `User` so
    ///      `detect_owner_type` picks the no-op branch.
    ///
    /// No `/orgs/.../issue-types` mock is mounted — if the User branch
    /// were broken and the call dispatched anyway, wiremock would reply
    /// 404 and the test would fail.
    #[tokio::test]
    async fn setup_fields_user_owner_skips_issue_type_ensure_and_returns_empty_bucket() {
        let server = MockServer::start().await;

        // GraphQL mock: return all 7 required fields with their
        // canonical option sets so `heal_select_field_options` takes
        // the fast-path (no mutation) on every single-select field.
        // Plain fields (Agent/ClaimedAt/StoryPoints/DeferUntil) need
        // only an id + name.
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "node": {
                        "fields": {
                            "pageInfo": { "endCursor": null, "hasNextPage": false },
                            "nodes": [
                                {
                                    "id": "FIELD_STATUS",
                                    "name": "Status",
                                    "dataType": "SINGLE_SELECT",
                                    "options": [
                                        { "id": "OPT_BACKLOG", "name": "Backlog", "color": "GRAY" },
                                        { "id": "OPT_READY", "name": "Ready", "color": "GREEN" },
                                        { "id": "OPT_IN_PROGRESS", "name": "In Progress", "color": "YELLOW" },
                                        { "id": "OPT_BLOCKED", "name": "Blocked", "color": "RED" },
                                        { "id": "OPT_DEFERRED", "name": "Deferred", "color": "BLUE" },
                                        { "id": "OPT_CLOSED", "name": "Closed", "color": "PURPLE" },
                                    ]
                                },
                                {
                                    "id": "FIELD_PRIORITY",
                                    "name": "Priority",
                                    "dataType": "SINGLE_SELECT",
                                    "options": [
                                        { "id": "OPT_P0", "name": "P0 - Critical" },
                                        { "id": "OPT_P1", "name": "P1 - High" },
                                        { "id": "OPT_P2", "name": "P2 - Medium" },
                                        { "id": "OPT_P3", "name": "P3 - Low" },
                                        { "id": "OPT_P4", "name": "P4 - Backlog" },
                                    ]
                                },
                                {
                                    "id": "FIELD_PIPELINE",
                                    "name": "PipelineStage",
                                    "dataType": "SINGLE_SELECT",
                                    "options": [
                                        { "id": "OPT_INV", "name": "investigation" },
                                        { "id": "OPT_IMPL", "name": "implementation" },
                                        { "id": "OPT_REV", "name": "review" },
                                        { "id": "OPT_REF", "name": "refactoring" },
                                        { "id": "OPT_QA", "name": "qa" },
                                        { "id": "OPT_DONE", "name": "done" },
                                    ]
                                },
                                { "id": "FIELD_AGENT", "name": "Agent", "dataType": "TEXT" },
                                { "id": "FIELD_CLAIMED", "name": "ClaimedAt", "dataType": "DATE" },
                                { "id": "FIELD_POINTS", "name": "StoryPoints", "dataType": "NUMBER" },
                                { "id": "FIELD_DEFER", "name": "DeferUntil", "dataType": "DATE" }
                            ]
                        }
                    }
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        // detect_owner_type: User account.
        Mock::given(method("GET"))
            .and(path("/users/test-owner"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "login": "test-owner",
                "type": "User"
            })))
            .expect(1)
            .mount(&server)
            .await;

        // No /orgs/test-owner/issue-types mock. If the User branch is
        // broken and a request is dispatched, wiremock returns 404 and
        // the test fails.

        let client = GitHubClient::new_for_test(&server.uri());

        let report = client
            .setup_fields("PROJECT_ID")
            .await
            .expect("setup_fields should succeed on User-owned repo");

        // All 7 fields fast-pathed to skipped (no heal, no create).
        assert!(
            report.created.is_empty(),
            "no fields should be created: {:?}",
            report.created
        );
        assert!(
            report.healed.is_empty(),
            "no fields should be healed: {:?}",
            report.healed
        );
        assert_eq!(
            report.skipped.len(),
            crate::projects::REQUIRED_FIELD_NAMES.len(),
            "all 7 required fields should be skipped"
        );

        // The contract under test: User-owned repo MUST yield an empty
        // `issue_types_created` bucket. Spec §5.7 step 3, Appendix B.3
        // obligation #2 second bullet.
        assert!(
            report.issue_types_created.is_empty(),
            "User-owned repo must skip org-level issue type ensure: got {:?}",
            report.issue_types_created
        );
    }
}
