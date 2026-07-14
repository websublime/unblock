//! Storage-layer NFR-1 HYBRID perf gate (T3.5/D34) — the FIRST `async_tokio` criterion bench.
//!
//! Measures the five LOCKED groups (`unblock-storage.md` §3.4) over a **file-backed** libsql DB (so
//! the D31 `.write.lock` + WAL are on the write path, not the non-WAL shared-cache in-memory path):
//!
//! - **`storage_create`** — a single [`Storage::create_issue`] into a fresh, small-seeded DB per
//!   iteration (via `iter_batched`), so the measured op is one insert, never a growing DB.
//! - **`storage_list` / `storage_ready`** — the NFR-1 read budgets over a corpus seeded ONCE at 1k
//!   and 10k (outside the timing loop). These flow through the production per-row-hydration read path
//!   (`collect_hydrated`), so they carry the real end-to-end cost the budget must bound.
//! - **`storage_count` / `storage_search`** — recorded-only (no hard ceiling in v1, PRD NFR-1).
//!
//! The bench BODY is `#[cfg(feature = "testkit")]` (it needs [`seed_corpus`]); with the feature off,
//! the target compiles to an empty `main`, so `cargo test`/`cargo clippy --workspace` (which build
//! all targets WITHOUT testkit) stay green. Run the real gate via
//! `cargo bench -p unblock-storage --features testkit`; the absolute per-op ceilings are enforced
//! afterward by `cargo xtask bench-gate` reading criterion's `estimates.json` (D34 tier-ii).
//!
//! **SF-7 (async setup nesting):** the read groups seed via a sequential `Runtime::block_on` BEFORE
//! handing `&rt` to criterion's `to_async`, and the `create` group's `iter_batched` setup and routine
//! each run their own `block_on` sequentially — no `block_on` is ever nested inside another, and
//! `Handle::current()` is never reached for (both would panic).

// `criterion_group!`/`criterion_main!` generate undocumented public items.
#![allow(missing_docs)]

#[cfg(feature = "testkit")]
mod gate {
    use std::hint::black_box;
    use std::path::Path;

    use chrono::{TimeZone, Utc};
    use criterion::{BatchSize, BenchmarkId, Criterion};
    use tokio::runtime::Runtime;

    use unblock_model::{CountGroupBy, Issue, ListFilters};
    use unblock_storage::testkit::seed_corpus;
    use unblock_storage::{DEFAULT_WRITE_LOCK_TIMEOUT_MS, LibsqlStorage, Storage};

    /// Corpus sizes for the read budgets (NFR-1: list 1k/10k, ready 1k/10k).
    const READ_SIZES: &[usize] = &[1_000, 10_000];

    /// Rows seeded into the fresh DB the `create` bench inserts into. Kept small: a single insert's
    /// cost is corpus-size-independent (a B-tree insert), so a modest base keeps the per-iteration
    /// `iter_batched` setup cheap while the DB is genuinely non-empty ("seeded, not growing").
    const CREATE_SEED_BASE: usize = 100;

    /// A dedicated single-threaded tokio runtime for the sequential `block_on` seeding + the
    /// `to_async` read loops (SF-7: never nested).
    fn runtime() -> Runtime {
        Runtime::new().expect("build tokio runtime")
    }

    /// Open a fresh **file-backed** migrated libsql DB under `dir` and seed it with `n` issues.
    async fn open_seeded(dir: &Path, n: usize) -> LibsqlStorage {
        let db = dir.join("unblock.db");
        let storage = LibsqlStorage::open_local(&db, DEFAULT_WRITE_LOCK_TIMEOUT_MS)
            .await
            .expect("open_local");
        storage.migrate().await.expect("migrate");
        seed_corpus(&storage, n).await.expect("seed_corpus");
        storage
    }

    /// The single issue the `create` bench inserts — a fixed id well past `CREATE_SEED_BASE`, so it is
    /// unique in every freshly-seeded DB (each iteration gets a fresh DB, so the id never collides).
    fn insert_fixture() -> Issue {
        let epoch = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).single().unwrap();
        Issue {
            id: "ub-9999999".to_string(),
            title: "bench insert".to_string(),
            created_at: epoch,
            updated_at: epoch,
            ..Issue::default()
        }
    }

    /// `storage_create`: a single insert into a fresh, small-seeded DB (never a growing DB).
    pub fn bench_create(c: &mut Criterion) {
        let rt = runtime();
        let mut group = c.benchmark_group("storage_create");
        // File I/O per iteration is slow relative to the CPU-only policy/render benches; a small
        // sample keeps the wall-clock bounded while staying statistically honest.
        group.sample_size(20);
        group.bench_function("insert", |b| {
            b.iter_batched(
                || {
                    // SETUP (untimed): a fresh tempdir + a small-seeded file DB + the insert fixture.
                    // `block_on` here completes before the routine's `block_on` runs (SF-7: not nested).
                    let dir = tempfile::tempdir().expect("tempdir");
                    let storage = rt.block_on(open_seeded(dir.path(), CREATE_SEED_BASE));
                    (dir, storage, insert_fixture())
                },
                |(_dir, storage, issue)| {
                    // ROUTINE (timed): exactly one insert on the D31 `.write.lock` + WAL write path.
                    rt.block_on(async {
                        black_box(storage.create_issue(&issue, "bench").await.expect("insert"));
                    });
                },
                BatchSize::SmallInput,
            );
        });
        group.finish();
    }

    /// The four read groups, each seeded ONCE per size outside the timing loop and driven via
    /// `to_async` (SF-7: the seed `block_on` completes before `to_async` is handed `&rt`).
    pub fn bench_reads(c: &mut Criterion) {
        let rt = runtime();
        for &size in READ_SIZES {
            // Seed once (outside timing); keep `dir`+`storage` alive for every group at this size.
            let dir = tempfile::tempdir().expect("tempdir");
            let storage = rt.block_on(open_seeded(dir.path(), size));
            let filters = ListFilters::default();

            read_group(c, "storage_list", size, &rt, || async {
                black_box(storage.list_issues(&filters).await.expect("list"));
            });
            read_group(c, "storage_ready", size, &rt, || async {
                black_box(storage.ready_issues(&filters).await.expect("ready"));
            });
            read_group(c, "storage_count", size, &rt, || async {
                black_box(
                    storage
                        .count_issues(&filters, Some(CountGroupBy::Status))
                        .await
                        .expect("count"),
                );
            });
            read_group(c, "storage_search", size, &rt, || async {
                // Every seeded title contains "seed", so the substring scan touches the whole corpus
                // (capped at the 50-row default) — a representative worst-case search cost.
                black_box(
                    storage
                        .search_issues("seed", &filters)
                        .await
                        .expect("search"),
                );
            });

            drop(storage);
            drop(dir);
        }
    }

    /// Run one read group at one corpus size with a bounded sample (file-DB reads are slow at 10k).
    fn read_group<F, Fut>(c: &mut Criterion, name: &str, size: usize, rt: &Runtime, op: F)
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let mut group = c.benchmark_group(name);
        group.sample_size(20);
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.to_async(rt).iter(&op);
        });
        group.finish();
    }
}

#[cfg(feature = "testkit")]
criterion::criterion_group!(benches, gate::bench_create, gate::bench_reads);

#[cfg(feature = "testkit")]
criterion::criterion_main!(benches);

// Without `testkit` the bench body is absent (it needs `seed_corpus`); provide a no-op `main` so the
// target still builds under `cargo test`/`cargo clippy --workspace` (no testkit feature).
#[cfg(not(feature = "testkit"))]
fn main() {}
