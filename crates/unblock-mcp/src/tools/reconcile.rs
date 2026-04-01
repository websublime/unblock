//! Reconcile tool — detects drift between the dependency graph and GitHub state.
//!
//! Performs a fresh fetch from GitHub (bypasses cache entirely), rebuilds the
//! dependency graph, and runs the pure [`ReconcileEngine`] to detect divergence.
//! After analysis, the cache is updated with the fresh graph data.
//!
//! This is a read-only tool by default (`fix: false`). The `fix: true` repair
//! path is implemented by task 1.6.4.

use std::collections::{HashMap, HashSet};

use chrono::Utc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use unblock_core::graph::DependencyGraph;
use unblock_core::reconcile::{DriftReport, ReconcileEngine};
use unblock_core::types::QualifiedId;

use crate::errors::github_error_to_mcp;
use crate::server::ServerState;

/// Default number of hours before a claim is considered stale.
fn default_stale_hours() -> u64 {
    24
}

/// Input parameters for the `reconcile` MCP tool.
///
/// Controls whether the tool operates in read-only (diagnostic) mode or
/// performs automatic repairs.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReconcileParams {
    /// If `true`, automatically repairs detected drift (`StaleReadyState`,
    /// `UncascadedClosure`). If `false` (default), reports only without making
    /// any changes. Design Decision R6: diagnose before acting.
    #[serde(default)]
    pub fix: bool,

    /// Hours without update before considering a claim stale. Default: 24.
    #[serde(default = "default_stale_hours")]
    pub stale_claim_hours: u64,
}

/// Output from the `reconcile` MCP tool.
///
/// Wraps the [`DriftReport`] from the reconciliation engine with an optional
/// human-readable message for the clean case.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ReconcileOutput {
    /// The full drift report from the reconciliation engine.
    pub report: ReconcileReport,
}

/// Schema-annotated drift report for MCP tool output.
///
/// Re-declared from [`DriftReport`] with `JsonSchema` derive, since core types
/// do not depend on `schemars`. Includes an optional `message` field for the
/// clean case.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ReconcileReport {
    /// Repository in `owner/repo` format.
    pub repo: String,
    /// Timestamp of the reconciliation run (ISO 8601 / RFC 3339).
    pub reconciled_at: String,
    /// Number of issues scanned during reconciliation.
    pub issues_scanned: usize,
    /// Number of edges scanned during reconciliation.
    pub edges_scanned: usize,
    /// `true` if no drift was detected — graph is consistent with GitHub state.
    pub clean: bool,
    /// All drift detected during reconciliation.
    pub drift_found: Vec<serde_json::Value>,
    /// Subset of drift that was successfully repaired (only when `fix: true`).
    pub repaired: Vec<serde_json::Value>,
    /// Drift that was detected but could not be repaired, with descriptive messages.
    pub errors: Vec<String>,
    /// Human-readable summary message. Present when `clean` is `true`.
    pub message: Option<String>,
}

impl ReconcileReport {
    /// Convert from a core [`DriftReport`] to a schema-annotated MCP result type.
    fn from_core(report: &DriftReport) -> Self {
        let message = if report.clean {
            Some("No drift detected. Graph is consistent.".to_owned())
        } else {
            None
        };

        // Serialize drift items as JSON values for schema flexibility.
        let drift_found: Vec<serde_json::Value> = report
            .drift_found
            .iter()
            .filter_map(|d| match serde_json::to_value(d) {
                Ok(v) => Some(v),
                Err(e) => {
                    warn!(?d, error = %e, "Failed to serialize drift item — dropping from report");
                    None
                }
            })
            .collect();

        let repaired: Vec<serde_json::Value> = report
            .repaired
            .iter()
            .filter_map(|d| match serde_json::to_value(d) {
                Ok(v) => Some(v),
                Err(e) => {
                    warn!(?d, error = %e, "Failed to serialize repaired item — dropping from report");
                    None
                }
            })
            .collect();

        Self {
            repo: report.repo.clone(),
            reconciled_at: report.reconciled_at.to_rfc3339(),
            issues_scanned: report.issues_scanned,
            edges_scanned: report.edges_scanned,
            clean: report.clean,
            drift_found,
            repaired,
            errors: report.errors.clone(),
            message,
        }
    }
}

/// Execute the reconcile tool handler.
///
/// # Flow
///
/// 1. Fresh fetch via `fetch_graph_data()` — bypasses cache entirely.
/// 2. Rebuild `DependencyGraph` from scratch.
/// 3. Compute ready set.
/// 4. Call `ReconcileEngine::analyse()` with all 4 args.
/// 5. Set `report.repo` from `state.client.owner()/repo()`.
/// 6. Update cache with the fresh graph already fetched.
/// 7. Return `ReconcileOutput { report }`.
///
/// The `fix: true` repair path is not implemented in this task (see task 1.6.4).
///
/// # Errors
///
/// Returns [`rmcp::model::ErrorData`] if the GitHub fetch fails.
pub async fn handle_reconcile(
    params: &ReconcileParams,
    state: &ServerState,
) -> Result<ReconcileOutput, rmcp::model::ErrorData> {
    info!(
        fix = params.fix,
        stale_claim_hours = params.stale_claim_hours,
        "Reconcile tool invoked"
    );

    // 1. Always fresh fetch — bypasses cache entirely.
    let (issues_vec, edges) = state
        .client
        .fetch_graph_data()
        .await
        .map_err(github_error_to_mcp)?;

    // Collect issues into both a Vec (for DependencyGraph::build) and a
    // HashMap<QualifiedId, Issue> (for ReconcileEngine::analyse).
    let issues: HashMap<QualifiedId, _> = issues_vec
        .iter()
        .map(|i| (i.qualified_id.clone(), i.clone()))
        .collect();

    // 2. Build graph and compute ready set.
    let graph = DependencyGraph::build(&issues_vec, &edges);
    let ready_summaries = graph.compute_ready_set(&issues_vec);
    let computed_ready: HashSet<QualifiedId> = ready_summaries
        .iter()
        .map(|s| s.qualified_id.clone())
        .collect();

    // 3. Analyse drift.
    let engine = ReconcileEngine::new(params.stale_claim_hours);
    let mut report = engine.analyse(&graph, &issues, &computed_ready, Utc::now());
    report.repo = format!("{}/{}", state.client.owner(), state.client.repo());

    // 4. fix: true repair path — deferred to task 1.6.4.
    // TODO(unblock-egf.4): implement fix=true repair logic for StaleReadyState
    //   and UncascadedClosure drift types. Until then, fix requests fall through
    //   to read-only mode with a warning surfaced to the MCP consumer.
    if params.fix {
        tracing::warn!(
            "reconcile --fix is not yet implemented (see task unblock-egf.4). \
             Running in read-only mode."
        );
        report.errors.push(
            "fix=true was requested but is not yet implemented. \
             Running in read-only mode. See task unblock-egf.4."
                .to_owned(),
        );
    }

    // 5. Update cache with the fresh graph we already have.
    //    Uses real 2-arg signature: cache.update(ready_summaries, graph).
    state.cache.update(ready_summaries, graph).await;
    tracing::debug!("Cache updated with fresh graph from reconcile");

    // 6. Return output.
    Ok(ReconcileOutput {
        report: ReconcileReport::from_core(&report),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use chrono::Utc;
    use unblock_core::cache::GraphCache;
    use unblock_core::config::Config;
    use unblock_core::graph::DependencyGraph;
    use unblock_core::types::{
        Issue, IssueState, IssueType, Priority, QualifiedId, ReadyState, Status,
    };

    use super::*;
    use crate::server::ServerState;

    // ── Test helpers ───────────────────────────────────────────────────

    /// Helper to create a `QualifiedId` for tests.
    fn qid(number: u64) -> QualifiedId {
        QualifiedId::new("test-owner", "test-repo", number)
    }

    /// Build a minimal `Issue` for testing.
    fn test_issue(number: u64, state: IssueState, ready_state: ReadyState) -> Issue {
        Issue {
            qualified_id: qid(number),
            number,
            node_id: format!("NODE_{number}"),
            title: format!("Issue #{number}"),
            issue_type: Some(IssueType::Task),
            status: Status::Open,
            priority: Priority::P1,
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
            url: format!("https://github.com/test-owner/test-repo/issues/{number}"),
            comments: vec![],
            blocked_by: vec![],
            blocking: vec![],
            parent: None,
            sub_issues: vec![],
        }
    }

    /// Create a `ServerState` with a real cache but a client that points
    /// at a non-existent host.
    async fn test_state() -> ServerState {
        let config = Config::load_from(|key| match key {
            "GITHUB_TOKEN" => Ok("ghp_test_token_for_unit_tests".to_owned()),
            "UNBLOCK_REPO" => Ok("test-owner/test-repo".to_owned()),
            _ => Err(std::env::VarError::NotPresent),
        })
        .expect("test config should load");

        let client = unblock_github::client::GitHubClient::new(&config)
            .await
            .expect("test client should initialize");

        ServerState {
            config: Arc::new(config),
            client: Arc::new(client),
            cache: Arc::new(GraphCache::new(Duration::from_secs(300))),
        }
    }

    // ── ReconcileReport conversion tests ──────────────────────────────

    #[test]
    fn clean_report_produces_message() {
        let report = DriftReport {
            repo: "test-owner/test-repo".to_owned(),
            reconciled_at: Utc::now(),
            issues_scanned: 10,
            edges_scanned: 5,
            drift_found: vec![],
            repaired: vec![],
            errors: vec![],
            clean: true,
        };

        let output = ReconcileReport::from_core(&report);
        assert!(output.clean);
        assert_eq!(
            output.message.as_deref(),
            Some("No drift detected. Graph is consistent.")
        );
        assert!(output.drift_found.is_empty());
    }

    #[test]
    fn dirty_report_has_no_message() {
        use unblock_core::reconcile::DriftKind;

        let report = DriftReport {
            repo: "test-owner/test-repo".to_owned(),
            reconciled_at: Utc::now(),
            issues_scanned: 10,
            edges_scanned: 5,
            drift_found: vec![DriftKind::StaleReadyState {
                issue: qid(1),
                field_says: ReadyState::Blocked,
                graph_says: ReadyState::Ready,
            }],
            repaired: vec![],
            errors: vec![],
            clean: false,
        };

        let output = ReconcileReport::from_core(&report);
        assert!(!output.clean);
        assert!(output.message.is_none());
        assert_eq!(output.drift_found.len(), 1);
    }

    // ── Cache update integration test ─────────────────────────────────

    #[tokio::test]
    async fn cache_updated_after_reconcile_analysis() {
        // Simulate the reconcile flow: build graph, compute ready set,
        // update cache — verify cache contains the data.
        let state = test_state().await;
        assert!(
            !state.cache.is_fresh().await,
            "cache should be empty initially"
        );

        // Build test data.
        let issues = vec![
            test_issue(1, IssueState::Open, ReadyState::Ready),
            test_issue(2, IssueState::Open, ReadyState::Ready),
        ];
        let graph = DependencyGraph::build(&issues, &[]);
        let ready_set = graph.compute_ready_set(&issues);

        // Update cache (same call the handler makes).
        state.cache.update(ready_set, graph).await;

        assert!(
            state.cache.is_fresh().await,
            "cache should be fresh after update"
        );
        let cached_ready = state.cache.get_ready_set().await;
        assert!(cached_ready.is_some());
        assert_eq!(cached_ready.unwrap().len(), 2);
    }

    #[test]
    fn reconcile_params_defaults() {
        // Verify serde defaults work correctly.
        let json = r#"{}"#;
        let params: ReconcileParams = serde_json::from_str(json).expect("should deserialize");
        assert!(!params.fix);
        assert_eq!(params.stale_claim_hours, 24);
    }

    #[test]
    fn reconcile_params_explicit_values() {
        let json = r#"{"fix": true, "stale_claim_hours": 48}"#;
        let params: ReconcileParams = serde_json::from_str(json).expect("should deserialize");
        assert!(params.fix);
        assert_eq!(params.stale_claim_hours, 48);
    }

    #[test]
    fn reconcile_output_serializes_clean() {
        let output = ReconcileOutput {
            report: ReconcileReport {
                repo: "acme/widgets".to_owned(),
                reconciled_at: "2026-03-31T12:00:00Z".to_owned(),
                issues_scanned: 47,
                edges_scanned: 23,
                clean: true,
                drift_found: vec![],
                repaired: vec![],
                errors: vec![],
                message: Some("No drift detected. Graph is consistent.".to_owned()),
            },
        };

        let json = serde_json::to_value(&output).expect("should serialize");
        assert_eq!(json["report"]["clean"], true);
        assert_eq!(json["report"]["issues_scanned"], 47);
        assert_eq!(json["report"]["edges_scanned"], 23);
        assert_eq!(
            json["report"]["message"],
            "No drift detected. Graph is consistent."
        );
    }

    #[test]
    fn reconcile_output_serializes_with_drift() {
        use unblock_core::reconcile::DriftKind;

        let drift = DriftKind::StaleReadyState {
            issue: qid(42),
            field_says: ReadyState::Blocked,
            graph_says: ReadyState::Ready,
        };
        let drift_json = serde_json::to_value(&drift).expect("drift should serialize");

        let output = ReconcileOutput {
            report: ReconcileReport {
                repo: "acme/widgets".to_owned(),
                reconciled_at: "2026-03-31T12:00:00Z".to_owned(),
                issues_scanned: 47,
                edges_scanned: 23,
                clean: false,
                drift_found: vec![drift_json],
                repaired: vec![],
                errors: vec![],
                message: None,
            },
        };

        let json = serde_json::to_value(&output).expect("should serialize");
        assert_eq!(json["report"]["clean"], false);
        assert!(json["report"]["message"].is_null());
        assert_eq!(json["report"]["drift_found"].as_array().unwrap().len(), 1);
    }

    // ── Integration tests (full pipeline) ────────────────────────────

    // These tests exercise the complete reconcile pipeline end-to-end:
    //   build issues → build graph → compute ready set → analyse → from_core → ReconcileOutput
    // They bypass the network (no fetch_graph_data) but exercise every in-process
    // step that handle_reconcile() performs after the fetch.

    /// Integration test: drift present → report shows drift, no mutations.
    ///
    /// Constructs a scenario where issue #2 is blocked by issue #1, but issue
    /// #1 is closed (simulating an external closure without cascade). The
    /// engine should detect `StaleReadyState` drift. No mutations are made
    /// because `fix` is false.
    #[test]
    fn integration_drift_present_reports_drift_no_mutations() {
        use std::collections::HashSet;

        use unblock_core::reconcile::ReconcileEngine;
        use unblock_core::types::BlockingEdge;

        // Issue #1: closed (simulates external closure via GitHub UI).
        let issue_one = test_issue(1, IssueState::Closed, ReadyState::Ready);
        // Issue #2: open, blocked by #1, but still marked Blocked (stale ready state).
        let issue_two = test_issue(2, IssueState::Open, ReadyState::Blocked);

        let issues_vec = vec![issue_one, issue_two];
        let edges = vec![BlockingEdge {
            source: qid(2),
            target: qid(1),
        }];

        // Build HashMap (same as handle_reconcile does after fetch).
        let issues_map: HashMap<QualifiedId, _> = issues_vec
            .iter()
            .map(|i| (i.qualified_id.clone(), i.clone()))
            .collect();

        // Build graph and compute ready set (steps 2-3 of handler).
        let graph = DependencyGraph::build(&issues_vec, &edges);
        let ready_summaries = graph.compute_ready_set(&issues_vec);
        let computed_ready: HashSet<QualifiedId> = ready_summaries
            .iter()
            .map(|s| s.qualified_id.clone())
            .collect();

        // Analyse drift (step 4 of handler).
        let engine = ReconcileEngine::new(24);
        let mut report = engine.analyse(&graph, &issues_map, &computed_ready, Utc::now());
        report.repo = "test-owner/test-repo".to_owned();

        // Convert through MCP layer (step 6 of handler).
        let output = ReconcileOutput {
            report: ReconcileReport::from_core(&report),
        };

        // Assertions: drift detected, no mutations.
        assert!(
            !output.report.clean,
            "report should NOT be clean when drift exists"
        );
        assert!(
            !output.report.drift_found.is_empty(),
            "drift_found should contain at least one item"
        );
        assert!(
            output.report.repaired.is_empty(),
            "repaired should be empty — fix is false, no mutations"
        );
        assert!(
            output.report.errors.is_empty(),
            "errors should be empty in read-only mode"
        );
        assert!(
            output.report.message.is_none(),
            "message should be None when drift is present"
        );

        // Verify the drift is specifically a StaleReadyState for issue #2.
        // DriftKind uses serde's default externally-tagged representation,
        // so the variant name is a top-level key: {"StaleReadyState": {...}}.
        let drift_json = &output.report.drift_found[0];
        assert!(
            drift_json.get("StaleReadyState").is_some(),
            "drift item should be a StaleReadyState variant, got: {drift_json}"
        );
    }

    /// Integration test: clean repo → clean: true with message.
    ///
    /// Constructs a scenario with two independent open issues, both correctly
    /// marked as `Ready`. No blocking edges exist. The engine should find no
    /// drift and the output should have `clean: true` with the standard message.
    #[test]
    fn integration_clean_repo_reports_clean() {
        use std::collections::HashSet;

        use unblock_core::reconcile::ReconcileEngine;

        // Two open issues, no dependencies, both correctly Ready.
        let issue_one = test_issue(1, IssueState::Open, ReadyState::Ready);
        let issue_two = test_issue(2, IssueState::Open, ReadyState::Ready);

        let issues_vec = vec![issue_one, issue_two];

        // Build HashMap (same as handle_reconcile does after fetch).
        let issues_map: HashMap<QualifiedId, _> = issues_vec
            .iter()
            .map(|i| (i.qualified_id.clone(), i.clone()))
            .collect();

        // Build graph with no edges and compute ready set.
        let graph = DependencyGraph::build(&issues_vec, &[]);
        let ready_summaries = graph.compute_ready_set(&issues_vec);
        let computed_ready: HashSet<QualifiedId> = ready_summaries
            .iter()
            .map(|s| s.qualified_id.clone())
            .collect();

        // Analyse drift — should find nothing.
        let engine = ReconcileEngine::new(24);
        let mut report = engine.analyse(&graph, &issues_map, &computed_ready, Utc::now());
        report.repo = "test-owner/test-repo".to_owned();

        // Convert through MCP layer.
        let output = ReconcileOutput {
            report: ReconcileReport::from_core(&report),
        };

        // Assertions: clean, with message.
        assert!(
            output.report.clean,
            "report should be clean when no drift exists"
        );
        assert!(
            output.report.drift_found.is_empty(),
            "drift_found should be empty"
        );
        assert!(
            output.report.repaired.is_empty(),
            "repaired should be empty"
        );
        assert!(output.report.errors.is_empty(), "errors should be empty");
        assert_eq!(
            output.report.message.as_deref(),
            Some("No drift detected. Graph is consistent."),
            "clean report should include the standard message"
        );
        assert_eq!(output.report.repo, "test-owner/test-repo");
        assert_eq!(output.report.issues_scanned, 2);
    }
}
