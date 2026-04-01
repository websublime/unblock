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

use crate::types::{
    BlockingEdge, Issue, IssueState, IssueSummary, QualifiedId, Status, TraversalDirection,
};

/// The dependency graph for one or more repositories.
///
/// Nodes are [`QualifiedId`] values (`owner/repo#number`), edges are blocking
/// relationships. Edge direction: `blocked_issue -> blocking_issue` — a
/// directed edge from node A to node B means "A is blocked by B".
///
/// Built via [`DependencyGraph::build()`] from a slice of issues and blocking edges.
/// The graph stores issue state and status snapshots taken at build time, enabling
/// purely computational queries without network access.
#[derive(Debug, Clone)]
pub struct DependencyGraph {
    /// The underlying directed graph. Node weights are [`QualifiedId`] values,
    /// edge weights are unit (no metadata on edges).
    graph: DiGraph<QualifiedId, ()>,
    /// Maps [`QualifiedId`] to its petgraph `NodeIndex` for O(1) lookups.
    node_map: HashMap<QualifiedId, NodeIndex>,
    /// Snapshot of each issue's workflow status at build time.
    issue_status: HashMap<QualifiedId, Status>,
    /// Snapshot of each issue's GitHub state (Open/Closed) at build time.
    issue_state: HashMap<QualifiedId, IssueState>,
}

impl DependencyGraph {
    /// Build a dependency graph from issues and blocking edges.
    ///
    /// Creates a node for each issue and adds directed edges per the blocking
    /// relationships. An edge from `source` to `target` means `source` is
    /// blocked by `target`.
    ///
    /// If an edge references a [`QualifiedId`] not present in the `issues` slice,
    /// a warning is logged and the edge is skipped (no panic).
    ///
    /// # Examples
    ///
    /// ```
    /// use unblock_core::types::{Issue, BlockingEdge, IssueState, Status, Priority, ReadyState, QualifiedId};
    /// use unblock_core::graph::DependencyGraph;
    /// use chrono::Utc;
    ///
    /// let qid = QualifiedId::new("owner", "repo", 1);
    /// let issues = vec![
    ///     Issue {
    ///         qualified_id: qid.clone(),
    ///         number: 1, node_id: String::new(), title: "A".into(),
    ///         issue_type: None, status: Status::Open, priority: Priority::P2,
    ///         agent: None, claimed_at: None, ready_state: ReadyState::Ready,
    ///         story_points: None, defer_until: None, labels: vec![],
    ///         milestone: None, assignees: vec![], state: IssueState::Open,
    ///         body: None, created_at: Utc::now(), updated_at: Utc::now(),
    ///         url: String::new(), comments: vec![],
    ///         blocked_by: vec![], blocking: vec![],
    ///         parent: None, sub_issues: vec![],
    ///     },
    /// ];
    /// let edges: Vec<BlockingEdge> = vec![];
    /// let graph = DependencyGraph::build(&issues, &edges);
    /// ```
    #[must_use]
    pub fn build(issues: &[Issue], edges: &[BlockingEdge]) -> Self {
        let mut graph = DiGraph::<QualifiedId, ()>::new();
        let mut node_map = HashMap::with_capacity(issues.len());
        let mut issue_status = HashMap::with_capacity(issues.len());
        let mut issue_state = HashMap::with_capacity(issues.len());

        // Create a node per issue, keyed by QualifiedId.
        for issue in issues {
            let qid = issue.qualified_id.clone();
            let idx = graph.add_node(qid.clone());
            node_map.insert(qid.clone(), idx);
            issue_status.insert(qid.clone(), issue.status);
            issue_state.insert(qid, issue.state);
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
                        source = %edge.source,
                        target = %edge.target,
                        "Skipping edge: one or both qualified IDs not found in issues slice"
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
            let is_blocked = if let Some(&node_idx) = self.node_map.get(&issue.qualified_id) {
                self.graph
                    .neighbors_directed(node_idx, Direction::Outgoing)
                    .any(|neighbor_idx| {
                        let neighbor_qid = &self.graph[neighbor_idx];
                        self.issue_state
                            .get(neighbor_qid)
                            .is_some_and(|state| *state == IssueState::Open)
                    })
            } else {
                // Issue not in graph — treat as unblocked (no edges).
                tracing::debug!(
                    issue = %issue.qualified_id,
                    "Issue not found in graph node_map, treating as unblocked"
                );
                false
            };

            if !is_blocked {
                ready.push(IssueSummary {
                    qualified_id: issue.qualified_id.clone(),
                    number: issue.number,
                    title: issue.title.clone(),
                    issue_type: issue.issue_type,
                    status: issue.status,
                    priority: issue.priority,
                    agent: issue.agent.clone(),
                    milestone: issue.milestone.clone(),
                    story_points: issue.story_points,
                    defer_until: issue.defer_until,
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

    /// Compute which issues become fully unblocked when the issue identified
    /// by `closed_id` closes.
    ///
    /// Finds all issues that list `closed_id` as a blocker, then checks
    /// whether each one's **remaining** blockers are all closed. An issue is
    /// returned only if every blocker is either `closed_id` itself or
    /// already [`IssueState::Closed`] in the graph's state snapshot.
    ///
    /// This method is purely computational — it does not mutate the graph,
    /// update issue state, or perform any I/O. It is called by the MCP
    /// `close` tool to determine which downstream issues need field updates
    /// (e.g., `Status → Ready`, cascade comment).
    ///
    /// If `closed_id` is not present in the graph, an empty `Vec` is
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
    pub fn compute_unblock_cascade(
        &self,
        closed_id: &QualifiedId,
        _all_issues: &[Issue],
    ) -> Vec<QualifiedId> {
        // Look up the node for the issue being closed.
        let Some(&closed_node) = self.node_map.get(closed_id) else {
            return Vec::new();
        };

        // Find issues that are blocked BY closed_id.
        // Edge direction: source -> target means "source is blocked by target".
        // So nodes with an edge TO closed_id are its Incoming neighbors.
        let dependents = self
            .graph
            .neighbors_directed(closed_node, Direction::Incoming);

        let mut unblocked = Vec::new();

        for dependent_idx in dependents {
            let dependent_qid = self.graph[dependent_idx].clone();

            // Check ALL blockers of this dependent (its Outgoing neighbors).
            let all_blockers_resolved = self
                .graph
                .neighbors_directed(dependent_idx, Direction::Outgoing)
                .all(|blocker_idx| {
                    let blocker_qid = &self.graph[blocker_idx];
                    // Treat closed_id as closed even if issue_state says Open.
                    if blocker_qid == closed_id {
                        return true;
                    }
                    // All other blockers must already be Closed.
                    self.issue_state
                        .get(blocker_qid)
                        .is_some_and(|state| *state == IssueState::Closed)
                });

            if all_blockers_resolved {
                unblocked.push(dependent_qid);
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
    pub fn would_create_cycle(&self, source: &QualifiedId, target: &QualifiedId) -> bool {
        let Some(&source_idx) = self.node_map.get(source) else {
            return false;
        };
        let Some(&target_idx) = self.node_map.get(target) else {
            return false;
        };

        // If a path target → source already exists, adding source → target
        // would close a cycle.
        has_path_connecting(&self.graph, target_idx, source_idx, None)
    }

    /// Detect all cycles in the dependency graph.
    ///
    /// Uses Tarjan's strongly-connected-components algorithm. Returns every
    /// SCC with more than one node — each inner `Vec` contains the
    /// [`QualifiedId`] values that form a cycle. Single-node SCCs (the common
    /// case) are filtered out. Returns an empty `Vec` when the graph is acyclic.
    #[must_use]
    pub fn detect_all_cycles(&self) -> Vec<Vec<QualifiedId>> {
        tarjan_scc(&self.graph)
            .into_iter()
            // Single-node SCCs are isolated nodes or self-loops; only multi-node
            // SCCs represent real dependency cycles. Self-loops are rejected at
            // insertion time by `add_blocked_by`.
            .filter(|scc| scc.len() > 1)
            .map(|scc| scc.into_iter().map(|idx| self.graph[idx].clone()).collect())
            .collect()
    }

    /// Walk the dependency tree from `root` via BFS, stopping at `max_depth`.
    ///
    /// - [`TraversalDirection::Upstream`] follows **outgoing** edges (blockers
    ///   of `root` — "who blocks this issue?").
    /// - [`TraversalDirection::Downstream`] follows **incoming** edges
    ///   (dependents of `root` — "what does this issue block?").
    /// - [`TraversalDirection::Both`] follows edges in **both** directions
    ///   (upstream blockers and downstream dependents combined).
    ///
    /// Returns `(QualifiedId, depth)` pairs **excluding** the root itself.
    /// If `root` is not in the graph the result is empty. Nodes are visited
    /// at most once (cycle-safe).
    // DEVIATION(unblock-b6b.20): Returns Vec<(QualifiedId, usize)> instead of
    // DependencyTree struct per ARCH §6.4. The richer type includes Status and
    // IssueState per node with recursive TreeNode children. Upgrade when the
    // show tool (unblock-45a.8) is implemented.
    #[must_use]
    pub fn dependency_tree(
        &self,
        root: &QualifiedId,
        direction: TraversalDirection,
        max_depth: usize,
    ) -> Vec<(QualifiedId, usize)> {
        let Some(&root_idx) = self.node_map.get(root) else {
            return Vec::new();
        };

        let directions: &[Direction] = match direction {
            // Upstream: follow outgoing edges (source → blocker).
            TraversalDirection::Upstream => &[Direction::Outgoing],
            // Downstream: follow incoming edges (dependent → source).
            TraversalDirection::Downstream => &[Direction::Incoming],
            // Both: follow edges in both directions from each node.
            TraversalDirection::Both => &[Direction::Outgoing, Direction::Incoming],
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

            for &dir in directions {
                for neighbor in self.graph.neighbors_directed(node, dir) {
                    if visited.insert(neighbor) {
                        let qid = self.graph[neighbor].clone();
                        let next_depth = depth + 1;
                        result.push((qid, next_depth));
                        queue.push_back((neighbor, next_depth));
                    }
                }
            }
        }

        result
    }

    /// Returns a reference to the internal node map.
    ///
    /// Useful for downstream methods (cascade, cycle detection, tree traversal)
    /// that need to look up nodes by [`QualifiedId`].
    #[must_use]
    pub fn node_map(&self) -> &HashMap<QualifiedId, NodeIndex> {
        &self.node_map
    }

    /// Returns a reference to the underlying petgraph `DiGraph`.
    ///
    /// Exposed for downstream graph algorithms (Tarjan SCC, path queries, BFS).
    #[must_use]
    pub fn inner_graph(&self) -> &DiGraph<QualifiedId, ()> {
        &self.graph
    }

    /// Returns a reference to the issue state snapshot.
    ///
    /// Maps [`QualifiedId`] to their [`IssueState`] at build time.
    #[must_use]
    pub fn issue_state(&self) -> &HashMap<QualifiedId, IssueState> {
        &self.issue_state
    }

    /// Returns a reference to the issue status snapshot.
    ///
    /// Maps [`QualifiedId`] to their workflow [`Status`] at build time.
    #[must_use]
    pub fn issue_status(&self) -> &HashMap<QualifiedId, Status> {
        &self.issue_status
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    use crate::types::{Priority, ReadyState};

    /// Default owner/repo used by test helpers.
    const TEST_OWNER: &str = "test";
    const TEST_REPO: &str = "repo";

    /// Helper to create a `QualifiedId` for tests using the default test owner/repo.
    fn qid(number: u64) -> QualifiedId {
        QualifiedId::new(TEST_OWNER, TEST_REPO, number)
    }

    /// Helper to create a `QualifiedId` for a specific owner/repo (cross-repo tests).
    fn qid_repo(owner: &str, repo: &str, number: u64) -> QualifiedId {
        QualifiedId::new(owner, repo, number)
    }

    /// Helper to create a minimal Issue for testing.
    fn make_issue(number: u64, state: IssueState, priority: Priority) -> Issue {
        Issue {
            qualified_id: qid(number),
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
            comments: vec![],
            blocked_by: vec![],
            blocking: vec![],
            parent: None,
            sub_issues: vec![],
        }
    }

    /// Helper to create an issue for a specific owner/repo (cross-repo tests).
    fn make_issue_repo(
        owner: &str,
        repo: &str,
        number: u64,
        state: IssueState,
        priority: Priority,
    ) -> Issue {
        let mut issue = make_issue(number, state, priority);
        issue.qualified_id = qid_repo(owner, repo, number);
        issue
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

    /// Helper to create a `BlockingEdge` from test issue numbers (same repo).
    fn edge(source: u64, target: u64) -> BlockingEdge {
        BlockingEdge {
            source: qid(source),
            target: qid(target),
        }
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
        assert!(graph.node_map.contains_key(&qid(1)));
        assert!(graph.node_map.contains_key(&qid(2)));
    }

    #[test]
    fn build_with_valid_edges() {
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
        ];
        // Issue 1 is blocked by issue 2.
        let edges = vec![edge(1, 2)];
        let graph = DependencyGraph::build(&issues, &edges);
        assert_eq!(graph.graph.node_count(), 2);
        assert_eq!(graph.graph.edge_count(), 1);
    }

    #[test]
    fn build_missing_edge_node_skipped_no_panic() {
        let issues = vec![make_issue(1, IssueState::Open, Priority::P2)];
        // Edge references issue 99 which doesn't exist.
        let edges = vec![edge(1, 99)];
        let graph = DependencyGraph::build(&issues, &edges);
        assert_eq!(graph.graph.node_count(), 1);
        assert_eq!(graph.graph.edge_count(), 0);
    }

    #[test]
    fn build_both_edge_nodes_missing() {
        let issues = vec![make_issue(1, IssueState::Open, Priority::P2)];
        let edges = vec![edge(88, 99)];
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
        let edges = vec![edge(1, 2)];
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
        let edges = vec![edge(1, 2)];
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
        let edges = vec![edge(1, 2), edge(1, 3)];
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
        let edges = vec![edge(1, 2), edge(2, 3)];
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
        let edges = vec![edge(2, 1), edge(3, 1)];
        let graph = DependencyGraph::build(&issues, &edges);
        let mut cascade = graph.compute_unblock_cascade(&qid(1), &issues);
        cascade.sort_by_key(|q| q.number);
        assert_eq!(cascade, vec![qid(2), qid(3)]);
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
        let edges = vec![edge(5, 1), edge(5, 4)];
        let graph = DependencyGraph::build(&issues, &edges);
        let cascade = graph.compute_unblock_cascade(&qid(1), &issues);
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
        let edges = vec![edge(5, 1), edge(5, 4)];
        let graph = DependencyGraph::build(&issues, &edges);
        let cascade = graph.compute_unblock_cascade(&qid(1), &issues);
        assert_eq!(cascade, vec![qid(5)]);
    }

    #[test]
    fn cascade_blocks_nothing_returns_empty() {
        // A (1) blocks nothing.
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
        ];
        let graph = DependencyGraph::build(&issues, &[]);
        let cascade = graph.compute_unblock_cascade(&qid(1), &issues);
        assert!(cascade.is_empty());
    }

    #[test]
    fn cascade_closed_number_not_in_graph_returns_empty() {
        // closed_number 99 doesn't exist in the graph.
        let issues = vec![make_issue(1, IssueState::Open, Priority::P2)];
        let graph = DependencyGraph::build(&issues, &[]);
        let cascade = graph.compute_unblock_cascade(&qid(99), &issues);
        assert!(cascade.is_empty());
    }

    #[test]
    fn cascade_empty_graph_returns_empty() {
        let graph = DependencyGraph::build(&[], &[]);
        let cascade = graph.compute_unblock_cascade(&qid(1), &[]);
        assert!(cascade.is_empty());
    }

    #[test]
    fn cascade_returns_qualified_ids() {
        // Verify the return type is Vec<QualifiedId>.
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
        ];
        let edges = vec![edge(2, 1)];
        let graph = DependencyGraph::build(&issues, &edges);
        let cascade: Vec<QualifiedId> = graph.compute_unblock_cascade(&qid(1), &issues);
        assert_eq!(cascade, vec![qid(2)]);
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
        let edges = vec![edge(2, 1)];
        let graph = DependencyGraph::build(&issues, &edges);
        // Adding 1→2 would create cycle: 1→2→1.
        assert!(graph.would_create_cycle(&qid(1), &qid(2)));
    }

    #[test]
    fn would_create_cycle_false_when_no_reverse_path() {
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
        ];
        let graph = DependencyGraph::build(&issues, &[]);
        assert!(!graph.would_create_cycle(&qid(1), &qid(2)));
    }

    #[test]
    fn would_create_cycle_false_when_source_unknown() {
        let issues = vec![make_issue(1, IssueState::Open, Priority::P2)];
        let graph = DependencyGraph::build(&issues, &[]);
        assert!(!graph.would_create_cycle(&qid(99), &qid(1)));
    }

    #[test]
    fn would_create_cycle_false_when_target_unknown() {
        let issues = vec![make_issue(1, IssueState::Open, Priority::P2)];
        let graph = DependencyGraph::build(&issues, &[]);
        assert!(!graph.would_create_cycle(&qid(1), &qid(99)));
    }

    #[test]
    fn would_create_cycle_transitive_path() {
        // Chain: 1→2→3. Adding 3→1 would create a cycle.
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
            make_issue(3, IssueState::Open, Priority::P0),
        ];
        let edges = vec![edge(1, 2), edge(2, 3)];
        let graph = DependencyGraph::build(&issues, &edges);
        // Path 1→2→3 exists, so adding 3→1 closes the cycle.
        assert!(graph.would_create_cycle(&qid(3), &qid(1)));
        // But adding 1→3 (parallel edge) does not create a cycle.
        assert!(!graph.would_create_cycle(&qid(1), &qid(3)));
    }

    #[test]
    fn would_create_cycle_empty_graph() {
        let graph = DependencyGraph::build(&[], &[]);
        assert!(!graph.would_create_cycle(&qid(1), &qid(2)));
    }

    #[test]
    fn would_create_cycle_self_loop() {
        // A node blocking itself is always a cycle, even with no existing edges.
        let issues = vec![make_issue(1, IssueState::Open, Priority::P2)];
        let graph = DependencyGraph::build(&issues, &[]);
        assert!(graph.would_create_cycle(&qid(1), &qid(1)));
    }

    // ── detect_all_cycles ─────────────────────────────────────────────────

    #[test]
    fn detect_all_cycles_empty_when_acyclic() {
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
        ];
        let edges = vec![edge(1, 2)];
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
        let edges = vec![edge(1, 2), edge(2, 1)];
        let graph = DependencyGraph::build(&issues, &edges);
        let cycles = graph.detect_all_cycles();
        assert_eq!(cycles.len(), 1);
        let mut cycle = cycles[0].clone();
        cycle.sort_by_key(|q| q.number);
        assert_eq!(cycle, vec![qid(1), qid(2)]);
    }

    #[test]
    fn detect_all_cycles_finds_three_node_cycle() {
        // 1→2→3→1.
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
            make_issue(3, IssueState::Open, Priority::P0),
        ];
        let edges = vec![edge(1, 2), edge(2, 3), edge(3, 1)];
        let graph = DependencyGraph::build(&issues, &edges);
        let cycles = graph.detect_all_cycles();
        assert_eq!(cycles.len(), 1);
        let mut cycle = cycles[0].clone();
        cycle.sort_by_key(|q| q.number);
        assert_eq!(cycle, vec![qid(1), qid(2), qid(3)]);
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
        // Upstream from 1 should return [(qid(2), 1), (qid(3), 2)].
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
            make_issue(3, IssueState::Open, Priority::P0),
        ];
        let edges = vec![edge(1, 2), edge(2, 3)];
        let graph = DependencyGraph::build(&issues, &edges);
        let tree = graph.dependency_tree(&qid(1), TraversalDirection::Upstream, 10);
        assert_eq!(tree, vec![(qid(2), 1), (qid(3), 2)]);
    }

    #[test]
    fn dependency_tree_downstream_returns_blocked_issues() {
        // 2 is blocked by 1, 3 is blocked by 1.
        // Downstream from 1 should return [(qid(2), 1), (qid(3), 1)] (order may vary).
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
            make_issue(3, IssueState::Open, Priority::P0),
        ];
        let edges = vec![edge(2, 1), edge(3, 1)];
        let graph = DependencyGraph::build(&issues, &edges);
        let mut tree = graph.dependency_tree(&qid(1), TraversalDirection::Downstream, 10);
        tree.sort_by_key(|(q, _)| q.number);
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0], (qid(2), 1));
        assert_eq!(tree[1], (qid(3), 1));
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
        let edges = vec![edge(1, 2), edge(2, 3), edge(3, 4)];
        let graph = DependencyGraph::build(&issues, &edges);
        let tree = graph.dependency_tree(&qid(1), TraversalDirection::Upstream, 2);
        assert_eq!(tree, vec![(qid(2), 1), (qid(3), 2)]);
        // 4 is at depth 3, beyond max_depth=2.
        assert!(!tree.iter().any(|(q, _)| q.number == 4));
    }

    #[test]
    fn dependency_tree_max_depth_zero_returns_empty() {
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
        ];
        let edges = vec![edge(1, 2)];
        let graph = DependencyGraph::build(&issues, &edges);
        let tree = graph.dependency_tree(&qid(1), TraversalDirection::Upstream, 0);
        assert!(tree.is_empty());
    }

    #[test]
    fn dependency_tree_root_not_in_graph_returns_empty() {
        let issues = vec![make_issue(1, IssueState::Open, Priority::P2)];
        let graph = DependencyGraph::build(&issues, &[]);
        let tree = graph.dependency_tree(&qid(99), TraversalDirection::Upstream, 10);
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
        let edges = vec![edge(1, 2), edge(1, 3), edge(2, 4), edge(3, 4)];
        let graph = DependencyGraph::build(&issues, &edges);
        let tree = graph.dependency_tree(&qid(1), TraversalDirection::Upstream, 10);
        // 4 should appear exactly once at depth 2.
        let fours: Vec<_> = tree.iter().filter(|(q, _)| q.number == 4).collect();
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
        let edges = vec![edge(1, 2), edge(2, 1)];
        let graph = DependencyGraph::build(&issues, &edges);
        let tree = graph.dependency_tree(&qid(1), TraversalDirection::Upstream, 10);
        // Should visit 2 at depth 1, then stop (1 already visited).
        assert_eq!(tree, vec![(qid(2), 1)]);
    }

    #[test]
    fn dependency_tree_excludes_root() {
        let issues = vec![make_issue(1, IssueState::Open, Priority::P2)];
        let graph = DependencyGraph::build(&issues, &[]);
        let tree = graph.dependency_tree(&qid(1), TraversalDirection::Upstream, 10);
        assert!(tree.is_empty());
    }

    #[test]
    fn dependency_tree_both_returns_upstream_and_downstream() {
        // Graph: 2→1→3 (1 is blocked by 3, 2 is blocked by 1).
        // Both from 1 should return upstream blocker (3) and downstream dependent (2).
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
            make_issue(3, IssueState::Open, Priority::P0),
        ];
        let edges = vec![edge(1, 3), edge(2, 1)];
        let graph = DependencyGraph::build(&issues, &edges);
        let mut tree = graph.dependency_tree(&qid(1), TraversalDirection::Both, 10);
        tree.sort_by_key(|(q, _)| q.number);
        assert_eq!(tree.len(), 2);
        // Both neighbors at depth 1.
        assert_eq!(tree[0], (qid(2), 1));
        assert_eq!(tree[1], (qid(3), 1));
    }

    #[test]
    fn dependency_tree_both_deduplicates_shared_nodes() {
        // Diamond: 1→2, 3→1, 2→4, 3→4.
        // Both from 1: upstream gives 2→4, downstream gives 3→4.
        // Node 4 is reachable from both directions but should appear only once.
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
            make_issue(3, IssueState::Open, Priority::P0),
            make_issue(4, IssueState::Open, Priority::P0),
        ];
        let edges = vec![edge(1, 2), edge(3, 1), edge(2, 4), edge(3, 4)];
        let graph = DependencyGraph::build(&issues, &edges);
        let tree = graph.dependency_tree(&qid(1), TraversalDirection::Both, 10);
        // All three nodes (2, 3, 4) reachable, 4 only once.
        let fours: Vec<_> = tree.iter().filter(|(q, _)| q.number == 4).collect();
        assert_eq!(fours.len(), 1);
        assert_eq!(tree.len(), 3);
    }

    #[test]
    fn dependency_tree_both_respects_max_depth() {
        // Chain: 2→1→3→4. Both from 1 with max_depth=1.
        // Should see 2 (downstream, depth 1) and 3 (upstream, depth 1), but not 4.
        let issues = vec![
            make_issue(1, IssueState::Open, Priority::P2),
            make_issue(2, IssueState::Open, Priority::P1),
            make_issue(3, IssueState::Open, Priority::P0),
            make_issue(4, IssueState::Open, Priority::P0),
        ];
        let edges = vec![edge(1, 3), edge(3, 4), edge(2, 1)];
        let graph = DependencyGraph::build(&issues, &edges);
        let mut tree = graph.dependency_tree(&qid(1), TraversalDirection::Both, 1);
        tree.sort_by_key(|(q, _)| q.number);
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0], (qid(2), 1));
        assert_eq!(tree[1], (qid(3), 1));
        // 4 is at depth 2, beyond max_depth=1.
        assert!(!tree.iter().any(|(q, _)| q.number == 4));
    }

    #[test]
    fn traversal_direction_serde_roundtrip_both() {
        let direction = TraversalDirection::Both;
        let json = serde_json::to_string(&direction).expect("serialize");
        assert_eq!(json, "\"Both\"");
        let deserialized: TraversalDirection = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized, TraversalDirection::Both);
    }

    // ── Cross-repo tests ────────────────────────────────────────────────

    #[test]
    fn cross_repo_same_number_different_repos_are_distinct_nodes() {
        // Two issues with the same number (42) from different repos must be distinct.
        let issue_a = make_issue_repo("acme", "widgets", 42, IssueState::Open, Priority::P1);
        let issue_b = make_issue_repo("acme", "gadgets", 42, IssueState::Open, Priority::P2);
        let issues = vec![issue_a, issue_b];
        let graph = DependencyGraph::build(&issues, &[]);
        assert_eq!(graph.graph.node_count(), 2);
        assert!(
            graph
                .node_map
                .contains_key(&qid_repo("acme", "widgets", 42))
        );
        assert!(
            graph
                .node_map
                .contains_key(&qid_repo("acme", "gadgets", 42))
        );
    }

    #[test]
    fn cross_repo_ready_set_with_mixed_repos() {
        // Issue acme/widgets#1 is blocked by acme/gadgets#1.
        // acme/gadgets#1 is open, so acme/widgets#1 should NOT be ready.
        let issue_a = make_issue_repo("acme", "widgets", 1, IssueState::Open, Priority::P1);
        let issue_b = make_issue_repo("acme", "gadgets", 1, IssueState::Open, Priority::P2);
        let issues = vec![issue_a, issue_b];
        let edges = vec![BlockingEdge {
            source: qid_repo("acme", "widgets", 1),
            target: qid_repo("acme", "gadgets", 1),
        }];
        let graph = DependencyGraph::build(&issues, &edges);
        let ready = graph.compute_ready_set(&issues);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].qualified_id, qid_repo("acme", "gadgets", 1));
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
                    .map(|(s, t)| edge(s, t))
                    .collect();

                let graph = DependencyGraph::build(&issues, &blocking_edges);
                let ready = graph.compute_ready_set(&issues);

                // Invariant 1: no issue in the ready set has an open blocker.
                for summary in &ready {
                    if let Some(&node_idx) = graph.node_map.get(&summary.qualified_id) {
                        for neighbor_idx in graph.graph.neighbors_directed(node_idx, Direction::Outgoing) {
                            let neighbor_qid = &graph.graph[neighbor_idx];
                            let neighbor_state = graph.issue_state.get(neighbor_qid);
                            prop_assert!(
                                neighbor_state != Some(&IssueState::Open),
                                "Ready issue {} has open blocker {}",
                                summary.qualified_id,
                                neighbor_qid
                            );
                        }
                    }
                }

                // Invariant 2: every issue in the ready set must be IssueState::Open.
                for summary in &ready {
                    let original = issues.iter().find(|i| i.qualified_id == summary.qualified_id);
                    if let Some(issue) = original {
                        prop_assert_eq!(
                            issue.state,
                            IssueState::Open,
                            "Ready issue {} should be Open, was {:?}",
                            summary.qualified_id,
                            issue.state
                        );
                    }
                }

                // Invariant 3: cascade result is a subset of dependents, and every
                // returned issue has no remaining open blockers (treating closed_id
                // as closed).
                // Pick an arbitrary open issue to close for cascade testing.
                let open_issues: Vec<&QualifiedId> = issues
                    .iter()
                    .filter(|i| i.state == IssueState::Open)
                    .map(|i| &i.qualified_id)
                    .collect();
                if let Some(closed_id) = open_issues.first() {
                    let cascade = graph.compute_unblock_cascade(closed_id, &issues);
                    // Soundness: every returned issue has no remaining open blockers.
                    for unblocked_qid in &cascade {
                        if let Some(&dep_node) = graph.node_map.get(unblocked_qid) {
                            for blocker_idx in graph.graph.neighbors_directed(dep_node, Direction::Outgoing) {
                                let blocker_qid = &graph.graph[blocker_idx];
                                if blocker_qid == *closed_id {
                                    continue; // treated as closed
                                }
                                let blocker_state = graph.issue_state.get(blocker_qid);
                                prop_assert!(
                                    blocker_state == Some(&IssueState::Closed),
                                    "Cascade returned {} but blocker {} is still open",
                                    unblocked_qid,
                                    blocker_qid
                                );
                            }
                        }
                    }

                    // Completeness: every dependent of closed_id whose blockers
                    // are all resolved MUST appear in the cascade result.
                    let cascade_set: std::collections::HashSet<_> =
                        cascade.iter().collect();
                    if let Some(&closed_node) = graph.node_map.get(*closed_id) {
                        for dep_idx in graph.graph.neighbors_directed(closed_node, Direction::Incoming) {
                            let dep_qid = &graph.graph[dep_idx];
                            let all_resolved = graph
                                .graph
                                .neighbors_directed(dep_idx, Direction::Outgoing)
                                .all(|blocker_idx| {
                                    let blocker_qid = &graph.graph[blocker_idx];
                                    if blocker_qid == *closed_id {
                                        return true;
                                    }
                                    graph
                                        .issue_state
                                        .get(blocker_qid)
                                        .is_some_and(|s| *s == IssueState::Closed)
                                });
                            if all_resolved {
                                prop_assert!(
                                    cascade_set.contains(dep_qid),
                                    "Issue {} has all blockers resolved after closing {} but is missing from cascade",
                                    dep_qid,
                                    closed_id
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
                    .map(|(s, t)| edge(s, t))
                    .collect();

                let graph = DependencyGraph::build(&issues, &blocking_edges);

                // Only test if both probe nodes exist in the graph and are distinct.
                if probe_source != probe_target
                    && probe_source <= num_issues
                    && probe_target <= num_issues
                {
                    let probe_source_qid = qid(probe_source);
                    let probe_target_qid = qid(probe_target);
                    let would_cycle = graph.would_create_cycle(&probe_source_qid, &probe_target_qid);

                    if !would_cycle {
                        // Add the edge and rebuild to check no new cycle containing both.
                        let mut extended_edges = blocking_edges.clone();
                        extended_edges.push(edge(probe_source, probe_target));
                        let extended_graph = DependencyGraph::build(&issues, &extended_edges);
                        let cycles = extended_graph.detect_all_cycles();
                        for cycle in &cycles {
                            prop_assert!(
                                !(cycle.contains(&probe_source_qid) && cycle.contains(&probe_target_qid)),
                                "would_create_cycle({}, {}) returned false but they are in the same SCC: {:?}",
                                probe_source_qid,
                                probe_target_qid,
                                cycle
                            );
                        }
                    }
                }
            }

            /// Property: cross-repo issues with same number are distinct graph nodes.
            ///
            /// A graph with issues from two different repos (same numbers) should have
            /// twice as many nodes. The ready set computation should treat them
            /// independently.
            #[test]
            fn cross_repo_same_numbers_distinct_in_graph(
                num_issues in 1_u64..30,
                issue_states_a in proptest::collection::vec(arb_issue_state(), 1..30),
                issue_states_b in proptest::collection::vec(arb_issue_state(), 1..30),
                edges_within_a in proptest::collection::vec((1_u64..30, 1_u64..30), 0..50),
                edges_within_b in proptest::collection::vec((1_u64..30, 1_u64..30), 0..50),
            ) {
                let repo_a_owner = "org-a";
                let repo_a_name = "repo-a";
                let repo_b_owner = "org-b";
                let repo_b_name = "repo-b";

                // Create issues in both repos with the same numbers.
                let mut all_issues: Vec<Issue> = Vec::new();
                for n in 1..=num_issues {
                    let idx = usize::try_from(n - 1).expect("fits");
                    let state_a = issue_states_a.get(idx).copied().unwrap_or(IssueState::Open);
                    let state_b = issue_states_b.get(idx).copied().unwrap_or(IssueState::Open);
                    all_issues.push(make_issue_repo(repo_a_owner, repo_a_name, n, state_a, Priority::P2));
                    all_issues.push(make_issue_repo(repo_b_owner, repo_b_name, n, state_b, Priority::P2));
                }

                // Create edges within each repo only.
                let mut all_edges: Vec<BlockingEdge> = Vec::new();
                for (s, t) in &edges_within_a {
                    if s != t && *s <= num_issues && *t <= num_issues {
                        all_edges.push(BlockingEdge {
                            source: qid_repo(repo_a_owner, repo_a_name, *s),
                            target: qid_repo(repo_a_owner, repo_a_name, *t),
                        });
                    }
                }
                for (s, t) in &edges_within_b {
                    if s != t && *s <= num_issues && *t <= num_issues {
                        all_edges.push(BlockingEdge {
                            source: qid_repo(repo_b_owner, repo_b_name, *s),
                            target: qid_repo(repo_b_owner, repo_b_name, *t),
                        });
                    }
                }

                let graph = DependencyGraph::build(&all_issues, &all_edges);

                // Invariant: node count equals 2 * num_issues (both repos' issues are distinct).
                let expected_nodes = usize::try_from(2 * num_issues).expect("fits");
                prop_assert_eq!(
                    graph.graph.node_count(),
                    expected_nodes,
                    "Expected {} nodes (2 repos x {} issues), got {}",
                    expected_nodes,
                    num_issues,
                    graph.graph.node_count()
                );

                // Invariant: ready set issues are all Open.
                let ready = graph.compute_ready_set(&all_issues);
                for summary in &ready {
                    let original = all_issues.iter().find(|i| i.qualified_id == summary.qualified_id);
                    if let Some(issue) = original {
                        prop_assert_eq!(
                            issue.state,
                            IssueState::Open,
                            "Ready issue {} should be Open",
                            summary.qualified_id
                        );
                    }
                }
            }
        }
    }
}
