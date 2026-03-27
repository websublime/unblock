//! Projects V2 field management.
//!
//! - `resolve_project()` — find linked project by repo
//! - `setup_fields()` — create 7 custom fields (idempotent), returns [`SetupReport`]
//! - `query_setup_status()` — check which fields exist without mutating (for dry-run)
//! - `update_field()` — update a single field value on an issue

use std::collections::HashMap;

use chrono::NaiveDate;
use serde::Deserialize;
use tracing::{debug, instrument, warn};

use crate::client::GitHubClient;
use crate::errors::{self, Error};

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
    async fn fetch_existing_fields(
        &self,
        project_id: &str,
    ) -> Result<HashMap<String, FieldMeta>, Error> {
        let query = "
            query ProjectFields($projectId: ID!) {
                node(id: $projectId) {
                    ... on ProjectV2 {
                        fields(first: 50) {
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

        let variables = serde_json::json!({
            "projectId": project_id,
        });

        let response = self.graphql(query, variables).await?;
        let nodes = response["data"]["node"]["fields"]["nodes"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        let mut fields: HashMap<String, FieldMeta> = HashMap::new();

        for node_value in nodes {
            // Skip null entries that can appear from union types.
            if node_value.is_null() {
                continue;
            }

            let field: FieldNode = match serde_json::from_value(node_value.clone()) {
                Ok(f) => f,
                Err(e) => {
                    warn!(error = %e, "Skipping unparseable field node");
                    continue;
                }
            };

            let mut option_map = HashMap::new();
            if let Some(options) = &field.options {
                for opt in options {
                    option_map.insert(opt.name.clone(), opt.id.clone());
                }
            }

            fields.insert(
                field.name.clone(),
                FieldMeta {
                    field_id: field.id,
                    options: option_map,
                },
            );
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
