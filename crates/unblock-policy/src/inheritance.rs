//! Minimal v1 pure ancestor-context selection (plan §2 `inheritance.rs`; original beads#297).
//!
//! Selects up to the **two bookends** of a caller-supplied ancestor chain — the immediate parent
//! and the root (an `epic` root takes the `"epic"` role, any other root the `"root"` role) — for
//! context inheritance. It is **infallible** (`-> Vec<InheritedBlock>`, no `Result`): the only error
//! source in the original was the storage I/O the engine owns, so an empty/parentless chain yields
//! an empty `Vec` and a tombstoned/field-less ancestor is silently skipped.
//!
//! # Chain ordering convention
//!
//! `chain[0]` is the **immediate parent** of the issue under evaluation and `chain[last]` is the
//! **root** (i.e. the chain is ordered nearest-first, walking up the parent tree). A single-element
//! chain means parent == root → one block (the root role wins). Disabled config → empty.
//!
//! # Emission order (v1.1 render reconciliation)
//!
//! v1 emits **parent-first** (`chain[0]` → `blocks[0]`, role `"parent"`; the root bookend last).
//! The original `beads` `collect_inherited_blocks` emitted **root-first** as a *text-layout*
//! (render) concern, not a policy invariant — the spine pins no normative order on inheritance
//! blocks and the plan pins only the *selected set*. So this pure L1 selector is free to return
//! parent-first; **`unblock-render` (v1.1) owns final presentation order** and must reorder for the
//! surfaced UX rather than silently inheriting this order.
//!
//! # v1 limits (full logic lands in v1.1)
//!
//! v1 ships only the two-bookend selection over an already-walked chain: it does not resolve
//! merge/precedence between overlapping fields, does not deduplicate by content, and does not wire
//! the `enabled`/`fields` config from a file (the engine supplies the chain + the
//! [`InheritanceConfig`] from config). The full role/epic/merge logic + config wiring is v1.1.

use unblock_model::IssueType;

/// One node in a caller-supplied ancestor chain (DB-derived, walked by the engine).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AncestorNode {
    /// The ancestor issue's id.
    pub id: String,
    /// The ancestor's issue type (an `Epic` root is given the `"epic"` role).
    pub issue_type: IssueType,
    /// The ancestor's title (carried into the selected block).
    pub title: String,
    /// The ancestor's inheritable `agent_context` field, if present.
    pub agent_context: Option<String>,
    /// Whether the ancestor is a tombstone (a tombstoned ancestor is skipped).
    pub is_tombstone: bool,
}

/// A selected inherited-context block, attributed to its source ancestor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InheritedBlock {
    /// The source ancestor's id.
    pub source_id: String,
    /// The role of the source in the chain: `"parent"`, `"root"`, or `"epic"`.
    pub source_role: String,
    /// The source ancestor's title.
    pub source_title: String,
    /// The configured field whose value was used (e.g. `"agent_context"`).
    pub field_used: String,
    /// The inherited content (the first-present configured field's value).
    pub content: String,
}

/// Inheritance configuration (the engine supplies this from config in v1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InheritanceConfig {
    /// Whether inheritance is enabled at all (disabled → no blocks).
    pub enabled: bool,
    /// The ordered list of fields to try; the **first present** field on an ancestor wins.
    pub fields: Vec<String>,
}

/// The roles assignable to a selected bookend.
const ROLE_PARENT: &str = "parent";
const ROLE_ROOT: &str = "root";
const ROLE_EPIC: &str = "epic";

/// The only inheritable field source in v1 (`agent_context`); the config field list is matched
/// case-sensitively against this known field name.
const FIELD_AGENT_CONTEXT: &str = "agent_context";

/// Resolve the first configured field that is present on `node`, returning `(field_name, content)`.
///
/// v1 knows exactly one inheritable field (`agent_context`); a configured field name that does not
/// name a known inheritable field, or whose value is absent, is skipped (first-present wins).
fn resolve_field<'a>(node: &'a AncestorNode, fields: &'a [String]) -> Option<(&'a str, &'a str)> {
    for field in fields {
        if field == FIELD_AGENT_CONTEXT
            && let Some(content) = node.agent_context.as_deref()
        {
            return Some((FIELD_AGENT_CONTEXT, content));
        }
    }
    None
}

/// Build an [`InheritedBlock`] for `node` in `role`, or `None` if it is tombstoned or has no
/// present configured field.
fn block_for(node: &AncestorNode, role: &str, fields: &[String]) -> Option<InheritedBlock> {
    if node.is_tombstone {
        return None;
    }
    let (field_used, content) = resolve_field(node, fields)?;
    Some(InheritedBlock {
        source_id: node.id.clone(),
        source_role: role.to_string(),
        source_title: node.title.clone(),
        field_used: field_used.to_string(),
        content: content.to_string(),
    })
}

/// The role string for the root bookend, given its issue type.
fn root_role(node: &AncestorNode) -> &'static str {
    if node.issue_type == IssueType::Epic {
        ROLE_EPIC
    } else {
        ROLE_ROOT
    }
}

/// Select up to two inherited-context bookends (immediate parent + root/epic) from an ancestor
/// chain (plan §2 `inheritance.rs`). **Infallible**; deterministic for a fixed input.
///
/// - `!cfg.enabled` → empty.
/// - empty `chain` → empty.
/// - one-element chain (parent == root) → at most **one** block, taking the root role
///   (`"epic"` if the node is an epic, else `"root"`).
/// - otherwise → at most **two** blocks: the immediate parent (`chain[0]`, role `"parent"`) and the
///   root (`chain[last]`, role `"epic"`/`"root"`), each skipped if tombstoned or field-less.
///
/// The output preserves parent-before-root order and is never longer than two.
///
/// # Examples
///
/// ```
/// use unblock_policy::{select_inherited_blocks, AncestorNode, InheritanceConfig};
/// use unblock_model::IssueType;
///
/// let chain = vec![
///     AncestorNode { id: "ub-parent".into(), issue_type: IssueType::Task,
///         title: "Parent".into(), agent_context: Some("ctx-p".into()), is_tombstone: false },
///     AncestorNode { id: "ub-root".into(), issue_type: IssueType::Epic,
///         title: "Root epic".into(), agent_context: Some("ctx-r".into()), is_tombstone: false },
/// ];
/// let cfg = InheritanceConfig { enabled: true, fields: vec!["agent_context".into()] };
/// let blocks = select_inherited_blocks(&chain, &cfg);
/// assert_eq!(blocks.len(), 2);
/// assert_eq!(blocks[0].source_role, "parent");
/// assert_eq!(blocks[1].source_role, "epic");
/// ```
#[must_use]
pub fn select_inherited_blocks(
    chain: &[AncestorNode],
    cfg: &InheritanceConfig,
) -> Vec<InheritedBlock> {
    if !cfg.enabled {
        return Vec::new();
    }

    match chain {
        [] => Vec::new(),
        // parent == root: a single bookend, taking the root role.
        [only] => block_for(only, root_role(only), &cfg.fields)
            .into_iter()
            .collect(),
        // Distinct parent (first) and root (last) bookends.
        [parent, .., root] => {
            let mut blocks = Vec::with_capacity(2);
            if let Some(block) = block_for(parent, ROLE_PARENT, &cfg.fields) {
                blocks.push(block);
            }
            if let Some(block) = block_for(root, root_role(root), &cfg.fields) {
                blocks.push(block);
            }
            blocks
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AncestorNode, InheritanceConfig, select_inherited_blocks};
    use unblock_model::IssueType;

    fn node(id: &str, ty: IssueType, ctx: Option<&str>, tombstone: bool) -> AncestorNode {
        AncestorNode {
            id: id.to_string(),
            issue_type: ty,
            title: format!("title-{id}"),
            agent_context: ctx.map(str::to_string),
            is_tombstone: tombstone,
        }
    }

    fn cfg() -> InheritanceConfig {
        InheritanceConfig {
            enabled: true,
            fields: vec!["agent_context".to_string()],
        }
    }

    #[test]
    fn disabled_yields_empty() {
        let chain = vec![node("ub-a", IssueType::Task, Some("c"), false)];
        let disabled = InheritanceConfig {
            enabled: false,
            fields: vec!["agent_context".to_string()],
        };
        assert!(select_inherited_blocks(&chain, &disabled).is_empty());
    }

    #[test]
    fn empty_chain_yields_empty() {
        assert!(select_inherited_blocks(&[], &cfg()).is_empty());
    }

    #[test]
    fn single_node_is_one_block_with_root_role() {
        let chain = vec![node("ub-root", IssueType::Task, Some("c"), false)];
        let blocks = select_inherited_blocks(&chain, &cfg());
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].source_role, "root");
        assert_eq!(blocks[0].source_id, "ub-root");
        assert_eq!(blocks[0].content, "c");
        assert_eq!(blocks[0].field_used, "agent_context");
    }

    #[test]
    fn single_epic_node_takes_epic_role() {
        let chain = vec![node("ub-root", IssueType::Epic, Some("c"), false)];
        let blocks = select_inherited_blocks(&chain, &cfg());
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].source_role, "epic");
    }

    #[test]
    fn parent_and_epic_root_two_blocks_in_order() {
        let chain = vec![
            node("ub-parent", IssueType::Task, Some("ctx-p"), false),
            node("ub-mid", IssueType::Task, Some("ctx-m"), false),
            node("ub-root", IssueType::Epic, Some("ctx-r"), false),
        ];
        let blocks = select_inherited_blocks(&chain, &cfg());
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].source_role, "parent");
        assert_eq!(blocks[0].source_id, "ub-parent");
        assert_eq!(blocks[1].source_role, "epic");
        assert_eq!(blocks[1].source_id, "ub-root");
    }

    #[test]
    fn non_epic_root_takes_root_role() {
        let chain = vec![
            node("ub-parent", IssueType::Task, Some("ctx-p"), false),
            node("ub-root", IssueType::Feature, Some("ctx-r"), false),
        ];
        let blocks = select_inherited_blocks(&chain, &cfg());
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[1].source_role, "root");
    }

    #[test]
    fn tombstoned_node_is_skipped() {
        let chain = vec![
            node("ub-parent", IssueType::Task, Some("ctx-p"), true), // tombstoned -> skipped
            node("ub-root", IssueType::Epic, Some("ctx-r"), false),
        ];
        let blocks = select_inherited_blocks(&chain, &cfg());
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].source_id, "ub-root");
        assert_eq!(blocks[0].source_role, "epic");
    }

    #[test]
    fn duplicate_id_chain_excludes_tombstone_parent() {
        // Storage never produces a chain with duplicate ids (id is a unique PK; an ancestor chain
        // walks distinct issues). This test pins that the selector is **position-based** (it picks
        // `chain[0]` as parent and `chain[last]` as root) and **tombstone-safe** even on a colliding
        // chain: a tombstoned parent and a non-tombstone root that happen to share id "ub-a" must
        // yield exactly the root's block, never the tombstone's. (The integration property's
        // find-by-id source lookup is ambiguous on such a chain, which is why the proptest generator
        // now enforces unique ids — but the production logic is robust regardless, as proven here.)
        let chain = vec![
            // parent (chain[0]): tombstoned, id collides with the root.
            node("ub-a", IssueType::Task, Some("ctx-p"), true),
            // root (chain[last]): non-tombstone epic, same id.
            node("ub-a", IssueType::Epic, Some("ctx"), false),
        ];
        let blocks = select_inherited_blocks(&chain, &cfg());
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].source_role, "epic");
        assert_eq!(blocks[0].source_id, "ub-a");
        assert_eq!(blocks[0].content, "ctx");
    }

    #[test]
    fn ancestor_without_configured_field_is_skipped() {
        let chain = vec![
            node("ub-parent", IssueType::Task, None, false), // no agent_context -> skipped
            node("ub-root", IssueType::Epic, Some("ctx-r"), false),
        ];
        let blocks = select_inherited_blocks(&chain, &cfg());
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].source_id, "ub-root");
    }

    #[test]
    fn field_list_first_present_wins() {
        // `design` is configured first but is not a v1 inheritable field; `agent_context` is, so it
        // wins when present.
        let chain = vec![node("ub-root", IssueType::Task, Some("ctx"), false)];
        let cfg = InheritanceConfig {
            enabled: true,
            fields: vec!["design".to_string(), "agent_context".to_string()],
        };
        let blocks = select_inherited_blocks(&chain, &cfg);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].field_used, "agent_context");
    }

    #[test]
    fn never_exceeds_two_blocks() {
        let chain: Vec<AncestorNode> = (0..10)
            .map(|i| node(&format!("ub-{i}"), IssueType::Task, Some("c"), false))
            .collect();
        assert!(select_inherited_blocks(&chain, &cfg()).len() <= 2);
    }

    #[test]
    fn deterministic_for_fixed_chain() {
        let chain = vec![
            node("ub-parent", IssueType::Task, Some("ctx-p"), false),
            node("ub-root", IssueType::Epic, Some("ctx-r"), false),
        ];
        assert_eq!(
            select_inherited_blocks(&chain, &cfg()),
            select_inherited_blocks(&chain, &cfg())
        );
    }
}
