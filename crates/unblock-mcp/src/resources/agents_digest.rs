//! `agents_digest()` -> [`AgentsDigest`] (spine §5.4, D33) — a pure, CLI-facing DERIVED VIEW over the
//! two discovery documents for the `unblock agents` managed AGENTS.md block (FR-14).
//!
//! NOT a resource (no `unblock://agents` URI, absent from `ResourceUri`/`resource_descriptors()`) and
//! NOT a member of the hashed contract tuple: it CONSUMES `capabilities()` + `schema_bundle()` bytes,
//! never alters them, and deliberately derives neither `Serialize` nor `JsonSchema` (a structural
//! guarantee it can never be folded into `SchemaBundle`/`Capabilities` or hashed). Re-serializing it is
//! not part of the FR-12 gate, so `CONTRACT_HASH`/`CONTRACT_VERSION` do not move when the digest or its
//! renderer change.
//!
//! Per action, the digest surfaces the FULL parameter surface — both required AND optional params —
//! derived structurally from the tool input's root `oneOf` arm (Miguel's ruling: an agent reading
//! AGENTS.md must know everything supported, not just the minimum). An arm-root `$ref` (e.g.
//! `issue.create` -> `#/$defs/CreateInput`) is resolved ONE level so the delegated payload's fields
//! surface too; a property-level `$ref` (e.g. `issue.delete.mode` -> `#/$defs/DeleteModeInput`) is
//! NEVER resolved — only the property key is listed, never its enum/const variants.

use std::collections::BTreeSet;

use serde_json::Value;

use super::capabilities::capabilities;
use super::schema::schema_bundle;
use crate::tools::args::discriminant_of;

/// A CLI-facing, fully-typed digest of the MCP contract surface (spine §5.4, D33): a pure DERIVED VIEW
/// over `capabilities()` + `schema_bundle()` that never alters their bytes. Rendered by `unblock agents`
/// into the managed AGENTS.md block (FR-14); keeps the schema-shape walk in this crate so the CLI needs
/// no `serde_json`.
#[derive(Debug, Clone)]
pub struct AgentsDigest {
    /// The mcp contract version this digest was derived under (copied from `capabilities()`).
    pub contract_version: String,
    /// One card per advertised tool, in `capabilities()`/spine §5.1 order.
    pub tools: Vec<ToolDigest>,
    /// The 5 advertised resources (uri + one-line description), copied from `capabilities()`.
    pub resources: Vec<ResourceDigest>,
    /// The 3 advertised prompts (name + one-line description), copied from `capabilities()`.
    pub prompts: Vec<PromptDigest>,
    /// The full error-code -> exit-code/retryable map, copied from `capabilities()`.
    pub error_codes: Vec<ErrorCodeDigest>,
}

/// One tool: its `capabilities()` descriptor + the structurally-derived action list.
#[derive(Debug, Clone)]
pub struct ToolDigest {
    /// The tool name (from `capabilities()`).
    pub name: String,
    /// The tool one-line description (from `capabilities()`).
    pub description: String,
    /// The actions this tool exposes, from `schema_bundle().<tool>.input`'s root `oneOf` (captures
    /// `create_bulk`). A FLAT input (`claim`) yields exactly one action with `discriminant == None`.
    pub actions: Vec<ToolAction>,
}

/// A single action: its wire discriminant pair + its FULL parameter surface (required and optional).
#[derive(Debug, Clone)]
pub struct ToolAction {
    /// The wire discriminant pair; `None` for a flat tool (`claim`), which has no `oneOf`.
    pub discriminant: Option<Discriminant>,
    /// Required param names (effective `required[]` incl. a resolved arm-root `$ref`, minus the
    /// discriminant), sorted.
    pub required: Vec<String>,
    /// Optional param names (effective properties minus required minus the discriminant), sorted.
    pub optional: Vec<String>,
}

/// The exact wire discriminant of one action, e.g. `{ key: "kind", value: "ready" }` (ub-q2v).
///
/// The KEY is detected per TOOL (a serde tag belongs to the whole enum); the VALUE is the arm's
/// `properties[key].const`. Deliberately NO `Serialize`/`JsonSchema`: the pair can never enter the
/// hashed `(capabilities(), schema_bundle())` tuple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discriminant {
    /// The discriminant field on the wire: `"action"`, or `"kind"` for `query`/`diagnostics`.
    pub key: String,
    /// The arm's `const` for that field, e.g. `"create_bulk"`.
    pub value: String,
}

/// A resource (uri + one-line description), copied from `capabilities()`.
#[derive(Debug, Clone)]
pub struct ResourceDigest {
    /// The `unblock://` uri or uri template.
    pub uri: String,
    /// The one-line description.
    pub description: String,
}

/// A prompt (name + one-line description), copied from `capabilities()`.
#[derive(Debug, Clone)]
pub struct PromptDigest {
    /// The prompt name.
    pub name: String,
    /// The one-line description.
    pub description: String,
}

/// An error code with its 0-8 exit code + retryability (`hint_shape` is intentionally omitted from the
/// AGENTS.md table).
#[derive(Debug, Clone)]
pub struct ErrorCodeDigest {
    /// The stable `SCREAMING_SNAKE_CASE` code.
    pub code: String,
    /// The 0-8 process exit code.
    pub exit_code: u8,
    /// Whether the failing operation is potentially retryable.
    pub retryable: bool,
}

/// Build the [`AgentsDigest`] (pure; no `Session`). A DERIVED VIEW over `capabilities()` +
/// `schema_bundle()` — it only READS their values, so `CONTRACT_HASH` is unaffected.
#[must_use]
pub fn agents_digest() -> AgentsDigest {
    let caps = capabilities();
    let bundle = schema_bundle();

    // The ONE place tool identity crosses the two documents: pair each tool NAME with its input
    // schema by the spine §5.1 field order (`SchemaBundle` is a struct, not a map).
    let inputs: [(&str, &Value); 8] = [
        ("issue", &bundle.issue.input),
        ("claim", &bundle.claim.input),
        ("defer", &bundle.defer.input),
        ("query", &bundle.query.input),
        ("dep", &bundle.dep.input),
        ("sync", &bundle.sync.input),
        ("diagnostics", &bundle.diagnostics.input),
        ("comment", &bundle.comment.input),
    ];

    let tools = caps
        .tools
        .into_iter()
        .map(|descriptor| {
            let actions = inputs
                .iter()
                .find(|pair| pair.0 == descriptor.name)
                .map(|(_, input)| tool_actions(input))
                // An unknown/renamed tool -> empty actions (never panics). This SWALLOWS a missing
                // pair above, so it is `every_tool_has_a_schema_pair` (below) — NOT this line —
                // that catches a tool omitted from `inputs`.
                .unwrap_or_default();
            ToolDigest {
                name: descriptor.name,
                description: descriptor.description,
                actions,
            }
        })
        .collect();

    let resources = caps
        .resources
        .into_iter()
        .map(|r| ResourceDigest {
            uri: r.uri,
            description: r.description,
        })
        .collect();

    let prompts = caps
        .prompts
        .into_iter()
        .map(|p| PromptDigest {
            name: p.name,
            description: p.description,
        })
        .collect();

    let error_codes = caps
        .error_codes
        .into_iter()
        .map(|e| ErrorCodeDigest {
            code: e.code,
            exit_code: e.exit_code,
            retryable: e.retryable,
        })
        .collect();

    AgentsDigest {
        contract_version: caps.contract_version,
        tools,
        resources,
        prompts,
        error_codes,
    }
}

/// Derive a tool's actions from its input. A TAGGED input carries a ROOT `oneOf` (one arm per action);
/// a FLAT input (only `claim`) has no root `oneOf` and yields one implicit action. Reads ONLY the root
/// `oneOf` — never `$defs` nor a nested property `oneOf`. Total: an unmatched shape -> the flat branch.
///
/// The discriminant KEY is detected ONCE per tool — the property carrying a string `const` in EVERY
/// arm (`discriminant_of`) — never per arm, where the first const-bearing property in `BTreeMap`
/// order could beat the real tag. Arms that agree on no key yield `discriminant: None`, which the
/// `discriminant_key_is_carried_per_tool` test rejects for a tagged tool.
fn tool_actions(input: &Value) -> Vec<ToolAction> {
    match input.get("oneOf").and_then(Value::as_array) {
        Some(arms) => {
            let key = discriminant_of(arms);
            arms.iter()
                .map(|arm| action_for_arm(arm, input, key.as_deref()))
                .collect()
        }
        None => vec![flat_action(input)],
    }
}

/// A flat tool's single implicit action (no discriminant): its FULL top-level parameter surface.
fn flat_action(input: &Value) -> ToolAction {
    let (required, optional) = effective_params(input, input, None);
    ToolAction {
        discriminant: None,
        required,
        optional,
    }
}

/// One tagged-enum `oneOf` arm -> a [`ToolAction`]. `key` is the tool's discriminant field (`action`
/// OR `kind`, detected STRUCTURALLY — no hard-coded key list); the arm's `properties[key].const` is
/// the action's value, and `key` is excluded from both the required and optional param lists.
fn action_for_arm(arm: &Value, root: &Value, key: Option<&str>) -> ToolAction {
    let (required, optional) = effective_params(arm, root, key);
    let discriminant = key.and_then(|key| {
        let value = arm.get("properties")?.get(key)?.get("const")?.as_str()?;
        Some(Discriminant {
            key: key.to_string(),
            value: value.to_string(),
        })
    });
    ToolAction {
        discriminant,
        required,
        optional,
    }
}

/// The FULL parameter surface for one node (an arm or a flat input): its required and optional param
/// names, sorted.
///
/// Merges TWO sources — (a) the node's OWN inline `properties`/`required[]`, and (b) IFF the node
/// carries a root-level `$ref` (e.g. `issue.create` -> `#/$defs/CreateInput`), the resolved-ONE-LEVEL
/// def's `properties`/`required[]` (looked up in `root["$defs"]`). A property-level `$ref` (a property
/// whose OWN schema is a `$ref`, e.g. `issue.delete.mode` -> `#/$defs/DeleteModeInput`) is NEVER
/// resolved — only the property KEY is collected, never the referenced def's shape.
///
/// `required` = effective `required[]` minus `exclude` (the discriminant field, if any).
/// `optional` = effective properties minus `required` minus `exclude`.
fn effective_params(
    node: &Value,
    root: &Value,
    exclude: Option<&str>,
) -> (Vec<String>, Vec<String>) {
    let mut properties = property_keys(node);
    let mut required = required_names(node);

    if let Some((def_properties, def_required)) = resolve_arm_root_ref(node, root) {
        properties.extend(def_properties);
        required.extend(def_required);
    }

    if let Some(field) = exclude {
        required.remove(field);
        properties.remove(field);
    }

    let optional: Vec<String> = properties.difference(&required).cloned().collect();
    let required: Vec<String> = required.into_iter().collect();
    (required, optional)
}

/// `node["properties"]` keys (empty if absent/malformed). `serde_json::Map` is a sorted `BTreeMap`
/// (the workspace does not enable `preserve_order`), and the caller merges into a `BTreeSet` anyway, so
/// this is sorted-by-construction; never a panic.
fn property_keys(node: &Value) -> BTreeSet<String> {
    node.get("properties")
        .and_then(Value::as_object)
        .map(|props| props.keys().cloned().collect())
        .unwrap_or_default()
}

/// `node["required"]` as a set of field names (empty if absent/malformed). Never a panic.
fn required_names(node: &Value) -> BTreeSet<String> {
    node.get("required")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Resolve a node's arm-ROOT `$ref` (a `$ref` sibling of `properties`, e.g. `issue.create`'s
/// `{"$ref": "#/$defs/CreateInput", "properties": {"action": ...}}`) ONE level into
/// `root["$defs"][<name>]`'s properties + required set. Returns `None` when the node carries no such
/// `$ref`, the ref string is not the expected `#/$defs/<name>` shape, or the def is missing —
/// never a panic (a missing/malformed `$defs` entry yields an empty contribution via the `None` arm).
fn resolve_arm_root_ref(
    node: &Value,
    root: &Value,
) -> Option<(BTreeSet<String>, BTreeSet<String>)> {
    let reference = node.get("$ref").and_then(Value::as_str)?;
    let name = reference.strip_prefix("#/$defs/")?;
    let def = root.get("$defs").and_then(Value::as_object)?.get(name)?;
    Some((property_keys(def), required_names(def)))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Discriminant, ToolAction, agents_digest, tool_actions};
    use crate::options::CONTRACT_VERSION;

    /// An action's discriminant VALUE (e.g. `"create_bulk"`), or `None` for a flat action.
    fn value(a: &ToolAction) -> Option<&str> {
        a.discriminant.as_ref().map(|d| d.value.as_str())
    }

    /// **THE DRIFT GUARD for the `inputs` pairs array.** Every tool in `capabilities()` MUST resolve
    /// to a schema pair and therefore to a NON-EMPTY action list.
    ///
    /// Without this test, omitting a tool from the hard-coded `inputs: [(&str, &Value); N]` array is
    /// SILENT: `find()` returns `None`, `unwrap_or_default()` swallows it, and the rendered
    /// `AGENTS.md` ships that tool with an EMPTY actions table — no panic, no failure. That is
    /// exactly how the 8th tool (`comment`, D37) would have shipped dark.
    ///
    /// Every v1 tool input is a tagged enum, so every tool has ≥ 1 action; a tool whose actions come
    /// back empty is proof its pair is missing.
    #[test]
    fn every_tool_has_a_schema_pair_and_non_empty_actions() {
        let digest = agents_digest();
        assert_eq!(
            digest.tools.len(),
            8,
            "the v1 surface is 8 tools (RK-3 FULL)"
        );
        for tool in &digest.tools {
            assert!(
                !tool.actions.is_empty(),
                "tool `{}` has NO actions — it is missing from the agents_digest `inputs` pairs \
                 array, so AGENTS.md would ship it with an empty table",
                tool.name
            );
        }
    }

    /// The 8th tool (D37) renders its four real actions.
    #[test]
    fn comment_tool_lists_its_four_actions() {
        let digest = agents_digest();
        let comment = digest
            .tools
            .iter()
            .find(|t| t.name == "comment")
            .expect("the comment tool must be in the digest");
        let names: Vec<&str> = comment.actions.iter().filter_map(value).collect();
        for action in ["add", "list", "update", "delete"] {
            assert!(
                names.contains(&action),
                "missing comment action {action}: {names:?}"
            );
        }
    }

    /// The `issue` tool's actions must include BOTH `create` and `create_bulk` — the structural proof:
    /// `create_bulk` exists only as a `oneOf` arm, never in the tool's one-line description prose.
    #[test]
    fn issue_has_create_and_create_bulk() {
        let digest = agents_digest();
        let issue = digest
            .tools
            .iter()
            .find(|t| t.name == "issue")
            .expect("issue tool present");
        let names: Vec<&str> = issue.actions.iter().filter_map(value).collect();
        assert!(names.contains(&"create"));
        assert!(names.contains(&"create_bulk"));
    }

    /// `issue create`'s arm-root `$ref` (`#/$defs/CreateInput`) must resolve one level: `title` is
    /// required, and at least one of `CreateInput`'s optional fields surfaces too — neither list ever
    /// carries the `action` discriminant.
    #[test]
    fn create_resolves_ref_required_title() {
        let digest = agents_digest();
        let issue = digest
            .tools
            .iter()
            .find(|t| t.name == "issue")
            .expect("issue tool present");
        let create = issue
            .actions
            .iter()
            .find(|a| value(a) == Some("create"))
            .expect("create action present");
        assert_eq!(create.required, vec!["title".to_string()]);
        assert!(create.optional.contains(&"description".to_string()));
        assert!(create.optional.contains(&"priority".to_string()));
        assert!(!create.required.iter().any(|p| p == "action"));
        assert!(!create.optional.iter().any(|p| p == "action"));
    }

    /// A property-level `$ref` (`issue.delete.mode` -> `#/$defs/DeleteModeInput`, a `oneOf` of consts
    /// `tombstone`/`cascade`/`hard`/`dry_run`) must NEVER be resolved: `delete.optional` carries the
    /// property KEY `mode`, but none of the referenced def's const variants (the M9 mutation target).
    #[test]
    fn property_level_ref_not_expanded() {
        let digest = agents_digest();
        let issue = digest
            .tools
            .iter()
            .find(|t| t.name == "issue")
            .expect("issue tool present");
        let delete = issue
            .actions
            .iter()
            .find(|a| value(a) == Some("delete"))
            .expect("delete action present");
        assert!(delete.optional.contains(&"mode".to_string()));
        for variant in ["tombstone", "cascade", "hard", "dry_run"] {
            assert!(
                !delete.optional.iter().any(|p| p == variant),
                "delete.optional must not contain the DeleteModeInput const {variant}"
            );
            assert!(!delete.required.iter().any(|p| p == variant));
        }
    }

    /// `claim` is the one FLAT tool (no root `oneOf`): a single implicit action (no discriminant)
    /// carrying the full top-level required/optional split, sorted.
    #[test]
    fn flat_input_tool_single_implicit_action() {
        let digest = agents_digest();
        let claim = digest
            .tools
            .iter()
            .find(|t| t.name == "claim")
            .expect("claim tool present");
        assert_eq!(claim.actions.len(), 1);
        let action = &claim.actions[0];
        assert!(action.discriminant.is_none());
        assert_eq!(
            action.required,
            vec!["assignee".to_string(), "id".to_string()]
        );
        assert_eq!(
            action.optional,
            vec![
                "agent_name".to_string(),
                "harness".to_string(),
                "model".to_string()
            ]
        );
    }

    /// `query`/`diagnostics` tag on `kind`, not `action` — proves the discriminant detection is
    /// STRUCTURAL (no hard-coded field name) and that `kind` never leaks into the param lists.
    #[test]
    fn kind_discriminant_walked() {
        let digest = agents_digest();
        let query = digest
            .tools
            .iter()
            .find(|t| t.name == "query")
            .expect("query tool present");
        let names: Vec<&str> = query.actions.iter().filter_map(value).collect();
        for expected in ["list", "ready", "blocked", "search", "count", "stale"] {
            assert!(names.contains(&expected));
        }
        for action in &query.actions {
            assert!(!action.required.iter().any(|p| p == "kind"));
            assert!(!action.optional.iter().any(|p| p == "kind"));
        }
    }

    /// The digest carries the wire discriminant KEY per tool (ub-q2v): every `query`/`diagnostics`
    /// action is keyed `kind` (not `action`), every other tagged tool's action `action`, and flat
    /// `claim` has none. A tagged tool whose arms agree on no key yields `None` and FAILS here — it
    /// must never render `—`. An unlisted tool fails too, so a new tool's key gets pinned.
    #[test]
    fn discriminant_key_is_carried_per_tool() {
        let digest = agents_digest();
        for tool in &digest.tools {
            for a in &tool.actions {
                match tool.name.as_str() {
                    "query" | "diagnostics" => assert!(
                        a.discriminant.as_ref().map(|d| d.key.as_str()) == Some("kind"),
                        "tool {} must be keyed `kind`: {a:?}",
                        tool.name
                    ),
                    "issue" | "defer" | "dep" | "sync" | "comment" => assert!(
                        a.discriminant.as_ref().map(|d| d.key.as_str()) == Some("action"),
                        "tool {} must be keyed `action`: {a:?}",
                        tool.name
                    ),
                    "claim" => assert!(a.discriminant.is_none(), "claim is flat: {a:?}"),
                    other => panic!("tool `{other}` has no pinned discriminant key"),
                }
            }
        }
    }

    /// The key is the property carrying a string `const` in EVERY arm, not the first const-bearing
    /// property of one arm in `BTreeMap` order: `agent` sorts before `kind` but is const in one arm
    /// only. Arms that agree on no key yield `None` for every action.
    #[test]
    fn discriminant_key_is_detected_across_all_arms() {
        let tagged = json!({"oneOf": [
            {"properties": {"agent": {"const": "x"}, "kind": {"const": "ready"}, "limit": {}}},
            {"properties": {"kind": {"const": "stale"}}},
        ]});
        let actions = tool_actions(&tagged);
        let pairs: Vec<Option<Discriminant>> =
            actions.iter().map(|a| a.discriminant.clone()).collect();
        assert_eq!(
            pairs,
            vec![
                Some(Discriminant {
                    key: "kind".to_string(),
                    value: "ready".to_string()
                }),
                Some(Discriminant {
                    key: "kind".to_string(),
                    value: "stale".to_string()
                }),
            ]
        );
        assert_eq!(
            actions[0].optional,
            vec!["agent".to_string(), "limit".to_string()]
        );

        let disagreeing = json!({"oneOf": [
            {"properties": {"action": {"const": "a"}}},
            {"properties": {"kind": {"const": "b"}}},
        ]});
        assert!(
            tool_actions(&disagreeing)
                .iter()
                .all(|a| a.discriminant.is_none())
        );
    }

    /// The full error-code map is copied verbatim (non-empty, one entry per `ErrorCode`).
    #[test]
    fn error_codes_match_all() {
        assert_eq!(
            agents_digest().error_codes.len(),
            unblock_error::ErrorCode::ALL.len()
        );
    }

    /// The digest is stamped with the SAME contract version as `capabilities()`/`schema_bundle()`.
    #[test]
    fn contract_version_stamped() {
        assert_eq!(agents_digest().contract_version, CONTRACT_VERSION);
    }
}
