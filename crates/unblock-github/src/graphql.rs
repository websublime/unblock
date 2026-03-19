//! GraphQL queries for GitHub API.
//!
//! - `fetch_graph_data()` — paginated query returning all open issues, blocking edges,
//!   and Projects V2 field values in a single request
//! - `fetch_issue()` — single issue with comments, deps, parent, sub-issues, and all fields
