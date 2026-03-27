//! MCP server bootstrap and state management.
//!
//! [`ServerState`] holds [`GitHubClient`](unblock_github::client::GitHubClient),
//! [`GraphCache`](unblock_core::cache::GraphCache), and
//! [`Config`](unblock_core::config::Config). [`UnblockServer`] implements the rmcp
//! [`ServerHandler`](rmcp::ServerHandler) trait, exposing MCP tools over stdio transport.
//!
//! The server is constructed via [`UnblockServer::new`], which takes ownership of a
//! [`ServerState`] and wraps it in an [`Arc`] for shared access across tool handlers.

use std::sync::Arc;

use crate::tools::execute_read_tool;
use crate::tools::execute_write_tool;
use crate::tools::init::{InitParams, InitResult};
use crate::tools::setup::{SetupParams, SetupResult};
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
- Read tools (ready, show, depends, init) use the cache for fast responses.
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
