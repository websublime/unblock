//! MCP error types and conversion.
//!
//! Maps domain errors (`unblock-core`) and infrastructure errors (`unblock-github`)
//! to MCP error responses with appropriate error codes.
//!
//! `github_error_to_mcp` provides the bridge between infrastructure errors and
//! JSON-RPC error responses, mapping HTTP status codes to appropriate MCP error codes.
//!
//! A `From` impl is not possible here due to the Rust orphan rule — neither
//! `ErrorData` (from `rmcp`) nor `Error` (from `unblock-github`) is defined in
//! this crate.

use rmcp::model::{ErrorCode, ErrorData};
use snafu::Snafu;

/// Errors that can occur during MCP server bootstrap.
///
/// Each variant wraps the underlying source error and carries a human-readable
/// message describing what went wrong, so that the operator can diagnose
/// startup failures without reading source code.
#[derive(Debug, Snafu)]
// `visibility(pub)` is required (not `pub(crate)`) because `unblock-mcp` has
// both a `[lib]` and a `[[bin]]` target. `src/main.rs` is a separate crate
// from the visibility standpoint and consumes the snafu-generated selectors
// (`ConfigLoadSnafu`, `ClientInitSnafu`, `TransportSnafu`, `RuntimeSnafu`)
// and `BootstrapError` itself across the lib/bin boundary via the
// `unblock_mcp::errors` path. Narrowing to `pub(crate)` would hide these
// items from `main.rs` and break the binary build.
//
// Practical leak risk is zero: `unblock-mcp` is a binary crate with no
// external downstream consumers — no other workspace crate depends on it
// and it is not published to crates.io. See bead unblock-b6b.65 for the
// full investigation and rationale.
#[snafu(visibility(pub))]
pub enum BootstrapError {
    /// Failed to load configuration from environment variables.
    #[snafu(display(
        "Failed to load configuration. Ensure GITHUB_TOKEN is set in the environment."
    ))]
    ConfigLoad {
        /// The underlying domain error from `Config::load`.
        source: unblock_core::errors::DomainError,
    },

    /// Failed to initialize the GitHub API client.
    #[snafu(display(
        "Failed to initialize GitHub client. Check GITHUB_TOKEN and repository settings."
    ))]
    ClientInit {
        /// The underlying GitHub client error.
        ///
        /// Boxed to keep `BootstrapError` compact. `unblock_github::errors::Error`
        /// transitively wraps `DomainError`, which now carries `IssueRef` fields
        /// (SPEC §11.1 Decision 1, 2026-04-17). Without boxing, `Result<(), BootstrapError>`
        /// at `main()` exceeds the `clippy::result_large_err` threshold. This mirrors
        /// the pattern already used by the `Transport` variant for
        /// `rmcp::service::ServerInitializeError`.
        #[snafu(source(from(unblock_github::errors::Error, Box::new)))]
        source: Box<unblock_github::errors::Error>,
    },

    /// Failed to start the MCP stdio transport.
    #[snafu(display("Failed to start MCP stdio transport"))]
    Transport {
        /// The underlying rmcp initialization error.
        #[snafu(source(from(rmcp::service::ServerInitializeError, Box::new)))]
        source: Box<rmcp::service::ServerInitializeError>,
    },

    /// The MCP runtime task panicked or was cancelled.
    #[snafu(display("MCP runtime task failed"))]
    Runtime {
        /// The underlying tokio `JoinError`.
        source: tokio::task::JoinError,
    },
}

/// Maps a GitHub infrastructure error to an MCP JSON-RPC error response.
///
/// Converts the HTTP status code from the error into the most appropriate
/// JSON-RPC error code:
///
/// | HTTP status                   | JSON-RPC code             |
/// |-------------------------------|---------------------------|
/// | 400, 403, 404, 409, 412, 422  | `INVALID_PARAMS` (-32602) |
/// | 429, 500, 503                 | `INTERNAL_ERROR` (-32603) |
/// | other                         | `INTERNAL_ERROR` (-32603) |
///
/// The 403 branch is an explicit match arm (not a catch-all fall-through)
/// per SPEC §11.3 / plan Task 02.02 "Error-side wiring" and plan
/// GAP-14.c Decision 2 #3 — this makes the `CrossRepoAccessDenied`
/// mapping a named branch so regressions are caught by the error-mapping
/// tests rather than silently collapsing into `INTERNAL_ERROR` when
/// future variants alter the status-code table.
///
/// The error's `Display` output becomes the JSON-RPC error message.
#[allow(clippy::needless_pass_by_value)] // Intentional: consumes the error in a map_err chain.
pub(crate) fn github_error_to_mcp(err: unblock_github::errors::Error) -> ErrorData {
    let code = match err.status_code() {
        400 | 403 | 404 | 409 | 412 | 422 => ErrorCode::INVALID_PARAMS,
        _ => ErrorCode::INTERNAL_ERROR,
    };
    ErrorData {
        code,
        message: err.to_string().into(),
        data: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use snafu::IntoError;
    use unblock_core::errors::{
        CrossRepoAccessDeniedSnafu, InvalidIssueRefSnafu, IssueNotFoundSnafu,
    };
    use unblock_github::errors::{
        CircuitBreakerOpenSnafu, GitHubApiSnafu, GitHubGraphQLSnafu, GitRemoteSnafu,
        ProjectNotConfiguredSnafu, RateLimitedSnafu,
    };

    /// Helper: convert a `unblock_github::errors::Error` to `ErrorData` and return (code, message).
    fn convert(err: unblock_github::errors::Error) -> (ErrorCode, String) {
        let ed = github_error_to_mcp(err);
        (ed.code, ed.message.into_owned())
    }

    #[test]
    fn domain_error_maps_to_invalid_params() {
        // Domain errors (e.g., IssueNotFound) have status_code 404.
        let err =
            unblock_github::errors::Error::from(IssueNotFoundSnafu { number: 42_u64 }.build());
        let (code, msg) = convert(err);
        assert_eq!(code, ErrorCode::INVALID_PARAMS);
        assert!(
            msg.contains("42"),
            "message should contain issue number: {msg}"
        );
    }

    #[test]
    fn invalid_issue_ref_maps_to_invalid_params() {
        // `InvalidIssueRef` has status_code 400; the 400 branch of
        // `github_error_to_mcp` must route it to `INVALID_PARAMS` and
        // preserve the raw input in the message so the agent can see
        // what they sent. Mirrors `domain_error_maps_to_invalid_params`
        // for the new 400 wiring added in unblock-6xj.
        let err = unblock_github::errors::Error::from(
            InvalidIssueRefSnafu {
                input: "not-a-ref".to_owned(),
            }
            .build(),
        );
        let (code, msg) = convert(err);
        assert_eq!(code, ErrorCode::INVALID_PARAMS);
        assert!(
            msg.contains("not-a-ref"),
            "message should contain raw input: {msg}"
        );
    }

    #[test]
    fn cross_repo_access_denied_maps_to_invalid_params() {
        // `CrossRepoAccessDenied` has status_code 403; the 403 branch of
        // `github_error_to_mcp` MUST route it to `INVALID_PARAMS` via an
        // explicit match arm (SPEC §11.3 / plan Task 02.02 "Error-side
        // wiring" / GAP-14.c Decision 2 #3). This test is the
        // regression guard: without the explicit 403 arm, 403 would
        // silently collapse into the catch-all `INTERNAL_ERROR`.
        let err = unblock_github::errors::Error::from(
            CrossRepoAccessDeniedSnafu {
                owner: "acme".to_owned(),
                repo: "widgets".to_owned(),
            }
            .build(),
        );
        let (code, msg) = convert(err);
        assert_eq!(code, ErrorCode::INVALID_PARAMS);
        assert!(msg.contains("acme"), "message should contain owner: {msg}");
        assert!(
            msg.contains("widgets"),
            "message should contain repo: {msg}"
        );
    }

    #[test]
    fn github_api_404_maps_to_invalid_params() {
        let err = GitHubApiSnafu {
            status: 404_u16,
            message: "Not Found".to_owned(),
        }
        .build();
        let (code, msg) = convert(err);
        assert_eq!(code, ErrorCode::INVALID_PARAMS);
        assert!(msg.contains("404"), "message should contain status: {msg}");
    }

    #[test]
    fn github_api_400_maps_to_invalid_params() {
        let err = GitHubApiSnafu {
            status: 400_u16,
            message: "Bad Request".to_owned(),
        }
        .build();
        let (code, _) = convert(err);
        assert_eq!(code, ErrorCode::INVALID_PARAMS);
    }

    #[test]
    fn github_api_409_maps_to_invalid_params() {
        let err = GitHubApiSnafu {
            status: 409_u16,
            message: "Conflict".to_owned(),
        }
        .build();
        let (code, _) = convert(err);
        assert_eq!(code, ErrorCode::INVALID_PARAMS);
    }

    #[test]
    fn github_graphql_maps_to_invalid_params() {
        // GraphQL errors have status_code 422.
        let err = GitHubGraphQLSnafu {
            errors: vec!["Field 'x' not found".to_owned()],
        }
        .build();
        let (code, msg) = convert(err);
        assert_eq!(code, ErrorCode::INVALID_PARAMS);
        assert!(
            msg.contains("Field 'x' not found"),
            "message should contain GraphQL error: {msg}"
        );
    }

    #[test]
    fn project_not_configured_maps_to_invalid_params() {
        // ProjectNotConfigured has status_code 412.
        let err = ProjectNotConfiguredSnafu.build();
        let (code, msg) = convert(err);
        assert_eq!(code, ErrorCode::INVALID_PARAMS);
        assert!(msg.contains("setup"), "message should mention setup: {msg}");
    }

    #[test]
    fn rate_limited_maps_to_internal_error() {
        // RateLimited has status_code 429.
        let err = RateLimitedSnafu {
            reset_at: Utc::now(),
        }
        .build();
        let (code, msg) = convert(err);
        assert_eq!(code, ErrorCode::INTERNAL_ERROR);
        assert!(
            msg.contains("rate limit"),
            "message should mention rate limit: {msg}"
        );
    }

    #[test]
    fn circuit_breaker_maps_to_internal_error() {
        // CircuitBreakerOpen has status_code 503.
        let err = CircuitBreakerOpenSnafu {
            since: std::time::Instant::now(),
        }
        .build();
        let (code, _) = convert(err);
        assert_eq!(code, ErrorCode::INTERNAL_ERROR);
    }

    #[tokio::test]
    async fn github_unavailable_maps_to_internal_error() {
        // GitHubUnavailable has status_code 503. Need a real reqwest error.
        let reqwest_err = reqwest::Client::new()
            .get("http://127.0.0.1:1")
            .send()
            .await
            .expect_err("connection to port 1 should be refused");
        let err = unblock_github::errors::GitHubUnavailableSnafu.into_error(reqwest_err);
        let (code, msg) = convert(err);
        assert_eq!(code, ErrorCode::INTERNAL_ERROR);
        assert!(
            msg.contains("Cannot connect"),
            "message should describe connectivity issue: {msg}"
        );
    }

    #[test]
    fn git_remote_maps_to_internal_error() {
        // GitRemote has status_code 500.
        let err = GitRemoteSnafu {
            message: "no origin remote".to_owned(),
        }
        .build();
        let (code, msg) = convert(err);
        assert_eq!(code, ErrorCode::INTERNAL_ERROR);
        assert!(
            msg.contains("no origin remote"),
            "message should contain detail: {msg}"
        );
    }

    #[test]
    fn error_data_has_no_extra_data_field() {
        let err = ProjectNotConfiguredSnafu.build();
        let ed = github_error_to_mcp(err);
        assert!(ed.data.is_none(), "data field should be None");
    }
}
