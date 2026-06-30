//! `unblock://schema` → [`SchemaBundle`] (spine §5.4, FR-12) — a pure builder via `schemars`.
//!
//! Carries the draft-2020-12 `JsonSchema` for every tool's input, stamped with the mcp-owned
//! [`crate::CONTRACT_VERSION`]. Pure (no `Session`) so the CLI can dump it offline; the per-tool
//! schemas are the `contract_version` drift detectors (the T2.3 FR-12 gate keys on them).

use rmcp::schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::options::CONTRACT_VERSION;
use crate::tools::claim::ClaimInput;
use crate::tools::defer::DeferInput;
use crate::tools::dep::DepToolInput;
use crate::tools::diagnostics::DiagnosticsInput;
use crate::tools::issue::IssueInput;
use crate::tools::query::QueryInput;
use crate::tools::sync::SyncInput;

/// The schema bundle: the input `JsonSchema` for each of the 7 tools (spine §5.4, FR-12).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SchemaBundle {
    /// The mcp contract version this bundle was generated under.
    pub contract_version: String,
    /// The `issue` tool input schema.
    pub issue: Value,
    /// The `claim` tool input schema.
    pub claim: Value,
    /// The `defer` tool input schema.
    pub defer: Value,
    /// The `query` tool input schema.
    pub query: Value,
    /// The `dep` tool input schema.
    pub dep: Value,
    /// The `sync` tool input schema.
    pub sync: Value,
    /// The `diagnostics` tool input schema.
    pub diagnostics: Value,
}

/// Build the [`SchemaBundle`] (pure; no `Session`).
#[must_use]
pub fn schema_bundle() -> SchemaBundle {
    SchemaBundle {
        contract_version: CONTRACT_VERSION.to_string(),
        issue: schema_for!(IssueInput).to_value(),
        claim: schema_for!(ClaimInput).to_value(),
        defer: schema_for!(DeferInput).to_value(),
        query: schema_for!(QueryInput).to_value(),
        dep: schema_for!(DepToolInput).to_value(),
        sync: schema_for!(SyncInput).to_value(),
        diagnostics: schema_for!(DiagnosticsInput).to_value(),
    }
}

#[cfg(test)]
mod tests {
    use super::schema_bundle;
    use crate::options::CONTRACT_VERSION;

    #[test]
    fn schema_bundle_stamps_the_contract_version() {
        assert_eq!(schema_bundle().contract_version, CONTRACT_VERSION);
    }

    #[test]
    fn every_tool_schema_is_an_object() {
        let bundle = schema_bundle();
        for schema in [
            &bundle.issue,
            &bundle.claim,
            &bundle.defer,
            &bundle.query,
            &bundle.dep,
            &bundle.sync,
            &bundle.diagnostics,
        ] {
            assert!(schema.is_object(), "tool schema must be a JSON object");
        }
    }
}
