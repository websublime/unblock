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

use crate::client::GitHubClient;
use crate::errors::{self, Error};
use crate::graphql::parse_rate_limit_reset;

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
    /// Status field — single select: Backlog, In Progress, Done, Blocked, Deferred.
    pub status: FieldMeta,
    /// Priority field — single select: P0, P1, P2, P3, P4.
    pub priority: FieldMeta,
    /// `IssueType` field — single select: Task, Bug, Feature, Epic, Chore.
    pub issue_type: FieldMeta,
    /// Agent field — text field (node ID only, no options).
    pub agent: String,
    /// `StoryPoints` field — number field (node ID only).
    pub story_points: String,
    /// `DeferUntil` field — date field (node ID only).
    pub defer_until: String,
    /// `ReadyState` field — single select: Ready, Not Ready.
    pub ready_state: FieldMeta,
}

/// Metadata for a single-select Projects V2 field.
///
/// Contains the field's node ID and a map from option display name to option
/// node ID, enabling the caller to resolve an option name (e.g. `"P1"`) to the
/// GraphQL ID required by `updateProjectV2ItemFieldValue`.
#[derive(Debug, Clone)]
pub struct FieldMeta {
    /// GraphQL node ID for the field.
    pub field_id: String,
    /// Map of option display name to option node ID.
    pub options: HashMap<String, String>,
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
pub const REQUIRED_FIELD_NAMES: &[&str] = &[
    "Status",
    "Priority",
    "IssueType",
    "Agent",
    "StoryPoints",
    "DeferUntil",
    "ReadyState",
];

/// Result of a `setup_fields()` call, including which fields were created
/// vs. skipped (already existed).
///
/// This is the enriched return type that enables the MCP setup tool to report
/// per-field creation status to the agent.
#[derive(Debug, Clone)]
pub struct SetupReport {
    /// The resolved field IDs for all 7 required fields.
    pub field_ids: ProjectFieldIds,
    /// Canonical names of fields that were newly created.
    pub created: Vec<String>,
    /// Canonical names of fields that already existed and were skipped.
    pub skipped: Vec<String>,
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

/// The GitHub REST API version header value for Projects V2 view and field
/// endpoints. This version is newer than the default `2022-11-28` and must
/// be sent as a per-request header override for `/projectsV2/*/views` and
/// `/projectsV2/*/fields` endpoints only.
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

/// The 7 required Projects V2 custom fields per the bead specification.
const REQUIRED_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "Status",
        data_type: "SINGLE_SELECT",
        options: &["Backlog", "In Progress", "Done", "Blocked", "Deferred"],
    },
    FieldSpec {
        name: "Priority",
        data_type: "SINGLE_SELECT",
        options: &["P0", "P1", "P2", "P3", "P4"],
    },
    FieldSpec {
        name: "IssueType",
        data_type: "SINGLE_SELECT",
        options: &["Task", "Bug", "Feature", "Epic", "Chore"],
    },
    FieldSpec {
        name: "Agent",
        data_type: "TEXT",
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
    FieldSpec {
        name: "ReadyState",
        data_type: "SINGLE_SELECT",
        options: &["Ready", "Not Ready"],
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
#[derive(Debug, Deserialize)]
struct OptionNode {
    id: String,
    name: String,
}

/// Removes a single-select [`FieldMeta`] from the map, returning a GraphQL
/// error if the field was not resolved.
fn remove_field(map: &mut HashMap<String, FieldMeta>, name: &str) -> Result<FieldMeta, Error> {
    map.remove(name).ok_or_else(|| {
        errors::GitHubGraphQLSnafu {
            errors: vec![format!("Required field '{name}' was not resolved")],
        }
        .build()
    })
}

/// Removes a plain field ID from the map, returning a GraphQL error if the
/// field was not resolved.
fn remove_plain_field(map: &mut HashMap<String, String>, name: &str) -> Result<String, Error> {
    map.remove(name).ok_or_else(|| {
        errors::GitHubGraphQLSnafu {
            errors: vec![format!("Required field '{name}' was not resolved")],
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
                errors: vec![format!(
                    "Project V2 #{project_number} not found on {}/{}",
                    self.owner(),
                    self.repo()
                )],
            }
            .build());
        }

        debug!(project_number, project_id = %project_id, "Resolved project V2");

        let number = u32::try_from(project_number).map_err(|_| {
            errors::GitHubGraphQLSnafu {
                errors: vec![format!("Project number {project_number} exceeds u32::MAX")],
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

        for spec in REQUIRED_FIELDS {
            if let Some(existing_field) = existing.get(spec.name) {
                debug!(
                    field = spec.name,
                    "Field already exists — skipping creation"
                );
                skipped.push(spec.name.to_owned());
                if spec.options.is_empty() {
                    resolved_plain.insert(spec.name.to_owned(), existing_field.field_id.clone());
                } else {
                    resolved.insert(spec.name.to_owned(), existing_field.clone());
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
            issue_type: remove_field(&mut resolved, "IssueType")?,
            agent: remove_plain_field(&mut resolved_plain, "Agent")?,
            story_points: remove_plain_field(&mut resolved_plain, "StoryPoints")?,
            defer_until: remove_plain_field(&mut resolved_plain, "DeferUntil")?,
            ready_state: remove_field(&mut resolved, "ReadyState")?,
        };

        debug!(
            created_count = created.len(),
            skipped_count = skipped.len(),
            "All 7 project fields resolved"
        );
        Ok(SetupReport {
            field_ids,
            created,
            skipped,
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
                    for opt in options.unwrap_or_default() {
                        option_map.insert(opt.name, opt.id);
                    }

                    fields.insert(
                        name,
                        FieldMeta {
                            field_id: id,
                            options: option_map,
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
                errors: vec![format!("Failed to create field '{}'", spec.name)],
            }
            .build());
        }

        debug!(field = spec.name, field_id = %field_id, "Created non-select field");

        Ok(FieldMeta {
            field_id,
            options: HashMap::new(),
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
                errors: vec![format!(
                    "Failed to create single-select field '{}'",
                    spec.name
                )],
            }
            .build());
        }

        let mut option_map = HashMap::new();
        if let Some(opts) = field_data["options"].as_array() {
            for opt in opts {
                if let (Some(id), Some(name)) = (opt["id"].as_str(), opt["name"].as_str()) {
                    option_map.insert(name.to_owned(), id.to_owned());
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
        })
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

        let status = response.status();

        if status.as_u16() == 429 {
            let reset_at = parse_rate_limit_reset(&response);
            return Err(errors::RateLimitedSnafu { reset_at }.build());
        }

        if !status.is_success() {
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_owned());
            return Err(errors::GitHubApiSnafu {
                status: status.as_u16(),
                message,
            }
            .build());
        }

        let user_info: UserTypeResponse = response
            .json()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        let owner_type = if user_info.account_type == "Organization" {
            OwnerType::Org
        } else {
            OwnerType::User
        };

        debug!(
            owner = %self.owner(),
            account_type = %user_info.account_type,
            owner_type = ?owner_type,
            "Detected owner type"
        );

        Ok(owner_type)
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

        let status = response.status();

        if status.as_u16() == 429 {
            let reset_at = parse_rate_limit_reset(&response);
            return Err(errors::RateLimitedSnafu { reset_at }.build());
        }

        if !status.is_success() {
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_owned());
            return Err(errors::GitHubApiSnafu {
                status: status.as_u16(),
                message,
            }
            .build());
        }

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

        let status = response.status();

        if status.as_u16() == 429 {
            let reset_at = parse_rate_limit_reset(&response);
            return Err(errors::RateLimitedSnafu { reset_at }.build());
        }

        if !status.is_success() {
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_owned());
            return Err(errors::GitHubApiSnafu {
                status: status.as_u16(),
                message,
            }
            .build());
        }

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
        // org-owned projects per research findings.
        let query = match owner_type {
            OwnerType::Org => {
                "
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
            "
            }
            OwnerType::User => {
                "
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
            "
            }
        };

        let variables = serde_json::json!({
            "login": self.owner(),
            "projectNumber": project_number,
        });

        let response = self.graphql(query, variables).await?;

        // The data path differs based on owner type.
        let owner_key = match owner_type {
            OwnerType::Org => "organization",
            OwnerType::User => "user",
        };

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
                errors: vec![format!(
                    "Could not resolve node ID for {} '{}' — check the owner name",
                    match owner_type {
                        OwnerType::Org => "organization",
                        OwnerType::User => "user",
                    },
                    self.owner()
                )],
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
                errors: vec![format!(
                    "createProjectV2 mutation returned empty project data for title '{title}'"
                )],
            }
            .build());
        }

        debug!(number, url = %url, "Created project V2");
        Ok(CreatedProject { number, url })
    }
}
