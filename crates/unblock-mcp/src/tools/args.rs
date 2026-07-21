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

/// The accepted top-level argument names for `T`, **DERIVED from the published schema**.
///
/// Hand-maintained field lists rot silently; this reads the same `schema_for_type` output that
/// `#[tool]` publishes as the `inputSchema`, so the hint can never enumerate a set the wire does not
/// accept. Walks the root `properties` plus every `oneOf`/`anyOf`/`allOf` arm (resolving a `$ref`
/// into `$defs`), which covers our tagged-enum inputs and their flattened targets.
///
/// Only ever called on the error path, and `schema_for_type` is thread-local-cached, so the cost is
/// paid once per type per thread on the first rejection.
fn known_fields_of<T: JsonSchema + std::any::Any>() -> String {
    let schema = rmcp::handler::server::common::schema_for_type::<T>();
    let root = serde_json::Value::Object((*schema).clone());
    let mut names = std::collections::BTreeSet::new();
    collect_properties(&root, &root, &mut names);
    names.into_iter().collect::<Vec<_>>().join(", ")
}

/// Collect the `properties` keys of `node` and of every composition arm below it.
fn collect_properties(
    node: &serde_json::Value,
    root: &serde_json::Value,
    out: &mut std::collections::BTreeSet<String>,
) {
    if let Some(props) = node
        .get("properties")
        .and_then(serde_json::Value::as_object)
    {
        out.extend(props.keys().cloned());
    }
    if let Some(reference) = node.get("$ref").and_then(serde_json::Value::as_str)
        && let Some(name) = reference.strip_prefix("#/$defs/")
        && let Some(target) = root.get("$defs").and_then(|d| d.get(name))
    {
        collect_properties(target, root, out);
    }
    for key in ["oneOf", "anyOf", "allOf"] {
        if let Some(arms) = node.get(key).and_then(serde_json::Value::as_array) {
            for arm in arms {
                collect_properties(arm, root, out);
            }
        }
    }
}

/// Map a serde deserialization failure to the FR-11 in-band [`StructuredError`].
///
/// The echoed text is **clipped before** it reaches `StructuredError::from_code` (which sanitizes),
/// so an attacker-supplied 64 KiB field name cannot be amplified into the response (§3.5.3).
fn args_error(err: &serde_json::Error, known_fields: &str) -> StructuredError {
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
        "check the argument names against the tool schema; accepted fields: {known_fields}"
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
    serde_json::from_value::<T>(serde_json::Value::Object(raw))
        .map_err(|err| args_error(&err, &known_fields_of::<T>()))
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
