//! NFR-18 untrusted-input boundary AC suite: each over-quota class is rejected at the preflight —
//! BEFORE any `Session`/`Storage` mutation — over a real `mcp_server_duplex_for_test` wire. The
//! `RecordingStorage` spy (wrapping a real in-memory backend) proves ZERO mutating storage calls were
//! made (the blast radius stays confined to the workspace).
//!
//! Classes: oversized request bytes, over-length arrays, over-length strings, over-length object
//! KEYS (the generic `enforce_quota`), and the D22 `max_batch` bulk record-count cap
//! (`enforce_batch_quota`, rejected at the `create_bulk` preflight before any mint). The F9 path-arg
//! bound is `max_string_len`-only in v1 (the real `../`/symlink/workspace confinement is a
//! downstream T2.4 concern).
//!
//! **D42:** the check moved from per-tool-body over the RE-SERIALIZED TYPED input to ONCE in
//! `call_tool` over the WHOLE `tools/call` `params` (name + arguments + `_meta` + `task`), inside
//! the rate-limit permit. The pre-D42 placement meant a payload parked under an unknown key was
//! never measured at all.

mod common;

use common::{call_tool, connect_with_quotas, session_recording};
use serde_json::json;
use unblock_mcp::Quotas;

/// A base quota with GENEROUS request-bytes / array / string / batch caps; each test tightens only the
/// ONE limit it exercises, so its specific guard fires first (the preflight checks request bytes →
/// strings/arrays in document order, so an isolating base prevents a different guard pre-empting it).
fn lax_base() -> Quotas {
    Quotas {
        max_request_bytes: 64 * 1024,
        max_array_len: 1024,
        max_string_len: 64 * 1024,
        max_batch: 1024,
        max_concurrent_requests: 64,
    }
}

/// An over-length STRING (a title longer than `max_string_len`) is rejected at the preflight with
/// ZERO storage mutations.
///
/// **D42 margin note — the `kind == "string"` assertion below is load-bearing.** Since the quota
/// walks the whole `params`, `max_string_len` also bounds object KEYS and the tool `name` value. At
/// `max_string_len: 16` the headroom is thin and quantified: the longest params-level key
/// `arguments` is 9 B (**+7**) and the longest tool name `diagnostics` is 11 B (**+5**). A future
/// test setting `max_string_len < 12` would fail on the tool NAME, and `< 10` on the `arguments`
/// key — with a message naming the wrong culprit. Asserting the `kind` makes that drift LOUD
/// instead of silent. There is deliberately no carve-out for `name`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn over_length_string_rejected_with_zero_storage_calls() {
    let quotas = Quotas {
        max_string_len: 16,
        ..lax_base()
    };
    let (session, spy) = session_recording().await;
    let (client, server, _cancel) = connect_with_quotas(session, quotas, None).await;

    let oversized_title = "x".repeat(64); // > max_string_len (16).
    let (is_error, payload) = call_tool(
        &client,
        "issue",
        json!({ "action": "create", "title": oversized_title }),
    )
    .await;

    assert!(is_error, "an over-length string is an in-band error");
    assert_eq!(payload["code"], "VALIDATION_FAILED");
    assert_eq!(payload["context"]["kind"], "string");
    assert_eq!(
        spy.mutation_count(),
        0,
        "rejected BEFORE any storage mutation"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// Tool #8 `comment` (D37) enforces the SAME quota preflight: an over-length `body` is rejected with
/// ZERO storage mutations.
///
/// **Why this case still exists.** Its ORIGINAL rationale — a "structural asymmetry" whereby the
/// rate limit lived in a pre-dispatch chokepoint no tool could bypass while the QUOTA was called
/// inside each tool body, so a new tool omitting `self.preflight(&input)` was caught by nothing — is
/// **INVERTED by D42 and has been deleted rather than softened.** Since D42 the quota is enforced
/// ONCE in `server.rs::call_tool`, in the same pre-dispatch chokepoint as the rate limit, so it
/// cannot be omitted per tool at all. The case is kept for its OTHER value: `max_string_len` is the
/// SOLE bound on a comment `body`, which is deliberately unbounded at the model layer (spine §1.9 —
/// the L7 transport quota IS the cap).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn comment_over_length_body_rejected_with_zero_storage_calls() {
    let quotas = Quotas {
        max_string_len: 16,
        ..lax_base()
    };
    let (session, spy) = session_recording().await;
    let (client, server, _cancel) = connect_with_quotas(session, quotas, None).await;

    let oversized_body = "x".repeat(64); // > max_string_len (16).
    let (is_error, payload) = call_tool(
        &client,
        "comment",
        json!({ "action": "add", "issue_id": "ub-1", "body": oversized_body }),
    )
    .await;

    assert!(is_error, "an over-length comment body is an in-band error");
    assert_eq!(payload["code"], "VALIDATION_FAILED");
    assert_eq!(payload["context"]["kind"], "string");
    assert_eq!(
        spy.mutation_count(),
        0,
        "the comment tool's quota preflight fires BEFORE any Session/storage mutation",
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// An over-length ARRAY (more labels than `max_array_len`) is rejected at the preflight with ZERO
/// storage mutations.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn over_length_array_rejected_with_zero_storage_calls() {
    let quotas = Quotas {
        max_array_len: 2,
        ..lax_base()
    };
    let (session, spy) = session_recording().await;
    let (client, server, _cancel) = connect_with_quotas(session, quotas, None).await;

    let (is_error, payload) = call_tool(
        &client,
        "issue",
        json!({ "action": "create", "title": "ok", "labels": ["a", "b", "c"] }),
    )
    .await;

    assert!(is_error);
    assert_eq!(payload["code"], "VALIDATION_FAILED");
    assert_eq!(payload["context"]["kind"], "array");
    assert_eq!(
        spy.mutation_count(),
        0,
        "rejected BEFORE any storage mutation"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// An over-size REQUEST (total serialized bytes beyond `max_request_bytes`) is rejected at the
/// preflight with ZERO storage mutations.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn over_size_request_rejected_with_zero_storage_calls() {
    // A request whose total bytes exceed max_request_bytes but whose individual strings/arrays are
    // under their caps — so only the request-bytes guard can catch it.
    let quotas = Quotas {
        max_request_bytes: 64,
        ..lax_base()
    };
    let (session, spy) = session_recording().await;
    let (client, server, _cancel) = connect_with_quotas(session, quotas, None).await;

    let big = "y".repeat(200); // one string, under max_string_len, but the whole request > 64 bytes.
    let (is_error, payload) = call_tool(
        &client,
        "issue",
        json!({ "action": "create", "title": big }),
    )
    .await;

    assert!(is_error);
    assert_eq!(payload["code"], "VALIDATION_FAILED");
    assert_eq!(payload["context"]["kind"], "request");
    assert_eq!(
        spy.mutation_count(),
        0,
        "rejected BEFORE any storage mutation"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// A `create_bulk` whose PARSED record count exceeds `max_batch` is rejected at the preflight (after
/// the parse, before any mint) with ZERO storage mutations (the D22 `max_batch` cap, F5).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn over_max_batch_create_bulk_rejected_with_zero_storage_calls() {
    // max_batch = 2; a 3-record document trips it. Generous byte/string caps so ONLY the batch cap can
    // fire (it is checked AFTER the parse, inside the create_bulk action).
    let quotas = Quotas {
        max_batch: 2,
        ..lax_base()
    };
    let (session, spy) = session_recording().await;
    let (client, server, _cancel) = connect_with_quotas(session, quotas, None).await;

    let markdown = "## One\n### Type\ntask\n\n## Two\n### Type\ntask\n\n## Three\n### Type\ntask\n";
    let (is_error, payload) = call_tool(
        &client,
        "issue",
        json!({ "action": "create_bulk", "markdown": markdown }),
    )
    .await;

    assert!(is_error, "an over-max_batch bulk is an in-band error");
    assert_eq!(payload["code"], "VALIDATION_FAILED");
    assert_eq!(payload["context"]["kind"], "batch");
    assert_eq!(
        spy.mutation_count(),
        0,
        "the batch cap fires at the preflight — ZERO Session::create_bulk / storage mutations",
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// A `create_bulk` AT the `max_batch` boundary is accepted (the cap is `>`, not `>=`) — proving the
/// over-cap rejection above is non-vacuous (a valid batch of the same shape does reach storage).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_bulk_at_max_batch_boundary_is_accepted() {
    let quotas = Quotas {
        max_batch: 2,
        ..lax_base()
    };
    let (session, spy) = session_recording().await;
    let (client, server, _cancel) = connect_with_quotas(session, quotas, None).await;

    let markdown = "## One\n### Type\ntask\n\n## Two\n### Type\ntask\n";
    let (is_error, _payload) = call_tool(
        &client,
        "issue",
        json!({ "action": "create_bulk", "markdown": markdown }),
    )
    .await;

    assert!(!is_error, "a batch AT max_batch is accepted");
    assert!(
        spy.mutation_count() >= 1,
        "an accepted batch reaches storage (non-vacuity for the over-cap rejection)",
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// F9 — a `sync` import path arg is bounded ONLY by `max_string_len` at the preflight in v1 (an
/// over-long path → over-quota `ValidationFailed`). The real `../`/symlink/workspace confinement is a
/// downstream `unblock-sync` concern landing at T2.4 (the preflight does NOT yet emit `PathTraversal`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_path_arg_is_string_length_bound_only_in_v1() {
    let quotas = Quotas {
        max_string_len: 16,
        ..lax_base()
    };
    let (session, spy) = session_recording().await;
    let (client, server, _cancel) = connect_with_quotas(session, quotas, None).await;

    let long_path = format!("/{}", "a".repeat(64)); // > max_string_len (16).
    let (is_error, payload) = call_tool(
        &client,
        "sync",
        json!({ "action": "import", "path": long_path }),
    )
    .await;

    assert!(is_error);
    assert_eq!(payload["code"], "VALIDATION_FAILED");
    assert_eq!(
        payload["context"]["kind"], "string",
        "v1 bounds the path arg by string length only (real ../ confinement = T2.4)",
    );
    assert_eq!(
        spy.mutation_count(),
        0,
        "rejected BEFORE any storage mutation"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

// --------------------------------------------------------------------------------------------------
// D42 — the quota is scoped to the WHOLE `tools/call` `params`, not the typed input.
// --------------------------------------------------------------------------------------------------

use common::call_tool_with_meta;

/// **The headline fix.** A 300 KB blob under `params._meta` alongside a perfectly valid
/// `issue create` was live-reproduced BEFORE D42 as: issue CREATED, `isError:false` — the 256 KiB
/// cap bypassed entirely, because `preflight` measured the re-serialized TYPED input and `_meta` is
/// a sibling of `arguments` that never appears in it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn meta_over_cap_rejected_in_band_with_zero_storage_calls() {
    let (session, spy) = session_recording().await;
    let (client, server, _cancel) = connect_with_quotas(session, Quotas::default(), None).await;

    let (is_error, payload) = call_tool_with_meta(
        &client,
        "issue",
        json!({ "action": "create", "title": "ok" }),
        json!({ "blob": "Z".repeat(300_000) }),
    )
    .await;

    assert!(is_error, "an over-cap `_meta` must be rejected: {payload}");
    assert_eq!(payload["code"], "VALIDATION_FAILED");
    assert_eq!(payload["retryable"], true);
    assert_eq!(payload["context"]["kind"], "request");
    assert!(
        payload["context"]["actual"].as_u64().unwrap_or(0) > 262_144,
        "the measurement must include `_meta`: {payload}"
    );
    assert_eq!(spy.mutation_count(), 0, "rejected before any Session call");

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// NON-VACUITY for the case above: a small, legitimate `_meta` (e.g. a `progressToken`) must still
/// succeed. Without this cell the test above would pass just as well against a boundary that
/// rejected ALL `_meta`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn meta_under_cap_still_succeeds() {
    let (session, spy) = session_recording().await;
    let (client, server, _cancel) = connect_with_quotas(session, Quotas::default(), None).await;

    let (is_error, payload) = call_tool_with_meta(
        &client,
        "issue",
        json!({ "action": "create", "title": "ok" }),
        json!({ "progressToken": 1 }),
    )
    .await;

    assert!(!is_error, "a small `_meta` must still succeed: {payload}");
    assert!(spy.mutation_count() >= 1, "the happy path reached storage");

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// A payload parked under UNKNOWN keys is now COUNTED. Live-reproduced pre-D42:
/// `arguments.junk = "Z"*200000` created the issue silently — the typed measurement dropped every
/// unknown key, so a minimal `issue create` measured ~346 B no matter how much junk rode with it.
///
/// The PAIRED control in the same test — the same bulk over KNOWN fields — is what proves this is
/// the fix rather than a coincidence: both must reach the identical `kind:"request"` verdict.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_key_payload_is_counted_in_request_bytes() {
    let quotas = Quotas {
        max_string_len: 64 * 1024,
        ..Quotas::default()
    };

    let mut unknown = serde_json::Map::new();
    unknown.insert("action".into(), json!("create"));
    unknown.insert("title".into(), json!("ok"));
    for i in 0..10 {
        unknown.insert(format!("junk{i}"), json!("Z".repeat(30_000)));
    }

    let mut known = serde_json::Map::new();
    known.insert("action".into(), json!("create"));
    known.insert("title".into(), json!("ok"));
    known.insert(
        "labels".into(),
        json!((0..10).map(|_| "Z".repeat(30_000)).collect::<Vec<_>>()),
    );

    for (cell, args) in [
        ("unknown keys", serde_json::Value::Object(unknown)),
        (
            "known fields (paired control)",
            serde_json::Value::Object(known),
        ),
    ] {
        let (session, spy) = session_recording().await;
        let (client, server, _cancel) = connect_with_quotas(session, quotas, None).await;
        let (is_error, payload) = call_tool(&client, "issue", args).await;
        assert!(is_error, "{cell}: must be rejected: {payload}");
        assert_eq!(
            payload["context"]["kind"], "request",
            "{cell}: both must reach the SAME verdict — that is what makes this the fix and not a \
             coincidence: {payload}"
        );
        assert_eq!(spy.mutation_count(), 0, "{cell}");
        let _ = client.cancel().await;
        let _ = server.cancel().await;
    }
}

/// An over-length object KEY is rejected. Pre-D42 the `Object` arm iterated `map.values()` only, so
/// a 100 000-byte key passed the quota and reached the tool boundary (live-reproduced).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn over_length_object_key_rejected() {
    let (session, spy) = session_recording().await;
    let (client, server, _cancel) = connect_with_quotas(session, Quotas::default(), None).await;

    let mut args = serde_json::Map::new();
    args.insert("action".into(), json!("show"));
    args.insert("id".into(), json!("ub-1"));
    args.insert("k".repeat(70_000), json!(1));

    let (is_error, payload) = call_tool(&client, "issue", serde_json::Value::Object(args)).await;
    assert!(is_error, "{payload}");
    assert_eq!(payload["context"]["kind"], "key");
    assert_eq!(payload["context"]["actual"], 70_000);
    assert_eq!(payload["context"]["limit"], 65_536);
    assert_eq!(spy.mutation_count(), 0);

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// Attacker-controlled echoed text is TRUNCATED. Post-`deny_unknown_fields` serde echoes an unknown
/// field name verbatim into the message, so an unbounded echo would amplify it into the response.
///
/// The bound is SOFT: `sanitize_message` runs after the clip and escapes control characters at up to
/// ~6 bytes each, so the final message is bounded at ~`6 * MAX_ECHOED_BYTES`, not `MAX_ECHOED_BYTES`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn echoed_unknown_field_is_truncated() {
    let (session, _spy) = session_recording().await;
    let (client, server, _cancel) = connect_with_quotas(session, Quotas::default(), None).await;

    let mut args = serde_json::Map::new();
    args.insert("action".into(), json!("show"));
    args.insert("id".into(), json!("ub-1"));
    args.insert("q".repeat(1_000), json!(1));

    let (is_error, payload) = call_tool(&client, "issue", serde_json::Value::Object(args)).await;
    assert!(is_error, "{payload}");
    let message = payload["message"].as_str().expect("message");
    assert!(
        message.len() <= 6 * 128 + 64,
        "message must be clipped (soft bound ~6x): {} bytes",
        message.len()
    );
    assert!(message.contains("…[truncated]"), "{message}");
    let field = payload["context"]["field"].as_str().expect("field");
    assert!(field.len() <= 128 + "…[truncated]".len(), "{}", field.len());

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// The quota check sits INSIDE the rate-limit permit. With zero permits an over-quota request must
/// report `RATE_LIMITED`, not `VALIDATION_FAILED` — proving the O(request bytes) walk cannot be made
/// to run outside the D34-F5 concurrency bound.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quota_check_sits_inside_the_rate_limit_permit() {
    let quotas = Quotas {
        max_concurrent_requests: 0,
        max_request_bytes: 32,
        ..Quotas::default()
    };
    let (session, _spy) = session_recording().await;
    let (client, server, _cancel) = connect_with_quotas(session, quotas, None).await;

    let (is_error, payload) = call_tool(
        &client,
        "issue",
        json!({ "action": "create", "title": "Z".repeat(500) }),
    )
    .await;
    assert!(is_error);
    assert_eq!(
        payload["code"], "RATE_LIMITED",
        "the rate limit must be reached FIRST — moving the quota above `try_acquire` inverts the \
         'the permit gates the WHOLE dispatch' invariant: {payload}"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// The re-serialization used for measurement is LOSSLESS. This is what pins "measuring the request
/// here is NOT the `preflight` defect": every `CallToolRequestParams` field is raw JSON, so
/// `to_value` cannot drop anything — unlike the typed DTO, which dropped every unknown key and
/// invented `#[serde(default)]` padding.
#[test]
fn request_measurement_is_lossless() {
    use rmcp::model::CallToolRequestParams;

    let mut arguments = serde_json::Map::new();
    arguments.insert("action".into(), json!("create"));
    arguments.insert("totally_unknown_key".into(), json!("PRESERVED"));
    let mut meta = serde_json::Map::new();
    meta.insert("blob".into(), json!("META-PRESERVED"));

    let mut params = CallToolRequestParams::new("issue").with_arguments(arguments);
    params.meta = Some(rmcp::model::Meta(meta));

    let value = serde_json::to_value(&params).expect("lossless");
    assert_eq!(value["name"], "issue");
    assert_eq!(value["arguments"]["totally_unknown_key"], "PRESERVED");
    assert_eq!(
        value["_meta"]["blob"], "META-PRESERVED",
        "`_meta` MUST appear in the measured value — measuring only `request.arguments` would \
         restore the bypass: {value}"
    );
}
