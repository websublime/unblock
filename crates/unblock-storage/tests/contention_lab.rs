//! # Contention lab — RK-1 / NFR-3, the **M0 EXIT GATE**
//!
//! This is the single empirical proof that libsql's WAL + the native `busy_timeout` (5000 ms) does
//! **not** 100%-CPU hot-spin under multi-writer write contention, and that concurrent writers stay
//! correct. It is the sanctioned inverse of *frankensqlite* defect-243 (which beads dodged with a
//! hand-rolled `busy_timeout = 0` + flock + sleep backoff): libsql ships real `SQLite`, whose native
//! busy handler **blocks** (sleeps), it never spins — and this lab is what makes that claim
//! falsifiable rather than asserted.
//!
//! ## Topology — multi-instance, one file (NOT the in-memory / Mutex-serialized path)
//!
//! `K` independent [`LibsqlStorage`] instances (each its own pair of connections + its own in-process
//! write `Mutex`) all `open_local` the **same temp FILE DB**. Cross-instance writers therefore contend
//! on the real OS-level WAL write lock — the only path that exercises WAL + native `busy_timeout`
//! concurrency (a shared-cache `:memory:` DB cannot use WAL, and the single-instance `behaviour.rs`
//! claim test is serialized by one `Mutex`, so neither exercises this). This mirrors D14's
//! single-serve-per-workspace model stressed by `K` would-be servers racing one file.
//!
//! ## The metric — a baseline-relative CPU-per-write **ratio**
//!
//! Spin vs block is distinguished by **CPU consumed per committed write**, not wall time: a blocking
//! busy handler spends its wait *asleep* (no CPU), a spinning one *burns* CPU. Whole-process CPU is
//! sampled with [`cpu_time::ProcessTime`] (sums every tokio worker thread, unsafe-free — no raw libc).
//! Two legs run **strictly sequentially** in one multi-thread runtime with an identical writer body /
//! writer count `K` / ops-per-writer:
//!
//! - **baseline** — `K` writers each on their **own** temp file → the busy handler never engages. The
//!   writers are run **sequentially** (one at a time) so the measured CPU-per-write is the *honest
//!   single-write cost*, free of the allocator / scheduler / cache-line overhead that flat-out raw
//!   parallelism would fold in (per-write CPU is **not** invariant to thread-parallelism — see the
//!   `Concurrency` enum). Since the contended leg is serialized by the one WAL write lock (effective DB
//!   concurrency ≈ 1), the sequential reference is the apples-to-apples denominator;
//! - **contended** — the same `K` writers on **one** shared temp file, run **in parallel** → the WAL
//!   write-lock + native `busy_timeout` engage.
//!
//! `R = (contended CPU-per-write) / (baseline CPU-per-write)`. Honest blocking ⇒ `R ≈ 1` (the extra
//! time is sleep, not CPU). A hot-spin ⇒ `R ≫ 5` (the `K - 1` losers burn cores while one writer works,
//! so per-write CPU rises ≈ `K×`). The ratio normalizes for machine speed (both legs, same runner, same
//! process), so it is far more flake-resistant than any absolute CPU/wall threshold. The gate asserts
//! **R ≤ 5.0** — a *provisional* wide categorical bound. On a multi-core dev machine independent runs
//! measure `R ≈ 1.0–1.2` blocking (the run-to-run spread; the band bounds it, not a single point) and
//! `R ≈ 25–27` for the forced-spin control, so 5.0 sits with large headroom on both sides. It is marked
//! provisional in code + docs; the actual measured `R` is printed on every run and recorded in
//! `STATUS.md`. Calibration of a tighter bound is pending (perf budgets are T3.5).
//!
//! The periodic passive checkpoint is **disabled inside both timed brackets** (so checkpoint CPU never
//! enters the ratio); open/migrate/warmup stay **outside** both brackets; the baseline tasks are
//! awaited and dropped **before** the contended timing starts.
//!
//! ## The deterministic contention witness (mandatory — never a silent pass)
//!
//! libsql exposes no busy-handler callback and the native `busy_timeout` resolves contention by
//! blocking *silently* (no error surfaces), so a busy/locked count of 0 would otherwise be invisible.
//! The lab enables a **zero-timeout busy-witness probe** on the write path (`testkit_set_busy_witness`):
//! each mutating `BEGIN IMMEDIATE` first tries to acquire the write lock with a zero timeout; if
//! another writer holds it that is one witnessed contention event, and the write then proceeds with
//! the real blocking begin (the gate's blocking semantics are unchanged). The lab asserts the
//! busy-retry witness is **> 0** in the contended leg and **== 0** in the baseline leg. A contended
//! witness of 0 ⇒ FAIL "no contention materialized" — the gate is INCONCLUSIVE (a harness defect), it
//! is never a silent pass. (Surfaced `DatabaseLocked` is *not* required — with `busy_timeout = 5000`
//! most losers block-then-win.)
//!
//! ## Falsifiability — the controls prove the gate is non-vacuous
//!
//! Two `#[ignore]`d controls (run explicitly with `-- --ignored`) prove the lab actually *detects* the
//! failures it gates against:
//!
//! - **forced-spin control** ([`forced_spin_control_blows_the_ratio`]) — a store built with
//!   `busy_timeout = 0` so losers spin-retry at the application level (the beads anti-pattern). It runs
//!   the same two legs and asserts `R ≫ 5` (the metric detects a real hot-spin).
//! - **WAL negative control** ([`wal_negative_control_breaches_ceiling`]) — the same sustained
//!   contention window with the periodic checkpoint **disabled**; it asserts the `-wal` sidecar
//!   **breaches** the ceiling that the positive WAL-bound sub-phase holds it under (so the positive
//!   assertion is falsifiable, not vacuous).
//!
//! ## Scope — what this gate does NOT assert
//!
//! **No throughput or latency budget is asserted here** — perf budgets (NFR-1/NFR-2) are T3.5
//! (`benches/storage.rs` + criterion). This gate is purely the *non-spin* + *correctness* proof.
//!
//! ## Failure → action
//!
//! | Symptom | Meaning | Action |
//! |---|---|---|
//! | High `R` (> 5) **with** a confirmed busy-retry witness | a real CPU hot-spin under contention | **STOP** — pivot to `rusqlite` behind the same `Storage` trait; re-open D14/D15. This is the RK-1 signal. |
//! | busy-retry witness **absent** (contended == 0) | no contention materialized | gate **INCONCLUSIVE** (a harness defect), **not** a pass and **not** a failure — fix the harness. |
//! | non-empty `integrity_check` / an unexpected error variant | data corruption / a lost write | **STOP** — corruption finding; pivot. |
//!
//! ## Why this is the SOLE test in the file
//!
//! [`cpu_time::ProcessTime`] measures **whole-process** CPU, so any other test running concurrently in
//! this binary's process would pollute the ratio. The gate and its controls therefore live alone in
//! this file (one `#[tokio::test]` + two `#[ignore]`d controls that the default run never executes
//! concurrently with it), and the gate is feature-gated so it only compiles under `--features testkit`.

#![cfg(feature = "testkit")]
// This file is a single empirical gate: it `panic!`s (via `assert!`/`expect`) on any gate violation,
// by design. Documenting a `# Panics` section on each helper would be noise.
#![allow(clippy::missing_panics_doc)]
// Benchmark/measurement-inherent lints, scoped to this gate harness:
// - `cast_precision_loss`: the CPU-per-write ratio is computed in `f64` from `u64` counts/durations
//   by design (the values are small — micro/milli-scale — so the precision loss is irrelevant);
// - `similar_names`: the two legs are deliberately named `baseline`/`contended` and the per-writer
//   handles `store`/`stores` — renaming for the lint would obscure the metric, not clarify it;
// - `too_many_lines`: the gate body and the correctness-phases driver are each one cohesive sequence
//   (splitting them would scatter the single auditable transcript this gate is meant to be).
#![allow(
    clippy::cast_precision_loss,
    clippy::similar_names,
    clippy::too_many_lines
)]

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use chrono::{TimeZone, Utc};
use cpu_time::ProcessTime;
use tempfile::TempDir;
use tokio::sync::Mutex;
use tokio::task::JoinSet;

use unblock_model::Issue;
use unblock_storage::{IssuePatch, LibsqlStorage, Storage, StorageError, StorageTestkit};

/// Process-wide serialization for the three tests in this file.
///
/// [`cpu_time::ProcessTime`] measures **whole-process** CPU, so two of these tests running at once
/// would pollute each other's ratio (and the forced-spin control alone burns tens of CPU-seconds).
/// Cargo runs a file's tests concurrently by default, and `--include-ignored` would run all three at
/// once — so every test acquires this lock for its **entire** body, guaranteeing strictly serial
/// execution regardless of how the binary is invoked (`cargo test`, `-- --ignored`,
/// `-- --include-ignored`, or `--test-threads=N`). A `tokio::sync::Mutex` guard is `Send`, so it can be
/// held across `.await` in the multi-thread runtime (a `std::sync::MutexGuard` could not).
static GATE_SERIAL: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

// --------------------------------------------------------------------------------------------------
// Tunables
// --------------------------------------------------------------------------------------------------

/// Provisional gate bound on the contended/baseline CPU-per-write ratio (see the module docs).
///
/// Wide categorical bound: far below a spin's `≈ 25–27`, comfortably above honest blocking's
/// `≈ 1.0–1.2`. The measured `R` is printed on every run; a tighter, calibrated bound is pending
/// (perf budgets = T3.5).
const PROVISIONAL_RATIO_CEILING: f64 = 5.0;

/// Floor on the adaptive writer count (the gate needs at least two writers to contend at all).
const MIN_WRITERS: usize = 2;

/// Per-writer committed mutations in each timed leg. Sized (× `K` writers) for ~1.5–3 s of contended
/// wall on a typical multi-core runner so a hot-spin accumulates measurable CPU and the fixed
/// open/warmup costs amortize out of the ratio.
const OPS_PER_WRITER: u32 = 600;

/// The WAL-ceiling the periodic passive checkpoint must hold the `-wal` sidecar under, in bytes.
///
/// `journal_size_limit` is 32 MiB (`33554432`); the periodic checkpoint reuses the WAL in place so it
/// stays near that. The ceiling is generous (2 × the limit) to stay flake-resistant on slow CI while
/// still being decisively breached by the unbounded negative control.
const WAL_CEILING_BYTES: u64 = 64 * 1024 * 1024;

/// Hard wall-clock cap on the whole gate body (defense against a pathological hang).
// `from_secs(120)` reads more naturally as a 120-second budget than `from_mins(2)` here.
#[allow(clippy::duration_suboptimal_units)]
const GATE_TIMEOUT: Duration = Duration::from_secs(120);

// --------------------------------------------------------------------------------------------------
// The M0 gate
// --------------------------------------------------------------------------------------------------

/// The RK-1 / NFR-3 M0 exit gate. Runs on a multi-thread runtime sized to the machine; **hard-fails**
/// on a single-core runner (the busy-retry witness is the backstop). The whole body is wrapped in a
/// 120 s timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn contention_lab_no_hot_spin_and_correct() {
    // Serialize against the controls (whole-process CPU; see `GATE_SERIAL`).
    let _serial = GATE_SERIAL.lock().await;

    let parallelism = available_parallelism();
    assert!(
        parallelism >= 2,
        "the contention lab requires a >= 2-vCPU runner (available_parallelism = {parallelism}); \
         on a single core the writers cannot genuinely contend and the gate is meaningless. The CI \
         M0-gate job MUST run with --features testkit on a >= 2-vCPU runner (T0.9 handoff)."
    );

    tokio::time::timeout(GATE_TIMEOUT, run_gate(parallelism))
        .await
        .expect("the contention lab exceeded its 120s budget — investigate a hang (possible spin)");
}

/// The gate body (separated so the 120 s timeout can wrap it).
async fn run_gate(parallelism: usize) {
    // Adaptive writer count: one writer per vCPU, floored at MIN_WRITERS, capped to keep the file
    // contention sane and the run bounded.
    let writers = parallelism.clamp(MIN_WRITERS, 16);
    let total_per_leg = writers as u64 * u64::from(OPS_PER_WRITER);

    println!("\n=== unblock contention lab (RK-1 / NFR-3, M0 exit gate) ===");
    println!(
        "available_parallelism = {parallelism}; writers K = {writers}; \
         ops/writer = {OPS_PER_WRITER}; total writes/leg = {total_per_leg}"
    );

    // --- Measure the CPU-per-write ratio (baseline then contended, checkpoint disabled in both) ---
    let baseline = run_baseline_leg(writers).await;
    let contended = run_contended_leg(writers).await;

    let ratio = contended.cpu_per_write() / baseline.cpu_per_write();

    // --- Correctness phases on a fresh shared file (count/shape assertions only) ---
    let correctness = run_correctness_phases(writers).await;

    // --- WAL-bound sub-phase (checkpoint ENABLED) on a fresh shared file ---
    let wal = run_wal_bound_phase(writers).await;

    // --- Final integrity check on the WAL-bound store's file ---
    let integrity = wal.integrity.clone();

    // ---- Diagnostic block (printed on EVERY run; capture with `-- --nocapture`) ----
    print_diagnostics(&baseline, &contended, ratio, &correctness, &wal, &integrity);

    // ==== Gate assertions ====

    // (1) Contention MUST have materialized — else the gate is inconclusive, never a silent pass.
    assert!(
        contended.busy_retries > 0,
        "GATE INCONCLUSIVE: no contention materialized (contended busy-retry witness == 0). This is \
         a harness defect, not a pass — the writers did not genuinely race the WAL write lock."
    );
    assert_eq!(
        baseline.busy_retries, 0,
        "the baseline leg (each writer on its own file) must record zero contention, got {}",
        baseline.busy_retries
    );

    // (2) The non-spin proof: the CPU-per-write ratio stays under the provisional ceiling.
    assert!(
        ratio <= PROVISIONAL_RATIO_CEILING,
        "RK-1 SIGNAL: contended/baseline CPU-per-write ratio R = {ratio:.2} exceeds the provisional \
         ceiling {PROVISIONAL_RATIO_CEILING:.1} — this indicates a CPU hot-spin under contention. STOP \
         and pivot to rusqlite behind the Storage trait; re-open D14/D15."
    );

    // (3) Correctness: exactly one claim winner; disjoint creates reconcile; only the allowlisted
    //     retryable errors appear under deep contention.
    assert_eq!(
        correctness.claim_winners, 1,
        "exactly one claimer must win the storm, got {} winners",
        correctness.claim_winners
    );
    assert_eq!(
        correctness.created,
        correctness.create_attempts - correctness.create_locked,
        "every non-DatabaseLocked create must have committed (no lost write): \
         created {} != attempts {} - locked {}",
        correctness.created,
        correctness.create_attempts,
        correctness.create_locked
    );
    // Update-storm no-lost-write reconciliation: every attempt either committed or lost with the
    // allowlisted DatabaseLocked (any other variant already panicked in the storm) — nothing vanished.
    assert_eq!(
        correctness.update_committed + correctness.update_locked,
        correctness.update_attempts,
        "every update-storm attempt must commit or lose with DatabaseLocked (no lost write): \
         committed {} + locked {} != attempts {}",
        correctness.update_committed,
        correctness.update_locked,
        correctness.update_attempts
    );

    // (4) WAL stays bounded under sustained contention with the periodic checkpoint on.
    assert!(
        wal.wal_bytes <= WAL_CEILING_BYTES,
        "the -wal sidecar ({} bytes) breached the ceiling ({WAL_CEILING_BYTES} bytes) WITH the \
         periodic passive checkpoint on — the checkpoint cadence is not bounding the WAL",
        wal.wal_bytes
    );

    // (5) Integrity is clean.
    assert!(
        integrity.is_empty(),
        "integrity_check reported problems (corruption / lost write): {integrity:?}"
    );

    println!(
        "\n=== GATE PASS: R = {ratio:.2} <= {PROVISIONAL_RATIO_CEILING:.1} (provisional), \
         contention witnessed ({} busy-retries), correctness + WAL-bound + integrity all green ===\n",
        contended.busy_retries
    );
}

// --------------------------------------------------------------------------------------------------
// CPU-per-write legs
// --------------------------------------------------------------------------------------------------

/// One timed leg's measurement.
struct LegStats {
    /// Whole-process CPU consumed during the timed write loop.
    cpu: Duration,
    /// Wall-clock elapsed during the timed write loop.
    wall: Duration,
    /// Committed writes during the timed loop.
    writes: u64,
    /// Witnessed write-lock contention events during the timed loop.
    busy_retries: u64,
}

impl LegStats {
    /// CPU seconds consumed per committed write (the ratio numerator/denominator).
    fn cpu_per_write(&self) -> f64 {
        // A leg always commits > 0 writes (the gate would have failed open/migrate otherwise); guard
        // against a zero divide defensively so a harness bug surfaces as a clear value, not a panic.
        let writes = self.writes.max(1) as f64;
        self.cpu.as_secs_f64() / writes
    }
}

/// How the timed write storm schedules its `K` writers.
///
/// This is the variable that distinguishes the two legs — and the subtle crux of the metric. The
/// **denominator** of the ratio must be the *honest per-write CPU cost* with no waiting; the
/// **numerator** is the same writes under contention. Crucially, per-write CPU is **not** invariant to
/// raw thread-parallelism — running `K` independent writers flat-out inflates each write's CPU with
/// allocator / scheduler / cache-line contention that has nothing to do with the *busy handler*. Since
/// the contended leg is serialized by the single WAL write lock (effective DB concurrency ≈ 1), the
/// fair baseline runs its writers **sequentially** (concurrency 1) so the denominator is the clean
/// single-write cost. The contended leg runs **parallel** — that is the contention under test.
///
/// (The writer body, writer count `K`, and ops-per-writer are identical across both legs, per the
/// spec; only the *scheduling* differs, which is exactly the property being isolated. A parallel
/// baseline would silently fold parallelism overhead into the denominator and suppress the ratio,
/// masking a real hot-spin — so it is deliberately avoided.)
///
/// **Spin isolation (why R is a clean spin signal).** The gate and the forced-spin control hold the
/// *scheduling* constant — `Sequential` baseline, `Parallel` contended — and vary **only**
/// `busy_timeout` (5000 ms blocking vs 0 spinning). Same topology, same writers, same ops; the lone
/// independent variable across the two configurations is the busy policy. So the resulting `R`
/// (`≈ 1.0–1.2` at 5000 ms vs `≈ 27` at 0) isolates the *spin*, not any scheduling or parallelism
/// artifact — which is exactly what makes the gate a sound, non-vacuous non-spin proof.
#[derive(Clone, Copy)]
enum Concurrency {
    /// Run the writers one after another (the honest, contention-free per-write CPU reference).
    Sequential,
    /// Run the writers concurrently (the real contention scenario).
    Parallel,
}

/// Baseline leg: `K` writers, each on its **own** temp file → no cross-writer contention, run
/// **sequentially** so the measured CPU-per-write is the honest single-write cost (see [`Concurrency`]).
/// The busy handler never engages, so the busy-retry witness must stay 0.
async fn run_baseline_leg(writers: usize) -> LegStats {
    // One store + temp dir per writer (each its own file ⇒ no contention). `dirs` is held to keep the
    // temp files alive for the whole leg, then dropped after the storm.
    let mut stores = Vec::with_capacity(writers);
    let mut dirs = Vec::with_capacity(writers);
    for _ in 0..writers {
        let (store, dir) = open_fresh_store(BusyMode::Native).await;
        store.testkit_set_busy_witness(true).await;
        store.testkit_set_checkpoint_interval(0).await; // checkpoint OFF inside the timed bracket
        warmup(&store).await;
        stores.push(Arc::new(store));
        dirs.push(dir);
    }

    let stats = timed_write_storm(&stores, writers, Concurrency::Sequential, "baseline").await;
    drop(dirs);
    stats
}

/// Contended leg: the same `K` writers on **one** shared temp file, run **in parallel** → the WAL
/// write-lock + native `busy_timeout` engage. Identical writer body / count / ops to the baseline leg.
async fn run_contended_leg(writers: usize) -> LegStats {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("unblock.db");

    // Migrate via the FIRST instance fully, THEN open the rest serially (never join_all) — all outside
    // the timed bracket.
    let stores = open_shared_instances(&path, writers, BusyMode::Native).await;
    for store in &stores {
        store.testkit_set_busy_witness(true).await;
        store.testkit_set_checkpoint_interval(0).await; // checkpoint OFF inside the timed bracket
        warmup(store).await;
    }

    timed_write_storm(&stores, writers, Concurrency::Parallel, "contended").await
}

/// The timed write storm shared by both legs: `K` writers each commit `OPS_PER_WRITER` updates to a
/// distinct seed row they own, bracketed by a single whole-process CPU sample. `stores[i]` is writer
/// `i`'s store (its own file in the baseline leg, the shared file in the contended leg). `concurrency`
/// selects sequential (baseline reference) vs parallel (the contention under test) — see
/// [`Concurrency`].
async fn timed_write_storm(
    stores: &[Arc<LibsqlStorage>],
    writers: usize,
    concurrency: Concurrency,
    label: &str,
) -> LegStats {
    // Each writer owns one pre-created seed row on its store, so the writes are pure updates (a hot,
    // race-sensitive single-row UPDATE — the contended path).
    for (i, store) in stores.iter().enumerate() {
        store
            .create_issue(&seed_issue(&format!("ub-storm-{i}")), "seed")
            .await
            .expect("seed create");
    }

    // Snapshot the busy-retry witness across all stores just before timing.
    let busy_before = sum_busy(stores).await;

    let cpu_start = ProcessTime::now();
    let wall_start = Instant::now();

    let writes = match concurrency {
        Concurrency::Sequential => {
            // Honest reference: one writer at a time, no parallelism overhead, no waiting.
            let mut writes = 0u64;
            for i in 0..writers {
                let store = &stores[i.min(stores.len() - 1)];
                let id = format!("ub-storm-{i}");
                writes += drive_writer(store, &id).await;
            }
            writes
        }
        Concurrency::Parallel => {
            // The contention scenario: all writers race.
            let mut set = JoinSet::new();
            for i in 0..writers {
                let store = Arc::clone(&stores[i.min(stores.len() - 1)]);
                let id = format!("ub-storm-{i}");
                set.spawn(async move { drive_writer(&store, &id).await });
            }
            let mut writes = 0u64;
            while let Some(res) = set.join_next().await {
                writes += res.expect("writer task join");
            }
            writes
        }
    };

    let cpu = cpu_start.elapsed();
    let wall = wall_start.elapsed();
    let busy_retries = sum_busy(stores).await - busy_before;

    println!(
        "[{label}] cpu={:?} wall={:?} writes={writes} busy_retries={busy_retries} \
         cpu/write={:.3}us",
        cpu,
        wall,
        (cpu.as_secs_f64() / writes.max(1) as f64) * 1e6
    );

    LegStats {
        cpu,
        wall,
        writes,
        busy_retries,
    }
}

/// Drive one writer: `OPS_PER_WRITER` title updates against its own row `id`. Returns committed writes.
/// A native-`busy_timeout` loser blocks-then-wins (so `DatabaseLocked` is rare but allowlisted); any
/// other error variant is a corruption finding and panics.
async fn drive_writer(store: &LibsqlStorage, id: &str) -> u64 {
    let mut committed = 0u64;
    for n in 0..OPS_PER_WRITER {
        let patch = IssuePatch {
            title: Some(format!("{id}-rev-{n}")),
            ..IssuePatch::default()
        };
        match store.update_issue(id, &patch, "writer").await {
            Ok(_) => committed += 1,
            Err(StorageError::DatabaseLocked) => {}
            Err(other) => panic!("unexpected write error in {id}: {other:?}"),
        }
    }
    committed
}

// --------------------------------------------------------------------------------------------------
// Correctness phases
// --------------------------------------------------------------------------------------------------

/// Outcomes of the correctness phases (count/shape only).
struct CorrectnessStats {
    /// Claim-storm winners (must be exactly 1).
    claim_winners: usize,
    /// Claim-storm losers that lost with `AlreadyClaimed` or `DatabaseLocked` (the allowlist).
    claim_losers: usize,
    /// Disjoint-create attempts.
    create_attempts: u64,
    /// Disjoint-creates that committed.
    created: u64,
    /// Disjoint-creates that lost with the allowlisted `DatabaseLocked`.
    create_locked: u64,
    /// Update-storm attempts (`upd_per_writer * writers`).
    update_attempts: u64,
    /// Update-storm commits.
    update_committed: u64,
    /// Update-storm `DatabaseLocked` losers (allowlisted).
    update_locked: u64,
}

/// Run the three correctness storms on a fresh shared file (claim, disjoint-create, update).
async fn run_correctness_phases(writers: usize) -> CorrectnessStats {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("unblock.db");
    let stores = open_shared_instances(&path, writers, BusyMode::Native).await;
    let reader = Arc::clone(&stores[0]);

    // (a) Claim storm: K instances race to claim ONE issue. Establish the winner from a single
    //     post-drain re-SELECT of the durable assignee+status (do NOT trust per-error `by`).
    reader
        .create_issue(&seed_issue("ub-claim"), "seed")
        .await
        .expect("seed claim target");

    let mut claim_set = JoinSet::new();
    for (i, store) in stores.iter().enumerate() {
        let store = Arc::clone(store);
        let actor = format!("agent-{i}");
        claim_set.spawn(async move { store.claim_issue("ub-claim", &actor, &actor).await });
    }
    let mut claim_ok = 0usize;
    let mut claim_losers = 0usize;
    while let Some(res) = claim_set.join_next().await {
        match res.expect("claim join") {
            Ok(_) => claim_ok += 1,
            Err(StorageError::AlreadyClaimed { .. } | StorageError::DatabaseLocked) => {
                claim_losers += 1;
            }
            Err(other) => panic!("claim storm: unexpected error variant (corruption?): {other:?}"),
        }
    }
    // The durable winner is the single issue's assignee+in_progress, read fresh after the drain.
    let claimed = reader
        .get_issue("ub-claim")
        .await
        .expect("re-select claim target")
        .expect("claim target present");
    let claim_winners = usize::from(
        claimed.assignee.is_some() && claimed.status == unblock_model::Status::InProgress,
    );
    assert_eq!(
        claim_winners, 1,
        "the durable re-SELECT must show exactly one winner (assignee set + in_progress); \
         Ok count was {claim_ok}, durable winner = {claim_winners}"
    );

    // (b) Disjoint-create storm: every writer creates its own distinct ids → no IdCollision is
    //     allowed (that would be a lost-write / duplicate-id bug); only DatabaseLocked is allowlisted.
    let per_writer = 40u32;
    let mut create_set = JoinSet::new();
    for (i, store) in stores.iter().enumerate() {
        let store = Arc::clone(store);
        create_set.spawn(async move {
            let mut created = 0u64;
            let mut locked = 0u64;
            for n in 0..per_writer {
                let id = format!("ub-mk-{i}-{n}");
                match store.create_issue(&seed_issue(&id), "creator").await {
                    Ok(_) => created += 1,
                    Err(StorageError::DatabaseLocked) => locked += 1,
                    Err(other) => {
                        panic!(
                            "create storm: unexpected error (IdCollision = lost write?): {other:?}"
                        )
                    }
                }
            }
            (created, locked)
        });
    }
    let create_attempts = u64::from(per_writer) * writers as u64;
    let mut created = 0u64;
    let mut create_locked = 0u64;
    while let Some(res) = create_set.join_next().await {
        let (c, l) = res.expect("create join");
        created += c;
        create_locked += l;
    }
    // Reconcile: the durable row count of the disjoint ids equals `created`.
    let durable = count_prefix(&reader, "ub-mk-").await;
    assert_eq!(
        durable, created,
        "disjoint-create reconciliation: durable rows ({durable}) must equal committed creates \
         ({created}) — a mismatch is a lost or duplicated write"
    );

    // (c) Update storm: K writers hammer ONE shared row; every commit is a real update, only
    //     DatabaseLocked is allowlisted.
    reader
        .create_issue(&seed_issue("ub-upd"), "seed")
        .await
        .expect("seed update target");
    let upd_per_writer = 60u32;
    let mut upd_set = JoinSet::new();
    for store in &stores {
        let store = Arc::clone(store);
        upd_set.spawn(async move {
            let mut committed = 0u64;
            let mut locked = 0u64;
            for n in 0..upd_per_writer {
                let patch = IssuePatch {
                    title: Some(format!("upd-{n}")),
                    ..IssuePatch::default()
                };
                match store.update_issue("ub-upd", &patch, "updater").await {
                    Ok(_) => committed += 1,
                    Err(StorageError::DatabaseLocked) => locked += 1,
                    Err(other) => panic!("update storm: unexpected error variant: {other:?}"),
                }
            }
            (committed, locked)
        });
    }
    let update_attempts = u64::from(upd_per_writer) * stores.len() as u64;
    let mut update_committed = 0u64;
    let mut update_locked = 0u64;
    while let Some(res) = upd_set.join_next().await {
        let (c, l) = res.expect("update join");
        update_committed += c;
        update_locked += l;
    }
    // The shared row survives, single, consistent.
    let final_upd = reader
        .get_issue("ub-upd")
        .await
        .expect("re-select update target")
        .expect("update target present");
    assert!(
        final_upd.title.starts_with("upd-"),
        "the update-storm row must reflect a committed update, got title {:?}",
        final_upd.title
    );

    CorrectnessStats {
        claim_winners,
        claim_losers,
        create_attempts,
        created,
        create_locked,
        update_attempts,
        update_committed,
        update_locked,
    }
}

// --------------------------------------------------------------------------------------------------
// WAL-bound sub-phase (checkpoint ENABLED)
// --------------------------------------------------------------------------------------------------

/// WAL-bound result.
struct WalStats {
    /// Final `-wal` sidecar size in bytes.
    wal_bytes: u64,
    /// Passive checkpoints fired during the phase (summed across instances).
    checkpoints: u64,
    /// Committed writes during the phase.
    writes: u64,
    /// Final `integrity_check` output (must be empty).
    integrity: Vec<String>,
}

/// Sustained contention with the periodic passive checkpoint **enabled** — the `-wal` sidecar must
/// stay bounded under [`WAL_CEILING_BYTES`].
async fn run_wal_bound_phase(writers: usize) -> WalStats {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("unblock.db");
    let stores = open_shared_instances(&path, writers, BusyMode::Native).await;
    // Checkpoint ENABLED at the production cadence (the default), witness on for completeness.
    for store in &stores {
        store.testkit_set_busy_witness(true).await;
    }

    let writes = sustained_update_storm(&stores, writers).await;

    let wal_bytes = wal_size(&path);
    let mut checkpoints = 0u64;
    for store in &stores {
        checkpoints += store.testkit_checkpoint_count().await;
    }
    let integrity = stores[0].integrity_check().await.expect("integrity_check");

    WalStats {
        wal_bytes,
        checkpoints,
        writes,
        integrity,
    }
}

/// A sustained update storm against a handful of shared rows (drives many WAL frames). Returns the
/// committed write count.
async fn sustained_update_storm(stores: &[Arc<LibsqlStorage>], writers: usize) -> u64 {
    // A small pool of shared hot rows.
    let rows = 8u32;
    for n in 0..rows {
        stores[0]
            .create_issue(&seed_issue(&format!("ub-wal-{n}")), "seed")
            .await
            .expect("seed wal row");
    }
    let per_writer = 400u32;
    let mut set = JoinSet::new();
    for store in stores.iter().take(writers) {
        let store = Arc::clone(store);
        set.spawn(async move {
            let mut committed = 0u64;
            for n in 0..per_writer {
                let id = format!("ub-wal-{}", n % rows);
                let patch = IssuePatch {
                    title: Some(format!("wal-rev-{n}")),
                    ..IssuePatch::default()
                };
                match store.update_issue(&id, &patch, "wal-writer").await {
                    Ok(_) => committed += 1,
                    Err(StorageError::DatabaseLocked) => {}
                    Err(other) => panic!("wal storm: unexpected error: {other:?}"),
                }
            }
            committed
        });
    }
    let mut writes = 0u64;
    while let Some(res) = set.join_next().await {
        writes += res.expect("wal writer join");
    }
    writes
}

// --------------------------------------------------------------------------------------------------
// #[ignore]d controls — prove the gate is non-vacuous (run with `-- --ignored`)
// --------------------------------------------------------------------------------------------------

/// FORCED-SPIN CONTROL (falsifiability): a store built with `busy_timeout = 0` so write-lock losers
/// spin-retry at the application level (the beads / *frankensqlite* defect-243 anti-pattern,
/// deliberately reproduced as a **tight, non-yielding** loop that pins its worker thread). Runs the
/// same two legs and asserts `R ≫ 5` — proving the lab's CPU-per-write metric actually detects a
/// hot-spin. `#[ignore]`d: this is a deliberate spin and would otherwise fail the gate's intent.
///
/// Built on its **own** runtime sized to `K + 4` worker threads (not `#[tokio::test]`): a true
/// non-yielding spin needs at least one thread per concurrent writer **plus** spare for the lock
/// holder, or the holder starves behind the spinners and the leg deadlocks. The native gate does not
/// need this (its blocked writers sleep on the native handler, freeing their threads).
#[test]
#[ignore = "forced-spin falsifiability control; run explicitly with -- --ignored"]
fn forced_spin_control_blows_the_ratio() {
    let parallelism = available_parallelism();
    assert!(
        parallelism >= 2,
        "forced-spin control needs a >= 2-vCPU runner"
    );
    let writers = parallelism.clamp(MIN_WRITERS, 16);

    // A non-yielding spin pins a thread per writer; give the holder spare threads so it never starves.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(writers + 4)
        .enable_all()
        .build()
        .expect("forced-spin runtime");

    runtime.block_on(async move {
        // Serialize against the gate + WAL control (this control burns tens of CPU-seconds).
        let _serial = GATE_SERIAL.lock().await;

        println!(
            "\n=== forced-spin control (busy_timeout = 0; tight non-yielding spin; \
             expect R >> {PROVISIONAL_RATIO_CEILING}) ==="
        );

        // Baseline leg: own files (no contention even at busy_timeout=0 → no spin, honest cost).
        // `base_dirs` keeps the temp files alive for the leg.
        let mut base_stores = Vec::with_capacity(writers);
        let mut base_dirs = Vec::with_capacity(writers);
        for _ in 0..writers {
            let (store, dir) = open_fresh_store(BusyMode::ForcedSpin).await;
            store.testkit_set_checkpoint_interval(0).await;
            warmup(&store).await;
            base_stores.push(Arc::new(store));
            base_dirs.push(dir);
        }
        let baseline =
            timed_write_storm(&base_stores, writers, Concurrency::Sequential, "spin-baseline").await;
        drop(base_dirs);

        // Contended leg: one shared file, busy_timeout=0 → losers spin on a pinned thread.
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("unblock.db");
        let stores = open_shared_instances(&path, writers, BusyMode::ForcedSpin).await;
        for store in &stores {
            store.testkit_set_checkpoint_interval(0).await;
            warmup(store).await;
        }
        let contended =
            timed_write_storm(&stores, writers, Concurrency::Parallel, "spin-contended").await;

        let ratio = contended.cpu_per_write() / baseline.cpu_per_write();
        println!(
            "[forced-spin] baseline cpu/write={:.3}us contended cpu/write={:.3}us R={ratio:.2} \
             busy_retries(contended)={}",
            baseline.cpu_per_write() * 1e6,
            contended.cpu_per_write() * 1e6,
            contended.busy_retries
        );

        assert!(
            contended.busy_retries > 0,
            "forced-spin control: the contended leg must record spins (busy_retries > 0)"
        );
        assert!(
            ratio > PROVISIONAL_RATIO_CEILING,
            "forced-spin control FAILED to blow the ratio: R = {ratio:.2} did not exceed \
             {PROVISIONAL_RATIO_CEILING} — the metric would NOT detect a real hot-spin (the gate would \
             be vacuous). Investigate."
        );
        println!(
            "=== forced-spin control PASS: R = {ratio:.2} >> {PROVISIONAL_RATIO_CEILING} \
             (the metric detects a hot-spin) ===\n"
        );
    });
}

/// WAL NEGATIVE CONTROL (falsifiability): the same sustained contention window with the periodic
/// passive checkpoint **disabled** — the `-wal` sidecar must **breach** [`WAL_CEILING_BYTES`], proving
/// the positive WAL-bound assertion is falsifiable (a real checkpoint is doing the bounding, not the
/// ceiling being trivially large). `#[ignore]`d: it deliberately lets the WAL grow unbounded.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "WAL negative control; run explicitly with -- --ignored"]
async fn wal_negative_control_breaches_ceiling() {
    // Serialize against the other tests (the forced-spin control's CPU must not bleed in, and this
    // window must not perturb a concurrent measuring leg).
    let _serial = GATE_SERIAL.lock().await;

    let parallelism = available_parallelism();
    let writers = parallelism.clamp(MIN_WRITERS, 16);

    println!(
        "\n=== WAL negative control (checkpoint DISABLED; expect -wal > {WAL_CEILING_BYTES} bytes) ==="
    );

    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("unblock.db");
    let stores = open_shared_instances(&path, writers, BusyMode::Native).await;
    // Checkpoint DISABLED for the whole window (the negative control).
    for store in &stores {
        store.testkit_set_checkpoint_interval(0).await;
    }

    let writes = sustained_update_storm(&stores, writers).await;
    let wal_bytes = wal_size(&path);
    println!("[wal-negative] writes={writes} wal_bytes={wal_bytes} ceiling={WAL_CEILING_BYTES}");

    assert!(
        wal_bytes > WAL_CEILING_BYTES,
        "WAL negative control FAILED to breach the ceiling: -wal = {wal_bytes} bytes did not exceed \
         {WAL_CEILING_BYTES} with the checkpoint OFF — the positive WAL-bound assertion would be \
         vacuous (the ceiling is too large or the window too small). Investigate."
    );
    println!(
        "=== WAL negative control PASS: -wal {wal_bytes} bytes > ceiling {WAL_CEILING_BYTES} \
         (the periodic checkpoint is what bounds it) ===\n"
    );
}

// --------------------------------------------------------------------------------------------------
// Shared helpers
// --------------------------------------------------------------------------------------------------

/// The number of logical CPUs available to this process (floored at 1).
fn available_parallelism() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
}

/// The current size of the `-wal` sidecar for the DB at `db_path`, or 0 if absent.
///
/// `SQLite` names the WAL file `<db>-wal` (the suffix is appended to the full DB filename), so this
/// builds the sibling path explicitly rather than via `with_extension`.
fn wal_size(db_path: &Path) -> u64 {
    let mut name = db_path.as_os_str().to_os_string();
    name.push("-wal");
    std::fs::metadata(std::path::PathBuf::from(name)).map_or(0, |m| m.len())
}

/// How a store's connections treat the busy handler.
#[derive(Clone, Copy)]
enum BusyMode {
    /// Production: native sleep-based `busy_timeout = 5000` (blocks, never spins).
    Native,
    /// Forced-spin control: `busy_timeout = 0` (losers surface `SQLITE_BUSY` and spin-retry).
    ForcedSpin,
}

/// Open a fresh, migrated store on its **own** temp dir (returns the dir so it outlives the store).
async fn open_fresh_store(mode: BusyMode) -> (LibsqlStorage, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("unblock.db");
    let store = open_one(&path, mode).await;
    store.migrate().await.expect("migrate");
    (store, dir)
}

/// Open `writers` independent instances sharing one file `path`: migrate the FIRST instance FULLY
/// (establishing WAL + schema + draining the bootstrap TRUNCATE), THEN open the rest **serially**
/// (never `join_all`). All open/migrate happens outside any timed bracket; an open error is a harness
/// failure.
async fn open_shared_instances(
    path: &Path,
    writers: usize,
    mode: BusyMode,
) -> Vec<Arc<LibsqlStorage>> {
    let first = open_one(path, mode).await;
    first.migrate().await.expect("first-instance migrate");
    let mut stores = vec![Arc::new(first)];
    for _ in 1..writers {
        let store = open_one(path, mode).await;
        // migrate() is idempotent: the schema is already stamped, so this is a no-op no-op version
        // check (it must NOT re-bootstrap). Serial, never concurrent.
        store
            .migrate()
            .await
            .expect("subsequent-instance migrate (no-op)");
        stores.push(Arc::new(store));
    }
    stores
}

/// Open one instance at `path` per the [`BusyMode`].
async fn open_one(path: &Path, mode: BusyMode) -> LibsqlStorage {
    match mode {
        BusyMode::Native => LibsqlStorage::open_local(path).await.expect("open_local"),
        BusyMode::ForcedSpin => LibsqlStorage::open_local_with_busy_timeout(path, 0)
            .await
            .expect("open_local_with_busy_timeout(0)"),
    }
}

/// A small warmup: a couple of throwaway writes outside the timed bracket so first-touch costs (page
/// cache, prepared-statement compilation) do not land in the measured CPU.
async fn warmup(store: &LibsqlStorage) {
    static WARMUP_SEQ: AtomicU32 = AtomicU32::new(0);
    for _ in 0..2 {
        let seq = WARMUP_SEQ.fetch_add(1, Ordering::Relaxed);
        let id = format!("ub-warmup-{seq}");
        // A create then a tombstone; ignore DatabaseLocked (warmup is best-effort).
        let _ = store.create_issue(&seed_issue(&id), "warmup").await;
    }
}

/// A minimal valid issue at the fixed epoch.
fn seed_issue(id: &str) -> Issue {
    Issue {
        id: id.to_string(),
        title: format!("seed {id}"),
        created_at: Utc
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .unwrap_or_else(Utc::now),
        updated_at: Utc
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .unwrap_or_else(Utc::now),
        ..Issue::default()
    }
}

/// Sum the witnessed busy-retry counter across all instances.
async fn sum_busy(stores: &[Arc<LibsqlStorage>]) -> u64 {
    let mut total = 0u64;
    for store in stores {
        total += store.testkit_busy_retry_count().await;
    }
    total
}

/// Count durable rows whose id starts with `prefix` (via a fresh `list`-style read on the reader).
async fn count_prefix(store: &LibsqlStorage, prefix: &str) -> u64 {
    use unblock_model::ListFilters;
    // A generous limit so we never truncate the disjoint-create set.
    let filters = ListFilters {
        limit: Some(100_000),
        include_closed: true,
        ..ListFilters::default()
    };
    store
        .list_issues(&filters)
        .await
        .expect("list_issues")
        .into_iter()
        .filter(|i| i.id.starts_with(prefix))
        .count() as u64
}

/// Print the full diagnostic block (on EVERY run; capture with `-- --nocapture`).
#[allow(clippy::too_many_arguments)]
fn print_diagnostics(
    baseline: &LegStats,
    contended: &LegStats,
    ratio: f64,
    correctness: &CorrectnessStats,
    wal: &WalStats,
    integrity: &[String],
) {
    println!("\n----- contention lab diagnostics -----");
    println!(
        "BASELINE : cpu={:?} wall={:?} writes={} cpu/write={:.3}us busy_retries={}",
        baseline.cpu,
        baseline.wall,
        baseline.writes,
        baseline.cpu_per_write() * 1e6,
        baseline.busy_retries
    );
    println!(
        "CONTENDED: cpu={:?} wall={:?} writes={} cpu/write={:.3}us busy_retries={}",
        contended.cpu,
        contended.wall,
        contended.writes,
        contended.cpu_per_write() * 1e6,
        contended.busy_retries
    );
    println!(
        "RATIO R  : {ratio:.3}  (provisional ceiling {PROVISIONAL_RATIO_CEILING:.1}; \
         blocking ~1.0-1.2, spin >> 5)"
    );
    println!(
        "WITNESS  : baseline busy_retries={} contended busy_retries={}",
        baseline.busy_retries, contended.busy_retries
    );
    println!(
        "CORRECT  : claim_winners={} claim_losers={} | creates: attempts={} created={} locked={} | \
         updates: attempts={} committed={} locked={}",
        correctness.claim_winners,
        correctness.claim_losers,
        correctness.create_attempts,
        correctness.created,
        correctness.create_locked,
        correctness.update_attempts,
        correctness.update_committed,
        correctness.update_locked
    );
    println!(
        "WAL      : -wal={} bytes ceiling={} bytes checkpoints={} writes={}",
        wal.wal_bytes, WAL_CEILING_BYTES, wal.checkpoints, wal.writes
    );
    println!("INTEGRITY: {integrity:?} (empty = clean)");
    println!("--------------------------------------\n");
}
