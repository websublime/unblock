//! Protocol-version negotiation over a LIVE `mcp_server_duplex_for_test` duplex — the SHIPPED
//! composition (PRD §4 D59 clauses (9) and (10), criterion (h)).
//!
//! The CD-4 version clamp is RETIRED: rmcp 1.7 derived the wire version as a lexical
//! `min(client, handler)`, which the clamp compensated for; rmcp 3.10 negotiates per the MCP
//! lifecycle specification itself (`negotiate_protocol_version`,
//! `rmcp-3.10.0/src/service/server.rs:478-509`). A client that sends `initialize` gets its own
//! version echoed when it is one of the four handshake versions, and 2025-11-25 otherwise — a
//! requested 2026-07-28 (which has no handshake), an unknown older and an unknown later string
//! included. Those are exactly the answers rmcp 1.7 plus the clamp gave.
//!
//! CD-6 (assumption pins) are RE-POINTED, never dropped: the seven answers below go through the
//! shipped composition, and `known_versions_latest_and_latest_with_initialize_are_pinned` pins
//! `KNOWN_VERSIONS` (five, in order), `LATEST` and `LATEST_WITH_INITIALIZE`, so an rmcp bump that
//! moves negotiation turns a cell red.

mod common;

use rmcp::ServiceExt;
use rmcp::model::{ClientCapabilities, ClientConfig, Implementation, ProtocolVersion};
use tokio_util::sync::CancellationToken;
use unblock_engine::Session;
use unblock_mcp::{Quotas, mcp_server_duplex_for_test};

/// Drive a full initialize handshake with a client that requests `requested`, returning the
/// `protocolVersion` the LIVE server negotiated back (its `InitializeResult`).
async fn negotiated_version(
    session: std::sync::Arc<Session>,
    requested: ProtocolVersion,
) -> ProtocolVersion {
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    // D43: the helper takes the byte streams themselves — the scanning transport owns the read
    // framing.
    let (server_read, server_write) = tokio::io::split(server_io);
    let cancel = CancellationToken::new();
    let server_task = tokio::spawn(mcp_server_duplex_for_test(
        session,
        Quotas::default(),
        None,
        server_read,
        server_write,
        cancel.clone(),
    ));

    // `ClientConfig` (= `InitializeRequestParams`) IS a `ClientHandler`, so serving it drives the
    // `initialize` handshake with exactly the requested `protocolVersion` on the wire.
    let client_config = ClientConfig::new(
        ClientCapabilities::default(),
        Implementation::from_build_env(),
    )
    .with_protocol_version(requested);
    let client = client_config
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

/// The four handshake versions are each echoed verbatim (D59 clause (9), criterion (f1)/(h)).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_supported_handshake_version_is_echoed() {
    for raw in ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"] {
        let requested = version(raw);
        let negotiated = negotiated_version(common::session().await, requested.clone()).await;
        assert_eq!(
            negotiated, requested,
            "a supported handshake version MUST be echoed (MCP lifecycle, D59 clause (9))"
        );
    }
}

/// A requested 2026-07-28 has NO `initialize` handshake, so the answer is the newest version that
/// does — 2025-11-25 (D59 clause (9)).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_requested_2026_07_28_is_answered_with_2025_11_25() {
    let negotiated =
        negotiated_version(common::session().await, ProtocolVersion::V_2026_07_28).await;
    assert_eq!(
        negotiated,
        version("2025-11-25"),
        "2026-07-28 has no handshake, so `initialize` is answered with 2025-11-25"
    );
}

/// An UNKNOWN older version is NOT echoed — it is answered with 2025-11-25, as the clamp answered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_older_version_is_answered_with_2025_11_25() {
    let bogus = version("1999-01-01");
    let negotiated = negotiated_version(common::session().await, bogus.clone()).await;
    assert_ne!(
        negotiated, bogus,
        "an unsupported version MUST NOT be echoed"
    );
    assert_eq!(negotiated, version("2025-11-25"));
}

/// An UNKNOWN later version is answered with 2025-11-25 too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_later_version_is_answered_with_2025_11_25() {
    let future = version("2099-01-01");
    let negotiated = negotiated_version(common::session().await, future.clone()).await;
    assert_ne!(
        negotiated, future,
        "an unsupported version MUST NOT be echoed"
    );
    assert_eq!(negotiated, version("2025-11-25"));
}

/// CD-6 SUPPORTED-SET PIN, re-pointed by D59 clause (10). rmcp 3.10's negotiation keys on
/// `KNOWN_VERSIONS` (the default `supported_protocol_versions()` this server keeps), `LATEST` and
/// `LATEST_WITH_INITIALIZE`; an rmcp bump that adds, removes or re-orders a version, or moves either
/// constant, fails LOUDLY here — re-derive D59 clause (9)'s answers before touching this pin.
#[test]
fn known_versions_latest_and_latest_with_initialize_are_pinned() {
    let expected = [
        version("2024-11-05"),
        version("2025-03-26"),
        version("2025-06-18"),
        version("2025-11-25"),
        version("2026-07-28"),
    ];
    assert_eq!(
        ProtocolVersion::KNOWN_VERSIONS,
        expected.as_slice(),
        "rmcp's KNOWN_VERSIONS changed — re-derive D59 clause (9)"
    );
    assert_eq!(
        ProtocolVersion::LATEST,
        version("2026-07-28"),
        "rmcp's LATEST changed — re-derive D59 clause (9)"
    );
    assert_eq!(
        ProtocolVersion::LATEST_WITH_INITIALIZE,
        version("2025-11-25"),
        "rmcp's LATEST_WITH_INITIALIZE (the fallback `initialize` answer) changed — re-derive D59 \
         clause (9)"
    );
}
