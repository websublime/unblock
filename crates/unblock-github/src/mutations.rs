//! REST and GraphQL mutations.
//!
//! - `create_issue()` — REST POST
//! - `close_issue()` — REST PATCH
//! - `add_comment()` — REST POST
//!
//! All mutations use the GitHub REST API for simplicity. Error handling follows
//! the same pattern as GraphQL: 429 → `RateLimited`, 404 → `IssueNotFound`,
//! other non-2xx → `GitHubApi`.

use serde::{Deserialize, Serialize};
use snafu::ResultExt as _;
use tracing::{instrument, warn};
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
/// Only the `number` field is needed — the full issue is re-fetched via
/// `fetch_issue()` to get Projects V2 fields and computed data.
#[derive(Debug, Deserialize)]
struct CreateIssueResponse {
    number: u64,
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

        // Best-effort: add to project if configured.
        if let Some(project_number) = self.project_number()
            && let Err(e) = self.add_issue_to_project(number, project_number).await
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
    /// Uses the GraphQL `addProjectV2ItemById` mutation. Requires the issue's
    /// node ID, which is fetched via a lightweight REST call first.
    ///
    /// This is an internal helper — callers use `create_issue()` which calls
    /// this automatically when a project is configured.
    async fn add_issue_to_project(
        &self,
        issue_number: u64,
        project_number: u64,
    ) -> Result<(), Error> {
        // Fetch the issue's node_id via REST (lightweight, avoids full GraphQL fetch).
        let url = self.rest_url(&format!(
            "/repos/{}/{}/issues/{issue_number}",
            self.owner(),
            self.repo()
        ));

        let response = self
            .http()
            .get(&url)
            .send()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        let status = response.status();

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

        let issue_json: serde_json::Value = response
            .json()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        let node_id = issue_json["node_id"]
            .as_str()
            .unwrap_or_default()
            .to_owned();

        if node_id.is_empty() {
            warn!(issue_number, "Issue has no node_id — cannot add to project");
            return Ok(());
        }

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
}
