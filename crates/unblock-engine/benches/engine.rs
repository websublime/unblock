//! Engine-layer NFR-1 HYBRID perf gate (T3.5/D34).
//!
//! Measures the engine's overhead over raw storage across the groups locked in `unblock-engine.md`
//! §Bench: `engine_export` (10k, [`Session::export_jsonl`]), `engine_import` (10k,
//! [`Session::import_jsonl`] — closes the NFR-1 import-budget gap, DRIFT-9), and the
//! `engine_create` / `engine_ready` / `engine_list` / `engine_claim` overhead groups. The
//! `ready→claim→close` round-trip latency instrument lives in `unblock-cli` (D34/F-4 / MF-4), NOT
//! here — the engine (L5) has no `unblock-mcp` dep and cannot spawn `unblock mcp`.
//!
//! The read/export corpora are seeded **storage-direct** via
//! [`unblock_storage::testkit::seed_corpus`] (validated-but-non-minted, NOT the O(N²) engine mint —
//! D34/F-2), so a 10k export corpus is built cheaply; the `engine_create` group separately measures
//! the real mint overhead on a small session.
//!
//! The bench BODY is `#[cfg(feature = "testkit")]` (it needs `seed_corpus`); with the feature off the
//! target compiles to an empty `main`, so `cargo test`/`clippy --workspace` (no testkit) stay green.
//! Run via `cargo bench -p unblock-engine --features testkit`.
//!
//! **SF-7 (async setup nesting):** every group seeds via a sequential `Runtime::block_on` that
//! completes BEFORE `to_async`/the timed routine runs — no `block_on` nests inside another, and
//! `Handle::current()` is never reached for.

// `criterion_group!`/`criterion_main!` generate undocumented public items.
#![allow(missing_docs)]

#[cfg(feature = "testkit")]
mod gate {
    use std::hint::black_box;
    use std::path::PathBuf;
    use std::sync::Arc;

    use criterion::{BatchSize, BenchmarkId, Criterion};
    use tempfile::TempDir;
    use tokio::runtime::Runtime;

    use unblock_config::{ConfigPaths, ResolvedConfig, WorkspaceContext, WorkspaceSource};
    use unblock_engine::{ImportOptions, NewIssue, Session, SessionConfig};
    use unblock_model::ListFilters;
    use unblock_storage::testkit::seed_corpus;
    use unblock_storage::{DEFAULT_WRITE_LOCK_TIMEOUT_MS, LibsqlStorage, Storage};

    /// Corpus sizes for the engine read budgets (NFR-1: ready/list 1k + 10k).
    const READ_SIZES: &[usize] = &[1_000, 10_000];
    /// The export/import corpus size (NFR-1: export 10k / import 10k).
    const IO_SIZE: usize = 10_000;
    /// A small base for the mutation (`create`/`claim`) groups — the mint overhead is what is
    /// measured, not a growing DB.
    const MUT_SEED_BASE: usize = 100;

    fn runtime() -> Runtime {
        Runtime::new().expect("build tokio runtime")
    }

    /// A live workspace: a real on-disk `.unblock/` (so export/import path-confinement is exercised),
    /// a file-backed migrated + `n`-seeded libsql store, and the opened [`Session`].
    struct Workspace {
        session: Session,
        jsonl: PathBuf,
        _tmp: TempDir,
    }

    /// Build a seeded workspace: `.unblock/` under a fresh tempdir, a file-backed migrated store
    /// seeded with `n` issues (storage-direct — F-2), and the opened session.
    async fn build_seeded(n: usize) -> Workspace {
        let tmp = tempfile::tempdir().expect("tempdir");
        let workspace_dir = tmp.path().to_path_buf();
        let unblock_dir = workspace_dir.join(".unblock");
        std::fs::create_dir_all(&unblock_dir).expect("create .unblock");

        let config = ResolvedConfig::default();
        let db_path = unblock_dir.join(&config.db_filename);
        let jsonl = unblock_dir.join(&config.jsonl_filename);

        let storage = LibsqlStorage::open_local(&db_path, DEFAULT_WRITE_LOCK_TIMEOUT_MS)
            .await
            .expect("open_local");
        storage.migrate().await.expect("migrate");
        seed_corpus(&storage, n).await.expect("seed_corpus");
        let storage: Arc<dyn Storage> = Arc::new(storage);

        let paths = ConfigPaths {
            db_path,
            jsonl_path: jsonl.clone(),
            unblock_dir,
        };
        let ctx = WorkspaceContext {
            storage,
            workspace_dir,
            actor: "bench".to_string(),
            config,
            paths,
            source: WorkspaceSource::WalkUp,
        };
        let session = Session::open(ctx, SessionConfig::default())
            .await
            .expect("open session");
        Workspace {
            session,
            jsonl,
            _tmp: tmp,
        }
    }

    /// The `NewIssue` the mint bench creates (a minimal title-only issue → the engine mints its id).
    fn new_issue() -> NewIssue {
        NewIssue {
            title: "bench mint".to_string(),
            ..NewIssue::default()
        }
    }

    /// `engine_export`: [`Session::export_jsonl`] over a 10k-seeded workspace (read-only, no permit).
    pub fn bench_export(c: &mut Criterion) {
        let rt = runtime();
        let ws = rt.block_on(build_seeded(IO_SIZE));
        let mut group = c.benchmark_group("engine_export");
        group.sample_size(20);
        group.bench_with_input(BenchmarkId::from_parameter(IO_SIZE), &IO_SIZE, |b, _| {
            b.to_async(&rt).iter(|| async {
                black_box(ws.session.export_jsonl(&ws.jsonl).await.expect("export"));
            });
        });
        group.finish();
    }

    /// `engine_import`: [`Session::import_jsonl`] of a 10k JSONL into a FRESH empty session per
    /// iteration (`iter_batched`) — closes the NFR-1 import-budget gap (DRIFT-9).
    pub fn bench_import(c: &mut Criterion) {
        let rt = runtime();
        // Build the 10k JSONL once by exporting a seeded workspace, then reuse its bytes.
        let jsonl_bytes = {
            let ws = rt.block_on(build_seeded(IO_SIZE));
            rt.block_on(async { ws.session.export_jsonl(&ws.jsonl).await.expect("export") });
            std::fs::read(&ws.jsonl).expect("read exported jsonl")
        };

        let mut group = c.benchmark_group("engine_import");
        group.sample_size(10);
        group.bench_with_input(BenchmarkId::from_parameter(IO_SIZE), &IO_SIZE, |b, _| {
            b.iter_batched(
                || {
                    // SETUP (untimed): a fresh EMPTY session with the 10k JSONL staged at its
                    // confined jsonl path. Sequential `block_on` (not nested with the routine's).
                    let ws = rt.block_on(build_seeded(0));
                    std::fs::write(&ws.jsonl, &jsonl_bytes).expect("stage jsonl");
                    ws
                },
                |ws| {
                    rt.block_on(async {
                        black_box(
                            ws.session
                                .import_jsonl(&ws.jsonl, ImportOptions { dry_run: false })
                                .await
                                .expect("import"),
                        );
                    });
                },
                BatchSize::SmallInput,
            );
        });
        group.finish();
    }

    /// `engine_create`: a single [`Session::create_issue`] mint into a fresh, small-seeded session
    /// (the real O(N) mint overhead on a small corpus — never a growing DB).
    pub fn bench_create(c: &mut Criterion) {
        let rt = runtime();
        let mut group = c.benchmark_group("engine_create");
        group.sample_size(20);
        group.bench_function("mint", |b| {
            b.iter_batched(
                || rt.block_on(build_seeded(MUT_SEED_BASE)),
                |ws| {
                    rt.block_on(async {
                        black_box(ws.session.create_issue(new_issue()).await.expect("mint"));
                    });
                },
                BatchSize::SmallInput,
            );
        });
        group.finish();
    }

    /// `engine_claim`: a single [`Session::claim`] (write-permit acquisition + atomic claim) into a
    /// fresh session whose one open issue is minted in setup.
    pub fn bench_claim(c: &mut Criterion) {
        let rt = runtime();
        let mut group = c.benchmark_group("engine_claim");
        group.sample_size(20);
        group.bench_function("claim", |b| {
            b.iter_batched(
                || {
                    let ws = rt.block_on(build_seeded(0));
                    let id = rt
                        .block_on(async {
                            ws.session.create_issue(new_issue()).await.expect("mint")
                        })
                        .id;
                    (ws, id)
                },
                |(ws, id)| {
                    rt.block_on(async {
                        black_box(ws.session.claim(&id, "bench-agent").await.expect("claim"));
                    });
                },
                BatchSize::SmallInput,
            );
        });
        group.finish();
    }

    /// The engine read groups (`engine_ready`/`engine_list`) — seeded ONCE per size, driven via
    /// `to_async`; the engine adds a policy re-rank (ready) over the raw storage read.
    pub fn bench_reads(c: &mut Criterion) {
        let rt = runtime();
        for &size in READ_SIZES {
            let ws = rt.block_on(build_seeded(size));
            let filters = ListFilters::default();

            read_group(c, "engine_ready", size, &rt, || async {
                black_box(ws.session.ready(&filters).await.expect("ready"));
            });
            read_group(c, "engine_list", size, &rt, || async {
                black_box(ws.session.list(&filters).await.expect("list"));
            });

            drop(ws);
        }
    }

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
criterion::criterion_group!(
    benches,
    gate::bench_export,
    gate::bench_import,
    gate::bench_create,
    gate::bench_claim,
    gate::bench_reads,
);

#[cfg(feature = "testkit")]
criterion::criterion_main!(benches);

// Without `testkit` the bench body is absent (it needs `seed_corpus`); provide a no-op `main` so the
// target still builds under `cargo test`/`cargo clippy --workspace` (no testkit feature).
#[cfg(not(feature = "testkit"))]
fn main() {}
