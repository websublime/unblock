//! NFR-2 250k-issue `scale` gate through the engine read path (T3.5/D34) — a **timed integration
//! test**, NOT a criterion bench.
//!
//! Seeds the storage-direct, validated-but-non-minted 250k corpus via
//! [`unblock_storage::testkit::seed_corpus`] (batched `create_issues`, NOT the O(N²) engine mint —
//! D34/F-2), wraps it in a real [`Session`] over a live on-disk `.unblock/` (the same
//! `WorkspaceContext` shape `unblock-config` builds), and asserts the engine's list/ready/count read
//! paths stay bounded at 250k and `Session::integrity_check` is clean. The engine adds a policy
//! re-rank over the raw storage read, so this covers the L5 read cost the storage `scale.rs` does not.
//!
//! Gated on `feature = "testkit"` (it needs `seed_corpus`); it compiles to zero tests under a plain
//! `cargo test --workspace`, and runs in the CI `scale` job via
//! `cargo test -p unblock-engine --features testkit --test scale`.
#![cfg(feature = "testkit")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use unblock_config::{ConfigPaths, ResolvedConfig, WorkspaceContext, WorkspaceSource};
use unblock_engine::{Session, SessionConfig};
use unblock_model::{CountGroupBy, ListFilters};
use unblock_storage::testkit::seed_corpus;
use unblock_storage::{DEFAULT_WRITE_LOCK_TIMEOUT_MS, LibsqlStorage, Storage};

/// The v1 NFR-2 acceptance corpus (per-PR).
const SCALE_N: usize = 250_000;

/// A generous per-op boundedness guard (see the storage `scale.rs` note — not a tight NFR-1 budget).
const READ_GUARD: Duration = Duration::from_secs(15);

/// A realistic page size for the list/ready reads at scale (agents page).
const PAGE: usize = 1_000;

/// Build a `Session` over a fresh on-disk `.unblock/` with a file-backed store seeded with `n`
/// issues (storage-direct — F-2), returning the session and its owning tempdir.
async fn build_session(n: usize) -> (Session, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let workspace_dir = tmp.path().to_path_buf();
    let unblock_dir = workspace_dir.join(".unblock");
    std::fs::create_dir_all(&unblock_dir).expect("create .unblock");

    let config = ResolvedConfig::default();
    let db_path = unblock_dir.join(&config.db_filename);
    let jsonl_path = unblock_dir.join(&config.jsonl_filename);

    let storage = LibsqlStorage::open_local(&db_path, DEFAULT_WRITE_LOCK_TIMEOUT_MS)
        .await
        .expect("open_local");
    storage.migrate().await.expect("migrate");
    seed_corpus(&storage, n).await.expect("seed corpus");
    let storage: Arc<dyn Storage> = Arc::new(storage);

    let paths = ConfigPaths {
        db_path,
        jsonl_path,
        unblock_dir,
    };
    let ctx = WorkspaceContext {
        storage,
        workspace_dir,
        actor: "scale".to_string(),
        config,
        paths,
        source: WorkspaceSource::WalkUp,
        schema_version_before_migrate: 0,
    };
    let session = Session::open(ctx, SessionConfig::default())
        .await
        .expect("open session");
    (session, tmp)
}

/// Seed `n` issues and assert the engine read paths stay bounded + integrity is clean at that scale.
async fn run_scale(n: usize) {
    let t = Instant::now();
    let (session, _tmp) = build_session(n).await;
    eprintln!(
        "engine scale: built + seeded {n} issues in {:?}",
        t.elapsed()
    );

    let filters = ListFilters::default();

    // Full-corpus presence via an O(N) count (no per-row hydration).
    let t = Instant::now();
    let total: usize = session
        .count(&filters, None)
        .await
        .expect("count total")
        .iter()
        .map(|b| b.count)
        .sum();
    let count_elapsed = t.elapsed();
    eprintln!("engine scale: count(total) = {total} in {count_elapsed:?}");
    assert_eq!(total, n, "every seeded row is present");
    assert!(
        count_elapsed < READ_GUARD,
        "count(total) at {n} exceeded the boundedness guard: {count_elapsed:?}"
    );

    // count grouped by status — still an O(N) scan.
    let t = Instant::now();
    let by_status = session
        .count(&filters, Some(CountGroupBy::Status))
        .await
        .expect("count by status");
    let group_elapsed = t.elapsed();
    eprintln!(
        "engine scale: count(status) buckets={} in {group_elapsed:?}",
        by_status.len()
    );
    assert!(
        group_elapsed < READ_GUARD,
        "count(status) at {n} exceeded the boundedness guard: {group_elapsed:?}"
    );

    // list — a realistic page at scale (the ordering index keeps it bounded).
    let paged = ListFilters {
        limit: Some(PAGE),
        ..ListFilters::default()
    };
    let t = Instant::now();
    let listed = session.list(&paged).await.expect("list page");
    let list_elapsed = t.elapsed();
    eprintln!(
        "engine scale: list(limit={PAGE}) = {} in {list_elapsed:?}",
        listed.len()
    );
    assert_eq!(listed.len(), PAGE, "the page is full at scale");
    assert!(
        list_elapsed < READ_GUARD,
        "list(limit={PAGE}) at {n} exceeded the boundedness guard: {list_elapsed:?}"
    );

    // ready — the same page; every seeded issue is open + undeferred + unblocked, and the engine
    // policy re-rank runs over the candidate set.
    let t = Instant::now();
    let ready = session.ready(&paged).await.expect("ready page");
    let ready_elapsed = t.elapsed();
    eprintln!(
        "engine scale: ready(limit={PAGE}) = {} in {ready_elapsed:?}",
        ready.len()
    );
    assert_eq!(ready.len(), PAGE, "the ready page is full at scale");
    assert!(
        ready_elapsed < READ_GUARD,
        "ready(limit={PAGE}) at {n} exceeded the boundedness guard: {ready_elapsed:?}"
    );

    // Clean integrity at scale (NFR-2).
    let t = Instant::now();
    let problems = session.integrity_check().await.expect("integrity_check");
    eprintln!(
        "engine scale: integrity_check problems={} in {:?}",
        problems.len(),
        t.elapsed()
    );
    assert!(
        problems.is_empty(),
        "integrity_check must be clean at {n}: {problems:?}"
    );

    #[cfg(feature = "health")]
    measure_d45_doctor_cost(&session, n).await;

    drop(session);
}

/// D45 — the doctor COST measurement the spine makes a standing obligation
/// (`docs/plans/01-design-spine.md`, the `Session::doctor()` row, "D45 — COST"): the number lives in
/// a test that re-derives it on every run, not in prose that can go stale.
///
/// **This measurement is why the D45 listing view is ONE SQL query.** As first shipped the fold was a
/// two-read engine composition (whole-graph edges differenced against a fully-inclusive `list_issues`
/// id set). THIS cell measured it on CI at 250k rows — `integrity_check` 5.51s, the fold 10.72s,
/// `doctor()` 16.31s — and the last of those tripped the `READ_GUARD` assertion below, turning a
/// required job RED. A laptop run had reported roughly half that and had been taken as evidence the
/// composition was affordable. The composition is now one `Storage::dangling_dependencies()` read.
/// **The guard was NOT relaxed to accommodate any of this**, which is the only reason the number
/// below can be trusted.
///
/// Two timings, because the clause is about what the FOLD adds:
/// - `diagnostics(Dangling)` is the fold's exact added work (ONE home — `doctor()` awaits the SAME
///   fn rather than recomputing);
/// - `doctor()` is the composed total, so `doctor − dangling` is the pre-D45 doctor.
///
/// HONESTY BOUND, recorded in the spine clause too: `seed_corpus` writes rows with NO dependencies,
/// so this corpus has an EMPTY `dependencies` table — the driving table of the new query. A workspace
/// with a real edge graph pays more, but now in proportion to its EDGE count rather than its ROW
/// count, which is exactly what the amendment bought.
#[cfg(feature = "health")]
async fn measure_d45_doctor_cost(session: &Session, n: usize) {
    use unblock_model::DiagnosticKind;

    let t = Instant::now();
    let dangling = session
        .diagnostics(DiagnosticKind::Dangling, None)
        .await
        .expect("diagnostics(dangling)");
    let dangling_elapsed = t.elapsed();
    eprintln!(
        "engine scale: D45 diagnostics(dangling) findings={} in {dangling_elapsed:?}",
        dangling.findings.len()
    );

    let t = Instant::now();
    let doctor = session.doctor().await.expect("doctor");
    let doctor_elapsed = t.elapsed();
    eprintln!(
        "engine scale: D45 doctor() (integrity + file-state + the dangling fold) findings={} in \
         {doctor_elapsed:?}",
        doctor.findings.len()
    );
    eprintln!(
        "engine scale: D45 doctor() fold share = {dangling_elapsed:?} of {doctor_elapsed:?} at {n} \
         issues"
    );

    assert!(
        dangling.findings.is_empty(),
        "the seeded corpus has no dangling edge: {:?}",
        dangling.findings
    );
    // Boundedness only (the same generous guard the reads above use) — NOT an NFR budget.
    assert!(
        doctor_elapsed < READ_GUARD,
        "doctor() at {n} exceeded the boundedness guard: {doctor_elapsed:?}"
    );
}

/// The per-PR NFR-2 gate through the engine read path: 250k issues, bounded reads, clean integrity.
#[tokio::test(flavor = "multi_thread")]
async fn scale_250k_engine_reads_bounded_and_integrity_clean() {
    run_scale(SCALE_N).await;
}

/// The `#[ignore]`-gated soak variant (the v1.5 1M corpus; run on demand).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "soak: 1M-issue corpus; run on demand (v1.5 gate), not per-PR"]
async fn scale_1m_soak() {
    run_scale(1_000_000).await;
}
