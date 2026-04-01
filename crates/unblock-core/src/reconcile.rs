//! Reconciliation drift types for the unblock system.
//!
//! Defines `DriftKind` (7 drift variants) and `DriftReport` used by the
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
//! | `StaleReadyState` | Ready State field diverges from graph computation |
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
use crate::types::{Issue, IssueState, QualifiedId, ReadyState, Status};

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
    /// Ready State field in GitHub diverges from what the graph computes.
    ///
    /// Covers: external close without cascade, external reopen, manual
    /// removal of blocking relationship via UI.
    StaleReadyState {
        /// The issue whose Ready State field is incorrect.
        issue: QualifiedId,
        /// The value currently stored in the GitHub Projects V2 field.
        field_says: ReadyState,
        /// The value the dependency graph computes as correct.
        graph_says: ReadyState,
    },

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
    /// Performs 6 checks in order:
    /// 1. **Stale Ready State** — `ReadyState` field diverges from graph computation
    /// 2. **Uncascaded Closure** — closed issue did not cascade to downstream
    /// 3. **Orphaned Blocking Edge** — edge references a non-existent issue
    /// 4. **Cycle Detected** — cycle in the dependency graph
    /// 5. **Stale Claim** — `InProgress` issue past threshold
    /// 6. **Malformed Agent Field** — agent field without `username:supervisor` format
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

        // 1. Stale Ready State fields
        // Compare all 4 ReadyState variants (Ready, Blocked, NotReady, Closed).
        for (qid, issue) in issues {
            if issue.state == IssueState::Closed {
                // Closed issues should have ReadyState::Closed.
                if issue.ready_state != ReadyState::Closed {
                    drift.push(DriftKind::StaleReadyState {
                        issue: qid.clone(),
                        field_says: issue.ready_state,
                        graph_says: ReadyState::Closed,
                    });
                }
                continue;
            }

            let graph_ready = computed_ready_set.contains(qid);
            let expected = if graph_ready {
                ReadyState::Ready
            } else {
                ReadyState::Blocked
            };

            if issue.ready_state != expected {
                drift.push(DriftKind::StaleReadyState {
                    issue: qid.clone(),
                    field_says: issue.ready_state,
                    graph_says: expected,
                });
            }
        }

        // 2. Uncascaded closures
        // Closed issues whose downstream issues are still marked as blocked.
        let issues_vec: Vec<Issue> = issues.values().cloned().collect();
        for (qid, issue) in issues {
            if issue.state == IssueState::Closed {
                let should_have_unblocked: Vec<QualifiedId> = graph
                    .compute_unblock_cascade(qid, &issues_vec)
                    .into_iter()
                    .filter(|id| {
                        issues.get(id).is_some_and(|i| {
                            i.ready_state != ReadyState::Ready && i.state == IssueState::Open
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

        // 3. Orphaned blocking edges
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

        // 4. Cycles
        for cycle in graph.detect_all_cycles() {
            drift.push(DriftKind::CycleDetected { cycle });
        }

        // 5. Stale claims
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

        // 6. Malformed agent fields
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::DependencyGraph;
    use crate::types::{
        BlockingEdge, Issue, IssueState, Priority, QualifiedId, ReadyState, Status,
    };
    use chrono::{Duration, Utc};
    use std::collections::{HashMap, HashSet};

    /// Helper to create a test `QualifiedId`.
    fn qid(owner: &str, repo: &str, number: u64) -> QualifiedId {
        QualifiedId::new(owner, repo, number)
    }

    #[test]
    fn construct_stale_ready_state() {
        let drift = DriftKind::StaleReadyState {
            issue: qid("acme", "widgets", 42),
            field_says: ReadyState::Blocked,
            graph_says: ReadyState::Ready,
        };
        assert!(matches!(drift, DriftKind::StaleReadyState { .. }));
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
            field_name: "Ready State".to_string(),
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
            drift_found: vec![DriftKind::StaleReadyState {
                issue: qid("acme", "widgets", 42),
                field_says: ReadyState::Blocked,
                graph_says: ReadyState::Ready,
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
            DriftKind::StaleReadyState {
                issue: qid("acme", "widgets", 1),
                field_says: ReadyState::Blocked,
                graph_says: ReadyState::Ready,
            },
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
                DriftKind::StaleReadyState {
                    issue: qid("acme", "widgets", 1),
                    field_says: ReadyState::NotReady,
                    graph_says: ReadyState::Ready,
                },
                DriftKind::CycleDetected {
                    cycle: vec![qid("acme", "widgets", 2), qid("acme", "widgets", 3)],
                },
            ],
            repaired: vec![DriftKind::StaleReadyState {
                issue: qid("acme", "widgets", 1),
                field_says: ReadyState::NotReady,
                graph_says: ReadyState::Ready,
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
        ready_state: ReadyState,
    ) -> Issue {
        Issue {
            qualified_id: qid(owner, repo, number),
            number,
            node_id: String::new(),
            title: format!("Issue #{number}"),
            issue_type: None,
            status: Status::Open,
            priority: Priority::P2,
            agent: None,
            claimed_at: None,
            ready_state,
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
    fn ready_set(graph: &DependencyGraph, issues: &[Issue]) -> HashSet<QualifiedId> {
        graph
            .compute_ready_set(issues)
            .into_iter()
            .map(|s| s.qualified_id)
            .collect()
    }

    #[test]
    fn detects_stale_ready_state_after_external_close() {
        // Issue #1 blocks #2. #1 is closed externally.
        // #2's Ready State still says Blocked but the graph says Ready.
        let q1 = qid("acme", "widgets", 1);
        let q2 = qid("acme", "widgets", 2);

        let issue1 = make_issue("acme", "widgets", 1, IssueState::Closed, ReadyState::Closed);
        let issue2 = make_issue("acme", "widgets", 2, IssueState::Open, ReadyState::Blocked);
        // issue2 still has Blocked even though its only blocker is closed.

        let issues_vec = vec![issue1, issue2];
        let edges = vec![edge(&q2, &q1)]; // #2 is blocked by #1
        let graph = DependencyGraph::build(&issues_vec, &edges);
        let computed_ready = ready_set(&graph, &issues_vec);
        let by_id = issue_map(&issues_vec);

        let engine = ReconcileEngine::new(24);
        let report = engine.analyse(&graph, &by_id, &computed_ready, Utc::now());

        assert!(!report.clean);
        assert!(report.drift_found.iter().any(|d| matches!(
            d,
            DriftKind::StaleReadyState {
                graph_says: ReadyState::Ready,
                field_says: ReadyState::Blocked,
                ..
            }
        )));
    }

    #[test]
    fn detects_uncascaded_closure() {
        // Issue #1 blocks #2. #1 was closed externally (no cascade).
        // #2 is still open and not marked Ready.
        let q1 = qid("acme", "widgets", 1);
        let q2 = qid("acme", "widgets", 2);

        let issue1 = make_issue("acme", "widgets", 1, IssueState::Closed, ReadyState::Closed);
        let issue2 = make_issue("acme", "widgets", 2, IssueState::Open, ReadyState::Blocked);

        let issues_vec = vec![issue1, issue2];
        let edges = vec![edge(&q2, &q1)]; // #2 is blocked by #1
        let graph = DependencyGraph::build(&issues_vec, &edges);
        let computed_ready = ready_set(&graph, &issues_vec);
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
        let issue1 = make_issue("acme", "widgets", 1, IssueState::Open, ReadyState::Ready);
        let issue2 = make_issue("acme", "widgets", 2, IssueState::Open, ReadyState::Ready);

        let issues_vec = vec![issue1, issue2];
        let graph = DependencyGraph::build(&issues_vec, &[]);
        let computed_ready = ready_set(&graph, &issues_vec);
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

        let issue1 = make_issue("acme", "widgets", 1, IssueState::Open, ReadyState::Blocked);
        let issue2 = make_issue("acme", "widgets", 2, IssueState::Open, ReadyState::Blocked);
        let issue3 = make_issue("acme", "widgets", 3, IssueState::Open, ReadyState::Blocked);

        let issues_vec = vec![issue1, issue2, issue3];
        let edges = vec![edge(&q1, &q2), edge(&q2, &q3), edge(&q3, &q1)];
        let graph = DependencyGraph::build(&issues_vec, &edges);
        let computed_ready = ready_set(&graph, &issues_vec);
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
        let mut issue1 = make_issue("acme", "widgets", 1, IssueState::Open, ReadyState::Ready);
        issue1.status = Status::InProgress;
        issue1.agent = Some("agent:supervisor".to_string());
        issue1.claimed_at = Some(Utc::now() - Duration::hours(48));

        let issues_vec = vec![issue1];
        let graph = DependencyGraph::build(&issues_vec, &[]);
        let computed_ready = ready_set(&graph, &issues_vec);
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
        let mut issue1 = make_issue("acme", "widgets", 1, IssueState::Open, ReadyState::Ready);
        issue1.agent = Some("bad-format-no-colon".to_string());

        let issues_vec = vec![issue1];
        let graph = DependencyGraph::build(&issues_vec, &[]);
        let computed_ready = ready_set(&graph, &issues_vec);
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

        let issue1 = make_issue("acme", "widgets", 1, IssueState::Open, ReadyState::Blocked);
        // #999 is in the graph (as a node created by the edge) but NOT in the issues map.
        // We need #999 to exist as a node in the graph for the edge to be added.
        let issue999 = make_issue("acme", "widgets", 999, IssueState::Open, ReadyState::Ready);

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
}
