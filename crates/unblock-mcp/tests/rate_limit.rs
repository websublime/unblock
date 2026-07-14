//! NFR-18 rate-limit AC suite (D34-F5): the `Arc<Semaphore>(max_concurrent_requests)` chokepoint on
//! `UnblockServer` (`server.rs`) bounds concurrent in-flight requests. `try_acquire` failure fast-fails
//! IN-BAND for a tool call (a retryable `RATE_LIMITED` `CallToolResult`) — never dropped or
//! backpressured. Two tests, both over a real `mcp_server_duplex_for_test` wire:
//!
//! - `rate_limit_chokepoint_is_the_live_tool_dispatch_path` — the SF-6 assumption pin: with 0 permits
//!   EVERY tool call must reject with `RATE_LIMITED`, proving the hand-written `call_tool` (which
//!   SUPPRESSES the rmcp-macros generated one) is still the live dispatch path.
//! - `at_cap_accepted_over_cap_rejected_in_band` — the NON-VACUOUS N/N+1 boundary: hold N tool calls
//!   in-flight via a REAL barrier (a blocking `Storage` double, never a `sleep`), then prove exactly
//!   the (N+1)th is rejected while the N in-flight calls all succeed. Removing the `try_acquire` guard
//!   in `server.rs::call_tool` makes the (N+1)th succeed too, so the whole assertion set fails — the
//!   guard is load-bearing (proven).
//!
//! The reject path uses `query{list}` (a READ), so it never serializes on the engine write permit and
//! mask the concurrency assertion (FR-10 reads bypass the write `Semaphore(1)`).

mod common;

use common::call_tool;
use rmcp::model::ReadResourceRequestParams;
use rmcp::service::ServiceError;
use serde_json::{Value, json};
use unblock_mcp::Quotas;

/// SF-6 ASSUMPTION PIN — the hand-written `call_tool` in `crates/unblock-mcp/src/server.rs` IS the live
/// tool-dispatch path.
///
/// `#[rmcp::tool_handler]` only generates a `call_tool` when the impl does NOT already define one
/// (`rmcp-macros-1.7.0/src/tool_handler.rs:44`, `if !has_method("call_tool", ...)`); `unblock` relies
/// on that suppression to install the NFR-18 rate-limit chokepoint. This pin proves the suppression
/// still holds: with `max_concurrent_requests = 0` EVERY `try_acquire` fails, so EVERY tool call MUST
/// reject in-band with `RATE_LIMITED`. If a future rmcp bypasses the hand-written method
/// (double-registers, or re-routes dispatch so its own generated `call_tool` wins), the tool would
/// instead run normally — this fails LOUDLY. Do NOT "fix" the pin to stay green: re-evaluate the
/// chokepoint in `server.rs` first. (`initialize` / `list_tools` are UNGATED, so the handshake and
/// discovery still complete at 0 permits — only actual tool CALLS are rejected.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rate_limit_chokepoint_is_the_live_tool_dispatch_path() {
    let quotas = Quotas {
        max_concurrent_requests: 0,
        ..Quotas::default()
    };
    let session = common::session().await;
    let (client, server, _cancel) = common::connect_with_quotas(session, quotas, None).await;

    // The handshake + discovery are UNGATED even at 0 permits (only `call_tool` / `read_resource` gate).
    let tools = client
        .list_all_tools()
        .await
        .expect("list_tools succeeds at 0 permits (discovery is not rate-limited)");
    assert_eq!(tools.len(), 7, "discovery is not rate-limited");

    // Every tool call is rejected in-band with RATE_LIMITED — the hand-written `call_tool` intercepts
    // before the router dispatch.
    let (is_error, payload) = call_tool(&client, "query", json!({ "kind": "list" })).await;
    assert!(
        is_error,
        "a tool call at 0 permits must be an in-band error"
    );
    assert_eq!(
        payload["code"], "RATE_LIMITED",
        "the chokepoint rejects with RateLimited (SF-6): if this is NOT RATE_LIMITED, rmcp may have \
         stopped honouring the hand-written `call_tool` suppression — re-check the chokepoint in \
         crates/unblock-mcp/src/server.rs before touching this pin"
    );
    assert_eq!(payload["retryable"], true, "RateLimited is retryable");

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// The NFR-18 rate-limit AC (D34-F5), NON-VACUOUS. Hold N tool calls in-flight (occupying all N
/// permits) via a REAL barrier in a blocking `Storage` double — never a `sleep` — then prove exactly
/// the (N+1)th concurrent call is rejected in-band while the N in-flight calls all succeed once
/// released (the boundary pair: at-cap accepted / over-cap rejected).
///
/// Non-vacuity: the N at-cap calls DO reach storage and return successfully, so removing the
/// `try_acquire` guard in `server.rs::call_tool` makes the (N+1)th succeed too → the over-cap
/// assertions fail. The guard is load-bearing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn at_cap_accepted_over_cap_rejected_in_band() {
    // A LOW cap so the boundary is cheap and deterministic: N permits, N held in-flight, the (N+1)th
    // rejected.
    const N: usize = 2;
    let quotas = Quotas {
        max_concurrent_requests: N,
        ..Quotas::default()
    };
    let (session, gate) = common::session_gated(N).await;
    let (client, server, _cancel) = common::connect_with_quotas(session, quotas, None).await;

    // Two concurrent `query{list}` calls: each reaches the gated `list_issues`, signals "entered"
    // (its rate-limit permit is held), and blocks. The control future waits until BOTH are in-flight
    // (both permits held), fires the 3rd (over-cap) call, asserts its in-band reject, then releases the
    // two in-flight reads (control is the barrier's (N+1)th party). All three futures are driven
    // concurrently on one task via `tokio::join!` (a shared `&client`, no `'static` spawn needed).
    let held_a = call_tool(&client, "query", json!({ "kind": "list" }));
    let held_b = call_tool(&client, "query", json!({ "kind": "list" }));
    let control = async {
        gate.await_all_entered(N).await;
        // The (N+1)th call: the semaphore is exhausted → rejected WITHOUT touching storage. Bounded by a
        // timeout so a non-vacuity regression (guard removed → this call reaches the gated storage and
        // BLOCKS on the barrier) fails as a clean assertion, not an opaque CI hang.
        let over = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            call_tool(&client, "query", json!({ "kind": "list" })),
        )
        .await
        .expect("the over-cap call must reject fast, not hang (guard removed?)");
        // Release the two in-flight reads so the at-cap calls can complete.
        gate.release().await;
        over
    };

    let ((a_is_error, _a), (b_is_error, _b), (over_is_error, over_payload)) =
        tokio::join!(held_a, held_b, control);

    // Over-cap: rejected in-band with the retryable RateLimited (NFR-18/D34).
    assert!(
        over_is_error,
        "the (N+1)th concurrent call is an in-band error"
    );
    assert_eq!(
        over_payload["code"], "RATE_LIMITED",
        "the over-cap call is rejected with RateLimited"
    );
    assert_eq!(over_payload["retryable"], true, "RateLimited is retryable");

    // At-cap: BOTH calls were accepted (the non-vacuity anchor — with the guard removed the (N+1)th is
    // accepted too and the over-cap assertions above fail).
    assert!(!a_is_error, "the 1st at-cap call is accepted");
    assert!(!b_is_error, "the 2nd at-cap call is accepted");

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// SF-1 — the `read_resource` rate-limit reject (MF-5). Resources have NO in-band channel, so the
/// chokepoint reject on a resource read is OUT-OF-BAND: an rmcp `ErrorData` (JSON-RPC `-32603`) whose
/// `data` payload carries the structured `RateLimited` (`code`/`retryable`). With 0 permits every
/// `read_resource` must reject this way — the asymmetric-to-tools path (an in-band `CallToolResult` for
/// a tool, an `ErrorData` for a resource) is a NORMATIVE spine §5.6 requirement, so it gets its own
/// guard test alongside the tool-path pins above.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_resource_at_zero_permits_rejects_out_of_band() {
    let quotas = Quotas {
        max_concurrent_requests: 0,
        ..Quotas::default()
    };
    let session = common::session().await;
    let (client, server, _cancel) = common::connect_with_quotas(session, quotas, None).await;

    let err = client
        .read_resource(ReadResourceRequestParams::new("unblock://capabilities"))
        .await
        .expect_err("read_resource at 0 permits must reject");
    // Out-of-band: an rmcp `ErrorData` at the pinned transport code `-32603` (deliberate, MF-5), the
    // structured `RateLimited` riding `data` so the client can still retry.
    let ServiceError::McpError(data) = err else {
        panic!("expected an rmcp McpError, got {err:?}");
    };
    assert_eq!(
        data.code.0, -32603,
        "resources have no in-band channel — the reject is out-of-band -32603 (MF-5)"
    );
    let payload: Value = data.data.expect("the structured payload rides `data`");
    assert_eq!(
        payload["code"], "RATE_LIMITED",
        "the resource reject carries RateLimited in its data payload"
    );
    assert_eq!(payload["retryable"], true, "RateLimited is retryable");

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}
