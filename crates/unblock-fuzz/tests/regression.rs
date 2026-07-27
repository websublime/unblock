//! Stable-side regression replay + randomized smoke for every fuzz target core.
//!
//! This runs under plain `cargo test` (no nightly, no libFuzzer): it replays the **committed seed
//! corpus** (`fuzz/corpus/<target>/*`) through each `run_<t>_case` core and asserts `Ok`, and a
//! `proptest!` smoke feeds random bytes to each core for extra coverage. New crash artifacts get
//! their input committed under `fuzz/corpus/<target>/` so this guards against regressions without the
//! libFuzzer toolchain (NFR-12, the hard stable PR gate).

use std::path::PathBuf;

use proptest::prelude::*;

use unblock_fuzz::{
    run_content_hash_case, run_cycle_detect_case, run_dup_scan_case, run_enum_deserialize_case,
    run_id_alloc_case, run_issue_ingest_case, run_parse_id_case, run_query_filters_case,
    run_sanitize_case,
};

/// The nested cargo-fuzz package's corpus root (sibling of the member crate's `src/`).
fn corpus_dir(target: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fuzz")
        .join("corpus")
        .join(target)
}

/// Replay every committed seed under `fuzz/corpus/<target>/` through `run`, asserting `Ok`.
///
/// A missing or empty corpus directory is not a failure here (the smoke proptest still covers the
/// core); the seeds are an additional, meaningful, hand-authored guard.
fn replay_corpus(target: &str, run: impl Fn(&[u8]) -> Result<(), unblock_fuzz::FuzzError>) {
    let dir = corpus_dir(target);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let mut replayed = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            let bytes = std::fs::read(&path).expect("read corpus seed");
            run(&bytes).unwrap_or_else(|e| {
                panic!(
                    "seed {} for {target} returned an error: {e}",
                    path.display()
                )
            });
            replayed += 1;
        }
    }
    assert!(
        replayed > 0,
        "expected at least one committed seed under {}",
        dir.display()
    );
}

// --- corpus replay (one test per target) ---

#[test]
fn replay_content_hash() {
    replay_corpus("content_hash", run_content_hash_case);
}

#[test]
fn replay_issue_ingest() {
    replay_corpus("issue_ingest", run_issue_ingest_case);
}

#[test]
fn replay_parse_id() {
    replay_corpus("parse_id", run_parse_id_case);
}

#[test]
fn replay_enum_deserialize() {
    replay_corpus("enum_deserialize", run_enum_deserialize_case);
}

#[test]
fn replay_sanitize() {
    replay_corpus("sanitize", run_sanitize_case);
}

#[test]
fn replay_dup_scan() {
    replay_corpus("dup_scan", run_dup_scan_case);
}

#[test]
fn replay_query_filters() {
    replay_corpus("query_filters", run_query_filters_case);
}

#[test]
fn replay_cycle_detect() {
    replay_corpus("cycle_detect", run_cycle_detect_case);
}

#[test]
fn replay_id_alloc() {
    replay_corpus("id_alloc", run_id_alloc_case);
}

// --- randomized smoke (one proptest per target) ---

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn smoke_content_hash(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        prop_assert!(run_content_hash_case(&bytes).is_ok());
    }

    #[test]
    fn smoke_issue_ingest(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        prop_assert!(run_issue_ingest_case(&bytes).is_ok());
    }

    #[test]
    fn smoke_parse_id(bytes in prop::collection::vec(any::<u8>(), 0..128)) {
        prop_assert!(run_parse_id_case(&bytes).is_ok());
    }

    #[test]
    fn smoke_enum_deserialize(bytes in prop::collection::vec(any::<u8>(), 0..128)) {
        prop_assert!(run_enum_deserialize_case(&bytes).is_ok());
    }

    #[test]
    fn smoke_sanitize(bytes in prop::collection::vec(any::<u8>(), 0..256)) {
        prop_assert!(run_sanitize_case(&bytes).is_ok());
    }

    /// Random BYTES rarely produce valid JSON, so this leg mostly exercises the fail-closed
    /// `Indeterminate` path (and its "never stricter than rmcp" guard). The `Duplicate` arm is
    /// reached by the committed seeds and by the fixed-seed generator inside the core's own unit
    /// suite, which asserts an explicit minimum-duplicate floor.
    #[test]
    fn smoke_dup_scan(bytes in prop::collection::vec(any::<u8>(), 0..256)) {
        prop_assert!(run_dup_scan_case(&bytes).is_ok());
    }

    /// A grammar-shaped generator, so the `Clean`/`Duplicate` arms are genuinely reached rather
    /// than left to chance.
    #[test]
    fn smoke_dup_scan_wellformed(
        keys in prop::collection::vec("[a-c]{1,3}", 1..8),
    ) {
        let members: Vec<String> = keys
            .iter()
            .enumerate()
            .map(|(index, key)| format!("\"{key}\":{index}"))
            .collect();
        let document = format!("{{\"params\":{{{}}}}}", members.join(","));
        prop_assert!(run_dup_scan_case(document.as_bytes()).is_ok());
    }
}

// Storage smoke runs fewer cases (each opens a fresh file-backed DB; keep it cheap but real).
proptest! {
    #![proptest_config(ProptestConfig::with_cases(12))]

    #[test]
    fn smoke_query_filters(bytes in prop::collection::vec(any::<u8>(), 0..128)) {
        prop_assert!(run_query_filters_case(&bytes).is_ok());
    }

    #[test]
    fn smoke_cycle_detect(bytes in prop::collection::vec(any::<u8>(), 0..128)) {
        prop_assert!(run_cycle_detect_case(&bytes).is_ok());
    }

    #[test]
    fn smoke_id_alloc(bytes in prop::collection::vec(any::<u8>(), 0..128)) {
        prop_assert!(run_id_alloc_case(&bytes).is_ok());
    }
}
