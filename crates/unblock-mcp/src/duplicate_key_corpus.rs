//! The D43 DUPLICATE-KEY case corpus — declared ONCE, consumed by every suite (TEST-ONLY).
//!
//! # Why this lives in `src/` behind `test-util` and not in one crate's `tests/`
//!
//! The corpus is consumed from **two crates**: `unblock-mcp`'s duplex suite and `unblock-cli`'s
//! raw-stdio suite (which spawns the real `unblock` binary and therefore cannot live here). A
//! `tests/` module in either crate is unreachable from the other, and two copies of a security
//! corpus is exactly the drift these rules forbid. It is `#[doc(hidden)]`, feature-gated, and never
//! compiled into a shipped build.
//!
//! # ⚠️ `arguments_text` IS THE TEST — never round-trip it through a serializer
//!
//! `serde_json::to_string` and `json!` structurally CANNOT emit a duplicate key: both build a `Map`
//! first, and the `Map` is where the collapse happens. Every cell's `arguments_text` is therefore a
//! raw string literal, spliced verbatim into the frame by [`raw_tools_call`]. The ONLY transformation
//! applied to it is [`instantiate`]'s literal `{ID}`/`{ID2}` placeholder substitution, so that a
//! cell can name a live, minted issue id without ever passing through serde.
//!
//! A green test that cannot express the input proves nothing.

use serde_json::Value;

use crate::tools::args::parse_args;

/// What a cell demonstrates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlipKind {
    /// The UNION DISCRIMINANT itself is duplicated: the frame reads as one action and executes
    /// another.
    TagFlip,
    /// A non-discriminant field is duplicated: the same action runs against a different target,
    /// actor, payload or blast radius than the text states.
    FieldSubstitution,
    /// The duplicate is nested below the top level of `arguments`.
    Nested,
    /// The duplicated key is spelled with a JSON escape, so the two key SPANS differ byte-wise while
    /// decoding equal. A raw-span comparator lets this through silently.
    EscapeEquivalent,
}

/// One duplicate-key case.
pub struct FlipCell {
    /// The stable cell id used in assertion messages.
    pub id: &'static str,
    /// The tool the frame targets.
    pub tool: &'static str,
    /// The RAW `arguments` object text, WITH the duplicate. Spliced verbatim; never serialized.
    pub arguments_text: &'static str,
    /// The arguments as a human READS them (first occurrence wins).
    pub shown: &'static str,
    /// The arguments as `serde_json` BUILDS them (last occurrence wins) — what GA executed.
    pub hidden: &'static str,
    /// The decoded duplicated key (the expected `context.field`).
    pub duplicated_key: &'static str,
    /// The expected `context.path`: an RFC 6901 pointer relative to `params`.
    pub pointer: &'static str,
    /// What the cell demonstrates.
    pub kind: FlipKind,
    /// Whether BOTH arms deserialize cleanly against the tool's published input type.
    ///
    /// Asserted against reality by the non-vacuity guard, so it cannot silently rot — in BOTH
    /// directions: a cell claiming `true` whose hidden arm stops parsing (a schema change) turns
    /// the guard RED instead of quietly degrading into "the duplicate was rejected for an
    /// unrelated reason", and a cell claiming `false` must really have a SHOWN arm the schema
    /// rejects and a HIDDEN arm it accepts (a `false` that nothing checks would be a flag that
    /// silently switches the guard off).
    pub both_arms_schema_clean: bool,
    /// Whether the SHOWN arm mutates the store (drives which control assertions a suite runs).
    pub shown_arm_mutates: bool,
}

/// The corpus.
///
/// # Coverage note — `comment` and `defer` carry no BOTH-ARMS-schema-clean TAG flip
///
/// A tag flip is only schema-clean **on both arms** when the two arms accept the SAME field set,
/// because every input container carries `#[serde(deny_unknown_fields)]` (D42) and the flattened
/// `Attribution` denies unknowns too. `comment`'s four arms are `add{issue_id,body}`,
/// `list{issue_id}`, `update{comment_id,body}`, `delete{comment_id}` — no two accept the same
/// fields. `defer`'s two arms differ by the required `until`. So for those two tools **no
/// both-arms-schema-clean tag flip is constructible**, and they are covered by a
/// [`FlipKind::FieldSubstitution`] cell, which is a real, schema-clean, harmful flip. (The other
/// five tagged tools DO carry a both-arms-clean tag flip.)
///
/// A **ONE-SIDED** tag flip is constructible for them, and it is the dangerous half — the arm that
/// EXECUTES is the schema-clean one — so BOTH are covered too, one cell each: `comment` by `T8`
/// and `defer` by `T9` below. (An earlier revision claimed the pair was covered while only
/// `comment` had a cell; `defer`'s flip was equally constructible and had none.)
pub const CELLS: &[FlipCell] = &[
    // -- §4.1 tag flips ---------------------------------------------------------------------
    FlipCell {
        id: "T1 issue{show->close}",
        tool: "issue",
        arguments_text: r#"{"action":"show","id":"{ID}","action":"close"}"#,
        shown: r#"{"action":"show","id":"{ID}"}"#,
        hidden: r#"{"action":"close","id":"{ID}"}"#,
        duplicated_key: "action",
        pointer: "/arguments",
        kind: FlipKind::TagFlip,
        both_arms_schema_clean: true,
        shown_arm_mutates: false,
    },
    FlipCell {
        id: "T3 dep{add->remove}",
        tool: "dep",
        arguments_text: r#"{"action":"add","issue_id":"{ID}","depends_on_id":"{ID2}","dep_type":"blocks","action":"remove"}"#,
        shown: r#"{"action":"add","issue_id":"{ID}","depends_on_id":"{ID2}","dep_type":"blocks"}"#,
        hidden: r#"{"action":"remove","issue_id":"{ID}","depends_on_id":"{ID2}","dep_type":"blocks"}"#,
        duplicated_key: "action",
        pointer: "/arguments",
        kind: FlipKind::TagFlip,
        both_arms_schema_clean: true,
        shown_arm_mutates: true,
    },
    FlipCell {
        id: "T5 sync{export->import}",
        tool: "sync",
        arguments_text: r#"{"action":"export","path":".unblock/dup-key-cell.jsonl","action":"import"}"#,
        shown: r#"{"action":"export","path":".unblock/dup-key-cell.jsonl"}"#,
        hidden: r#"{"action":"import","path":".unblock/dup-key-cell.jsonl"}"#,
        duplicated_key: "action",
        pointer: "/arguments",
        kind: FlipKind::TagFlip,
        both_arms_schema_clean: true,
        shown_arm_mutates: true,
    },
    FlipCell {
        id: "T6 query{ready->blocked}",
        tool: "query",
        arguments_text: r#"{"kind":"ready","kind":"blocked"}"#,
        shown: r#"{"kind":"ready"}"#,
        hidden: r#"{"kind":"blocked"}"#,
        duplicated_key: "kind",
        pointer: "/arguments",
        kind: FlipKind::TagFlip,
        both_arms_schema_clean: true,
        shown_arm_mutates: false,
    },
    FlipCell {
        id: "T7 diagnostics{stats->orphans}",
        tool: "diagnostics",
        arguments_text: r#"{"kind":"stats","kind":"orphans"}"#,
        shown: r#"{"kind":"stats"}"#,
        hidden: r#"{"kind":"orphans"}"#,
        duplicated_key: "kind",
        pointer: "/arguments",
        kind: FlipKind::TagFlip,
        both_arms_schema_clean: true,
        shown_arm_mutates: false,
    },
    // -- the two tools with no schema-clean tag flip (see the type-level note above) ----------
    FlipCell {
        id: "T2 comment{add body substitution}",
        tool: "comment",
        arguments_text: r#"{"action":"add","issue_id":"{ID}","body":"a harmless note","body":"INJECTED CONTENT"}"#,
        shown: r#"{"action":"add","issue_id":"{ID}","body":"a harmless note"}"#,
        hidden: r#"{"action":"add","issue_id":"{ID}","body":"INJECTED CONTENT"}"#,
        duplicated_key: "body",
        pointer: "/arguments",
        kind: FlipKind::FieldSubstitution,
        both_arms_schema_clean: true,
        shown_arm_mutates: true,
    },
    FlipCell {
        id: "T4 defer{until substitution}",
        tool: "defer",
        arguments_text: r#"{"action":"defer","id":"{ID}","until":"2030-01-01T00:00:00Z","until":"2999-12-31T23:59:59Z"}"#,
        shown: r#"{"action":"defer","id":"{ID}","until":"2030-01-01T00:00:00Z"}"#,
        hidden: r#"{"action":"defer","id":"{ID}","until":"2999-12-31T23:59:59Z"}"#,
        duplicated_key: "until",
        pointer: "/arguments",
        kind: FlipKind::FieldSubstitution,
        both_arms_schema_clean: true,
        shown_arm_mutates: true,
    },
    // -- a ONE-SIDED tag flip: only the HIDDEN (executing) arm is schema-clean -----------------
    //
    // The type-level note above says `comment` has no BOTH-arms-clean tag flip. It does have a
    // one-sided one, and this is the harmful orientation: the text reads as a read-only `list`,
    // while what `serde_json` BUILDS is a `delete` — an executing soft-redact.
    //
    // It also discriminates something no other cell does: WHERE the gate sits. The frame is
    // refused before `parse_args` ever runs, so a gate moved after the parse would hand this
    // (perfectly schema-clean) `delete` straight to the tool body.
    FlipCell {
        id: "T8 comment{list->delete, one-sided}",
        tool: "comment",
        arguments_text: r#"{"action":"list","comment_id":1,"action":"delete"}"#,
        shown: r#"{"action":"list","comment_id":1}"#,
        hidden: r#"{"action":"delete","comment_id":1}"#,
        duplicated_key: "action",
        pointer: "/arguments",
        kind: FlipKind::TagFlip,
        both_arms_schema_clean: false,
        shown_arm_mutates: false,
    },
    // `defer`'s one-sided tag flip — the second tool the type-level note names, and equally
    // constructible: the text reads as an `undefer` (clearing a defer) while `serde_json` BUILDS a
    // `defer` that hides the issue until the year 2999. The `until` member is what makes it
    // one-sided: it is REQUIRED by the executing arm and UNKNOWN to the shown one, which
    // `deny_unknown_fields` (D42) refuses. Without this cell the class was covered by `comment`
    // alone while the prose claimed both.
    FlipCell {
        id: "T9 defer{undefer->defer, one-sided}",
        tool: "defer",
        arguments_text: r#"{"action":"undefer","id":"{ID}","until":"2999-12-31T23:59:59Z","action":"defer"}"#,
        shown: r#"{"action":"undefer","id":"{ID}","until":"2999-12-31T23:59:59Z"}"#,
        hidden: r#"{"action":"defer","id":"{ID}","until":"2999-12-31T23:59:59Z"}"#,
        duplicated_key: "action",
        pointer: "/arguments",
        kind: FlipKind::TagFlip,
        both_arms_schema_clean: false,
        shown_arm_mutates: false,
    },
    // -- §4.2 `claim` (the 8th tool, non-union) ------------------------------------------------
    FlipCell {
        id: "C1 claim{target substitution}",
        tool: "claim",
        arguments_text: r#"{"id":"{ID}","assignee":"alice","id":"{ID2}"}"#,
        shown: r#"{"id":"{ID}","assignee":"alice"}"#,
        hidden: r#"{"id":"{ID2}","assignee":"alice"}"#,
        duplicated_key: "id",
        pointer: "/arguments",
        kind: FlipKind::FieldSubstitution,
        both_arms_schema_clean: true,
        shown_arm_mutates: true,
    },
    FlipCell {
        id: "C2 claim{actor substitution}",
        tool: "claim",
        arguments_text: r#"{"id":"{ID}","assignee":"alice","assignee":"mallory"}"#,
        shown: r#"{"id":"{ID}","assignee":"alice"}"#,
        hidden: r#"{"id":"{ID}","assignee":"mallory"}"#,
        duplicated_key: "assignee",
        pointer: "/arguments",
        kind: FlipKind::FieldSubstitution,
        both_arms_schema_clean: true,
        shown_arm_mutates: true,
    },
    // -- §4.3 non-tag flips that change target or blast radius --------------------------------
    FlipCell {
        id: "B1 issue{delete target substitution}",
        tool: "issue",
        arguments_text: r#"{"action":"delete","ids":["{ID}"],"ids":["{ID2}"]}"#,
        shown: r#"{"action":"delete","ids":["{ID}"]}"#,
        hidden: r#"{"action":"delete","ids":["{ID2}"]}"#,
        duplicated_key: "ids",
        pointer: "/arguments",
        kind: FlipKind::FieldSubstitution,
        both_arms_schema_clean: true,
        shown_arm_mutates: true,
    },
    FlipCell {
        id: "B2 issue{delete blast-radius escalation}",
        tool: "issue",
        arguments_text: r#"{"action":"delete","ids":["{ID}"],"mode":"dry_run","mode":"cascade"}"#,
        shown: r#"{"action":"delete","ids":["{ID}"],"mode":"dry_run"}"#,
        hidden: r#"{"action":"delete","ids":["{ID}"],"mode":"cascade"}"#,
        duplicated_key: "mode",
        pointer: "/arguments",
        kind: FlipKind::FieldSubstitution,
        both_arms_schema_clean: true,
        shown_arm_mutates: true,
    },
    FlipCell {
        id: "B3 issue{update target substitution}",
        tool: "issue",
        arguments_text: r#"{"action":"update","ids":["{ID}"],"priority":1,"ids":["{ID2}"]}"#,
        shown: r#"{"action":"update","ids":["{ID}"],"priority":1}"#,
        hidden: r#"{"action":"update","ids":["{ID2}"],"priority":1}"#,
        duplicated_key: "ids",
        pointer: "/arguments",
        kind: FlipKind::FieldSubstitution,
        both_arms_schema_clean: true,
        shown_arm_mutates: true,
    },
    // -- §4.4 nested (an object nested in an ARRAY, depth 3) -----------------------------------
    FlipCell {
        id: "N2 issue{create deps[0].dep_type}",
        tool: "issue",
        arguments_text: r#"{"action":"create","title":"nested dup","deps":[{"issue_id":"{ID}","depends_on_id":"{ID2}","dep_type":"blocks","dep_type":"discovered-from"}]}"#,
        shown: r#"{"action":"create","title":"nested dup","deps":[{"issue_id":"{ID}","depends_on_id":"{ID2}","dep_type":"blocks"}]}"#,
        hidden: r#"{"action":"create","title":"nested dup","deps":[{"issue_id":"{ID}","depends_on_id":"{ID2}","dep_type":"discovered-from"}]}"#,
        duplicated_key: "dep_type",
        pointer: "/arguments/deps/0",
        kind: FlipKind::Nested,
        both_arms_schema_clean: true,
        shown_arm_mutates: true,
    },
    // -- §4.5 escape equivalence, AT THE WIRE --------------------------------------------------
    //
    // The second key's SOURCE SPAN is the six characters `\`,`u`,`0`,`0`,`6`,`1` followed by
    // `ction` — byte-different from the bare `action`, DECODING equal. A raw-span comparator reports
    // this frame CLEAN and the flip executes.
    FlipCell {
        id: "E1 issue{escaped tag key}",
        tool: "issue",
        arguments_text: r#"{"action":"show","id":"{ID}","\u0061ction":"close"}"#,
        shown: r#"{"action":"show","id":"{ID}"}"#,
        hidden: r#"{"action":"close","id":"{ID}"}"#,
        duplicated_key: "action",
        pointer: "/arguments",
        kind: FlipKind::EscapeEquivalent,
        both_arms_schema_clean: true,
        shown_arm_mutates: false,
    },
];

/// A `tools/call` frame with `arguments_text` spliced in VERBATIM (no serde on the payload).
#[must_use]
pub fn raw_tools_call(id: i64, tool: &str, arguments_text: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"{tool}","arguments":{arguments_text}}}}}"#
    )
}

/// Substitute the `{ID}`/`{ID2}` placeholders with live, minted issue ids.
///
/// This is the ONLY transformation applied to a cell's raw text — a literal string replace, never a
/// parse-and-reserialize (which would delete the duplicate).
#[must_use]
pub fn instantiate(text: &str, first: &str, second: &str) -> String {
    text.replace("{ID2}", second).replace("{ID}", first)
}

/// Does `arguments` deserialize cleanly against `tool`'s published input type?
///
/// This is the non-vacuity oracle: it drives the SAME `parse_args` the tool bodies use, so a cell
/// claiming "both arms are schema-clean" is checked against the real schema rather than asserted.
///
/// # Errors
/// The structured error message when the payload does not match the tool's input type, or when
/// `tool` is not one of the eight published tools.
pub fn parses_as_tool_input(tool: &str, arguments: &Value) -> Result<(), String> {
    fn check<T>(raw: rmcp::model::JsonObject) -> Result<(), String>
    where
        T: serde::de::DeserializeOwned + rmcp::schemars::JsonSchema + std::any::Any,
    {
        parse_args::<T>(raw).map(|_| ()).map_err(|err| err.message)
    }

    let Value::Object(map) = arguments.clone() else {
        return Err("arguments must be a JSON object".to_string());
    };
    match tool {
        "issue" => check::<crate::tools::issue::IssueInput>(map),
        "comment" => check::<crate::tools::comment::CommentToolInput>(map),
        "dep" => check::<crate::tools::dep::DepToolInput>(map),
        "defer" => check::<crate::tools::defer::DeferInput>(map),
        "sync" => check::<crate::tools::sync::SyncInput>(map),
        "query" => check::<crate::tools::query::QueryInput>(map),
        "diagnostics" => check::<crate::tools::diagnostics::DiagnosticsInput>(map),
        "claim" => check::<crate::tools::claim::ClaimInput>(map),
        other => Err(format!("unknown tool `{other}`")),
    }
}

/// The distinct tool names the corpus covers.
#[must_use]
pub fn covered_tools() -> std::collections::BTreeSet<&'static str> {
    CELLS.iter().map(|cell| cell.tool).collect()
}
