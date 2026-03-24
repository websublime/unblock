//! GraphQL queries for GitHub API.
//!
//! - `fetch_graph_data()` — paginated query returning all open issues, blocking edges,
//!   and Projects V2 field values in a single request
//! - `fetch_issue()` — single issue with comments, deps, parent, sub-issues, and all fields

use chrono::{DateTime, Utc};
use snafu::ResultExt as _;
use tracing::{debug, instrument};
use unblock_core::types::{
    Issue, IssueComment, IssueState, IssueType, Priority, ReadyState, RelatedIssue, Status,
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
        let variables = serde_json::json!({
            "owner": self.owner(),
            "repo": self.repo(),
            "number": number.cast_signed(),
        });

        let response = self.graphql(FETCH_ISSUE_QUERY, variables).await?;

        let issue_value = &response["data"]["repository"]["issue"];

        if issue_value.is_null() {
            return Err(unblock_core::errors::IssueNotFoundSnafu { number }
                .build()
                .into());
        }

        Ok(parse_issue(issue_value))
    }

    /// Sends a GraphQL query to the GitHub API.
    ///
    /// Posts the query and variables as JSON to the GraphQL endpoint.
    /// Handles GraphQL-level errors (errors array in response) and
    /// HTTP-level errors (non-2xx status codes, rate limiting).
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
        let url = self.graphql_url();
        let body = serde_json::json!({
            "query": query,
            "variables": variables,
        });

        debug!(url = %url, "Sending GraphQL request");

        let response = self
            .http()
            .post(&url)
            .json(&body)
            .send()
            .await
            .context(errors::GitHubUnavailableSnafu)?;

        let status = response.status();

        // Handle rate limiting.
        if status.as_u16() == 429 {
            let reset_at = parse_rate_limit_reset(&response);
            return Err(errors::RateLimitedSnafu { reset_at }.build());
        }

        // Handle non-2xx status codes.
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
            return Err(errors::GitHubGraphQLSnafu {
                errors: messages.join("; "),
            }
            .build());
        }

        Ok(json)
    }
}

/// Parses the `X-RateLimit-Reset` header from a response into a `DateTime<Utc>`.
///
/// Falls back to `Utc::now()` if the header is missing or unparseable.
fn parse_rate_limit_reset(response: &reqwest::Response) -> DateTime<Utc> {
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
fn parse_issue(value: &serde_json::Value) -> Issue {
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
    let blocked_by = parse_related_issues(value, "trackedInIssues");
    let blocking = parse_related_issues(value, "trackedBy");
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
    let ready_state = parse_ready_state_field(&field_values);
    let claimed_at = field_values
        .get("ClaimedAt")
        .and_then(|v| v.parse::<DateTime<Utc>>().ok());

    Issue {
        number,
        node_id,
        title,
        issue_type,
        status,
        priority,
        agent,
        claimed_at,
        ready_state,
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
        _ => Status::Open,
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

/// Maps the `ReadyState` field value to a [`ReadyState`] enum variant.
fn parse_ready_state_field(fields: &std::collections::HashMap<String, String>) -> ReadyState {
    match fields.get("ReadyState").map(String::as_str) {
        Some("Ready") => ReadyState::Ready,
        Some("Blocked") => ReadyState::Blocked,
        Some("Closed") => ReadyState::Closed,
        // Default to NotReady when missing, "Not Ready", or unrecognized.
        _ => ReadyState::NotReady,
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
    fn status_field_missing_defaults_open() {
        let fields = std::collections::HashMap::new();
        assert_eq!(parse_status_field(&fields), Status::Open);
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

    // ── parse_ready_state_field ─────────────────────────────────────────

    #[test]
    fn ready_state_field_mapping() {
        let mut fields = std::collections::HashMap::new();
        for (label, expected) in [
            ("Ready", ReadyState::Ready),
            ("Blocked", ReadyState::Blocked),
            ("Closed", ReadyState::Closed),
            ("Not Ready", ReadyState::NotReady),
        ] {
            fields.insert("ReadyState".to_owned(), label.to_owned());
            assert_eq!(parse_ready_state_field(&fields), expected);
        }
    }

    #[test]
    fn ready_state_field_missing_defaults_not_ready() {
        let fields = std::collections::HashMap::new();
        assert_eq!(parse_ready_state_field(&fields), ReadyState::NotReady);
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
                    {"number": 10, "title": "Dep A", "state": "OPEN"}
                ]
            },
            "trackedBy": {
                "nodes": [
                    {"number": 20, "title": "Blocked by me", "state": "OPEN"}
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

        let issue = parse_issue(&json);
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

        let issue = parse_issue(&json);
        assert_eq!(issue.number, 1);
        assert!(issue.body.is_none());
        assert!(issue.comments.is_empty());
        assert!(issue.blocked_by.is_empty());
        assert!(issue.blocking.is_empty());
        assert!(issue.parent.is_none());
        assert!(issue.sub_issues.is_empty());
        assert_eq!(issue.status, Status::Open);
        assert_eq!(issue.priority, Priority::P2);
    }
}
