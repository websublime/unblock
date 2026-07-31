//! Shared input DTOs that flatten across tools + the input→model conversion glue (spine §5.2/§1.10).
//!
//! - [`Attribution`] — capture-only Tier-1 metadata on the wire (spine §5.2). It is **never
//!   enforced** here (the policy gate type is the distinct `AttributionPolicy`, G-23e).
//! - [`FilterInput`] — mirrors `ListFilters` (spine §1.10); [`FilterInput::into_list_filters`] is the
//!   total conversion into the engine-facing [`unblock_model::ListFilters`].
//! - [`DepInput`] — the create-arm edge input: an element of the `issue` tool's `create.deps` array,
//!   and nothing else. [`DepInput::into_new_dep`] maps it to the engine-owned, SOURCE-LESS
//!   [`unblock_engine::NewDep`], which the engine anchors on the id it mints (D44). There is
//!   deliberately NO conversion from this type to a model [`unblock_model::Dependency`] — see the
//!   type's own doc-comment.
//!
//! The result/display DTOs returned through the spine §5.3 per-tool outputs (`CountBucket`, `DepTree`,
//! `CloseOutcome`, `ExportReport`, `ImportReport`, `DiagnosticReport`) are the spine §1.10 types
//! **sourced from `unblock-model`** (CF-A/CF-B) and serialized as-is — never redefined here.

use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use unblock_engine::NewDep;
use unblock_model::{DependencyType, IssueType, ListFilters, Priority, Status};

/// MCP wire attribution — capture-only Tier-1 metadata (spine §5.2).
///
/// Distinct from the policy enforcement type (`AttributionPolicy`, G-23e): this `Attribution` is
/// mcp-owned and **never enforced**. It flattens into mutating tool inputs so an agent can
/// self-report `agent_name`/`harness`/`model`.
///
/// # ⚠️ It is NOT persisted — capture-only means capture-only
///
/// Every tool arm destructures this as `attribution: _`. Nothing downstream consumes it: the L2
/// event writer `crates/unblock-storage/src/libsql/events.rs:31` INSERTs **7** columns
/// (`issue_id`, `event_type`, `actor`, `old_value`, `new_value`, `comment`, `created_at`) and binds
/// **none** of `agent_name`/`harness`/`model`, while the read at `:62` DOES select them — so they
/// read back `NULL`. Accepting it on the wire and dropping it is a **deliberate v1 deferral**, to be
/// wired at L7 with **FR-22 [v1.1]**; tracked as `ub-lp9.22` and carved out at `docs/PRD.md` FR-20
/// (i). Do not restate this field as "recorded best-effort via the audit event" — that claim was
/// live here and false.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
// D42: `#[serde(deny_unknown_fields)]` — an unknown/misspelled argument is REJECTED in-band
// instead of being silently dropped. NOT recursive and inert on a flatten TARGET: every nested
// container needs its OWN attribute (see `tools/args.rs` + the CHECK-3 container guard).
#[serde(deny_unknown_fields)]
pub(crate) struct Attribution {
    /// Self-reported agent name (capture-only).
    #[serde(default)]
    pub agent_name: Option<String>,
    /// Self-reported harness identifier (capture-only).
    #[serde(default)]
    pub harness: Option<String>,
    /// Self-reported model identifier (capture-only).
    #[serde(default)]
    pub model: Option<String>,
}

/// A dependency edge declared on `issue create` (spine §5.2 `DepInput`).
///
/// It is an element of the `issue` tool's `create.deps` array and nothing else. The edge is sourced
/// on the issue that call is creating — the server mints that id (D21) — so the adapter maps this to
/// the engine-owned, SOURCE-LESS `NewDep` and the engine stamps the minted id, the actor and the
/// timestamp when it seeds the edge onto the new issue.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
// D42: `#[serde(deny_unknown_fields)]` — an unknown/misspelled argument is REJECTED in-band
// instead of being silently dropped. NOT recursive and inert on a flatten TARGET: every nested
// container needs its OWN attribute (see `tools/args.rs` + the CHECK-3 container guard).
#[serde(deny_unknown_fields)]
pub(crate) struct DepInput {
    /// The dependent issue id. **OPTIONAL, and on `issue create` it MUST be omitted.** The edge is
    /// sourced on the issue this call is creating, whose id the SERVER mints (D21) — so the field has
    /// exactly one correct value and the client cannot derive it. A PRESENT value is REJECTED with
    /// `VALIDATION_FAILED` and a field hint naming `deps[i].issue_id`; it is NEVER silently ignored
    /// and NEVER applied to the issue it names. An explicit JSON `null` is an ABSENCE, and is
    /// accepted.
    #[serde(default)]
    pub issue_id: Option<String>,
    /// The blocker issue id (target).
    pub depends_on_id: String,
    /// The dependency type.
    pub dep_type: DependencyType,
    /// Optional JSON metadata for the edge.
    #[serde(default)]
    pub metadata: Option<String>,
}

impl DepInput {
    /// Map this wire edge to the engine-owned, SOURCE-LESS [`NewDep`] (D44, spine §4.1/§5.2).
    ///
    /// `issue_id` is DROPPED here and is never read for an edge source on any path: it exists on the
    /// wire only so a create that carries one can be REJECTED in-band by the `issue` tool's create
    /// arm (which runs BEFORE this mapping), rather than have the value silently ignored. Because
    /// [`NewDep`] cannot carry a source at all, a foreign anchor is unrepresentable below L7.
    ///
    /// `created_at`/`created_by` are deliberately NOT supplied here: on the create arm the ENGINE
    /// stamps them from the session actor when it seeds `Issue.dependencies`, because only the engine
    /// knows the minted id they belong to.
    pub(crate) fn into_new_dep(self) -> NewDep {
        let Self {
            issue_id: _,
            depends_on_id,
            dep_type,
            metadata,
        } = self;
        NewDep {
            depends_on_id,
            dep_type,
            metadata,
        }
    }
}

/// Query/list filters on the wire — mirrors [`ListFilters`] (spine §1.10/§5.2).
///
/// Every field is `#[serde(default)]` so a bare `{}` filter is valid (an unconstrained list). The
/// total mapping [`FilterInput::into_list_filters`] is lossless on the supported fields.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
// D42: `#[serde(deny_unknown_fields)]` — an unknown/misspelled argument is REJECTED in-band
// instead of being silently dropped. NOT recursive and inert on a flatten TARGET: every nested
// container needs its OWN attribute (see `tools/args.rs` + the CHECK-3 container guard).
#[serde(deny_unknown_fields)]
pub(crate) struct FilterInput {
    /// Status OR-set (a match on ANY listed status).
    #[serde(default)]
    pub status: Vec<Status>,
    /// Issue-type OR-set.
    #[serde(default)]
    pub issue_type: Vec<IssueType>,
    /// Assignee equality filter.
    #[serde(default)]
    pub assignee: Option<String>,
    /// Labels that must ALL be present (AND).
    #[serde(default)]
    pub labels_all: Vec<String>,
    /// Labels of which ANY may be present (OR).
    #[serde(default)]
    pub labels_any: Vec<String>,
    /// Minimum priority (inclusive).
    #[serde(default)]
    pub priority_min: Option<Priority>,
    /// Maximum priority (inclusive).
    #[serde(default)]
    pub priority_max: Option<Priority>,
    /// Substring match over title/description.
    #[serde(default)]
    pub text_contains: Option<String>,
    /// Include deferred issues.
    #[serde(default)]
    pub include_deferred: bool,
    /// Include closed/tombstone issues.
    #[serde(default)]
    pub include_closed: bool,
    /// Result cap (`None` = unlimited; `ready` is default-complete).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Result offset.
    #[serde(default)]
    pub offset: Option<usize>,
}

impl FilterInput {
    /// Total, lossless conversion into the engine-facing [`ListFilters`] (spine §1.10).
    pub(crate) fn into_list_filters(self) -> ListFilters {
        ListFilters {
            status: self.status,
            issue_type: self.issue_type,
            assignee: self.assignee,
            labels_all: self.labels_all,
            labels_any: self.labels_any,
            priority_min: self.priority_min,
            priority_max: self.priority_max,
            text_contains: self.text_contains,
            include_deferred: self.include_deferred,
            include_closed: self.include_closed,
            // `include_tombstone` never rides the wire (FORK-1/D23, spine §5.2 non-mirror): no
            // query TOOL sets it (its in-process consumers are the sync export + the mcp
            // `issues/{id}` not-found scan, T2.6/D25), so `FilterInput` deliberately does NOT carry
            // it — always `false` here.
            include_tombstone: false,
            limit: self.limit,
            offset: self.offset,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Attribution, FilterInput};
    use unblock_model::{Priority, Status};

    #[test]
    fn attribution_default_is_all_none() {
        let a = Attribution::default();
        assert!(a.agent_name.is_none() && a.harness.is_none() && a.model.is_none());
    }

    #[test]
    fn attribution_flattens_from_partial_json() {
        let a: Attribution = serde_json::from_value(serde_json::json!({
            "agent_name": "claude"
        }))
        .expect("parse");
        assert_eq!(a.agent_name.as_deref(), Some("claude"));
        assert!(a.harness.is_none());
    }

    #[test]
    fn filter_input_maps_every_field_losslessly() {
        let input = FilterInput {
            status: vec![Status::Open],
            assignee: Some("a".to_string()),
            labels_all: vec!["x".to_string()],
            priority_min: Some(Priority::HIGH),
            include_deferred: true,
            limit: Some(7),
            ..FilterInput::default()
        };
        let filters = input.into_list_filters();
        assert_eq!(filters.status, vec![Status::Open]);
        assert_eq!(filters.assignee.as_deref(), Some("a"));
        assert_eq!(filters.labels_all, vec!["x".to_string()]);
        assert_eq!(filters.priority_min, Some(Priority::HIGH));
        assert!(filters.include_deferred);
        assert_eq!(filters.limit, Some(7));
    }
}
