//! `ready → claim → close` round-trip latency instrument (T3.5/D34/F-4, MF-4) — RECORD-ONLY.
//!
//! Measures the end-to-end p50/p99 of the core agent loop — `query{ready}` → `claim` → `issue{close}`
//! — over a **real spawned `unblock mcp` child** speaking MCP JSON-RPC over piped stdio (the D31
//! `.write.lock` world). It lives in `unblock-cli` (NOT `unblock-engine`): the engine (L5) has no
//! `unblock-mcp` dep and cannot spawn `unblock mcp` / reference `CARGO_BIN_EXE_unblock` (an L5→L7
//! back-edge the layering check rejects — MF-4). It reuses the existing `common::McpClient` spawn
//! harness (`mcp_lifecycle.rs` precedent); the `initialize` handshake is the readiness barrier.
//!
//! **Record-only (F-4):** this PUBLISHES the v1 baseline (printed to stderr) with **NO hard latency
//! gate** — a hard target lands in v1.1 with real data. The only assertions are that the loop
//! mechanism actually ran (every op succeeded), so the recorded numbers are non-vacuous; it is NOT a
//! criterion micro-bench (process startup would pollute the sample), it is an integration test with
//! timing.
//!
//! Unix-only: `unblock mcp` is a no-op EOF path on Windows (NFR-11), so a round-trip is meaningful
//! only where the stdio server actually serves — `#![cfg(unix)]` (the `mcp_lifecycle.rs` precedent).
#![cfg(unix)]

mod common;

use std::time::{Duration, Instant};

use common::{McpClient, Workspace, id_set, issue_id};
use serde_json::json;

/// Round-trip iterations (each consumes one seeded issue). Modest so the per-PR cli suite stays fast;
/// large enough for a stable p50/p99.
const N: usize = 60;

#[test]
fn ready_claim_close_round_trip_records_p50_p99() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());
    client.initialize(); // the readiness barrier.

    // Seed N ready issues via the MINTING create path (each fresh issue is Open + unblocked = ready).
    for k in 0..N {
        let (err, created) = client.call_tool(
            "issue",
            &json!({"action": "create", "title": format!("round-trip {k}")}),
        );
        assert!(!err, "seed create must succeed: {created}");
        // Prove the id is well-formed (non-vacuous seed).
        let _ = issue_id(&created);
    }

    // Time N × (query{ready} → claim → close). Each iteration claims + closes one ready issue, so the
    // ready set shrinks by one and the next iteration always finds a fresh ready id.
    let mut samples: Vec<Duration> = Vec::with_capacity(N);
    for k in 0..N {
        let start = Instant::now();

        let (err, ready) = client.call_tool("query", &json!({"kind": "ready"}));
        assert!(!err, "query ready must succeed at iter {k}: {ready}");
        let ready_ids = id_set(&ready);
        let id = ready_ids
            .iter()
            .next()
            .unwrap_or_else(|| panic!("a ready issue must exist at iter {k}: {ready}"))
            .clone();

        let (err, claimed) = client.call_tool("claim", &json!({"id": id, "assignee": "agent-rt"}));
        assert!(!err, "claim must succeed at iter {k}: {claimed}");

        let (err, closed) = client.call_tool("issue", &json!({"action": "close", "id": id}));
        assert!(!err, "close must succeed at iter {k}: {closed}");

        samples.push(start.elapsed());
    }

    assert_eq!(samples.len(), N, "every round-trip iteration completed");

    // Nearest-rank percentiles (record-only; NO latency assertion — F-4).
    samples.sort_unstable();
    let p50 = samples[N / 2];
    let p99 = samples[(N * 99 / 100).min(N - 1)];
    let min = samples[0];
    let max = samples[N - 1];

    // Structured, greppable stderr record (diagnostics to stderr, NFR-14). Published as the v1
    // baseline; the hard target is v1.1 (D34/F-4).
    eprintln!(
        "round-trip[ready->claim->close] n={N} p50={p50:?} p99={p99:?} min={min:?} max={max:?}"
    );

    // Clean shutdown (EOF → exit 0).
    client.close_stdin();
    let status = common::wait_for(&mut client.child, Duration::from_secs(20));
    assert_eq!(
        status.code(),
        Some(0),
        "clean EOF exit 0 after the round-trip"
    );
}
