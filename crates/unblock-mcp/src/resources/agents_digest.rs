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
    /// `create_bulk`). A FLAT input (`claim`) yields exactly one action with `name == None`.
    pub actions: Vec<ToolAction>,
}

/// A single action: its discriminant name + its FULL parameter surface (required and optional).
#[derive(Debug, Clone)]
pub struct ToolAction {
    /// The discriminant `const` (e.g. `"create_bulk"`); `None` for a flat tool's single implicit action.
    pub name: Option<String>,
    /// Required param names (effective `required[]` incl. a resolved arm-root `$ref`, minus the
    /// discriminant), sorted.
    pub required: Vec<String>,
    /// Optional param names (effective properties minus required minus the discriminant), sorted.
    pub optional: Vec<String>,
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
    let inputs: [(&str, &Value); 7] = [
        ("issue", &bundle.issue.input),
        ("claim", &bundle.claim.input),
        ("defer", &bundle.defer.input),
        ("query", &bundle.query.input),
        ("dep", &bundle.dep.input),
        ("sync", &bundle.sync.input),
        ("diagnostics", &bundle.diagnostics.input),
    ];

    let tools = caps
        .tools
        .into_iter()
        .map(|descriptor| {
            let actions = inputs
                .iter()
                .find(|pair| pair.0 == descriptor.name)
                .map(|(_, input)| tool_actions(input))
                .unwrap_or_default(); // unknown/renamed tool -> empty (never panics; caught by a test)
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
fn tool_actions(input: &Value) -> Vec<ToolAction> {
    match input.get("oneOf").and_then(Value::as_array) {
        Some(arms) => arms.iter().map(|arm| action_for_arm(arm, input)).collect(),
        None => vec![flat_action(input)],
    }
}

/// A flat tool's single implicit action (no discriminant): its FULL top-level parameter surface.
fn flat_action(input: &Value) -> ToolAction {
    let (required, optional) = effective_params(input, input, None);
    ToolAction {
        name: None,
        required,
        optional,
    }
}

/// One tagged-enum `oneOf` arm -> a [`ToolAction`]. The discriminant is the arm property whose schema
/// carries a string `const` (field `action` OR `kind`, detected STRUCTURALLY — no hard-coded key list);
/// its `const` is the action name, excluded from both the required and optional param lists.
fn action_for_arm(arm: &Value, root: &Value) -> ToolAction {
    let disc = discriminant(arm);
    let exclude = disc.as_ref().map(|(field, _)| field.as_str());
    let (required, optional) = effective_params(arm, root, exclude);
    ToolAction {
        name: disc.map(|(_, value)| value),
        required,
        optional,
    }
}

/// The discriminant of a tagged-enum arm: the first `properties.<field>` whose schema carries a string
/// `const`. Returns `(field_name, const_value)`, or `None` (defensive — every v1 arm has one, but the
/// walk stays total). Order-independent: exactly one property is a `const` discriminant per arm.
fn discriminant(node: &Value) -> Option<(String, String)> {
    let props = node.get("properties").and_then(Value::as_object)?;
    props.iter().find_map(|(field, schema)| {
        schema
            .get("const")
            .and_then(Value::as_str)
            .map(|c| (field.clone(), c.to_string()))
    })
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
    use super::agents_digest;
    use crate::options::CONTRACT_VERSION;

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
        let names: Vec<&str> = issue
            .actions
            .iter()
            .filter_map(|a| a.name.as_deref())
            .collect();
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
            .find(|a| a.name.as_deref() == Some("create"))
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
            .find(|a| a.name.as_deref() == Some("delete"))
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

    /// `claim` is the one FLAT tool (no root `oneOf`): a single implicit action (`name == None`)
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
        assert!(action.name.is_none());
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
        let names: Vec<&str> = query
            .actions
            .iter()
            .filter_map(|a| a.name.as_deref())
            .collect();
        for expected in ["list", "ready", "blocked", "search", "count", "stale"] {
            assert!(names.contains(&expected));
        }
        for action in &query.actions {
            assert!(!action.required.iter().any(|p| p == "kind"));
            assert!(!action.optional.iter().any(|p| p == "kind"));
        }
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
