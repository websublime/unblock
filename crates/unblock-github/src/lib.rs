//! # unblock-github
//!
//! GitHub API client shared across unblock products (MCP server and desktop app).
//!
//! Provides:
//!
//! - **Client** — `GitHubClient` with auth, repo resolution, and API base URL for GHE
//! - **GraphQL** — paginated queries for issues, blocking edges, and Projects V2 fields
//! - **Mutations** — REST and GraphQL mutations for create, close, reopen, update
//! - **Projects** — Projects V2 field resolution, setup, and batch updates
//! - **Errors** — infrastructure error types with MCP error conversion

/// GitHub API client bootstrap and configuration.
pub mod client;

/// GraphQL queries for fetching issues, dependencies, and field values.
pub mod graphql;

/// REST and GraphQL mutations for issue and dependency operations.
pub mod mutations;

/// Projects V2 field management: resolve, setup, update.
pub mod projects;

/// Infrastructure error types.
pub mod errors;
