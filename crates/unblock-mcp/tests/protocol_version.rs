//! CD-4 (MCP lifecycle spec) — protocol-version negotiation over a LIVE `serve_duplex_for_test` duplex.
//!
//! The MCP lifecycle spec requires: if the server SUPPORTS the client's requested `protocolVersion` it
//! MUST echo it; otherwise it MUST answer with a version it supports (SHOULD be its latest). rmcp 1.7's
//! stdio serve-loop, left to itself, echoes ANY client-sent version that sorts lexically below the
//! server's latest — including a bogus, UNSUPPORTED one like `"1999-01-01"`. `unblock` clamps an
//! unsupported inbound version to `LATEST` before that negotiation (a `VersionClampingTransport`), so:
//!
//! - an UNSUPPORTED requested version is answered with a server-supported version (RED before the
//!   clamp — the server echoed the bogus value verbatim; GREEN after — it answers `LATEST`);
//! - a SUPPORTED requested version (latest OR an older known one) is still echoed verbatim.

mod common;

use rmcp::ServiceExt;
use rmcp::model::{ClientCapabilities, ClientInfo, Implementation, ProtocolVersion};
use tokio_util::sync::CancellationToken;
use unblock_engine::Session;
use unblock_mcp::{Quotas, serve_duplex_for_test};

/// Drive a full initialize handshake with a client that requests `requested`, returning the
/// `protocolVersion` the LIVE server negotiated back (its `InitializeResult`).
async fn negotiated_version(
    session: std::sync::Arc<Session>,
    requested: ProtocolVersion,
) -> ProtocolVersion {
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let cancel = CancellationToken::new();
    let server_task = tokio::spawn(serve_duplex_for_test(
        session,
        Quotas::default(),
        None,
        server_io,
        cancel.clone(),
    ));

    // `ClientInfo` (= `InitializeRequestParams`) IS a `ClientHandler`, so serving it drives the
    // handshake with exactly the requested `protocolVersion` on the wire.
    let client_info = ClientInfo::new(
        ClientCapabilities::default(),
        Implementation::from_build_env(),
    )
    .with_protocol_version(requested);
    let client = client_info
        .serve(client_io)
        .await
        .expect("client initializes");
    let server = server_task
        .await
        .expect("server task joins")
        .expect("server starts over duplex");

    let negotiated = client
        .peer_info()
        .expect("server InitializeResult after handshake")
        .protocol_version
        .clone();

    let _ = client.cancel().await;
    let _ = server.cancel().await;
    cancel.cancel();
    negotiated
}

/// Parse a raw version string into a `ProtocolVersion` (deserialize accepts any string, so this can
/// mint an UNKNOWN/unsupported version the typed constants cannot name).
fn version(raw: &str) -> ProtocolVersion {
    serde_json::from_value(serde_json::json!(raw)).expect("parse protocol version")
}

/// CD-4: an UNSUPPORTED requested version is NOT echoed — the server answers with a version it
/// supports (its latest), never the bogus client value.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsupported_requested_version_is_clamped_to_a_supported_one() {
    let bogus = version("1999-01-01");
    let negotiated = negotiated_version(common::session().await, bogus.clone()).await;

    assert_ne!(
        negotiated, bogus,
        "an unsupported version MUST NOT be echoed verbatim (CD-4)"
    );
    assert!(
        ProtocolVersion::KNOWN_VERSIONS.contains(&negotiated),
        "the answered version MUST be one the server supports, got {negotiated} (CD-4)"
    );
    assert_eq!(
        negotiated,
        ProtocolVersion::LATEST,
        "the answer SHOULD be the server's latest supported version (CD-4)"
    );
}

/// CD-4 (the same defect with a future-dated bogus version that sorts ABOVE latest): still answered
/// with a supported version.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn far_future_unsupported_version_is_clamped_to_a_supported_one() {
    let future = version("9999-12-31");
    let negotiated = negotiated_version(common::session().await, future.clone()).await;

    assert_ne!(
        negotiated, future,
        "an unsupported version MUST NOT be echoed (CD-4)"
    );
    assert_eq!(
        negotiated,
        ProtocolVersion::LATEST,
        "answered with latest (CD-4)"
    );
}

/// CD-4: a SUPPORTED requested version (the server's latest) is echoed verbatim — the clamp must not
/// break the normal handshake for current clients.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn supported_latest_version_is_echoed() {
    let latest = ProtocolVersion::LATEST;
    let negotiated = negotiated_version(common::session().await, latest.clone()).await;

    assert_eq!(
        negotiated, latest,
        "a supported (latest) requested version MUST be echoed (CD-4)"
    );
}

/// CD-4: a SUPPORTED but OLDER known version is echoed verbatim (the clamp must not over-rewrite a
/// version the server genuinely speaks — it downgrades, per the spec).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn supported_older_version_is_echoed() {
    // Pick a KNOWN version that is not the latest (guaranteed to exist: rmcp ships several).
    let older = ProtocolVersion::KNOWN_VERSIONS
        .iter()
        .find(|v| **v != ProtocolVersion::LATEST)
        .cloned()
        .expect("at least one non-latest known version");
    let negotiated = negotiated_version(common::session().await, older.clone()).await;

    assert_eq!(
        negotiated, older,
        "a supported older version MUST be echoed (downgrade), not clamped (CD-4)"
    );
}
