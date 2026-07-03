//! Server options + untrusted-input quotas + the mcp-owned `contract_version` SSOT.
//!
//! - [`ServeOptions`] / [`Quotas`] are the cross-crate config the CLI hands to [`crate::serve`]
//!   (spine §5, mcp plan public-API table).
//! - [`CONTRACT_VERSION`] is the **mcp-owned SSOT** (F-5) for the MCP `contract_version` — disjoint
//!   from policy's `POLICY_CONTRACT_VERSION`. It is surfaced in [`crate::capabilities`] and the
//!   `diagnostics version` finding, and is **bumped whenever EITHER discovery document changes** —
//!   the FR-12 drift gate (D22 widened by D25), proven by `tests/contract_suite.rs`.

use tokio_util::sync::CancellationToken;

/// The mcp-owned contract version (F-5).
///
/// The single source of the MCP `contract_version` (surfaced in [`crate::capabilities`] and the
/// `diagnostics version` finding). **Bump this whenever EITHER discovery document changes** — any tool
/// input OR output schema, the shared error schema, a tool/resource/prompt descriptor copy, or the
/// error-code map (incl. hint shapes). The FR-12 drift gate keys on it, hash-coupled to
/// [`CONTRACT_HASH`] (D22 widened by D25).
///
/// History: `unblock.mcp.v1` (T2.2) → `unblock.mcp.v1.1` (T2.3 / D22 — the `issue` schema gained the
/// `create_bulk` action arm + the 4 `Create` fields `design`/`acceptance_criteria`/`assignee`/
/// `agent_context`) → `unblock.mcp.v1.2` (T2.6/D25 — `SchemaBundle` recomposed to per-tool
/// `{input,output}` + the shared error schema; `capabilities()` enters the gate; `ErrorCodeDescriptor`
/// gained `hint_shape`; the pin renamed `SCHEMA_BUNDLE_HASH` → `CONTRACT_HASH`). The
/// `unblock.mcp.vN[.M]` family preserves the contract-id convention while the `.M` revision marks an
/// additive contract change within the v1 product.
pub const CONTRACT_VERSION: &str = "unblock.mcp.v1.2";

/// The pinned SHA-256 digest of the ordered two-document tuple `(capabilities(), schema_bundle())` —
/// the HASH-COUPLED half of the FR-12 drift gate (D22 widened by D25, `tests/contract_suite.rs`).
///
/// Moves IN LOCKSTEP with [`CONTRACT_VERSION`]; non-vacuous both directions (both documents embed
/// `contract_version`). **Re-pin this whenever EITHER discovery document changes:** any tool
/// input/output schema, the shared error schema, a tool/resource/prompt descriptor copy, or the
/// error-code map (incl. hint shapes) — alongside the `CONTRACT_VERSION` bump + the re-blessed
/// `capabilities`/`schema_bundle` goldens. Prompt rendered MESSAGES are golden-only (insta), never
/// version-coupled.
///
/// Determinism failure mode: this digest's stability relies on `serde_json`'s DEFAULT `BTreeMap` map
/// representation — any future dep enabling `serde_json/preserve_order` (feature unification, dev-deps
/// included) reorders the schemars-generated maps and moves `CONTRACT_HASH` with NO contract change;
/// if the gate fires with "nothing changed", check `Cargo.lock` feature unification first.
pub const CONTRACT_HASH: &str = "d3cbd6c08b0e02a47f9565d3c281e9ccf1d2109367dca3acf1b826de18a4dc70";

/// Untrusted-input limits enforced **before** any `Session` call (NFR-18).
///
/// rmcp provides NO built-in request-size / array-length / string-length / batch cap on the stdio
/// path (and there is no stdio middleware layer), so these limits are enforced by a shared
/// `enforce_quota(&args)` preflight inside each tool body — after `Parameters<T>` deserialization and
/// before the engine is touched. A breach is returned in-band as the shared structured error
/// (`is_error=true`, the `SchemaBundle.error` shape) — the blast radius stays confined to the workspace.
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
    fn contract_version_is_the_bumped_v1_id() {
        // T2.6/D25 bumped the contract surface (per-tool outputs + capabilities enter the gate) → v1.2.
        assert_eq!(CONTRACT_VERSION, "unblock.mcp.v1.2");
    }

    #[test]
    fn contract_hash_is_a_64_char_hex() {
        use super::CONTRACT_HASH;
        assert_eq!(CONTRACT_HASH.len(), 64, "a SHA-256 hex digest");
        assert!(CONTRACT_HASH.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
