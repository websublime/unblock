//! Corpus-green + non-vacuity integration test for the `knowledge-lint` gate (ci-cd §2.3).
//!
//! Mirrors the doc-lint proof pattern: the lint is only trustworthy if it (1) reports ZERO findings
//! on the REAL, in-tree `.knowledge/` corpus — running at the real repo root, so the real
//! `CLAUDE.md` / `docs/PROCESS.md` / `.unblock/issues.jsonl` exercise the out-of-tree point-reads —
//! and (2) FAILs hard when the skeleton is absent (the structure guard; a vacuous pass is not a
//! pass). The unit tests in `knowledge_lint.rs` cover planted violations per check k1..k6.

use std::path::PathBuf;

use xtask::knowledge_lint;

/// The workspace root = the parent of this crate's manifest dir.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask manifest has a parent (the workspace root)")
        .to_path_buf()
}

#[test]
fn real_knowledge_is_green() {
    let root = workspace_root();
    let findings =
        knowledge_lint::lint_at(&root).expect("the in-tree knowledge corpus is complete");
    assert!(
        findings.is_empty(),
        "knowledge-lint must be clean on the real tree, but found {} finding(s):\n{}",
        findings.len(),
        findings
            .iter()
            .map(|f| format!(
                "  {}:{}: [{}] {}",
                f.file,
                f.line,
                f.check.code(),
                f.message
            ))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

#[test]
fn missing_skeleton_fails_the_structure_guard() {
    // A directory without the skeleton must trip the structure guard — an absent skeleton is a
    // vacuous pass and is rejected (exactly the truncated-corpus test's shape).
    let nowhere = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let result = knowledge_lint::lint_at(&nowhere);
    assert!(
        result.is_err(),
        "the structure guard must FAIL without the skeleton, got {result:?}",
    );
}
