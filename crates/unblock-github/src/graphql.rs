//! GraphQL queries for GitHub API.
//!
//! - `fetch_graph_data()` — paginated query returning all open issues, blocking edges,
//!   and Projects V2 field values in a single request
//! - `fetch_issue()` — single issue with comments, deps, parent, sub-issues, and all fields

use chrono::{DateTime, Utc};
use snafu::ResultExt as _;
use tracing::{debug, instrument, warn};
use unblock_core::types::{
    BlockingEdge, Issue, IssueComment, IssueState, IssueType, Priority, RelatedIssue, Status,
};

use crate::client::GitHubClient;
use crate::errors::{self, Error};

/// GraphQL query for fetching a single issue with full details.
///
/// Includes: all standard fields, comments (first 50), blockedBy, blocking,
/// parent, subIssues, and Projects V2 field values.
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
      trackedInIssues(first: 50) {
        nodes {
          number
          title
          state
        }
      }
      trackedBy: trackedByIssues(first: 50) {
        nodes {
          number
          title
          state
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

/// GraphQL query for fetching all open issues with pagination.
///
/// Returns issues with standard fields, blocking relationships (via
/// `trackedByIssues`), and Projects V2 field values. Does **not** include
/// comments, parent, sub-issues, or `trackedInIssues` — those are only
/// fetched by [`FETCH_ISSUE_QUERY`] for single-issue detail views.
///
/// Uses cursor-based pagination on the `issues` connection (`first: 100`,
/// `after: $cursor`). The caller must loop until `pageInfo.hasNextPage` is
/// false.
const FETCH_GRAPH_DATA_QUERY: &str = "
query FetchGraphData($owner: String!, $repo: String!, $cursor: String) {
  repository(owner: $owner, name: $repo) {
    issues(first: 100, states: OPEN, after: $cursor) {
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
        trackedBy: trackedByIssues(first: 50) {
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

        let response = self.graphql(FETCH_ISSUE_QUERY, variables).await?;

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

    /// Fetches all open issues and blocking edges for the dependency graph.
    ///
    /// Returns a tuple of `(issues, edges)` where:
    /// - `issues` contains all open issues with standard fields and Projects V2
    ///   field values, but **not** comments, parent, sub-issues, or the
    ///   `blocked_by`/`blocking` vectors on [`Issue`] (those remain empty).
    /// - `edges` contains [`BlockingEdge`] entries extracted from GitHub's
    ///   `trackedByIssues` relationship, where `source` is the blocked issue
    ///   and `target` is the blocker.
    ///
    /// Paginates using GraphQL cursor pagination (100 issues per page) until
    /// all open issues are fetched. Returns empty vectors for a repo with no
    /// open issues.
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
                    // Extract blocking edges from trackedByIssues.
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
        if let Some(arr) = json
            .get("errors")
            .and_then(serde_json::Value::as_array)
            .filter(|a| !a.is_empty())
        {
            let messages: Vec<String> = arr
                .iter()
                .filter_map(|e| e.get("message").and_then(|m| m.as_str()))
                .map(String::from)
                .collect();
            return Err(errors::GitHubGraphQLSnafu { errors: messages }.build());
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
    let blocked_by = parse_related_issues(value, "trackedBy");
    let blocking = parse_related_issues(value, "trackedInIssues");
    let parent = parse_parent_issue(value);
    let sub_issues = parse_related_issues(value, "subIssues");

    // Extract Projects V2 field values.
    let field_values = extract_field_values(value);
    let status = parse_status_field(&field_values);
    let priority = parse_priority_field(&field_values);
    let issue_type = parse_issue_type_field(&field_values);
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
        pipeline_stage: None,
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
    let issue_type = parse_issue_type_field(&field_values);
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
        pipeline_stage: None,
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
/// Reads the `trackedBy` (alias for `trackedByIssues`) connection and creates
/// one edge per blocker. Each edge has `source` = the blocked issue's
/// [`QualifiedId`] and `target` = the blocker's [`QualifiedId`]. Skips entries
/// with a zero issue number.
///
/// Currently all edges are within the same `owner/repo` because GitHub's
/// `trackedByIssues` connection only returns issues in the same repository.
/// Cross-repo blockers (added via the `depends` MCP tool) will produce edges
/// with different `owner/repo` prefixes once the GraphQL query is extended.
fn extract_blocking_edges(node: &serde_json::Value, owner: &str, repo: &str) -> Vec<BlockingEdge> {
    let issue_number = json_u64(node, "number");
    let source_qid = unblock_core::types::QualifiedId::new(owner, repo, issue_number);
    let mut edges = Vec::new();

    if let Some(blockers) = node
        .get("trackedBy")
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

/// Parses a related-issues connection (blockedBy, blocking, subIssues) into
/// [`RelatedIssue`] instances.
fn parse_related_issues(value: &serde_json::Value, key: &str) -> Vec<RelatedIssue> {
    value
        .get(key)
        .and_then(|v| v.get("nodes"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|node| RelatedIssue {
                    number: json_u64(node, "number"),
                    title: json_string(node, "title"),
                    state: parse_issue_state(node),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parses the parent issue field into an optional [`RelatedIssue`].
fn parse_parent_issue(value: &serde_json::Value) -> Option<RelatedIssue> {
    let parent = value.get("parent")?;
    if parent.is_null() {
        return None;
    }
    Some(RelatedIssue {
        number: json_u64(parent, "number"),
        title: json_string(parent, "title"),
        state: parse_issue_state(parent),
    })
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
fn parse_status_field(fields: &std::collections::HashMap<String, String>) -> Status {
    match fields.get("Status").map(String::as_str) {
        Some("In Progress") => Status::InProgress,
        Some("Blocked") => Status::Blocked,
        Some("Deferred") => Status::Deferred,
        Some("Done" | "Closed") => Status::Closed,
        _ => Status::Ready,
    }
}

/// Maps the "Priority" field value to a [`Priority`] enum variant.
fn parse_priority_field(fields: &std::collections::HashMap<String, String>) -> Priority {
    match fields.get("Priority").map(String::as_str) {
        Some("P0") => Priority::P0,
        Some("P1") => Priority::P1,
        Some("P3") => Priority::P3,
        Some("P4") => Priority::P4,
        // Default to medium (P2) when missing or unrecognized.
        _ => Priority::P2,
    }
}

/// Maps the `IssueType` field value to an [`IssueType`] enum variant.
fn parse_issue_type_field(fields: &std::collections::HashMap<String, String>) -> Option<IssueType> {
    match fields.get("IssueType").map(String::as_str) {
        Some("Task") => Some(IssueType::Task),
        Some("Bug") => Some(IssueType::Bug),
        Some("Feature") => Some(IssueType::Feature),
        Some("Epic") => Some(IssueType::Epic),
        Some("Chore") => Some(IssueType::Chore),
        Some("Spike") => Some(IssueType::Spike),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "trackedInIssues": {
                "nodes": [
                    {"number": 5, "title": "Blocker", "state": "OPEN"},
                    {"number": 10, "title": "Other blocker", "state": "CLOSED"}
                ]
            }
        });
        let related = parse_related_issues(&v, "trackedInIssues");
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
    fn status_field_mapping() {
        let mut fields = std::collections::HashMap::new();
        fields.insert("Status".to_owned(), "In Progress".to_owned());
        assert_eq!(parse_status_field(&fields), Status::InProgress);

        fields.insert("Status".to_owned(), "Blocked".to_owned());
        assert_eq!(parse_status_field(&fields), Status::Blocked);

        fields.insert("Status".to_owned(), "Deferred".to_owned());
        assert_eq!(parse_status_field(&fields), Status::Deferred);

        fields.insert("Status".to_owned(), "Done".to_owned());
        assert_eq!(parse_status_field(&fields), Status::Closed);

        fields.insert("Status".to_owned(), "Closed".to_owned());
        assert_eq!(parse_status_field(&fields), Status::Closed);
    }

    #[test]
    fn status_field_missing_defaults_ready() {
        let fields = std::collections::HashMap::new();
        assert_eq!(parse_status_field(&fields), Status::Ready);
    }

    // ── parse_priority_field ────────────────────────────────────────────

    #[test]
    fn priority_field_mapping() {
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

    // ── parse_issue_type_field ──────────────────────────────────────────

    #[test]
    fn issue_type_field_mapping() {
        let mut fields = std::collections::HashMap::new();
        for (label, expected) in [
            ("Task", IssueType::Task),
            ("Bug", IssueType::Bug),
            ("Feature", IssueType::Feature),
            ("Epic", IssueType::Epic),
            ("Chore", IssueType::Chore),
            ("Spike", IssueType::Spike),
        ] {
            fields.insert("IssueType".to_owned(), label.to_owned());
            assert_eq!(parse_issue_type_field(&fields), Some(expected));
        }
    }

    #[test]
    fn issue_type_field_missing_returns_none() {
        let fields = std::collections::HashMap::new();
        assert!(parse_issue_type_field(&fields).is_none());
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
            "trackedInIssues": {
                "nodes": [
                    {"number": 20, "title": "Blocks me", "state": "OPEN"}
                ]
            },
            "trackedBy": {
                "nodes": [
                    {"number": 10, "title": "Dep A", "state": "OPEN"}
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

        assert_eq!(issue.blocking.len(), 1);
        assert_eq!(issue.blocking[0].number, 20);

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
            "trackedInIssues": {"nodes": []},
            "trackedBy": {"nodes": []},
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
        assert_eq!(issue.status, Status::Ready);
        assert_eq!(issue.priority, Priority::P2);
    }

    // ── parse_graph_issue ──────────────────────────────────────────────

    #[test]
    fn parse_graph_issue_extracts_standard_fields() {
        let json = serde_json::json!({
            "id": "MDU6SXNzdWUx",
            "number": 42,
            "title": "Graph issue",
            "body": "Some body",
            "state": "OPEN",
            "url": "https://github.com/owner/repo/issues/42",
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-02T00:00:00Z",
            "labels": {"nodes": [{"name": "bug"}]},
            "milestone": {"title": "v1.0"},
            "assignees": {"nodes": [{"login": "alice"}]},
            "trackedBy": {
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
                            {"field": {"name": "IssueType"}, "name": "Bug"},
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
        assert!(issue.pipeline_stage.is_none());
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
            "trackedBy": {
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
        assert_eq!(issue.status, Status::Ready);
        assert_eq!(issue.priority, Priority::P2);
        assert!(issue.issue_type.is_none());
        assert!(issue.agent.is_none());
        assert!(issue.story_points.is_none());
        assert!(issue.defer_until.is_none());
        assert!(issue.pipeline_stage.is_none());
        assert!(issue.claimed_at.is_none());
    }

    // ── BlockingEdge extraction (via extract_blocking_edges helper) ──────

    #[test]
    fn blocking_edge_direction_source_blocked_by_target() {
        let node = serde_json::json!({
            "number": 42,
            "trackedBy": {
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
            "trackedBy": {
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
    fn blocking_edge_empty_tracked_by_yields_no_edges() {
        let node = serde_json::json!({
            "number": 42,
            "trackedBy": {
                "nodes": []
            }
        });

        let edges = extract_blocking_edges(&node, "test-owner", "test-repo");
        assert!(edges.is_empty());
    }

    #[test]
    fn blocking_edge_missing_tracked_by_yields_no_edges() {
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
    /// milestone, assignees, and optionally trackedBy edges.
    fn make_page_response(
        issues: &[(u64, &str, Vec<u64>)],
        has_next_page: bool,
        end_cursor: Option<&str>,
    ) -> serde_json::Value {
        let nodes: Vec<serde_json::Value> = issues
            .iter()
            .map(|(number, title, blockers)| {
                let tracked_by_nodes: Vec<serde_json::Value> = blockers
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
                    "trackedBy": {"nodes": tracked_by_nodes}
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
}
