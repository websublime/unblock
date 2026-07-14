//! `bench-compare` — the NFR-1 HYBRID perf gate, tier i ADVISORY relative-10% drift report
//! (D34/F-1).
//!
//! The hard per-PR enforcement is [`bench_gate`](crate::bench_gate) (a generous absolute-ms
//! ceiling). This is the SEPARATE, **report-only** advisory leg: after `cargo bench` writes
//! `target/criterion/<id>/new/estimates.json`, this reads each `mean.point_estimate` and compares
//! it to a **committed baseline** (`xtask/bench-baseline.json`, mean-ns per op captured from the
//! fixed read path). Any op more than **+10%** over its baseline is REPORTED — but the command
//! **ALWAYS exits 0**: it NEVER fails a PR and NEVER fails a nightly run (D34/F-1). It runs only on
//! the nightly / `workflow_dispatch` `fuzz-smoke` schedule, so subtle drift is surfaced without
//! gating any merge (the generous [`bench_gate`] ceiling remains the only per-PR enforcement).
//!
//! Reuses the already-present `serde_json` (no new dependency) and the [`bench_gate`] criterion
//! walk (one implementation of the `estimates.json` discovery).
//!
//! Run: `cargo xtask bench-compare [<criterion-dir>] [<baseline-json>]` (both optional overrides
//! back the non-vacuity proof + testing).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::bench_gate::{collect, criterion_dir};

/// Nanoseconds per millisecond.
const NS_PER_MS: f64 = 1_000_000.0;

/// The advisory regression threshold: a measured mean more than this multiple of its committed
/// baseline is REPORTED (never fails the run) — the tier-i relative-10% delta (D34/F-1).
const REGRESSION_THRESHOLD: f64 = 1.10;

/// The committed baseline path (relative to the workspace root under `cargo xtask`).
const DEFAULT_BASELINE: &str = "xtask/bench-baseline.json";

/// Parse the committed baseline JSON: a flat `{ "<group>/<param>": <mean_ns>, ... }` map. A
/// non-numeric value (e.g. a leading `_comment` string documenting the capture) is ignored.
fn load_baseline(path: &Path) -> Option<BTreeMap<String, f64>> {
    let bytes = std::fs::read(path).ok()?;
    let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let obj = json.as_object()?;
    let mut out = BTreeMap::new();
    for (key, value) in obj {
        if let Some(mean_ns) = value.as_f64() {
            out.insert(key.clone(), mean_ns);
        }
    }
    Some(out)
}

/// The advisory percentage (`+10%`) for messages.
fn threshold_pct() -> f64 {
    (REGRESSION_THRESHOLD - 1.0) * 100.0
}

/// Entry point for `cargo xtask bench-compare [<criterion-dir>] [<baseline-json>]`.
///
/// ADVISORY / report-only: it prints a per-op relative delta against the committed baseline and
/// flags any op more than +10% slower, but **ALWAYS** exits 0 — it never fails a PR or a nightly
/// run (D34/F-1). A missing baseline, a missing criterion dir, or an op absent from the baseline are
/// all reported and skipped, never a failure.
#[must_use]
pub fn bench_compare(override_dir: Option<String>, baseline_override: Option<String>) -> ExitCode {
    let root = criterion_dir(override_dir);
    let baseline_path =
        baseline_override.map_or_else(|| PathBuf::from(DEFAULT_BASELINE), PathBuf::from);

    println!(
        "bench-compare (NFR-1 tier-i advisory relative-{:.0}% delta, D34/F-1 — REPORT-ONLY, never \
         fails a PR/nightly):",
        threshold_pct()
    );

    let Some(baseline) = load_baseline(&baseline_path) else {
        println!(
            "  (no readable baseline at {} — skipping the advisory comparison; report-only)",
            baseline_path.display()
        );
        return ExitCode::SUCCESS;
    };
    if !root.is_dir() {
        println!(
            "  (no criterion output at {} — run `cargo bench` first; report-only)",
            root.display()
        );
        return ExitCode::SUCCESS;
    }

    let benches = collect(&root);
    let mut compared = 0usize;
    let mut regressions = 0usize;
    for bench in &benches {
        let Some(&base_ns) = baseline.get(&bench.id) else {
            continue; // Not tracked in the committed baseline — nothing to compare.
        };
        compared += 1;
        let ratio = if base_ns > 0.0 {
            bench.mean_ns / base_ns
        } else {
            f64::INFINITY
        };
        let delta_pct = (ratio - 1.0) * 100.0;
        let now_ms = bench.mean_ns / NS_PER_MS;
        let baseline_ms = base_ns / NS_PER_MS;
        let tag = if ratio > REGRESSION_THRESHOLD {
            regressions += 1;
            "REGRESSION"
        } else {
            "ok        "
        };
        println!(
            "  {tag}  {:<28} {:>9.3} ms  vs baseline {:>9.3} ms  ({delta_pct:+.1}%)",
            bench.id, now_ms, baseline_ms
        );
    }

    if compared == 0 {
        println!("  (no measured op matched a baseline entry — nothing to compare; report-only)");
    } else if regressions == 0 {
        println!(
            "\nbench-compare: {compared} op(s) within +{:.0}% of baseline (no advisory drift).",
            threshold_pct()
        );
    } else {
        println!(
            "\nbench-compare: {regressions}/{compared} op(s) exceeded the advisory +{:.0}% delta \
             (REPORT-ONLY — this never fails a PR/nightly; investigate a persistent regression, and \
             re-capture the baseline on the pinned runner if it is a legitimate new floor).",
            threshold_pct()
        );
    }

    // ADVISORY: always succeed — the hard per-PR enforcement is `bench-gate` (D34/F-1).
    ExitCode::SUCCESS
}
