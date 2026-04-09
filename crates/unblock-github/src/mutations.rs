//! REST and GraphQL mutations.
//!
//! - `create_issue()` — REST POST
//! - `close_issue()` — REST PATCH
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
//!
//! REST mutations use the GitHub REST API for simplicity. Blocking and sub-issue
//! mutations use GraphQL because these features are only available via GraphQL.
//! Error handling follows the same pattern: 429 → `RateLimited`, 404 →
//! `IssueNotFound`, other non-2xx → `GitHubApi`.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use snafu::ResultExt as _;
use tracing::{debug, instrument, warn};
use unblock_core::types::Issue;

use crate::client::GitHubClient;
use crate::errors::{self, Error};
use crate::graphql::check_rest_response;

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

    /// Adds a comment to a GitHub issue.
    ///
    /// Sends a REST POST to `/repos/{owner}/{repo}/issues/{number}/comments`.
    /// Returns the HTML URL of the created comment.
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
        let url = self.rest_url(&format!(
            "/repos/{}/{}/issues/{number}/comments",
            self.owner(),
            self.repo()
        ));

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
                source: issue_number,
                target: blocked_by_number,
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

                let response = self.graphql(query, variables).await?;
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
    use wiremock::matchers::{method, path};
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
}
