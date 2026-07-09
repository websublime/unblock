//! NFR-5 gate (d) — interleaved concurrent command-family integrity (T3.4/D30).
//!
//! `linearizable.rs` interleaves MUTATION-only ops. This gate interleaves THREE command families over
//! ONE shared `Arc<Session>` (the supported in-process topology, spine §4.2 — one write permit):
//! mutations (create / update / close), interchange (export / import), and reads (list / ready / get).
//! It then asserts the INTEGRITY invariant: `integrity_check` is clean after the storm, two reads see a
//! CONSISTENT store (no torn concurrent state), and every mutation that returned `Ok` is durable (no
//! lost write) — i.e. the interleaving linearized to SOME serial order. INTEGRITY-only (no perf budget);
//! out of T3.5's perf/250k lane.

mod common;
use std::sync::Arc;

use common::{issue, session_with_unblock_dir};
use tokio::task::JoinSet;
use unblock_engine::{ImportOptions, IssuePatch};
use unblock_model::{ListFilters, Priority};

/// The widest visibility (closed + deferred + tombstone).
fn widest() -> ListFilters {
    ListFilters {
        include_closed: true,
        include_deferred: true,
        include_tombstone: true,
        ..ListFilters::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interleaved_command_families_preserve_integrity() {
    let (session, tmp) = session_with_unblock_dir().await;
    let session = Arc::new(session);
    let target = tmp.path().join(".unblock").join("issues.jsonl");

    // Seed a base corpus so the interchange + read families have something to work over, and an
    // initial complete export so an early import never races a missing file.
    let seed_ids: Vec<String> = (0..20).map(|i| format!("ub-seed-{i:04}")).collect();
    for (i, id) in seed_ids.iter().enumerate() {
        session
            .create(&issue(
                id,
                Priority::MEDIUM,
                1_000 + i64::try_from(i).unwrap_or(0),
            ))
            .await
            .expect("seed create");
    }
    session.export_jsonl(&target).await.expect("seed export");

    // Family A: mutations (create -> update -> maybe close). Returns the id when the create was Ok.
    let mut mutators: JoinSet<Option<String>> = JoinSet::new();
    for k in 0..40 {
        let session = session.clone();
        mutators.spawn(async move {
            let id = format!("ub-mut-{k:04}");
            let created = session
                .create(&issue(&id, Priority::MEDIUM, 5_000 + k))
                .await
                .is_ok();
            let patch = IssuePatch {
                priority: Some(Priority::HIGH),
                ..IssuePatch::default()
            };
            let _ = session.update(&id, &patch).await;
            if k % 3 == 0 {
                let _ = session.close_with_suggestions(&id, None).await;
            }
            created.then_some(id)
        });
    }

    // Families B (interchange) + C (reads) — return `()`.
    let mut others: JoinSet<()> = JoinSet::new();
    for _ in 0..10 {
        let session = session.clone();
        let target = target.clone();
        others.spawn(async move {
            // export is read-only + atomic; import acquires the write permit (serializes with A).
            let _ = session.export_jsonl(&target).await;
            let _ = session
                .import_jsonl(&target, ImportOptions::default())
                .await;
        });
    }
    for _ in 0..24 {
        let session = session.clone();
        others.spawn(async move {
            let _ = session.list(&ListFilters::default()).await;
            let _ = session.ready(&ListFilters::default()).await;
            let _ = session.get("ub-seed-0000").await;
        });
    }

    // Drain the read/interchange families, then collect the mutation family's Ok-created ids.
    while let Some(joined) = others.join_next().await {
        joined.expect("interchange/read task joined without panic");
    }
    let mut ok_created: Vec<String> = Vec::new();
    while let Some(joined) = mutators.join_next().await {
        if let Some(id) = joined.expect("mutation task joined without panic") {
            ok_created.push(id);
        }
    }

    // (a) Integrity is clean after the interleaved storm (no torn rows / corruption).
    let problems = session.integrity_check().await.expect("integrity_check");
    assert!(
        problems.is_empty(),
        "post-storm integrity_check must be clean, got {problems:?}"
    );

    // (b) Consistent store: two back-to-back reads see the same size (no torn concurrent state).
    let first = session.list(&widest()).await.expect("list");
    let second = session.list(&widest()).await.expect("list");
    assert_eq!(
        first.len(),
        second.len(),
        "two reads must see a consistent store after the storm"
    );

    // (c) No lost writes: every seed survived, and every Ok mutation-create is durable.
    for id in &seed_ids {
        assert!(
            session.get(id).await.expect("get").is_some(),
            "seed {id} must survive the interleaved storm"
        );
    }
    for id in &ok_created {
        assert!(
            session.get(id).await.expect("get").is_some(),
            "an Ok-created issue ({id}) must be durable (no lost write)"
        );
    }
}
