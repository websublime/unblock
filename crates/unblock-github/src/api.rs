//! `GitHubApi` trait abstraction over [`crate::client::GitHubClient`].
//!
//! This trait mirrors the full public async surface of [`crate::client::GitHubClient`] plus
//! its synchronous accessors, so that call sites can depend on
//! `Arc<dyn GitHubApi>` instead of a concrete struct. This enables:
//!
//! - Dependency injection in tests (see sibling `MockGitHubClient`).
//! - Invocation counters for assertions on retry / dedup logic.
//! - Swapping transport backends without touching callers.
//!
//! The trait is intentionally scoped to the methods the rest of the workspace
//! already calls against `GitHubClient`. It deliberately does **not** expose
//! `fn http(&self) -> &reqwest::Client` — leaking `reqwest` through a trait
//! object would force every mock implementation to materialise a real HTTP
//! client. An audit of the workspace confirmed no external (outside
//! `unblock-github`) call sites use `client.http()` directly, so no HTTP
//! wrapper methods are required on the trait surface in this iteration.
//!
//! This module introduces the trait only; call-site migration to
//! `Arc<dyn GitHubApi>` happens in follow-up beads.

use async_trait::async_trait;

use unblock_core::types::{BlockingEdge, Issue, IssueRef, IssueSummary};

use crate::client::GitHubClient;
use crate::errors::Error;
use crate::mutations::{CreateIssueParams, Milestone};
use crate::projects::{
    CreateViewParams, CreatedProject, FieldValue, OwnerProject, OwnerType, ProjectFieldIds,
    ProjectInfo, ProjectView, RestField, SetupReport, SetupStatus,
};

/// Trait abstraction over [`GitHubClient`].
///
/// Covers every async method currently exposed on `GitHubClient`, plus the
/// synchronous accessors callers rely on (`owner`, `repo`, `project_number`,
/// `rest_url`, `graphql_url`, `api_base_url`).
///
/// Implementations must be `Send + Sync` so they can be stored in
/// `Arc<dyn GitHubApi>` and shared across tokio tasks.
#[async_trait]
pub trait GitHubApi: Send + Sync {
    // ── Diagnostics ───────────────────────────────────────────────────

    /// Returns a human-readable label identifying the concrete implementation
    /// behind the trait object, for use in [`Debug`](std::fmt::Debug) output
    /// and structured logs.
    ///
    /// Because [`Debug`](std::fmt::Debug) is intentionally **not** a
    /// supertrait of `GitHubApi`, callers cannot forward formatting through
    /// the trait object. This method gives them a stable hook for emitting a
    /// meaningful identifier (e.g. `unblock_github::client::GitHubClient` vs.
    /// `unblock_github::mock::MockGitHubClient`) without forcing every
    /// implementor to derive `Debug` over its private state.
    ///
    /// The default implementation returns [`std::any::type_name::<Self>()`],
    /// which resolves to the concrete impl type at each call site (the trait
    /// object's vtable carries the right monomorphisation). The exact output
    /// of `type_name` is not guaranteed stable across compiler versions, so
    /// the returned string is suitable for diagnostics only — never parse it.
    fn debug_label(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    // ── Sync accessors ────────────────────────────────────────────────

    /// Returns the repository owner (e.g. `websublime`).
    fn owner(&self) -> &str;

    /// Returns the repository name (e.g. `unblock`).
    fn repo(&self) -> &str;

    /// Returns the configured GitHub Projects V2 number, if any.
    fn project_number(&self) -> Option<u64>;

    /// Returns the GitHub API base URL (e.g. `https://api.github.com`).
    fn api_base_url(&self) -> &str;

    /// Builds a REST API URL from a path suffix.
    fn rest_url(&self, path: &str) -> String;

    /// Builds the GraphQL endpoint URL (handles GHE `/api/v3` rewriting).
    fn graphql_url(&self) -> String;

    // ── Field id cache ────────────────────────────────────────────────

    /// Returns a clone of the cached [`ProjectFieldIds`], if set.
    async fn field_ids(&self) -> Option<ProjectFieldIds>;

    /// Caches the resolved [`ProjectFieldIds`] on this client.
    async fn set_field_ids(&self, ids: ProjectFieldIds);

    // ── Project resolution and setup ──────────────────────────────────

    /// Resolves the configured project to its node id and metadata.
    async fn resolve_project_info(&self) -> Result<ProjectInfo, Error>;

    /// Ensures the 7 required Projects V2 fields exist and caches their ids.
    async fn setup_fields(&self, project_id: &str) -> Result<SetupReport, Error>;

    /// Queries the current setup status of the project fields.
    async fn query_setup_status(&self, project_id: &str) -> Result<SetupStatus, Error>;

    /// Updates a single Projects V2 field value on an item.
    async fn update_field(
        &self,
        project_id: &str,
        item_id: &str,
        field_id: &str,
        value: &FieldValue,
    ) -> Result<(), Error>;

    /// Detects whether the configured owner is a user or organization.
    async fn detect_owner_type(&self) -> Result<OwnerType, Error>;

    /// Lists the REST-exposed Projects V2 fields for the configured project.
    async fn list_rest_fields(&self, owner_type: OwnerType) -> Result<Vec<RestField>, Error>;

    /// Creates a new Projects V2 view.
    async fn create_view(
        &self,
        owner_type: OwnerType,
        params: &CreateViewParams,
    ) -> Result<ProjectView, Error>;

    /// Lists existing Projects V2 views for the configured project.
    async fn list_views(&self, owner_type: OwnerType) -> Result<Vec<ProjectView>, Error>;

    /// Resolves the owner (user/org) to its GraphQL node id.
    async fn resolve_owner_node_id(&self, owner_type: OwnerType) -> Result<String, Error>;

    /// Lists all Projects V2 for the configured owner.
    async fn list_owner_projects(&self, owner_type: OwnerType) -> Result<Vec<OwnerProject>, Error>;

    /// Creates a new Projects V2 under the given owner node id.
    async fn create_project(
        &self,
        owner_node_id: &str,
        title: &str,
    ) -> Result<CreatedProject, Error>;

    // ── GraphQL reads ─────────────────────────────────────────────────

    /// Fetches a single issue by number.
    async fn fetch_issue(&self, number: u64) -> Result<Issue, Error>;

    /// Fetches a single issue identified by an [`IssueRef`].
    ///
    /// Supports both `Local` (configured repository) and `CrossRepo`
    /// (arbitrary `owner/repo`) references.
    async fn fetch_issue_ref(&self, issue_ref: &IssueRef) -> Result<Issue, Error>;

    /// Fetches the full set of project issues and blocking edges in one pass.
    async fn fetch_graph_data(&self) -> Result<(Vec<Issue>, Vec<BlockingEdge>), Error>;

    // ── Mutations ─────────────────────────────────────────────────────

    /// Creates a new issue.
    async fn create_issue(&self, params: CreateIssueParams) -> Result<Issue, Error>;

    /// Closes an issue with an optional state reason.
    async fn close_issue(&self, number: u64, reason: Option<String>) -> Result<(), Error>;

    /// Reopens a previously closed issue.
    async fn reopen_issue(&self, number: u64) -> Result<(), Error>;

    /// Full-text search of issues via GitHub's REST Search API.
    ///
    /// Scoped to the configured `owner/repo`. Bypasses any caller-side cache.
    /// Returns lightweight [`IssueSummary`] entries; Projects V2 custom fields
    /// are populated with defaults since the Search API does not include them.
    async fn search_issues(
        &self,
        query: &str,
        limit: Option<u32>,
    ) -> Result<Vec<IssueSummary>, Error>;

    /// Adds a comment to an issue; returns the comment node id.
    async fn add_comment(&self, number: u64, body: String) -> Result<String, Error>;

    /// Replaces the body of an issue.
    async fn update_issue_body(&self, number: u64, body: String) -> Result<(), Error>;

    /// Adds labels to an issue.
    async fn add_labels_to_issue(&self, number: u64, labels: Vec<String>) -> Result<(), Error>;

    /// Removes a single label from an issue.
    async fn remove_label_from_issue(&self, number: u64, label: &str) -> Result<(), Error>;

    /// Adds assignees to an issue.
    async fn add_assignees_to_issue(
        &self,
        number: u64,
        assignees: Vec<String>,
    ) -> Result<(), Error>;

    /// Removes assignees from an issue.
    async fn remove_assignees_from_issue(
        &self,
        number: u64,
        assignees: Vec<String>,
    ) -> Result<(), Error>;

    /// Lists repository milestones.
    async fn list_milestones(&self) -> Result<Vec<Milestone>, Error>;

    /// Sets or clears an issue's milestone.
    async fn update_issue_milestone(
        &self,
        number: u64,
        milestone_number: Option<u64>,
    ) -> Result<(), Error>;

    /// Adds a blocking edge: `issue_number` is blocked by `blocked_by_number`.
    async fn add_blocked_by(&self, issue_number: u64, blocked_by_number: u64) -> Result<(), Error>;

    /// Removes a blocking edge.
    async fn remove_blocked_by(
        &self,
        issue_number: u64,
        blocked_by_number: u64,
    ) -> Result<(), Error>;

    /// Links a child sub-issue under a parent issue.
    async fn add_sub_issue(&self, parent_number: u64, child_number: u64) -> Result<(), Error>;

    /// Resolves an [`IssueRef`] to its GraphQL node id.
    async fn resolve_issue_ref(&self, issue_ref: &IssueRef) -> Result<String, Error>;

    /// Returns the Projects V2 item id for an issue node id within a project.
    async fn get_project_item_id(
        &self,
        issue_node_id: &str,
        project_id: &str,
    ) -> Result<String, Error>;

    /// Ensures the given labels exist on the repository, creating any missing.
    async fn ensure_labels(&self, labels: &[String]) -> Result<(), Error>;

    /// Adds a blocking edge using an [`IssueRef`] as the blocker.
    async fn add_blocked_by_ref(&self, issue_number: u64, blocker: &IssueRef) -> Result<(), Error>;

    /// Adds a blocking edge using [`IssueRef`] for both endpoints.
    ///
    /// Enables cross-repo `source` issues (e.g. a dependency where the
    /// blocked issue lives outside the configured project). For `Local`
    /// sources this delegates to
    /// [`add_blocked_by_ref`](Self::add_blocked_by_ref) so the existing
    /// local-source duplicate pre-check is preserved.
    ///
    /// See spec §8.4 (`depends` tool contract) and §5 cross-repo scope table.
    async fn add_blocked_by_refs(&self, source: &IssueRef, blocker: &IssueRef)
    -> Result<(), Error>;
}

// ── Blanket delegation impl on GitHubClient ──────────────────────────
//
// Every method simply forwards to the inherent method with the same name and
// signature. This keeps `GitHubClient` usable both directly (existing call
// sites) and via `Arc<dyn GitHubApi>` (future call sites).

#[async_trait]
impl GitHubApi for GitHubClient {
    fn owner(&self) -> &str {
        GitHubClient::owner(self)
    }

    fn repo(&self) -> &str {
        GitHubClient::repo(self)
    }

    fn project_number(&self) -> Option<u64> {
        GitHubClient::project_number(self)
    }

    fn api_base_url(&self) -> &str {
        GitHubClient::api_base_url(self)
    }

    fn rest_url(&self, path: &str) -> String {
        GitHubClient::rest_url(self, path)
    }

    fn graphql_url(&self) -> String {
        GitHubClient::graphql_url(self)
    }

    async fn field_ids(&self) -> Option<ProjectFieldIds> {
        GitHubClient::field_ids(self).await
    }

    async fn set_field_ids(&self, ids: ProjectFieldIds) {
        GitHubClient::set_field_ids(self, ids).await;
    }

    async fn resolve_project_info(&self) -> Result<ProjectInfo, Error> {
        GitHubClient::resolve_project_info(self).await
    }

    async fn setup_fields(&self, project_id: &str) -> Result<SetupReport, Error> {
        GitHubClient::setup_fields(self, project_id).await
    }

    async fn query_setup_status(&self, project_id: &str) -> Result<SetupStatus, Error> {
        GitHubClient::query_setup_status(self, project_id).await
    }

    async fn update_field(
        &self,
        project_id: &str,
        item_id: &str,
        field_id: &str,
        value: &FieldValue,
    ) -> Result<(), Error> {
        GitHubClient::update_field(self, project_id, item_id, field_id, value).await
    }

    async fn detect_owner_type(&self) -> Result<OwnerType, Error> {
        GitHubClient::detect_owner_type(self).await
    }

    async fn list_rest_fields(&self, owner_type: OwnerType) -> Result<Vec<RestField>, Error> {
        GitHubClient::list_rest_fields(self, owner_type).await
    }

    async fn create_view(
        &self,
        owner_type: OwnerType,
        params: &CreateViewParams,
    ) -> Result<ProjectView, Error> {
        GitHubClient::create_view(self, owner_type, params).await
    }

    async fn list_views(&self, owner_type: OwnerType) -> Result<Vec<ProjectView>, Error> {
        GitHubClient::list_views(self, owner_type).await
    }

    async fn resolve_owner_node_id(&self, owner_type: OwnerType) -> Result<String, Error> {
        GitHubClient::resolve_owner_node_id(self, owner_type).await
    }

    async fn list_owner_projects(&self, owner_type: OwnerType) -> Result<Vec<OwnerProject>, Error> {
        GitHubClient::list_owner_projects(self, owner_type).await
    }

    async fn create_project(
        &self,
        owner_node_id: &str,
        title: &str,
    ) -> Result<CreatedProject, Error> {
        GitHubClient::create_project(self, owner_node_id, title).await
    }

    async fn fetch_issue(&self, number: u64) -> Result<Issue, Error> {
        GitHubClient::fetch_issue(self, number).await
    }

    async fn fetch_issue_ref(&self, issue_ref: &IssueRef) -> Result<Issue, Error> {
        GitHubClient::fetch_issue_ref(self, issue_ref).await
    }

    async fn fetch_graph_data(&self) -> Result<(Vec<Issue>, Vec<BlockingEdge>), Error> {
        GitHubClient::fetch_graph_data(self).await
    }

    async fn create_issue(&self, params: CreateIssueParams) -> Result<Issue, Error> {
        GitHubClient::create_issue(self, params).await
    }

    async fn close_issue(&self, number: u64, reason: Option<String>) -> Result<(), Error> {
        GitHubClient::close_issue(self, number, reason).await
    }

    async fn reopen_issue(&self, number: u64) -> Result<(), Error> {
        GitHubClient::reopen_issue(self, number).await
    }

    async fn search_issues(
        &self,
        query: &str,
        limit: Option<u32>,
    ) -> Result<Vec<IssueSummary>, Error> {
        GitHubClient::search_issues(self, query, limit).await
    }

    async fn add_comment(&self, number: u64, body: String) -> Result<String, Error> {
        GitHubClient::add_comment(self, number, body).await
    }

    async fn update_issue_body(&self, number: u64, body: String) -> Result<(), Error> {
        GitHubClient::update_issue_body(self, number, body).await
    }

    async fn add_labels_to_issue(&self, number: u64, labels: Vec<String>) -> Result<(), Error> {
        GitHubClient::add_labels_to_issue(self, number, labels).await
    }

    async fn remove_label_from_issue(&self, number: u64, label: &str) -> Result<(), Error> {
        GitHubClient::remove_label_from_issue(self, number, label).await
    }

    async fn add_assignees_to_issue(
        &self,
        number: u64,
        assignees: Vec<String>,
    ) -> Result<(), Error> {
        GitHubClient::add_assignees_to_issue(self, number, assignees).await
    }

    async fn remove_assignees_from_issue(
        &self,
        number: u64,
        assignees: Vec<String>,
    ) -> Result<(), Error> {
        GitHubClient::remove_assignees_from_issue(self, number, assignees).await
    }

    async fn list_milestones(&self) -> Result<Vec<Milestone>, Error> {
        GitHubClient::list_milestones(self).await
    }

    async fn update_issue_milestone(
        &self,
        number: u64,
        milestone_number: Option<u64>,
    ) -> Result<(), Error> {
        GitHubClient::update_issue_milestone(self, number, milestone_number).await
    }

    async fn add_blocked_by(&self, issue_number: u64, blocked_by_number: u64) -> Result<(), Error> {
        GitHubClient::add_blocked_by(self, issue_number, blocked_by_number).await
    }

    async fn remove_blocked_by(
        &self,
        issue_number: u64,
        blocked_by_number: u64,
    ) -> Result<(), Error> {
        GitHubClient::remove_blocked_by(self, issue_number, blocked_by_number).await
    }

    async fn add_sub_issue(&self, parent_number: u64, child_number: u64) -> Result<(), Error> {
        GitHubClient::add_sub_issue(self, parent_number, child_number).await
    }

    async fn resolve_issue_ref(&self, issue_ref: &IssueRef) -> Result<String, Error> {
        GitHubClient::resolve_issue_ref(self, issue_ref).await
    }

    async fn get_project_item_id(
        &self,
        issue_node_id: &str,
        project_id: &str,
    ) -> Result<String, Error> {
        GitHubClient::get_project_item_id(self, issue_node_id, project_id).await
    }

    async fn ensure_labels(&self, labels: &[String]) -> Result<(), Error> {
        GitHubClient::ensure_labels(self, labels).await
    }

    async fn add_blocked_by_ref(&self, issue_number: u64, blocker: &IssueRef) -> Result<(), Error> {
        GitHubClient::add_blocked_by_ref(self, issue_number, blocker).await
    }

    async fn add_blocked_by_refs(
        &self,
        source: &IssueRef,
        blocker: &IssueRef,
    ) -> Result<(), Error> {
        GitHubClient::add_blocked_by_refs(self, source, blocker).await
    }
}
