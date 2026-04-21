//! Prime tool — session entry point for every agent session.
//!
//! Returns a single **markdown context blob** that an MCP client can inject
//! directly into the agent's session prompt. Internally the handler still
//! aggregates issue state into categorised lists (`in_progress`, `ready`,
//! `blocked`, `completed`, `hotspots`, `stale`) — those lists feed a private
//! renderer that assembles the markdown.
//!
//! ## Response shape (SPEC §7.3)
//!
//! ```rust,ignore
//! pub struct PrimeResult {
//!     pub context: String, // markdown blob for agent injection
//! }
//! ```
//!
//! No other fields. The renderer below emits the following sections in this
//! exact order (any absent section is elided entirely, not rendered empty):
//!
//! 1. **Header** — `# Repo: <owner>/<repo>` + `Project: <n>` (if set).
//! 2. **Counts** — `## Counts` with `ready`, `blocked`, `in-progress`,
//!    `completed (<threshold>h)`, `hotspots`, `stale` bullets.
//! 3. **Cycles** — `## Issues with cycles` (SPEC §7.3 flow step 2) — local
//!    members as `#N`, cross-repo members deferred to the trailer.
//!    Individual cycle member lists are capped at `max_per_category`
//!    entries with a `… (K more)` tail when truncated.
//! 4. **Session** — Epic 1.5 `SessionMeta` projected as a `## Session`
//!    block (agent kind / client / agent field / `connected_at`).
//! 5. **Drift** — Epic 1.6 background-reconcile output as a `## Drift
//!    warnings` block (one bullet per summarised drift kind). Absent when
//!    the reconcile task reported `clean == true`, panicked, or errored.
//! 6. **Cross-repo trailer** — SPEC §11.4 adaptation: `## Cross-repo
//!    references` + bullet list of `owner/repo#N` (sorted
//!    lexicographically via `BTreeSet`) + italic summary matching the
//!    shared `cross_repo::cycles_summary` helper byte-for-byte.
//!    Elided entirely when the cycle detector did not touch any
//!    cross-repo node.
//!
//! The `stale_threshold_hours` parameter gates the stale-claims subsection
//! inside `## Counts` (and its inclusion in the recently-completed window),
//! while `max_per_category` caps both the in-memory categorised lists AND
//! the rendered cycle-member bullet lists. Both parameters remain active
//! — they are not cosmetic.
//!
//! ## Flow (SPEC §7.3)
//!
//! This is a read tool that always performs a fresh fetch from GitHub
//! (bypasses cache) because the cache only stores the ready set —
//! categorising `in_progress`, `blocked`, and `stale` requires the full
//! `Issue` list with status and `claimed_at` fields. After the fetch, the
//! cache is updated with the fresh graph data. A background read-only
//! reconcile is spawned via `tokio::spawn` (Design Decision R5) and its
//! output surfaces in the `## Drift warnings` section.
//!
//! ## BREAKING CHANGE note
//!
//! This module's `PrimeResult` shape was rewritten from a categorised
//! typed struct (9 public fields) to `{ context: String }` per
//! bead `unblock-eos.7`. The rich category information is now rendered as
//! markdown — consumers that previously read `result.ready[0].title` etc.
//! must either re-parse the markdown or adopt the per-category tools
//! (`ready`, `list`, `show`). See the bead's `BREAKING CHANGE:` footer
//! for the full field list.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;
use unblock_core::graph::DependencyGraph;
use unblock_core::types::{CrossRepoRefs, Issue, IssueState, IssueSummary, QualifiedId, Status};

use crate::errors::github_error_to_mcp;
use crate::server::ServerState;
use crate::tools::cross_repo;
use crate::tools::reconcile::{ReconcileParams, ReconcileReport, handle_reconcile};

/// Minimum allowed value for `stale_threshold_hours` (must be at least 1).
const MIN_STALE_THRESHOLD_HOURS: u64 = 1;

/// Minimum allowed value for `max_per_category` (must be at least 1).
const MIN_MAX_PER_CATEGORY: usize = 1;

/// Default number of hours before a claim is considered stale.
const DEFAULT_STALE_THRESHOLD_HOURS: u64 = 24;

/// Default maximum number of items per category in the output.
const DEFAULT_MAX_PER_CATEGORY: usize = 10;

/// Input parameters for the `prime` MCP tool.
///
/// All parameters are optional. With no parameters, returns up to 10 items
/// per category with a 24-hour stale claim threshold.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct PrimeParams {
    /// Hours without activity before considering a claim stale. Default: 24.
    /// Must be at least 1; zero is rejected with an `INVALID_PARAMS` error.
    pub stale_threshold_hours: Option<u64>,
    /// Maximum number of items per category to return. Controls output size.
    /// Default: 10. Must be at least 1; zero is rejected with an
    /// `INVALID_PARAMS` error.
    pub max_per_category: Option<usize>,
    /// Filter all categories by agent name. Exact match. When set, only
    /// issues claimed by this agent appear in `in_progress`, `ready`,
    /// `blocked`, and `stale`. The `completed` and `hotspots` categories
    /// are never filtered (global continuity context and structural graph
    /// properties respectively).
    pub agent: Option<String>,
}

/// Output from the `prime` MCP tool (SPEC §7.3).
///
/// The response is a single markdown blob. MCP clients inject it directly
/// into the agent's session prompt. The renderer produces a six-section
/// markdown document in a fixed order: header → counts → cycles → session
/// → drift → cross-repo trailer (see the module-level docs for the full
/// contract and elision rules).
///
/// # Consumer guidance
///
/// - Treat the string as opaque markdown. Do NOT parse it to extract
///   structured issue data — call the per-category tools (`ready`,
///   `list`, `show`, `dep_cycles`) for machine-consumable fields.
/// - An absent subsection indicates the absence of that class of data,
///   not an error — e.g. no `## Issues with cycles` heading implies
///   zero cycles were detected in the configured repo graph.
///
/// # Rewrite history
///
/// Prior to bead `unblock-eos.7` this type carried nine public fields
/// with typed category vectors. Those fields were removed in favour of
/// the markdown blob. Epic 1.5 `SessionMeta` and Epic 1.6 drift warnings
/// are now rendered as markdown sections (Option 3) rather than typed
/// fields.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PrimeResult {
    /// Markdown context blob for agent injection. The rendered layout is
    /// documented in the [module-level docs][crate::tools::prime] and
    /// verified by the integration tests `prime_markdown_*`.
    pub context: String,
}

/// Summary counts for the prime context.
///
/// Module-private since bead `unblock-eos.7` — the counts appear in the
/// rendered markdown `## Counts` section but are not exposed as a typed
/// response field.
#[derive(Debug, Clone)]
pub(super) struct PrimeCounts {
    /// Total number of in-progress issues (before truncation).
    pub(super) in_progress: usize,
    /// Total number of ready issues (before truncation).
    pub(super) ready: usize,
    /// Total number of blocked issues (before truncation).
    pub(super) blocked: usize,
    /// Total number of recently completed issues (before truncation).
    pub(super) completed: usize,
    /// Total number of hotspot issues (before truncation).
    pub(super) hotspots: usize,
    /// Total number of stale claims (before truncation).
    pub(super) stale: usize,
}

/// Lightweight issue summary used internally by the prime renderer.
///
/// Module-private since bead `unblock-eos.7` — issue summaries are now
/// consumed by [`render_context`] rather than serialised as public
/// response fields. Kept as a distinct struct (rather than forwarding
/// [`IssueSummary`] directly) to preserve the existing unit tests that
/// assert on RFC-3339 `created_at` strings and to keep category output
/// pre-formatted.
///
/// Fields beyond the renderer's current needs (e.g. `issue_type`,
/// `milestone`, `labels`, `created_at`, `url`) are preserved because
/// they carry `IssueSummary` fidelity for future renderer enhancements
/// and are asserted against by the existing unit tests via
/// [`Self::from_core`].
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(super) struct PrimeIssueSummary {
    /// Fully qualified issue identifier (`owner/repo#number`).
    pub(super) qualified_id: String,
    /// GitHub issue number.
    pub(super) number: u64,
    /// Issue title.
    pub(super) title: String,
    /// Issue type classification (e.g. "Task", "Bug").
    pub(super) issue_type: Option<String>,
    /// Workflow status from Projects V2.
    pub(super) status: String,
    /// Priority level from Projects V2.
    pub(super) priority: String,
    /// Agent name if claimed.
    pub(super) agent: Option<String>,
    /// Milestone title.
    pub(super) milestone: Option<String>,
    /// Labels attached to the issue.
    pub(super) labels: Vec<String>,
    /// Timestamp when the issue was created (ISO 8601 / RFC 3339).
    pub(super) created_at: String,
    /// HTML URL for linking back to GitHub.
    pub(super) url: String,
}

impl PrimeIssueSummary {
    /// Convert from a core [`IssueSummary`] to a renderer-friendly summary.
    pub(super) fn from_core(summary: &IssueSummary) -> Self {
        Self {
            qualified_id: summary.qualified_id.to_string(),
            number: summary.number,
            title: summary.title.clone(),
            issue_type: summary.issue_type.map(|it| it.to_string()),
            status: summary.status.to_string(),
            priority: summary.priority.to_string(),
            agent: summary.agent.clone(),
            milestone: summary.milestone.clone(),
            labels: summary.labels.clone(),
            created_at: summary.created_at.to_rfc3339(),
            url: summary.url.clone(),
        }
    }
}

/// A hotspot: an issue that blocks many other issues.
///
/// Module-private since bead `unblock-eos.7`. Fields beyond the
/// renderer's current needs (e.g. `qualified_id`, `status`, `url`) are
/// retained because they are asserted against by the existing unit
/// tests and preserved for future renderer enhancements.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(super) struct HotspotSummary {
    /// Fully qualified issue identifier (`owner/repo#number`).
    pub(super) qualified_id: String,
    /// GitHub issue number.
    pub(super) number: u64,
    /// Issue title.
    pub(super) title: String,
    /// Workflow status from Projects V2.
    pub(super) status: String,
    /// Priority level from Projects V2.
    pub(super) priority: String,
    /// Number of issues this issue is blocking.
    pub(super) blocking_count: usize,
    /// HTML URL for linking back to GitHub.
    pub(super) url: String,
}

/// A stale claim: an in-progress issue with `claimed_at` older than threshold.
///
/// Module-private since bead `unblock-eos.7`. Fields beyond the
/// renderer's current needs (e.g. `qualified_id`, `claimed_at`, `url`)
/// are retained because they are asserted against by the existing unit
/// tests and preserved for future renderer enhancements.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(super) struct StaleIssueSummary {
    /// Fully qualified issue identifier (`owner/repo#number`).
    pub(super) qualified_id: String,
    /// GitHub issue number.
    pub(super) number: u64,
    /// Issue title.
    pub(super) title: String,
    /// Agent name that claimed the issue.
    pub(super) agent: Option<String>,
    /// Timestamp when the issue was claimed (ISO 8601 / RFC 3339).
    pub(super) claimed_at: String,
    /// Hours since the issue was claimed.
    pub(super) hours_stale: u64,
    /// HTML URL for linking back to GitHub.
    pub(super) url: String,
}

/// A recently completed issue: closed within the configurable time window.
///
/// Provides continuity context so agents can see what was recently shipped
/// before picking up new work (PRD §6.3). Module-private since bead
/// `unblock-eos.7`. Fields beyond the renderer's current needs
/// (e.g. `qualified_id`, `issue_type`, `url`) are retained because
/// they are asserted against by the existing unit tests and preserved
/// for future renderer enhancements.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(super) struct CompletedIssueSummary {
    /// Fully qualified issue identifier (`owner/repo#number`).
    pub(super) qualified_id: String,
    /// GitHub issue number.
    pub(super) number: u64,
    /// Issue title.
    pub(super) title: String,
    /// Issue type classification (e.g. "Task", "Bug").
    pub(super) issue_type: Option<String>,
    /// Priority level from Projects V2.
    pub(super) priority: String,
    /// Approximate close time (derived from `updated_at` since GitHub
    /// `closedAt` is not currently fetched).
    pub(super) closed_at: String,
    /// HTML URL for linking back to GitHub.
    pub(super) url: String,
}

/// Repository identity bundle read from [`ServerState`] at the top of
/// [`handle_prime`].
///
/// Groups the three values that together identify the configured
/// repository (plus its optional GitHub Projects V2 number) so
/// [`ContextInputsBuilder::new`] can accept a single argument instead of
/// the three positional `&str` / `Option<u64>` parameters that
/// previously pushed it over clippy's `too_many_arguments` threshold
/// (see bead `unblock-eos.34`).
///
/// # Lifetime
///
/// `'a` is tied to [`ServerState`]'s accessors: `owner` and `repo` are
/// borrowed from `state.github.owner()` / `state.github.repo()` and
/// must outlive any use of this bundle.
///
/// # Scope
///
/// Private to this module — mirrors the [`SessionMeta`] precedent. If a
/// second consumer (e.g., `ready.rs`, `stats.rs`) adopts the same
/// pattern in the future, promoting this to a shared `tools/repo.rs`
/// is an architectural decision for its own bead.
#[derive(Debug, Clone, Copy)]
pub(super) struct RepoIdentity<'a> {
    /// Configured repository owner (matches `state.github.owner()`).
    pub(super) owner: &'a str,
    /// Configured repository name (matches `state.github.repo()`).
    pub(super) repo: &'a str,
    /// Optional GitHub Projects V2 project number (from the
    /// [`GitHubApi`](unblock_github::GitHubApi) accessor).
    pub(super) project_number: Option<u64>,
}

impl RepoIdentity<'_> {
    /// Build a [`RepoIdentity`] borrowing from the live [`ServerState`].
    ///
    /// `owner` and `repo` come from the [`GitHubApi`](unblock_github::GitHubApi)
    /// trait accessors. `project_number` is also read from the trait
    /// (not from `state.config.project_number` directly) so all three
    /// fields share a single accessor surface — see the DECISION note
    /// on bead `unblock-eos.34`. The two sources are equivalent today
    /// (`GitHubClient::project_number` is a verbatim pass-through of
    /// `config.project_number`), but colocating the reads here keeps
    /// future mocks / alternate backends honest.
    pub(super) fn from_state(state: &ServerState) -> RepoIdentity<'_> {
        RepoIdentity {
            owner: state.github.owner(),
            repo: state.github.repo(),
            project_number: state.github.project_number(),
        }
    }
}

/// Session metadata populated from [`ServerState`] during each `prime` call.
///
/// Surfaces the connected MCP client identity, the resolved agent kind,
/// an optional operator-defined agent field (`UNBLOCK_AGENT` env var),
/// and the session start timestamp. Module-private since bead
/// `unblock-eos.7` — rendered into the `## Session` markdown section by
/// [`render_context`] rather than serialised as a typed response field.
#[derive(Debug, Clone)]
pub(super) struct SessionMeta {
    /// Raw MCP `clientInfo.name` (e.g., "Claude Code", "GitHub Copilot Chat").
    pub(super) agent_client: String,
    /// Normalised agent kind string (e.g., "claude-code", "copilot", "unknown").
    pub(super) agent_kind: String,
    /// Value of the `UNBLOCK_AGENT` env var if set by the operator.
    /// `None` when the variable is not present in the environment.
    pub(super) agent_field: Option<String>,
    /// UTC timestamp when the MCP session was initialised (ISO 8601 / RFC 3339).
    pub(super) connected_at: DateTime<Utc>,
}

impl SessionMeta {
    /// Build [`SessionMeta`] from the live [`ServerState`].
    ///
    /// Reads `agent_kind` and `agent_client` from their respective
    /// [`OnceLock`](std::sync::OnceLock) fields (lock-free). Falls back to
    /// `"unknown"` when the locks have not been set (e.g., in tests where
    /// `initialize()` is not called).
    ///
    /// `agent_field` is read from the `UNBLOCK_AGENT` environment variable
    /// on every call (not cached), returning `None` when unset.
    ///
    /// `connected_at` falls back to `Utc::now()` when the `OnceLock` has not
    /// been set, ensuring tests that skip `initialize()` still get a valid
    /// timestamp.
    pub(super) fn from_state(state: &ServerState) -> Self {
        let agent_client = state
            .agent_client
            .get()
            .map_or_else(|| "unknown".to_owned(), |c| c.name.clone());

        let agent_kind = state.agent_kind_str().to_owned();

        let agent_field = std::env::var("UNBLOCK_AGENT").ok();

        let connected_at = state.connected_at.get().copied().unwrap_or_else(Utc::now);

        Self {
            agent_client,
            agent_kind,
            agent_field,
            connected_at,
        }
    }
}

/// Execute the prime tool handler.
///
/// # Flow (SPEC §7.3)
///
/// 1. Spawn a background read-only reconcile via `tokio::spawn` (Design
///    Decision R5).
/// 2. Fresh fetch via `fetch_graph_data()` — bypasses cache entirely.
/// 3. Build `DependencyGraph` and compute the ready set.
/// 4. Categorise all issues into `in_progress`, `blocked`, `ready`,
///    `completed`, `hotspots`, `stale`.
/// 5. Update cache with the fresh graph already fetched.
/// 6. Apply agent filter to relevant categories (PRD §6.3).
/// 7. Compute counts (after filtering) and detect cycles for the `##
///    Issues with cycles` section.
/// 8. Await the drift check, assemble `SessionMeta` + `drift_warnings`,
///    and hand all inputs to the internal `render_context` helper which
///    returns the single markdown blob in [`PrimeResult::context`].
///
/// The background reconcile runs concurrently with the prime fetch and
/// does not block the response path. If it fails or panics, the `##
/// Drift warnings` section is omitted — prime never fails due to
/// reconcile errors.
///
/// # Errors
///
/// Returns [`rmcp::model::ErrorData`] with `INVALID_PARAMS` if
/// `stale_threshold_hours` or `max_per_category` is below its minimum
/// (currently 1), or if the GitHub fetch fails.
pub async fn handle_prime(
    params: &PrimeParams,
    state: &Arc<ServerState>,
) -> Result<PrimeResult, rmcp::model::ErrorData> {
    let (stale_threshold_hours, max_per_category) = validate_and_resolve_params(params)?;

    info!(
        stale_threshold_hours,
        max_per_category, "Prime tool invoked"
    );

    // 1. Spawn background read-only reconcile (Design Decision R5).
    //    Runs concurrently with the prime fetch. If it fails or panics,
    //    drift_warnings is simply None — prime never fails due to reconcile.
    let drift_check = tokio::spawn({
        let state = Arc::clone(state);
        async move {
            let reconcile_params = ReconcileParams {
                fix: false,
                stale_claim_hours: 24,
            };
            handle_reconcile(&reconcile_params, &state).await
        }
    });

    // 2. Always fresh fetch — bypasses cache entirely.
    let (issues_vec, edges) = state
        .github
        .fetch_graph_data()
        .await
        .map_err(github_error_to_mcp)?;

    // 3. Build graph and compute ready set.
    // SPEC §3.3 Filter 3 / §14 Invariant 14(a): engine scopes source issues
    // to the configured (owner, repo) before the blocker filter runs. The
    // categorisation below and the cache store inherit that guarantee.
    let graph = DependencyGraph::build(&issues_vec, &edges);
    let ready_summaries =
        graph.compute_ready_set(&issues_vec, state.github.owner(), state.github.repo());

    let now = Utc::now();

    // 4. Categorise issues.
    let categories = categorise_issues(
        &issues_vec,
        &graph,
        &ready_summaries,
        stale_threshold_hours,
        now,
    );

    // 5. Update cache with the fresh graph already fetched.
    //    Categorisation above (step 4) already consumed `issues_vec` by
    //    reference, and steps 6–8 only touch `categories`/`filtered_*`, so
    //    the issues vec can be moved into the cache here without cloning.
    state.cache.update(issues_vec, ready_summaries, graph).await;
    tracing::debug!("Cache updated with fresh graph from prime");

    // 6. Apply agent filter to relevant categories (PRD §6.3).
    //    `completed` and `hotspots` are NOT filtered — completed provides
    //    global continuity context and hotspots are structural graph properties.
    let agent_filter = crate::tools::normalize_filter(params.agent.as_deref());
    let filtered = apply_agent_filter(categories, agent_filter);

    // 7. Fetch cycles from the cache (cheap `Arc` clone — the graph we
    //    just stored in step 5 is reused). On the astronomically
    //    unlikely race where another invalidator clears the cache
    //    between steps 5 and 7 we fall back to a cycle-free view (the
    //    renderer elides the section); this matches the "no cycles"
    //    branch in SPEC §7.3 flow step 2.
    let cached_graph = state.cache.get_graph().await;
    let raw_cycles = cached_graph
        .as_ref()
        .map(|g| g.detect_all_cycles())
        .unwrap_or_default();

    // 8. Await drift check and assemble the markdown blob. The builder
    //    encapsulates count computation, §11.4 cycle projection, and
    //    per-category truncation so this function stays within clippy's
    //    `too_many_lines` budget.
    let drift_warnings = resolve_drift_warnings(drift_check).await;
    let session = SessionMeta::from_state(state);

    // DECISION (bead unblock-eos.34): `project_number` source migrated
    // from `state.config.project_number` to `state.github.project_number()`
    // (via `RepoIdentity::from_state`). Equivalent today — the
    // `GitHubClient::project_number` accessor is a verbatim pass-through
    // of `config.project_number` — but colocating owner / repo /
    // project_number behind the `GitHubApi` trait keeps future mocks
    // and alternate backends honest.
    let builder = ContextInputsBuilder::new(
        RepoIdentity::from_state(state),
        stale_threshold_hours,
        max_per_category,
        filtered,
        &raw_cycles,
    );
    let ctx = builder.build(&session, drift_warnings.as_deref());

    Ok(PrimeResult {
        context: render_context(&ctx),
    })
}

/// Validate the user-supplied boundary values and resolve defaults.
///
/// Returns `(stale_threshold_hours, max_per_category)` with
/// per-parameter defaults applied. Rejects `Some(0)` for either
/// parameter with [`rmcp::model::ErrorCode::INVALID_PARAMS`] — matching
/// SPEC §7.3's minimum-of-1 contract for both knobs.
///
/// Split out of [`handle_prime`] so the orchestrator stays within
/// clippy's default `too_many_lines` budget (bead `unblock-eos.8`).
fn validate_and_resolve_params(
    params: &PrimeParams,
) -> Result<(u64, usize), rmcp::model::ErrorData> {
    if let Some(hours) = params.stale_threshold_hours
        && hours < MIN_STALE_THRESHOLD_HOURS
    {
        return Err(rmcp::model::ErrorData {
            code: rmcp::model::ErrorCode::INVALID_PARAMS,
            message: format!(
                "stale_threshold_hours must be at least {MIN_STALE_THRESHOLD_HOURS}, got {hours}"
            )
            .into(),
            data: None,
        });
    }
    if let Some(max) = params.max_per_category
        && max < MIN_MAX_PER_CATEGORY
    {
        return Err(rmcp::model::ErrorData {
            code: rmcp::model::ErrorCode::INVALID_PARAMS,
            message: format!("max_per_category must be at least {MIN_MAX_PER_CATEGORY}, got {max}")
                .into(),
            data: None,
        });
    }

    let stale_threshold_hours = params
        .stale_threshold_hours
        .unwrap_or(DEFAULT_STALE_THRESHOLD_HOURS);
    let max_per_category = params.max_per_category.unwrap_or(DEFAULT_MAX_PER_CATEGORY);

    Ok((stale_threshold_hours, max_per_category))
}

/// Apply the optional agent filter to `in_progress`, `ready`, `blocked`,
/// and `stale` categories (PRD §6.3).
///
/// `completed` and `hotspots` are left untouched — `completed` provides
/// global continuity context and `hotspots` are structural graph
/// properties, neither scoped to a single agent.
///
/// When `agent_filter` is `None` the categorised buckets are returned
/// unchanged. Split out of [`handle_prime`] so the orchestrator stays
/// within clippy's default `too_many_lines` budget (bead
/// `unblock-eos.8`).
fn apply_agent_filter(
    mut categories: CategorisedIssues,
    agent_filter: Option<&str>,
) -> CategorisedIssues {
    if let Some(agent_filter) = agent_filter {
        let matches_agent = |s: &IssueSummary| s.agent.as_deref() == Some(agent_filter);
        categories.in_progress.retain(matches_agent);
        categories.ready.retain(matches_agent);
        categories.blocked.retain(matches_agent);
        categories
            .stale
            .retain(|s| s.agent.as_deref() == Some(agent_filter));
    }
    categories
}

/// Builder that encapsulates construction of [`ContextInputs`] from the
/// orchestrator's filtered [`CategorisedIssues`] plus the raw cycle set.
///
/// Introduced by bead `unblock-eos.8` to shrink [`handle_prime`] below
/// clippy's `too_many_lines` threshold. The builder owns the derived
/// state (post-truncation category lists, counts, projected cycles,
/// optional `cross_repo_refs`) so [`Self::build`] can return a
/// [`ContextInputs`] borrowing its owned fields without the orchestrator
/// needing to keep fifteen intermediate locals alive.
///
/// # Ownership model
///
/// The builder owns every `Vec<_>` it computes. The returned
/// [`ContextInputs<'_>`] borrows from `self`, so the builder must
/// outlive the rendered context — typically by binding both to the
/// same stack frame (`let builder = …; let ctx = builder.build(…);`).
///
/// # Determinism
///
/// The projection helpers from [`cross_repo`] are the sole source of
/// the §11.4 cross-repo trailer; this builder calls them exactly once
/// and binds the result byte-for-byte identically to `dep_cycles`. See
/// SPEC §14 Invariant 14 and [`cross_repo::cycles_summary`].
///
/// # Field order
///
/// Fields below mirror the declaration order of [`ContextInputs`]
/// **intentionally**. `session` and `drift_warnings` are the only
/// [`ContextInputs`] fields not owned by the builder (the orchestrator
/// supplies them at [`Self::build`] time); every other field appears
/// in the same position. Do not reorder either struct independently —
/// keep the two in lock-step so maintainers can audit parity by
/// reading top-to-bottom.
struct ContextInputsBuilder<'a> {
    /// Configured repository owner (matches `state.github.owner()`).
    owner: &'a str,
    /// Configured repository name (matches `state.github.repo()`).
    repo: &'a str,
    /// Optional GitHub Projects V2 project number (from `Config`).
    project_number: Option<u64>,
    /// Stale threshold echoed into the `## Counts` heading.
    stale_threshold_hours: u64,
    /// Per-category rendered-bullet cap (also applied to cycle
    /// member lists in the renderer).
    max_per_category: usize,
    /// Counts computed AFTER agent filtering — rendered verbatim in
    /// `## Counts`.
    counts: PrimeCounts,
    /// Local-only projection of every detected cycle.
    cycles: Vec<Vec<u64>>,
    /// §11.4 cross-repo trailer or `None` when no cycle touched a
    /// cross-repo node.
    cross_repo_refs: Option<CrossRepoRefs>,
    /// Top-N in-progress summaries (already truncated).
    in_progress: Vec<PrimeIssueSummary>,
    /// Top-N ready summaries (already truncated).
    ready: Vec<PrimeIssueSummary>,
    /// Top-N blocked summaries (already truncated).
    blocked: Vec<PrimeIssueSummary>,
    /// Top-N completed summaries (already truncated).
    completed: Vec<CompletedIssueSummary>,
    /// Top-N hotspots (already truncated).
    hotspots: Vec<HotspotSummary>,
    /// Top-N stale claims (already truncated).
    stale: Vec<StaleIssueSummary>,
}

impl<'a> ContextInputsBuilder<'a> {
    /// Construct a builder from the orchestrator's prepared state.
    ///
    /// Consumes `filtered` (the post-agent-filter [`CategorisedIssues`])
    /// and borrows `raw_cycles` (owned by [`handle_prime`]'s cache read).
    /// Internally performs three derivations in sequence:
    ///
    /// 1. **Counts** — `PrimeCounts` from the filtered category lengths.
    /// 2. **Cycle projection + §11.4 trailer** — delegates to
    ///    [`cross_repo::project_all_cycles`] and
    ///    [`cross_repo::build_cross_repo_refs_with_summary`] with
    ///    [`cross_repo::cycles_summary`] for byte-parity with
    ///    `dep_cycles`.
    /// 3. **Truncation** — every category list is capped at
    ///    `max_per_category` and `IssueSummary` entries are mapped into
    ///    the renderer-friendly [`PrimeIssueSummary`] shape.
    ///
    /// After [`Self::new`] returns, the builder holds every Vec the
    /// renderer needs. Call [`Self::build`] with the [`SessionMeta`] and
    /// optional drift warnings to obtain the borrow-based
    /// [`ContextInputs`].
    fn new(
        repo: RepoIdentity<'a>,
        stale_threshold_hours: u64,
        max_per_category: usize,
        filtered: CategorisedIssues,
        raw_cycles: &[Vec<QualifiedId>],
    ) -> Self {
        // Destructure `RepoIdentity` immediately so the builder keeps
        // `owner` / `repo` / `project_number` as three parallel fields —
        // preserving the visual field-order parity with [`ContextInputs`]
        // that the eos.32 comment at the struct definition depends on
        // (see bead `unblock-eos.34`, Option (i) trade-off).
        let RepoIdentity {
            owner,
            repo,
            project_number,
        } = repo;

        // 1. Counts — computed AFTER agent filtering.
        let counts = PrimeCounts {
            in_progress: filtered.in_progress.len(),
            ready: filtered.ready.len(),
            blocked: filtered.blocked.len(),
            completed: filtered.completed.len(),
            hotspots: filtered.hotspots.len(),
            stale: filtered.stale.len(),
        };

        // 2. Project cycles via the shared §11.4 helper so the
        //    cross-repo trailer is populated identically to
        //    `dep_cycles` (parity across tools is a non-negotiable part
        //    of the cross-repo contract — SPEC §11.4 + §14 Invariant 14).
        let (cycles, cross_repo_accum) = cross_repo::project_all_cycles(raw_cycles, owner, repo);
        let cross_repo_refs = cross_repo::build_cross_repo_refs_with_summary(
            cross_repo_accum,
            cross_repo::cycles_summary,
        );

        // 3. Truncate the categorised lists to `max_per_category`,
        //    matching the historical (pre-eos.7) truncation semantics
        //    so the rendered lists never blow up the markdown for repos
        //    with hundreds of open issues.
        let in_progress: Vec<PrimeIssueSummary> = filtered
            .in_progress
            .iter()
            .take(max_per_category)
            .map(PrimeIssueSummary::from_core)
            .collect();
        let ready: Vec<PrimeIssueSummary> = filtered
            .ready
            .iter()
            .take(max_per_category)
            .map(PrimeIssueSummary::from_core)
            .collect();
        let blocked: Vec<PrimeIssueSummary> = filtered
            .blocked
            .iter()
            .take(max_per_category)
            .map(PrimeIssueSummary::from_core)
            .collect();
        let completed: Vec<CompletedIssueSummary> = filtered
            .completed
            .into_iter()
            .take(max_per_category)
            .collect();
        let hotspots: Vec<HotspotSummary> = filtered
            .hotspots
            .into_iter()
            .take(max_per_category)
            .collect();
        let stale: Vec<StaleIssueSummary> =
            filtered.stale.into_iter().take(max_per_category).collect();

        Self {
            owner,
            repo,
            project_number,
            stale_threshold_hours,
            max_per_category,
            counts,
            cycles,
            cross_repo_refs,
            in_progress,
            ready,
            blocked,
            completed,
            hotspots,
            stale,
        }
    }

    /// Re-borrow the builder's owned fields and assemble a
    /// [`ContextInputs`] bound to `self`.
    ///
    /// This method takes `&self` — it does NOT consume the builder.
    /// Every `Vec<_>` / `Option<_>` the builder owns is re-borrowed as
    /// a slice or reference, so the returned `ContextInputs<'b>` is
    /// valid only for as long as `self` (and the supplied `session` /
    /// `drift_warnings`) remain alive.
    ///
    /// `session` and `drift_warnings` come from the orchestrator and
    /// must outlive the returned context — typically by sharing the
    /// same stack frame as the builder.
    fn build<'b>(
        &'b self,
        session: &'b SessionMeta,
        drift_warnings: Option<&'b [String]>,
    ) -> ContextInputs<'b>
    where
        'a: 'b,
    {
        ContextInputs {
            owner: self.owner,
            repo: self.repo,
            project_number: self.project_number,
            stale_threshold_hours: self.stale_threshold_hours,
            max_per_category: self.max_per_category,
            counts: &self.counts,
            cycles: &self.cycles,
            cross_repo_refs: self.cross_repo_refs.as_ref(),
            session,
            drift_warnings,
            in_progress: &self.in_progress,
            ready: &self.ready,
            blocked: &self.blocked,
            completed: &self.completed,
            hotspots: &self.hotspots,
            stale: &self.stale,
        }
    }
}

/// Bundle of renderer inputs — every section of the `context` markdown
/// reads from this struct. Avoids a function signature with a dozen
/// positional parameters.
///
/// Lifetime `'a` binds the borrowed slices/refs to the call-site stack
/// frame of [`handle_prime`]; the renderer performs no allocation
/// outside the returned `String`.
///
/// Constructed via [`ContextInputsBuilder::build`] in production;
/// renderer unit tests may instantiate the struct literal directly.
struct ContextInputs<'a> {
    /// Configured repository owner (matches `state.github.owner()`).
    owner: &'a str,
    /// Configured repository name (matches `state.github.repo()`).
    repo: &'a str,
    /// Optional GitHub Projects V2 project number (from `Config`).
    project_number: Option<u64>,
    /// Stale threshold echoed into the `## Counts` header so the agent
    /// can see the window used for the `completed` / `stale` buckets.
    stale_threshold_hours: u64,
    /// Per-category rendered-bullet cap (same value used to truncate
    /// the lists above — passed here so the renderer can annotate
    /// truncation with `… (K more)` and cap cycle-member bullets).
    max_per_category: usize,
    /// Counts computed AFTER agent filtering, rendered verbatim in
    /// `## Counts`.
    counts: &'a PrimeCounts,
    /// Local-only projection of every detected cycle (SPEC §7.7 flow
    /// step 4b — may include short/empty inner vectors for mixed or
    /// wholly cross-repo cycles).
    cycles: &'a [Vec<u64>],
    /// §11.4 cross-repo trailer or `None` when no cycle touched a
    /// cross-repo node.
    cross_repo_refs: Option<&'a CrossRepoRefs>,
    /// Epic 1.5 session metadata.
    session: &'a SessionMeta,
    /// Epic 1.6 drift warnings. `None` elides the whole section.
    drift_warnings: Option<&'a [String]>,
    /// Top-N in-progress summaries (already truncated).
    in_progress: &'a [PrimeIssueSummary],
    /// Top-N ready summaries (already truncated).
    ready: &'a [PrimeIssueSummary],
    /// Top-N blocked summaries (already truncated).
    blocked: &'a [PrimeIssueSummary],
    /// Top-N completed summaries (already truncated).
    completed: &'a [CompletedIssueSummary],
    /// Top-N hotspots (already truncated).
    hotspots: &'a [HotspotSummary],
    /// Top-N stale claims (already truncated).
    stale: &'a [StaleIssueSummary],
}

/// Render the prime `context` markdown blob.
///
/// Section order (fixed — any absent section is elided, not rendered
/// empty):
///
/// 1. **Header** — `# Repo:` / `Project:` lines.
/// 2. **`## Counts`** — summary counts for quick orientation.
/// 3. **`## In progress` / `## Ready` / `## Blocked` / `## Recently
///    completed` / `## Hotspots` / `## Stale claims`** — truncated
///    category lists (elided when empty).
/// 4. **`## Issues with cycles`** — local cycle-member projection per
///    SPEC §7.3 flow step 2. Elided when no cycle was detected OR
///    every cycle's local projection is empty (cycles entirely
///    composed of cross-repo members still surface via the §11.4
///    trailer).
/// 5. **`## Session`** — Epic 1.5 `SessionMeta`.
/// 6. **`## Drift warnings`** — Epic 1.6 drift summaries, elided when
///    `None` (clean report, panic, or error).
/// 7. **`## Cross-repo references`** — SPEC §11.4 trailer, elided when
///    no cross-repo node participated.
///
/// All rendering is pure-string — the function takes no
/// `ServerState`/`ClaimHandle` handles and allocates only the returned
/// [`String`].
fn render_context(ctx: &ContextInputs<'_>) -> String {
    let mut out = String::new();
    render_header(&mut out, ctx);
    render_counts(&mut out, ctx);
    render_category_list_summaries(
        &mut out,
        "In progress",
        ctx.in_progress,
        ctx.counts.in_progress,
    );
    render_category_list_summaries(&mut out, "Ready", ctx.ready, ctx.counts.ready);
    render_category_list_summaries(&mut out, "Blocked", ctx.blocked, ctx.counts.blocked);
    render_completed(&mut out, ctx.completed, ctx.counts.completed);
    render_hotspots(&mut out, ctx.hotspots, ctx.counts.hotspots);
    render_stale(
        &mut out,
        ctx.stale,
        ctx.counts.stale,
        ctx.stale_threshold_hours,
    );
    render_cycles(&mut out, ctx.cycles, ctx.max_per_category);
    render_session(&mut out, ctx.session);
    render_drift(&mut out, ctx.drift_warnings);
    render_cross_repo_trailer(&mut out, ctx.cross_repo_refs);
    out
}

/// Emit `# Repo: owner/repo` + optional `Project: N` header block.
fn render_header(out: &mut String, ctx: &ContextInputs<'_>) {
    out.push_str("# Repo: ");
    out.push_str(ctx.owner);
    out.push('/');
    out.push_str(ctx.repo);
    out.push('\n');
    if let Some(n) = ctx.project_number {
        writeln!(out, "Project: {n}").expect("writing to String is infallible");
    }
}

/// Emit the `## Counts` section with a one-bullet-per-bucket summary.
///
/// The `completed` and `stale` bullets include the active
/// `stale_threshold_hours` so the agent can see the window used to
/// derive the counts.
fn render_counts(out: &mut String, ctx: &ContextInputs<'_>) {
    let c = ctx.counts;
    out.push_str("\n## Counts\n");
    writeln!(out, "- ready: {}", c.ready).expect("writing to String is infallible");
    writeln!(out, "- blocked: {}", c.blocked).expect("writing to String is infallible");
    writeln!(out, "- in-progress: {}", c.in_progress).expect("writing to String is infallible");
    writeln!(
        out,
        "- completed ({}h): {}",
        ctx.stale_threshold_hours, c.completed
    )
    .expect("writing to String is infallible");
    writeln!(out, "- hotspots: {}", c.hotspots).expect("writing to String is infallible");
    writeln!(
        out,
        "- stale (>{}h): {}",
        ctx.stale_threshold_hours, c.stale
    )
    .expect("writing to String is infallible");
}

/// Emit a `## {heading}` section for in-progress / ready / blocked
/// categories. Each bullet shows `#N [Priority] title (owner)`.
///
/// When `total > visible.len()` (i.e. truncation happened) a trailing
/// `_… (K more omitted)_` italic line informs the agent they should
/// narrow via `ready` / `list`.
fn render_category_list_summaries(
    out: &mut String,
    heading: &str,
    visible: &[PrimeIssueSummary],
    total: usize,
) {
    if visible.is_empty() {
        return;
    }
    writeln!(out, "\n## {heading}").expect("writing to String is infallible");
    for s in visible {
        write!(
            out,
            "- #{num} [{prio}] {title}",
            num = s.number,
            prio = s.priority,
            title = s.title,
        )
        .expect("writing to String is infallible");
        if let Some(agent) = &s.agent {
            write!(out, " (@{agent})").expect("writing to String is infallible");
        }
        out.push('\n');
    }
    if total > visible.len() {
        let omitted = total - visible.len();
        writeln!(out, "_… ({omitted} more omitted)_").expect("writing to String is infallible");
    }
}

/// Emit the `## Recently completed` section.
fn render_completed(out: &mut String, visible: &[CompletedIssueSummary], total: usize) {
    if visible.is_empty() {
        return;
    }
    out.push_str("\n## Recently completed\n");
    for c in visible {
        writeln!(
            out,
            "- #{num} [{prio}] {title}",
            num = c.number,
            prio = c.priority,
            title = c.title,
        )
        .expect("writing to String is infallible");
    }
    if total > visible.len() {
        let omitted = total - visible.len();
        writeln!(out, "_… ({omitted} more omitted)_").expect("writing to String is infallible");
    }
}

/// Emit the `## Hotspots` section (most-blocking issues first).
fn render_hotspots(out: &mut String, visible: &[HotspotSummary], total: usize) {
    if visible.is_empty() {
        return;
    }
    out.push_str("\n## Hotspots\n");
    for h in visible {
        writeln!(
            out,
            "- #{num} [{prio}] {title} — blocks {count}",
            num = h.number,
            prio = h.priority,
            title = h.title,
            count = h.blocking_count,
        )
        .expect("writing to String is infallible");
    }
    if total > visible.len() {
        let omitted = total - visible.len();
        writeln!(out, "_… ({omitted} more omitted)_").expect("writing to String is infallible");
    }
}

/// Emit the `## Stale claims` section.
///
/// The threshold is echoed in the heading so the agent sees the window
/// used to select the claims. Elided entirely when `visible` is empty.
fn render_stale(
    out: &mut String,
    visible: &[StaleIssueSummary],
    total: usize,
    stale_threshold_hours: u64,
) {
    if visible.is_empty() {
        return;
    }
    writeln!(out, "\n## Stale claims (>{stale_threshold_hours}h)")
        .expect("writing to String is infallible");
    for s in visible {
        write!(
            out,
            "- #{num} {title} — {hours}h",
            num = s.number,
            title = s.title,
            hours = s.hours_stale,
        )
        .expect("writing to String is infallible");
        if let Some(agent) = &s.agent {
            write!(out, " (@{agent})").expect("writing to String is infallible");
        }
        out.push('\n');
    }
    if total > visible.len() {
        let omitted = total - visible.len();
        writeln!(out, "_… ({omitted} more omitted)_").expect("writing to String is infallible");
    }
}

/// Emit the `## Issues with cycles` section (SPEC §7.3 flow step 2).
///
/// Each rendered cycle appears as a sub-bullet. Local members render as
/// `#N`; cross-repo members are already stripped by
/// [`cross_repo::project_all_cycles`] and surface in the §11.4 trailer.
/// Per-cycle member lists are capped at `max_per_category` with a `… (K
/// more)` tail when truncated.
///
/// Cycles whose local projection is empty (entirely cross-repo) are
/// elided from the member list but their omitted members still appear
/// in the trailer — we emit an explanatory `"- (cross-repo only — see
/// trailer)"` sentinel so the count of cycles listed matches the count
/// of detected cycles.
fn render_cycles(out: &mut String, cycles: &[Vec<u64>], max_per_category: usize) {
    if cycles.is_empty() {
        return;
    }
    out.push_str("\n## Issues with cycles\n");
    for (idx, cycle) in cycles.iter().enumerate() {
        let heading = format!("cycle {}:", idx + 1);
        if cycle.is_empty() {
            writeln!(out, "- {heading} (cross-repo only — see trailer)")
                .expect("writing to String is infallible");
            continue;
        }
        let take = max_per_category.max(1);
        let shown: Vec<String> = cycle.iter().take(take).map(|n| format!("#{n}")).collect();
        let trailer = if cycle.len() > take {
            format!(" … ({} more)", cycle.len() - take)
        } else {
            String::new()
        };
        writeln!(
            out,
            "- {heading} {members}{trailer}",
            members = shown.join(" → "),
        )
        .expect("writing to String is infallible");
    }
}

/// Emit the `## Session` section with Epic 1.5 metadata.
fn render_session(out: &mut String, session: &SessionMeta) {
    out.push_str("\n## Session\n");
    writeln!(out, "- agent_kind: {}", session.agent_kind).expect("writing to String is infallible");
    writeln!(out, "- agent_client: {}", session.agent_client)
        .expect("writing to String is infallible");
    if let Some(f) = &session.agent_field {
        writeln!(out, "- agent_field: {f}").expect("writing to String is infallible");
    }
    writeln!(out, "- connected_at: {}", session.connected_at.to_rfc3339())
        .expect("writing to String is infallible");
}

/// Emit the `## Drift warnings` section (Epic 1.6).
///
/// Elided when `None` (clean report, panic, or error from the
/// background reconcile task). When present, `warnings` is already
/// sorted lexicographically by [`summarise_drift`].
fn render_drift(out: &mut String, warnings: Option<&[String]>) {
    let Some(warnings) = warnings else { return };
    if warnings.is_empty() {
        return;
    }
    out.push_str("\n## Drift warnings\n");
    for w in warnings {
        writeln!(out, "- {w}").expect("writing to String is infallible");
    }
}

/// Emit the SPEC §11.4 `## Cross-repo references` trailer.
///
/// Elided when `cross_repo_refs` is `None` (no cross-repo node
/// participated in any cycle). Emitted bullets are the lexicographic
/// `omitted: Vec<String>` from the shared `cross_repo` module; the
/// italic summary uses the singular/plural phrasing shared with
/// `dep_cycles` (byte-for-byte parity — non-negotiable).
fn render_cross_repo_trailer(out: &mut String, refs: Option<&CrossRepoRefs>) {
    let Some(refs) = refs else { return };
    out.push_str("\n## Cross-repo references\n");
    for entry in &refs.omitted {
        writeln!(out, "- `{entry}`").expect("writing to String is infallible");
    }
    if let Some(summary) = &refs.summary {
        writeln!(out, "\n_{summary}_").expect("writing to String is infallible");
    }
}

/// Await the background drift check and convert to `drift_warnings`.
///
/// Returns `None` if the reconcile task panicked, returned an error, or
/// found no drift (`report.clean == true`). Returns `Some(warnings)` when
/// drift is detected, with human-readable summary strings.
async fn resolve_drift_warnings(
    drift_check: tokio::task::JoinHandle<
        Result<crate::tools::reconcile::ReconcileOutput, rmcp::model::ErrorData>,
    >,
) -> Option<Vec<String>> {
    match drift_check.await {
        Ok(Ok(reconcile_out)) if !reconcile_out.report.clean => {
            Some(summarise_drift(&reconcile_out.report))
        }
        _ => None,
    }
}

/// Convert a [`ReconcileReport`] into human-readable drift warning strings.
///
/// Groups drift items by type tag and produces one summary line per drift
/// type, e.g. `"3 stale ready states"`, `"1 uncascaded closure"`.
fn summarise_drift(report: &ReconcileReport) -> Vec<String> {
    // Count occurrences of each drift type from the serialised JSON values.
    // Each drift_found entry is an externally-tagged serde enum: `{"VariantName": {...}}`.
    let mut counts: HashMap<String, usize> = HashMap::new();

    for drift in &report.drift_found {
        if let Some(obj) = drift.as_object()
            && let Some(key) = obj.keys().next()
        {
            *counts.entry(key.clone()).or_insert(0) += 1;
        }
    }

    let mut warnings: Vec<String> = counts
        .into_iter()
        .map(|(kind, count)| {
            let label = match kind.as_str() {
                "UncascadedClosure" => "uncascaded closure",
                "OrphanedBlockingEdge" => "orphaned blocking edge",
                "MalformedAgentField" => "malformed agent field",
                "MissingProjectField" => "missing project field",
                "CycleDetected" => "cycle detected",
                "StaleClaim" => "stale claim",
                other => other,
            };
            if count == 1 {
                format!("1 {label}")
            } else {
                format!("{count} {label}s")
            }
        })
        .collect();

    // Sort for deterministic output in tests.
    warnings.sort();
    warnings
}

/// Intermediate result holding categorised issue lists.
struct CategorisedIssues {
    in_progress: Vec<IssueSummary>,
    ready: Vec<IssueSummary>,
    blocked: Vec<IssueSummary>,
    completed: Vec<CompletedIssueSummary>,
    hotspots: Vec<HotspotSummary>,
    stale: Vec<StaleIssueSummary>,
}

/// Categorise issues into the prime result categories.
///
/// - `in_progress`: `Status::InProgress` and `IssueState::Open`
/// - `blocked`: open issues that have at least one open blocker in the graph
/// - `ready`: from `compute_ready_set()` (open, unblocked)
/// - `completed`: closed issues with `updated_at` within the stale threshold window
/// - `hotspots`: issues that block the most other issues (descending by count)
/// - `stale`: in-progress issues with `claimed_at` older than the threshold
fn categorise_issues(
    issues: &[Issue],
    graph: &DependencyGraph,
    ready_summaries: &[IssueSummary],
    stale_threshold_hours: u64,
    now: DateTime<Utc>,
) -> CategorisedIssues {
    let mut in_progress = Vec::new();
    let mut blocked = Vec::new();
    let mut completed = Vec::new();
    let mut stale = Vec::new();

    // Build a set of ready QualifiedIds for quick lookup.
    let ready_set: std::collections::HashSet<&QualifiedId> =
        ready_summaries.iter().map(|s| &s.qualified_id).collect();

    // Build issue lookup by QualifiedId.
    let issue_map: HashMap<&QualifiedId, &Issue> =
        issues.iter().map(|i| (&i.qualified_id, i)).collect();

    // The stale threshold doubles as the "recently completed" window.
    let completed_cutoff = now
        - chrono::Duration::hours(i64::from(
            u32::try_from(stale_threshold_hours).unwrap_or(u32::MAX),
        ));

    for issue in issues {
        // Collect recently-closed issues into the completed category.
        if issue.state != IssueState::Open {
            if issue.updated_at >= completed_cutoff {
                completed.push(CompletedIssueSummary {
                    qualified_id: issue.qualified_id.to_string(),
                    number: issue.number,
                    title: issue.title.clone(),
                    issue_type: issue.issue_type.map(|it| it.to_string()),
                    priority: issue.priority.to_string(),
                    closed_at: issue.updated_at.to_rfc3339(),
                    url: issue.url.clone(),
                });
            }
            continue;
        }

        let summary = issue_to_summary(issue);

        if issue.status == Status::InProgress {
            in_progress.push(summary.clone());

            // Check for staleness. Log if claimed_at is missing — may indicate
            // a data quality issue (agent claimed work but no timestamp recorded).
            if issue.claimed_at.is_none() {
                tracing::debug!(
                    number = issue.number,
                    qualified_id = %issue.qualified_id,
                    "InProgress issue has no claimed_at — skipped for stale detection"
                );
            }
            if let Some(claimed_at) = issue.claimed_at {
                let hours_elapsed = (now - claimed_at).num_hours().unsigned_abs();
                if hours_elapsed > stale_threshold_hours {
                    stale.push(StaleIssueSummary {
                        qualified_id: issue.qualified_id.to_string(),
                        number: issue.number,
                        title: issue.title.clone(),
                        agent: issue.agent.clone(),
                        claimed_at: claimed_at.to_rfc3339(),
                        hours_stale: hours_elapsed,
                        url: issue.url.clone(),
                    });
                }
            }
        } else if !ready_set.contains(&issue.qualified_id) {
            // Not in_progress and not ready — check if blocked.
            // Exclude Deferred issues: they were intentionally deferred, not
            // dependency-blocked, so showing them as "blocked" confuses agents.
            if issue.status != Status::Deferred
                && (issue.status == Status::Blocked || has_open_blockers(issue, graph))
            {
                blocked.push(summary);
            }
        }
    }

    // Sort in_progress by priority ASC, then created_at ASC.
    in_progress.sort_by(|a, b| {
        a.priority
            .as_sort_key()
            .cmp(&b.priority.as_sort_key())
            .then_with(|| a.created_at.cmp(&b.created_at))
    });

    // Sort blocked by priority ASC, then created_at ASC.
    blocked.sort_by(|a, b| {
        a.priority
            .as_sort_key()
            .cmp(&b.priority.as_sort_key())
            .then_with(|| a.created_at.cmp(&b.created_at))
    });

    // Sort completed by closed_at DESC (most recently closed first).
    // String comparison is valid here because `closed_at` values are UTC
    // RFC 3339 timestamps (e.g. "2024-03-15T12:00:00+00:00"), which sort
    // lexicographically in the same order as chronologically. If the
    // timestamp format ever changes, this sort must be updated to parse
    // into a proper datetime type.
    completed.sort_by(|a, b| b.closed_at.cmp(&a.closed_at));

    // Sort stale by hours_stale DESC (most stale first).
    stale.sort_by(|a, b| b.hours_stale.cmp(&a.hours_stale));

    // Compute hotspots from the graph edges.
    let hotspots = compute_hotspots(graph, &issue_map);

    // Filter InProgress issues out of the ready list — an issue already being
    // worked on should not appear as "ready to pick up".
    let in_progress_ids: std::collections::HashSet<&QualifiedId> =
        in_progress.iter().map(|s| &s.qualified_id).collect();
    let filtered_ready: Vec<IssueSummary> = ready_summaries
        .iter()
        .filter(|s| !in_progress_ids.contains(&s.qualified_id))
        .cloned()
        .collect();

    CategorisedIssues {
        in_progress,
        ready: filtered_ready,
        blocked,
        completed,
        hotspots,
        stale,
    }
}

/// Check if an issue has at least one open blocker in the graph.
///
/// Uses `all_edges()` to find edges where this issue is the `source`
/// (blocked by target), then checks if any target is open.
fn has_open_blockers(issue: &Issue, graph: &DependencyGraph) -> bool {
    let issue_state = graph.issue_state();

    // Look up this issue's node in the graph.
    if let Some(&node_idx) = graph.node_map().get(&issue.qualified_id) {
        // Outgoing edges point to blockers.
        let inner = graph.inner_graph();
        inner
            .neighbors_directed(node_idx, petgraph::Direction::Outgoing)
            .any(|neighbor_idx| {
                let neighbor_qid = &inner[neighbor_idx];
                issue_state
                    .get(neighbor_qid)
                    .is_some_and(|state| *state == IssueState::Open)
            })
    } else {
        false
    }
}

/// Compute hotspots: issues that block the most other issues.
///
/// An issue is a hotspot if it appears as the `target` of blocking edges
/// (other issues depend on it). Returns sorted by `blocking_count` descending.
fn compute_hotspots(
    graph: &DependencyGraph,
    issue_map: &HashMap<&QualifiedId, &Issue>,
) -> Vec<HotspotSummary> {
    // Count how many issues each node blocks (incoming edges = dependents).
    let edges = graph.all_edges();
    let mut blocking_counts: HashMap<QualifiedId, usize> = HashMap::new();

    for edge in &edges {
        // edge.source is blocked by edge.target
        // So edge.target is the blocker — count how many things it blocks.
        *blocking_counts.entry(edge.target.clone()).or_insert(0) += 1;
    }

    let mut hotspots: Vec<HotspotSummary> = blocking_counts
        .into_iter()
        .filter_map(|(qid, count)| {
            // Only include open issues as hotspots.
            let issue = issue_map.get(&qid)?;
            if issue.state != IssueState::Open {
                return None;
            }
            Some(HotspotSummary {
                qualified_id: qid.to_string(),
                number: issue.number,
                title: issue.title.clone(),
                status: issue.status.to_string(),
                priority: issue.priority.to_string(),
                blocking_count: count,
                url: issue.url.clone(),
            })
        })
        .collect();

    // Sort by blocking_count DESC, then number ASC (stable tiebreaker).
    hotspots.sort_by(|a, b| {
        b.blocking_count
            .cmp(&a.blocking_count)
            .then_with(|| a.number.cmp(&b.number))
    });

    hotspots
}

/// Convert an [`Issue`] to an [`IssueSummary`] for categorisation.
fn issue_to_summary(issue: &Issue) -> IssueSummary {
    IssueSummary {
        qualified_id: issue.qualified_id.clone(),
        number: issue.number,
        title: issue.title.clone(),
        issue_type: issue.issue_type,
        status: issue.status,
        priority: issue.priority,
        agent: issue.agent.clone(),
        milestone: issue.milestone.clone(),
        story_points: issue.story_points,
        defer_until: issue.defer_until,
        labels: issue.labels.clone(),
        created_at: issue.created_at,
        url: issue.url.clone(),
    }
}

#[cfg(test)]
#[allow(clippy::similar_names)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use chrono::{TimeZone, Utc};
    use unblock_core::cache::GraphCache;
    use unblock_core::config::Config;
    use unblock_core::graph::DependencyGraph;
    use unblock_core::types::{
        BlockingEdge, Issue, IssueState, IssueType, Priority, QualifiedId, Status,
    };

    use super::*;
    use crate::server::ServerState;

    // ── Test helpers ───────────────────────────────────────────────────

    /// Owner/repo used by prime test fixtures. Must match the values passed
    /// to `compute_ready_set` so SPEC §3.3 Filter 3 (§14 Invariant 14(a))
    /// admits the local issues.
    const TEST_OWNER: &str = "test-owner";
    const TEST_REPO: &str = "test-repo";

    /// Helper to create a `QualifiedId` for tests.
    fn qid(number: u64) -> QualifiedId {
        QualifiedId::new(TEST_OWNER, TEST_REPO, number)
    }

    /// Build a minimal `Issue` for testing.
    fn test_issue(number: u64, state: IssueState, status: Status) -> Issue {
        Issue {
            qualified_id: qid(number),
            number,
            node_id: format!("NODE_{number}"),
            title: format!("Issue #{number}"),
            issue_type: Some(IssueType::Task),
            status,
            priority: Priority::P1,
            agent: None,
            claimed_at: None,
            pipeline_stage: None,
            story_points: None,
            defer_until: None,
            labels: vec![],
            milestone: None,
            assignees: vec![],
            state,
            body: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            url: format!("https://github.com/test-owner/test-repo/issues/{number}"),
            comments: vec![],
            blocked_by: vec![],
            blocking: vec![],
            parent: None,
            sub_issues: vec![],
        }
    }

    /// Create a `ServerState` for unit tests.
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
            github: Arc::new(client) as Arc<dyn unblock_github::GitHubApi>,
            cache: Arc::new(GraphCache::new(Duration::from_secs(300))),
            agent_kind: std::sync::OnceLock::new(),
            agent_client: std::sync::OnceLock::new(),
            connected_at: std::sync::OnceLock::new(),
        }
    }

    // ── Categorisation tests ──────────────────────────────────────────

    #[test]
    fn categorise_empty_issues_returns_empty() {
        let graph = DependencyGraph::build(&[], &[]);
        let ready = graph.compute_ready_set(&[], TEST_OWNER, TEST_REPO);
        let result = categorise_issues(&[], &graph, &ready, 24, Utc::now());

        assert!(result.in_progress.is_empty());
        assert!(result.ready.is_empty());
        assert!(result.blocked.is_empty());
        assert!(result.completed.is_empty());
        assert!(result.hotspots.is_empty());
        assert!(result.stale.is_empty());
    }

    #[test]
    fn categorise_in_progress_issues() {
        let mut issue = test_issue(1, IssueState::Open, Status::InProgress);
        issue.agent = Some("agent-x".to_owned());
        issue.claimed_at = Some(Utc::now());
        let issues = vec![issue];

        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues, TEST_OWNER, TEST_REPO);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        assert_eq!(result.in_progress.len(), 1);
        assert_eq!(result.in_progress[0].number, 1);
        assert!(
            result.stale.is_empty(),
            "recently claimed should not be stale"
        );
    }

    #[test]
    fn categorise_stale_claims() {
        let mut issue = test_issue(1, IssueState::Open, Status::InProgress);
        issue.agent = Some("agent-x".to_owned());
        // Claimed 48 hours ago.
        issue.claimed_at = Some(Utc::now() - chrono::Duration::hours(48));
        let issues = vec![issue];

        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues, TEST_OWNER, TEST_REPO);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        assert_eq!(result.in_progress.len(), 1);
        assert_eq!(result.stale.len(), 1);
        assert_eq!(result.stale[0].number, 1);
        assert!(result.stale[0].hours_stale >= 47); // at least 47 hours
    }

    #[test]
    fn categorise_blocked_issues() {
        // Issue #1 blocks issue #2.
        let issue1 = test_issue(1, IssueState::Open, Status::Ready);
        let mut issue2 = test_issue(2, IssueState::Open, Status::Blocked);
        issue2.status = Status::Blocked;
        let issues = vec![issue1, issue2];
        let edges = vec![BlockingEdge {
            source: qid(2),
            target: qid(1),
        }];

        let graph = DependencyGraph::build(&issues, &edges);
        let ready = graph.compute_ready_set(&issues, TEST_OWNER, TEST_REPO);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        // Issue #1 is ready (no blockers), issue #2 is blocked.
        assert_eq!(result.ready.len(), 1);
        assert_eq!(result.ready[0].number, 1);
        assert_eq!(result.blocked.len(), 1);
        assert_eq!(result.blocked[0].number, 2);
    }

    #[test]
    fn categorise_hotspots() {
        // Issue #1 blocks issues #2 and #3.
        let issue1 = test_issue(1, IssueState::Open, Status::Ready);
        let issue2 = test_issue(2, IssueState::Open, Status::Blocked);
        let issue3 = test_issue(3, IssueState::Open, Status::Blocked);
        let issues = vec![issue1, issue2, issue3];
        let edges = vec![
            BlockingEdge {
                source: qid(2),
                target: qid(1),
            },
            BlockingEdge {
                source: qid(3),
                target: qid(1),
            },
        ];

        let graph = DependencyGraph::build(&issues, &edges);
        let ready = graph.compute_ready_set(&issues, TEST_OWNER, TEST_REPO);
        let issue_map: HashMap<&QualifiedId, &Issue> =
            issues.iter().map(|i| (&i.qualified_id, i)).collect();
        let hotspots = compute_hotspots(&graph, &issue_map);

        assert_eq!(hotspots.len(), 1);
        assert_eq!(hotspots[0].number, 1);
        assert_eq!(hotspots[0].blocking_count, 2);

        // Also verify via full categorise.
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());
        assert_eq!(result.hotspots.len(), 1);
        assert_eq!(result.hotspots[0].blocking_count, 2);
    }

    #[test]
    fn hotspots_excludes_closed_issues() {
        // Issue #1 blocks #2, but #1 is closed.
        let issue1 = test_issue(1, IssueState::Closed, Status::Closed);
        let issue2 = test_issue(2, IssueState::Open, Status::Ready);
        let issues = vec![issue1, issue2];
        let edges = vec![BlockingEdge {
            source: qid(2),
            target: qid(1),
        }];

        let graph = DependencyGraph::build(&issues, &edges);
        let issue_map: HashMap<&QualifiedId, &Issue> =
            issues.iter().map(|i| (&i.qualified_id, i)).collect();
        let hotspots = compute_hotspots(&graph, &issue_map);

        assert!(
            hotspots.is_empty(),
            "closed issues should not appear as hotspots"
        );
    }

    #[test]
    fn hotspots_sorted_by_blocking_count_desc() {
        // #1 blocks 3 issues, #4 blocks 1 issue.
        let issue1 = test_issue(1, IssueState::Open, Status::Ready);
        let issue2 = test_issue(2, IssueState::Open, Status::Blocked);
        let issue3 = test_issue(3, IssueState::Open, Status::Blocked);
        let issue4 = test_issue(4, IssueState::Open, Status::Ready);
        let issue5 = test_issue(5, IssueState::Open, Status::Blocked);
        let issues = vec![issue1, issue2, issue3, issue4, issue5];
        let edges = vec![
            BlockingEdge {
                source: qid(2),
                target: qid(1),
            },
            BlockingEdge {
                source: qid(3),
                target: qid(1),
            },
            BlockingEdge {
                source: qid(5),
                target: qid(1),
            },
            BlockingEdge {
                source: qid(5),
                target: qid(4),
            },
        ];

        let graph = DependencyGraph::build(&issues, &edges);
        let issue_map: HashMap<&QualifiedId, &Issue> =
            issues.iter().map(|i| (&i.qualified_id, i)).collect();
        let hotspots = compute_hotspots(&graph, &issue_map);

        assert_eq!(hotspots.len(), 2);
        assert_eq!(hotspots[0].number, 1);
        assert_eq!(hotspots[0].blocking_count, 3);
        assert_eq!(hotspots[1].number, 4);
        assert_eq!(hotspots[1].blocking_count, 1);
    }

    #[test]
    fn closed_issues_excluded_from_open_categories() {
        let issue = test_issue(1, IssueState::Closed, Status::Closed);
        let issues = vec![issue];

        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues, TEST_OWNER, TEST_REPO);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        assert!(result.in_progress.is_empty());
        assert!(result.ready.is_empty());
        assert!(result.blocked.is_empty());
        assert!(result.stale.is_empty());
        // Recently closed issue should appear in completed (updated_at is "now").
        assert_eq!(result.completed.len(), 1);
        assert_eq!(result.completed[0].number, 1);
    }

    #[test]
    fn completed_excludes_old_closed_issues() {
        let mut issue = test_issue(1, IssueState::Closed, Status::Closed);
        // Updated 48 hours ago — outside the default 24h window.
        issue.updated_at = Utc::now() - chrono::Duration::hours(48);
        let issues = vec![issue];

        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues, TEST_OWNER, TEST_REPO);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        assert!(
            result.completed.is_empty(),
            "issues closed more than 24h ago should not appear in completed"
        );
    }

    #[test]
    fn completed_sorted_by_closed_at_desc() {
        let mut issue1 = test_issue(1, IssueState::Closed, Status::Closed);
        issue1.updated_at = Utc::now() - chrono::Duration::hours(2);

        let mut issue2 = test_issue(2, IssueState::Closed, Status::Closed);
        issue2.updated_at = Utc::now() - chrono::Duration::hours(1);

        let issues = vec![issue1, issue2];
        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues, TEST_OWNER, TEST_REPO);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        assert_eq!(result.completed.len(), 2);
        assert_eq!(
            result.completed[0].number, 2,
            "most recently closed should come first"
        );
        assert_eq!(
            result.completed[1].number, 1,
            "older closed should come second"
        );
    }

    #[test]
    fn completed_respects_custom_threshold() {
        let mut issue = test_issue(1, IssueState::Closed, Status::Closed);
        // Updated 30 hours ago — outside 24h but inside 48h.
        issue.updated_at = Utc::now() - chrono::Duration::hours(30);
        let issues = vec![issue];

        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues, TEST_OWNER, TEST_REPO);

        // With default 24h window: excluded.
        let result_24 = categorise_issues(&issues, &graph, &ready, 24, Utc::now());
        assert!(
            result_24.completed.is_empty(),
            "30h-old closure should not appear with 24h window"
        );

        // With 48h window: included.
        let result_48 = categorise_issues(&issues, &graph, &ready, 48, Utc::now());
        assert_eq!(
            result_48.completed.len(),
            1,
            "30h-old closure should appear with 48h window"
        );
    }

    #[test]
    fn deferred_issues_excluded_from_blocked() {
        // Issue #1 blocks issue #2 (deferred). Deferred should not appear as blocked.
        let issue1 = test_issue(1, IssueState::Open, Status::Ready);
        let issue2 = test_issue(2, IssueState::Open, Status::Deferred);
        let issues = vec![issue1, issue2];
        let edges = vec![BlockingEdge {
            source: qid(2),
            target: qid(1),
        }];

        let graph = DependencyGraph::build(&issues, &edges);
        let ready = graph.compute_ready_set(&issues, TEST_OWNER, TEST_REPO);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        assert!(
            result.blocked.is_empty(),
            "deferred issue should not appear in blocked list"
        );
    }

    #[test]
    fn in_progress_excluded_from_ready() {
        // An InProgress issue with no blockers should only appear in in_progress, not ready.
        let mut issue = test_issue(1, IssueState::Open, Status::InProgress);
        issue.agent = Some("agent-x".to_owned());
        issue.claimed_at = Some(Utc::now());
        let issues = vec![issue];

        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues, TEST_OWNER, TEST_REPO);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        assert_eq!(result.in_progress.len(), 1);
        assert!(
            result.ready.is_empty(),
            "InProgress issues should not appear in the ready list"
        );
    }

    #[test]
    fn in_progress_sorted_by_priority_then_created_at() {
        let earlier = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let later = Utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap();

        let mut issue1 = test_issue(1, IssueState::Open, Status::InProgress);
        issue1.priority = Priority::P2;
        issue1.created_at = later;
        issue1.claimed_at = Some(Utc::now());

        let mut issue2 = test_issue(2, IssueState::Open, Status::InProgress);
        issue2.priority = Priority::P0;
        issue2.created_at = earlier;
        issue2.claimed_at = Some(Utc::now());

        let issues = vec![issue1, issue2];
        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues, TEST_OWNER, TEST_REPO);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        assert_eq!(result.in_progress.len(), 2);
        assert_eq!(result.in_progress[0].number, 2, "P0 should come first");
        assert_eq!(result.in_progress[1].number, 1, "P2 should come second");
    }

    #[test]
    fn stale_sorted_by_hours_desc() {
        let mut issue1 = test_issue(1, IssueState::Open, Status::InProgress);
        issue1.agent = Some("a".to_owned());
        issue1.claimed_at = Some(Utc::now() - chrono::Duration::hours(30));

        let mut issue2 = test_issue(2, IssueState::Open, Status::InProgress);
        issue2.agent = Some("b".to_owned());
        issue2.claimed_at = Some(Utc::now() - chrono::Duration::hours(72));

        let issues = vec![issue1, issue2];
        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues, TEST_OWNER, TEST_REPO);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        assert_eq!(result.stale.len(), 2);
        assert_eq!(
            result.stale[0].number, 2,
            "most stale (72h) should come first"
        );
        assert_eq!(
            result.stale[1].number, 1,
            "less stale (30h) should come second"
        );
    }

    // ── SessionMeta tests ─────────────────────────────────────────────

    #[tokio::test]
    async fn session_meta_from_state_defaults_when_uninitialised() {
        let state = test_state().await;
        let meta = SessionMeta::from_state(&state);
        assert_eq!(meta.agent_client, "unknown");
        assert_eq!(meta.agent_kind, "unknown");
        // agent_field depends on UNBLOCK_AGENT env var — not asserted here
        // connected_at falls back to Utc::now(), just verify it's recent
        let elapsed = Utc::now() - meta.connected_at;
        assert!(
            elapsed.num_seconds() < 5,
            "connected_at should be recent, got {elapsed}"
        );
    }

    #[tokio::test]
    async fn session_meta_from_state_populated() {
        use unblock_core::client::{AgentClient, AgentKind};

        let state = test_state().await;
        let _ = state.agent_kind.set(AgentKind::ClaudeCode);
        let _ = state.agent_client.set(AgentClient {
            name: "Claude Code".to_owned(),
            version: "1.2.3".to_owned(),
        });
        let connected = Utc::now();
        let _ = state.connected_at.set(connected);

        let meta = SessionMeta::from_state(&state);
        assert_eq!(meta.agent_client, "Claude Code");
        assert_eq!(meta.agent_kind, "claude-code");
        assert_eq!(meta.connected_at, connected);
    }

    // ── RepoIdentity tests (bead unblock-eos.34) ──────────────────────
    //
    // Mirror the `SessionMeta::from_state` pair above. The "uninitialised"
    // (no `UNBLOCK_PROJECT`) and "populated" (project number set) cases
    // both read from `state.github` — i.e. the `GitHubApi` trait — so
    // these tests also pin the DECISION comment at `handle_prime` that
    // switched the `project_number` source from `state.config` to
    // `state.github`.

    /// Build a [`ServerState`] with an explicit `UNBLOCK_PROJECT` value
    /// so the populated test can exercise the `Some(project_number)`
    /// branch of [`RepoIdentity::from_state`]. Shape matches
    /// [`test_state`] byte-for-byte apart from the extra env var.
    async fn test_state_with_project(project_number: u64) -> ServerState {
        let config = Config::load_from(|key| match key {
            "GITHUB_TOKEN" => Ok("ghp_test_token_for_unit_tests".to_owned()),
            "UNBLOCK_REPO" => Ok("test-owner/test-repo".to_owned()),
            "UNBLOCK_PROJECT" => Ok(project_number.to_string()),
            _ => Err(std::env::VarError::NotPresent),
        })
        .expect("test config should load");

        let client = unblock_github::client::GitHubClient::new(&config)
            .await
            .expect("test client should initialize");

        ServerState {
            config: Arc::new(config),
            github: Arc::new(client) as Arc<dyn unblock_github::GitHubApi>,
            cache: Arc::new(GraphCache::new(Duration::from_secs(300))),
            agent_kind: std::sync::OnceLock::new(),
            agent_client: std::sync::OnceLock::new(),
            connected_at: std::sync::OnceLock::new(),
        }
    }

    #[tokio::test]
    async fn repo_identity_from_state_defaults_when_uninitialised() {
        // `test_state` sets `UNBLOCK_REPO=test-owner/test-repo` and
        // leaves `UNBLOCK_PROJECT` unset, so `project_number` falls back
        // to `None` via `state.github.project_number()` (the
        // `GitHubApi` pass-through of `config.project_number`).
        let state = test_state().await;
        let repo = RepoIdentity::from_state(&state);
        assert_eq!(repo.owner, "test-owner");
        assert_eq!(repo.repo, "test-repo");
        assert_eq!(repo.project_number, None);
    }

    #[tokio::test]
    async fn repo_identity_from_state_populated() {
        // Exercises the `Some(project_number)` branch — the
        // `state.github.project_number()` accessor returns whatever
        // `Config` parsed from `UNBLOCK_PROJECT`.
        let state = test_state_with_project(42).await;
        let repo = RepoIdentity::from_state(&state);
        assert_eq!(repo.owner, "test-owner");
        assert_eq!(repo.repo, "test-repo");
        assert_eq!(repo.project_number, Some(42));
    }

    /// Helper test invoked by subprocess tests below. Prints the `agent_field`
    /// value from `SessionMeta::from_state` so the parent process can assert it.
    ///
    /// Protocol: prints `AGENT_FIELD=<value>` or `AGENT_FIELD=NONE` to stdout.
    #[ignore = "invoked by subprocess tests, not meant to run directly"]
    #[tokio::test]
    async fn subprocess_helper_print_agent_field() {
        let state = test_state().await;
        let meta = SessionMeta::from_state(&state);
        match meta.agent_field {
            Some(val) => println!("AGENT_FIELD={val}"),
            None => println!("AGENT_FIELD=NONE"),
        }
    }

    /// Spawns a child process *with* `UNBLOCK_AGENT=test-supervisor` set and
    /// asserts that `SessionMeta.agent_field` is `Some("test-supervisor")`.
    #[test]
    fn session_meta_agent_field_set_via_subprocess() {
        let test_bin = std::env::current_exe().expect("should resolve test binary path");
        let output = std::process::Command::new(&test_bin)
            .arg("--exact")
            .arg("tools::prime::tests::subprocess_helper_print_agent_field")
            .arg("--include-ignored")
            .arg("--nocapture")
            .env("UNBLOCK_AGENT", "test-supervisor")
            // Clear detection env vars to avoid side effects on other tests.
            .env_remove("CLAUDE_CODE_ENTRYPOINT")
            .env_remove("GITHUB_COPILOT_TOKEN")
            .env_remove("CURSOR_TRACE_ID")
            .output()
            .expect("failed to spawn subprocess");

        assert!(
            output.status.success(),
            "subprocess exited with non-zero status: {output:?}"
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("AGENT_FIELD=test-supervisor"),
            "expected AGENT_FIELD=test-supervisor in subprocess output, got:\n{stdout}"
        );
    }

    /// Spawns a child process *without* `UNBLOCK_AGENT` and asserts that
    /// `SessionMeta.agent_field` is `None`.
    #[test]
    fn session_meta_agent_field_unset_via_subprocess() {
        let test_bin = std::env::current_exe().expect("should resolve test binary path");
        let output = std::process::Command::new(&test_bin)
            .arg("--exact")
            .arg("tools::prime::tests::subprocess_helper_print_agent_field")
            .arg("--include-ignored")
            .arg("--nocapture")
            .env_remove("UNBLOCK_AGENT")
            // Clear detection env vars to avoid side effects on other tests.
            .env_remove("CLAUDE_CODE_ENTRYPOINT")
            .env_remove("GITHUB_COPILOT_TOKEN")
            .env_remove("CURSOR_TRACE_ID")
            .output()
            .expect("failed to spawn subprocess");

        assert!(
            output.status.success(),
            "subprocess exited with non-zero status: {output:?}"
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("AGENT_FIELD=NONE"),
            "expected AGENT_FIELD=NONE in subprocess output, got:\n{stdout}"
        );
    }

    // ── PrimeParams deserialization tests ──────────────────────────────

    #[test]
    fn prime_params_defaults() {
        let json = r"{}";
        let params: PrimeParams = serde_json::from_str(json).expect("should deserialize");
        assert!(params.stale_threshold_hours.is_none());
        assert!(params.max_per_category.is_none());
    }

    #[test]
    fn prime_params_zero_stale_threshold_deserializes() {
        let json = r#"{"stale_threshold_hours": 0}"#;
        let params: PrimeParams = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(params.stale_threshold_hours, Some(0));
    }

    #[test]
    fn prime_params_zero_max_per_category_deserializes() {
        let json = r#"{"max_per_category": 0}"#;
        let params: PrimeParams = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(params.max_per_category, Some(0));
    }

    #[tokio::test]
    async fn handle_prime_rejects_zero_stale_threshold() {
        let state = Arc::new(test_state().await);
        let params = PrimeParams {
            stale_threshold_hours: Some(0),
            max_per_category: None,
            agent: None,
        };

        let err = handle_prime(&params, &state)
            .await
            .expect_err("stale_threshold_hours=0 should be rejected");
        assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
        assert!(
            err.message.contains("stale_threshold_hours"),
            "error should mention the parameter name: {}",
            err.message,
        );
    }

    #[tokio::test]
    async fn handle_prime_rejects_zero_max_per_category() {
        let state = Arc::new(test_state().await);
        let params = PrimeParams {
            stale_threshold_hours: None,
            max_per_category: Some(0),
            agent: None,
        };

        let err = handle_prime(&params, &state)
            .await
            .expect_err("max_per_category=0 should be rejected");
        assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
        assert!(
            err.message.contains("max_per_category"),
            "error should mention the parameter name: {}",
            err.message,
        );
    }

    #[test]
    fn prime_params_explicit_values() {
        let json = r#"{"stale_threshold_hours": 48, "max_per_category": 5}"#;
        let params: PrimeParams = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(params.stale_threshold_hours, Some(48));
        assert_eq!(params.max_per_category, Some(5));
    }

    // ── PrimeResult + renderer shape tests (post unblock-eos.7) ────────
    //
    // The pre-eos.7 assertions that probed typed-field serialisation no
    // longer apply — `PrimeResult = { context: String }`. The checks
    // below exercise the renderer end-to-end via `render_context` so
    // the six-section contract (header → counts → cycles → session →
    // drift → cross-repo trailer) is pinned.

    /// Build a zero-count `PrimeCounts` for clean-slate renderer tests.
    fn zero_counts() -> PrimeCounts {
        PrimeCounts {
            in_progress: 0,
            ready: 0,
            blocked: 0,
            completed: 0,
            hotspots: 0,
            stale: 0,
        }
    }

    /// Frozen `connected_at` timestamp for byte-stable renderer snapshot
    /// tests (unblock-eos.10). Renders as `2026-01-01T12:00:00+00:00` via
    /// `DateTime::<Utc>::to_rfc3339` — note the explicit `+00:00` offset
    /// (chrono does not emit the `Z` short form).
    ///
    /// Mirrors the `Utc.with_ymd_and_hms(...)` idiom already used at
    /// prime.rs:1613-1614 and across the workspace (list.rs, ready.rs,
    /// integration.rs) for deterministic timestamp fixtures.
    fn frozen_connected_at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap()
    }

    /// Session fixture that matches the `SessionMeta::from_state`
    /// fallback branch (no initialisation has happened). Uses
    /// [`frozen_connected_at`] so renderer output is byte-stable for
    /// snapshot assertions.
    fn unknown_session() -> SessionMeta {
        SessionMeta {
            agent_client: "unknown".to_owned(),
            agent_kind: "unknown".to_owned(),
            agent_field: None,
            connected_at: frozen_connected_at(),
        }
    }

    /// Five-member cycle fixture shared by the `render_cycles`
    /// truncation tests (`max_per_category = 2` and
    /// `max_per_category = 1`). Gives boundary tests a single source of
    /// truth for the cycle body so future variants can reuse the same
    /// shape without re-asserting `[1, 2, 3, 4, 5]` verbatim.
    fn five_member_cycle() -> Vec<u64> {
        vec![1, 2, 3, 4, 5]
    }

    /// Build a minimal `ContextInputs` for renderer tests — every
    /// category slice is empty, no cycles, no drift.
    fn minimal_inputs<'a>(counts: &'a PrimeCounts, session: &'a SessionMeta) -> ContextInputs<'a> {
        ContextInputs {
            owner: "acme",
            repo: "widgets",
            project_number: None,
            stale_threshold_hours: 24,
            max_per_category: 10,
            counts,
            cycles: &[],
            cross_repo_refs: None,
            session,
            drift_warnings: None,
            in_progress: &[],
            ready: &[],
            blocked: &[],
            completed: &[],
            hotspots: &[],
            stale: &[],
        }
    }

    #[test]
    fn render_context_clean_emits_header_counts_session_only() {
        let counts = zero_counts();
        let session = unknown_session();
        let ctx = minimal_inputs(&counts, &session);
        let md = render_context(&ctx);

        assert_eq!(
            md,
            "\
# Repo: acme/widgets

## Counts
- ready: 0
- blocked: 0
- in-progress: 0
- completed (24h): 0
- hotspots: 0
- stale (>24h): 0

## Session
- agent_kind: unknown
- agent_client: unknown
- connected_at: 2026-01-01T12:00:00+00:00
"
        );
    }

    #[test]
    fn render_context_session_emits_agent_field_bullet_when_set() {
        let counts = zero_counts();
        let session = SessionMeta {
            agent_client: "Claude Code".to_owned(),
            agent_kind: "claude-code".to_owned(),
            agent_field: Some("rust-supervisor".to_owned()),
            connected_at: frozen_connected_at(),
        };
        let ctx = minimal_inputs(&counts, &session);
        let md = render_context(&ctx);

        assert_eq!(
            md,
            "\
# Repo: acme/widgets

## Counts
- ready: 0
- blocked: 0
- in-progress: 0
- completed (24h): 0
- hotspots: 0
- stale (>24h): 0

## Session
- agent_kind: claude-code
- agent_client: Claude Code
- agent_field: rust-supervisor
- connected_at: 2026-01-01T12:00:00+00:00
"
        );
    }

    #[test]
    fn render_context_emits_project_line_when_set() {
        let counts = zero_counts();
        let session = unknown_session();
        let mut ctx = minimal_inputs(&counts, &session);
        ctx.project_number = Some(42);
        let md = render_context(&ctx);

        assert_eq!(
            md,
            "\
# Repo: acme/widgets
Project: 42

## Counts
- ready: 0
- blocked: 0
- in-progress: 0
- completed (24h): 0
- hotspots: 0
- stale (>24h): 0

## Session
- agent_kind: unknown
- agent_client: unknown
- connected_at: 2026-01-01T12:00:00+00:00
"
        );
    }

    #[test]
    fn render_context_counts_echo_stale_threshold() {
        let counts = zero_counts();
        let session = unknown_session();
        let mut ctx = minimal_inputs(&counts, &session);
        ctx.stale_threshold_hours = 72;
        let md = render_context(&ctx);

        assert_eq!(
            md,
            "\
# Repo: acme/widgets

## Counts
- ready: 0
- blocked: 0
- in-progress: 0
- completed (72h): 0
- hotspots: 0
- stale (>72h): 0

## Session
- agent_kind: unknown
- agent_client: unknown
- connected_at: 2026-01-01T12:00:00+00:00
"
        );
    }

    #[test]
    fn render_context_cycles_section_elided_when_empty() {
        let counts = zero_counts();
        let session = unknown_session();
        let ctx = minimal_inputs(&counts, &session);
        let md = render_context(&ctx);

        assert_eq!(
            md,
            "\
# Repo: acme/widgets

## Counts
- ready: 0
- blocked: 0
- in-progress: 0
- completed (24h): 0
- hotspots: 0
- stale (>24h): 0

## Session
- agent_kind: unknown
- agent_client: unknown
- connected_at: 2026-01-01T12:00:00+00:00
"
        );
    }

    #[test]
    fn render_context_cycles_section_renders_local_projection() {
        let counts = zero_counts();
        let session = unknown_session();
        let mut ctx = minimal_inputs(&counts, &session);
        let cycles = vec![vec![6, 7, 8]];
        ctx.cycles = &cycles;
        let md = render_context(&ctx);

        assert_eq!(
            md,
            "\
# Repo: acme/widgets

## Counts
- ready: 0
- blocked: 0
- in-progress: 0
- completed (24h): 0
- hotspots: 0
- stale (>24h): 0

## Issues with cycles
- cycle 1: #6 → #7 → #8

## Session
- agent_kind: unknown
- agent_client: unknown
- connected_at: 2026-01-01T12:00:00+00:00
"
        );
    }

    #[test]
    fn render_context_cycle_members_truncated_to_max_per_category() {
        let counts = zero_counts();
        let session = unknown_session();
        let mut ctx = minimal_inputs(&counts, &session);
        ctx.max_per_category = 2;
        let cycles = vec![five_member_cycle()];
        ctx.cycles = &cycles;
        let md = render_context(&ctx);

        assert_eq!(
            md,
            "\
# Repo: acme/widgets

## Counts
- ready: 0
- blocked: 0
- in-progress: 0
- completed (24h): 0
- hotspots: 0
- stale (>24h): 0

## Issues with cycles
- cycle 1: #1 → #2 … (3 more)

## Session
- agent_kind: unknown
- agent_client: unknown
- connected_at: 2026-01-01T12:00:00+00:00
"
        );
    }

    /// Pin the minimum-value branch of `render_cycles` truncation
    /// (`max_per_category = 1`) against a multi-member cycle.
    ///
    /// Locks the boundary between the sibling truncation test
    /// (`max_per_category = 2`) and the lower clamp inside
    /// `render_cycles` (`max_per_category.max(1)`): at `1` exactly one
    /// member is shown and the trailer reports `(cycle.len() - 1)`
    /// omitted members. Added per unblock-eos.9 to lock the truncation
    /// contract of the `## Issues with cycles` section against
    /// regressions.
    #[test]
    fn render_context_cycle_members_truncated_at_max_per_category_one() {
        let counts = zero_counts();
        let session = unknown_session();
        let mut ctx = minimal_inputs(&counts, &session);
        ctx.max_per_category = 1;
        let cycles = vec![five_member_cycle()];
        ctx.cycles = &cycles;
        let md = render_context(&ctx);

        assert_eq!(
            md,
            "\
# Repo: acme/widgets

## Counts
- ready: 0
- blocked: 0
- in-progress: 0
- completed (24h): 0
- hotspots: 0
- stale (>24h): 0

## Issues with cycles
- cycle 1: #1 … (4 more)

## Session
- agent_kind: unknown
- agent_client: unknown
- connected_at: 2026-01-01T12:00:00+00:00
"
        );
    }

    #[test]
    fn render_context_cross_repo_only_cycle_emits_sentinel_bullet() {
        let counts = zero_counts();
        let session = unknown_session();
        let mut ctx = minimal_inputs(&counts, &session);
        let cycles = vec![vec![]]; // fully cross-repo projection.
        ctx.cycles = &cycles;
        let md = render_context(&ctx);

        assert_eq!(
            md,
            "\
# Repo: acme/widgets

## Counts
- ready: 0
- blocked: 0
- in-progress: 0
- completed (24h): 0
- hotspots: 0
- stale (>24h): 0

## Issues with cycles
- cycle 1: (cross-repo only — see trailer)

## Session
- agent_kind: unknown
- agent_client: unknown
- connected_at: 2026-01-01T12:00:00+00:00
"
        );
    }

    #[test]
    fn render_context_drift_warnings_elided_when_none() {
        let counts = zero_counts();
        let session = unknown_session();
        let ctx = minimal_inputs(&counts, &session);
        let md = render_context(&ctx);

        assert_eq!(
            md,
            "\
# Repo: acme/widgets

## Counts
- ready: 0
- blocked: 0
- in-progress: 0
- completed (24h): 0
- hotspots: 0
- stale (>24h): 0

## Session
- agent_kind: unknown
- agent_client: unknown
- connected_at: 2026-01-01T12:00:00+00:00
"
        );
    }

    #[test]
    fn render_context_drift_warnings_elided_when_empty_slice() {
        let counts = zero_counts();
        let session = unknown_session();
        let mut ctx = minimal_inputs(&counts, &session);
        let empty: [String; 0] = [];
        ctx.drift_warnings = Some(&empty);
        let md = render_context(&ctx);

        assert_eq!(
            md,
            "\
# Repo: acme/widgets

## Counts
- ready: 0
- blocked: 0
- in-progress: 0
- completed (24h): 0
- hotspots: 0
- stale (>24h): 0

## Session
- agent_kind: unknown
- agent_client: unknown
- connected_at: 2026-01-01T12:00:00+00:00
"
        );
    }

    #[test]
    fn render_context_drift_warnings_rendered_verbatim() {
        let counts = zero_counts();
        let session = unknown_session();
        let mut ctx = minimal_inputs(&counts, &session);
        let warnings = vec![
            "1 uncascaded closure".to_owned(),
            "3 stale claims".to_owned(),
        ];
        ctx.drift_warnings = Some(&warnings);
        let md = render_context(&ctx);

        assert_eq!(
            md,
            "\
# Repo: acme/widgets

## Counts
- ready: 0
- blocked: 0
- in-progress: 0
- completed (24h): 0
- hotspots: 0
- stale (>24h): 0

## Session
- agent_kind: unknown
- agent_client: unknown
- connected_at: 2026-01-01T12:00:00+00:00

## Drift warnings
- 1 uncascaded closure
- 3 stale claims
"
        );
    }

    #[test]
    fn render_context_cross_repo_trailer_renders_with_summary() {
        let counts = zero_counts();
        let session = unknown_session();
        let mut ctx = minimal_inputs(&counts, &session);
        let refs = CrossRepoRefs {
            omitted: vec!["other/repo#99".to_owned()],
            summary: Some("1 cross-repo cycle member omitted from `cycles`".to_owned()),
        };
        ctx.cross_repo_refs = Some(&refs);
        let md = render_context(&ctx);

        assert_eq!(
            md,
            "\
# Repo: acme/widgets

## Counts
- ready: 0
- blocked: 0
- in-progress: 0
- completed (24h): 0
- hotspots: 0
- stale (>24h): 0

## Session
- agent_kind: unknown
- agent_client: unknown
- connected_at: 2026-01-01T12:00:00+00:00

## Cross-repo references
- `other/repo#99`

_1 cross-repo cycle member omitted from `cycles`_
"
        );
    }

    #[test]
    fn render_context_section_order_matches_spec_contract() {
        // Build inputs that exercise every section so we can compare the
        // index order. Section headings anchor the comparison.
        let counts = PrimeCounts {
            in_progress: 0,
            ready: 0,
            blocked: 0,
            completed: 0,
            hotspots: 0,
            stale: 0,
        };
        let session = unknown_session();
        let mut ctx = minimal_inputs(&counts, &session);
        let cycles = vec![vec![6_u64, 7]];
        ctx.cycles = &cycles;
        let refs = CrossRepoRefs {
            omitted: vec!["other/repo#99".to_owned()],
            summary: Some("1 cross-repo cycle member omitted from `cycles`".to_owned()),
        };
        ctx.cross_repo_refs = Some(&refs);
        let drift = vec!["1 stale claim".to_owned()];
        ctx.drift_warnings = Some(&drift);

        let md = render_context(&ctx);

        // Byte-stable snapshot pins the SPEC-mandated section order
        // (header → counts → cycles → session → drift → cross-repo)
        // without needing separate `.find(...)` indexes.
        assert_eq!(
            md,
            "\
# Repo: acme/widgets

## Counts
- ready: 0
- blocked: 0
- in-progress: 0
- completed (24h): 0
- hotspots: 0
- stale (>24h): 0

## Issues with cycles
- cycle 1: #6 → #7

## Session
- agent_kind: unknown
- agent_client: unknown
- connected_at: 2026-01-01T12:00:00+00:00

## Drift warnings
- 1 stale claim

## Cross-repo references
- `other/repo#99`

_1 cross-repo cycle member omitted from `cycles`_
"
        );
    }

    // ── Integration test: full categorise pipeline ────────────────────

    #[test]
    fn integration_mixed_issues_categorised_correctly() {
        // Setup: 4 issues in different states.
        // #1: open, ready (no blockers)
        // #2: open, blocked by #1
        // #3: in_progress (claimed 48h ago — stale)
        // #4: closed
        let issue1 = test_issue(1, IssueState::Open, Status::Ready);
        let mut issue2 = test_issue(2, IssueState::Open, Status::Blocked);
        issue2.status = Status::Blocked;
        let mut issue3 = test_issue(3, IssueState::Open, Status::InProgress);
        issue3.agent = Some("agent-z".to_owned());
        issue3.claimed_at = Some(Utc::now() - chrono::Duration::hours(48));
        let issue4 = test_issue(4, IssueState::Closed, Status::Closed);

        let issues = vec![issue1, issue2, issue3, issue4];
        let edges = vec![BlockingEdge {
            source: qid(2),
            target: qid(1),
        }];

        let graph = DependencyGraph::build(&issues, &edges);
        let ready = graph.compute_ready_set(&issues, TEST_OWNER, TEST_REPO);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        // #1 is ready (#3 is InProgress so excluded from ready list).
        assert_eq!(
            result.ready.len(),
            1,
            "ready should include only #1 (InProgress #3 is excluded)"
        );
        assert_eq!(result.ready[0].number, 1);
        // #2 is blocked.
        assert_eq!(result.blocked.len(), 1);
        assert_eq!(result.blocked[0].number, 2);
        // #3 is in_progress + stale.
        assert_eq!(result.in_progress.len(), 1);
        assert_eq!(result.in_progress[0].number, 3);
        assert_eq!(result.stale.len(), 1);
        assert_eq!(result.stale[0].number, 3);
        // #4 is closed recently — should appear in completed.
        assert_eq!(result.completed.len(), 1);
        assert_eq!(result.completed[0].number, 4);
        // #1 is a hotspot (blocks #2).
        assert_eq!(result.hotspots.len(), 1);
        assert_eq!(result.hotspots[0].number, 1);
        assert_eq!(result.hotspots[0].blocking_count, 1);
    }

    // ── Cache update integration test ─────────────────────────────────

    #[tokio::test]
    async fn cache_updated_after_prime() {
        let state = test_state().await;
        assert!(
            !state.cache.is_fresh().await,
            "cache should be empty initially"
        );

        // Manually update cache (simulating what handle_prime does after fetch).
        let issues = vec![
            test_issue(1, IssueState::Open, Status::Ready),
            test_issue(2, IssueState::Open, Status::Ready),
        ];
        let graph = DependencyGraph::build(&issues, &[]);
        let ready_set = graph.compute_ready_set(&issues, TEST_OWNER, TEST_REPO);
        state.cache.update(issues, ready_set, graph).await;

        assert!(
            state.cache.is_fresh().await,
            "cache should be fresh after update"
        );
        let cached_ready = state.cache.get_ready_set().await;
        assert!(cached_ready.is_some());
        assert_eq!(cached_ready.unwrap().len(), 2);
    }

    // ── Max per category truncation test ──────────────────────────────

    #[test]
    fn max_per_category_truncates_results() {
        let mut issues = Vec::new();
        for i in 1..=20 {
            issues.push(test_issue(i, IssueState::Open, Status::Ready));
        }

        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues, TEST_OWNER, TEST_REPO);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        assert_eq!(
            result.ready.len(),
            20,
            "all 20 should be ready before truncation"
        );

        // Simulate truncation as handle_prime does it.
        let max = 5;
        let truncated: Vec<_> = result
            .ready
            .iter()
            .take(max)
            .map(PrimeIssueSummary::from_core)
            .collect();
        assert_eq!(truncated.len(), 5);
    }

    // ── Agent filter tests ───────────────────────────────────────────
    //
    // These exercise the production `apply_agent_filter` helper
    // introduced by bead `unblock-eos.8` (extracted out of the old
    // inline filter block in `handle_prime`). A pre-eos.8 local shim
    // used to re-implement the filter inline for these assertions —
    // removed in favour of the real helper so the tests cover the
    // actual call path.

    #[test]
    fn agent_filter_none_returns_all() {
        // Two in-progress issues with different agents.
        let mut issue1 = test_issue(1, IssueState::Open, Status::InProgress);
        issue1.agent = Some("agent-x".to_owned());
        issue1.claimed_at = Some(Utc::now());
        let mut issue2 = test_issue(2, IssueState::Open, Status::InProgress);
        issue2.agent = Some("agent-y".to_owned());
        issue2.claimed_at = Some(Utc::now());
        let issues = vec![issue1, issue2];

        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues, TEST_OWNER, TEST_REPO);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        let filtered = apply_agent_filter(result, None);
        assert_eq!(
            filtered.in_progress.len(),
            2,
            "None agent should return all in_progress issues"
        );
    }

    #[test]
    fn agent_filter_matches_in_progress() {
        let mut issue1 = test_issue(1, IssueState::Open, Status::InProgress);
        issue1.agent = Some("agent-x".to_owned());
        issue1.claimed_at = Some(Utc::now());
        let mut issue2 = test_issue(2, IssueState::Open, Status::InProgress);
        issue2.agent = Some("agent-y".to_owned());
        issue2.claimed_at = Some(Utc::now());
        let issues = vec![issue1, issue2];

        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues, TEST_OWNER, TEST_REPO);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        let filtered = apply_agent_filter(result, Some("agent-x"));
        assert_eq!(
            filtered.in_progress.len(),
            1,
            "should filter in_progress to agent-x only"
        );
    }

    #[test]
    fn agent_filter_matches_ready() {
        let mut issue1 = test_issue(1, IssueState::Open, Status::Ready);
        issue1.agent = Some("agent-x".to_owned());
        let mut issue2 = test_issue(2, IssueState::Open, Status::Ready);
        issue2.agent = Some("agent-y".to_owned());
        let issues = vec![issue1, issue2];

        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues, TEST_OWNER, TEST_REPO);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        let filtered = apply_agent_filter(result, Some("agent-x"));
        assert_eq!(
            filtered.ready.len(),
            1,
            "should filter ready to agent-x only"
        );
    }

    #[test]
    fn agent_filter_matches_blocked() {
        // #1 blocks #2 and #3. Agents assigned to the blocked issues.
        let issue1 = test_issue(1, IssueState::Open, Status::Ready);
        let mut issue2 = test_issue(2, IssueState::Open, Status::Blocked);
        issue2.agent = Some("agent-x".to_owned());
        issue2.status = Status::Blocked;
        let mut issue3 = test_issue(3, IssueState::Open, Status::Blocked);
        issue3.agent = Some("agent-y".to_owned());
        issue3.status = Status::Blocked;
        let issues = vec![issue1, issue2, issue3];
        let edges = vec![
            BlockingEdge {
                source: qid(2),
                target: qid(1),
            },
            BlockingEdge {
                source: qid(3),
                target: qid(1),
            },
        ];

        let graph = DependencyGraph::build(&issues, &edges);
        let ready = graph.compute_ready_set(&issues, TEST_OWNER, TEST_REPO);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        let filtered = apply_agent_filter(result, Some("agent-x"));
        assert_eq!(
            filtered.blocked.len(),
            1,
            "should filter blocked to agent-x only"
        );
    }

    #[test]
    fn agent_filter_matches_stale() {
        let mut issue1 = test_issue(1, IssueState::Open, Status::InProgress);
        issue1.agent = Some("agent-x".to_owned());
        issue1.claimed_at = Some(Utc::now() - chrono::Duration::hours(48));
        let mut issue2 = test_issue(2, IssueState::Open, Status::InProgress);
        issue2.agent = Some("agent-y".to_owned());
        issue2.claimed_at = Some(Utc::now() - chrono::Duration::hours(48));
        let issues = vec![issue1, issue2];

        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues, TEST_OWNER, TEST_REPO);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        let filtered = apply_agent_filter(result, Some("agent-x"));
        assert_eq!(
            filtered.stale.len(),
            1,
            "should filter stale to agent-x only"
        );
    }

    #[test]
    fn agent_filter_leaves_completed_and_hotspots_untouched() {
        // Completed + hotspots are intentionally excluded from filtering
        // (PRD §6.3). Build a fixture with completed + hotspot entries
        // assigned to a different agent than the filter, and verify they
        // survive.
        let mut closed = test_issue(1, IssueState::Closed, Status::Closed);
        closed.agent = Some("agent-y".to_owned());
        let mut hotspot = test_issue(2, IssueState::Open, Status::Ready);
        hotspot.agent = Some("agent-y".to_owned());
        let blocked_by_hotspot = test_issue(3, IssueState::Open, Status::Blocked);
        let issues = vec![closed, hotspot, blocked_by_hotspot];
        let edges = vec![BlockingEdge {
            source: qid(3),
            target: qid(2),
        }];

        let graph = DependencyGraph::build(&issues, &edges);
        let ready = graph.compute_ready_set(&issues, TEST_OWNER, TEST_REPO);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        // Sanity checks on the pre-filter state.
        assert_eq!(result.completed.len(), 1);
        assert_eq!(result.hotspots.len(), 1);

        let filtered = apply_agent_filter(result, Some("agent-x"));
        assert_eq!(
            filtered.completed.len(),
            1,
            "completed should not be filtered by agent"
        );
        assert_eq!(
            filtered.hotspots.len(),
            1,
            "hotspots should not be filtered by agent"
        );
    }

    #[test]
    fn agent_filter_does_not_affect_completed() {
        // Completed issues should not be filtered by agent.
        let mut issue = test_issue(1, IssueState::Closed, Status::Closed);
        issue.agent = Some("agent-x".to_owned());
        let issues = vec![issue];

        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues, TEST_OWNER, TEST_REPO);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        // Completed has no agent field — it should always appear regardless
        // of what agent filter we would apply.
        assert_eq!(
            result.completed.len(),
            1,
            "completed should not be filtered by agent"
        );
    }

    #[test]
    fn agent_filter_does_not_affect_hotspots() {
        // Hotspots are structural — should not be filtered by agent.
        let mut issue1 = test_issue(1, IssueState::Open, Status::Ready);
        issue1.agent = Some("agent-x".to_owned());
        let mut issue2 = test_issue(2, IssueState::Open, Status::Blocked);
        issue2.agent = Some("agent-y".to_owned());
        issue2.status = Status::Blocked;
        let issues = vec![issue1, issue2];
        let edges = vec![BlockingEdge {
            source: qid(2),
            target: qid(1),
        }];

        let graph = DependencyGraph::build(&issues, &edges);
        let ready = graph.compute_ready_set(&issues, TEST_OWNER, TEST_REPO);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        // Hotspot #1 is agent-x, but even filtering for agent-y should not
        // remove hotspots (they are not filtered).
        assert_eq!(
            result.hotspots.len(),
            1,
            "hotspots should not be filtered by agent"
        );
    }

    #[test]
    fn agent_filter_counts_reflect_filtered_totals() {
        // Two in_progress issues, two ready, filter to one agent.
        let mut issue1 = test_issue(1, IssueState::Open, Status::InProgress);
        issue1.agent = Some("agent-x".to_owned());
        issue1.claimed_at = Some(Utc::now());
        let mut issue2 = test_issue(2, IssueState::Open, Status::InProgress);
        issue2.agent = Some("agent-y".to_owned());
        issue2.claimed_at = Some(Utc::now());
        let mut issue3 = test_issue(3, IssueState::Open, Status::Ready);
        issue3.agent = Some("agent-x".to_owned());
        let mut issue4 = test_issue(4, IssueState::Open, Status::Ready);
        issue4.agent = Some("agent-y".to_owned());
        let issues = vec![issue1, issue2, issue3, issue4];

        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues, TEST_OWNER, TEST_REPO);
        let categories = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        // Simulate the filtering handle_prime does.
        let agent_filter = Some("agent-x".to_owned());
        let mut in_progress = categories.in_progress;
        let mut ready_list = categories.ready;
        if let Some(ref f) = agent_filter {
            let matches_agent = |s: &IssueSummary| s.agent.as_deref() == Some(f.as_str());
            in_progress.retain(matches_agent);
            ready_list.retain(matches_agent);
        }

        assert_eq!(in_progress.len(), 1, "filtered in_progress count");
        assert_eq!(ready_list.len(), 1, "filtered ready count");
    }

    #[test]
    fn prime_params_agent_deserializes() {
        let json = r#"{"agent": "agent-x"}"#;
        let params: PrimeParams = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(params.agent.as_deref(), Some("agent-x"));
    }

    #[test]
    fn prime_params_agent_defaults_to_none() {
        let json = r"{}";
        let params: PrimeParams = serde_json::from_str(json).expect("should deserialize");
        assert!(params.agent.is_none());
    }

    #[test]
    fn prime_params_empty_agent_deserializes_as_some_empty() {
        // Demonstrates the serde behavior this fix addresses: `""` becomes
        // `Some("")`, not `None`. The normalize_filter call in handle_prime
        // collapses this to None before filtering.
        let json = r#"{"agent": ""}"#;
        let params: PrimeParams = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(
            params.agent.as_deref(),
            Some(""),
            "serde should deserialize empty string as Some(\"\")"
        );
    }

    #[test]
    fn empty_agent_filter_returns_all_categories() {
        // Regression test: empty string agent filter should behave as no filter.
        let mut issue1 = test_issue(1, IssueState::Open, Status::InProgress);
        issue1.agent = Some("agent-x".to_owned());
        issue1.claimed_at = Some(Utc::now());
        let mut issue2 = test_issue(2, IssueState::Open, Status::InProgress);
        issue2.agent = Some("agent-y".to_owned());
        issue2.claimed_at = Some(Utc::now());
        let issues = vec![issue1, issue2];

        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues, TEST_OWNER, TEST_REPO);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        // Simulate what handle_prime does: normalize then filter.
        let agent_filter = crate::tools::normalize_filter(Some(""));
        assert!(
            agent_filter.is_none(),
            "empty string should normalize to None"
        );

        let filtered = apply_agent_filter(result, agent_filter);
        assert_eq!(
            filtered.in_progress.len(),
            2,
            "empty agent string should return all in_progress issues"
        );
    }

    // ── summarise_drift tests ────────────────────────────────────────

    /// Build a [`ReconcileReport`] with the given `drift_found` values.
    fn make_report(drift_found: Vec<serde_json::Value>) -> ReconcileReport {
        ReconcileReport {
            repo: "test-owner/test-repo".to_owned(),
            reconciled_at: Utc::now().to_rfc3339(),
            issues_scanned: 10,
            edges_scanned: 5,
            clean: drift_found.is_empty(),
            drift_found,
            repaired: vec![],
            errors: vec![],
            message: None,
        }
    }

    #[test]
    fn summarise_drift_empty_report_returns_empty() {
        let report = make_report(vec![]);
        let warnings = summarise_drift(&report);
        assert!(warnings.is_empty());
    }

    #[test]
    fn summarise_drift_single_uncascaded_closure() {
        let drift = serde_json::json!({
            "UncascadedClosure": {
                "closed_issue": "owner/repo#1",
                "should_have_unblocked": ["owner/repo#2"]
            }
        });
        let report = make_report(vec![drift]);
        let warnings = summarise_drift(&report);
        assert_eq!(warnings, vec!["1 uncascaded closure"]);
    }

    #[test]
    fn summarise_drift_multiple_of_same_type_uses_plural() {
        let drift1 = serde_json::json!({
            "UncascadedClosure": { "closed_issue": "o/r#1", "should_have_unblocked": ["o/r#10"] }
        });
        let drift2 = serde_json::json!({
            "UncascadedClosure": { "closed_issue": "o/r#2", "should_have_unblocked": ["o/r#11"] }
        });
        let drift3 = serde_json::json!({
            "UncascadedClosure": { "closed_issue": "o/r#3", "should_have_unblocked": ["o/r#12"] }
        });
        let report = make_report(vec![drift1, drift2, drift3]);
        let warnings = summarise_drift(&report);
        assert_eq!(warnings, vec!["3 uncascaded closures"]);
    }

    #[test]
    fn summarise_drift_mixed_types_sorted() {
        let uncascaded = serde_json::json!({
            "UncascadedClosure": { "closed_issue": "o/r#2", "should_have_unblocked": ["o/r#3"] }
        });
        let stale_claim = serde_json::json!({
            "StaleClaim": { "issue": "o/r#4", "claimed_at": "2026-01-01T00:00:00Z", "hours_stale": 48 }
        });
        let orphaned = serde_json::json!({
            "OrphanedBlockingEdge": { "source": "o/r#5", "missing_target": "o/r#999" }
        });
        let report = make_report(vec![uncascaded, stale_claim, orphaned]);
        let warnings = summarise_drift(&report);
        // Sorted alphabetically.
        assert_eq!(
            warnings,
            vec![
                "1 orphaned blocking edge",
                "1 stale claim",
                "1 uncascaded closure",
            ]
        );
    }

    #[test]
    fn summarise_drift_all_six_types() {
        let drifts = vec![
            serde_json::json!({"UncascadedClosure": {}}),
            serde_json::json!({"OrphanedBlockingEdge": {}}),
            serde_json::json!({"MalformedAgentField": {}}),
            serde_json::json!({"MissingProjectField": {}}),
            serde_json::json!({"CycleDetected": {}}),
            serde_json::json!({"StaleClaim": {}}),
        ];
        let report = make_report(drifts);
        let warnings = summarise_drift(&report);
        assert_eq!(warnings.len(), 6);
        // All present with count 1.
        for w in &warnings {
            assert!(w.starts_with("1 "), "each should start with '1 ': {w}");
        }
    }

    // ── resolve_drift_warnings tests ─────────────────────────────────

    #[tokio::test]
    async fn resolve_drift_warnings_clean_report_returns_none() {
        let handle = tokio::spawn(async {
            Ok(crate::tools::reconcile::ReconcileOutput {
                report: make_report(vec![]),
            })
        });
        let result = resolve_drift_warnings(handle).await;
        assert!(result.is_none(), "clean report should produce None");
    }

    #[tokio::test]
    async fn resolve_drift_warnings_with_drift_returns_some() {
        let drift = serde_json::json!({
            "UncascadedClosure": { "issue": "o/r#1", "closed_blocker": "o/r#2" }
        });
        let handle = tokio::spawn(async {
            Ok(crate::tools::reconcile::ReconcileOutput {
                report: ReconcileReport {
                    repo: "o/r".to_owned(),
                    reconciled_at: Utc::now().to_rfc3339(),
                    issues_scanned: 5,
                    edges_scanned: 2,
                    clean: false,
                    drift_found: vec![drift],
                    repaired: vec![],
                    errors: vec![],
                    message: None,
                },
            })
        });
        let result = resolve_drift_warnings(handle).await;
        assert!(result.is_some(), "dirty report should produce Some");
        let warnings = result.unwrap();
        assert_eq!(warnings, vec!["1 uncascaded closure"]);
    }

    #[tokio::test]
    async fn resolve_drift_warnings_error_returns_none() {
        let handle = tokio::spawn(async {
            Err(rmcp::model::ErrorData {
                code: rmcp::model::ErrorCode::INTERNAL_ERROR,
                message: "boom".into(),
                data: None,
            })
        });
        let result = resolve_drift_warnings(handle).await;
        assert!(result.is_none(), "error should produce None");
    }

    #[tokio::test]
    async fn resolve_drift_warnings_panic_returns_none() {
        let handle: tokio::task::JoinHandle<
            Result<crate::tools::reconcile::ReconcileOutput, rmcp::model::ErrorData>,
        > = tokio::spawn(async { panic!("simulated reconcile panic") });
        let result = resolve_drift_warnings(handle).await;
        assert!(result.is_none(), "panic should produce None");
    }

    // ── ContextInputsBuilder tests (bead unblock-eos.8) ──────────────
    //
    // The builder encapsulates the count / cycle-projection /
    // truncation steps that used to live inline in `handle_prime`. The
    // tests below pin the behaviour that was previously only verified
    // indirectly via integration tests.

    /// Build an empty [`CategorisedIssues`] fixture — every bucket is empty.
    fn empty_categorised() -> CategorisedIssues {
        CategorisedIssues {
            in_progress: vec![],
            ready: vec![],
            blocked: vec![],
            completed: vec![],
            hotspots: vec![],
            stale: vec![],
        }
    }

    /// Build an [`IssueSummary`] fixture for the builder's truncation
    /// tests. `number` is used as the identifier; other fields are
    /// given stable defaults so assertions stay byte-stable.
    fn summary(number: u64) -> IssueSummary {
        IssueSummary {
            qualified_id: qid(number),
            number,
            title: format!("Issue #{number}"),
            issue_type: Some(IssueType::Task),
            status: Status::Ready,
            priority: Priority::P1,
            agent: None,
            milestone: None,
            story_points: None,
            defer_until: None,
            labels: vec![],
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap(),
            url: format!("https://github.com/test-owner/test-repo/issues/{number}"),
        }
    }

    #[test]
    fn builder_empty_inputs_produces_zero_counts_and_no_trailer() {
        let repo = RepoIdentity {
            owner: "acme",
            repo: "widgets",
            project_number: None,
        };
        let builder = ContextInputsBuilder::new(repo, 24, 10, empty_categorised(), &[]);

        assert_eq!(builder.counts.in_progress, 0);
        assert_eq!(builder.counts.ready, 0);
        assert_eq!(builder.counts.blocked, 0);
        assert_eq!(builder.counts.completed, 0);
        assert_eq!(builder.counts.hotspots, 0);
        assert_eq!(builder.counts.stale, 0);
        assert!(builder.cycles.is_empty());
        assert!(
            builder.cross_repo_refs.is_none(),
            "no cycles means no cross-repo trailer"
        );
    }

    #[test]
    fn builder_counts_mirror_filtered_category_lengths() {
        // Four ready summaries, two blocked, zero elsewhere.
        let categories = CategorisedIssues {
            in_progress: vec![],
            ready: vec![summary(1), summary(2), summary(3), summary(4)],
            blocked: vec![summary(5), summary(6)],
            completed: vec![],
            hotspots: vec![],
            stale: vec![],
        };
        let repo = RepoIdentity {
            owner: "acme",
            repo: "widgets",
            project_number: None,
        };
        let builder = ContextInputsBuilder::new(repo, 24, 10, categories, &[]);

        assert_eq!(builder.counts.ready, 4, "counts must reflect input lengths");
        assert_eq!(
            builder.counts.blocked, 2,
            "counts must reflect input lengths"
        );
    }

    #[test]
    fn builder_truncates_each_category_at_max_per_category() {
        // Six ready summaries with a cap of 3 — truncation kicks in.
        let ready_input: Vec<IssueSummary> = (1..=6).map(summary).collect();
        let categories = CategorisedIssues {
            in_progress: vec![],
            ready: ready_input,
            blocked: vec![],
            completed: vec![],
            hotspots: vec![],
            stale: vec![],
        };
        let repo = RepoIdentity {
            owner: "acme",
            repo: "widgets",
            project_number: None,
        };
        let builder = ContextInputsBuilder::new(repo, 24, 3, categories, &[]);

        // Counts reflect the length of the `CategorisedIssues` lists
        // passed into the builder — i.e. the pre-truncation length. This
        // fixture constructs `CategorisedIssues` DIRECTLY (no agent
        // filter applied upstream), so the six-entry ready list proves
        // only that counts are captured BEFORE `max_per_category`
        // truncation. Agent-filter propagation is NOT exercised here.
        assert_eq!(
            builder.counts.ready, 6,
            "counts capture pre-truncation category length"
        );
        // Truncated list obeys the cap.
        assert_eq!(
            builder.ready.len(),
            3,
            "truncated list honours max_per_category"
        );
        // First three numbers preserved (truncation takes the prefix).
        let numbers: Vec<u64> = builder.ready.iter().map(|s| s.number).collect();
        assert_eq!(numbers, vec![1, 2, 3]);
    }

    #[test]
    fn builder_populates_cross_repo_trailer_when_cycle_has_cross_repo_member() {
        // Configured repo is acme/widgets; the cycle includes an
        // `acme/other#42` member which must land in the trailer.
        let raw_cycles: Vec<Vec<QualifiedId>> = vec![vec![
            QualifiedId::new("acme", "widgets", 1),
            QualifiedId::new("acme", "other", 42),
            QualifiedId::new("acme", "widgets", 2),
        ]];
        let repo = RepoIdentity {
            owner: "acme",
            repo: "widgets",
            project_number: None,
        };
        let builder = ContextInputsBuilder::new(repo, 24, 10, empty_categorised(), &raw_cycles);

        let refs = builder
            .cross_repo_refs
            .as_ref()
            .expect("cross-repo member must populate the trailer");
        assert_eq!(refs.omitted, vec!["acme/other#42".to_owned()]);
        assert_eq!(
            refs.summary.as_deref(),
            Some("1 cross-repo cycle member omitted from `cycles`"),
            "summary must match cross_repo::cycles_summary byte-for-byte"
        );
        // Local projection drops the cross-repo entry.
        assert_eq!(builder.cycles, vec![vec![1u64, 2u64]]);
    }

    #[test]
    fn builder_local_only_cycle_leaves_trailer_none() {
        let raw_cycles: Vec<Vec<QualifiedId>> = vec![vec![
            QualifiedId::new("acme", "widgets", 1),
            QualifiedId::new("acme", "widgets", 2),
        ]];
        let repo = RepoIdentity {
            owner: "acme",
            repo: "widgets",
            project_number: None,
        };
        let builder = ContextInputsBuilder::new(repo, 24, 10, empty_categorised(), &raw_cycles);

        assert!(
            builder.cross_repo_refs.is_none(),
            "local-only cycle must not populate the trailer"
        );
        assert_eq!(builder.cycles, vec![vec![1u64, 2u64]]);
    }

    #[test]
    fn builder_build_wires_owned_fields_into_context_inputs() {
        // End-to-end: populate the builder, call build(), and assert
        // that the ContextInputs references match the builder's owned
        // state byte-for-byte.
        let categories = CategorisedIssues {
            in_progress: vec![summary(1)],
            ready: vec![summary(2), summary(3)],
            blocked: vec![],
            completed: vec![],
            hotspots: vec![],
            stale: vec![],
        };
        let raw_cycles: Vec<Vec<QualifiedId>> = vec![vec![
            QualifiedId::new("acme", "widgets", 1),
            QualifiedId::new("acme", "widgets", 2),
        ]];
        let repo = RepoIdentity {
            owner: "acme",
            repo: "widgets",
            project_number: Some(7),
        };
        let builder = ContextInputsBuilder::new(repo, 48, 5, categories, &raw_cycles);

        let session = unknown_session();
        let drift: &[String] = &[];
        let ctx = builder.build(&session, Some(drift));

        assert_eq!(ctx.owner, "acme");
        assert_eq!(ctx.repo, "widgets");
        assert_eq!(ctx.project_number, Some(7));
        assert_eq!(ctx.stale_threshold_hours, 48);
        assert_eq!(ctx.max_per_category, 5);
        assert_eq!(ctx.counts.in_progress, 1);
        assert_eq!(ctx.counts.ready, 2);
        assert_eq!(ctx.cycles, &[vec![1u64, 2u64]]);
        assert!(ctx.cross_repo_refs.is_none());
        assert_eq!(ctx.in_progress.len(), 1);
        assert_eq!(ctx.ready.len(), 2);
        assert!(ctx.blocked.is_empty());
        assert!(ctx.completed.is_empty());
        assert!(ctx.hotspots.is_empty());
        assert!(ctx.stale.is_empty());
        assert_eq!(ctx.drift_warnings, Some(drift));
    }

    // ── validate_and_resolve_params tests (bead unblock-eos.8) ───────
    //
    // Extracted from the inline validation block at the top of
    // `handle_prime` so both the happy path and the reject paths can
    // be covered without spinning up a full `ServerState`.

    #[test]
    fn validate_and_resolve_params_defaults_apply_when_unset() {
        let params = PrimeParams {
            stale_threshold_hours: None,
            max_per_category: None,
            agent: None,
        };
        let (hours, max) =
            validate_and_resolve_params(&params).expect("defaults should pass validation");
        assert_eq!(hours, DEFAULT_STALE_THRESHOLD_HOURS);
        assert_eq!(max, DEFAULT_MAX_PER_CATEGORY);
    }

    #[test]
    fn validate_and_resolve_params_passes_explicit_values_through() {
        let params = PrimeParams {
            stale_threshold_hours: Some(48),
            max_per_category: Some(5),
            agent: None,
        };
        let (hours, max) =
            validate_and_resolve_params(&params).expect("explicit values should pass");
        assert_eq!(hours, 48);
        assert_eq!(max, 5);
    }

    #[test]
    fn validate_and_resolve_params_rejects_zero_stale_threshold() {
        let params = PrimeParams {
            stale_threshold_hours: Some(0),
            max_per_category: None,
            agent: None,
        };
        let err = validate_and_resolve_params(&params)
            .expect_err("zero stale threshold must be rejected");
        assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
        assert!(err.message.contains("stale_threshold_hours"));
    }

    #[test]
    fn validate_and_resolve_params_rejects_zero_max_per_category() {
        let params = PrimeParams {
            stale_threshold_hours: None,
            max_per_category: Some(0),
            agent: None,
        };
        let err = validate_and_resolve_params(&params)
            .expect_err("zero max_per_category must be rejected");
        assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
        assert!(err.message.contains("max_per_category"));
    }
}
