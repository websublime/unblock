//! MCP server bootstrap and state management.
//!
//! [`ServerState`](crate::server::ServerState) holds an
//! [`Arc`](std::sync::Arc) over a [`GitHubApi`](unblock_github::GitHubApi) trait object
//! (typically backed by [`GitHubClient`](unblock_github::client::GitHubClient)),
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
use crate::tools::dep_cycles::{DepCyclesParams, DepCyclesResult};
use crate::tools::dep_remove::{DepRemoveParams, DepRemoveResult};
use crate::tools::depends::{DependsParams, DependsResult};
use crate::tools::execute_read_tool;
use crate::tools::execute_write_tool;
use crate::tools::init::{InitParams, InitResult};
use crate::tools::list::{ListParams, ListResult};
use crate::tools::prime::{PrimeParams, PrimeResult};
use crate::tools::ready::{ReadyParams, ReadyResult};
use crate::tools::reconcile::{ReconcileOutput, ReconcileParams};
use crate::tools::reopen::{ReopenParams, ReopenResult};
use crate::tools::search::{SearchParams, SearchResult};
use crate::tools::setup::{REQUIRED_VIEWS, SetupParams, SetupResult};
use crate::tools::show::{
    ShowBodySections, ShowComment, ShowIssue, ShowParams, ShowRelatedIssue, ShowResult,
    ShowTreeNode,
};
use crate::tools::stats::{StatsParams, StatsResult};
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
use unblock_core::errors::{
    CircularDependencySnafu, DuplicateDependencySnafu, InvalidIssueRefSnafu, IssueClosedSnafu,
};
use unblock_core::types::IssueState;
use unblock_github::GitHubApi;
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
| reopen  | Reopen a closed issue, re-evaluating blockers       | id                                  |

### Query & Dependencies
| Tool       | Purpose                                              | Key Params                          |
|------------|------------------------------------------------------|-------------------------------------|
| show       | Get full details for a single issue                  | issue                               |
| list       | Filtered, sorted, paginated open-issue view          | status?, priority?, sort?, limit?   |
| search     | Full-text search via GitHub Search API (no cache)    | query, limit?                       |
| stats      | Aggregate counts + per-agent throughput              | milestone?                          |
| depends    | Add a blocking dependency (source blocked by target) | source, target                      |
| dep_remove | Remove a blocking dependency                         | source, target                      |
| dep_cycles | Detect dependency cycles (Tarjan SCC)                | id?                                 |
| comment    | Add a comment to an issue                            | issue_number, body                  |
| update     | Update issue fields (priority, labels, body, etc.)   | issue_number, fields...             |

### Diagnostics
| Tool      | Purpose                                            | Key Params                          |
|-----------|-----------------------------------------------------|-------------------------------------|
| reconcile | Detect drift between graph and GitHub state         | fix?, stale_claim_hours?            |

## Tips
- Run `init` once to create a project, then `setup` to configure it.
- Always call `ready` first to find unblocked work.
- Use `claim` before starting work to prevent conflicts.
- After `close`, dependents are automatically re-evaluated.
- Use `list` for filtered browsing (status/priority/milestone/agent/label/assignee) with pagination; use `search` for GitHub-side full-text queries (bypasses the cache).
- Use `stats` for dashboards and `dep_cycles` to surface circular dependencies before claiming.
- Write tools (create, close, update, claim, depends, dep_remove, comment, reopen) trigger a graph rebuild.
- Read tools (ready, show, list, stats, dep_cycles) use the cache for fast responses.
- `search` bypasses the cache entirely — every call hits GitHub fresh.
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
/// - [`GitHubApi`] is a trait object (`dyn GitHubApi`) bounded by `Send + Sync`;
///   the production blanket impl on [`GitHubClient`](unblock_github::client::GitHubClient) wraps `reqwest::Client`
///   which is `Send + Sync`.
/// - [`GraphCache`] uses `tokio::sync::RwLock` which is `Send + Sync`.
/// - [`OnceLock<AgentKind>`] is `Send + Sync` because `AgentKind` is `Send + Sync`.
/// - [`OnceLock<AgentClient>`] is `Send + Sync` because `AgentClient` is `Send + Sync`.
/// - [`OnceLock<DateTime<Utc>>`] is `Send + Sync` because `DateTime<Utc>` is `Send + Sync`.
pub struct ServerState {
    /// Application configuration loaded from environment variables.
    pub config: Arc<Config>,
    /// GitHub API client for GraphQL and REST operations.
    ///
    /// Stored as a [`GitHubApi`] trait object so tests and alternative
    /// implementations (e.g. mocks introduced in sibling beads) can be
    /// substituted for the real [`GitHubClient`](unblock_github::client::GitHubClient) without changing the handler
    /// surface.
    pub github: Arc<dyn GitHubApi>,
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
    /// Used by the `prime` tool's internal `SessionMeta` projection to
    /// surface the raw client name in the rendered markdown output.
    pub agent_client: OnceLock<AgentClient>,
    /// UTC timestamp recorded when `initialize()` is called.
    ///
    /// Represents the session start time. Used by the `prime` tool's
    /// internal `SessionMeta` projection for the `connected_at` field.
    pub connected_at: OnceLock<chrono::DateTime<Utc>>,
}

/// Local newtype that wraps a [`Config`] reference and renders it via
/// [`Debug`](std::fmt::Debug) with the GitHub PAT replaced by a redaction
/// marker.
///
/// This wrapper exists solely to keep [`ServerState`]'s [`Debug`] impl from
/// leaking the token to logs, traces, or panic messages. It is **not** a
/// substitute for `Config`'s derived [`Debug`] impl — other call sites that
/// format `Config` directly remain unchanged on purpose.
struct RedactedConfig<'a>(&'a Config);

impl std::fmt::Debug for RedactedConfig<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("token", &"[REDACTED]")
            .field("api_base_url", &self.0.api_base_url)
            .field("github_url", &self.0.github_url)
            .field("repo", &self.0.repo)
            .field("project_number", &self.0.project_number)
            .field("agent", &self.0.agent)
            .field("cache_ttl", &self.0.cache_ttl)
            .field("log_level", &self.0.log_level)
            .field("otel_endpoint", &self.0.otel_endpoint)
            .finish()
    }
}

impl std::fmt::Debug for ServerState {
    /// Manual [`Debug`] implementation that:
    ///
    /// - Renders [`Config`] through a private `RedactedConfig` wrapper so the
    ///   GitHub PAT stored in `config.token` never leaks via `{:?}` formatting.
    /// - Substitutes a meaningful identifier for the `github` field instead
    ///   of forwarding to a [`Debug`](std::fmt::Debug) impl that does not
    ///   exist on [`dyn GitHubApi`](GitHubApi). The label is obtained via
    ///   [`GitHubApi::debug_label`], which defaults to
    ///   `std::any::type_name::<Self>()` and therefore reports the concrete
    ///   implementation type behind the trait object (e.g. `GitHubClient` or
    ///   `MockGitHubClient`).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerState")
            .field("config", &RedactedConfig(&self.config))
            .field("github", &self.github.debug_label())
            .field("cache", &self.cache)
            .field("agent_kind", &self.agent_kind)
            .field("agent_client", &self.agent_client)
            .field("connected_at", &self.connected_at)
            .finish()
    }
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

    /// Returns the alphabetically-sorted list of tool names advertised by
    /// this server.
    ///
    /// Backed by the same [`ToolRouter::list_all`] vector that the MCP
    /// `list_tools` handler returns during the client handshake. Exposed
    /// as a lightweight accessor so integration tests can pin the 17-tool
    /// contract (SPEC §6) without spinning up an MCP transport or
    /// fabricating a [`RequestContext`].
    ///
    /// [`RequestContext`]: rmcp::service::RequestContext
    #[must_use]
    pub fn tool_names(&self) -> Vec<String> {
        self.tool_router
            .list_all()
            .into_iter()
            .map(|tool| tool.name.into_owned())
            .collect()
    }
}

/// Set project fields on a newly created issue's project item.
///
/// Generates [`set_project_fields`] with the correct visibility:
/// `pub` when the `test-hooks` feature is enabled (integration tests),
/// `pub(crate)` otherwise (production builds).
macro_rules! define_set_project_fields {
    ($vis:vis) => {
        /// Updates Priority, Status, `StoryPoints`, and `DeferUntil`.
        /// Each field update is best-effort: failures are logged as warnings
        /// but do not abort the remaining updates. This keeps the create flow
        /// resilient to partial project configuration (e.g. missing option
        /// values).
        ///
        /// The `status` parameter controls the initial Status field value.
        /// Callers MUST source the string from
        /// [`unblock_core::types::Status::option_name`] — never a raw
        /// literal. Per `unblock-1zj` (spec §8.3) `create` always lands
        /// new issues in `Status::Backlog.option_name()` (= `"Backlog"`)
        /// regardless of blocker state, because Backlog is sticky.
        ///
        /// Priority uses prefix matching so callers can pass short codes
        /// like `"P0"` which resolve to the full option name `"P0 - Critical"`.
        ///
        /// Exposed to integration tests when the `test-hooks` feature is
        /// enabled. Production builds keep this `pub(crate)` so it never
        /// appears on the library surface.
        #[allow(clippy::too_many_arguments)]
        $vis async fn set_project_fields(
            client: &dyn GitHubApi,
            project_id: &str,
            item_id: &str,
            field_ids: &unblock_github::projects::ProjectFieldIds,
            priority: &str,
            status: &str,
            story_points: Option<f64>,
            defer_until: Option<chrono::NaiveDate>,
        ) {
            use unblock_github::projects::FieldValue;

            // Set Priority (prefix match: "P0" -> "P0 - Critical", etc.).
            if let Some(option_id) = field_ids.priority.option_id_by_prefix(priority)
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

            // Set Status. Per `unblock-1zj` (spec §8.3 / Decision 2 — Backlog
            // sticky), `create` lands every new issue in
            // `Status::Backlog.option_name()` regardless of blocker state;
            // the pre-`unblock-1zj` `ready` / `blocked` branch on
            // `blocked_by_refs.is_empty()` is REMOVED. Other write tools
            // (e.g. `claim`, `update`) drive Status away from `Backlog` via
            // explicit user/agent transitions — they pass their own
            // canonical option name through `status` here.
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
    };
}

#[cfg(feature = "test-hooks")]
define_set_project_fields!(pub);

#[cfg(not(feature = "test-hooks"))]
define_set_project_fields!(pub(crate));

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

/// Log-message config for the close handler's Status=closed write
/// (unblock-29p.24 — see [`crate::tools::update_status_field_best_effort`]).
///
/// `option_missing_warn` is `None` to preserve the pre-refactor silent
/// behaviour from the original `if let Some(option_id) = ... && let Err(e)
/// = ...` chain — close did NOT warn when the configured Status field had
/// no `closed` option (unlike `reopen` and `dep_remove`, which do).
static CLOSE_HANDLER_STATUS_LOG: crate::tools::StatusUpdateLogConfig =
    crate::tools::StatusUpdateLogConfig {
        no_field_ids_debug: Some(
            "No field IDs cached — run setup first to enable project field assignment",
        ),
        resolve_project_warn: Some(
            "Failed to resolve project info — closed issue fields will not be set",
        ),
        item_id_warn: "Failed to get project item ID for closed issue — fields will not be set",
        option_missing_warn: None,
        update_field_warn: "Failed to set Status=closed on closed issue",
    };

/// Log-message config for the close-cascade Status=ready write per
/// cascaded dependent (unblock-29p.24 — see
/// [`crate::tools::update_status_field_best_effort`]).
///
/// `no_field_ids_debug` and `resolve_project_warn` are both `None` to
/// preserve the pre-refactor outer `if let Some(field_ids) && let
/// Ok(project_info)`-chain behaviour — the cascade iterates over many
/// dependents and silently swallowed missing setup so logs did not flood
/// per-iteration when the project was misconfigured.
/// `option_missing_warn` is also `None` for the same reason (the
/// pre-refactor `if cascaded_issue.status != ... && let Some(option_id) =
/// ... && let Err(e) = ...` chain silently exited on a missing option).
/// Per-iteration `cascaded_qid` context is propagated via a
/// `tracing::info_span!` wrapping the helper invocation.
static CLOSE_CASCADE_STATUS_LOG: crate::tools::StatusUpdateLogConfig =
    crate::tools::StatusUpdateLogConfig {
        no_field_ids_debug: None,
        resolve_project_warn: None,
        item_id_warn: "Failed to get project item ID for cascaded issue",
        option_missing_warn: None,
        update_field_warn: "Failed to set Status=ready on cascaded issue",
    };

/// Log-message config for the depends handler's Status=blocked write on
/// a configured-repo source issue (unblock-29p.24 — see
/// [`crate::tools::update_status_field_best_effort`]).
///
/// `option_missing_warn` is `None` to preserve the pre-refactor silent
/// behaviour from the original `if let Some(option_id) = ... && let Err(e)
/// = ...` chain. The cross-repo gate at the call site short-circuits
/// before this helper is invoked, so the helper never observes
/// cross-repo source issues here (per spec §5 cross-repo scope table).
static DEPENDS_HANDLER_STATUS_LOG: crate::tools::StatusUpdateLogConfig =
    crate::tools::StatusUpdateLogConfig {
        no_field_ids_debug: Some(
            "No field IDs cached — run setup first to enable project field assignment",
        ),
        resolve_project_warn: Some(
            "Failed to resolve project info — source issue fields will not be set",
        ),
        item_id_warn: "Failed to get project item ID for source issue — fields will not be set",
        option_missing_warn: None,
        update_field_warn: "Failed to set Status=blocked on source issue",
    };

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
    pub async fn init(
        &self,
        Parameters(params): Parameters<InitParams>,
    ) -> Result<Json<InitResult>, ErrorData> {
        let state = self.state();
        let kind = state.agent_kind_str();
        let client = &state.github;

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
            let result = InitResult {
                project_number: existing.number,
                url: existing.url.clone(),
                created: false,
                scope: scope_str.to_owned(),
                hint: format!(
                    "Project already exists. Run `setup` with project number {} to configure fields and views.",
                    existing.number
                ),
            };
            debug_assert!(
                !result.hint.is_empty(),
                "InitResult hint must be non-empty per ARCH §10.18"
            );
            return Ok(Json(result));
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

        let result = InitResult {
            project_number: created.number,
            url: created.url,
            created: true,
            scope: scope_str.to_owned(),
            hint: format!(
                "Project created! Run `setup` with project number {} to configure fields and views.",
                created.number
            ),
        };
        debug_assert!(
            !result.hint.is_empty(),
            "InitResult hint must be non-empty per ARCH §10.18"
        );
        Ok(Json(result))
    }

    /// Configure required Projects V2 fields and views (idempotent).
    ///
    /// Ensures the 7 required custom fields and 5 pre-configured views exist
    /// on the project. With `dry_run: true`, reports what would be created
    /// without mutating anything.
    #[tool(
        name = "setup",
        description = "Configure Projects V2 fields (Status, Priority, PipelineStage, Agent, ClaimedAt, StoryPoints, DeferUntil) and views (://ready, ://team, ://pipeline, ://roadmap, ://timeline). Safe to call repeatedly — existing fields/views are skipped. Use dry_run=true to check without mutating."
    )]
    pub async fn setup(
        &self,
        Parameters(params): Parameters<SetupParams>,
    ) -> Result<Json<SetupResult>, ErrorData> {
        let state = self.state();
        let kind = state.agent_kind_str();
        let dry_run = params.dry_run.unwrap_or(false);

        // Resolve project info — use param override or configured project number.
        let client = &state.github;

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
                // Dry-run cannot detect option-set drift on existing
                // single-select fields without dispatching the heal
                // mutation, so we conservatively report nothing healed.
                // The non-dry-run call path will surface real heal
                // activity.
                fields_healed: Vec::new(),
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
            fields_healed: report.healed,
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
        description = "Get full details for a single issue: body sections, blocking relationships, dependency tree (from cache), and comments. The `issue` field accepts a local number (`42`), a hash-prefixed local number (`#42`), or a cross-repo reference (`owner/repo#42`). Use include_comments=false or include_deps=false to skip optional sections."
    )]
    pub async fn show(
        &self,
        Parameters(params): Parameters<ShowParams>,
    ) -> Result<Json<ShowResult>, ErrorData> {
        let state = self.state();
        let kind = state.agent_kind_str();
        let client = &state.github;

        let include_comments = params.include_comments.unwrap_or(true);
        let include_deps = params.include_deps.unwrap_or(true);

        info!(
            agent.kind = %kind,
            issue = %params.issue,
            include_comments, include_deps, "Show tool invoked"
        );

        // Parse the input string into an IssueRef. Per SPEC §11.1 / plan
        // Task 02.02 "Error-side wiring", parse failures at the tool
        // boundary MUST propagate as `InvalidIssueRefSnafu { input }`
        // through `github_error_to_mcp` (HTTP 400 → MCP `-32602`). The
        // raw user string flows into `input` so the agent can see
        // exactly what they sent.
        let issue_ref = params
            .issue
            .parse::<unblock_core::types::IssueRef>()
            .map_err(|_| {
                crate::errors::github_error_to_mcp(unblock_github::errors::Error::from(
                    InvalidIssueRefSnafu {
                        input: params.issue.clone(),
                    }
                    .build(),
                ))
            })?;

        // Step 1: Fetch the full issue via execute_read_tool. `fetch_issue_ref`
        // dispatches to the local repo for `Local` refs and to the target
        // `owner/repo` for `CrossRepo` refs.
        let issue =
            execute_read_tool(state, || async { client.fetch_issue_ref(&issue_ref).await }).await?;

        // Step 2: Parse body sections.
        let body_sections = unblock_core::types::BodySections::from_markdown(
            issue.body.as_deref().unwrap_or_default(),
        );

        // Step 3: Extract blocking/blocked_by from the issue.
        let blocking: Vec<ShowRelatedIssue> = issue.blocking.iter().map(Into::into).collect();
        let blocked_by: Vec<ShowRelatedIssue> = issue.blocked_by.iter().map(Into::into).collect();

        // Step 3b: Extract parent and sub-issues from the issue.
        let parent: Option<ShowRelatedIssue> = issue.parent.as_ref().map(Into::into);
        let sub_issues: Vec<ShowRelatedIssue> = issue.sub_issues.iter().map(Into::into).collect();

        // Step 4: If include_deps, get dependency tree from cached graph.
        // Only local issues live in the cached graph — cross-repo targets
        // return `None` (no tree) since the graph doesn't track them.
        let (upstream, downstream) = if include_deps {
            let (owner, repo, number) = match &issue_ref {
                unblock_core::types::IssueRef::Local(n) => {
                    (client.owner().to_owned(), client.repo().to_owned(), *n)
                }
                unblock_core::types::IssueRef::CrossRepo {
                    owner,
                    repo,
                    number,
                } => (owner.clone(), repo.clone(), *number),
            };
            let issue_qid = unblock_core::types::QualifiedId::new(&owner, &repo, number);
            match state.cache.get_graph().await {
                Some(graph) => {
                    let tree = graph.dependency_tree(
                        &issue_qid,
                        unblock_core::types::TraversalDirection::Both,
                        3,
                    );
                    (
                        Some(tree.upstream.iter().map(ShowTreeNode::from_core).collect()),
                        Some(
                            tree.downstream
                                .iter()
                                .map(ShowTreeNode::from_core)
                                .collect(),
                        ),
                    )
                }
                None => (None, None),
            }
        } else {
            (None, None)
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
            status: issue.status.to_string(),
            priority: issue.priority.to_string(),
            agent: issue.agent.clone(),
            claimed_at: issue.claimed_at.map(|dt| dt.to_rfc3339()),
            pipeline_stage: issue.pipeline_stage.map(|ps| ps.to_string()),
            story_points: issue.story_points,
            defer_until: issue.defer_until.map(|d| d.to_string()),
            labels: issue.labels.clone(),
            milestone: issue.milestone.clone(),
            assignees: issue.assignees.clone(),
            state: issue.state.to_string(),
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
            upstream,
            downstream,
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
    pub async fn ready(
        &self,
        Parameters(params): Parameters<ReadyParams>,
    ) -> Result<Json<ReadyResult>, ErrorData> {
        // Forward to the extracted handler (SPEC §11.4 retro-fit, unblock-cjv).
        // The #[tool] wrapper does no work beyond routing — all logic, including
        // cache warm-up and `cross_repo_refs` computation, lives in
        // `crate::tools::ready::handle_ready` so integration tests can drive it
        // without booting UnblockServer (dep_cycles house style).
        let state = self.state();
        let result = crate::tools::ready::handle_ready(state, params).await?;
        Ok(Json(result))
    }

    /// Claim an issue for an agent — marks it as in-progress.
    ///
    /// Validates the issue is open, unblocked, not deferred, and not already
    /// claimed. Then updates Projects V2 fields (Status=In Progress, Agent=name,
    /// and posts a claim comment.
    ///
    /// Validation order (cheapest first): closed, blocked, deferred, already claimed.
    ///
    /// This is a write tool — the cache is invalidated and rebuilt after all
    /// mutations complete.
    #[tool(
        name = "claim",
        description = "Claim an issue for an agent. Validates the issue is open, unblocked, not deferred, and not already claimed. Sets Status=In Progress, Agent=name, and posts a comment. Triggers graph rebuild."
    )]
    pub async fn claim(
        &self,
        Parameters(params): Parameters<ClaimParams>,
    ) -> Result<Json<ClaimResult>, ErrorData> {
        let state = self.state();
        let kind = state.agent_kind_str();
        let client = Arc::clone(&state.github);
        let config = Arc::clone(&state.config);

        let issue_number = params.id;
        // Reject empty/whitespace-only agent strings (unblock-b6b.80). Falling
        // through to config fallback on empty would mask caller intent.
        crate::tools::claim::validate_agent(params.agent.as_deref())
            .map_err(crate::errors::github_error_to_mcp)?;
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
                // Capture `now` once and reuse across the Projects V2 ladder,
                // the claim comment, and the response payload. This guarantees
                // that SPEC §8.1 step 3 "Claimed At → now", the claim-comment
                // timestamp, and `ClaimResult::claimed_at` are byte-for-byte
                // consistent — a single authoritative wall-clock read.
                let now = Utc::now();
                validate_claimable(&candidate, now.date_naive())?;

                // Step 6 (SPEC §8.1 step 3): Update Projects V2 fields in the
                // three-write ladder — Status, Agent, Claimed At. Each rung
                // logs on failure and continues so a flaky non-Status write
                // does not block the claim (matches the existing swallow-
                // and-warn posture of the Status and Agent rungs).
                if let Some(field_ids) = client.field_ids().await {
                    if let Ok(project_info) = client.resolve_project_info().await {
                        if let Ok(item_id) = client
                            .get_project_item_id(&issue.node_id, &project_info.id)
                            .await
                        {
                            // Status -> In Progress (sourced from
                            // `Status::option_name` per spec §2.3 / §8.1
                            // — `claim` is one of the explicit transitions
                            // out of Backlog).
                            if let Some(option_id) = field_ids
                                .status
                                .options
                                .get(unblock_core::types::Status::InProgress.option_name())
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

                            // Claimed At -> now.date_naive()
                            // Projects V2 `Date` field — serializes to ISO
                            // 8601 YYYY-MM-DD per `FieldValue::Date` contract
                            // (projects.rs:98-99). Uses the same `now`
                            // captured above so the date on the Projects V2
                            // field matches the response payload and the
                            // claim-comment timestamp.
                            if let Err(e) = client
                                .update_field(
                                    &project_info.id,
                                    &item_id,
                                    &field_ids.claimed_at,
                                    &FieldValue::Date(now.date_naive()),
                                )
                                .await
                            {
                                tracing::warn!(error = %e, "Failed to set Claimed At field");
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
    /// via the GitHub API, updates Projects V2 fields (Status=Done),
    /// rebuilds the cache, then computes the unblock cascade. For each newly
    /// unblocked issue, updates its Projects V2 fields (Status=Backlog if not
    /// already `InProgress`) and posts an unblock comment.
    ///
    /// This is a write tool. The handler runs in four explicit phases per
    /// SPEC §8.2 + §3.4 Critical (GAP-15 remediation):
    ///
    /// - **Phase 0 (PRE-close cascade capture):** ensures the cache is
    ///   primed (cold-cache path calls [`rebuild_cache`][`crate::tools::rebuild_cache`]),
    ///   reads the graph, and calls
    ///   [`compute_unblock_cascade`][`unblock_core::graph::DependencyGraph::compute_unblock_cascade`]
    ///   while the closed issue is still an OPEN node. The resulting
    ///   `Vec<QualifiedId>` is captured in a handler-local binding and
    ///   used authoritatively by Phases 2 and 3 — it is NOT re-read
    ///   from the post-close cache. After bead `unblock-a36` the
    ///   POST-close cache DOES include the just-closed issue (as
    ///   `Closed`), but PRE-close ordering stays mandatory per SPEC
    ///   §8.2 step 2 to freeze the cascade snapshot against concurrent
    ///   blocker-state mutations and to sidestep the
    ///   already-closed-dependent walk nuance in
    ///   `compute_unblock_cascade` (see `tools::close` module doc).
    /// - **Phase 1 (MUTATION):** `execute_write_tool` runs
    ///   `fetch_issue` + state validation + `close_issue` + Projects V2
    ///   `Status → closed` ladder + cache rebuild.
    /// - **Phase 2 (CASCADE FIELD-UPDATE LOOP):** iterates the Phase-0
    ///   captured list, dispatching per-dependent side-effects via the
    ///   `*_ref` primitives (SPEC §8.2 step 6 / §5.6 `close` row).
    /// - **Phase 3 (RESPONSE PROJECTION):** partitions the Phase-0
    ///   cascade into `unblocked: Vec<u64>` + `cross_repo_refs`.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorData`] in the following cases:
    /// - Phase 0 cold-cache prime fails (`fetch_graph_data` 503) → a
    ///   503-class [`PreMutationPrimeFailed`](unblock_github::errors::Error::PreMutationPrimeFailed)
    ///   error is surfaced *before* the close mutation. The mutation is
    ///   NOT attempted on an empty graph. Recovery: retry or run `prime`
    ///   first. (Bead `unblock-29p.69` introduced this dedicated variant
    ///   as the symmetric pre-mutation counterpart to
    ///   [`PostMutationRebuildFailed`](unblock_github::errors::Error::PostMutationRebuildFailed).)
    /// - `fetch_issue` fails (e.g. 404) → mapped via `github_error_to_mcp`.
    /// - The fetched issue is already Closed → `IssueClosed` domain error.
    /// - `close_issue` fails → mapped via `github_error_to_mcp`.
    /// - Cache rebuild fails after Phase 1 and leaves the cache empty →
    ///   a 503-class
    ///   [`PostMutationRebuildFailed`](unblock_github::errors::Error::PostMutationRebuildFailed)
    ///   (mutation `"close_cascade"`) error is surfaced. The cascade list
    ///   from Phase 0 is durable in memory and the close mutation has
    ///   already landed; the error signals only that the step 8
    ///   `update_status_fields` reconciliation could not run (R3 — see
    ///   `tools::close` module-doc). Recovery: re-run `show`.
    #[tool(
        name = "close",
        description = "Close an issue and cascade-unblock dependents. Validates the issue is open, closes it, updates project fields (Status=Done), and auto-unblocks any dependent issues whose blockers are now all closed. Returns the list of newly unblocked issue numbers. Triggers graph rebuild."
    )]
    pub async fn close(
        &self,
        Parameters(params): Parameters<CloseParams>,
    ) -> Result<Json<CloseResult>, ErrorData> {
        let state = self.state();
        let kind = state.agent_kind_str();
        let client = Arc::clone(&state.github);

        let issue_number = params.id;
        // Treat Some("") (or whitespace-only) the same as None to avoid posting
        // an empty comment to the issue timeline. See unblock-b6b.85.
        let reason = params.reason.filter(|r| !r.trim().is_empty());

        info!(
            agent.kind = %kind,
            issue_number,
            reason = reason.as_deref(),
            "Close tool invoked"
        );

        // Phase 0: PRE-CLOSE cascade capture (SPEC §8.2 step 2 / §3.4
        // Critical). The cascade MUST be computed against a graph that
        // still contains the closed issue as an OPEN node — this is the
        // only chokepoint where the cascade list can be captured soundly.
        // After bead `unblock-a36` widened `fetch_graph_data` to
        // `states: [OPEN, CLOSED]` (see `unblock-github/src/graphql.rs`
        // FETCH_GRAPH_DATA_QUERY), the post-close rebuild WILL contain
        // the just-closed node (as `IssueState::Closed`) and the
        // `blocker_qid == closed_id` special-case at
        // `unblock-core/src/graph.rs:312-314` would let the walk
        // proceed. PRE-close ordering is nevertheless mandatory because
        // already-closed dependents would enter the `Incoming`
        // traversal on POST-close rebuild and
        // `compute_unblock_cascade` does not filter them out on
        // `issue_state == Closed` alone, and because any concurrent
        // blocker mutation between the close and the rebuild could
        // silently shift the cascade set. GAP-15 fixed the original
        // correctness defect; bead `unblock-a36` reshapes the
        // rationale without reordering phases.
        //
        // Cold-cache prime: if the cache is empty (first tool after
        // server start, or a stale invalidation from a prior write), use
        // the existing `rebuild_cache` helper to fetch and populate the
        // graph. If the prime itself fails (transient 503 during
        // `fetch_graph_data`), the cache stays empty and this handler
        // cannot proceed — surface a pre-mutation 503 so the caller
        // knows the close was NOT attempted. Distinct from the
        // post-close R3 path (rebuild-after-close) which fires only
        // after `execute_write_tool` lands the mutation on GitHub.
        let issue_qid =
            unblock_core::types::QualifiedId::new(client.owner(), client.repo(), issue_number);

        if state.cache.get_graph().await.is_none() {
            tracing::debug!(
                issue_number,
                "Cache cold at Phase 0 — priming via rebuild_cache before cascade capture"
            );
            crate::tools::rebuild_cache(state).await;
        }

        let Some(pre_close_graph) = state.cache.get_graph().await else {
            tracing::warn!(
                issue_number,
                "Cache empty after prime attempt — cannot capture pre-close cascade; aborting close"
            );
            // PRE-mutation prime failure — surfaces via the dedicated
            // `Error::PreMutationPrimeFailed` variant (bead
            // `unblock-29p.69`), the symmetric pre-mutation counterpart
            // to `PostMutationRebuildFailed`. The variant's `Display`
            // matches the wording previously synthesized via
            // `GitHubApiSnafu { status: 503 }` (the `prime the
            // dependency graph` / `before cascade capture` / `retry or
            // run `prime` first` contract pinned by the
            // `close_surfaces_error_when_phase0_prime_fails` regression
            // guard), so no message-text drift. Status code stays 503;
            // `github_error_to_mcp` maps 503 → INTERNAL_ERROR.
            return Err(crate::errors::github_error_to_mcp(
                unblock_github::errors::PreMutationPrimeFailedSnafu { qid: issue_qid }.build(),
            ));
        };

        // compute_unblock_cascade's _all_issues param is currently unused —
        // pass an empty slice (see graph.rs:215-220 for rationale).
        let cascade = pre_close_graph.compute_unblock_cascade(&issue_qid, &[]);
        // Drop the Arc early so the write-tool rebuild's update() is not
        // blocked on a lingering reader (cache uses RwLock — see
        // GraphCache::update).
        drop(pre_close_graph);

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
                // Status → closed. Delegated to the shared
                // [`update_status_field_best_effort`] helper
                // (unblock-29p.24) — see [`CLOSE_HANDLER_STATUS_LOG`] for
                // the per-rung message strings, including the silent-on-
                // missing-option behaviour preserved bit-for-bit from the
                // pre-refactor `if let Some(option_id) = ... && let
                // Err(e) = ...` chain.
                crate::tools::update_status_field_best_effort(
                    client.as_ref(),
                    &issue.node_id,
                    unblock_core::types::Status::Closed.option_name(),
                    &CLOSE_HANDLER_STATUS_LOG,
                )
                .await;

                Ok(())
            }
        })
        .await?;

        // R3 — honest partial state under PRE-close ordering (see
        // `tools::close` module-doc): if the rebuild inside
        // `execute_write_tool` failed, the cache is empty. The cascade
        // list captured in Phase 0 is durable in memory and the close
        // mutation has already succeeded server-side, so the response
        // projection below is still authoritative. However, the step 8
        // `update_status_fields` reconciliation (SPEC §8.2 step 8 —
        // cross-check Status fields for issues NOT already handled by
        // the Phase 2 cascade loop) requires the rebuilt graph and
        // cannot run against an empty cache. Surface a 503-class error
        // instructing the caller to re-run `show` so the Status
        // fan-out is reconciled on the next read. Preserves §14
        // invariants 8 and 13 (no fictional Status-sync claims when
        // the graph cannot be consulted).
        let rebuild_graph_available = state.cache.get_graph().await.is_some();

        // Phase 2: cascade field-update loop. Iterates the Phase-0
        // captured list (not the post-close rebuilt cache) so the
        // correctness contract in SPEC §8.2 step 6 holds even when the
        // post-close rebuild fails — the dependents we must update are
        // known at mutation time. Each per-dependent update is
        // best-effort; individual failures are logged and the cascade
        // continues.
        //
        // SPEC §8.2 step 6 / §11.4 row 4 / §5.6 row `close`: cross-repo
        // dependents ARE still cascade-updated. The loop dispatches via
        // the `*_ref` primitives so the comment and issue-fetch land on
        // the correct `(owner, repo)` — bare-`u64` variants would silently
        // retarget the configured repo (unblock-eos.13 finding RISK #2).
        // Project-field updates remain scoped to the configured board
        // because `project_id` / `item_id` are globally scoped node IDs;
        // if a cross-repo dependent is not on the board,
        // `get_project_item_id` fails best-effort and the comment still
        // lands — matching the spec intent of "cascade side-effects only".
        let configured_owner = client.owner().to_owned();
        let configured_repo = client.repo().to_owned();
        for cascaded_qid in &cascade {
            // Normalize to IssueRef so the downstream call dispatches via
            // the `_ref` primitive: `add_comment_ref` matches on the
            // IssueRef variant, and both arms ultimately funnel through
            // `add_comment_in_repo` — `Local` delegates to `add_comment`
            // (which calls `add_comment_in_repo` with the configured
            // `(owner, repo)`), and `CrossRepo` calls `add_comment_in_repo`
            // directly with the ref's own `(owner, repo)`. Collapsing a
            // QualifiedId whose `(owner, repo)` matches the configured
            // repo to `Local` keeps the configured-repo path tagged as
            // such (and preserves the `add_comment` tracing span /
            // instrumented fields) rather than routing an
            // effectively-local ref through the CrossRepo arm.
            let cascaded_ref =
                if cascaded_qid.owner == configured_owner && cascaded_qid.repo == configured_repo {
                    unblock_core::types::IssueRef::Local(cascaded_qid.number)
                } else {
                    unblock_core::types::IssueRef::CrossRepo {
                        owner: cascaded_qid.owner.clone(),
                        repo: cascaded_qid.repo.clone(),
                        number: cascaded_qid.number,
                    }
                };
            // Post unblock comment. Stays best-effort per the pre-existing
            // §8.2 step 6 semantics — the token may lack write scope on a
            // foreign repo and we must not tear down the cascade for a
            // permission error (see unblock-eos.13 investigation Risks).
            // Qualified ID is logged so operators can distinguish
            // local-repo failures from cross-repo permission denials.
            let comment_body = format!("\u{2705} Unblocked by closing #{issue_number}");
            if let Err(e) = client.add_comment_ref(&cascaded_ref, comment_body).await {
                tracing::warn!(
                    cascaded_qid = %cascaded_qid,
                    error = %e,
                    "Failed to post unblock comment on cascaded issue"
                );
            }

            // Update Projects V2 fields:
            // Status → ready (if not already InProgress).
            //
            // Delegated to the shared
            // [`update_status_field_best_effort`] helper (unblock-29p.24)
            // — see [`CLOSE_CASCADE_STATUS_LOG`] for the per-rung message
            // strings. The cascade config silences the outer field-IDs
            // and project-resolution rungs so per-iteration log spam
            // from a misconfigured project does not flood the cascade
            // loop (matching the pre-refactor outer `if let Some(field_ids)
            // && let Ok(project_info)` chain bit-for-bit). The
            // `fetch_issue_ref` and `cascaded_issue.status != InProgress`
            // gate stay at this call site — the helper takes a `node_id`
            // and a slug, not a "fetch then maybe-update" closure.
            // Wrapping the helper future in a `tracing::info_span!`
            // propagates `cascaded_qid` to every log record the helper
            // emits, preserving the per-iteration structured field on
            // the pre-refactor warns.
            //
            // Fetch the cascaded issue (by ref, not bare number) to get
            // its node_id and current status. For cross-repo dependents
            // this reads from the foreign repo; for local the _ref
            // variant delegates to the unchanged path.
            match client.fetch_issue_ref(&cascaded_ref).await {
                Ok(cascaded_issue) => {
                    // Spec §8.2 step 6: `Backlog` is sticky — a
                    // graph-driven cascade does NOT promote a Backlog
                    // dependent out of Backlog. Skip the Status update
                    // entirely; the unblock comment above still landed.
                    // Also skip when the dependent is already InProgress
                    // (pre-existing rule — preserves agent claims).
                    let cascaded_status = cascaded_issue.status;
                    if cascaded_status != unblock_core::types::Status::InProgress
                        && cascaded_status != unblock_core::types::Status::Backlog
                    {
                        use tracing::Instrument as _;
                        crate::tools::update_status_field_best_effort(
                            client.as_ref(),
                            &cascaded_issue.node_id,
                            unblock_core::types::Status::Ready.option_name(),
                            &CLOSE_CASCADE_STATUS_LOG,
                        )
                        .instrument(tracing::info_span!(
                            "close_cascade_status_update",
                            cascaded_qid = %cascaded_qid,
                        ))
                        .await;
                    } else if cascaded_status == unblock_core::types::Status::Backlog {
                        tracing::debug!(
                            cascaded_qid = %cascaded_qid,
                            "Cascaded dependent is in Backlog — Status update skipped per §8.2 step 6 sticky-Backlog rule"
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        cascaded_qid = %cascaded_qid,
                        error = %e,
                        "Failed to fetch cascaded issue for field updates"
                    );
                }
            }
        }

        // Phase 3: response projection. SPEC §8.2 flow step 9 / §11.4
        // row 4: partition the cascade list (captured Phase 0) into
        // local dependents (projected to `unblocked: Vec<u64>`) vs
        // cross-repo dependents (surfaced via `cross_repo_refs`).
        // Phase 2 above intentionally does NOT gate on
        // (owner, repo) == (config.owner, config.repo) — cross-repo
        // dependents ARE still cascade-updated; only the response
        // shape differs here (SPEC §11.4 affected-tools table).
        let (local_unblocked, cross_repo_accum) =
            crate::tools::cross_repo::project_cascade(&cascade, client.owner(), client.repo());
        let unblocked = local_unblocked;
        let cross_repo_refs = crate::tools::cross_repo::build_cross_repo_refs_with_summary(
            cross_repo_accum,
            crate::tools::cross_repo::close_summary,
        );

        // R3 post-close rebuild failure (refocused under PRE-close
        // ordering per GAP-15 / SPEC §8.2 "Post-rebuild field-sync
        // failure"): the Phase-0 cascade list is authoritative in the
        // response envelope, the close mutation is durable on GitHub,
        // and the Phase 2 cascade field-updates were applied
        // best-effort. The remaining post-close step is the step 8
        // `update_status_fields` reconciliation — cross-check Status
        // fields for issues NOT already handled by the Phase 2
        // cascade loop — which requires the rebuilt graph. If that
        // graph is missing (rebuild failed), surface a 503-class
        // error instructing the caller to re-run `show` to confirm
        // the Status fan-out. This does NOT invalidate the cascade
        // list returned above.
        if !rebuild_graph_available {
            tracing::warn!(
                issue_number,
                "Cache not available after close — rebuild failed; step 8 `update_status_fields` reconciliation could not run"
            );
            return Err(crate::errors::github_error_to_mcp(
                unblock_github::errors::PostMutationRebuildFailedSnafu {
                    mutation: "close_cascade".to_owned(),
                    qid: issue_qid.clone(),
                }
                .build(),
            ));
        }

        Ok(Json(CloseResult {
            issue: issue_number,
            unblocked,
            cross_repo_refs,
        }))
    }

    /// Add a blocking dependency between two issues.
    ///
    /// Makes the source issue blocked by the target issue. Validates the source
    /// exists, checks for cycles and duplicates using the cached graph, creates
    /// the blocking relationship via the GitHub API, updates Projects V2 fields
    /// (Status=Blocked) on the source, and rebuilds the
    /// cache.
    ///
    /// The target accepts a local issue number (e.g. `"42"`) or a cross-repo
    /// reference in `owner/repo#number` format (e.g. `"websublime/other-repo#7"`).
    ///
    /// This is a write tool — uses `execute_write_tool` for the mutation and
    /// cache rebuild.
    #[tool(
        name = "depends",
        description = "Add a blocking dependency: source becomes blocked by target. Validates both issues exist, rejects cycles and duplicates, and rejects source == target. Both source and target accept a local number (42 / #42) or owner/repo#number for cross-repo. Updates project fields (Status=Blocked) on source when source is local to the configured project. Triggers graph rebuild."
    )]
    pub async fn depends(
        &self,
        Parameters(params): Parameters<DependsParams>,
    ) -> Result<Json<DependsResult>, ErrorData> {
        let state = self.state();
        let kind = state.agent_kind_str();
        let client = Arc::clone(&state.github);

        let source_str = params.source.clone();
        let target_str = params.target.clone();

        info!(
            agent.kind = %kind,
            source = %source_str,
            target = %target_str,
            "Depends tool invoked"
        );

        // Parse source string into IssueRef. Per SPEC §11.1 / plan Task
        // 02.02 "Error-side wiring", parse failures at the tool boundary
        // MUST propagate as `InvalidIssueRefSnafu { input }` through
        // `github_error_to_mcp` (HTTP 400 → MCP `-32602`).
        let source_ref_raw = source_str
            .parse::<unblock_core::types::IssueRef>()
            .map_err(|_| {
                crate::errors::github_error_to_mcp(unblock_github::errors::Error::from(
                    InvalidIssueRefSnafu {
                        input: source_str.clone(),
                    }
                    .build(),
                ))
            })?;

        // Parse target string into IssueRef. See `source_ref_raw` for
        // the wiring rationale.
        let target_ref_raw = target_str
            .parse::<unblock_core::types::IssueRef>()
            .map_err(|_| {
                crate::errors::github_error_to_mcp(unblock_github::errors::Error::from(
                    InvalidIssueRefSnafu {
                        input: target_str.clone(),
                    }
                    .build(),
                ))
            })?;

        // Normalize both refs against the configured repo. A
        // `CrossRepo { owner, repo, number }` that happens to spell the
        // configured repo collapses to `Local(number)`, so every
        // downstream guard that dispatches on `Local` vs `CrossRepo`
        // (fetch path, cycle detection, Projects V2 field update,
        // mutation dispatch) treats aliased forms identically to the
        // canonical local form. See `IssueRef::normalize` for details.
        let source_ref = source_ref_raw.normalize(client.owner(), client.repo());
        let target_ref = target_ref_raw.normalize(client.owner(), client.repo());

        // Spec §8.4: source != target is a required validation.
        // Compare on the fully-qualified id (resolved against the configured
        // repo) so that e.g. `"42"` and `"#42"` collapse to the same identity
        // and the check also rejects a Local vs. CrossRepo pointing at the
        // same configured-repo issue. Both `resolve` and `normalize` agree
        // on this identity, so using the normalized refs here is equivalent
        // to using the raw refs.
        let source_qid = source_ref.resolve(client.owner(), client.repo());
        let target_qid = target_ref.resolve(client.owner(), client.repo());
        if source_qid == target_qid {
            return Err(crate::errors::github_error_to_mcp(
                unblock_core::errors::ValidationSnafu {
                    message: format!(
                        "depends: source and target must differ (both resolved to {source_qid})"
                    ),
                }
                .build()
                .into(),
            ));
        }

        // Step 1: Validate source issue exists, resolving against its own
        // owner/repo so cross-repo sources are fetched correctly. After
        // normalization, a source that aliases the configured repo is
        // `Local(n)` and `fetch_issue_ref` takes the repo-scoped fast path.
        let source_issue = client
            .fetch_issue_ref(&source_ref)
            .await
            .map_err(crate::errors::github_error_to_mcp)?;

        // Step 2: Cycle detection using cached graph.
        // The cache only covers the configured project's repo, so we can
        // only detect cycles when BOTH endpoints are local to that repo.
        // When either endpoint is cross-repo, the cached graph does not
        // contain it; we skip client-side cycle detection and rely on
        // GitHub's server-side rejection at mutation time. Normalization
        // above ensures that a caller spelling the configured repo as
        // `owner/repo#n` still enters this arm.
        match (&source_ref, &target_ref) {
            (
                unblock_core::types::IssueRef::Local(source_number),
                unblock_core::types::IssueRef::Local(target_number),
            ) => {
                if let Some(graph) = state.cache.get_graph().await
                    && graph.would_create_cycle(
                        &unblock_core::types::QualifiedId::new(
                            client.owner(),
                            client.repo(),
                            *source_number,
                        ),
                        &unblock_core::types::QualifiedId::new(
                            client.owner(),
                            client.repo(),
                            *target_number,
                        ),
                    )
                {
                    return Err(crate::errors::github_error_to_mcp(
                        CircularDependencySnafu {
                            source: unblock_core::types::IssueRef::Local(*source_number),
                            target: unblock_core::types::IssueRef::Local(*target_number),
                        }
                        .build()
                        .into(),
                    ));
                }
            }
            _ => {
                tracing::warn!(
                    source = %source_ref,
                    target = %target_ref,
                    "Cross-repo endpoint: client-side cycle detection skipped (graph covers configured repo only); relying on server-side rejection."
                );
            }
        }

        // Step 3 (SPEC §8.4 step 3): Duplicate-edge detection using cached
        // graph. Mirrors the Local/Local scope of the cycle-detection branch
        // above — the cache only covers the configured project's repo, so
        // client-side duplicate detection is only sound when BOTH endpoints
        // are local. Cross-repo pairs fall through to GitHub's server-side
        // rejection at mutation time, matching the cycle-detection posture.
        //
        // The check composes two existing public `Graph` APIs rather than
        // adding a new `edge_exists` helper to `unblock-core`:
        //   1. `node_map().get(&qid)` resolves both endpoints to
        //      `NodeIndex` (same lookup `would_create_cycle` performs
        //      internally at graph.rs:341-346).
        //   2. `inner_graph().contains_edge(src_idx, tgt_idx)` is
        //      petgraph's O(E)-to-O(1) edge-existence probe per edge
        //      (constant-factor for typical project sizes).
        // If either node is absent from the cached graph the edge cannot
        // exist in the cache by construction, so the check short-circuits
        // false and we fall through to the mutation — mirroring the
        // `would_create_cycle` behaviour on unknown nodes at
        // graph.rs:341-346.
        match (&source_ref, &target_ref) {
            (
                unblock_core::types::IssueRef::Local(source_number),
                unblock_core::types::IssueRef::Local(target_number),
            ) => {
                if let Some(graph) = state.cache.get_graph().await {
                    let source_qid = unblock_core::types::QualifiedId::new(
                        client.owner(),
                        client.repo(),
                        *source_number,
                    );
                    let target_qid = unblock_core::types::QualifiedId::new(
                        client.owner(),
                        client.repo(),
                        *target_number,
                    );
                    if let (Some(&source_idx), Some(&target_idx)) = (
                        graph.node_map().get(&source_qid),
                        graph.node_map().get(&target_qid),
                    ) && graph.inner_graph().contains_edge(source_idx, target_idx)
                    {
                        // Edge already exists (`source → target` — source
                        // is already blocked by target). SPEC §8.4 step 3
                        // mandates explicit `DuplicateDependency` rejection
                        // so callers can distinguish legitimate retry from
                        // erroneous double-call. `DuplicateDependency` maps
                        // to HTTP 409 in `DomainError::status_code` →
                        // `INVALID_PARAMS` via `github_error_to_mcp`
                        // (errors.rs:100), consistent with the
                        // `CircularDependency` mapping above.
                        return Err(crate::errors::github_error_to_mcp(
                            DuplicateDependencySnafu {
                                source: unblock_core::types::IssueRef::Local(*source_number),
                                target: unblock_core::types::IssueRef::Local(*target_number),
                            }
                            .build()
                            .into(),
                        ));
                    }
                }
            }
            _ => {
                tracing::warn!(
                    source = %source_ref,
                    target = %target_ref,
                    "Cross-repo endpoint: client-side duplicate-edge detection skipped (graph covers configured repo only); relying on server-side rejection."
                );
            }
        }

        // Step 4: Add blocking relationship and rebuild cache via execute_write_tool.
        execute_write_tool(state, || {
            let client = Arc::clone(&client);
            let source_ref = source_ref.clone();
            let target_ref = target_ref.clone();

            async move { client.add_blocked_by_refs(&source_ref, &target_ref).await }
        })
        .await?;

        // Step 5: Update Projects V2 fields on source issue (Status=Blocked).
        // Only applies when source is local to the configured project: the
        // Projects V2 item lookup (`get_project_item_id`) is scoped to the
        // configured project, so cross-repo sources cannot have their
        // fields updated here per spec §5 cross-repo scope table.
        // Normalization above ensures a caller spelling the configured
        // repo as `owner/repo#n` still takes this branch.
        // Delegated to the shared
        // [`update_status_field_best_effort`] helper (unblock-29p.24) —
        // see [`DEPENDS_HANDLER_STATUS_LOG`] for the per-rung message
        // strings. The cross-repo gate stays at this call site because
        // it short-circuits before any GraphQL round-trip (per spec §5
        // cross-repo scope table), so the helper never observes
        // cross-repo source issues here.
        if matches!(source_ref, unblock_core::types::IssueRef::Local(_)) {
            // Spec §8.4 step 5: `Backlog` is sticky — adding a blocker
            // to a Backlog issue MUST NOT auto-promote it to `Blocked`.
            // The blocker is recorded; the next explicit transition out
            // of Backlog will land it in `Ready`/`Blocked` per the graph.
            if source_issue.status == unblock_core::types::Status::Backlog {
                tracing::debug!(
                    source = %source_ref,
                    "Source is in Backlog — Status update skipped per §8.4 step 5 sticky-Backlog rule"
                );
            } else {
                crate::tools::update_status_field_best_effort(
                    client.as_ref(),
                    &source_issue.node_id,
                    unblock_core::types::Status::Blocked.option_name(),
                    &DEPENDS_HANDLER_STATUS_LOG,
                )
                .await;
            }
        } else {
            tracing::debug!(
                source = %source_ref,
                "Cross-repo source: skipping Projects V2 field update (source is outside the configured project)."
            );
        }

        // Render source and target refs in canonical form. After
        // normalization, `owner/repo#n` aliasing the configured repo
        // renders as `#n` (Local), matching the `"42"` and `"#42"` input
        // forms and fulfilling the "stable output regardless of input
        // form" contract. Cross-repo refs that point at other repos
        // render as `owner/repo#n`.
        let source_rendered = source_ref.to_string();
        let target_rendered = target_ref.to_string();

        Ok(Json(DependsResult {
            created: true,
            source: source_rendered.clone(),
            target: target_rendered.clone(),
            message: format!(
                "Issue {source_rendered} is now blocked by {target_rendered}. Source marked as blocked."
            ),
        }))
    }

    /// Create a new GitHub Issue with optional dependencies, project fields,
    /// and parent linkage.
    ///
    /// Creates the issue via REST, adds it to the configured project, sets
    /// custom fields (Priority, `StoryPoints`, `DeferUntil`, Status),
    /// and optionally adds blocking relationships and parent linkage.
    ///
    /// This is a write tool — the cache is invalidated and rebuilt after all
    /// mutations complete.
    #[tool(
        name = "create",
        description = "Create a new GitHub Issue. Set title (required), issue_type (default Task), priority (default P2), body, labels, blocked_by (local number or owner/repo#number), parent, story_points, defer_until. Labels are auto-created if missing. Triggers graph rebuild."
    )]
    pub async fn create(
        &self,
        Parameters(params): Parameters<CreateParams>,
    ) -> Result<Json<CreateResult>, ErrorData> {
        let state = self.state();
        let kind = state.agent_kind_str();
        let client = Arc::clone(&state.github);

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

        // Parse blocked_by refs. Per SPEC §11.1 / plan Task 02.02
        // "Error-side wiring", each per-element parse failure
        // propagates as `InvalidIssueRefSnafu { input }` through
        // `github_error_to_mcp` (HTTP 400 → MCP `-32602`).
        let blocked_by_refs: Vec<unblock_core::types::IssueRef> =
            if let Some(ref refs) = params.blocked_by {
                refs.iter()
                    .map(|s| {
                        s.parse::<unblock_core::types::IssueRef>().map_err(|_| {
                            crate::errors::github_error_to_mcp(unblock_github::errors::Error::from(
                                InvalidIssueRefSnafu { input: s.clone() }.build(),
                            ))
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

                                    // Spec §8.3 step 3: every newly
                                    // created issue lands in
                                    // `Status::Backlog` regardless of
                                    // blocker state. Backlog is sticky
                                    // (§2.3, §3.3 Filter 2, §10.2);
                                    // explicit user/agent transitions
                                    // (e.g. `update`, `claim`) move it
                                    // out. The pre-`unblock-1zj` choice
                                    // of `ready`/`blocked` based on
                                    // `blocked_by_refs.is_empty()` is
                                    // REMOVED.
                                    let initial_status =
                                        unblock_core::types::Status::Backlog.option_name();

                                    set_project_fields(
                                        client.as_ref(),
                                        &project_info.id,
                                        &item_id,
                                        &field_ids,
                                        &priority_owned,
                                        initial_status,
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
    pub async fn comment(
        &self,
        Parameters(params): Parameters<CommentParams>,
    ) -> Result<Json<CommentResult>, ErrorData> {
        let state = self.state();
        let kind = state.agent_kind_str();
        let client = &state.github;
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
    pub async fn update(
        &self,
        Parameters(params): Parameters<UpdateParams>,
    ) -> Result<Json<UpdateResult>, ErrorData> {
        let state = self.state();
        let kind = state.agent_kind_str();
        let client = Arc::clone(&state.github);

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

        // Validate status if provided. Accepts the canonical TitleCase
        // option names sourced from `Status::option_name` (post-
        // `unblock-1zj`). The validator iterates `Status::ALL` so this
        // automatically tracks new variants.
        if let Some(ref s) = params.status
            && !unblock_core::types::Status::ALL
                .iter()
                .any(|status| status.option_name() == s.as_str())
        {
            let valid: Vec<&str> = unblock_core::types::Status::ALL
                .iter()
                .map(|status| status.option_name())
                .collect();
            return Err(ErrorData {
                code: rmcp::model::ErrorCode::INVALID_PARAMS,
                message: format!("Invalid status '{s}' — must be one of {}", valid.join(", "))
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
                                        field_ids.priority.option_id_by_prefix(p.as_str())
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

    /// Session entry point — returns a markdown context blob for agent
    /// orientation.
    ///
    /// Aggregates issue state internally (`in_progress`, `ready`, `blocked`,
    /// `completed`, `hotspots`, `stale`) and renders it as a single
    /// `PrimeResult.context: String` markdown blob ready for injection into
    /// the agent's session prompt. Sections: header → counts → per-category
    /// lists → issues with cycles → session → drift → cross-repo trailer
    /// (SPEC §7.3 + §11.4). Always does a fresh GitHub fetch; bypasses the
    /// cache.
    #[tool(
        name = "prime",
        description = "Session entry point for every agent session. Returns a single markdown `context` blob for agent injection: header (repo/project), counts (ready/blocked/in-progress/completed/hotspots/stale), per-category top-N lists, `## Issues with cycles`, `## Session` (agent identity), `## Drift warnings` (background reconcile), and a trailing `## Cross-repo references` section per SPEC §11.4 when any cycle touched a cross-repo node. Always does a fresh fetch — bypasses cache. Parameters: `stale_threshold_hours` (default 24) gates the stale subsection and the completed window; `max_per_category` (default 10) caps both rendered category lists and cycle-member bullets; `agent` filters in_progress/ready/blocked/stale by exact match (completed and hotspots are never filtered)."
    )]
    pub async fn prime(
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
    /// [`DriftReport`](unblock_core::reconcile::DriftReport) with all detected drift.
    ///
    /// By default operates in read-only mode (`fix: false`). The `fix: true`
    /// repair path is implemented by task 1.6.4.
    ///
    /// After analysis, the cache is updated with the freshly fetched graph data.
    #[tool(
        name = "reconcile",
        description = "Detect drift between the computed dependency graph and GitHub state. Returns a DriftReport listing stale ready states, uncascaded closures, orphaned edges, cycles, and stale claims. Use fix=true to auto-repair (not yet implemented). Always does a fresh fetch — bypasses cache."
    )]
    pub async fn reconcile(
        &self,
        Parameters(params): Parameters<ReconcileParams>,
    ) -> Result<Json<ReconcileOutput>, ErrorData> {
        let state = self.state();
        let kind = state.agent_kind_str();

        info!(agent.kind = %kind, "Reconcile tool invoked");

        let output = crate::tools::reconcile::handle_reconcile(&params, state).await?;
        Ok(Json(output))
    }

    /// Filtered, sorted, paginated view over the open issue set.
    ///
    /// Per SPEC §7.5. Supports filters for `status`, `priority`, `type`,
    /// `milestone`, `agent`, `label`, `assignee`, with `sort`
    /// (`priority`/`created`/`updated`) and `limit`/`offset` pagination.
    ///
    /// This is a read tool — the handler consults the cache (triggering a
    /// lazy rebuild if stale) and never invalidates. See
    /// [`handle_list`](crate::tools::list::handle_list) for the full flow.
    #[tool(
        name = "list",
        description = "List issues (both OPEN and CLOSED) with optional filters (status, priority, issue_type, milestone, agent, label, assignee), sorted by priority (default), created, or updated, paginated via limit (1–200, default 50) and offset. Uses cache; rebuilds lazily if stale."
    )]
    pub async fn list(
        &self,
        Parameters(params): Parameters<ListParams>,
    ) -> Result<Json<ListResult>, ErrorData> {
        let state = self.state();
        let kind = state.agent_kind_str();

        info!(agent.kind = %kind, "List tool invoked");

        let result = crate::tools::list::handle_list(state, params).await?;
        Ok(Json(result))
    }

    /// Full-text search via GitHub's Issue Search API.
    ///
    /// Per SPEC §7.6. Bypasses the cache entirely — each call issues a
    /// single REST request. The transport layer prepends
    /// `repo:{owner}/{repo} is:issue` automatically; callers can append
    /// any GitHub search qualifier (e.g. `label:bug author:octocat`).
    #[tool(
        name = "search",
        description = "Full-text search via GitHub's /search/issues endpoint, scoped automatically to repo:{owner}/{repo} is:issue. Bypasses the cache — every call hits GitHub fresh. Params: query (required, non-empty), limit (default 20, clamped to GitHub's 100/page max). Returns IssueSummary-shaped rows; Projects V2 fields fall back to defaults because /search/issues does not expose them."
    )]
    pub async fn search(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<Json<SearchResult>, ErrorData> {
        let state = self.state();
        let kind = state.agent_kind_str();

        info!(agent.kind = %kind, "Search tool invoked");

        let result = crate::tools::search::handle_search(state, params).await?;
        Ok(Json(result))
    }

    /// Aggregate counts and metrics across the open issue set.
    ///
    /// Per SPEC §7.4. Returns totals by status and priority, blocked vs
    /// ready counts, cycle count, and per-agent throughput. Optional
    /// `milestone` filter scopes every aggregate except `cycle_count`
    /// (which always reflects the full graph — see R5 decision).
    ///
    /// This is a read tool — the handler consults the cache (triggering a
    /// lazy rebuild if stale) and never invalidates.
    #[tool(
        name = "stats",
        description = "Aggregate counts and metrics across open issues: total, by_status (ready/in_progress/blocked/deferred/closed), by_priority (P0–P4), blocked_count, ready_count, cycle_count, and per-agent throughput (agents[]). Optional milestone filter (exact title match) scopes every aggregate except cycle_count. Uses cache; rebuilds lazily if stale."
    )]
    pub async fn stats(
        &self,
        Parameters(params): Parameters<StatsParams>,
    ) -> Result<Json<StatsResult>, ErrorData> {
        let state = self.state();
        let kind = state.agent_kind_str();

        info!(agent.kind = %kind, "Stats tool invoked");

        let result = crate::tools::stats::handle_stats(state, params).await?;
        Ok(Json(result))
    }

    /// Reopen a closed issue and re-evaluate its blocking status.
    ///
    /// Per SPEC §8.7. Validates the issue is currently Closed, reopens it
    /// via REST `PATCH state: "open"`, rebuilds the graph, and sets the
    /// Projects V2 Status to `blocked` (when open blockers remain) or
    /// `ready` otherwise.
    ///
    /// This is a write tool — the cache is invalidated and rebuilt.
    #[tool(
        name = "reopen",
        description = "Reopen a closed issue. Validates IssueState == Closed, PATCHes state=open, rebuilds the graph, and sets Projects V2 Status to 'blocked' if the reopened issue still has open blockers or 'ready' otherwise. Triggers graph rebuild. Params: id (positive integer)."
    )]
    pub async fn reopen(
        &self,
        Parameters(params): Parameters<ReopenParams>,
    ) -> Result<Json<ReopenResult>, ErrorData> {
        let state = self.state();
        let kind = state.agent_kind_str();

        info!(agent.kind = %kind, "Reopen tool invoked");

        let result = crate::tools::reopen::handle_reopen(state, params).await?;
        Ok(Json(result))
    }

    /// Remove a blocking dependency between two issues.
    ///
    /// Per SPEC §8.5. Validates both refs, confirms the edge exists in
    /// the graph, calls `remove_blocked_by`, and (when source now has
    /// zero open blockers) sets the source's Projects V2 Status to
    /// `ready`. Both refs accept local (`42` / `#42`) or cross-repo
    /// (`owner/repo#42`) form.
    ///
    /// This is a write tool — the cache is invalidated and rebuilt.
    #[tool(
        name = "dep_remove",
        description = "Remove a blocking dependency: target no longer blocks source. Validates both IssueRefs (local 42/#42 or cross-repo owner/repo#42), confirms the edge exists, and flips source Status to 'ready' when it has zero open blockers after removal. Triggers graph rebuild. Cross-repo supported per SPEC §5.6."
    )]
    pub async fn dep_remove(
        &self,
        Parameters(params): Parameters<DepRemoveParams>,
    ) -> Result<Json<DepRemoveResult>, ErrorData> {
        let state = self.state();
        let kind = state.agent_kind_str();

        info!(agent.kind = %kind, "Dep_remove tool invoked");

        let result = crate::tools::dep_remove::handle_dep_remove(state, params).await?;
        Ok(Json(result))
    }

    /// Detect dependency cycles in the graph.
    ///
    /// Per SPEC §7.7. With no `id`, runs `detect_all_cycles()` over the
    /// full graph (Tarjan SCC). With `id` present, returns only cycles
    /// that contain `(configured_owner, configured_repo, id)`. Projects
    /// `Vec<Vec<QualifiedId>>` to `Vec<Vec<u64>>` by dropping cross-repo
    /// members; dropped `QualifiedId`s surface in `cross_repo_refs` per
    /// SPEC §11.4.
    ///
    /// This is a read tool — the handler consults the cache (triggering a
    /// lazy rebuild if stale) and never invalidates.
    #[tool(
        name = "dep_cycles",
        description = "Detect dependency cycles via Tarjan SCC. Optional id (local issue number) scopes the result to cycles containing that node; omit id for the full graph. Returns cycles: Vec<Vec<u64>> (local-projection), count, and cross_repo_refs per SPEC §11.4 when a cycle traverses cross-repo nodes (those QualifiedIds are omitted from the bare-u64 projection). Uses cache; rebuilds lazily if stale."
    )]
    pub async fn dep_cycles(
        &self,
        Parameters(params): Parameters<DepCyclesParams>,
    ) -> Result<Json<DepCyclesResult>, ErrorData> {
        let state = self.state();
        let kind = state.agent_kind_str();

        info!(agent.kind = %kind, "Dep_cycles tool invoked");

        let result = crate::tools::dep_cycles::handle_dep_cycles(state, params).await?;
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
    use std::sync::{Mutex, OnceLock};
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
    /// to retrieve a snapshot of the captured output as a `String` with
    /// UTF-8 validated once at construction time.
    #[derive(Clone)]
    struct TracingCapture(Arc<Mutex<Vec<u8>>>);

    /// Owned snapshot of the captured tracing output, validated as UTF-8
    /// once at construction time.
    ///
    /// Returned by [`TracingCapture::output`]. UTF-8 validation happens
    /// exactly once when the snapshot is created; subsequent [`Deref`]
    /// calls are zero-cost borrows. The mutex is released immediately
    /// after the snapshot is taken.
    struct CapturedOutput(String);

    impl std::ops::Deref for CapturedOutput {
        type Target = str;

        fn deref(&self) -> &str {
            &self.0
        }
    }

    impl std::fmt::Display for CapturedOutput {
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

        /// Return a snapshot of the captured output as a validated UTF-8
        /// string.
        ///
        /// UTF-8 validation is performed exactly once when the snapshot
        /// is created. The mutex is released immediately after copying
        /// the buffer, so callers can inspect the output without holding
        /// the lock.
        ///
        /// # Panics
        ///
        /// Panics if the mutex is poisoned or the buffer is not valid
        /// UTF-8.
        fn output(&self) -> CapturedOutput {
            let bytes = self.0.lock().unwrap().clone();
            CapturedOutput(String::from_utf8(bytes).expect("captured output is not valid UTF-8"))
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

        let client = unblock_github::client::GitHubClient::new(&config)
            .await
            .expect("test client should initialize");

        ServerState {
            config: Arc::new(config),
            github: Arc::new(client) as Arc<dyn GitHubApi>,
            cache: Arc::new(GraphCache::new(Duration::from_secs(300))),
            agent_kind: OnceLock::new(),
            agent_client: OnceLock::new(),
            connected_at: OnceLock::new(),
        }
    }

    /// The manual [`Debug`] impl for [`ServerState`] must:
    ///
    /// 1. Never leak the raw `Config.token` (a GitHub PAT) into the
    ///    formatted output, replacing it with a `[REDACTED]` marker via
    ///    the local `RedactedConfig` wrapper.
    /// 2. Render the `github` trait-object field with a meaningful
    ///    concrete-type label sourced from
    ///    [`GitHubApi::debug_label`](unblock_github::api::GitHubApi::debug_label),
    ///    rather than the historical `<dyn GitHubApi>` placeholder.
    #[tokio::test]
    async fn debug_redacts_token_and_labels_github_concrete_type() {
        const SECRET: &str = "ghp_test_token_for_unit_tests";

        let state = test_state().await;

        // Sanity-check the precondition: the test fixture really did load
        // the secret value into Config.token. Without this guard, a future
        // refactor of `test_state` could silently turn the redaction
        // assertion into a no-op.
        assert_eq!(
            state.config.token, SECRET,
            "test fixture must keep using the documented test token"
        );

        let rendered = format!("{state:?}");

        // Invariant 1: token must be absent from the Debug output.
        assert!(
            !rendered.contains(SECRET),
            "ServerState Debug output leaked the GitHub PAT: {rendered}"
        );
        assert!(
            rendered.contains("[REDACTED]"),
            "ServerState Debug output should mark the token as [REDACTED]: {rendered}"
        );

        // Invariant 2: github field must surface a meaningful concrete label,
        // not the legacy `<dyn GitHubApi>` placeholder. The default
        // `debug_label` impl forwards to `std::any::type_name::<Self>()`,
        // which on the production `GitHubClient` resolves to a path
        // containing `GitHubClient`.
        assert!(
            !rendered.contains("<dyn GitHubApi>"),
            "ServerState Debug output still uses the legacy placeholder: {rendered}"
        );
        assert!(
            rendered.contains("GitHubClient"),
            "ServerState Debug output should mention the concrete GitHubClient type: {rendered}"
        );
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
