//! Mutation-path integration tests: engine-side validation (FR-11), delete `DryRun`, reparent-cycle
//! rejection (FR-5), close-with-suggestions newly-unblocked (FR-11), claim idempotency (FR-2),
//! and the never-run Cascade/Hard/Tombstone/DryRun delete paths (FR-1c) + reparent guards (FR-1b).

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::parked::ParkedStorage;
use common::{add_blocks, issue, seed_hierarchy, session, session_over, t};
use unblock_engine::{DeleteMode, DeletePlan, EngineError, IssuePatch};
use unblock_error::{CodedError, ErrorCode};
use unblock_model::{Dependency, DependencyType, IssueType, Priority, Status};

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

/// M-E2 [AC #1] — a cycle-closing `add_dep` surfaces the `CycleDetected` path through the
/// Storage→Engine transparent mapping, and the path NAMES the cycle nodes (not just the code). The
/// transparent `EngineError::Storage` source is downcast to reach `StorageError::CycleDetected.path`.
#[tokio::test]
async fn add_dep_cycle_surfaces_named_path_through_engine() {
    let session = session().await;
    for (id, secs) in [("ub-a", 1000), ("ub-b", 1001)] {
        session
            .create(&issue(id, Priority::MEDIUM, secs))
            .await
            .expect("create");
    }
    add_blocks(&session, "ub-a", "ub-b").await; // a -> b
    // b -> a closes the cycle.
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

    // Reach the actual path: the transparent storage source carries `CycleDetected { path }`.
    match err {
        EngineError::Storage {
            source: unblock_storage::StorageError::CycleDetected { path },
        } => {
            assert!(
                path.contains("ub-a") && path.contains("ub-b"),
                "the engine-surfaced cycle path names the cycle nodes: {path}"
            );
            let nodes: Vec<&str> = path.split(" -> ").collect();
            assert_eq!(
                nodes.first(),
                nodes.last(),
                "the path is an ordered cycle [start, …, start]: {path}"
            );
            assert!(!path.contains('…'), "no synthetic placeholder: {path}");
        }
        other => panic!("expected a transparent CycleDetected, got {other:?}"),
    }
}

/// M-E5 — engine error-code mapping for the dependency write surface: duplicate →
/// `DuplicateDependency`, self-edge → `SelfDependency`, removing a missing edge →
/// `DependencyNotFound` (storage covers these; the engine forward was unproven).
#[tokio::test]
async fn add_remove_dep_error_codes_map_through_engine() {
    let session = session().await;
    for (id, secs) in [("ub-a", 1000), ("ub-b", 1001)] {
        session
            .create(&issue(id, Priority::MEDIUM, secs))
            .await
            .expect("create");
    }
    add_blocks(&session, "ub-a", "ub-b").await;

    // Duplicate edge.
    let dup = Dependency {
        issue_id: "ub-a".to_string(),
        depends_on_id: "ub-b".to_string(),
        dep_type: DependencyType::Blocks,
        created_at: chrono::Utc::now(),
        created_by: Some("tester".to_string()),
        metadata: None,
        thread_id: None,
    };
    assert_eq!(
        session.add_dep(&dup).await.expect_err("dup").code(),
        ErrorCode::DuplicateDependency
    );

    // Self-edge.
    let self_edge = Dependency {
        issue_id: "ub-a".to_string(),
        depends_on_id: "ub-a".to_string(),
        dep_type: DependencyType::Blocks,
        created_at: chrono::Utc::now(),
        created_by: Some("tester".to_string()),
        metadata: None,
        thread_id: None,
    };
    assert_eq!(
        session.add_dep(&self_edge).await.expect_err("self").code(),
        ErrorCode::SelfDependency
    );

    // Remove a non-existent edge.
    assert_eq!(
        session
            .remove_dep("ub-b", "ub-a", &DependencyType::Blocks)
            .await
            .expect_err("missing")
            .code(),
        ErrorCode::DependencyNotFound
    );
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

// --------------------------------------------------------------------------------------------------
// FR-1c: Cascade / Hard / Tombstone / DryRun delete through the Session (Gaps 1c, 1d, 2, 7)
//
// Per-affected Deleted-event + deleted_by-actor assertions MUST live at the storage layer
// (unblock_storage/tests/behaviour.rs Gap 1a/1b) because the Session read surface (read.rs) exposes
// no `list_events` / `list_dependencies` method. The engine layer asserts only what the Session
// surface exposes: post-state via `get`, the returned DeletePlan.cascade_children, and orphan-edge
// absence via `dependency_graph(&[])`. (S5 — T1.4 LOCKED spec.)
//
// The dotted-id corpus (ub-1, ub-1.1, ub-1.1.1, ub-1.2-Closed, ub-10, ub-2) is built by
// `common::seed_hierarchy`; reparent tests use a separate flat-id corpus (D-DRIFT-A).
// --------------------------------------------------------------------------------------------------

/// Gap 1c — Cascade delete through the Session tombstones target + all `{target}.%` descendants.
///
/// The Session passes `cascade_children: Vec::new()` to storage; storage resolves internally.
/// Asserts the returned plan's `cascade_children` is NON-EMPTY (proves the resolver ran), and that
/// every affected issue has `status == Tombstone` + `deleted_by == Some("tester")` via `get`.
/// Bounding witnesses `ub-10` (dot-boundary decoy) and `ub-2` (unrelated) remain Open.
#[tokio::test]
async fn delete_cascade_through_engine_tombstones_descendants() {
    let session = session().await;
    seed_hierarchy(&session).await;

    let plan = DeletePlan {
        mode: DeleteMode::Cascade,
        targets: vec!["ub-1".to_string()],
        cascade_children: Vec::new(),
    };
    let resolved = session.delete(&plan).await.expect("cascade delete");

    // The returned plan must list the full blast radius (sorted, non-empty).
    assert_eq!(resolved.mode, DeleteMode::Cascade);
    assert_eq!(
        resolved.cascade_children,
        vec![
            "ub-1.1".to_string(),
            "ub-1.1.1".to_string(),
            "ub-1.2".to_string()
        ],
        "cascade_children must be sorted and contain exactly the three dotted-prefix descendants"
    );

    // All four affected issues (target + three children) are Tombstone with deleted_by set.
    // NOTE: per-event assertions (Deleted event emitted for non-terminal only) live at
    // storage (Gap 1a, behaviour.rs) because Session has no list_events (S5).
    for id in ["ub-1", "ub-1.1", "ub-1.1.1", "ub-1.2"] {
        let fetched = session
            .get(id)
            .await
            .expect("get")
            .expect("must be present as tombstone");
        assert_eq!(
            fetched.status,
            Status::Tombstone,
            "{id} must be Tombstone after Cascade"
        );
        assert_eq!(
            fetched.deleted_by.as_deref(),
            Some("tester"),
            "{id} deleted_by must be the session actor"
        );
    }

    // Dot-boundary decoy ub-10 and unrelated ub-2 are UNTOUCHED.
    let decoy = session
        .get("ub-10")
        .await
        .expect("get")
        .expect("ub-10 must exist");
    assert_eq!(
        decoy.status,
        Status::Open,
        "ub-10 (dot-boundary decoy) must be untouched"
    );
    assert!(decoy.deleted_by.is_none(), "ub-10 must have no deleted_by");

    let unrelated = session
        .get("ub-2")
        .await
        .expect("get")
        .expect("ub-2 must exist");
    assert_eq!(
        unrelated.status,
        Status::Open,
        "ub-2 (unrelated) must be untouched"
    );
    assert!(
        unrelated.deleted_by.is_none(),
        "ub-2 must have no deleted_by"
    );
}

/// Gap 1d — Hard delete through the Session removes ONLY the target row.
///
/// Hard ≠ Cascade: `issues` has NO self-parent FK (schema.rs:34-78), so child issue rows SURVIVE.
/// (M1 fix — the prior spec-plan.md 1d wording was incorrect; this test proves the correct behaviour.)
///
/// `PRAGMA foreign_keys = ON` is verified by `pragmas_readback_in_memory` (mod.rs:702-727) and
/// `foreign_keys_enforced` (mod.rs:853). The FK CASCADE assertion (own dep/event rows gone) is
/// exercised at storage (Gap 1b). This test proves the through-engine observable facts only:
/// target absent, child SURVIVES, no orphan edges in the global graph. (S6 / S5)
#[tokio::test]
async fn delete_hard_through_engine_removes_target_only() {
    let session = session().await;
    // Minimal corpus: target + the child that MUST survive + an inbound edge for orphan verification.
    session
        .create(&issue("ub-1", Priority::MEDIUM, 100))
        .await
        .expect("create ub-1");
    session
        .create(&issue("ub-1.1", Priority::MEDIUM, 101))
        .await
        .expect("create ub-1.1");
    session
        .create(&issue("ub-2", Priority::MEDIUM, 102))
        .await
        .expect("create ub-2");
    // Inbound blocker: ub-2 --blocks--> ub-1 (inbound dep, no FK — must be cleaned by Hard delete).
    add_blocks(&session, "ub-2", "ub-1").await;

    let plan = DeletePlan {
        mode: DeleteMode::Hard,
        targets: vec!["ub-1".to_string()],
        cascade_children: Vec::new(),
    };
    let resolved = session.delete(&plan).await.expect("hard delete");

    // Mode is preserved in the returned plan.
    assert_eq!(resolved.mode, DeleteMode::Hard);

    // Target row is gone (Hard removes it entirely — no tombstone).
    assert!(
        session.get("ub-1").await.expect("get").is_none(),
        "Hard delete must remove the target row entirely"
    );

    // Hard ≠ Cascade: the child issue row SURVIVES (M1: `issues` has no self-parent FK).
    assert!(
        session.get("ub-1.1").await.expect("get").is_some(),
        "ub-1.1 (child issue row) must SURVIVE a Hard delete — Hard does not cascade issue rows"
    );

    // No orphan edges: the whole dependency graph has no edge referencing the deleted ub-1.
    let graph = session.dependency_graph(&[]).await.expect("graph");
    assert!(
        !graph
            .edges
            .iter()
            .any(|e| e.from == "ub-1" || e.to == "ub-1"),
        "no edge in the global graph may reference the hard-deleted ub-1"
    );
}

/// Gap 2 — Tombstone delete through the Session persists the row but the row is NOT patchable (FR-1c).
///
/// The existing `delete_dry_run_mutates_nothing` (writes.rs:257) tests `DryRun` only; this proves
/// the default Tombstone path: the row persists (status == `Tombstone`), contrasting Hard (row gone).
///
/// # DESIGN NOTE — tombstone retention is NOT live-update recovery; restore IS the recovery path
///
/// `update_issue` (crud.rs:332-334) intentionally rejects any patch on a tombstoned row with
/// `StorageError::IssueNotFound` — the "original rejects this" design from the classic `bd` system.
/// So a tombstone is NOT recoverable via the live `update` path: the row is *retained* (not
/// hard-deleted), and recovery is the dedicated `Session::restore` command (D20). **T1.7 has
/// landed**, so this test asserts BOTH sides: `update` still rejects a tombstone patch (restore is a
/// separate path — the two are not unified), AND `session.restore(id)` DOES recover the row. The
/// tombstoned row IS observable via `get` (the storage `get_issue` returns tombstones).
///
/// What this test proves (matching the actual storage contract):
/// - Tombstone row persists and is visible via `get` (row retained, not hard-deleted).
/// - `update(status: Open)` on a tombstone returns `IssueNotFound` by design (storage guard) —
///   i.e. the row is NOT patchable via `update`; recovery is `Session::restore`, not `update`.
/// - `session.restore(id)` recovers the row to active (the positive path — see the dedicated
///   `restore_through_engine_recovers_tombstone_*` tests for the full assertions).
/// - Contrast: Hard delete → `get` returns `None` (row gone entirely).
#[tokio::test]
async fn delete_tombstone_through_engine_persists_row_and_is_not_patchable() {
    let session = session().await;
    session
        .create(&issue("ub-0001", Priority::MEDIUM, 1000))
        .await
        .expect("create");

    let plan = DeletePlan {
        mode: DeleteMode::Tombstone,
        targets: vec!["ub-0001".to_string()],
        cascade_children: Vec::new(),
    };
    let resolved = session.delete(&plan).await.expect("tombstone delete");
    assert_eq!(resolved.mode, DeleteMode::Tombstone);

    // Row persists as a tombstone with actor propagation — this is the retained-on-soft-delete invariant (the recovery PATH itself is T1.7).
    // The row is retained (contrast Hard where get returns None), enabling future recovery.
    let tombstoned =
        session.get("ub-0001").await.expect("get").expect(
            "tombstoned row must still be present via get (row retained, not hard-deleted)",
        );
    assert_eq!(
        tombstoned.status,
        Status::Tombstone,
        "status must be Tombstone"
    );
    assert!(
        tombstoned.original_type.is_some(),
        "original_type must be preserved on Tombstone"
    );
    assert_eq!(
        tombstoned.deleted_by.as_deref(),
        Some("tester"),
        "deleted_by must be the session actor"
    );

    // The storage guard: update_issue intentionally rejects patches on tombstoned rows
    // (crud.rs:332-334, "A tombstone cannot be patched"). The row is retained but not patchable
    // via the live update path — recovery is the dedicated restore command (T1.7 — Session::restore), not this live-update path.
    let patch = IssuePatch {
        status: Some(Status::Open),
        ..IssuePatch::default()
    };
    let recover_err = session
        .update("ub-0001", &patch)
        .await
        .expect_err("update on tombstone must be rejected by storage (crud.rs:332-334)");
    assert_eq!(
        recover_err.code(),
        ErrorCode::IssueNotFound,
        "tombstone update must surface IssueNotFound (the storage guard returns IssueNotFound for tombstones)"
    );

    // T1.7 has landed: the DEDICATED restore path DOES recover the tombstone (the positive path,
    // cross-referenced from `restore_through_engine_recovers_tombstone_was_open`). Restore is
    // STRUCTURALLY separate from the rejected update path above — the two are not unified (D20).
    let restored = session
        .restore("ub-0001")
        .await
        .expect("Session::restore recovers a soft-deleted issue (T1.7)");
    assert_eq!(
        restored.status,
        Status::Open,
        "restore returns the (was-Open) tombstone to an active Open status"
    );
    assert!(
        restored.deleted_at.is_none() && restored.original_type.is_none(),
        "restore clears the tombstone fields + original_type"
    );
}

/// T1.7 positive path — `Session::restore` recovers a was-Open soft-deleted issue: create → delete
/// (Tombstone) → restore → `get` shows active, `original_type` cleared, `issue_type` preserved (D20).
#[tokio::test]
async fn restore_through_engine_recovers_tombstone_was_open() {
    let session = session().await;
    let mut bug = issue("ub-0001", Priority::MEDIUM, 1000);
    bug.issue_type = IssueType::Bug;
    session.create(&bug).await.expect("create");

    let plan = DeletePlan {
        mode: DeleteMode::Tombstone,
        targets: vec!["ub-0001".to_string()],
        cascade_children: Vec::new(),
    };
    session.delete(&plan).await.expect("tombstone delete");
    let tombstoned = session.get("ub-0001").await.expect("get").expect("present");
    assert_eq!(tombstoned.status, Status::Tombstone);
    assert_eq!(tombstoned.original_type.as_deref(), Some("bug"));

    let restored = session.restore("ub-0001").await.expect("restore");
    assert_eq!(restored.status, Status::Open);
    assert_eq!(restored.original_type, None, "original_type cleared");
    assert_eq!(
        restored.issue_type,
        IssueType::Bug,
        "issue_type preserved across restore"
    );

    // The recovered row is active and visible via get.
    let fetched = session.get("ub-0001").await.expect("get").expect("present");
    assert_eq!(fetched.status, Status::Open);
}

/// T1.7 — restore of an already-active issue is an idempotent `Ok` no-op (D20).
#[tokio::test]
async fn restore_through_engine_already_active_is_noop_ok() {
    let session = session().await;
    session
        .create(&issue("ub-0001", Priority::MEDIUM, 1000))
        .await
        .expect("create");

    let restored = session
        .restore("ub-0001")
        .await
        .expect("restore of an active issue is an idempotent Ok");
    assert_eq!(restored.status, Status::Open);
}

/// T1.7 — restore of a hard-deleted (gone) id surfaces `IssueNotFound` (restore is bounded to SOFT
/// deletes; no new `ErrorCode` minted — D20).
#[tokio::test]
async fn restore_through_engine_hard_deleted_is_issue_not_found() {
    let session = session().await;
    session
        .create(&issue("ub-0001", Priority::MEDIUM, 1000))
        .await
        .expect("create");
    let plan = DeletePlan {
        mode: DeleteMode::Hard,
        targets: vec!["ub-0001".to_string()],
        cascade_children: Vec::new(),
    };
    session.delete(&plan).await.expect("hard delete");

    let err = session
        .restore("ub-0001")
        .await
        .expect_err("restore of a hard-deleted id must fail");
    assert_eq!(err.code(), ErrorCode::IssueNotFound);
}

/// Gap 7 — `DryRun` over the hierarchical corpus reports the full blast-radius plan (FR-1c, M3).
///
/// The existing `delete_dry_run_mutates_nothing` (writes.rs:257) asserts only `mode==DryRun` +
/// nothing-mutated; it uses a flat single-issue corpus that leaves `cascade_children` empty.
/// This test proves the PLAN half: over the dotted-id hierarchy, `DryRun` must return a NON-EMPTY
/// `cascade_children == ["ub-1.1","ub-1.1.1","ub-1.2"]` AND leave every issue in its original state.
#[tokio::test]
async fn delete_dry_run_reports_plan_over_hierarchy() {
    let session = session().await;
    seed_hierarchy(&session).await;

    let plan = DeletePlan {
        mode: DeleteMode::DryRun,
        targets: vec!["ub-1".to_string()],
        cascade_children: Vec::new(),
    };
    let resolved = session.delete(&plan).await.expect("dry-run");

    // Mode is preserved.
    assert_eq!(resolved.mode, DeleteMode::DryRun);

    // The returned plan reports the full blast radius (NON-EMPTY — this is what was missing before).
    assert_eq!(
        resolved.cascade_children,
        vec![
            "ub-1.1".to_string(),
            "ub-1.1.1".to_string(),
            "ub-1.2".to_string()
        ],
        "DryRun must report the full cascade_children plan (non-empty) over the hierarchy"
    );

    // Nothing mutated: every issue is in its original state.
    let target = session
        .get("ub-1")
        .await
        .expect("get")
        .expect("ub-1 must exist");
    assert_eq!(
        target.status,
        Status::Open,
        "DryRun must not tombstone the target"
    );

    let child = session
        .get("ub-1.1")
        .await
        .expect("get")
        .expect("ub-1.1 must exist");
    assert_eq!(child.status, Status::Open, "DryRun must not affect ub-1.1");

    let grandchild = session
        .get("ub-1.1.1")
        .await
        .expect("get")
        .expect("ub-1.1.1 must exist");
    assert_eq!(
        grandchild.status,
        Status::Open,
        "DryRun must not affect ub-1.1.1"
    );

    // ub-1.2 was Closed (terminal) when seeded; DryRun must leave it Closed (not tombstoned).
    let closed_child = session
        .get("ub-1.2")
        .await
        .expect("get")
        .expect("ub-1.2 must exist");
    assert_eq!(
        closed_child.status,
        Status::Closed,
        "DryRun must not affect ub-1.2 (Closed)"
    );

    // Bounding witnesses are also untouched.
    let decoy = session
        .get("ub-10")
        .await
        .expect("get")
        .expect("ub-10 must exist");
    assert_eq!(
        decoy.status,
        Status::Open,
        "DryRun must not affect ub-10 (decoy)"
    );

    let unrelated = session
        .get("ub-2")
        .await
        .expect("get")
        .expect("ub-2 must exist");
    assert_eq!(
        unrelated.status,
        Status::Open,
        "DryRun must not affect ub-2 (unrelated)"
    );
}

// --------------------------------------------------------------------------------------------------
// FR-1b: Reparent cycle/self guard + updated_at advance + no-op (Gaps 3a, 3b, 5a, 5b)
//
// Reparent uses the PATCH path (Session::update with parent field), NOT add_dep — this is the
// never-run branch in apply_reparent (crud.rs:625-668) that the existing `reparent_cycle_is_rejected`
// (writes.rs:278) does NOT exercise (it uses add_dep directly). These tests prove the PATCH path.
// --------------------------------------------------------------------------------------------------

/// Gap 3a — Reparent-via-patch: closing a cycle is rejected with `CycleDetected` (exit 5).
///
/// Build edge ub-b -> ub-a via `update{parent: Some(Some("ub-a"))}`, then attempt to close the
/// cycle `ub-a -> ub-b -> ub-a` via `update("ub-a", {parent: Some(Some("ub-b"))})`.
/// The second update must fail with `CycleDetected` (exit 5) and leave the DB unchanged: the original
/// ub-b->ub-a edge survives (transaction rolled back), and ub-a has no new parent-child edge.
///
/// This exercises the PATCH path through `apply_reparent`/`would_cycle_in_tx` (crud.rs:650-668),
/// distinct from the `add_dep` path tested by the existing `reparent_cycle_is_rejected` (line 278).
#[tokio::test]
async fn reparent_via_patch_cycle_is_rejected() {
    let session = session().await;
    session
        .create(&issue("ub-a", Priority::MEDIUM, 1000))
        .await
        .expect("create ub-a");
    session
        .create(&issue("ub-b", Priority::MEDIUM, 1001))
        .await
        .expect("create ub-b");

    // First reparent: ub-b -> ub-a (parent-child edge). This must succeed.
    let patch_b_to_a = IssuePatch {
        parent: Some(Some("ub-a".to_string())),
        ..IssuePatch::default()
    };
    session
        .update("ub-b", &patch_b_to_a)
        .await
        .expect("first reparent ub-b -> ub-a must succeed");

    // Second reparent: ub-a -> ub-b would close a->b->a cycle. Must fail.
    let patch_a_to_b = IssuePatch {
        parent: Some(Some("ub-b".to_string())),
        ..IssuePatch::default()
    };
    let err = session
        .update("ub-a", &patch_a_to_b)
        .await
        .expect_err("cycle-closing reparent must be rejected");

    assert_eq!(
        err.code(),
        ErrorCode::CycleDetected,
        "must surface CycleDetected, got {err:?}"
    );
    assert_eq!(
        err.code().exit_code(),
        5,
        "CycleDetected must map to exit code 5"
    );
    // The reparent cycle path is the REAL ordered path naming both nodes (D2) — not a placeholder.
    match err {
        EngineError::Storage {
            source: unblock_storage::StorageError::CycleDetected { path },
        } => {
            assert!(
                path.contains("ub-a") && path.contains("ub-b") && !path.contains('…'),
                "the reparent cycle path names every node, no placeholder: {path}"
            );
        }
        other => panic!("expected a transparent CycleDetected, got {other:?}"),
    }

    // DB unchanged: the original ub-b->ub-a edge SURVIVES (the cyclic tx was rolled back).
    // Verify via the whole dependency graph — no parent-child edge FROM ub-a should exist,
    // and the ub-b->ub-a parent-child edge should still be present.
    let graph = session.dependency_graph(&[]).await.expect("graph");
    let has_a_to_b = graph
        .edges
        .iter()
        .any(|e| e.from == "ub-a" && e.to == "ub-b");
    assert!(
        !has_a_to_b,
        "the cyclic ub-a->ub-b edge must NOT be present (tx was rolled back)"
    );
    // The original ub-b->ub-a parent-child edge survives.
    let has_b_to_a = graph
        .edges
        .iter()
        .any(|e| e.from == "ub-b" && e.to == "ub-a");
    assert!(
        has_b_to_a,
        "the original ub-b->ub-a parent-child edge must survive the failed cyclic reparent"
    );
}

/// Gap 3b — Self-reparent via patch is rejected with `SelfDependency` (exit 5).
///
/// `apply_reparent` (crud.rs:650-651) returns `SelfDependency` BEFORE the cycle check when
/// `child_id == parent_id`. This branch is never-run; this test exercises it for the first time.
#[tokio::test]
async fn self_reparent_via_patch_is_rejected() {
    let session = session().await;
    session
        .create(&issue("ub-a", Priority::MEDIUM, 1000))
        .await
        .expect("create ub-a");

    let patch = IssuePatch {
        parent: Some(Some("ub-a".to_string())),
        ..IssuePatch::default()
    };
    let err = session
        .update("ub-a", &patch)
        .await
        .expect_err("self-reparent must be rejected");

    assert_eq!(
        err.code(),
        ErrorCode::SelfDependency,
        "must surface SelfDependency, got {err:?}"
    );
    assert_eq!(
        err.code().exit_code(),
        5,
        "SelfDependency must map to exit code 5"
    );
}

/// Gap 5a — `update` advances `updated_at` through the engine (FR-1b).
///
/// The engine `issue()` helper pins `created_at = updated_at = t(secs)`, a frozen past instant
/// (~1970). `update_issue` stamps `Utc::now()` — so strict `>` is safe here (S7: the frozen-past
/// `created_at` means `Utc::now()` is always later, making `>` sound, not a time-fragile flake).
/// The test also confirms the advance is persisted: a fresh `get` shows the new title AND the
/// advanced timestamp (not just the returned value).
#[tokio::test]
async fn update_advances_updated_at_through_engine() {
    let session = session().await;
    // `issue()` sets created_at = updated_at = t(1000) ≈ 1970-01-01T00:16:40Z (frozen past).
    session
        .create(&issue("ub-0001", Priority::MEDIUM, 1000))
        .await
        .expect("create");

    let created_at = session
        .get("ub-0001")
        .await
        .expect("get")
        .expect("present")
        .created_at;

    let patch = IssuePatch {
        title: Some("renamed".to_string()),
        ..IssuePatch::default()
    };
    let returned = session.update("ub-0001", &patch).await.expect("update");

    // Strict `>`: created_at is frozen to the past (t(1000) ≈ 1970), Utc::now() is always later.
    assert!(
        returned.updated_at > created_at,
        "update must advance updated_at (returned): {:?} must be > {:?}",
        returned.updated_at,
        created_at
    );
    assert_eq!(
        returned.title, "renamed",
        "returned value must carry the new title"
    );

    // Confirm the advance is persisted (not just a returned-value artifact).
    let persisted = session.get("ub-0001").await.expect("get").expect("present");
    assert!(
        persisted.updated_at > created_at,
        "update must advance updated_at (persisted): {:?} must be > {:?}",
        persisted.updated_at,
        created_at
    );
    assert_eq!(persisted.title, "renamed", "persisted title must match");
}

/// Gap 5b — No-op update leaves `updated_at` unchanged through the engine (FR-1b).
///
/// The no-EVENT half (storage writes no event on a no-op patch) is discharged at storage layer
/// (`noop_update_writes_no_event_and_leaves_updated_at`, behaviour.rs:100) — the Session read
/// surface has no `list_events`. This test covers only the through-engine timestamp half: a
/// `IssuePatch::default()` patch must leave `updated_at` identical to `created_at` (S8).
#[tokio::test]
async fn noop_update_is_detectable_through_engine() {
    let session = session().await;
    session
        .create(&issue("ub-0001", Priority::MEDIUM, 1000))
        .await
        .expect("create");

    let before = session
        .get("ub-0001")
        .await
        .expect("get")
        .expect("present")
        .updated_at;

    // A fully-default patch (no field set) must be a no-op at storage.
    session
        .update("ub-0001", &IssuePatch::default())
        .await
        .expect("no-op update");

    let after = session
        .get("ub-0001")
        .await
        .expect("get")
        .expect("present")
        .updated_at;

    // The no-EVENT guarantee lives at storage (behaviour.rs:100, `noop_update_writes_no_event_and_
    // leaves_updated_at`); the Session has no list_events to assert here. We assert the timestamp
    // half only (S8).
    assert_eq!(
        after, before,
        "no-op update (IssuePatch::default) must not advance updated_at"
    );
}

/// Gap 6 — Max-populated create round-trips through the engine (FR-1a, S2-pruned, optional).
///
/// Asserts every genuine `IssueInput::Create`-equivalent field on the `Issue` struct
/// (spine §5.2:942-957) round-trips through `create` → `get` with field-for-field equality —
/// INCLUDING the two relation fields a Create carries: `parent` (a `parent-child` dependency edge)
/// and `deps` (a `blocks` edge to a pre-created sibling). The relations round-trip via the hydrated
/// `Issue.dependencies` on `get` AND via `dependency_graph`.
///
/// S2 pruning: `comments` are a separate action (not a Create field); `content_hash` is derived
/// and excluded from the round-trip claim (it is `#[serde(skip)]` and recomputed on load).
/// `content_hash` derived sanity: `got.compute_content_hash() == got.compute_content_hash()` (deterministic).
///
/// NOTE on `slug`/`external_ref`: these are two DISTINCT fields, not interchangeable. `external_ref`
/// is the issue's own column (an external system reference, e.g. `gh-12345`); a `slug` is a separate
/// id-formatting convention baked into the `id` string at allocation time — it is NOT carried by
/// `external_ref`. This test round-trips `external_ref` as its own field and makes no slug claim.
#[tokio::test]
async fn create_max_populated_round_trips_through_engine() {
    let session = session().await;

    // Pre-create a sibling (dep target) and a parent (parent-child edge target).
    session
        .create(&issue("ub-sibling", Priority::MEDIUM, 900))
        .await
        .expect("create sibling");
    session
        .create(&issue("ub-parent", Priority::MEDIUM, 901))
        .await
        .expect("create parent");

    // The two relations a Create carries: a parent (parent-child edge) + a dep (blocks the sibling).
    let edge = |to: &str, dep_type| Dependency {
        issue_id: "ub-full".to_string(),
        depends_on_id: to.to_string(),
        dep_type,
        created_at: t(1000),
        created_by: Some("external-author".to_string()),
        metadata: None,
        thread_id: None,
    };

    // Build a maximally-populated issue with every genuine Create field (incl. parent + deps).
    let full = unblock_model::Issue {
        id: "ub-full".to_string(),
        title: "full-create round-trip".to_string(),
        description: Some("detailed description".to_string()),
        design: Some("technical design notes".to_string()),
        acceptance_criteria: Some("acceptance criteria".to_string()),
        notes: Some("additional notes".to_string()),
        issue_type: IssueType::Bug,
        priority: Priority::HIGH,
        labels: vec!["alpha".to_string(), "beta".to_string()],
        due_at: Some(t(9_999_999)),
        defer_until: Some(t(8_888_888)),
        estimated_minutes: Some(120),
        ephemeral: true,
        // `attribution` maps to `created_by` in the Issue struct.
        created_by: Some("external-author".to_string()),
        // `external_ref` is the issue's own external-system reference column (distinct from a slug).
        external_ref: Some("gh-12345".to_string()),
        // parent + deps: the relation half of a Create (parent-child edge + a blocks edge).
        dependencies: vec![
            edge("ub-parent", DependencyType::ParentChild),
            edge("ub-sibling", DependencyType::Blocks),
        ],
        created_at: t(1000),
        updated_at: t(1000),
        ..unblock_model::Issue::default()
    };

    let id = session.create(&full).await.expect("create full issue");
    assert_eq!(id, "ub-full");

    let got = session
        .get("ub-full")
        .await
        .expect("get")
        .expect("must exist");

    // Field-for-field equality for all genuine scalar/text Create fields.
    assert_eq!(got.title, full.title);
    assert_eq!(got.description, full.description);
    assert_eq!(got.design, full.design);
    assert_eq!(got.acceptance_criteria, full.acceptance_criteria);
    assert_eq!(got.notes, full.notes);
    assert_eq!(got.issue_type, full.issue_type);
    assert_eq!(got.priority, full.priority);
    assert_eq!(got.due_at, full.due_at);
    assert_eq!(got.defer_until, full.defer_until);
    assert_eq!(got.estimated_minutes, full.estimated_minutes);
    assert_eq!(got.ephemeral, full.ephemeral);
    assert_eq!(got.created_by, full.created_by);
    assert_eq!(got.external_ref, full.external_ref);
    // Labels round-trip as a sorted set (storage may normalise order).
    let mut got_labels = got.labels.clone();
    got_labels.sort();
    let mut expected_labels = full.labels.clone();
    expected_labels.sort();
    assert_eq!(got_labels, expected_labels, "labels must round-trip");

    // parent + deps round-trip via the hydrated `Issue.dependencies` on `get` AND `dependency_graph`.
    let graph = session.dependency_graph(&[]).await.expect("graph");
    assert_edge_round_trips(&got, &graph, "ub-parent", &DependencyType::ParentChild);
    assert_edge_round_trips(&got, &graph, "ub-sibling", &DependencyType::Blocks);

    // Derived sanity: content_hash is deterministic (not an option field — excluded from claim).
    let h1 = got.compute_content_hash();
    let h2 = got.compute_content_hash();
    assert_eq!(h1, h2, "content_hash must be deterministic");
    assert_eq!(h1.len(), 64, "content_hash must be 64 hex chars");
}

/// Assert the edge `got.id --dep_type--> to` round-trips both via the hydrated `got.dependencies`
/// and via the whole-graph `dependency_graph(&[])` snapshot.
fn assert_edge_round_trips(
    got: &unblock_model::Issue,
    graph: &unblock_model::DepTree,
    to: &str,
    dep_type: &DependencyType,
) {
    assert!(
        got.dependencies
            .iter()
            .any(|d| d.depends_on_id == to && &d.dep_type == dep_type),
        "edge {} --{dep_type:?}--> {to} must round-trip via get",
        got.id
    );
    assert!(
        graph
            .edges
            .iter()
            .any(|e| e.from == got.id && e.to == to && &e.dep_type == dep_type),
        "edge {} --{dep_type:?}--> {to} must be in the dependency graph",
        got.id
    );
}
