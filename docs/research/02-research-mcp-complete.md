# Research 02 — MCP Complete (Phase 02)

> Phase: 02
> Author: Smith (feasibility research)
> Date: 2026-04-28
> Source plan: [02-plan-mcp-complete.md](../plans/02-plan-mcp-complete.md)
> Source PRD: [PRD §7 Phase 02](../PRD.md)
> Status: Findings ready for `/spec` consumption

---

## Summary

| # | Status | One-line takeaway |
|---|---|---|
| RG-1 | **CONFIRMED — recommend `unblock-core::metrics`** | `ServerMetrics` is naturally pure (atomics + histograms + read-only snapshot). Placing it in `unblock-core` is preferable: enables Phase 06 OTel adapter to live in `unblock-core` (or in a new `unblock-otel` adapter crate) and gives Phase 03 a direct dep path without going through the `unblock-mcp` binary crate. |
| RG-3 | **CONFIRMED — recommend `gix` for the read-only commit_context surface** | `gix` is pure-Rust, MIT/Apache-2.0, latest 0.81.0. Its `config_snapshot()` (read user.name/email) and `status()` (working-tree status) cover everything `commit_context --with-changes` needs. `git2` (libgit2 binding) requires a C toolchain and `vendored-libgit2` for static cross-target builds — risk for Phase 04 cargo-dist (Windows / musl). Caveat: gix `gix` crate is "Initial Development" stability tier; only `gix-lock` and `gix-tempfile` are Tier 1. The narrow read-only surface in `commit_context` is well-covered by stable subcrate APIs. |
| RG-4 | **PARTIALLY CONFIRMED — `failsafe` works for execution, but lacks public state inspection** | `failsafe` 1.3.0 has working tokio-compatible `futures::CircuitBreaker::call(future)` and an `Instrument` trait (`on_open`, `on_half_open`, `on_closed`, `on_call_rejected`) that is the supported path to populate `BreakerSnapshot { state, last_failure_at }`. The `StateMachine` does NOT expose a public `state()` getter — only `is_call_permitted()`. This means `BreakerSnapshot` MUST be populated by an `Instrument` impl that mirrors state transitions into our own atomics; it cannot be polled from `failsafe` directly. **Action: §6.3's `BreakerSnapshot` API stays valid, but its implementation is mirror-via-Instrument, not direct query.** Decision L1.1 does NOT need to reopen. |
| RG-5 | **CONFIRMED — `backoff` 0.4 works, but the crate is unmaintained** | `backoff::future::retry` is tokio-compatible, supports `ExponentialBackoff` with jitter (`randomization_factor`, `multiplier`, `max_interval`, `max_elapsed_time`), and `Error::Transient { err, retry_after: Option<Duration> }` is the documented mechanism for honouring `Retry-After` (cited in upstream docs as "useful for 429 errors"). HOWEVER: the crate has 17 open issues, last release 0.4.0 dates from 2022, an open community fork question from May 2023, and a transitive `instant` dep is itself unmaintained. **Plan adoption acceptable for Phase 02 (functionality is sufficient); flag a needs-review item to evaluate `backon` as a successor in Phase 06 if the maintenance situation worsens.** |
| RG-6 | **PARTIALLY CONFIRMED — `hdrhistogram` has NO atomic/concurrent variant** | `Histogram<T>` records in 3-6 ns under single-thread, but `AtomicHistogram` / `ConcurrentHistogram` are explicitly listed by upstream as "not yet implemented" (HdrHistogram_rust README). The only concurrent option is `SyncHistogram` + per-thread `Recorder` + a synchronisation phase via `refresh()`. This **does not match** the plan's data shape: `tool_durations: HashMap<&'static str, Histogram>`. The plan implicitly assumes a single shared histogram per metric callable from any task — that requires `Arc<Mutex<Histogram>>` (lock contention) OR a sharded recorder pattern. **Action: §2.2 `ServerMetrics` shape needs spec-time clarification on the concurrency primitive (Mutex<Histogram> per metric vs sharded recorders); the plan's <1µs per-call overhead target is reachable with `Arc<Mutex<Histogram>>` at expected MCP call rates (sub-100/s) but NOT at the 10k calls/s bench load the RG specifies. Bench gate must therefore use realistic load, not the inflated 10k/s figure.** |
| ~~RG-7~~ | **DROPPED — scope error.** Conflated bd (our internal dev PM tool) with unblock product runtime. `commit_context` resolves `Closes:` directly from the active GitHub issue claim in the unblock-mcp graph cache; no bd indirection. See dropped section below. | — |
| ~~RG-8~~ | **DROPPED — scope error.** Same root cause as RG-7. `Phase:` trailer is opt-in via repo config (`.unblock/commit_context.toml`), not derived from any external tracker. | — |
| RG-9 | **CONFIRMED — existing `MockGitHubClient` already supports the F1–F4 fixture pattern** | `MockGitHubClient` already exposes `push_update_field` and `update_field` (verified at `crates/unblock-github/src/mock.rs:417, 539`). The reconcile engine reads the Status field via `fetch_graph_data` (which already returns `ProjectV2ItemFieldSingleSelectValue.name` per `crates/unblock-github/src/graphql.rs:189-193`), so F1–F4 are constructible with `push_fetch_graph_data` + the existing `Issue` fixture builder. **No new mock methods required** — F4 (None field state) is already representable by an `Issue` whose `projectItems` returns no Status field value, which the existing GraphQL parser already handles. Mock extension PR not needed; pattern reuses `update_project_field` test idiom. |

**Recommendation:** **Proceed to spec authoring with one needs-review item locked in:**
1. **RG-6** — concurrency primitive for `ServerMetrics::*_durations` must be pinned at spec time (Mutex<Histogram> per metric is the simplest viable option; bench gate uses realistic call rate, not the unrealistic 10k/s figure currently in the plan).

RG-7 and RG-8 are dropped (scope error — see sections below). RG-2 and RG-10 closed by user decisions during /plan iteration.

No locked decision in plan §4 is unworkable. RG-4's `BreakerSnapshot` requires a thin Instrument-mirror pattern (documented below) but the public surface in §6.3 stays as written.

---

## RG-1 — `ServerMetrics` placement

**Validated finding.** The struct shape in plan §2.2 is

```rust
pub struct ServerMetrics {
    tool_calls: HashMap<&'static str, AtomicU64>,
    tool_durations: HashMap<&'static str, Histogram>,
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

This struct has **zero MCP-specific types**. It depends only on `std::sync::atomic`, `std::collections::HashMap`, and `hdrhistogram::Histogram`. There is no `rmcp::*`, no `schemars::JsonSchema` requirement, no MCP transport coupling. It is therefore a **pure crate citizen** of `unblock-core`.

**Recommended decision.** Place `ServerMetrics` in `crates/unblock-core/src/metrics.rs`. Add `hdrhistogram` to `unblock-core/Cargo.toml` (workspace dependency).

**Rationale.**
1. **Phase 06 OTel adapter** (Decision L2.2) wraps the same struct. If `ServerMetrics` lives in `unblock-mcp`, the OTel adapter (likely a new `unblock-otel` crate or `unblock-mcp-remote` per the plan's Phase 06 cross-reference) would have to depend on the binary crate `unblock-mcp` — backwards in the dep graph. Living in `unblock-core` lets the adapter sit anywhere downstream.
2. **Phase 03 `unblock-indexer`** wires `api_calls` / `api_durations` from inside `ResiliencePolicy::execute` (per Epic 02.B Task 4). `ResiliencePolicy` lives in `unblock-resilience` (no unblock deps). For `unblock-resilience::execute` to record into `ServerMetrics`, the metrics struct must be reachable from `unblock-resilience` OR injected as a generic. Cleanest: pass `&ServerMetrics` (or `Arc<ServerMetrics>`) as a parameter to a `record_api()` callback, defined as a trait in `unblock-resilience`. The implementation is owned by whatever crate holds `ServerMetrics` — if `unblock-core`, both `unblock-mcp` and `unblock-indexer` see the same instrumentation type.
3. **Pure-Rust, zero-network discipline of `unblock-core`** is preserved — `ServerMetrics` does no I/O.
4. **Phase 06 forward-compat contract test** (Decision L2.2) lives next to the struct. In `unblock-core`, that test belongs in `crates/unblock-core/src/metrics.rs` `#[cfg(test)] mod tests`.

**Evidence.**
- Existing `unblock-core` crate manifest already isolates pure types and the graph engine — `ServerMetrics` is a structural fit for the same crate (`/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-core/src/lib.rs`).
- The plan's Phase 06 forward-compat section (§2.2 "Forward-compat for Phase 06") explicitly states the OTel layer is a wrapper that reads the same atomic counters and histograms. A wrapper that lives in `unblock-otel` (Phase 06) cannot reach into `unblock-mcp`'s private modules without breaking the binary-crate boundary.
- Plan §6.2 already establishes the precedent of extracting reuse-required code to a neutral crate (`unblock-resilience`). `ServerMetrics` deserves the same treatment, with `unblock-core` as the natural home (no need for a 5th crate).

**Risk register.**
- The `hdrhistogram` workspace dep adds ~50 KB to the `unblock-core` build artefact. Acceptable — `unblock-core` already ships `petgraph`, `chrono`, and `serde` for similar reasons.
- The `Histogram` type is `Send` but not `Sync`. RG-6 surfaces the consequences for shared-instance recording. The placement decision is independent of the concurrency primitive choice.
- If RG-6 forces a switch to a third-party concurrent histogram crate (e.g. `prometheus-client`'s histogram, or hand-rolled atomic buckets), that dep also goes into `unblock-core`. No placement implication.

**Open questions (none).**

---

## RG-3 — Git inspection library: `git2` vs `gix`

**Validated finding.** The `commit_context --with-changes` surface needs:

| Need | gix support | git2 support |
|---|---|---|
| Read `user.name` / `user.email` | `Repository::config_snapshot()`, `Repository::committer()`, `Repository::author()` | `Repository::config()` |
| Open repo from cwd | `gix::discover(".")` | `Repository::discover(".")` |
| Working-tree status (modified / added / deleted files) | `Repository::status()` (feature-gated `status`); `Repository::index_worktree_status()` | `Repository::statuses(...)` |
| Diff worktree vs HEAD (for scope detection) | `gix-diff` (feature-gated) | `Repository::diff_index_to_workdir(...)` |

Both crates support the full read-only surface needed by `commit_context`. No write operations are needed (the tool generates a commit message; git itself performs the commit).

**Cross-platform implications (Phase 04 cargo-dist).**

- **`git2`** binds to **libgit2** (C library). Required transitive deps: `libgit2-sys` (vendored or system), C compiler at build time, optional `openssl-sys` for SSH. The `vendored-libgit2` feature ships a vendored copy and links statically when `LIBGIT2_NO_VENDOR=1` is unset. For `cargo-dist` cross-target releases (Phase 04 scope: Windows MSVC, x86_64-unknown-linux-musl, macOS arm64+x86_64), every target needs a working C toolchain in CI. Linux musl in particular is historically painful for `git2` — vendored is mandatory and even then OpenSSL needs `vendored-openssl`. Documented working-set: enable both `vendored-libgit2` and `vendored-openssl` features.
- **`gix`** is **pure Rust**. No C toolchain. No system OpenSSL. The whole graph compiles for any Rust target Cargo supports, including `x86_64-unknown-linux-musl` without special features. Upstream tests Windows on CI.

**Stability tier comparison.**

`gix`'s author maintains an explicit stability tier system. Per the upstream README and stability guide:
- Tier 1 (production): `gix-lock`, `gix-tempfile`.
- "Initial development — usable, possibly incomplete functionality": `gix` itself, `gix-diff`, `gix-status`, `gix-config`.

That said, the read-only surface `commit_context` consumes (config read, status read, simple HEAD-vs-worktree diff) is the well-trodden core path. None of these are bleeding-edge gix features. The cli `ein`/`gix` binaries are flagged "do not rely on them in scripts" but the **library** API used by `commit_context` is stable enough for a generate-only commit-message tool.

**Recommended decision.** Use **`gix`** for `commit_context`.

Rationale:
1. **No C toolchain in CI** — aligns with the project's pure-Rust stance and removes a Phase 04 cargo-dist failure mode.
2. **musl + Windows out of the box** — direct support in current CI matrix (`ubuntu-latest`, `macos-latest`) without hidden static-linking caveats.
3. **`commit_context`'s surface is narrow** — config read + status read. Even at gix's "initial development" tier, these subcrates have been API-stable for >12 months at the level we need.
4. **License compatibility** — `gix` is `MIT OR Apache-2.0`, matches workspace policy (`unblock-core`/`unblock-github` ship with the same dual license; `unblock-resilience` per plan §6.2 ships MIT).

**Evidence.**
- gix latest version: 0.81.0 (docs.rs `gix` crate page).
- `git2-rs` README confirms libgit2-sys C dep, `vendored-libgit2` feature, OpenSSL coupling for the `ssh` feature.
- gitoxide README lists `gix-lock`/`gix-tempfile` as Tier 1; `gix-diff`/`gix-status` as "initial development, usable".

**Risk register.**
- **R-3a (mitigated):** `gix` major version bumps may break the read-only surface. Pin to a specific minor in `Cargo.toml`; allow patch updates only via Cargo.lock; revisit at every minor.
- **R-3b (low impact):** `gix::Repository::status()` is feature-gated under `status` and `index`. Both are default-on but explicit feature opt-in is recommended in `unblock-mcp/Cargo.toml` for documentation discipline.
- **R-3c (low):** `gix-diff` for worktree-vs-HEAD scope detection is in initial-development tier. The `--with-changes` use case (file list + change kinds, NOT line-level diff parsing) only needs the index-vs-worktree comparison from `gix-status`, which is more mature than `gix-diff`. If we later want richer diff inspection for Phase 07 LLM, escalate.

**Open questions (none.)** RG-3 is a clean choice for `gix`.

---

## RG-4 — `failsafe` crate fitness

**Validated finding.** `failsafe` 1.3.0 supplies the circuit breaker primitives the plan needs, with one observability gap that requires implementation work but **not** a redesign.

**Working surface.**

```rust
// failsafe 1.3.0 docs/src
use failsafe::Config;
use failsafe::futures::CircuitBreaker;     // tokio-compatible

let breaker = Config::new()
    .failure_policy(failsafe::failure_policy::consecutive_failures(5, /* backoff */))
    .build();

let result = breaker.call(my_async_op()).await;
// breaker.is_call_permitted() — boolean
```

The `futures::CircuitBreaker` adapter wraps any `Future<Output = Result<T, E>>` and works with tokio. Failure policies include `consecutive_failures(threshold, backoff)` and `success_rate_over_time_window(...)`. `Config::cooldown(Duration)` configures the open→half-open transition.

**The state-inspection gap.** Confirmed by reading `state_machine.rs`: the `StateMachine::Inner` holds `enum State { Closed, Open(Instant, Duration), HalfOpen(Duration) }` privately. Public methods are `new`, `is_call_permitted`, `reset`, `on_success`, `on_error`. There is **no `state()` getter, no `is_open()`, and no opened-at timestamp accessor**.

**Bridge: the `Instrument` trait.** `failsafe` provides an `Instrument` trait with these callbacks (subset):
- `on_open()` — entered open state.
- `on_half_open()` — entered half-open state.
- `on_closed()` — entered closed state.
- `on_call_rejected()` — call rejected because of open breaker.

This **is** a state-transition observation hook. To populate `BreakerSnapshot { state: BreakerState, failure_count: usize, last_failure_at: Option<Instant> }`, the implementation pattern is:

```rust
struct ResilienceInstrument {
    state: AtomicU8,                       // 0=Closed, 1=Open, 2=HalfOpen
    last_transition_at: Mutex<Option<Instant>>,
    failure_count: AtomicUsize,            // separately tracked from on_error path
}

impl failsafe::Instrument for Arc<ResilienceInstrument> {
    fn on_open(&self) {
        self.state.store(1, Ordering::SeqCst);
        *self.last_transition_at.lock().unwrap() = Some(Instant::now());
    }
    // ... similarly for on_half_open / on_closed
}
```

Then `breaker_snapshot()` reads the atomics. The plan's §6.3 public API is unchanged; the implementation is a thin mirror.

**Recommended decision.** Adopt `failsafe` 1.3.0 per Decision L1.1. Implement `BreakerSnapshot` via an `Instrument` impl that mirrors state into local atomics. Document in the spec that `BreakerSnapshot.state` is **eventually consistent** with the breaker's actual state (the mirror updates on transition callbacks, which fire synchronously inside `failsafe`'s state-machine mutex but are observed lock-free by the snapshot reader).

**Evidence.**
- `failsafe::futures::CircuitBreaker::call(future)` confirmed (docs.rs `failsafe::futures` module).
- `failsafe::trait.CircuitBreaker` exposes `is_call_permitted`, `call`, `call_with` — no state getter.
- `failsafe::Instrument` trait provides 4 callbacks (`on_call_rejected`, `on_open`, `on_half_open`, `on_closed`) — confirmed via docs.rs `failsafe::Instrument` page.
- `failsafe::state_machine::StateMachine` source confirms the private `State` enum and absence of public state getter.
- Latest 1.3.0 release; no bug tracker pinned messages indicating abandonment, but only modest activity. Acceptable for the narrow surface we use.

**Risk register.**
- **R-4a (medium):** The `failure_policy` API is module-level functions returning a `FailurePolicy` impl, NOT a fluent builder. Spec must pin the exact failure policy: `consecutive_failures(5, equal_jittered_backoff(Duration::from_secs(10), Duration::from_secs(60)))`. Document the bound.
- **R-4b (low):** `Instrument` callbacks fire under `failsafe`'s internal mutex. Implementations MUST be cheap (no I/O, no blocking) — atomics-only. Spec MUST require this discipline.
- **R-4c (medium):** `failsafe`'s consecutive-failures policy resets the failure counter on success. The plan §11.1 acceptance ("Breaker opens after 5 consecutive failures") is consistent. But if Phase 06 wants success-rate semantics instead, that is a Phase 06 plan revision, not a Phase 02 risk.

**Open questions.** RG-4 closed. Decision L1.1 stays.

---

## RG-5 — `backoff` crate fitness

**Validated finding.** `backoff` 0.4.0 covers the functional requirements but its maintenance signal is weakening.

**Working surface.**

```rust
// backoff 0.4.0
use backoff::ExponentialBackoff;
use backoff::future::retry;

let policy = ExponentialBackoff {
    initial_interval: Duration::from_millis(500),
    randomization_factor: 0.5,         // jitter
    multiplier: 2.0,
    max_interval: Duration::from_secs(30),
    max_elapsed_time: Some(Duration::from_secs(30)),  // hybrid deadline
    ..Default::default()
};

let result = retry(policy, || async {
    op().await.map_err(|e| {
        if e.is_retryable() {
            backoff::Error::Transient { err: e, retry_after: e.retry_after() }
        } else {
            backoff::Error::Permanent(e)
        }
    })
}).await;
```

The `Error::Transient { err, retry_after: Option<Duration> }` shape is **the** documented `Retry-After` mechanism. Upstream docs explicitly cite "useful for handling rate limits like a HTTP 429 response". `backoff::future::retry` requires the `tokio` (or `async-std`) feature flag. Tokio 1.x compatibility confirmed.

**Mapping to plan §2.1.**

| Plan requirement (Decision L1.5/L1.6) | `backoff` mechanism |
|---|---|
| Max 5 attempts | `ExponentialBackoff` doesn't have a max-attempts knob directly. Workaround: track attempt count inside the closure, return `Permanent` after 5. |
| 30s deadline | `max_elapsed_time = Some(Duration::from_secs(30))`. |
| Hybrid (whichever first) | Combine the two — closure-counted attempts for the 5-cap, `max_elapsed_time` for the deadline. |
| `Retry-After` capped at 30s; >30s fail-fast | Caller-side: parse the header, if value > 30s return `Permanent` immediately; else return `Transient { retry_after: Some(value) }`. |

**Maintenance signal.** Per the upstream issue tracker (`github.com/ihrwein/backoff/issues`):
- 17 open issues; 9 open PRs.
- Issue #66 (May 2023): community asks "Does someone want to fork this?" — unanswered.
- Issue #72 (Dec 2024): the transitive `instant` dep is unmaintained.
- Latest activity Feb 2025; no pinned "looking for maintainers" or "abandoned" notice.

**Recommended decision.** Adopt `backoff` 0.4.0 per Decision L1.1 for Phase 02. Wrap the public API surface in `unblock-resilience` so the dep is replaceable. **Add a Phase 06+ needs-review item: re-evaluate `backon` (1.6.0, actively maintained, Apache-2.0)** as a successor when one of the following triggers fires: (a) `backoff` last release becomes >24 months stale; (b) a security advisory lands on `backoff` or `instant`; (c) `tokio` 2.0 breaks `backoff`.

`backon` is the most credible successor: pure-Rust, tokio-compatible, supports custom backoff strategies (constant/exponential/Fibonacci) with jitter, but does NOT have a documented Retry-After path equivalent to `Error::Transient { retry_after }`. Migration is feasible but non-trivial — a one-time cost when the trigger fires, not a Phase 02 blocker.

**Evidence.**
- `backoff::future::retry` signature confirmed: `pub fn retry<I, E, Fn, Fut, B>(backoff: B, operation: Fn) -> Retry<...>` (docs.rs `backoff::future::retry`).
- `Error::Transient { err: E, retry_after: Option<Duration> }` confirmed (docs.rs `backoff::Error`).
- `ExponentialBackoff` parameters confirmed: `initial_interval`, `randomization_factor` (jitter), `multiplier`, `max_interval`, `max_elapsed_time` (docs.rs `ExponentialBackoff`).
- Maintenance status: GitHub `ihrwein/backoff` issues page, Issue #66, #72, #74.
- `backon` 1.6.0 at docs.rs.

**Risk register.**
- **R-5a (medium):** `backoff` may go fully unmaintained mid-Phase. Mitigation: keep the dep wrapped behind `unblock-resilience::ResiliencePolicy`; trigger condition (a)/(b)/(c) above forces a `backon` migration with bounded blast radius (one crate, no public API change).
- **R-5b (low):** `backoff::Error` is generic over `E`; the `Permanent` and `Transient` ctors do not forward `IsRetryable` automatically. Plan §6.3's `IsRetryable` trait is unblock's bridge — the spec must pin the conversion adapter (e.g. `to_backoff_error<E: IsRetryable>(e: E) -> backoff::Error<E>`).
- **R-5c (low):** `backoff` honours `Retry-After` via `Transient { retry_after }`, but the **30-second cap** (Decision L1.6) must be enforced **caller-side** before constructing the `Transient` value. `backoff` does not cap `retry_after` itself.

**Open questions.** RG-5 closed for Phase 02. Successor evaluation deferred to Phase 06+.

---

## RG-6 — `hdrhistogram` overhead at MCP tool-call frequency

**Validated finding.** `hdrhistogram` records in **3-6 ns per value** under single-thread conditions, with no allocation. However, the crate does **not** provide a lock-free atomic histogram — `AtomicHistogram` and `ConcurrentHistogram` are explicitly listed by upstream as "not yet implemented" in the README.

The **only** concurrent option is `SyncHistogram`, which uses a per-thread `Recorder` pattern:
1. Each writer obtains its own `Recorder` via `SyncHistogram::recorder()`.
2. Recorders write lock-free to thread-local state.
3. The reader thread calls `SyncHistogram::refresh()` to merge recorders' state into the visible histogram.
4. **Recorded samples are NOT visible until `refresh()` runs.**

This **does not match the plan's data shape**. Plan §2.2 declares:

```rust
tool_durations: HashMap<&'static str, Histogram>,
api_durations:  HashMap<&'static str, Histogram>,
```

A bare `Histogram<T>` is `Send` but not `Sync`. With `&'static str` keys, the map cannot be a `&Histogram` source for concurrent recorders without a wrapping primitive.

**Three viable concurrency primitives, in order of fitness for the MCP server's call profile:**

| Option | Cost per record | Snapshot freshness | Implementation complexity |
|---|---|---|---|
| **A. `Mutex<Histogram>` per metric** | ~50-200 ns (lock contention dominated by mutex acquisition; `Histogram::record` itself is 3-6 ns) | Always fresh | Simplest; map shape preserved as `HashMap<&'static str, Mutex<Histogram>>` |
| **B. `SyncHistogram` per metric + `Recorder` per task** | ~10-20 ns per record (lock-free) | Stale until next `refresh()` | Complex: tasks must hold their own `Recorder`s; `doctor`/snapshot path must call `refresh_timeout` |
| **C. Sharded histograms (one per CPU/shard) + merge on snapshot** | ~10-20 ns | Always fresh | Custom infrastructure; highest dev cost |

**Realistic load projection.** The plan's RG-6 acceptance gate specifies "10k tool calls/s". This figure is **unrealistic** for a stdio MCP server:
- An MCP server processes one tool call at a time per stdio channel.
- Each tool call involves at least one GitHub round-trip (50-500ms typical) — a full warm-cache `ready` does no GitHub work but still serialises through tokio handlers.
- Realistic peak: 10-100 tool calls/s, sustained sub-10/s.

At the realistic load, **Option A (`Mutex<Histogram>` per metric)** has negligible contention. p99 of warm-cache `ready` (current target <2s) is dominated by I/O at ~10-100 ms; a 200 ns mutex acquisition is a 0.0001% delta. The plan's acceptance "<1µs per-call overhead" (§11.2) is met with margin.

The **bench gate (Epic 02.B Task 9)** must be re-scoped:
- Target call rate: **1k recorded values/s sustained** (10× realistic peak), measured per metric, not aggregate.
- Pass criterion: p99 record time <1µs per metric.
- 10k/s is fine as a stress floor, not a gate.

**Recommended decision.**

1. Adopt **Option A**: `tool_durations: HashMap<&'static str, Mutex<Histogram<u64>>>` (and similarly `api_durations`).
2. Wrap the histogram access in a small `record_duration(&self, name: &'static str, duration: Duration)` helper that hides the mutex.
3. Re-scope the RG-6 bench gate to the realistic 1k/s sustained / 10k/s burst profile and accept Option A's measured cost.
4. Defer Option B/C to Phase 06+ if a real-world load profile demands sub-50ns per-record cost (none of the projected MCP tools approaches that).

**Evidence.**
- `hdrhistogram` README "Not yet implemented" — `AtomicHistogram`, `ConcurrentHistogram` (HdrHistogram_rust GitHub README).
- Recording cost "3-6 nanoseconds on modern Intel CPUs" — docs.rs `hdrhistogram`.
- `SyncHistogram` recorder pattern + `refresh()` semantics — docs.rs `hdrhistogram::sync::SyncHistogram`.
- MCP rmcp transport: stdio is single-channel — confirmed by Phase 01 handler dispatch architecture (`crates/unblock-mcp/src/server.rs`).

**Risk register.**
- **R-6a (medium):** If ServerMetrics moves to `unblock-core` (RG-1), `Mutex<Histogram>` adds `parking_lot`/`std::sync::Mutex` overhead to a previously zero-network crate. Acceptable — `unblock-core::cache` already uses internal sync primitives.
- **R-6b (low):** Histograms are configured with bounds (`Histogram::new_with_bounds(low, high, sigfig)`). Spec must pin sensible defaults: e.g. for tool durations, `(1µs, 60s, sigfig=3)`; for API durations, `(1ms, 120s, sigfig=3)`. Out-of-range record() returns a recoverable error — spec must define the swallow path (log + tracing::warn, never propagate).
- **R-6c (low):** Histograms grow with the unique-key set (per-tool, per-API endpoint). The `&'static str` keys are bounded by the (small) tool/endpoint vocabulary. No leak risk.

**Needs-review items (carry to spec phase):**
- The plan §11.2 bench gate language ("Per-call instrumentation overhead < 1µs (bench RG-6)") is achievable with Option A at realistic load. Spec must replace the implicit 10k/s figure with a concrete realistic-load methodology paragraph (mirroring Phase 03 research's R8/R10 methodology pattern).

**Open questions (one).**

- Q-6.1 — Should the spec call out a specific `Histogram::new_with_bounds(low, high, sigfig)` default per metric category, or leave it implementation-decision? **Recommendation:** spec pins defaults; implementation may override per-metric only with explicit comment justifying the deviation.

---

## ~~RG-7~~ — DROPPED (scope error)

This RG conflated **bd (our internal dev PM tool)** with the **unblock product runtime**. bd is NOT part of the unblock product; it is the issue tracker the unblock team uses while developing unblock. The product's `commit_context` MCP tool reads the agent's active GitHub issue claim from the unblock-mcp graph cache — there is no indirection through bd.

**Corrected design** (now reflected in plan §2.4 and Epic 02.D Task 5): `commit_context` resolves the `Closes:` trailer URL directly from the active GitHub issue in the graph cache. No bd field is involved. No `bd github sync` configuration is required. This RG is dropped from spec consideration.

The original draft of this RG, including the Path A/B/C analysis and the NR-7.1 needs-review item, was based on the same scope error and is therefore obsolete.

---

## ~~RG-8~~ — DROPPED (scope error)

Same scope error as RG-7. The `Phase:` trailer is **not** derived from any external tracker (bd or otherwise). It is **opt-in** at the user-repo level: when a repo declares a phase via `.unblock/commit_context.toml`, the trailer is emitted with that value; otherwise it is omitted silently.

**Corrected design** (now reflected in plan §2.4 and Epic 02.D Task 7): repo config drives the `Phase:` trailer; no parent-chain walking, no regex parsing of titles. The original NR-8.1 needs-review item ("soften L4.3 from Always to best-effort") is also obsolete — L4.3 is corrected to declare `Phase:` as opt-in from the start, not as "Always emitted."

---

## RG-9 — Test fixtures for `StaleStatus` drift detection

**Validated finding.** The existing `MockGitHubClient` already supports the read+write-on-Status-field pattern needed by F1–F4. **No mock extension is required.**

**Existing mock surface (verified at `crates/unblock-github/src/mock.rs`):**
- `push_fetch_graph_data(GraphDataResult)` (line 433) — pre-loads the response from `fetch_graph_data` (returns `(Vec<Issue>, Vec<BlockingEdge>)`).
- `push_update_field(Result<(), Error>)` (line 417) — pre-loads the response of `update_field`.
- `update_field(...)` mock impl (line 539) — pops the queued response, increments `calls.update_field`.

**Existing graph-data parser (verified at `crates/unblock-github/src/graphql.rs:189-193`):**

```graphql
... on ProjectV2ItemFieldSingleSelectValue {
  field { ... on ProjectV2FieldCommon { name } }
  name
}
```

This means `Issue` already carries the Status field value as a parsed single-select option name — F1–F4 fixtures construct `Issue` with the appropriate `projectItems.fieldValues` and the existing GraphQL parser handles them.

**F1–F4 fixture construction, all viable from the existing mock:**

| Fixture | Setup |
|---|---|
| F1 (graph: closed; field: in_progress) | `Issue.state = Closed`; `Issue.project_field("Status") = Some("in_progress")` |
| F2 (graph: blocked; field: ready) | `Issue.state = Open`, blocking-edge present; `project_field("Status") = Some("ready")` |
| F3 (graph: ready; field: ready) | `Issue.state = Open`, no blocking edges; `project_field("Status") = Some("ready")` |
| F4 (graph: any; field: None) | `Issue.project_field("Status") = None` — should yield `MissingProjectField`, not `StaleStatus` |

The detection routine reads the graph (already cached) and the Status field value (from the `Issue` projection). No additional mock state is needed.

**Recommended decision.** Reuse the existing `update_project_field` test pattern (see `crates/unblock-github/src/projects.rs:587-641` for the production code path; `MockGitHubClient::push_update_field` for the mock). Construct F1–F4 as `Issue` fixtures, call `MockGitHubClient::push_fetch_graph_data` once with the topology, and assert the resulting `DriftReport.drift_found` contents.

**Evidence.**
- `crates/unblock-github/src/mock.rs:417` — `push_result!(update_field, push_update_field, ());`
- `crates/unblock-github/src/mock.rs:539-548` — mock `update_field` impl.
- `crates/unblock-github/src/projects.rs:599-641` — production `update_field` mutation.
- `crates/unblock-github/src/graphql.rs:189-193` — `ProjectV2ItemFieldSingleSelectValue` parser already in place.

**Risk register.**
- **R-9a (low):** `Issue` type's project-field representation may need verification at spec time. The `crates/unblock-core/src/types.rs` `Issue` struct should expose a `status_field()` accessor or equivalent — Epic 02.E Task 3 ("iterate graph nodes, compare `compute_status()` to Projects V2 Status field") implies this access path exists; if it doesn't, Task 3 carries a small accessor-add subtask.
- **R-9b (low):** F4 (None field state) currently maps to `MissingProjectField` per existing detection. Spec must clarify that `MissingProjectField` is **per-project** (the field is absent from the whole project), not per-issue (the field exists but a specific issue has no value). The plan §2.5 fixture table says F4 yields `MissingProjectField (NOT StaleStatus)` — verify this matches the existing detection for the per-issue case. If not, spec must introduce a 7th vs 8th drift type distinction OR redefine F4.

**Needs-review item (CARRY TO SPEC):**
- **NR-9.1** — Verify whether the existing `MissingProjectField` is per-project or per-issue. Plan §2.5's F4 expectation requires per-issue semantics; if the existing variant is per-project, spec must either widen `MissingProjectField` or introduce a new variant.

**Open questions (one).**
- Q-9.1 — Is `Issue::project_field("Status")` an existing accessor or does Epic 02.E Task 3 add it? Quick code-walk should answer; spec to confirm.

---

## Cross-cutting risks and needs-review items

### CC-1 — Plan's "<1µs per-call overhead" target vs `hdrhistogram` reality

The plan §11.2 acceptance "Per-call instrumentation overhead < 1µs (bench RG-6)" is **achievable** but only with Option A (`Mutex<Histogram>` per metric) under the realistic call rate (10-100/s). The 10k tool-calls/s figure in the plan §8 is unrealistic for stdio MCP and should be re-scoped.

**Action:** Spec re-states the bench methodology with realistic load assumptions; plan §8 RG-6 row updated post-research to reflect this finding.

### CC-2 — `failsafe` `BreakerSnapshot` is mirror-eventually-consistent

The plan §6.3 declares `BreakerSnapshot { state, failure_count, last_failure_at }` as a public API for Phase 03 consumers. The implementation must rely on `failsafe::Instrument` callbacks to mirror state into local atomics, NOT a direct `failsafe::StateMachine::state()` call (which doesn't exist publicly).

**Action:** Spec documents the eventual-consistency contract: snapshot may briefly disagree with the breaker's actual state during a transition, but converges within one callback-mutex window (sub-microsecond). This does not affect Phase 03 correctness — `unblock-indexer` uses `BreakerSnapshot` for `doctor` reporting only, never for control-flow gating.

### ~~CC-3~~ — DROPPED (scope error, see RG-7/RG-8 dropped sections)

This section conflated bd (our internal dev PM tool) with the unblock product runtime. The `Closes:` and `Phase:` trailers do NOT depend on bd state — see the corrected design notes in the dropped RG-7 and RG-8 sections.

### CC-4 — `backoff` maintenance is a Phase 06 latent risk, not a Phase 02 blocker

`backoff` 0.4.0 is functional but the upstream maintenance signal is weakening. `unblock-resilience` wraps the dep so a future `backon` migration is bounded.

**Action:** Add a Phase 06+ revisit task to the plan / risk register (already partially captured at plan §12 row "backoff does not honour Retry-After natively"; broaden to include "maintenance signal").

### CC-5 — Plan claims existing `is_retryable()` helper but it does not exist in the workspace

Plan §0.2 states: "`Error::RateLimited { reset_at }` — Variant + `is_retryable()` helper exist; never used in a retry loop." Workspace grep finds **zero** matches for `is_retryable` or `retryable` anywhere in `crates/`. The variant exists; the helper does not.

**Action:** Spec drops the "wraps existing `is_retryable()` helper" claim from §6.3 and §13.4, and treats `IsRetryable for unblock_github::Error` as a **new** impl in Epic 02.A Task 3, defined from scratch:

```rust
impl IsRetryable for unblock_github::Error {
    fn is_retryable(&self) -> bool {
        matches!(self,
            Error::RateLimited { .. }
          | Error::GitHubUnavailable { .. }
          | Error::PostMutationRebuildFailed { .. }   // 503 class
          | Error::PreMutationPrimeFailed { .. }      // 503 class
        ) || matches!(self.status_code(), 429 | 502 | 503)
    }
    fn retry_after(&self) -> Option<Duration> {
        match self {
            Error::RateLimited { reset_at } => {
                let now = Utc::now();
                if *reset_at > now {
                    Some((*reset_at - now).to_std().ok()?)
                } else { None }
            }
            _ => None,
        }
    }
}
```

This is a small change but Plan §0.2's audit text needs the correction so Fernando doesn't dispatch a "wire existing helper" bead that has no helper to wire.

### ~~CC-6~~ — DROPPED (scope error)

Same root cause as CC-3 / RG-8. `Phase:` trailer is opt-in via repo config, not derived from bd. Plan §2.4 / Decision L4.3 has been corrected accordingly.

---

## Recommendation

**Proceed to spec authoring** with the following preconditions visible to the user during /spec sign-off:

1. **NR-6.1** — Spec's RG-6 bench gate uses realistic load methodology, not 10k/s. (Cross-cutting CC-1.)
2. **NR-9.1** — Verify `MissingProjectField` is per-issue vs per-project before locking F4 expectation.
3. **CC-5** — Plan §0.2 audit correction: `is_retryable()` does not exist; Epic 02.A Task 3 implements from scratch.

NR-7.1, NR-8.1, CC-3, CC-6 dropped (scope error: confused bd-as-dev-tool with bd-as-product). RG-2 and RG-10 remain closed. RG-4 and RG-5 confirm the library choices in Decision L1.1. Spec authoring (Ada) can proceed once the user adjudicates NR-6.1 / NR-9.1 / CC-5.
