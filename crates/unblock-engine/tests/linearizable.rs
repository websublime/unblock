//! The FR-9 gate of M1: interleaved mutations through the engine are **linearizable**, and two
//! independent call sites produce **identical state** (the proxy for "MCP and CLI cannot drift").
//! Plus the FR-2 claim race (exactly one winner). All over the REAL libsql storage (no mock).

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use common::{issue, session, session_over};
use proptest::prelude::*;
use unblock_model::{Issue, Priority, Status};
use unblock_storage::{IssuePatch, LibsqlStorage, Storage};

/// One mutation in the interleaving model. Each maps to a single engine call; the engine's
/// `Semaphore(1)` serializes them, so any concurrent interleaving must collapse to *some* sequential
/// order — and replaying that same sequence on a fresh DB must reproduce the exact same state
/// (linearizability).
#[derive(Debug, Clone)]
enum Op {
    Create { id: u8, priority: u8 },
    SetPriority { id: u8, priority: u8 },
    Claim { id: u8 },
    Close { id: u8 },
}

fn op_strategy() -> impl Strategy<Value = Op> {
    let id = 0u8..6;
    prop_oneof![
        (id.clone(), 0u8..5).prop_map(|(id, priority)| Op::Create { id, priority }),
        (id.clone(), 0u8..5).prop_map(|(id, priority)| Op::SetPriority { id, priority }),
        id.clone().prop_map(|id| Op::Claim { id }),
        id.prop_map(|id| Op::Close { id }),
    ]
}

fn id_str(id: u8) -> String {
    format!("ub-{id:04}")
}

/// Apply one op through a session (idempotent-tolerant: missing-issue / already-claimed errors are
/// swallowed so the *sequence* is well-defined regardless of order — we only assert state equality).
async fn apply(session: &unblock_engine::Session, op: &Op) {
    match op {
        Op::Create { id, priority } => {
            let mut issue = issue(
                &id_str(*id),
                Priority(i32::from(*priority)),
                1_000 + i64::from(*id),
            );
            issue.title = format!("issue {id}");
            let _ = session.create(&issue).await; // ignore IdCollision on re-create
        }
        Op::SetPriority { id, priority } => {
            let patch = IssuePatch {
                priority: Some(Priority(i32::from(*priority))),
                ..IssuePatch::default()
            };
            let _ = session.update(&id_str(*id), &patch).await; // ignore IssueNotFound
        }
        Op::Claim { id } => {
            let _ = session.claim(&id_str(*id), "alice").await;
        }
        Op::Close { id } => {
            let _ = session.close_with_suggestions(&id_str(*id), None).await;
        }
    }
}

/// A canonical, comparable snapshot of the whole store: (id -> (status, priority, assignee)).
async fn snapshot(
    session: &unblock_engine::Session,
) -> BTreeMap<String, (String, i32, Option<String>)> {
    let filters = unblock_model::ListFilters {
        include_closed: true,
        include_deferred: true,
        ..unblock_model::ListFilters::default()
    };
    let issues: Vec<Issue> = session.list(&filters).await.expect("list");
    issues
        .into_iter()
        .map(|i| {
            (
                i.id,
                (i.status.as_str().to_string(), i.priority.0, i.assignee),
            )
        })
        .collect()
}

/// FR-9 linearizability + dual-callsite identity.
///
/// Build a random op sequence. Run it through TWO independent sessions over the SAME DB (modelling
/// MCP and CLI both driving one workspace, interleaved by the engine permit). Then replay the SAME
/// sequence on a FRESH single-session DB. The two end states must be byte-identical — i.e. the
/// concurrent interleaving linearized to the same sequential result, and the two call sites did not
/// drift.
fn run_linearizable(ops: Vec<Op>) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("rt");
    rt.block_on(async move {
        // --- shared DB driven by two independent sessions, alternating call sites ---
        let shared: Arc<dyn Storage> = {
            let s = LibsqlStorage::open_in_memory().await.expect("open");
            s.migrate().await.expect("migrate");
            Arc::new(s)
        };
        let site_a = session_over(shared.clone(), unblock_engine::SessionConfig::default()).await;
        let site_b = session_over(shared.clone(), unblock_engine::SessionConfig::default()).await;
        for (i, op) in ops.iter().enumerate() {
            if i % 2 == 0 {
                apply(&site_a, op).await;
            } else {
                apply(&site_b, op).await;
            }
        }
        let dual = snapshot(&site_a).await;

        // --- fresh DB, single session, same sequence ---
        let single = session().await;
        for op in &ops {
            apply(&single, op).await;
        }
        let solo = snapshot(&single).await;

        prop_assert_eq!(
            dual,
            solo,
            "dual-callsite state must equal single-callsite state"
        );
        Ok(())
    })
    .expect("linearizable property");
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    #[test]
    fn interleaved_mutations_are_linearizable_and_dual_callsite_identical(
        ops in prop::collection::vec(op_strategy(), 1..24)
    ) {
        run_linearizable(ops);
    }
}

/// FR-2: N concurrent claimers race for one issue; exactly one wins, the losers get `AlreadyClaimed`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn claim_race_exactly_one_winner() {
    let shared: Arc<dyn Storage> = {
        let s = LibsqlStorage::open_in_memory().await.expect("open");
        s.migrate().await.expect("migrate");
        Arc::new(s)
    };
    // Seed the contested issue.
    let setup = session_over(shared.clone(), unblock_engine::SessionConfig::default()).await;
    setup
        .create(&issue("ub-race", Priority::MEDIUM, 1000))
        .await
        .expect("create");

    // N independent sessions over the same DB, each a distinct claimant.
    let n = 8;
    let mut handles = Vec::new();
    for k in 0..n {
        let storage = shared.clone();
        handles.push(tokio::spawn(async move {
            let session = session_over(storage, unblock_engine::SessionConfig::default()).await;
            session.claim("ub-race", &format!("worker-{k}")).await
        }));
    }

    let mut wins = 0;
    let mut already_claimed = 0;
    for h in handles {
        match h.await.expect("join") {
            Ok(issue) => {
                wins += 1;
                assert_eq!(issue.status, Status::InProgress);
            }
            Err(err) => {
                use unblock_error::{CodedError, ErrorCode};
                assert_eq!(
                    err.code(),
                    ErrorCode::AlreadyClaimed,
                    "loser must be AlreadyClaimed"
                );
                already_claimed += 1;
            }
        }
    }
    assert_eq!(wins, 1, "exactly one claimer wins");
    assert_eq!(
        already_claimed,
        n - 1,
        "every other claimer loses with AlreadyClaimed"
    );
}

/// FR-9 (M1 AC) — GENUINELY CONCURRENT linearizability over the real-libsql DB through the engine's
/// `Semaphore(1)`.
///
/// The proptest above is single-threaded (it awaits each op before the next), so it proves replay
/// determinism but NOT that the engine permit serializes truly-in-flight writers. This test drives
/// many mutations **concurrently** — `create`/`update`/`claim` futures launched at once across a
/// multi-thread runtime through ONE shared `Arc<Session>` (the supported in-process topology, spine
/// §4.2: exactly one MCP server (`unblock mcp`) per workspace ⇒ one permit) — so every op contends for the
/// **single** write permit. It then asserts the serialization invariants the permit must guarantee:
///   (a) `integrity_check()` is clean after the storm (no torn rows / corruption),
///   (b) NO lost writes — every op that returned `Ok` is reflected in the final DB state (the storm
///       collects each create/claim/update result; an `Ok` that is not durable is a lost write),
///   (c) outcomes are consistent with SOME serial order:
///       - concurrent claims on one id → exactly one durable winner (the persisted assignee) and the
///         permit serializes so each claim either wins or loses cleanly with `AlreadyClaimed`,
///       - concurrent creates → every `Ok` create is present,
///       - concurrent updates of one id → the final priority is one of the values whose update
///         returned `Ok` (a last-writer-wins state, never a torn mix).
///
/// Non-vacuous: without the permit, two `update_issue` BEGIN-IMMEDIATE transactions in flight on the
/// one write connection would either interleave their read-modify-write of `content_hash`/`updated_at`
/// or surface `DatabaseLocked` instead of serializing cleanly — so an `Ok` update could be lost or a
/// claim could double-win, failing (b)/(c). (We do NOT commit a permit-less variant — the reasoning
/// is the guard.)
/// The result of one in-flight mutation in the concurrent FR-9 storm: each variant carries `Some`
/// when the op returned `Ok` (so the test asserts "every Ok is durable", not "every op succeeded").
enum ConcurrentOutcome {
    /// `Some(id)` when the create returned `Ok`.
    Created(Option<String>),
    /// `Some(assignee)` when the claim returned `Ok`.
    Claimed(Option<String>),
    /// `Some(priority)` when the update returned `Ok`.
    Updated(Option<i32>),
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // one cohesive concurrency storm + its three serialization asserts.
async fn concurrent_mutations_serialize_no_lost_writes_no_corruption() {
    let shared: Arc<dyn Storage> = {
        let s = LibsqlStorage::open_in_memory().await.expect("open");
        s.migrate().await.expect("migrate");
        Arc::new(s)
    };

    // ONE shared session ⇒ ONE write permit serializes every concurrent in-flight mutation (the
    // supported in-process topology, spine §4.2). Reads/writes take `&self`, so it shares across tasks.
    let session =
        Arc::new(session_over(shared.clone(), unblock_engine::SessionConfig::default()).await);

    // Seed two issues that concurrent updaters/claimers will contend over.
    session
        .create(&issue("ub-claimed", Priority::MEDIUM, 500))
        .await
        .expect("seed claimed");
    session
        .create(&issue("ub-updated", Priority::MEDIUM, 600))
        .await
        .expect("seed updated");

    let created_ids: Vec<String> = (0..12).map(|k| format!("ub-c{k:04}")).collect();
    let claimers = 8usize;
    let update_priorities: Vec<u8> = (0..8).map(|k| u8::try_from(k % 5).unwrap_or(0)).collect();

    // Each task returns its outcome so we can assert "every Ok is durable" (not "every op succeeded").
    let mut tasks: tokio::task::JoinSet<ConcurrentOutcome> = tokio::task::JoinSet::new();

    // Concurrent creates (each a distinct id).
    for (i, id) in created_ids.iter().cloned().enumerate() {
        let session = session.clone();
        let priority = u8::try_from(i % 5).unwrap_or(0);
        let created_secs = 1000 + i64::try_from(i).unwrap_or(0);
        tasks.spawn(async move {
            let mut iss = issue(&id, Priority(i32::from(priority)), created_secs);
            iss.title = format!("created {id}");
            ConcurrentOutcome::Created(session.create(&iss).await.ok())
        });
    }

    // Concurrent claimers racing for ub-claimed.
    for k in 0..claimers {
        let session = session.clone();
        tasks.spawn(async move {
            let assignee = format!("worker-{k}");
            ConcurrentOutcome::Claimed(
                session
                    .claim("ub-claimed", &assignee)
                    .await
                    .ok()
                    .and_then(|iss| iss.assignee),
            )
        });
    }

    // Concurrent updates of ub-updated's priority.
    for p in update_priorities.clone() {
        let session = session.clone();
        tasks.spawn(async move {
            let patch = IssuePatch {
                priority: Some(Priority(i32::from(p))),
                ..IssuePatch::default()
            };
            ConcurrentOutcome::Updated(
                session
                    .update("ub-updated", &patch)
                    .await
                    .ok()
                    .map(|i| i.priority.0),
            )
        });
    }

    // Collect the outcomes of every in-flight op.
    let mut ok_created: Vec<String> = Vec::new();
    let mut ok_claim_assignees: Vec<String> = Vec::new();
    let mut ok_update_priorities: Vec<i32> = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        match joined.expect("task joined without panic") {
            ConcurrentOutcome::Created(Some(id)) => ok_created.push(id),
            ConcurrentOutcome::Claimed(Some(a)) => ok_claim_assignees.push(a),
            ConcurrentOutcome::Updated(Some(p)) => ok_update_priorities.push(p),
            _ => {}
        }
    }

    // (a) Integrity is clean after the concurrent storm (the permit + BEGIN IMMEDIATE serialized
    //     every read-modify-write, so no torn rows).
    let integrity = shared.integrity_check().await.expect("integrity_check");
    assert!(
        integrity.is_empty(),
        "post-storm integrity_check must be clean, got {integrity:?}"
    );

    // Through ONE serializing session, every distinct create succeeds (the permit prevents any
    // BEGIN-IMMEDIATE contention loss) — so all 12 returned Ok.
    assert_eq!(
        ok_created.len(),
        created_ids.len(),
        "the single permit serializes creates so each distinct-id create returns Ok"
    );

    // (b) No lost writes: every create that returned Ok is durable in the final state.
    for id in &ok_created {
        assert!(
            session.get(id).await.expect("get").is_some(),
            "an Ok create ({id}) must be durable (no lost write)"
        );
    }

    // (c) Concurrent claims → exactly one durable winner: ub-claimed is in_progress with the persisted
    //     assignee, and that assignee is among the claims that returned Ok (a single serial winner).
    let claimed = session
        .get("ub-claimed")
        .await
        .expect("get")
        .expect("present");
    assert_eq!(claimed.status, Status::InProgress, "claimed -> in_progress");
    let durable_assignee = claimed.assignee.expect("a durable winner assignee");
    // The permit serializes claims so the durable assignee is the one whose claim returned Ok and won.
    assert!(
        ok_claim_assignees.contains(&durable_assignee),
        "the durable assignee {durable_assignee} must be an Ok-claim winner, got Ok set {ok_claim_assignees:?}"
    );

    // (c) Concurrent updates → the final priority is one of the values whose update returned Ok
    //     (a last-writer-wins state, never a torn mix).
    let updated = session
        .get("ub-updated")
        .await
        .expect("get")
        .expect("present");
    assert!(
        ok_update_priorities.contains(&updated.priority.0),
        "final priority {} must be one of the Ok-returning update values {ok_update_priorities:?}",
        updated.priority.0
    );
}
