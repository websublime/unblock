//! MCP server bootstrap and state management.
//!
//! [`ServerState`](crate::server::ServerState) holds
//! [`GitHubClient`](unblock_github::client::GitHubClient),
//! [`GraphCache`](unblock_core::cache::GraphCache), and
//! [`Config`](unblock_core::config::Config).
//! [`UnblockServer`](crate::server::UnblockServer) implements the rmcp
//! [`ServerHandler`](rmcp::ServerHandler) trait, exposing MCP tools over stdio transport.
//!
//! The server is constructed via
//! [`UnblockServer::new`](crate::server::UnblockServer::new), which takes ownership of a
//! [`ServerState`](crate::server::ServerState) and wraps it in an
//! [`Arc`](std::sync::Arc) for shared access across tool handlers.

use std::sync::Arc;

use crate::tools::create::{CreateParams, CreateResult};
use crate::tools::execute_read_tool;
use crate::tools::execute_write_tool;
use crate::tools::init::{InitParams, InitResult};
use crate::tools::ready::{ReadyParams, ReadyResult};
use crate::tools::setup::{SetupParams, SetupResult};
use crate::tools::show::{
    DependencyTreeEntry, ShowBodySections, ShowComment, ShowIssue, ShowParams, ShowRelatedIssue,
    ShowResult,
};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{ErrorData, Implementation, ServerInfo};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use tracing::info;
use unblock_core::cache::GraphCache;
use unblock_core::config::Config;
use unblock_github::client::GitHubClient;

/// Instructions string injected into the agent context window.
///
/// Provides a concise reference card for agents describing each tool, its purpose,
/// key parameters, and when to use it. This is sent as part of the MCP `ServerInfo`
/// during initialization.
pub const INSTRUCTIONS_STR: &str = "\
# unblock — Dependency-Aware Task Tracking

unblock turns GitHub Issues into a dependency graph. Ask `ready` to get unblocked work.

## Workflow
1. `init` — Bootstrap a new Projects V2 project (run once per repository)
2. `setup` — Configure fields and views on the project (run once per session)
3. `ready` — Get issues with no active blockers (the main entry point)
4. `claim` — Assign yourself to an issue before starting work
5. `close` — Close a completed issue (auto-unblocks dependents)

## Tools

### Core Workflow
| Tool    | Purpose                                              | Key Params                          |
|---------|------------------------------------------------------|-------------------------------------|
| init    | Bootstrap a new Projects V2 project                  | scope?, title?, description?        |
| setup   | Set target repo and project                          | owner, repo, project_number?        |
| ready   | Find issues that can be worked on right now           | limit?, type?, priority?, agent?    |
| claim   | Assign yourself to an issue                          | issue_number, agent?                |
| close   | Close an issue and cascade-unblock dependents        | issue_number                        |
| create  | Create a new issue with optional dependencies        | title, body?, blocked_by?           |

### Query & Dependencies
| Tool    | Purpose                                              | Key Params                          |
|---------|------------------------------------------------------|-------------------------------------|
| show    | Get full details for a single issue                  | issue_number                        |
| depends | Show the dependency tree for an issue                | issue_number, direction?            |
| comment | Add a comment to an issue                            | issue_number, body                  |
| update  | Update issue fields (priority, labels, body, etc.)   | issue_number, fields...             |

## Tips
- Run `init` once to create a project, then `setup` to configure it.
- Always call `ready` first to find unblocked work.
- Use `claim` before starting work to prevent conflicts.
- After `close`, dependents are automatically re-evaluated.
- Write tools (create, close, update, comment, claim) trigger a graph rebuild.
- Read tools (ready, show, depends) use the cache for fast responses.
- Bootstrap tools (init, setup) manage the project itself and do not affect the dependency graph.
";

/// Shared state for all MCP tool handlers.
///
/// Holds the GitHub API client, the in-memory graph cache, and the application
/// configuration. All fields are wrapped in [`Arc`] so that `ServerState` itself
/// can be shared across tool handlers via `Arc<ServerState>`.
///
/// # Thread Safety
///
/// `ServerState` is `Send + Sync` because all inner types are `Send + Sync`:
/// - [`Config`] is `Clone + Send + Sync` (plain data).
/// - [`GitHubClient`] wraps `reqwest::Client` which is `Send + Sync`.
/// - [`GraphCache`] uses `tokio::sync::RwLock` which is `Send + Sync`.
#[derive(Debug)]
pub struct ServerState {
    /// Application configuration loaded from environment variables.
    #[allow(dead_code)] // Used by tool handlers added in beads 45a.4–45a.11.
    pub config: Arc<Config>,
    /// GitHub API client for GraphQL and REST operations.
    pub client: Arc<GitHubClient>,
    /// In-memory cache for the dependency graph and ready set.
    pub cache: Arc<GraphCache>,
}

/// MCP server implementation for unblock.
///
/// Wraps [`ServerState`] in an [`Arc`] and provides tool routing via rmcp macros.
/// Implements [`ServerHandler`] to serve MCP requests over stdio transport.
///
/// Constructed via [`UnblockServer::new`].
pub struct UnblockServer {
    /// Shared server state accessible by all tool handlers.
    state: Arc<ServerState>,
    /// Tool router generated by the `#[tool_router]` macro.
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

impl UnblockServer {
    /// Creates a new `UnblockServer` from the given state.
    ///
    /// Wraps the state in an `Arc` for shared access across tool handlers
    /// and initializes the tool router.
    #[must_use]
    pub fn new(state: ServerState) -> Self {
        Self {
            state: Arc::new(state),
            tool_router: Self::tool_router(),
        }
    }

    /// Returns a reference to the shared server state.
    #[must_use]
    pub fn state(&self) -> &Arc<ServerState> {
        &self.state
    }
}

/// Set project fields on a newly created issue's project item.
///
/// Updates Priority, `IssueType`, Status, `ReadyState`, `StoryPoints`, and
/// `DeferUntil`. Each field update is best-effort: failures are logged as
/// warnings but do not abort the remaining updates. This keeps the create flow
/// resilient to partial project configuration (e.g. missing option values).
#[allow(clippy::too_many_arguments)]
async fn set_project_fields(
    client: &GitHubClient,
    project_id: &str,
    item_id: &str,
    field_ids: &unblock_github::projects::ProjectFieldIds,
    priority: &str,
    issue_type: &str,
    ready_state: &str,
    story_points: Option<f64>,
    defer_until: Option<chrono::NaiveDate>,
) {
    use unblock_github::projects::FieldValue;

    // Set Priority.
    if let Some(option_id) = field_ids.priority.options.get(priority)
        && let Err(e) = client
            .update_field(
                project_id,
                item_id,
                &field_ids.priority.field_id,
                &FieldValue::SingleSelectOption(option_id.clone()),
            )
            .await
    {
        tracing::warn!(error = %e, "Failed to set Priority field");
    }

    // Set IssueType.
    if let Some(option_id) = field_ids.issue_type.options.get(issue_type)
        && let Err(e) = client
            .update_field(
                project_id,
                item_id,
                &field_ids.issue_type.field_id,
                &FieldValue::SingleSelectOption(option_id.clone()),
            )
            .await
    {
        tracing::warn!(error = %e, "Failed to set IssueType field");
    }

    // Set Status to Backlog.
    if let Some(option_id) = field_ids.status.options.get("Backlog")
        && let Err(e) = client
            .update_field(
                project_id,
                item_id,
                &field_ids.status.field_id,
                &FieldValue::SingleSelectOption(option_id.clone()),
            )
            .await
    {
        tracing::warn!(error = %e, "Failed to set Status field");
    }

    // Set ReadyState.
    if let Some(option_id) = field_ids.ready_state.options.get(ready_state)
        && let Err(e) = client
            .update_field(
                project_id,
                item_id,
                &field_ids.ready_state.field_id,
                &FieldValue::SingleSelectOption(option_id.clone()),
            )
            .await
    {
        tracing::warn!(error = %e, "Failed to set ReadyState field");
    }

    // Set StoryPoints if provided.
    if let Some(sp) = story_points
        && let Err(e) = client
            .update_field(
                project_id,
                item_id,
                &field_ids.story_points,
                &FieldValue::Number(sp),
            )
            .await
    {
        tracing::warn!(error = %e, "Failed to set StoryPoints field");
    }

    // Set DeferUntil if provided.
    if let Some(du) = defer_until
        && let Err(e) = client
            .update_field(
                project_id,
                item_id,
                &field_ids.defer_until,
                &FieldValue::Date(du),
            )
            .await
    {
        tracing::warn!(error = %e, "Failed to set DeferUntil field");
    }
}

/// Tool router implementation for MCP tools.
#[tool_router]
impl UnblockServer {
    /// Bootstrap a new Projects V2 project for the repository (idempotent).
    ///
    /// Creates a project container via the `createProjectV2` GraphQL mutation.
    /// If a project with the same title already exists, returns it with
    /// `created: false`. This tool is functional in bootstrap mode (no project
    /// configured) and does not affect the dependency graph.
    #[tool(
        name = "init",
        description = "Bootstrap a new GitHub Projects V2 project for the repository. Idempotent — returns existing project if a matching title is found. Run this before setup on a new repository."
    )]
    async fn init(
        &self,
        Parameters(params): Parameters<InitParams>,
    ) -> Result<Json<InitResult>, ErrorData> {
        let state = self.state();
        let client = &state.client;

        // Step 1: Detect or use provided owner type.
        let owner_type = if let Some(ref scope) = params.scope {
            match scope.to_lowercase().as_str() {
                "org" => unblock_github::projects::OwnerType::Org,
                "user" => unblock_github::projects::OwnerType::User,
                other => {
                    return Err(ErrorData {
                        code: rmcp::model::ErrorCode::INVALID_PARAMS,
                        message: format!("Invalid scope '{other}' — must be 'org' or 'user'")
                            .into(),
                        data: None,
                    });
                }
            }
        } else {
            execute_read_tool(state, || client.detect_owner_type()).await?
        };

        let scope_str = match owner_type {
            unblock_github::projects::OwnerType::Org => "org",
            unblock_github::projects::OwnerType::User => "user",
        };

        info!(
            owner = %client.owner(),
            scope = scope_str,
            "Init tool invoked"
        );

        // Step 2: Resolve owner node ID.
        let owner_node_id =
            execute_read_tool(state, || client.resolve_owner_node_id(owner_type)).await?;

        // Step 3: Determine project title.
        let title = params
            .title
            .unwrap_or_else(|| format!("{} Tasks", client.repo()));

        // Step 4: Query existing projects for matching title.
        let existing_projects =
            execute_read_tool(state, || client.list_owner_projects(owner_type)).await?;

        if let Some(existing) = existing_projects.iter().find(|p| p.title == title) {
            info!(
                project_number = existing.number,
                title = %existing.title,
                "Found existing project with matching title"
            );
            return Ok(Json(InitResult {
                project_number: existing.number,
                url: existing.url.clone(),
                created: false,
                scope: scope_str.to_owned(),
                hint: format!(
                    "Project already exists. Run `setup` with project number {} to configure fields and views.",
                    existing.number
                ),
            }));
        }

        // Log forward-compat params that are accepted but not wired.
        if let Some(ref description) = params.description {
            tracing::warn!(
                description = %description,
                "init 'description' parameter is not yet supported by createProjectV2 — ignored"
            );
        }
        if let Some(public) = params.public {
            tracing::warn!(
                public,
                "init 'public' parameter is not yet supported by createProjectV2 — ignored"
            );
        }

        // Step 5: Create a new project.
        let created =
            execute_read_tool(state, || client.create_project(&owner_node_id, &title)).await?;

        info!(
            project_number = created.number,
            url = %created.url,
            "Created new project V2"
        );

        Ok(Json(InitResult {
            project_number: created.number,
            url: created.url,
            created: true,
            scope: scope_str.to_owned(),
            hint: format!(
                "Project created! Run `setup` with project number {} to configure fields and views.",
                created.number
            ),
        }))
    }

    /// Create required Projects V2 fields on the configured project (idempotent).
    ///
    /// This is typically the first tool an agent calls on a fresh repository.
    /// With `dry_run: true`, reports which fields exist and which are missing
    /// without creating anything.
    #[tool(
        name = "setup",
        description = "Create required Projects V2 custom fields (Status, Priority, IssueType, Agent, StoryPoints, DeferUntil, ReadyState). Safe to call repeatedly — existing fields are skipped. Use dry_run=true to check without mutating."
    )]
    async fn setup(
        &self,
        Parameters(params): Parameters<SetupParams>,
    ) -> Result<Json<SetupResult>, ErrorData> {
        let state = self.state();
        let dry_run = params.dry_run.unwrap_or(false);

        // Resolve project info — use param override or configured project number.
        let client = &state.client;

        // The `project` param is accepted for forward-compatibility but not yet
        // wired to resolve_project_info(). Warn agents so the silent ignore is
        // not confusing.
        if let Some(project_number) = params.project {
            tracing::warn!(
                project_number,
                "setup 'project' parameter is not yet supported — using configured project number"
            );
        }

        let project_info = client
            .resolve_project_info()
            .await
            .map_err(crate::errors::github_error_to_mcp)?;

        info!(
            project_id = %project_info.id,
            project_number = project_info.number,
            dry_run,
            "Setup tool invoked"
        );

        if dry_run {
            // Dry-run: query which fields exist without creating anything.
            let status = client
                .query_setup_status(&project_info.id)
                .await
                .map_err(crate::errors::github_error_to_mcp)?;

            return Ok(Json(SetupResult {
                fields_created: Vec::new(),
                fields_skipped: status.existing,
                fields_missing: status.missing,
                project_id: project_info.id,
            }));
        }

        // Write path: create fields, invalidate cache, rebuild.
        let project_id = project_info.id.clone();
        let client_clone = Arc::clone(client);

        let report = execute_write_tool(state, || async move {
            client_clone.setup_fields(&project_id).await
        })
        .await?;

        // Cache the resolved field IDs on the client for subsequent update_field calls.
        client.set_field_ids(report.field_ids).await;

        Ok(Json(SetupResult {
            fields_created: report.created,
            fields_skipped: report.skipped,
            fields_missing: Vec::new(),
            project_id: project_info.id,
        }))
    }

    /// Fetch full detail for a single issue.
    ///
    /// Returns the complete issue with parsed body sections, blocking/blocked-by
    /// relationships, an optional dependency tree (from the cached graph up to
    /// depth 3), and optional comments.
    ///
    /// This is a read tool — it does not mutate state or invalidate the cache.
    #[tool(
        name = "show",
        description = "Get full details for a single issue: body sections, blocking relationships, dependency tree (from cache), and comments. Use include_comments=false or include_deps=false to skip optional sections."
    )]
    async fn show(
        &self,
        Parameters(params): Parameters<ShowParams>,
    ) -> Result<Json<ShowResult>, ErrorData> {
        let state = self.state();
        let client = &state.client;

        let include_comments = params.include_comments.unwrap_or(true);
        let include_deps = params.include_deps.unwrap_or(true);
        let issue_number = params.id;

        info!(
            issue_number,
            include_comments, include_deps, "Show tool invoked"
        );

        // Step 1: Fetch the full issue via execute_read_tool.
        let issue = execute_read_tool(state, || client.fetch_issue(issue_number)).await?;

        // Step 2: Parse body sections.
        let body_sections = unblock_core::types::BodySections::from_markdown(
            issue.body.as_deref().unwrap_or_default(),
        );

        // Step 3: Extract blocking/blocked_by from the issue.
        let blocking: Vec<ShowRelatedIssue> = issue
            .blocking
            .iter()
            .map(|r| ShowRelatedIssue {
                number: r.number,
                title: r.title.clone(),
                state: format!("{:?}", r.state),
            })
            .collect();

        let blocked_by: Vec<ShowRelatedIssue> = issue
            .blocked_by
            .iter()
            .map(|r| ShowRelatedIssue {
                number: r.number,
                title: r.title.clone(),
                state: format!("{:?}", r.state),
            })
            .collect();

        // Step 4: If include_deps, get dependency tree from cached graph.
        let dependency_tree = if include_deps {
            state.cache.get_graph().await.map(|graph| {
                graph
                    .dependency_tree(
                        issue_number,
                        unblock_core::types::TraversalDirection::Both,
                        3,
                    )
                    .into_iter()
                    .map(|(num, depth)| DependencyTreeEntry {
                        issue_number: num,
                        depth,
                    })
                    .collect()
            })
        } else {
            None
        };

        // Step 5: If include_comments, include comments from the issue.
        let comments = if include_comments {
            Some(
                issue
                    .comments
                    .iter()
                    .map(|c| ShowComment {
                        author: c.author.clone(),
                        body: c.body.clone(),
                        created_at: c.created_at.to_rfc3339(),
                    })
                    .collect(),
            )
        } else {
            None
        };

        // Step 6: Build the ShowIssue from the fetched Issue.
        let show_issue = ShowIssue {
            number: issue.number,
            node_id: issue.node_id.clone(),
            title: issue.title.clone(),
            issue_type: issue.issue_type.map(|it| format!("{it:?}")),
            status: format!("{:?}", issue.status),
            priority: format!("{:?}", issue.priority),
            agent: issue.agent.clone(),
            claimed_at: issue.claimed_at.map(|dt| dt.to_rfc3339()),
            ready_state: format!("{:?}", issue.ready_state),
            story_points: issue.story_points,
            defer_until: issue.defer_until.map(|d| d.to_string()),
            labels: issue.labels.clone(),
            milestone: issue.milestone.clone(),
            assignees: issue.assignees.clone(),
            state: format!("{:?}", issue.state),
            body: issue.body.clone(),
            created_at: issue.created_at.to_rfc3339(),
            updated_at: issue.updated_at.to_rfc3339(),
            url: issue.url.clone(),
        };

        Ok(Json(ShowResult {
            issue: show_issue,
            body_sections: ShowBodySections {
                description: body_sections.description,
                design_notes: body_sections.design_notes,
                acceptance_criteria: body_sections.acceptance_criteria,
            },
            blocking,
            blocked_by,
            dependency_tree,
            comments,
        }))
    }

    /// Find issues with no active blockers that can be worked on now.
    ///
    /// Returns open, unblocked issues sorted by priority (P0 first) then
    /// creation date (oldest first). The cache is rebuilt lazily if stale.
    ///
    /// By default, deferred issues (`defer_until > today`) and in-progress
    /// (claimed) issues are excluded. Use `include_claimed=true` to include
    /// claimed issues.
    #[tool(
        name = "ready",
        description = "Find issues with no active blockers, sorted by priority. Filters: limit, issue_type, priority, milestone, agent, label, include_claimed. Returns from cache (rebuilds lazily if stale)."
    )]
    async fn ready(
        &self,
        Parameters(params): Parameters<ReadyParams>,
    ) -> Result<Json<ReadyResult>, ErrorData> {
        let state = self.state();

        info!(
            limit = params.limit,
            issue_type = params.issue_type.as_deref(),
            priority = params.priority.as_deref(),
            milestone = params.milestone.as_deref(),
            agent = params.agent.as_deref(),
            label = params.label.as_deref(),
            include_claimed = params.include_claimed,
            "Ready tool invoked"
        );

        // Step 1: Check cache freshness — rebuild lazily if stale.
        if !state.cache.is_fresh().await {
            tracing::debug!("Cache is stale — triggering lazy rebuild");
            crate::tools::rebuild_cache(state).await;
        }

        // Step 2: Get ready set from cache.
        let stale;
        let issues = if let Some(ready_set) = state.cache.get_ready_set().await {
            stale = false;
            crate::tools::ready::filter_ready_set(&ready_set, &params)
        } else {
            // Cache is still empty after rebuild attempt (e.g., fetch failed).
            tracing::warn!("Cache still empty after rebuild — returning stale=true");
            stale = true;
            Vec::new()
        };

        let count = issues.len();

        Ok(Json(ReadyResult {
            issues,
            count,
            stale,
        }))
    }

    /// Create a new GitHub Issue with optional dependencies, project fields,
    /// and parent linkage.
    ///
    /// Creates the issue via REST, adds it to the configured project, sets
    /// custom fields (Priority, `IssueType`, `StoryPoints`, `DeferUntil`, Status,
    /// `ReadyState`), and optionally adds blocking relationships and parent linkage.
    ///
    /// This is a write tool — the cache is invalidated and rebuilt after all
    /// mutations complete.
    #[tool(
        name = "create",
        description = "Create a new GitHub Issue. Set title (required), issue_type (default Task), priority (default P2), body, labels, blocked_by (local number or owner/repo#number), parent, story_points, defer_until. Labels are auto-created if missing. Triggers graph rebuild."
    )]
    async fn create(
        &self,
        Parameters(params): Parameters<CreateParams>,
    ) -> Result<Json<CreateResult>, ErrorData> {
        let state = self.state();
        let client = Arc::clone(&state.client);

        info!(
            title = %params.title,
            issue_type = params.issue_type.as_deref(),
            priority = params.priority.as_deref(),
            "Create tool invoked"
        );

        // Validate issue_type if provided.
        let issue_type_str = params.issue_type.as_deref().unwrap_or("Task");
        if !matches!(
            issue_type_str,
            "Task" | "Bug" | "Feature" | "Epic" | "Chore" | "Spike"
        ) {
            return Err(ErrorData {
                code: rmcp::model::ErrorCode::INVALID_PARAMS,
                message: format!(
                    "Invalid issue_type '{issue_type_str}' — must be Task, Bug, Feature, Epic, Chore, or Spike"
                )
                .into(),
                data: None,
            });
        }

        // Validate priority if provided.
        let priority_str = params.priority.as_deref().unwrap_or("P2");
        if !matches!(priority_str, "P0" | "P1" | "P2" | "P3" | "P4") {
            return Err(ErrorData {
                code: rmcp::model::ErrorCode::INVALID_PARAMS,
                message: format!(
                    "Invalid priority '{priority_str}' — must be P0, P1, P2, P3, or P4"
                )
                .into(),
                data: None,
            });
        }

        // Parse defer_until if provided.
        let defer_until = if let Some(ref date_str) = params.defer_until {
            Some(
                chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d").map_err(|e| ErrorData {
                    code: rmcp::model::ErrorCode::INVALID_PARAMS,
                    message: format!("Invalid defer_until date '{date_str}': {e}").into(),
                    data: None,
                })?,
            )
        } else {
            None
        };

        // Parse blocked_by refs.
        let blocked_by_refs: Vec<unblock_core::types::IssueRef> =
            if let Some(ref refs) = params.blocked_by {
                refs.iter()
                    .map(|s| {
                        s.parse::<unblock_core::types::IssueRef>()
                            .map_err(|e| ErrorData {
                                code: rmcp::model::ErrorCode::INVALID_PARAMS,
                                message: format!("Invalid blocked_by reference '{s}': {e}").into(),
                                data: None,
                            })
                    })
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                Vec::new()
            };

        // Build body — use provided body or generate BodySections template.
        let body = if let Some(ref body_text) = params.body {
            Some(body_text.clone())
        } else {
            // Generate a template with section headers so the issue body has
            // a useful scaffold rather than being empty.
            let sections = unblock_core::types::BodySections {
                description: Some(String::new()),
                design_notes: Some(String::new()),
                acceptance_criteria: Some(String::new()),
            };
            let md = sections.to_markdown();
            if md.is_empty() { None } else { Some(md) }
        };

        let labels = params.labels.clone().unwrap_or_default();

        // Capture params for the closure.
        let title = params.title.clone();
        let parent = params.parent;
        let story_points = params.story_points;
        let issue_type_owned = issue_type_str.to_owned();
        let priority_owned = priority_str.to_owned();
        let milestone_title = params.milestone.clone();

        let result = execute_write_tool(state, || {
            let client = Arc::clone(&client);
            let title = title.clone();
            let body = body.clone();
            let labels = labels.clone();
            let blocked_by_refs = blocked_by_refs.clone();
            let issue_type_owned = issue_type_owned.clone();
            let priority_owned = priority_owned.clone();
            let milestone_title = milestone_title.clone();

            async move {
                // Step 1: Ensure labels exist on the repo.
                if !labels.is_empty() {
                    client.ensure_labels(&labels).await?;
                }

                // Step 2: Log milestone if provided (not yet resolved to ID).
                if let Some(ref ms) = milestone_title {
                    tracing::warn!(
                        milestone = %ms,
                        "Milestone resolution from title to ID is not yet implemented — milestone will not be set on the issue"
                    );
                }

                // Step 3: Create the issue.
                let create_params = unblock_github::mutations::CreateIssueParams {
                    title,
                    body,
                    labels,
                    milestone: None, // Milestone ID resolution not yet implemented.
                    assignees: Vec::new(),
                };

                let issue = client.create_issue(create_params).await?;
                let issue_number = issue.number;
                let issue_node_id = issue.node_id.clone();
                let issue_url = issue.url.clone();
                let issue_title = issue.title.clone();

                // Step 4: Set project fields if project is configured.
                let mut added_to_project = false;
                let mut fields_attempted = false;

                if let Some(field_ids) = client.field_ids().await {
                    // Resolve the project info to get the project ID.
                    match client.resolve_project_info().await {
                        Ok(project_info) => {
                            // Get the project item ID for this issue.
                            match client
                                .get_project_item_id(&issue_node_id, &project_info.id)
                                .await
                            {
                                Ok(item_id) => {
                                    added_to_project = true;

                                    let initial_ready_state = if blocked_by_refs.is_empty() {
                                        "Ready"
                                    } else {
                                        "Not Ready"
                                    };

                                    set_project_fields(
                                        &client,
                                        &project_info.id,
                                        &item_id,
                                        &field_ids,
                                        &priority_owned,
                                        &issue_type_owned,
                                        initial_ready_state,
                                        story_points,
                                        defer_until,
                                    )
                                    .await;

                                    fields_attempted = true;
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        error = %e,
                                        "Failed to get project item ID — fields will not be set"
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "Failed to resolve project info — fields will not be set"
                            );
                        }
                    }
                } else {
                    tracing::debug!(
                        "No field IDs cached — run setup first to enable project field assignment"
                    );
                }

                // Step 5: Add blocking relationships.
                let mut blockers_added: u32 = 0;
                for blocker in &blocked_by_refs {
                    match client.add_blocked_by_ref(issue_number, blocker).await {
                        Ok(()) => blockers_added += 1,
                        Err(e) => {
                            tracing::warn!(
                                blocker = %blocker,
                                error = %e,
                                "Failed to add blocking relationship"
                            );
                        }
                    }
                }

                // Step 6: Add parent relationship.
                let mut parent_set = false;
                if let Some(parent_number) = parent {
                    match client.add_sub_issue(parent_number, issue_number).await {
                        Ok(()) => parent_set = true,
                        Err(e) => {
                            tracing::warn!(
                                parent_number,
                                error = %e,
                                "Failed to add parent relationship"
                            );
                        }
                    }
                }

                Ok(CreateResult {
                    number: issue_number,
                    url: issue_url,
                    title: issue_title,
                    issue_type: issue_type_owned,
                    priority: priority_owned,
                    added_to_project,
                    fields_attempted,
                    blockers_added,
                    parent_set,
                    hint: format!(
                        "Issue #{issue_number} created. Use `show` to verify or `ready` to check if it appears in the ready set."
                    ),
                })
            }
        })
        .await?;

        Ok(Json(result))
    }
}

#[tool_handler]
impl ServerHandler for UnblockServer {
    fn get_info(&self) -> ServerInfo {
        let capabilities = rmcp::model::ServerCapabilities::builder()
            .enable_tools()
            .build();
        ServerInfo::new(capabilities)
            .with_server_info(Implementation::new("unblock", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS_STR)
    }
}

// Static assertions: ServerState must be Send + Sync.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ServerState>();
    assert_send_sync::<Arc<ServerState>>();
};
