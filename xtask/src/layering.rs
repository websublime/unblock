//! `check-layering` — acyclic crate-graph enforcement (NFR-15).
//!
//! Reads the *resolved* workspace metadata (`cargo metadata`) — not by text-parsing manifests — so
//! it sees feature-gated `dep:` edges, renames, and path-vs-registry forms correctly. Run:
//! `cargo xtask check-layering`. The CI `layering` job wires this in (ci-cd §2).

use std::collections::{BTreeMap, BTreeSet};
use std::process::{Command, ExitCode};

/// Allowed *internal* (workspace) dependency edges per crate (NFR-15 layering).
///
/// Source of truth: PRD §8.1 + design-spine §0. Every internal normal-kind edge must
/// appear here; `unblock-cli -> unblock-mcp` is the only intra-L7 edge; no edge may
/// point upward in the layer order
/// `model|error -> policy -> storage -> sync|health -> config -> engine -> render -> mcp|cli`.
fn allowed_edges() -> BTreeMap<&'static str, BTreeSet<&'static str>> {
    let table: &[(&str, &[&str])] = &[
        ("unblock-error", &[]),
        ("unblock-model", &["unblock-error"]),
        ("unblock-policy", &["unblock-model", "unblock-error"]),
        ("unblock-storage", &["unblock-model", "unblock-error"]),
        (
            "unblock-sync",
            &["unblock-storage", "unblock-model", "unblock-error"],
        ),
        (
            "unblock-health",
            &["unblock-model", "unblock-error", "unblock-sync"],
        ),
        (
            "unblock-config",
            &[
                "unblock-storage",
                "unblock-sync",
                "unblock-health",
                "unblock-model",
                "unblock-error",
            ],
        ),
        (
            "unblock-engine",
            &[
                "unblock-config",
                "unblock-sync",
                "unblock-storage",
                "unblock-policy",
                "unblock-health",
                "unblock-model",
                "unblock-error",
            ],
        ),
        ("unblock-render", &["unblock-model", "unblock-error"]),
        (
            "unblock-mcp",
            &[
                "unblock-engine",
                "unblock-render",
                "unblock-policy",
                "unblock-model",
                "unblock-error",
            ],
        ),
        (
            "unblock-cli",
            &[
                "unblock-engine",
                // DIRECT dep (D27/AF-3, T3.1): the cli NAMES `CliOverrides`/`open_*_with_cli`/
                // `WorkspaceContext`/`ConfigError` — a valid L7 -> L4 downward edge, no cycle.
                "unblock-config",
                "unblock-render",
                "unblock-policy",
                // DIRECT dep (D27/AF-3, T3.1): `normalize_prefix` for `init --prefix` — L7 -> L0.
                "unblock-model",
                "unblock-mcp",
                "unblock-error",
            ],
        ),
        (
            "unblock-fuzz",
            &[
                "unblock-model",
                "unblock-sync",
                "unblock-storage",
                "unblock-error",
            ],
        ),
        ("xtask", &[]),
    ];
    table
        .iter()
        .map(|(k, v)| (*k, v.iter().copied().collect()))
        .collect()
}

/// Entry point for `cargo xtask check-layering`.
#[must_use]
pub fn check_layering() -> ExitCode {
    let cargo = option_env!("CARGO").unwrap_or("cargo");
    let output = match Command::new(cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
        ])
        .output()
    {
        Ok(out) => out,
        Err(err) => {
            eprintln!("failed to invoke `cargo metadata`: {err}");
            return ExitCode::FAILURE;
        }
    };
    if !output.status.success() {
        eprintln!(
            "`cargo metadata` failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return ExitCode::FAILURE;
    }

    let meta: serde_json::Value = match serde_json::from_slice(&output.stdout) {
        Ok(value) => value,
        Err(err) => {
            eprintln!("failed to parse `cargo metadata` output: {err}");
            return ExitCode::FAILURE;
        }
    };

    let allowed = allowed_edges();
    let packages = meta["packages"].as_array().cloned().unwrap_or_default();
    let members: BTreeSet<&str> = packages
        .iter()
        .filter_map(|pkg| pkg["name"].as_str())
        .collect();

    // Guard against a vacuous pass: every crate named in the matrix must be a real
    // workspace member. Catches empty/garbled `cargo metadata` output and matrix drift
    // (a renamed/removed crate) before the edge loop can silently report "OK".
    let missing: Vec<&str> = allowed
        .keys()
        .copied()
        .filter(|name| !members.contains(name))
        .collect();
    if !missing.is_empty() {
        eprintln!(
            "layering check could not verify — `cargo metadata` listed no member for: {} \
             (empty/garbled metadata, or matrix drift in xtask/src/layering.rs)",
            missing.join(", ")
        );
        return ExitCode::FAILURE;
    }

    let mut violations: Vec<String> = Vec::new();
    let mut unlisted: Vec<String> = Vec::new();

    for pkg in &packages {
        let name = pkg["name"].as_str().unwrap_or_default();
        let Some(allow) = allowed.get(name) else {
            unlisted.push(name.to_owned());
            continue;
        };
        for dep in pkg["dependencies"].as_array().into_iter().flatten() {
            let dep_name = dep["name"].as_str().unwrap_or_default();
            // Only internal (workspace) edges form the layering graph.
            if !members.contains(dep_name) {
                continue;
            }
            // `kind` is null for normal deps; dev/build deps do not constrain layering.
            if matches!(dep["kind"].as_str(), Some("dev" | "build")) {
                continue;
            }
            if !allow.contains(dep_name) {
                violations.push(format!("  DISALLOWED: {name} -> {dep_name}"));
            }
        }
    }

    for name in &unlisted {
        eprintln!("WARN: workspace package not in the layering matrix: {name}");
    }

    if violations.is_empty() && unlisted.is_empty() {
        println!("layering OK: every internal crate edge conforms to the NFR-15 matrix");
        ExitCode::SUCCESS
    } else if violations.is_empty() {
        // An unlisted package is a matrix-maintenance gap, not a layering breach.
        eprintln!("layering check incomplete: update the matrix in xtask/src/layering.rs");
        ExitCode::FAILURE
    } else {
        eprintln!("LAYERING VIOLATIONS (NFR-15):");
        for line in &violations {
            eprintln!("{line}");
        }
        eprintln!(
            "\nallowed edges are pinned in xtask/src/layering.rs (source: PRD §8.1 + spine §0)"
        );
        ExitCode::FAILURE
    }
}
