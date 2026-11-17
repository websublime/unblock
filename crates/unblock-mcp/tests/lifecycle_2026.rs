//! **PRD §4 D59 clause (9), criteria (f2)–(f4), (g) and (m) — MCP 2026-07-28 beside `initialize`,
//! the redesigned pre-lifecycle gate and cancellation, over a LIVE duplex through the SHIPPED
//! composition (`PreHandshakeGateTransport<DupScanningTransport>` via `mcp_server_duplex_for_test`).**
//!
//! Every frame is hand-written and read back raw, so the cells see exactly the bytes a stateless
//! client sees. The in-flight cells hold a `tools/call` inside its storage write (the `gated` create
//! hold of `tests/common`) and order their frames through the serve loop, which handles inbound frames
//! in arrival order: a `ping` sent after a `notifications/cancelled` and answered proves the
//! cancellation was registered. The NEGATIVE half of every no-reply cell first waits on the
//! held-handler-returned signal (`mcp_server_duplex_with_returned_signal_for_test`), then closes the
//! client's write half and reads stdout to EOF, because rmcp 3.10 drains in-flight responses before it
//! closes (`rmcp-3.10.0/src/service.rs:1788-1830`). No sleep decides an outcome.

mod common;

use std::sync::Arc;

use common::RawDuplexClient;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt as _;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use unblock_engine::Session;
use unblock_mcp::{
    Quotas, mcp_server_duplex_for_test, mcp_server_duplex_with_returned_signal_for_test,
};

/// The complete 2026-07-28 request `_meta`: a supported version AND client capabilities.
fn meta_2026() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {}
    })
}

/// A server over the shipped composition with NO handshake done, and its raw client.
fn raw_server(session: Arc<Session>) -> (RawDuplexClient, CancellationToken) {
    let (server_io, client_io) = tokio::io::duplex(1024 * 1024);
    let (server_read, server_write) = tokio::io::split(server_io);
    let cancel = CancellationToken::new();
    let server_cancel = cancel.clone();
    tokio::spawn(async move {
        // A started lifecycle returns the RunningService; keep it alive (its drop cancels the server).
        if let Ok(running) = mcp_server_duplex_for_test(
            session,
            Quotas::default(),
            None,
            server_read,
            server_write,
            server_cancel,
        )
        .await
        {
            let _ = running.waiting().await;
        }
    });
    (common::raw_client(client_io), cancel)
}

/// Send one raw JSON-RPC request object and read its response.
async fn send(client: &mut RawDuplexClient, frame: &Value) -> Value {
    let id = frame["id"].as_i64().expect("an integer id");
    client
        .request_raw(id, &serde_json::to_string(frame).expect("serialize"))
        .await
}

/// A `tools/call` `issue create` frame on `id`, optionally carrying `_meta`.
fn create_frame(id: i64, title: &str, meta: Option<Value>) -> Value {
    let mut params = json!({"name": "issue", "arguments": {"action": "create", "title": title}});
    if let Some(meta) = meta {
        params["_meta"] = meta;
    }
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": params})
}

/// **(f2)** A FIRST-frame `server/discover` with complete 2026-07-28 `_meta` returns exactly the
/// clause-(9) `DiscoverResult`, and an `initialize` sent after it still completes a handshake.
///
/// Mutant: a gate that drops or answers `server/discover`; a latch that opens on the discover reply.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_first_frame_server_discover_is_answered_and_initialize_still_completes() {
    let (mut client, _cancel) = raw_server(common::session().await);
    let discover = send(
        &mut client,
        &json!({"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta": meta_2026()}}),
    )
    .await;
    let result = &discover["result"];
    assert_eq!(
        result["supportedVersions"],
        json!([
            "2024-11-05",
            "2025-03-26",
            "2025-06-18",
            "2025-11-25",
            "2026-07-28"
        ]),
        "{discover}"
    );
    assert_eq!(result["resultType"], "complete", "{discover}");
    assert_eq!(result["ttlMs"], 0, "{discover}");
    assert_eq!(result["cacheScope"], "private", "{discover}");
    assert_eq!(
        result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"], "unblock",
        "{discover}"
    );
    assert_eq!(
        result["_meta"]["io.modelcontextprotocol/serverInfo"]["version"],
        env!("CARGO_PKG_VERSION"),
        "{discover}"
    );
    assert!(
        result["instructions"]
            .as_str()
            .is_some_and(|i| i.contains("unblock MCP server")),
        "the discover answer carries the server's instructions: {discover}"
    );

    let init = client.initialize().await;
    assert_eq!(
        init["result"]["protocolVersion"], "2025-11-25",
        "the server stays open to the initialize lifecycle after discover: {init}"
    );
}

/// **(f3)** A FIRST-frame `tools/call` with complete 2026-07-28 `_meta` STARTS the stateless session
/// and executes on 2026-07-28 semantics; in that session a request without `_meta` gets out-of-band
/// `-32602`, `ping` gets `-32601`, an unknown resource gets `-32602` with the `StructuredError` in
/// `data`, and `tools/list` carries the cache hints.
///
/// Mutant: a gate that never opens on the stateless edge (the later frames would meet the
/// pre-lifecycle loop again); a gate that answers Requests itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_first_frame_stateless_tools_call_starts_the_2026_session() {
    let (mut client, _cancel) = raw_server(common::session().await);

    let created = send(
        &mut client,
        &create_frame(1, "stateless", Some(meta_2026())),
    )
    .await;
    assert_eq!(created["result"]["resultType"], "complete", "{created}");
    assert_ne!(created["result"]["isError"], true, "{created}");

    let bare = send(
        &mut client,
        &json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
    )
    .await;
    assert_eq!(
        bare["error"]["code"], -32602,
        "in a stateless session every request must carry the 2026-07-28 `_meta`: {bare}"
    );

    let ping = send(
        &mut client,
        &json!({"jsonrpc":"2.0","id":3,"method":"ping","params":{"_meta": meta_2026()}}),
    )
    .await;
    assert_eq!(
        ping["error"]["code"], -32601,
        "the inline lifecycle has no ping: {ping}"
    );

    let missing = send(
        &mut client,
        &json!({"jsonrpc":"2.0","id":4,"method":"resources/read","params":{"uri":"unblock://nope","_meta": meta_2026()}}),
    )
    .await;
    assert_eq!(
        missing["error"]["code"], -32602,
        "a missing resource is -32602 on 2026-07-28: {missing}"
    );
    assert_eq!(
        missing["error"]["data"]["code"], "ISSUE_NOT_FOUND",
        "with our StructuredError still in data: {missing}"
    );

    let listed = send(
        &mut client,
        &json!({"jsonrpc":"2.0","id":5,"method":"tools/list","params":{"_meta": meta_2026()}}),
    )
    .await;
    assert!(
        listed["result"].get("ttlMs").is_some() && listed["result"].get("cacheScope").is_some(),
        "2026-07-28 list results carry the cache hints: {listed}"
    );
}

/// **(f4)** A FIRST-frame request whose `_meta` names `2030-01-01` WITH `clientCapabilities` gets
/// `-32022`, the process stays alive, and a following `initialize` completes. An id-less notification
/// sent after it is still DROPPED, so the latch stayed shut (criterion (g)).
///
/// Mutant: a gate that opens on any complete `_meta` regardless of the supported set (the
/// notification would reach rmcp's pre-lifecycle loop and end the session).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unsupported_inline_version_gets_32022_and_leaves_the_latch_shut() {
    let (mut client, _cancel) = raw_server(common::session().await);
    let refused = send(
        &mut client,
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"_meta":{
            "io.modelcontextprotocol/protocolVersion": "2030-01-01",
            "io.modelcontextprotocol/clientCapabilities": {}
        }}}),
    )
    .await;
    assert_eq!(refused["error"]["code"], -32022, "{refused}");

    client
        .write_raw_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
        .await;
    let init = client.initialize().await;
    assert!(
        init["result"].get("serverInfo").is_some(),
        "the process stays alive and the handshake completes: {init}"
    );
}

/// **(g)** A premature `tools/list` with no `_meta` is answered `-32602` on its OWN id by rmcp and the
/// following `initialize` completes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_premature_request_without_meta_gets_32602_and_the_handshake_completes() {
    let (mut client, _cancel) = raw_server(common::session().await);
    let answered = send(
        &mut client,
        &json!({"jsonrpc":"2.0","id":41,"method":"tools/list","params":{}}),
    )
    .await;
    assert_eq!(answered["error"]["code"], -32602, "{answered}");
    assert_eq!(answered["id"], 41, "{answered}");
    let init = client.initialize().await;
    assert!(init["result"].get("serverInfo").is_some(), "{init}");
}

/// Read everything the server writes until it closes its end.
async fn drain_to_eof(client: RawDuplexClient) -> String {
    let mut rest = String::new();
    let mut reader = client.into_reader();
    let _ = reader.read_to_string(&mut rest).await;
    rest
}

/// A held-create server: `tools/call issue create` parks inside its storage write until released,
/// and every returned `call_tool` adds one permit to `returned`.
async fn held_server() -> (
    RawDuplexClient,
    common::gated::Gate,
    Arc<Semaphore>,
    Arc<Session>,
) {
    let (session, gate) = common::session_create_gated().await;
    let returned = Arc::new(Semaphore::new(0));
    let (server_io, client_io) = tokio::io::duplex(1024 * 1024);
    let (server_read, server_write) = tokio::io::split(server_io);
    let (server_session, server_returned) = (Arc::clone(&session), Arc::clone(&returned));
    tokio::spawn(async move {
        // A started lifecycle returns the RunningService; keep it alive (its drop cancels the server).
        if let Ok(running) = mcp_server_duplex_with_returned_signal_for_test(
            server_session,
            Quotas::default(),
            server_read,
            server_write,
            CancellationToken::new(),
            server_returned,
        )
        .await
        {
            let _ = running.waiting().await;
        }
    });
    (common::raw_client(client_io), gate, returned, session)
}

/// The titles of every issue in the store, read straight from the session (no MCP round trip).
async fn titles(session: &Session) -> Vec<String> {
    session
        .list(&unblock_model::ListFilters::default())
        .await
        .expect("list issues")
        .into_iter()
        .map(|issue| issue.title)
        .collect()
}

/// **(m)** A `tools/call` held inside its storage write, then cancelled, is released, RUNS TO
/// COMPLETION (the store shows the issue) and gets NO reply. The uncancelled twin gets its reply.
///
/// Mutants: a server that still answers a cancelled request (rmcp 1.7's behaviour); a handler that
/// observes the token and rolls the write back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_write_commits_and_gets_no_reply() {
    for cancelled in [true, false] {
        let (mut client, gate, returned, session) = held_server().await;
        client.initialize().await;
        let title = if cancelled {
            "cancelled-write"
        } else {
            "uncancelled-twin"
        };
        client
            .write_raw_line(&serde_json::to_string(&create_frame(77, title, None)).expect("frame"))
            .await;
        gate.await_all_entered(1).await;
        if cancelled {
            client
                .write_raw_line(r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":77,"reason":"test"}}"#)
                .await;
        }
        // The serve loop handles inbound frames in arrival order, so this answered ping proves the
        // cancellation (when sent) was registered before the release.
        let ping = send(
            &mut client,
            &json!({"jsonrpc":"2.0","id":78,"method":"ping"}),
        )
        .await;
        assert!(ping.get("result").is_some(), "{ping}");
        gate.release().await;
        returned
            .acquire_many(1)
            .await
            .expect("the held call_tool returned")
            .forget();

        if !cancelled {
            // The uncancelled twin's reply is READ before stdin closes: the returned signal fires
            // before rmcp's send task writes the response, so only a read proves it arrived.
            let reply = client.read_response(77).await;
            assert_ne!(reply["result"]["isError"], true, "{reply}");
        }
        // The negative half: the handler has returned (the signal above), then stdin closes and
        // stdout is read to EOF. rmcp 3.10 drains in-flight responses before closing, a drain
        // BOUNDED at 5 s on a closed input (`rmcp-3.10.0/src/service.rs:1788-1830`), so the drain
        // can only hide a regression, never cause a flake; the returned signal is the primary proof.
        let mut seen = client.seen_lines.join("\n");
        seen.push('\n');
        client.close_write().await;
        seen.push_str(&drain_to_eof(client).await);
        let answered = seen
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .any(|frame| frame["id"] == 77);
        assert_eq!(
            answered, !cancelled,
            "a cancelled request gets NO reply and its uncancelled twin does (cancelled={cancelled}): {seen}"
        );
        assert!(
            titles(&session).await.iter().any(|t| t == title),
            "the write COMMITS either way (cancelled={cancelled})"
        );
    }
}

/// **(g)** After a STATELESS first request opens the latch, a `notifications/cancelled` for a second
/// request held inside its storage write reaches rmcp's serve loop: once the held request is released
/// and the handler-returned signal fires, stdin closes, stdout drains to EOF (bounded at 5 s,
/// `rmcp-3.10.0/src/service.rs:1788-1830`) and NO frame carries the held id, while the write committed.
///
/// Mutant: a latch that opens only on `InitializeResult` — the cancellation is then dropped by the
/// gate and the held request gets its reply.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancellation_after_a_stateless_start_reaches_rmcp() {
    let (mut client, gate, returned, session) = held_server().await;
    let started = send(
        &mut client,
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"_meta": meta_2026()}}),
    )
    .await;
    assert!(started["result"].get("tools").is_some(), "{started}");

    client
        .write_raw_line(
            &serde_json::to_string(&create_frame(2, "stateless-held", Some(meta_2026())))
                .expect("frame"),
        )
        .await;
    gate.await_all_entered(1).await;
    client
        .write_raw_line(r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2,"reason":"test"}}"#)
        .await;
    // Inbound frames are handled in arrival order: this answered request proves the cancellation
    // was registered before the release.
    let barrier = send(
        &mut client,
        &json!({"jsonrpc":"2.0","id":3,"method":"tools/list","params":{"_meta": meta_2026()}}),
    )
    .await;
    assert!(barrier["result"].get("tools").is_some(), "{barrier}");
    gate.release().await;
    returned
        .acquire_many(1)
        .await
        .expect("the held call_tool returned")
        .forget();

    let mut seen = client.seen_lines.join("\n");
    seen.push('\n');
    client.close_write().await;
    seen.push_str(&drain_to_eof(client).await);
    assert!(
        !seen
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .any(|frame| frame["id"] == 2),
        "the cancelled stateless request gets NO reply: {seen}"
    );
    assert!(
        titles(&session).await.iter().any(|t| t == "stateless-held"),
        "its write committed"
    );
}
