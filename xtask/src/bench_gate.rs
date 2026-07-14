//! `bench-gate` — the NFR-1 HYBRID perf gate, tier ii (the hard per-PR absolute-ms ceiling, D34/F-1).
//!
//! `criterion` never fails a build on its own, so this is the explicit enforcement step: after
//! `cargo bench` writes `target/criterion/<group>/<param>/new/estimates.json`, this reads each
//! `mean.point_estimate` (nanoseconds) and **fails** (non-zero exit) if a measured op exceeds its
//! **generous absolute ceiling** ([`ceiling_ns`]). Ceilings are calibrated at T3.5 Implement against
//! the first real libsql/engine criterion run on the pinned ≥2-vCPU runner and are deliberately
//! generous (a gross O(N)→O(N²)/missing-index regression trips them; subtle drift is caught by the
//! SEPARATE advisory/nightly relative-10% delta, never a per-PR gate — D34/F-1).
//!
//! Scope (F-7): storage + engine (NFR-1) **and** the pre-existing policy (`cmp_ready_sort`) + render
//! (`render_issues`) benches are wired into this ONE gate — not demoted to informational.
//! `count`/`search` have no hard ceiling in v1 (PRD NFR-1) and are record-only, as is any unmapped
//! benchmark. Run: `cargo xtask bench-gate [<criterion-dir>]` (the optional dir override backs the
//! non-vacuity proof and testing; default is `${CARGO_TARGET_DIR:-target}/criterion`).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Nanoseconds per millisecond.
const NS_PER_MS: f64 = 1_000_000.0;

/// The hard per-PR ceiling (nanoseconds) for a criterion benchmark id (the `<group>/<param>` path),
/// or `None` when the op is **record-only** (no v1 hard ceiling).
///
/// The numbers are the T3.5-calibrated PROVISIONAL ceilings (PRD NFR-1 tier ii). They were widened
/// from the PR-0 provisional set against the first real libsql/engine run because the per-row-hydrated
/// read path (`collect_hydrated`) put `ready`/`list` above the original 5× ceilings — see the T3.5
/// calibration note in the commit body / PRD NFR-1.
fn ceiling_ns(bench: &str) -> Option<f64> {
    // Exact-id ceilings in MILLISECONDS (PRD NFR-1 tier ii, T3.5-calibrated). Same-ceiling
    // storage/engine ops share a row. An id absent from the table (storage_count/*, storage_search/*)
    // is record-only — no hard ceiling in v1 (PRD NFR-1); `render_issues/*` is matched by prefix below.
    const CEILINGS_MS: &[(&str, f64)] = &[
        // single mutation: storage insert / engine mint / engine claim (D34: the insert-path budget).
        ("storage_create/insert", 15.0),
        ("engine_create/mint", 15.0),
        ("engine_claim/claim", 15.0),
        // 1k read budgets (list/ready) — the N+1-hydration read path, T3.5-calibrated.
        ("storage_list/1000", 100.0),
        ("storage_ready/1000", 100.0),
        ("engine_list/1000", 100.0),
        ("engine_ready/1000", 100.0),
        // 10k read budgets.
        ("storage_list/10000", 1000.0),
        ("storage_ready/10000", 1000.0),
        ("engine_list/10000", 1000.0),
        ("engine_ready/10000", 1000.0),
        // engine JSONL I/O (export/import 10k).
        ("engine_export/10000", 2500.0),
        ("engine_import/10000", 5000.0),
        // policy comparator (F-7 — backs the NFR-1 ready re-rank cost).
        ("cmp_ready_sort/10000", 20.0),
        ("cmp_ready_sort/250000", 500.0),
    ];

    // render_issues/<fmt>/<size> — pure-CPU formatting (F-7): one generous ceiling per corpus size.
    if let Some(rest) = bench.strip_prefix("render_issues/") {
        if rest.ends_with("/1000") {
            return Some(10.0 * NS_PER_MS);
        }
        if rest.ends_with("/10000") {
            return Some(50.0 * NS_PER_MS);
        }
        return None;
    }

    CEILINGS_MS
        .iter()
        .find(|(id, _)| *id == bench)
        .map(|(_, ceiling_ms)| ceiling_ms * NS_PER_MS)
}

/// Benchmarks that MUST be present for a non-vacuous pass. If any is missing the benches were not run
/// (or a group was renamed) — the gate fails loudly rather than silently passing (the `check-layering`
/// empty-metadata guard precedent).
const REQUIRED: &[&str] = &[
    "storage_create/insert",
    "storage_ready/10000",
    "storage_list/10000",
    "engine_export/10000",
    "engine_import/10000",
    "cmp_ready_sort/250000",
    "render_issues/json/10000",
];

/// One parsed benchmark result.
struct Bench {
    /// The `<group>/<param>` criterion id.
    id: String,
    /// The `mean.point_estimate` in nanoseconds.
    mean_ns: f64,
}

/// Resolve the criterion output directory: the optional CLI override, else
/// `${CARGO_TARGET_DIR:-target}/criterion` relative to the current dir (the workspace root under
/// `cargo xtask`).
fn criterion_dir(override_dir: Option<String>) -> PathBuf {
    if let Some(dir) = override_dir {
        return PathBuf::from(dir);
    }
    let target =
        std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| PathBuf::from("target"), PathBuf::from);
    target.join("criterion")
}

/// Recursively collect every `<...>/new/estimates.json` under `root`, deriving its `<group>/<param>`
/// id (the path from `root` with the trailing `/new/estimates.json` stripped, `/`-normalised).
fn collect(root: &Path) -> Vec<Bench> {
    let mut out = Vec::new();
    collect_into(root, root, &mut out);
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

fn collect_into(root: &Path, dir: &Path, out: &mut Vec<Bench>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_into(root, &path, out);
        } else if path.file_name().and_then(|n| n.to_str()) == Some("estimates.json")
            && path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                == Some("new")
            && let Some(bench) = parse_bench(root, &path)
        {
            out.push(bench);
        }
    }
}

/// Parse one `new/estimates.json` into a [`Bench`], or `None` if the path/JSON is malformed.
fn parse_bench(root: &Path, estimates: &Path) -> Option<Bench> {
    // <root>/<group>/<param>/new/estimates.json → id = "<group>/<param>".
    let rel = estimates.strip_prefix(root).ok()?;
    let mut comps: Vec<String> = rel
        .components()
        .filter_map(|c| c.as_os_str().to_str().map(str::to_string))
        .collect();
    // Drop the trailing "new" + "estimates.json".
    comps.pop()?; // estimates.json
    comps.pop()?; // new
    if comps.is_empty() {
        return None;
    }
    let id = comps.join("/");

    let bytes = std::fs::read(estimates).ok()?;
    let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let mean_ns = json.get("mean")?.get("point_estimate")?.as_f64()?;
    Some(Bench { id, mean_ns })
}

/// Entry point for `cargo xtask bench-gate [<criterion-dir>]`.
#[must_use]
pub fn bench_gate(override_dir: Option<String>) -> ExitCode {
    let root = criterion_dir(override_dir);
    if !root.is_dir() {
        eprintln!(
            "bench-gate: no criterion output at {} — run `cargo bench` (with `--features testkit` \
             for storage/engine) first",
            root.display()
        );
        return ExitCode::FAILURE;
    }

    let benches = collect(&root);
    if benches.is_empty() {
        eprintln!(
            "bench-gate: no `new/estimates.json` under {} — benches did not run",
            root.display()
        );
        return ExitCode::FAILURE;
    }

    let mut enforced = 0usize;
    let mut violations: Vec<String> = Vec::new();
    let mut present: Vec<&str> = Vec::new();

    println!("bench-gate (NFR-1 tier-ii absolute ceilings, D34):");
    for bench in &benches {
        present.push(&bench.id);
        let mean_ms = bench.mean_ns / NS_PER_MS;
        match ceiling_ns(&bench.id) {
            Some(ceiling) => {
                enforced += 1;
                let ceiling_ms = ceiling / NS_PER_MS;
                if bench.mean_ns > ceiling {
                    violations.push(format!(
                        "  FAIL  {:<28} mean {:>10.3} ms  >  ceiling {:>8.1} ms",
                        bench.id, mean_ms, ceiling_ms
                    ));
                    println!(
                        "  FAIL  {:<28} mean {:>10.3} ms  >  ceiling {:>8.1} ms",
                        bench.id, mean_ms, ceiling_ms
                    );
                } else {
                    println!(
                        "  ok    {:<28} mean {:>10.3} ms  <= ceiling {:>8.1} ms",
                        bench.id, mean_ms, ceiling_ms
                    );
                }
            }
            None => {
                println!(
                    "  rec   {:<28} mean {:>10.3} ms  (record-only)",
                    bench.id, mean_ms
                );
            }
        }
    }

    // Non-vacuity guard: every REQUIRED bench must be present (else the run was partial/renamed).
    let missing: Vec<&&str> = REQUIRED
        .iter()
        .filter(|req| !present.contains(req))
        .collect();
    if !missing.is_empty() {
        eprintln!(
            "\nbench-gate FAILED: required benchmarks missing (benches not run, or a group was \
             renamed without updating xtask/src/bench_gate.rs):"
        );
        for m in missing {
            eprintln!("  MISSING: {m}");
        }
        return ExitCode::FAILURE;
    }

    if violations.is_empty() {
        println!(
            "\nbench-gate OK: {enforced} enforced op(s) within the NFR-1 tier-ii ceilings \
             ({} bench(es) total).",
            benches.len()
        );
        ExitCode::SUCCESS
    } else {
        eprintln!("\nbench-gate FAILED (NFR-1 tier-ii ceiling exceeded):");
        for v in &violations {
            eprintln!("{v}");
        }
        eprintln!(
            "\nCeilings are pinned in xtask/src/bench_gate.rs (PRD NFR-1 tier ii, D34). A genuine \
             regression must be fixed; a re-baseline (a widened ceiling) must be replicated across \
             the number-equality sites (SF-2) and justified in the commit body."
        );
        ExitCode::FAILURE
    }
}
