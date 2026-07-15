//! Corpus-green + non-vacuity integration test for the `verify-pins` gate (NFR-9).
//!
//! Mirrors `doc_lint_corpus.rs`: a SHA-pin gate is only trustworthy if it (1) reports ZERO findings on
//! the REAL, in-tree workflows, (2) FAILS hard when the workflows dir is missing/empty (the vacuous-pass
//! guard `scan_at` promises — the primary anti-false-green net; a refactor to `Ok((vec![], 0))` would ship
//! silently GREEN untested), and (3) DETECTS a planted floating `uses:` end-to-end over a directory TREE
//! (the walk + `scan_workflow` composition, not just the line-checker the unit tests cover).

use std::fs;
use std::path::{Path, PathBuf};

use xtask::verify_pins::scan_at;

/// The workspace root = the parent of this crate's manifest dir.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask manifest has a parent (the workspace root)")
        .to_path_buf()
}

/// Create `<root>/.github/workflows/<name>` with `body`.
fn write_workflow(root: &Path, name: &str, body: &str) {
    let dir = root.join(".github").join("workflows");
    fs::create_dir_all(&dir).expect("mkdir .github/workflows");
    fs::write(dir.join(name), body).expect("write fixture workflow");
}

#[test]
fn real_workflows_are_all_pinned() {
    let (findings, scanned) = scan_at(&workspace_root()).expect("the in-tree workflows dir exists");
    assert!(
        findings.is_empty(),
        "verify-pins must be clean on the real workflows, but found {} finding(s):\n{}",
        findings.len(),
        findings
            .iter()
            .map(|f| format!("  {}:{}: {} — {}", f.file, f.line, f.uses, f.reason))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    assert!(
        scanned >= 3,
        "expected >= 3 workflow files (ci.yml, fuzz-smoke.yml, release.yml), scanned {scanned}"
    );
}

#[test]
fn missing_workflows_dir_is_rejected() {
    // A tree with no `.github/workflows` at all must trip the vacuous-pass guard (read_dir Err arm) —
    // an absent corpus is an `Err`, not a silent GREEN pass.
    let nowhere = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    assert!(
        scan_at(&nowhere).is_err(),
        "scan_at over a tree with no .github/workflows must be Err (refuse a vacuous pass)"
    );
}

#[test]
fn empty_workflows_dir_is_rejected() {
    // An EXISTING-but-empty `.github/workflows` (no *.yml/*.yaml) must also be `Err` (the
    // `files.is_empty()` arm) — the explicit vacuous-pass guard the docstring promises.
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(tmp.path().join(".github").join("workflows")).expect("mkdir workflows");
    assert!(
        scan_at(tmp.path()).is_err(),
        "an empty .github/workflows dir must be Err (refuse a vacuous pass)"
    );
}

#[test]
fn fixture_tree_floating_ref_is_detected() {
    // scan_at end-to-end over a fixture TREE: a planted floating `uses: actions/attest@v4` (the exact
    // dist-template risk MF-5 calls out) must be reported.
    let tmp = tempfile::tempdir().expect("tempdir");
    write_workflow(
        tmp.path(),
        "release.yml",
        "jobs:\n  attest:\n    steps:\n      - uses: actions/attest@v4\n",
    );
    let (findings, scanned) = scan_at(tmp.path()).expect("existing non-empty workflows dir");
    assert_eq!(scanned, 1, "one fixture workflow scanned");
    assert_eq!(
        findings.len(),
        1,
        "the floating @v4 must be detected, got {findings:?}"
    );
    assert_eq!(findings[0].uses, "actions/attest@v4");
}

#[test]
fn fixture_tree_all_pinned_is_clean() {
    // The other side of non-vacuity: a fully SHA-pinned fixture tree returns Ok with zero findings.
    let tmp = tempfile::tempdir().expect("tempdir");
    write_workflow(
        tmp.path(),
        "ci.yml",
        "jobs:\n  build:\n    steps:\n      - uses: actions/checkout@08c6903cd8c0fde910a37f88322edcfb5dd907a8 # v5.0.0\n",
    );
    let (findings, _) = scan_at(tmp.path()).expect("existing non-empty workflows dir");
    assert!(
        findings.is_empty(),
        "a fully-pinned fixture must be clean, got {findings:?}"
    );
}
