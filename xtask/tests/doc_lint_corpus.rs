//! Corpus-green + non-vacuity integration test for the `doc-lint` gate.
//!
//! This mirrors how T0.2 proved the layering check (by injecting a back-edge): the doc-lint is only
//! trustworthy if it (1) reports ZERO findings on the REAL, in-tree corpus, and (2) FAILS hard when
//! the corpus is truncated (the existence / vacuous-pass guard). The unit tests in `doc_lint.rs`
//! cover one planted violation per class; this test pins the live docs and the guard.

use std::path::PathBuf;

use xtask::doc_lint;

/// The workspace root = the parent of this crate's manifest dir.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask manifest has a parent (the workspace root)")
        .to_path_buf()
}

#[test]
fn real_corpus_is_green() {
    let root = workspace_root();
    let findings = doc_lint::lint_at(&root).expect("the in-tree corpus is complete");
    assert!(
        findings.is_empty(),
        "doc-lint must be clean on the real corpus, but found {} finding(s):\n{}",
        findings.len(),
        findings
            .iter()
            .map(|f| format!("  {}:{}: [{}] {}", f.file, f.line, f.class, f.message))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

#[test]
fn real_corpus_has_no_surface_count_drift() {
    // Explicit class-(g) pin: the real corpus + the real capabilities.rs anchor (derived 8/5/3)
    // must yield ZERO MCP surface-count findings, so the D37-style 7->8 drift cannot reappear
    // un-annotated even if the aggregate green assertion above is ever relaxed.
    let root = workspace_root();
    let findings = doc_lint::lint_at(&root).expect("the in-tree corpus + anchor are complete");
    let class_g: Vec<_> = findings.iter().filter(|f| f.class == 'g').collect();
    assert!(
        class_g.is_empty(),
        "class-(g) surface-count drift on the real corpus:\n{}",
        class_g
            .iter()
            .map(|f| format!("  {}:{}: {}", f.file, f.line, f.message))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

#[test]
fn truncated_corpus_fails_the_existence_guard() {
    // A directory that is NOT the workspace root (the corpus files are absent) must trip the
    // existence guard — a smaller-than-expected corpus is a vacuous pass and is rejected.
    let nowhere = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let result = doc_lint::lint_at(&nowhere);
    assert!(
        result.is_err(),
        "the existence guard must FAIL on an incomplete corpus (vacuous-pass guard), got {result:?}",
    );
}
