//! Reconciliation drift types for the unblock system.
//!
//! Defines `DriftKind` (6 drift variants) and `DriftReport` used by the
//! reconciliation engine to detect and report divergence between the computed
//! dependency graph and the state stored in GitHub Projects V2 fields.
//!
//! These types live in `unblock-core` to keep the reconcile engine pure (no I/O,
//! fully testable without network). See the reconciliation plan (§4) for the
//! complete drift taxonomy and design rationale.
//!
//! # Drift Types
//!
//! | Variant | Cause |
//! |---------|-------|
//! | `UncascadedClosure` | Issue closed via UI without cascade firing |
//! | `OrphanedBlockingEdge` | Edge references a non-existent or inaccessible issue |
//! | `MalformedAgentField` | Agent field has invalid format |
//! | `MissingProjectField` | Required Projects V2 field was deleted |
//! | `CycleDetected` | Cycle introduced by manual editing |
//! | `StaleClaim` | In-progress issue past the stale threshold |
//!
//! # Design Note
//!
//! `GhostedBlockingEdge` is intentionally absent from the taxonomy. In our model,
//! edges come from GitHub's `trackedByIssues` API (not body text), so the graph
//! cannot contain an edge that GitHub does not have. See spec §4 for the full
//! rationale.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::graph::DependencyGraph;
use crate::types::{Issue, IssueState, QualifiedId, Status};

/// Category of detected divergence between the dependency graph and GitHub state.
///
/// Covers all 7 realistic external mutation scenarios. Each variant carries
/// enough context to produce a human-readable diagnostic and, where applicable,
/// to drive automated repair.
///
/// # Serialization
///
/// Serializes as a tagged enum (default serde behaviour) so that drift reports
/// can be returned as structured JSON to MCP tool callers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DriftKind {
    /// Issue closed via UI — downstream issues should have received a cascade
    /// but did not.
    ///
    /// No cascade comment was added. Ready State was not updated on downstream
    /// issues.
    UncascadedClosure {
        /// The issue that was closed outside the MCP server.
        closed_issue: QualifiedId,
        /// Downstream issues that should have been unblocked by the closure.
        should_have_unblocked: Vec<QualifiedId>,
    },

    /// Blocking edge references an issue that does not exist or is inaccessible.
    ///
    /// Cause: issue was deleted (admin action), or references a cross-repo issue
    /// the token cannot access.
    OrphanedBlockingEdge {
        /// The source issue of the orphaned edge.
        source: QualifiedId,
        /// The target issue that does not exist or is inaccessible.
        missing_target: QualifiedId,
    },

    /// Agent field has invalid format (must be `username:supervisor`).
    MalformedAgentField {
        /// The issue with the malformed field.
        issue: QualifiedId,
        /// The raw value found in the Agent field.
        raw_value: String,
    },

    /// Required Projects V2 field is missing.
    ///
    /// Cause: field deleted in GitHub Projects settings.
    MissingProjectField {
        /// Name of the missing field (e.g., `"Ready State"`, `"Agent"`).
        field_name: String,
    },

    /// Cycle detected in the dependency graph.
    ///
    /// Likely introduced by manual editing. Requires human decision to resolve
    /// — the reconcile engine reports but does not auto-repair cycles.
    CycleDetected {
        /// The issues forming the cycle, in order.
        cycle: Vec<QualifiedId>,
    },

    /// Issue in `in_progress` state with `claimed_at` more than N hours ago
    /// without update.
    ///
    /// Reported but not auto-repaired (Design Decision R2) — releasing a stale
    /// claim is an agent or human decision.
    StaleClaim {
        /// The issue with the stale claim.
        issue: QualifiedId,
        /// When the issue was claimed by an agent.
        claimed_at: DateTime<Utc>,
        /// How many hours the claim has been stale.
        hours_stale: u64,
    },
}

/// Full report from a reconciliation run.
///
/// Produced by the reconciliation engine after comparing the computed dependency
/// graph against the state stored in GitHub. Contains all detected drift, any
/// repairs performed (when `--fix` is enabled), and errors for drift that could
/// not be automatically repaired.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriftReport {
    /// Repository in `owner/repo` format.
    pub repo: String,
    /// Timestamp of the reconciliation run.
    pub reconciled_at: DateTime<Utc>,
    /// Number of issues scanned during reconciliation.
    pub issues_scanned: usize,
    /// Number of edges scanned during reconciliation.
    pub edges_scanned: usize,
    /// All drift detected during reconciliation.
    pub drift_found: Vec<DriftKind>,
    /// Subset of `drift_found` that was successfully repaired (only when `--fix` is enabled).
    pub repaired: Vec<DriftKind>,
    /// Drift that was detected but could not be repaired, with descriptive messages.
    pub errors: Vec<String>,
    /// `true` if `drift_found` is empty — no divergence detected.
    pub clean: bool,
}

/// Pure reconciliation engine — detects drift without performing I/O.
///
/// Compares the computed dependency graph against stored GitHub Projects V2
/// field values and reports all divergence as a [`DriftReport`].
///
/// Design Decision R1: the engine is pure (no I/O). It receives everything
/// it needs as arguments. I/O stays in the MCP tool handler (task 1.6.3/1.6.4).
///
/// # Usage
///
/// ```
/// use unblock_core::reconcile::ReconcileEngine;
///
/// let engine = ReconcileEngine::new(24); // stale threshold: 24 hours
/// // let report = engine.analyse(&graph, &issues, &ready_set, now);
/// ```
#[derive(Debug, Clone)]
pub struct ReconcileEngine {
    /// Claims older than this many hours are flagged as [`DriftKind::StaleClaim`].
    stale_claim_threshold_hours: u64,
}

impl ReconcileEngine {
    /// Create a new `ReconcileEngine` with the given stale claim threshold.
    ///
    /// `stale_claim_threshold_hours` controls how many hours an `InProgress`
    /// issue can remain claimed before it is reported as a [`DriftKind::StaleClaim`].
    /// A typical default is 24 hours.
    #[must_use]
    pub fn new(stale_claim_threshold_hours: u64) -> Self {
        Self {
            stale_claim_threshold_hours,
        }
    }

    /// Analyse the dependency graph for drift against stored GitHub field values.
    ///
    /// Performs 5 checks in order:
    /// 1. **Uncascaded Closure** — closed issue did not cascade to downstream
    /// 2. **Orphaned Blocking Edge** — edge references a non-existent issue
    /// 3. **Cycle Detected** — cycle in the dependency graph
    /// 4. **Stale Claim** — `InProgress` issue past threshold
    /// 5. **Malformed Agent Field** — agent field without `username:supervisor` format
    ///
    /// `MissingProjectField` is NOT detected here — it requires I/O to check
    /// GitHub Projects V2 field existence and is handled by the tool handler.
    ///
    /// # Arguments
    ///
    /// * `graph` — The dependency graph built from the current issue set.
    /// * `issues` — All issues keyed by [`QualifiedId`], as fetched from GitHub.
    /// * `computed_ready_set` — The set of [`QualifiedId`] values that the graph
    ///   computes as ready (derived from [`DependencyGraph::compute_ready_set()`]).
    /// * `now` — Current time, injected for testability.
    #[must_use]
    pub fn analyse(
        &self,
        graph: &DependencyGraph,
        issues: &HashMap<QualifiedId, Issue>,
        computed_ready_set: &HashSet<QualifiedId>,
        now: DateTime<Utc>,
    ) -> DriftReport {
        let mut drift = Vec::new();

        Self::check_uncascaded_closures(graph, issues, computed_ready_set, &mut drift);
        Self::check_orphaned_edges(graph, issues, &mut drift);
        Self::check_cycles(graph, &mut drift);
        self.check_stale_claims(issues, now, &mut drift);
        Self::check_malformed_agents(issues, &mut drift);

        DriftReport {
            repo: String::new(), // Filled by the tool handler.
            reconciled_at: now,
            issues_scanned: issues.len(),
            edges_scanned: graph.edge_count(),
            clean: drift.is_empty(),
            drift_found: drift,
            repaired: vec![],
            errors: vec![],
        }
    }

    /// Pass 1 — detect closed issues whose downstream dependents are still
    /// blocked (unblock cascade did not run).
    ///
    /// Uses the graph-computed ready set: if a downstream issue is open, in the
    /// ready set (all blockers resolved), but still has `Status::Blocked`, the
    /// cascade was missed.
    fn check_uncascaded_closures(
        graph: &DependencyGraph,
        issues: &HashMap<QualifiedId, Issue>,
        computed_ready_set: &HashSet<QualifiedId>,
        drift: &mut Vec<DriftKind>,
    ) {
        for (qid, issue) in issues {
            if issue.state == IssueState::Closed {
                // `compute_unblock_cascade` reserves an `_all_issues` slice for
                // future use but currently ignores it; pass an empty slice to
                // avoid cloning every issue on each reconcile cycle.
                let should_have_unblocked: Vec<QualifiedId> = graph
                    .compute_unblock_cascade(qid, &[])
                    .into_iter()
                    .filter(|id| {
                        issues.get(id).is_some_and(|i| {
                            i.state == IssueState::Open
                                && computed_ready_set.contains(&i.qualified_id)
                                && i.status != Status::Ready
                        })
                    })
                    .collect();

                if !should_have_unblocked.is_empty() {
                    drift.push(DriftKind::UncascadedClosure {
                        closed_issue: qid.clone(),
                        should_have_unblocked,
                    });
                }
            }
        }
    }

    /// Pass 3 — detect blocking edges whose source or target issue is missing
    /// from the fetched issue map.
    fn check_orphaned_edges(
        graph: &DependencyGraph,
        issues: &HashMap<QualifiedId, Issue>,
        drift: &mut Vec<DriftKind>,
    ) {
        // Check both endpoints: an edge is orphaned if either the source or target
        // issue is missing from the fetched issues map. When the source is missing,
        // we report it using the existing variant with the source as the missing
        // endpoint in `missing_target` (the field semantically means "missing node").
        for edge in graph.all_edges() {
            if !issues.contains_key(&edge.target) {
                drift.push(DriftKind::OrphanedBlockingEdge {
                    source: edge.source.clone(),
                    missing_target: edge.target.clone(),
                });
            }
            if !issues.contains_key(&edge.source) {
                drift.push(DriftKind::OrphanedBlockingEdge {
                    source: edge.target.clone(),
                    missing_target: edge.source.clone(),
                });
            }
        }
    }

    /// Pass 4 — detect cycles in the dependency graph.
    fn check_cycles(graph: &DependencyGraph, drift: &mut Vec<DriftKind>) {
        for cycle in graph.detect_all_cycles() {
            drift.push(DriftKind::CycleDetected { cycle });
        }
    }

    /// Pass 5 — detect in-progress issues whose claim exceeds the configured
    /// stale-claim threshold.
    fn check_stale_claims(
        &self,
        issues: &HashMap<QualifiedId, Issue>,
        now: DateTime<Utc>,
        drift: &mut Vec<DriftKind>,
    ) {
        for (qid, issue) in issues {
            if issue.status == Status::InProgress
                && let Some(claimed_at) = issue.claimed_at
            {
                let hours = (now - claimed_at).num_hours().max(0).cast_unsigned();
                if hours > self.stale_claim_threshold_hours {
                    drift.push(DriftKind::StaleClaim {
                        issue: qid.clone(),
                        claimed_at,
                        hours_stale: hours,
                    });
                }
            }
        }
    }

    /// Pass 6 — detect malformed `agent` fields (non-empty, missing the
    /// `type:id` colon separator).
    fn check_malformed_agents(issues: &HashMap<QualifiedId, Issue>, drift: &mut Vec<DriftKind>) {
        for (qid, issue) in issues {
            if let Some(ref agent) = issue.agent
                && !agent.contains(':')
                && !agent.is_empty()
            {
                drift.push(DriftKind::MalformedAgentField {
                    issue: qid.clone(),
                    raw_value: agent.clone(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::DependencyGraph;
    use crate::types::{BlockingEdge, Issue, IssueState, Priority, QualifiedId, Status};
    use chrono::{Duration, Utc};
    use std::collections::{HashMap, HashSet};

    /// Helper to create a test `QualifiedId`.
    fn qid(owner: &str, repo: &str, number: u64) -> QualifiedId {
        QualifiedId::new(owner, repo, number)
    }

    #[test]
    fn construct_uncascaded_closure() {
        let drift = DriftKind::UncascadedClosure {
            closed_issue: qid("acme", "widgets", 10),
            should_have_unblocked: vec![qid("acme", "widgets", 11), qid("acme", "widgets", 12)],
        };
        assert!(matches!(drift, DriftKind::UncascadedClosure { .. }));
    }

    #[test]
    fn construct_orphaned_blocking_edge() {
        let drift = DriftKind::OrphanedBlockingEdge {
            source: qid("acme", "widgets", 5),
            missing_target: qid("acme", "widgets", 999),
        };
        assert!(matches!(drift, DriftKind::OrphanedBlockingEdge { .. }));
    }

    #[test]
    fn construct_malformed_agent_field() {
        let drift = DriftKind::MalformedAgentField {
            issue: qid("acme", "widgets", 7),
            raw_value: "bad-format-no-colon".to_string(),
        };
        assert!(matches!(drift, DriftKind::MalformedAgentField { .. }));
    }

    #[test]
    fn construct_missing_project_field() {
        let drift = DriftKind::MissingProjectField {
            field_name: "Status".to_string(),
        };
        assert!(matches!(drift, DriftKind::MissingProjectField { .. }));
    }

    #[test]
    fn construct_cycle_detected() {
        let drift = DriftKind::CycleDetected {
            cycle: vec![
                qid("acme", "widgets", 1),
                qid("acme", "widgets", 2),
                qid("acme", "widgets", 3),
            ],
        };
        assert!(matches!(drift, DriftKind::CycleDetected { .. }));
    }

    #[test]
    fn construct_stale_claim() {
        let drift = DriftKind::StaleClaim {
            issue: qid("acme", "widgets", 15),
            claimed_at: Utc::now(),
            hours_stale: 48,
        };
        assert!(matches!(drift, DriftKind::StaleClaim { .. }));
    }

    #[test]
    fn construct_drift_report() {
        let report = DriftReport {
            repo: "acme/widgets".to_string(),
            reconciled_at: Utc::now(),
            issues_scanned: 47,
            edges_scanned: 23,
            drift_found: vec![DriftKind::UncascadedClosure {
                closed_issue: qid("acme", "widgets", 42),
                should_have_unblocked: vec![qid("acme", "widgets", 43)],
            }],
            repaired: vec![],
            errors: vec![],
            clean: false,
        };
        assert!(!report.clean);
        assert_eq!(report.issues_scanned, 47);
        assert_eq!(report.edges_scanned, 23);
        assert_eq!(report.drift_found.len(), 1);
    }

    #[test]
    fn drift_report_clean_when_no_drift() {
        let report = DriftReport {
            repo: "acme/widgets".to_string(),
            reconciled_at: Utc::now(),
            issues_scanned: 10,
            edges_scanned: 5,
            drift_found: vec![],
            repaired: vec![],
            errors: vec![],
            clean: true,
        };
        assert!(report.clean);
        assert!(report.drift_found.is_empty());
    }

    #[test]
    fn drift_kind_serde_round_trip() {
        let variants: Vec<DriftKind> = vec![
            DriftKind::UncascadedClosure {
                closed_issue: qid("acme", "widgets", 2),
                should_have_unblocked: vec![qid("acme", "widgets", 3)],
            },
            DriftKind::OrphanedBlockingEdge {
                source: qid("acme", "widgets", 4),
                missing_target: qid("acme", "widgets", 5),
            },
            DriftKind::MalformedAgentField {
                issue: qid("acme", "widgets", 6),
                raw_value: "no-colon".to_string(),
            },
            DriftKind::MissingProjectField {
                field_name: "Status".to_string(),
            },
            DriftKind::CycleDetected {
                cycle: vec![qid("acme", "widgets", 7), qid("acme", "widgets", 8)],
            },
            DriftKind::StaleClaim {
                issue: qid("acme", "widgets", 9),
                claimed_at: Utc::now(),
                hours_stale: 72,
            },
        ];

        for variant in &variants {
            let json = serde_json::to_string(variant).expect("serialize DriftKind");
            let deserialized: DriftKind =
                serde_json::from_str(&json).expect("deserialize DriftKind");
            assert_eq!(*variant, deserialized);
        }
    }

    #[test]
    fn drift_report_serde_round_trip() {
        let report = DriftReport {
            repo: "acme/widgets".to_string(),
            reconciled_at: Utc::now(),
            issues_scanned: 100,
            edges_scanned: 50,
            drift_found: vec![
                DriftKind::UncascadedClosure {
                    closed_issue: qid("acme", "widgets", 1),
                    should_have_unblocked: vec![qid("acme", "widgets", 4)],
                },
                DriftKind::CycleDetected {
                    cycle: vec![qid("acme", "widgets", 2), qid("acme", "widgets", 3)],
                },
            ],
            repaired: vec![DriftKind::UncascadedClosure {
                closed_issue: qid("acme", "widgets", 1),
                should_have_unblocked: vec![qid("acme", "widgets", 4)],
            }],
            errors: vec!["CycleDetected: manual resolution required".to_string()],
            clean: false,
        };

        let json = serde_json::to_string(&report).expect("serialize DriftReport");
        let deserialized: DriftReport =
            serde_json::from_str(&json).expect("deserialize DriftReport");
        assert_eq!(report, deserialized);
    }

    // ── ReconcileEngine tests ───────────────────────────────────────────

    /// Helper to create a minimal [`Issue`] for reconciliation tests.
    fn make_issue(
        owner: &str,
        repo: &str,
        number: u64,
        state: IssueState,
        status: Status,
    ) -> Issue {
        Issue {
            qualified_id: qid(owner, repo, number),
            number,
            node_id: String::new(),
            title: format!("Issue #{number}"),
            issue_type: None,
            status,
            priority: Priority::P2,
            agent: None,
            claimed_at: None,
            pipeline_stage: None,
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

    /// Helper to create a [`BlockingEdge`].
    fn edge(source: &QualifiedId, target: &QualifiedId) -> BlockingEdge {
        BlockingEdge {
            source: source.clone(),
            target: target.clone(),
        }
    }

    /// Build a `HashMap<QualifiedId, Issue>` from a slice of issues.
    fn issue_map(issues: &[Issue]) -> HashMap<QualifiedId, Issue> {
        issues
            .iter()
            .map(|i| (i.qualified_id.clone(), i.clone()))
            .collect()
    }

    /// Compute the ready set as a `HashSet<QualifiedId>` from the graph.
    ///
    /// Takes the configured `(owner, repo)` pair so SPEC §3.3 Filter 3
    /// (§14 Invariant 14(a)) admits the local issues — reconcile tests use
    /// a single repo per fixture (`acme/widgets` or `acme/test`).
    fn ready_set(
        graph: &DependencyGraph,
        issues: &[Issue],
        configured_owner: &str,
        configured_repo: &str,
    ) -> HashSet<QualifiedId> {
        graph
            .compute_ready_set(issues, configured_owner, configured_repo)
            .into_iter()
            .map(|s| s.qualified_id)
            .collect()
    }

    #[test]
    fn detects_uncascaded_closure() {
        // Issue #1 blocks #2. #1 was closed externally (no cascade).
        // #2 is still open and not marked Ready.
        let q1 = qid("acme", "widgets", 1);
        let q2 = qid("acme", "widgets", 2);

        let issue1 = make_issue("acme", "widgets", 1, IssueState::Closed, Status::Closed);
        let issue2 = make_issue("acme", "widgets", 2, IssueState::Open, Status::Blocked);

        let issues_vec = vec![issue1, issue2];
        let edges = vec![edge(&q2, &q1)]; // #2 is blocked by #1
        let graph = DependencyGraph::build(&issues_vec, &edges);
        let computed_ready = ready_set(&graph, &issues_vec, "acme", "widgets");
        let by_id = issue_map(&issues_vec);

        let engine = ReconcileEngine::new(24);
        let report = engine.analyse(&graph, &by_id, &computed_ready, Utc::now());

        assert!(!report.clean);
        assert!(
            report
                .drift_found
                .iter()
                .any(|d| matches!(d, DriftKind::UncascadedClosure { .. }))
        );
    }

    #[test]
    fn clean_report_when_consistent() {
        // Two open issues, no edges. Both marked Ready. Graph is consistent.
        let issue1 = make_issue("acme", "widgets", 1, IssueState::Open, Status::Ready);
        let issue2 = make_issue("acme", "widgets", 2, IssueState::Open, Status::Ready);

        let issues_vec = vec![issue1, issue2];
        let graph = DependencyGraph::build(&issues_vec, &[]);
        let computed_ready = ready_set(&graph, &issues_vec, "acme", "widgets");
        let by_id = issue_map(&issues_vec);

        let engine = ReconcileEngine::new(24);
        let report = engine.analyse(&graph, &by_id, &computed_ready, Utc::now());

        assert!(report.clean);
        assert!(report.drift_found.is_empty());
        assert_eq!(report.issues_scanned, 2);
        assert_eq!(report.edges_scanned, 0);
    }

    #[test]
    fn detects_cycle() {
        // A -> B -> C -> A (cycle introduced by manual editing).
        let q1 = qid("acme", "widgets", 1);
        let q2 = qid("acme", "widgets", 2);
        let q3 = qid("acme", "widgets", 3);

        let issue1 = make_issue("acme", "widgets", 1, IssueState::Open, Status::Blocked);
        let issue2 = make_issue("acme", "widgets", 2, IssueState::Open, Status::Blocked);
        let issue3 = make_issue("acme", "widgets", 3, IssueState::Open, Status::Blocked);

        let issues_vec = vec![issue1, issue2, issue3];
        let edges = vec![edge(&q1, &q2), edge(&q2, &q3), edge(&q3, &q1)];
        let graph = DependencyGraph::build(&issues_vec, &edges);
        let computed_ready = ready_set(&graph, &issues_vec, "acme", "widgets");
        let by_id = issue_map(&issues_vec);

        let engine = ReconcileEngine::new(24);
        let report = engine.analyse(&graph, &by_id, &computed_ready, Utc::now());

        assert!(!report.clean);
        assert!(
            report
                .drift_found
                .iter()
                .any(|d| matches!(d, DriftKind::CycleDetected { .. }))
        );
    }

    #[test]
    fn detects_stale_claim() {
        // Issue #1 is InProgress, claimed 48 hours ago (threshold = 24h).
        let mut issue1 = make_issue("acme", "widgets", 1, IssueState::Open, Status::Ready);
        issue1.status = Status::InProgress;
        issue1.agent = Some("agent:supervisor".to_string());
        issue1.claimed_at = Some(Utc::now() - Duration::hours(48));

        let issues_vec = vec![issue1];
        let graph = DependencyGraph::build(&issues_vec, &[]);
        let computed_ready = ready_set(&graph, &issues_vec, "acme", "widgets");
        let by_id = issue_map(&issues_vec);

        let engine = ReconcileEngine::new(24);
        let report = engine.analyse(&graph, &by_id, &computed_ready, Utc::now());

        assert!(!report.clean);
        assert!(report.drift_found.iter().any(|d| matches!(
            d,
            DriftKind::StaleClaim { hours_stale, .. } if *hours_stale >= 47
        )));
    }

    #[test]
    fn detects_malformed_agent_field() {
        // Issue #1 has agent "bad-format-no-colon" (missing ':').
        let mut issue1 = make_issue("acme", "widgets", 1, IssueState::Open, Status::Ready);
        issue1.agent = Some("bad-format-no-colon".to_string());

        let issues_vec = vec![issue1];
        let graph = DependencyGraph::build(&issues_vec, &[]);
        let computed_ready = ready_set(&graph, &issues_vec, "acme", "widgets");
        let by_id = issue_map(&issues_vec);

        let engine = ReconcileEngine::new(24);
        let report = engine.analyse(&graph, &by_id, &computed_ready, Utc::now());

        assert!(!report.clean);
        assert!(report.drift_found.iter().any(|d| matches!(
            d,
            DriftKind::MalformedAgentField {
                raw_value,
                ..
            } if raw_value == "bad-format-no-colon"
        )));
    }

    #[test]
    fn detects_orphaned_blocking_edge() {
        // Issue #1 exists, but the edge points to #999 which is NOT in the issues map.
        let q1 = qid("acme", "widgets", 1);
        let q999 = qid("acme", "widgets", 999);

        let issue1 = make_issue("acme", "widgets", 1, IssueState::Open, Status::Blocked);
        // #999 is in the graph (as a node created by the edge) but NOT in the issues map.
        // We need #999 to exist as a node in the graph for the edge to be added.
        let issue999 = make_issue("acme", "widgets", 999, IssueState::Open, Status::Ready);

        let issues_vec = vec![issue1, issue999.clone()];
        let edges = vec![edge(&q1, &q999)]; // #1 is blocked by #999
        let graph = DependencyGraph::build(&issues_vec, &edges);

        // Build the issues map with only #1 — #999 is "missing" from the fetched issues.
        let mut by_id = HashMap::new();
        by_id.insert(q1.clone(), issues_vec[0].clone());
        // Intentionally omit #999 to simulate orphaned edge.

        let computed_ready: HashSet<QualifiedId> = HashSet::new();

        let engine = ReconcileEngine::new(24);
        let report = engine.analyse(&graph, &by_id, &computed_ready, Utc::now());

        assert!(!report.clean);
        assert!(report.drift_found.iter().any(|d| matches!(
            d,
            DriftKind::OrphanedBlockingEdge {
                missing_target,
                ..
            } if missing_target.number == 999
        )));
    }

    #[test]
    fn no_false_positives_on_clean_graph() {
        // 5+ issues with varied topology (some edges, some standalone),
        // all with CORRECT Status. The engine must return clean: true.
        //
        // Topology:
        //   #1 (open, standalone)          → Ready
        //   #2 (open, blocked by #5 closed) → Ready (blocker is closed)
        //   #3 (open, blocked by #4 open)   → Blocked
        //   #4 (open, standalone)           → Ready
        //   #5 (closed)                     → Closed
        //   #6 (open, blocked by #5 closed) → Ready (blocker is closed)

        let q2 = qid("acme", "widgets", 2);
        let q3 = qid("acme", "widgets", 3);
        let q4 = qid("acme", "widgets", 4);
        let q5 = qid("acme", "widgets", 5);
        let q6 = qid("acme", "widgets", 6);

        let issue1 = make_issue("acme", "widgets", 1, IssueState::Open, Status::Ready);
        let issue2 = make_issue("acme", "widgets", 2, IssueState::Open, Status::Ready);
        let issue3 = make_issue("acme", "widgets", 3, IssueState::Open, Status::Blocked);
        let issue4 = make_issue("acme", "widgets", 4, IssueState::Open, Status::Ready);
        let issue5 = make_issue("acme", "widgets", 5, IssueState::Closed, Status::Closed);
        let issue6 = make_issue("acme", "widgets", 6, IssueState::Open, Status::Ready);

        let issues_vec = vec![issue1, issue2, issue3, issue4, issue5, issue6];
        let edges = vec![
            edge(&q2, &q5), // #2 blocked by #5 (closed — so #2 is Ready)
            edge(&q3, &q4), // #3 blocked by #4 (open — so #3 is Blocked)
            edge(&q6, &q5), // #6 blocked by #5 (closed — so #6 is Ready)
        ];
        let graph = DependencyGraph::build(&issues_vec, &edges);
        let computed_ready = ready_set(&graph, &issues_vec, "acme", "widgets");
        let by_id = issue_map(&issues_vec);

        let engine = ReconcileEngine::new(24);
        let report = engine.analyse(&graph, &by_id, &computed_ready, Utc::now());

        assert!(
            report.clean,
            "Expected clean graph but found drift: {:?}",
            report.drift_found
        );
        assert!(report.drift_found.is_empty());
        assert_eq!(report.issues_scanned, 6);
        assert_eq!(report.edges_scanned, 3);
    }

    // ── Property tests (proptest) ───────────────────────────────────────

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
            /// Property: a consistent graph (where every issue's Status matches
            /// what the graph computes) always produces a clean DriftReport.
            ///
            /// Strategy:
            /// 1. Generate N issues with random IssueState and Priority.
            /// 2. Generate random edges (filtering self-loops and out-of-range).
            /// 3. Remove edges that would create cycles (to avoid CycleDetected drift).
            /// 4. Build the graph and compute the ready set.
            /// 5. SET each issue's Status to match the graph computation:
            ///    - Closed → Status::Closed
            ///    - Open + in ready set → Status::Ready
            ///    - Open + not in ready set → Status::Blocked
            /// 6. Assert ReconcileEngine::analyse() returns clean: true.
            #[test]
            fn prop_reconcile_on_consistent_graph_always_clean(
                num_issues in 1_u64..30,
                issue_states in proptest::collection::vec(arb_issue_state(), 1..30),
                issue_priorities in proptest::collection::vec(arb_priority(), 1..30),
                raw_edges in proptest::collection::vec((1_u64..30, 1_u64..30), 0..60),
            ) {
                // 1. Generate issues with random states and priorities.
                let mut issues: Vec<Issue> = (1..=num_issues)
                    .map(|n| {
                        let idx = usize::try_from(n - 1).expect("fits in usize");
                        let state = issue_states.get(idx).copied().unwrap_or(IssueState::Open);
                        let priority = issue_priorities.get(idx).copied().unwrap_or(Priority::P2);
                        let mut issue = make_issue("acme", "test", n, state, Status::Ready);
                        issue.priority = priority;
                        issue
                    })
                    .collect();

                // 2. Filter edges: valid range, no self-loops.
                let candidate_edges: Vec<(u64, u64)> = raw_edges
                    .into_iter()
                    .filter(|(s, t)| *s != *t && *s <= num_issues && *t <= num_issues)
                    .collect();

                // 3. Remove edges that would create cycles.
                // Add edges one at a time, checking for cycles.
                let mut safe_edges: Vec<BlockingEdge> = Vec::new();
                for (s, t) in candidate_edges {
                    let source_qid = qid("acme", "test", s);
                    let target_qid = qid("acme", "test", t);

                    // Build a temporary graph with the candidate edge to check for cycles.
                    let mut test_edges = safe_edges.clone();
                    test_edges.push(BlockingEdge {
                        source: source_qid,
                        target: target_qid,
                    });
                    let test_graph = DependencyGraph::build(&issues, &test_edges);
                    if test_graph.detect_all_cycles().is_empty() {
                        safe_edges = test_edges;
                    }
                }

                // 4. Build graph and compute ready set.
                let graph = DependencyGraph::build(&issues, &safe_edges);
                let computed_ready = ready_set(&graph, &issues, "acme", "test");

                // 5. Set each issue's Status to match what the graph says.
                for issue in &mut issues {
                    if issue.state == IssueState::Closed {
                        issue.status = Status::Closed;
                    } else if computed_ready.contains(&issue.qualified_id) {
                        issue.status = Status::Ready;
                    } else {
                        issue.status = Status::Blocked;
                    }
                }

                // Rebuild graph with corrected Status values (graph itself
                // doesn't use Status, but we need the same structure).
                let graph = DependencyGraph::build(&issues, &safe_edges);
                let computed_ready = ready_set(&graph, &issues, "acme", "test");
                let by_id = issue_map(&issues);

                // 6. Analyse and assert clean.
                let engine = ReconcileEngine::new(24);
                let report = engine.analyse(&graph, &by_id, &computed_ready, chrono::Utc::now());

                // The graph should be clean: no cycles (we filtered them),
                // no uncascaded closures (status matches graph), no stale claims
                // (no agent/claimed_at set), no malformed agents (agent is None),
                // no orphaned edges (all issues exist in the map).
                prop_assert!(
                    report.clean,
                    "Expected clean report for consistent graph but got drift: {:?}",
                    report.drift_found
                );
            }
        }
    }
}
