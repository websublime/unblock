//! ub-m07 wire repro (D18 addendum): `query{kind:"ready"}` honours the `FilterInput` narrowing facets
//! and `offset`/`limit` over a live duplex against the REAL server. Before the fix, ready ignored
//! `labels_any`/`text_contains`/`status`/`offset`. No schema bytes move (`CONTRACT_HASH` unchanged).

mod common;

use chrono::{TimeZone, Utc};
use common::{call_tool, connect};
use serde_json::{Value, json};
use unblock_model::{Issue, Priority, Status};

/// An open issue with a fixed `created_at` (seconds since the epoch).
fn row(id: &str, title: &str, priority: Priority, labels: &[&str], secs: i64) -> Issue {
    let at = Utc.timestamp_opt(secs, 0).single().expect("valid ts");
    Issue {
        id: id.to_string(),
        title: title.to_string(),
        priority,
        status: Status::Open,
        labels: labels.iter().map(|l| (*l).to_string()).collect(),
        created_at: at,
        updated_at: at,
        ..Issue::default()
    }
}

/// The ordered ids of a CD-2 `{"issues":[…]}` query result.
fn ids(out: &Value) -> Vec<String> {
    out["issues"]
        .as_array()
        .expect("CD-2 object-wrapped {\"issues\":[…]}")
        .iter()
        .map(|issue| issue["id"].as_str().unwrap_or("").to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn query_ready_honours_filters_over_the_wire() {
    let session = common::session().await;
    // Two ready-eligible open issues: one labelled `x`, one unlabelled.
    session
        .create(&row("ub-x", "fix the parser", Priority::HIGH, &["x"], 1002))
        .await
        .expect("create");
    session
        .create(&row(
            "ub-plain",
            "fix the parser",
            Priority::CRITICAL,
            &[],
            1003,
        ))
        .await
        .expect("create");
    let (client, server, _cancel) = connect(session.clone()).await;

    // The repro: labels_any=["x"] returns ONLY the labelled issue (pre-fix it returned both).
    let (is_error, out) = call_tool(
        &client,
        "query",
        json!({ "kind": "ready", "labels_any": ["x"] }),
    )
    .await;
    assert!(!is_error, "ready query must succeed");
    assert_eq!(ids(&out), ["ub-x"], "labels_any narrows ready");

    // Extended fixture: the F-DECOY `text_contains` decoy (a second ready row, later in
    // `cmp_ready` order) and an unblocked, undeferred `in_progress` row.
    session
        .create(&row(
            "ub-text",
            "fix the lexer",
            Priority::MEDIUM,
            &["x"],
            1000,
        ))
        .await
        .expect("create");
    session
        .create(&row(
            "ub-wip",
            "fix the parser",
            Priority::MEDIUM,
            &[],
            1001,
        ))
        .await
        .expect("create");
    session.claim("ub-wip", "bob").await.expect("claim");

    // Non-vacuity: the unfiltered ready set in `cmp_ready` order (bucket 0 oldest-first, then
    // bucket 1). `Session::ready` re-sorts, so this unpaged call cannot tell the storage order apart.
    let (e, out) = call_tool(&client, "query", json!({ "kind": "ready" })).await;
    assert!(!e, "{out}");
    assert_eq!(
        ids(&out),
        ["ub-x", "ub-plain", "ub-text"],
        "unfiltered ready"
    );

    let (e, out) = call_tool(
        &client,
        "query",
        json!({ "kind": "ready", "text_contains": "parser" }),
    )
    .await;
    assert!(!e, "{out}");
    assert_eq!(
        ids(&out),
        ["ub-x", "ub-plain"],
        "text_contains narrows ready"
    );

    let (e, out) = call_tool(
        &client,
        "query",
        json!({ "kind": "ready", "status": ["in_progress"] }),
    )
    .await;
    assert!(!e, "{out}");
    assert!(ids(&out).is_empty(), "status=[in_progress] ⇒ []");

    let (e, out) = call_tool(
        &client,
        "query",
        json!({ "kind": "ready", "offset": 1, "limit": 1 }),
    )
    .await;
    assert!(!e, "{out}");
    assert_eq!(
        ids(&out),
        ["ub-plain"],
        "offset=1,limit=1 ⇒ the second issue in cmp_ready order"
    );

    // The paged call is where the storage order and offset show. Rows: ub-x P1 @1002, ub-plain P0
    // @1003, ub-text P2 @1000 (ub-wip is in_progress, never ready). cmp_ready order =
    // [ub-x, ub-plain, ub-text] ⇒ offset=1,limit=2 = [ub-plain, ub-text]. Wrong behaviours (after the
    // engine re-sort): pre-fix (priority ASC, offset ignored) [ub-plain, ub-x] → [ub-x, ub-plain];
    // priority ASC + correct offset [ub-x, ub-text]; correct order, offset ignored [ub-x, ub-plain].
    let (e, out) = call_tool(
        &client,
        "query",
        json!({ "kind": "ready", "offset": 1, "limit": 2 }),
    )
    .await;
    assert!(!e, "{out}");
    assert_eq!(
        ids(&out),
        ["ub-plain", "ub-text"],
        "offset=1,limit=2 pages the cmp_ready order after the blocked filter"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}
