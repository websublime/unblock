//! `unblock://capabilities` → [`Capabilities`] (spine §5.4, FR-12) — a pure builder (no `Session`).
//!
//! Lists the 7 tools, 5 resources, 3 prompts, and the full `ErrorCode`→exit-code/retryable map, all
//! stamped with the mcp-owned [`crate::CONTRACT_VERSION`] (F-5). Pure so the CLI can dump the contract
//! offline (FR-12) without a running server.

use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use unblock_error::{ErrorCode, HintShape};

use crate::options::CONTRACT_VERSION;

/// The discovery document (spine §5.4 `Capabilities`, FR-12).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Capabilities {
    /// The mcp contract version (bumped on any tool/resource/prompt schema change).
    pub contract_version: String,
    /// The advertised tools.
    pub tools: Vec<ToolDescriptor>,
    /// The advertised resources.
    pub resources: Vec<ResourceDescriptor>,
    /// The advertised prompts.
    pub prompts: Vec<PromptDescriptor>,
    /// The full error-code → exit-code/retryable/hint-shape map.
    pub error_codes: Vec<ErrorCodeDescriptor>,
}

/// A tool descriptor (name + one-line description).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ToolDescriptor {
    /// The tool name.
    pub name: String,
    /// A one-line description.
    pub description: String,
}

/// A resource descriptor (uri template + one-line description).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ResourceDescriptor {
    /// The uri (or uri template) the resource is read at.
    pub uri: String,
    /// A one-line description.
    pub description: String,
}

/// A prompt descriptor (name + one-line description).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PromptDescriptor {
    /// The prompt name.
    pub name: String,
    /// A one-line description.
    pub description: String,
}

/// An error-code descriptor: the stable code, its 0–8 exit code, retryability, and a hint shape.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ErrorCodeDescriptor {
    /// The stable `SCREAMING_SNAKE_CASE` code.
    pub code: String,
    /// The 0–8 process exit code (parity with the CLI, spine §2.3).
    pub exit_code: u8,
    /// Whether the failing operation is potentially retryable.
    pub retryable: bool,
    /// The static shape of the self-correction hint this code may carry (spine §2.2, D25/FORK-4B).
    pub hint_shape: HintShape,
}

/// Build the [`Capabilities`] document (pure; no `Session`).
#[must_use]
pub fn capabilities() -> Capabilities {
    Capabilities {
        contract_version: CONTRACT_VERSION.to_string(),
        tools: tool_descriptors(),
        resources: resource_descriptors(),
        prompts: prompt_descriptors(),
        error_codes: error_code_descriptors(),
    }
}

/// The 7 tools (spine §5.1).
fn tool_descriptors() -> Vec<ToolDescriptor> {
    [
        (
            "issue",
            "Create, show, update, close, reopen, delete, or restore issues.",
        ),
        ("claim", "Atomically claim an issue for an assignee."),
        (
            "defer",
            "Defer an issue until a future timestamp, or undefer it.",
        ),
        (
            "query",
            "Query issues: list, ready, blocked, search, count, or stale.",
        ),
        (
            "dep",
            "Manage and query dependencies: add, remove, list, tree, cycles, or graph.",
        ),
        (
            "sync",
            "Export/import the issue store as JSONL, or one-shot import a bd export.",
        ),
        (
            "diagnostics",
            "Diagnostics: stats, info, where, version, lint, changelog, or orphans.",
        ),
    ]
    .into_iter()
    .map(|(name, description)| ToolDescriptor {
        name: name.to_string(),
        description: description.to_string(),
    })
    .collect()
}

/// The 5 resources (spine §5.4).
fn resource_descriptors() -> Vec<ResourceDescriptor> {
    [
        ("unblock://issues/{id}", "A single issue by id."),
        (
            "unblock://issues/ready",
            "The default-complete ready set (agent entrypoint).",
        ),
        ("unblock://issues/blocked", "The blocked set."),
        ("unblock://capabilities", "This discovery document."),
        (
            "unblock://schema",
            "The JsonSchema bundle for every tool I/O.",
        ),
    ]
    .into_iter()
    .map(|(uri, description)| ResourceDescriptor {
        uri: uri.to_string(),
        description: description.to_string(),
    })
    .collect()
}

/// The 3 prompts (spine §5.5).
fn prompt_descriptors() -> Vec<PromptDescriptor> {
    [
        (
            "triage",
            "A guided triage workflow over blocked/unassigned/deferred work.",
        ),
        (
            "plan_next_work",
            "Drive the ready -> claim selection (FR-20).",
        ),
        (
            "close_with_suggestions",
            "Close an issue and surface the newly-unblocked set (FR-11).",
        ),
    ]
    .into_iter()
    .map(|(name, description)| PromptDescriptor {
        name: name.to_string(),
        description: description.to_string(),
    })
    .collect()
}

/// The full error-code map (every `ErrorCode`, in declaration order, with its exit code +
/// retryability — FR-11 parity with the spine §2.3 exit table).
fn error_code_descriptors() -> Vec<ErrorCodeDescriptor> {
    ErrorCode::ALL
        .into_iter()
        .map(|code| ErrorCodeDescriptor {
            code: code.as_str().to_string(),
            exit_code: code.exit_code(),
            retryable: code.is_retryable(),
            hint_shape: code.hint_shape(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::capabilities;
    use crate::options::CONTRACT_VERSION;
    use unblock_error::ErrorCode;

    #[test]
    fn capabilities_stamps_the_contract_version() {
        assert_eq!(capabilities().contract_version, CONTRACT_VERSION);
    }

    #[test]
    fn capabilities_lists_the_seven_tools() {
        assert_eq!(capabilities().tools.len(), 7);
    }

    #[test]
    fn capabilities_lists_five_resources_and_three_prompts() {
        let caps = capabilities();
        assert_eq!(caps.resources.len(), 5);
        assert_eq!(caps.prompts.len(), 3);
    }

    #[test]
    fn capabilities_includes_every_error_code() {
        assert_eq!(capabilities().error_codes.len(), ErrorCode::ALL.len());
    }
}
