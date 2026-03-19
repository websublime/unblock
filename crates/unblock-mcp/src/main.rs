//! # unblock-mcp
//!
//! MCP server binary for dependency-aware task tracking powered by GitHub.
//!
//! Connects to GitHub via `unblock-github`, builds a dependency graph via `unblock-core`,
//! and exposes 17 MCP tools over stdio transport.

/// MCP server bootstrap, state, and tool registration.
mod server;

/// MCP error types and conversion from domain/infrastructure errors.
mod errors;

/// MCP tool handlers.
mod tools;

fn main() {
    // Server bootstrap will be implemented in Phase 1.
    // Config → tracing → GitHubClient → GraphCache → ServerState → rmcp stdio.
}
