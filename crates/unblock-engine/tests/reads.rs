//! Read-path integration tests: ready semantics (FR-4/FR-5), hybrid sort (spine §4.1), diagnostics
//! dispatch (FR-15), and reads-succeed-while-a-write-holds-the-permit (FR-10).

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::parked::ParkedStorage;
use common::{add_blocks, issue, session, session_over};
use unblock_engine::DiagnosticKind;
use unblock_model::Priority;

#[tokio::test]
async fn ready_excludes_blocked_deferred_closed_and_reflects_edge_change() {
    let session = session().await;

    // Three open issues.
    for (id, secs) in [("ub-a", 1000), ("ub-b", 1001), ("ub-c", 1002)] {
        session
            .create(&issue(id, Priority::MEDIUM, secs))
            .await
            .expect("create");
    }

    // ub-b is blocked by ub-a; ub-c is deferred.
    add_blocks(&session, "ub-b", "ub-a").await;
    session
        .defer("ub-c", chrono::Utc::now() + chrono::Duration::days(7))
        .await
        .expect("defer");

    let ready_ids: Vec<String> = session
        .ready(&unblock_model::ListFilters::default())
        .await
        .expect("ready")
        .into_iter()
        .map(|i| i.id)
        .collect();
    // Only ub-a is ready (ub-b blocked, ub-c deferred).
    assert_eq!(ready_ids, vec!["ub-a".to_string()]);

    // Close ub-a -> ub-b's only blocker is resolved -> ub-b becomes ready immediately (FR-5).
    session
        .close_with_suggestions("ub-a", None)
        .await
        .expect("close");
    let ready_after: Vec<String> = session
        .ready(&unblock_model::ListFilters::default())
        .await
        .expect("ready")
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert_eq!(ready_after, vec!["ub-b".to_string()]);
}

#[tokio::test]
async fn ready_hybrid_sort_equals_policy_order_on_a_fixed_corpus() {
    let session = session().await;

    // Corpus crossing the P1/P2 bucket boundary with mixed ages.
    // bucket 0 (P0/P1): ub-p1-old (older) then ub-p0-new (newer) -> oldest-first within bucket.
    // bucket 1 (P2/P3): ub-p2-old then ub-p3-new.
    session
        .create(&issue("ub-p0-new", Priority::CRITICAL, 2000))
        .await
        .expect("c");
    session
        .create(&issue("ub-p1-old", Priority::HIGH, 1000))
        .await
        .expect("c");
    session
        .create(&issue("ub-p3-new", Priority::LOW, 2000))
        .await
        .expect("c");
    session
        .create(&issue("ub-p2-old", Priority::MEDIUM, 1000))
        .await
        .expect("c");

    let ready = session
        .ready(&unblock_model::ListFilters::default())
        .await
        .expect("ready");
    let order: Vec<String> = ready.iter().map(|i| i.id.clone()).collect();

    // Independently re-rank the same set via the policy free fn; the engine order must match.
    let mut expected = ready.clone();
    expected.sort_by(unblock_policy::cmp_ready);
    let expected_order: Vec<String> = expected.iter().map(|i| i.id.clone()).collect();
    assert_eq!(order, expected_order);

    // And it is the byte-faithful hybrid order: bucket 0 oldest-first, then bucket 1 oldest-first.
    assert_eq!(
        order,
        vec![
            "ub-p1-old".to_string(), // bucket 0, created 1000
            "ub-p0-new".to_string(), // bucket 0, created 2000
            "ub-p2-old".to_string(), // bucket 1, created 1000
            "ub-p3-new".to_string(), // bucket 1, created 2000
        ]
    );
}

#[tokio::test]
async fn reads_succeed_while_a_write_holds_the_permit() {
    use unblock_storage::Storage;
    // FR-10: a write parked mid-tx holds the engine's single permit; a concurrent read must still
    // return (reads never touch the permit).
    let inner = unblock_storage::LibsqlStorage::open_in_memory()
        .await
        .expect("open");
    inner.migrate().await.expect("migrate");
    // Seed an issue so the concurrent read has something to return.
    inner
        .create_issue(&issue("ub-seed", Priority::MEDIUM, 1000), "tester")
        .await
        .expect("seed");

    let parked: Arc<ParkedStorage> = ParkedStorage::new(Arc::new(inner));
    let storage: Arc<dyn Storage> = parked.clone();
    let session = Arc::new(session_over(storage, unblock_engine::SessionConfig::default()).await);

    // Spawn a write that will park inside create_issue (holding the engine permit).
    let writer_session = session.clone();
    let writer = tokio::spawn(async move {
        writer_session
            .create(&issue("ub-parked", Priority::MEDIUM, 2000))
            .await
    });

    // Wait until the write is parked mid-tx (permit held).
    parked.wait_until_parked().await;

    // A concurrent read must complete WHILE the write holds the permit (FR-10).
    let read = tokio::time::timeout(Duration::from_secs(2), session.get("ub-seed"))
        .await
        .expect("read must not block on the write permit")
        .expect("read ok");
    assert!(read.is_some(), "the seeded issue is readable mid-write");

    // Release the parked write; it completes its tx.
    parked.release();
    let created = writer.await.expect("join").expect("parked write completes");
    assert_eq!(created, "ub-parked");
}

#[tokio::test]
async fn diagnostics_dispatch_covers_every_kind() {
    let session = session().await;
    session
        .create(&issue("ub-0001", Priority::MEDIUM, 1000))
        .await
        .expect("create");

    for kind in [
        DiagnosticKind::Stats,
        DiagnosticKind::Info,
        DiagnosticKind::Where,
        DiagnosticKind::Version,
        DiagnosticKind::Lint,
        DiagnosticKind::Changelog,
        DiagnosticKind::Orphans,
    ] {
        let report = session.diagnostics(kind).await.expect("diagnostics");
        // The report's kind is the INPUT kind (never an integrity placeholder).
        assert_eq!(report.kind, kind);
    }

    // Stats reflects the one created issue in the total.
    let stats = session
        .diagnostics(DiagnosticKind::Stats)
        .await
        .expect("stats");
    let total = stats
        .findings
        .iter()
        .find(|f| f.label == "total")
        .expect("total finding");
    assert_eq!(total.detail, "1");
}
