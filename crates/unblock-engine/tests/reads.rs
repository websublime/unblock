//! Read-path integration tests: ready semantics (FR-4/FR-5), hybrid sort (spine §4.1), diagnostics
//! dispatch (FR-15), and reads-succeed-while-a-write-holds-the-permit (FR-10).

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::parked::ParkedStorage;
use common::{add_blocks, issue, seed_open, session, session_over};
use unblock_engine::DiagnosticKind;
use unblock_model::{CountGroupBy, ListFilters, Priority};

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
async fn search_applies_default_cap_when_limit_unset_and_honours_an_explicit_limit() {
    // FR-4: with no `filters.limit`, the engine fills the default `search_cap` (50). Seed 55 matching
    // issues so the uncapped result would be 55 — the cap must clamp it to 50; an explicit small
    // limit must be honoured verbatim.
    let session = session().await;
    seed_open(&session, 55).await; // titles are "issue ub-XXXX" — all match the needle "issue".

    // No limit set -> the engine applies the default cap of 50.
    let capped = session
        .search("issue", &ListFilters::default())
        .await
        .expect("search");
    assert_eq!(
        capped.len(),
        50,
        "default search_cap (50) must clamp the result"
    );

    // An explicit limit is honoured verbatim (the engine does not override a set limit).
    let limited = session
        .search(
            "issue",
            &ListFilters {
                limit: Some(7),
                ..ListFilters::default()
            },
        )
        .await
        .expect("search");
    assert_eq!(limited.len(), 7, "an explicit limit must be honoured");
}

#[tokio::test]
async fn count_group_by_status_buckets_sum_to_total() {
    // FR-4: count with a group-by returns per-key buckets; a separate close moves one issue into a
    // distinct status bucket. The buckets must sum to the grand total.
    let session = session().await;
    for (id, secs) in [("ub-a", 1000), ("ub-b", 1001), ("ub-c", 1002)] {
        session
            .create(&issue(id, Priority::MEDIUM, secs))
            .await
            .expect("create");
    }
    // Close one so two statuses exist (open + closed).
    session
        .close_with_suggestions("ub-c", None)
        .await
        .expect("close");

    let filters = ListFilters {
        include_closed: true,
        ..ListFilters::default()
    };
    let buckets = session
        .count(&filters, Some(CountGroupBy::Status))
        .await
        .expect("count");
    let total: usize = buckets.iter().map(|b| b.count).sum();
    assert_eq!(total, 3, "buckets must sum to the 3 issues");
    // At least two distinct status keys are present (open + closed).
    assert!(
        buckets.len() >= 2,
        "a closed issue creates a second status bucket, got {buckets:?}"
    );
}

#[tokio::test]
async fn stale_returns_only_issues_older_than_the_cutoff() {
    // FR-4: `stale(older_than, filters)` returns issues whose `updated_at` predates the cutoff.
    let session = session().await;
    // Two issues created at fixed deterministic timestamps (1000s and 5000s epoch via the corpus
    // builder), so a cutoff between them isolates exactly the older one.
    session
        .create(&issue("ub-old", Priority::MEDIUM, 1000))
        .await
        .expect("create old");
    session
        .create(&issue("ub-new", Priority::MEDIUM, 5000))
        .await
        .expect("create new");

    // Cutoff at 3000s: only ub-old (updated_at == 1000s) is older.
    let cutoff = common::t(3000);
    let stale: Vec<String> = session
        .stale(cutoff, &ListFilters::default())
        .await
        .expect("stale")
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert_eq!(
        stale,
        vec!["ub-old".to_string()],
        "only the older issue is stale"
    );
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

#[tokio::test]
async fn diagnostics_report_shapes_are_golden_pinned() {
    // Golden-pin the v1 DiagnosticReport SHAPE per kind (label set + per-kind detail policy) over a
    // fixed, deterministic corpus, so T2.7 (changelog `since`, richer lint) cannot silently drift the
    // wire shapes. Volatile details (absolute paths in Info/Where, the crate version, env-specific
    // workspace facts) are REDACTED — we pin the structure, not the machine.
    let session = session().await;
    // A deterministic corpus: one open issue + one closed issue (so Stats/Changelog are non-empty).
    session
        .create(&issue("ub-open", Priority::MEDIUM, 1000))
        .await
        .expect("create open");
    session
        .create(&issue("ub-done", Priority::HIGH, 2000))
        .await
        .expect("create done");
    session
        .close_with_suggestions("ub-done", Some("finished".to_string()))
        .await
        .expect("close");

    // For each kind, render a stable "kind: [label=detail, ...]" line, redacting the volatile labels.
    let volatile = [
        "workspace_dir",
        "db_path",
        "jsonl_path",
        "unblock_dir",
        "version",
    ];
    let mut lines = Vec::new();
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
        let rendered: Vec<String> = report
            .findings
            .iter()
            .map(|f| {
                if volatile.contains(&f.label.as_str()) {
                    format!("{}=<redacted>", f.label)
                } else {
                    format!("{}={}", f.label, f.detail)
                }
            })
            .collect();
        lines.push(format!("{kind:?}: [{}]", rendered.join(", ")));
    }

    insta::assert_snapshot!("diagnostics_report_shapes", lines.join("\n"));
}
