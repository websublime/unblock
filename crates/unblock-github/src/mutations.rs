//! REST and GraphQL mutations.
//!
//! - `create_issue()` — REST POST
//! - `close_issue()` — REST PATCH
//! - `reopen_issue()` — REST PATCH (reopens a closed issue)
//! - `update_issue_body()` — REST PATCH (update body only)
//! - `add_labels_to_issue()` — REST POST (add labels)
//! - `remove_label_from_issue()` — DELETE (remove single label)
//! - `add_assignees_to_issue()` — REST POST (add assignees)
//! - `remove_assignees_from_issue()` — REST DELETE (remove assignees)
//! - `list_milestones()` — REST GET (list milestones)
//! - `update_issue_milestone()` — REST PATCH (set milestone)
//! - `add_comment()` — REST POST
//! - `add_blocked_by()` — GraphQL mutation (blocking relationship)
//! - `remove_blocked_by()` — GraphQL mutation (blocking relationship)
//! - `add_sub_issue()` — GraphQL mutation (sub-issue relationship, preview)
//! - `search_issues()` — REST GET /search/issues (read-only, bypasses cache)
//!
//! REST mutations use the GitHub REST API for simplicity. Blocking and sub-issue
//! mutations use GraphQL because these features are only available via GraphQL.
//! Error handling follows the same pattern: 429 → `RateLimited`, 404 →
//! `IssueNotFound`, other non-2xx → `GitHubApi`.

use std::fmt::Write as _;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use snafu::ResultExt as _;
use tracing::{debug, instrument, warn};
use unblock_core::types::{Issue, IssueSummary, IssueType, Priority, QualifiedId, Status};

use crate::client::GitHubClient;
use crate::errors::{self, Error};
use crate::graphql::{check_rest_response, classify_cross_repo_fetch};

/// Parameters for creating a new GitHub issue.
///
/// Only `title` is required; all other fields are optional and default to
/// empty/None when omitted.
#[derive(Debug, Clone)]
pub struct CreateIssueParams {
    /// Issue title (required).
    pub title: String,
    /// Issue body in markdown.
    pub body: Option<String>,
    /// Labels to attach to the issue.
    pub labels: Vec<String>,
    /// Milestone number to assign (the numeric ID, not the title).
    pub milestone: Option<u64>,
    /// GitHub usernames to assign.
    pub assignees: Vec<String>,
}

/// Minimal response shape from the REST POST /issues endpoint.
///
/// Captures `number` for re-fetch and `node_id` for project mutations,
/// avoiding an extra REST GET when adding the issue to a project.
#[derive(Debug, Deserialize)]
struct CreateIssueResponse {
    number: u64,
    node_id: String,
}

/// Minimal response shape from the REST POST /issues/{n}/comments endpoint.
#[derive(Debug, Deserialize)]
struct CreateCommentResponse {
    html_url: String,
}

/// Request body for creating a GitHub issue via REST API.
#[derive(Debug, Serialize)]
struct CreateIssueBody {
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    labels: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    milestone: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    assignees: Vec<String>,
}

/// Request body for closing a GitHub issue via REST API.
#[derive(Debug, Serialize)]
struct CloseIssueBody {
    state: &'static str,
    state_reason: &'static str,
}

/// Request body for reopening a GitHub issue via REST API.
///
/// Mirrors [`CloseIssueBody`] with `state = "open"` and
/// `state_reason = "reopened"` per GitHub REST API semantics.
#[derive(Debug, Serialize)]
struct ReopenIssueBody {
    state: &'static str,
    state_reason: &'static str,
}

/// Response envelope for the `GET /search/issues` endpoint.
///
/// Captures only the `items` array; the `total_count` and `incomplete_results`
/// fields from the GitHub API response are intentionally ignored since
/// [`SearchParams::limit`] caps the page size we request and we return
/// `issues.len()` as the caller-visible count.
#[derive(Debug, Deserialize)]
struct SearchIssuesResponse {
    items: Vec<SearchIssueItem>,
}

/// A single item returned by `GET /search/issues`.
///
/// REST search returns a reduced issue shape — crucially it does **not**
/// include Projects V2 custom field values (Status, Priority, Agent, etc.).
/// Fields that are not present on the search response are mapped to their
/// defaults when converted to [`IssueSummary`] (see
/// [`GitHubClient::search_issues`] for the contract).
#[derive(Debug, Deserialize)]
struct SearchIssueItem {
    number: u64,
    title: String,
    #[serde(default)]
    labels: Vec<SearchLabel>,
    #[serde(default)]
    milestone: Option<SearchMilestone>,
    html_url: String,
    created_at: DateTime<Utc>,
}

/// Minimal label shape from the REST search response.
#[derive(Debug, Deserialize)]
struct SearchLabel {
    name: String,
}

/// Minimal milestone shape from the REST search response.
#[derive(Debug, Deserialize)]
struct SearchMilestone {
    title: String,
}

/// Request body for updating an issue body via REST PATCH.
#[derive(Debug, Serialize)]
struct UpdateIssueBodyRequest {
    body: String,
}

/// Request body for adding labels to a GitHub issue via REST POST.
#[derive(Debug, Serialize)]
struct AddLabelsBody {
    labels: Vec<String>,
}

/// Request body for adding or removing assignees on a GitHub issue via REST.
///
/// Used by both `POST /repos/{o}/{r}/issues/{n}/assignees` (add) and
/// `DELETE /repos/{o}/{r}/issues/{n}/assignees` (remove).
#[derive(Debug, Serialize)]
struct AssigneesBody {
    assignees: Vec<String>,
}

/// Request body for setting a milestone on an issue via REST PATCH.
#[derive(Debug, Serialize)]
struct UpdateMilestoneBody {
    milestone: Option<u64>,
}

/// A GitHub milestone as returned by the REST API.
#[derive(Debug, Deserialize)]
pub struct Milestone {
    /// The milestone number (not the node ID).
    pub number: u64,
    /// The milestone title.
    pub title: String,
}

/// Request body for adding a comment to a GitHub issue via REST API.
#[derive(Debug, Serialize)]
struct AddCommentBody {
    body: String,
}

impl GitHubClient {
    /// Creates a new GitHub issue.
    ///
    /// Sends a REST POST to `/repos/{owner}/{repo}/issues` with the given
    /// parameters. After creation, re-fetches the issue via [`fetch_issue()`]
    /// to return the full field set including Projects V2 fields.
    ///
    /// If a project is configured, the newly created issue is added to the
    /// project on a best-effort basis (failure is logged as a warning, not
    /// propagated).
    ///
    /// # Errors
    ///
    /// Returns [`Error::GitHubApi`] for non-2xx responses.
    /// Returns [`Error::RateLimited`] for HTTP 429 responses.
    /// Returns [`Error::GitHubUnavailable`] for network failures.
    ///
    /// [`fetch_issue()`]: crate::graphql
    #[instrument(skip(self, params), fields(owner = %self.owner(), repo = %self.repo(), title = %params.title))]
    pub async fn create_issue(&self, params: CreateIssueParams) -> Result<Issue, Error> {
        let url = self.rest_url(&format!("/repos/{}/{}/issues", self.owner(), self.repo()));

        let request_body = CreateIssueBody {
            title: params.title,
            body: params.body,
            labels: params.labels,
            milestone: params.milestone,
            assignees: params.assignees,
        };

        let response = self
            .http()
            .post(&url)
            .json(&request_body)
            .send()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        let response = check_rest_response(response).await?;

        let created: CreateIssueResponse = response
            .json()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        let number = created.number;
        let node_id = created.node_id;

        // Best-effort: add to project if configured.
        if let Some(project_number) = self.project_number() {
            match self.add_issue_to_project(&node_id, project_number).await {
                Ok(item_id) => {
                    debug!(number, item_id = %item_id, "Added issue to project");
                }
                Err(e) => {
                    warn!(
                        number,
                        project_number,
                        error = %e,
                        "Failed to add issue to project (best-effort)"
                    );
                }
            }
        }

        // Re-fetch via GraphQL for full field set.
        self.fetch_issue(number).await
    }

    /// Closes a GitHub issue.
    ///
    /// If `reason` is provided, adds a comment with the reason text before
    /// closing the issue. Sends a REST PATCH to
    /// `/repos/{owner}/{repo}/issues/{number}` with `state: "closed"`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Domain`] with [`DomainError::IssueNotFound`] if the
    /// issue does not exist (HTTP 404).
    /// Returns [`Error::RateLimited`] for HTTP 429 responses.
    /// Returns [`Error::GitHubApi`] for other non-2xx responses.
    ///
    /// [`DomainError::IssueNotFound`]: unblock_core::errors::DomainError::IssueNotFound
    #[instrument(skip(self), fields(owner = %self.owner(), repo = %self.repo()))]
    pub async fn close_issue(&self, number: u64, reason: Option<String>) -> Result<(), Error> {
        // If reason is provided and non-empty, add a "Closed: {reason}" comment first.
        // Per ARCH §10.3 / PRD §6.1 step 4, reason comments must be prefixed with
        // "Closed: ". An empty or whitespace-only reason string is treated the same
        // as None — no comment is posted to avoid timeline noise. This mirrors the
        // MCP handler layer's trim-based filter so direct library callers get the
        // same defense-in-depth guarantee.
        if let Some(reason_text) = reason
            && !reason_text.trim().is_empty()
        {
            let body = format!("Closed: {reason_text}");
            self.add_comment(number, body).await?;
        }

        let url = self.rest_url(&format!(
            "/repos/{}/{}/issues/{number}",
            self.owner(),
            self.repo()
        ));

        let request_body = CloseIssueBody {
            state: "closed",
            state_reason: "completed",
        };

        let response = self
            .http()
            .patch(&url)
            .json(&request_body)
            .send()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        if response.status().as_u16() == 404 {
            return Err(unblock_core::errors::IssueNotFoundSnafu { number }
                .build()
                .into());
        }

        check_rest_response(response).await?;

        Ok(())
    }

    /// Adds a comment to a GitHub issue in the configured repository.
    ///
    /// Thin convenience wrapper over
    /// [`add_comment_in_repo`](Self::add_comment_in_repo) — delegates to
    /// the `(self.owner(), self.repo())` tuple so single-repo callers that
    /// only speak in local issue numbers keep a single codepath.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Domain`] with [`DomainError::IssueNotFound`] if the
    /// issue does not exist (HTTP 404).
    /// Returns [`Error::RateLimited`] for HTTP 429 responses.
    /// Returns [`Error::GitHubApi`] for other non-2xx responses.
    ///
    /// [`DomainError::IssueNotFound`]: unblock_core::errors::DomainError::IssueNotFound
    #[instrument(skip(self, body), fields(owner = %self.owner(), repo = %self.repo()))]
    pub async fn add_comment(&self, number: u64, body: String) -> Result<String, Error> {
        self.add_comment_in_repo(self.owner(), self.repo(), number, body)
            .await
    }

    /// Adds a comment to a GitHub issue in the specified `owner/repo`.
    ///
    /// Sends a REST POST to `/repos/{owner}/{repo}/issues/{number}/comments`.
    /// Returns the HTML URL of the created comment. Unlike
    /// [`add_comment`](Self::add_comment), the target repository is
    /// supplied by the caller instead of being read from `self` — this
    /// is what backs [`add_comment_ref`](Self::add_comment_ref) for
    /// cross-repo cascade side effects (see SPEC §8.2 step 6 and §11.4
    /// row 4).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Domain`] with [`DomainError::IssueNotFound`] if the
    /// issue does not exist in the target repository (HTTP 404).
    /// Returns [`Error::RateLimited`] for HTTP 429 responses.
    /// Returns [`Error::GitHubApi`] for other non-2xx responses (notably
    /// HTTP 403 when the configured token lacks write access on a foreign
    /// repository — the common failure mode for cross-repo cascades).
    ///
    /// [`DomainError::IssueNotFound`]: unblock_core::errors::DomainError::IssueNotFound
    #[instrument(skip(self, body))]
    pub async fn add_comment_in_repo(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        body: String,
    ) -> Result<String, Error> {
        let url = self.rest_url(&format!("/repos/{owner}/{repo}/issues/{number}/comments"));

        let request_body = AddCommentBody { body };

        let response = self
            .http()
            .post(&url)
            .json(&request_body)
            .send()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        if response.status().as_u16() == 404 {
            return Err(unblock_core::errors::IssueNotFoundSnafu { number }
                .build()
                .into());
        }

        let response = check_rest_response(response).await?;

        let comment: CreateCommentResponse = response
            .json()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        Ok(comment.html_url)
    }

    /// Adds a comment to an issue identified by an [`IssueRef`].
    ///
    /// Dispatches on the ref variant:
    /// - [`Local`](unblock_core::types::IssueRef::Local) delegates to
    ///   [`add_comment`](Self::add_comment) (posts against the configured
    ///   repository), preserving the existing single-repo codepath.
    /// - [`CrossRepo`](unblock_core::types::IssueRef::CrossRepo) delegates
    ///   to [`add_comment_in_repo`](Self::add_comment_in_repo) with the
    ///   ref's `(owner, repo, number)` so the comment lands on the
    ///   correct foreign repository.
    ///
    /// Mirrors [`fetch_issue_ref`](Self::fetch_issue_ref) and is what
    /// the `close` Phase-3 cascade loop (SPEC §8.2 step 6) uses to honor
    /// the cross-repo cascade contract (§11.4 row 4 / §5.6).
    ///
    /// # Errors
    ///
    /// Returns the same errors as the underlying
    /// [`add_comment`](Self::add_comment) /
    /// [`add_comment_in_repo`](Self::add_comment_in_repo) paths.
    ///
    /// [`IssueRef`]: unblock_core::types::IssueRef
    #[instrument(skip(self, body))]
    pub async fn add_comment_ref(
        &self,
        issue_ref: &unblock_core::types::IssueRef,
        body: String,
    ) -> Result<String, Error> {
        match issue_ref {
            unblock_core::types::IssueRef::Local(number) => self.add_comment(*number, body).await,
            unblock_core::types::IssueRef::CrossRepo {
                owner,
                repo,
                number,
            } => self.add_comment_in_repo(owner, repo, *number, body).await,
        }
    }

    /// Updates the body of a GitHub issue.
    ///
    /// Sends a REST PATCH to `/repos/{owner}/{repo}/issues/{number}` with the
    /// new body content.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Domain`] with [`DomainError::IssueNotFound`] if the
    /// issue does not exist (HTTP 404).
    /// Returns [`Error::RateLimited`] for HTTP 429 responses.
    /// Returns [`Error::GitHubApi`] for other non-2xx responses.
    ///
    /// [`DomainError::IssueNotFound`]: unblock_core::errors::DomainError::IssueNotFound
    #[instrument(skip(self, body), fields(owner = %self.owner(), repo = %self.repo()))]
    pub async fn update_issue_body(&self, number: u64, body: String) -> Result<(), Error> {
        let url = self.rest_url(&format!(
            "/repos/{}/{}/issues/{number}",
            self.owner(),
            self.repo()
        ));

        let request_body = UpdateIssueBodyRequest { body };

        let response = self
            .http()
            .patch(&url)
            .json(&request_body)
            .send()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        if response.status().as_u16() == 404 {
            return Err(unblock_core::errors::IssueNotFoundSnafu { number }
                .build()
                .into());
        }

        check_rest_response(response).await?;

        debug!(number, "Updated issue body");
        Ok(())
    }

    /// Adds labels to a GitHub issue.
    ///
    /// Sends a REST POST to `/repos/{owner}/{repo}/issues/{number}/labels`.
    /// Labels that already exist on the issue are silently ignored by the API.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Domain`] with [`DomainError::IssueNotFound`] if the
    /// issue does not exist (HTTP 404).
    /// Returns [`Error::RateLimited`] for HTTP 429 responses.
    /// Returns [`Error::GitHubApi`] for other non-2xx responses.
    ///
    /// [`DomainError::IssueNotFound`]: unblock_core::errors::DomainError::IssueNotFound
    #[instrument(skip(self, labels), fields(owner = %self.owner(), repo = %self.repo()))]
    pub async fn add_labels_to_issue(&self, number: u64, labels: Vec<String>) -> Result<(), Error> {
        let url = self.rest_url(&format!(
            "/repos/{}/{}/issues/{number}/labels",
            self.owner(),
            self.repo()
        ));

        let request_body = AddLabelsBody { labels };

        let response = self
            .http()
            .post(&url)
            .json(&request_body)
            .send()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        if response.status().as_u16() == 404 {
            return Err(unblock_core::errors::IssueNotFoundSnafu { number }
                .build()
                .into());
        }

        check_rest_response(response).await?;

        debug!(number, "Added labels to issue");
        Ok(())
    }

    /// Removes a single label from a GitHub issue.
    ///
    /// Sends a REST DELETE to
    /// `/repos/{owner}/{repo}/issues/{number}/labels/{label}`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RateLimited`] for HTTP 429 responses.
    /// Returns [`Error::GitHubApi`] for other non-2xx responses.
    ///
    /// **Note:** HTTP 404 is treated as success — the label is not on the issue
    /// either way (the issue may not exist, or the label was never applied).
    /// This follows the best-effort pattern used by other mutation methods.
    #[instrument(skip(self), fields(owner = %self.owner(), repo = %self.repo()))]
    pub async fn remove_label_from_issue(&self, number: u64, label: &str) -> Result<(), Error> {
        let encoded_label = encode_path_segment(label);
        let url = self.rest_url(&format!(
            "/repos/{}/{}/issues/{number}/labels/{encoded_label}",
            self.owner(),
            self.repo()
        ));

        let response = self
            .http()
            .delete(&url)
            .send()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        if response.status().as_u16() == 404 {
            // 404 can mean the issue doesn't exist or the label isn't on the issue.
            // Log and treat as success — the label is not on the issue either way.
            warn!(
                number,
                label, "Label not found on issue (404) — treating as success"
            );
            return Ok(());
        }

        check_rest_response(response).await?;

        debug!(number, label, "Removed label from issue");
        Ok(())
    }

    /// Adds assignees to a GitHub issue.
    ///
    /// Sends a REST POST to
    /// `/repos/{owner}/{repo}/issues/{number}/assignees` with a JSON body
    /// containing the list of GitHub usernames to add.
    ///
    /// Assignees that are already on the issue are silently ignored by the API.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Domain`] with [`DomainError::IssueNotFound`] if the
    /// issue does not exist (HTTP 404).
    /// Returns [`Error::RateLimited`] for HTTP 429 responses.
    /// Returns [`Error::GitHubApi`] for other non-2xx responses.
    ///
    /// [`DomainError::IssueNotFound`]: unblock_core::errors::DomainError::IssueNotFound
    #[instrument(skip(self, assignees), fields(owner = %self.owner(), repo = %self.repo()))]
    pub async fn add_assignees_to_issue(
        &self,
        number: u64,
        assignees: Vec<String>,
    ) -> Result<(), Error> {
        let url = self.rest_url(&format!(
            "/repos/{}/{}/issues/{number}/assignees",
            self.owner(),
            self.repo()
        ));

        let request_body = AssigneesBody { assignees };

        let response = self
            .http()
            .post(&url)
            .json(&request_body)
            .send()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        if response.status().as_u16() == 404 {
            return Err(unblock_core::errors::IssueNotFoundSnafu { number }
                .build()
                .into());
        }

        check_rest_response(response).await?;

        debug!(number, "Added assignees to issue");
        Ok(())
    }

    /// Removes assignees from a GitHub issue.
    ///
    /// Sends a REST DELETE to
    /// `/repos/{owner}/{repo}/issues/{number}/assignees` with a JSON body
    /// containing the list of GitHub usernames to remove.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RateLimited`] for HTTP 429 responses.
    /// Returns [`Error::GitHubApi`] for other non-2xx responses.
    ///
    /// **Note:** HTTP 404 is treated as success — the assignee may not be on
    /// the issue, or the issue may not exist. This follows the best-effort
    /// pattern used by `remove_label_from_issue`.
    #[instrument(skip(self, assignees), fields(owner = %self.owner(), repo = %self.repo()))]
    pub async fn remove_assignees_from_issue(
        &self,
        number: u64,
        assignees: Vec<String>,
    ) -> Result<(), Error> {
        let url = self.rest_url(&format!(
            "/repos/{}/{}/issues/{number}/assignees",
            self.owner(),
            self.repo()
        ));

        let request_body = AssigneesBody { assignees };

        let response = self
            .http()
            .delete(&url)
            .json(&request_body)
            .send()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        if response.status().as_u16() == 404 {
            warn!(
                number,
                "Assignees not found on issue (404) — treating as success"
            );
            return Ok(());
        }

        check_rest_response(response).await?;

        debug!(number, "Removed assignees from issue");
        Ok(())
    }

    /// Lists all milestones for the configured repository.
    ///
    /// Sends a REST GET to `/repos/{owner}/{repo}/milestones` with
    /// `state=open&per_page=100`. Returns a list of [`Milestone`] structs.
    ///
    /// **Known limitation:** fetches only the first page (up to 100 milestones).
    /// Repos with more than 100 open milestones will silently miss entries beyond
    /// the first page. This is acceptable for typical project sizes.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RateLimited`] for HTTP 429 responses.
    /// Returns [`Error::GitHubApi`] for other non-2xx responses.
    #[instrument(skip(self), fields(owner = %self.owner(), repo = %self.repo()))]
    pub async fn list_milestones(&self) -> Result<Vec<Milestone>, Error> {
        let url = self.rest_url(&format!(
            "/repos/{}/{}/milestones?state=open&per_page=100",
            self.owner(),
            self.repo()
        ));

        let response = self
            .http()
            .get(&url)
            .send()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        let response = check_rest_response(response).await?;

        let milestones: Vec<Milestone> = response
            .json()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        debug!(count = milestones.len(), "Listed milestones");
        Ok(milestones)
    }

    /// Updates the milestone on a GitHub issue.
    ///
    /// Sends a REST PATCH to `/repos/{owner}/{repo}/issues/{number}` with the
    /// milestone number. Pass `None` to clear the milestone.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Domain`] with [`DomainError::IssueNotFound`] if the
    /// issue does not exist (HTTP 404).
    /// Returns [`Error::RateLimited`] for HTTP 429 responses.
    /// Returns [`Error::GitHubApi`] for other non-2xx responses.
    ///
    /// [`DomainError::IssueNotFound`]: unblock_core::errors::DomainError::IssueNotFound
    #[instrument(skip(self), fields(owner = %self.owner(), repo = %self.repo()))]
    pub async fn update_issue_milestone(
        &self,
        number: u64,
        milestone_number: Option<u64>,
    ) -> Result<(), Error> {
        let url = self.rest_url(&format!(
            "/repos/{}/{}/issues/{number}",
            self.owner(),
            self.repo()
        ));

        let request_body = UpdateMilestoneBody {
            milestone: milestone_number,
        };

        let response = self
            .http()
            .patch(&url)
            .json(&request_body)
            .send()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        if response.status().as_u16() == 404 {
            return Err(unblock_core::errors::IssueNotFoundSnafu { number }
                .build()
                .into());
        }

        check_rest_response(response).await?;

        debug!(number, milestone_number, "Updated issue milestone");
        Ok(())
    }

    /// Adds an issue to a GitHub Projects V2 project.
    ///
    /// Uses the GraphQL `addProjectV2ItemById` mutation. The caller provides
    /// the issue `node_id` directly (from the REST create response), avoiding
    /// an extra REST GET round-trip.
    ///
    /// Returns the `ProjectV2Item` node ID on success, which is needed by
    /// `update_field()` to set project field values on the item.
    ///
    /// This is an internal helper — callers use `create_issue()` which calls
    /// this automatically when a project is configured.
    async fn add_issue_to_project(
        &self,
        node_id: &str,
        project_number: u64,
    ) -> Result<String, Error> {
        // Fetch the project's node ID via GraphQL.
        let project_query = "
            query FindProject($owner: String!, $repo: String!, $projectNumber: Int!) {
                repository(owner: $owner, name: $repo) {
                    projectV2(number: $projectNumber) {
                        id
                    }
                }
            }
        ";

        let project_vars = serde_json::json!({
            "owner": self.owner(),
            "repo": self.repo(),
            "projectNumber": project_number,
        });

        let project_response = self.graphql(project_query, project_vars).await?;
        let project_id = project_response["data"]["repository"]["projectV2"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_owned();

        if project_id.is_empty() {
            warn!(
                project_number,
                "Project not found — cannot add issue to project"
            );
            return Err(errors::ProjectNotConfiguredSnafu.build());
        }

        // Add the issue to the project.
        let mutation = "
            mutation AddToProject($projectId: ID!, $contentId: ID!) {
                addProjectV2ItemById(input: {projectId: $projectId, contentId: $contentId}) {
                    item {
                        id
                    }
                }
            }
        ";

        let mutation_vars = serde_json::json!({
            "projectId": project_id,
            "contentId": node_id,
        });

        let response = self.graphql(mutation, mutation_vars).await?;
        let item_id = response["data"]["addProjectV2ItemById"]["item"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_owned();

        debug!(item_id = %item_id, "Added issue to project");
        Ok(item_id)
    }

    /// Resolves a GitHub issue number to a GraphQL global node ID.
    ///
    /// Queries `repository(owner, name) { issue(number: N) { id } }` and
    /// returns the node ID string. Returns [`IssueNotFound`] if the issue
    /// does not exist (null response from the API).
    ///
    /// [`IssueNotFound`]: unblock_core::errors::DomainError::IssueNotFound
    async fn resolve_node_id(&self, number: u64) -> Result<String, Error> {
        let query = "
            query ResolveNodeId($owner: String!, $repo: String!, $number: Int!) {
                repository(owner: $owner, name: $repo) {
                    issue(number: $number) {
                        id
                    }
                }
            }
        ";

        let variables = serde_json::json!({
            "owner": self.owner(),
            "repo": self.repo(),
            "number": number,
        });

        let response = self.graphql(query, variables).await?;
        let node_id = response["data"]["repository"]["issue"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_owned();

        if node_id.is_empty() {
            return Err(unblock_core::errors::IssueNotFoundSnafu { number }
                .build()
                .into());
        }

        debug!(number, node_id = %node_id, "Resolved issue number to node ID");
        Ok(node_id)
    }

    /// Adds a blocking relationship between two issues.
    ///
    /// After this call, `issue_number` is blocked by `blocked_by_number`.
    /// Both issue numbers are resolved to GraphQL node IDs internally.
    ///
    /// Uses the GitHub GraphQL `addIssueDependency` mutation with
    /// `dependentId` (the blocked issue) and `dependencyId` (the blocker).
    ///
    /// **Note:** Cycle detection is **not** performed here — it is the
    /// responsibility of the MCP tool handler layer (see bead 1.4.9).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Domain`] with [`DomainError::IssueNotFound`] if either
    /// issue does not exist.
    /// Returns [`Error::Domain`] with [`DomainError::DuplicateDependency`] if
    /// the blocking relationship already exists.
    /// Returns [`Error::GitHubGraphQL`] for other GraphQL errors.
    ///
    /// [`DomainError::IssueNotFound`]: unblock_core::errors::DomainError::IssueNotFound
    /// [`DomainError::DuplicateDependency`]: unblock_core::errors::DomainError::DuplicateDependency
    #[instrument(skip(self), fields(owner = %self.owner(), repo = %self.repo()))]
    pub async fn add_blocked_by(
        &self,
        issue_number: u64,
        blocked_by_number: u64,
    ) -> Result<(), Error> {
        // Fetch the issue first: validates existence, provides the node_id, and
        // gives us blocked_by for the duplicate pre-check — avoiding a redundant
        // resolve_node_id call for issue_number.
        let issue = self.fetch_issue(issue_number).await?;
        let issue_id = issue.node_id;

        let blocker_id = self.resolve_node_id(blocked_by_number).await?;

        // Pre-check: see if the blocking relationship already exists.
        // NOTE: This has a TOCTOU race — between this check and the mutation,
        // another caller could add the same relationship. Acceptable for the
        // current single-agent MCP server use case.
        let already_blocked = issue
            .blocked_by
            .iter()
            .any(|r| r.number == blocked_by_number);

        if already_blocked {
            return Err(unblock_core::errors::DuplicateDependencySnafu {
                source: unblock_core::types::IssueRef::Local(issue_number),
                target: unblock_core::types::IssueRef::Local(blocked_by_number),
            }
            .build()
            .into());
        }

        let mutation = "
            mutation AddBlockedBy($dependentId: ID!, $dependencyId: ID!) {
                addIssueDependency(input: {dependentId: $dependentId, dependencyId: $dependencyId}) {
                    clientMutationId
                }
            }
        ";

        let variables = serde_json::json!({
            "dependentId": issue_id,
            "dependencyId": blocker_id,
        });

        self.graphql(mutation, variables).await?;

        debug!(
            issue_number,
            blocked_by_number, "Added blocking relationship"
        );
        Ok(())
    }

    /// Removes a blocking relationship between two issues.
    ///
    /// After this call, `issue_number` is no longer blocked by
    /// `blocked_by_number`. Both issue numbers are resolved to GraphQL node
    /// IDs internally.
    ///
    /// Uses the GitHub GraphQL `removeIssueDependency` mutation.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Domain`] with [`DomainError::IssueNotFound`] if either
    /// issue does not exist.
    /// Returns [`Error::GitHubGraphQL`] for other GraphQL errors.
    ///
    /// [`DomainError::IssueNotFound`]: unblock_core::errors::DomainError::IssueNotFound
    #[instrument(skip(self), fields(owner = %self.owner(), repo = %self.repo()))]
    pub async fn remove_blocked_by(
        &self,
        issue_number: u64,
        blocked_by_number: u64,
    ) -> Result<(), Error> {
        let issue_id = self.resolve_node_id(issue_number).await?;
        let blocker_id = self.resolve_node_id(blocked_by_number).await?;

        let mutation = "
            mutation RemoveBlockedBy($dependentId: ID!, $dependencyId: ID!) {
                removeIssueDependency(input: {dependentId: $dependentId, dependencyId: $dependencyId}) {
                    clientMutationId
                }
            }
        ";

        let variables = serde_json::json!({
            "dependentId": issue_id,
            "dependencyId": blocker_id,
        });

        self.graphql(mutation, variables).await?;

        debug!(
            issue_number,
            blocked_by_number, "Removed blocking relationship"
        );
        Ok(())
    }

    /// Adds an issue as a sub-issue of a parent issue.
    ///
    /// After this call, `child_number` becomes a sub-issue of
    /// `parent_number`. Both issue numbers are resolved to GraphQL node IDs
    /// internally.
    ///
    /// Uses the GitHub GraphQL `addSubIssue` mutation with the
    /// `GraphQL-Features: sub_issues` preview header, which is required as
    /// of March 2026.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Domain`] with [`DomainError::IssueNotFound`] if either
    /// issue does not exist.
    /// Returns [`Error::GitHubGraphQL`] for other GraphQL errors.
    ///
    /// [`DomainError::IssueNotFound`]: unblock_core::errors::DomainError::IssueNotFound
    #[instrument(skip(self), fields(owner = %self.owner(), repo = %self.repo()))]
    pub async fn add_sub_issue(&self, parent_number: u64, child_number: u64) -> Result<(), Error> {
        let parent_id = self.resolve_node_id(parent_number).await?;
        let child_id = self.resolve_node_id(child_number).await?;

        let mutation = "
            mutation AddSubIssue($parentId: ID!, $childId: ID!) {
                addSubIssue(input: {issueId: $parentId, subIssueId: $childId}) {
                    issue {
                        id
                    }
                }
            }
        ";

        let variables = serde_json::json!({
            "parentId": parent_id,
            "childId": child_id,
        });

        self.graphql_with_features(mutation, variables, &["sub_issues"])
            .await?;

        debug!(parent_number, child_number, "Added sub-issue relationship");
        Ok(())
    }

    /// Resolves an [`IssueRef`] to a GraphQL global node ID.
    ///
    /// For local refs, resolves against the configured repository.
    /// For cross-repo refs, resolves against the specified `owner/repo`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Domain`] with [`DomainError::IssueNotFound`] if the
    /// issue does not exist.
    ///
    /// [`IssueRef`]: unblock_core::types::IssueRef
    /// [`DomainError::IssueNotFound`]: unblock_core::errors::DomainError::IssueNotFound
    #[instrument(skip(self), fields(owner = %self.owner(), repo = %self.repo()))]
    pub async fn resolve_issue_ref(
        &self,
        issue_ref: &unblock_core::types::IssueRef,
    ) -> Result<String, Error> {
        match issue_ref {
            unblock_core::types::IssueRef::Local(number) => self.resolve_node_id(*number).await,
            unblock_core::types::IssueRef::CrossRepo {
                owner,
                repo,
                number,
            } => {
                let query = "
                    query ResolveNodeId($owner: String!, $repo: String!, $number: Int!) {
                        repository(owner: $owner, name: $repo) {
                            issue(number: $number) {
                                id
                            }
                        }
                    }
                ";

                let variables = serde_json::json!({
                    "owner": owner,
                    "repo": repo,
                    "number": number,
                });

                // Per SPEC §11.1 / plan Task 02.02 "Error-side wiring",
                // a 403 (HTTP) or GraphQL FORBIDDEN response on this
                // cross-repo resolver MUST upgrade to
                // `DomainError::CrossRepoAccessDenied { owner, repo }`.
                let response = self
                    .graphql(query, variables)
                    .await
                    .map_err(|err| classify_cross_repo_fetch(err, owner, repo))?;
                let node_id = response["data"]["repository"]["issue"]["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();

                if node_id.is_empty() {
                    return Err(unblock_core::errors::IssueNotFoundSnafu { number: *number }
                        .build()
                        .into());
                }

                debug!(
                    owner,
                    repo,
                    number,
                    node_id = %node_id,
                    "Resolved cross-repo issue ref to node ID"
                );
                Ok(node_id)
            }
        }
    }

    /// Resolves the `ProjectV2Item` node ID for an issue in the configured project.
    ///
    /// Queries the issue's `projectItems` connection to find the item that belongs
    /// to the specified `project_id`. Returns the item's node ID, which is needed
    /// by [`update_field()`](crate::projects) to set field values.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Domain`] with [`DomainError::IssueNotFound`] if the issue
    /// is not in the project.
    ///
    /// [`DomainError::IssueNotFound`]: unblock_core::errors::DomainError::IssueNotFound
    #[instrument(skip(self))]
    pub async fn get_project_item_id(
        &self,
        issue_node_id: &str,
        project_id: &str,
    ) -> Result<String, Error> {
        let query = "
            query GetProjectItemId($nodeId: ID!) {
                node(id: $nodeId) {
                    ... on Issue {
                        projectItems(first: 20) {
                            nodes {
                                id
                                project {
                                    id
                                }
                            }
                        }
                    }
                }
            }
        ";

        let variables = serde_json::json!({
            "nodeId": issue_node_id,
        });

        let response = self.graphql(query, variables).await?;
        let items = response["data"]["node"]["projectItems"]["nodes"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        for item in items {
            let item_project_id = item["project"]["id"].as_str().unwrap_or_default();
            if item_project_id == project_id {
                let item_id = item["id"].as_str().unwrap_or_default().to_owned();
                if !item_id.is_empty() {
                    debug!(item_id = %item_id, "Resolved project item ID");
                    return Ok(item_id);
                }
            }
        }

        Err(unblock_core::errors::ValidationSnafu {
            message: format!(
                "Issue not found in project — node_id={issue_node_id}, project_id={project_id}"
            ),
        }
        .build()
        .into())
    }

    /// Ensures that all labels in the list exist on the repository.
    ///
    /// For each label, checks if it exists via GET and creates it via POST if not.
    /// Label creation uses a deterministic color derived from the label name.
    ///
    /// # Errors
    ///
    /// Returns [`Error::GitHubApi`] if a label creation fails for reasons other
    /// than the label already existing (HTTP 422 with `already_exists` is ignored).
    #[instrument(skip(self, labels), fields(owner = %self.owner(), repo = %self.repo()))]
    pub async fn ensure_labels(&self, labels: &[String]) -> Result<(), Error> {
        for label in labels {
            let encoded_label = encode_path_segment(label);
            let url = self.rest_url(&format!(
                "/repos/{}/{}/labels/{encoded_label}",
                self.owner(),
                self.repo(),
            ));

            let response = self
                .http()
                .get(&url)
                .send()
                .await
                .context(errors::GitHubUnavailableSnafu)?;

            if response.status().is_success() {
                debug!(label = %label, "Label already exists");
                continue;
            }

            // Label does not exist — create it.
            let create_url =
                self.rest_url(&format!("/repos/{}/{}/labels", self.owner(), self.repo()));

            let color = deterministic_label_color(label);
            let body = serde_json::json!({
                "name": label,
                "color": color,
            });

            let create_response = self
                .http()
                .post(&create_url)
                .json(&body)
                .send()
                .await
                .context(errors::GitHubUnavailableSnafu)?;

            let status = create_response.status();
            if status.is_success() {
                debug!(label = %label, color = %color, "Created label");
            } else if status.as_u16() == 422 {
                // 422 with "already_exists" — race condition, label was created concurrently.
                debug!(label = %label, "Label creation returned 422 — likely already exists");
            } else {
                let message = create_response
                    .text()
                    .await
                    .unwrap_or_else(|_| "unknown error".to_owned());
                return Err(errors::GitHubApiSnafu {
                    status: status.as_u16(),
                    message,
                }
                .build());
            }
        }

        Ok(())
    }

    /// Reopens a closed GitHub issue.
    ///
    /// Sends a REST PATCH to `/repos/{owner}/{repo}/issues/{number}` with
    /// `state: "open"` and `state_reason: "reopened"`. The reason value
    /// mirrors the symmetry with [`close_issue`](Self::close_issue), which
    /// sends `state_reason: "completed"`.
    ///
    /// The mutation is a no-op against an issue that is already open — GitHub
    /// returns a 200 response unchanged. Callers that need blocking
    /// re-evaluation after reopen must perform a graph rebuild themselves; per
    /// spec §8.7 the MCP `reopen` tool is responsible for that orchestration.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Domain`] with [`DomainError::IssueNotFound`] if the
    /// issue does not exist (HTTP 404).
    /// Returns [`Error::RateLimited`] for HTTP 429 responses.
    /// Returns [`Error::GitHubApi`] for other non-2xx responses.
    ///
    /// [`DomainError::IssueNotFound`]: unblock_core::errors::DomainError::IssueNotFound
    #[instrument(skip(self), fields(owner = %self.owner(), repo = %self.repo()))]
    pub async fn reopen_issue(&self, number: u64) -> Result<(), Error> {
        let url = self.rest_url(&format!(
            "/repos/{}/{}/issues/{number}",
            self.owner(),
            self.repo()
        ));

        let request_body = ReopenIssueBody {
            state: "open",
            state_reason: "reopened",
        };

        let response = self
            .http()
            .patch(&url)
            .json(&request_body)
            .send()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        if response.status().as_u16() == 404 {
            return Err(unblock_core::errors::IssueNotFoundSnafu { number }
                .build()
                .into());
        }

        check_rest_response(response).await?;

        debug!(number, "Reopened issue");
        Ok(())
    }

    /// Full-text search of issues in the configured repository.
    ///
    /// Sends a REST GET to `/search/issues` scoped to the configured
    /// `owner/repo` with `is:issue` and the caller-supplied free-text query.
    /// The final query string is `"repo:{owner}/{repo} is:issue {query}"` —
    /// URL encoding is handled by `reqwest`'s `.query(...)` helper so callers
    /// do not need to pre-escape special characters (`#`, spaces, quotes,
    /// etc.).
    ///
    /// `limit` defaults to 20 when `None` per spec §7.6, and is clamped to the
    /// GitHub Search API's maximum page size of 100 to avoid 422 responses.
    ///
    /// **Cache:** This method deliberately bypasses any caller-side cache.
    /// Each invocation hits GitHub's Search API directly — this matches the
    /// "API calls: 1" contract documented in spec §7.6.
    ///
    /// **Reduced issue shape.** The Search API does not return Projects V2
    /// custom field values. Entries in the returned [`IssueSummary`] list
    /// therefore carry default values for `status` (Ready), `priority` (P2),
    /// and `None` for `agent`, `story_points`, and `defer_until`. GitHub-native
    /// fields (`title`, `labels`, `milestone`, `created_at`, `url`) are
    /// populated from the search response.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RateLimited`] for HTTP 429 responses.
    /// Returns [`Error::GitHubApi`] for other non-2xx responses.
    /// Returns [`Error::GitHubUnavailable`] for network failures.
    #[instrument(skip(self, query), fields(owner = %self.owner(), repo = %self.repo()))]
    pub async fn search_issues(
        &self,
        query: &str,
        limit: Option<u32>,
    ) -> Result<Vec<IssueSummary>, Error> {
        let url = self.rest_url("/search/issues");

        // Compose the search query and clamp limit to GitHub's per_page max.
        let q = format!("repo:{}/{} is:issue {query}", self.owner(), self.repo());
        let per_page = limit.unwrap_or(20).min(100);

        let response = self
            .http()
            .get(&url)
            .query(&[("q", q.as_str()), ("per_page", &per_page.to_string())])
            .send()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        let response = check_rest_response(response).await?;

        let body: SearchIssuesResponse = response
            .json()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        let summaries = body
            .items
            .into_iter()
            .map(|item| self.search_item_to_summary(item))
            .collect::<Vec<_>>();

        debug!(count = summaries.len(), "Searched issues");
        Ok(summaries)
    }

    /// Maps a [`SearchIssueItem`] to an [`IssueSummary`].
    ///
    /// Defaults Projects V2 fields to their `Default`-equivalent values (see
    /// the `search_issues` doc comment for the full contract). The
    /// `qualified_id` is constructed from the client's configured owner/repo
    /// — search is scoped to the local repository so this is always correct.
    fn search_item_to_summary(&self, item: SearchIssueItem) -> IssueSummary {
        let labels = item.labels.into_iter().map(|l| l.name).collect::<Vec<_>>();
        IssueSummary {
            qualified_id: QualifiedId::new(self.owner(), self.repo(), item.number),
            number: item.number,
            title: item.title,
            issue_type: None::<IssueType>,
            status: Status::Ready,
            priority: Priority::P2,
            agent: None,
            milestone: item.milestone.map(|m| m.title),
            story_points: None,
            defer_until: None,
            labels,
            created_at: item.created_at,
            url: item.html_url,
        }
    }

    /// Adds a blocking relationship using an [`IssueRef`] as the blocker.
    ///
    /// For local refs, delegates to [`add_blocked_by()`](Self::add_blocked_by).
    /// For cross-repo refs, resolves the blocker's node ID via the target repo
    /// and calls `addIssueDependency` directly.
    ///
    /// [`IssueRef`]: unblock_core::types::IssueRef
    ///
    /// # Errors
    ///
    /// Returns [`Error::Domain`] with [`DomainError::IssueNotFound`] if either
    /// issue does not exist.
    ///
    /// [`DomainError::IssueNotFound`]: unblock_core::errors::DomainError::IssueNotFound
    #[instrument(skip(self), fields(owner = %self.owner(), repo = %self.repo()))]
    pub async fn add_blocked_by_ref(
        &self,
        issue_number: u64,
        blocker: &unblock_core::types::IssueRef,
    ) -> Result<(), Error> {
        match blocker {
            unblock_core::types::IssueRef::Local(blocked_by_number) => {
                self.add_blocked_by(issue_number, *blocked_by_number).await
            }
            unblock_core::types::IssueRef::CrossRepo { .. } => {
                let issue_id = self.resolve_node_id(issue_number).await?;
                let blocker_id = self.resolve_issue_ref(blocker).await?;

                let mutation = "
                    mutation AddBlockedBy($dependentId: ID!, $dependencyId: ID!) {
                        addIssueDependency(input: {dependentId: $dependentId, dependencyId: $dependencyId}) {
                            clientMutationId
                        }
                    }
                ";

                let variables = serde_json::json!({
                    "dependentId": issue_id,
                    "dependencyId": blocker_id,
                });

                self.graphql(mutation, variables).await?;

                debug!(
                    issue_number,
                    blocker = %blocker,
                    "Added cross-repo blocking relationship"
                );
                Ok(())
            }
        }
    }

    /// Adds a blocking relationship using [`IssueRef`] for both endpoints.
    ///
    /// Spec §8.4 requires the `depends` tool to accept cross-repo references
    /// for both `source` and `target`. This method generalises
    /// [`add_blocked_by_ref()`](Self::add_blocked_by_ref) (which only accepts a
    /// cross-repo blocker) to also accept a cross-repo source.
    ///
    /// Dispatch:
    /// - `source = Local(n)` → delegates to
    ///   [`add_blocked_by_ref()`](Self::add_blocked_by_ref) so the existing
    ///   local-source fast path (single `fetch_issue` + duplicate pre-check) is
    ///   preserved.
    /// - `source = CrossRepo { .. }` → fetches the source via
    ///   [`fetch_issue_ref()`](Self::fetch_issue_ref) against its own
    ///   owner/repo, performs the duplicate pre-check against its
    ///   `blocked_by` list, resolves the blocker via
    ///   [`resolve_issue_ref()`](Self::resolve_issue_ref), and runs the
    ///   `addIssueDependency` mutation with both node IDs.
    ///
    /// The duplicate pre-check has a TOCTOU race like
    /// [`add_blocked_by()`](Self::add_blocked_by) — acceptable for the
    /// single-agent MCP server use case.
    ///
    /// [`IssueRef`]: unblock_core::types::IssueRef
    ///
    /// # Errors
    ///
    /// Returns [`Error::Domain`] with [`DomainError::IssueNotFound`] if either
    /// issue does not exist.
    /// Returns [`Error::Domain`] with [`DomainError::DuplicateDependency`] if
    /// the blocking relationship already exists on the source.
    /// Returns [`Error::GitHubGraphQL`] for other GraphQL errors.
    ///
    /// [`DomainError::IssueNotFound`]: unblock_core::errors::DomainError::IssueNotFound
    /// [`DomainError::DuplicateDependency`]: unblock_core::errors::DomainError::DuplicateDependency
    #[instrument(skip(self), fields(owner = %self.owner(), repo = %self.repo()))]
    pub async fn add_blocked_by_refs(
        &self,
        source: &unblock_core::types::IssueRef,
        blocker: &unblock_core::types::IssueRef,
    ) -> Result<(), Error> {
        match source {
            unblock_core::types::IssueRef::Local(source_number) => {
                // Preserve the local-source fast path and its existing
                // duplicate pre-check (performed inside add_blocked_by for
                // local blockers; preserved by add_blocked_by_ref delegation
                // for cross-repo blockers).
                self.add_blocked_by_ref(*source_number, blocker).await
            }
            unblock_core::types::IssueRef::CrossRepo {
                owner: source_owner,
                repo: source_repo,
                number: source_number,
            } => {
                // Fetch source via its own owner/repo: validates existence,
                // gives us the node_id, and provides blocked_by for the
                // duplicate pre-check in a single round-trip.
                let source_issue = self.fetch_issue_ref(source).await?;

                // Duplicate pre-check: GitHub's `trackedByIssues` connection
                // only surfaces blockers within the same `owner/repo` as the
                // source issue. Therefore the `blocked_by` list on a
                // cross-repo source contains only entries in the source's
                // own repo.
                //
                // So a duplicate match only exists when the blocker resolves
                // to the same `owner/repo` as the source. For any other
                // blocker (including a `Local` blocker, which lives in the
                // configured project repo and not in the source repo), we
                // cannot detect a duplicate client-side and rely on GitHub
                // server-side rejection instead.
                let blocker_number_in_source_repo = match blocker {
                    unblock_core::types::IssueRef::CrossRepo {
                        owner,
                        repo,
                        number,
                    } if owner == source_owner && repo == source_repo => Some(*number),
                    _ => None,
                };

                if let Some(n) = blocker_number_in_source_repo
                    && source_issue.blocked_by.iter().any(|r| r.number == n)
                {
                    // See docs/specs/01-spec-mcp-foundation.md §11.1 Decision 1 for the cross-repo error context rationale.
                    return Err(unblock_core::errors::DuplicateDependencySnafu {
                        source: source.clone(),
                        target: unblock_core::types::IssueRef::CrossRepo {
                            owner: source_owner.clone(),
                            repo: source_repo.clone(),
                            number: n,
                        },
                    }
                    .build()
                    .into());
                }

                let source_id = source_issue.node_id;
                let blocker_id = self.resolve_issue_ref(blocker).await?;

                let mutation = "
                    mutation AddBlockedBy($dependentId: ID!, $dependencyId: ID!) {
                        addIssueDependency(input: {dependentId: $dependentId, dependencyId: $dependencyId}) {
                            clientMutationId
                        }
                    }
                ";

                let variables = serde_json::json!({
                    "dependentId": source_id,
                    "dependencyId": blocker_id,
                });

                self.graphql(mutation, variables).await?;

                debug!(
                    source_owner = %source_owner,
                    source_repo = %source_repo,
                    source_number,
                    blocker = %blocker,
                    "Added cross-repo blocking relationship (cross-repo source)"
                );
                Ok(())
            }
        }
    }

    /// Removes a blocking relationship using an [`IssueRef`] as the
    /// blocker.
    ///
    /// Symmetric counterpart of
    /// [`add_blocked_by_ref()`](Self::add_blocked_by_ref). For local refs
    /// this delegates to [`remove_blocked_by()`](Self::remove_blocked_by)
    /// so the existing local-number fast path is preserved. For cross-repo
    /// refs it resolves the blocker's node ID against the target repo and
    /// runs the `removeIssueDependency` mutation directly with both node
    /// IDs.
    ///
    /// See spec §5.6 (cross-repo scope) and §8.5 (`dep_remove`).
    ///
    /// [`IssueRef`]: unblock_core::types::IssueRef
    ///
    /// # Errors
    ///
    /// Returns [`Error::Domain`] with [`DomainError::IssueNotFound`] if
    /// either issue does not exist. Returns [`Error::GitHubGraphQL`] for
    /// other GraphQL errors.
    ///
    /// [`DomainError::IssueNotFound`]: unblock_core::errors::DomainError::IssueNotFound
    #[instrument(skip(self), fields(owner = %self.owner(), repo = %self.repo()))]
    pub async fn remove_blocked_by_ref(
        &self,
        issue_number: u64,
        blocker: &unblock_core::types::IssueRef,
    ) -> Result<(), Error> {
        match blocker {
            unblock_core::types::IssueRef::Local(blocked_by_number) => {
                self.remove_blocked_by(issue_number, *blocked_by_number)
                    .await
            }
            unblock_core::types::IssueRef::CrossRepo { .. } => {
                let issue_id = self.resolve_node_id(issue_number).await?;
                let blocker_id = self.resolve_issue_ref(blocker).await?;

                let mutation = "
                    mutation RemoveBlockedBy($dependentId: ID!, $dependencyId: ID!) {
                        removeIssueDependency(input: {dependentId: $dependentId, dependencyId: $dependencyId}) {
                            clientMutationId
                        }
                    }
                ";

                let variables = serde_json::json!({
                    "dependentId": issue_id,
                    "dependencyId": blocker_id,
                });

                self.graphql(mutation, variables).await?;

                debug!(
                    issue_number,
                    blocker = %blocker,
                    "Removed cross-repo blocking relationship"
                );
                Ok(())
            }
        }
    }

    /// Removes a blocking relationship using [`IssueRef`] for both
    /// endpoints.
    ///
    /// Spec §8.5 requires the `dep_remove` tool to accept cross-repo
    /// references for both `source` and `target`. This method generalises
    /// [`remove_blocked_by_ref()`](Self::remove_blocked_by_ref) (which
    /// only accepts a cross-repo blocker) to also accept a cross-repo
    /// source.
    ///
    /// Dispatch:
    /// - `source = Local(n)` → delegates to
    ///   [`remove_blocked_by_ref()`](Self::remove_blocked_by_ref) so the
    ///   existing local-source fast path is preserved.
    /// - `source = CrossRepo { .. }` → resolves both the source and the
    ///   blocker via [`resolve_issue_ref()`](Self::resolve_issue_ref)
    ///   against their own repositories and runs the
    ///   `removeIssueDependency` mutation directly with both node IDs.
    ///
    /// Unlike [`add_blocked_by_refs()`](Self::add_blocked_by_refs) this
    /// method does not perform a client-side edge-existence pre-check:
    /// GitHub's `removeIssueDependency` is idempotent with respect to
    /// missing edges (no error on a non-existent edge), so an optional
    /// pre-check here would only add a round-trip. The MCP tool layer
    /// may still perform a cache-based pre-check when both endpoints are
    /// local, purely to surface a more informative error to the caller.
    ///
    /// See spec §8.5 (`dep_remove` tool contract) and §5.6 cross-repo
    /// scope table.
    ///
    /// [`IssueRef`]: unblock_core::types::IssueRef
    ///
    /// # Errors
    ///
    /// Returns [`Error::Domain`] with [`DomainError::IssueNotFound`] if
    /// either issue does not exist. Returns [`Error::GitHubGraphQL`] for
    /// other GraphQL errors.
    ///
    /// [`DomainError::IssueNotFound`]: unblock_core::errors::DomainError::IssueNotFound
    #[instrument(skip(self), fields(owner = %self.owner(), repo = %self.repo()))]
    pub async fn remove_blocked_by_refs(
        &self,
        source: &unblock_core::types::IssueRef,
        blocker: &unblock_core::types::IssueRef,
    ) -> Result<(), Error> {
        match source {
            unblock_core::types::IssueRef::Local(source_number) => {
                self.remove_blocked_by_ref(*source_number, blocker).await
            }
            unblock_core::types::IssueRef::CrossRepo {
                owner: source_owner,
                repo: source_repo,
                number: source_number,
            } => {
                let source_id = self.resolve_issue_ref(source).await?;
                let blocker_id = self.resolve_issue_ref(blocker).await?;

                let mutation = "
                    mutation RemoveBlockedBy($dependentId: ID!, $dependencyId: ID!) {
                        removeIssueDependency(input: {dependentId: $dependentId, dependencyId: $dependencyId}) {
                            clientMutationId
                        }
                    }
                ";

                let variables = serde_json::json!({
                    "dependentId": source_id,
                    "dependencyId": blocker_id,
                });

                self.graphql(mutation, variables).await?;

                debug!(
                    source_owner = %source_owner,
                    source_repo = %source_repo,
                    source_number,
                    blocker = %blocker,
                    "Removed cross-repo blocking relationship (cross-repo source)"
                );
                Ok(())
            }
        }
    }
}

/// Percent-encodes a string for use as a URL path segment.
///
/// Encodes all characters that are not unreserved per RFC 3986.
fn encode_path_segment(s: &str) -> String {
    let mut encoded = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                let _ = write!(encoded, "%{byte:02X}");
            }
        }
    }
    encoded
}

/// Generates a deterministic 6-hex-digit color from a label name.
///
/// Uses a simple hash to produce visually distinct colors for different labels.
/// The algorithm is intentionally simple — color aesthetics are not critical.
fn deterministic_label_color(label: &str) -> String {
    let mut hash: u32 = 5381;
    for byte in label.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(u32::from(byte));
    }
    // Mask to 24 bits for RGB.
    format!("{:06x}", hash & 0x00FF_FFFF)
}

#[cfg(test)]
mod tests {
    use crate::client::GitHubClient;
    use crate::errors::Error;
    use unblock_core::errors::DomainError;
    use unblock_core::types::{Priority, Status};
    use wiremock::matchers::{body_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Regression guard for the library-level close guard: a whitespace-only
    /// `reason` passed directly to [`GitHubClient::close_issue`] must be
    /// treated the same as `None` and must NOT post a `"Closed:  "` comment
    /// before closing. Mirrors the MCP handler layer's trim-based filter.
    #[tokio::test]
    async fn close_issue_with_whitespace_reason_skips_comment() {
        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        // The comments endpoint MUST NOT be called. `expect(0)` on wiremock
        // fails the test on drop if any matching request was received.
        Mock::given(method("POST"))
            .and(path("/repos/test-owner/test-repo/issues/42/comments"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "html_url": "https://example.invalid/should-not-be-called"
            })))
            .expect(0)
            .mount(&server)
            .await;

        // The close PATCH must still be called exactly once.
        Mock::given(method("PATCH"))
            .and(path("/repos/test-owner/test-repo/issues/42"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "number": 42,
                "state": "closed"
            })))
            .expect(1)
            .mount(&server)
            .await;

        client
            .close_issue(42, Some("   ".to_owned()))
            .await
            .expect("close_issue with whitespace-only reason should succeed");
        // MockServer's Drop verifies the expect(0) / expect(1) counters.
    }

    /// Happy path: `reopen_issue(n)` issues a single REST PATCH with
    /// `{"state": "open", "state_reason": "reopened"}` body and succeeds on
    /// HTTP 200.
    #[tokio::test]
    async fn reopen_issue_sends_state_open_and_state_reason_reopened() {
        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        Mock::given(method("PATCH"))
            .and(path("/repos/test-owner/test-repo/issues/77"))
            .and(body_json(serde_json::json!({
                "state": "open",
                "state_reason": "reopened",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "number": 77,
                "state": "open"
            })))
            .expect(1)
            .mount(&server)
            .await;

        client
            .reopen_issue(77)
            .await
            .expect("reopen_issue should succeed against a 200 response");
    }

    /// Failure path: a 404 on reopen surfaces as
    /// [`DomainError::IssueNotFound`] through the infra `Error::Domain`
    /// wrapper.
    #[tokio::test]
    async fn reopen_issue_returns_issue_not_found_on_404() {
        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        Mock::given(method("PATCH"))
            .and(path("/repos/test-owner/test-repo/issues/999"))
            .respond_with(ResponseTemplate::new(404).set_body_string("Not Found"))
            .mount(&server)
            .await;

        let err = client
            .reopen_issue(999)
            .await
            .expect_err("reopen_issue should return Err on 404");

        match err {
            Error::Domain {
                source: DomainError::IssueNotFound { number },
            } => assert_eq!(number, 999),
            other => panic!("expected IssueNotFound, got: {other}"),
        }
    }

    /// Happy path: `search_issues(query, Some(limit))` sends the REST search
    /// request with the correctly formatted `q` and `per_page` query params
    /// and maps the response items to [`IssueSummary`] with spec defaults for
    /// Projects V2 fields.
    #[tokio::test]
    async fn search_issues_composes_query_and_maps_to_summary() {
        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        let response_body = serde_json::json!({
            "total_count": 1,
            "incomplete_results": false,
            "items": [
                {
                    "number": 42,
                    "title": "Ship it",
                    "labels": [
                        { "name": "bug" },
                        { "name": "priority-high" }
                    ],
                    "milestone": { "title": "v0.1.0" },
                    "html_url": "https://github.com/test-owner/test-repo/issues/42",
                    "created_at": "2025-01-01T00:00:00Z"
                }
            ]
        });

        Mock::given(method("GET"))
            .and(path("/search/issues"))
            .and(query_param("q", "repo:test-owner/test-repo is:issue ship"))
            .and(query_param("per_page", "5"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
            .expect(1)
            .mount(&server)
            .await;

        let results = client
            .search_issues("ship", Some(5))
            .await
            .expect("search_issues should succeed");

        assert_eq!(results.len(), 1);
        let s = &results[0];
        assert_eq!(s.number, 42);
        assert_eq!(s.title, "Ship it");
        assert_eq!(s.qualified_id.owner, "test-owner");
        assert_eq!(s.qualified_id.repo, "test-repo");
        assert_eq!(s.qualified_id.number, 42);
        assert_eq!(s.labels, vec!["bug".to_owned(), "priority-high".to_owned()]);
        assert_eq!(s.milestone.as_deref(), Some("v0.1.0"));
        assert_eq!(
            s.url,
            "https://github.com/test-owner/test-repo/issues/42".to_owned()
        );
        // Projects V2 defaults — not returned by REST search.
        assert_eq!(s.status, Status::Ready);
        assert_eq!(s.priority, Priority::P2);
        assert!(s.issue_type.is_none());
        assert!(s.agent.is_none());
        assert!(s.story_points.is_none());
        assert!(s.defer_until.is_none());
    }

    /// An empty `items` array must yield an empty `Vec<IssueSummary>` — no
    /// 404, no error.
    #[tokio::test]
    async fn search_issues_empty_items_returns_empty_vec() {
        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        Mock::given(method("GET"))
            .and(path("/search/issues"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "total_count": 0,
                "incomplete_results": false,
                "items": []
            })))
            .mount(&server)
            .await;

        let results = client
            .search_issues("no-hits", None)
            .await
            .expect("search_issues with no hits should return Ok(empty)");
        assert!(results.is_empty());
    }

    /// `limit = None` must default to 20 (spec §7.6). The test asserts the
    /// `per_page` query parameter sent on the wire.
    #[tokio::test]
    async fn search_issues_defaults_limit_to_20() {
        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        Mock::given(method("GET"))
            .and(path("/search/issues"))
            .and(query_param("per_page", "20"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "total_count": 0,
                "incomplete_results": false,
                "items": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        client
            .search_issues("anything", None)
            .await
            .expect("search_issues should succeed");
    }

    /// `limit > 100` must be clamped to 100 (GitHub Search API cap).
    #[tokio::test]
    async fn search_issues_clamps_limit_to_100() {
        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        Mock::given(method("GET"))
            .and(path("/search/issues"))
            .and(query_param("per_page", "100"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "total_count": 0,
                "incomplete_results": false,
                "items": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        client
            .search_issues("anything", Some(500))
            .await
            .expect("search_issues should succeed with clamp");
    }

    /// `search_issues` must surface a 429 as [`Error::RateLimited`].
    #[tokio::test]
    async fn search_issues_returns_rate_limited_on_429() {
        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        Mock::given(method("GET"))
            .and(path("/search/issues"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("x-ratelimit-reset", "1700000000")
                    .set_body_string(""),
            )
            .mount(&server)
            .await;

        let err = client
            .search_issues("q", Some(10))
            .await
            .expect_err("search_issues should return Err on 429");
        assert!(matches!(err, Error::RateLimited { .. }));
    }

    /// Envelope-field tolerance (unblock-29p.20).
    ///
    /// GitHub's `/search/issues` endpoint always wraps the result list in a
    /// JSON object that carries `total_count`, `incomplete_results`, and
    /// `items`. The crate-internal `SearchIssuesResponse` type intentionally
    /// captures only `items` (see `SearchIssuesResponse` doc comment), and
    /// relies on serde's default ignore-unknown-fields behaviour — there is
    /// no `#[serde(deny_unknown_fields)]` attribute — to drop the envelope
    /// metadata silently.
    ///
    /// This regression guard pins that contract: a future serde-attribute
    /// change (e.g. globally enabling `deny_unknown_fields`, switching to a
    /// different deserializer, or renaming the wrapper type) would flip
    /// every real GitHub response into a deserialization error. The fixture
    /// here mirrors the actual envelope shape — `total_count`,
    /// `incomplete_results`, and a representative `items` array — so the
    /// test fails loudly if the tolerance is silently broken.
    #[tokio::test]
    async fn search_issues_tolerates_envelope_metadata_fields() {
        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        // Embed BOTH `total_count` and `incomplete_results` alongside the
        // `items` array. If a future change flips `SearchIssuesResponse`
        // into a strict-fields deserializer, this body will fail to
        // deserialize and the test will fail with a serde error rather
        // than a missing-field assertion — making the regression cause
        // self-evident.
        let response_body = serde_json::json!({
            "total_count": 1,
            "incomplete_results": false,
            "items": [
                {
                    "number": 7,
                    "title": "Envelope-tolerant",
                    "labels": [],
                    "milestone": null,
                    "html_url": "https://github.com/test-owner/test-repo/issues/7",
                    "created_at": "2026-04-14T00:00:00Z"
                }
            ]
        });

        Mock::given(method("GET"))
            .and(path("/search/issues"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
            .expect(1)
            .mount(&server)
            .await;

        let results = client
            .search_issues("anything", Some(10))
            .await
            .expect("search_issues must ignore total_count / incomplete_results");

        // Caller-visible count comes from `items.len()`, not `total_count`
        // — pin that contract too so a future refactor that starts honouring
        // `total_count` (which would change the public count semantics)
        // surfaces here.
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].number, 7);
        assert_eq!(results[0].title, "Envelope-tolerant");
    }

    // ── add_comment_in_repo / add_comment_ref (unblock-eos.13) ────────────
    //
    // Regression guards for the cross-repo cascade primitives introduced in
    // unblock-eos.13 (SPEC §5.6 row `close`, §8.2 step 6, §11.4 row 4). The
    // key contract is that `add_comment_in_repo` routes the REST POST to
    // `/repos/{owner}/{repo}/issues/{number}/comments` using the ARGUMENTS,
    // not the configured `self.owner()/self.repo()` — otherwise a cross-repo
    // dependent's unblock comment silently lands on the wrong repo (the
    // pre-fix behaviour flagged in the bead's investigation).

    #[tokio::test]
    async fn add_comment_in_repo_uses_argument_owner_and_repo_not_self() {
        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        // Configured repo is `test-owner/test-repo` (see `new_for_test`),
        // but we post to `other/repo#99` — the URL path MUST carry the
        // argument tuple, NOT the configured one. `expect(1)` on the
        // correct path + `expect(0)` on the wrong path encodes the
        // argument-routing contract.
        Mock::given(method("POST"))
            .and(path("/repos/other/repo/issues/99/comments"))
            .and(body_json(serde_json::json!({ "body": "cross-repo body" })))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "html_url": "https://github.com/other/repo/issues/99#issuecomment-1"
            })))
            .expect(1)
            .mount(&server)
            .await;

        // Wrong-target guard: if the impl incorrectly retargets the
        // configured repo, this mock would fire and the test would fail
        // at Drop via `expect(0)`.
        Mock::given(method("POST"))
            .and(path("/repos/test-owner/test-repo/issues/99/comments"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "html_url": "https://should-not-be-called.invalid"
            })))
            .expect(0)
            .mount(&server)
            .await;

        let url = client
            .add_comment_in_repo("other", "repo", 99, "cross-repo body".to_owned())
            .await
            .expect("add_comment_in_repo should succeed");
        assert_eq!(
            url, "https://github.com/other/repo/issues/99#issuecomment-1",
            "returned html_url must come from the response body"
        );
    }

    #[tokio::test]
    async fn add_comment_in_repo_returns_issue_not_found_on_404() {
        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        Mock::given(method("POST"))
            .and(path("/repos/other/repo/issues/4242/comments"))
            .respond_with(ResponseTemplate::new(404).set_body_string("Not Found"))
            .mount(&server)
            .await;

        let err = client
            .add_comment_in_repo("other", "repo", 4242, "ignored".to_owned())
            .await
            .expect_err("add_comment_in_repo should fail on 404");
        assert!(
            matches!(
                err,
                Error::Domain {
                    source: DomainError::IssueNotFound { number: 4242 },
                }
            ),
            "404 MUST surface as DomainError::IssueNotFound preserving the argument number; \
             got: {err:?}"
        );
    }

    #[tokio::test]
    async fn add_comment_ref_dispatches_local_to_configured_repo() {
        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        // IssueRef::Local → must post against the CONFIGURED repo path.
        Mock::given(method("POST"))
            .and(path("/repos/test-owner/test-repo/issues/7/comments"))
            .and(body_json(serde_json::json!({ "body": "local-path body" })))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "html_url": "https://github.com/test-owner/test-repo/issues/7#issuecomment-local"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let url = client
            .add_comment_ref(
                &unblock_core::types::IssueRef::Local(7),
                "local-path body".to_owned(),
            )
            .await
            .expect("add_comment_ref Local should succeed");
        assert_eq!(
            url,
            "https://github.com/test-owner/test-repo/issues/7#issuecomment-local"
        );
    }

    #[tokio::test]
    async fn add_comment_ref_dispatches_cross_repo_to_argument_repo() {
        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        // IssueRef::CrossRepo → must post against the REF's owner/repo,
        // NOT the configured repo. This is the core unblock-eos.13 fix.
        Mock::given(method("POST"))
            .and(path("/repos/alpha/upstream/issues/42/comments"))
            .and(body_json(serde_json::json!({ "body": "cross-repo body" })))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "html_url": "https://github.com/alpha/upstream/issues/42#issuecomment-cross"
            })))
            .expect(1)
            .mount(&server)
            .await;

        // Wrong-target guard: the configured repo MUST NOT be hit.
        Mock::given(method("POST"))
            .and(path("/repos/test-owner/test-repo/issues/42/comments"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "html_url": "https://should-not-be-called.invalid"
            })))
            .expect(0)
            .mount(&server)
            .await;

        let url = client
            .add_comment_ref(
                &unblock_core::types::IssueRef::CrossRepo {
                    owner: "alpha".to_owned(),
                    repo: "upstream".to_owned(),
                    number: 42,
                },
                "cross-repo body".to_owned(),
            )
            .await
            .expect("add_comment_ref CrossRepo should succeed");
        assert_eq!(
            url,
            "https://github.com/alpha/upstream/issues/42#issuecomment-cross"
        );
    }
}
