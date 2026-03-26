//! REST and GraphQL mutations.
//!
//! - `create_issue()` — REST POST
//! - `close_issue()` — REST PATCH
//! - `add_comment()` — REST POST
//! - `add_blocked_by()` — GraphQL mutation (blocking relationship)
//! - `remove_blocked_by()` — GraphQL mutation (blocking relationship)
//! - `add_sub_issue()` — GraphQL mutation (sub-issue relationship, preview)
//!
//! REST mutations use the GitHub REST API for simplicity. Blocking and sub-issue
//! mutations use GraphQL because these features are only available via GraphQL.
//! Error handling follows the same pattern: 429 → `RateLimited`, 404 →
//! `IssueNotFound`, other non-2xx → `GitHubApi`.

use serde::{Deserialize, Serialize};
use snafu::ResultExt as _;
use tracing::{debug, instrument, warn};
use unblock_core::types::Issue;

use crate::client::GitHubClient;
use crate::errors::{self, Error};
use crate::graphql::parse_rate_limit_reset;

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

        let created: CreateIssueResponse = response
            .json()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        let number = created.number;
        let node_id = created.node_id;

        // Best-effort: add to project if configured.
        if let Some(project_number) = self.project_number()
            && let Err(e) = self.add_issue_to_project(&node_id, project_number).await
        {
            warn!(
                number,
                project_number,
                error = %e,
                "Failed to add issue to project (best-effort)"
            );
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
        // If reason is provided, add a comment first.
        if let Some(reason_text) = reason {
            self.add_comment(number, reason_text).await?;
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

        let status = response.status();

        if status.as_u16() == 429 {
            let reset_at = parse_rate_limit_reset(&response);
            return Err(errors::RateLimitedSnafu { reset_at }.build());
        }

        if status.as_u16() == 404 {
            return Err(unblock_core::errors::IssueNotFoundSnafu { number }
                .build()
                .into());
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

        let status = response.status();

        if status.as_u16() == 429 {
            let reset_at = parse_rate_limit_reset(&response);
            return Err(errors::RateLimitedSnafu { reset_at }.build());
        }

        if status.as_u16() == 404 {
            return Err(unblock_core::errors::IssueNotFoundSnafu { number }
                .build()
                .into());
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

        let comment: CreateCommentResponse = response
            .json()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        Ok(comment.html_url)
    }

    /// Adds an issue to a GitHub Projects V2 project.
    ///
    /// Uses the GraphQL `addProjectV2ItemById` mutation. The caller provides
    /// the issue `node_id` directly (from the REST create response), avoiding
    /// an extra REST GET round-trip.
    ///
    /// This is an internal helper — callers use `create_issue()` which calls
    /// this automatically when a project is configured.
    async fn add_issue_to_project(&self, node_id: &str, project_number: u64) -> Result<(), Error> {
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
            return Ok(());
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

        self.graphql(mutation, mutation_vars).await?;

        Ok(())
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
        let issue_id = self.resolve_node_id(issue_number).await?;
        let blocker_id = self.resolve_node_id(blocked_by_number).await?;

        // Pre-check: fetch the issue and see if the relationship already exists.
        let issue = self.fetch_issue(issue_number).await?;
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
}
