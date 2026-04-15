#![allow(missing_docs)]

//! Criterion benchmarks for `GraphCache` Arc read cost vs graph build cost.
//!
//! Benchmark groups:
//! - `graph_build` — `DependencyGraph::build` at N=10, 100, 500, 1000, 2000
//! - `cache_get_ready_set` — `GraphCache::get_ready_set()` Arc clone at same N values
//! - `cache_get_graph` — `GraphCache::get_graph()` Arc clone at same N values
//! - `concurrent_readers` — 10 tokio tasks calling `get_ready_set()` concurrently

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use tokio::runtime::Runtime;

use unblock_core::cache::GraphCache;
use unblock_core::graph::DependencyGraph;
use unblock_core::types::{
    BlockingEdge, Issue, IssueState, IssueType, Priority, QualifiedId, Status,
};

/// Node counts used across all benchmark groups.
const NODE_COUNTS: &[usize] = &[10, 100, 500, 1000, 2000];

/// Node counts for the concurrent-readers group (smaller range to keep CI fast).
const CONCURRENT_NODE_COUNTS: &[usize] = &[100, 500, 1000];

/// Number of concurrent reader tasks.
const CONCURRENT_READERS: usize = 10;

/// Generate a `Vec<Issue>` of size `n` with realistic content.
///
/// Approximately 30% of issues have a blocking edge forming a sparse chain
/// (issue `i` blocked by issue `i-1`).
fn generate_issues(n: usize) -> Vec<Issue> {
    let now = Utc::now();
    (0..n)
        .map(|i| {
            let number = (i + 1) as u64;
            Issue {
                qualified_id: QualifiedId::new("bench", "repo", number),
                number,
                node_id: format!("MDExOklzc3VlTm9kZV9{number}"),
                title: format!("Benchmark issue #{number}: implement feature {i}"),
                issue_type: Some(IssueType::Task),
                status: Status::Ready,
                priority: match i % 5 {
                    0 => Priority::P0,
                    1 => Priority::P1,
                    2 => Priority::P2,
                    3 => Priority::P3,
                    _ => Priority::P4,
                },
                agent: if i % 3 == 0 {
                    Some(format!("agent-{}", i % 4))
                } else {
                    None
                },
                claimed_at: None,
                pipeline_stage: None,
                #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
                story_points: Some((i % 8 + 1) as i32),
                defer_until: None,
                labels: vec![format!("area-{}", i % 5), "benchmark".to_owned()],
                milestone: if i % 4 == 0 {
                    Some(format!("Sprint {}", i / 10 + 1))
                } else {
                    None
                },
                assignees: vec![],
                state: IssueState::Open,
                body: Some(format!(
                    "## Description\n\nBenchmark issue body for issue {number}.\n\n\
                     ## Acceptance Criteria\n\n- [ ] Criterion {i} passes"
                )),
                created_at: now,
                updated_at: now,
                url: format!("https://github.com/test/repo/issues/{number}"),
                comments: vec![],
                blocked_by: vec![],
                blocking: vec![],
                parent: None,
                sub_issues: vec![],
            }
        })
        .collect()
}

/// Generate sparse blocking edges: ~30% of issues blocked by the previous issue.
fn generate_edges(n: usize) -> Vec<BlockingEdge> {
    (1..n)
        .filter(|i| i % 3 == 0)
        .map(|i| BlockingEdge {
            source: QualifiedId::new("bench", "repo", (i + 1) as u64),
            target: QualifiedId::new("bench", "repo", i as u64),
        })
        .collect()
}

/// Pre-build a populated `GraphCache` for a given issue count.
fn build_populated_cache(rt: &Runtime, n: usize) -> Arc<GraphCache> {
    let issues = generate_issues(n);
    let edges = generate_edges(n);
    let graph = DependencyGraph::build(&issues, &edges);
    let ready_set = graph.compute_ready_set(&issues);
    let cache = Arc::new(GraphCache::new(Duration::from_secs(300)));
    rt.block_on(cache.update(issues, ready_set, graph));
    cache
}

// ── Benchmark: DependencyGraph::build ────────────────────────────────

fn bench_graph_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("graph_build");
    for &n in NODE_COUNTS {
        let issues = generate_issues(n);
        let edges = generate_edges(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| DependencyGraph::build(&issues, &edges));
        });
    }
    group.finish();
}

// ── Benchmark: cache get_ready_set (Arc clone) ───────────────────────

fn bench_cache_get_ready_set(c: &mut Criterion) {
    let rt = Runtime::new().expect("failed to create tokio runtime");
    let mut group = c.benchmark_group("cache_get_ready_set");
    for &n in NODE_COUNTS {
        let cache = build_populated_cache(&rt, n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| cache.get_ready_set());
        });
    }
    group.finish();
}

// ── Benchmark: cache get_graph (Arc clone) ───────────────────────────

fn bench_cache_get_graph(c: &mut Criterion) {
    let rt = Runtime::new().expect("failed to create tokio runtime");
    let mut group = c.benchmark_group("cache_get_graph");
    for &n in NODE_COUNTS {
        let cache = build_populated_cache(&rt, n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| cache.get_graph());
        });
    }
    group.finish();
}

// ── Benchmark: concurrent readers ────────────────────────────────────

fn bench_concurrent_readers(c: &mut Criterion) {
    let rt = Runtime::new().expect("failed to create tokio runtime");
    let mut group = c.benchmark_group("concurrent_readers");
    for &n in CONCURRENT_NODE_COUNTS {
        let cache = build_populated_cache(&rt, n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| async {
                let mut handles = Vec::with_capacity(CONCURRENT_READERS);
                for _ in 0..CONCURRENT_READERS {
                    let c = Arc::clone(&cache);
                    handles.push(tokio::spawn(async move { c.get_ready_set().await }));
                }
                for handle in handles {
                    handle.await.expect("benchmark task panicked");
                }
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_graph_build,
    bench_cache_get_ready_set,
    bench_cache_get_graph,
    bench_concurrent_readers,
);
criterion_main!(benches);
