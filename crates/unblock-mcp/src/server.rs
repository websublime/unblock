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

use std::sync::{Arc, OnceLock};

use chrono::Utc;

use crate::tools::claim::{ClaimCandidate, ClaimParams, ClaimResult, validate_claimable};
use crate::tools::close::{CloseParams, CloseResult};
use crate::tools::comment::{CommentParams, CommentResult};
use crate::tools::create::{CreateParams, CreateResult};
use crate::tools::depends::{DependsParams, DependsResult};
use crate::tools::execute_read_tool;
use crate::tools::execute_write_tool;
use crate::tools::init::{InitParams, InitResult};
use crate::tools::prime::{PrimeParams, PrimeResult};
use crate::tools::ready::{ReadyParams, ReadyResult};
use crate::tools::reconcile::{ReconcileOutput, ReconcileParams};
use crate::tools::setup::{REQUIRED_VIEWS, SetupParams, SetupResult};
use crate::tools::show::{
    DependencyTreeEntry, ShowBodySections, ShowComment, ShowIssue, ShowParams, ShowRelatedIssue,
    ShowResult,
};
use crate::tools::update::{BodySectionUpdate, SectionName, UpdateParams, UpdateResult};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{
    ErrorData, Implementation, InitializeRequestParams, InitializeResult, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler, tool, tool_handler, tool_router};
use tracing::info;
use unblock_core::cache::GraphCache;
use unblock_core::client::{AgentClient, AgentKind};
use unblock_core::config::Config;
use unblock_core::detection::ClientDetector;
use unblock_core::errors::{CircularDependencySnafu, IssueClosedSnafu};
use unblock_core::types::IssueState;
use unblock_github::client::GitHubClient;
use unblock_github::projects::FieldValue;
use unblock_github::projects::{CreateViewParams, ViewLayout};

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
| close   | Close an issue and cascade-unblock dependents        | id, reason?                         |
| create  | Create a new issue with optional dependencies        | title, body?, blocked_by?           |

### Query & Dependencies
| Tool    | Purpose                                              | Key Params                          |
|---------|------------------------------------------------------|-------------------------------------|
| show    | Get full details for a single issue                  | issue_number                        |
| depends | Show the dependency tree for an issue                | issue_number, direction?            |
| comment | Add a comment to an issue                            | issue_number, body                  |
| update  | Update issue fields (priority, labels, body, etc.)   | issue_number, fields...             |

### Diagnostics
| Tool      | Purpose                                            | Key Params                          |
|-----------|-----------------------------------------------------|-------------------------------------|
| reconcile | Detect drift between graph and GitHub state         | fix?, stale_claim_hours?            |

## Tips
- Run `init` once to create a project, then `setup` to configure it.
- Always call `ready` first to find unblocked work.
- Use `claim` before starting work to prevent conflicts.
- After `close`, dependents are automatically re-evaluated.
- Write tools (create, close, update, claim) trigger a graph rebuild.
- Read tools (ready, show, depends, comment) use the cache for fast responses.
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
/// - [`OnceLock<AgentKind>`] is `Send + Sync` because `AgentKind` is `Send + Sync`.
/// - [`OnceLock<AgentClient>`] is `Send + Sync` because `AgentClient` is `Send + Sync`.
/// - [`OnceLock<DateTime<Utc>>`] is `Send + Sync` because `DateTime<Utc>` is `Send + Sync`.
#[derive(Debug)]
pub struct ServerState {
    /// Application configuration loaded from environment variables.
    #[allow(dead_code)] // Used by tool handlers added in beads 45a.4–45a.11.
    pub config: Arc<Config>,
    /// GitHub API client for GraphQL and REST operations.
    pub client: Arc<GitHubClient>,
    /// In-memory cache for the dependency graph and ready set.
    pub cache: Arc<GraphCache>,
    /// Resolved once during the MCP `initialize` handshake.
    ///
    /// [`OnceLock`] guarantees a single write and lock-free reads thereafter.
    /// Tool handlers access the stored value via [`OnceLock::get`], falling
    /// back to `"unknown"` if the lock has not been set (e.g., in tests or
    /// when `initialize` is not called).
    pub agent_kind: OnceLock<AgentKind>,
    /// Raw MCP `clientInfo` stored once during the `initialize` handshake.
    ///
    /// Used by [`SessionMeta`](crate::tools::prime::SessionMeta) to surface
    /// the raw client name in the `prime` tool output.
    pub agent_client: OnceLock<AgentClient>,
    /// UTC timestamp recorded when `initialize()` is called.
    ///
    /// Represents the session start time. Used by
    /// [`SessionMeta`](crate::tools::prime::SessionMeta) for the
    /// `connected_at` field.
    pub connected_at: OnceLock<chrono::DateTime<Utc>>,
}

impl ServerState {
    /// Returns the normalised agent kind string for use in tracing spans and log fields.
    ///
    /// Reads [`AgentKind`] from the [`OnceLock`] (lock-free) and converts it to its
    /// string representation via [`AgentKind::as_str`]. Falls back to `"unknown"`
    /// when the lock has not been set (e.g., in tests or when `initialize` is not
    /// called).
    #[must_use]
    pub fn agent_kind_str(&self) -> &str {
        self.agent_kind.get().map_or("unknown", AgentKind::as_str)
    }
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
///
/// The `status` parameter controls the initial Status field value. Callers
/// should pass `"Blocked"` when the issue has blockers, or `"Backlog"` otherwise
/// (per PRD section 6.1 and ARCH section 10.4).
#[allow(clippy::too_many_arguments)]
async fn set_project_fields(
    client: &GitHubClient,
    project_id: &str,
    item_id: &str,
    field_ids: &unblock_github::projects::ProjectFieldIds,
    priority: &str,
    issue_type: &str,
    status: &str,
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

    // Set Status (Backlog when unblocked, Blocked when blocked_by is present).
    if let Some(option_id) = field_ids.status.options.get(status)
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

/// Applies a [`BodySectionUpdate`] to a [`BodySections`] struct.
///
/// Maps the [`SectionName`] variant to the corresponding field in `BodySections`
/// and sets it to the new content. The content is stored as `Some` unless the
/// content string is empty or whitespace-only, in which case it is set to `None`.
fn apply_body_section_update(
    sections: &mut unblock_core::types::BodySections,
    update: &BodySectionUpdate,
) {
    let content = if update.content.trim().is_empty() {
        None
    } else {
        Some(update.content.clone())
    };

    match update.section {
        SectionName::Description => sections.description = content,
        SectionName::Acceptance => sections.acceptance_criteria = content,
        SectionName::Design => sections.design_notes = content,
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
        let kind = state.agent_kind_str();
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
            agent.kind = %kind,
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

    /// Configure required Projects V2 fields and views (idempotent).
    ///
    /// Ensures the 7 required custom fields and 5 pre-configured views exist
    /// on the project. With `dry_run: true`, reports what would be created
    /// without mutating anything.
    #[tool(
        name = "setup",
        description = "Configure Projects V2 fields (Status, Priority, IssueType, Agent, StoryPoints, DeferUntil, ReadyState) and views (://ready, ://team, ://pipeline, ://roadmap, ://timeline). Safe to call repeatedly — existing fields/views are skipped. Use dry_run=true to check without mutating."
    )]
    async fn setup(
        &self,
        Parameters(params): Parameters<SetupParams>,
    ) -> Result<Json<SetupResult>, ErrorData> {
        let state = self.state();
        let kind = state.agent_kind_str();
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
            agent.kind = %kind,
            project_id = %project_info.id,
            project_number = project_info.number,
            dry_run,
            "Setup tool invoked"
        );

        // Step 4: Detect owner type (read-only GET, safe for dry-run too).
        let owner_type = execute_read_tool(state, || client.detect_owner_type()).await?;

        if dry_run {
            // Dry-run: query which fields and views exist without creating anything.
            let project_id = project_info.id.clone();
            let field_status =
                execute_read_tool(state, || client.query_setup_status(&project_id)).await?;

            // Step 5: Query existing views.
            let existing_views = execute_read_tool(state, || client.list_views(owner_type)).await?;

            let existing_view_names: std::collections::HashSet<&str> =
                existing_views.iter().map(|v| v.name.as_str()).collect();

            let mut views_existing = Vec::new();
            let mut views_would_create = Vec::new();
            for spec in REQUIRED_VIEWS {
                if existing_view_names.contains(spec.name) {
                    views_existing.push(spec.name.to_owned());
                } else {
                    views_would_create.push(spec.name.to_owned());
                }
            }

            return Ok(Json(SetupResult {
                fields_created: Vec::new(),
                fields_existing: field_status.existing,
                fields_missing: field_status.missing,
                views_created: views_would_create,
                views_existing,
                project_number: u64::from(project_info.number),
                dry_run: true,
            }));
        }

        // ── Write path ──────────────────────────────────────────────────

        // Step 1–3: Create fields (with cache rebuild).
        let project_id = project_info.id.clone();
        let client_clone = Arc::clone(client);

        let report = execute_write_tool(state, || async move {
            client_clone.setup_fields(&project_id).await
        })
        .await?;

        // Cache the resolved field IDs on the client for subsequent update_field calls.
        client.set_field_ids(report.field_ids).await;

        // Step 5: Query existing views.
        let existing_views = execute_read_tool(state, || client.list_views(owner_type)).await?;

        let existing_view_names: std::collections::HashSet<&str> =
            existing_views.iter().map(|v| v.name.as_str()).collect();

        // Step 6: Discover all field IDs via REST (needed for visible_fields).
        let rest_fields = execute_read_tool(state, || client.list_rest_fields(owner_type)).await?;

        let all_field_ids: Vec<u64> = rest_fields.iter().map(|f| f.id).collect();

        // Step 7: Create missing views.
        let mut views_created = Vec::new();
        let mut views_existing = Vec::new();

        for spec in REQUIRED_VIEWS {
            if existing_view_names.contains(spec.name) {
                views_existing.push(spec.name.to_owned());
                continue;
            }

            // Roadmap views do not support visible_fields (ARCH §8.5).
            let visible_fields = if spec.layout == ViewLayout::Roadmap {
                None
            } else {
                Some(all_field_ids.clone())
            };

            let view_params = CreateViewParams {
                name: spec.name.to_owned(),
                layout: spec.layout,
                filter: spec.filter.map(String::from),
                visible_fields,
            };

            // NOTE: execute_read_tool is intentional here despite create_view being a
            // mutating POST. Views do not affect the dependency graph, so no cache
            // rebuild is needed. execute_read_tool provides consistent error mapping
            // without the unnecessary cache invalidation of execute_write_tool.
            execute_read_tool(state, || client.create_view(owner_type, &view_params)).await?;

            views_created.push(spec.name.to_owned());
        }

        Ok(Json(SetupResult {
            fields_created: report.created,
            fields_existing: report.skipped,
            fields_missing: Vec::new(),
            views_created,
            views_existing,
            project_number: u64::from(project_info.number),
            dry_run: false,
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
        let kind = state.agent_kind_str();
        let client = &state.client;

        let include_comments = params.include_comments.unwrap_or(true);
        let include_deps = params.include_deps.unwrap_or(true);
        let issue_number = params.id;

        info!(
            agent.kind = %kind,
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
        // TODO(unblock-b6b.62): Extract shared helper fn for RelatedIssue-to-ShowRelatedIssue mapping
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

        // Step 3b: Extract parent and sub-issues from the issue.
        let parent: Option<ShowRelatedIssue> = issue.parent.as_ref().map(|r| ShowRelatedIssue {
            number: r.number,
            title: r.title.clone(),
            state: format!("{:?}", r.state),
        });

        let sub_issues: Vec<ShowRelatedIssue> = issue
            .sub_issues
            .iter()
            .map(|r| ShowRelatedIssue {
                number: r.number,
                title: r.title.clone(),
                state: format!("{:?}", r.state),
            })
            .collect();

        // Step 4: If include_deps, get dependency tree from cached graph.
        let dependency_tree = if include_deps {
            let issue_qid =
                unblock_core::types::QualifiedId::new(client.owner(), client.repo(), issue_number);
            state.cache.get_graph().await.map(|graph| {
                graph
                    .dependency_tree(&issue_qid, unblock_core::types::TraversalDirection::Both, 3)
                    .into_iter()
                    .map(|(qid, depth)| DependencyTreeEntry {
                        issue_number: qid.number,
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
            issue_type: issue.issue_type.map(|it| it.to_string()),
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
            parent,
            sub_issues,
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
        let kind = state.agent_kind_str();

        info!(
            agent.kind = %kind,
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

    /// Claim an issue for an agent — marks it as in-progress.
    ///
    /// Validates the issue is open, unblocked, not deferred, and not already
    /// claimed. Then updates Projects V2 fields (Status=In Progress, Agent=name,
    /// ReadyState=Not Ready) and posts a claim comment.
    ///
    /// Validation order (cheapest first): closed, blocked, deferred, already claimed.
    ///
    /// This is a write tool — the cache is invalidated and rebuilt after all
    /// mutations complete.
    #[tool(
        name = "claim",
        description = "Claim an issue for an agent. Validates the issue is open, unblocked, not deferred, and not already claimed. Sets Status=In Progress, Agent=name, ReadyState=Not Ready, and posts a comment. Triggers graph rebuild."
    )]
    async fn claim(
        &self,
        Parameters(params): Parameters<ClaimParams>,
    ) -> Result<Json<ClaimResult>, ErrorData> {
        let state = self.state();
        let kind = state.agent_kind_str();
        let client = Arc::clone(&state.client);
        let config = Arc::clone(&state.config);

        let issue_number = params.id;
        let agent_name = params.agent.unwrap_or_else(|| config.agent.clone());

        info!(
            agent.kind = %kind,
            issue_number,
            agent = %agent_name,
            "Claim tool invoked"
        );

        let result = execute_write_tool(state, || {
            let client = Arc::clone(&client);
            let agent_name = agent_name.clone();

            async move {
                // Step 1: Fetch the issue.
                let issue = client.fetch_issue(issue_number).await?;

                // Steps 2–5: Validate claimability (closed, blocked, deferred, already claimed).
                let candidate = ClaimCandidate {
                    number: issue.number,
                    state: issue.state,
                    status: issue.status,
                    agent: issue.agent.clone(),
                    blocked_by: issue.blocked_by.clone(),
                    defer_until: issue.defer_until,
                };
                validate_claimable(&candidate, Utc::now().date_naive())?;

                // Step 6: Update Projects V2 fields.
                if let Some(field_ids) = client.field_ids().await {
                    if let Ok(project_info) = client.resolve_project_info().await {
                        if let Ok(item_id) = client
                            .get_project_item_id(&issue.node_id, &project_info.id)
                            .await
                        {
                            // Status -> In Progress
                            if let Some(option_id) = field_ids.status.options.get("In Progress")
                                && let Err(e) = client
                                    .update_field(
                                        &project_info.id,
                                        &item_id,
                                        &field_ids.status.field_id,
                                        &FieldValue::SingleSelectOption(option_id.clone()),
                                    )
                                    .await
                            {
                                tracing::warn!(error = %e, "Failed to set Status field");
                            }

                            // Agent -> agent_name
                            if let Err(e) = client
                                .update_field(
                                    &project_info.id,
                                    &item_id,
                                    &field_ids.agent,
                                    &FieldValue::Text(agent_name.clone()),
                                )
                                .await
                            {
                                tracing::warn!(error = %e, "Failed to set Agent field");
                            }

                            // ReadyState -> Not Ready
                            if let Some(option_id) = field_ids.ready_state.options.get("Not Ready")
                                && let Err(e) = client
                                    .update_field(
                                        &project_info.id,
                                        &item_id,
                                        &field_ids.ready_state.field_id,
                                        &FieldValue::SingleSelectOption(option_id.clone()),
                                    )
                                    .await
                            {
                                tracing::warn!(error = %e, "Failed to set ReadyState field");
                            }
                        } else {
                            tracing::warn!(
                                "Failed to get project item ID — fields will not be set"
                            );
                        }
                    } else {
                        tracing::warn!("Failed to resolve project info — fields will not be set");
                    }
                } else {
                    tracing::debug!(
                        "No field IDs cached — run setup first to enable project field assignment"
                    );
                }

                // Step 7: Post claim comment.
                let now = Utc::now();
                let comment_body =
                    format!("\u{1F916} Claimed by {agent_name} at {}", now.to_rfc3339());
                if let Err(e) = client.add_comment(issue_number, comment_body).await {
                    tracing::warn!(error = %e, "Failed to post claim comment");
                }

                // Step 8: Return result (cache rebuild handled by execute_write_tool).
                Ok(ClaimResult {
                    issue_number,
                    agent: agent_name,
                    claimed_at: now,
                })
            }
        })
        .await?;

        Ok(Json(result))
    }

    /// Close an issue and cascade-unblock dependents.
    ///
    /// Validates the issue is open, optionally adds a reason comment, closes it
    /// via the GitHub API, updates Projects V2 fields (Status=Done,
    /// ReadyState=Not Ready), rebuilds the cache, then computes the unblock
    /// cascade. For each newly unblocked issue, updates its Projects V2 fields
    /// (ReadyState=Ready, Status=Backlog if not already `InProgress`) and posts
    /// an unblock comment.
    ///
    /// This is a write tool -- uses `execute_write_tool` for the close mutation
    /// and cache rebuild, then performs cascade updates as a second phase.
    #[tool(
        name = "close",
        description = "Close an issue and cascade-unblock dependents. Validates the issue is open, closes it, updates project fields (Status=Done, ReadyState=Not Ready), and auto-unblocks any dependent issues whose blockers are now all closed. Returns the list of newly unblocked issue numbers. Triggers graph rebuild."
    )]
    async fn close(
        &self,
        Parameters(params): Parameters<CloseParams>,
    ) -> Result<Json<CloseResult>, ErrorData> {
        let state = self.state();
        let kind = state.agent_kind_str();
        let client = Arc::clone(&state.client);

        let issue_number = params.id;
        let reason = params.reason;

        info!(
            agent.kind = %kind,
            issue_number,
            reason = reason.as_deref(),
            "Close tool invoked"
        );

        // Phase 1: Validate, close, and rebuild cache via execute_write_tool.
        execute_write_tool(state, || {
            let client = Arc::clone(&client);
            let reason = reason.clone();

            async move {
                // Step 1: Fetch the issue and validate it is open.
                let issue = client.fetch_issue(issue_number).await?;

                if issue.state == IssueState::Closed {
                    return Err(IssueClosedSnafu {
                        number: issue_number,
                    }
                    .build()
                    .into());
                }

                // Step 2: Close the issue (this handles adding a reason comment
                // internally if reason is Some — see mutations.rs).
                client.close_issue(issue_number, reason).await?;

                // Step 3: Update Projects V2 fields on the closed issue:
                // Status → Done, ReadyState → Not Ready.
                // TODO(unblock-b6b.79): Extract shared project field update helper to
                // deduplicate this if-let ladder (also in claim handler and cascade below).
                if let Some(field_ids) = client.field_ids().await {
                    if let Ok(project_info) = client.resolve_project_info().await {
                        if let Ok(item_id) = client
                            .get_project_item_id(&issue.node_id, &project_info.id)
                            .await
                        {
                            // Status → Done
                            if let Some(option_id) = field_ids.status.options.get("Done")
                                && let Err(e) = client
                                    .update_field(
                                        &project_info.id,
                                        &item_id,
                                        &field_ids.status.field_id,
                                        &FieldValue::SingleSelectOption(option_id.clone()),
                                    )
                                    .await
                            {
                                tracing::warn!(error = %e, "Failed to set Status=Done on closed issue");
                            }

                            // ReadyState → Not Ready
                            if let Some(option_id) =
                                field_ids.ready_state.options.get("Not Ready")
                                && let Err(e) = client
                                    .update_field(
                                        &project_info.id,
                                        &item_id,
                                        &field_ids.ready_state.field_id,
                                        &FieldValue::SingleSelectOption(option_id.clone()),
                                    )
                                    .await
                            {
                                tracing::warn!(
                                    error = %e,
                                    "Failed to set ReadyState=Not Ready on closed issue"
                                );
                            }
                        } else {
                            tracing::warn!(
                                "Failed to get project item ID for closed issue — fields will not be set"
                            );
                        }
                    } else {
                        tracing::warn!(
                            "Failed to resolve project info — closed issue fields will not be set"
                        );
                    }
                } else {
                    tracing::debug!(
                        "No field IDs cached — run setup first to enable project field assignment"
                    );
                }

                Ok(())
            }
        })
        .await?;

        // Phase 2: Compute cascade from the freshly rebuilt cache.
        let mut unblocked = Vec::new();

        if let Some(graph) = state.cache.get_graph().await {
            // compute_unblock_cascade's _all_issues param is currently unused —
            // pass an empty slice (see graph.rs:215-220 for rationale).
            let issue_qid =
                unblock_core::types::QualifiedId::new(client.owner(), client.repo(), issue_number);
            let cascade = graph.compute_unblock_cascade(&issue_qid, &[]);

            // Phase 3: For each newly unblocked issue, update project fields and
            // post an unblock comment. Each update is best-effort — failures are
            // logged but do not abort the cascade.
            for cascaded_qid in &cascade {
                let cascaded_number = cascaded_qid.number;
                // Post unblock comment.
                let comment_body = format!("\u{2705} Unblocked by closing #{issue_number}");
                if let Err(e) = client.add_comment(cascaded_number, comment_body).await {
                    tracing::warn!(
                        cascaded_number,
                        error = %e,
                        "Failed to post unblock comment on cascaded issue"
                    );
                }

                // Update Projects V2 fields: ReadyState → Ready,
                // Status → Backlog (if not already InProgress).
                // TODO(unblock-b6b.79): Third copy of field update ladder — extract shared helper.
                if let Some(field_ids) = client.field_ids().await
                    && let Ok(project_info) = client.resolve_project_info().await
                {
                    // Fetch the cascaded issue to get its node_id and current status.
                    match client.fetch_issue(cascaded_number).await {
                        Ok(cascaded_issue) => {
                            if let Ok(item_id) = client
                                .get_project_item_id(&cascaded_issue.node_id, &project_info.id)
                                .await
                            {
                                // ReadyState → Ready
                                if let Some(option_id) = field_ids.ready_state.options.get("Ready")
                                    && let Err(e) = client
                                        .update_field(
                                            &project_info.id,
                                            &item_id,
                                            &field_ids.ready_state.field_id,
                                            &FieldValue::SingleSelectOption(option_id.clone()),
                                        )
                                        .await
                                {
                                    tracing::warn!(
                                        cascaded_number,
                                        error = %e,
                                        "Failed to set ReadyState=Ready on cascaded issue"
                                    );
                                }

                                // Status → Backlog (only if not already InProgress).
                                if cascaded_issue.status != unblock_core::types::Status::InProgress
                                    && let Some(option_id) = field_ids.status.options.get("Backlog")
                                    && let Err(e) = client
                                        .update_field(
                                            &project_info.id,
                                            &item_id,
                                            &field_ids.status.field_id,
                                            &FieldValue::SingleSelectOption(option_id.clone()),
                                        )
                                        .await
                                {
                                    tracing::warn!(
                                        cascaded_number,
                                        error = %e,
                                        "Failed to set Status=Backlog on cascaded issue"
                                    );
                                }
                            } else {
                                tracing::warn!(
                                    cascaded_number,
                                    "Failed to get project item ID for cascaded issue"
                                );
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                cascaded_number,
                                error = %e,
                                "Failed to fetch cascaded issue for field updates"
                            );
                        }
                    }
                }
            }

            unblocked = cascade.iter().map(|q| q.number).collect();
        } else {
            tracing::warn!("Cache not available after rebuild — cascade computation skipped");
        }

        Ok(Json(CloseResult {
            issue_number,
            unblocked,
        }))
    }

    /// Add a blocking dependency between two issues.
    ///
    /// Makes the source issue blocked by the target issue. Validates the source
    /// exists, checks for cycles and duplicates using the cached graph, creates
    /// the blocking relationship via the GitHub API, updates Projects V2 fields
    /// (ReadyState=Not Ready, Status=Blocked) on the source, and rebuilds the
    /// cache.
    ///
    /// The target accepts a local issue number (e.g. `"42"`) or a cross-repo
    /// reference in `owner/repo#number` format (e.g. `"websublime/other-repo#7"`).
    ///
    /// This is a write tool — uses `execute_write_tool` for the mutation and
    /// cache rebuild.
    #[tool(
        name = "depends",
        description = "Add a blocking dependency: source becomes blocked by target. Validates both issues exist, rejects cycles and duplicates. Target accepts local number or owner/repo#number for cross-repo. Updates project fields (ReadyState=Not Ready, Status=Blocked) on source. Triggers graph rebuild."
    )]
    async fn depends(
        &self,
        Parameters(params): Parameters<DependsParams>,
    ) -> Result<Json<DependsResult>, ErrorData> {
        let state = self.state();
        let kind = state.agent_kind_str();
        let client = Arc::clone(&state.client);

        let source = params.source;
        let target_str = params.target.clone();

        info!(agent.kind = %kind, source, target = %target_str, "Depends tool invoked");

        // Parse target string into IssueRef.
        let issue_ref = target_str
            .parse::<unblock_core::types::IssueRef>()
            .map_err(|e| ErrorData {
                code: rmcp::model::ErrorCode::INVALID_PARAMS,
                message: format!("Invalid target reference '{target_str}': {e}").into(),
                data: None,
            })?;

        // Step 1: Validate source issue exists.
        let source_issue = client
            .fetch_issue(source)
            .await
            .map_err(crate::errors::github_error_to_mcp)?;

        // Step 2: Cycle detection using cached graph (local targets only).
        // Cross-repo targets are not present in the local graph, so cycle
        // detection is skipped — no local cycle is possible. Checking with
        // the bare number from a CrossRepo ref would incorrectly match a
        // local issue with the same number, causing false positive rejections.
        if let unblock_core::types::IssueRef::Local(target_number) = &issue_ref
            && let Some(graph) = state.cache.get_graph().await
            && graph.would_create_cycle(
                &unblock_core::types::QualifiedId::new(client.owner(), client.repo(), source),
                &unblock_core::types::QualifiedId::new(
                    client.owner(),
                    client.repo(),
                    *target_number,
                ),
            )
        {
            return Err(crate::errors::github_error_to_mcp(
                CircularDependencySnafu {
                    source,
                    target: *target_number,
                }
                .build()
                .into(),
            ));
        }

        // Step 3: Add blocking relationship and rebuild cache via execute_write_tool.
        execute_write_tool(state, || {
            let client = Arc::clone(&client);
            let issue_ref = issue_ref.clone();

            async move { client.add_blocked_by_ref(source, &issue_ref).await }
        })
        .await?;

        // Step 4: Update Projects V2 fields on source issue:
        // ReadyState → Not Ready, Status → Blocked.
        // TODO(unblock-b6b.79): Fourth copy of field update ladder — extract shared helper.
        if let Some(field_ids) = client.field_ids().await {
            if let Ok(project_info) = client.resolve_project_info().await {
                if let Ok(item_id) = client
                    .get_project_item_id(&source_issue.node_id, &project_info.id)
                    .await
                {
                    // ReadyState → Not Ready
                    if let Some(option_id) = field_ids.ready_state.options.get("Not Ready")
                        && let Err(e) = client
                            .update_field(
                                &project_info.id,
                                &item_id,
                                &field_ids.ready_state.field_id,
                                &FieldValue::SingleSelectOption(option_id.clone()),
                            )
                            .await
                    {
                        tracing::warn!(
                            error = %e,
                            "Failed to set ReadyState=Not Ready on source issue"
                        );
                    }

                    // Status → Blocked
                    if let Some(option_id) = field_ids.status.options.get("Blocked")
                        && let Err(e) = client
                            .update_field(
                                &project_info.id,
                                &item_id,
                                &field_ids.status.field_id,
                                &FieldValue::SingleSelectOption(option_id.clone()),
                            )
                            .await
                    {
                        tracing::warn!(
                            error = %e,
                            "Failed to set Status=Blocked on source issue"
                        );
                    }
                } else {
                    tracing::warn!(
                        "Failed to get project item ID for source issue — fields will not be set"
                    );
                }
            } else {
                tracing::warn!(
                    "Failed to resolve project info — source issue fields will not be set"
                );
            }
        } else {
            tracing::debug!(
                "No field IDs cached — run setup first to enable project field assignment"
            );
        }

        Ok(Json(DependsResult {
            source,
            target: target_str,
            message: format!(
                "Issue #{source} is now blocked by {issue_ref}. Source marked as Not Ready/Blocked."
            ),
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
        let kind = state.agent_kind_str();
        let client = Arc::clone(&state.client);

        info!(
            agent.kind = %kind,
            title = %params.title,
            issue_type = params.issue_type.as_deref(),
            priority = params.priority.as_deref(),
            "Create tool invoked"
        );

        // Validate issue_type if provided.
        let issue_type_str = params.issue_type.as_deref().unwrap_or("Task");
        if !matches!(
            issue_type_str,
            "Task" | "Bug" | "Feature" | "Epic" | "Chore"
        ) {
            return Err(ErrorData {
                code: rmcp::model::ErrorCode::INVALID_PARAMS,
                message: format!(
                    "Invalid issue_type '{issue_type_str}' — must be Task, Bug, Feature, Epic, or Chore"
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

                // Step 2: Resolve milestone title to milestone number.
                let milestone_number = if let Some(ref ms_title) = milestone_title {
                    match client.list_milestones().await {
                        Ok(milestones) => {
                            if let Some(ms) = milestones.iter().find(|m| m.title == *ms_title) {
                                Some(ms.number)
                            } else {
                                tracing::warn!(
                                    milestone = %ms_title,
                                    "Milestone not found — milestone will not be set on the issue"
                                );
                                None
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                milestone = %ms_title,
                                "Failed to list milestones (best-effort) — milestone will not be set on the issue"
                            );
                            None
                        }
                    }
                } else {
                    None
                };
                let milestone_set = milestone_number.is_some();

                // Step 3: Create the issue.
                let create_params = unblock_github::mutations::CreateIssueParams {
                    title,
                    body,
                    labels,
                    milestone: milestone_number,
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

                                    let (initial_status, initial_ready_state) =
                                        if blocked_by_refs.is_empty() {
                                            ("Backlog", "Ready")
                                        } else {
                                            ("Blocked", "Not Ready")
                                        };

                                    set_project_fields(
                                        &client,
                                        &project_info.id,
                                        &item_id,
                                        &field_ids,
                                        &priority_owned,
                                        &issue_type_owned,
                                        initial_status,
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
                    milestone_set,
                    hint: format!(
                        "Issue #{issue_number} created. Use `show` to verify or `ready` to check if it appears in the ready set."
                    ),
                })
            }
        })
        .await?;

        Ok(Json(result))
    }

    /// Post a comment on an issue.
    ///
    /// Validates that the body is non-empty and that the issue exists before
    /// calling the GitHub API. This is a read tool from the graph perspective —
    /// comments do not affect the dependency graph or ready set, so no cache
    /// invalidation is needed.
    #[tool(
        name = "comment",
        description = "Post a comment on an issue. Does not affect the dependency graph or ready set — no graph rebuild is triggered. Returns the URL of the new comment."
    )]
    async fn comment(
        &self,
        Parameters(params): Parameters<CommentParams>,
    ) -> Result<Json<CommentResult>, ErrorData> {
        let state = self.state();
        let kind = state.agent_kind_str();
        let client = &state.client;
        let issue_number = params.id;
        let body = params.body;

        info!(agent.kind = %kind, issue_number, "Comment tool invoked");

        // Step 1: Validate body is non-empty (before any API call).
        if body.trim().is_empty() {
            return Err(ErrorData {
                code: rmcp::model::ErrorCode::INVALID_PARAMS,
                message: "Comment body must not be empty or whitespace-only".into(),
                data: None,
            });
        }

        // Step 2: Validate issue exists.
        let _issue = execute_read_tool(state, || client.fetch_issue(issue_number)).await?;

        // Step 3: Post the comment.
        let comment_url =
            execute_read_tool(state, || client.add_comment(issue_number, body)).await?;

        Ok(Json(CommentResult {
            issue_number,
            comment_url,
        }))
    }

    /// Update issue metadata and Projects V2 fields.
    ///
    /// Selectively updates only the fields provided in the input. Supports:
    /// `priority`, `status`, `story_points`, `defer_until` (via Projects V2),
    /// `labels_add`/`labels_remove` (via REST), `milestone` (resolved by title),
    /// and `body_section` (read-modify-write on issue body).
    ///
    /// This is a write tool — the cache is invalidated and rebuilt after all
    /// mutations complete.
    #[tool(
        name = "update",
        description = "Update issue fields selectively. Supports: priority, status, labels_add, labels_remove, body_section (section: Description/Acceptance/Design, content), milestone (title), story_points, defer_until (YYYY-MM-DD). Only provided fields are changed. Triggers graph rebuild."
    )]
    async fn update(
        &self,
        Parameters(params): Parameters<UpdateParams>,
    ) -> Result<Json<UpdateResult>, ErrorData> {
        let state = self.state();
        let kind = state.agent_kind_str();
        let client = Arc::clone(&state.client);

        let issue_number = params.id;

        info!(
            agent.kind = %kind,
            issue_number,
            priority = params.priority.as_deref(),
            status = params.status.as_deref(),
            "Update tool invoked"
        );

        // Validate priority if provided.
        if let Some(ref p) = params.priority
            && !matches!(p.as_str(), "P0" | "P1" | "P2" | "P3" | "P4")
        {
            return Err(ErrorData {
                code: rmcp::model::ErrorCode::INVALID_PARAMS,
                message: format!("Invalid priority '{p}' — must be P0, P1, P2, P3, or P4").into(),
                data: None,
            });
        }

        // Validate status if provided.
        if let Some(ref s) = params.status
            && !matches!(
                s.as_str(),
                "Backlog" | "In Progress" | "Done" | "Blocked" | "Deferred"
            )
        {
            return Err(ErrorData {
                code: rmcp::model::ErrorCode::INVALID_PARAMS,
                message: format!(
                    "Invalid status '{s}' — must be Backlog, In Progress, Done, Blocked, or Deferred"
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

        // Move params into the closure — validation above used only `ref` borrows
        // so the struct is still fully owned here.
        let result = execute_write_tool(state, || {
            let client = Arc::clone(&client);
            let params = params;

            async move {
                // Step 1: Fetch the issue — validates existence.
                let issue = client.fetch_issue(issue_number).await?;

                // Step 1b: Validate the issue is open (spec 10.15 step 2).
                if issue.state == IssueState::Closed {
                    return Err(IssueClosedSnafu {
                        number: issue_number,
                    }
                    .build()
                    .into());
                }

                let mut fields_updated: Vec<String> = Vec::new();

                // Step 2: Update Projects V2 fields if any project fields changed.
                let has_project_updates = params.priority.is_some()
                    || params.status.is_some()
                    || params.story_points.is_some()
                    || defer_until.is_some();

                if has_project_updates {
                    if let Some(field_ids) = client.field_ids().await {
                        if let Ok(project_info) = client.resolve_project_info().await {
                            if let Ok(item_id) = client
                                .get_project_item_id(&issue.node_id, &project_info.id)
                                .await
                            {
                                // Priority
                                if let Some(ref p) = params.priority
                                    && let Some(option_id) =
                                        field_ids.priority.options.get(p.as_str())
                                {
                                    if let Err(e) = client
                                        .update_field(
                                            &project_info.id,
                                            &item_id,
                                            &field_ids.priority.field_id,
                                            &FieldValue::SingleSelectOption(option_id.clone()),
                                        )
                                        .await
                                    {
                                        tracing::warn!(error = %e, "Failed to set Priority field");
                                    } else {
                                        fields_updated.push(format!("priority={p}"));
                                    }
                                }

                                // Status
                                if let Some(ref s) = params.status
                                    && let Some(option_id) =
                                        field_ids.status.options.get(s.as_str())
                                {
                                    if let Err(e) = client
                                        .update_field(
                                            &project_info.id,
                                            &item_id,
                                            &field_ids.status.field_id,
                                            &FieldValue::SingleSelectOption(option_id.clone()),
                                        )
                                        .await
                                    {
                                        tracing::warn!(error = %e, "Failed to set Status field");
                                    } else {
                                        fields_updated.push(format!("status={s}"));
                                    }
                                }

                                // StoryPoints
                                if let Some(sp) = params.story_points {
                                    match client
                                        .update_field(
                                            &project_info.id,
                                            &item_id,
                                            &field_ids.story_points,
                                            &FieldValue::Number(sp),
                                        )
                                        .await
                                    {
                                        Ok(()) => fields_updated.push(format!("story_points={sp}")),
                                        Err(e) => tracing::warn!(error = %e, "Failed to set StoryPoints field"),
                                    }
                                }

                                // DeferUntil
                                if let Some(du) = defer_until {
                                    match client
                                        .update_field(
                                            &project_info.id,
                                            &item_id,
                                            &field_ids.defer_until,
                                            &FieldValue::Date(du),
                                        )
                                        .await
                                    {
                                        Ok(()) => fields_updated.push(format!("defer_until={du}")),
                                        Err(e) => tracing::warn!(error = %e, "Failed to set DeferUntil field"),
                                    }
                                }
                            } else {
                                tracing::warn!(
                                    "Failed to get project item ID — project fields will not be set"
                                );
                            }
                        } else {
                            tracing::warn!(
                                "Failed to resolve project info — project fields will not be set"
                            );
                        }
                    } else {
                        tracing::debug!(
                            "No field IDs cached — run setup first to enable project field updates"
                        );
                    }
                }

                // Step 3: Add labels.
                if let Some(ref add) = params.labels_add
                    && !add.is_empty()
                {
                    // Ensure labels exist on the repo first.
                    client.ensure_labels(add).await?;
                    client
                        .add_labels_to_issue(issue_number, add.clone())
                        .await?;
                    fields_updated.push(format!("labels_add={}", add.join(",")));
                }

                // Step 4: Remove labels.
                if let Some(ref remove) = params.labels_remove {
                    for label in remove {
                        if let Err(e) = client
                            .remove_label_from_issue(issue_number, label)
                            .await
                        {
                            tracing::warn!(
                                label = %label,
                                error = %e,
                                "Failed to remove label (best-effort)"
                            );
                        }
                    }
                    if !remove.is_empty() {
                        fields_updated.push(format!("labels_remove={}", remove.join(",")));
                    }
                }

                // Step 5: Add assignees.
                if let Some(ref add) = params.assignees_add
                    && !add.is_empty()
                {
                    client
                        .add_assignees_to_issue(issue_number, add.clone())
                        .await?;
                    fields_updated.push(format!("assignees_add={}", add.join(",")));
                }

                // Step 6: Remove assignees.
                if let Some(ref remove) = params.assignees_remove
                    && !remove.is_empty()
                {
                    if let Err(e) = client
                        .remove_assignees_from_issue(issue_number, remove.clone())
                        .await
                    {
                        tracing::warn!(
                            error = %e,
                            "Failed to remove assignees (best-effort)"
                        );
                    } else {
                        fields_updated.push(format!("assignees_remove={}", remove.join(",")));
                    }
                }

                // Step 7: Milestone resolution and update.
                if let Some(ref ms_title) = params.milestone {
                    let milestones = client.list_milestones().await?;
                    if let Some(ms) = milestones.iter().find(|m| m.title == *ms_title) {
                        client
                            .update_issue_milestone(issue_number, Some(ms.number))
                            .await?;
                        fields_updated.push(format!("milestone={ms_title}"));
                    } else {
                        tracing::warn!(
                            milestone = %ms_title,
                            "Milestone not found — milestone will not be set"
                        );
                    }
                }

                // Step 8: Body section update.
                if let Some(ref section_update) = params.body_section {
                    let current_body = issue.body.as_deref().unwrap_or_default();
                    let mut sections =
                        unblock_core::types::BodySections::from_markdown(current_body);

                    apply_body_section_update(&mut sections, section_update);

                    let new_body = sections.to_markdown();
                    client
                        .update_issue_body(issue_number, new_body)
                        .await?;

                    let section_label = match section_update.section {
                        SectionName::Description => "description",
                        SectionName::Acceptance => "acceptance_criteria",
                        SectionName::Design => "design_notes",
                    };
                    fields_updated.push(format!("body_section={section_label}"));
                }

                // Step 9: Re-fetch to confirm updates.
                let updated_issue = client.fetch_issue(issue_number).await?;

                Ok(UpdateResult {
                    number: updated_issue.number,
                    url: updated_issue.url,
                    fields_updated,
                    hint: format!(
                        "Issue #{issue_number} updated. Use `show` to verify the changes."
                    ),
                })
            }
        })
        .await?;

        Ok(Json(result))
    }

    /// Session entry point — aggregates issue state for agent orientation.
    ///
    /// Returns categorised lists of issues (`in_progress`, `ready`, `blocked`,
    /// hotspots, stale claims) from a fresh GitHub fetch. Updates the cache
    /// with the fetched data. Includes stub session metadata and no drift
    /// warnings until Epics 1.5 and 1.6 wire them in.
    #[tool(
        name = "prime",
        description = "Session entry point for every agent session. Returns categorised issue lists: in_progress, ready, blocked, hotspots (most-blocking), and stale claims. Includes session metadata and drift warnings. Always does a fresh fetch — bypasses cache. Use stale_threshold_hours to configure stale claim detection (default 24h), max_per_category to limit output size (default 10), and agent to filter in_progress/ready/blocked/stale by agent name (exact match; completed and hotspots are never filtered)."
    )]
    async fn prime(
        &self,
        Parameters(params): Parameters<PrimeParams>,
    ) -> Result<Json<PrimeResult>, ErrorData> {
        let state = self.state();
        let kind = state.agent_kind_str();

        info!(agent.kind = %kind, "Prime tool invoked");

        let output = crate::tools::prime::handle_prime(&params, state).await?;
        Ok(Json(output))
    }

    /// Detect and optionally repair drift between the dependency graph and GitHub.
    ///
    /// Performs a fresh fetch from GitHub (bypasses cache), rebuilds the graph,
    /// and runs the reconciliation engine to detect divergence. Returns a
    /// [`DriftReport`] with all detected drift.
    ///
    /// By default operates in read-only mode (`fix: false`). The `fix: true`
    /// repair path is implemented by task 1.6.4.
    ///
    /// After analysis, the cache is updated with the freshly fetched graph data.
    #[tool(
        name = "reconcile",
        description = "Detect drift between the computed dependency graph and GitHub state. Returns a DriftReport listing stale ready states, uncascaded closures, orphaned edges, cycles, and stale claims. Use fix=true to auto-repair (not yet implemented). Always does a fresh fetch — bypasses cache."
    )]
    async fn reconcile(
        &self,
        Parameters(params): Parameters<ReconcileParams>,
    ) -> Result<Json<ReconcileOutput>, ErrorData> {
        let state = self.state();
        let kind = state.agent_kind_str();

        info!(agent.kind = %kind, "Reconcile tool invoked");

        let output = crate::tools::reconcile::handle_reconcile(&params, state).await?;
        Ok(Json(output))
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

    /// Override `initialize` to capture MCP `clientInfo`, resolve [`AgentKind`],
    /// store it in the [`OnceLock`], and emit a structured tracing event.
    ///
    /// Delegates to rmcp's default `peer_info` storage so downstream
    /// `Peer<RoleServer>` usage continues to work.
    fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<InitializeResult, ErrorData>> + Send + '_ {
        // Build AgentClient from MCP clientInfo
        let agent_client = AgentClient {
            name: request.client_info.name.clone(),
            version: request.client_info.version.clone(),
        };

        // Resolve kind once and store in OnceLock
        let kind = ClientDetector::resolve(Some(&agent_client));
        let _ = self.state.agent_kind.set(kind.clone());

        // Store raw AgentClient for SessionMeta (prime tool output)
        let _ = self.state.agent_client.set(agent_client.clone());

        // Record session start time
        let _ = self.state.connected_at.set(Utc::now());

        info!(
            client.name    = &agent_client.name,
            client.version = &agent_client.version,
            client.kind    = %kind,
            "mcp client connected"
        );

        // Delegate to rmcp default for peer_info storage
        if context.peer.peer_info().is_none() {
            context.peer.set_peer_info(request);
        }

        std::future::ready(Ok(self.get_info()))
    }
}

// Static assertions: ServerState must be Send + Sync.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ServerState>();
    assert_send_sync::<Arc<ServerState>>();
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use std::time::Duration;
    use tracing_subscriber::fmt;
    use tracing_subscriber::prelude::*;
    use unblock_core::cache::GraphCache;
    use unblock_core::client::{AgentClient, AgentKind};
    use unblock_core::detection::ClientDetector;

    /// Shared buffer for capturing tracing output in tests.
    ///
    /// Wraps an `Arc<Mutex<Vec<u8>>>` and implements `io::Write` so it can
    /// be used as a `tracing_subscriber` writer. Call [`TracingCapture::new`]
    /// to create an instance, [`TracingCapture::subscriber`] to build a
    /// JSON subscriber wired to the buffer, and [`TracingCapture::output`]
    /// to retrieve the captured output as a zero-copy `&str` borrow.
    #[derive(Clone)]
    struct TracingCapture(Arc<Mutex<Vec<u8>>>);

    /// RAII guard that holds a [`MutexGuard`] and exposes the captured
    /// bytes as a `&str` without cloning.
    ///
    /// Returned by [`TracingCapture::output`]. The guard keeps the mutex
    /// locked while the caller inspects the output; it is released when
    /// the guard is dropped.
    struct CapturedOutput<'a>(MutexGuard<'a, Vec<u8>>);

    impl std::ops::Deref for CapturedOutput<'_> {
        type Target = str;

        fn deref(&self) -> &str {
            std::str::from_utf8(&self.0).expect("captured output is not valid UTF-8")
        }
    }

    impl std::fmt::Display for CapturedOutput<'_> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self)
        }
    }

    impl TracingCapture {
        /// Create a new, empty capture buffer.
        fn new() -> Self {
            Self(Arc::new(Mutex::new(Vec::new())))
        }

        /// Build a JSON tracing subscriber that writes to this buffer.
        fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync {
            let writer = self.clone();
            tracing_subscriber::registry().with(
                fmt::layer()
                    .json()
                    .with_writer(move || writer.clone())
                    .with_target(false),
            )
        }

        /// Return the captured output as a zero-copy `&str` borrow.
        ///
        /// Returns a [`CapturedOutput`] guard that derefs to `&str`,
        /// avoiding a full buffer clone. The mutex stays locked while
        /// the guard is alive.
        ///
        /// # Panics
        ///
        /// Panics if the mutex is poisoned or the buffer is not valid
        /// UTF-8.
        fn output(&self) -> CapturedOutput<'_> {
            CapturedOutput(self.0.lock().unwrap())
        }
    }

    impl io::Write for TracingCapture {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.0.lock().unwrap().flush()
        }
    }

    /// Helper: create a minimal `ServerState` for unit tests.
    async fn test_state() -> ServerState {
        let config = Config::load_from(|key| match key {
            "GITHUB_TOKEN" => Ok("ghp_test_token_for_unit_tests".to_owned()),
            "UNBLOCK_REPO" => Ok("test-owner/test-repo".to_owned()),
            _ => Err(std::env::VarError::NotPresent),
        })
        .expect("test config should load");

        let client = GitHubClient::new(&config)
            .await
            .expect("test client should initialize");

        ServerState {
            config: Arc::new(config),
            client: Arc::new(client),
            cache: Arc::new(GraphCache::new(Duration::from_secs(300))),
            agent_kind: OnceLock::new(),
            agent_client: OnceLock::new(),
            connected_at: OnceLock::new(),
        }
    }

    /// `ServerState.agent_kind` starts empty (`OnceLock` not yet set).
    #[tokio::test]
    async fn agent_kind_starts_empty() {
        let state = test_state().await;
        assert!(state.agent_kind.get().is_none());
    }

    /// Storing an `AgentKind` in the `OnceLock` makes it retrievable.
    #[tokio::test]
    async fn agent_kind_set_and_get() {
        let state = test_state().await;
        let kind = AgentKind::ClaudeCode;
        let _ = state.agent_kind.set(kind.clone());
        assert_eq!(state.agent_kind.get(), Some(&AgentKind::ClaudeCode));
    }

    /// `OnceLock::set` is a single write — second write is silently ignored.
    #[tokio::test]
    async fn agent_kind_once_lock_single_write() {
        let state = test_state().await;
        let _ = state.agent_kind.set(AgentKind::ClaudeCode);
        // Second write returns Err but does not panic.
        let result = state.agent_kind.set(AgentKind::Copilot);
        assert!(result.is_err());
        // Original value is retained.
        assert_eq!(state.agent_kind.get(), Some(&AgentKind::ClaudeCode));
    }

    /// The full resolve pattern used in `initialize()`: build `AgentClient`,
    /// call `ClientDetector::resolve`, store in `OnceLock`.
    #[tokio::test]
    async fn initialize_resolve_pattern_known_client() {
        let state = test_state().await;
        let agent_client = AgentClient {
            name: "claude-code".into(),
            version: "1.2.3".into(),
        };
        let kind = ClientDetector::resolve(Some(&agent_client));
        let _ = state.agent_kind.set(kind.clone());

        assert_eq!(state.agent_kind.get(), Some(&AgentKind::ClaudeCode));
    }

    /// When `clientInfo.name` is unrecognised, `ClientDetector::resolve` returns
    /// `Unknown` — this is stored successfully in the `OnceLock`.
    #[tokio::test]
    async fn initialize_resolve_pattern_unknown_client() {
        let state = test_state().await;
        let agent_client = AgentClient {
            name: "my-custom-tool".into(),
            version: "0.1.0".into(),
        };
        let kind = ClientDetector::resolve(Some(&agent_client));
        let _ = state.agent_kind.set(kind.clone());

        assert_eq!(
            state.agent_kind.get(),
            Some(&AgentKind::Unknown("my-custom-tool".into()))
        );
    }

    /// When no `AgentClient` is provided, `ClientDetector::resolve(None)` falls
    /// back to env detection and ultimately `Unknown("unknown")`.
    #[tokio::test]
    async fn initialize_resolve_pattern_no_client() {
        let state = test_state().await;
        // Use resolve_with to avoid reading actual env vars in tests.
        let kind = ClientDetector::resolve_with(None, |_| Err(std::env::VarError::NotPresent));
        let _ = state.agent_kind.set(kind.clone());

        assert_eq!(
            state.agent_kind.get(),
            Some(&AgentKind::Unknown("unknown".into()))
        );
    }

    /// Tool handler fallback pattern: `OnceLock::get()` returns `None` when
    /// initialize has not been called, and handlers fall back to `"unknown"`.
    #[tokio::test]
    async fn agent_kind_handler_fallback() {
        let state = test_state().await;
        let kind_str = state.agent_kind_str();
        assert_eq!(kind_str, "unknown");
    }

    /// Verify `agent.kind` field is present in tracing output when the
    /// `OnceLock` is set. Exercises the same extraction pattern used by
    /// every tool handler.
    #[tokio::test]
    async fn agent_kind_appears_in_tracing_output() {
        let capture = TracingCapture::new();
        let subscriber = capture.subscriber();

        let state = test_state().await;
        let _ = state.agent_kind.set(AgentKind::ClaudeCode);

        // Exercise the extraction pattern inside the subscriber scope.
        tracing::subscriber::with_default(subscriber, || {
            let kind: &str = state.agent_kind_str();
            info!(agent.kind = %kind, "Ready tool invoked");
        });

        let output = capture.output();
        assert!(
            output.contains("\"agent\":{\"kind\":\"claude-code\"}")
                || output.contains("claude-code"),
            "Expected agent.kind=claude-code in tracing output, got: {output}"
        );
        assert!(
            output.contains("Ready tool invoked"),
            "Expected message in output, got: {output}"
        );
        // Verify token does not appear in output.
        assert!(
            !output.contains("ghp_test_token"),
            "Token must not appear in tracing output: {output}"
        );
    }

    /// Verify `agent.kind` falls back to "unknown" when `OnceLock` is not set.
    #[tokio::test]
    async fn agent_kind_unknown_in_tracing_when_unset() {
        let capture = TracingCapture::new();
        let subscriber = capture.subscriber();

        let state = test_state().await;
        // Do NOT set agent_kind — test the fallback.

        tracing::subscriber::with_default(subscriber, || {
            let kind: &str = state.agent_kind_str();
            info!(agent.kind = %kind, "Claim tool invoked");
        });

        let output = capture.output();
        assert!(
            output.contains("\"agent\":{\"kind\":\"unknown\"}") || output.contains("unknown"),
            "Expected agent.kind=unknown in tracing output, got: {output}"
        );
        assert!(
            output.contains("Claim tool invoked"),
            "Expected message in output, got: {output}"
        );
    }
}
