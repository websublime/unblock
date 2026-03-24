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

use chrono::{DateTime, Utc};

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
    #[snafu(display("GitHub API error ({status}): {message}"))]
    GitHubApi {
        /// HTTP status code from the API response.
        status: u16,
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
    #[snafu(display("GitHub rate limit exceeded — resets at {reset_at}"))]
    RateLimited {
        /// When the rate limit resets (from the `X-RateLimit-Reset` header).
        reset_at: DateTime<Utc>,
    },

    /// Circuit breaker is open due to repeated GitHub failures (Phase 2 stub).
    #[snafu(display("Circuit breaker open — GitHub consistently failing (tripped at {since:?})"))]
    CircuitBreakerOpen {
        /// The instant the circuit breaker was tripped.
        since: std::time::Instant,
    },

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

impl Error {
    /// Returns the HTTP status code associated with this error variant.
    ///
    /// Used by the MCP error conversion layer to map infrastructure errors to
    /// protocol-level error codes without coupling to the variant list.
    #[must_use]
    pub fn status_code(&self) -> u16 {
        match self {
            Self::Domain { source } => source.status_code(),
            Self::GitHubApi { status, .. } => *status,
            Self::GitHubGraphQL { .. } => 422,
            Self::GitHubUnavailable { .. } | Self::CircuitBreakerOpen { .. } => 503,
            Self::RateLimited { .. } => 429,
            Self::ProjectNotConfigured => 412,
            Self::GitRemote { .. } => 500,
        }
    }
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
            status: 404_u16,
            message: "Not Found".to_owned(),
        }
        .build();
        assert_eq!(err.to_string(), "GitHub API error (404): Not Found");
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
        assert_eq!(err.status_code(), 503);
    }

    #[test]
    fn rate_limited_display() {
        let reset_at = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .expect("valid timestamp")
            .with_timezone(&Utc);
        let err = RateLimitedSnafu { reset_at }.build();
        let msg = err.to_string();
        assert!(
            msg.contains("rate limit exceeded"),
            "unexpected display: {msg}"
        );
        assert!(
            msg.contains("2026-01-01"),
            "display must include reset_at: {msg}"
        );
    }

    #[test]
    fn circuit_breaker_open_display() {
        let since = std::time::Instant::now();
        let err = CircuitBreakerOpenSnafu { since }.build();
        let msg = err.to_string();
        assert!(
            msg.starts_with("Circuit breaker open"),
            "unexpected display: {msg}"
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
                status: 500_u16,
                message: "err".to_owned(),
            }
            .build(),
            GitHubGraphQLSnafu {
                errors: "err".to_owned(),
            }
            .build(),
            RateLimitedSnafu {
                reset_at: Utc::now(),
            }
            .build(),
            CircuitBreakerOpenSnafu {
                since: std::time::Instant::now(),
            }
            .build(),
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

    #[test]
    fn status_code_domain_delegates() {
        let err = Error::from(IssueNotFoundSnafu { number: 7_u64 }.build());
        assert_eq!(err.status_code(), 404);
    }

    #[test]
    fn status_code_github_api() {
        let err = GitHubApiSnafu {
            status: 403_u16,
            message: "Forbidden".to_owned(),
        }
        .build();
        assert_eq!(err.status_code(), 403);
    }

    #[test]
    fn status_code_github_graphql() {
        let err = GitHubGraphQLSnafu {
            errors: "bad field".to_owned(),
        }
        .build();
        assert_eq!(err.status_code(), 422);
    }

    #[test]
    fn status_code_rate_limited() {
        let err = RateLimitedSnafu {
            reset_at: Utc::now(),
        }
        .build();
        assert_eq!(err.status_code(), 429);
    }

    #[test]
    fn status_code_circuit_breaker_open() {
        let err = CircuitBreakerOpenSnafu {
            since: std::time::Instant::now(),
        }
        .build();
        assert_eq!(err.status_code(), 503);
    }

    #[test]
    fn status_code_project_not_configured() {
        let err = ProjectNotConfiguredSnafu.build();
        assert_eq!(err.status_code(), 412);
    }

    #[test]
    fn status_code_git_remote() {
        let err = GitRemoteSnafu {
            message: "no origin".to_owned(),
        }
        .build();
        assert_eq!(err.status_code(), 500);
    }
}
