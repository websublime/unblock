//! Dependency graph engine powered by petgraph.
//!
//! Provides `DependencyGraph` with operations:
//! - `build()` — construct graph from issues and blocking edges
//! - `compute_ready_set()` — find issues with no active blockers
//! - `compute_unblock_cascade()` — determine what unblocks when an issue closes
//! - `would_create_cycle()` — check before adding a dependency
//! - `detect_all_cycles()` — find all circular dependencies via Tarjan's SCC
//! - `dependency_tree()` — BFS traversal with depth limit

use std::collections::{HashMap, HashSet, VecDeque};

use petgraph::Direction;
use petgraph::algo::{has_path_connecting, tarjan_scc};
use petgraph::graph::{DiGraph, NodeIndex};

use crate::types::{BlockingEdge, Issue, IssueState, IssueSummary, Status, TraversalDirection};

/// The dependency graph for a single repository.
///
/// Nodes are issue numbers, edges are blocking relationships.
/// Edge direction: `blocked_issue -> blocking_issue` — a directed edge from
/// node A to node B means "A is blocked by B".
///
/// Built via [`DependencyGraph::build()`] from a slice of issues and blocking edges.
/// The graph stores issue state and status snapshots taken at build time, enabling
/// purely computational queries without network access.
#[derive(Debug, Clone)]
pub struct DependencyGraph {
    /// The underlying directed graph. Node weights are issue numbers, edge
    /// weights are unit (no metadata on edges).
    graph: DiGraph<u64, ()>,
    /// Maps issue number to its petgraph `NodeIndex` for O(1) lookups.
    node_map: HashMap<u64, NodeIndex>,
    /// Snapshot of each issue's workflow status at build time.
    issue_status: HashMap<u64, Status>,
    /// Snapshot of each issue's GitHub state (Open/Closed) at build time.
    issue_state: HashMap<u64, IssueState>,
}

impl DependencyGraph {
    /// Build a dependency graph from issues and blocking edges.
    ///
    /// Creates a node for each issue and adds directed edges per the blocking
    /// relationships. An edge from `source` to `target` means `source` is
    /// blocked by `target`.
    ///
    /// If an edge references an issue number not present in the `issues` slice,
    /// a warning is logged and the edge is skipped (no panic).
    ///
    /// # Examples
    ///
    /// ```
    /// use unblock_core::types::{Issue, BlockingEdge, IssueState, Status, Priority, ReadyState};
    /// use unblock_core::graph::DependencyGraph;
    /// use chrono::Utc;
    ///
    /// let issues = vec![
    ///     Issue {
    ///         number: 1, node_id: String::new(), title: "A".into(),
    ///         issue_type: None, status: Status::Open, priority: Priority::P2,
    ///         agent: None, claimed_at: None, ready_state: ReadyState::Ready,
    ///         story_points: None, defer_until: None, labels: vec![],
    ///         milestone: None, assignees: vec![], state: IssueState::Open,
    ///         body: None, created_at: Utc::now(), updated_at: Utc::now(),
    ///         url: String::new(),
    ///     },
    /// ];
    /// let edges: Vec<BlockingEdge> = vec![];
    /// let graph = DependencyGraph::build(&issues, &edges);
    /// ```
    #[must_use]
    pub fn build(issues: &[Issue], edges: &[BlockingEdge]) -> Self {
        let mut graph = DiGraph::<u64, ()>::new();
        let mut node_map = HashMap::with_capacity(issues.len());
        let mut issue_status = HashMap::with_capacity(issues.len());
        let mut issue_state = HashMap::with_capacity(issues.len());

        // Create a node per issue.
        for issue in issues {
            let idx = graph.add_node(issue.number);
            node_map.insert(issue.number, idx);
            issue_status.insert(issue.number, issue.status);
            issue_state.insert(issue.number, issue.state);
        }

        // Add directed edges: source -> target means source is blocked by target.
        for edge in edges {
            let source_idx = node_map.get(&edge.source);
            let target_idx = node_map.get(&edge.target);

            match (source_idx, target_idx) {
                (Some(&src), Some(&tgt)) => {
                    graph.add_edge(src, tgt, ());
                }
                _ => {
                    tracing::warn!(
                        source = edge.source,
                        target = edge.target,
                        "Skipping edge: one or both issue numbers not found in issues slice"
                    );
                }
            }
        }

        Self {
            graph,
            node_map,
            issue_status,
            issue_state,
        }
    }

    /// Compute the set of issues that are ready to work on.
    ///
    /// An issue is considered ready if:
    /// 1. Its GitHub state is [`IssueState::Open`]
    /// 2. It has no outgoing edges to issues that are still [`IssueState::Open`]
    ///    (i.e., all of its blockers are closed)
    ///
    /// **Note:** `defer_until` filtering is intentionally not applied here.
    /// Per ARCH section 6.2, defer-until is a post-filter at the MCP tool layer, not
    /// in the graph engine. The graph engine remains date-free.
    ///
    /// **Contract:** The `issues` slice should match the issues used to build the
    /// graph. The blocker evaluation uses the graph's internal state snapshot (built
    /// at construction time), while open-issue filtering uses the passed-in slice.
    /// Passing a different set of issues than what was used in `build()` may produce
    /// inconsistent results.
    ///
    /// Results are sorted by priority ascending (P0 first), then by `created_at`
    /// ascending (oldest first) as a tiebreaker.
    // TODO(unblock-45a.4): ARCH §6.2 specifies Status == Open filter here (excluding
    // InProgress, Blocked, Deferred, Closed). Currently only IssueState::Open is
    // checked. The ready tool layer partially handles this (excludes InProgress),
    // but consider adding Status::Open filtering in the graph engine per ARCH spec.
    #[must_use]
    pub fn compute_ready_set(&self, issues: &[Issue]) -> Vec<IssueSummary> {
        let mut ready: Vec<IssueSummary> = Vec::new();

        for issue in issues {
            // Only consider open issues.
            if issue.state != IssueState::Open {
                continue;
            }

            // Check if this issue has any open blockers.
            // Outgoing edges point to blockers.
            let is_blocked = if let Some(&node_idx) = self.node_map.get(&issue.number) {
                self.graph
                    .neighbors_directed(node_idx, Direction::Outgoing)
                    .any(|neighbor_idx| {
                        let neighbor_number = self.graph[neighbor_idx];
                        self.issue_state
                            .get(&neighbor_number)
                            .is_some_and(|state| *state == IssueState::Open)
                    })
            } else {
                // Issue not in graph — treat as unblocked (no edges).
                tracing::debug!(
                    issue = issue.number,
                    "Issue not found in graph node_map, treating as unblocked"
                );
                false
            };

            if !is_blocked {
                ready.push(IssueSummary {
                    number: issue.number,
                    title: issue.title.clone(),
                    issue_type: issue.issue_type,
                    status: issue.status,
                    priority: issue.priority,
                    agent: issue.agent.clone(),
                    milestone: issue.milestone.clone(),
                    story_points: issue.story_points,
                    labels: issue.labels.clone(),
                    created_at: issue.created_at,
                    url: issue.url.clone(),
                });
            }
        }

        // Sort by priority ASC (P0 first), then by created_at ASC (oldest first).
        ready.sort_by(|a, b| {
            a.priority
                .as_sort_key()
                .cmp(&b.priority.as_sort_key())
                .then_with(|| a.created_at.cmp(&b.created_at))
        });

        ready
    }

    /// Compute which issues become fully unblocked when `closed_number` closes.
    ///
    /// Finds all issues that list `closed_number` as a blocker, then checks
    /// whether each one's **remaining** blockers are all closed. An issue is
    /// returned only if every blocker is either `closed_number` itself or
    /// already [`IssueState::Closed`] in the graph's state snapshot.
    ///
    /// This method is purely computational — it does not mutate the graph,
    /// update issue state, or perform any I/O. It is called by the MCP
    /// `close` tool to determine which downstream issues need field updates
    /// (e.g., `Status → Ready`, cascade comment).
    ///
    /// If `closed_number` is not present in the graph, an empty `Vec` is
    /// returned without panicking.
    ///
    /// # Note on `_all_issues`
    ///
    /// The `_all_issues` parameter is intentionally unused in this initial
    /// implementation. It is part of the public signature because future
    /// enhancements (e.g., ancestry filtering, subgraph scoping, enriching
    /// cascade results with full [`Issue`] metadata) will require access to
    /// the complete issue list beyond what the graph topology alone provides.
    /// Including it now avoids a breaking API change later.
    #[must_use]
    pub fn compute_unblock_cascade(&self, closed_number: u64, _all_issues: &[Issue]) -> Vec<u64> {
        // Look up the node for the issue being closed.
        let Some(&closed_node) = self.node_map.get(&closed_number) else {
            return Vec::new();
        };

        // Find issues that are blocked BY closed_number.
        // Edge direction: source -> target means "source is blocked by target".
        // So nodes with an edge TO closed_number are its Incoming neighbors.
        let dependents = self
            .graph
            .neighbors_directed(closed_node, Direction::Incoming);

        let mut unblocked = Vec::new();

        for dependent_idx in dependents {
            let dependent_number = self.graph[dependent_idx];

            // Check ALL blockers of this dependent (its Outgoing neighbors).
            let all_blockers_resolved = self
                .graph
                .neighbors_directed(dependent_idx, Direction::Outgoing)
                .all(|blocker_idx| {
                    let blocker_number = self.graph[blocker_idx];
                    // Treat closed_number as closed even if issue_state says Open.
                    if blocker_number == closed_number {
                        return true;
                    }
                    // All other blockers must already be Closed.
                    self.issue_state
                        .get(&blocker_number)
                        .is_some_and(|state| *state == IssueState::Closed)
                });

            if all_blockers_resolved {
                unblocked.push(dependent_number);
            }
        }

        unblocked
    }

    /// Check whether adding a dependency edge from `source` to `target` would
    /// create a cycle in the graph.
    ///
    /// Returns `true` if a path already exists from `target` to `source` — adding
    /// the edge `source → target` would then close a cycle. Returns `false` when
    /// either node is unknown to the graph (the edge would reference a
    /// non-existent issue, so no cycle is possible).
    ///
    /// This is a pre-mutation check: call it **before** adding the edge. Used by
    /// the `depends` MCP tool for early rejection of circular dependencies.
    #[must_use]
    pub fn would_create_cycle(&self, source: u64, target: u64) -> bool {
        let Some(&source_idx) = self.node_map.get(&source) else {
            return false;
        };
        let Some(&target_idx) = self.node_map.get(&target) else {
            return false;
        };

        // If a path target → source already exists, adding source → target
        // would close a cycle.
        has_path_connecting(&self.graph, target_idx, source_idx, None)
    }

    /// Detect all cycles in the dependency graph.
    ///
    /// Uses Tarjan's strongly-connected-components algorithm. Returns every
    /// SCC with more than one node — each inner `Vec` contains the issue
    /// numbers that form a cycle. Single-node SCCs (the common case) are
    /// filtered out. Returns an empty `Vec` when the graph is acyclic.
    #[must_use]
    pub fn detect_all_cycles(&self) -> Vec<Vec<u64>> {
        tarjan_scc(&self.graph)
            .into_iter()
            .filter(|scc| scc.len() > 1)
            .map(|scc| scc.into_iter().map(|idx| self.graph[idx]).collect())
            .collect()
    }

    /// Walk the dependency tree from `root` via BFS, stopping at `max_depth`.
    ///
    /// - [`TraversalDirection::Upstream`] follows **outgoing** edges (blockers
    ///   of `root` — "who blocks this issue?").
    /// - [`TraversalDirection::Downstream`] follows **incoming** edges
    ///   (dependents of `root` — "what does this issue block?").
    ///
    /// Returns `(issue_number, depth)` pairs **excluding** the root itself.
    /// If `root` is not in the graph the result is empty. Nodes are visited
    /// at most once (cycle-safe).
    #[must_use]
    pub fn dependency_tree(
        &self,
        root: u64,
        direction: TraversalDirection,
        max_depth: usize,
    ) -> Vec<(u64, usize)> {
        let Some(&root_idx) = self.node_map.get(&root) else {
            return Vec::new();
        };

        let graph_direction = match direction {
            // Upstream: follow outgoing edges (source → blocker).
            TraversalDirection::Upstream => Direction::Outgoing,
            // Downstream: follow incoming edges (dependent → source).
            TraversalDirection::Downstream => Direction::Incoming,
        };

        let mut visited = HashSet::new();
        visited.insert(root_idx);

        let mut queue: VecDeque<(NodeIndex, usize)> = VecDeque::new();
        queue.push_back((root_idx, 0));

        let mut result = Vec::new();

        while let Some((node, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }

            for neighbor in self.graph.neighbors_directed(node, graph_direction) {
                if visited.insert(neighbor) {
                    let issue_number = self.graph[neighbor];
                    let next_depth = depth + 1;
                    result.push((issue_number, next_depth));
                    queue.push_back((neighbor, next_depth));
                }
            }
        }

        result
    }

    /// Returns a reference to the internal node map.
    ///
    /// Useful for downstream methods (cascade, cycle detection, tree traversal)
    /// that need to look up nodes by issue number.
    #[must_use]
    pub fn node_map(&self) -> &HashMap<u64, NodeIndex> {
        &self.node_map
    }

    /// Returns a reference to the underlying petgraph `DiGraph`.
    ///
    /// Exposed for downstream graph algorithms (Tarjan SCC, path queries, BFS).
    #[must_use]
    pub fn inner_graph(&self) -> &DiGraph<u64, ()> {
        &self.graph
    }

    /// Returns a reference to the issue state snapshot.
    ///
    /// Maps issue numbers to their [`IssueState`] at build time.
    #[must_use]
    pub fn issue_state(&self) -> &HashMap<u64, IssueState> {
        &self.issue_state
    }

    /// Returns a reference to the issue status snapshot.
    ///
    /// Maps issue numbers to their workflow [`Status`] at build time.
    #[must_use]
    pub fn issue_status(&self) -> &HashMap<u64, Status> {
        &self.issue_status
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    use crate::types::{Priority, ReadyState};

    /// Helper to create a minimal Issue for testing.
    fn make_issue(number: u64, state: IssueState, priority: Priority) -> Issue {
        Issue {
            number,
            node_id: String::new(),
            title: format!("Issue #{number}"),
            issue_type: None,
            status: Status::Open,
            priority,
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
            url: String::new(),
        }
    }

    /// Helper to create an issue with a specific `created_at` for sort testing.
    fn make_issue_at(
        number: u64,
        state: IssueState,
        priority: Priority,
        created_at: chrono::DateTime<Utc>,
    ) -> Issue {
        let mut issue = make_issue(number, state, priority);
        issue.created_at = created_at;
        issue
    }

    // ── DependencyGraph::build ────────────────────────────────────────────

    #[test]
    fn build_empty_inputs() {
        let graph = DependencyGraph::build(&[], &[]);
        assert!(graph.node_map.is_empty());
        assert_eq!(graph.graph.node_count(), 0);
        assert_eq!(graph.graph.edge_count(), 0);
    }

    #[test]
    fn build_issues_no_edges() {
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
        ];
        let graph = DependencyGraph::build(&issues, &[]);
        assert_eq!(graph.graph.node_count(), 2);
        assert_eq!(graph.graph.edge_count(), 0);
        assert!(graph.node_map.contains_key(&1));
        assert!(graph.node_map.contains_key(&2));
    }

    #[test]
    fn build_with_valid_edges() {
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
        ];
        // Issue 1 is blocked by issue 2.
        let edges = vec![BlockingEdge {
            source: 1,
            target: 2,
        }];
        let graph = DependencyGraph::build(&issues, &edges);
        assert_eq!(graph.graph.node_count(), 2);
        assert_eq!(graph.graph.edge_count(), 1);
    }

    #[test]
    fn build_missing_edge_node_skipped_no_panic() {
        let issues = vec![make_issue(1, IssueState::Open, Priority::P2)];
        // Edge references issue 99 which doesn't exist.
        let edges = vec![BlockingEdge {
            source: 1,
            target: 99,
        }];
        let graph = DependencyGraph::build(&issues, &edges);
        assert_eq!(graph.graph.node_count(), 1);
        assert_eq!(graph.graph.edge_count(), 0);
    }

    #[test]
    fn build_both_edge_nodes_missing() {
        let issues = vec![make_issue(1, IssueState::Open, Priority::P2)];
        let edges = vec![BlockingEdge {
            source: 88,
            target: 99,
        }];
        let graph = DependencyGraph::build(&issues, &edges);
        assert_eq!(graph.graph.edge_count(), 0);
    }

    // ── compute_ready_set ─────────────────────────────────────────────────

    #[test]
    fn ready_set_no_edges_all_open_issues_ready() {
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
        ];
        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues);
        assert_eq!(ready.len(), 2);
        // P1 (issue 2) should come first due to priority sorting.
        assert_eq!(ready[0].number, 2);
        assert_eq!(ready[1].number, 1);
    }

    #[test]
    fn ready_set_closed_issues_excluded() {
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Closed, Priority::P1),
        ];
        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].number, 1);
    }

    #[test]
    fn ready_set_blocked_issue_excluded() {
        // A (issue 1) is blocked by B (issue 2). B is open.
        // A should NOT be in the ready set.
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
        ];
        let edges = vec![BlockingEdge {
            source: 1,
            target: 2,
        }];
        let graph = DependencyGraph::build(&issues, &edges);
        let ready = graph.compute_ready_set(&issues);

        let ready_numbers: Vec<u64> = ready.iter().map(|s| s.number).collect();
        assert!(
            !ready_numbers.contains(&1),
            "Issue 1 should be blocked by issue 2"
        );
        assert!(
            ready_numbers.contains(&2),
            "Issue 2 has no blockers, should be ready"
        );
    }

    #[test]
    fn ready_set_blocker_closed_issue_becomes_ready() {
        // A (issue 1) is blocked by B (issue 2). B is now closed.
        // A should appear in the ready set.
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Closed, Priority::P1),
        ];
        let edges = vec![BlockingEdge {
            source: 1,
            target: 2,
        }];
        let graph = DependencyGraph::build(&issues, &edges);
        let ready = graph.compute_ready_set(&issues);

        let ready_numbers: Vec<u64> = ready.iter().map(|s| s.number).collect();
        assert!(
            ready_numbers.contains(&1),
            "Issue 1 should be ready since its blocker (issue 2) is closed"
        );
    }

    #[test]
    fn ready_set_partially_unblocked_still_blocked() {
        // Issue 1 is blocked by both issue 2 and issue 3.
        // Issue 2 is closed but issue 3 is open.
        // Issue 1 should NOT be in the ready set.
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Closed, Priority::P1),
            make_issue(3, IssueState::Open, Priority::P3),
        ];
        let edges = vec![
            BlockingEdge {
                source: 1,
                target: 2,
            },
            BlockingEdge {
                source: 1,
                target: 3,
            },
        ];
        let graph = DependencyGraph::build(&issues, &edges);
        let ready = graph.compute_ready_set(&issues);

        let ready_numbers: Vec<u64> = ready.iter().map(|s| s.number).collect();
        assert!(
            !ready_numbers.contains(&1),
            "Issue 1 still has open blocker (issue 3)"
        );
        assert!(
            ready_numbers.contains(&3),
            "Issue 3 has no blockers, should be ready"
        );
    }

    #[test]
    fn ready_set_empty_inputs() {
        let graph = DependencyGraph::build(&[], &[]);
        let ready = graph.compute_ready_set(&[]);
        assert!(ready.is_empty());
    }

    #[test]
    fn ready_set_sorted_by_priority_then_created_at() {
        let now = Utc::now();
        let earlier = now - chrono::Duration::hours(1);
        let issues = vec![
            make_issue_at(1, IssueState::Open, Priority::P2, now),
            make_issue_at(2, IssueState::Open, Priority::P2, earlier),
            make_issue_at(3, IssueState::Open, Priority::P0, now),
        ];
        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues);

        assert_eq!(ready.len(), 3);
        // P0 first.
        assert_eq!(ready[0].number, 3);
        // Then P2 sorted by created_at — earlier first.
        assert_eq!(ready[1].number, 2);
        assert_eq!(ready[2].number, 1);
    }

    #[test]
    fn ready_set_issue_not_in_graph_treated_as_unblocked() {
        // Build graph with issue 1 only, but compute ready set with issue 1 and 2.
        let issue1 = make_issue(1, IssueState::Open, Priority::P2);
        let issue2 = make_issue(2, IssueState::Open, Priority::P1);
        let graph = DependencyGraph::build(std::slice::from_ref(&issue1), &[]);
        let ready = graph.compute_ready_set(&[issue1, issue2]);

        let ready_numbers: Vec<u64> = ready.iter().map(|s| s.number).collect();
        assert!(
            ready_numbers.contains(&2),
            "Issue 2 not in graph is unblocked"
        );
    }

    #[test]
    fn ready_set_chain_only_leaf_ready() {
        // Chain: 1 -> 2 -> 3 (1 blocked by 2, 2 blocked by 3). All open.
        // Only issue 3 should be ready.
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
            make_issue(3, IssueState::Open, Priority::P0),
        ];
        let edges = vec![
            BlockingEdge {
                source: 1,
                target: 2,
            },
            BlockingEdge {
                source: 2,
                target: 3,
            },
        ];
        let graph = DependencyGraph::build(&issues, &edges);
        let ready = graph.compute_ready_set(&issues);

        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].number, 3);
    }

    // ── compute_unblock_cascade ────────────────────────────────────────────

    #[test]
    fn cascade_a_blocks_b_and_c_returns_both() {
        // A (1) blocks B (2) and C (3). Close A → both B and C are fully unblocked.
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
            make_issue(3, IssueState::Open, Priority::P0),
        ];
        // B is blocked by A, C is blocked by A.
        let edges = vec![
            BlockingEdge {
                source: 2,
                target: 1,
            },
            BlockingEdge {
                source: 3,
                target: 1,
            },
        ];
        let graph = DependencyGraph::build(&issues, &edges);
        let mut cascade = graph.compute_unblock_cascade(1, &issues);
        cascade.sort_unstable();
        assert_eq!(cascade, vec![2, 3]);
    }

    #[test]
    fn cascade_co_blockers_returns_empty_when_other_open() {
        // A (1) and D (4) both block E (5). Close A → E still has D as open blocker.
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(4, IssueState::Open, Priority::P1),
            make_issue(5, IssueState::Open, Priority::P0),
        ];
        // E is blocked by A and D.
        let edges = vec![
            BlockingEdge {
                source: 5,
                target: 1,
            },
            BlockingEdge {
                source: 5,
                target: 4,
            },
        ];
        let graph = DependencyGraph::build(&issues, &edges);
        let cascade = graph.compute_unblock_cascade(1, &issues);
        assert!(
            cascade.is_empty(),
            "E still has open blocker D, cascade should be empty but got {cascade:?}"
        );
    }

    #[test]
    fn cascade_co_blockers_returns_unblocked_when_other_closed() {
        // A (1) and D (4) both block E (5). D is already closed. Close A → E is unblocked.
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(4, IssueState::Closed, Priority::P1),
            make_issue(5, IssueState::Open, Priority::P0),
        ];
        let edges = vec![
            BlockingEdge {
                source: 5,
                target: 1,
            },
            BlockingEdge {
                source: 5,
                target: 4,
            },
        ];
        let graph = DependencyGraph::build(&issues, &edges);
        let cascade = graph.compute_unblock_cascade(1, &issues);
        assert_eq!(cascade, vec![5]);
    }

    #[test]
    fn cascade_blocks_nothing_returns_empty() {
        // A (1) blocks nothing.
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
        ];
        let graph = DependencyGraph::build(&issues, &[]);
        let cascade = graph.compute_unblock_cascade(1, &issues);
        assert!(cascade.is_empty());
    }

    #[test]
    fn cascade_closed_number_not_in_graph_returns_empty() {
        // closed_number 99 doesn't exist in the graph.
        let issues = vec![make_issue(1, IssueState::Open, Priority::P2)];
        let graph = DependencyGraph::build(&issues, &[]);
        let cascade = graph.compute_unblock_cascade(99, &issues);
        assert!(cascade.is_empty());
    }

    #[test]
    fn cascade_empty_graph_returns_empty() {
        let graph = DependencyGraph::build(&[], &[]);
        let cascade = graph.compute_unblock_cascade(1, &[]);
        assert!(cascade.is_empty());
    }

    #[test]
    fn cascade_returns_issue_numbers_not_summaries() {
        // Verify the return type is Vec<u64> (compile-time check, but let's be explicit).
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
        ];
        let edges = vec![BlockingEdge {
            source: 2,
            target: 1,
        }];
        let graph = DependencyGraph::build(&issues, &edges);
        let cascade: Vec<u64> = graph.compute_unblock_cascade(1, &issues);
        assert_eq!(cascade, vec![2]);
    }

    // ── would_create_cycle ──────────────────────────────────────────────

    #[test]
    fn would_create_cycle_true_when_reverse_path_exists() {
        // B→A path exists. Adding A→B would create a cycle.
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
        ];
        // Edge: 2 is blocked by 1 (edge 2→1).
        let edges = vec![BlockingEdge {
            source: 2,
            target: 1,
        }];
        let graph = DependencyGraph::build(&issues, &edges);
        // Adding 1→2 would create cycle: 1→2→1.
        assert!(graph.would_create_cycle(1, 2));
    }

    #[test]
    fn would_create_cycle_false_when_no_reverse_path() {
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
        ];
        let graph = DependencyGraph::build(&issues, &[]);
        assert!(!graph.would_create_cycle(1, 2));
    }

    #[test]
    fn would_create_cycle_false_when_source_unknown() {
        let issues = vec![make_issue(1, IssueState::Open, Priority::P2)];
        let graph = DependencyGraph::build(&issues, &[]);
        assert!(!graph.would_create_cycle(99, 1));
    }

    #[test]
    fn would_create_cycle_false_when_target_unknown() {
        let issues = vec![make_issue(1, IssueState::Open, Priority::P2)];
        let graph = DependencyGraph::build(&issues, &[]);
        assert!(!graph.would_create_cycle(1, 99));
    }

    #[test]
    fn would_create_cycle_transitive_path() {
        // Chain: 1→2→3. Adding 3→1 would create a cycle.
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
            make_issue(3, IssueState::Open, Priority::P0),
        ];
        let edges = vec![
            BlockingEdge {
                source: 1,
                target: 2,
            },
            BlockingEdge {
                source: 2,
                target: 3,
            },
        ];
        let graph = DependencyGraph::build(&issues, &edges);
        // Path 1→2→3 exists, so adding 3→1 closes the cycle.
        assert!(graph.would_create_cycle(3, 1));
        // But adding 1→3 (parallel edge) does not create a cycle.
        assert!(!graph.would_create_cycle(1, 3));
    }

    #[test]
    fn would_create_cycle_empty_graph() {
        let graph = DependencyGraph::build(&[], &[]);
        assert!(!graph.would_create_cycle(1, 2));
    }

    // ── detect_all_cycles ─────────────────────────────────────────────────

    #[test]
    fn detect_all_cycles_empty_when_acyclic() {
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
        ];
        let edges = vec![BlockingEdge {
            source: 1,
            target: 2,
        }];
        let graph = DependencyGraph::build(&issues, &edges);
        assert!(graph.detect_all_cycles().is_empty());
    }

    #[test]
    fn detect_all_cycles_finds_two_node_cycle() {
        // A→B and B→A form a cycle.
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
        ];
        let edges = vec![
            BlockingEdge {
                source: 1,
                target: 2,
            },
            BlockingEdge {
                source: 2,
                target: 1,
            },
        ];
        let graph = DependencyGraph::build(&issues, &edges);
        let cycles = graph.detect_all_cycles();
        assert_eq!(cycles.len(), 1);
        let mut cycle = cycles[0].clone();
        cycle.sort_unstable();
        assert_eq!(cycle, vec![1, 2]);
    }

    #[test]
    fn detect_all_cycles_finds_three_node_cycle() {
        // 1→2→3→1.
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
            make_issue(3, IssueState::Open, Priority::P0),
        ];
        let edges = vec![
            BlockingEdge {
                source: 1,
                target: 2,
            },
            BlockingEdge {
                source: 2,
                target: 3,
            },
            BlockingEdge {
                source: 3,
                target: 1,
            },
        ];
        let graph = DependencyGraph::build(&issues, &edges);
        let cycles = graph.detect_all_cycles();
        assert_eq!(cycles.len(), 1);
        let mut cycle = cycles[0].clone();
        cycle.sort_unstable();
        assert_eq!(cycle, vec![1, 2, 3]);
    }

    #[test]
    fn detect_all_cycles_empty_graph() {
        let graph = DependencyGraph::build(&[], &[]);
        assert!(graph.detect_all_cycles().is_empty());
    }

    #[test]
    fn detect_all_cycles_no_edges() {
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
        ];
        let graph = DependencyGraph::build(&issues, &[]);
        assert!(graph.detect_all_cycles().is_empty());
    }

    // ── dependency_tree ───────────────────────────────────────────────────

    #[test]
    fn dependency_tree_upstream_returns_blockers() {
        // 1 is blocked by 2, 2 is blocked by 3.
        // Upstream from 1 should return [(2, 1), (3, 2)].
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
            make_issue(3, IssueState::Open, Priority::P0),
        ];
        let edges = vec![
            BlockingEdge {
                source: 1,
                target: 2,
            },
            BlockingEdge {
                source: 2,
                target: 3,
            },
        ];
        let graph = DependencyGraph::build(&issues, &edges);
        let tree = graph.dependency_tree(1, TraversalDirection::Upstream, 10);
        assert_eq!(tree, vec![(2, 1), (3, 2)]);
    }

    #[test]
    fn dependency_tree_downstream_returns_blocked_issues() {
        // 2 is blocked by 1, 3 is blocked by 1.
        // Downstream from 1 should return [(2, 1), (3, 1)] (order may vary).
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
            make_issue(3, IssueState::Open, Priority::P0),
        ];
        let edges = vec![
            BlockingEdge {
                source: 2,
                target: 1,
            },
            BlockingEdge {
                source: 3,
                target: 1,
            },
        ];
        let graph = DependencyGraph::build(&issues, &edges);
        let mut tree = graph.dependency_tree(1, TraversalDirection::Downstream, 10);
        tree.sort_by_key(|&(num, _)| num);
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0], (2, 1));
        assert_eq!(tree[1], (3, 1));
    }

    #[test]
    fn dependency_tree_stops_at_max_depth() {
        // Chain: 1→2→3→4. With max_depth=2 from 1, should see (2,1) and (3,2) but not (4,3).
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
            make_issue(3, IssueState::Open, Priority::P0),
            make_issue(4, IssueState::Open, Priority::P0),
        ];
        let edges = vec![
            BlockingEdge {
                source: 1,
                target: 2,
            },
            BlockingEdge {
                source: 2,
                target: 3,
            },
            BlockingEdge {
                source: 3,
                target: 4,
            },
        ];
        let graph = DependencyGraph::build(&issues, &edges);
        let tree = graph.dependency_tree(1, TraversalDirection::Upstream, 2);
        assert_eq!(tree, vec![(2, 1), (3, 2)]);
        // 4 is at depth 3, beyond max_depth=2.
        assert!(!tree.iter().any(|&(num, _)| num == 4));
    }

    #[test]
    fn dependency_tree_max_depth_zero_returns_empty() {
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
        ];
        let edges = vec![BlockingEdge {
            source: 1,
            target: 2,
        }];
        let graph = DependencyGraph::build(&issues, &edges);
        let tree = graph.dependency_tree(1, TraversalDirection::Upstream, 0);
        assert!(tree.is_empty());
    }

    #[test]
    fn dependency_tree_root_not_in_graph_returns_empty() {
        let issues = vec![make_issue(1, IssueState::Open, Priority::P2)];
        let graph = DependencyGraph::build(&issues, &[]);
        let tree = graph.dependency_tree(99, TraversalDirection::Upstream, 10);
        assert!(tree.is_empty());
    }

    #[test]
    fn dependency_tree_handles_diamond() {
        // Diamond: 1→2, 1→3, 2→4, 3→4. Upstream from 1.
        // Should visit 2, 3 at depth 1, then 4 at depth 2 (once, not twice).
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
            make_issue(3, IssueState::Open, Priority::P0),
            make_issue(4, IssueState::Open, Priority::P0),
        ];
        let edges = vec![
            BlockingEdge {
                source: 1,
                target: 2,
            },
            BlockingEdge {
                source: 1,
                target: 3,
            },
            BlockingEdge {
                source: 2,
                target: 4,
            },
            BlockingEdge {
                source: 3,
                target: 4,
            },
        ];
        let graph = DependencyGraph::build(&issues, &edges);
        let tree = graph.dependency_tree(1, TraversalDirection::Upstream, 10);
        // 4 should appear exactly once at depth 2.
        let fours: Vec<_> = tree.iter().filter(|&&(num, _)| num == 4).collect();
        assert_eq!(fours.len(), 1);
        assert_eq!(fours[0].1, 2);
        assert_eq!(tree.len(), 3); // 2, 3, 4
    }

    #[test]
    fn dependency_tree_cycle_safe() {
        // 1→2→1 (cycle). Should not loop forever.
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
        ];
        let edges = vec![
            BlockingEdge {
                source: 1,
                target: 2,
            },
            BlockingEdge {
                source: 2,
                target: 1,
            },
        ];
        let graph = DependencyGraph::build(&issues, &edges);
        let tree = graph.dependency_tree(1, TraversalDirection::Upstream, 10);
        // Should visit 2 at depth 1, then stop (1 already visited).
        assert_eq!(tree, vec![(2, 1)]);
    }

    #[test]
    fn dependency_tree_excludes_root() {
        let issues = vec![make_issue(1, IssueState::Open, Priority::P2)];
        let graph = DependencyGraph::build(&issues, &[]);
        let tree = graph.dependency_tree(1, TraversalDirection::Upstream, 10);
        assert!(tree.is_empty());
    }

    // ── Proptest ──────────────────────────────────────────────────────────

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        /// Strategy to generate a random `IssueState`.
        fn arb_issue_state() -> impl Strategy<Value = IssueState> {
            prop_oneof![Just(IssueState::Open), Just(IssueState::Closed),]
        }

        /// Strategy to generate a random `Priority`.
        fn arb_priority() -> impl Strategy<Value = Priority> {
            prop_oneof![
                Just(Priority::P0),
                Just(Priority::P1),
                Just(Priority::P2),
                Just(Priority::P3),
                Just(Priority::P4),
            ]
        }

        proptest! {
            #[test]
            fn ready_set_never_contains_issue_with_open_blocker(
                num_issues in 1_u64..100,
                issue_states in proptest::collection::vec(arb_issue_state(), 1..100),
                issue_priorities in proptest::collection::vec(arb_priority(), 1..100),
                edges in proptest::collection::vec((1_u64..100, 1_u64..100), 0..200),
            ) {
                // Generate issues with random states and priorities.
                let issues: Vec<Issue> = (1..=num_issues)
                    .map(|n| {
                        let idx = usize::try_from(n - 1).expect("issue number fits in usize");
                        let state = issue_states.get(idx).copied().unwrap_or(IssueState::Open);
                        let priority = issue_priorities.get(idx).copied().unwrap_or(Priority::P2);
                        make_issue(n, state, priority)
                    })
                    .collect();

                // Filter edges to only reference existing issue numbers.
                let blocking_edges: Vec<BlockingEdge> = edges
                    .into_iter()
                    .filter(|(s, t)| *s != *t && *s <= num_issues && *t <= num_issues)
                    .map(|(s, t)| BlockingEdge { source: s, target: t })
                    .collect();

                let graph = DependencyGraph::build(&issues, &blocking_edges);
                let ready = graph.compute_ready_set(&issues);

                // Invariant 1: no issue in the ready set has an open blocker.
                for summary in &ready {
                    if let Some(&node_idx) = graph.node_map.get(&summary.number) {
                        for neighbor_idx in graph.graph.neighbors_directed(node_idx, Direction::Outgoing) {
                            let neighbor_number = graph.graph[neighbor_idx];
                            let neighbor_state = graph.issue_state.get(&neighbor_number);
                            prop_assert!(
                                neighbor_state != Some(&IssueState::Open),
                                "Ready issue {} has open blocker {}",
                                summary.number,
                                neighbor_number
                            );
                        }
                    }
                }

                // Invariant 2: every issue in the ready set must be IssueState::Open.
                for summary in &ready {
                    let original = issues.iter().find(|i| i.number == summary.number);
                    if let Some(issue) = original {
                        prop_assert_eq!(
                            issue.state,
                            IssueState::Open,
                            "Ready issue {} should be Open, was {:?}",
                            summary.number,
                            issue.state
                        );
                    }
                }

                // Invariant 3: cascade result is a subset of dependents, and every
                // returned issue has no remaining open blockers (treating closed_number
                // as closed).
                // Pick an arbitrary open issue to close for cascade testing.
                let open_issues: Vec<u64> = issues
                    .iter()
                    .filter(|i| i.state == IssueState::Open)
                    .map(|i| i.number)
                    .collect();
                if let Some(&closed_number) = open_issues.first() {
                    let cascade = graph.compute_unblock_cascade(closed_number, &issues);
                    // Soundness: every returned issue has no remaining open blockers.
                    for &unblocked_num in &cascade {
                        if let Some(&dep_node) = graph.node_map.get(&unblocked_num) {
                            for blocker_idx in graph.graph.neighbors_directed(dep_node, Direction::Outgoing) {
                                let blocker_num = graph.graph[blocker_idx];
                                if blocker_num == closed_number {
                                    continue; // treated as closed
                                }
                                let blocker_state = graph.issue_state.get(&blocker_num);
                                prop_assert!(
                                    blocker_state == Some(&IssueState::Closed),
                                    "Cascade returned {} but blocker {} is still open",
                                    unblocked_num,
                                    blocker_num
                                );
                            }
                        }
                    }

                    // Completeness: every dependent of closed_number whose blockers
                    // are all resolved MUST appear in the cascade result.
                    let mut cascade_set =
                        std::collections::HashSet::with_capacity(cascade.len());
                    cascade_set.extend(cascade.iter().copied());
                    if let Some(&closed_node) = graph.node_map.get(&closed_number) {
                        for dep_idx in graph.graph.neighbors_directed(closed_node, Direction::Incoming) {
                            let dep_number = graph.graph[dep_idx];
                            let all_resolved = graph
                                .graph
                                .neighbors_directed(dep_idx, Direction::Outgoing)
                                .all(|blocker_idx| {
                                    let blocker_num = graph.graph[blocker_idx];
                                    if blocker_num == closed_number {
                                        return true;
                                    }
                                    graph
                                        .issue_state
                                        .get(&blocker_num)
                                        .is_some_and(|s| *s == IssueState::Closed)
                                });
                            if all_resolved {
                                prop_assert!(
                                    cascade_set.contains(&dep_number),
                                    "Issue {} has all blockers resolved after closing {} but is missing from cascade",
                                    dep_number,
                                    closed_number
                                );
                            }
                        }
                    }
                }

                // Invariant 4: ready set is sorted by priority ASC, then created_at ASC.
                for window in ready.windows(2) {
                    let a_key = window[0].priority.as_sort_key();
                    let b_key = window[1].priority.as_sort_key();
                    prop_assert!(
                        (a_key, window[0].created_at) <= (b_key, window[1].created_at),
                        "Ready set not sorted: issue {} (P{}, {:?}) should come before {} (P{}, {:?})",
                        window[0].number, a_key, window[0].created_at,
                        window[1].number, b_key, window[1].created_at
                    );
                }
            }

            /// Property: would_create_cycle is consistent with detect_all_cycles.
            ///
            /// If would_create_cycle returns false for edge (A, B), then adding
            /// that edge should not place A and B in the same SCC.
            #[test]
            fn would_create_cycle_consistent_with_detect(
                num_issues in 2_u64..50,
                edges in proptest::collection::vec((1_u64..50, 1_u64..50), 0..100),
                probe_source in 1_u64..50,
                probe_target in 1_u64..50,
            ) {
                // Build all issues as Open.
                let issues: Vec<Issue> = (1..=num_issues)
                    .map(|n| make_issue(n, IssueState::Open, Priority::P2))
                    .collect();

                // Filter edges to valid, non-self-loop.
                let blocking_edges: Vec<BlockingEdge> = edges
                    .into_iter()
                    .filter(|(s, t)| *s != *t && *s <= num_issues && *t <= num_issues)
                    .map(|(s, t)| BlockingEdge { source: s, target: t })
                    .collect();

                let graph = DependencyGraph::build(&issues, &blocking_edges);

                // Only test if both probe nodes exist in the graph and are distinct.
                if probe_source != probe_target
                    && probe_source <= num_issues
                    && probe_target <= num_issues
                {
                    let would_cycle = graph.would_create_cycle(probe_source, probe_target);

                    if !would_cycle {
                        // Add the edge and rebuild to check no new cycle containing both.
                        let mut extended_edges = blocking_edges.clone();
                        extended_edges.push(BlockingEdge {
                            source: probe_source,
                            target: probe_target,
                        });
                        let extended_graph = DependencyGraph::build(&issues, &extended_edges);
                        let cycles = extended_graph.detect_all_cycles();
                        for cycle in &cycles {
                            prop_assert!(
                                !(cycle.contains(&probe_source) && cycle.contains(&probe_target)),
                                "would_create_cycle({}, {}) returned false but they are in the same SCC: {:?}",
                                probe_source,
                                probe_target,
                                cycle
                            );
                        }
                    }
                }
            }
        }
    }
}
