//! GitHub API client bootstrap.
//!
//! `GitHubClient::new(config)` creates a `reqwest` client with auth headers.
//! Supports repo resolution from env or git remote and project resolution
//! from linked Projects V2. Configurable `api_base_url` for GitHub Enterprise.

use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
use tracing::info;
use unblock_core::config::Config;

use crate::errors::{self, Error, GitRemoteSnafu};
use snafu::ResultExt as _;

/// Central struct for all GitHub API communication.
///
/// Holds a configured `reqwest::Client` with default auth headers, the resolved
/// repository owner/name, and optional Projects V2 metadata.
///
/// Created via [`GitHubClient::new`], which resolves the repository from
/// `UNBLOCK_REPO` or the git remote, and the project number from
/// `UNBLOCK_PROJECT` or auto-detection.
#[derive(Debug)]
pub struct GitHubClient {
    /// Pre-configured HTTP client with auth and API headers.
    http: reqwest::Client,
    /// GitHub API base URL (e.g. `https://api.github.com`).
    api_base_url: String,
    /// Repository owner (e.g. `websublime`).
    owner: String,
    /// Repository name (e.g. `unblock`).
    repo: String,
    /// Optional GitHub Projects V2 number.
    project_number: Option<u64>,
}

impl GitHubClient {
    /// Creates a new `GitHubClient` from the given configuration.
    ///
    /// Builds a `reqwest::Client` with the following default headers:
    /// - `Authorization: Bearer {token}`
    /// - `User-Agent: unblock-github/{version}`
    /// - `Accept: application/vnd.github+json`
    /// - `X-GitHub-Api-Version: 2022-11-28`
    ///
    /// Then resolves the repository owner/name and project number.
    ///
    /// # Errors
    ///
    /// Returns [`Error::GitRemote`] if repo resolution fails, or
    /// [`Error::GitHubUnavailable`] if the HTTP client cannot be built.
    #[allow(clippy::unused_async)] // Will be truly async once resolve_project does GraphQL (bead 467.6).
    pub async fn new(config: &Config) -> Result<Self, Error> {
        let mut headers = HeaderMap::new();

        // Authorization header — bearer token.
        let auth_value = format!("Bearer {}", config.token);
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&auth_value).map_err(|e| {
                errors::GitRemoteSnafu {
                    message: format!("invalid token header value: {e}"),
                }
                .build()
            })?,
        );

        // User-Agent header.
        // Uses unblock-github crate name + version since this library is shared
        // across products (MCP server, desktop app). env!("CARGO_PKG_VERSION")
        // resolves to this crate's version, not the binary's.
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static(concat!("unblock-github/", env!("CARGO_PKG_VERSION"))),
        );

        // Accept header — GitHub JSON format.
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );

        // GitHub API version header.
        headers.insert(
            "X-GitHub-Api-Version",
            HeaderValue::from_static("2022-11-28"),
        );

        let http = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .context(errors::GitHubUnavailableSnafu)?;

        let (owner, repo) = Self::resolve_repo(config)?;
        let project_number = Self::resolve_project(config);

        info!(
            owner = %owner,
            repo = %repo,
            project_number = ?project_number,
            api_base_url = %config.api_base_url,
            "GitHubClient initialized"
        );

        Ok(Self {
            http,
            api_base_url: config.api_base_url.clone(),
            owner,
            repo,
            project_number,
        })
    }

    /// Returns a reference to the underlying HTTP client.
    #[must_use]
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// Returns the repository owner.
    #[must_use]
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Returns the repository name.
    #[must_use]
    pub fn repo(&self) -> &str {
        &self.repo
    }

    /// Returns the GitHub API base URL.
    #[must_use]
    pub fn api_base_url(&self) -> &str {
        &self.api_base_url
    }

    /// Returns the project number, if configured.
    #[must_use]
    pub fn project_number(&self) -> Option<u64> {
        self.project_number
    }

    /// Builds a REST API URL from a path suffix.
    ///
    /// Example: `rest_url("/repos/owner/repo/issues")` produces
    /// `https://api.github.com/repos/owner/repo/issues`.
    #[must_use]
    pub fn rest_url(&self, path: &str) -> String {
        format!("{}{path}", self.api_base_url)
    }

    /// Builds the GraphQL endpoint URL.
    ///
    /// Handles both github.com and GitHub Enterprise Server:
    /// - `https://api.github.com` -> `https://api.github.com/graphql`
    /// - `https://<host>/api/v3` -> `https://<host>/api/graphql`
    #[must_use]
    pub fn graphql_url(&self) -> String {
        let base = self
            .api_base_url
            .strip_suffix("/v3")
            .unwrap_or(&self.api_base_url);
        format!("{base}/graphql")
    }

    /// Resolves the repository owner and name from configuration.
    ///
    /// If `config.repo` is set (from `UNBLOCK_REPO`), it is split on `/`.
    /// Otherwise, the git remote origin URL is read from `.git/config` in the
    /// current working directory and parsed via [`parse_github_url`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::GitRemote`] if the repo cannot be determined.
    fn resolve_repo(config: &Config) -> Result<(String, String), Error> {
        if let Some(ref repo_str) = config.repo {
            // Config already validated the owner/repo format.
            let (owner, repo) = repo_str.split_once('/').ok_or_else(|| {
                GitRemoteSnafu {
                    message: format!("invalid repo format: {repo_str}"),
                }
                .build()
            })?;
            return Ok((owner.to_owned(), repo.to_owned()));
        }

        // Fall back to reading the git remote origin URL.
        // NOTE: Uses a relative path per the bead spec ("read .git/config in cwd").
        // This assumes the process CWD is the repository root at the time of client
        // construction. For the MCP server, this is guaranteed by the stdio transport
        // launching in the workspace root.
        let git_config = std::fs::read_to_string(".git/config").map_err(|e| {
            GitRemoteSnafu {
                message: format!("failed to read .git/config: {e}"),
            }
            .build()
        })?;

        let url = parse_remote_origin_url(&git_config).ok_or_else(|| {
            GitRemoteSnafu {
                message: "no remote origin URL found in .git/config".to_owned(),
            }
            .build()
        })?;

        parse_github_url(&url)
    }

    /// Resolves the project number from configuration.
    ///
    /// If `config.project_number` is set (from `UNBLOCK_PROJECT`), it is used
    /// directly. Otherwise, returns `None`. Full auto-detection via the GitHub
    /// Projects V2 API is implemented in a later task (bead unblock-467.6).
    fn resolve_project(config: &Config) -> Option<u64> {
        config.project_number
    }
}

/// Parses a GitHub URL into `(owner, repo)`.
///
/// Supported formats:
/// - `https://github.com/owner/repo.git`
/// - `https://github.com/owner/repo`
/// - `git@github.com:owner/repo.git`
/// - `git@github.com:owner/repo`
///
/// # Errors
///
/// Returns [`Error::GitRemote`] if the URL is not a recognized GitHub format.
pub fn parse_github_url(url: &str) -> Result<(String, String), Error> {
    let url = url.trim();

    // Try HTTPS format: https://github.com/owner/repo[.git]
    if let Some(path) = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
    {
        return parse_owner_repo_from_path(path, url);
    }

    // Try SSH format: git@github.com:owner/repo[.git]
    if let Some(path) = url.strip_prefix("git@github.com:") {
        return parse_owner_repo_from_path(path, url);
    }

    Err(GitRemoteSnafu {
        message: format!("not a GitHub URL: {url}"),
    }
    .build())
}

/// Extracts `(owner, repo)` from a `owner/repo[.git]` path segment.
fn parse_owner_repo_from_path(path: &str, original_url: &str) -> Result<(String, String), Error> {
    let path = path.strip_suffix(".git").unwrap_or(path);
    let path = path.trim_end_matches('/');

    let (owner, repo) = path.split_once('/').ok_or_else(|| {
        GitRemoteSnafu {
            message: format!("cannot extract owner/repo from URL: {original_url}"),
        }
        .build()
    })?;

    // Ensure no extra path segments (e.g. owner/repo/pulls).
    if repo.contains('/') {
        return Err(GitRemoteSnafu {
            message: format!("cannot extract owner/repo from URL: {original_url}"),
        }
        .build());
    }

    if owner.is_empty() || repo.is_empty() {
        return Err(GitRemoteSnafu {
            message: format!("cannot extract owner/repo from URL: {original_url}"),
        }
        .build());
    }

    Ok((owner.to_owned(), repo.to_owned()))
}

/// Parses the `[remote "origin"]` section of a `.git/config` file and extracts
/// the `url = ...` value.
///
/// Returns `None` if no remote origin URL is found.
fn parse_remote_origin_url(git_config: &str) -> Option<String> {
    let mut in_remote_origin = false;
    for line in git_config.lines() {
        let trimmed = line.trim();
        if trimmed == "[remote \"origin\"]" {
            in_remote_origin = true;
            continue;
        }
        if trimmed.starts_with('[') {
            if in_remote_origin {
                // We've left the [remote "origin"] section without finding a URL.
                return None;
            }
            continue;
        }
        if in_remote_origin {
            if let Some(value) = trimmed.strip_prefix("url = ") {
                return Some(value.trim().to_owned());
            }
            if let Some(value) = trimmed.strip_prefix("url=") {
                return Some(value.trim().to_owned());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_github_url ─────────────────────────────────────────────

    #[test]
    fn parse_https_with_git_suffix() {
        let (owner, repo) = parse_github_url("https://github.com/websublime/unblock.git").unwrap();
        assert_eq!(owner, "websublime");
        assert_eq!(repo, "unblock");
    }

    #[test]
    fn parse_https_without_git_suffix() {
        let (owner, repo) = parse_github_url("https://github.com/websublime/unblock").unwrap();
        assert_eq!(owner, "websublime");
        assert_eq!(repo, "unblock");
    }

    #[test]
    fn parse_ssh_with_git_suffix() {
        let (owner, repo) = parse_github_url("git@github.com:websublime/unblock.git").unwrap();
        assert_eq!(owner, "websublime");
        assert_eq!(repo, "unblock");
    }

    #[test]
    fn parse_ssh_without_git_suffix() {
        let (owner, repo) = parse_github_url("git@github.com:websublime/unblock").unwrap();
        assert_eq!(owner, "websublime");
        assert_eq!(repo, "unblock");
    }

    #[test]
    fn parse_https_with_trailing_slash() {
        let (owner, repo) = parse_github_url("https://github.com/acme/widgets/").unwrap();
        assert_eq!(owner, "acme");
        assert_eq!(repo, "widgets");
    }

    #[test]
    fn parse_non_github_url_returns_error() {
        let err = parse_github_url("https://gitlab.com/owner/repo").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("not a GitHub URL"),
            "expected 'not a GitHub URL' in: {msg}"
        );
    }

    #[test]
    fn parse_empty_string_returns_error() {
        assert!(parse_github_url("").is_err());
    }

    #[test]
    fn parse_garbage_returns_error() {
        assert!(parse_github_url("not-a-url").is_err());
    }

    #[test]
    fn parse_github_url_with_extra_segments_returns_error() {
        assert!(parse_github_url("https://github.com/owner/repo/pulls").is_err());
    }

    // ── parse_remote_origin_url ──────────────────────────────────────

    #[test]
    fn parse_git_config_https_remote() {
        let config = r#"
[core]
	repositoryformatversion = 0

[remote "origin"]
	url = https://github.com/websublime/unblock.git
	fetch = +refs/heads/*:refs/remotes/origin/*

[branch "main"]
	remote = origin
"#;
        let url = parse_remote_origin_url(config).unwrap();
        assert_eq!(url, "https://github.com/websublime/unblock.git");
    }

    #[test]
    fn parse_git_config_ssh_remote() {
        let config = r#"
[remote "origin"]
	url = git@github.com:websublime/unblock.git
	fetch = +refs/heads/*:refs/remotes/origin/*
"#;
        let url = parse_remote_origin_url(config).unwrap();
        assert_eq!(url, "git@github.com:websublime/unblock.git");
    }

    #[test]
    fn parse_git_config_no_remote_origin() {
        let config = r#"
[core]
	repositoryformatversion = 0
[branch "main"]
	remote = origin
"#;
        assert!(parse_remote_origin_url(config).is_none());
    }

    #[test]
    fn parse_git_config_other_remote_ignored() {
        let config = r#"
[remote "upstream"]
	url = https://github.com/other/repo.git
"#;
        assert!(parse_remote_origin_url(config).is_none());
    }

    // ── resolve_repo with config.repo set ────────────────────────────

    #[test]
    fn resolve_repo_uses_config_repo_when_set() {
        let config = Config {
            token: "ghp_test".to_owned(),
            api_base_url: "https://api.github.com".to_owned(),
            repo: Some("acme/widgets".to_owned()),
            project_number: None,
            agent: "agent".to_owned(),
            cache_ttl: 30,
            log_level: "info".to_owned(),
            otel_endpoint: None,
        };
        let (owner, repo) = GitHubClient::resolve_repo(&config).unwrap();
        assert_eq!(owner, "acme");
        assert_eq!(repo, "widgets");
    }

    // ── resolve_project ──────────────────────────────────────────────

    #[test]
    fn resolve_project_returns_config_value() {
        let config = Config {
            token: "ghp_test".to_owned(),
            api_base_url: "https://api.github.com".to_owned(),
            repo: None,
            project_number: Some(42),
            agent: "agent".to_owned(),
            cache_ttl: 30,
            log_level: "info".to_owned(),
            otel_endpoint: None,
        };
        assert_eq!(GitHubClient::resolve_project(&config), Some(42));
    }

    #[test]
    fn resolve_project_returns_none_when_not_set() {
        let config = Config {
            token: "ghp_test".to_owned(),
            api_base_url: "https://api.github.com".to_owned(),
            repo: None,
            project_number: None,
            agent: "agent".to_owned(),
            cache_ttl: 30,
            log_level: "info".to_owned(),
            otel_endpoint: None,
        };
        assert_eq!(GitHubClient::resolve_project(&config), None);
    }

    // ── rest_url and graphql_url ─────────────────────────────────────

    #[test]
    fn rest_url_formats_correctly() {
        let client = make_test_client("https://api.github.com");
        assert_eq!(
            client.rest_url("/repos/owner/repo/issues"),
            "https://api.github.com/repos/owner/repo/issues"
        );
    }

    #[test]
    fn graphql_url_github_com() {
        let client = make_test_client("https://api.github.com");
        assert_eq!(client.graphql_url(), "https://api.github.com/graphql");
    }

    #[test]
    fn graphql_url_ghe_server() {
        let client = make_test_client("https://ghe.example.com/api/v3");
        assert_eq!(client.graphql_url(), "https://ghe.example.com/api/graphql");
    }

    /// Creates a `GitHubClient` for unit testing (no network).
    fn make_test_client(api_base_url: &str) -> GitHubClient {
        GitHubClient {
            http: reqwest::Client::new(),
            api_base_url: api_base_url.to_owned(),
            owner: "owner".to_owned(),
            repo: "repo".to_owned(),
            project_number: None,
        }
    }
}
