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

use std::borrow::Cow;
use std::fmt::Write as _;
use std::marker::PhantomData;

use rmcp::ErrorData;
use rmcp::handler::server::common::FromContextPart;
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::model::JsonObject;
use rmcp::schemars::JsonSchema;
use serde::de::DeserializeOwned;
use unblock_error::{ErrorCode, StructuredError};

/// The maximum number of bytes of attacker-controlled text echoed back into an error payload.
///
/// **This is a SOFT bound.** [`unblock_error::sanitize_message`] runs *after* the clip and escapes
/// control characters at up to ~6 bytes each (`\x1b` → `\u{1b}`), so the final `message` is bounded
/// at roughly `6 * MAX_ECHOED_BYTES` ≈ 768 B, not 128 B. Clipping BEFORE sanitizing is deliberate:
/// clipping after could cut inside an escape sequence and yield a misleading fragment.
pub(crate) const MAX_ECHOED_BYTES: usize = 128;

/// The marker appended to clipped text.
pub(crate) const TRUNCATION_MARKER: &str = "…[truncated]";

/// Clip attacker-controlled text to [`MAX_ECHOED_BYTES`] on a char boundary.
///
/// Returns the input borrowed when it already fits, so the common path allocates nothing.
pub(crate) fn clip(s: &str) -> Cow<'_, str> {
    if s.len() <= MAX_ECHOED_BYTES {
        return Cow::Borrowed(s);
    }
    let mut end = MAX_ECHOED_BYTES;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    Cow::Owned(format!("{}{TRUNCATION_MARKER}", &s[..end]))
}

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
    use super::{MAX_ECHOED_BYTES, TRUNCATION_MARKER, args_error, clip, field_from_serde_message};

    #[test]
    fn clip_leaves_short_text_untouched() {
        assert_eq!(clip("short"), "short");
    }

    #[test]
    fn clip_truncates_on_a_char_boundary() {
        let long = "é".repeat(500);
        let clipped = clip(&long);
        assert!(clipped.ends_with(TRUNCATION_MARKER));
        assert!(clipped.len() <= MAX_ECHOED_BYTES + TRUNCATION_MARKER.len());
        // Truncating mid-`é` would have produced invalid UTF-8 (a panic on the slice); reaching
        // here at all proves the boundary walk worked. Also assert the kept prefix is intact.
        let kept = clipped.strip_suffix(TRUNCATION_MARKER).expect("marker");
        assert!(
            kept.chars().all(|c| c == 'é'),
            "kept prefix must be whole chars"
        );
        assert_eq!(
            kept.len() % 2,
            0,
            "`é` is 2 bytes; a mid-char cut would be odd"
        );
    }

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

    /// **THE MF-2 INVARIANT, executed.** For every tool, every arm, and every field the hint
    /// enumerates for that arm: a call carrying `{tag: value, field: …}` must NOT be rejected as an
    /// `unknown_field`. A `type_mismatch` is fine — the point is that the field EXISTS.
    ///
    /// The pre-fix derivation unioned the root with every `oneOf`/`anyOf`/`allOf` arm, so
    /// `issue{action:"show"}` advertised all 35 fields across the 7 arms and 33 of the 37 arms
    /// over-stated. This test turns RED under any return to that shape.
    #[test]
    fn no_arm_hint_ever_enumerates_a_field_the_same_call_rejects() {
        // `claim` is the only non-union input; it has no arms and returns early. Its own field list
        // is covered by `the_non_union_input_enumerates_only_fields_it_accepts`.
        macro_rules! run {
            ($tool:expr, $ty:ty) => {
                (|| {
                    let index = super::hint_index_of::<$ty>();
                    let Some(tag) = index.tag.as_deref() else {
                        return;
                    };
                    assert!(!index.arms.is_empty(), "{} has no arms", $tool);
                    for (value, hint) in &index.arms {
                        for field in accepted_fields(hint) {
                            let mut raw = serde_json::Map::new();
                            raw.insert(tag.to_string(), serde_json::json!(value));
                            raw.insert(field.clone(), serde_json::json!("probe"));
                            if let Err(err) = super::parse_args::<$ty>(raw) {
                                assert_ne!(
                                    err.context["kind"], "unknown_field",
                                    "{} `{}`=`{}`: the hint advertises `{}`, which the SAME call \
                                     rejects — the exact defect D42 exists to kill",
                                    $tool, tag, value, field
                                );
                            }
                        }
                    }
                })();
            };
        }
        for_each_tool_input!(run);
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
