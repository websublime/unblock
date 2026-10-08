//! **[v1.0.1/D54] A parse error on a READABLE id, over a live duplex.**
//!
//! Its own file because `envelope_id_duplex.rs`'s module doc is scoped to D47's class, and this
//! class is a different route: the line FAILS the typed parse, so D47's arm never sees it.
//!
//! * The store-EFFECT oracle: a duplicated-`method` `tools/call` naming a DESTRUCTIVE action is
//!   answered `-32700` on its id, and the store does not move.
//! * A REAL rmcp client: the only cell in the tree that observes rmcp's own routing of an error to a
//!   pending request by id. An rmcp client cannot emit a malformed line, so a relay corrupts one of
//!   its requests in flight.

mod common;

use rmcp::ServiceExt;
use rmcp::model::{ClientRequest, ErrorCode, ListToolsRequest, PingRequest};
use rmcp::service::{PeerRequestOptions, ServiceError};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use unblock_mcp::envelope_id_corpus::{
    ISSUE_ID_PLACEHOLDER, ParseExpect, parse_error_bytes, parse_error_corpus,
};
use unblock_mcp::{Quotas, mcp_server_duplex_for_test};

/// **C-E1** — an in-class `tools/call` naming a DESTRUCTIVE action is answered `-32700` ON its id,
/// and executes NOTHING.
///
/// The reply is observed by SENTINEL FOLLOW over `seen_lines`, so a mutant that drops the id fails
/// at once instead of waiting out `read_response`'s deadline (ub-f1k); the fingerprint proves the
/// class never executes (PRD D54 clause (1)'s measured "created NOTHING", on the real serve path).
///
/// Mutants: the recovered arm answering `None`; deliver-instead-of-answer; answer-AND-deliver.
#[tokio::test]
async fn an_in_class_tools_call_is_answered_on_its_id_and_executes_nothing() {
    let session = common::session().await;
    let (mut client, _server, _cancel) = common::connect_raw(session, Quotas::default()).await;

    let target = common::create_issue(&mut client, "D54 store-effect target").await;
    let before = common::store_fingerprint(&mut client, std::slice::from_ref(&target)).await;

    let entry = parse_error_corpus()
        .into_iter()
        .find(|f| f.id == "E19")
        .expect("E19 missing");
    assert_eq!(
        entry.expect,
        ParseExpect::RecoveredNum(54019),
        "E19 must recover id 54019"
    );
    let raw = String::from_utf8(entry.frame.clone())
        .expect("E19 is UTF-8")
        .replace(ISSUE_ID_PLACEHOLDER, &target);
    assert!(
        raw.contains(&target) && raw.contains("delete"),
        "non-vacuity: the line must really name a DESTRUCTIVE action against the live id: {raw}"
    );

    // Sentinel follow, timeout-free: the server answers the bad line before it reads the sentinel.
    client.write_raw_line(&raw).await;
    let sentinel = client.request("ping", serde_json::json!({})).await;
    assert!(
        sentinel.get("result").is_some(),
        "the connection recovers: {sentinel}"
    );
    let expected = String::from_utf8(parse_error_bytes(&entry.expect)).expect("UTF-8");
    assert!(
        client
            .seen_lines
            .iter()
            .any(|line| line == expected.trim_end()),
        "the line must be answered -32700 ON id 54019; lines: {:?}",
        client.seen_lines
    );

    let after = common::store_fingerprint(&mut client, std::slice::from_ref(&target)).await;
    assert_eq!(
        before, after,
        "THE EFFECT ORACLE FAILED — the line was answered but the store MOVED; a line that fails \
         the typed parse must never execute"
    );
}

/// **C-E2** — a REAL rmcp client whose request fails the typed parse in flight is RELEASED by the
/// `-32700` on its id, and the proof needs no timeout.
///
/// A relay rewrites the client's first `tools/list` line into a duplicated `method` (the shape of
/// E01/E02, the client's own id kept) and forwards everything else verbatim. The client sends that
/// request with NO timeout, then a `ping` sentinel. The server writes the first reply before it
/// reads the second line, and the client handles peer messages one at a time in arrival order, so
/// when the sentinel's response has been processed, the first request's outcome is already decided:
/// ONE poll tells released from pending.
///
/// Mutants: the recovered arm answering `None` (the poll finds the request still PENDING). It is
/// also the only pin on the assumption that rmcp's client routes an error to its waiter by id
/// (`rmcp-1.7.0/src/service.rs:1030-1041`).
#[tokio::test]
async fn an_rmcp_client_is_released_by_the_parse_error_on_its_recovered_id() {
    let (client_w, relay_r) = tokio::io::duplex(1 << 20);
    let (relay_w, server_r) = tokio::io::duplex(1 << 20);
    let (server_w, client_r) = tokio::io::duplex(1 << 20);
    let cancel = tokio_util::sync::CancellationToken::new();
    let server = tokio::spawn(mcp_server_duplex_for_test(
        common::session().await,
        Quotas::default(),
        None,
        server_r,
        server_w,
        cancel.clone(),
    ));

    let (mutated_tx, mut mutated_rx) = tokio::sync::oneshot::channel::<String>();
    tokio::spawn(async move {
        const METHOD: &str = r#""method":"tools/list""#;
        let mut reader = BufReader::new(relay_r);
        let mut writer = relay_w;
        let mut mutated_tx = Some(mutated_tx);
        let mut line = String::new();
        while reader.read_line(&mut line).await.expect("relay read") > 0 {
            if mutated_tx.is_some() && line.contains(METHOD) {
                line = line.replacen(METHOD, &format!("{METHOD},{METHOD}"), 1);
                writer
                    .write_all(line.as_bytes())
                    .await
                    .expect("relay write");
                writer.flush().await.expect("relay flush");
                let _ = mutated_tx.take().expect("first rewrite").send(line.clone());
            } else {
                writer
                    .write_all(line.as_bytes())
                    .await
                    .expect("relay write");
                writer.flush().await.expect("relay flush");
            }
            line.clear();
        }
    });

    let client = ().serve((client_r, client_w)).await.expect("client initializes");
    let _server = server.await.expect("server task").expect("server starts");

    let pending = client
        .peer()
        .send_cancellable_request(
            ClientRequest::ListToolsRequest(ListToolsRequest::default()),
            PeerRequestOptions::no_options(),
        )
        .await
        .expect("request sent");

    client
        .peer()
        .send_request(ClientRequest::PingRequest(PingRequest::default()))
        .await
        .expect("the sentinel ping is answered");
    // Checked only AFTER the sentinel: the relay sends the oneshot before it reads the ping line,
    // so a request it never rewrote is an immediate `Empty`, never a hang.
    let injected = mutated_rx
        .try_recv()
        .expect("the relay rewrote the request");
    assert_eq!(
        injected.matches(r#""method":"tools/list""#).count(),
        2,
        "non-vacuity: the request on the wire must carry a duplicated method: {injected}"
    );

    let mut response = std::pin::pin!(pending.await_response());
    let polled = tokio::select! {
        biased;
        r = &mut response => Some(r),
        () = std::future::ready(()) => None,
    };
    match polled {
        Some(Err(ServiceError::McpError(error))) => {
            assert_eq!(error.code, ErrorCode::PARSE_ERROR, "released by the -32700");
        }
        other => panic!(
            "the rmcp client is still PENDING (or released wrongly) after the sentinel: {:?}",
            other.map(|r| r.map(|_| ()))
        ),
    }
    cancel.cancel();
}
