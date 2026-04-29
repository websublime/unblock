# Spec 02 — MCP Complete (v0.2.0)

**Status:** APPROVED
**Author:** Ada (architect)
**Date:** 2026-04-29
**Crates (new):** `unblock-resilience`
**Crates (modified):** `unblock-core`, `unblock-github`, `unblock-mcp`
**Source PRD:** [docs/PRD.md](../PRD.md) (§7 Phase 02)
**Source Plan:** [docs/plans/02-plan-mcp-complete.md](../plans/02-plan-mcp-complete.md) (APPROVED)
**Source Research:** [docs/research/02-research-mcp-complete.md](../research/02-research-mcp-complete.md)
**Companion:** [MANIFESTO](../MANIFESTO.md) · [SPEC](../SPEC.md)

---

## Table of Contents

1. [Scope & Conventions](#1-scope--conventions)
2. [Research Alignment & Locked Resolutions](#2-research-alignment--locked-resolutions)
3. [Crate Architecture](#3-crate-architecture)
4. [`unblock-resilience` — Public Surface](#4-unblock-resilience--public-surface)
5. [`unblock-resilience` — Internal Design](#5-unblock-resilience--internal-design)
6. [`ServerMetrics` — Module in `unblock-core`](#6-servermetrics--module-in-unblock-core)
7. [`unblock-github` — Resilience Wiring](#7-unblock-github--resilience-wiring)
8. [`reconcile` — `StaleStatus` Drift Type](#8-reconcile--stalestatus-drift-type)
9. [`doctor` MCP Tool](#9-doctor-mcp-tool)
10. [`commit_context` MCP Tool](#10-commit_context-mcp-tool)
11. [`#[non_exhaustive]` Project-Wide Policy](#11-non_exhaustive-project-wide-policy)
12. [Configuration & Environment Variables](#12-configuration--environment-variables)
13. [Error Model Additions](#13-error-model-additions)
14. [Observability & Tracing](#14-observability--tracing)
15. [Testing Strategy](#15-testing-strategy)
16. [Performance Methodology & Gates](#16-performance-methodology--gates)
17. [Cross-Phase Contracts](#17-cross-phase-contracts)
18. [Implementation Tasks](#18-implementation-tasks)
19. [Acceptance Criteria](#19-acceptance-criteria)
20. [Open Items & Forward References](#20-open-items--forward-references)

---

## 1. Scope & Conventions

### 1.1 What this spec authoritatively defines

This document is the **authoritative technical contract** for Phase 02. It pins:

- Exact public API of the new `unblock-resilience` crate.
- Exact module placement and signatures of `ServerMetrics`.
- Wiring rules between `unblock-github`, `unblock-resilience`, and `ServerMetrics`.
- Exact MCP tool surfaces for `doctor`, `commit_context`, and the `reconcile` extension.
- Test fixtures and acceptance gates for every new behaviour.

Where the plan committed to a decision, this spec **expands** it into a contract.
Where the research validated or contradicted an assumption, this spec **codifies the
research-validated reality** — never the original assumption.

### 1.2 What is NOT in scope

Mirror of plan §3, restated here for spec-time discipline:

- OpenTelemetry exporter (Phase 06).
- Remote MCP transport, HTTP server, webhook handler (Phase 06).
- Plugin pipeline (Phase 05).
- Code indexer (Phase 03 — separate spec).
- Materialised Fast Path (Phase 04).
- Watchdog auto-trigger for `doctor` (Phase 06+).
- Validation/parsing of pre-existing commit messages by `commit_context` (post-02).
- GitHub App authentication, GHE testing, cargo-dist binaries (Phase 04).

### 1.3 Conventions used in this spec

- **MUST / MUST NOT / SHOULD / MAY** follow RFC 2119 conventions.
- Code blocks tagged `rust` are **normative signatures** unless explicitly marked `// illustrative`.
- Where a doctest-style example appears (`fn main()`), it is **non-normative illustration**.
- Schema fragments tagged `json` are **wire-format normative**.
- File paths are absolute or workspace-relative (`crates/...`); relative paths in module
  layouts are resolved against the crate root.
- Pre-production stance applies (per `feedback_pre_production`): no migrations, no
  backward-compat shims, breaking changes acceptable across all unblock crates.

### 1.4 Decision provenance

Every locked decision in this spec carries provenance to the plan's §4 (`Lx.y`),
the research's RG-N rows, or the `NR-x.y` resolutions captured at research adjudication.
A spec-introduced decision (one not present in plan or research) is marked **SPEC-ORIGINAL**
and MUST be brought to the user's attention during spec review.

---

## 2. Research Alignment & Locked Resolutions

This section records how research findings translate into spec-level constraints.
**The spec MUST conform to the research-validated reality, not the original plan
assumption.**

| Plan / research item | Status | Spec-level resolution |
|---|---|---|
| **RG-1 — `ServerMetrics` placement** | CONFIRMED → `unblock-core::metrics` | §6.1 places the module in `unblock-core/src/metrics.rs`. |
| **RG-3 — Git library choice** | CONFIRMED → `gix` | §10.4 pins `gix` 0.81.x with `status` + `index` features; `git2` is rejected. |
| **RG-4 — `failsafe` fitness** | PARTIALLY CONFIRMED — no public state getter | §5.3 mandates the `Instrument`-mirror pattern; `BreakerSnapshot` is **eventually consistent**. |
| **RG-5 — `backoff` fitness** | CONFIRMED with maintenance flag | §5.4 wraps `backoff` 0.4 behind `RetryPolicy`; §20.3 records the Phase 06+ revisit trigger for `backon`. |
| **RG-6 — `hdrhistogram` concurrency** | PARTIALLY CONFIRMED — no atomic histogram | §6.2 mandates `Mutex<Histogram<u64>>` per metric (Option A); §16.2 re-scopes the bench to realistic load. |
| **RG-9 — `StaleStatus` fixtures** | CONFIRMED — existing mock surface sufficient | §8.4 reuses existing `MockGitHubClient` accessors; F4 codified as a negative filter test. |
| **CC-5 — `is_retryable()` helper missing** | CONFIRMED — helper does NOT exist in workspace | §7.2 implements `IsRetryable for unblock_github::Error` from scratch (no "wraps existing helper" claim). |
| **Q-6.1 — Histogram bounds** | RESOLVED at adjudication | §6.2 pins `tool_durations=(1µs, 60s, 3)`, `api_durations=(1ms, 60s, 3)`. |
| **NR-6.1.x — bench load + gates** | RESOLVED at adjudication | §16.2 codifies 1k/s sustained + 10k/s burst, <500ns uncontested / <5µs burst, soft 5% p99 regression budget. |
| **NR-9.1 — F4 reformulation** | RESOLVED at adjudication | §8.4 codifies the pre-detection filter invariant; F4 is a negative test. |
| **RG-7 / RG-8 / CC-3 / CC-6** | DROPPED (scope error) | No bd indirection anywhere in `commit_context`. §10 reads the active GitHub claim directly. |
| **RG-2 / RG-10** | CLOSED at plan time | Spec applies the locked decisions (`unblock-resilience` extracted; `#[non_exhaustive]` policy project-wide). |

**Spec invariant:** any text below that contradicts the table above is a defect.

---

## 3. Crate Architecture

Phase 02 introduces one new crate and modifies three existing crates. The dependency
graph after Phase 02 is:

```
unblock-core ────────────────── (no unblock deps; gains hdrhistogram + ServerMetrics)
    ▲
    │
unblock-resilience ──────────── (no unblock deps; new crate)
    ▲                       ▲
    │                       │
unblock-github          unblock-indexer (Phase 03)
    ▲                       ▲
    │                       │
unblock-mcp ────────────────┘  (modified: doctor, commit_context, reconcile ext.)
```

### 3.1 New crate — `unblock-resilience`

```
crates/unblock-resilience/
├── Cargo.toml          (license = "MIT", edition = "2024", deny(unsafe_code))
└── src/
    ├── lib.rs          ← module-level docs + re-exports
    ├── traits.rs       ← `IsRetryable` trait + `ResilienceError<E>`
    ├── breaker.rs      ← `Breaker` (failsafe wrapper + Instrument mirror)
    ├── retry.rs        ← `RetryPolicy` (backoff wrapper)
    ├── policy.rs       ← `ResiliencePolicy` (composition)
    └── snapshot.rs     ← `BreakerSnapshot`, `RetrySnapshot`, `BreakerState`
```

**Dependencies (workspace):**

| Dep | Purpose | Version |
|---|---|---|
| `failsafe` | Circuit breaker primitive | `1.3` |
| `backoff` | Retry-with-backoff primitive (with `tokio` feature) | `0.4` |
| `tokio` | Async runtime adapter for `failsafe::futures` and `backoff::future` | `1` (workspace) |
| `tracing` | Structured logging | workspace |
| `snafu` | Error type | workspace |
| `serde` (optional) | Snapshot serialisation for `doctor` JSON output | feature `serde` |

**No** dependency on `unblock-core`, `unblock-github`, `unblock-mcp`, or any future
`unblock-indexer*` crate. Verified by `cargo tree -p unblock-resilience` in CI.

### 3.2 Modified — `unblock-core`

```
crates/unblock-core/src/
├── lib.rs                  ← module declaration: pub mod metrics;
├── metrics.rs              ← NEW: ServerMetrics + MetricsSnapshot
├── reconcile.rs            ← MODIFIED: DriftKind::StaleStatus + #[non_exhaustive]
└── (unchanged)
```

**New dependency:** `hdrhistogram` (workspace dep, version `7`).

### 3.3 Modified — `unblock-github`

```
crates/unblock-github/src/
├── client.rs               ← MODIFIED: holds ResiliencePolicy; HTTP wrapped
├── errors.rs               ← MODIFIED: impl IsRetryable for Error (NEW impl)
└── (unchanged)
```

**New dependencies:** `unblock-resilience` (this workspace crate). No new external deps.

### 3.4 Modified — `unblock-mcp`

```
crates/unblock-mcp/src/
├── server.rs               ← MODIFIED: ServerState carries Arc<ServerMetrics>;
│                              tool dispatch records to ServerMetrics
├── tools/
│   ├── mod.rs              ← MODIFIED: register doctor + commit_context
│   ├── doctor.rs           ← NEW
│   ├── commit_context.rs   ← NEW
│   └── reconcile.rs        ← MODIFIED: surface StaleStatus drift kind
```

**New dependencies:** `unblock-resilience`, `gix` (with `status` + `index` features),
`unblock-core::metrics` (already a workspace path dep — no manifest change beyond the
internal use).

### 3.5 Workspace constraints (unchanged)

- Edition 2024.
- `#![deny(unsafe_code)]` workspace-wide.
- `snafu` exclusive (no `thiserror`, no `anyhow`).
- `///` doc on every `pub fn` / `pub struct` / `pub enum`; `//!` on every module.
- `tracing` JSON to **stderr** only (stdio reserved for MCP protocol).

---

## 4. `unblock-resilience` — Public Surface

This is the **stable contract** consumed by `unblock-github` (Phase 02) and
`unblock-indexer` (Phase 03). API stability guarantee is semver within unblock.

### 4.1 Module-level overview

```rust
//! Generic HTTP-resilience policy: composed circuit breaker + retry-with-backoff.
//!
//! Two consumer crates: `unblock-github` (this phase) and `unblock-indexer`
//! (Phase 03). Both are architecturally orthogonal — `unblock-resilience` carries
//! zero unblock-domain knowledge.
//!
//! See [`ResiliencePolicy::execute`] for the single entry point. State scope is
//! per-process: each `ResiliencePolicy` instance owns its own breaker and retry
//! configuration.
```

### 4.2 Types

```rust
// crates/unblock-resilience/src/policy.rs

/// Composed circuit-breaker + retry-with-backoff policy.
///
/// Order of operations (LOCKED — Decision L1.4): breaker **outside**, retry
/// **inside**. The breaker observes only the FINAL outcome of the retry loop.
/// A successful retry → breaker records success. Final failure after retries
/// exhaust → breaker records failure.
#[derive(Clone)]
pub struct ResiliencePolicy {
    breaker: Arc<Breaker>,        // see §5.3
    retry:   Arc<RetryPolicy>,    // see §5.4
}

impl ResiliencePolicy {
    /// Build a policy with default knobs.
    ///
    /// Defaults (Decision L1.5/L1.6):
    /// - Max retries: 5 attempts
    /// - Total deadline: 30 s
    /// - Retry-After cap: 30 s (hard-coded)
    /// - Breaker failure threshold: 5 consecutive
    /// - Breaker cooldown: 10 s
    pub fn default() -> Self;

    /// Build a policy from environment variables.
    ///
    /// Reads `UNBLOCK_RETRY_MAX_ATTEMPTS`, `UNBLOCK_RETRY_DEADLINE_SECS`. Variables
    /// not set fall back to `Self::default()` values. Parse failures emit
    /// `tracing::warn!` and use the default for that knob.
    pub fn from_env() -> Self;

    /// Customise the breaker. Consumer-supplied breaker MUST satisfy the
    /// Instrument-mirror contract (§5.3) — call sites in unblock crates MUST use
    /// the constructors in `unblock_resilience::breaker` rather than building
    /// `failsafe::CircuitBreaker` directly.
    pub fn with_breaker(self, breaker: Breaker) -> Self;

    /// Customise the retry policy.
    pub fn with_retry(self, retry: RetryPolicy) -> Self;

    /// Execute an idempotent async operation under the composed policy.
    ///
    /// The closure `op` is invoked once per attempt. Each invocation MUST return
    /// a fresh `Future` (no shared state across attempts). The error type `E`
    /// MUST implement [`IsRetryable`].
    ///
    /// Returns `Ok(T)` on success; on final failure the inner error is wrapped in
    /// [`ResilienceError`] and the breaker is informed of the failure.
    pub async fn execute<F, Fut, T, E>(&self, op: F) -> Result<T, ResilienceError<E>>
    where
        F: FnMut() -> Fut + Send,
        Fut: Future<Output = Result<T, E>> + Send,
        T: Send,
        E: IsRetryable + Send + std::error::Error + 'static;

    /// Read-only snapshot of the breaker state. **Eventually consistent** with the
    /// underlying `failsafe` state machine (§5.3): may briefly disagree during a
    /// state transition. Convergence window is sub-microsecond. Safe for
    /// non-control-flow consumers (e.g. `doctor` reporting).
    pub fn breaker_snapshot(&self) -> BreakerSnapshot;

    /// Read-only snapshot of the retry policy configuration.
    pub fn retry_snapshot(&self) -> RetrySnapshot;
}
```

### 4.3 `IsRetryable` trait

```rust
// crates/unblock-resilience/src/traits.rs

/// Bridge trait that lets `ResiliencePolicy::execute` interpret any error type.
///
/// Implementations MUST be pure (no I/O, no allocation beyond what the variant
/// already holds) — they are called once per attempt inside the retry loop.
pub trait IsRetryable {
    /// `true` if the error represents a transient condition that retry SHOULD
    /// re-attempt (e.g. 429 rate limit, 503 server unavailable, network glitch).
    fn is_retryable(&self) -> bool;

    /// Honoured retry delay, if the error carries one (e.g. parsed `Retry-After`
    /// header). The retry loop MUST cap this at 30 s (LOCKED — Decision L1.6); a
    /// value above the cap MUST cause the retry loop to fail-fast.
    ///
    /// The cap is enforced **inside** `ResiliencePolicy::execute`, not in
    /// implementations of this trait. Implementations report the **observed**
    /// duration unmodified.
    fn retry_after(&self) -> Option<Duration> {
        None
    }
}
```

### 4.4 `ResilienceError`

```rust
// crates/unblock-resilience/src/traits.rs

/// Outcome of a failed `ResiliencePolicy::execute`.
#[derive(Debug, Snafu)]
#[non_exhaustive]
pub enum ResilienceError<E>
where
    E: std::error::Error + 'static,
{
    /// The breaker rejected the call before any attempt (state was Open or
    /// HalfOpen-with-probe-already-issued).
    #[snafu(display("circuit breaker open — call rejected"))]
    BreakerOpen,

    /// The retry loop exhausted attempts or the deadline elapsed; the inner
    /// error is the **last** observed failure.
    #[snafu(display("retry exhausted: {source}"))]
    RetryExhausted { source: E },

    /// The error was non-retryable (`IsRetryable::is_retryable() == false`); no
    /// retry was attempted.
    #[snafu(display("{source}"))]
    Permanent { source: E },
}
```

### 4.5 Snapshots

```rust
// crates/unblock-resilience/src/snapshot.rs

/// Read-only snapshot of the circuit-breaker state.
///
/// `state` and `last_failure_at` are populated by an `Instrument` mirror (§5.3);
/// they are eventually consistent with the underlying state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct BreakerSnapshot {
    pub state: BreakerState,
    pub failure_count: usize,
    pub last_failure_at: Option<SystemTime>,  // SystemTime, not Instant — serializable
}

/// Read-only snapshot of the retry-policy configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct RetrySnapshot {
    pub max_attempts: u32,
    pub deadline: Duration,
    pub initial_interval: Duration,
    pub multiplier: f64,
    pub max_interval: Duration,
    pub randomization_factor: f64,
}

/// Circuit-breaker state, in the canonical three-state form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum BreakerState {
    Closed,
    Open,
    HalfOpen,
}
```

> **Spec note (SPEC-ORIGINAL):** the plan §6.3 carried `BreakerState::Open { since: Instant }`
> as a struct variant. This spec replaces the embedded `Instant` with a separate
> `last_failure_at: Option<SystemTime>` on `BreakerSnapshot`, because (a) `Instant`
> is **not** serializable and `doctor` JSON output needs the timestamp, (b) keeping
> `BreakerState` as a flat enum mirrors `failsafe`'s internal three-state model
> faithfully and avoids leaking implementation detail. **User must approve.**

---

## 5. `unblock-resilience` — Internal Design

### 5.1 Composition order (LOCKED — Decision L1.4)

```
caller                                              caller
  │                                                   │
  ▼                                                   ▼
ResiliencePolicy::execute                ┌──── ResilienceError ────┐
  │                                                   │
  ▼                                                   │
breaker.call(retry_loop)              <─── breaker observes the FINAL outcome
                                            (success or failure after retries
                                             exhaust, NOT each per-attempt error)
  │
  ▼
retry_loop
  │  on each attempt
  ▼
op() → Future<Result<T, E>>
```

**Invariant:** the breaker's failure counter MUST NOT advance on a per-attempt
error inside the retry loop. Only the loop's final error increments the counter.
Verified by property test (§15.3).

### 5.2 The `execute` algorithm

Pseudocode (normative):

```text
fn execute(policy, op):
    breaker.call(async {
        let start = Instant::now()
        let mut attempt = 0
        loop:
            attempt += 1
            let result = op().await
            match result:
                Ok(t)            => return Ok(t)            // breaker.on_success
                Err(e):
                    if !e.is_retryable():
                        return Err(Permanent(e))            // breaker.on_error
                    if attempt >= policy.retry.max_attempts:
                        return Err(RetryExhausted(e))       // breaker.on_error
                    let backoff = next_backoff_interval(policy.retry, attempt)
                    let wait    = match e.retry_after():
                        Some(d) if d > Duration::from_secs(30) =>
                            return Err(RetryExhausted(e))   // fail-fast on >30s
                        Some(d) => d                         // honour
                        None    => backoff
                    if start.elapsed() + wait > policy.retry.deadline:
                        return Err(RetryExhausted(e))       // deadline beats wait
                    tokio::time::sleep(wait).await
    })
```

**Notes:**

- The `30 s` Retry-After cap is the documented L1.6 hard limit; not configurable.
- Hybrid limit (L1.5): both `max_attempts` AND `deadline` are checked **every** loop
  iteration. Whichever fires first wins.
- The breaker wraps the **whole** loop (`breaker.call(retry_loop)`); the breaker is
  consulted **once** per `execute` call.

### 5.3 Breaker module — `Breaker`

Wraps `failsafe::Config::new().build()` plus an Instrument mirror.

```rust
// crates/unblock-resilience/src/breaker.rs (normative skeleton)

pub struct Breaker {
    inner: failsafe::futures::CircuitBreaker</* concrete type */>,
    mirror: Arc<InstrumentMirror>,
}

struct InstrumentMirror {
    state:               AtomicU8,        // 0=Closed, 1=Open, 2=HalfOpen
    failure_count:       AtomicUsize,
    last_failure_at:     Mutex<Option<SystemTime>>,
}

impl failsafe::Instrument for Arc<InstrumentMirror> {
    fn on_call_rejected(&self) {
        // No state change — only logs a counter for observability.
        // (Optional: increment a rejected_calls counter on ServerMetrics; wired in §7.)
    }
    fn on_open(&self) {
        self.state.store(1 /* Open */, Ordering::SeqCst);
        if let Ok(mut g) = self.last_failure_at.lock() {
            *g = Some(SystemTime::now());
        }
        tracing::warn!(target: "unblock_resilience", "circuit breaker opened");
    }
    fn on_half_open(&self) {
        self.state.store(2 /* HalfOpen */, Ordering::SeqCst);
        tracing::info!(target: "unblock_resilience", "circuit breaker half-open");
    }
    fn on_closed(&self) {
        self.state.store(0 /* Closed */, Ordering::SeqCst);
        tracing::info!(target: "unblock_resilience", "circuit breaker closed");
    }
}

impl Breaker {
    pub fn new(failure_threshold: u32, cooldown: Duration) -> Self;
    pub fn snapshot(&self) -> BreakerSnapshot;        // reads atomics + mutex
    pub(crate) async fn call<F, T, E>(&self, fut: F) -> Result<T, BreakerError<E>>
    where
        F: Future<Output = Result<T, E>>,
        E: std::error::Error;
}
```

**Invariants:**

- `Instrument` callbacks MUST be cheap (atomics + ≤1 short mutex) per RG-4 R-4b.
  No I/O, no allocation beyond `SystemTime::now()`.
- `state` is the **single source of truth** for `BreakerSnapshot::state`. The
  `failsafe` private `State` enum is never exposed.
- `failure_count` is incremented on **every** call to `on_open` (transition into
  Open). It is **not** the per-attempt failure count of the retry loop — it is the
  cumulative count of breaker-observed failures.

**Failure policy (LOCKED):**

```rust
failsafe::Config::new()
    .failure_policy(failsafe::failure_policy::consecutive_failures(
        5,                                                       // threshold
        failsafe::backoff::equal_jittered(
            Duration::from_secs(10),                             // cooldown
            Duration::from_secs(60),                             // max cooldown
        ),
    ))
    .build()
```

The 60 s upper bound on the failsafe-internal cooldown is **distinct** from the
plan's "10 s cooldown" knob: `failsafe`'s `equal_jittered` policy uses the first
duration as the base cooldown; the second is the jitter ceiling. The plan's
"10 s cooldown" is the **base**; the jitter ceiling is set to 60 s so that under
sustained outage the breaker doesn't slam GitHub every 10 s.

### 5.4 Retry module — `RetryPolicy`

Wraps `backoff::ExponentialBackoff`.

```rust
// crates/unblock-resilience/src/retry.rs (normative skeleton)

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub deadline:     Duration,

    // Underlying backoff parameters (passed to ExponentialBackoff at execute time)
    pub initial_interval:    Duration,    // default 500 ms
    pub multiplier:          f64,         // default 2.0
    pub max_interval:        Duration,    // default 30 s
    pub randomization_factor: f64,        // default 0.5  (jitter)
}

impl RetryPolicy {
    pub fn default() -> Self;
    pub fn from_env() -> Self;            // reads UNBLOCK_RETRY_*

    /// Build the per-call backoff iterator. A new iterator is created on every
    /// `ResiliencePolicy::execute` call so the loop starts from `initial_interval`.
    pub(crate) fn make_backoff(&self) -> backoff::ExponentialBackoff {
        backoff::ExponentialBackoff {
            initial_interval:     self.initial_interval,
            randomization_factor: self.randomization_factor,
            multiplier:           self.multiplier,
            max_interval:         self.max_interval,
            max_elapsed_time:     Some(self.deadline),
            ..Default::default()
        }
    }
}
```

The `max_attempts` cap is enforced **outside** `backoff` (in §5.2's algorithm):
`backoff` itself does not have a max-attempts knob. The `deadline` is enforced
**both** by `backoff::max_elapsed_time` (defensive) and by the loop's
`start.elapsed() + wait > deadline` check (authoritative — produces a clean
`RetryExhausted` error before sleeping past the deadline).

### 5.5 Configuration env-var contract

| Variable | Type | Default | Invalid value behaviour |
|---|---|---|---|
| `UNBLOCK_RETRY_MAX_ATTEMPTS` | `u32` | `5` | `tracing::warn!` + use default |
| `UNBLOCK_RETRY_DEADLINE_SECS` | `u64` | `30` | `tracing::warn!` + use default |

Breaker thresholds (`failure_threshold`, `cooldown`) are **not** env-configurable in
Phase 02 (LOCKED — plan §2.1 default table). They are constants in `Breaker::default()`
construction. Phase 06 may revisit.

---

## 6. `ServerMetrics` — Module in `unblock-core`

### 6.1 Placement and module skeleton

**Locked location (RG-1):** `crates/unblock-core/src/metrics.rs`.

```rust
//! In-memory server-side metrics: counters, gauges, latency histograms.
//!
//! Read-only snapshot delivery vehicle for the `doctor` MCP tool. Phase 06's
//! OpenTelemetry adapter will wrap this struct without modifying its shape — the
//! same atomics and histograms will become OTel signals (Decision L2.2 forward-
//! compat invariant).
//!
//! All recorders are **lock-cheap**: counters/gauges use atomics; histograms use
//! `Mutex<Histogram<u64>>` per metric (RG-6 — `hdrhistogram` has no atomic variant).
//!
//! See spec §6 for the full invariants.

pub use snapshot::MetricsSnapshot;

mod snapshot;
```

The metrics module is **pure**: no I/O, no async, no rmcp / MCP types. Verified by
`cargo doc --no-deps -p unblock-core` rendering a clean module surface.

### 6.2 `ServerMetrics` (locked)

```rust
// crates/unblock-core/src/metrics.rs

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use hdrhistogram::Histogram;

/// In-memory server metrics aggregator.
///
/// Single instance per MCP server process (held in `Arc` and shared across
/// handlers). All recording methods are lock-cheap and safe to call from any
/// async task.
pub struct ServerMetrics {
    // === per-tool counters and durations ===
    tool_calls:     HashMap<&'static str, AtomicU64>,
    tool_durations: HashMap<&'static str, Mutex<Histogram<u64>>>,

    // === per-API counters and durations ===
    api_calls:     HashMap<&'static str, AtomicU64>,
    api_durations: HashMap<&'static str, Mutex<Histogram<u64>>>,

    // === cache stats ===
    cache_hits:      AtomicU64,
    cache_misses:    AtomicU64,
    cache_evictions: AtomicU64,
    cache_size:      AtomicU64,    // gauge

    // === graph stats ===
    graph_issues: AtomicU64,    // gauge
    graph_edges:  AtomicU64,    // gauge
}
```

**Histogram bounds (LOCKED — Q-6.1):**

| Histogram family | low | high | sigfig | Construction |
|---|---|---|---|---|
| `tool_durations` (per tool name) | 1 µs | 60 s | 3 | `Histogram::<u64>::new_with_bounds(1, 60_000_000_000, 3)` |
| `api_durations` (per API method)  | 1 ms | 60 s | 3 | `Histogram::<u64>::new_with_bounds(1_000_000, 60_000_000_000, 3)` |

Units: **nanoseconds** stored. The `Duration::as_nanos()` value is clamped to
`u64::MAX` (saturation cast); out-of-range values trigger `tracing::warn!` and are
silently truncated **into** range (not dropped) — this preserves p99 visibility
for catastrophic-latency outliers.

**Per-metric override discipline:** any call site that constructs a histogram with
bounds **other than** the defaults above MUST carry an inline justification
comment of the form:

```rust
// SAFETY: bounds widened because <reason> — see <bead-or-doc-reference>
let h = Histogram::<u64>::new_with_bounds(/* custom */)?;
```

### 6.3 Constructors and recorders

```rust
impl ServerMetrics {
    /// Build with the locked tool/API name vocabulary.
    ///
    /// `tool_names` is the set of MCP tool names the server registers (currently:
    /// `ready`, `claim`, `release`, `close`, `dep_add`, `dep_remove`, `reopen`,
    /// `show`, `prime`, `setup`, `reconcile`, `doctor`, `commit_context` — 13
    /// tools post-Phase 02).
    ///
    /// `api_names` is the set of GitHub API method labels the resilience layer
    /// records (e.g. `"graphql.fetch_graph_data"`, `"rest.update_field"`).
    pub fn new(
        tool_names: &[&'static str],
        api_names:  &[&'static str],
    ) -> Self;

    /// Record a tool invocation. Increments `tool_calls[name]` and records the
    /// duration into `tool_durations[name]`. Unknown `name` triggers a
    /// `tracing::warn!` and is ignored (NEVER panics — out-of-vocabulary tool
    /// names are operational, not programmer errors).
    pub fn record_tool(&self, name: &'static str, duration: Duration);

    /// Record a GitHub API call (post-resilience).
    pub fn record_api(&self, name: &'static str, duration: Duration);

    /// Cache event recorders.
    pub fn record_cache_hit(&self);
    pub fn record_cache_miss(&self);
    pub fn record_cache_eviction(&self);
    pub fn set_cache_size(&self, size: u64);

    /// Graph gauges (called after every cache rebuild).
    pub fn set_graph_size(&self, issues: u64, edges: u64);

    /// Materialise a serializable snapshot. Thread-safe; clones histograms and
    /// reads atomics with `Ordering::Relaxed` (snapshot is a coherent **point-in-
    /// time** view but does NOT linearise across counters).
    pub fn snapshot(&self) -> MetricsSnapshot;
}
```

**`record_tool` / `record_api` invariants:**

- Lock acquisition is **the only** synchronisation cost. Per RG-6 / NR-6.1.x: under
  uncontested load (1k records/s sustained per metric) the lock acquisition + record
  cost is < 500 ns.
- The `Histogram::record` may return `Err(RecordError::ValueOutOfRangeResizeDisabled)`.
  The recorders **MUST** call `record_correct` or saturate the value via
  `min(value, histogram.high())` before `record` to avoid the error path; the bounds
  in §6.2 are sized so saturation fires only on catastrophic outliers (>60 s), which
  triggers a `tracing::warn!` log.
- Recorders MUST be cancel-safe: `tokio::time::Instant::elapsed()` is captured
  outside `record_tool`, and the recorder MUST NOT hold the mutex across an
  `await` point. (No `await` inside `record_*` — these are sync functions.)

### 6.4 `MetricsSnapshot`

```rust
// crates/unblock-core/src/metrics/snapshot.rs

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct MetricsSnapshot {
    pub tool_calls:     HashMap<String, u64>,
    pub tool_durations: HashMap<String, HistogramSummary>,
    pub api_calls:      HashMap<String, u64>,
    pub api_durations:  HashMap<String, HistogramSummary>,
    pub cache:          CacheSnapshot,
    pub graph:          GraphSnapshot,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistogramSummary {
    pub count: u64,
    pub min_ns:  u64,
    pub mean_ns: u64,
    pub p50_ns:  u64,
    pub p90_ns:  u64,
    pub p99_ns:  u64,
    pub max_ns:  u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CacheSnapshot {
    pub hits:      u64,
    pub misses:    u64,
    pub evictions: u64,
    pub size:      u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphSnapshot {
    pub issues: u64,
    pub edges:  u64,
}
```

### 6.5 Forward-compat contract test (Phase 06)

A test in `crates/unblock-core/src/metrics.rs` (cfg-test) verifies that
`MetricsSnapshot` round-trips through `serde_json` and that adding a new metric
**does not** alter any existing field's serialisation key. This is the "schema
stability" gate that Phase 06's OTel adapter relies on.

```rust
#[cfg(test)]
mod forward_compat {
    use super::*;

    /// Locked schema keys — Phase 06 OTel adapter depends on these.
    /// Adding a new key is allowed; renaming or removing a key is a BREAKING CHANGE.
    const REQUIRED_KEYS: &[&str] = &[
        "tool_calls", "tool_durations",
        "api_calls",  "api_durations",
        "cache",      "graph",
    ];

    #[test]
    fn snapshot_serialisation_preserves_required_keys() { /* ... */ }

    #[test]
    fn histogram_summary_preserves_required_keys() { /* ... */ }
}
```

---

## 7. `unblock-github` — Resilience Wiring

### 7.1 `GitHubClient` modifications

```rust
// crates/unblock-github/src/client.rs (normative diff sketch)

pub struct GitHubClient {
    http:   reqwest::Client,
    policy: ResiliencePolicy,            // NEW
    /* existing fields unchanged */
}

impl GitHubClient {
    pub fn new(/* existing params */) -> Self {
        Self {
            http: reqwest::Client::builder()/* ... */.build().unwrap_or_default(),
            policy: ResiliencePolicy::from_env(),    // NEW: env-driven knobs
            /* ... */
        }
    }

    /// Read-only access for the `doctor` tool (§9). Returns the pair
    /// `(BreakerSnapshot, RetrySnapshot)` for inclusion in `metrics_snapshot`.
    pub fn resilience_snapshot(&self) -> (BreakerSnapshot, RetrySnapshot) {
        (self.policy.breaker_snapshot(), self.policy.retry_snapshot())
    }
}
```

### 7.2 `IsRetryable` impl (NEW — per CC-5)

The plan's claim that `is_retryable()` already exists is **incorrect** (research
CC-5). The trait impl is built **from scratch**:

```rust
// crates/unblock-github/src/errors.rs (added at bottom)

use unblock_resilience::IsRetryable;
use std::time::Duration;

impl IsRetryable for Error {
    fn is_retryable(&self) -> bool {
        match self {
            Error::RateLimited { .. }
          | Error::GitHubUnavailable { .. }
          | Error::PostMutationRebuildFailed { .. }
          | Error::PreMutationPrimeFailed { .. } => true,
            // Status-code based fallback for variants that carry a status.
            Error::GitHubApi { status, .. } => matches!(*status, 429 | 502 | 503 | 504),
            // Non-retryable: domain errors, GraphQL errors, FORBIDDEN, missing config,
            // git-remote, unknown owner, mock stub, circuit breaker (already a final
            // outcome — re-attempting would be pointless inside the same execute call).
            _ => false,
        }
    }

    fn retry_after(&self) -> Option<Duration> {
        match self {
            Error::RateLimited { reset_at } => {
                let now = chrono::Utc::now();
                if *reset_at > now {
                    (*reset_at - now).to_std().ok()
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}
```

**Note:** `Error::CircuitBreakerOpen` is **not** retryable inside `execute` —
the breaker is the **outer** layer; if a breaker error reaches the inner retry loop
(impossible in normal flow), retrying would be a logic error.

### 7.3 HTTP-call wrapping invariant

**Static audit (Epic 02.A Task 4 — acceptance gate):** every HTTP call site in
`unblock-github` MUST go through `self.policy.execute(...)`. No raw `self.http.get()`
or `self.http.post()` calls. Verified by:

```bash
# Acceptance audit (run in CI):
rg --type rust 'self\.http\.(get|post|put|delete|head|patch)' crates/unblock-github/src/
# Expected output: matches MUST be inside a closure passed to `policy.execute(...)`.
```

A small `pub(crate) async fn http_request(&self, …) -> Result<Response, Error>`
helper inside `client.rs` wraps the raw call so every endpoint method calls
`self.policy.execute(|| self.http_request(...))`. The helper is the **only** place
the raw `reqwest::Client` is dereferenced.

### 7.4 Construction of new error variants

After Phase 02 the two stub variants are **constructed by real code**:

| Variant | Constructed by |
|---|---|
| `Error::CircuitBreakerOpen { since }` | `unblock-github` translates `ResilienceError::BreakerOpen` → `Error::CircuitBreakerOpen` at the boundary; `since` is computed from `BreakerSnapshot::last_failure_at`. |
| `Error::RateLimited { reset_at }` | `unblock-github` parses GitHub `X-RateLimit-Reset` header on 429 responses and constructs the variant before returning the error to the retry loop. |

### 7.5 Wiring metrics from inside `ResiliencePolicy::execute`

`unblock-resilience` itself does **not** know about `ServerMetrics`. The wiring
point is in `unblock-github::client`:

```rust
// crates/unblock-github/src/client.rs (illustrative)

async fn graphql<T>(&self, query: &str /* ... */) -> Result<T, Error> {
    let metrics = self.metrics.clone();         // Arc<ServerMetrics>
    let label   = "graphql";                    // or specific operation name
    let started = std::time::Instant::now();
    let res = self.policy.execute(|| async {
        /* one HTTP attempt */
    }).await;
    metrics.record_api(label, started.elapsed());
    res.map_err(translate_resilience_error)
}
```

`ServerMetrics::record_api` is called with the **wall-clock** duration of the
entire `execute` call (including retries + sleeps). This is the user-meaningful
latency; it matches what the agent perceives as "how long did this GitHub call take".

> **Spec note (SPEC-ORIGINAL):** the plan §9 Epic 02.B Task 4 said "Wire `api_calls` +
> `api_durations` into `ResiliencePolicy::execute` (single instrumentation point)".
> This spec moves the instrumentation **out** of `unblock-resilience` and **into**
> the `unblock-github` call sites. Rationale: keeping `unblock-resilience` free of
> any knowledge of `ServerMetrics` preserves the orthogonality that justified the
> crate extraction in the first place. Phase 03's `unblock-indexer` will wire its
> own metrics path the same way. **User must approve.**

---

## 8. `reconcile` — `StaleStatus` Drift Type

### 8.1 `DriftKind` variant addition

```rust
// crates/unblock-core/src/reconcile.rs (modified)

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]                                  // NEW — §11 policy
pub enum DriftKind {
    /* existing 6 variants unchanged */

    /// The graph-computed expected Status differs from the value stored in the
    /// Projects V2 Status field for an issue that IS a member of the Project.
    ///
    /// Cause: external mutation of the Status field (manual UI edit, third-party
    /// automation), OR a graph state change that was not propagated to the field
    /// (e.g. a closure cascade that failed mid-way).
    ///
    /// Severity: WARN in Phase 02 (cosmetic — agents read the graph, not the
    /// field, for `ready`). Phase 04 escalates to FAIL when the Materialised
    /// Fast Path makes the field correctness-critical.
    StaleStatus {
        /// The issue exhibiting drift.
        issue_id: QualifiedId,
        /// The graph-computed Status (source of truth).
        expected: Status,
        /// The value currently stored in the Projects V2 Status field.
        actual: Status,
    },
}
```

### 8.2 Detection algorithm

Pseudocode (normative):

```text
fn detect_stale_status(graph: &DependencyGraph, issues: &HashMap<QualifiedId, Issue>)
    -> Vec<DriftKind>
{
    let mut drift = Vec::new()

    // Filter invariant (codified by F4 — see §8.4):
    // Only iterate issues that are MEMBERS of the Project V2. The reconcile
    // engine receives `issues` already filtered by the caller.
    for (qid, expected_status) in graph.issue_status() {
        if let Some(issue) = issues.get(qid) {
            if issue.status != *expected_status {
                drift.push(DriftKind::StaleStatus {
                    issue_id: qid.clone(),
                    expected: *expected_status,
                    actual:   issue.status,
                })
            }
        }
        // Issues in `graph` but not in `issues` are out of Project V2 scope:
        // already filtered upstream — the loop body is a no-op for them.
    }

    drift
}
```

**Filter invariant:** issues that are not Project V2 members MUST be filtered
**before** detection. The current reconcile pipeline (`unblock-mcp::tools::reconcile::reconcile_handler`)
already constructs `issues` only from project items; this spec does not change
that, but it pins the invariant by F4 (§8.4).

### 8.3 Repair routine

```text
fn repair_stale_status(client: &dyn GitHubClient, drift: &DriftKind, project: &ProjectInfo)
    -> Result<(), Error>
{
    if let DriftKind::StaleStatus { issue_id, expected, .. } = drift {
        let item_id  = project.item_id_for(issue_id)?
        let field_id = project.field_ids.status
        let option   = project.field_ids.status_option_for(*expected)?
        client.update_field(project.id, item_id, field_id,
                            FieldValue::SingleSelectOption(option)).await
    }
}
```

**Idempotency:** running repair twice is safe — the second call writes the same
value. Verified by §8.4 fixture F1 / F2 round-trip test.

### 8.4 Test fixtures (F1–F4) — LOCKED

All four fixtures use the **existing** `MockGitHubClient` (RG-9 confirmed).

| Fixture | State | Member of Project V2? | Expected (graph) | Actual (field) | Assertion |
|---|---|---|---|---|---|
| **F1** (positive) | Closed but field says InProgress | Yes | `Closed` | `InProgress` | `drift.contains(StaleStatus { qid, Closed, InProgress })` |
| **F2** (positive) | Open with blocker, field says Ready | Yes | `Blocked` | `Ready` | `drift.contains(StaleStatus { qid, Blocked, Ready })` |
| **F3** (happy path) | Open + ready, field says Ready | Yes | `Ready` | `Ready` | `drift.iter().none(StaleStatus for qid)` |
| **F4** (negative filter) | Issue exists in repo, NOT in Project V2 | **No** | n/a (filtered out) | n/a (filtered out) | `drift.iter().none(StaleStatus for qid)` |

**F4 filter invariant (codified):** the test constructs an `Issue` whose
`projectItems.fieldValues` does NOT include the Project V2 of interest, asserts that
the reconcile pipeline filters it out **before** the detection loop runs, and asserts
no `StaleStatus` is emitted. This is a **negative filter test** — distinct from F3
which is a **positive happy path** confirming detection logic on member-with-matching-
state.

**Repair round-trip (extension of F1, F2):**

1. Run `reconcile --fix`.
2. Assert `update_project_field` was called once per drift with the expected option.
3. Re-run `reconcile` (no fix).
4. Assert `drift.is_empty()`.

### 8.5 Severity classification

The engine emits drift as a flat `Vec<DriftKind>`. Severity is a **tool-output**
concern (in `unblock-mcp::tools::reconcile`):

```rust
fn severity_for(kind: &DriftKind) -> &'static str {
    match kind {
        DriftKind::CycleDetected { .. }       => "FAIL",
        DriftKind::OrphanedBlockingEdge { .. } => "FAIL",
        DriftKind::MissingProjectField { .. } => "FAIL",
        DriftKind::StaleStatus { .. }         => "WARN",   // Phase 02
        DriftKind::UncascadedClosure { .. }   => "WARN",
        DriftKind::MalformedAgentField { .. } => "WARN",
        DriftKind::StaleClaim { .. }          => "WARN",
    }
}
```

**Phase 04 hook:** `StaleStatus` severity is the only line that changes when
Phase 04 lands. The function lives in `unblock-mcp::tools::reconcile` (not in
`unblock-core`), so the severity escalation is a binary-crate-only edit.

---

## 9. `doctor` MCP Tool

### 9.1 Tool registration

Registered in `unblock-mcp::tools::mod` alongside the existing 11 tools. Follows
the same `JsonSchema` derivation pattern as `reconcile`.

### 9.2 Input schema

```rust
// crates/unblock-mcp/src/tools/doctor.rs

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DoctorParams {
    /// If `true`, attempt safe auto-repairs (cache invalidation, missing labels via
    /// `setup`, drift via `reconcile --fix`, config reload).
    #[serde(default)]
    pub fix: bool,

    /// If `true`, also run drift detection by delegating to `reconcile` (no fix).
    /// Combine with `fix: true` to repair drift.
    #[serde(default)]
    pub with_drift: bool,
}
```

### 9.3 Output schema

```json
{
  "overall_status": "HEALTHY" | "DEGRADED" | "UNHEALTHY",
  "checks": [
    {
      "name": "github.connectivity",
      "category": "connectivity" | "config" | "state",
      "status": "OK" | "WARN" | "FAIL",
      "detail": "<human-readable diagnostic>"
    }
  ],
  "metrics_snapshot": { /* full MetricsSnapshot from §6.4 */ },
  "resilience_snapshot": {
    "breaker": { "state": "Closed" | "Open" | "HalfOpen", "failure_count": 0,
                 "last_failure_at": "2026-04-29T..." | null },
    "retry":   { "max_attempts": 5, "deadline_secs": 30, /* ... */ }
  },
  "repairs_attempted": [
    { "name": "<repair-name>", "outcome": "ok" | "failed", "detail": "..." }
  ],
  "human_actions_needed": [
    { "issue": "<problem>", "remediation": "<what the operator must do>" }
  ]
}
```

### 9.4 Check catalogue

| Check name | Category | OK | WARN | FAIL |
|---|---|---|---|---|
| `github.connectivity` | connectivity | `GET /user` returns 200 | non-2xx but not auth-class | network unreachable, DNS fail |
| `github.token` | config | token valid | scopes missing for write tools | token expired / 401 |
| `github.project` | config | configured project resolvable | (n/a) | `Error::ProjectNotConfigured` or 404 |
| `env.required_vars` | config | `GITHUB_TOKEN` set, valid | (n/a) | missing or empty |
| `state.cache` | state | cache age fresh OR ttl-respecting | cache age > 5 min | (n/a — cache absence is OK) |
| `state.breaker` | state | `Closed` | `HalfOpen` | `Open` |
| `state.drift` (if `with_drift`) | state | drift count = 0 | only WARN drifts | any FAIL drift |

**`overall_status` derivation (LOCKED — Decision L3.7):**

```text
if any FAIL in {connectivity, config}:           UNHEALTHY
else if any WARN OR (drift detected without --fix): DEGRADED
else:                                              HEALTHY
```

`state.breaker == Open` produces a **state.FAIL**, not connectivity.FAIL — the
breaker is server-internal state. This means an open breaker → DEGRADED unless
the underlying connectivity check also FAILs.

> **Spec note (SPEC-ORIGINAL):** plan §2.3 mapping says "UNHEALTHY ⇐ connectivity/config
> FAIL". This spec interprets `state.breaker == Open` as `state.FAIL`, which
> produces DEGRADED (not UNHEALTHY) in isolation. Rationale: a breaker can open due
> to a transient outage that has since resolved; the connectivity check is the
> authoritative live signal. **User must approve.**

### 9.5 `--fix` repair catalogue

| Repair | Trigger | Action | Failure handling |
|---|---|---|---|
| `cache.invalidate` | `state.cache` WARN (stale) | `GraphCache::invalidate()` | "ok" — cannot fail |
| `setup.repair_labels` | `state.config` WARN, missing labels | delegate to existing `setup` tool | propagate failure to `repairs_attempted[].outcome` |
| `reconcile.fix` | `with_drift && fix` | invoke `reconcile { fix: true }` | propagate |
| `config.reload` | env var changed (best-effort) | re-construct `Config` | propagate |

**Idempotency:** every repair is idempotent. Re-running `doctor --fix` on a healthy
system produces an empty `repairs_attempted` list (skipped because trigger
conditions are not met).

### 9.6 Read-only invocation invariant

`doctor` (no flags) MUST NOT mutate any GitHub state. Verified by integration test:
run `doctor` against a `MockGitHubClient`, assert `mock.calls.update_field == 0`,
`mock.calls.add_comment == 0`, etc.

### 9.7 `human_actions_needed` rules

Populated for failures whose remediation requires a human:

- token expired / invalid → "Refresh `GITHUB_TOKEN`; current token expired at <date>".
- GitHub Project deleted → "Re-create project or update `UNBLOCK_PROJECT_NUMBER`".
- network unreachable → "Check VPN / firewall connectivity to api.github.com".
- env var missing → "Set `GITHUB_TOKEN` in environment".

The list is **structured** (not free text) so downstream agents can pattern-match.

---

## 10. `commit_context` MCP Tool

### 10.1 Tool registration

Registered alongside `doctor`. Generate-only — no parsing of pre-existing messages
(LOCKED — Decision L4.1).

### 10.2 Input schema

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CommitContextParams {
    /// Optional list of additional GitHub issue URLs to emit as `Refs:` trailers.
    /// Each value MUST be a `https://github.com/<owner>/<repo>/issues/<n>` URL.
    #[serde(default)]
    pub refs: Vec<String>,

    /// If `true`, inspect the working-tree diff to suggest a scope, an `API:`
    /// line, and a `BREAKING CHANGE:` footer when applicable.
    #[serde(default)]
    pub with_changes: bool,
}
```

**No** positional bd reference (RG-7/RG-8 dropped). The active GitHub claim is
read from the unblock-mcp graph cache directly.

### 10.3 Output schema

```json
{
  "subject_template": "feat(scope): <imperative summary>",
  "body_template":    "<paragraph(s) describing what and why>",
  "trailers": [
    { "key": "Closes", "value": "https://github.com/owner/repo/issues/42" },
    { "key": "Refs",   "value": "https://github.com/owner/repo/issues/41" },
    { "key": "Spec",   "value": "docs/specs/02-spec-mcp-complete.md#section-10" },
    { "key": "Plan",   "value": "docs/plans/02-plan-mcp-complete.md" },
    { "key": "Phase",  "value": "02" }
  ],
  "formatted": "<ready-to-paste full commit message>",
  "warnings": [ "<string>" ]
}
```

### 10.4 Git library — `gix` (LOCKED — RG-3)

```toml
# crates/unblock-mcp/Cargo.toml
gix = { version = "0.81", default-features = false,
        features = ["status", "index", "blocking-network-client"] }
```

The `blocking-network-client` feature is **disabled in default builds** —
`commit_context` does NOT make network calls (the GitHub side is via
`unblock-github`). It is listed in case future tools need it; for Phase 02 the
crate-level features used are exclusively `status` + `index`.

**Calls used:**

| Need | `gix` API |
|---|---|
| Open repo from cwd | `gix::discover(".")` |
| Read `user.name` / `user.email` | `Repository::config_snapshot()` then `.string("user.name")` etc. |
| Working-tree status (modified / added / deleted) | `Repository::status(...)` (feature-gated) |

**No write operations.** `commit_context` does **not** make commits — it produces a
message string for the agent to use with `git commit`.

### 10.5 Active-claim resolution

```text
fn resolve_active_claim(server_state: &ServerState) -> Result<QualifiedId, Error> {
    // The MCP server tracks the agent's active claim via the existing
    // `claim` / `release` tools in the graph cache.
    server_state.graph_cache.active_claim()
        .ok_or(Error::NoActiveClaim)
}
```

Once `qid: QualifiedId` is resolved, the `Closes:` URL is constructed directly:

```text
format!("https://github.com/{}/{}/issues/{}", qid.owner, qid.repo, qid.number)
```

**No bd indirection. No external tracker lookup.**

### 10.6 Subject-template builder (LOCKED — Decision L4.4)

GitHub issue type → Conventional Commits prefix:

| GitHub issue type | CC prefix | Notes |
|---|---|---|
| `feature` | `feat` | |
| `bug` | `fix` | |
| `task` | `chore` | agent free to upgrade to `refactor`/`docs`/`test` |
| `epic` | n/a — tool **errors** | epics are not directly committable |
| `chore` | `chore` | |
| `spike` | `chore` | |

Scope detection (without `--with-changes`):

```text
scope = qid.repo                                       // owner/repo → "repo"
subject_template = format!("{prefix}({scope}): TODO short imperative summary")
```

With `--with-changes`, scope is refined from the diff (see §10.10).

### 10.7 Trailer collector — STABLE 5-trailer contract

Per Decision L4.3, the **5 canonical trailers** are emitted under the rules in the
table below. The vocabulary is **EXTENSIBLE** — future phases may add new trailer
keys; the parser MUST round-trip unknown keys.

| Trailer | When emitted | Source | Omission rule |
|---|---|---|---|
| `Closes` | Always when active claim exists | `qid` from graph cache | Omit only if no active claim AND tool was invoked anyway (returns `Error::NoActiveClaim`). |
| `Refs` | When `params.refs` non-empty | Input | Repeated trailer key allowed. |
| `Spec` | When issue body links a spec | parse issue body for `docs/specs/...md` | Omit if no spec link found. |
| `Plan` | When issue body links a plan | parse issue body for `docs/plans/...md` | Omit if no plan link found. |
| `Phase` | When `.unblock/commit_context.toml` declares it | repo config | Omit silently if config absent or `phase` key missing. |

**Issue body parsing for `Spec:` / `Plan:`:**

Regex (normative): `(?m)^\s*(docs/(?:specs|plans)/[A-Za-z0-9._/-]+\.md(?:#[A-Za-z0-9_-]+)?)\s*$`

- Matches a markdown-relative path on its own line, optional `#anchor`.
- First match wins per category; multiple matches → log a `warning` in the output
  array (`"multiple Spec links in issue body — using first"`).

**`.unblock/commit_context.toml` schema:**

```toml
# repo-root .unblock/commit_context.toml
phase = "02"          # opt-in; if present, emitted as Phase: trailer
```

### 10.8 Trailer parser — round-trip discipline (Decision L4.3 stable contract)

A trailer parser lives in `commit_context::trailers` for the `--with-changes`
round-trip path and any future validation tooling. The parser:

- Accepts any `Key: value` line at the end of a commit message (RFC 5322-ish, as
  understood by `git interpret-trailers`).
- **MUST NOT** reject unknown keys.
- **MUST** preserve original key spelling and value content on round-trip.
- **MUST NOT** normalise whitespace beyond trimming end-of-line.

**Acceptance test:** generate a commit message including `Closes`, `Investigation`
(unknown key), `Verdict` (unknown key); parse; emit; assert byte-equal except
end-of-line normalisation.

### 10.9 `formatted` field

```text
<subject_template>
<blank line>
<body_template>
<blank line>
Closes: https://github.com/.../issues/42
Refs: https://github.com/.../issues/41
Spec: docs/specs/02-spec-mcp-complete.md#section-10
Plan: docs/plans/02-plan-mcp-complete.md
Phase: 02
```

The format MUST round-trip through `git interpret-trailers --parse` — verified by
acceptance test (§19.4).

### 10.10 `--with-changes` path

Inspect working-tree status via `gix`:

| Detection | Output |
|---|---|
| Single modified file in `crates/<X>/` | `scope = X` |
| Modified files in multiple crates | `scope = "workspace"`; `warnings.push("multi-crate change")` |
| `pub fn` / `pub struct` / `pub enum` signature changed in a library crate | `body_template` includes `API: <signature change description>` line |
| Removed item from public API | `warnings.push("BREAKING CHANGE candidate")`; `body_template` includes `BREAKING CHANGE: <description>` footer |

**Implementation detail:** the API-change detection for Phase 02 is **best-effort**
based on textual diff inspection (`gix` provides the file list and per-file
patches). A full AST-based detection is out of scope (Phase 03 will provide the
indexer that makes this trivial).

### 10.11 Error paths

| Condition | Tool behaviour |
|---|---|
| No active claim | Return structured error: `Error::NoActiveClaim` ("agent has no active GitHub issue claim — call `claim` first") |
| GitHub issue type is `epic` | Return structured error: `Error::EpicNotCommittable` ("epic-type issues are not directly committable; commit against a child task") |
| `gix::discover(".")` fails (not a git repo) | Return structured error: `Error::NotAGitRepo` |
| `params.refs[i]` not a valid GitHub issue URL | Return structured validation error before doing any work |

All error variants live on a new module-local `enum CommitContextError` with
`#[non_exhaustive]`, mapped to MCP `ErrorCode` via the existing
`unblock-mcp::errors` translation layer.

---

## 11. `#[non_exhaustive]` Project-Wide Policy

**Scope (LOCKED — plan §2.6):** every public enum in a library crate that is
expected to grow over time MUST carry `#[non_exhaustive]`. This applies to:

| Crate | Enum | Status post-Phase 02 |
|---|---|---|
| `unblock-core` | `DomainError` | Already applied (`unblock-29p.70`) |
| `unblock-core` | `DriftKind` | **Apply in Phase 02** (Epic 02.E) |
| `unblock-github` | `Error` | Already applied (`unblock-29p.70`) |
| `unblock-resilience` | `BreakerState` | Apply at introduction (this spec §4.5) |
| `unblock-resilience` | `ResilienceError<E>` | Apply at introduction (this spec §4.4) |

**Downstream impact audit (Epic 02.E task — RG-10 closed):**

```bash
# Verify match-without-wildcard sites against DriftKind:
rg -t rust 'match.*DriftKind' crates/
```

Every match site found MUST have a `_ =>` wildcard arm OR be expanded to cover all
7 variants. Compile-time gated — `cargo build` after `#[non_exhaustive]` addition
catches non-exhaustive matches that lack the wildcard.

**Binary crate (`unblock-mcp`) exemption:** the policy applies in **library** crates
because semver only matters there. `unblock-mcp` enums may be exhaustive, but the
discipline is recommended.

**CLAUDE.md update:** the `#[non_exhaustive]` bullet was already added to
"Coding Standards" at plan APPROVED time (§13.5). No further doc work needed for
this policy.

---

## 12. Configuration & Environment Variables

### 12.1 New environment variables

| Variable | Type | Default | Read by | Notes |
|---|---|---|---|---|
| `UNBLOCK_RETRY_MAX_ATTEMPTS` | `u32` | `5` | `unblock-resilience::RetryPolicy::from_env` | parse failure → warn + default |
| `UNBLOCK_RETRY_DEADLINE_SECS` | `u64` | `30` | same | parse failure → warn + default |

### 12.2 New repo-local config file

| Path | Format | Purpose |
|---|---|---|
| `.unblock/commit_context.toml` | TOML | Optional repo-level settings for `commit_context` (currently: `phase = "02"`) |

The directory `.unblock/` is **created on demand** — it is NOT created by `setup`
in Phase 02 (would be a feature creep). When the file is absent, `commit_context`
silently omits the `Phase:` trailer.

### 12.3 No changes to existing `Config` struct

`unblock_core::Config` is unchanged in its serialised shape. The new env vars are
read **directly** by `RetryPolicy::from_env` without going through `Config`. This
keeps `Config` focused on GitHub credentials + project info.

> **Spec note (SPEC-ORIGINAL):** plan §9 Epic 02.A Task 5 said "Wire env-var config
> via `Config::load_from`". This spec routes the env vars **directly** through
> `RetryPolicy::from_env`, not through `Config`. Rationale: `unblock-resilience` has
> zero deps on `unblock-core::Config`, and threading them through `Config` would
> create a back-reference. **User must approve.**

---

## 13. Error Model Additions

### 13.1 New variants on existing enums

| Crate | Enum | New variant | When |
|---|---|---|---|
| `unblock-core` | `DriftKind` | `StaleStatus { issue_id, expected, actual }` | Epic 02.E |
| `unblock-resilience` | `ResilienceError<E>` (new enum) | `BreakerOpen`, `RetryExhausted { source }`, `Permanent { source }` | Epic 02.A |
| `unblock-mcp::tools::commit_context` | `CommitContextError` (new enum, module-local) | `NoActiveClaim`, `EpicNotCommittable`, `NotAGitRepo`, `InvalidRefUrl { url }` | Epic 02.D |

### 13.2 Translation at boundaries

| Translation | Where |
|---|---|
| `ResilienceError<unblock_github::Error>` → `unblock_github::Error` | `unblock-github::client` (private function `translate_resilience_error`) — `BreakerOpen` → `Error::CircuitBreakerOpen { since }`; `RetryExhausted { source }` → `source` (the inner GitHub error); `Permanent { source }` → `source`. |
| `unblock_github::Error` → MCP `ErrorCode` | Existing `unblock-mcp::errors::github_error_to_mcp` — extended to handle `CircuitBreakerOpen` (→ `INTERNAL_ERROR`, status 503) and `RateLimited` (→ `INTERNAL_ERROR`, status 429). The variants already exist; only the construction path is new. |
| `CommitContextError` → MCP `ErrorCode` | `unblock-mcp::errors` extension — `NoActiveClaim` → `INVALID_PARAMS`; `EpicNotCommittable` → `INVALID_PARAMS`; `NotAGitRepo` → `INVALID_PARAMS`; `InvalidRefUrl` → `INVALID_PARAMS`. |

### 13.3 `#[non_exhaustive]` discipline applied

All new public enums carry `#[non_exhaustive]` per §11. New variants on existing
enums (`DriftKind`) are additive — `#[non_exhaustive]` is added **at the same time**
the new variant lands.

---

## 14. Observability & Tracing

### 14.1 Spans

Each MCP tool dispatch is wrapped in a `tracing::info_span!("mcp.tool", name=…)`
that captures:

- `name` (tool name)
- `agent_id` (from `SessionMeta`)
- `duration_ms` (recorded at span exit)

The recording into `ServerMetrics::record_tool` happens at span exit via the
existing dispatch wrapper in `unblock-mcp::server`. Single instrumentation point
(plan §9 Epic 02.B Task 3 — confirmed).

### 14.2 Resilience-layer spans

`ResiliencePolicy::execute` emits the following spans / events:

- `tracing::debug!` on every retry attempt: `retry.attempt`, `retry.wait_ms`, `retry.reason`.
- `tracing::warn!` on breaker state transitions (in `Instrument::on_open` / `on_half_open`).
- `tracing::error!` on `RetryExhausted`.

### 14.3 No PII in logs

GitHub tokens MUST NOT appear in any log output. Existing `tracing` configuration
already redacts; this spec mandates a static audit (`rg`) that no
`format!("{token}", …)` or `Display` impl on a token-bearing struct exists in any
new code.

---

## 15. Testing Strategy

### 15.1 Unit tests — `unblock-resilience`

- `RetryPolicy::default` produces correct knobs.
- `RetryPolicy::from_env` reads env, falls back on parse failure (test with
  `temp_env`).
- `Breaker::new` constructs failsafe config with the locked failure policy.
- `Instrument` mirror updates atomics on `on_open` / `on_half_open` / `on_closed`.
- `BreakerSnapshot` reads atomics correctly.
- `IsRetryable` blanket impl tests for the trait contract.

### 15.2 Integration tests — `unblock-resilience`

- **Breaker opens after 5 consecutive failures.** Drive `policy.execute(|| async { Err(transient) })`
  5 times; assert next call returns `BreakerOpen`.
- **Breaker cools down to half-open.** After 5 failures + cooldown, next call is
  admitted (and either re-fails to Open or succeeds to Closed).
- **Retry honours `Retry-After`.** Mock returns `RateLimited { reset_at = now + 5s }`;
  assert the loop sleeps 5 s (not the default backoff).
- **Retry-After cap.** Mock returns `reset_at = now + 60s`; assert immediate
  `RetryExhausted` (fail-fast, no sleep).
- **Deadline beats max-attempts.** Configure 10 attempts but 1 s deadline; assert
  loop exits on deadline before exhausting attempts.
- **Order of operations** (property test). Run a script of N attempts with a mix
  of failures and successes; assert the breaker's failure counter equals the
  number of `execute` calls that **finally** failed (not the per-attempt count).

### 15.3 Unit tests — `unblock-core::metrics`

- `ServerMetrics::new` constructs maps with the expected keys.
- `record_tool` / `record_api` increment counters and record durations.
- Out-of-vocabulary names log warn and are silently dropped.
- `snapshot` produces a coherent `MetricsSnapshot`.
- Forward-compat snapshot keys preserved (§6.5).

### 15.4 Integration tests — `unblock-github`

- Static audit (CI-runnable shell command per §7.3) confirms zero raw HTTP outside
  the resilience boundary.
- `Error::CircuitBreakerOpen` is constructed by a real path: simulate 6 consecutive
  failures via `MockGitHubClient`, assert next call returns `Error::CircuitBreakerOpen`.
- `Error::RateLimited` is constructed: simulate a 429 response with `X-RateLimit-Reset`
  header; assert the variant is returned with the parsed timestamp.

### 15.5 Integration tests — `reconcile` `StaleStatus`

Fixtures F1–F4 per §8.4. Plus an idempotency property test: detection on the same
state twice yields the same drift set (set equality, not vec ordering).

### 15.6 Integration tests — `doctor`

- HEALTHY path: mock returns 200 on `/user`, breaker `Closed`, no drift → assert
  `overall_status == "HEALTHY"`.
- DEGRADED path: breaker `Open` → assert `DEGRADED`.
- UNHEALTHY path: mock returns 401 on `/user` → assert `UNHEALTHY` + `human_actions_needed`
  contains a token-refresh entry.
- Read-only invariant: `doctor` (no flags) → assert no GitHub mutations on mock.
- `--fix` idempotency: run twice; second run produces empty `repairs_attempted`.

### 15.7 Integration tests — `commit_context`

- Active claim → `Closes:` trailer matches the expected URL.
- Issue body contains spec link → `Spec:` trailer present.
- `.unblock/commit_context.toml` declares phase → `Phase:` trailer present; absent
  config → omitted.
- `epic`-type issue → returns `EpicNotCommittable` error.
- No active claim → returns `NoActiveClaim` error.
- `formatted` round-trips through `git interpret-trailers --parse`.
- Trailer parser preserves unknown keys (e.g. `Investigation`, `Verdict`) on
  round-trip.

### 15.8 Property tests

- Resilience composition order (§15.2).
- `StaleStatus` detection idempotency (§15.5).
- Trailer parser round-trip (proptest generator over `Key: value` lines).

---

## 16. Performance Methodology & Gates

### 16.1 Pre-Phase-02 baseline (must be captured first)

Before any Phase 02 instrumentation lands, capture a baseline:

- p50, p90, p99 of `ready` tool call (warm cache) over 100 invocations.
- p50, p90, p99 of `show` tool call (warm cache) over 100 invocations.

Recorded in `target/criterion/phase01-baseline/` and committed to the repo's
`benches/baselines/` directory as a JSON file. This is the **regression baseline**
the post-Phase 02 numbers are compared against.

### 16.2 `ServerMetrics` overhead bench (NR-6.1.x — LOCKED)

**Bench harness (Epic 02.B):** `crates/unblock-core/benches/metrics_overhead.rs`
using `criterion`.

**Methodology:**

| Profile | Description | Gate |
|---|---|---|
| **Sustained 1k rec/s** | 10 threads × 100 records/s for 30 s, per metric | Hard gate: per-call < 500 ns |
| **Burst 10k rec/s** | 10 threads × 1000 records/s for 5 s, per metric | Hard gate: per-call < 5 µs |

**End-to-end p99 gate:** post-metrics p99 of warm-cache `ready` MUST stay below
2 s (existing Phase 01 invariant). **Soft gate:** p99 regression < 5 % vs baseline
(§16.1) is informational; a soft regression > 5 % triggers a `tracing::warn!` in
the bench harness output but does not fail the gate.

### 16.3 Resilience-layer overhead

Bench `crates/unblock-resilience/benches/policy_overhead.rs`:

- `ResiliencePolicy::execute(|| async { Ok(()) })` — happy path overhead.
- Gate: < 10 µs per call uncontested (allows for breaker state read + atomics +
  one async hop).

### 16.4 `commit_context` end-to-end

Bench `crates/unblock-mcp/benches/commit_context.rs`:

- Generate a commit message from a fixture repo (10 modified files).
- Gate: < 50 ms p99.

### 16.5 No regression of existing Phase 01 gates

All existing Phase 01 acceptance gates (warm-cache `ready` < 2 s, etc.) MUST pass
unchanged. The CI runs the existing bench suite plus the Phase 02 additions.

---

## 17. Cross-Phase Contracts

### 17.1 Phase 03 — `unblock-indexer` consumes `unblock-resilience`

Phase 03 spec §20.1 was marked UNRESOLVED pending the Phase 02 decision; the plan
APPROVED-time patch resolved it to "direct dep on `unblock-resilience`". This spec
locks the API surface in §4 — Phase 03 codes against §4 verbatim.

**Acceptance gate (Epic 02.A Task 2 / §19.6):** a smoke-test prototype in
`crates/unblock-indexer/` (or a private test harness if the crate doesn't yet
exist) imports `ResiliencePolicy` and constructs a policy. The smoke test runs in
CI as part of Phase 02's acceptance — not Phase 03's.

### 17.2 Phase 04 — `StaleStatus` severity escalation

Phase 04 changes `severity_for(DriftKind::StaleStatus { .. })` from `"WARN"` to
`"FAIL"` in `unblock-mcp::tools::reconcile`. Hook is a single line; tracked in
the Phase 04 plan.

### 17.3 Phase 06 — OTel adapter wraps `ServerMetrics`

`ServerMetrics` shape is **frozen** after Phase 02. The Phase 06 OTel adapter:

- Reads atomics directly (`ServerMetrics::tool_calls.get(name).load(Relaxed)`).
- Reads histograms via `snapshot()` for p50/p99 export.
- Lives in a new crate (`unblock-otel` or `unblock-mcp-remote`) with a workspace
  dep on `unblock-core`.
- Does NOT modify `ServerMetrics`.

The forward-compat contract test (§6.5) is the gate that protects this invariant.

### 17.4 No backward-compat shims for the commit-message convention break

Decision L4.2 declared the subject-only → trailers convention upgrade a BREAKING
CHANGE under pre-prod stance. No migration tooling is provided. CHANGELOG entry
+ CLAUDE.md "Commit Strategy" subsection update (Epic 02.D) document the change.

---

## 18. Implementation Tasks

Tasks are organised by epic per the plan. Each is implementable from this spec
without re-reading the plan, but bead descriptions MUST reference both this spec
and the plan (per `feedback_bead_description_not_spec`).

### 18.1 Epic 02.A — Resilience Layer → rust-supervisor

1. **Create `crates/unblock-resilience/`** with the module skeleton in §3.1; lock
   `failsafe = "1.3"`, `backoff = { version = "0.4", features = ["tokio"] }`.
2. **Implement `IsRetryable` trait + `ResilienceError<E>`** per §4.3, §4.4.
3. **Implement `Breaker`** per §5.3 with `Instrument` mirror + atomics.
4. **Implement `RetryPolicy`** per §5.4 with `from_env` + default knobs.
5. **Implement `ResiliencePolicy::execute`** per §5.2 (composition + cap + deadline).
6. **Wire `unblock-github::client`**: hold `ResiliencePolicy`, route every HTTP call
   through `policy.execute`, add `resilience_snapshot()` accessor (§7.1).
7. **Implement `IsRetryable for unblock_github::Error`** per §7.2 (CC-5: from
   scratch, NOT wrapping a non-existent helper).
8. **Add `Error::RateLimited` and `Error::CircuitBreakerOpen` construction sites**
   (§7.4) — parse `X-RateLimit-Reset` on 429; translate `ResilienceError::BreakerOpen`
   at the boundary.
9. **Static-audit gate** in CI (§7.3) — `rg` for raw `self.http.*` outside the
   policy boundary.
10. **Phase 03 smoke-test prototype** — stub `unblock-indexer` consumer compiles
    against `ResiliencePolicy` (§17.1).
11. **Unit + integration tests** per §15.1, §15.2.

### 18.2 Epic 02.B — `ServerMetrics` → rust-supervisor

1. **Create `crates/unblock-core/src/metrics.rs`** with the module skeleton in §6.1
   and add `hdrhistogram = "7"` workspace dep.
2. **Implement `ServerMetrics`** per §6.2, §6.3.
3. **Implement `MetricsSnapshot` + `snapshot()`** per §6.4.
4. **Wire tool-dispatch instrumentation** in `unblock-mcp::server` (single span
   wrapper per §14.1).
5. **Wire API-call instrumentation** in `unblock-github::client` (per §7.5 — at
   each HTTP call site, NOT inside `unblock-resilience`).
6. **Wire cache events** to `record_cache_*` — `GraphCache` already emits the
   appropriate hooks; thread `Arc<ServerMetrics>` to the cache.
7. **Wire graph gauges** to `set_graph_size` — invoked after every cache rebuild.
8. **Forward-compat contract test** per §6.5.
9. **Bench harness** per §16.2.
10. **Unit tests** per §15.3.

### 18.3 Epic 02.C — `doctor` Tool → rust-supervisor

1. **Tool registration** in `unblock-mcp::tools::mod`.
2. **Schema** per §9.2, §9.3.
3. **Check catalogue** per §9.4 — implement each check.
4. **`overall_status` derivation** per §9.4.
5. **`--fix` repair catalogue** per §9.5 — delegate to `setup` and `reconcile`.
6. **`human_actions_needed` rules** per §9.7.
7. **Read-only invariant audit** (§9.6).
8. **Integration tests** per §15.6.
9. **README.md update** — `doctor` entry.

### 18.4 Epic 02.D — `commit_context` Tool → rust-supervisor

1. **Add `gix` dep** with the locked features (§10.4).
2. **Tool registration** in `unblock-mcp::tools::mod`.
3. **Schema** per §10.2, §10.3.
4. **Active-claim resolver** per §10.5.
5. **Subject-template builder** per §10.6.
6. **Trailer collector** per §10.7 (5 canonical trailers, omission rules).
7. **Trailer parser** per §10.8 (round-trip discipline).
8. **`formatted` field assembly** per §10.9.
9. **`--with-changes` path** per §10.10 (gix status + heuristics).
10. **Error paths** per §10.11.
11. **Integration tests** per §15.7.
12. **README.md update** — `commit_context` entry.
13. **CLAUDE.md "Commit Strategy" subsection** — trailer convention.
14. **CHANGELOG.md entry** — BREAKING CHANGE notice.

### 18.5 Epic 02.E — `StaleStatus` Drift → rust-supervisor

1. **Add `DriftKind::StaleStatus` variant** per §8.1.
2. **Apply `#[non_exhaustive]` to `DriftKind`** per §11. Audit downstream `match`
   sites; add `_ =>` arms or extend matches.
3. **Detection routine** per §8.2 in `ReconcileEngine`.
4. **Repair routine** per §8.3.
5. **Severity classification** per §8.5 (in `unblock-mcp::tools::reconcile`).
6. **Fixtures F1–F4** per §8.4 + repair round-trip + idempotency property test
   (§15.5, §15.8).
7. **Update `reconcile` MCP tool description** to enumerate all 7 drift types.

### 18.6 Epic 02.F — Documentation & Phase 03 Cross-Link → docs / Ada

1. **README.md** — add `doctor`, `commit_context`, `UNBLOCK_RETRY_*` env vars.
2. **CLAUDE.md "Commit Strategy"** — trailer convention subsection (depends on
   Epic 02.D shipping).
3. **CHANGELOG.md** — Phase 02 entry.
4. **Phase 03 spec §20.1 cross-link verification** — re-read, confirm UNRESOLVED
   → RESOLVED transition reads cleanly post-merge.
5. **Forward-compat contract test verification** — confirm §6.5 test exists and
   passes.

---

## 19. Acceptance Criteria

### 19.1 Resilience layer

- [ ] `crates/unblock-resilience/` exists with the public surface in §4. `cargo doc -p unblock-resilience` produces zero warnings.
- [ ] `cargo tree -p unblock-resilience` shows zero deps on other unblock crates.
- [ ] Static audit (§7.3) reports zero raw HTTP calls outside the policy boundary in `unblock-github`.
- [ ] All §15.2 integration tests pass.
- [ ] Composition order property test passes (§15.2 last bullet, §15.8).
- [ ] `Error::CircuitBreakerOpen` and `Error::RateLimited` are constructed by real code paths (§15.4).

### 19.2 Observability

- [ ] `ServerMetrics` lives at `crates/unblock-core/src/metrics.rs`. `cargo doc --no-deps -p unblock-core` shows no MCP-specific types in the module.
- [ ] Histogram bounds match §6.2 defaults (`tool_durations`: 1µs–60s, sigfig 3; `api_durations`: 1ms–60s, sigfig 3).
- [ ] Per-call overhead < 500 ns sustained 1k/s; < 5 µs burst 10k/s (§16.2).
- [ ] p99 of warm-cache `ready` < 2 s post-metrics (hard gate).
- [ ] p99 regression vs Phase 01 baseline < 5 % (soft gate; warn, do not fail).
- [ ] Forward-compat contract test (§6.5) passes.
- [ ] Tool-dispatch and API-call instrumentation each have **single** wiring points (audit by `rg`).

### 19.3 `doctor` tool

- [ ] All 4 invocation modes (`doctor`, `doctor --fix`, `doctor --with-drift`, `doctor --fix --with-drift`) produce the JSON shape of §9.3.
- [ ] `overall_status` derivation matches §9.4 mapping in tests.
- [ ] Read-only invocation never mutates GitHub state (mock-asserted, §15.6).
- [ ] `human_actions_needed` populated for token / project / network failures.
- [ ] `--fix` is idempotent — second run on a healthy system produces empty `repairs_attempted`.

### 19.4 `commit_context` tool

- [ ] All 5 trailers emitted per §10.7 with the right omission rules.
- [ ] Subject template uses correct CC prefix per §10.6 (GitHub issue type-driven).
- [ ] `--with-changes` produces accurate scope, suggests `API:` line and `BREAKING CHANGE:` footer when applicable.
- [ ] GitHub issue type `epic` is rejected with `EpicNotCommittable`.
- [ ] `formatted` field round-trips through `git interpret-trailers --parse`.
- [ ] Trailer parser preserves unknown keys on round-trip (§15.7, §15.8).

### 19.5 `StaleStatus` reconcile

- [ ] `DriftKind` has 7 variants; `StaleStatus` is the new one.
- [ ] `#[non_exhaustive]` applied to `DriftKind`; downstream `match` sites compile clean.
- [ ] Fixtures F1, F2 (positive) — drift report contains `StaleStatus`.
- [ ] Fixture F3 (happy path) — drift report contains no `StaleStatus`.
- [ ] Fixture F4 (negative filter) — drift report contains no `StaleStatus`; the filter invariant is enforced before detection (§8.2).
- [ ] Repair via `update_project_field` is idempotent (round-trip yields zero drift on second pass).
- [ ] Severity in Phase 02 is `"WARN"`.

### 19.6 Cross-phase contracts

- [x] Phase 03 spec §20.1 transitioned UNRESOLVED → RESOLVED at plan APPROVED time.
- [ ] Phase 03 smoke-test prototype compiles against the Phase 02 `unblock-resilience` public surface (§17.1).
- [ ] `ServerMetrics` shape is frozen — forward-compat test (§6.5) passes.

### 19.7 Quality gates

- [ ] `cargo fmt --check --all` clean.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] `cargo test --workspace` passes.
- [ ] `cargo doc --no-deps --workspace` zero warnings.
- [ ] Coverage ≥ 80 % for new resilience and metrics modules.
- [ ] CI matrix: ubuntu-latest, macos-latest pass (Windows / musl in Phase 04).

### 19.8 Documentation

- [ ] README.md updated with `doctor`, `commit_context`, `UNBLOCK_RETRY_*`.
- [ ] CLAUDE.md "Commit Strategy" updated with trailer convention (post-Epic 02.D).
- [ ] CHANGELOG.md Phase 02 entry includes BREAKING CHANGE marker for commit convention.
- [ ] Phase 03 spec §20.1 cross-link reads cleanly.

---

## 20. Open Items & Forward References

### 20.1 Spec-original decisions requiring user approval

| ID | Topic | Spec section | Plan/research deviation |
|---|---|---|---|
| **SO-1** | `BreakerState` flat-enum + separate `last_failure_at` on snapshot | §4.5 | Plan §6.3 had `Open { since: Instant }` struct variant; spec splits it for serializability. |
| **SO-2** | API-call instrumentation lives in `unblock-github`, NOT inside `unblock-resilience::execute` | §7.5 | Plan §9 Epic 02.B Task 4 said "wire into `ResiliencePolicy::execute`"; spec moves out to preserve crate orthogonality. |
| **SO-3** | Open breaker → `state.FAIL` (DEGRADED), not connectivity FAIL (UNHEALTHY) | §9.4 | Plan §2.3 was ambiguous; spec disambiguates. |
| **SO-4** | `UNBLOCK_RETRY_*` env vars read directly by `RetryPolicy::from_env`, not via `Config::load_from` | §12.3 | Plan §9 Epic 02.A Task 5 routed via `Config`; spec routes direct to avoid back-reference. |

**Status: ALL FOUR APPROVED by user on 2026-04-29.** SO-1 (BreakerState flat enum + last_failure_at on snapshot — Instant serialization fix), SO-2 (API metrics in unblock-github call sites — preserves crate orthogonality), SO-3 (open breaker → DEGRADED, not UNHEALTHY — semantically correct for transient state), SO-4 (RetryPolicy::from_env reads env directly — preserves no back-references from unblock-resilience to unblock-core). All four are recorded as binding spec decisions and the spec is now APPROVED.

### 20.2 Deferred to Phase 04

- `StaleStatus` severity escalation WARN → FAIL (Materialised Fast Path makes the
  Status field correctness-critical).
- cargo-dist cross-platform binaries — Windows / musl will exercise the `gix`
  pure-Rust dep that this spec selected (RG-3 mitigation).

### 20.3 Deferred to Phase 06+

- OpenTelemetry adapter wrapping `ServerMetrics` (§17.3).
- `backon` migration evaluation — trigger conditions: `backoff` last release > 24 mo,
  security advisory, `tokio` 2.0 incompat (CC-4).
- Watchdog auto-trigger for `doctor` (Decision L3.5).
- Multi-tenant breaker scope (Decision L1.3 documents the per-process scope).

### 20.4 Spec-time risks

| Risk | Mitigation |
|---|---|
| `failsafe::Instrument` callback fires under internal mutex; misuse blocks the breaker | Spec §5.3 mandates atomics-only; CI lint via `clippy::await_holding_lock` (already enabled). |
| Histogram `record` returns out-of-range error on extreme outliers | Spec §6.3 mandates pre-record saturation + warn log. |
| `gix` "initial development" tier instability | Spec §10.4 narrows the dep surface to `status` + `index` features only; pinned to 0.81.x. |
| `commit_context` API-change detection is textual, not AST-based | Acceptable for Phase 02 — Phase 03 indexer will provide the AST capability for a future revision. |

### 20.5 Forward references

- **Phase 03 spec §20.1** — cross-link verification at Phase 02 merge.
- **Phase 04 plan** — must include the `StaleStatus` severity-escalation task.
- **Phase 06 plan** — must include the OTel adapter task that consumes the frozen
  `ServerMetrics` shape.

---

**End of Spec 02.**

Status: **APPROVED** — SO-1 through SO-4 confirmed by user on 2026-04-29.
