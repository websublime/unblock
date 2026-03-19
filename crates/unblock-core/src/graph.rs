//! Dependency graph engine powered by petgraph.
//!
//! Provides `DependencyGraph` with operations:
//! - `build()` — construct graph from issues and blocking edges
//! - `compute_ready_set()` — find issues with no active blockers
//! - `compute_unblock_cascade()` — determine what unblocks when an issue closes
//! - `would_create_cycle()` — check before adding a dependency
//! - `detect_all_cycles()` — find all circular dependencies via Tarjan's SCC
//! - `dependency_tree()` — BFS traversal with depth limit
