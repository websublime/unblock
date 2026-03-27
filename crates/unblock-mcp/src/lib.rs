//! # unblock-mcp
//!
//! MCP server library for dependency-aware task tracking powered by GitHub.
//!
//! This crate exposes the server types and tool definitions as a library
//! for integration testing. The binary entry point is in `main.rs`.

/// MCP server bootstrap, state, and tool registration.
pub mod server;

/// MCP error types and conversion from domain/infrastructure errors.
pub mod errors;

/// MCP tool handlers.
pub mod tools;
