//! NFR-2 250k-issue `scale` gate (T3.5/D34) — a **timed integration test**, NOT a criterion bench.
//!
//! Seeds the storage-direct, validated-but-non-minted 250k corpus via
//! [`unblock_storage::testkit::seed_corpus`] (batched `create_issues` ~1k/tx + per-issue
//! `IssueValidator::validate`, NOT the O(N²) engine mint — D34/F-2) over a **file-backed** libsql DB
//! with the live D31 `.write.lock` on the write path, then asserts the corpus is fully present, the
//! read paths (list/ready/count) stay bounded at scale, and `integrity_check()` is clean at 250k.
//!
//! The corpus is committed to under NFR-2. There is no numeric NFR-1 *budget* at 250k (the numeric
//! budgets are the 1k/10k criterion tiers; the 1M corpus is a v1.5 gate), so the per-op guards here
//! are **generous boundedness guards** — they prove the paths do not blow up (an O(N)→O(N²)
//! regression or a missing index would breach them by orders of magnitude) — and the real elapsed is
//! printed for the record. list/ready use a realistic `limit` (agents page); an unbounded 250k
//! hydration is not a realistic read shape and would only measure the known per-row-hydration cost.
//!
//! Gated on `feature = "testkit"` (it needs `seed_corpus`); it compiles to zero tests under a plain
//! `cargo test --workspace`, and runs in the dedicated CI `scale` job via
//! `cargo test -p unblock-storage --features testkit --test scale`.
#![cfg(feature = "testkit")]

use std::time::{Duration, Instant};

use unblock_model::{CountGroupBy, ListFilters};
use unblock_storage::testkit::seed_corpus;
use unblock_storage::{DEFAULT_WRITE_LOCK_TIMEOUT_MS, LibsqlStorage, Storage};

/// The v1 NFR-2 acceptance corpus (per-PR).
const SCALE_N: usize = 250_000;

/// A generous per-op boundedness guard (see the module doc — not a tight NFR-1 budget).
const READ_GUARD: Duration = Duration::from_secs(15);

/// A realistic page size for the list/ready reads at scale (agents page; an unbounded 250k hydration
/// is not a realistic read shape).
const PAGE: usize = 1_000;

/// Open a fresh file-backed migrated libsql DB under a temp dir.
async fn open_file_db(dir: &std::path::Path) -> LibsqlStorage {
    let db = dir.join("unblock.db");
    let storage = LibsqlStorage::open_local(&db, DEFAULT_WRITE_LOCK_TIMEOUT_MS)
        .await
        .expect("open_local");
    storage.migrate().await.expect("migrate");
    storage
}

/// Seed `n` issues and assert the read paths stay bounded + integrity is clean at that scale.
async fn run_scale(n: usize) {
    let dir = tempfile::tempdir().expect("tempdir");
    let storage = open_file_db(dir.path()).await;

    let t = Instant::now();
    seed_corpus(&storage, n).await.expect("seed corpus");
    eprintln!("scale: seeded {n} issues in {:?}", t.elapsed());

    // Full-corpus presence: an ungrouped count is an O(N) scan (no per-row hydration), so it is the
    // cheap, exact proof every seeded row committed.
    let filters = ListFilters::default();
    let t = Instant::now();
    let total: usize = storage
        .count_issues(&filters, None)
        .await
        .expect("count total")
        .iter()
        .map(|b| b.count)
        .sum();
    let count_elapsed = t.elapsed();
    eprintln!("scale: count(total) = {total} in {count_elapsed:?}");
    assert_eq!(total, n, "every seeded row is present");
    assert!(
        count_elapsed < READ_GUARD,
        "count(total) at {n} exceeded the boundedness guard: {count_elapsed:?}"
    );

    // Grouped count (status bucket) — still an O(N) scan.
    let t = Instant::now();
    let by_status = storage
        .count_issues(&filters, Some(CountGroupBy::Status))
        .await
        .expect("count by status");
    let group_elapsed = t.elapsed();
    eprintln!(
        "scale: count(status) buckets={} in {group_elapsed:?}",
        by_status.len()
    );
    assert!(
        group_elapsed < READ_GUARD,
        "count(status) at {n} exceeded the boundedness guard: {group_elapsed:?}"
    );

    // list — a realistic page (limit) at scale; the ordering index keeps this bounded.
    let paged = ListFilters {
        limit: Some(PAGE),
        ..ListFilters::default()
    };
    let t = Instant::now();
    let listed = storage.list_issues(&paged).await.expect("list page");
    let list_elapsed = t.elapsed();
    eprintln!(
        "scale: list(limit={PAGE}) = {} in {list_elapsed:?}",
        listed.len()
    );
    assert_eq!(listed.len(), PAGE, "the page is full at scale");
    assert!(
        list_elapsed < READ_GUARD,
        "list(limit={PAGE}) at {n} exceeded the boundedness guard: {list_elapsed:?}"
    );

    // ready — the same realistic page; every seeded issue is open + undeferred + unblocked, so the
    // ready candidate set is the whole corpus, narrowed by the page.
    let t = Instant::now();
    let ready = storage.ready_issues(&paged).await.expect("ready page");
    let ready_elapsed = t.elapsed();
    eprintln!(
        "scale: ready(limit={PAGE}) = {} in {ready_elapsed:?}",
        ready.len()
    );
    assert_eq!(ready.len(), PAGE, "the ready page is full at scale");
    assert!(
        ready_elapsed < READ_GUARD,
        "ready(limit={PAGE}) at {n} exceeded the boundedness guard: {ready_elapsed:?}"
    );

    // Clean integrity at scale (NFR-2): the seeded corpus is structurally sound.
    let t = Instant::now();
    let problems = storage.integrity_check().await.expect("integrity_check");
    eprintln!(
        "scale: integrity_check problems={} in {:?}",
        problems.len(),
        t.elapsed()
    );
    assert!(
        problems.is_empty(),
        "integrity_check must be clean at {n}: {problems:?}"
    );

    drop(storage);
    drop(dir);
}

/// The per-PR NFR-2 gate: 250k issues, bounded reads, clean integrity.
#[tokio::test(flavor = "multi_thread")]
async fn scale_250k_reads_bounded_and_integrity_clean() {
    run_scale(SCALE_N).await;
}

/// The `#[ignore]`-gated soak variant (the v1.5 1M corpus; run on demand). Same shape at 4× scale —
/// proves the storage-direct seeder + read paths + integrity hold well past the v1 acceptance corpus.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "soak: 1M-issue corpus; run on demand (v1.5 gate), not per-PR"]
async fn scale_1m_soak() {
    run_scale(1_000_000).await;
}

/// More issues than `SQLite`'s default bound-parameter ceiling (32 766 since 3.32): a rename that bound
/// the affected ids as one list would fail outright here.
const RENAME_SCALE_N: usize = 40_000;

/// D56 clause (5) SCALE (NFR-2): `rename_label` over a label carried by more issues than the
/// bound-parameter limit succeeds in ONE transaction, because its merge `DELETE` and rename `UPDATE`
/// select rows by SUBQUERY and every per-issue statement binds a fixed handful of parameters —
/// never a bound id list (the constraint `HYDRATE_ID_CHUNK` exists for). Elapsed is printed for the
/// record and held to the generous boundedness guard.
#[tokio::test(flavor = "multi_thread")]
async fn scale_rename_label_past_the_bound_parameter_limit() {
    use chrono::{TimeZone, Utc};
    use unblock_model::Issue;

    let dir = tempfile::tempdir().expect("tempdir");
    let storage = open_file_db(dir.path()).await;
    let epoch = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();

    let t = Instant::now();
    let ids: Vec<usize> = (0..RENAME_SCALE_N).collect();
    for chunk in ids.chunks(1_000) {
        let batch: Vec<Issue> = chunk
            .iter()
            .map(|i| Issue {
                id: format!("ub-rn{i:06}"),
                title: format!("rename scale {i}"),
                created_at: epoch,
                updated_at: epoch,
                // Every tenth issue already carries the target name, so the merge path runs at
                // scale too.
                labels: if i % 10 == 0 {
                    vec!["bulk-new".to_string(), "bulk-old".to_string()]
                } else {
                    vec!["bulk-old".to_string()]
                },
                ..Issue::default()
            })
            .collect();
        storage
            .create_issues(&batch, "scale")
            .await
            .expect("seed batch");
    }
    eprintln!(
        "scale: seeded {RENAME_SCALE_N} labelled issues in {:?}",
        t.elapsed()
    );

    let t = Instant::now();
    let affected = storage
        .rename_label("bulk-old", "bulk-new", "scale")
        .await
        .expect("rename past the bound-parameter limit");
    let rename_elapsed = t.elapsed();
    eprintln!("scale: rename_label({RENAME_SCALE_N}) in {rename_elapsed:?}");
    assert_eq!(
        affected, RENAME_SCALE_N,
        "merged + renamed issues are all counted"
    );
    assert!(
        rename_elapsed < Duration::from_mins(2),
        "rename at {RENAME_SCALE_N} exceeded the boundedness guard: {rename_elapsed:?}"
    );

    let catalog = storage.list_all_labels().await.expect("catalog");
    assert_eq!(
        catalog,
        vec![unblock_model::LabelCount {
            label: "bulk-new".to_string(),
            count: RENAME_SCALE_N,
        }],
        "every live `bulk-old` row became (or merged into) `bulk-new`"
    );

    drop(storage);
    drop(dir);
}
