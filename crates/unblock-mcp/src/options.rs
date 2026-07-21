//! Server options + untrusted-input quotas + the mcp-owned `contract_version` SSOT.
//!
//! - [`McpServerOptions`] / [`Quotas`] are the cross-crate config the CLI hands to [`crate::run_mcp_server`]
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
/// gained `hint_shape`; the pin renamed `SCHEMA_BUNDLE_HASH` → `CONTRACT_HASH`) → `unblock.mcp.v1.3`
/// (MCP-conformance drifts CD-1/CD-2, spine §5.2a/§5.3 — the six tagged-enum inputs inject a root
/// `"type": "object"` and the `query`/`dep`/`issue` list arms are object-wrapped; both change
/// schema-bundle bytes, so they ship JOINTLY as one bump) → `unblock.mcp.v1.4` (T3.5/D34 — the
/// `ErrorCode::RateLimited` mint for the NFR-18 rate-limit chokepoint; the taxonomy grows 35→36, so
/// `capabilities().error_codes` gains a descriptor AND `schema_bundle()`'s shared `ErrorCode` `oneOf`
/// gains a `const`, moving both discovery documents' bytes) → `unblock.mcp.v1.5` (T3.9/D37 — the
/// comment surface pulled forward into v1. The version-coupled set moves on TWO axes: the NEW 8th
/// `comment` tool adds a per-tool `{input,output}` pair to `schema_bundle()` + an 8th
/// `capabilities()` descriptor, AND the `$defs/Comment` embedded inside the EXISTING `issue`/`query`
/// OUTPUT schemas gains 2 properties (`updated_at`/`redacted_at`) — so existing schema bytes move
/// too, not merely a new pair). The `unblock.mcp.vN[.M]` family preserves the contract-id convention
/// while the `.M` revision marks an additive contract change within the v1 product.
/// → `unblock.mcp.v1.6` (v1.0.1/D42 — the MCP argument-boundary defect class. Every input container
/// gains `#[serde(deny_unknown_fields)]`, so schemars emits `additionalProperties: false` per
/// `oneOf` arm and the tagged-enum newtype arm is inlined rather than `$ref`-ed; the `create_bulk`
/// doc-comment and the `markdown` field description are rewritten to publish the three new
/// rejections and the closed section-name set. All of that moves `schema_bundle()` bytes. Per D35 an
/// additive `.M` bump inside 1.x is NON-breaking, so this ships in the v1.0.1 patch — it is not a
/// 2.0.0 event).
pub const CONTRACT_VERSION: &str = "unblock.mcp.v1.6";

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
pub const CONTRACT_HASH: &str = "295ab0298f65073c9e27676ffd18691f9b80bbe4f86569b1397601bf57fa1ccf";

/// Untrusted-input limits enforced **before** any `Session` call (NFR-18).
///
/// rmcp provides NO built-in request-size / array-length / string-length / batch cap on the stdio
/// path (and there is no stdio middleware layer), so these limits are enforced by a single
/// `enforce_quota` call in `ServerHandler::call_tool`, inside the rate-limit permit and **before**
/// dispatch (D42). A breach is returned in-band as the shared structured error (`is_error=true`, the
/// `SchemaBundle.error` shape) — the blast radius stays confined to the workspace.
///
/// **"Per-request bytes" (NFR-18) is DEFINED as the serialized `tools/call` `params` object** —
/// `name` + `arguments` + `_meta` + `task` — excluding the JSON-RPC envelope (`jsonrpc` / `id` /
/// `method`), which is not reachable from `ServerHandler::call_tool`. Before D42 the check ran per
/// tool body over the **re-serialized typed input**, so anything the client parked under an unknown
/// key was never measured at all.
///
/// **Residual, deliberately not claimed closed:** rmcp deserializes the whole
/// `ClientJsonRpcMessage` off the transport *before* any handler runs, so these limits bound what a
/// request may *do*, **not** the parsing work an oversized message costs. Only a transport-level
/// byte cap would bound that, and there is none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quotas {
    /// Maximum serialized size of the `tools/call` `params` object in bytes (default 256 KiB).
    pub max_request_bytes: usize,
    /// Maximum length of any input array (default `10_000`).
    pub max_array_len: usize,
    /// Maximum length of any input string — **and, since D42, of any object KEY** (default 64 KiB).
    ///
    /// Keys are measured against this same limit rather than a new `Quotas::max_key_len` field:
    /// `Quotas` is `pub`, has public fields, is not `#[non_exhaustive]`, and is pinned in
    /// `tests/public_api.rs`, so adding a field would break literal construction and widen the
    /// public surface inside a patch release. The margin is thin and deliberately documented: at
    /// `max_string_len: 16` the longest params-level key `arguments` (9 B) has **+7 B** of headroom
    /// and the longest tool name value `diagnostics` (11 B) has **+5 B**. A future test setting
    /// `max_string_len < 12` would fail on the tool `name` — with a message naming the wrong
    /// culprit. There is deliberately NO carve-out for `name`.
    pub max_string_len: usize,
    /// Maximum batch size (default 100). *(v1.5 batch surface; the limit lands now.)*
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

/// Options for [`crate::run_mcp_server`] (spine §5, mcp plan public-API table).
///
/// Carries the optional instruction string surfaced to clients, the request quotas (NFR-18), and the
/// cooperative-shutdown handle (FR-17) — a `cancel()` on this token drains in-flight work and returns
/// cleanly from `run_mcp_server`.
#[derive(Debug, Clone)]
pub struct McpServerOptions {
    /// Optional human-readable instructions advertised to MCP clients.
    pub instructions: Option<String>,
    /// Untrusted-input limits (NFR-18).
    pub quotas: Quotas,
    /// Cooperative-shutdown handle (FR-17). The CLI installs the OS signal handler that cancels it.
    pub cancel: CancellationToken,
}

impl Default for McpServerOptions {
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
    use super::{CONTRACT_VERSION, McpServerOptions, Quotas};

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
    fn mcp_server_options_default_has_empty_instructions_and_default_quotas() {
        let opts = McpServerOptions::default();
        assert!(opts.instructions.is_none());
        assert_eq!(opts.quotas, Quotas::default());
        assert!(!opts.cancel.is_cancelled());
    }

    #[test]
    fn contract_version_is_the_bumped_v1_id() {
        // v1.0.1/D42: `deny_unknown_fields` on every input container emits `additionalProperties:
        // false` into `schema_bundle()`, so the contract bumped → v1.6 (additive, D35).
        assert_eq!(CONTRACT_VERSION, "unblock.mcp.v1.6");
    }

    #[test]
    fn contract_hash_is_a_64_char_hex() {
        use super::CONTRACT_HASH;
        assert_eq!(CONTRACT_HASH.len(), 64, "a SHA-256 hex digest");
        assert!(CONTRACT_HASH.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
