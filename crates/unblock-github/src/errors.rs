//! Infrastructure error types for the GitHub API client.
//!
//! Defines `Error` with variants for all infrastructure-level failure modes:
//! network errors, API errors, GraphQL errors, rate limiting, and git remote
//! detection failures.
//!
//! Domain errors from `unblock-core` are wrapped transparently via the
//! `Domain` variant with `#[snafu(context(false))]`.

use snafu::prelude::*;
use unblock_core::errors::DomainError;

/// Infrastructure-level errors for the GitHub API client.
///
/// Each variant represents a specific infrastructure failure mode. Domain errors
/// from `unblock-core` are wrapped transparently via the [`Domain`](Self::Domain)
/// variant.
///
/// Use the generated snafu context selectors (e.g. [`GitHubApiSnafu`]) to
/// construct errors ergonomically.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    /// Wraps a domain-level error from `unblock-core`.
    #[snafu(display("{source}"))]
    #[snafu(context(false))]
    Domain {
        /// The underlying domain error.
        source: DomainError,
    },

    /// A non-2xx response from the GitHub REST API.
    #[snafu(display("GitHub API error: {message}"))]
    GitHubApi {
        /// Human-readable error message from the API response.
        message: String,
    },

    /// One or more errors in a GitHub GraphQL response.
    #[snafu(display("GitHub GraphQL error: {errors}"))]
    GitHubGraphQL {
        /// Concatenated error messages from the GraphQL response.
        errors: String,
    },

    /// Network or connection failure when reaching GitHub.
    #[snafu(display("Cannot connect to GitHub: {source}"))]
    GitHubUnavailable {
        /// The underlying reqwest error.
        source: reqwest::Error,
    },

    /// GitHub returned HTTP 429 — rate limit exceeded.
    #[snafu(display("GitHub rate limit exceeded"))]
    RateLimited,

    /// Circuit breaker is open due to repeated GitHub failures (Phase 2 stub).
    #[snafu(display("Circuit breaker open — GitHub consistently failing"))]
    CircuitBreakerOpen,

    /// No Projects V2 project is configured or discoverable.
    #[snafu(display("Projects V2 not configured — run `setup` first"))]
    ProjectNotConfigured,

    /// Failed to read or parse the git remote URL.
    #[snafu(display("Failed to detect git remote: {message}"))]
    GitRemote {
        /// Description of the git remote detection failure.
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use snafu::IntoError;
    use unblock_core::errors::IssueNotFoundSnafu;

    #[test]
    fn domain_display_delegates_to_source() {
        let domain_err = IssueNotFoundSnafu { number: 42_u64 }.build();
        let expected = domain_err.to_string();
        let err = Error::from(domain_err);
        assert_eq!(err.to_string(), expected);
    }

    #[test]
    fn github_api_display() {
        let err = GitHubApiSnafu {
            message: "Not Found".to_owned(),
        }
        .build();
        assert_eq!(err.to_string(), "GitHub API error: Not Found");
    }

    #[test]
    fn github_graphql_display() {
        let err = GitHubGraphQLSnafu {
            errors: "Field 'x' not found".to_owned(),
        }
        .build();
        assert_eq!(err.to_string(), "GitHub GraphQL error: Field 'x' not found");
    }

    #[tokio::test]
    async fn github_unavailable_display() {
        // Connect to IPv4 loopback port 1 — port 1 is reserved/privileged and
        // reliably refused on all platforms. Uses 127.0.0.1 instead of [::1] to
        // avoid failures on CI environments where IPv6 is disabled.
        let reqwest_err = reqwest::Client::new()
            .get("http://127.0.0.1:1")
            .send()
            .await
            .expect_err("connection to port 1 should be refused");
        let err = GitHubUnavailableSnafu.into_error(reqwest_err);
        let msg = err.to_string();
        assert!(
            msg.starts_with("Cannot connect to GitHub: "),
            "unexpected display: {msg}"
        );
    }

    #[test]
    fn rate_limited_display() {
        let err = RateLimitedSnafu.build();
        assert_eq!(err.to_string(), "GitHub rate limit exceeded");
    }

    #[test]
    fn circuit_breaker_open_display() {
        let err = CircuitBreakerOpenSnafu.build();
        assert_eq!(
            err.to_string(),
            "Circuit breaker open \u{2014} GitHub consistently failing"
        );
    }

    #[test]
    fn project_not_configured_display() {
        let err = ProjectNotConfiguredSnafu.build();
        assert_eq!(
            err.to_string(),
            "Projects V2 not configured \u{2014} run `setup` first"
        );
    }

    #[test]
    fn git_remote_display() {
        let err = GitRemoteSnafu {
            message: "no origin remote".to_owned(),
        }
        .build();
        assert_eq!(
            err.to_string(),
            "Failed to detect git remote: no origin remote"
        );
    }

    #[test]
    fn all_non_network_variants_implement_error_trait() {
        let errors: Vec<Error> = vec![
            Error::from(IssueNotFoundSnafu { number: 1_u64 }.build()),
            GitHubApiSnafu {
                message: "err".to_owned(),
            }
            .build(),
            GitHubGraphQLSnafu {
                errors: "err".to_owned(),
            }
            .build(),
            RateLimitedSnafu.build(),
            CircuitBreakerOpenSnafu.build(),
            ProjectNotConfiguredSnafu.build(),
            GitRemoteSnafu {
                message: "err".to_owned(),
            }
            .build(),
        ];

        for err in &errors {
            let dyn_err: &dyn std::error::Error = err;
            _ = dyn_err;
            assert!(!err.to_string().is_empty());
        }
    }
}
