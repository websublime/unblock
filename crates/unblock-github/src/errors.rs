//! Infrastructure error types using snafu.
//!
//! Variants: `GitHubApi`, `GitHubGraphQL`, `GitHubUnavailable`, `RateLimited`,
//! `CircuitBreakerOpen`, `ProjectNotConfigured`, `GitRemote`.
//!
//! Includes `From<Error> for McpError` conversion for MCP tool responses.
