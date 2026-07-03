//! `unblock://schema` → [`SchemaBundle`] (spine §5.4, FR-12) — a pure builder via `schemars`.
//!
//! Carries the draft-2020-12 `JsonSchema` for every tool's INPUT **and** OUTPUT as a per-tool
//! [`ToolSchemas`] pair (D25/FORK-1B — "`JsonSchema` per tool I/O" is now TRUE as written), plus the
//! bundle-level shared `error` schema (`StructuredError`, the in-band FR-11 shape published ONCE; the
//! rmcp `is_error` flag is the channel discriminator). Stamped with the mcp-owned
//! [`crate::CONTRACT_VERSION`]. Pure (no `Session`) so the CLI can dump it offline; both discovery
//! documents are the `contract_version` drift detectors pinned by `CONTRACT_HASH` (the T2.6 FR-12 gate).

use rmcp::schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use unblock_error::StructuredError;
use unblock_model::{DiagnosticReport, Issue};

use crate::options::CONTRACT_VERSION;
use crate::tools::claim::ClaimInput;
use crate::tools::defer::DeferInput;
use crate::tools::dep::DepToolInput;
use crate::tools::diagnostics::DiagnosticsInput;
use crate::tools::issue::IssueInput;
use crate::tools::output::{DepOutput, IssueOutput, QueryOutput, SyncOutput};
use crate::tools::query::QueryInput;
use crate::tools::sync::SyncInput;

/// The input + output `JsonSchema` pair for one tool (spine §5.4, D25).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ToolSchemas {
    /// The tool's input schema (`schema_for!(<Tool>Input)`, draft 2020-12).
    pub input: Value,
    /// The tool's success-output schema (`schema_for!(<tool's §5.3 output>)`).
    pub output: Value,
}

/// The schema bundle: the per-tool `{input, output}` `JsonSchema` pairs + the shared in-band error
/// schema (spine §5.4, FR-12/D25).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SchemaBundle {
    /// The mcp contract version this bundle was generated under.
    pub contract_version: String,
    /// The `issue` tool input + output schemas (output = `IssueOutput`).
    pub issue: ToolSchemas,
    /// The `claim` tool input + output schemas (output = `Issue`).
    pub claim: ToolSchemas,
    /// The `defer` tool input + output schemas (output = `Issue`).
    pub defer: ToolSchemas,
    /// The `query` tool input + output schemas (output = `QueryOutput`).
    pub query: ToolSchemas,
    /// The `dep` tool input + output schemas (output = `DepOutput`).
    pub dep: ToolSchemas,
    /// The `sync` tool input + output schemas (output = `SyncOutput`).
    pub sync: ToolSchemas,
    /// The `diagnostics` tool input + output schemas (output = `DiagnosticReport`).
    pub diagnostics: ToolSchemas,
    /// The shared in-band error output (`StructuredError`) every tool may return with `is_error=true`
    /// (FR-11) — published ONCE, bundle-level (the rmcp `is_error` flag is the channel discriminator).
    pub error: Value,
}

/// Build the [`SchemaBundle`] (pure; no `Session`).
#[must_use]
pub fn schema_bundle() -> SchemaBundle {
    SchemaBundle {
        contract_version: CONTRACT_VERSION.to_string(),
        issue: ToolSchemas {
            input: schema_for!(IssueInput).to_value(),
            output: schema_for!(IssueOutput).to_value(),
        },
        claim: ToolSchemas {
            input: schema_for!(ClaimInput).to_value(),
            output: schema_for!(Issue).to_value(),
        },
        defer: ToolSchemas {
            input: schema_for!(DeferInput).to_value(),
            output: schema_for!(Issue).to_value(),
        },
        query: ToolSchemas {
            input: schema_for!(QueryInput).to_value(),
            output: schema_for!(QueryOutput).to_value(),
        },
        dep: ToolSchemas {
            input: schema_for!(DepToolInput).to_value(),
            output: schema_for!(DepOutput).to_value(),
        },
        sync: ToolSchemas {
            input: schema_for!(SyncInput).to_value(),
            output: schema_for!(SyncOutput).to_value(),
        },
        diagnostics: ToolSchemas {
            input: schema_for!(DiagnosticsInput).to_value(),
            output: schema_for!(DiagnosticReport).to_value(),
        },
        error: schema_for!(StructuredError).to_value(),
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
        let pairs = [
            &bundle.issue,
            &bundle.claim,
            &bundle.defer,
            &bundle.query,
            &bundle.dep,
            &bundle.sync,
            &bundle.diagnostics,
        ];
        for pair in pairs {
            assert!(
                pair.input.is_object(),
                "tool input schema must be an object"
            );
            assert!(
                pair.output.is_object(),
                "tool output schema must be an object"
            );
        }
        assert!(
            bundle.error.is_object(),
            "the shared error schema must be an object"
        );
    }
}
