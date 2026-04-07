//! Reconcile tool — detects and repairs drift between the dependency graph and
//! GitHub state.
//!
//! Performs a fresh fetch from GitHub (bypasses cache entirely), rebuilds the
//! dependency graph, and runs the pure [`ReconcileEngine`] to detect divergence.
//! After analysis, the cache is updated with the fresh graph data.
//!
//! When `fix: true`, auto-repairs two drift types:
//! - **`StaleReadyState`** — updates the Ready State field to the correct value.
//! - **`UncascadedClosure`** — updates downstream Ready States + posts audit comment.
//!
//! Other drift types are logged or pushed to `report.errors` without repair.
//! See Design Decisions R2–R4 in the reconciliation plan.

use std::collections::{HashMap, HashSet};

use chrono::Utc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use unblock_core::graph::DependencyGraph;
use unblock_core::reconcile::{DriftKind, DriftReport, ReconcileEngine};
use unblock_core::types::{Issue, QualifiedId, ReadyState};
use unblock_github::projects::FieldValue;

use unblock_github::projects::{ProjectFieldIds, ProjectInfo};

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
/// 6. If `fix: true`, repair `StaleReadyState` and `UncascadedClosure` drift.
/// 7. Update cache with the fresh graph already fetched.
/// 8. Return `ReconcileOutput { report }`.
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

    // 4. fix: true repair path — repair StaleReadyState and UncascadedClosure.
    if params.fix {
        apply_repairs(&mut report, state, &issues).await;
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

/// Tri-state memoization for repair context resolution within a single
/// reconcile run.
///
/// Unlike a plain `Option`, this also remembers *failed* resolution attempts
/// so subsequent repairable drift items don't retry the failing API calls
/// and don't push duplicate error messages into `report.errors`.
enum RepairContext {
    /// No attempt has been made yet.
    Unresolved,
    /// A previous attempt failed. The error has already been pushed to
    /// `report.errors` exactly once; further attempts short-circuit.
    Failed,
    /// Successfully resolved; cached for reuse.
    Resolved(Box<(ProjectFieldIds, ProjectInfo)>),
}

/// Lazily resolve field IDs and project info for repair operations.
///
/// On first call (`Unresolved`), attempts resolution. On success the context
/// transitions to `Resolved` and subsequent calls return the cached values.
/// On failure, the error is pushed to `report.errors` exactly once, the
/// context transitions to `Failed`, and all further calls short-circuit to
/// `None` without re-running the API calls or pushing duplicate errors.
async fn resolve_repair_context<'a>(
    ctx: &'a mut RepairContext,
    state: &ServerState,
    errors: &mut Vec<String>,
) -> Option<&'a (ProjectFieldIds, ProjectInfo)> {
    match ctx {
        RepairContext::Resolved(_) | RepairContext::Failed => {}
        RepairContext::Unresolved => {
            let Some(field_ids) = state.client.field_ids().await else {
                errors
                    .push("Field IDs not available — run setup first. Skipping repair.".to_owned());
                *ctx = RepairContext::Failed;
                return None;
            };
            let Ok(project_info) = state.client.resolve_project_info().await else {
                errors.push("Failed to resolve project info — skipping repair.".to_owned());
                *ctx = RepairContext::Failed;
                return None;
            };
            *ctx = RepairContext::Resolved(Box::new((field_ids, project_info)));
        }
    }
    match ctx {
        RepairContext::Resolved(boxed) => Some(&**boxed),
        RepairContext::Failed | RepairContext::Unresolved => None,
    }
}

/// Apply repairs for detected drift items.
///
/// Iterates over `report.drift_found` and repairs `StaleReadyState` and
/// `UncascadedClosure` drift. Other drift types are logged or pushed to
/// `report.errors` without repair.
///
/// Resolves `field_ids` and `project_info` lazily on first repairable drift,
/// then caches for subsequent repairs — avoiding redundant API calls without
/// blocking non-repairable drift handling.
async fn apply_repairs(
    report: &mut DriftReport,
    state: &ServerState,
    issues: &HashMap<QualifiedId, Issue>,
) {
    // Lazily resolved on first repairable drift type encountered.
    let mut repair_context: RepairContext = RepairContext::Unresolved;

    // Take drift_found out of report to avoid cloning the entire Vec.
    // We drain rather than clone so large drift reports don't allocate a full copy.
    let drift_items = std::mem::take(&mut report.drift_found);
    for drift in &drift_items {
        match drift {
            DriftKind::StaleReadyState {
                issue, graph_says, ..
            } => {
                // Lazily resolve repair context on first repairable drift.
                let ctx =
                    resolve_repair_context(&mut repair_context, state, &mut report.errors).await;
                if let Some((field_ids, project_info)) = ctx {
                    match repair_ready_state(
                        state,
                        issue,
                        graph_says,
                        issues,
                        field_ids,
                        project_info,
                    )
                    .await
                    {
                        Ok(()) => report.repaired.push(drift.clone()),
                        Err(e) => report
                            .errors
                            .push(format!("Failed to repair Ready State for {issue}: {e}")),
                    }
                }
            }

            DriftKind::UncascadedClosure {
                closed_issue,
                should_have_unblocked,
            } => {
                // Lazily resolve repair context on first repairable drift.
                let ctx =
                    resolve_repair_context(&mut repair_context, state, &mut report.errors).await;
                if let Some((field_ids, project_info)) = ctx {
                    let mut repaired_qids: Vec<&QualifiedId> = Vec::new();
                    for unblocked_qid in should_have_unblocked {
                        match repair_ready_state(
                            state,
                            unblocked_qid,
                            &ReadyState::Ready,
                            issues,
                            field_ids,
                            project_info,
                        )
                        .await
                        {
                            Ok(()) => repaired_qids.push(unblocked_qid),
                            Err(e) => {
                                report.errors.push(format!(
                                    "Cascade repair failed for {unblocked_qid} \
                                     (was blocked by {closed_issue}): {e}"
                                ));
                            }
                        }
                    }
                    // Push to repaired once per UncascadedClosure (not per downstream issue)
                    // and post audit comment on the closed issue for traceability (R4).
                    // When partially successful, list only the issues that were repaired.
                    if !repaired_qids.is_empty() {
                        report.repaired.push(drift.clone());

                        let comment_qids: Vec<QualifiedId> =
                            repaired_qids.iter().map(|q| (*q).clone()).collect();
                        let comment_body = format_cascade_repair_comment(&comment_qids);
                        if let Err(e) = state
                            .client
                            .add_comment(closed_issue.number, comment_body)
                            .await
                        {
                            tracing::warn!(
                                closed_issue = %closed_issue,
                                error = %e,
                                "Failed to post cascade repair audit comment"
                            );
                        }
                    }
                }
            }

            DriftKind::StaleClaim {
                issue, hours_stale, ..
            } => {
                // Design Decision R2: stale claims are NOT auto-repaired.
                tracing::warn!(
                    issue = %issue,
                    hours_stale,
                    "Stale claim detected — not auto-repaired (agent/human decision)"
                );
            }

            DriftKind::CycleDetected { .. } => {
                // Design Decision R3: cycles require human decision.
                report.errors.push(format!(
                    "Cycle detected — manual resolution required: {drift:?}"
                ));
            }

            // Explicit arms for remaining drift types — no auto-repair, log warning only.
            // Enumerated explicitly (instead of wildcard) so the compiler catches new
            // DriftKind variants added in the future.
            DriftKind::OrphanedBlockingEdge { .. }
            | DriftKind::MalformedAgentField { .. }
            | DriftKind::MissingProjectField { .. } => {
                tracing::warn!(?drift, "Drift detected — not auto-repaired");
            }
        }
    }
    // Restore drift_found so the report still contains all detected drift.
    report.drift_found = drift_items;
}

/// Maps a [`ReadyState`] to the GitHub Projects V2 single-select option name.
///
/// The `ReadyState` field has exactly two options: `"Ready"` and `"Not Ready"`.
/// The Rust enum has 4 variants — `Ready` maps to `"Ready"`, everything else
/// maps to `"Not Ready"`.
fn ready_state_option_name(state: ReadyState) -> &'static str {
    match state {
        ReadyState::Ready => "Ready",
        ReadyState::Blocked | ReadyState::NotReady | ReadyState::Closed => "Not Ready",
    }
}

/// Repair a single issue's Ready State field in GitHub Projects V2.
///
/// Uses pre-resolved `field_ids` and `project_info` to avoid redundant API
/// calls when repairing multiple issues in a single reconciliation run.
/// Only the per-issue `get_project_item_id` call is made per invocation.
///
/// # Errors
///
/// Returns a human-readable error string if any step fails (item not found,
/// option not found, API error).
async fn repair_ready_state(
    state: &ServerState,
    qid: &QualifiedId,
    target_state: &ReadyState,
    issues: &HashMap<QualifiedId, Issue>,
    field_ids: &ProjectFieldIds,
    project_info: &ProjectInfo,
) -> Result<(), String> {
    // Look up the issue's node_id from the issues map.
    let issue = issues
        .get(qid)
        .ok_or_else(|| format!("Issue {qid} not found in fetched issues map"))?;

    // Resolve the project item ID for this issue.
    let item_id = state
        .client
        .get_project_item_id(&issue.node_id, &project_info.id)
        .await
        .map_err(|e| format!("Failed to get project item ID for {qid}: {e}"))?;

    // Map the ReadyState to an option name and look up its option ID.
    let option_name = ready_state_option_name(*target_state);
    let option_id = field_ids
        .ready_state
        .options
        .get(option_name)
        .ok_or_else(|| format!("ReadyState option '{option_name}' not found in field options"))?;

    // Update the field.
    state
        .client
        .update_field(
            &project_info.id,
            &item_id,
            &field_ids.ready_state.field_id,
            &FieldValue::SingleSelectOption(option_id.clone()),
        )
        .await
        .map_err(|e| format!("Failed to update ReadyState field for {qid}: {e}"))?;

    tracing::info!(
        issue = %qid,
        target_state = option_name,
        "Repaired Ready State field"
    );

    Ok(())
}

/// Format the audit comment posted on a closed issue after cascade repair.
///
/// Design Decision R4: cascade repair adds an audit comment for traceability.
fn format_cascade_repair_comment(unblocked: &[QualifiedId]) -> String {
    let list: String = unblocked
        .iter()
        .map(|qid| format!("- #{}", qid.number))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "\u{1f527} **Unblock reconciliation** — cascade repair\n\n\
         This issue was closed outside the MCP server. \
         The following issues were unblocked retroactively:\n{list}"
    )
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
            agent_kind: std::sync::OnceLock::new(),
            agent_client: std::sync::OnceLock::new(),
            connected_at: std::sync::OnceLock::new(),
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
        let json = r"{}";
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

    // ── Helper function unit tests ───────────────────────────────────

    #[test]
    fn ready_state_option_name_maps_correctly() {
        use super::ready_state_option_name;

        assert_eq!(ready_state_option_name(ReadyState::Ready), "Ready");
        assert_eq!(ready_state_option_name(ReadyState::Blocked), "Not Ready");
        assert_eq!(ready_state_option_name(ReadyState::NotReady), "Not Ready");
        assert_eq!(ready_state_option_name(ReadyState::Closed), "Not Ready");
    }

    #[test]
    fn format_cascade_repair_comment_single_issue() {
        use super::format_cascade_repair_comment;

        let unblocked = vec![qid(43)];
        let comment = format_cascade_repair_comment(&unblocked);

        assert!(
            comment.contains("cascade repair"),
            "comment should mention cascade repair"
        );
        assert!(
            comment.contains("#43"),
            "comment should reference issue #43"
        );
        assert!(
            comment.contains("closed outside the MCP server"),
            "comment should explain the cause"
        );
    }

    #[test]
    fn format_cascade_repair_comment_multiple_issues() {
        use super::format_cascade_repair_comment;

        let unblocked = vec![qid(43), qid(51)];
        let comment = format_cascade_repair_comment(&unblocked);

        assert!(comment.contains("- #43"), "should list issue #43");
        assert!(comment.contains("- #51"), "should list issue #51");
    }

    // ── fix: true integration tests ──────────────────────────────────
    // These exercise apply_repairs with a non-functional client (no GitHub
    // connection). Repairs that require API calls will fail and push errors,
    // while non-repairable drift types are handled without API calls.

    /// Integration test: cycle detected → pushed to report.errors, NOT repaired.
    #[tokio::test]
    async fn fix_mode_cycle_reported_as_error_not_repaired() {
        use unblock_core::reconcile::DriftKind;

        let state = test_state().await;

        let mut report = DriftReport {
            repo: "test-owner/test-repo".to_owned(),
            reconciled_at: Utc::now(),
            issues_scanned: 3,
            edges_scanned: 3,
            drift_found: vec![DriftKind::CycleDetected {
                cycle: vec![qid(1), qid(2), qid(3)],
            }],
            repaired: vec![],
            errors: vec![],
            clean: false,
        };

        super::apply_repairs(&mut report, &state, &HashMap::new()).await;

        assert!(report.repaired.is_empty(), "cycles should NOT be repaired");
        assert_eq!(
            report.errors.len(),
            1,
            "cycle should produce exactly one error"
        );
        assert!(
            report.errors[0].contains("manual resolution required"),
            "error message should indicate manual resolution: {}",
            report.errors[0]
        );
    }

    /// Integration test: stale claim → logged as warning, NOT in report.errors,
    /// NOT in report.repaired.
    #[tokio::test]
    async fn fix_mode_stale_claim_not_repaired_not_in_errors() {
        use unblock_core::reconcile::DriftKind;

        let state = test_state().await;

        let mut report = DriftReport {
            repo: "test-owner/test-repo".to_owned(),
            reconciled_at: Utc::now(),
            issues_scanned: 1,
            edges_scanned: 0,
            drift_found: vec![DriftKind::StaleClaim {
                issue: qid(1),
                claimed_at: Utc::now() - chrono::Duration::hours(48),
                hours_stale: 48,
            }],
            repaired: vec![],
            errors: vec![],
            clean: false,
        };

        super::apply_repairs(&mut report, &state, &HashMap::new()).await;

        assert!(
            report.repaired.is_empty(),
            "stale claims should NOT be repaired"
        );
        assert!(
            report.errors.is_empty(),
            "stale claims should NOT be pushed to errors (Design Decision R2)"
        );
    }

    /// Integration test: orphaned blocking edge → logged as warning, NOT in
    /// report.errors, NOT repaired.
    #[tokio::test]
    async fn fix_mode_orphaned_edge_not_repaired_not_in_errors() {
        use unblock_core::reconcile::DriftKind;

        let state = test_state().await;

        let mut report = DriftReport {
            repo: "test-owner/test-repo".to_owned(),
            reconciled_at: Utc::now(),
            issues_scanned: 1,
            edges_scanned: 1,
            drift_found: vec![DriftKind::OrphanedBlockingEdge {
                source: qid(1),
                missing_target: qid(999),
            }],
            repaired: vec![],
            errors: vec![],
            clean: false,
        };

        super::apply_repairs(&mut report, &state, &HashMap::new()).await;

        assert!(
            report.repaired.is_empty(),
            "orphaned edges should NOT be repaired"
        );
        assert!(
            report.errors.is_empty(),
            "orphaned edges should NOT be pushed to errors"
        );
    }

    /// Integration test: malformed agent field → logged as warning, NOT in
    /// report.errors, NOT repaired.
    #[tokio::test]
    async fn fix_mode_malformed_agent_not_repaired_not_in_errors() {
        use unblock_core::reconcile::DriftKind;

        let state = test_state().await;

        let mut report = DriftReport {
            repo: "test-owner/test-repo".to_owned(),
            reconciled_at: Utc::now(),
            issues_scanned: 1,
            edges_scanned: 0,
            drift_found: vec![DriftKind::MalformedAgentField {
                issue: qid(7),
                raw_value: "bad-no-colon".to_owned(),
            }],
            repaired: vec![],
            errors: vec![],
            clean: false,
        };

        super::apply_repairs(&mut report, &state, &HashMap::new()).await;

        assert!(
            report.repaired.is_empty(),
            "malformed agent fields should NOT be repaired"
        );
        assert!(
            report.errors.is_empty(),
            "malformed agent fields should NOT be pushed to errors"
        );
    }

    /// Integration test: `StaleReadyState` repair attempt → fails gracefully
    /// (no field IDs set up) and pushes to report.errors.
    #[tokio::test]
    async fn fix_mode_stale_ready_state_pushes_error_on_failure() {
        use unblock_core::reconcile::DriftKind;

        let state = test_state().await;

        // The issue exists in the issues map (required for repair_ready_state).
        let issue = test_issue(2, IssueState::Open, ReadyState::Blocked);
        let issues: HashMap<QualifiedId, _> = [(qid(2), issue)].into_iter().collect();

        let mut report = DriftReport {
            repo: "test-owner/test-repo".to_owned(),
            reconciled_at: Utc::now(),
            issues_scanned: 1,
            edges_scanned: 0,
            drift_found: vec![DriftKind::StaleReadyState {
                issue: qid(2),
                field_says: ReadyState::Blocked,
                graph_says: ReadyState::Ready,
            }],
            repaired: vec![],
            errors: vec![],
            clean: false,
        };

        super::apply_repairs(&mut report, &state, &issues).await;

        // Repair should fail (field IDs not set up) → error pushed.
        assert!(
            report.repaired.is_empty(),
            "repair should fail without field IDs configured"
        );
        assert!(
            !report.errors.is_empty(),
            "failed repair should push a descriptive error"
        );
        assert!(
            report.errors[0].contains("Field IDs not available"),
            "error message should indicate missing field IDs: {}",
            report.errors[0]
        );
    }

    /// Integration test: `UncascadedClosure` repair attempt → fails gracefully
    /// (no field IDs set up) and pushes to report.errors.
    #[tokio::test]
    async fn fix_mode_uncascaded_closure_pushes_error_on_failure() {
        use unblock_core::reconcile::DriftKind;

        let state = test_state().await;

        // The downstream issue exists but the closed issue does not need
        // to be in the map (only downstream issues need repair).
        let issue43 = test_issue(43, IssueState::Open, ReadyState::Blocked);
        let issues: HashMap<QualifiedId, _> = [(qid(43), issue43)].into_iter().collect();

        let mut report = DriftReport {
            repo: "test-owner/test-repo".to_owned(),
            reconciled_at: Utc::now(),
            issues_scanned: 2,
            edges_scanned: 1,
            drift_found: vec![DriftKind::UncascadedClosure {
                closed_issue: qid(10),
                should_have_unblocked: vec![qid(43)],
            }],
            repaired: vec![],
            errors: vec![],
            clean: false,
        };

        super::apply_repairs(&mut report, &state, &issues).await;

        // Repair should fail (field IDs not set up) → error pushed.
        assert!(
            report.repaired.is_empty(),
            "cascade repair should fail without field IDs configured"
        );
        assert!(
            !report.errors.is_empty(),
            "failed cascade repair should push a descriptive error"
        );
        assert!(
            report.errors[0].contains("Field IDs not available"),
            "error message should indicate missing field IDs: {}",
            report.errors[0]
        );
    }

    /// Integration test: mixed drift types in fix mode — each handled correctly.
    #[tokio::test]
    async fn fix_mode_mixed_drift_types_handled_correctly() {
        use unblock_core::reconcile::DriftKind;

        let state = test_state().await;

        let mut report = DriftReport {
            repo: "test-owner/test-repo".to_owned(),
            reconciled_at: Utc::now(),
            issues_scanned: 5,
            edges_scanned: 3,
            drift_found: vec![
                DriftKind::CycleDetected {
                    cycle: vec![qid(1), qid(2)],
                },
                DriftKind::StaleClaim {
                    issue: qid(3),
                    claimed_at: Utc::now() - chrono::Duration::hours(48),
                    hours_stale: 48,
                },
                DriftKind::OrphanedBlockingEdge {
                    source: qid(4),
                    missing_target: qid(999),
                },
            ],
            repaired: vec![],
            errors: vec![],
            clean: false,
        };

        super::apply_repairs(&mut report, &state, &HashMap::new()).await;

        // CycleDetected → error (1 item).
        assert_eq!(report.errors.len(), 1, "only cycle should produce an error");
        assert!(report.errors[0].contains("manual resolution required"));

        // StaleClaim → neither repaired nor error.
        assert!(report.repaired.is_empty());

        // OrphanedBlockingEdge → neither repaired nor error.
        // (already asserted via errors.len() == 1)
    }

    /// Regression test for bead unblock-b6b.124: when repair context
    /// resolution fails, subsequent repairable drift items must NOT push
    /// duplicate error messages into `report.errors`. Exactly one error
    /// should appear per failed context resolution per reconcile run.
    #[tokio::test]
    async fn fix_mode_failed_context_resolution_is_deduped() {
        use unblock_core::reconcile::DriftKind;

        let state = test_state().await;

        // Two repairable drift items that both require repair context.
        let first = test_issue(2, IssueState::Open, ReadyState::Blocked);
        let second = test_issue(3, IssueState::Open, ReadyState::Blocked);
        let downstream = test_issue(43, IssueState::Open, ReadyState::Blocked);
        let issues: HashMap<QualifiedId, _> =
            [(qid(2), first), (qid(3), second), (qid(43), downstream)]
                .into_iter()
                .collect();

        let mut report = DriftReport {
            repo: "test-owner/test-repo".to_owned(),
            reconciled_at: Utc::now(),
            issues_scanned: 3,
            edges_scanned: 1,
            drift_found: vec![
                DriftKind::StaleReadyState {
                    issue: qid(2),
                    field_says: ReadyState::Blocked,
                    graph_says: ReadyState::Ready,
                },
                DriftKind::StaleReadyState {
                    issue: qid(3),
                    field_says: ReadyState::Blocked,
                    graph_says: ReadyState::Ready,
                },
                DriftKind::UncascadedClosure {
                    closed_issue: qid(10),
                    should_have_unblocked: vec![qid(43)],
                },
            ],
            repaired: vec![],
            errors: vec![],
            clean: false,
        };

        super::apply_repairs(&mut report, &state, &issues).await;

        // All three repair attempts must be skipped.
        assert!(
            report.repaired.is_empty(),
            "no repairs should succeed without field IDs"
        );
        // Exactly ONE error — not three — despite three repairable drift items.
        assert_eq!(
            report.errors.len(),
            1,
            "failed context resolution must be deduped to a single error, got: {:?}",
            report.errors
        );
        assert!(
            report.errors[0].contains("Field IDs not available"),
            "error should be the field-ids failure message: {}",
            report.errors[0]
        );
    }
}
