//! NFR-18 untrusted-input boundary AC suite: each over-quota class is rejected at the preflight —
//! BEFORE any `Session`/`Storage` mutation — over a real `mcp_server_duplex_for_test` wire. The
//! `RecordingStorage` spy (wrapping a real in-memory backend) proves ZERO mutating storage calls were
//! made (the blast radius stays confined to the workspace).
//!
//! Classes: oversized request bytes, over-length arrays, over-length strings (the generic
//! `enforce_quota`), and the D22 `max_batch` bulk record-count cap (`enforce_batch_quota`, rejected at
//! the `create_bulk` preflight before any mint). The F9 path-arg bound is `max_string_len`-only in v1
//! (the real `../`/symlink/workspace confinement is a downstream T2.4 concern).

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
/// **Why this case exists (a STRUCTURAL asymmetry, not redundancy with the `issue` case above).** The
/// NFR-18 RATE limit lives in a pre-dispatch chokepoint (`server.rs::call_tool`) that no tool can
/// bypass, so testing it once suffices. The QUOTA preflight is called INSIDE each tool body — a new
/// tool that simply OMITS `self.preflight(&input)` is caught by NOTHING. This suite covered only the
/// `issue` tool, so deleting the preflight from `tools/comment.rs` left the ENTIRE unblock-mcp suite
/// green. That matters most here: `max_string_len` is the SOLE bound on a comment `body`, which is
/// DELIBERATELY unbounded at the model layer (spine §1.9 — the L7 transport quota IS the cap).
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
