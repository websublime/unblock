//! `unblock-mcp` (L7) — the PRIMARY product surface: an `rmcp` stdio MCP server (`unblock serve`)
//! exposing the engine as the consolidated 7-tool taxonomy + resources + prompts, schemars-validated
//! under quotas (NFR-18), discoverable via a `contract_version`-stamped bundle (FR-12), every error
//! mapped to the structured boundary (FR-11). A thin adapter over `Session` (no write orchestration).
//! See `docs/plans/crates/unblock-mcp.md`.
//!
//! # Public surface (the cross-crate contract — consumed by `unblock-cli`'s `serve` command, D3/§0.1)
//!
//! - [`serve`] — build the rmcp server, bind the stdio transport, run until cancellation (FR-17).
//! - [`ServeOptions`] / [`Quotas`] — server config + untrusted-input limits (NFR-18).
//! - [`CONTRACT_VERSION`] — the mcp-owned `contract_version` SSOT (F-5; bumped on any schema change).
//! - [`capabilities`] / [`schema_bundle`] — pure builders (no `Session`) so the CLI can dump the
//!   contract offline (FR-12).
//! - [`McpServerError`] — server lifecycle/transport errors only; per-tool domain errors flow in-band
//!   as `ToolOutput::Error` (FR-11).
//!
//! Everything else (tool routers, input/output DTOs, resource/prompt handlers, the error mapper) is
//! `pub(crate)` — not part of the cross-crate contract.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod error;
mod options;
mod prompts;
mod resources;
mod server;
mod tools;

pub use error::McpServerError;
pub use options::{CONTRACT_VERSION, Quotas, ServeOptions};
pub use resources::{
    Capabilities, ErrorCodeDescriptor, PromptDescriptor, ResourceDescriptor, SchemaBundle,
    ToolDescriptor, capabilities, schema_bundle,
};
pub use server::serve;
