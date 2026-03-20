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
