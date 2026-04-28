# Plan 02 — MCP Complete (v0.2.0)

> Phase: 02
> Status: APPROVED
> Author: Ada (architect)
> Date: 2026-04-28
> Crates (modified): `unblock-core`, `unblock-github`, `unblock-mcp`
> Crates (new): `unblock-resilience` (extracted in Epic 02.A — see §6.2)
> Depends on: Phase 01 (MCP Foundation) — complete per bd
> Required by: Phase 03 (Code Indexer) — Epic 03.2 consumes `unblock-resilience` directly (no `unblock-github` dep)
> Source: [MANIFESTO](../MANIFESTO.md) · [PRD §7 Phase 02](../PRD.md#phase-02--mcp-complete-v020--02-plan-mcp-completemd) · [SPEC §13–14](../SPEC.md)
> Bd source of truth: bd is authoritative for execution status. PRD §7, PRD §6, SPEC §13.3, SPEC §14, SPEC §12.2, CLAUDE.md "Coding Standards", and Phase 03 spec §20.1 have been patched in this iteration to align with the locked decisions — see §13.

---

## Table of Contents

0. [Implementation History & Audit](#0-implementation-history--audit)
1. [Purpose](#1-purpose)
2. [Scope](#2-scope)
3. [Out of Scope (Non-Goals)](#3-out-of-scope-non-goals)
4. [Locked Architectural Decisions](#4-locked-architectural-decisions)
5. [Crate Architecture & Module Layout](#5-crate-architecture--module-layout)
6. [Public API Surface for Phase 03](#6-public-api-surface-for-phase-03)
7. [External Dependencies & APIs](#7-external-dependencies--apis)
8. [Research Gaps (Smith input)](#8-research-gaps-smith-input)
9. [Epic Breakdown](#9-epic-breakdown)
10. [Task Dependencies](#10-task-dependencies)
11. [Acceptance Criteria](#11-acceptance-criteria)
12. [Risks & Mitigations](#12-risks--mitigations)
13. [PRD / SPEC Patches Required](#13-prd--spec-patches-required)
14. [Definition of Done](#14-definition-of-done)

---

## 0. Implementation History & Audit

Phase 02 partially landed during Phase 01 execution. The pre-plan audit (Fase A, executed by the orchestrator before this plan was authored) produced the following ground truth.

### 0.1 Already implemented (drop from Phase 02 scope)

| Feature | Location | Notes |
|---|---|---|
| `reconcile` MCP tool + `ReconcileEngine` | `crates/unblock-mcp/src/tools/reconcile.rs`, `crates/unblock-core/src/reconcile.rs` | 6 of 7 drift types: `UncascadedClosure`, `OrphanedBlockingEdge`, `MalformedAgentField`, `MissingProjectField`, `CycleDetected`, `StaleClaim`. Missing: `StaleStatus` — see Epic 02.E |
| Agent client detection | `crates/unblock-core/src/{client.rs, detection.rs}`, integrated in `tools/prime.rs` and `server.rs` | `AgentKind`, `AgentClient`, `ClientDetector`, `SessionMeta` already present |

### 0.2 Stubs present, no behaviour wired

| Feature | Location | Status |
|---|---|---|
| `Error::CircuitBreakerOpen { since }` | `crates/unblock-github/src/errors.rs` | Variant + `status_code() == 503` exist; never constructed by any code path |
| `Error::RateLimited { reset_at }` | `crates/unblock-github/src/errors.rs` | Variant + `is_retryable()` helper exist; never used in a retry loop |

These stubs are forward-compatible with the chosen libraries (Decision L1, see §4) and will be **wired** rather than redesigned.

### 0.3 Still to implement (Phase 02 scope)

- New crate `unblock-resilience` — neutral home for the breaker + retry layer (consumed by `unblock-github` and, in Phase 03, by `unblock-indexer`).
- Circuit breaker logic — `failsafe` integration inside `unblock-resilience`, wired into `unblock-github::client`.
- Retry-with-backoff logic — `backoff` integration, retry-inside-breaker order.
- In-memory `ServerMetrics` infrastructure (replaces deferred OTel).
- `commit_context` MCP tool (file does not exist).
- `doctor` MCP tool (file does not exist).
- `StaleStatus` drift type — extend `ReconcileEngine`.
- `#[non_exhaustive]` applied to `DriftKind` (and audited on other growth-prone public enums) — see §2.6.

### 0.4 bd state

Clean slate — no Phase 02 beads tracked yet. `/tasks` (Fernando) creates the bead graph after this plan reaches APPROVED and after Smith resolves the research gaps in §8.

---

## 1. Purpose

Phase 02 takes the Phase 01 MCP server from **functional** to **production-resilient**. Three orthogonal hardening surfaces are added:

1. **Resilience.** GitHub API calls survive transient failures (network, 429, 503) without surfacing them to the agent, and degrade gracefully when GitHub is sustained-down.
2. **Operational observability.** The server can answer "am I healthy and why" without an external OTel collector. In-memory metrics + a self-diagnostic `doctor` tool replace the OpenTelemetry surface that the original PRD §7.2 scoped here. **OpenTelemetry is deferred to Phase 06** (Decision 2.0).
3. **Drift completeness + audit trail.** `reconcile` covers all 7 drift types declared in the SPEC. A new `commit_context` tool emits structured git trailers so every commit becomes a queryable audit trail.

**Outcome:** `v0.2.0` — the MCP binary handles GitHub outages, exposes its own health, completes drift detection, and provides a rich commit-message convention that downstream phases (Plugin §7.5, LLM Agent Phase 07) can rely on.

**Phase positioning.** Phase 02 sits between the Phase 01 minimum-viable loop and the Phase 03 code indexer. Phase 03 Epic 03.2 (grammar fetcher) is HTTP-bound and **reuses the resilience layer built here** — see §6 for the public API contract Phase 03 will consume.

**Governing constraints:**

- **No simplifications without user approval.** The 26 locked decisions in §4 were negotiated through extended user iteration. Re-litigation requires explicit user sign-off.
- **Pre-production stance.** No users, no migrations, no deprecation shims. Breaking changes acceptable across all unblock crates (per `feedback_pre_production`). This permits:
  - Decision 4.2 — commit convention upgrade (subject-only → trailers).
  - Decision 5.1 — `DriftKind` enum extension (additive, but flagged for the `non_exhaustive` posture per `unblock-29p.70`).
  - Decision 2.0 — PRD §7.2 scope override (OTel out, in-memory metrics in).
- **bd is the source of truth** for execution status. PRD §7 prose will be patched (§13) where it diverges from bd reality.
- **Skill instructions are followed exactly** — no unilateral parameter additions to Task() dispatches downstream.

---

## 2. Scope

### 2.1 Resilience layer (new module)

A wrapper inside `unblock-github` that makes circuit-breaker + retry caller-transparent. `GitHubClient` continues to expose its current method surface; internally each HTTP call passes through the breaker and retry policy.

**Order of operations (Decision 1.4):** breaker **outside**, retry **inside**. The breaker counts only the **final** outcome (after retries exhaust). A successful retry → breaker records success.

**Libraries (Decision 1.1):** `failsafe` (circuit breaker) + `backoff` (exponential backoff with jitter). No roll-our-own.

**Configuration (Decision 1.5, 1.6):**

| Knob | Default | Env var | Notes |
|---|---|---|---|
| Max retry attempts | 5 | `UNBLOCK_RETRY_MAX_ATTEMPTS` | Hybrid limit — first cap |
| Total deadline | 30s | `UNBLOCK_RETRY_DEADLINE_SECS` | Hybrid limit — second cap; whichever hits first wins |
| Retry-After cap | 30s | (hard-coded) | If `Retry-After` > 30s on a 429, fail fast (do not sleep) |
| Breaker failure threshold | 5 consecutive | (Phase 02 default) | Matches SPEC §14.1 |
| Breaker cooldown | 10s | (Phase 02 default) | Matches SPEC §14.1 |

**State scope (Decision 1.3):** per-process singleton. GitHub rate limits are per-token, and the local MCP binary is single-process. Phase 06 (remote server) will revisit if multi-tenant token isolation is needed.

**Retryable errors (existing `is_retryable()` contract):** `RateLimited`, `GitHubUnavailable`, `GitHubServerError { status: 503 }`. No change.

### 2.2 Observability — in-memory `ServerMetrics`

A single `ServerMetrics` struct providing read-only snapshot output for the `doctor` tool. Not exported as OTel.

```rust
pub struct ServerMetrics {
    tool_calls: HashMap<&'static str, AtomicU64>,
    tool_durations: HashMap<&'static str, Histogram>,    // hdrhistogram
    api_calls: HashMap<&'static str, AtomicU64>,
    api_durations: HashMap<&'static str, Histogram>,
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
    cache_evictions: AtomicU64,
    cache_size: AtomicU64,
    graph_issues: AtomicU64,
    graph_edges: AtomicU64,
}
```

Captures everything the original PRD §7.2 listed for OTel:

- `unblock.tool.duration` → `tool_durations` (per-tool histogram)
- `unblock.github.request.duration` → `api_durations` (per-API histogram)
- `unblock.cache.hits` / `cache.misses` → atomic counters
- `unblock.graph.nodes` / `graph.edges` → atomic gauges
- `unblock.graph.recalculations` → derivable from `tool_calls` of write tools

**Forward-compat for Phase 06 (Decision 2.2).** The `ServerMetrics` struct does not change when Phase 06 adds OTel. The OTel layer is an **adapter** that reads the same atomic counters and histograms — wrap, do not replace. This contract is documented in the spec and pinned by a contract test in Phase 02 itself.

**Location (research gap RG-1).** Provisional location: `unblock-mcp::metrics`. If the struct is naturally pure (no MCP types) it migrates to `unblock-core::metrics`. Decision finalised in §11 acceptance after `cargo doc --no-deps` on the candidate.

### 2.3 `doctor` MCP tool (new)

Operational health orchestrator. Read-only by default; `--with-drift` opts into drift detection; `--fix` opts into self-repair.

**Boundaries (Decision 3.2):**

- `setup` — creates from scratch what should exist (labels, fields, milestones, views). Idempotent.
- `reconcile` — detects/repairs semantic drift in DATA (7 drift types).
- `doctor` — orchestrator. Checks operational health, **delegates** repairs to `setup`/`reconcile` when appropriate. Does NOT duplicate their checks.

**Self-repair semantics (Decision 3.3):**

| Invocation | Behaviour |
|---|---|
| `doctor` (no flag) | Read-only diagnostic |
| `doctor --with-drift` | Diagnostic + invoke `reconcile` (no fix) |
| `doctor --fix` | Attempt all auto-repairs |
| `doctor --fix --with-drift` | All of the above + `reconcile --fix` |

**Auto-repairable:** cache invalidation, missing labels (via `setup`), drift (via `reconcile --fix`), config reload.

**Requires human:** token expired/invalid, GitHub Project deleted, network/connectivity issues, invalid env vars.

**Output (Decision 3.4, 3.6):**

```json
{
  "overall_status": "HEALTHY | DEGRADED | UNHEALTHY",
  "checks": [
    { "name": "...", "category": "connectivity|config|state", "status": "OK|WARN|FAIL", "detail": "..." }
  ],
  "metrics_snapshot": { /* full ServerMetrics snapshot */ },
  "repairs_attempted": [ { "name": "...", "outcome": "ok|failed", "detail": "..." } ],
  "human_actions_needed": [ { "issue": "...", "remediation": "..." } ]
}
```

**`overall_status` mapping (Decision 3.7):**

- `UNHEALTHY` — any check FAIL in `connectivity` or `config` category.
- `DEGRADED` — any check WARN, OR drift detected without `--fix`.
- `HEALTHY` — otherwise.

**Triggers (Decision 3.5):** on-demand only. Watchdog auto-trigger deferred to Phase 06+.

### 2.4 `commit_context` MCP tool (new)

Generates a rich, structured commit message with git trailers from a bd issue. Generate-only (Decision 4.1) — validation/parsing of existing messages is deferred.

**Convention upgrade (Decision 4.2 — BREAKING CHANGE).** Pre-production stance permits the break. Old convention: bd-id-in-subject only. New convention: bd-id stays in subject AND becomes a `Bd-Issue:` trailer; additional trailers carry full provenance.

**Canonical trailer schema (Decision 4.3) — STABLE contract:**

| Trailer | When emitted | Source |
|---|---|---|
| `Bd-Issue: <bd-id>` | Always | Input arg |
| `Closes: <github-issue-url>` | When bd issue has linked GitHub issue | bd → GitHub mapping |
| `Refs: <bd-id-or-url>` | Optional, repeatable | Input arg |
| `Spec: <path>#<anchor>` | When bd's `design` field references a spec | bd issue field |
| `Plan: <path>` | Always | Derived from bd parent epic |
| `Phase: <NN>` | Always | Derived from bd issue's epic phase |

**Vocabulary is EXTENSIBLE, not closed (LOCKED — user-confirmed during plan iteration).** The 6 trailers above are the **STABLE contract** Phase 02 ships — semver-style guarantee within unblock. Future phases (especially Phase 07 LLM Agent) are FREE to add new trailer keys (e.g., `Reviewed-By`, `Investigation`, `Verdict`) without modifying the canonical set. Implementation requirements:

- The `commit_context` generator emits the 6 canonical trailers under their stable definitions.
- The trailer parser (used by `--with-changes` round-trip and any future validation tooling) **accepts unknown trailer keys** — does NOT reject them, MUST round-trip them unchanged. This preserves forward-compat for downstream agents that learn new keys.
- The Phase 02 spec (when authored) MUST specify parser behaviour for unknown keys in concrete terms (no silent drops, no normalisation).

**Subject line (Decision 4.4):** tool returns `subject_template`; agent free to modify. Mapping bd type → Conventional Commits prefix:

| bd type | CC prefix |
|---|---|
| `feature` | `feat` |
| `bug` | `fix` |
| `task` | `chore` (agent adjusts to `refactor`/`docs`/`test`/etc.) |
| `epic` | not directly committable — tool errors |

**Sources (Decision 4.5):** bd primary + GitHub URL resolution + git config + opt-in scope detection.

**Output (Decision 4.6):**

```json
{
  "subject_template": "feat(scope): ...",
  "body_template": "...",
  "trailers": [ { "key": "Bd-Issue", "value": "..." }, ... ],
  "formatted": "<full ready-to-paste commit message>",
  "warnings": [ "..." ]
}
```

**`--with-changes` flag (Decision 4.7):** opt-in. Inspects working-tree diff to detect scope, suggest `API:` line, suggest `BREAKING CHANGE:` footer. Reuses the API-tracking convention already documented in `CLAUDE.md`.

### 2.5 `reconcile` extension — `StaleStatus` drift type

Adds the 7th drift type and brings `ReconcileEngine` to PRD §7.2 completeness.

**Detection (Decision 5.2):**

```text
for each issue in graph:
    expected = compute_status(graph, issue)        // existing graph engine
    actual   = projects_v2.status_field(issue)
    if expected != actual:
        drift_found.push(StaleStatus { issue_id, expected, actual })
```

`compute_status()` already exists in the graph engine (Phase 01). No new computation logic — only field comparison + reporting.

**Severity (Decision 5.3):**

- **Phase 02:** WARN. Cosmetic — agents read the graph, not the field, for `ready`.
- **Phase 04 escalation:** FAIL. Phase 04 introduces the Materialised Fast Path which reads the Status field as cold-start cache; an inconsistent field then becomes correctness-critical. Escalation tracked in Phase 04 plan.

**Repair (Decision 5.4):** write computed value via the existing `update_project_field` GraphQL mutation. Idempotent.

**Test fixtures (Decision 5.5):**

| Fixture | Graph state | Field state | Expected drift |
|---|---|---|---|
| F1 | closed | in_progress | `StaleStatus` |
| F2 | blocked | ready | `StaleStatus` |
| F3 | ready | ready | none |
| F4 | (any) | None | `MissingProjectField` (NOT `StaleStatus`) |

### 2.6 `non_exhaustive` posture — locked project-wide policy

**Decision (LOCKED — user-confirmed during plan iteration):** any public enum that is expected to grow over time carries `#[non_exhaustive]`. This codifies the precedent already applied to `unblock-github::Error` and `unblock-core::DomainError` under `unblock-29p.70` and extends it to all future growth-prone public enums.

**Scope (non-exhaustive list of enums covered by this policy):**

- `unblock_core::reconcile::DriftKind` — applied in Epic 02.E (trigger: `StaleStatus` addition).
- `unblock_github::Error` — already applied (`unblock-29p.70`).
- `unblock_core::DomainError` — already applied (`unblock-29p.70`).
- Any future public enum representing status, kind, category, or variant-by-extension semantics (e.g., new drift types, new error variants, new check categories in `doctor`) — applied at introduction.

**Rationale:** forward-compat hardening. Future variant additions remain non-breaking even after v1.0 ships. This is non-negotiable for library crates (`unblock-core`, `unblock-github`, `unblock-resilience`, future `unblock-indexer-core`, etc.); the binary crate (`unblock-mcp`) is excluded from semver concerns but still benefits from the discipline.

**Enforcement:** Epic 02.E adds the attribute to `DriftKind`; Epic 02.F documents the policy in CLAUDE.md "Coding Standards" so it propagates to every future PR.

**RG-10 status:** this gap is no longer a research question — it is a locked decision. RG-10 is reframed in §8 to a downstream impact audit only ("does adding `non_exhaustive` to `DriftKind` break any current `match` site in the workspace?"). The answer-or-fix is part of Epic 02.E.

---

## 3. Out of Scope (Non-Goals)

Explicitly **not** in Phase 02:

| Item | Phase | Rationale |
|---|---|---|
| OpenTelemetry exporter | 06 | Decision 2.0 — deferred. In-memory metrics + `doctor` cover the operational need without the OTel toolchain |
| Remote MCP transport (HTTP) | 06 | Phase 06 introduces `unblock-mcp-remote` |
| HTTP server / axum / SharedGraphCache | 06 | Phase 06 |
| Webhook handler | 06 | Phase 06 |
| Plugin pipeline (skills, agents, hooks) | 05 | Phase 05 |
| Code indexer (tree-sitter, sqlx, FTS5) | 03 | Phase 03 (already specced) |
| Materialised Fast Path | 04 | Phase 04 — escalates `StaleStatus` severity |
| LLM agent (autonomous) | 07 | Phase 07 |
| Cross-platform binaries / cargo-dist | 04 | Phase 04 |
| GitHub App authentication | 04 | Phase 04 |
| GHE Server testing | 04 | Phase 04 |
| Watchdog auto-trigger for `doctor` | 06+ | Decision 3.5 |
| Validation/parsing of existing commit messages by `commit_context` | post-02 | Decision 4.1 — generate only |

---

## 4. Locked Architectural Decisions

These 26 decisions are **CONFIRMED** through prior user iteration. Re-litigation requires explicit user sign-off.

### Item 1 — Circuit breaker + retry exponential backoff (6)

| # | Decision | Locked Value |
|---|---|---|
| L1.1 | Library | `failsafe` (breaker) + `backoff` (retry). No roll-our-own |
| L1.2 | Location | New crate `unblock-resilience` (per §6.2). `GitHubClient` consumes it: holds `reqwest::Client` + `ResiliencePolicy`. Caller-transparent. Phase 03's `unblock-indexer` consumes the same crate directly — no transitive dep on `unblock-github`. |
| L1.3 | State scope | Per-process singleton |
| L1.4 | Order of operations | Breaker **outside**, retry **inside**. Breaker counts only final result |
| L1.5 | Max retries / time budget | Hybrid: max 5 attempts OR 30s deadline. Env-configurable |
| L1.6 | Retry-After header | Respect, capped at 30s. If header > 30s, fail fast |

### Item 2 — Observability (REVISED, Caminho A) (3)

| # | Decision | Locked Value |
|---|---|---|
| L2.0 | OTel scope | DEFERRED to Phase 06. PRD §7.2 must be patched |
| L2.1 | In-memory `ServerMetrics` | Atomic counters + hdrhistogram histograms. Read by `doctor` |
| L2.2 | Phase 06 forward-compat | OTel layer wraps the same struct; struct does not change |

### Item 3 — `doctor` MCP tool (7)

| # | Decision | Locked Value |
|---|---|---|
| L3.1 | Default scope | Connectivity + config + internal state. `--with-drift` opts in to `reconcile` |
| L3.2 | Boundaries | `setup` creates / `reconcile` repairs data drift / `doctor` orchestrates. No duplication |
| L3.3 | Self-repair | `doctor` read-only; `--fix` attempts auto-repairs (cache, labels, drift, config) |
| L3.4 | Output | Structured JSON: `overall_status`, `checks[]`, `metrics_snapshot`, `repairs_attempted[]`, `human_actions_needed[]` |
| L3.5 | Triggers | On-demand only. Watchdog deferred to Phase 06+ |
| L3.6 | Metrics encoding | Full snapshot by default |
| L3.7 | Status mapping | UNHEALTHY ⇐ connectivity/config FAIL; DEGRADED ⇐ WARN or drift-without-fix; HEALTHY otherwise |

### Item 4 — `commit_context` MCP tool (7)

| # | Decision | Locked Value |
|---|---|---|
| L4.1 | Function | Generate only. Validate/parse-existing deferred |
| L4.2 | Convention migration | BREAKING CHANGE — pre-prod permits. Subject-only → subject + trailers |
| L4.3 | Trailer schema | 6 canonical trailers (Bd-Issue, Closes, Refs, Spec, Plan, Phase) — STABLE contract; vocabulary EXTENSIBLE for future phases; parser MUST accept and round-trip unknown trailer keys |
| L4.4 | Subject line | Returns `subject_template`. bd type → CC prefix mapping |
| L4.5 | Sources | bd + GitHub URL resolution + git config + opt-in scope detection |
| L4.6 | Output format | JSON with subject_template, body_template, trailers[], formatted, warnings[] |
| L4.7 | `--with-changes` | Opt-in working-tree diff inspection for scope / API: / BREAKING CHANGE: |

### Item 5 — `reconcile` extension `StaleStatus` (5)

| # | Decision | Locked Value |
|---|---|---|
| L5.1 | Enum extension | Add `StaleStatus` variant — pre-prod permits |
| L5.2 | Detection | Reuse `compute_status()` from graph engine; field comparison only |
| L5.3 | Severity | Phase 02: WARN; Phase 04 escalates to FAIL |
| L5.4 | Repair | Existing `update_project_field` GraphQL mutation; idempotent |
| L5.5 | Test fixtures | 4 canonical fixtures (F1–F4 in §2.5) |

**Total: 26 locked decisions.**

---

## 5. Crate Architecture & Module Layout

One new crate (`unblock-resilience`, per Decision §6.2). Other modifications to existing crates only.

### 5.1 `unblock-resilience` (new crate)

```
crates/unblock-resilience/
├── Cargo.toml                 ← new crate (MIT)
└── src/
    ├── lib.rs                 ← public surface; re-exports of types in §6.3
    ├── breaker.rs             ← `Breaker` wrapping failsafe::Config
    ├── retry.rs               ← `RetryPolicy` wrapping backoff::ExponentialBackoff
    ├── policy.rs              ← `ResiliencePolicy` (breaker + retry composition)
    └── traits.rs              ← `IsRetryable` trait + `ResilienceError<E>`
```

**Key types (public surface, full contract pinned in §6.3):**

```rust
pub struct ResiliencePolicy { /* breaker + retry, composed */ }

impl ResiliencePolicy {
    pub fn from_env() -> Self;            // reads UNBLOCK_RETRY_*
    pub fn default() -> Self;             // 5 attempts / 30s / 5 fail / 10s cooldown
    pub fn with_breaker(...) -> Self;
    pub fn with_retry(...) -> Self;

    pub async fn execute<F, Fut, T, E>(
        &self,
        op: F,
    ) -> Result<T, ResilienceError<E>>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<T, E>>,
        E: IsRetryable;
}

pub struct BreakerSnapshot {
    pub state: BreakerState,           // Closed | Open { since } | HalfOpen
    pub failure_count: usize,
    pub last_failure_at: Option<Instant>,
}

pub struct RetrySnapshot {
    pub max_attempts: u32,
    pub deadline: Duration,
    pub current_backoff: Duration,
}

pub enum BreakerState { Closed, Open { since: Instant }, HalfOpen }

pub trait IsRetryable {
    fn is_retryable(&self) -> bool;
    fn retry_after(&self) -> Option<Duration> { None }
}
```

`unblock-resilience` has zero dependencies on other unblock crates. Both `unblock-github` (this phase) and `unblock-indexer` (Phase 03) consume it directly — see §6.2 for rationale.

### 5.2 `unblock-github`

```
crates/unblock-github/src/
├── lib.rs
├── client.rs                  ← MODIFIED: holds `ResiliencePolicy`; every HTTP call
│                                  passes through `policy.execute(...)`
├── errors.rs                  ← MODIFIED: `impl IsRetryable for Error` (wraps existing
│                                  `is_retryable()` helper); variants unchanged
├── graphql.rs
├── mutations.rs
└── projects.rs
```

`ResiliencePolicy::execute` is the **only** way `GitHubClient` performs HTTP work after this phase. Static audit (Epic 02.A) verifies zero direct `reqwest::Client` calls outside the resilience boundary.

### 5.3 `unblock-core` and/or `unblock-mcp`

Provisional placement of `ServerMetrics`:

```
crates/unblock-mcp/src/
├── metrics.rs                 ← NEW: ServerMetrics + Histogram type
├── tools/
│   ├── doctor.rs              ← NEW
│   ├── commit_context.rs      ← NEW
│   └── reconcile.rs           ← MODIFIED: StaleStatus drift type
└── ...

crates/unblock-core/src/
├── reconcile.rs               ← MODIFIED: DriftKind::StaleStatus variant
└── ...
```

**Decision deferred to spec phase (RG-1):** if `ServerMetrics` requires no rmcp / MCP types, it migrates to `unblock-core::metrics`.

### 5.4 Workspace constraints

Unchanged from Phase 01:

- Edition 2024, `#![deny(unsafe_code)]`.
- `snafu` exclusive — no `thiserror`, no `anyhow`.
- `///` docs on all `pub fn` / `pub struct`, `//!` on all modules.
- `tracing` JSON to stderr (stdio reserved for MCP protocol).

---

## 6. Public API Surface for Phase 03

> Phase 03 (Code Indexer) Epic 03.2 reuses Phase 02's resilience layer for the WASM grammar fetcher. Phase 03 spec §20.1 marked the surface as UNRESOLVED pending this plan. This section pins the contract.

### 6.1 What Phase 03 needs

A non-GitHub HTTP fetcher (the grammar fetcher in `unblock-indexer`) needs:

1. Exponential backoff with jitter on transient HTTP failures.
2. Circuit breaker on sustained failure of the `objects.githubusercontent.com` (release asset) endpoint.
3. Same env-var configuration story as the GitHub client.
4. Same observability hooks (`api_calls`, `api_durations` in `ServerMetrics`).

### 6.2 Reuse mechanism — LOCKED: extracted `unblock-resilience` crate

**Decision (LOCKED — user-confirmed during plan iteration):** the resilience layer is extracted to a new neutral crate `unblock-resilience` as part of Epic 02.A. Phase 03's `unblock-indexer` consumes it directly without a transitive dependency on `unblock-github`.

**Rationale.** `unblock-github` (issue domain) and `unblock-indexer` (code domain) are architecturally orthogonal. Forcing `unblock-indexer` to depend on `unblock-github` to obtain a generic HTTP resilience policy would couple two otherwise independent product surfaces and pollute the dep graph. The user explicitly rejected the "start in-place, extract later" path: extraction happens in Phase 02 implementation time, not deferred.

**Crate placement.**

```
crates/unblock-resilience/
├── Cargo.toml                  ← new crate (MIT, no_std-compatible target if feasible)
└── src/
    ├── lib.rs                  ← public API (§6.3)
    ├── breaker.rs              ← failsafe wrapper
    ├── retry.rs                ← backoff wrapper
    ├── policy.rs               ← ResiliencePolicy (composition)
    └── traits.rs               ← IsRetryable
```

**Dependency graph after Phase 02.**

```
unblock-resilience  ──── (no deps on other unblock crates) ────┐
                                                                │
unblock-github  ─── depends on ───▶ unblock-resilience          │
                                                                │
unblock-indexer (Phase 03) ─── depends on ───▶ unblock-resilience
                                                                │
                            (NO dep from unblock-indexer ──▶ unblock-github)
```

**License.** `unblock-resilience` ships under MIT, matching `unblock-core` / `unblock-github`. It is part of the open-source foundation.

**Phase 03 spec §20.1.** This decision **resolves** the previously UNRESOLVED `unblock-resilience` reuse question carried in Phase 03 §20.1. The Phase 03 plan/spec must be updated to reflect the resolved direct-crate-dependency model — Epic 02.F task added (§13.5).

**API stability.** The public surface in §6.3 is the contract Phase 03 codes against; Epic 02.A acceptance includes a smoke-test that imports `ResiliencePolicy` from a Phase 03 prototype harness before Phase 03 begins.

### 6.3 Pinned API for Phase 03 consumption

The following types live in `unblock-resilience` and are **public** consumed directly by `unblock-indexer`:

```rust
// crates/unblock-resilience/src/lib.rs

pub struct ResiliencePolicy { /* ... */ }
pub struct BreakerSnapshot   { /* ... */ }
pub struct RetrySnapshot     { /* ... */ }
pub enum   BreakerState      { /* ... */ }

impl ResiliencePolicy {
    pub fn from_env() -> Self;
    pub fn default() -> Self;
    pub async fn execute<F, Fut, T, E>(&self, op: F) -> Result<T, ResilienceError<E>>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<T, E>>,
        E: IsRetryable;          // trait — see below

    pub fn breaker_snapshot(&self) -> BreakerSnapshot;
    pub fn retry_snapshot(&self) -> RetrySnapshot;
}

pub trait IsRetryable {
    fn is_retryable(&self) -> bool;
    fn retry_after(&self) -> Option<Duration> { None }
}
```

`unblock-github::Error::is_retryable()` already exists (Phase 01 audit); Epic 02.A wires it into a thin `impl IsRetryable for Error` in `unblock-github`. `unblock-indexer` (Phase 03) will implement `IsRetryable` on its own grammar-fetch error type. Neither crate depends on the other.

**Acceptance criterion (§11.5):** Phase 03 spec §20.1 is **closed** — no UNRESOLVED markers — once this plan reaches APPROVED.

---

## 7. External Dependencies & APIs

| Dependency | Use | Version target | Notes |
|---|---|---|---|
| `failsafe` | Circuit breaker | latest 1.x | Decision L1.1 |
| `backoff` | Exponential backoff | latest 0.4.x | Decision L1.1 |
| `hdrhistogram` | Latency histograms | latest 7.x | For `ServerMetrics::*_durations` |
| `git2` (or `gix`) | Git config / working-tree diff for `commit_context` | TBD by RG-3 | New dependency for Phase 02 |

External APIs touched:

- **GitHub GraphQL** — already used; gains breaker + retry transparently.
- **GitHub REST** — already used; gains breaker + retry transparently.
- **Git CLI / git config** — read-only for `commit_context` (`git config user.email`, `git diff --stat`).

No new GitHub permissions required.

---

## 8. Research Gaps (Smith input)

These are **not blockers for plan approval** but **must be validated before §11 spec authoring**. Smith is dispatched after this plan reaches APPROVED.

| # | Gap | Why it matters | Expected output |
|---|---|---|---|
| RG-1 | `ServerMetrics` placement — `unblock-core` (pure) vs `unblock-mcp` (binary) | Determines whether Phase 06 OTel adapter lives in core or in `unblock-mcp-remote` | Compile-test of both placements + recommendation |
| ~~RG-2~~ | ~~Resilience module reuse — Option A vs Option B~~ — **CLOSED** by §6.2 lock (Decision: extracted `unblock-resilience` crate). No Smith input required. | — | — |
| RG-3 | Git inspection library — `git2` (libgit2 binding) vs `gix` (pure Rust) | `git2` adds C dep; `gix` is younger but pure | Feature audit + Phase 04 cross-platform implications (Windows, musl, etc.) |
| RG-4 | `failsafe` crate fitness — feature set, async support, compatibility with `tokio` | Foundation of resilience layer; if unfit, Decision L1.1 reopens | Compile + unit-test prototype |
| RG-5 | `backoff` crate fitness — same as RG-4, plus `Retry-After` header support | Same | Compile + unit-test prototype |
| RG-6 | `hdrhistogram` overhead at MCP tool-call frequency | Atomic histogram updates on every tool call; must not regress p99 | Bench: 10k tool calls/s, measure overhead |
| RG-7 | bd → GitHub URL resolution path for `commit_context` `Closes:` trailer | Need a stable bd field or query that yields the linked GitHub issue URL | bd CLI / API path + fallback when no link exists |
| RG-8 | bd parent-epic → phase number derivation | `Phase: NN` trailer derivation (Decision L4.3) | Documented bd query path; fallback when epic missing |
| RG-9 | Test fixtures for `StaleStatus` — minimal Projects V2 mock that supports field read+write in `MockGitHubClient` | F1–F4 fixtures (Decision L5.5) require this | Mock extension PR or pattern reuse from existing `update_project_field` tests |
| ~~RG-10~~ | ~~`non_exhaustive` impact on `DriftKind`~~ — **CLOSED** by §2.6 lock (project-wide policy: growable public enums MUST carry `#[non_exhaustive]`). Downstream `match` audit is now part of Epic 02.E task work, not a research question. | — | — |

**Open research gaps after this iteration: 8 (RG-1, RG-3, RG-4, RG-5, RG-6, RG-7, RG-8, RG-9).**

**Plan invariant:** every open research gap above maps to at least one task in §9. Smith's findings either confirm the plan or surface contradictions that loop back to Ada for plan revision.

---

## 9. Epic Breakdown

Six epics (02.A–02.F). Each epic decomposes into beads during `/tasks` (Fernando) **after** research validates this plan. Bead descriptions reference this plan + the spec; they do not duplicate authoritative content (per `feedback_bead_description_not_spec`).

### Epic 02.A — Resilience Layer

**Owner:** rust-supervisor (Neo)
**Output:** Every GitHub HTTP call passes through breaker + retry; `ResiliencePolicy::execute` is the sole call site for HTTP work.

Tasks:

1. Add `failsafe` + `backoff` workspace deps; lock versions (depends on RG-4, RG-5).
2. Create new crate `crates/unblock-resilience/` (per §6.2): expose `Breaker`, `RetryPolicy`, `ResiliencePolicy`, `BreakerSnapshot`, `RetrySnapshot`, `BreakerState`, `IsRetryable`, `ResilienceError<E>`. Crate has zero deps on other unblock crates.
3. Add `unblock-github` → `unblock-resilience` dependency. Implement `IsRetryable` for `unblock_github::Error` (wraps existing `is_retryable()` helper) inside `unblock-github`.
4. Wrap every HTTP call in `GitHubClient` (graphql + REST) with `policy.execute(...)`. Caller signature unchanged. Static audit verifies zero `reqwest::Client` direct calls outside the policy boundary.
5. Wire env-var config (`UNBLOCK_RETRY_MAX_ATTEMPTS`, `UNBLOCK_RETRY_DEADLINE_SECS`) via `Config::load_from`.
6. Implement `Retry-After` header parsing with 30s cap; respect on 429 only.
7. Expose `BreakerSnapshot` / `RetrySnapshot` via `GitHubClient::resilience_snapshot()` for `doctor`.
8. Unit tests: breaker opens after 5 failures, cooldowns to half-open, success closes; retry honours `Retry-After`; deadline beats max-attempts when relevant.
9. Property test: composition is `breaker(retry(op))` — never `retry(breaker(op))`.
10. Integration test against `MockGitHubClient` simulating 429 / 503 storms.

### Epic 02.B — In-Memory `ServerMetrics`

**Owner:** rust-supervisor (Neo)
**Output:** `ServerMetrics` exists, is updated by every tool call + every API call + cache events + graph events, snapshot is consumable by `doctor`.

Tasks:

1. Decide placement — `unblock-core::metrics` vs `unblock-mcp::metrics` — based on RG-1.
2. Implement `ServerMetrics` per §2.2 (atomics + hdrhistogram).
3. Wire `tool_calls` + `tool_durations` recording into the MCP tool dispatch wrapper (single instrumentation point).
4. Wire `api_calls` + `api_durations` into `ResiliencePolicy::execute` (single instrumentation point).
5. Wire `cache_*` counters into `GraphCache` (already invalidates on every write).
6. Wire `graph_issues` + `graph_edges` gauges to be updated after every cache rebuild.
7. Snapshot API: `ServerMetrics::snapshot() -> MetricsSnapshot` (cloneable, serde-friendly).
8. Contract test: forward-compat — adding a new metric does not break the snapshot serialisation (Decision L2.2 invariant).
9. Bench: per-call overhead < 1µs (RG-6).

### Epic 02.C — `doctor` MCP Tool

**Owner:** rust-supervisor (Neo)
**Output:** Tool `doctor` is registered, returns the structured JSON of §2.3, supports `--fix` and `--with-drift`, delegates to `setup` and `reconcile`.

Tasks:

1. Schema definition (`schemars`) for input + output per Decision L3.4.
2. Check categories: `connectivity` (GitHub `/user` ping), `config` (env vars, project ID), `state` (cache stats, breaker state, drift count if `--with-drift`).
3. `--fix` path: cache invalidation, missing-label repair (delegate to `setup`), drift repair (delegate to `reconcile --fix`), config reload.
4. `overall_status` derivation per Decision L3.7.
5. Output construction includes full `metrics_snapshot` (Decision L3.6).
6. Idempotency: `doctor` (no flag) never mutates state; `--fix` is idempotent on re-run.
7. Integration tests against `MockGitHubClient` for HEALTHY / DEGRADED / UNHEALTHY paths.
8. Documentation update: `doctor` entry in `README.md` user-facing usage.

### Epic 02.D — `commit_context` MCP Tool

**Owner:** rust-supervisor (Neo)
**Output:** Tool `commit_context` is registered, returns the structured JSON of Decision L4.6, optionally inspects working-tree diff with `--with-changes`.

Tasks:

1. Choose git inspection library based on RG-3 (`git2` vs `gix`).
2. Schema definition for input (`bd_id`, `with_changes: bool`) + output.
3. Subject-template builder (Decision L4.4): bd-type → CC prefix mapping.
4. Trailer collector (Decision L4.3): each of the 6 canonical trailers, with empty-omission rules.
5. bd → GitHub URL resolution for `Closes:` (RG-7).
6. bd → epic → phase derivation for `Phase: NN` (RG-8).
7. `--with-changes` path: diff inspection, scope detection, `API:` line suggestion, `BREAKING CHANGE:` footer suggestion.
8. `formatted` field: assembled commit message, ready to paste.
9. Reject bd type `epic` with a clear error per Decision L4.4.
10. Documentation update: `commit_context` entry in `README.md`; convention upgrade noted in CHANGELOG.
11. Update `CLAUDE.md` "Commit Strategy" section to reference the new tool and the trailer-based convention (BREAKING CHANGE marker).

### Epic 02.E — `reconcile` Extension: `StaleStatus`

**Owner:** rust-supervisor (Neo)
**Output:** `ReconcileEngine` covers all 7 drift types; `StaleStatus` detected and (with `--fix`) repaired.

Tasks:

1. Add `StaleStatus { issue_id, expected, actual }` variant to `DriftKind` enum in `unblock-core::reconcile`.
2. Apply `#[non_exhaustive]` to `DriftKind` (mandated by §2.6 project-wide policy). Audit and fix any downstream `match` site that breaks (compile-time gated).
3. Detection routine per Decision L5.2 — iterate graph nodes, compare `compute_status()` to Projects V2 Status field.
4. Repair routine per Decision L5.4 — call existing `update_project_field` mutation.
5. Severity classification: WARN in Phase 02 (Decision L5.3). Severity is a tool-output concern; the engine emits the drift, the tool flags severity.
6. Test fixtures F1–F4 per Decision L5.5 (depends on RG-9).
7. Property test: detection is idempotent — running twice on the same state yields the same drift set.
8. Update `reconcile` MCP tool description to enumerate all 7 drift types.

### Epic 02.F — Documentation, PRD/SPEC Patches, Phase 06 Forward-Compat

**Owner:** docs / Ada (architect)
**Output:** Remaining doc tasks after the plan-time patches in §13. The PRD §7, PRD §6, SPEC §12.2, SPEC §13.3, SPEC §14, CLAUDE.md "Coding Standards", and Phase 03 spec §20.1 patches were applied during plan APPROVED — see §13 for the canonical record.

Tasks:

1. ~~PRD §7 Phase 02 patch~~ — APPLIED at plan APPROVED. See §13.1.
2. ~~PRD §6 workspace evolution + dep graph + licensing~~ — APPLIED at plan APPROVED. See §13.2.
3. ~~SPEC §13.3 (Metrics)~~ — APPLIED at plan APPROVED. See §13.3.
4. ~~SPEC §14 (Resilience) + §12.2 (`#[non_exhaustive]` on Error)~~ — APPLIED at plan APPROVED. See §13.4.
5. ~~CLAUDE.md "Coding Standards" — `#[non_exhaustive]` policy~~ — APPLIED at plan APPROVED. See §13.5.
6. ~~Phase 03 spec §20.1 — UNRESOLVED → RESOLVED~~ — APPLIED at plan APPROVED.
7. CLAUDE.md "Commit Strategy" subsection — point at `commit_context` and document the trailer convention. Lands when `commit_context` ships (Epic 02.D).
8. README.md — add `doctor` and `commit_context` to the Tools section; document `UNBLOCK_RETRY_*` env vars. Lands when those tools ship.
9. Forward-compat contract test (Phase 06): `ServerMetrics` snapshot serialises stably; structure not changed when a hypothetical OTel adapter is added. Implementation in Epic 02.B; Epic 02.F verifies the test exists and passes.

---

## 10. Task Dependencies

```
Epic 02.A (resilience)         depends on RG-4, RG-5
   └── Epic 02.B (metrics)     depends on RG-1, RG-6, Epic 02.A (instrumentation point)
         └── Epic 02.C (doctor)  depends on RG-9, Epics 02.A + 02.B + 02.E
   └── Epic 02.D (commit_context)  depends on RG-3, RG-7, RG-8 (parallel with 02.B/C)
Epic 02.E (StaleStatus)        depends on RG-9 (parallel with 02.A)
                               (RG-10 closed; non_exhaustive application is engineering work)
Epic 02.F (docs/patches)       depends on all prior epics for accuracy
```

External-phase dependency: **Phase 03 Epic 03.2 cannot begin until Phase 02 Epic 02.A is merged.** Phase 03 Epics 03.1, 03.3, 03.4, 03.5 (storage, AST, walker) can proceed in parallel with the tail of Phase 02.

---

## 11. Acceptance Criteria

The phase is complete when **all** of the following hold.

### 11.1 Resilience

- [ ] All HTTP work in `GitHubClient` passes through `ResiliencePolicy::execute`. Static audit: zero direct `reqwest::Client` calls outside the resilience module.
- [ ] Breaker opens after 5 consecutive failures; cools down 10s; half-open admits 1 probe.
- [ ] Retry honours hybrid limit (5 attempts OR 30s deadline) — env-overridable.
- [ ] `Retry-After` header on 429 respected, capped at 30s, exceeded value triggers fail-fast.
- [ ] Order is breaker(retry(op)) — verified by property test.
- [ ] `Error::CircuitBreakerOpen` and `Error::RateLimited` are constructed by real code paths (no longer stub-only).

### 11.2 Observability

- [ ] `ServerMetrics` instrumented at single dispatch points (1× for tools, 1× for API).
- [ ] `doctor` returns full snapshot in default invocation.
- [ ] Per-call instrumentation overhead < 1µs (bench RG-6).
- [ ] Snapshot serialisation is stable — contract test for Phase 06 forward-compat passes.

### 11.3 `doctor` tool

- [ ] All 4 invocation modes produce the JSON shape of §2.3.
- [ ] `overall_status` mapping matches Decision L3.7 exactly.
- [ ] `--fix` delegates to `setup` for missing labels and to `reconcile --fix` for drift.
- [ ] Read-only invocation never mutates GitHub state (audit).
- [ ] `human_actions_needed[]` populated for token / project / network failures.

### 11.4 `commit_context` tool

- [ ] All 6 trailers emitted per Decision L4.3 with the right omission rules.
- [ ] Subject template uses correct CC prefix per Decision L4.4.
- [ ] `--with-changes` produces accurate scope, suggests `API:` line and `BREAKING CHANGE:` footer when applicable.
- [ ] bd type `epic` is rejected with a clear error.
- [ ] `formatted` field is a valid commit message that passes `git interpret-trailers --parse` round-trip.

### 11.5 `StaleStatus` reconcile

- [ ] `DriftKind` has 7 variants; `StaleStatus` is one of them.
- [ ] Detection finds drift in fixtures F1, F2; not in F3; F4 yields `MissingProjectField` not `StaleStatus`.
- [ ] `--fix` writes the computed value via `update_project_field`; second pass yields zero drift (idempotent).
- [ ] Severity in Phase 02 is WARN.
- [ ] `#[non_exhaustive]` applied to `DriftKind` per §2.6 project-wide policy; downstream `match` sites compile clean.

### 11.6 Phase 03 surface

- [x] Phase 03 spec §20.1 updated from UNRESOLVED to RESOLVED (committed alongside this plan APPROVED).
- [ ] `unblock-indexer` (in Phase 03) compiles with `use unblock_resilience::{ResiliencePolicy, IsRetryable}` against the merged Phase 02 surface.
- [ ] Smoke-test imports `ResiliencePolicy` from a Phase 03 prototype harness before Phase 03 begins (per §6.2 acceptance).

### 11.7 Quality gates (workspace)

- [ ] `cargo fmt --check --all` clean.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] `cargo test --workspace` passes.
- [ ] `cargo doc --no-deps --workspace` passes with zero warnings.
- [ ] Coverage ≥ 80% for the new resilience and metrics modules.

### 11.8 Documentation

- [x] PRD §7 Phase 02 patched per §13.1 (applied during plan APPROVED).
- [x] PRD §6 (workspace evolution + dep graph + licensing + core deps) patched per §13.2 (applied during plan APPROVED).
- [x] SPEC §13.3 + §14 + §12.2 patched per §13.3 / §13.4 (applied during plan APPROVED).
- [x] CLAUDE.md "Coding Standards" patched with `#[non_exhaustive]` policy per §13.5 (applied during plan APPROVED).
- [ ] CLAUDE.md "Commit Strategy" trailer-convention subsection (deferred to Epic 02.D — depends on `commit_context` shipping).
- [ ] README.md updated with `doctor`, `commit_context`, and `UNBLOCK_RETRY_*` env vars (Epic 02.F).

---

## 12. Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| `failsafe` async support is awkward; needs adapter | Medium | Medium | RG-4 prototype; if unfit, escalate to user — Decision L1.1 reopens |
| `backoff` does not honour `Retry-After` natively | Medium | Medium | RG-5 prototype; wrap with explicit pre-check if needed |
| `hdrhistogram` lock contention regresses p99 | Low | High | RG-6 bench gate; fall back to lock-free `quanta` + manual buckets if it fails |
| Per-process breaker is wrong for Phase 06 multi-tenant | High (in P06) | Medium | Decision L1.3 documents the scope; Phase 06 plan revisits explicitly |
| `git2` C-dep breaks Phase 04 cross-platform builds | Medium | High | RG-3 audit before commit; `gix` is the fallback |
| bd → GitHub URL resolution path is unstable | Medium | Medium | RG-7 documents the path; tool emits warning trailer if resolution fails |
| `commit_context` BREAKING CHANGE confuses existing agents | Low | Low | Pre-prod stance — no migration needed; CHANGELOG entry + CLAUDE.md update |
| `StaleStatus` false positives flood `reconcile` output | Medium | Medium | WARN severity in Phase 02 (Decision L5.3); Phase 04 escalation requires real-world calibration |
| Resilience layer extraction late-discovered as wrong shape for Phase 03 | Low | Medium | Mitigated by Decision §6.2 — `unblock-resilience` extracted in Epic 02.A; smoke-test imports the public API from a Phase 03 prototype harness before Phase 03 begins |

---

## 13. PRD / SPEC / CLAUDE.md Patches — APPLIED in this iteration

> **Status: APPLIED 2026-04-28.** Per the orchestrator's Decision 5, the patches drafted in earlier DRAFT revisions of this plan have now been applied to the live PRD.md / SPEC.md / CLAUDE.md alongside the locking of Decisions 1–5. The patches are listed below as the canonical record of what changed; Epic 02.F's remaining work is README.md, Phase 03 plan §20.1 cross-link verification, and the forward-compat contract test.

### 13.1 PRD §7 Phase 02 — APPLIED

The Phase 02 entry in PRD §7 has been rewritten to:

- Drop the OpenTelemetry bullet from the in-scope feature list and add a **Deferred** subsection cross-referencing Phase 06 for the OTel adapter.
- Mention `StaleStatus` explicitly as the 7th drift type (Phase 02 brings `ReconcileEngine` to completeness).
- Annotate `commit_context` with the BREAKING CHANGE note (subject-only bd-id → subject + canonical trailers) and explicitly note the trailer vocabulary is **extensible** (Decision 4).
- Add the new `unblock-resilience` crate to the Phase 02 scope bullets, cross-linked to §6.2 of this plan.
- Add `failsafe` / `backoff` library attribution.
- Replace the `OpenTelemetry` line with the `ServerMetrics` (in-memory) line and reference the `doctor` tool snapshot delivery vehicle.

The Phase 03 cross-reference text in PRD §7 was also updated: the previous "leverage the OpenTelemetry, circuit breaker, and retry policies" line was rewritten to "leverage the `unblock-resilience` crate (circuit breaker + retry); OpenTelemetry export is deferred to Phase 06" so Phase 03 prose no longer implies an OTel dependency in Phase 02.

The Phase 01 status line ("Complete (per bd …)") is already correct — no patch needed.

### 13.2 PRD §6 Rust Workspace — APPLIED (Decision 1)

PRD §6.1 (workspace evolution) was restructured to:

- Split the prior "Phases 01–02" combined entry into separate **Phase 01** (3 crates) and **Phase 02** (4 crates) entries, with `unblock-resilience` introduced at Phase 02.
- Carry `unblock-resilience` forward into Phase 03 (now 6 crates), Phase 04–05 (6 crates), Phase 06 (8 crates), and Phase 07 (9 crates).
- Add a paragraph explaining the rationale for extracting `unblock-resilience` immediately rather than later (cross-link to §6.2 of this plan).

PRD §6.2 (dep graph diagram) was updated to show `unblock-resilience` as a leaf with no unblock deps, consumed by both `unblock-github` and `unblock-indexer`.

PRD §6.3 (core dependencies table) gained `failsafe`, `backoff`, and `hdrhistogram` rows.

PRD §6.5 (licensing) added an `unblock-resilience` row (MIT, "open-source foundation — generic HTTP resilience policy").

### 13.3 SPEC §13.3 Metrics — APPLIED

Heading renamed from `13.3 Metrics (OpenTelemetry, optional)` to `13.3 Metrics`. Subsection rewritten to lock the Phase 02 in-memory `ServerMetrics` schema (counters, histograms, snapshot delivery via `doctor`) and to document the Phase 06 OTel adapter as a wrap-around (no schema change, OTel metric names listed). `ServerMetrics` provisional location flagged (`unblock-mcp::metrics` → possibly `unblock-core::metrics`, finalised by RG-1 in spec authoring).

### 13.4 SPEC §14 Resilience — APPLIED

Section preface added pointing at the `unblock-resilience` crate (Phase 02+) and stating the orthogonal-domain rationale for direct consumption by `unblock-indexer`. §14.1 (Circuit Breaker), §14.2 (Retry Policy), and a new §14.3 (Reuse by other crates) replaced the prior bare struct dumps with `failsafe`/`backoff`-based defaults, env-var table, and the `IsRetryable` trait-generic story. SPEC §12.2 also gained `#[non_exhaustive]` on `Error` and updated `RateLimited { reset_at }` / `CircuitBreakerOpen { since }` shapes to match the existing variant fields (per Phase 01 audit) and the project-wide `non_exhaustive` policy.

### 13.5 CLAUDE.md "Coding Standards" — APPLIED (Decision 3)

Added a new bullet codifying `#[non_exhaustive]` on growable public enums as a project-wide convention, with the precedent (`unblock-29p.70`) and Phase 02 extension (`DriftKind`) cited inline.

The "Commit Strategy" subsection patch (trailer convention) is **deferred to Epic 02.D** and lands when `commit_context` ships; CLAUDE.md cannot reference an unimplemented tool. This is a scope adjustment, not a decision change.

### 13.6 Remaining Epic 02.F work

Patches that are NOT yet applied (intentional — they depend on implementation):

- README.md: `doctor`, `commit_context`, `UNBLOCK_RETRY_*` env-var entries (depend on the tools shipping).
- CLAUDE.md "Commit Strategy" trailer-convention subsection (depends on `commit_context` shipping).
- Phase 03 plan §20.1 cross-link verification — re-read after this plan flips to APPROVED to confirm the UNRESOLVED → RESOLVED transition message reads cleanly.
- Forward-compat contract test (`ServerMetrics` snapshot serialisation) — Epic 02.B deliverable.

---

## 14. Definition of Done

The phase is **DONE** when:

1. All §11 acceptance criteria are met.
2. All **8 open** research gaps in §8 (RG-1, RG-3, RG-4, RG-5, RG-6, RG-7, RG-8, RG-9) have validated answers in `docs/research/02-*.md` (Smith). RG-2 and RG-10 are closed by Decisions 1 and 3 respectively at plan APPROVED time.
3. The spec `docs/specs/02-spec-mcp-complete.md` has been authored (Ada, after research) and approved.
4. Beads have been created for every task in §9 (Fernando), referencing this plan and the spec, with parent epics 02.A–02.F.
5. Implementation closes all beads through the standard pipeline (investigate → do → review → quality).
6. Remaining doc patches (README.md, CLAUDE.md "Commit Strategy" trailer subsection) merged. PRD / SPEC / CLAUDE.md "Coding Standards" / Phase 03 spec §20.1 patches were applied at plan APPROVED — see §13.
7. `unblock-mcp` v0.2.0 is tagged.
8. Phase 03 Epic 03.2 dispatches successfully against the Phase 02 resilience surface — verified by a smoke test that imports `ResiliencePolicy` from `unblock-resilience` in a Phase 03 crate prototype before Phase 03 starts.
