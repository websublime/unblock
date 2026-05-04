//! GraphQL queries for GitHub API.
//!
//! - `fetch_graph_data()` — paginated query returning all issues
//!   (`OPEN` + `CLOSED`), blocking edges, and Projects V2 field values
//!   in a single request
//! - `fetch_issue()` — single issue with comments, deps, parent, sub-issues, and all fields

use chrono::{DateTime, Utc};
use snafu::ResultExt as _;
use tracing::{debug, instrument, warn};
use unblock_core::types::{
    BlockingEdge, Issue, IssueComment, IssueState, IssueType, PipelineStage, Priority,
    RelatedIssue, Status,
};

use crate::client::GitHubClient;
use crate::errors::{self, Error};

/// GraphQL query for fetching a single issue with full details.
///
/// Includes: all standard fields, comments (first 50), `blockedBy`,
/// `blocking`, `parent`, `subIssues`, and Projects V2 field values.
///
/// SAFETY: matches GitHub's public GraphQL schema as of 2026-04-30. The
/// `blockedBy` / `blocking` connections are GA on `Issue`, return
/// `IssueConnection!`, and require no `GraphQL-Features` preview header
/// (verified via live introspection against `api.github.com/graphql`).
/// `blockedBy` enumerates issues that block the current issue (= our
/// `blocked_by`); `blocking` enumerates issues the current issue blocks
/// (= our `blocking`). The `repository { owner { login } name }`
/// subselection on `blockedBy` is load-bearing for cross-repo blocker
/// disambiguation in `dep_remove` (see `unblock-29p.43`); do not drop it.
const FETCH_ISSUE_QUERY: &str = "
query FetchIssue($owner: String!, $repo: String!, $number: Int!) {
  repository(owner: $owner, name: $repo) {
    issue(number: $number) {
      id
      number
      title
      body
      state
      url
      createdAt
      updatedAt
      issueType {
        name
      }
      labels(first: 50) {
        nodes {
          name
        }
      }
      milestone {
        title
      }
      assignees(first: 20) {
        nodes {
          login
        }
      }
      comments(first: 50) {
        nodes {
          author {
            login
          }
          body
          createdAt
        }
      }
      blocking(first: 50) {
        nodes {
          number
          title
          state
        }
      }
      blockedBy(first: 50) {
        nodes {
          number
          title
          state
          repository {
            name
            owner {
              login
            }
          }
        }
      }
      parent {
        number
        title
        state
      }
      subIssues(first: 50) {
        nodes {
          number
          title
          state
        }
      }
      projectItems(first: 10) {
        nodes {
          fieldValues(first: 20) {
            nodes {
              ... on ProjectV2ItemFieldTextValue {
                field { ... on ProjectV2FieldCommon { name } }
                text
              }
              ... on ProjectV2ItemFieldNumberValue {
                field { ... on ProjectV2FieldCommon { name } }
                number
              }
              ... on ProjectV2ItemFieldDateValue {
                field { ... on ProjectV2FieldCommon { name } }
                date
              }
              ... on ProjectV2ItemFieldSingleSelectValue {
                field { ... on ProjectV2FieldCommon { name } }
                name
              }
            }
          }
        }
      }
    }
  }
}
";

/// GraphQL query for fetching all issues (both `OPEN` and `CLOSED`)
/// with pagination.
///
/// Returns issues with standard fields, blocking relationships (via the
/// `blockedBy` connection — see SAFETY note below), and Projects V2 field
/// values. Does **not** include comments, parent, sub-issues, or the
/// `blocking` connection — those are only fetched by [`FETCH_ISSUE_QUERY`]
/// for single-issue detail views.
///
/// SAFETY: matches GitHub's public GraphQL schema as of 2026-04-30. The
/// `blockedBy` connection is GA on `Issue`, returns `IssueConnection!`,
/// and requires no `GraphQL-Features` preview header (verified via live
/// introspection against `api.github.com/graphql`). The number-only node
/// projection here is sufficient for [`extract_blocking_edges`] which
/// only consumes `number`; per-blocker `repository { ... }` is intentionally
/// omitted because graph edges in this query are scoped to the configured
/// repo (cross-repo identity is reconstructed by the orchestrator from the
/// configured `(owner, repo)` per [`BlockingEdge`]).
///
/// Uses cursor-based pagination on the `issues` connection (`first: 100`,
/// `after: $cursor`). The caller must loop until `pageInfo.hasNextPage` is
/// false.
///
/// The `states: [OPEN, CLOSED]` filter matches SPEC §5.5 literally and
/// ensures every downstream consumer that reads from the cache (`list`,
/// `stats`, `dep_cycles`, `reopen`, `close`, `dep_remove`, `prime`,
/// `reconcile`) sees the full issue universe. Issues that do not yet
/// exist on GitHub are naturally absent from both pages. Closed issues
/// enter the cache with `IssueState::Closed`; `compute_ready_set` still
/// excludes them (graph Filter 1), and the cascade / cycle helpers
/// already key on [`IssueState`] so the open/closed boundary is
/// preserved on the graph layer.
const FETCH_GRAPH_DATA_QUERY: &str = "
query FetchGraphData($owner: String!, $repo: String!, $cursor: String) {
  repository(owner: $owner, name: $repo) {
    issues(first: 100, states: [OPEN, CLOSED], after: $cursor) {
      pageInfo {
        endCursor
        hasNextPage
      }
      nodes {
        id
        number
        title
        body
        state
        url
        createdAt
        updatedAt
        issueType {
          name
        }
        labels(first: 50) {
          nodes {
            name
          }
        }
        milestone {
          title
        }
        assignees(first: 20) {
          nodes {
            login
          }
        }
        blockedBy(first: 50) {
          nodes {
            number
          }
        }
        projectItems(first: 10) {
          nodes {
            fieldValues(first: 20) {
              nodes {
                ... on ProjectV2ItemFieldTextValue {
                  field { ... on ProjectV2FieldCommon { name } }
                  text
                }
                ... on ProjectV2ItemFieldNumberValue {
                  field { ... on ProjectV2FieldCommon { name } }
                  number
                }
                ... on ProjectV2ItemFieldDateValue {
                  field { ... on ProjectV2FieldCommon { name } }
                  date
                }
                ... on ProjectV2ItemFieldSingleSelectValue {
                  field { ... on ProjectV2FieldCommon { name } }
                  name
                }
              }
            }
          }
        }
      }
    }
  }
}
";

impl GitHubClient {
    /// Fetches a single issue with full details from GitHub.
    ///
    /// Returns the issue with all standard fields, comments (up to 50),
    /// blocking relationships, parent/sub-issue links, and Projects V2
    /// field values.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Domain`] with [`DomainError::IssueNotFound`] if
    /// the issue number does not exist. Returns network or GraphQL errors
    /// for infrastructure failures.
    ///
    /// [`DomainError::IssueNotFound`]: unblock_core::errors::DomainError::IssueNotFound
    #[instrument(skip(self), fields(owner = %self.owner(), repo = %self.repo()))]
    pub async fn fetch_issue(&self, number: u64) -> Result<Issue, Error> {
        self.fetch_issue_in_repo(self.owner(), self.repo(), number)
            .await
    }

    /// Fetches a single issue by owner/repo/number.
    ///
    /// Like [`fetch_issue`](Self::fetch_issue), but targets an arbitrary
    /// `owner/repo` instead of the configured repository. Used to back
    /// [`fetch_issue_ref`](Self::fetch_issue_ref) for cross-repo reads.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Domain`] with [`DomainError::IssueNotFound`] if the
    /// issue number does not exist in the target repository.
    ///
    /// [`DomainError::IssueNotFound`]: unblock_core::errors::DomainError::IssueNotFound
    #[instrument(skip(self))]
    pub async fn fetch_issue_in_repo(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<Issue, Error> {
        let variables = serde_json::json!({
            "owner": owner,
            "repo": repo,
            "number": number.cast_signed(),
        });

        // Per SPEC §11.1 / plan Task 02.02 "Error-side wiring", a 403
        // (HTTP) or a GraphQL FORBIDDEN response on a cross-repo fetch
        // MUST be upgraded to `DomainError::CrossRepoAccessDenied`.
        // Local fetches (owner/repo == configured) stay as the
        // underlying infrastructure error — `CrossRepoAccessDenied`
        // is cross-repo-semantic by name.
        let is_cross_repo = owner != self.owner() || repo != self.repo();
        let response = self
            .graphql(FETCH_ISSUE_QUERY, variables)
            .await
            .map_err(|err| {
                if is_cross_repo {
                    classify_cross_repo_fetch(err, owner, repo)
                } else {
                    err
                }
            })?;

        let issue_value = &response["data"]["repository"]["issue"];

        if issue_value.is_null() {
            return Err(unblock_core::errors::IssueNotFoundSnafu { number }
                .build()
                .into());
        }

        Ok(parse_issue(issue_value, owner, repo))
    }

    /// Fetches a single issue identified by an [`IssueRef`].
    ///
    /// Resolves `Local` refs against the configured repository and
    /// `CrossRepo` refs against the specified `owner/repo`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Domain`] with [`DomainError::IssueNotFound`] if the
    /// referenced issue does not exist.
    ///
    /// [`IssueRef`]: unblock_core::types::IssueRef
    /// [`DomainError::IssueNotFound`]: unblock_core::errors::DomainError::IssueNotFound
    #[instrument(skip(self))]
    pub async fn fetch_issue_ref(
        &self,
        issue_ref: &unblock_core::types::IssueRef,
    ) -> Result<Issue, Error> {
        match issue_ref {
            unblock_core::types::IssueRef::Local(number) => self.fetch_issue(*number).await,
            unblock_core::types::IssueRef::CrossRepo {
                owner,
                repo,
                number,
            } => self.fetch_issue_in_repo(owner, repo, *number).await,
        }
    }

    /// Fetches all issues (`OPEN` + `CLOSED`) and blocking edges for the
    /// dependency graph.
    ///
    /// Returns a tuple of `(issues, edges)` where:
    /// - `issues` contains every `OPEN` and `CLOSED` issue with standard
    ///   fields and Projects V2 field values, but **not** comments,
    ///   parent, sub-issues, or the `blocked_by`/`blocking` vectors on
    ///   [`Issue`] (those remain empty). Closed issues enter the result
    ///   with `state == IssueState::Closed` so downstream consumers can
    ///   honour the open/closed boundary on the graph layer
    ///   (`compute_ready_set` continues to exclude closed issues,
    ///   `compute_unblock_cascade` keys on `issue_state`, etc.).
    /// - `edges` contains [`BlockingEdge`] entries extracted from GitHub's
    ///   `blockedBy` connection (schema as of 2026-04-30), where `source`
    ///   is the blocked issue and `target` is the blocker.
    ///
    /// Paginates using GraphQL cursor pagination (100 issues per page) until
    /// every issue is fetched. Returns empty vectors for a repo with no
    /// issues at all.
    ///
    /// # Errors
    ///
    /// Returns [`Error::GitHubGraphQL`] if the GraphQL response contains errors.
    /// Returns [`Error::GitHubApi`] for non-2xx HTTP responses.
    /// Returns [`Error::RateLimited`] for HTTP 429 responses.
    /// Returns [`Error::GitHubUnavailable`] for network failures.
    #[instrument(skip(self), fields(owner = %self.owner(), repo = %self.repo()))]
    pub async fn fetch_graph_data(&self) -> Result<(Vec<Issue>, Vec<BlockingEdge>), Error> {
        let mut all_issues = Vec::new();
        let mut all_edges = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let variables = serde_json::json!({
                "owner": self.owner(),
                "repo": self.repo(),
                "cursor": cursor,
            });

            let response = self.graphql(FETCH_GRAPH_DATA_QUERY, variables).await?;

            let issues_connection = &response["data"]["repository"]["issues"];

            // Parse issue nodes from this page.
            if let Some(nodes) = issues_connection
                .get("nodes")
                .and_then(serde_json::Value::as_array)
            {
                for node in nodes {
                    // Extract blocking edges from the `blockedBy` connection
                    // (per FETCH_GRAPH_DATA_QUERY SAFETY note — schema as of
                    // 2026-04-30).
                    all_edges.extend(extract_blocking_edges(node, self.owner(), self.repo()));

                    // Parse issue with graph-specific parser (omits comments,
                    // blocked_by, blocking, parent, sub_issues).
                    all_issues.push(parse_graph_issue(node, self.owner(), self.repo()));
                }
            }

            // Check pagination: advance cursor or break.
            let page_info = &issues_connection["pageInfo"];
            let has_next_page = page_info
                .get("hasNextPage")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);

            if !has_next_page {
                break;
            }

            let next_cursor = page_info
                .get("endCursor")
                .and_then(serde_json::Value::as_str)
                .map(String::from);

            // Guard against infinite loop: if GitHub returns hasNextPage:true
            // but endCursor is null, cursor would reset to None and re-fetch
            // the first page forever. Break to prevent this.
            if next_cursor.is_none() {
                warn!(
                    "GitHub API returned hasNextPage=true but endCursor=null; \
                     stopping pagination to avoid infinite loop"
                );
                break;
            }

            cursor = next_cursor;
        }

        debug!(
            issues = all_issues.len(),
            edges = all_edges.len(),
            "fetch_graph_data complete"
        );

        Ok((all_issues, all_edges))
    }

    /// Sends a GraphQL query to the GitHub API.
    ///
    /// Posts the query and variables as JSON to the GraphQL endpoint.
    /// Handles GraphQL-level errors (errors array in response) and
    /// HTTP-level errors (non-2xx status codes, rate limiting).
    ///
    /// This is a convenience wrapper around
    /// [`graphql_with_features()`](Self::graphql_with_features) with no
    /// preview features enabled.
    ///
    /// # Errors
    ///
    /// Returns [`Error::GitHubGraphQL`] if the response contains GraphQL errors.
    /// Returns [`Error::GitHubApi`] for non-2xx HTTP responses.
    /// Returns [`Error::RateLimited`] for HTTP 429 responses.
    /// Returns [`Error::GitHubUnavailable`] for network failures.
    #[instrument(skip(self, query, variables))]
    pub(crate) async fn graphql(
        &self,
        query: &str,
        variables: serde_json::Value,
    ) -> Result<serde_json::Value, Error> {
        self.graphql_with_features(query, variables, &[]).await
    }

    /// Sends a GraphQL query with optional `GraphQL-Features` header values.
    ///
    /// Posts the query and variables as JSON to the GraphQL endpoint.
    /// When `features` is non-empty, appends a `GraphQL-Features` header with
    /// the given feature names (comma-separated). This is required for preview
    /// API features such as `sub_issues`.
    ///
    /// Handles GraphQL-level errors (errors array in response) and
    /// HTTP-level errors (non-2xx status codes, rate limiting).
    ///
    /// # Errors
    ///
    /// Returns [`Error::GitHubGraphQL`] if the response contains GraphQL errors.
    /// Returns [`Error::GitHubApi`] for non-2xx HTTP responses.
    /// Returns [`Error::RateLimited`] for HTTP 429 responses.
    /// Returns [`Error::GitHubUnavailable`] for network failures.
    #[instrument(skip(self, query, variables, features))]
    pub(crate) async fn graphql_with_features(
        &self,
        query: &str,
        variables: serde_json::Value,
        features: &[&str],
    ) -> Result<serde_json::Value, Error> {
        let url = self.graphql_url();
        let body = serde_json::json!({
            "query": query,
            "variables": variables,
        });

        let mut request = self.http().post(&url);

        if features.is_empty() {
            debug!(url = %url, "Sending GraphQL request");
        } else {
            let features_value = features.join(",");
            debug!(url = %url, features = %features_value, "Sending GraphQL request with features");
            request = request.header("GraphQL-Features", &features_value);
        }

        let response = request
            .json(&body)
            .send()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        let response = check_rest_response(response).await?;

        let json: serde_json::Value = response
            .json()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        // Check for GraphQL-level errors.
        //
        // Inspect each error's `type` BEFORE reducing to message
        // strings: GitHub's GraphQL API returns HTTP 200 with a typed
        // `errors` array for permission-denied cases, and the `type`
        // field (e.g. `"FORBIDDEN"`) is the wire-safe signal rather
        // than the free-text `message`. Per SPEC §11.1 wiring (user
        // decision 2026-04-17 for unblock-6xj), we partition the
        // errors into FORBIDDEN-typed and other-typed before emitting
        // the appropriate variant, so cross-repo classifiers can
        // upgrade FORBIDDEN to `DomainError::CrossRepoAccessDenied`
        // without substring-sniffing messages.
        if let Some(arr) = json
            .get("errors")
            .and_then(serde_json::Value::as_array)
            .filter(|a| !a.is_empty())
        {
            // Pre-scan: `type == "FORBIDDEN"` is the authoritative
            // wire signal for the variant choice — the VARIANT (not
            // its `errors` vector contents) is driven by type
            // presence, independent of whether any FORBIDDEN entry
            // carries a populated `message`. This is load-bearing:
            // the message-partition loop below filters empty-message
            // entries for hygiene (unblock-eos.22), and without this
            // pre-scan an all-FORBIDDEN-with-empty-messages payload
            // would silently fall through to the non-FORBIDDEN bucket
            // and defeat the cross-repo 403 classifier. See
            // unblock-eos.24.
            let has_forbidden = arr
                .iter()
                .any(|e| e.get("type").and_then(serde_json::Value::as_str) == Some("FORBIDDEN"));
            let mut forbidden_messages: Vec<String> = Vec::new();
            let mut other_messages: Vec<String> = Vec::new();
            for err in arr {
                // Skip entries without a non-empty `message`: a missing
                // or empty message carries no information and would
                // silently pollute the message vectors (previously a
                // FORBIDDEN entry with no message body produced an
                // empty-string entry in `forbidden_messages`).
                let Some(message) = err
                    .get("message")
                    .and_then(|m| m.as_str())
                    .filter(|m| !m.is_empty())
                    .map(str::to_owned)
                else {
                    continue;
                };
                let ty = err.get("type").and_then(|t| t.as_str()).unwrap_or_default();
                if ty == "FORBIDDEN" {
                    forbidden_messages.push(message);
                } else {
                    other_messages.push(message);
                }
            }
            if has_forbidden {
                return Err(errors::GitHubGraphQLForbiddenSnafu {
                    errors: forbidden_messages,
                }
                .build());
            }
            return Err(errors::GitHubGraphQLSnafu {
                errors: other_messages,
            }
            .build());
        }

        Ok(json)
    }
}

/// Checks an HTTP response for rate limiting and non-2xx status codes.
///
/// Consumes the response on the error path (reading the body as text for the
/// [`Error::GitHubApi`](errors::Error::GitHubApi) variant's `message` field),
/// and passes the response through unchanged on success so callers can still
/// deserialize the JSON body.
///
/// Behavior:
/// - HTTP 429: returns [`Error::RateLimited`](errors::Error::RateLimited) with
///   `reset_at` parsed from the `X-RateLimit-Reset` header.
/// - Non-2xx (other than 429): reads the body as text and returns
///   [`Error::GitHubApi`](errors::Error::GitHubApi).
/// - 2xx: returns `Ok(response)`.
///
/// Callers that need to distinguish HTTP 404 (to surface
/// [`IssueNotFound`](unblock_core::errors::DomainError::IssueNotFound)) must
/// check `response.status()` for 404 **before** calling this helper, otherwise
/// 404 will be collapsed into a generic `GitHubApi` error.
pub(crate) async fn check_rest_response(
    response: reqwest::Response,
) -> Result<reqwest::Response, errors::Error> {
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

    Ok(response)
}

/// Upgrades a cross-repo fetch error to
/// [`DomainError::CrossRepoAccessDenied`] when the underlying failure
/// is an HTTP 403 ([`Error::GitHubApi`]) or a GraphQL FORBIDDEN
/// ([`Error::GitHubGraphQLForbidden`]).
///
/// Other errors pass through unchanged. Per SPEC §11.1 / plan Task
/// 02.02 "Error-side wiring" this is the cross-repo-only classifier:
/// local-repo 403s stay as [`Error::GitHubApi`] because
/// `CrossRepoAccessDenied` is cross-repo-semantic by name.
///
/// [`DomainError::CrossRepoAccessDenied`]:
///     unblock_core::errors::DomainError::CrossRepoAccessDenied
/// [`Error::GitHubApi`]: errors::Error::GitHubApi
/// [`Error::GitHubGraphQLForbidden`]: errors::Error::GitHubGraphQLForbidden
pub(crate) fn classify_cross_repo_fetch(
    err: errors::Error,
    owner: &str,
    repo: &str,
) -> errors::Error {
    match err {
        errors::Error::GitHubApi { status: 403, .. }
        | errors::Error::GitHubGraphQLForbidden { .. } => {
            unblock_core::errors::CrossRepoAccessDeniedSnafu {
                owner: owner.to_owned(),
                repo: repo.to_owned(),
            }
            .build()
            .into()
        }
        other => other,
    }
}

/// Parses the `X-RateLimit-Reset` header from a response into a `DateTime<Utc>`.
///
/// Falls back to `Utc::now()` if the header is missing or unparseable.
pub(crate) fn parse_rate_limit_reset(response: &reqwest::Response) -> DateTime<Utc> {
    response
        .headers()
        .get("x-ratelimit-reset")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<i64>().ok())
        .and_then(|ts| DateTime::from_timestamp(ts, 0))
        .unwrap_or_else(Utc::now)
}

/// Parses a GraphQL issue JSON value into a domain [`Issue`].
///
/// Extracts all standard fields, comments, blocking relationships,
/// parent/sub-issue links, and Projects V2 field values from the
/// GraphQL response. Missing fields use sensible defaults.
fn parse_issue(value: &serde_json::Value, owner: &str, repo: &str) -> Issue {
    let number = json_u64(value, "number");
    let node_id = json_string(value, "id");
    let title = json_string(value, "title");
    let body = value.get("body").and_then(|v| v.as_str()).map(String::from);
    let state = parse_issue_state(value);
    let url = json_string(value, "url");
    let created_at = parse_datetime(value, "createdAt");
    let updated_at = parse_datetime(value, "updatedAt");

    let labels = parse_string_nodes(value, "labels", "name");
    let milestone = value
        .get("milestone")
        .and_then(|m| m.get("title"))
        .and_then(|t| t.as_str())
        .map(String::from);
    let assignees = parse_string_nodes(value, "assignees", "login");

    let comments = parse_comments(value);
    // Response keys match the GraphQL field names selected by
    // FETCH_ISSUE_QUERY. `blockedBy` carries cross-repo identity via the
    // nested `repository { owner { login } name }` subselection; `blocking`
    // omits it (intra-repo only on the read path) — consistent with the
    // SAFETY note on FETCH_ISSUE_QUERY (schema as of 2026-04-30).
    let blocked_by = parse_related_issues(value, "blockedBy");
    let blocking = parse_related_issues(value, "blocking");
    let parent = parse_parent_issue(value);
    let sub_issues = parse_related_issues(value, "subIssues");

    // Extract Projects V2 field values.
    let field_values = extract_field_values(value);
    let status = parse_status_field(&field_values);
    let priority = parse_priority_field(&field_values);
    // IssueType is GitHub native (spec §2.6) — read it off the issue
    // node's `issueType { name }` selection rather than from a Projects
    // V2 SingleSelect HashMap (the prior reader was a drift bug —
    // unblock-wgj.15).
    let issue_type = parse_issue_type_from_native(value);
    let pipeline_stage = parse_pipeline_stage_field(&field_values);
    let agent = field_values.get("Agent").cloned();
    let story_points = field_values
        .get("StoryPoints")
        .and_then(|v| v.parse::<i32>().ok());
    let defer_until = field_values
        .get("DeferUntil")
        .and_then(|v| chrono::NaiveDate::parse_from_str(v, "%Y-%m-%d").ok());
    let claimed_at = field_values
        .get("ClaimedAt")
        .and_then(|v| v.parse::<DateTime<Utc>>().ok());

    Issue {
        qualified_id: unblock_core::types::QualifiedId::new(owner, repo, number),
        number,
        node_id,
        title,
        issue_type,
        status,
        priority,
        agent,
        claimed_at,
        pipeline_stage,
        story_points,
        defer_until,
        labels,
        milestone,
        assignees,
        state,
        body,
        created_at,
        updated_at,
        url,
        comments,
        blocked_by,
        blocking,
        parent,
        sub_issues,
    }
}

/// Parses a GraphQL issue JSON value into a domain [`Issue`] for bulk graph data.
///
/// Similar to [`parse_issue`] but leaves `comments`, `blocked_by`, `blocking`,
/// `parent`, and `sub_issues` empty — per the [`Issue`] type contract, those
/// fields are only populated by `fetch_issue()`. Blocking relationships are
/// extracted separately as [`BlockingEdge`] entries by the caller.
fn parse_graph_issue(value: &serde_json::Value, owner: &str, repo: &str) -> Issue {
    let number = json_u64(value, "number");
    let node_id = json_string(value, "id");
    let title = json_string(value, "title");
    let body = value.get("body").and_then(|v| v.as_str()).map(String::from);
    let state = parse_issue_state(value);
    let url = json_string(value, "url");
    let created_at = parse_datetime(value, "createdAt");
    let updated_at = parse_datetime(value, "updatedAt");

    let labels = parse_string_nodes(value, "labels", "name");
    let milestone = value
        .get("milestone")
        .and_then(|m| m.get("title"))
        .and_then(|t| t.as_str())
        .map(String::from);
    let assignees = parse_string_nodes(value, "assignees", "login");

    // Extract Projects V2 field values.
    let field_values = extract_field_values(value);
    let status = parse_status_field(&field_values);
    let priority = parse_priority_field(&field_values);
    // IssueType is GitHub native (spec §2.6) — read it off the issue
    // node's `issueType { name }` selection rather than from a Projects
    // V2 SingleSelect HashMap (the prior reader was a drift bug —
    // unblock-wgj.15).
    let issue_type = parse_issue_type_from_native(value);
    let pipeline_stage = parse_pipeline_stage_field(&field_values);
    let agent = field_values.get("Agent").cloned();
    let story_points = field_values
        .get("StoryPoints")
        .and_then(|v| v.parse::<i32>().ok());
    let defer_until = field_values
        .get("DeferUntil")
        .and_then(|v| chrono::NaiveDate::parse_from_str(v, "%Y-%m-%d").ok());
    let claimed_at = field_values
        .get("ClaimedAt")
        .and_then(|v| v.parse::<DateTime<Utc>>().ok());

    Issue {
        qualified_id: unblock_core::types::QualifiedId::new(owner, repo, number),
        number,
        node_id,
        title,
        issue_type,
        status,
        priority,
        agent,
        claimed_at,
        pipeline_stage,
        story_points,
        defer_until,
        labels,
        milestone,
        assignees,
        state,
        body,
        created_at,
        updated_at,
        url,
        // Graph data does not include these — populated by fetch_issue() only.
        comments: Vec::new(),
        blocked_by: Vec::new(),
        blocking: Vec::new(),
        parent: None,
        sub_issues: Vec::new(),
    }
}

/// Extracts [`BlockingEdge`] entries from a single issue JSON node.
///
/// Reads the `blockedBy` connection (per `FETCH_GRAPH_DATA_QUERY` SAFETY
/// note, schema as of 2026-04-30) and creates one edge per blocker. Each
/// edge has `source` = the blocked issue's [`QualifiedId`] and `target` =
/// the blocker's [`QualifiedId`]. Skips entries with a zero issue number.
///
/// Currently all edges are within the same `owner/repo` because the bulk
/// graph query selects only `number` on each `blockedBy` node. Cross-repo
/// blockers (added via the `depends` MCP tool) will produce edges with
/// different `owner/repo` prefixes once the bulk query is extended to
/// select per-blocker `repository { ... }`.
fn extract_blocking_edges(node: &serde_json::Value, owner: &str, repo: &str) -> Vec<BlockingEdge> {
    let issue_number = json_u64(node, "number");
    let source_qid = unblock_core::types::QualifiedId::new(owner, repo, issue_number);
    let mut edges = Vec::new();

    if let Some(blockers) = node
        .get("blockedBy")
        .and_then(|v| v.get("nodes"))
        .and_then(|v| v.as_array())
    {
        for blocker in blockers {
            let blocker_number = json_u64(blocker, "number");
            if blocker_number > 0 {
                edges.push(BlockingEdge {
                    source: source_qid.clone(),
                    target: unblock_core::types::QualifiedId::new(owner, repo, blocker_number),
                });
            }
        }
    }

    edges
}

/// Extracts a `u64` from a JSON object by key.
fn json_u64(value: &serde_json::Value, key: &str) -> u64 {
    value
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default()
}

/// Extracts a `String` from a JSON object by key.
fn json_string(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_owned()
}

/// Parses a datetime field from a JSON object.
fn parse_datetime(value: &serde_json::Value, key: &str) -> DateTime<Utc> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<DateTime<Utc>>().ok())
        .unwrap_or_else(Utc::now)
}

/// Parses the `state` field into an [`IssueState`].
fn parse_issue_state(value: &serde_json::Value) -> IssueState {
    match value.get("state").and_then(|v| v.as_str()) {
        Some("CLOSED") => IssueState::Closed,
        _ => IssueState::Open,
    }
}

/// Extracts string values from a connection's nodes by a nested field name.
///
/// Handles the pattern: `{ key: { nodes: [ { field: "value" }, ... ] } }`
fn parse_string_nodes(value: &serde_json::Value, key: &str, field: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(|v| v.get("nodes"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|node| node.get(field).and_then(|v| v.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Parses the comments connection into [`IssueComment`] instances.
fn parse_comments(value: &serde_json::Value) -> Vec<IssueComment> {
    value
        .get("comments")
        .and_then(|v| v.get("nodes"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|node| {
                    let author = node
                        .get("author")
                        .and_then(|a| a.get("login"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("ghost")
                        .to_owned();
                    let body = json_string(node, "body");
                    let created_at = parse_datetime(node, "createdAt");
                    IssueComment {
                        author,
                        body,
                        created_at,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parses a related-issues connection (`blockedBy`, `blocking`,
/// `subIssues`) into [`RelatedIssue`] instances.
///
/// When an individual node carries a complete `repository { owner { login }
/// name }` subfield set, those values populate the resulting
/// [`RelatedIssue::repo_owner`] / [`RelatedIssue::repo_name`] via
/// [`RelatedIssue::cross_repo`]. The `blockedBy` connection inside
/// [`FETCH_ISSUE_QUERY`] selects those fields so blocker edges can
/// disambiguate cross-repo blockers from same-repo blockers (required by
/// `dep_remove` single-issue edge validation on cross-repo paths — see
/// `unblock-29p.43`).
///
/// Connections that omit the `repository` selection (e.g. `subIssues`,
/// `blocking`, `parent`) and nodes whose `repository` subfield is partial
/// (only `owner` or only `name` present — a malformed GraphQL response)
/// route through [`RelatedIssue::local`], leaving both repo identity
/// fields `None`. A partial subfield cannot disambiguate a cross-repo
/// reference (identification requires the full `(owner, name)` pair) so
/// treating it as "no identity" is the semantically coherent fallback.
fn parse_related_issues(value: &serde_json::Value, key: &str) -> Vec<RelatedIssue> {
    value
        .get(key)
        .and_then(|v| v.get("nodes"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|node| {
                    let number = json_u64(node, "number");
                    let title = json_string(node, "title");
                    let state = parse_issue_state(node);
                    match (
                        parse_related_repo_owner(node),
                        parse_related_repo_name(node),
                    ) {
                        (Some(owner), Some(name)) => {
                            RelatedIssue::cross_repo(number, title, state, owner, name)
                        }
                        _ => RelatedIssue::local(number, title, state),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parses the optional `repository.owner.login` subfield of a related-
/// issue node. Returns `None` when the GraphQL selection omitted the
/// subfield or when the value is not a string.
fn parse_related_repo_owner(node: &serde_json::Value) -> Option<String> {
    node.get("repository")
        .and_then(|r| r.get("owner"))
        .and_then(|o| o.get("login"))
        .and_then(|l| l.as_str())
        .map(String::from)
}

/// Parses the optional `repository.name` subfield of a related-issue
/// node. Returns `None` when the GraphQL selection omitted the subfield
/// or when the value is not a string.
fn parse_related_repo_name(node: &serde_json::Value) -> Option<String> {
    node.get("repository")
        .and_then(|r| r.get("name"))
        .and_then(|n| n.as_str())
        .map(String::from)
}

/// Parses the parent issue field into an optional [`RelatedIssue`].
///
/// The parent selection does not include `repository { ... }`, so the
/// result always routes through [`RelatedIssue::local`] — callers
/// interpret the resulting `None` `repo_owner` / `repo_name` as
/// "same repo as the containing issue" per the `RelatedIssue` docs.
fn parse_parent_issue(value: &serde_json::Value) -> Option<RelatedIssue> {
    let parent = value.get("parent")?;
    if parent.is_null() {
        return None;
    }
    Some(RelatedIssue::local(
        json_u64(parent, "number"),
        json_string(parent, "title"),
        parse_issue_state(parent),
    ))
}

/// Extracts all Projects V2 field values into a name-to-value map.
///
/// Iterates over all `projectItems` and their `fieldValues`, collecting
/// field name/value pairs. For single-select fields, uses the option name.
/// For text/number/date fields, uses the raw value.
fn extract_field_values(value: &serde_json::Value) -> std::collections::HashMap<String, String> {
    let mut fields = std::collections::HashMap::new();

    let Some(project_items) = value
        .get("projectItems")
        .and_then(|v| v.get("nodes"))
        .and_then(|v| v.as_array())
    else {
        return fields;
    };

    for item in project_items {
        let Some(field_values) = item
            .get("fieldValues")
            .and_then(|v| v.get("nodes"))
            .and_then(|v| v.as_array())
        else {
            continue;
        };

        for fv in field_values {
            let Some(field_name) = fv
                .get("field")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
            else {
                continue;
            };

            // Single select: has a top-level "name" key (the option name).
            if let Some(option_name) = fv.get("name").and_then(|v| v.as_str()) {
                fields.insert(field_name.to_owned(), option_name.to_owned());
                continue;
            }

            // Text field.
            if let Some(text) = fv.get("text").and_then(|v| v.as_str()) {
                fields.insert(field_name.to_owned(), text.to_owned());
                continue;
            }

            // Number field.
            if let Some(num) = fv.get("number").and_then(serde_json::Value::as_f64) {
                fields.insert(field_name.to_owned(), num.to_string());
                continue;
            }

            // Date field.
            if let Some(date) = fv.get("date").and_then(|v| v.as_str()) {
                fields.insert(field_name.to_owned(), date.to_owned());
            }
        }
    }

    fields
}

/// Maps the "Status" field value to a [`Status`] enum variant.
///
/// **Canonical wire format (post-`unblock-1zj`):** the `TitleCase` strings
/// produced by [`Status::option_name`] — `Backlog`, `Ready`, `In Progress`,
/// `Blocked`, `Deferred`, `Closed`. These are the only values written by
/// the system after `unblock-1zj`.
///
/// **Legacy aliases (parse-only, never written):** to keep boards mid-
/// migration parsing correctly during the auto-heal window, the parser
/// also accepts:
///
/// - the prior lowercase / `snake_case` set used pre-`unblock-1zj`:
///   `ready`, `in_progress`, `blocked`, `deferred`, `closed`;
/// - the GitHub-default built-in field's `Done` / `Todo` options that
///   pre-existed any unblock setup pass.
///
/// **Round-trip contract (Invariant 16, §14, Appendix A.3 obligation 1).**
/// For every variant `s`, `parse_status_field({"Status" -> s.option_name()})
/// == s`. This is the single source of truth contract — wire strings written
/// by the system (`Status::option_name`) round-trip through this parser.
#[allow(
    clippy::match_same_arms,
    reason = "explicit `Todo`/missing arms document the legacy-alias contract; \
              collapsing into the wildcard would lose intent"
)]
fn parse_status_field(fields: &std::collections::HashMap<String, String>) -> Status {
    match fields.get("Status").map(String::as_str) {
        // Canonical `TitleCase` (post-`unblock-1zj`, sourced from
        // `Status::option_name` — see Invariant 16).
        Some(s) if s == Status::Backlog.option_name() => Status::Backlog,
        Some(s) if s == Status::Ready.option_name() => Status::Ready,
        Some(s) if s == Status::InProgress.option_name() => Status::InProgress,
        Some(s) if s == Status::Blocked.option_name() => Status::Blocked,
        Some(s) if s == Status::Deferred.option_name() => Status::Deferred,
        Some(s) if s == Status::Closed.option_name() => Status::Closed,
        // Legacy lowercase / `snake_case` — pre-`unblock-1zj` boards
        // mid-migration. Accepted for parsing only; the auto-heal pass
        // renames them to the canonical `TitleCase` strings on the next
        // `setup` call. The `Done` (legacy GitHub-default built-in
        // field option, pre-any unblock setup pass) is folded into the
        // `Closed` arm; `Todo` (the analogous default option) is the
        // only legacy alias for `Backlog`.
        Some("in_progress") => Status::InProgress,
        Some("blocked") => Status::Blocked,
        Some("deferred") => Status::Deferred,
        Some("closed" | "Done") => Status::Closed,
        Some("ready") => Status::Ready,
        // Symmetry with the other lowercase legacy aliases above. Same
        // mapping the wildcard arm produces, but explicit so the arm-set
        // matches the canonical 6-entry list and a future variant
        // addition does not accidentally fall through to the wildcard
        // for the `backlog` lowercase form.
        Some("backlog") => Status::Backlog,
        Some("Todo") => Status::Backlog,
        // Missing or unrecognized -> Backlog (sticky default for unmanaged
        // items, consistent with the create-time default).
        _ => Status::Backlog,
    }
}

/// Maps the "Priority" field value to a [`Priority`] enum variant.
///
/// Handles both the current spec option names (`P0 - Critical`, `P1 - High`,
/// etc.) and the legacy short names (`P0`, `P1`, etc.) for backward
/// compatibility. Uses prefix matching so `"P0 - Critical"` maps to `P0`.
fn parse_priority_field(fields: &std::collections::HashMap<String, String>) -> Priority {
    match fields.get("Priority").map(String::as_str) {
        Some(s) if s.starts_with("P0") => Priority::P0,
        Some(s) if s.starts_with("P1") => Priority::P1,
        Some(s) if s.starts_with("P3") => Priority::P3,
        Some(s) if s.starts_with("P4") => Priority::P4,
        // Default to medium (P2) when missing, unrecognized, or "P2*".
        _ => Priority::P2,
    }
}

/// Reads GitHub's native `issueType { name }` field off an issue JSON
/// node and resolves it to an [`IssueType`] variant.
///
/// Per spec §2.6, `IssueType` is GitHub's native org-level issue type
/// (NOT a Projects V2 custom field). The previous `HashMap`-based reader
/// keyed on a Projects V2 `SingleSelect` option named `"IssueType"` was a
/// drift bug — that field never exists on real boards, so
/// `issue.issue_type` always resolved to `None` (verified empirically
/// via `gh api graphql` against the live API).
///
/// Resolution routes through [`IssueType::from_canonical_name`]
/// (case-insensitive + byte-trim per the §5.7 normaliser) so values
/// returned by GitHub's API match canonical variants regardless of
/// trailing whitespace or unexpected casing. Returns `None` when the
/// `issueType` field is absent (no native type assigned), `null`, or
/// carries a name that is not in the canonical taxonomy.
fn parse_issue_type_from_native(value: &serde_json::Value) -> Option<IssueType> {
    let name = value.get("issueType")?.get("name")?.as_str()?;
    IssueType::from_canonical_name(name)
}

/// Maps the `PipelineStage` field value to a [`PipelineStage`] enum variant.
///
/// Matches the canonical lowercase option names emitted by
/// `setup_fields` (spec §5 / spec §2.5): `investigation`, `implementation`,
/// `review`, `refactoring`, `qa`, `done`. Missing or unrecognised values
/// fall back to `None` — symmetric with [`parse_issue_type_field`] and
/// consistent with the silent-fallback semantics of
/// [`parse_status_field`] / [`parse_priority_field`].
fn parse_pipeline_stage_field(
    fields: &std::collections::HashMap<String, String>,
) -> Option<PipelineStage> {
    match fields.get("PipelineStage").map(String::as_str) {
        Some("investigation") => Some(PipelineStage::Investigation),
        Some("implementation") => Some(PipelineStage::Implementation),
        Some("review") => Some(PipelineStage::Review),
        Some("refactoring") => Some(PipelineStage::Refactoring),
        Some("qa") => Some(PipelineStage::Qa),
        Some("done") => Some(PipelineStage::Done),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── classify_cross_repo_fetch ───────────────────────────────────────

    #[test]
    fn classify_cross_repo_fetch_upgrades_403_to_cross_repo_access_denied() {
        // SPEC §11.1 wiring: a cross-repo fetch returning HTTP 403
        // (wrapped as `Error::GitHubApi { status: 403, .. }`) MUST
        // upgrade to `DomainError::CrossRepoAccessDenied` carrying the
        // known owner/repo context.
        let api_err = errors::GitHubApiSnafu {
            status: 403_u16,
            message: "Must have push access to repository".to_owned(),
        }
        .build();
        let upgraded = classify_cross_repo_fetch(api_err, "acme", "widgets");
        match upgraded {
            errors::Error::Domain { source } => {
                let msg = source.to_string();
                assert_eq!(source.status_code(), 403);
                assert!(msg.contains("acme"), "expected owner in message: {msg}");
                assert!(msg.contains("widgets"), "expected repo in message: {msg}");
            }
            other => panic!("expected CrossRepoAccessDenied Domain error, got: {other:?}"),
        }
    }

    #[test]
    fn classify_cross_repo_fetch_upgrades_graphql_forbidden() {
        // SPEC §11.1 wiring: a cross-repo fetch returning GraphQL
        // FORBIDDEN (wrapped as `Error::GitHubGraphQLForbidden`) MUST
        // upgrade to `DomainError::CrossRepoAccessDenied`. The reducer
        // emits this variant by inspecting `errors[i].type == "FORBIDDEN"`
        // BEFORE reducing to messages.
        let gql_err = errors::GitHubGraphQLForbiddenSnafu {
            errors: vec!["Resource not accessible by integration".to_owned()],
        }
        .build();
        let upgraded = classify_cross_repo_fetch(gql_err, "acme", "widgets");
        match upgraded {
            errors::Error::Domain { source } => {
                let msg = source.to_string();
                assert_eq!(source.status_code(), 403);
                assert!(msg.contains("acme"), "expected owner in message: {msg}");
                assert!(msg.contains("widgets"), "expected repo in message: {msg}");
            }
            other => panic!("expected CrossRepoAccessDenied Domain error, got: {other:?}"),
        }
    }

    #[test]
    fn classify_cross_repo_fetch_passes_through_non_403_api() {
        // A non-403 GitHubApi error MUST stay as GitHubApi — only 403
        // is cross-repo-access-semantic.
        let api_err = errors::GitHubApiSnafu {
            status: 500_u16,
            message: "Internal Server Error".to_owned(),
        }
        .build();
        let result = classify_cross_repo_fetch(api_err, "acme", "widgets");
        match result {
            errors::Error::GitHubApi { status, .. } => assert_eq!(status, 500),
            other => panic!("expected GitHubApi passthrough, got: {other:?}"),
        }
    }

    #[test]
    fn classify_cross_repo_fetch_passes_through_non_forbidden_graphql() {
        // A non-FORBIDDEN GraphQL error (the reducer's default bucket)
        // MUST stay as GitHubGraphQL — only the FORBIDDEN-typed variant
        // upgrades.
        let gql_err = errors::GitHubGraphQLSnafu {
            errors: vec!["Field 'x' not found".to_owned()],
        }
        .build();
        let result = classify_cross_repo_fetch(gql_err, "acme", "widgets");
        match result {
            errors::Error::GitHubGraphQL { errors } => {
                assert_eq!(errors, vec!["Field 'x' not found".to_owned()]);
            }
            other => panic!("expected GitHubGraphQL passthrough, got: {other:?}"),
        }
    }

    #[test]
    fn classify_cross_repo_fetch_passes_through_unrelated_errors() {
        // A rate-limit error MUST pass through unchanged — the
        // classifier is scoped to 403 / FORBIDDEN only.
        let err = errors::RateLimitedSnafu {
            reset_at: chrono::Utc::now(),
        }
        .build();
        let result = classify_cross_repo_fetch(err, "acme", "widgets");
        match result {
            errors::Error::RateLimited { .. } => {}
            other => panic!("expected RateLimited passthrough, got: {other:?}"),
        }
    }

    // ── parse_issue_state ───────────────────────────────────────────────

    #[test]
    fn parse_state_open() {
        let v = serde_json::json!({"state": "OPEN"});
        assert_eq!(parse_issue_state(&v), IssueState::Open);
    }

    #[test]
    fn parse_state_closed() {
        let v = serde_json::json!({"state": "CLOSED"});
        assert_eq!(parse_issue_state(&v), IssueState::Closed);
    }

    #[test]
    fn parse_state_missing_defaults_open() {
        let v = serde_json::json!({});
        assert_eq!(parse_issue_state(&v), IssueState::Open);
    }

    // ── parse_string_nodes ──────────────────────────────────────────────

    #[test]
    fn parse_labels_from_nodes() {
        let v = serde_json::json!({
            "labels": {
                "nodes": [
                    {"name": "bug"},
                    {"name": "urgent"}
                ]
            }
        });
        assert_eq!(
            parse_string_nodes(&v, "labels", "name"),
            vec!["bug", "urgent"]
        );
    }

    #[test]
    fn parse_empty_nodes_returns_empty() {
        let v = serde_json::json!({"labels": {"nodes": []}});
        assert!(parse_string_nodes(&v, "labels", "name").is_empty());
    }

    #[test]
    fn parse_missing_connection_returns_empty() {
        let v = serde_json::json!({});
        assert!(parse_string_nodes(&v, "labels", "name").is_empty());
    }

    // ── parse_comments ──────────────────────────────────────────────────

    #[test]
    fn parse_comments_with_data() {
        let v = serde_json::json!({
            "comments": {
                "nodes": [
                    {
                        "author": {"login": "alice"},
                        "body": "Hello",
                        "createdAt": "2026-01-15T10:00:00Z"
                    },
                    {
                        "author": {"login": "bob"},
                        "body": "World",
                        "createdAt": "2026-01-15T11:00:00Z"
                    }
                ]
            }
        });
        let comments = parse_comments(&v);
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].author, "alice");
        assert_eq!(comments[0].body, "Hello");
        assert_eq!(comments[1].author, "bob");
    }

    #[test]
    fn parse_comments_null_author_becomes_ghost() {
        let v = serde_json::json!({
            "comments": {
                "nodes": [{
                    "author": null,
                    "body": "Deleted user comment",
                    "createdAt": "2026-01-15T10:00:00Z"
                }]
            }
        });
        let comments = parse_comments(&v);
        assert_eq!(comments[0].author, "ghost");
    }

    #[test]
    fn parse_comments_empty() {
        let v = serde_json::json!({"comments": {"nodes": []}});
        assert!(parse_comments(&v).is_empty());
    }

    // ── parse_related_issues ────────────────────────────────────────────

    #[test]
    fn parse_blocked_by_issues() {
        let v = serde_json::json!({
            "blockedBy": {
                "nodes": [
                    {"number": 5, "title": "Blocker", "state": "OPEN"},
                    {"number": 10, "title": "Other blocker", "state": "CLOSED"}
                ]
            }
        });
        let related = parse_related_issues(&v, "blockedBy");
        assert_eq!(related.len(), 2);
        assert_eq!(related[0].number, 5);
        assert_eq!(related[0].state, IssueState::Open);
        assert_eq!(related[1].number, 10);
        assert_eq!(related[1].state, IssueState::Closed);
    }

    // ── parse_parent_issue ──────────────────────────────────────────────

    #[test]
    fn parse_parent_present() {
        let v = serde_json::json!({
            "parent": {"number": 1, "title": "Epic", "state": "OPEN"}
        });
        let parent = parse_parent_issue(&v).expect("should parse parent");
        assert_eq!(parent.number, 1);
        assert_eq!(parent.title, "Epic");
    }

    #[test]
    fn parse_parent_null() {
        let v = serde_json::json!({"parent": null});
        assert!(parse_parent_issue(&v).is_none());
    }

    #[test]
    fn parse_parent_missing() {
        let v = serde_json::json!({});
        assert!(parse_parent_issue(&v).is_none());
    }

    // ── extract_field_values ────────────────────────────────────────────

    #[test]
    fn extract_single_select_field() {
        let v = serde_json::json!({
            "projectItems": {
                "nodes": [{
                    "fieldValues": {
                        "nodes": [{
                            "field": {"name": "Status"},
                            "name": "In Progress"
                        }]
                    }
                }]
            }
        });
        let fields = extract_field_values(&v);
        assert_eq!(
            fields.get("Status").map(String::as_str),
            Some("In Progress")
        );
    }

    #[test]
    fn extract_text_field() {
        let v = serde_json::json!({
            "projectItems": {
                "nodes": [{
                    "fieldValues": {
                        "nodes": [{
                            "field": {"name": "Agent"},
                            "text": "claude-bot"
                        }]
                    }
                }]
            }
        });
        let fields = extract_field_values(&v);
        assert_eq!(fields.get("Agent").map(String::as_str), Some("claude-bot"));
    }

    #[test]
    fn extract_number_field() {
        let v = serde_json::json!({
            "projectItems": {
                "nodes": [{
                    "fieldValues": {
                        "nodes": [{
                            "field": {"name": "StoryPoints"},
                            "number": 5.0
                        }]
                    }
                }]
            }
        });
        let fields = extract_field_values(&v);
        assert_eq!(fields.get("StoryPoints").map(String::as_str), Some("5"));
    }

    #[test]
    fn extract_date_field() {
        let v = serde_json::json!({
            "projectItems": {
                "nodes": [{
                    "fieldValues": {
                        "nodes": [{
                            "field": {"name": "DeferUntil"},
                            "date": "2026-04-01"
                        }]
                    }
                }]
            }
        });
        let fields = extract_field_values(&v);
        assert_eq!(
            fields.get("DeferUntil").map(String::as_str),
            Some("2026-04-01")
        );
    }

    #[test]
    fn extract_no_project_items_returns_empty() {
        let v = serde_json::json!({});
        assert!(extract_field_values(&v).is_empty());
    }

    // ── parse_status_field ──────────────────────────────────────────────

    #[test]
    fn status_field_mapping_canonical_titlecase_names() {
        // Canonical wire format (post-`unblock-1zj`): the TitleCase
        // strings produced by `Status::option_name`. This is the
        // round-trip case for Invariant 16 — exercised exhaustively in
        // `status_option_name_round_trip_through_parse_status_field`
        // below.
        let mut fields = std::collections::HashMap::new();
        fields.insert("Status".to_owned(), "Backlog".to_owned());
        assert_eq!(parse_status_field(&fields), Status::Backlog);

        fields.insert("Status".to_owned(), "Ready".to_owned());
        assert_eq!(parse_status_field(&fields), Status::Ready);

        fields.insert("Status".to_owned(), "In Progress".to_owned());
        assert_eq!(parse_status_field(&fields), Status::InProgress);

        fields.insert("Status".to_owned(), "Blocked".to_owned());
        assert_eq!(parse_status_field(&fields), Status::Blocked);

        fields.insert("Status".to_owned(), "Deferred".to_owned());
        assert_eq!(parse_status_field(&fields), Status::Deferred);

        fields.insert("Status".to_owned(), "Closed".to_owned());
        assert_eq!(parse_status_field(&fields), Status::Closed);
    }

    #[test]
    fn status_option_name_round_trip_through_parse_status_field() {
        // Invariant 16 / Appendix A.3 obligation 1: every variant's
        // `option_name()` parses back to the same variant. Round-trip
        // exhaustiveness is the contract that pins the helper as the
        // single source of truth for wire-format Status strings.
        for s in Status::ALL {
            let mut fields = std::collections::HashMap::new();
            fields.insert("Status".to_owned(), s.option_name().to_owned());
            assert_eq!(
                parse_status_field(&fields),
                s,
                "round-trip failed for {s:?} (option_name={:?})",
                s.option_name()
            );
        }
    }

    #[test]
    fn status_field_mapping_legacy_lowercase_names() {
        // Legacy lowercase / `snake_case` set used pre-`unblock-1zj`.
        // Accepted for parsing only; the auto-heal pass renames these
        // to canonical `TitleCase` on the next `setup` call.
        let mut fields = std::collections::HashMap::new();
        fields.insert("Status".to_owned(), "ready".to_owned());
        assert_eq!(parse_status_field(&fields), Status::Ready);

        fields.insert("Status".to_owned(), "in_progress".to_owned());
        assert_eq!(parse_status_field(&fields), Status::InProgress);

        fields.insert("Status".to_owned(), "blocked".to_owned());
        assert_eq!(parse_status_field(&fields), Status::Blocked);

        fields.insert("Status".to_owned(), "deferred".to_owned());
        assert_eq!(parse_status_field(&fields), Status::Deferred);

        fields.insert("Status".to_owned(), "closed".to_owned());
        assert_eq!(parse_status_field(&fields), Status::Closed);

        // `backlog` (lowercase) explicit-arm symmetry with the other
        // legacy aliases above. Behaviour-equivalent to the wildcard
        // fallthrough but pins the explicit map so a future variant
        // addition does not regress this case.
        fields.insert("Status".to_owned(), "backlog".to_owned());
        assert_eq!(parse_status_field(&fields), Status::Backlog);
    }

    #[test]
    fn status_field_mapping_legacy_github_default_names() {
        // GitHub-default built-in single-select Status field options,
        // pre-any unblock setup pass. Charitably mapped: `Done` →
        // `Closed`, `Todo` → `Backlog`.
        let mut fields = std::collections::HashMap::new();
        fields.insert("Status".to_owned(), "Done".to_owned());
        assert_eq!(parse_status_field(&fields), Status::Closed);

        fields.insert("Status".to_owned(), "Todo".to_owned());
        assert_eq!(parse_status_field(&fields), Status::Backlog);
    }

    #[test]
    fn status_field_missing_defaults_to_backlog() {
        // Missing or unrecognized -> Backlog (sticky default for
        // unmanaged items, consistent with the create-time default per
        // §2.3 sticky semantics).
        let fields = std::collections::HashMap::new();
        assert_eq!(parse_status_field(&fields), Status::Backlog);

        let mut fields = std::collections::HashMap::new();
        fields.insert("Status".to_owned(), "something_else".to_owned());
        assert_eq!(parse_status_field(&fields), Status::Backlog);
    }

    // ── parse_priority_field ────────────────────────────────────────────

    #[test]
    fn priority_field_mapping_spec_names() {
        let mut fields = std::collections::HashMap::new();
        for (label, expected) in [
            ("P0 - Critical", Priority::P0),
            ("P1 - High", Priority::P1),
            ("P2 - Medium", Priority::P2),
            ("P3 - Low", Priority::P3),
            ("P4 - Backlog", Priority::P4),
        ] {
            fields.insert("Priority".to_owned(), label.to_owned());
            assert_eq!(parse_priority_field(&fields), expected);
        }
    }

    #[test]
    fn priority_field_mapping_legacy_names() {
        let mut fields = std::collections::HashMap::new();
        for (label, expected) in [
            ("P0", Priority::P0),
            ("P1", Priority::P1),
            ("P2", Priority::P2),
            ("P3", Priority::P3),
            ("P4", Priority::P4),
        ] {
            fields.insert("Priority".to_owned(), label.to_owned());
            assert_eq!(parse_priority_field(&fields), expected);
        }
    }

    #[test]
    fn priority_field_missing_defaults_p2() {
        let fields = std::collections::HashMap::new();
        assert_eq!(parse_priority_field(&fields), Priority::P2);
    }

    // ── parse_issue_type_from_native (unblock-wgj.15) ───────────────────
    //
    // Pre-`unblock-wgj` the IssueType was read off a Projects V2
    // SingleSelect HashMap that NEVER materialises on real boards
    // (verified empirically — `gh api graphql` returned `issueType: null`
    // because no such Projects V2 field was ever provisioned). The
    // canonical source of truth per spec §2.6 is the native
    // `issueType { name }` selection on the issue node — see
    // `parse_issue_type_from_native`.

    #[test]
    fn issue_type_from_native_round_trips_all_eight_variants() {
        // Graph-invariant 10 (§13.3): every `IssueType::canonical_name`
        // round-trips through `parse_issue_type_from_native`.
        for variant in IssueType::ALL {
            let json = serde_json::json!({
                "issueType": { "name": variant.canonical_name() }
            });
            assert_eq!(parse_issue_type_from_native(&json), Some(variant));
        }
    }

    #[test]
    fn issue_type_from_native_is_case_insensitive() {
        // §5.7 normaliser parity: GitHub's API may return the name in
        // any case; we accept canonical variants regardless of casing.
        let json = serde_json::json!({
            "issueType": { "name": "BUG" }
        });
        assert_eq!(parse_issue_type_from_native(&json), Some(IssueType::Bug));

        let json = serde_json::json!({
            "issueType": { "name": "  refactor  " }
        });
        assert_eq!(
            parse_issue_type_from_native(&json),
            Some(IssueType::Refactor)
        );
    }

    #[test]
    fn issue_type_from_native_returns_none_when_field_absent() {
        // Issues with no native type assigned have no `issueType` key
        // (or `null`) — must collapse to `None`.
        let json = serde_json::json!({ "number": 1 });
        assert!(parse_issue_type_from_native(&json).is_none());

        let json = serde_json::json!({ "issueType": null });
        assert!(parse_issue_type_from_native(&json).is_none());
    }

    #[test]
    fn issue_type_from_native_returns_none_for_unrecognised_name() {
        // Names outside the canonical taxonomy collapse to `None` — the
        // parser is closed against the eight variants in spec §2.6.
        let json = serde_json::json!({
            "issueType": { "name": "Improvement" }
        });
        assert!(parse_issue_type_from_native(&json).is_none());
    }

    // ── parse_pipeline_stage_field (unblock-29p.18) ─────────────────────

    /// `setup_fields` (in `crates/unblock-github/src/projects.rs`) writes the
    /// canonical lowercase option set for the `PipelineStage` Projects V2
    /// field per spec §5 / §2.5: `investigation`, `implementation`,
    /// `review`, `refactoring`, `qa`, `done`. The parser must round-trip
    /// every variant.
    #[test]
    fn pipeline_stage_field_mapping_spec_names() {
        let mut fields = std::collections::HashMap::new();
        for (label, expected) in [
            ("investigation", PipelineStage::Investigation),
            ("implementation", PipelineStage::Implementation),
            ("review", PipelineStage::Review),
            ("refactoring", PipelineStage::Refactoring),
            ("qa", PipelineStage::Qa),
            ("done", PipelineStage::Done),
        ] {
            fields.insert("PipelineStage".to_owned(), label.to_owned());
            assert_eq!(parse_pipeline_stage_field(&fields), Some(expected));
        }
    }

    /// Missing `PipelineStage` field-value collapses to `None` — symmetric
    /// with [`parse_issue_type_field`] silent-fallback semantics.
    #[test]
    fn pipeline_stage_field_missing_returns_none() {
        let fields = std::collections::HashMap::new();
        assert!(parse_pipeline_stage_field(&fields).is_none());
    }

    /// Unrecognised `PipelineStage` option-value collapses to `None`.
    /// Includes case mismatches (`"Investigation"` vs canonical
    /// `"investigation"`) — the matcher is strictly case-sensitive
    /// against the spec lowercase taxonomy.
    #[test]
    fn pipeline_stage_field_unrecognised_returns_none() {
        let mut fields = std::collections::HashMap::new();
        for unknown in [
            "Investigation", // wrong case (spec mandates lowercase)
            "IMPLEMENTATION",
            "deployed", // not in taxonomy
            "",
            "  ",
        ] {
            fields.insert("PipelineStage".to_owned(), unknown.to_owned());
            assert!(
                parse_pipeline_stage_field(&fields).is_none(),
                "expected None for unrecognised PipelineStage value: {unknown:?}"
            );
        }
    }

    // ── parse_issue (full roundtrip) ────────────────────────────────────

    #[test]
    fn parse_full_issue_from_json() {
        let json = serde_json::json!({
            "id": "MDU6SXNzdWUx",
            "number": 42,
            "title": "Fix the bug",
            "body": "## Description\n\nSome bug.",
            "state": "OPEN",
            "url": "https://github.com/owner/repo/issues/42",
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-02T00:00:00Z",
            "labels": {"nodes": [{"name": "bug"}, {"name": "P0"}]},
            "milestone": {"title": "v1.0"},
            "assignees": {"nodes": [{"login": "alice"}]},
            "comments": {
                "nodes": [{
                    "author": {"login": "bob"},
                    "body": "Working on it",
                    "createdAt": "2026-01-01T12:00:00Z"
                }]
            },
            "blocking": {
                "nodes": [
                    {"number": 20, "title": "Blocks me", "state": "OPEN"}
                ]
            },
            "blockedBy": {
                "nodes": [
                    {
                        "number": 10,
                        "title": "Dep A",
                        "state": "OPEN",
                        // `FETCH_ISSUE_QUERY` blockedBy subselection
                        // carries `repository { owner { login } name }`
                        // so blockers can be disambiguated as same-repo
                        // or cross-repo (see `unblock-29p.43`).
                        "repository": {
                            "name": "test-repo",
                            "owner": {"login": "test-owner"}
                        }
                    }
                ]
            },
            "parent": {"number": 1, "title": "Epic", "state": "OPEN"},
            "subIssues": {"nodes": []},
            "projectItems": {
                "nodes": [{
                    "fieldValues": {
                        "nodes": [
                            {"field": {"name": "Status"}, "name": "In Progress"},
                            {"field": {"name": "Priority"}, "name": "P1"},
                            {"field": {"name": "Agent"}, "text": "claude"},
                            {"field": {"name": "StoryPoints"}, "number": 3.0}
                        ]
                    }
                }]
            }
        });

        let issue = parse_issue(&json, "test-owner", "test-repo");
        assert_eq!(issue.number, 42);
        assert_eq!(issue.node_id, "MDU6SXNzdWUx");
        assert_eq!(issue.title, "Fix the bug");
        assert_eq!(issue.body.as_deref(), Some("## Description\n\nSome bug."));
        assert_eq!(issue.state, IssueState::Open);
        assert_eq!(issue.url, "https://github.com/owner/repo/issues/42");
        assert_eq!(issue.labels, vec!["bug", "P0"]);
        assert_eq!(issue.milestone.as_deref(), Some("v1.0"));
        assert_eq!(issue.assignees, vec!["alice"]);
        assert_eq!(issue.status, Status::InProgress);
        assert_eq!(issue.priority, Priority::P1);
        assert_eq!(issue.agent.as_deref(), Some("claude"));
        assert_eq!(issue.story_points, Some(3));

        assert_eq!(issue.comments.len(), 1);
        assert_eq!(issue.comments[0].author, "bob");
        assert_eq!(issue.comments[0].body, "Working on it");

        assert_eq!(issue.blocked_by.len(), 1);
        assert_eq!(issue.blocked_by[0].number, 10);
        // Repo identity must propagate end-to-end from the blockedBy
        // subselection — load-bearing for cross-repo dep_remove edge
        // validation (see `unblock-29p.43`).
        assert_eq!(
            issue.blocked_by[0].repo_owner.as_deref(),
            Some("test-owner"),
            "blockedBy blocker must carry repository.owner.login"
        );
        assert_eq!(
            issue.blocked_by[0].repo_name.as_deref(),
            Some("test-repo"),
            "blockedBy blocker must carry repository.name"
        );

        assert_eq!(issue.blocking.len(), 1);
        assert_eq!(issue.blocking[0].number, 20);
        // The `blocking` connection does NOT request the `repository`
        // subselection — repo identity stays `None` (same-repo by
        // convention).
        assert!(issue.blocking[0].repo_owner.is_none());
        assert!(issue.blocking[0].repo_name.is_none());

        let parent = issue.parent.expect("should have parent");
        assert_eq!(parent.number, 1);

        assert!(issue.sub_issues.is_empty());
    }

    #[test]
    fn parse_minimal_issue() {
        let json = serde_json::json!({
            "id": "node123",
            "number": 1,
            "title": "Minimal",
            "body": null,
            "state": "OPEN",
            "url": "",
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-01T00:00:00Z",
            "labels": {"nodes": []},
            "milestone": null,
            "assignees": {"nodes": []},
            "comments": {"nodes": []},
            "blocking": {"nodes": []},
            "blockedBy": {"nodes": []},
            "parent": null,
            "subIssues": {"nodes": []}
        });

        let issue = parse_issue(&json, "test-owner", "test-repo");
        assert_eq!(issue.number, 1);
        assert!(issue.body.is_none());
        assert!(issue.comments.is_empty());
        assert!(issue.blocked_by.is_empty());
        assert!(issue.blocking.is_empty());
        assert!(issue.parent.is_none());
        assert!(issue.sub_issues.is_empty());
        // Post-`unblock-1zj`: missing Status defaults to `Backlog`
        // (sticky default for unmanaged items, consistent with the
        // create-time default per §2.3 sticky semantics).
        assert_eq!(issue.status, Status::Backlog);
        assert_eq!(issue.priority, Priority::P2);
    }

    // ── parse_related_issues + repo identity (unblock-29p.43) ───────────

    /// `FETCH_ISSUE_QUERY` `blockedBy` emits `repository { owner { login }
    /// name }` so cross-repo blockers can be disambiguated. Verifies the
    /// parser round-trips those subfields into `RelatedIssue`.
    #[test]
    fn parse_related_issues_extracts_cross_repo_identity() {
        let json = serde_json::json!({
            "blockedBy": {
                "nodes": [
                    {
                        "number": 99,
                        "title": "Cross-repo blocker",
                        "state": "OPEN",
                        "repository": {
                            "name": "other-repo",
                            "owner": {"login": "other-owner"}
                        }
                    }
                ]
            }
        });
        let blockers = parse_related_issues(&json, "blockedBy");
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].number, 99);
        assert_eq!(blockers[0].repo_owner.as_deref(), Some("other-owner"));
        assert_eq!(blockers[0].repo_name.as_deref(), Some("other-repo"));
    }

    /// Connections whose GraphQL selection omits `repository { ... }`
    /// (e.g. `blocking`, `subIssues`, `parent`) must leave `repo_owner` /
    /// `repo_name` as `None`. Callers treat `None` as "same repo as the
    /// containing issue" per `RelatedIssue` docs.
    #[test]
    fn parse_related_issues_without_repository_subfield_keeps_none() {
        let json = serde_json::json!({
            "subIssues": {
                "nodes": [
                    {"number": 7, "title": "Sub", "state": "OPEN"}
                ]
            }
        });
        let subs = parse_related_issues(&json, "subIssues");
        assert_eq!(subs.len(), 1);
        assert!(subs[0].repo_owner.is_none());
        assert!(subs[0].repo_name.is_none());
    }

    /// A partial `repository` subfield (owner without name, or name
    /// without owner) cannot identify a cross-repo reference — that
    /// requires the full `(owner, name)` pair — so the parser routes
    /// partial nodes through [`RelatedIssue::local`], normalising BOTH
    /// repo identity fields to `None`. Callers then treat the result
    /// as "same repo as the containing issue" per `RelatedIssue` docs.
    ///
    /// Behaviour change: prior to unblock-29p.66 the parser retained
    /// the parsed half of a partial subfield (e.g. `repository:
    /// {name: "only-name"}` → `repo_owner: None, repo_name:
    /// Some("only-name")`). That state was incoherent because cross-
    /// repo disambiguation needs both halves; the hardened API surface
    /// (strict `local` / `cross_repo` helpers, see `unblock-29p.66`)
    /// makes the normalisation explicit.
    #[test]
    fn parse_related_issues_partial_repository_normalises_to_local() {
        let json = serde_json::json!({
            "blockedBy": {
                "nodes": [
                    {
                        "number": 11,
                        "title": "Partial",
                        "state": "OPEN",
                        "repository": {"name": "only-name"}
                    },
                    {
                        "number": 12,
                        "title": "Other partial",
                        "state": "OPEN",
                        "repository": {"owner": {"login": "only-owner"}}
                    }
                ]
            }
        });
        let blockers = parse_related_issues(&json, "blockedBy");
        assert_eq!(blockers.len(), 2);
        assert!(blockers[0].repo_owner.is_none());
        assert!(blockers[0].repo_name.is_none());
        assert!(blockers[1].repo_owner.is_none());
        assert!(blockers[1].repo_name.is_none());
    }

    // ── parse_graph_issue ──────────────────────────────────────────────

    #[test]
    fn parse_graph_issue_extracts_standard_fields() {
        // Post-`unblock-wgj.15`: IssueType is read off the native
        // `issueType { name }` field on the issue node, NOT from the
        // Projects V2 SingleSelect HashMap. The fixture below provides
        // the native field; the legacy `IssueType` Projects V2 field is
        // explicitly absent.
        let json = serde_json::json!({
            "id": "MDU6SXNzdWUx",
            "number": 42,
            "title": "Graph issue",
            "body": "Some body",
            "state": "OPEN",
            "url": "https://github.com/owner/repo/issues/42",
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-02T00:00:00Z",
            "issueType": { "name": "Bug" },
            "labels": {"nodes": [{"name": "bug"}]},
            "milestone": {"title": "v1.0"},
            "assignees": {"nodes": [{"login": "alice"}]},
            "blockedBy": {
                "nodes": [{"number": 10}]
            },
            "projectItems": {
                "nodes": [{
                    "fieldValues": {
                        "nodes": [
                            {"field": {"name": "Status"}, "name": "In Progress"},
                            {"field": {"name": "Priority"}, "name": "P1"},
                            {"field": {"name": "Agent"}, "text": "claude"},
                            {"field": {"name": "StoryPoints"}, "number": 5.0},
                            {"field": {"name": "DeferUntil"}, "date": "2026-06-01"},
                            {"field": {"name": "PipelineStage"}, "name": "implementation"},
                            {"field": {"name": "ClaimedAt"}, "text": "2026-03-15T10:30:00Z"}
                        ]
                    }
                }]
            }
        });

        let issue = parse_graph_issue(&json, "test-owner", "test-repo");
        assert_eq!(issue.number, 42);
        assert_eq!(issue.node_id, "MDU6SXNzdWUx");
        assert_eq!(issue.title, "Graph issue");
        assert_eq!(issue.body.as_deref(), Some("Some body"));
        assert_eq!(issue.state, IssueState::Open);
        assert_eq!(issue.labels, vec!["bug"]);
        assert_eq!(issue.milestone.as_deref(), Some("v1.0"));
        assert_eq!(issue.assignees, vec!["alice"]);
        assert_eq!(issue.status, Status::InProgress);
        assert_eq!(issue.priority, Priority::P1);
        assert_eq!(issue.agent.as_deref(), Some("claude"));
        assert_eq!(issue.story_points, Some(5));
        assert_eq!(
            issue.defer_until,
            Some(chrono::NaiveDate::from_ymd_opt(2026, 6, 1).expect("valid date"))
        );
        assert_eq!(issue.issue_type, Some(IssueType::Bug));
        // Regression guard for unblock-29p.18: parse_graph_issue MUST
        // round-trip the canonical lowercase PipelineStage option set
        // emitted by setup_fields. Prior to the fix this was hardcoded
        // to `None` and the absence of the field in the fixture masked
        // the bug — see test commentary in
        // `parse_graph_issue_round_trips_pipeline_stage_variants`.
        assert_eq!(issue.pipeline_stage, Some(PipelineStage::Implementation));
        assert!(issue.claimed_at.is_some(), "ClaimedAt should be parsed");
        assert_eq!(
            issue.claimed_at.expect("just asserted").to_rfc3339(),
            "2026-03-15T10:30:00+00:00"
        );
    }

    #[test]
    fn parse_graph_issue_leaves_detail_fields_empty() {
        let json = serde_json::json!({
            "id": "node1",
            "number": 1,
            "title": "Test",
            "body": null,
            "state": "OPEN",
            "url": "",
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-01T00:00:00Z",
            "labels": {"nodes": []},
            "milestone": null,
            "assignees": {"nodes": []},
            "blockedBy": {
                "nodes": [{"number": 5}, {"number": 10}]
            }
        });

        let issue = parse_graph_issue(&json, "test-owner", "test-repo");
        assert!(
            issue.comments.is_empty(),
            "comments should be empty for graph issues"
        );
        assert!(
            issue.blocked_by.is_empty(),
            "blocked_by should be empty for graph issues"
        );
        assert!(
            issue.blocking.is_empty(),
            "blocking should be empty for graph issues"
        );
        assert!(
            issue.parent.is_none(),
            "parent should be None for graph issues"
        );
        assert!(
            issue.sub_issues.is_empty(),
            "sub_issues should be empty for graph issues"
        );
    }

    #[test]
    fn parse_graph_issue_without_project_items_uses_defaults() {
        let json = serde_json::json!({
            "id": "node2",
            "number": 2,
            "title": "No project",
            "body": null,
            "state": "OPEN",
            "url": "",
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-01T00:00:00Z",
            "labels": {"nodes": []},
            "milestone": null,
            "assignees": {"nodes": []}
        });

        let issue = parse_graph_issue(&json, "test-owner", "test-repo");
        // Post-`unblock-1zj`: missing Status defaults to `Backlog`.
        assert_eq!(issue.status, Status::Backlog);
        assert_eq!(issue.priority, Priority::P2);
        assert!(issue.issue_type.is_none());
        assert!(issue.agent.is_none());
        assert!(issue.story_points.is_none());
        assert!(issue.defer_until.is_none());
        // No `projectItems` connection → `extract_field_values` returns
        // an empty map → `parse_pipeline_stage_field` correctly yields
        // `None` (genuine absence, not the prior hardcoded-None bug).
        assert!(issue.pipeline_stage.is_none());
        assert!(issue.claimed_at.is_none());
    }

    // ── PipelineStage round-trip (unblock-29p.18) ───────────────────────

    /// Helper: builds a `projectItems` fixture with a single
    /// `PipelineStage` field-value, isolating the parser surface from
    /// unrelated Status/Priority/etc. defaults.
    fn pipeline_stage_fixture(option_name: &str) -> serde_json::Value {
        serde_json::json!({
            "id": "node-ps",
            "number": 99,
            "title": "Pipeline stage round-trip",
            "body": null,
            "state": "OPEN",
            "url": "",
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-01T00:00:00Z",
            "labels": {"nodes": []},
            "milestone": null,
            "assignees": {"nodes": []},
            "comments": {"nodes": []},
            "blocking": {"nodes": []},
            "blockedBy": {"nodes": []},
            "parent": null,
            "subIssues": {"nodes": []},
            "projectItems": {
                "nodes": [{
                    "fieldValues": {
                        "nodes": [
                            {"field": {"name": "PipelineStage"}, "name": option_name}
                        ]
                    }
                }]
            }
        })
    }

    /// `parse_issue` (full roundtrip) MUST round-trip every canonical
    /// `PipelineStage` option emitted by `setup_fields` (spec §5 / §2.5).
    /// Prior to unblock-29p.18 this site hardcoded `None`, silently
    /// dropping whatever value GitHub stored.
    #[test]
    fn parse_issue_round_trips_pipeline_stage_variants() {
        for (label, expected) in [
            ("investigation", PipelineStage::Investigation),
            ("implementation", PipelineStage::Implementation),
            ("review", PipelineStage::Review),
            ("refactoring", PipelineStage::Refactoring),
            ("qa", PipelineStage::Qa),
            ("done", PipelineStage::Done),
        ] {
            let json = pipeline_stage_fixture(label);
            let issue = parse_issue(&json, "test-owner", "test-repo");
            assert_eq!(
                issue.pipeline_stage,
                Some(expected),
                "parse_issue must round-trip canonical PipelineStage \
                 option {label:?}"
            );
        }
    }

    /// `parse_graph_issue` (bulk graph data) MUST round-trip every
    /// canonical `PipelineStage` option emitted by `setup_fields`. This
    /// is the second of the two parser paths flagged in
    /// unblock-29p.18 — both must be migrated together.
    #[test]
    fn parse_graph_issue_round_trips_pipeline_stage_variants() {
        for (label, expected) in [
            ("investigation", PipelineStage::Investigation),
            ("implementation", PipelineStage::Implementation),
            ("review", PipelineStage::Review),
            ("refactoring", PipelineStage::Refactoring),
            ("qa", PipelineStage::Qa),
            ("done", PipelineStage::Done),
        ] {
            let json = pipeline_stage_fixture(label);
            let issue = parse_graph_issue(&json, "test-owner", "test-repo");
            assert_eq!(
                issue.pipeline_stage,
                Some(expected),
                "parse_graph_issue must round-trip canonical \
                 PipelineStage option {label:?}"
            );
        }
    }

    /// Unrecognised `PipelineStage` option-values collapse to `None` in
    /// both parser paths — preserves silent-fallback symmetry with
    /// `parse_status_field` / `parse_priority_field` (which fall back
    /// to defaults rather than emitting a `tracing::warn!`).
    #[test]
    fn parse_issue_unknown_pipeline_stage_yields_none() {
        let json = pipeline_stage_fixture("deployed");
        let full = parse_issue(&json, "test-owner", "test-repo");
        let graph = parse_graph_issue(&json, "test-owner", "test-repo");
        assert!(full.pipeline_stage.is_none());
        assert!(graph.pipeline_stage.is_none());
    }

    // ── BlockingEdge extraction (via extract_blocking_edges helper) ──────

    #[test]
    fn blocking_edge_direction_source_blocked_by_target() {
        let node = serde_json::json!({
            "number": 42,
            "blockedBy": {
                "nodes": [
                    {"number": 10},
                    {"number": 20}
                ]
            }
        });

        let edges = extract_blocking_edges(&node, "test-owner", "test-repo");

        assert_eq!(edges.len(), 2);
        // source = blocked issue (42), target = blocker
        assert_eq!(edges[0].source.number, 42);
        assert_eq!(edges[0].target.number, 10);
        assert_eq!(edges[1].source.number, 42);
        assert_eq!(edges[1].target.number, 20);
    }

    #[test]
    fn blocking_edge_skips_zero_number() {
        let node = serde_json::json!({
            "number": 42,
            "blockedBy": {
                "nodes": [
                    {"number": 0},
                    {"number": 5}
                ]
            }
        });

        let edges = extract_blocking_edges(&node, "test-owner", "test-repo");

        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].target.number, 5);
    }

    #[test]
    fn blocking_edge_empty_blocked_by_yields_no_edges() {
        let node = serde_json::json!({
            "number": 42,
            "blockedBy": {
                "nodes": []
            }
        });

        let edges = extract_blocking_edges(&node, "test-owner", "test-repo");
        assert!(edges.is_empty());
    }

    #[test]
    fn blocking_edge_missing_blocked_by_yields_no_edges() {
        let node = serde_json::json!({
            "number": 42
        });

        let edges = extract_blocking_edges(&node, "test-owner", "test-repo");
        assert!(edges.is_empty());
    }

    // ── fetch_graph_data pagination (wiremock) ─────────────────────────

    /// Builds a mock GraphQL response page for `fetch_graph_data`.
    ///
    /// Each issue node has the minimal fields required by `parse_graph_issue`:
    /// number, id, title, body, state, url, createdAt, updatedAt, labels,
    /// milestone, assignees, and optionally `blockedBy` edges.
    fn make_page_response(
        issues: &[(u64, &str, Vec<u64>)],
        has_next_page: bool,
        end_cursor: Option<&str>,
    ) -> serde_json::Value {
        let nodes: Vec<serde_json::Value> = issues
            .iter()
            .map(|(number, title, blockers)| {
                let blocked_by_nodes: Vec<serde_json::Value> = blockers
                    .iter()
                    .map(|n| serde_json::json!({"number": n}))
                    .collect();
                serde_json::json!({
                    "id": format!("node-{number}"),
                    "number": number,
                    "title": title,
                    "body": null,
                    "state": "OPEN",
                    "url": format!("https://github.com/test-owner/test-repo/issues/{number}"),
                    "createdAt": "2026-01-01T00:00:00Z",
                    "updatedAt": "2026-01-01T00:00:00Z",
                    "labels": {"nodes": []},
                    "milestone": null,
                    "assignees": {"nodes": []},
                    "blockedBy": {"nodes": blocked_by_nodes}
                })
            })
            .collect();

        serde_json::json!({
            "data": {
                "repository": {
                    "issues": {
                        "pageInfo": {
                            "hasNextPage": has_next_page,
                            "endCursor": end_cursor,
                        },
                        "nodes": nodes,
                    }
                }
            }
        })
    }

    #[tokio::test]
    async fn fetch_graph_data_multi_page_pagination() {
        use wiremock::matchers::{body_string_contains, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        // Page 1: cursor is null → returns 2 issues, hasNextPage: true.
        // The JSON body will contain `"cursor":null` for the first request.
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_string_contains("\"cursor\":null"))
            .respond_with(ResponseTemplate::new(200).set_body_json(make_page_response(
                &[(1, "Issue one", vec![]), (2, "Issue two", vec![1])],
                true,
                Some("cursor-page1"),
            )))
            .expect(1)
            .mount(&server)
            .await;

        // Page 2: cursor is "cursor-page1" → returns 1 issue, hasNextPage: false.
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_string_contains("\"cursor\":\"cursor-page1\""))
            .respond_with(ResponseTemplate::new(200).set_body_json(make_page_response(
                &[(3, "Issue three", vec![2])],
                false,
                None,
            )))
            .expect(1)
            .mount(&server)
            .await;

        let (issues, edges) = client.fetch_graph_data().await.expect("should succeed");

        // All 3 issues from both pages.
        assert_eq!(issues.len(), 3);
        assert_eq!(issues[0].number, 1);
        assert_eq!(issues[0].title, "Issue one");
        assert_eq!(issues[1].number, 2);
        assert_eq!(issues[1].title, "Issue two");
        assert_eq!(issues[2].number, 3);
        assert_eq!(issues[2].title, "Issue three");

        // Edges: issue 2 blocked by 1, issue 3 blocked by 2.
        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0].source.number, 2);
        assert_eq!(edges[0].target.number, 1);
        assert_eq!(edges[1].source.number, 3);
        assert_eq!(edges[1].target.number, 2);
    }

    #[tokio::test]
    async fn fetch_graph_data_single_page_no_extra_requests() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        // Single page: hasNextPage: false on the first (and only) request.
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(make_page_response(
                &[(10, "Only issue", vec![])],
                false,
                None,
            )))
            .expect(1) // Exactly 1 request — no extra page fetches.
            .mount(&server)
            .await;

        let (issues, edges) = client.fetch_graph_data().await.expect("should succeed");

        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].number, 10);
        assert!(edges.is_empty());
    }

    #[tokio::test]
    async fn fetch_graph_data_null_end_cursor_breaks_pagination() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        // Pathological response: hasNextPage: true but endCursor: null.
        // The infinite-loop guard (graphql.rs:289-298) should break out
        // and return the partial results from this single page.
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(make_page_response(
                &[(5, "Partial one", vec![]), (6, "Partial two", vec![5])],
                true, // hasNextPage: true (would loop forever without guard)
                None, // endCursor: null (triggers the guard)
            )))
            .expect(1) // Only 1 request — guard prevents re-fetch.
            .mount(&server)
            .await;

        let (issues, edges) = client.fetch_graph_data().await.expect("should succeed");

        // Returns partial results from the single page.
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].number, 5);
        assert_eq!(issues[1].number, 6);

        // Edge: issue 6 blocked by issue 5.
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].source.number, 6);
        assert_eq!(edges[0].target.number, 5);
    }

    #[tokio::test]
    async fn fetch_graph_data_empty_repo_returns_empty_vecs() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        // Empty repo: no issue nodes, hasNextPage: false.
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(make_page_response(
                &[],
                false,
                None,
            )))
            .expect(1)
            .mount(&server)
            .await;

        let (issues, edges) = client.fetch_graph_data().await.expect("should succeed");

        assert!(issues.is_empty());
        assert!(edges.is_empty());
    }

    /// Builds a mock GraphQL response page that lets the caller set the
    /// native GitHub state (`"OPEN"` / `"CLOSED"`) per issue node.
    ///
    /// Used by [`fetch_graph_data_parses_closed_state_round_trip`] to
    /// verify that `states: [OPEN, CLOSED]` (per SPEC §5.5, bead
    /// `unblock-a36`) round-trips a `CLOSED` node through
    /// `parse_graph_issue` into `IssueState::Closed`.
    fn make_page_response_with_states(
        issues: &[(u64, &str, &str, Vec<u64>)],
        has_next_page: bool,
        end_cursor: Option<&str>,
    ) -> serde_json::Value {
        let nodes: Vec<serde_json::Value> = issues
            .iter()
            .map(|(number, title, state, blockers)| {
                let blocked_by_nodes: Vec<serde_json::Value> = blockers
                    .iter()
                    .map(|n| serde_json::json!({"number": n}))
                    .collect();
                serde_json::json!({
                    "id": format!("node-{number}"),
                    "number": number,
                    "title": title,
                    "body": null,
                    "state": state,
                    "url": format!("https://github.com/test-owner/test-repo/issues/{number}"),
                    "createdAt": "2026-01-01T00:00:00Z",
                    "updatedAt": "2026-01-01T00:00:00Z",
                    "labels": {"nodes": []},
                    "milestone": null,
                    "assignees": {"nodes": []},
                    "blockedBy": {"nodes": blocked_by_nodes}
                })
            })
            .collect();

        serde_json::json!({
            "data": {
                "repository": {
                    "issues": {
                        "pageInfo": {
                            "hasNextPage": has_next_page,
                            "endCursor": end_cursor,
                        },
                        "nodes": nodes,
                    }
                }
            }
        })
    }

    /// `fetch_graph_data` must round-trip BOTH `OPEN` and `CLOSED`
    /// nodes through `parse_graph_issue` after the SPEC §5.5 widening
    /// (`states: [OPEN, CLOSED]`, bead `unblock-a36`).
    ///
    /// Seeds a single page containing one `OPEN` and one `CLOSED`
    /// issue, and asserts:
    /// - both are returned (no filtering happens on the client side),
    /// - the `CLOSED` node materialises as `IssueState::Closed`,
    /// - the `OPEN` node materialises as `IssueState::Open`,
    /// - blocking edges are independent of `IssueState` (the `OPEN`
    ///   issue's blocker entry references the `CLOSED` one, matching
    ///   a real "closed blocker" scenario).
    ///
    /// Complements [`fetch_graph_data_multi_page_pagination`] (which
    /// covers pagination) and the live integration test (which asserts
    /// the `OPEN|CLOSED` invariant against a real repo).
    #[tokio::test]
    async fn fetch_graph_data_parses_closed_state_round_trip() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        // Issue #1 is CLOSED (no blockers). Issue #2 is OPEN and is
        // blocked by #1 — the "still-tracked closed blocker" scenario
        // that unblock-a36's Block A rehydrates in the cache.
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(make_page_response_with_states(
                    &[
                        (1, "Closed one", "CLOSED", vec![]),
                        (2, "Open two", "OPEN", vec![1]),
                    ],
                    false,
                    None,
                )),
            )
            .expect(1)
            .mount(&server)
            .await;

        let (issues, edges) = client.fetch_graph_data().await.expect("should succeed");

        assert_eq!(issues.len(), 2, "both OPEN and CLOSED nodes must surface");

        let closed = issues
            .iter()
            .find(|i| i.number == 1)
            .expect("issue #1 must be present");
        assert_eq!(
            closed.state,
            IssueState::Closed,
            "node with state=\"CLOSED\" must map to IssueState::Closed",
        );

        let open = issues
            .iter()
            .find(|i| i.number == 2)
            .expect("issue #2 must be present");
        assert_eq!(
            open.state,
            IssueState::Open,
            "node with state=\"OPEN\" must map to IssueState::Open",
        );

        // Edge #2 → #1 must be present even though the blocker (#1) is
        // CLOSED — the graph layer is responsible for the open/closed
        // boundary, not the GraphQL fetch.
        assert_eq!(edges.len(), 1, "closed blockers still produce edges");
        assert_eq!(edges[0].source.number, 2);
        assert_eq!(edges[0].target.number, 1);
    }

    // ── check_rest_response ────────────────────────────────────────────

    /// Builds a `reqwest::Response` from raw parts without any network access.
    fn mock_response(status: u16, headers: &[(&str, &str)], body: &str) -> reqwest::Response {
        let mut builder = http::Response::builder().status(status);
        for &(k, v) in headers {
            builder = builder.header(k, v);
        }
        let http_resp = builder.body(body.to_owned()).unwrap();
        reqwest::Response::from(http_resp)
    }

    #[tokio::test]
    async fn check_rest_response_200_passes_through() {
        let resp = mock_response(200, &[], "ok");
        let result = check_rest_response(resp).await;
        let ok_resp = result.expect("200 should return Ok");
        assert_eq!(ok_resp.text().await.unwrap(), "ok");
    }

    #[tokio::test]
    async fn check_rest_response_429_returns_rate_limited() {
        // Unix timestamp 1_700_000_000 → 2023-11-14T22:13:20Z
        let resp = mock_response(429, &[("x-ratelimit-reset", "1700000000")], "");
        let err = check_rest_response(resp)
            .await
            .expect_err("429 should return Err");

        match err {
            errors::Error::RateLimited { reset_at } => {
                let expected = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
                assert_eq!(reset_at, expected);
            }
            other => panic!("expected RateLimited, got: {other}"),
        }
    }

    #[tokio::test]
    async fn check_rest_response_429_missing_header_falls_back() {
        let before = Utc::now();
        let resp = mock_response(429, &[], "");
        let err = check_rest_response(resp)
            .await
            .expect_err("429 should return Err");

        match err {
            errors::Error::RateLimited { reset_at } => {
                let after = Utc::now();
                assert!(
                    reset_at >= before && reset_at <= after,
                    "reset_at ({reset_at}) should be between {before} and {after}",
                );
            }
            other => panic!("expected RateLimited, got: {other}"),
        }
    }

    #[tokio::test]
    async fn check_rest_response_500_returns_github_api() {
        let resp = mock_response(500, &[], "Internal Server Error");
        let err = check_rest_response(resp)
            .await
            .expect_err("500 should return Err");

        match err {
            errors::Error::GitHubApi { status, message } => {
                assert_eq!(status, 500);
                assert_eq!(message, "Internal Server Error");
            }
            other => panic!("expected GitHubApi, got: {other}"),
        }
    }

    #[tokio::test]
    async fn check_rest_response_404_returns_github_api() {
        let resp = mock_response(404, &[], "Not Found");
        let err = check_rest_response(resp)
            .await
            .expect_err("404 should return Err");

        match err {
            errors::Error::GitHubApi { status, message } => {
                assert_eq!(status, 404);
                assert_eq!(message, "Not Found");
            }
            other => panic!("expected GitHubApi, got: {other}"),
        }
    }

    // ── GraphQL reducer: FORBIDDEN partition (SPEC §11.1 wiring) ───────

    #[tokio::test]
    async fn graphql_errors_array_with_forbidden_type_emits_forbidden_variant() {
        // When any errors[i].type == "FORBIDDEN", the reducer MUST emit
        // `GitHubGraphQLForbidden` with the forbidden messages, rather
        // than substring-sniffing after reducing to a `Vec<String>`.
        // This is the wire-form partition contract per user decision
        // 2026-04-17 (unblock-6xj).
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "errors": [
                    {
                        "type": "FORBIDDEN",
                        "message": "Resource not accessible by integration"
                    }
                ]
            })))
            .mount(&server)
            .await;

        let err = client
            .graphql("query { viewer { login } }", serde_json::json!({}))
            .await
            .expect_err("FORBIDDEN should produce Err");

        match err {
            errors::Error::GitHubGraphQLForbidden { errors } => {
                assert_eq!(errors.len(), 1);
                assert_eq!(errors[0], "Resource not accessible by integration");
            }
            other => panic!("expected GitHubGraphQLForbidden, got: {other}"),
        }
    }

    #[tokio::test]
    async fn graphql_errors_array_without_forbidden_type_emits_graphql_variant() {
        // Non-FORBIDDEN-typed GraphQL errors MUST keep their existing
        // `GitHubGraphQL` shape. Regression guard: the partition logic
        // must NOT default all typed errors into the FORBIDDEN bucket.
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "errors": [
                    {
                        "type": "NOT_FOUND",
                        "message": "Could not resolve to a Repository with the name"
                    }
                ]
            })))
            .mount(&server)
            .await;

        let err = client
            .graphql("query { viewer { login } }", serde_json::json!({}))
            .await
            .expect_err("error should produce Err");

        match err {
            errors::Error::GitHubGraphQL { errors } => {
                assert_eq!(errors.len(), 1);
                assert!(errors[0].contains("Repository"));
            }
            other => panic!("expected GitHubGraphQL, got: {other}"),
        }
    }

    #[tokio::test]
    async fn graphql_errors_mixed_forbidden_and_other_prefers_forbidden() {
        // When at least one error is FORBIDDEN-typed and others are
        // not, the reducer MUST emit `GitHubGraphQLForbidden` with
        // ONLY the forbidden messages — FORBIDDEN is authoritative.
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "errors": [
                    { "type": "NOT_FOUND", "message": "side-effect error" },
                    { "type": "FORBIDDEN", "message": "access denied" }
                ]
            })))
            .mount(&server)
            .await;

        let err = client
            .graphql("query { viewer { login } }", serde_json::json!({}))
            .await
            .expect_err("error should produce Err");

        match err {
            errors::Error::GitHubGraphQLForbidden { errors } => {
                assert_eq!(errors.len(), 1);
                assert_eq!(errors[0], "access denied");
            }
            other => panic!("expected GitHubGraphQLForbidden, got: {other}"),
        }
    }

    #[tokio::test]
    async fn graphql_errors_all_forbidden_with_empty_messages_still_emits_forbidden() {
        // An `errors` array whose every FORBIDDEN-typed entry has a
        // missing or empty `message` field MUST still emit the
        // `GitHubGraphQLForbidden` variant with an empty `errors`
        // vector. The `type == "FORBIDDEN"` field is the authoritative
        // wire signal for the variant choice (per SPEC §11.1 and the
        // inline comment at `graphql.rs:454-465`): variant presence is
        // driven by type presence, not by whether any FORBIDDEN entry
        // carries a populated `message` body.
        //
        // The message-partition loop still drops empty-message entries
        // for hygiene (unblock-eos.22) — so the resulting FORBIDDEN
        // variant carries an empty `Vec<String>` — but the variant
        // itself MUST survive so downstream classifiers
        // (`classify_cross_repo_fetch`) can upgrade no-message-body 403
        // payloads to `DomainError::CrossRepoAccessDenied`. Without
        // this guarantee the cross-repo 403 signal would silently
        // downgrade to a generic 422-bucket `GitHubGraphQL` variant.
        //
        // See unblock-eos.24 (this bead). Companion test
        // `graphql_errors_array_drops_empty_message_but_keeps_populated_forbidden`
        // guards that the message-level filter contract from
        // unblock-eos.22 is preserved — this test guards that the
        // variant-level decision is driven by `type`, not by message
        // population.
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        // Two entries: one FORBIDDEN-typed with NO message field, one
        // FORBIDDEN-typed with an explicit empty string. Both must be
        // dropped by the message-partition loop (empty-message
        // hygiene) but the pre-scan MUST still observe `type ==
        // "FORBIDDEN"` and emit the FORBIDDEN variant with an empty
        // vector.
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "errors": [
                    { "type": "FORBIDDEN" },
                    { "type": "FORBIDDEN", "message": "" }
                ]
            })))
            .mount(&server)
            .await;

        let err = client
            .graphql("query { viewer { login } }", serde_json::json!({}))
            .await
            .expect_err("empty-message FORBIDDEN errors array should still produce Err");

        match err {
            errors::Error::GitHubGraphQLForbidden { errors } => {
                assert!(
                    errors.is_empty(),
                    "empty-message entries must not pollute the vector, got: {errors:?}"
                );
            }
            errors::Error::GitHubGraphQL { errors } => panic!(
                "GitHubGraphQL fall-through must NOT occur when any entry has type=FORBIDDEN, got: {errors:?}"
            ),
            other => panic!("expected GitHubGraphQLForbidden, got: {other}"),
        }
    }

    #[tokio::test]
    async fn graphql_errors_forbidden_empty_message_plus_other_typed_populated() {
        // Mixed payload: one FORBIDDEN entry with NO message body,
        // alongside a NOT_FOUND entry with a populated message. The
        // FORBIDDEN variant MUST still win by type even though its
        // message was dropped by the hygiene filter — the pre-scan's
        // authority over mixed non-FORBIDDEN entries is the
        // load-bearing invariant.
        //
        // Guards the "FORBIDDEN is authoritative regardless of peers"
        // invariant from regressions: a future refactor that moves the
        // variant decision back onto `!forbidden_messages.is_empty()`
        // would cause this test to fail by emitting
        // `GitHubGraphQL { errors: ["not found"] }` instead of the
        // FORBIDDEN variant. See unblock-eos.24.
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "errors": [
                    { "type": "FORBIDDEN" },
                    { "type": "NOT_FOUND", "message": "not found" }
                ]
            })))
            .mount(&server)
            .await;

        let err = client
            .graphql("query { viewer { login } }", serde_json::json!({}))
            .await
            .expect_err("mixed FORBIDDEN/NOT_FOUND should produce Err");

        match err {
            errors::Error::GitHubGraphQLForbidden { errors } => {
                assert!(
                    errors.is_empty(),
                    "FORBIDDEN entry had no message body; NOT_FOUND peer's message MUST NOT leak into FORBIDDEN vector, got: {errors:?}"
                );
            }
            errors::Error::GitHubGraphQL { errors } => panic!(
                "GitHubGraphQL fall-through must NOT occur when any entry has type=FORBIDDEN, got: {errors:?}"
            ),
            other => panic!("expected GitHubGraphQLForbidden, got: {other}"),
        }
    }

    #[tokio::test]
    async fn graphql_errors_array_drops_empty_message_but_keeps_populated_forbidden() {
        // Mixed payload: one FORBIDDEN entry with a real message, one
        // FORBIDDEN entry without a message body. The empty-message
        // entry MUST be dropped, but the populated one MUST still
        // drive the FORBIDDEN variant. Guards against over-aggressive
        // filtering that could mask a real permission-denied signal.
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "errors": [
                    { "type": "FORBIDDEN" },
                    { "type": "FORBIDDEN", "message": "access denied" }
                ]
            })))
            .mount(&server)
            .await;

        let err = client
            .graphql("query { viewer { login } }", serde_json::json!({}))
            .await
            .expect_err("FORBIDDEN should produce Err");

        match err {
            errors::Error::GitHubGraphQLForbidden { errors } => {
                assert_eq!(errors.len(), 1, "empty-message entry must be dropped");
                assert_eq!(errors[0], "access denied");
            }
            other => panic!("expected GitHubGraphQLForbidden, got: {other}"),
        }
    }

    // ── fetch_issue_in_repo: cross-repo classifier (SPEC §11.1) ────────

    #[tokio::test]
    async fn fetch_issue_in_repo_cross_repo_http_403_upgrades_to_access_denied() {
        // A cross-repo fetch returning HTTP 403 MUST upgrade to
        // `DomainError::CrossRepoAccessDenied` carrying the target
        // owner/repo. Local fetches do NOT upgrade.
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());
        // client.owner()/repo() are "test-owner"/"test-repo" — any
        // other owner/repo is treated as cross-repo.

        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(403).set_body_string("Forbidden"))
            .mount(&server)
            .await;

        let err = client
            .fetch_issue_in_repo("acme", "widgets", 1)
            .await
            .expect_err("403 should produce Err");

        match err {
            errors::Error::Domain { source } => {
                let msg = source.to_string();
                assert_eq!(source.status_code(), 403);
                assert!(msg.contains("acme"), "expected owner in message: {msg}");
                assert!(msg.contains("widgets"), "expected repo in message: {msg}");
            }
            other => panic!("expected CrossRepoAccessDenied Domain error, got: {other}"),
        }
    }

    #[tokio::test]
    async fn fetch_issue_in_repo_cross_repo_graphql_forbidden_upgrades_to_access_denied() {
        // A cross-repo fetch whose GraphQL response contains
        // errors[i].type == "FORBIDDEN" MUST upgrade to
        // `CrossRepoAccessDenied`.
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "errors": [
                    {
                        "type": "FORBIDDEN",
                        "message": "Resource not accessible by integration"
                    }
                ]
            })))
            .mount(&server)
            .await;

        let err = client
            .fetch_issue_in_repo("acme", "widgets", 1)
            .await
            .expect_err("FORBIDDEN should produce Err");

        match err {
            errors::Error::Domain { source } => {
                let msg = source.to_string();
                assert_eq!(source.status_code(), 403);
                assert!(msg.contains("acme"), "expected owner in message: {msg}");
                assert!(msg.contains("widgets"), "expected repo in message: {msg}");
            }
            other => panic!("expected CrossRepoAccessDenied Domain error, got: {other}"),
        }
    }

    #[tokio::test]
    async fn fetch_issue_in_repo_local_403_stays_as_github_api() {
        // A local fetch (owner/repo == configured) returning HTTP 403
        // MUST stay as `GitHubApi` — `CrossRepoAccessDenied` is
        // cross-repo-semantic by name.
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let client = GitHubClient::new_for_test(&server.uri());

        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(403).set_body_string("Forbidden"))
            .mount(&server)
            .await;

        let err = client
            .fetch_issue_in_repo("test-owner", "test-repo", 1)
            .await
            .expect_err("403 should produce Err");

        match err {
            errors::Error::GitHubApi { status, .. } => assert_eq!(status, 403),
            other => panic!("expected GitHubApi passthrough for local 403, got: {other}"),
        }
    }
}
