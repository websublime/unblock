//! In-memory graph cache with TTL and invalidation.
//!
//! [`GraphCache`](crate::cache::GraphCache) holds a cached graph state behind [`tokio::sync::RwLock`] with
//! configurable TTL. Every write operation invalidates the cache, triggering a
//! rebuild on the next read.
//!
//! The cache stores both the computed ready set and the full
//! [`DependencyGraph`](crate::graph::DependencyGraph), so callers can run ad-hoc
//! graph queries (e.g., dependency tree, cycle detection) without a full rebuild.
//!
//! Cached values are wrapped in [`Arc`](std::sync::Arc) so that read accessors return
//! reference-counted handles instead of deep-cloning the entire data structure.
//! This makes `get_ready_set()` and `get_graph()` O(1) regardless of graph size.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

use crate::graph::DependencyGraph;
use crate::types::IssueSummary;

/// A snapshot of the computed graph state at a point in time.
///
/// Crate-internal only — external consumers access cached data through
/// [`GraphCache`] accessor methods (`get_ready_set`, `get_graph`).
///
/// Stored inside [`GraphCache`] and replaced atomically on each update.
/// Both `ready_set` and `graph` are wrapped in [`Arc`] so that read
/// accessors return cheap reference-counted handles (O(1) atomic
/// increment) instead of deep-cloning the entire data structure.
#[derive(Debug, Clone)]
pub(crate) struct CacheEntry {
    /// The pre-computed ready set (issues with no active blockers).
    pub(crate) ready_set: Arc<Vec<IssueSummary>>,
    /// Timestamp when this entry was computed.
    pub(crate) computed_at: Instant,
    /// The full dependency graph, enabling ad-hoc queries without a rebuild.
    pub(crate) graph: Arc<DependencyGraph>,
}

/// In-memory cache for the computed ready set and dependency graph.
///
/// Uses [`tokio::sync::RwLock`] for async-safe concurrent access. Multiple
/// readers can access the cache simultaneously; writers acquire exclusive access.
///
/// The cache is a write-through design: every write operation (issue create,
/// update, close, dependency add/remove) should call [`invalidate()`](Self::invalidate)
/// to force a rebuild on the next read.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use unblock_core::cache::GraphCache;
///
/// let cache = GraphCache::new(Duration::from_secs(30));
/// ```
#[derive(Debug)]
pub struct GraphCache {
    /// The cached entry, or `None` if the cache is empty or invalidated.
    inner: RwLock<Option<CacheEntry>>,
    /// Time-to-live for cache entries. Entries older than this are considered stale.
    ttl: Duration,
}

impl GraphCache {
    /// Create a new empty cache with the given TTL.
    ///
    /// The cache starts empty — [`get_ready_set()`](Self::get_ready_set) will
    /// return `None` until [`update()`](Self::update) is called.
    #[must_use]
    pub fn new(ttl: Duration) -> Self {
        Self {
            inner: RwLock::new(None),
            ttl,
        }
    }

    /// Return the cached ready set if fresh, or `None` if stale or empty.
    ///
    /// A cache entry is considered fresh when it exists and
    /// `computed_at + ttl > now`. The returned [`Arc`] is a cheap
    /// reference-counted handle (O(1) atomic increment) — no deep clone
    /// occurs regardless of how many issues are in the ready set.
    #[must_use]
    pub async fn get_ready_set(&self) -> Option<Arc<Vec<IssueSummary>>> {
        let guard = self.inner.read().await;
        guard
            .as_ref()
            .filter(|entry| entry.computed_at.elapsed() < self.ttl)
            .map(|entry| Arc::clone(&entry.ready_set))
    }

    /// Replace the cached entry with a new ready set and graph.
    ///
    /// The provided values are wrapped in [`Arc`] before storing, so
    /// subsequent reads return cheap reference-counted handles.
    /// Sets `computed_at` to `Instant::now()`, resetting the TTL window.
    /// This acquires an exclusive write lock.
    pub async fn update(&self, ready_set: Vec<IssueSummary>, graph: DependencyGraph) {
        let mut guard = self.inner.write().await;
        *guard = Some(CacheEntry {
            ready_set: Arc::new(ready_set),
            computed_at: Instant::now(),
            graph: Arc::new(graph),
        });
    }

    /// Invalidate the cache, forcing a rebuild on the next read.
    ///
    /// Sets the inner value to `None`. Subsequent calls to
    /// [`get_ready_set()`](Self::get_ready_set) and
    /// [`get_graph()`](Self::get_graph) will return `None` until
    /// [`update()`](Self::update) is called again.
    pub async fn invalidate(&self) {
        let mut guard = self.inner.write().await;
        *guard = None;
    }

    /// Check whether the cache contains a fresh (non-expired) entry.
    ///
    /// Returns `true` if an entry exists and `computed_at + ttl > now`.
    #[must_use]
    pub async fn is_fresh(&self) -> bool {
        let guard = self.inner.read().await;
        guard
            .as_ref()
            .is_some_and(|entry| entry.computed_at.elapsed() < self.ttl)
    }

    /// Return the cached dependency graph if fresh, or `None` if stale or empty.
    ///
    /// Enables ad-hoc graph queries (dependency tree, cycle detection) without
    /// triggering a full rebuild. The returned [`Arc`] is a cheap
    /// reference-counted handle (O(1) atomic increment) — no deep clone
    /// occurs regardless of graph size.
    #[must_use]
    pub async fn get_graph(&self) -> Option<Arc<DependencyGraph>> {
        let guard = self.inner.read().await;
        guard
            .as_ref()
            .filter(|entry| entry.computed_at.elapsed() < self.ttl)
            .map(|entry| Arc::clone(&entry.graph))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::Utc;

    use super::*;
    use crate::types::{BlockingEdge, Issue, IssueState, IssueType, Priority, ReadyState, Status};

    // ── Test helpers ───────────────────────────────────────────────────

    /// Build a minimal `Issue` for testing.
    fn test_issue(number: u64, state: IssueState) -> Issue {
        Issue {
            number,
            node_id: format!("NODE_{number}"),
            title: format!("Issue #{number}"),
            issue_type: Some(IssueType::Task),
            status: Status::Open,
            priority: Priority::P1,
            agent: None,
            claimed_at: None,
            ready_state: ReadyState::Ready,
            story_points: None,
            defer_until: None,
            labels: vec![],
            milestone: None,
            assignees: vec![],
            state,
            body: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            url: format!("https://github.com/test/repo/issues/{number}"),
            comments: vec![],
        }
    }

    /// Build a `DependencyGraph` with two issues and one blocking edge.
    fn test_graph() -> (DependencyGraph, Vec<IssueSummary>) {
        let issues = vec![
            test_issue(1, IssueState::Open),
            test_issue(2, IssueState::Open),
        ];
        let edges = vec![BlockingEdge {
            source: 1,
            target: 2,
        }];
        let graph = DependencyGraph::build(&issues, &edges);
        let ready_set = graph.compute_ready_set(&issues);
        (graph, ready_set)
    }

    // ── update → get_ready_set returns data ────────────────────────────

    #[tokio::test]
    async fn update_then_get_ready_set_returns_data() {
        let cache = GraphCache::new(Duration::from_secs(60));
        let (graph, ready_set) = test_graph();

        // Before update, cache is empty.
        assert!(cache.get_ready_set().await.is_none());

        cache.update(ready_set.clone(), graph).await;

        let cached = cache.get_ready_set().await;
        assert!(cached.is_some());
        assert_eq!(*cached.unwrap(), ready_set);
    }

    // ── TTL expiry returns None ────────────────────────────────────────

    #[tokio::test]
    async fn stale_cache_returns_none() {
        let cache = GraphCache::new(Duration::from_millis(10));
        let (graph, ready_set) = test_graph();

        cache.update(ready_set, graph).await;

        // Cache is fresh immediately after update.
        assert!(cache.is_fresh().await);
        assert!(cache.get_ready_set().await.is_some());

        // Wait for TTL to expire.
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(!cache.is_fresh().await);
        assert!(cache.get_ready_set().await.is_none());
        assert!(cache.get_graph().await.is_none());
    }

    // ── invalidate → get_ready_set returns None ────────────────────────

    #[tokio::test]
    async fn invalidate_clears_cache() {
        let cache = GraphCache::new(Duration::from_secs(60));
        let (graph, ready_set) = test_graph();

        cache.update(ready_set, graph).await;
        assert!(cache.get_ready_set().await.is_some());

        cache.invalidate().await;

        assert!(cache.get_ready_set().await.is_none());
        assert!(cache.get_graph().await.is_none());
        assert!(!cache.is_fresh().await);
    }

    // ── get_graph returns cached graph ─────────────────────────────────

    #[tokio::test]
    async fn get_graph_returns_cached_graph() {
        let cache = GraphCache::new(Duration::from_secs(60));
        let (graph, ready_set) = test_graph();

        cache.update(ready_set, graph.clone()).await;

        let cached_graph = cache.get_graph().await;
        assert!(cached_graph.is_some());

        // Verify the cached graph has the same structure by checking node_map keys.
        let cached = cached_graph.unwrap();
        assert_eq!(
            cached.node_map().keys().len(),
            graph.node_map().keys().len()
        );
        for key in graph.node_map().keys() {
            assert!(cached.node_map().contains_key(key));
        }
    }

    // ── is_fresh tracks state correctly ────────────────────────────────

    #[tokio::test]
    async fn is_fresh_empty_cache() {
        let cache = GraphCache::new(Duration::from_secs(60));
        assert!(!cache.is_fresh().await);
    }

    #[tokio::test]
    async fn is_fresh_after_update() {
        let cache = GraphCache::new(Duration::from_secs(60));
        let (graph, ready_set) = test_graph();

        cache.update(ready_set, graph).await;
        assert!(cache.is_fresh().await);
    }

    #[tokio::test]
    async fn is_fresh_after_invalidate() {
        let cache = GraphCache::new(Duration::from_secs(60));
        let (graph, ready_set) = test_graph();

        cache.update(ready_set, graph).await;
        cache.invalidate().await;
        assert!(!cache.is_fresh().await);
    }

    // ── zero-duration TTL is never fresh ────────────────────────────────

    #[tokio::test]
    async fn zero_ttl_is_never_fresh() {
        let cache = GraphCache::new(Duration::ZERO);
        let (graph, ready_set) = test_graph();

        cache.update(ready_set, graph).await;

        // With a zero TTL the strict `elapsed() < ttl` check can never
        // succeed — the cache is always stale immediately after population.
        assert!(!cache.is_fresh().await);
        assert!(cache.get_ready_set().await.is_none());
        assert!(cache.get_graph().await.is_none());
    }

    // ── concurrent readers ─────────────────────────────────────────────

    #[tokio::test]
    async fn concurrent_readers_do_not_deadlock() {
        let cache = std::sync::Arc::new(GraphCache::new(Duration::from_secs(60)));
        let (graph, ready_set) = test_graph();

        cache.update(ready_set.clone(), graph).await;

        // Spawn 10 concurrent readers via tokio::join!
        let results = tokio::join!(
            {
                let c = cache.clone();
                async move { c.get_ready_set().await }
            },
            {
                let c = cache.clone();
                async move { c.get_ready_set().await }
            },
            {
                let c = cache.clone();
                async move { c.get_ready_set().await }
            },
            {
                let c = cache.clone();
                async move { c.get_ready_set().await }
            },
            {
                let c = cache.clone();
                async move { c.get_ready_set().await }
            },
            {
                let c = cache.clone();
                async move { c.get_graph().await }
            },
            {
                let c = cache.clone();
                async move { c.is_fresh().await }
            },
            {
                let c = cache.clone();
                async move { c.get_ready_set().await }
            },
            {
                let c = cache.clone();
                async move { c.get_ready_set().await }
            },
            {
                let c = cache.clone();
                async move { c.get_ready_set().await }
            },
        );

        // All ready_set readers should return the same data.
        assert_eq!(*results.0.unwrap(), ready_set);
        assert_eq!(*results.1.unwrap(), ready_set);
        assert_eq!(*results.2.unwrap(), ready_set);
        assert_eq!(*results.3.unwrap(), ready_set);
        assert_eq!(*results.4.unwrap(), ready_set);
        // Graph reader should return Some.
        assert!(results.5.is_some());
        // is_fresh should return true.
        assert!(results.6);
        assert_eq!(*results.7.unwrap(), ready_set);
        assert_eq!(*results.8.unwrap(), ready_set);
        assert_eq!(*results.9.unwrap(), ready_set);
    }

    // ── update replaces previous entry ─────────────────────────────────

    #[tokio::test]
    async fn update_replaces_previous_entry() {
        let cache = GraphCache::new(Duration::from_secs(60));
        let (graph, ready_set) = test_graph();

        cache.update(ready_set, graph.clone()).await;

        // Update with a different ready set (empty).
        let new_ready_set: Vec<IssueSummary> = vec![];
        cache.update(new_ready_set.clone(), graph).await;

        let cached = cache.get_ready_set().await.unwrap();
        assert!(cached.is_empty());
    }
}
