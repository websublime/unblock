//! CD-4 (MCP lifecycle spec) — protocol-version negotiation over a LIVE `mcp_server_duplex_for_test` duplex.
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
//!
//! CD-6 (assumption pin) — the four CD-4 tests above all drive the CLAMPED `mcp_server_duplex_for_test`
//! path, so they prove the clamp WORKS but do NOT pin the rmcp behaviour the clamp COMPENSATES for.
//! The `VersionClampingTransport` is correct only while rmcp's serve-loop keeps deriving the wire
//! version as a lexical `min(client, handler)` — an undocumented internal. Two pins below fail loudly
//! if that assumption ever shifts, pointing maintainers straight at the clamp in `server.rs`:
//! `rmcp_serve_loop_echoes_unsupported_below_latest_version_verbatim` (drives the RAW, UNCLAMPED serve
//! path and asserts rmcp still echoes the bogus version verbatim) and
//! `known_versions_and_latest_match_the_clamp_key_set` (pins the `KNOWN_VERSIONS`/`LATEST` set the
//! clamp keys on).

mod common;

use rmcp::ServiceExt;
use rmcp::model::{ClientCapabilities, ClientInfo, Implementation, ProtocolVersion};
use tokio_util::sync::CancellationToken;
use unblock_engine::Session;
use unblock_mcp::{Quotas, mcp_server_duplex_for_test, mcp_server_duplex_unclamped_for_test};

/// Drive a full initialize handshake with a client that requests `requested`, returning the
/// `protocolVersion` the LIVE server negotiated back (its `InitializeResult`).
async fn negotiated_version(
    session: std::sync::Arc<Session>,
    requested: ProtocolVersion,
) -> ProtocolVersion {
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let cancel = CancellationToken::new();
    let server_task = tokio::spawn(mcp_server_duplex_for_test(
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

// ---------------------------------------------------------------------------------------------
// CD-6 — assumption pins for the rmcp serve-loop negotiation the clamp depends on.
// ---------------------------------------------------------------------------------------------

/// Like [`negotiated_version`], but drives the RAW, UNCLAMPED rmcp serve path
/// ([`mcp_server_duplex_unclamped_for_test`], WITHOUT the `VersionClampingTransport`) — so the pin can
/// observe rmcp 1.7's un-guarded serve-loop version negotiation directly.
async fn raw_negotiated_version(
    session: std::sync::Arc<Session>,
    requested: ProtocolVersion,
) -> ProtocolVersion {
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let cancel = CancellationToken::new();
    let server_task = tokio::spawn(mcp_server_duplex_unclamped_for_test(
        session,
        Quotas::default(),
        None,
        server_io,
        cancel.clone(),
    ));

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

/// CD-6 ASSUMPTION PIN — rmcp 1.7's RAW (unclamped) serve-loop ECHOES an unsupported, below-latest
/// requested `protocolVersion` VERBATIM. This is the exact spec-non-conformant behaviour that
/// [`mcp_server_duplex_for_test`]'s `VersionClampingTransport` compensates for; the four CD-4 tests above
/// only prove the clamp, never the underlying rmcp assumption.
///
/// If this test ever FAILS, rmcp changed its serve-loop version negotiation → the clamp in
/// `crates/unblock-mcp/src/server.rs` may now be wrong or redundant. Re-evaluate (possibly REMOVE) the
/// `VersionClampingTransport` before touching this pin — do NOT "fix" the pin to stay green.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rmcp_serve_loop_echoes_unsupported_below_latest_version_verbatim() {
    let bogus = version("1999-01-01");
    // Preconditions the pin depends on: genuinely UNSUPPORTED and sorting lexically BELOW latest (the
    // exact case `VersionClampingTransport` rewrites). If either changes, the pin no longer isolates
    // the misbehaviour.
    assert!(
        !ProtocolVersion::KNOWN_VERSIONS.contains(&bogus),
        "the pin needs a genuinely UNSUPPORTED version (outside KNOWN_VERSIONS)"
    );
    assert!(
        bogus < ProtocolVersion::LATEST,
        "the pin needs a version sorting lexically BELOW LATEST (the clamp's exact trigger)"
    );

    let negotiated = raw_negotiated_version(common::session().await, bogus.clone()).await;

    assert_eq!(
        negotiated, bogus,
        "PINNED rmcp 1.7 misbehaviour: the RAW (unclamped) serve-loop echoes an unsupported \
         below-latest version VERBATIM — exactly what VersionClampingTransport compensates for (CD-6). \
         If this fails, rmcp changed its version negotiation: re-evaluate / possibly REMOVE the clamp \
         in crates/unblock-mcp/src/server.rs before touching this pin."
    );
}

/// CD-6 SUPPORTED-SET PIN — the clamp treats anything OUTSIDE rmcp's `KNOWN_VERSIONS` as unsupported
/// and rewrites it to `LATEST`. Pin that exact set (order included) and `LATEST` to the wire strings the
/// clamp was designed against, so an rmcp bump that adds / removes / re-orders a supported version — or
/// moves `LATEST` — fails LOUDLY here instead of silently shifting what the clamp preserves vs rewrites.
#[test]
fn known_versions_and_latest_match_the_clamp_key_set() {
    let expected = [
        version("2024-11-05"),
        version("2025-03-26"),
        version("2025-06-18"),
        version("2025-11-25"),
    ];
    assert_eq!(
        ProtocolVersion::KNOWN_VERSIONS,
        expected.as_slice(),
        "rmcp's KNOWN_VERSIONS (the set VersionClampingTransport keys on) changed — re-evaluate the \
         clamp (CD-6): crates/unblock-mcp/src/server.rs"
    );
    assert_eq!(
        ProtocolVersion::LATEST,
        version("2025-11-25"),
        "rmcp's LATEST (the clamp's rewrite target) changed — re-evaluate the clamp (CD-6)"
    );
}
