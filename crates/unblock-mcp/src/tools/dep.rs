//! Tool **#5 `dep`** — dependency edges and graph queries (spine §5.1/§5.2, FR-5).
//!
//! Maps `DepToolInput{action}` to `Session::{add_dep, remove_dep, list_dependencies,
//! dependency_tree, dependency_graph, detect_cycles}`:
//! - `list` → `Session::list_dependencies(id)` (D1, the direct edges declared BY `id`).
//! - `graph` → `Session::dependency_graph(roots)` (empty roots = the whole graph).
//! - `cycles` → `Session::detect_cycles(blocking_only)`; `blocking_only` defaults TRUE (gating-only,
//!   the FR-5 ready view; `false` = all dep types, the integrity/lint view — D19).
//!
//! A cycle-rejecting `add` surfaces the engine's ordered cycle path naming every node (D2).

use chrono::Utc;
// D42 SEAM: this is the CRATE-LOCAL `Parameters` (`crate::tools::args`), NOT rmcp's. It defers
// deserialization so argument errors reach the FR-11 in-band channel instead of an out-of-band
// `-32602`. The NAME IS LOAD-BEARING (rmcp-macros matches the ident `Parameters` to pick the
// published inputSchema) — see `tools/args.rs`. Do NOT "fix" this back to rmcp's wrapper.
use crate::tools::args::{Parameters, parse_args};
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::tool;
use serde::{Deserialize, Serialize};
use unblock_model::DependencyType;

use crate::server::UnblockServer;
use crate::tools::dto::Attribution;
use crate::tools::output::{CycleList, DepAdded, DepList, DepOutput, DepRemoved};
use crate::tools::{engine_err_json, err_json, ok_json};

/// serde default for [`DepToolInput::Cycles::blocking_only`] (wire-only; the trait takes a bare bool).
fn default_true() -> bool {
    true
}

/// The `dep` tool input (spine §5.2 — EXACT shape; was `DepInput2`).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
// §5.2a (CD-1): inject the root `"type": "object"` (the tagged-enum `oneOf` root omits it, which
// strict MCP clients reject) — the union is preserved verbatim.
#[schemars(extend("type" = "object"))]
// D42: `#[serde(deny_unknown_fields)]` — an unknown/misspelled argument is REJECTED in-band
// instead of being silently dropped. NOT recursive and inert on a flatten TARGET: every nested
// container needs its OWN attribute (see `tools/args.rs` + the CHECK-3 container guard).
#[serde(deny_unknown_fields)]
pub(crate) enum DepToolInput {
    /// Add a dependency edge (cycle-rejecting).
    Add {
        /// The dependent issue id.
        issue_id: String,
        /// The blocker issue id.
        depends_on_id: String,
        /// The dependency type.
        dep_type: DependencyType,
        /// Optional JSON metadata for the edge.
        #[serde(default)]
        metadata: Option<String>,
        #[serde(flatten)]
        attribution: Attribution,
    },
    /// Remove a dependency edge.
    Remove {
        /// The dependent issue id.
        issue_id: String,
        /// The blocker issue id.
        depends_on_id: String,
        /// The dependency type.
        dep_type: DependencyType,
        #[serde(flatten)]
        attribution: Attribution,
    },
    /// List the direct edges declared by an issue.
    List {
        /// The issue id.
        id: String,
    },
    /// The dependency subtree rooted at an issue.
    Tree {
        /// The root issue id.
        id: String,
    },
    /// Detect dependency cycles.
    Cycles {
        /// Restrict to gating edges only (default TRUE; `false` = all dep types, D19).
        #[serde(default = "default_true")]
        blocking_only: bool,
    },
    /// The dependency graph for a root set (empty roots = the whole graph).
    Graph {
        /// The root ids (empty = the whole graph).
        #[serde(default)]
        roots: Vec<String>,
    },
}

#[rmcp::tool_router(router = dep_router, vis = "pub(crate)")]
impl UnblockServer {
    /// Dependency edges and graph queries (FR-5).
    #[tool(
        name = "dep",
        description = "Manage and query dependencies: add, remove, list, tree, cycles, or graph."
    )]
    pub(crate) async fn dep(&self, Parameters(raw, _): Parameters<DepToolInput>) -> CallToolResult {
        // D42 PROLOGUE: the ONLY deserialization of tool arguments. The NFR-18 quota already
        // ran once in `call_tool` over the whole `params`. `DepToolInput` carries
        // `#[serde(deny_unknown_fields)]`, so an unknown/misspelled argument is REJECTED here,
        // in-band, instead of being silently discarded.
        let input: DepToolInput = match parse_args(raw) {
            Ok(input) => input,
            Err(structured) => return err_json(&structured),
        };
        match input {
            DepToolInput::Add {
                issue_id,
                depends_on_id,
                dep_type,
                metadata,
                attribution: _,
            } => {
                let now = Utc::now();
                let actor = self.session.actor().to_string();
                let dep = crate::tools::dto::DepInput {
                    issue_id,
                    depends_on_id,
                    dep_type,
                    metadata,
                }
                .into_dependency(&actor, now);
                match self.session.add_dep(&dep).await {
                    Ok(()) => ok_json(&DepOutput::Added(DepAdded { added: true })),
                    Err(err) => engine_err_json(&err),
                }
            }
            DepToolInput::Remove {
                issue_id,
                depends_on_id,
                dep_type,
                attribution: _,
            } => match self
                .session
                .remove_dep(&issue_id, &depends_on_id, &dep_type)
                .await
            {
                Ok(()) => ok_json(&DepOutput::Removed(DepRemoved { removed: true })),
                Err(err) => engine_err_json(&err),
            },
            DepToolInput::List { id } => match self.session.list_dependencies(&id).await {
                Ok(deps) => ok_json(&DepOutput::Deps(DepList { deps })),
                Err(err) => engine_err_json(&err),
            },
            DepToolInput::Tree { id } => match self.session.dependency_tree(&id).await {
                Ok(tree) => ok_json(&DepOutput::Tree(tree)),
                Err(err) => engine_err_json(&err),
            },
            DepToolInput::Cycles { blocking_only } => {
                match self.session.detect_cycles(blocking_only).await {
                    Ok(cycles) => ok_json(&DepOutput::Cycles(CycleList { cycles })),
                    Err(err) => engine_err_json(&err),
                }
            }
            DepToolInput::Graph { roots } => match self.session.dependency_graph(&roots).await {
                Ok(graph) => ok_json(&DepOutput::Tree(graph)),
                Err(err) => engine_err_json(&err),
            },
        }
    }
}
