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
    #[snafu(display("GitHub GraphQL error: {}", errors.join("; ")))]
    GitHubGraphQL {
        /// Individual error messages from the GraphQL response.
        errors: Vec<String>,
    },

    /// A GitHub GraphQL response containing at least one error with
    /// `type == "FORBIDDEN"`.
    ///
    /// The GraphQL spec (and GitHub's implementation) returns HTTP 200
    /// with a typed `errors` array for permission-denied cases — the
    /// `type` field is the wire-safe signal rather than the free-text
    /// `message`. The GraphQL-level reducer in `graphql_with_features`
    /// on [`crate::client::GitHubClient`] inspects `type` BEFORE
    /// reducing to messages (per SPEC §11.1 wiring, user decision
    /// 2026-04-17) and emits this variant when any error carries
    /// `FORBIDDEN`. Callers that know a cross-repo `owner/repo` context
    /// (e.g. `fetch_issue_in_repo`, `resolve_issue_ref` cross-repo arm)
    /// upgrade this variant to [`DomainError::CrossRepoAccessDenied`].
    ///
    /// [`DomainError::CrossRepoAccessDenied`]:
    ///     unblock_core::errors::DomainError::CrossRepoAccessDenied
    #[snafu(display("GitHub GraphQL FORBIDDEN: {}", errors.join("; ")))]
    GitHubGraphQLForbidden {
        /// Messages from the GraphQL errors whose `type` was
        /// `FORBIDDEN`. Non-FORBIDDEN messages from the same response
        /// are discarded; the classifier treats any FORBIDDEN as
        /// authoritative.
        errors: Vec<String>,
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

    /// GitHub returned an account type that is not recognized as either
    /// `Organization` or `User`.
    ///
    /// This guards against silent misrouting of REST calls to `/users/{owner}`
    /// when GitHub introduces new account types (e.g., `Bot`, `Enterprise`).
    #[snafu(display(
        "Unknown GitHub account type '{account_type}' for owner '{owner}' — expected 'Organization' or 'User'"
    ))]
    UnknownOwnerType {
        /// The GitHub login of the owner being queried.
        owner: String,
        /// The `type` field returned by the GitHub REST API.
        account_type: String,
    },

    /// A `MockGitHubClient` method was called without a queued stub response.
    ///
    /// Only constructible when the `test-hooks` feature is enabled. Tests
    /// should pre-load the relevant stub queue (e.g. via
    /// `MockGitHubClient::push_resolve_project_info`) before exercising the
    /// code under test, otherwise the mock returns this variant to surface
    /// the missing setup loudly instead of silently producing a default.
    #[cfg(feature = "test-hooks")]
    #[snafu(display("MockGitHubClient: `{method}` was called without a queued stub response"))]
    MockNotStubbed {
        /// The trait method that was invoked without a stub.
        method: &'static str,
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
            // 403 Forbidden — GraphQL FORBIDDEN-typed error before
            // cross-repo classification upgrades it to DomainError
            // CrossRepoAccessDenied. An un-upgraded variant still
            // reaches the MCP layer with the right status-code bucket
            // so `github_error_to_mcp` maps it to INVALID_PARAMS.
            Self::GitHubGraphQLForbidden { .. } => 403,
            Self::GitHubUnavailable { .. } | Self::CircuitBreakerOpen { .. } => 503,
            Self::RateLimited { .. } => 429,
            Self::ProjectNotConfigured => 412,
            Self::GitRemote { .. } => 500,
            // 502 Bad Gateway: the upstream (GitHub) returned an unexpected account
            // type, so our server cannot produce a valid response.  This distinguishes
            // "GitHub gave us something we don't understand" (502) from "our own logic
            // broke" (500).  The MCP layer maps all github errors to INTERNAL_ERROR
            // regardless, so the distinction is informational for logs/diagnostics only.
            Self::UnknownOwnerType { .. } => 502,
            #[cfg(feature = "test-hooks")]
            Self::MockNotStubbed { .. } => 500,
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
            errors: vec!["Field 'x' not found".to_owned()],
        }
        .build();
        assert_eq!(err.to_string(), "GitHub GraphQL error: Field 'x' not found");
    }

    #[test]
    fn github_graphql_display_multi_error() {
        let err = GitHubGraphQLSnafu {
            errors: vec![
                "Field 'x' not found".to_owned(),
                "Variable '$id' is not defined".to_owned(),
            ],
        }
        .build();
        assert_eq!(
            err.to_string(),
            "GitHub GraphQL error: Field 'x' not found; Variable '$id' is not defined"
        );
    }

    #[test]
    fn github_graphql_display_empty_vec() {
        let err = GitHubGraphQLSnafu {
            errors: Vec::<String>::new(),
        }
        .build();
        assert_eq!(err.to_string(), "GitHub GraphQL error: ");
    }

    #[test]
    fn github_graphql_forbidden_display_and_status() {
        // The `GitHubGraphQLForbidden` variant is emitted by the
        // GraphQL reducer when any errors[i].type == "FORBIDDEN" (per
        // SPEC §11.1 wiring, user decision 2026-04-17). Display embeds
        // the FORBIDDEN messages; status_code is 403 so the un-upgraded
        // variant still reaches `github_error_to_mcp` with the right
        // bucket (INVALID_PARAMS).
        let err = GitHubGraphQLForbiddenSnafu {
            errors: vec!["Resource not accessible by integration".to_owned()],
        }
        .build();
        let msg = err.to_string();
        assert!(
            msg.starts_with("GitHub GraphQL FORBIDDEN: "),
            "unexpected display: {msg}"
        );
        assert!(
            msg.contains("Resource not accessible by integration"),
            "display must include FORBIDDEN messages: {msg}"
        );
        assert_eq!(err.status_code(), 403);
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
                errors: vec!["err".to_owned()],
            }
            .build(),
            GitHubGraphQLForbiddenSnafu {
                errors: vec!["Resource not accessible by integration".to_owned()],
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
            errors: vec!["bad field".to_owned()],
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
