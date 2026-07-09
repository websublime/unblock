//! NFR-5 gate (c) — long-lived single-workspace stress (T3.4/D30).
//!
//! A MODEST mixed-operation run over ONE workspace on the order of 10^3–10^4 ops
//! (create/update/close/dep/export/import) — explicitly NOT the 250k perf corpus (that is T3.5).
//! INTEGRITY-only assertions: `Session::integrity_check` clean throughout + linearizable (a stable,
//! consistent store) + no partial write (every export is a complete valid JSONL that round-trips into
//! a fresh session). NO latency/throughput budget. A default-CI-sized run PLUS an `#[ignore]`-gated
//! longer soak (the T3.2 FORK-3 default+gated-soak precedent).

mod common;
use common::{dep, issue, session_with_unblock_dir};

use unblock_engine::{ImportOptions, IssuePatch};
use unblock_model::{DependencyType, ListFilters, Priority};

/// The widest visibility (closed + deferred + tombstone) — matches what `export_jsonl` pulls, so the
/// source and round-tripped counts are comparable.
fn widest() -> ListFilters {
    ListFilters {
        include_closed: true,
        include_deferred: true,
        include_tombstone: true,
        ..ListFilters::default()
    }
}

/// Run `ops` mixed operations over ONE workspace, asserting integrity throughout + a no-partial-write
/// export/import round-trip at the end.
async fn run_stress(ops: usize) {
    let (session, tmp) = session_with_unblock_dir().await;
    let target = tmp.path().join(".unblock").join("issues.jsonl");

    let mut live: Vec<String> = Vec::with_capacity(ops);
    for i in 0..ops {
        // create (id-preserving path).
        let id = format!("ub-{i:06}");
        let created = i64::try_from(i).unwrap_or(i64::MAX);
        session
            .create(&issue(&id, Priority::MEDIUM, 1_000 + created))
            .await
            .expect("create");
        live.push(id);

        // update a recent issue's priority.
        if i % 4 == 3 && live.len() >= 2 {
            let target_id = &live[live.len() - 2];
            let patch = IssuePatch {
                priority: Some(Priority::HIGH),
                ..IssuePatch::default()
            };
            let _ = session.update(target_id, &patch).await; // ignore any not-found edge
        }
        // add a Blocks dep (a recent issue depends on an older one).
        if i % 7 == 6 && live.len() >= 3 {
            let from = live[live.len() - 1].clone();
            let on = live[live.len() - 3].clone();
            let _ = session
                .add_dep(&dep(&from, &on, DependencyType::Blocks))
                .await;
        }
        // close an older issue.
        if i % 5 == 4 && live.len() >= 4 {
            let older = live[live.len() - 4].clone();
            let _ = session.close_with_suggestions(&older, None).await;
        }

        // Periodic export + integrity checkpoint: no corruption / no partial write mid-run.
        if i % 250 == 249 {
            session
                .export_jsonl(&target)
                .await
                .expect("checkpoint export");
            let problems = session.integrity_check().await.expect("integrity_check");
            assert!(
                problems.is_empty(),
                "integrity_check must be clean at op {i}, got {problems:?}"
            );
            let content = std::fs::read_to_string(&target).expect("read checkpoint export");
            for line in content.lines() {
                let _: serde_json::Value =
                    serde_json::from_str(line).expect("every exported line is complete valid JSON");
            }
        }
    }

    // Final integrity + a no-partial-write export/import round-trip into a FRESH session.
    let final_problems = session.integrity_check().await.expect("integrity_check");
    assert!(
        final_problems.is_empty(),
        "final integrity_check must be clean, got {final_problems:?}"
    );

    session.export_jsonl(&target).await.expect("final export");
    let exported = std::fs::read_to_string(&target).expect("read final export");

    let (fresh, tmp2) = session_with_unblock_dir().await;
    let fresh_target = tmp2.path().join(".unblock").join("issues.jsonl");
    std::fs::write(&fresh_target, &exported).expect("stage fresh import");
    let report = fresh
        .import_jsonl(&fresh_target, ImportOptions::default())
        .await
        .expect("round-trip import");

    let source_count = session.list(&widest()).await.expect("source list").len();
    let fresh_count = fresh.list(&widest()).await.expect("fresh list").len();
    assert_eq!(
        fresh_count, source_count,
        "the whole corpus round-trips (no partial write / no lost issue)"
    );
    assert_eq!(
        report.imported, source_count,
        "the fresh import applied the whole corpus"
    );
    assert_eq!(
        source_count, ops,
        "every created issue survived the long-lived run"
    );
}

/// Default-CI-sized long-lived run (~2k ops — the 10^3 order; INTEGRITY-only, no perf budget).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stress_longlived_default_size() {
    run_stress(2_000).await;
}

/// The `#[ignore]`-gated longer soak (~20k ops — the 10^4 order; still NOT the 250k T3.5 corpus). Run
/// on demand: `cargo test -p unblock-engine --test stress_longlived -- --ignored`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "long soak; run on demand (NFR-5, T3.4) — not part of the default-CI stress-integrity gate"]
async fn stress_longlived_soak() {
    run_stress(20_000).await;
}
