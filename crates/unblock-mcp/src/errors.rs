//! MCP error types and conversion.
//!
//! Maps domain errors (`unblock-core`) and infrastructure errors (`unblock-github`)
//! to MCP error responses with appropriate error codes.

use snafu::Snafu;

/// Errors that can occur during MCP server bootstrap.
///
/// Each variant wraps the underlying source error and carries a human-readable
/// message describing what went wrong, so that the operator can diagnose
/// startup failures without reading source code.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
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
        source: unblock_github::errors::Error,
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
