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
