//! GitHub API client bootstrap.
//!
//! `GitHubClient::new(config)` creates a `reqwest` client with auth headers.
//! Supports repo resolution from env or git remote and project resolution
//! from linked Projects V2. Configurable `api_base_url` for GitHub Enterprise.

use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
use tokio::sync::Mutex;
use tracing::info;
use unblock_core::config::Config;

use crate::errors::{self, Error, GitRemoteSnafu};
use crate::projects::ProjectFieldIds;
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
    /// Cached Projects V2 field IDs, populated by `setup_fields()`.
    ///
    /// Uses a `Mutex` for interior mutability because `setup_fields()` takes
    /// `&self` (not `&mut self`) and needs to cache the result after creating
    /// fields via async GraphQL calls.
    field_ids: Mutex<Option<ProjectFieldIds>>,
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
    #[allow(clippy::unused_async)] // Async signature required by callers; resolve_project_info() is separate.
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
            field_ids: Mutex::new(None),
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

    /// Returns a clone of the cached [`ProjectFieldIds`], if set.
    ///
    /// This acquires the internal mutex briefly. Returns `None` if
    /// `setup_fields()` has not been called yet.
    pub async fn field_ids(&self) -> Option<ProjectFieldIds> {
        self.field_ids.lock().await.clone()
    }

    /// Caches the resolved [`ProjectFieldIds`] on this client.
    ///
    /// Called by `setup_fields()` after successfully resolving or creating
    /// all 7 required fields. Subsequent calls overwrite the previous value.
    pub async fn set_field_ids(&self, ids: ProjectFieldIds) {
        *self.field_ids.lock().await = Some(ids);
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

        parse_github_url(&url, &config.github_url)
    }

    /// Resolves the project number from configuration.
    ///
    /// If `config.project_number` is set (from `UNBLOCK_PROJECT`), it is used
    /// directly. Otherwise, returns `None`. Full auto-detection via the GitHub
    /// Projects V2 API is implemented in a later task (bead unblock-467.6).
    fn resolve_project(config: &Config) -> Option<u64> {
        config.project_number
    }

    /// Creates a `GitHubClient` for unit testing with a custom API base URL.
    ///
    /// Constructs a bare client with no auth headers, pointing at the given
    /// `api_base_url`. The `api_base_url` should be a wiremock server URI so
    /// that `graphql_url()` routes requests to the mock. Available only in
    /// `#[cfg(test)]` builds and accessible from other crate modules.
    #[cfg(test)]
    pub(crate) fn new_for_test(api_base_url: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_base_url: api_base_url.to_owned(),
            owner: "test-owner".to_owned(),
            repo: "test-repo".to_owned(),
            project_number: None,
            field_ids: Mutex::new(None),
        }
    }
}

/// Parses a GitHub URL into `(owner, repo)`.
///
/// The `github_url` parameter specifies the expected GitHub host as a web URL
/// (e.g. `https://github.com` or `https://ghe.corp.com`). The hostname is
/// extracted and used to match HTTPS and SSH remote URL formats.
///
/// Supported formats (where `<host>` is derived from `github_url`):
/// - `https://<host>/owner/repo.git`
/// - `https://<host>/owner/repo`
/// - `http://<host>/owner/repo[.git]`
/// - `git@<host>:owner/repo.git`
/// - `git@<host>:owner/repo`
///
/// # Errors
///
/// Returns [`Error::GitRemote`] if the URL is not a recognized GitHub format
/// or if the `github_url` cannot be parsed to extract a hostname.
pub fn parse_github_url(url: &str, github_url: &str) -> Result<(String, String), Error> {
    let url = url.trim();

    // Extract the hostname from github_url.
    // github_url is expected to be like "https://github.com" or "https://ghe.corp.com".
    let host = extract_host(github_url).ok_or_else(|| {
        GitRemoteSnafu {
            message: format!("cannot extract hostname from GITHUB_URL: {github_url}"),
        }
        .build()
    })?;

    // Try HTTPS format: https://<host>/owner/repo[.git]
    let secure_prefix = format!("https://{host}/");
    let plain_prefix = format!("http://{host}/");
    if let Some(path) = url
        .strip_prefix(&secure_prefix)
        .or_else(|| url.strip_prefix(&plain_prefix))
    {
        return parse_owner_repo_from_path(path, url);
    }

    // Try SSH format: git@<host>:owner/repo[.git]
    let ssh_prefix = format!("git@{host}:");
    if let Some(path) = url.strip_prefix(&ssh_prefix) {
        return parse_owner_repo_from_path(path, url);
    }

    Err(GitRemoteSnafu {
        message: format!("not a GitHub URL: {url}"),
    }
    .build())
}

/// Extracts the hostname from a URL string.
///
/// Handles URLs with or without a scheme. For example:
/// - `https://github.com` -> `github.com`
/// - `https://ghe.corp.com/` -> `ghe.corp.com`
/// - `https://ghe.corp.com:8443` -> `ghe.corp.com:8443`
fn extract_host(url: &str) -> Option<String> {
    // Strip the scheme (e.g. "https://").
    let after_scheme = url.find("://").map_or(url, |i| &url[i + 3..]);

    // Take everything up to the first '/' (the host, possibly with port).
    let host = after_scheme.split('/').next()?;

    if host.is_empty() {
        return None;
    }

    Some(host.to_owned())
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

    const DEFAULT_GH_URL: &str = "https://github.com";

    #[test]
    fn parse_https_with_git_suffix() {
        let (owner, repo) =
            parse_github_url("https://github.com/websublime/unblock.git", DEFAULT_GH_URL).unwrap();
        assert_eq!(owner, "websublime");
        assert_eq!(repo, "unblock");
    }

    #[test]
    fn parse_https_without_git_suffix() {
        let (owner, repo) =
            parse_github_url("https://github.com/websublime/unblock", DEFAULT_GH_URL).unwrap();
        assert_eq!(owner, "websublime");
        assert_eq!(repo, "unblock");
    }

    #[test]
    fn parse_ssh_with_git_suffix() {
        let (owner, repo) =
            parse_github_url("git@github.com:websublime/unblock.git", DEFAULT_GH_URL).unwrap();
        assert_eq!(owner, "websublime");
        assert_eq!(repo, "unblock");
    }

    #[test]
    fn parse_ssh_without_git_suffix() {
        let (owner, repo) =
            parse_github_url("git@github.com:websublime/unblock", DEFAULT_GH_URL).unwrap();
        assert_eq!(owner, "websublime");
        assert_eq!(repo, "unblock");
    }

    #[test]
    fn parse_https_with_trailing_slash() {
        let (owner, repo) =
            parse_github_url("https://github.com/acme/widgets/", DEFAULT_GH_URL).unwrap();
        assert_eq!(owner, "acme");
        assert_eq!(repo, "widgets");
    }

    #[test]
    fn parse_non_github_url_returns_error() {
        let err = parse_github_url("https://gitlab.com/owner/repo", DEFAULT_GH_URL).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("not a GitHub URL"),
            "expected 'not a GitHub URL' in: {msg}"
        );
    }

    #[test]
    fn parse_empty_string_returns_error() {
        assert!(parse_github_url("", DEFAULT_GH_URL).is_err());
    }

    #[test]
    fn parse_garbage_returns_error() {
        assert!(parse_github_url("not-a-url", DEFAULT_GH_URL).is_err());
    }

    #[test]
    fn parse_github_url_with_extra_segments_returns_error() {
        assert!(parse_github_url("https://github.com/owner/repo/pulls", DEFAULT_GH_URL).is_err());
    }

    // ── parse_github_url with GHE host ────────────────────────────────

    #[test]
    fn parse_ghe_https_with_git_suffix() {
        let (owner, repo) = parse_github_url(
            "https://ghe.corp.com/acme/widgets.git",
            "https://ghe.corp.com",
        )
        .unwrap();
        assert_eq!(owner, "acme");
        assert_eq!(repo, "widgets");
    }

    #[test]
    fn parse_ghe_https_without_git_suffix() {
        let (owner, repo) =
            parse_github_url("https://ghe.corp.com/acme/widgets", "https://ghe.corp.com").unwrap();
        assert_eq!(owner, "acme");
        assert_eq!(repo, "widgets");
    }

    #[test]
    fn parse_ghe_ssh() {
        let (owner, repo) =
            parse_github_url("git@ghe.corp.com:acme/widgets.git", "https://ghe.corp.com").unwrap();
        assert_eq!(owner, "acme");
        assert_eq!(repo, "widgets");
    }

    #[test]
    fn parse_ghe_http() {
        let (owner, repo) =
            parse_github_url("http://ghe.corp.com/acme/widgets", "https://ghe.corp.com").unwrap();
        assert_eq!(owner, "acme");
        assert_eq!(repo, "widgets");
    }

    #[test]
    fn parse_ghe_url_mismatch_returns_error() {
        // Remote points to github.com but GITHUB_URL is set to a GHE host.
        let err = parse_github_url("https://github.com/acme/widgets", "https://ghe.corp.com")
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("not a GitHub URL"),
            "expected 'not a GitHub URL' in: {msg}"
        );
    }

    #[test]
    fn parse_ghe_with_port() {
        let (owner, repo) = parse_github_url(
            "https://ghe.corp.com:8443/acme/widgets",
            "https://ghe.corp.com:8443",
        )
        .unwrap();
        assert_eq!(owner, "acme");
        assert_eq!(repo, "widgets");
    }

    // ── extract_host ──────────────────────────────────────────────────

    #[test]
    fn extract_host_from_https_url() {
        assert_eq!(
            extract_host("https://github.com").as_deref(),
            Some("github.com")
        );
    }

    #[test]
    fn extract_host_from_ghe_url() {
        assert_eq!(
            extract_host("https://ghe.corp.com").as_deref(),
            Some("ghe.corp.com")
        );
    }

    #[test]
    fn extract_host_with_trailing_slash() {
        assert_eq!(
            extract_host("https://ghe.corp.com/").as_deref(),
            Some("ghe.corp.com")
        );
    }

    #[test]
    fn extract_host_with_port() {
        assert_eq!(
            extract_host("https://ghe.corp.com:8443").as_deref(),
            Some("ghe.corp.com:8443")
        );
    }

    #[test]
    fn extract_host_empty_returns_none() {
        assert_eq!(extract_host("https://"), None);
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
            github_url: "https://github.com".to_owned(),
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
            github_url: "https://github.com".to_owned(),
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
            github_url: "https://github.com".to_owned(),
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
            field_ids: Mutex::new(None),
        }
    }
}
