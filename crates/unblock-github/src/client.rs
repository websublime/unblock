//! GitHub API client bootstrap.
//!
//! `GitHubClient::new(config)` creates a reqwest client with auth headers.
//! Supports `resolve_repo()` from env or git remote and `resolve_project()`
//! from linked Projects V2. Configurable `api_base_url` for GitHub Enterprise.
