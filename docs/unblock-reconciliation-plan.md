# Unblock — Reconciliation System
## Implementation Plan

| | |
|---|---|
| **Feature** | `reconcile` tool + drift detection |
| **Epic** | 1.6 — Reconciliation |
| **Target version** | v0.1.0 (Phase 1) |
| **Crates affected** | `unblock-core`, `unblock-mcp` |
| **Effort estimate** | ~2 focused days |
| **Author** | Miguel Ramos |
| **Date** | March 2026 |

---

## 1. Why This Is Necessary — The Problem

### 1.1 The Current Model Assumes a Single Actor

`://unblock` was designed around a solid principle: **"GitHub stores, Rust computes"**. The MCP server is the sole actor that writes workflow fields (Status, Agent, Ready State, Priority). The dependency graph is always recomputed from GitHub — a single source of truth.

This model is correct and must be preserved.

The problem is that **GitHub is an open system**. A human can — and will — interact with issues directly outside the MCP:

```
Human closes issue #42 via GitHub UI
  → no cascade fires
  → issues that depended on #42 still have Ready State = "blocked"
  → agent calls `ready` → does not see #43 or #44 as available
  → work is lost, context tokens wasted
```

Other examples of external mutations that create drift:

| External action | Effect on the system |
|---|---|
| Close a blocker issue via UI | Cascade never fires. Downstream issues remain artificially blocked |
| Remove blocking relationship via UI | Edge disappears from the graph. ReadyState field not updated |
| Change Status field directly on the board | MCP sees status inconsistent with what it last wrote |
| Delete a Projects V2 field | MCP fails on next write operation |
| Manually create a dep that introduces a cycle | Graph becomes invalid, `ready` returns incorrect results |
| Reopen an issue without going through the MCP | Ready State field is never recalculated |

### 1.2 The TTL Cache Does Not Solve This

The 30s TTL ensures the in-memory cache is eventually replaced with fresh data from GitHub. This solves **staleness** (outdated data), but does not solve **semantic drift** (system invariants violated).

After a `ready` call with an expired cache:
- The data is fresh ✅
- But if a cascade did not fire, the Ready State fields in GitHub are wrong
- `ready` reads the graph, not the fields — but the **GitHub board** shows incorrect data to the human
- Other agents that read the field directly (Beads compatibility, future features) see inconsistency

### 1.3 The Inconsistency Is Silent

The system does not know it is inconsistent. There is no alarm, no log warning, no DriftReport. The agent acts on data that appears correct but reflects a state of the world that no longer exists.

**This is the real risk.** It is not data corruption — GitHub has everything correct. It is the semantic layer (Ready State fields, cascade expectations) that has diverged from reality.

---

## 2. Why NOT SQLite — Alternatives Eliminated

Before detailing the solution, it is important to document what was rejected and why.

### SQLite as a local cache

```
GitHub → SQLite → in-memory → MCP tools
```

**Problems:**
1. **Three sources of truth.** GitHub, SQLite, and in-memory. Synchronising three layers is exponentially more complex.
2. **Schema migrations.** Every change to the domain model (`Issue`, `Status`, etc.) requires a migration. For a single-binary CLI tool, this is unnecessary complexity.
3. **Freshness paradox.** To know if SQLite is fresh, you need to make a GitHub API call — which costs the same as rebuilding the graph directly.
4. **Recreates Beads.** The very problem `://unblock` was built to solve (Beads had its own storage, drift from the source of truth) would be reintroduced into our own codebase.
5. **Violates principle P1.** "GitHub stores, Rust computes" — SQLite would mean "GitHub stores, SQLite also stores, Rust computes over SQLite (maybe)".

### Persistent daemon

Previously rejected: changes the deployment model from "binary, use it" to "install daemon + manage lifecycle". Not worth it for this problem.

### GitHub Webhooks → MCP server

Valid as a future upgrade, but requires a public HTTP endpoint, receiving infrastructure, and secrets management. Too much for v1.

---

## 3. The Right Solution — Reconciliation

### 3.1 Guiding Principle

**Embrace the drift, do not prevent it.** The user can and should be able to edit GitHub directly — that is the whole point of choosing GitHub as storage. The system accepts this and provides a tool that detects the divergence and repairs it — always using GitHub as the absolute source of truth.

```
GitHub (truth, immutable to us)
    ↕  always fresh
MCP server (computes + writes back)
    ↕  after every reconcile
Ready State fields (materialised view, stored IN GitHub)
```

The Ready State field already exists and is already written by the MCP after every operation. It is literally a materialised view persisted in GitHub. What is missing is:
1. A mechanism to detect when that view has diverged from reality
2. An operation to repair it

### 3.2 What Reconciliation Does

```
reconcile:
  1. Fetch all open + recently closed issues (always fresh, ignores cache)
  2. Rebuild the full dependency graph from scratch
  3. Compute the correct ready set
  4. Compute pending cascades (issues closed without cascade)
  5. Diff: computed ready set vs Ready State fields stored in GitHub
  6. Diff: blocking edges in graph vs body text of issues
  7. Validate: no cycles, required fields exist, agent field format is correct
  8. Repair (if --fix): batch update divergent fields, add audit comments
  9. Report: DriftReport with everything detected and repaired
```

---

## 4. Drift Types — Complete Taxonomy

```rust
// crates/unblock-core/src/reconcile.rs

/// Category of detected divergence.
///
/// 7 drift types covering all realistic external mutation scenarios.
/// Note: `GhostedBlockingEdge` was evaluated and rejected — in our model,
/// edges come from GitHub's `trackedByIssues` API (not body text), so the
/// graph cannot contain an edge that GitHub doesn't have.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DriftKind {
    /// Ready State field in GitHub diverges from what the graph computes.
    /// Covers: external close without cascade, external reopen, manual
    /// removal of blocking relationship via UI.
    StaleReadyState {
        issue: QualifiedId,
        field_says: ReadyState,
        graph_says: ReadyState,
    },

    /// Issue closed via UI — downstream issues should have received a cascade.
    /// No cascade comment was added. Ready State was not updated.
    UncascadedClosure {
        closed_issue: QualifiedId,
        should_have_unblocked: Vec<QualifiedId>,
    },

    /// Blocking edge references an issue that does not exist or is inaccessible.
    /// Cause: issue was deleted (admin action), or references a cross-repo issue
    /// the token cannot access.
    OrphanedBlockingEdge {
        source: QualifiedId,
        missing_target: QualifiedId,
    },

    /// Agent field has invalid format (must be `username:supervisor`).
    MalformedAgentField {
        issue: QualifiedId,
        raw_value: String,
    },

    /// Required Projects V2 field is missing.
    /// Cause: field deleted in GitHub Projects settings.
    MissingProjectField {
        field_name: String,
    },

    /// Cycle detected in the graph. Likely introduced by manual editing.
    CycleDetected {
        cycle: Vec<QualifiedId>,
    },

    /// Issue in `in_progress` state with `claimed_at` more than N hours ago without update.
    StaleClaim {
        issue: QualifiedId,
        claimed_at: DateTime<Utc>,
        hours_stale: u64,
    },
}

/// Full report from a reconciliation run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriftReport {
    pub repo: String,
    pub reconciled_at: DateTime<Utc>,
    pub issues_scanned: usize,
    pub edges_scanned: usize,
    pub drift_found: Vec<DriftKind>,
    pub repaired: Vec<DriftKind>,   // subset of drift_found, only if --fix
    pub errors: Vec<String>,        // drift that was detected but could not be repaired
    pub clean: bool,                // true if drift_found.is_empty()
}
```

---

## 5. Implementation — Step by Step

### 5.1 File Structure

```
crates/
├── unblock-core/
│   └── src/
│       ├── reconcile.rs     ← NEW: DriftKind, DriftReport, ReconcileEngine
│       └── lib.rs           ← add: pub mod reconcile;
│
└── unblock-mcp/
    └── src/
        └── tools/
            └── reconcile.rs ← NEW: tool handler, params, MCP response
```

### 5.2 `unblock-core/src/reconcile.rs`

The reconciliation engine is **pure** — no I/O, no GitHub calls. It receives the already-fetched graph and issues, and returns the DriftReport. Fully testable with in-memory data.

**Note on API alignment:** The signatures below reference methods that may need to be added
to `DependencyGraph` (`all_edges() -> Vec<BlockingEdge>`, `edge_count() -> usize`). Both are
trivial wrappers over petgraph's `edge_references()` and `edge_count()`.

The `compute_ready_set()` method currently returns `Vec<IssueSummary>`. The engine converts
this to a `HashSet<QualifiedId>` for O(1) lookups. The `compute_unblock_cascade()` method takes
`(closed_id: &QualifiedId, &[Issue])` — the second argument is currently unused but reserved.

```rust
// crates/unblock-core/src/reconcile.rs

use crate::{graph::DependencyGraph, types::*};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub struct ReconcileEngine {
    stale_claim_threshold_hours: u64,  // default: 24
}

impl ReconcileEngine {
    pub fn new(stale_claim_threshold_hours: u64) -> Self {
        Self { stale_claim_threshold_hours }
    }

    /// Produces a DriftReport by comparing the computed graph against stored fields.
    /// No I/O. Receives everything it needs as arguments.
    ///
    /// `computed_ready_set` is derived from `DependencyGraph::compute_ready_set()`
    /// by collecting qualified IDs into a HashSet.
    pub fn analyse(
        &self,
        graph: &DependencyGraph,
        issues: &HashMap<QualifiedId, Issue>,
        computed_ready_set: &HashSet<QualifiedId>,
        now: DateTime<Utc>,
    ) -> DriftReport {
        let mut drift = Vec::new();

        // 1. Stale Ready State fields
        // Compares all 4 ReadyState variants (Ready, Blocked, NotReady, Closed)
        for (qid, issue) in issues {
            if issue.state == IssueState::Closed {
                // Closed issues should have ReadyState::Closed
                if issue.ready_state != ReadyState::Closed {
                    drift.push(DriftKind::StaleReadyState {
                        issue: qid.clone(),
                        field_says: issue.ready_state.clone(),
                        graph_says: ReadyState::Closed,
                    });
                }
                continue;
            }

            let graph_ready = computed_ready_set.contains(qid);
            let expected = if graph_ready { ReadyState::Ready } else { ReadyState::Blocked };

            if issue.ready_state != expected {
                drift.push(DriftKind::StaleReadyState {
                    issue: qid.clone(),
                    field_says: issue.ready_state.clone(),
                    graph_says: expected,
                });
            }
        }

        // 2. Uncascaded closures
        // Closed issues whose downstream are still marked as blocked
        let issues_vec: Vec<Issue> = issues.values().cloned().collect();
        for (qid, issue) in issues {
            if issue.state == IssueState::Closed {
                let should_have_unblocked: Vec<QualifiedId> = graph
                    .compute_unblock_cascade(qid, &issues_vec)
                    .into_iter()
                    .filter(|id| {
                        issues.get(id)
                            .map(|i| i.ready_state != ReadyState::Ready && i.state == IssueState::Open)
                            .unwrap_or(false)
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
        for edge in graph.all_edges() {
            if !issues.contains_key(&edge.target) {
                drift.push(DriftKind::OrphanedBlockingEdge {
                    source: edge.source.clone(),
                    missing_target: edge.target.clone(),
                });
            }
        }

        // 4. Cycles
        for cycle in graph.detect_all_cycles() {
            drift.push(DriftKind::CycleDetected { cycle });
        }

        // 5. Stale claims
        for (qid, issue) in issues {
            if issue.status == Status::InProgress {
                if let Some(claimed_at) = issue.claimed_at {
                    let hours = (now - claimed_at).num_hours() as u64;
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

        // 6. Malformed agent fields
        for (qid, issue) in issues {
            if let Some(ref agent) = issue.agent {
                if !agent.contains(':') && !agent.is_empty() {
                    drift.push(DriftKind::MalformedAgentField {
                        issue: qid.clone(),
                        raw_value: agent.clone(),
                    });
                }
            }
        }

        DriftReport {
            repo: String::new(), // filled by the tool handler
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
```

### 5.3 `unblock-mcp/src/tools/reconcile.rs`

The tool handler has I/O: fetches from GitHub, calls the engine, and if `--fix` performs repairs.

```rust
// crates/unblock-mcp/src/tools/reconcile.rs

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use unblock_core::reconcile::{DriftKind, ReconcileEngine};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReconcileParams {
    /// If true, automatically repairs detected drift.
    /// If false (default), reports only without making any changes.
    #[serde(default)]
    pub fix: bool,

    /// Hours without update before considering a claim stale. Default: 24.
    #[serde(default = "default_stale_hours")]
    pub stale_claim_hours: u64,
}

fn default_stale_hours() -> u64 { 24 }

/// Detects and optionally repairs divergences between the computed graph
/// and the state stored in GitHub Projects V2 fields.
///
/// **API note:** Uses `fetch_graph_data()` (not a hypothetical `fetch_all_issues_with_edges`).
/// Ready State repairs use the existing `update_field()` path via `ProjectFieldIds`,
/// which requires the issue's project item ID. The handler must resolve this from the
/// fetched data or re-fetch per issue.
///
/// **Cache note:** `reconcile` always does a fresh fetch, bypassing the cache entirely.
/// After analysis (and optional repair), it populates the cache with the fresh graph.
/// It does NOT call `cache.invalidate()` — see ARCH §7.2 invalidation matrix.
pub async fn handle_reconcile(
    params: ReconcileParams,
    state: &AppState,
) -> Result<ReconcileOutput, McpError> {
    // 1. Always fresh fetch — bypasses cache entirely
    let (issues_vec, edges) = state.client
        .fetch_graph_data()
        .await
        .map_err(github_error_to_mcp)?;

    let issues: HashMap<QualifiedId, Issue> = issues_vec
        .into_iter()
        .map(|i| (i.qualified_id.clone(), i))
        .collect();

    // 2. Build graph and compute ready set
    let graph = DependencyGraph::build(
        &issues.values().cloned().collect::<Vec<_>>(),
        &edges,
    );
    let ready_summaries = graph.compute_ready_set(
        &issues.values().cloned().collect::<Vec<_>>(),
    );
    let computed_ready: HashSet<QualifiedId> = ready_summaries
        .iter()
        .map(|s| s.qualified_id.clone())
        .collect();

    // 3. Analyse drift
    let engine = ReconcileEngine::new(params.stale_claim_hours);
    let mut report = engine.analyse(&graph, &issues, &computed_ready, Utc::now());
    report.repo = format!("{}/{}", state.client.owner(), state.client.repo());

    // 4. Repair if --fix
    if params.fix {
        for drift in &report.drift_found {
            match drift {
                DriftKind::StaleReadyState { issue, graph_says, .. } => {
                    // Repair uses the existing field update path via ProjectFieldIds.
                    // Requires resolving the issue's project item ID.
                    match repair_ready_state(state, issue, graph_says).await {
                        Ok(_) => report.repaired.push(drift.clone()),
                        Err(e) => report.errors.push(format!(
                            "Failed to repair Ready State for {issue}: {e}"
                        )),
                    }
                }

                DriftKind::UncascadedClosure { closed_issue, should_have_unblocked } => {
                    for unblocked in should_have_unblocked {
                        match repair_ready_state(state, unblocked, &ReadyState::Ready).await {
                            Ok(_) => report.repaired.push(drift.clone()),
                            Err(e) => report.errors.push(format!(
                                "Cascade repair failed for {unblocked} \
                                 (was blocked by {closed_issue}): {e}"
                            )),
                        }
                    }
                    // Add audit comment on the closed issue
                    let _ = state.client.add_comment(
                        closed_issue,
                        format_cascade_repair_comment(should_have_unblocked),
                    ).await;
                }

                DriftKind::StaleClaim { issue, hours_stale, .. } => {
                    // Stale claims are not auto-repaired — agent or human decides.
                    tracing::warn!(issue, hours_stale, "Stale claim detected, not auto-repaired");
                }

                DriftKind::CycleDetected { .. } => {
                    // Cycles are not auto-repairable — require human decision.
                    report.errors.push(format!(
                        "Cycle detected — manual resolution required: {drift:?}"
                    ));
                }

                // Orphaned edges, malformed fields: log only, no auto-repair
                _ => {
                    tracing::warn!(?drift, "Drift detected, not auto-repaired");
                }
            }
        }
    }

    // 5. Update cache with the fresh graph we already have
    state.cache.update(graph);

    Ok(ReconcileOutput { report })
}

fn format_cascade_repair_comment(unblocked: &[QualifiedId]) -> String {
    let list = unblocked.iter()
        .map(|qid| format!("- {qid}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "🔧 **Unblock reconciliation** — cascade repair\n\n\
         This issue was closed outside the MCP server. \
         The following issues were unblocked retroactively:\n{list}"
    )
}
```

### 5.4 Integration into `prime`

`prime` is the entry point for every agent session. It is the right place for a **silent, non-blocking** reconciliation:

```rust
// crates/unblock-mcp/src/tools/prime.rs

pub async fn handle_prime(params: PrimeParams, state: &AppState) -> Result<PrimeOutput, McpError> {
    // Read-only reconciliation in background — does not block prime
    let drift_check = tokio::spawn({
        let state = state.clone();
        async move {
            let params = ReconcileParams { fix: false, stale_claim_hours: 24 };
            handle_reconcile(params, &state).await
        }
    });

    // Prime continues normally...
    let mut context = build_prime_context(state).await?;

    // Await the drift check and include warnings in output if relevant drift exists
    if let Ok(Ok(reconcile_out)) = drift_check.await {
        if !reconcile_out.report.clean {
            context.drift_warnings = Some(summarise_drift(&reconcile_out.report));
        }
    }

    Ok(context)
}
```

---

## 6. GitHub Actions Sentinel — Automatic Repair

For the case where the MCP server is not running (no active session) and a human makes mutations in GitHub, a GitHub Action acts as a sentinel:

```yaml
# .github/workflows/unblock-reconcile.yml
name: Unblock Reconcile

on:
  issues:
    types: [closed, reopened, edited, deleted]
  issue_comment:
    types: [created]

jobs:
  reconcile:
    runs-on: ubuntu-latest
    # Only runs if it was not the unblock-bot itself that triggered the event
    if: github.actor != 'github-actions[bot]'
    steps:
      - uses: actions/checkout@v4
      - name: Install unblock
        run: curl -fsSL https://get.unblock.dev | sh
      - name: Reconcile and fix drift
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          unblock reconcile --fix --output json | tee reconcile-report.json
          cat reconcile-report.json | jq '.drift_found | length' | \
            xargs -I{} echo "Drift items found: {}"
```

**Effect:** whenever a human closes an issue via the UI, the Action runs in ~30s, detects the pending cascade, and repairs the Ready State fields automatically. Zero manual intervention required.

---

## 7. Tool Output — What the Agent Sees

```jsonc
// reconcile (no drift)
{
  "clean": true,
  "issues_scanned": 47,
  "edges_scanned": 23,
  "reconciled_at": "2026-03-31T14:22:01Z",
  "message": "No drift detected. Graph is consistent."
}

// reconcile --fix (drift found and repaired)
{
  "clean": false,
  "issues_scanned": 47,
  "edges_scanned": 23,
  "drift_found": [
    {
      "type": "UncascadedClosure",
      "closed_issue": 42,
      "should_have_unblocked": [43, 51]
    },
    {
      "type": "StaleReadyState",
      "issue": 38,
      "field_says": "not_ready",
      "graph_says": "ready"
    }
  ],
  "repaired": [
    { "type": "UncascadedClosure", "closed_issue": 42, "should_have_unblocked": [43, 51] },
    { "type": "StaleReadyState", "issue": 38, "field_says": "not_ready", "graph_says": "ready" }
  ],
  "errors": [],
  "message": "2 drift items found and repaired."
}
```

---

## 8. Placement Within the Existing Plan

### Phase positioning

**Promoted to Phase 1 (Epic 1.6).** Reconciliation is a data integrity requirement for the
core workflow loop. External mutations can break the system from day one — this cannot wait
for Phase 2.

| Phase | Task | Type | Rationale |
|---|---|---|---|
| **Phase 1 (1.6)** | `DriftKind` + `DriftReport` in `unblock-core` | Required | Pure types, no I/O, zero risk |
| **Phase 1 (1.6)** | `ReconcileEngine::analyse()` | Required | Pure, testable, foundation for everything else |
| **Phase 1 (1.6)** | `reconcile` tool (read-only, `fix: false`) | Required | Diagnosis without side-effects |
| **Phase 1 (1.6)** | `reconcile --fix` repair logic | Required | Completes the loop |
| **Phase 1 (1.6)** | Integration into `prime` (background spawn) | Required | Every session starts with validated state. Depends on `prime` (1.4.14) |
| **Phase 3** | GitHub Actions sentinel | Future | Additional infrastructure, does not block v1 |
| **Phase 3** | `doctor` incorporates reconcile | Future | `doctor` becomes the full health check |

### Effort breakdown

| Component | Estimated lines | Days |
|---|---|---|
| `unblock-core/src/reconcile.rs` | ~200 | 0.5 |
| `unblock-mcp/src/tools/reconcile.rs` | ~180 | 0.5 |
| Integration into `prime` | ~30 | 0.25 |
| Tests (unit + integration) | ~200 | 0.5 |
| **Total (Phase 1)** | **~610** | **~1.75 days** |
| GitHub Actions sentinel (Phase 3) | ~30 (YAML) | 0.25 |

---

## 9. Tests

### Unit tests — `unblock-core` (no I/O)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_stale_ready_state_after_external_close() {
        // Issue #1 was blocking #2. #1 was closed via UI.
        // Ready State for #2 still says "not_ready" but the graph says "ready".
        let mut issues = fixture_issues(); // returns HashMap<QualifiedId, Issue>
        let qid1 = qid("owner", "repo", 1);
        let qid2 = qid("owner", "repo", 2);
        issues.get_mut(&qid1).unwrap().state = IssueState::Closed;
        issues.get_mut(&qid2).unwrap().ready_state = ReadyState::NotReady; // stale

        let graph = DependencyGraph::build(&issues, &[edge(&qid1, &qid2)]);
        let ready = graph.compute_ready_set(); // contains qid2

        let engine = ReconcileEngine::new(24);
        let report = engine.analyse(&graph, &issues, &ready, Utc::now());

        assert!(!report.clean);
        assert!(report.drift_found.iter().any(|d| matches!(
            d,
            DriftKind::UncascadedClosure { .. }
        )));
    }

    #[test]
    fn clean_report_when_consistent() {
        let issues = fixture_consistent_issues();
        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set();

        let engine = ReconcileEngine::new(24);
        let report = engine.analyse(&graph, &issues, &ready, Utc::now());

        assert!(report.clean);
        assert!(report.drift_found.is_empty());
    }

    #[test]
    fn detects_cycle_introduced_externally() {
        // A → B → C → A introduced by manual editing
        let (qid1, qid2, qid3) = (qid("o", "r", 1), qid("o", "r", 2), qid("o", "r", 3));
        let issues = fixture_issues_for_cycle();
        let graph = DependencyGraph::build(&issues, &[
            edge(&qid1, &qid2), edge(&qid2, &qid3), edge(&qid3, &qid1),
        ]);
        let ready = graph.compute_ready_set();

        let engine = ReconcileEngine::new(24);
        let report = engine.analyse(&graph, &issues, &ready, Utc::now());

        assert!(report.drift_found.iter().any(|d| matches!(
            d, DriftKind::CycleDetected { .. }
        )));
    }
}
```

---

## 10. Design Decisions

| # | Decision | Rejected alternative | Reason |
|---|---|---|---|
| R1 | `ReconcileEngine` is pure (no I/O) | Engine with integrated GitHub client | Testability. I/O stays in the tool handler |
| R2 | Stale claims: report, do not auto-repair | Auto-reopen or auto-close | Agent or human decision, not the system's |
| R3 | Cycles: report as error, do not auto-repair | Remove an edge arbitrarily | Cycles require a semantic decision |
| R4 | Cascade repair adds an audit comment | Repair silently | Traceability. Human sees what was repaired and why |
| R5 | `prime` runs reconcile in background (non-blocking) | Synchronous reconcile in prime | P5: "agent is always one command away". Do not penalise latency |
| R6 | `fix: false` by default | `fix: true` by default | Principle of least surprise. Diagnose before acting |
| R7 | GitHub Actions sentinel uses native `GITHUB_TOKEN` | Dedicated PAT | Zero extra config. Token already exists on every repo |
| R8 | `GhostedBlockingEdge` removed from taxonomy | Keep as drift type | Blocking edges come from GitHub's `trackedByIssues` API, not body text. The graph cannot contain an edge GitHub doesn't have. The scenario is impossible in our architecture |
| R9 | 7 drift types (not 8) | Original 8-type taxonomy | Simpler, no overlaps, no impossible states. `UncascadedClosure` + `StaleReadyState` cover all edge-related semantic drift |
| R10 | Promoted to Phase 1 (Epic 1.6) | Phase 2 stretch | Data integrity is foundational. External mutations break the system from day one |

---

*This document complements `unblock-architecture-github.md` and `unblock-project-plan.md`. The implementation follows the quality gates defined in those documents.*
