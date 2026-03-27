//! Init tool — bootstraps a new Projects V2 project for the repository.
//!
//! Creates a project container via the `createProjectV2` GraphQL mutation.
//! Idempotent: if a project with the same title already exists, returns it
//! with `created: false`. Does not affect the dependency graph — no cache
//! invalidation is needed.
//!
//! This tool is functional in bootstrap mode (no project configured).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Input parameters for the `init` MCP tool.
///
/// All fields are optional. When omitted, the tool auto-detects the owner
/// type (org vs. user) and uses a default title of `"{repo} Tasks"`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct InitParams {
    /// Owner scope: `"org"` or `"user"`. Auto-detected from the repository
    /// owner if omitted.
    pub scope: Option<String>,
    /// Project title. Defaults to `"{repo} Tasks"` if omitted.
    pub title: Option<String>,
    /// Project description. Accepted for forward-compatibility but not yet
    /// sent to the API (the `createProjectV2` mutation does not support a
    /// description input).
    pub description: Option<String>,
    /// Whether the project should be public. Defaults to `false`.
    ///
    /// **Note:** Project visibility is not configurable via the
    /// `createProjectV2` mutation — this parameter is accepted for
    /// forward-compatibility and logged but not yet wired.
    pub public: Option<bool>,
}

/// Result returned by the `init` MCP tool.
///
/// Contains the project number, URL, whether it was newly created, the
/// resolved scope, and a hint message for the next step.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct InitResult {
    /// The project number (visible in the GitHub UI URL).
    pub project_number: u64,
    /// The project URL.
    pub url: String,
    /// `true` if the project was newly created, `false` if an existing
    /// project with a matching title was found.
    pub created: bool,
    /// The resolved owner scope: `"org"` or `"user"`.
    pub scope: String,
    /// A hint message for the agent, suggesting the next step.
    pub hint: String,
}
