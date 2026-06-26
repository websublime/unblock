//! Mutation-path integration tests: engine-side validation (FR-11), delete `DryRun`, reparent-cycle
//! rejection (FR-5), close-with-suggestions newly-unblocked (FR-11), claim idempotency (FR-2).

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::parked::ParkedStorage;
use common::{add_blocks, issue, session, session_over};
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
async fn update_blank_title_is_model_error_and_db_unchanged() {
    // MUST-FIX A: storage update_issue is validation-free, so the engine must run the FULL
    // IssueValidator on the merged candidate. A whitespace-only title must be rejected as the
    // ModelError aggregate, and the row must keep its original title.
    let session = session().await;
    session
        .create(&issue("ub-0001", Priority::MEDIUM, 1000))
        .await
        .expect("create");
    let original_title = session
        .get("ub-0001")
        .await
        .expect("get")
        .expect("present")
        .title;

    let patch = IssuePatch {
        title: Some("   ".to_string()),
        ..IssuePatch::default()
    };
    let err = session
        .update("ub-0001", &patch)
        .await
        .expect_err("blank title rejected");
    assert!(matches!(err, EngineError::Model { .. }), "got {err:?}");
    assert_eq!(err.code(), ErrorCode::ValidationFailed);
    assert_eq!(err.code().exit_code(), 4);

    // DB unchanged: the title is still the original.
    let got = session.get("ub-0001").await.expect("get").expect("present");
    assert_eq!(
        got.title, original_title,
        "blank-title update must not persist"
    );
}

#[tokio::test]
async fn update_nul_byte_in_text_is_model_error_and_db_unchanged() {
    // MUST-FIX A: a NUL byte in any text field is rejected by the full validator on update.
    let session = session().await;
    session
        .create(&issue("ub-0001", Priority::MEDIUM, 1000))
        .await
        .expect("create");

    let patch = IssuePatch {
        description: Some(Some("bad\0desc".to_string())),
        ..IssuePatch::default()
    };
    let err = session
        .update("ub-0001", &patch)
        .await
        .expect_err("NUL rejected");
    assert!(matches!(err, EngineError::Model { .. }), "got {err:?}");
    assert_eq!(err.code(), ErrorCode::ValidationFailed);

    // DB unchanged: the NUL description was never persisted.
    let got = session.get("ub-0001").await.expect("get").expect("present");
    assert!(
        got.description.is_none(),
        "NUL description must not persist"
    );
}

#[tokio::test]
async fn update_invalid_external_ref_is_model_error_and_db_unchanged() {
    // MUST-FIX A: a whitespace-containing or over-length external_ref is rejected by the validator
    // on update (it would otherwise persist unvalidated through the validation-free storage path).
    let session = session().await;
    session
        .create(&issue("ub-0001", Priority::MEDIUM, 1000))
        .await
        .expect("create");

    // Whitespace in external_ref → rejected.
    let whitespace = IssuePatch {
        external_ref: Some(Some("gh 12".to_string())),
        ..IssuePatch::default()
    };
    let err = session
        .update("ub-0001", &whitespace)
        .await
        .expect_err("whitespace external_ref rejected");
    assert!(matches!(err, EngineError::Model { .. }), "got {err:?}");
    assert_eq!(err.code(), ErrorCode::ValidationFailed);

    // Over-length external_ref (> 200 chars) → rejected.
    let over_length = IssuePatch {
        external_ref: Some(Some("x".repeat(201))),
        ..IssuePatch::default()
    };
    let err = session
        .update("ub-0001", &over_length)
        .await
        .expect_err("over-length external_ref rejected");
    assert!(matches!(err, EngineError::Model { .. }), "got {err:?}");

    // DB unchanged: external_ref was never set.
    let got = session.get("ub-0001").await.expect("get").expect("present");
    assert!(
        got.external_ref.is_none(),
        "invalid external_ref must not persist"
    );
}

#[tokio::test]
async fn update_nonexistent_issue_is_issue_not_found() {
    // The load-before-validate path surfaces IssueNotFound for a missing id (transparent storage
    // source), not a validation error.
    let session = session().await;
    let patch = IssuePatch {
        title: Some("new".to_string()),
        ..IssuePatch::default()
    };
    let err = session
        .update("ub-missing", &patch)
        .await
        .expect_err("missing issue");
    assert_eq!(err.code(), ErrorCode::IssueNotFound);
    assert_eq!(err.code().exit_code(), 3);
}

#[tokio::test]
async fn update_valid_patch_persists_after_full_validation() {
    // A legitimate update passes the full validator and persists (the validation gate does not
    // reject valid patches).
    let session = session().await;
    session
        .create(&issue("ub-0001", Priority::MEDIUM, 1000))
        .await
        .expect("create");

    let patch = IssuePatch {
        title: Some("renamed".to_string()),
        description: Some(Some("a real description".to_string())),
        priority: Some(Priority::HIGH),
        ..IssuePatch::default()
    };
    let updated = session
        .update("ub-0001", &patch)
        .await
        .expect("valid update");
    assert_eq!(updated.title, "renamed");
    assert_eq!(updated.description.as_deref(), Some("a real description"));
    assert_eq!(updated.priority, Priority::HIGH);
}

#[tokio::test]
async fn close_with_reason_persists_close_reason() {
    // T1.2: close_with_suggestions(id, Some(reason)) persists the reason to the close_reason column
    // (no longer tracing-only), and the newly-unblocked outcome stays correct.
    let session = session().await;
    for (id, secs) in [("ub-a", 1000), ("ub-b", 1001)] {
        session
            .create(&issue(id, Priority::MEDIUM, secs))
            .await
            .expect("create");
    }
    add_blocks(&session, "ub-b", "ub-a").await;

    let outcome = session
        .close_with_suggestions("ub-a", Some("shipped in v1".to_string()))
        .await
        .expect("close");
    assert_eq!(outcome.closed.status, unblock_model::Status::Closed);
    // The reason is persisted on the closed issue and on a fresh read.
    assert_eq!(
        outcome.closed.close_reason.as_deref(),
        Some("shipped in v1")
    );
    let reloaded = session.get("ub-a").await.expect("get").expect("present");
    assert_eq!(reloaded.close_reason.as_deref(), Some("shipped in v1"));

    // The newly-unblocked set is still correct (ub-b's only blocker resolved).
    let unblocked: Vec<String> = outcome.newly_unblocked.into_iter().map(|i| i.id).collect();
    assert_eq!(unblocked, vec!["ub-b".to_string()]);
}

#[tokio::test]
async fn close_without_reason_leaves_close_reason_none() {
    // A None reason leaves close_reason unset (the patch field stays None).
    let session = session().await;
    session
        .create(&issue("ub-solo", Priority::MEDIUM, 1000))
        .await
        .expect("create");
    let outcome = session
        .close_with_suggestions("ub-solo", None)
        .await
        .expect("close");
    assert!(outcome.closed.close_reason.is_none());
    let reloaded = session.get("ub-solo").await.expect("get").expect("present");
    assert!(reloaded.close_reason.is_none());
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
async fn cancelled_write_releases_permit_and_leaves_no_partial_state() {
    use unblock_storage::Storage;
    // NFR-5 / D14 cancel-safety (second half): a write parked mid-tx holds the engine permit; if its
    // future is dropped (cancelled) before commit, (a) the permit must be reusable by a subsequent
    // write AND (b) the parked op must leave NO partial state (its row is absent). The first half
    // (reads succeed during a held permit) lives in reads.rs.
    let inner = unblock_storage::LibsqlStorage::open_in_memory()
        .await
        .expect("open");
    inner.migrate().await.expect("migrate");

    let parked: Arc<ParkedStorage> = ParkedStorage::new(Arc::new(inner));
    let storage: Arc<dyn Storage> = parked.clone();
    let session = Arc::new(session_over(storage, unblock_engine::SessionConfig::default()).await);

    // Spawn a write that parks inside create_issue (holding the engine permit), then CANCEL it.
    let writer_session = session.clone();
    let writer = tokio::spawn(async move {
        writer_session
            .create(&issue("ub-parked", Priority::MEDIUM, 2000))
            .await
    });
    parked.wait_until_parked().await; // the permit is now held mid-tx.
    writer.abort(); // drop the future before it commits (cancellation).
    let _ = writer.await; // observe the JoinError (cancelled).

    // (a) The permit is reusable: a subsequent write through the SAME session completes promptly —
    //     if the cancelled writer had leaked the permit, this would hang.
    let next = tokio::time::timeout(
        Duration::from_secs(2),
        session.create(&issue("ub-after", Priority::MEDIUM, 3000)),
    )
    .await
    .expect("the permit must be reusable after a cancelled write")
    .expect("subsequent write succeeds");
    assert_eq!(next, "ub-after");

    // (b) No partial state: the parked op never ran its storage tx, so its row is absent.
    assert!(
        session.get("ub-parked").await.expect("get").is_none(),
        "the cancelled write must leave no partial row"
    );
    assert!(
        session.get("ub-after").await.expect("get").is_some(),
        "the subsequent write is durable"
    );
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
