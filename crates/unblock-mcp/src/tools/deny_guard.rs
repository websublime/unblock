//! **The done-condition for D42's unknown-field rejection: a RUNTIME guard, one case per input
//! container.**
//!
//! # Why this is a runtime test and not a source-text check
//!
//! A grep/regex for `deny_unknown_fields` over the source **cannot decide this property**. The
//! counter-example is real and cheap to write:
//!
//! ```ignore
//! #[serde(expecting = "x" /* deny_unknown_fields */)]
//! ```
//!
//! That line satisfies every plausible "the attribute is present" regex while serde happily accepts
//! unknown keys. Any static gate over these files is therefore vacuous by construction.
//!
//! # The shape of each case, and why BOTH halves are mandatory
//!
//! Every container gets a pair:
//!
//! - **REJECT** — `serde_json::from_str::<T>` on a payload carrying an unknown key must return `Err`.
//! - **ACCEPT** — `serde_json::from_str::<T>` on the *equivalent well-formed* payload must return
//!   `Ok`.
//!
//! **The ACCEPT half is the non-vacuity guard.** Without it a case passes for the wrong reason: if
//! the fixture never deserializes at all (a renamed field, a changed tag, a wrong variant name), the
//! REJECT half is satisfied by an error that has nothing to do with unknown-field denial, and the
//! test reports green while facet (b) is wide open.
//!
//! A missing attribute on ANY container in this table turns its REJECT case RED. That is the
//! observable both-directions property the mutation table exercises, one row per container.
//!
//! # What these cases do NOT model — and why a duplicate-key case here is FORBIDDEN as evidence
//!
//! Every case below drives `serde_json::from_str::<T>` on a **string**. Production never does that:
//! it reaches `T` from an **already-parsed** `JsonObject` that rmcp built while decoding the frame,
//! downstream of the transport. For unknown-field denial the two are equivalent, which is what makes
//! this file valid evidence for D42.
//!
//! For a DUPLICATE JSON KEY they are **not** equivalent, and the difference is the whole defect: the
//! collapse happens while the `Map` is BUILT, so a case written here would exercise a `from_str` this
//! seam never performs and would pass or fail for a reason unrelated to production. Such a case is
//! therefore forbidden as evidence. The duplicate-key regression lives where the bytes are — at the
//! wire (`crates/unblock-cli/tests/duplicate_key_frames.rs`) and at the duplex verdict level
//! (`crates/unblock-mcp/tests/duplicate_key_duplex.rs`) — under D43. This is a sibling caveat to the
//! "a static grep cannot decide the deny property" argument above: the wrong harness is as vacuous
//! as the wrong technique.

#![cfg(test)]

use super::claim::ClaimInput;
use super::comment::CommentToolInput;
use super::defer::DeferInput;
use super::dep::DepToolInput;
use super::diagnostics::DiagnosticsInput;
use super::dto::{Attribution, DepInput, FilterInput};
use super::issue::{CreateInput, IssueInput, PatchInput};
use super::query::QueryInput;
use super::sync::SyncInput;

/// Assert the container both REJECTS the unknown-key payload and ACCEPTS the well-formed one.
///
/// `T` is named only for the failure message; the two payloads must be the *same* document apart
/// from the unknown key, or the accept half stops guarding anything.
fn assert_denies_unknown_fields<T: serde::de::DeserializeOwned>(
    name: &str,
    with_unknown: &str,
    well_formed: &str,
) {
    // Non-vacuity FIRST: if this fails, the fixture is wrong and the reject half proves nothing.
    let accepted = serde_json::from_str::<T>(well_formed);
    assert!(
        accepted.is_ok(),
        "{name}: the well-formed payload MUST deserialize, otherwise the reject case below is \
         vacuous (it would pass on any error at all). serde said: {:?}",
        accepted.err().map(|e| e.to_string())
    );

    let rejected = serde_json::from_str::<T>(with_unknown);
    let err = rejected.err().unwrap_or_else(|| {
        panic!(
            "{name}: an UNKNOWN FIELD was silently ACCEPTED — `#[serde(deny_unknown_fields)]` is \
             missing or ineffective on this container (facet (b) is live here)"
        )
    });
    let message = err.to_string();
    assert!(
        message.contains("unknown field"),
        "{name}: rejected, but not as an unknown field — the fixture likely differs from the \
         well-formed payload in more than the unknown key. serde said: {message}"
    );
}

// --- the 8 OUTER containers (one per published tool) ---------------------------------------------

#[test]
fn issue_input_denies_unknown_fields() {
    assert_denies_unknown_fields::<IssueInput>(
        "IssueInput",
        r#"{"action":"show","id":"ub-1","_junk":"X"}"#,
        r#"{"action":"show","id":"ub-1"}"#,
    );
}

#[test]
fn claim_input_denies_unknown_fields() {
    assert_denies_unknown_fields::<ClaimInput>(
        "ClaimInput",
        r#"{"id":"ub-1","assignee":"a","assignie":"typo"}"#,
        r#"{"id":"ub-1","assignee":"a"}"#,
    );
}

#[test]
fn defer_input_denies_unknown_fields() {
    assert_denies_unknown_fields::<DeferInput>(
        "DeferInput",
        r#"{"action":"defer","id":"ub-1","until":"2030-01-01T00:00:00Z","untill":"x"}"#,
        r#"{"action":"defer","id":"ub-1","until":"2030-01-01T00:00:00Z"}"#,
    );
}

#[test]
fn dep_tool_input_denies_unknown_fields() {
    assert_denies_unknown_fields::<DepToolInput>(
        "DepToolInput",
        r#"{"action":"add","issue_id":"ub-1","depends_on_id":"ub-2","dep_type":"blocks","typo":1}"#,
        r#"{"action":"add","issue_id":"ub-1","depends_on_id":"ub-2","dep_type":"blocks"}"#,
    );
}

#[test]
fn diagnostics_input_denies_unknown_fields() {
    assert_denies_unknown_fields::<DiagnosticsInput>(
        "DiagnosticsInput",
        r#"{"kind":"changelog","sinse":"2030-01-01T00:00:00Z"}"#,
        r#"{"kind":"changelog"}"#,
    );
}

#[test]
fn query_input_denies_unknown_fields() {
    assert_denies_unknown_fields::<QueryInput>(
        "QueryInput",
        r#"{"kind":"ready","assignie":"a"}"#,
        r#"{"kind":"ready","assignee":"a"}"#,
    );
}

#[test]
fn sync_input_denies_unknown_fields() {
    assert_denies_unknown_fields::<SyncInput>(
        "SyncInput",
        r#"{"action":"import","path":"x.jsonl","dry_runn":true}"#,
        r#"{"action":"import","path":"x.jsonl","dry_run":true}"#,
    );
}

#[test]
fn comment_tool_input_denies_unknown_fields() {
    assert_denies_unknown_fields::<CommentToolInput>(
        "CommentToolInput",
        r#"{"action":"add","issue_id":"ub-1","body":"b","bodyy":"LOST"}"#,
        r#"{"action":"add","issue_id":"ub-1","body":"b"}"#,
    );
}

// --- the 5 NESTED containers ---------------------------------------------------------------------
//
// `deny_unknown_fields` is NOT recursive and is inert on a flatten TARGET, so each of these needs —
// and gets — its own attribute. These are the cases the 8 above cannot cover.

#[test]
fn create_input_denies_unknown_fields_through_the_enum() {
    // THE highest-blast-radius case. The enum-level attribute on `IssueInput` does NOT reach the
    // newtype variant `Create(Box<CreateInput>)`: verified by execution — with `CreateInput`'s own
    // attribute removed, `{"action":"create","title":"t","descriptionn":"LOST"}` deserializes `Ok`
    // while every other arm still rejects. So this case is driven THROUGH `IssueInput`, which is the
    // only way it discriminates.
    assert_denies_unknown_fields::<IssueInput>(
        "IssueInput::Create -> CreateInput",
        r#"{"action":"create","title":"t","descriptionn":"LOST"}"#,
        r#"{"action":"create","title":"t","description":"kept"}"#,
    );
}

#[test]
fn create_input_denies_unknown_fields_directly() {
    assert_denies_unknown_fields::<CreateInput>(
        "CreateInput",
        r#"{"title":"t","descriptionn":"LOST"}"#,
        r#"{"title":"t","description":"kept"}"#,
    );
}

#[test]
fn create_input_still_accepts_its_flattened_attribution() {
    // Non-vacuity for the flatten interaction: a denying container that carries `#[serde(flatten)]`
    // must still accept the flattened target's legitimate keys. If this ever goes RED the attribute
    // placement broke the highest-traffic action while every rejection case stayed green.
    let parsed = serde_json::from_str::<CreateInput>(r#"{"title":"t","agent_name":"claude"}"#)
        .expect("a flattened Attribution key must still be accepted");
    assert_eq!(parsed.attribution.agent_name.as_deref(), Some("claude"));
}

#[test]
fn patch_input_denies_unknown_fields_through_the_enum() {
    assert_denies_unknown_fields::<IssueInput>(
        "IssueInput::Update -> PatchInput",
        r#"{"action":"update","ids":["ub-1"],"titlee":"x"}"#,
        r#"{"action":"update","ids":["ub-1"],"title":"x"}"#,
    );
}

#[test]
fn patch_input_denies_unknown_fields_directly() {
    assert_denies_unknown_fields::<PatchInput>(
        "PatchInput",
        r#"{"titlee":"x"}"#,
        r#"{"title":"x"}"#,
    );
}

#[test]
fn dep_input_denies_unknown_fields_nested_in_create() {
    // The nested-deny regression: `deny_unknown_fields` is NOT recursive, so `CreateInput`'s
    // attribute does nothing for the elements of `deps`. `DepInput` needs its own.
    assert_denies_unknown_fields::<IssueInput>(
        "CreateInput.deps -> DepInput",
        r#"{"action":"create","title":"t","deps":[{"issue_id":"a","depends_on_id":"b","dep_type":"blocks","metadataa":"LOST"}]}"#,
        r#"{"action":"create","title":"t","deps":[{"issue_id":"a","depends_on_id":"b","dep_type":"blocks","metadata":"kept"}]}"#,
    );
}

#[test]
fn dep_input_denies_unknown_fields_directly() {
    assert_denies_unknown_fields::<DepInput>(
        "DepInput",
        r#"{"issue_id":"a","depends_on_id":"b","dep_type":"blocks","metadataa":"LOST"}"#,
        r#"{"issue_id":"a","depends_on_id":"b","dep_type":"blocks","metadata":"kept"}"#,
    );
}

#[test]
fn filter_input_denies_unknown_fields_directly() {
    assert_denies_unknown_fields::<FilterInput>(
        "FilterInput",
        r#"{"assignie":"a"}"#,
        r#"{"assignee":"a"}"#,
    );
}

#[test]
fn attribution_denies_unknown_fields_directly() {
    assert_denies_unknown_fields::<Attribution>(
        "Attribution",
        r#"{"agent_name":"a","harnes":"x"}"#,
        r#"{"agent_name":"a","harness":"x"}"#,
    );
}

// --- the ONE exempt container --------------------------------------------------------------------

#[test]
fn delete_mode_input_is_fieldless_so_the_exemption_stays_valid() {
    // `DeleteModeInput` is the single container with NO `deny_unknown_fields`, because it is a
    // fieldless unit-variant enum where the attribute has no meaning. That exemption is only valid
    // while it stays fieldless — this re-proves it. Add a field and this goes RED, at which point
    // the container needs the attribute like every other.
    let schema = serde_json::to_value(rmcp::schemars::schema_for!(super::issue::DeleteModeInput))
        .expect("schema");
    assert!(
        schema.get("properties").is_none(),
        "DeleteModeInput gained fields — its `deny_unknown_fields` exemption is no longer valid: \
         {schema}"
    );
    // And it still deserializes as a bare string discriminator.
    let parsed = serde_json::from_str::<super::issue::DeleteModeInput>(r#""cascade""#);
    assert!(parsed.is_ok(), "{parsed:?}");
}

/// The `_`-prefix NEGATIVE case, stated on its own because it is a deliberate DESIGN decision.
///
/// There is **no** `_`-prefix strip at the seam. A conformant MCP `_meta` is a sibling of
/// `arguments` on `CallToolRequestParams` and is destructured away by rmcp before the extractor, so
/// it never reaches `context.arguments` — a strip would protect nothing while permanently
/// re-opening facet (b) for any key an agent happens to prefix with `_`.
#[test]
fn an_underscore_prefixed_unknown_key_inside_arguments_is_rejected() {
    let err = serde_json::from_str::<IssueInput>(r#"{"action":"show","id":"ub-1","_junk":"X"}"#)
        .expect_err("an unknown `_`-prefixed key must be REJECTED, never stripped");
    assert!(err.to_string().contains("unknown field `_junk`"), "{err}");
}
