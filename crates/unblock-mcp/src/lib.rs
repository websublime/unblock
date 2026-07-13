//! `unblock-mcp` (L7) — the PRIMARY product surface: an `rmcp` stdio MCP server (`unblock mcp`)
//! exposing the engine as the consolidated 7-tool taxonomy + resources + prompts, schemars-validated
//! under quotas (NFR-18), discoverable via a `contract_version`-stamped bundle (FR-12), every error
//! mapped to the structured boundary (FR-11). A thin adapter over `Session` (no write orchestration).
//! See `docs/plans/crates/unblock-mcp.md`.
//!
//! # Public surface (the cross-crate contract — consumed by `unblock-cli`'s `mcp` command, D3/§0.1)
//!
//! - [`run_mcp_server`] — build the rmcp server, bind the stdio transport, run until cancellation (FR-17).
//! - [`McpServerOptions`] / [`Quotas`] — server config + untrusted-input limits (NFR-18).
//! - [`CONTRACT_VERSION`] / [`CONTRACT_HASH`] — the mcp-owned `contract_version` SSOT (F-5; bumped
//!   when EITHER discovery document changes) + the ONE hash-coupled drift pin over the two-document
//!   tuple `(capabilities(), schema_bundle())` (D22 widened by D25).
//! - [`capabilities`] / [`schema_bundle`] — pure builders (no `Session`) so the CLI can dump the
//!   contract offline (FR-12).
//! - [`agents_digest`] — a pure typed digest of the two discovery documents (D33), consumed by the
//!   CLI's `unblock agents` managed AGENTS.md renderer; a DERIVED VIEW, never hash-coupled.
//! - [`McpServerError`] — server lifecycle/transport errors only; per-tool domain errors flow in-band
//!   as the shared structured error (`is_error=true`, FR-11).
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
pub use options::{CONTRACT_HASH, CONTRACT_VERSION, McpServerOptions, Quotas};
pub use resources::{
    AgentsDigest, Capabilities, ErrorCodeDescriptor, ErrorCodeDigest, PromptDescriptor,
    PromptDigest, ResourceDescriptor, ResourceDigest, SchemaBundle, ToolAction, ToolDescriptor,
    ToolDigest, ToolSchemas, agents_digest, capabilities, schema_bundle,
};
pub use server::run_mcp_server;

// Test-only seam (feature-gated, `#[doc(hidden)]`): the in-process MCP lifecycle test drives the
// real server over an in-memory duplex transport via these. NEVER part of the shipped contract.
// `mcp_server_duplex_unclamped_for_test` is the CD-6 assumption-pin variant that OMITS the
// `VersionClampingTransport` so `tests/protocol_version.rs` can pin rmcp's raw serve-loop negotiation.
#[cfg(feature = "test-util")]
#[doc(hidden)]
pub use server::{UnblockServer, mcp_server_duplex_for_test, mcp_server_duplex_unclamped_for_test};
