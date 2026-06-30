//! Server options + untrusted-input quotas + the mcp-owned `contract_version` SSOT.
//!
//! - [`ServeOptions`] / [`Quotas`] are the cross-crate config the CLI hands to [`crate::serve`]
//!   (spine §5, mcp plan public-API table).
//! - [`CONTRACT_VERSION`] is the **mcp-owned SSOT** (F-5) for the MCP `contract_version` — disjoint
//!   from policy's `POLICY_CONTRACT_VERSION`. It is surfaced in [`crate::capabilities`] and the
//!   `diagnostics version` finding, and is **bumped on any tool/resource/prompt schema change**
//!   (the FR-12 drift gate, proven by `tests/contract_suite.rs` at T2.3).

use tokio_util::sync::CancellationToken;

/// The mcp-owned contract version (F-5).
///
/// The single source of the MCP `contract_version` (surfaced in [`crate::capabilities`] and the
/// `diagnostics version` finding). **Bump this whenever any tool/resource/prompt input or output
/// schema changes** — the FR-12 drift gate keys on it.
pub const CONTRACT_VERSION: &str = "unblock.mcp.v1";

/// Untrusted-input limits enforced **before** any `Session` call (NFR-18).
///
/// rmcp provides NO built-in request-size / array-length / string-length / batch cap on the stdio
/// path (and there is no stdio middleware layer), so these limits are enforced by a shared
/// `enforce_quota(&args)` preflight inside each tool body — after `Parameters<T>` deserialization and
/// before the engine is touched. A breach is returned in-band as a `ToolOutput::Error` (the blast
/// radius stays confined to the workspace).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quotas {
    /// Maximum serialized request size in bytes (default 256 KiB).
    pub max_request_bytes: usize,
    /// Maximum length of any input array (default `10_000`).
    pub max_array_len: usize,
    /// Maximum length of any input string (default 64 KiB).
    pub max_string_len: usize,
    /// Maximum batch size (default 100). *(v1.3 batch surface; the limit lands now.)*
    pub max_batch: usize,
    /// Maximum number of concurrent in-flight requests (default 64).
    pub max_concurrent_requests: usize,
}

impl Default for Quotas {
    fn default() -> Self {
        Self {
            max_request_bytes: 256 * 1024,
            max_array_len: 10_000,
            max_string_len: 64 * 1024,
            max_batch: 100,
            max_concurrent_requests: 64,
        }
    }
}

/// Options for [`crate::serve`] (spine §5, mcp plan public-API table).
///
/// Carries the optional instruction string surfaced to clients, the request quotas (NFR-18), and the
/// cooperative-shutdown handle (FR-17) — a `cancel()` on this token drains in-flight work and returns
/// cleanly from `serve`.
#[derive(Debug, Clone)]
pub struct ServeOptions {
    /// Optional human-readable instructions advertised to MCP clients.
    pub instructions: Option<String>,
    /// Untrusted-input limits (NFR-18).
    pub quotas: Quotas,
    /// Cooperative-shutdown handle (FR-17). The CLI installs the OS signal handler that cancels it.
    pub cancel: CancellationToken,
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            instructions: None,
            quotas: Quotas::default(),
            cancel: CancellationToken::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CONTRACT_VERSION, Quotas, ServeOptions};

    #[test]
    fn quotas_default_values_are_pinned() {
        let q = Quotas::default();
        assert_eq!(q.max_request_bytes, 256 * 1024);
        assert_eq!(q.max_array_len, 10_000);
        assert_eq!(q.max_string_len, 64 * 1024);
        assert_eq!(q.max_batch, 100);
        assert_eq!(q.max_concurrent_requests, 64);
    }

    #[test]
    fn serve_options_default_has_empty_instructions_and_default_quotas() {
        let opts = ServeOptions::default();
        assert!(opts.instructions.is_none());
        assert_eq!(opts.quotas, Quotas::default());
        assert!(!opts.cancel.is_cancelled());
    }

    #[test]
    fn contract_version_is_the_v1_id() {
        assert_eq!(CONTRACT_VERSION, "unblock.mcp.v1");
    }
}
