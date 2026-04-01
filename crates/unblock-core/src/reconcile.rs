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

use crate::types::{QualifiedId, ReadyState};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{QualifiedId, ReadyState};
    use chrono::Utc;

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
}
