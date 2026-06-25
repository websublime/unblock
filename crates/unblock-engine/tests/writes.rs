//! Mutation-path integration tests: engine-side validation (FR-11), delete `DryRun`, reparent-cycle
//! rejection (FR-5), close-with-suggestions newly-unblocked (FR-11), claim idempotency (FR-2).

mod common;

use common::{add_blocks, issue, session};
use unblock_engine::{DeleteMode, DeletePlan, EngineError, IssuePatch};
use unblock_error::{CodedError, ErrorCode};
use unblock_model::{Dependency, DependencyType, Priority};

#[tokio::test]
async fn create_validation_runs_in_engine_as_model_aggregate_not_flattened() {
    let session = session().await;
    // An invalid issue: empty title + out-of-range priority. The engine validates FIRST, so this
    // must surface as the ModelError aggregate (VALIDATION_FAILED, exit 4) — NOT a flattened
    // StorageError::InvalidId.
    let mut bad = issue("ub-0001", Priority(9), 1000);
    bad.title = "   ".to_string();

    let err = session.create(&bad).await.expect_err("invalid");
    assert!(
        matches!(err, EngineError::Model { .. }),
        "must be the engine ModelError source, got {err:?}"
    );
    assert_eq!(err.code(), ErrorCode::ValidationFailed);
    assert_eq!(err.code().exit_code(), 4);

    // Nothing was created (validation rejected before storage).
    assert!(session.get("ub-0001").await.expect("get").is_none());
}

#[tokio::test]
async fn update_out_of_range_priority_is_model_error() {
    let session = session().await;
    session
        .create(&issue("ub-0001", Priority::MEDIUM, 1000))
        .await
        .expect("create");

    let patch = IssuePatch {
        priority: Some(Priority(99)),
        ..IssuePatch::default()
    };
    let err = session
        .update("ub-0001", &patch)
        .await
        .expect_err("invalid");
    assert!(matches!(err, EngineError::Model { .. }));
    assert_eq!(err.code(), ErrorCode::ValidationFailed);

    // The issue is unchanged (priority still MEDIUM).
    let got = session.get("ub-0001").await.expect("get").expect("present");
    assert_eq!(got.priority, Priority::MEDIUM);
}

#[tokio::test]
async fn delete_dry_run_mutates_nothing() {
    let session = session().await;
    session
        .create(&issue("ub-0001", Priority::MEDIUM, 1000))
        .await
        .expect("create");

    let plan = DeletePlan {
        mode: DeleteMode::DryRun,
        targets: vec!["ub-0001".to_string()],
        cascade_children: Vec::new(),
    };
    let resolved = session.delete(&plan).await.expect("dry-run");
    assert_eq!(resolved.mode, DeleteMode::DryRun);

    // The issue is still present and open (DryRun mutated nothing).
    let got = session.get("ub-0001").await.expect("get").expect("present");
    assert_eq!(got.status, unblock_model::Status::Open);
}

#[tokio::test]
async fn reparent_cycle_is_rejected() {
    let session = session().await;
    // A gating cycle: a Blocks b, then b Blocks a would close a cycle and must be rejected.
    session
        .create(&issue("ub-a", Priority::MEDIUM, 1000))
        .await
        .expect("c");
    session
        .create(&issue("ub-b", Priority::MEDIUM, 1001))
        .await
        .expect("c");
    add_blocks(&session, "ub-a", "ub-b").await; // a depends on b

    // b depends on a -> closes a -> b -> a gating cycle.
    let cyclic = Dependency {
        issue_id: "ub-b".to_string(),
        depends_on_id: "ub-a".to_string(),
        dep_type: DependencyType::Blocks,
        created_at: chrono::Utc::now(),
        created_by: Some("tester".to_string()),
        metadata: None,
        thread_id: None,
    };
    let err = session.add_dep(&cyclic).await.expect_err("cycle");
    assert_eq!(err.code(), ErrorCode::CycleDetected);
    assert_eq!(err.code().exit_code(), 5);
}

#[tokio::test]
async fn close_with_suggestions_returns_newly_unblocked() {
    let session = session().await;
    // ub-b is blocked by ub-a; ub-c is blocked by ub-a too; closing ub-a unblocks both.
    for (id, secs) in [("ub-a", 1000), ("ub-b", 1001), ("ub-c", 1002)] {
        session
            .create(&issue(id, Priority::MEDIUM, secs))
            .await
            .expect("create");
    }
    add_blocks(&session, "ub-b", "ub-a").await;
    add_blocks(&session, "ub-c", "ub-a").await;

    let outcome = session
        .close_with_suggestions("ub-a", Some("done".to_string()))
        .await
        .expect("close");
    assert_eq!(outcome.closed.id, "ub-a");
    assert_eq!(outcome.closed.status, unblock_model::Status::Closed);

    let unblocked: Vec<String> = outcome.newly_unblocked.into_iter().map(|i| i.id).collect();
    assert_eq!(unblocked, vec!["ub-b".to_string(), "ub-c".to_string()]);
}

#[tokio::test]
async fn close_with_no_dependents_yields_empty_newly_unblocked() {
    let session = session().await;
    session
        .create(&issue("ub-solo", Priority::MEDIUM, 1000))
        .await
        .expect("create");
    let outcome = session
        .close_with_suggestions("ub-solo", None)
        .await
        .expect("close");
    assert!(outcome.newly_unblocked.is_empty());
}

#[tokio::test]
async fn claim_same_actor_is_idempotent() {
    let session = session().await;
    session
        .create(&issue("ub-0001", Priority::MEDIUM, 1000))
        .await
        .expect("create");

    let first = session.claim("ub-0001", "alice").await.expect("claim");
    assert_eq!(first.assignee.as_deref(), Some("alice"));
    // Re-claiming what you already hold is an idempotent Ok (FR-2, spine §3.2.1).
    let second = session
        .claim("ub-0001", "alice")
        .await
        .expect("re-claim ok");
    assert_eq!(second.assignee.as_deref(), Some("alice"));
}

#[tokio::test]
async fn defer_excludes_from_ready_then_undefer_restores() {
    let session = session().await;
    session
        .create(&issue("ub-0001", Priority::MEDIUM, 1000))
        .await
        .expect("create");

    session
        .defer("ub-0001", chrono::Utc::now() + chrono::Duration::days(3))
        .await
        .expect("defer");
    assert!(
        session
            .ready(&unblock_model::ListFilters::default())
            .await
            .expect("ready")
            .is_empty()
    );

    session.undefer("ub-0001").await.expect("undefer");
    let ready = session
        .ready(&unblock_model::ListFilters::default())
        .await
        .expect("ready");
    assert_eq!(ready.len(), 1);
}
