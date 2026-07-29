//! The **argument seam** (D42) — a crate-local, DEFERRING `Parameters<T>` extractor.
//!
//! # ⚠️ THE NAME `Parameters` IS LOAD-BEARING. DO NOT RENAME IT. ⚠️
//!
//! `rmcp-macros` picks the type it publishes as a tool's `inputSchema` by matching the **last path
//! segment ident** of a handler argument against the literal ident `Parameters`
//! (`rmcp-macros-1.7.0/src/common.rs::find_parameters_type_in_sig`). If this type were called
//! `Deferred`, `RawArgs` or anything else, `#[tool]` would fall through to
//! `rmcp::handler::server::common::schema_for_empty_input()` and **every one of the 8 published
//! `inputSchema`s would silently collapse to `{"type":"object","properties":{}}`** — a total
//! contract blackout with no compile error and no test failure outside the contract suite.
//!
//! So this type deliberately **shadows** `rmcp::handler::server::wrapper::Parameters` at every
//! import site. That shadowing is the design, not an accident.
//!
//! # Why defer at all (facet (a))
//!
//! rmcp's `Parameters<P>` deserializes `P` **inside the extractor**
//! (`rmcp-1.7.0/src/handler/server/tool.rs:181-195`) and maps any serde failure to
//! `ErrorData::invalid_params` — an **out-of-band** JSON-RPC `-32602` with `data: null`. That
//! escapes the FR-11 in-band structured-error channel entirely (spine §5.6).
//!
//! This type stores the raw [`JsonObject`] **without deserializing**, so its `FromContextPart` impl
//! is **infallible** and the rmcp `-32602` arm is *structurally unreachable* for our 8 tools. The
//! typed parse then happens inside the tool body, via [`parse_args`], where a failure can be
//! returned in-band as a `StructuredError`.
//!
//! # Mitigations against the shadowing being "fixed" back, strongest first
//!
//! 1. **Compile-time (primary).** This type holds a `JsonObject` + `PhantomData<T>`, NOT a `T`.
//!    Swapping an import back to rmcp's `Parameters` yields a `T` at the destructuring site → a hard
//!    type error at that tool. Importing both is `E0252`.
//! 2. **Test-time.** `crates/unblock-cli/tests/error_channel.rs` asserts `resp["error"].is_none()`
//!    on every malformed-argument cell; the out-of-band arm fails all of them.
//! 3. **Docs.** This module doc + a pointed comment at each of the 8 import sites (weakest — never
//!    rely on it alone).
//!
//! The documented fallback, if a future review rejects the shadowing, is
//! `#[tool(input_schema = …)]` with a freely-named extractor — it works, but it duplicates the
//! schema expression at 8 sites with no compiler coupling.
//!
//! # What this seam CANNOT see (D43)
//!
//! *(Unrelated to the sentence just above about `#[tool(input_schema = …)]` "duplicating" a schema
//! expression — that is a different word wearing the same letters. This section is about DUPLICATE
//! JSON KEYS.)*
//!
//! [`Parameters::from_context_part`] takes `context.arguments` — a `serde_json::Map` **rmcp already
//! built** inside `from_slice::<ClientJsonRpcMessage>` while decoding the frame. A duplicated JSON
//! key is collapsed **last-wins** at that build, so the shadowed member is destroyed *before this
//! type exists*: neither the extractor, nor [`parse_args`], nor `#[serde(deny_unknown_fields)]` can
//! ever observe it. **No fix can live here.**
//!
//! Detection therefore lives in the scanning transport (`crates/unblock-mcp/src/wire.rs`), which
//! owns the read framing, and arrives as a verdict on `RequestContext.extensions`. The gate is
//! `crate::server::frame_scan_gate`, and **an ABSENT verdict rejects** — an empty `Extensions` is
//! the default state, so the opposite encoding would fail OPEN.
//!
//! **Do NOT use rmcp's `Extension<T>` extractor to read it.** Its absent arm is
//! `ErrorData::invalid_params(format!("missing extension {}", type_name::<T>()), None)` ⇒ a
//! **`-32602`** — exactly the out-of-band arm this whole seam exists to keep shut, and the one
//! `crates/unblock-cli/tests/error_channel.rs` pins closed. Read `context.extensions` directly.

use std::borrow::Cow;
use std::fmt::Write as _;
use std::marker::PhantomData;

use rmcp::ErrorData;
use rmcp::handler::server::common::FromContextPart;
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::model::JsonObject;
use rmcp::schemars::JsonSchema;
use serde::de::DeserializeOwned;
// `clip` + its two bound constants are SHARED (D43): they were crate-local here until the
// duplicate-key scan gave `unblock-sync`'s `bd` line parser the same attacker-echo problem. Two
// copies of a security helper is drift, so the single definition now lives beside the scanner in
// `unblock-error` (L0) — `crates/unblock-error/src/sanitize.rs`.
use unblock_error::{ErrorCode, StructuredError, clip};

/// The crate-local, **deferring** parameter extractor. See the module doc — **the name is
/// load-bearing and must stay `Parameters`.**
///
/// Holds the raw `arguments` object plus a `PhantomData<T>` that carries the schema type through to
/// `#[tool]`'s `schema_for_type::<Parameters<T>>()` call. `T` is never deserialized here.
pub(crate) struct Parameters<T>(pub(crate) JsonObject, pub(crate) PhantomData<T>);

/// Byte-identical `JsonSchema` delegation to `T` — exactly what rmcp's own wrapper does
/// (`rmcp-1.7.0/src/handler/server/wrapper/parameters.rs:48-56`). This is what makes the extractor
/// swap `CONTRACT_HASH`-neutral **on its own**: the published `inputSchema` is unchanged.
impl<T: JsonSchema> JsonSchema for Parameters<T> {
    fn schema_name() -> Cow<'static, str> {
        T::schema_name()
    }

    fn json_schema(generator: &mut rmcp::schemars::SchemaGenerator) -> rmcp::schemars::Schema {
        T::json_schema(generator)
    }
}

/// **Infallible** extraction — this is the whole point of the seam (facet (a)).
///
/// Takes `context.arguments` verbatim (absent → `{}`, so a `tools/call` with no `arguments` member
/// reaches the in-band channel instead of rmcp's out-of-band `missing field` `-32602`) and never
/// returns `Err`, so `rmcp`'s `invalid_params` arm cannot fire for our tools.
impl<S, T> FromContextPart<ToolCallContext<'_, S>> for Parameters<T> {
    fn from_context_part(context: &mut ToolCallContext<S>) -> Result<Self, ErrorData> {
        Ok(Self(
            context.arguments.take().unwrap_or_default(),
            PhantomData,
        ))
    }
}

/// Extract the offending field name out of a serde error message.
///
/// serde's own message is the BEST available source here: on an internally-tagged enum serde buffers
/// content through `ContentDeserializer` and erases path tracking, so `serde_path_to_error` would
/// report `"."` for 7 of our 8 tools — while the message still carries the field name verbatim
/// (``unknown field `bodyy` ``). That is why `serde_path_to_error` is deliberately NOT a dependency.
fn field_from_serde_message(message: &str) -> Option<String> {
    // `unknown field `x`, expected …` / `missing field `x`` / `invalid type: …, at field `x``
    for prefix in ["unknown field `", "missing field `", "field `"] {
        if let Some(rest) = message.split(prefix).nth(1)
            && let Some(name) = rest.split('`').next()
            && !name.is_empty()
        {
            return Some(name.to_string());
        }
    }
    None
}

/// Classify a serde failure for `context.kind`.
fn kind_from_serde_message(message: &str) -> &'static str {
    if message.starts_with("unknown field") {
        "unknown_field"
    } else if message.starts_with("missing field") {
        "missing_field"
    } else if message.starts_with("invalid type") || message.starts_with("invalid value") {
        "type_mismatch"
    } else {
        "malformed_arguments"
    }
}

/// The `$defs` map of a published schema, if it has one.
type Defs<'a> = Option<&'a serde_json::Map<String, serde_json::Value>>;
/// A JSON-Schema `properties` map.
type Props<'a> = &'a serde_json::Map<String, serde_json::Value>;

/// The maximum `$ref` hops followed when resolving a subschema. A `$defs` graph is not guaranteed
/// acyclic, and this walk runs on attacker-reachable input, so the recursion is HARD-bounded rather
/// than trusted.
const MAX_REF_HOPS: usize = 8;

/// The precomputed, per-type hint texts, **DERIVED from the published `inputSchema`**.
///
/// # Why this is not one flat field list
///
/// The previous shape unioned the root with EVERY `oneOf`/`anyOf`/`allOf` arm, so on a tagged-union
/// input it enumerated every field of every action. That reintroduced the exact defect D42 exists to
/// kill: `issue{action:"show", id, junk}` rejected `junk` while advertising all 35 fields across the
/// 7 arms — and the follow-up `issue{action:"show", id, markdown}` the hint invited was then
/// rejected by the same call. 33 of the 37 arms over-stated.
///
/// So the index is keyed by the DISCRIMINANT VALUE: the hint enumerates the MATCHED arm only. Per
/// the D42 analysis the hint is one of only two signals that survive a flattening MCP client, so a
/// hint that lists fields the wire rejects is worse than no hint.
struct HintIndex {
    /// The discriminant property name (`action` / `kind`), when the input is a tagged union.
    tag: Option<String>,
    /// Hint text per discriminant value.
    arms: std::collections::BTreeMap<String, String>,
    /// Used when the input is NOT a union, or when the tag is absent / not a string / unrecognized
    /// — in which case the actionable information is the accepted TAG VALUES, not any field list.
    fallback: String,
}

impl HintIndex {
    /// The hint text for `raw`.
    ///
    /// The returned `&str` borrows `self` ONLY (not `raw`), so the caller can look the hint up
    /// while it still owns the raw arguments and then move them into the deserializer — which is
    /// what keeps the OK path free of any extra allocation.
    fn hint_for<'a>(&'a self, raw: &JsonObject) -> &'a str {
        if let Some(tag) = self.tag.as_deref()
            && let Some(value) = raw.get(tag).and_then(serde_json::Value::as_str)
            && let Some(text) = self.arms.get(value)
        {
            return text;
        }
        &self.fallback
    }
}

/// Follow `$ref` hops into `$defs`, bounded by [`MAX_REF_HOPS`].
fn resolve<'a>(mut node: &'a serde_json::Value, defs: Defs<'a>) -> &'a serde_json::Value {
    for _ in 0..MAX_REF_HOPS {
        let Some(target) = node
            .get("$ref")
            .and_then(serde_json::Value::as_str)
            .and_then(|r| r.strip_prefix("#/$defs/"))
            .and_then(|name| defs?.get(name))
        else {
            return node;
        };
        node = target;
    }
    node
}

/// The `properties` map of a nested CONTAINER reachable through `sub`, plus the suffix that names
/// how it is reached (`"[]"` for an array element, `""` for a plain object).
///
/// **This is where `items` is traversed.** Without it a nested container is unreachable from the
/// hint: `deps:[{…,"metadataa":"LOST"}]` is correctly REJECTED by `DepInput`'s own
/// `deny_unknown_fields`, but the hint used to list the OUTER `issue` fields and omit `metadata`
/// entirely — telling the caller to fix a field that was never the problem.
fn nested_container<'a>(
    sub: &'a serde_json::Value,
    defs: Defs<'a>,
) -> Option<(&'static str, Props<'a>)> {
    let sub = resolve(sub, defs);
    if let Some(items) = sub.get("items") {
        let items = resolve(items, defs);
        return items
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .map(|p| ("[]", p));
    }
    if let Some(props) = sub.get("properties").and_then(serde_json::Value::as_object) {
        return Some(("", props));
    }
    // `Option<Nested>` renders as `anyOf: [{$ref: …}, {"type":"null"}]`.
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(alternatives) = sub.get(key).and_then(serde_json::Value::as_array) {
            for alternative in alternatives {
                if let Some(found) = nested_container(alternative, defs) {
                    return Some(found);
                }
            }
        }
    }
    None
}

/// Render the accepted-field clauses for one container: its own keys, then one clause per nested
/// container reachable from them.
fn field_clauses(props: Props<'_>, defs: Defs<'_>) -> String {
    let own = props.keys().cloned().collect::<Vec<_>>().join(", ");
    let mut text = format!("accepted fields: {own}");
    for (name, sub) in props {
        if let Some((suffix, nested)) = nested_container(sub, defs) {
            let inner = nested.keys().cloned().collect::<Vec<_>>().join(", ");
            let _ = write!(text, "; nested `{name}{suffix}` accepts: {inner}");
        }
    }
    text
}

/// The discriminant property name shared by every arm — the key that carries a string `const` in
/// ALL of them. Returns `None` if the arms do not agree, in which case no arm can be selected and
/// the index falls back rather than guessing.
fn discriminant_of(arms: &[serde_json::Value]) -> Option<String> {
    let first = arms.first()?.get("properties")?.as_object()?;
    let candidates = first
        .iter()
        .filter(|(_, sub)| {
            sub.get("const")
                .and_then(serde_json::Value::as_str)
                .is_some()
        })
        .map(|(name, _)| name.clone());
    candidates.into_iter().find(|candidate| {
        arms.iter().all(|arm| {
            arm.get("properties")
                .and_then(serde_json::Value::as_object)
                .and_then(|p| p.get(candidate))
                .and_then(|sub| sub.get("const"))
                .and_then(serde_json::Value::as_str)
                .is_some()
        })
    })
}

/// Build the [`HintIndex`] for a published schema, **walking the `Arc<JsonObject>` IN PLACE**.
///
/// The previous cut did `serde_json::Value::Object((*schema).clone())` — a deep clone of a
/// ~11 KB schema on EVERY rejection, measured at ~20 µs, i.e. ~100x the whole OK path and ~610 ns
/// per attacker byte on a 42-byte malformed call. Nothing here clones the schema.
fn build_hint_index(schema: &JsonObject) -> HintIndex {
    let defs = schema.get("$defs").and_then(serde_json::Value::as_object);
    let arms = schema.get("oneOf").and_then(serde_json::Value::as_array);

    let Some(arms) = arms.filter(|a| !a.is_empty()) else {
        let fallback = schema
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .map_or_else(
                || "this tool takes no arguments".to_string(),
                |props| field_clauses(props, defs),
            );
        return HintIndex {
            tag: None,
            arms: std::collections::BTreeMap::new(),
            fallback,
        };
    };

    let Some(tag) = discriminant_of(arms) else {
        // An untagged union: no arm can be selected from the payload, so enumerate nothing rather
        // than over-state. This is unreachable for the 8 shipped inputs (pinned by
        // `every_shipped_tool_input_has_a_selectable_discriminant`).
        return HintIndex {
            tag: None,
            arms: std::collections::BTreeMap::new(),
            fallback: "check the argument names against the tool schema".to_string(),
        };
    };

    let mut by_value = std::collections::BTreeMap::new();
    for arm in arms {
        let Some(props) = arm.get("properties").and_then(serde_json::Value::as_object) else {
            continue;
        };
        let Some(value) = props
            .get(&tag)
            .and_then(|sub| sub.get("const"))
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        by_value.insert(
            value.to_string(),
            format!("for `{tag}` = `{value}`, {}", field_clauses(props, defs)),
        );
    }

    let values = by_value.keys().cloned().collect::<Vec<_>>().join(", ");
    HintIndex {
        fallback: format!("`{tag}` must be one of: {values}"),
        tag: Some(tag),
        arms: by_value,
    }
}

/// The memoized [`HintIndex`] for `T`.
///
/// Mirrors the thread-local, `TypeId`-keyed memo `rmcp` uses for the schema itself
/// (`rmcp-1.7.0/src/handler/server/common.rs:12-16`): the schema walk, the `BTreeMap` build and
/// every hint string are produced ONCE per type per thread, so a rejection costs one map lookup and
/// one `Arc` clone rather than a schema clone plus a full re-walk.
fn hint_index_of<T: JsonSchema + std::any::Any>() -> std::sync::Arc<HintIndex> {
    thread_local! {
        static CACHE: std::cell::RefCell<
            std::collections::HashMap<std::any::TypeId, std::sync::Arc<HintIndex>>,
        > = std::cell::RefCell::new(std::collections::HashMap::new());
    }
    let key = std::any::TypeId::of::<T>();
    CACHE.with(|cache| {
        if let Some(index) = cache.borrow().get(&key) {
            return index.clone();
        }
        let index = std::sync::Arc::new(build_hint_index(
            &rmcp::handler::server::common::schema_for_type::<T>(),
        ));
        cache.borrow_mut().insert(key, index.clone());
        index
    })
}

/// Map a serde deserialization failure to the FR-11 in-band [`StructuredError`].
///
/// The echoed text is **clipped before** it reaches `StructuredError::from_code` (which sanitizes),
/// so an attacker-supplied 64 KiB field name cannot be amplified into the response (§3.5.3).
fn args_error(err: &serde_json::Error, hint: &str) -> StructuredError {
    let raw = err.to_string();
    let clipped = clip(&raw);
    let kind = kind_from_serde_message(&raw);
    let mut structured = StructuredError::from_code(
        ErrorCode::ValidationFailed,
        format!("invalid tool arguments: {clipped}"),
    )
    // The enumerating hint must be SYNTHESISED: serde DROPS its "expected one of" list on any
    // variant carrying `#[serde(flatten)]`, which is most of our arms. Per the D42 analysis the
    // hint and the field descriptions are the ONLY two levers that survive a flattening MCP client,
    // so this is load-bearing, not cosmetic.
    .with_hint(format!(
        "check the argument names against the tool schema; {hint}"
    ))
    .with_context("kind", serde_json::json!(kind));
    if let Some(field) = field_from_serde_message(&raw) {
        structured = structured.with_context("field", serde_json::json!(clip(&field).as_ref()));
    }
    structured
}

/// The D43 in-band reject for a frame whose `params` subtree carries a DUPLICATE JSON KEY.
///
/// Reuses [`ErrorCode::ValidationFailed`] — minting a variant would move `ErrorCode::ALL`, which
/// `capabilities().error_codes` is built from, which `CONTRACT_HASH` digests: a breaking contract
/// change under the GA freeze, for a patch-release defect fix. The duplicate-key KIND therefore
/// rides `context.kind`, a free-form slot that is hash-neutral.
///
/// Both echoed values are [`clip`]ped **before** they are attached: `from_code` sanitizes but does
/// not bound, and `with_context` does neither — the caller is the only bound on a `context` value.
/// `path` is clipped as an ALREADY-ASSEMBLED pointer (never per segment: many short segments could
/// otherwise still sum to an unbounded total).
pub(crate) fn duplicate_key_error(key: &str, path: &str) -> StructuredError {
    let key = clip(key);
    StructuredError::from_code(
        ErrorCode::ValidationFailed,
        format!("invalid tool arguments: duplicate JSON key `{key}` in the request `params`"),
    )
    .with_hint(format!(
        "a JSON object key may appear at most once; `{key}` appears more than once inside the \
         request, so the request was AMBIGUOUS and was NOT executed. Send each key exactly once \
         and resend."
    ))
    .with_context("kind", serde_json::json!("duplicate_key"))
    .with_context("field", serde_json::json!(key.as_ref()))
    // NEW slot: a nested duplicate is otherwise unlocatable — `field` alone cannot say WHICH
    // object carried it.
    .with_context("path", serde_json::json!(clip(path).as_ref()))
}

/// The D43 in-band reject for a frame the scanner could not resolve to a verdict.
///
/// Fail-closed: an ambiguous frame is refused, never waved through.
pub(crate) fn indeterminate_frame_error() -> StructuredError {
    StructuredError::from_code(
        ErrorCode::ValidationFailed,
        "tool arguments could not be unambiguously scanned; refusing to execute",
    )
    .with_hint(
        "the request bytes could not be tokenized to a duplicate-key decision (malformed JSON, \
         non-UTF-8, or nesting past the parser's depth limit). Send a well-formed request.",
    )
    .with_context("kind", serde_json::json!("indeterminate_frame"))
}

/// The D43 in-band reject for a frame that carries **no** verdict at all.
///
/// **This arm is the whole security property.** `Extensions` starts EMPTY, so "absent ⇒ clean"
/// would make any path that reaches a handler without traversing the scanning transport fail
/// **OPEN**. It mirrors the stance already written for the quota preflight: an un-measurable
/// request is rejected, because the untrusted-input boundary must never fail open.
///
/// It carries a `hint` like its two siblings: the hint is one of only two signals that survive a
/// flattening MCP client (the D42 analysis), and this is the one rejection a caller cannot act on
/// by editing its request — so saying WHOSE fault it is, is the only useful thing to say.
pub(crate) fn unscanned_frame_error() -> StructuredError {
    StructuredError::from_code(
        ErrorCode::InternalError,
        "tool arguments were not scanned for wire ambiguity; refusing to execute",
    )
    .with_hint(
        "this is a SERVER-side wiring fault, not a malformed request: the frame reached the tool \
         boundary without passing the duplicate-key scan, and an unscanned frame is refused rather \
         than trusted. Resending the same request will not help — report it against the server.",
    )
    .with_context("kind", serde_json::json!("unscanned_frame"))
}

/// Deserialize the raw arguments into `T`, mapping any failure to the in-band structured error.
///
/// This is the **only** deserialization of tool arguments in the process — it simply MOVED out of
/// rmcp's extractor into our boundary, where the contracted error shape is constructible.
///
/// # Errors
///
/// Returns a `VALIDATION_FAILED` [`StructuredError`] carrying a `hint` and
/// `context{kind, field}` when the payload does not match `T` — including an **unknown field**,
/// because every input container carries `#[serde(deny_unknown_fields)]`.
pub(crate) fn parse_args<T: DeserializeOwned + JsonSchema + std::any::Any>(
    raw: JsonObject,
) -> Result<T, StructuredError> {
    let index = hint_index_of::<T>();
    // `hint` borrows `index`, NOT `raw` — so `raw` can still be moved into the deserializer and the
    // OK path pays nothing beyond one memo lookup.
    let hint = index.hint_for(&raw);
    serde_json::from_value::<T>(serde_json::Value::Object(raw))
        .map_err(|err| args_error(&err, hint))
}

#[cfg(test)]
mod tests {
    // `clip`/`MAX_ECHOED_BYTES`/`TRUNCATION_MARKER` moved to `unblock-error` with the D43 scanner;
    // the helper's OWN unit cells moved with it. What stays here is what is about THIS seam: that
    // `args_error` actually applies the bound (see `oversized_field_name_is_clipped_in_message`).
    use super::{args_error, field_from_serde_message};
    use unblock_error::{MAX_ECHOED_BYTES, TRUNCATION_MARKER};

    #[test]
    fn field_is_extracted_from_the_serde_message() {
        assert_eq!(
            field_from_serde_message("unknown field `bodyy`, expected one of `a`, `b`").as_deref(),
            Some("bodyy")
        );
        assert_eq!(
            field_from_serde_message("missing field `body`").as_deref(),
            Some("body")
        );
        assert_eq!(field_from_serde_message("invalid type: integer"), None);
    }

    // --- MF-2: the hint must never advertise a field the same call rejects -----------------------

    /// The 8 shipped tool inputs, as `(tool name, input type)`. Every cell below runs over ALL of
    /// them, so a new tool cannot quietly skip the invariant.
    macro_rules! for_each_tool_input {
        ($mac:ident) => {
            $mac!("issue", crate::tools::issue::IssueInput);
            $mac!("claim", crate::tools::claim::ClaimInput);
            $mac!("defer", crate::tools::defer::DeferInput);
            $mac!("query", crate::tools::query::QueryInput);
            $mac!("dep", crate::tools::dep::DepToolInput);
            $mac!("sync", crate::tools::sync::SyncInput);
            $mac!("diagnostics", crate::tools::diagnostics::DiagnosticsInput);
            $mac!("comment", crate::tools::comment::CommentToolInput);
        };
    }

    // --- the probe corpus the MF-2 invariant is executed over ------------------------------------
    //
    // The first cut of this barrier built `{tag, field}` and NOTHING ELSE. On every arm that has a
    // required field beyond the discriminant, serde reported `missing field` BEFORE it ever visited
    // the probe key, so `assert_ne!(kind, "unknown_field")` passed without deciding anything:
    // 100 of 220 cells were vacuous (all of dep/add, dep/remove, comment/add, comment/update, most
    // of query/stale and issue/create). The masking was directly demonstrable —
    // `{action:"close", title:"probe"}` is NOT reported as `unknown_field`, while
    // `{action:"close", id:"ub-1", title:"probe"}` IS. The corpus below therefore fills each arm's
    // REQUIRED fields from the published schema first, and `every_probe_cell_is_conclusive` asserts
    // the mask rate stays at zero so the barrier cannot silently rot back.

    /// Recursion bound for [`sample`]. `$defs` is not guaranteed acyclic and this walk is driven by
    /// the published schema, so the depth is hard-bounded rather than trusted (same reasoning as
    /// the production `MAX_REF_HOPS`).
    const MAX_SAMPLE_DEPTH: usize = 8;

    /// One `(arm, field)` probe and what the wire actually decided about it.
    struct Cell {
        /// `tool action=value field=name` — the human-readable cell id.
        id: String,
        /// The probed field was reported back as an `unknown_field`: the hint advertised a name the
        /// SAME call rejects. This is the MF-2 defect.
        rejected: bool,
        /// The call failed on some OTHER field, so the probed field's acceptance was never decided.
        /// A masked cell proves nothing — it is the vacuity this corpus exists to eliminate.
        masked: bool,
    }

    /// A schema-shaped placeholder value for `schema`, so that filling a required field does not
    /// itself become the reason the parse fails.
    fn sample(
        schema: &serde_json::Value,
        defs: super::Defs<'_>,
        depth: usize,
    ) -> serde_json::Value {
        if depth == 0 {
            return serde_json::Value::Null;
        }
        let schema = super::resolve(schema, defs);
        if let Some(constant) = schema.get("const") {
            return constant.clone();
        }
        if let Some(first) = schema
            .get("enum")
            .and_then(serde_json::Value::as_array)
            .and_then(|values| values.iter().find(|value| !value.is_null()))
        {
            return first.clone();
        }
        let ty = match schema.get("type") {
            Some(serde_json::Value::String(name)) => Some(name.as_str()),
            Some(serde_json::Value::Array(names)) => names
                .iter()
                .filter_map(serde_json::Value::as_str)
                .find(|name| *name != "null"),
            _ => None,
        };
        let format = schema.get("format").and_then(serde_json::Value::as_str);
        match ty {
            // A `DateTime<Utc>` is a `string` whose deserializer rejects "probe" with a chrono
            // message that names NO field — which is exactly how `defer`, `query stale` and
            // `diagnostics changelog` stayed masked even after their required keys were filled.
            Some("string") if format == Some("date-time") => {
                serde_json::json!("2026-01-01T00:00:00Z")
            }
            Some("string") => serde_json::json!("probe"),
            Some("integer" | "number") => serde_json::json!(1),
            Some("boolean") => serde_json::json!(false),
            // An empty array satisfies every shipped `Vec<_>` without needing an element shape.
            Some("array") => serde_json::json!([]),
            Some("object") => serde_json::Value::Object(required_object(schema, defs, depth - 1)),
            _ => {
                // `Option<T>` / untagged alternations render as `anyOf`/`oneOf`; take the first
                // alternative that yields anything.
                for key in ["anyOf", "oneOf", "allOf"] {
                    if let Some(alternatives) =
                        schema.get(key).and_then(serde_json::Value::as_array)
                    {
                        for alternative in alternatives {
                            let candidate = sample(alternative, defs, depth - 1);
                            if !candidate.is_null() {
                                return candidate;
                            }
                        }
                    }
                }
                serde_json::Value::Null
            }
        }
    }

    /// Every REQUIRED property of `container`, filled with a schema-shaped [`sample`]. This is what
    /// carries a probe past serde's `missing field` short-circuit and into the field decision.
    fn required_object(
        container: &serde_json::Value,
        defs: super::Defs<'_>,
        depth: usize,
    ) -> serde_json::Map<String, serde_json::Value> {
        let props = container
            .get("properties")
            .and_then(serde_json::Value::as_object);
        let mut filled = serde_json::Map::new();
        for name in container
            .get("required")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
        {
            let value = props.and_then(|props| props.get(name)).map_or_else(
                || serde_json::json!("probe"),
                |sub| sample(sub, defs, depth),
            );
            filled.insert(name.to_string(), value);
        }
        filled
    }

    /// Run one probe and classify what the wire decided about `field`.
    fn probe<T: serde::de::DeserializeOwned + rmcp::schemars::JsonSchema + std::any::Any>(
        id: String,
        raw: serde_json::Map<String, serde_json::Value>,
        field: &str,
    ) -> Cell {
        probe_nested::<T>(id, raw, &[field])
    }

    /// As [`probe`], but the cell's subject is a PATH of names (the nested-container clause probes
    /// `deps` AND `deps[].metadata` in one call, and the hint is wrong if the wire rejects either).
    fn probe_nested<T: serde::de::DeserializeOwned + rmcp::schemars::JsonSchema + std::any::Any>(
        id: String,
        raw: serde_json::Map<String, serde_json::Value>,
        subject: &[&str],
    ) -> Cell {
        match super::parse_args::<T>(raw) {
            // The whole path was accepted outright — maximally conclusive.
            Ok(_) => Cell {
                id,
                rejected: false,
                masked: false,
            },
            Err(err) => {
                let blamed = err.context.get("field").and_then(serde_json::Value::as_str);
                let kind = err.context.get("kind").and_then(serde_json::Value::as_str);
                let on_subject = blamed.is_some_and(|name| subject.contains(&name));
                Cell {
                    id,
                    rejected: kind == Some("unknown_field") && on_subject,
                    // The parse died on something OUTSIDE the subject (or on nothing nameable), so
                    // this cell never decided whether the subject is accepted.
                    masked: !on_subject,
                }
            }
        }
    }

    /// Probe every field the hint enumerates for `T` — the arm's own clause AND every nested
    /// container clause (the "nested deps[] accepts: …" tail), which the first cut never touched
    /// because `accepted_fields` splits on a semicolon and takes `.next()`.
    ///
    /// The arm schemas are re-derived HERE, straight from the published schema, rather than read
    /// out of `HintIndex` — so a mutation to the production index cannot also move this oracle.
    fn probe_tool<T: serde::de::DeserializeOwned + rmcp::schemars::JsonSchema + std::any::Any>(
        tool: &str,
        cells: &mut Vec<Cell>,
    ) {
        let schema = rmcp::handler::server::common::schema_for_type::<T>();
        let defs = schema.get("$defs").and_then(serde_json::Value::as_object);
        let index = super::hint_index_of::<T>();
        let root = serde_json::Value::Object((*schema).clone());

        // (arm label, the arm's own schema, the hint rendered for it).
        let mut arms: Vec<(String, &serde_json::Value, &str)> = Vec::new();
        let by_value;
        if let Some(tag) = index.tag.as_deref() {
            assert!(!index.arms.is_empty(), "{tool} has no arms");
            let raw_arms = schema
                .get("oneOf")
                .and_then(serde_json::Value::as_array)
                .unwrap_or_else(|| panic!("{tool} publishes a tag `{tag}` but no `oneOf`"));
            by_value = raw_arms
                .iter()
                .filter_map(|arm| {
                    let value = arm
                        .get("properties")?
                        .get(tag)?
                        .get("const")?
                        .as_str()?
                        .to_string();
                    Some((value, arm))
                })
                .collect::<std::collections::BTreeMap<_, _>>();
            for (value, hint) in &index.arms {
                let arm = by_value
                    .get(value)
                    .unwrap_or_else(|| panic!("{tool}: no `{tag}`=`{value}` arm in the schema"));
                arms.push((format!("{tool} {tag}={value}"), arm, hint.as_str()));
            }
        } else {
            // `claim` — the only non-union outer. Its whole surface is the root object.
            arms.push((tool.to_string(), &root, index.fallback.as_str()));
        }

        for (label, arm, hint) in arms {
            let base = required_object(arm, defs, MAX_SAMPLE_DEPTH);
            let arm_props = arm.get("properties").and_then(serde_json::Value::as_object);

            for field in accepted_fields(hint) {
                let mut raw = base.clone();
                // Probe with a value shaped for the field WHEN the arm actually declares it. A
                // field the arm does NOT declare (which is exactly what a union-hint regression
                // advertises) falls back to a bare string and is rejected as `unknown_field`.
                let value = arm_props.and_then(|props| props.get(&field)).map_or_else(
                    || serde_json::json!("probe"),
                    |sub| sample(sub, defs, MAX_SAMPLE_DEPTH),
                );
                raw.insert(field.clone(), value);
                cells.push(probe::<T>(format!("{label} field={field}"), raw, &field));
            }

            for (container, fields) in nested_clauses(hint) {
                let (name, is_array) = container
                    .strip_suffix("[]")
                    .map_or_else(|| (container.as_str(), false), |stripped| (stripped, true));
                // The container may itself be absent from the arm — that is exactly what a
                // union-hint regression advertises — so it is PROBED, never asserted away: the
                // wire then blames the container name and the cell is recorded as rejected.
                let nested = arm_props
                    .and_then(|props| props.get(name))
                    .and_then(|sub| super::nested_container(sub, defs).map(|(_, p)| (sub, p)));
                let nested_schema = nested.map(|(sub, nested_props)| {
                    serde_json::Value::Object(
                        std::iter::once((
                            "properties".to_string(),
                            serde_json::Value::Object(nested_props.clone()),
                        ))
                        .chain(nested_required(sub, defs).map(|required| {
                            ("required".to_string(), serde_json::Value::Array(required))
                        }))
                        .collect(),
                    )
                });

                for field in fields {
                    let mut element = nested_schema
                        .as_ref()
                        .map(|schema| required_object(schema, defs, MAX_SAMPLE_DEPTH))
                        .unwrap_or_default();
                    let value = nested.and_then(|(_, props)| props.get(&field)).map_or_else(
                        || serde_json::json!("probe"),
                        |sub| sample(sub, defs, MAX_SAMPLE_DEPTH),
                    );
                    element.insert(field.clone(), value);
                    let element = serde_json::Value::Object(element);

                    let mut raw = base.clone();
                    raw.insert(
                        name.to_string(),
                        if is_array {
                            serde_json::Value::Array(vec![element])
                        } else {
                            element
                        },
                    );
                    // The hint is wrong if EITHER the container or the nested field is rejected.
                    cells.push(probe_nested::<T>(
                        format!("{label} nested `{container}` field={field}"),
                        raw,
                        &[name, field.as_str()],
                    ));
                }
            }
        }
    }

    /// The `required` array of the container a hint clause names (array element or plain object).
    fn nested_required(
        sub: &serde_json::Value,
        defs: super::Defs<'_>,
    ) -> Option<Vec<serde_json::Value>> {
        let sub = super::resolve(sub, defs);
        let container = sub
            .get("items")
            .map_or(sub, |items| super::resolve(items, defs));
        container
            .get("required")
            .and_then(serde_json::Value::as_array)
            .cloned()
    }

    /// The whole probe corpus, over all 8 shipped tool inputs.
    fn hint_probe_corpus() -> Vec<Cell> {
        let mut cells = Vec::new();
        macro_rules! run {
            ($tool:expr, $ty:ty) => {
                probe_tool::<$ty>($tool, &mut cells);
            };
        }
        for_each_tool_input!(run);
        cells
    }

    /// **THE MF-2 INVARIANT, executed.** For every tool, every arm, every field the hint enumerates
    /// for that arm — and every field of every NESTED container clause — a call carrying the arm's
    /// required fields plus that field must NOT be rejected as an `unknown_field`. A failure on
    /// some other field is fine for this cell's verdict; a `type_mismatch` on the probed field
    /// itself is fine too — the point is that the field EXISTS.
    ///
    /// The pre-fix derivation unioned the root with every `oneOf`/`anyOf`/`allOf` arm, so
    /// `issue{action:"show"}` advertised all 35 fields across the 7 arms and 33 of the 37 arms
    /// over-stated. This test turns RED under any return to that shape.
    #[test]
    fn no_arm_hint_ever_enumerates_a_field_the_same_call_rejects() {
        let rejected = hint_probe_corpus()
            .into_iter()
            .filter(|cell| cell.rejected)
            .map(|cell| cell.id)
            .collect::<Vec<_>>();
        assert!(
            rejected.is_empty(),
            "the hint advertises {} field(s) the SAME call rejects — the exact defect D42 exists \
             to kill:\n  {}",
            rejected.len(),
            rejected.join("\n  ")
        );
    }

    /// **The barrier on the barrier.** Every cell above must actually DECIDE the field it probes.
    /// Without this the corpus can rot back into vacuity by nothing more than a new required field
    /// on an arm: the parse would short-circuit on the missing key and
    /// `no_arm_hint_ever_enumerates_a_field_the_same_call_rejects` would pass on an empty question.
    #[test]
    fn every_probe_cell_is_conclusive() {
        let cells = hint_probe_corpus();
        let masked = cells
            .iter()
            .filter(|cell| cell.masked)
            .map(|cell| cell.id.clone())
            .collect::<Vec<_>>();
        assert!(
            masked.is_empty(),
            "{} of {} probe cells never reached the field decision, so the MF-2 invariant is \
             asserted on nothing there:\n  {}",
            masked.len(),
            cells.len(),
            masked.join("\n  ")
        );
        // Non-vacuity of the corpus ITSELF: 37 arms across 8 tools, plus the nested clauses.
        assert!(
            cells.len() >= 220,
            "the corpus shrank to {} cells — arms or hint clauses stopped being probed",
            cells.len()
        );
    }

    /// The non-union input (`claim`) enumerates its own fields, and every one is accepted.
    #[test]
    fn the_non_union_input_enumerates_only_fields_it_accepts() {
        let index = super::hint_index_of::<crate::tools::claim::ClaimInput>();
        assert!(index.tag.is_none(), "`claim` is the only non-enum outer");
        let fields = accepted_fields(&index.fallback);
        assert!(fields.contains(&"id".to_string()), "{}", index.fallback);
        for field in fields {
            let mut raw = serde_json::Map::new();
            raw.insert(field.clone(), serde_json::json!("probe"));
            if let Err(err) = super::parse_args::<crate::tools::claim::ClaimInput>(raw) {
                assert_ne!(err.context["kind"], "unknown_field", "claim: `{field}`");
            }
        }
    }

    /// NON-VACUITY + the concrete regression: `issue{action:"show"}` must enumerate the SHOW arm
    /// only. Pre-fix it listed all 35 fields across the 7 arms, including `markdown` — which the
    /// very next call with `{action:"show", markdown:"x"}` then rejected.
    #[test]
    fn the_show_arm_hint_is_the_show_arm_only() {
        let mut raw = serde_json::Map::new();
        raw.insert("action".into(), serde_json::json!("show"));
        raw.insert("id".into(), serde_json::json!("ub-1"));
        raw.insert("junk".into(), serde_json::json!(1));
        let err = super::parse_args::<crate::tools::issue::IssueInput>(raw)
            .expect_err("`junk` is an unknown field");
        let hint = err.hint.as_deref().expect("hint");
        assert_eq!(
            accepted_fields(hint),
            vec!["action".to_string(), "id".to_string()],
            "{hint}"
        );
        assert!(
            !hint.contains("markdown"),
            "the pre-fix hint advertised `markdown` on `show`, and the follow-up call it invited \
             was then rejected by the same tool: {hint}"
        );
    }

    /// **MF-2 (b): `items` traversal.** `deps:[{…,"metadataa":"LOST"}]` is correctly rejected by
    /// `DepInput`'s own `deny_unknown_fields`, but the pre-fix hint listed the OUTER `issue` fields
    /// and OMITTED `metadata` — pointing the caller at the wrong container entirely.
    #[test]
    fn a_nested_container_is_reachable_from_the_hint() {
        let mut raw = serde_json::Map::new();
        raw.insert("action".into(), serde_json::json!("create"));
        raw.insert("title".into(), serde_json::json!("t"));
        raw.insert(
            "deps".into(),
            serde_json::json!([{
                "issue_id": "a", "depends_on_id": "b", "dep_type": "blocks", "metadataa": "LOST"
            }]),
        );
        let err = super::parse_args::<crate::tools::issue::IssueInput>(raw)
            .expect_err("`metadataa` is an unknown field on DepInput");
        assert_eq!(err.context["field"], "metadataa");
        let hint = err.hint.as_deref().expect("hint");
        assert!(
            hint.contains("nested `deps[]` accepts:"),
            "the nested container must be named: {hint}"
        );
        for field in ["issue_id", "depends_on_id", "dep_type", "metadata"] {
            assert!(hint.contains(field), "`{field}` missing from: {hint}");
        }
    }

    /// An ABSENT / unrecognized discriminant yields the accepted TAG VALUES — which is the
    /// actionable information — never a union of every arm's fields.
    #[test]
    fn a_missing_or_unknown_tag_enumerates_the_accepted_tag_values() {
        for tag_value in [serde_json::json!("bogus_action"), serde_json::json!(7)] {
            let mut raw = serde_json::Map::new();
            raw.insert("action".into(), tag_value.clone());
            let err = super::parse_args::<crate::tools::issue::IssueInput>(raw)
                .expect_err("not a known action");
            let hint = err.hint.as_deref().expect("hint");
            assert!(hint.contains("`action` must be one of:"), "{hint}");
            assert!(hint.contains("create_bulk"), "{hint}");
            assert!(
                !hint.contains("markdown"),
                "field names must not leak into the tag-value hint: {hint}"
            );
        }
    }

    /// Every shipped union input must expose a SELECTABLE discriminant; otherwise `build_hint_index`
    /// silently degrades to the no-enumeration fallback and the hint stops being a signal at all.
    #[test]
    fn every_shipped_tool_input_has_a_selectable_discriminant() {
        macro_rules! run {
            ($tool:expr, $ty:ty) => {
                (|| {
                    let index = super::hint_index_of::<$ty>();
                    if $tool == "claim" {
                        assert!(index.tag.is_none(), "claim is the non-enum outer");
                        return;
                    }
                    let tag = index
                        .tag
                        .as_deref()
                        .unwrap_or_else(|| panic!("{} lost its discriminant", $tool));
                    assert!(tag == "action" || tag == "kind", "{}: {tag}", $tool);
                    assert!(!index.arms.is_empty(), "{}", $tool);
                })();
            };
        }
        for_each_tool_input!(run);
    }

    /// `nested_container` must see through an `anyOf` alternation — which is how schemars renders an
    /// `Option<NestedStruct>`. No SHIPPED input has that shape today, so without this cell the
    /// alternation branch is dead code and a future `Option<DepInput>` field would silently drop out
    /// of the hint. (Mutation M15.)
    #[test]
    fn a_nested_container_behind_an_any_of_is_still_found() {
        let schema = serde_json::json!({
            "anyOf": [{ "$ref": "#/$defs/Nested" }, { "type": "null" }]
        });
        let defs = serde_json::json!({
            "Nested": { "type": "object", "properties": { "alpha": {}, "beta": {} } }
        });
        let defs = defs.as_object();
        let (suffix, props) =
            super::nested_container(&schema, defs).expect("the anyOf alternative is a container");
        assert_eq!(suffix, "");
        assert_eq!(props.keys().collect::<Vec<_>>(), vec!["alpha", "beta"]);
    }

    /// The discriminant must be a key every arm agrees on. A key that carries a `const` in only SOME
    /// arms cannot select an arm from the payload, so taking the first const-bearing key of the
    /// first arm would mis-key the whole index. No shipped schema exercises the disagreement (the
    /// first const key IS the shared tag in all 8), so this is the only cell that covers it.
    /// (Mutation M17.)
    #[test]
    fn the_discriminant_must_be_agreed_by_every_arm() {
        let arms = vec![
            serde_json::json!({"properties": {
                "local": {"const": "only-here"}, "action": {"const": "a"}
            }}),
            serde_json::json!({"properties": { "action": {"const": "b"} }}),
        ];
        assert_eq!(
            super::discriminant_of(&arms).as_deref(),
            Some("action"),
            "`local` is const in ONE arm only and must not be chosen"
        );

        let no_agreement = vec![
            serde_json::json!({"properties": { "x": {"const": "a"} }}),
            serde_json::json!({"properties": { "y": {"const": "b"} }}),
        ];
        assert_eq!(
            super::discriminant_of(&no_agreement),
            None,
            "with no shared tag the index must fall back, never guess"
        );
    }

    /// The index is MEMOIZED per `TypeId`. Without this the schema walk, the `BTreeMap` build and
    /// every hint string are rebuilt on every rejection — the cost class MF-2 (c) exists to remove,
    /// and a purely non-functional property no other cell can observe. (Mutation M21.)
    #[test]
    fn the_hint_index_is_memoized_per_type() {
        let first = super::hint_index_of::<crate::tools::issue::IssueInput>();
        let second = super::hint_index_of::<crate::tools::issue::IssueInput>();
        assert!(
            std::sync::Arc::ptr_eq(&first, &second),
            "a rejection must not rebuild the index"
        );
        let other = super::hint_index_of::<crate::tools::claim::ClaimInput>();
        assert!(
            !std::sync::Arc::ptr_eq(&first.clone(), &other),
            "distinct types must not share an entry"
        );
    }

    /// Parse the field names out of a rendered hint clause (`…accepted fields: a, b, c…`).
    fn accepted_fields(hint: &str) -> Vec<String> {
        let Some(rest) = hint.split("accepted fields: ").nth(1) else {
            return Vec::new();
        };
        rest.split(';')
            .next()
            .unwrap_or_default()
            .split(", ")
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// Parse the NESTED clauses out of a rendered hint (each "nested … accepts: …" tail),
    /// returning `(container name + suffix, fields)`.
    ///
    /// `accepted_fields` deliberately stops at the first `';'`, so without this the nested clauses
    /// — the `items` traversal that MF-2 (b) added — were published but never probed.
    fn nested_clauses(hint: &str) -> Vec<(String, Vec<String>)> {
        hint.split("; nested `")
            .skip(1)
            .filter_map(|clause| {
                let (name, rest) = clause.split_once("` accepts: ")?;
                let fields = rest
                    .split(';')
                    .next()
                    .unwrap_or_default()
                    .split(", ")
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                Some((name.to_string(), fields))
            })
            .collect()
    }

    #[test]
    fn the_nested_clause_parser_reads_what_field_clauses_renders() {
        let clauses = nested_clauses(
            "for `action` = `create`, accepted fields: action, deps; \
             nested `deps[]` accepts: issue_id, metadata; nested `x` accepts: alpha",
        );
        assert_eq!(
            clauses,
            vec![
                (
                    "deps[]".to_string(),
                    vec!["issue_id".to_string(), "metadata".to_string()]
                ),
                ("x".to_string(), vec!["alpha".to_string()]),
            ]
        );
        assert!(nested_clauses("accepted fields: a, b").is_empty());
    }

    #[test]
    fn args_error_is_a_retryable_validation_failure_with_a_hint() {
        let err = serde_json::from_str::<std::collections::HashMap<String, u8>>("{\"a\":\"x\"}")
            .expect_err("type mismatch");
        let structured = args_error(&err, "a, b");
        assert_eq!(structured.code, unblock_error::ErrorCode::ValidationFailed);
        assert!(structured.retryable, "VALIDATION_FAILED is retryable");
        assert!(structured.hint.is_some_and(|h| h.contains("a, b")));
    }

    #[test]
    fn echoed_text_is_clipped_before_it_reaches_the_message() {
        let long_key = "z".repeat(4000);
        let err =
            serde_json::from_str::<super::super::dto::DepInput>(&format!("{{\"{long_key}\":1}}"))
                .expect_err("unknown field");
        let structured = args_error(&err, "issue_id");
        assert!(
            structured.message.len() <= 6 * MAX_ECHOED_BYTES + 64,
            "message soft-bound: {}",
            structured.message.len()
        );
        assert!(structured.message.contains(TRUNCATION_MARKER));
        assert!(
            structured.context["field"]
                .as_str()
                .is_some_and(|f| f.len() <= MAX_ECHOED_BYTES + TRUNCATION_MARKER.len())
        );
    }
}
