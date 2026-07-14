//! Behavioural contract tests for the libsql backend (T0.6).
//!
//! These exercise the spine §3.2.1 semantics directly against `LibsqlStorage::open_in_memory`:
//! schema shape (insta column-order + index-list goldens, CHECK rejection), migration stamping,
//! per-mutation `EventType` emission, ready/blocked/search behaviour, claim contention, and cycle
//! detection. The reusable backend-independent contract suite (NFR-16) lands at T0.7; this file is
//! the libsql-specific behavioural floor.

use chrono::{DateTime, TimeZone, Utc};

use unblock_model::{Dependency, DependencyType, Issue, IssueType, ListFilters, Priority, Status};
use unblock_storage::{DeleteMode, DeletePlan, IssuePatch, LibsqlStorage, Storage, StorageError};

fn ts(year: i32, month: u32, day: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(year, month, day, 0, 0, 0).unwrap()
}

fn issue(id: &str, title: &str) -> Issue {
    Issue {
        id: id.to_string(),
        title: title.to_string(),
        created_at: ts(2026, 1, 1),
        updated_at: ts(2026, 1, 1),
        ..Issue::default()
    }
}

async fn fresh() -> LibsqlStorage {
    let storage = LibsqlStorage::open_in_memory().await.expect("open");
    storage.migrate().await.expect("migrate");
    storage
}

/// Collect the `event_type` strings for an issue, oldest first.
async fn event_types(storage: &LibsqlStorage, id: &str) -> Vec<String> {
    storage
        .list_events(id)
        .await
        .expect("events")
        .into_iter()
        .map(|e| e.event_type.as_str().to_string())
        .collect()
}

// (Schema shape, CHECK rejection, and migration stamping live in the crate-internal `libsql::tests`
// module — they need the two raw connections, which are not part of the public API. This file
// exercises only the public `Storage` trait surface.)

// --------------------------------------------------------------------------------------------------
// CRUD + EventType oracle
// --------------------------------------------------------------------------------------------------

#[tokio::test]
async fn create_emits_created_no_dedup() {
    let storage = fresh().await;
    let id = storage
        .create_issue(&issue("ub-1", "first"), "alice")
        .await
        .expect("create");
    assert_eq!(id, "ub-1");

    // Same content, different id — NOT deduped (no content-hash short-circuit).
    let id2 = storage
        .create_issue(&issue("ub-2", "first"), "alice")
        .await
        .expect("create dup content");
    assert_eq!(id2, "ub-2");

    // Same id → IdCollision.
    let collision = storage.create_issue(&issue("ub-1", "x"), "alice").await;
    assert!(matches!(collision, Err(StorageError::IdCollision { .. })));

    assert_eq!(event_types(&storage, "ub-1").await, vec!["created"]);
}

#[tokio::test]
async fn update_title_emits_updated_and_advances_updated_at() {
    let storage = fresh().await;
    storage
        .create_issue(&issue("ub-1", "old"), "a")
        .await
        .unwrap();
    let before = storage.get_issue("ub-1").await.unwrap().unwrap();

    let patch = IssuePatch {
        title: Some("new".to_string()),
        ..IssuePatch::default()
    };
    let after = storage.update_issue("ub-1", &patch, "a").await.unwrap();
    assert_eq!(after.title, "new");
    assert!(after.updated_at >= before.updated_at);

    assert_eq!(
        event_types(&storage, "ub-1").await,
        vec!["created", "updated"]
    );
}

#[tokio::test]
async fn noop_update_writes_no_event_and_leaves_updated_at() {
    let storage = fresh().await;
    storage
        .create_issue(&issue("ub-1", "same"), "a")
        .await
        .unwrap();
    let before = storage.get_issue("ub-1").await.unwrap().unwrap();

    // Patch the title to its current value: nothing changes.
    let patch = IssuePatch {
        title: Some("same".to_string()),
        ..IssuePatch::default()
    };
    let after = storage.update_issue("ub-1", &patch, "a").await.unwrap();
    assert_eq!(
        after.updated_at, before.updated_at,
        "updated_at must not move"
    );
    assert_eq!(event_types(&storage, "ub-1").await, vec!["created"]);
}

#[tokio::test]
async fn body_field_update_emits_no_event() {
    let storage = fresh().await;
    storage
        .create_issue(&issue("ub-1", "t"), "a")
        .await
        .unwrap();

    let patch = IssuePatch {
        description: Some(Some("a body".to_string())),
        ..IssuePatch::default()
    };
    storage.update_issue("ub-1", &patch, "a").await.unwrap();
    // Body fields write no event (only the row changed).
    assert_eq!(event_types(&storage, "ub-1").await, vec!["created"]);
    let fetched = storage.get_issue("ub-1").await.unwrap().unwrap();
    assert_eq!(fetched.description.as_deref(), Some("a body"));
}

#[tokio::test]
async fn close_reason_round_trips_via_update_set_clear_leave() {
    let storage = fresh().await;
    storage
        .create_issue(&issue("ub-1", "t"), "a")
        .await
        .unwrap();
    // A freshly created issue has no close reason (DEFAULT '' → None on load).
    let before = storage.get_issue("ub-1").await.unwrap().unwrap();
    assert_eq!(before.close_reason, None);

    // Some(Some("done")) → set, and it persists across a re-read.
    storage
        .update_issue(
            "ub-1",
            &IssuePatch {
                close_reason: Some(Some("done".to_string())),
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .unwrap();
    let set = storage.get_issue("ub-1").await.unwrap().unwrap();
    assert_eq!(set.close_reason.as_deref(), Some("done"));

    // None (leave unchanged) → the stored reason survives an unrelated patch.
    storage
        .update_issue(
            "ub-1",
            &IssuePatch {
                title: Some("retitled".to_string()),
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .unwrap();
    let left = storage.get_issue("ub-1").await.unwrap().unwrap();
    assert_eq!(
        left.close_reason.as_deref(),
        Some("done"),
        "None must leave close_reason untouched"
    );

    // Some(None) → clear to the column default '' (coalesces to None on load).
    storage
        .update_issue(
            "ub-1",
            &IssuePatch {
                close_reason: Some(None),
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .unwrap();
    let cleared = storage.get_issue("ub-1").await.unwrap().unwrap();
    assert_eq!(cleared.close_reason, None, "Some(None) clears the reason");

    // close_reason is a body column → it writes NO own event (only created + the title `updated`).
    assert_eq!(
        event_types(&storage, "ub-1").await,
        vec!["created".to_string(), "updated".to_string()]
    );
}

#[tokio::test]
async fn status_priority_assignee_emit_their_events() {
    let storage = fresh().await;
    storage
        .create_issue(&issue("ub-1", "t"), "a")
        .await
        .unwrap();

    let patch = IssuePatch {
        status: Some(Status::InProgress),
        priority: Some(Priority::HIGH),
        assignee: Some(Some("bob".to_string())),
        ..IssuePatch::default()
    };
    storage.update_issue("ub-1", &patch, "a").await.unwrap();

    // EXACT ordered equality (the §3.2.1 oracle is order-sensitive; list_events is ASC). The
    // update applies status -> priority -> assignee, so the events follow that order after `created`.
    assert_eq!(
        event_types(&storage, "ub-1").await,
        vec![
            "created".to_string(),
            "status_changed".to_string(),
            "priority_changed".to_string(),
            "assignee_changed".to_string(),
        ]
    );
}

#[tokio::test]
async fn close_emits_closed_reopen_emits_reopened() {
    let storage = fresh().await;
    storage
        .create_issue(&issue("ub-1", "t"), "a")
        .await
        .unwrap();

    storage
        .update_issue(
            "ub-1",
            &IssuePatch {
                status: Some(Status::Closed),
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .unwrap();
    storage
        .update_issue(
            "ub-1",
            &IssuePatch {
                status: Some(Status::Open),
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .unwrap();

    // EXACT ordered equality: each status change emits StatusChanged first, then the Closed/Reopened
    // refinement. Close (open->closed): status_changed + closed. Reopen (closed->open):
    // status_changed + reopened.
    assert_eq!(
        event_types(&storage, "ub-1").await,
        vec![
            "created".to_string(),
            "status_changed".to_string(),
            "closed".to_string(),
            "status_changed".to_string(),
            "reopened".to_string(),
        ]
    );
}

#[tokio::test]
async fn delete_tombstone_from_non_terminal_emits_deleted() {
    let storage = fresh().await;
    storage
        .create_issue(&issue("ub-1", "t"), "a")
        .await
        .unwrap();

    let plan = DeletePlan {
        mode: DeleteMode::Tombstone,
        targets: vec!["ub-1".to_string()],
        cascade_children: Vec::new(),
    };
    storage.delete_issue(&plan, "admin").await.unwrap();

    let fetched = storage.get_issue("ub-1").await.unwrap().unwrap();
    assert_eq!(fetched.status, Status::Tombstone);
    assert!(fetched.original_type.is_some(), "original_type preserved");
    // EXACT ordered equality: tombstoning a non-terminal issue emits exactly one Deleted event.
    assert_eq!(
        event_types(&storage, "ub-1").await,
        vec!["created".to_string(), "deleted".to_string()]
    );
}

/// `visibility_branch` (FORK-1/D23): the full-corpus export filter
/// (`include_closed=true, include_deferred=true, include_tombstone=true`) returns EVERY status
/// (Open + Closed + Tombstone), while the DEFAULT filter still excludes closed + tombstone — the
/// regression guard that the new widest branch did NOT widen the default visibility.
#[tokio::test]
async fn list_visibility_include_tombstone_returns_full_corpus() {
    let storage = fresh().await;
    // Open (ub-open).
    storage
        .create_issue(&issue("ub-open", "open"), "a")
        .await
        .unwrap();
    // Closed (ub-closed) via an update.
    storage
        .create_issue(&issue("ub-closed", "closed"), "a")
        .await
        .unwrap();
    storage
        .update_issue(
            "ub-closed",
            &IssuePatch {
                status: Some(Status::Closed),
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .unwrap();
    // Tombstone (ub-tomb) via a soft delete.
    storage
        .create_issue(&issue("ub-tomb", "tomb"), "a")
        .await
        .unwrap();
    storage
        .delete_issue(
            &DeletePlan {
                mode: DeleteMode::Tombstone,
                targets: vec!["ub-tomb".to_string()],
                cascade_children: Vec::new(),
            },
            "admin",
        )
        .await
        .unwrap();

    // Full-corpus export filter → all three statuses present.
    let full = ListFilters {
        include_closed: true,
        include_deferred: true,
        include_tombstone: true,
        ..ListFilters::default()
    };
    let mut ids: Vec<String> = storage
        .list_issues(&full)
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    ids.sort();
    assert_eq!(
        ids,
        vec![
            "ub-closed".to_string(),
            "ub-open".to_string(),
            "ub-tomb".to_string()
        ],
        "full-corpus filter must include closed + tombstone rows"
    );

    // DEFAULT filter → only the Open row (closed + tombstone still excluded — regression guard).
    let default_ids: Vec<String> = storage
        .list_issues(&ListFilters::default())
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert_eq!(
        default_ids,
        vec!["ub-open".to_string()],
        "the default filter must still exclude closed + tombstone"
    );
}

#[tokio::test]
async fn delete_from_terminal_emits_no_deleted_event() {
    let storage = fresh().await;
    storage
        .create_issue(&issue("ub-1", "t"), "a")
        .await
        .unwrap();
    // Close it first (terminal).
    storage
        .update_issue(
            "ub-1",
            &IssuePatch {
                status: Some(Status::Closed),
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .unwrap();

    let plan = DeletePlan {
        mode: DeleteMode::Tombstone,
        targets: vec!["ub-1".to_string()],
        cascade_children: Vec::new(),
    };
    storage.delete_issue(&plan, "admin").await.unwrap();

    // A CLOSED issue tombstoned records NO Deleted event.
    let deleted_events = event_types(&storage, "ub-1")
        .await
        .into_iter()
        .filter(|e| e == "deleted")
        .count();
    assert_eq!(deleted_events, 0, "no Deleted event from a terminal status");
}

#[tokio::test]
async fn dry_run_delete_mutates_nothing() {
    let storage = fresh().await;
    storage
        .create_issue(&issue("ub-1", "t"), "a")
        .await
        .unwrap();

    let plan = DeletePlan {
        mode: DeleteMode::DryRun,
        targets: vec!["ub-1".to_string()],
        cascade_children: Vec::new(),
    };
    let resolved = storage.delete_issue(&plan, "admin").await.unwrap();
    assert_eq!(resolved.targets, vec!["ub-1".to_string()]);

    let fetched = storage.get_issue("ub-1").await.unwrap().unwrap();
    assert_eq!(fetched.status, Status::Open, "DryRun must not mutate");
}

// --------------------------------------------------------------------------------------------------
// Restore (un-tombstone) — FR-1c "recoverable", D20
// --------------------------------------------------------------------------------------------------

/// The was-CLOSED case is where the CHECK constraint bites: restoring a was-Closed tombstone MUST
/// land `status='closed'` with `closed_at IS NOT NULL` (the issues-table CHECK fails otherwise, the
/// tx rolls back, and restore silently fails). The bug hides for the Open case, so this is the
/// load-bearing branch.
#[tokio::test]
async fn restore_was_closed_lands_closed_with_closed_at_preserved() {
    let storage = fresh().await;
    storage
        .create_issue(&issue("ub-1", "t"), "a")
        .await
        .unwrap();
    // Close it (sets closed_at), then tombstone it (keeps closed_at; emits no Deleted from terminal).
    storage
        .update_issue(
            "ub-1",
            &IssuePatch {
                status: Some(Status::Closed),
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .unwrap();
    let closed_at = storage
        .get_issue("ub-1")
        .await
        .unwrap()
        .unwrap()
        .closed_at
        .expect("closed_at set on close");

    let plan = DeletePlan {
        mode: DeleteMode::Tombstone,
        targets: vec!["ub-1".to_string()],
        cascade_children: Vec::new(),
    };
    storage.delete_issue(&plan, "admin").await.unwrap();

    // Restore: the was-Closed signal (closed_at) lands status=Closed with closed_at preserved — the
    // CHECK passes (no rollback / silent failure).
    let restored = storage
        .restore_issue("ub-1", "admin")
        .await
        .expect("restore");
    assert_eq!(restored.status, Status::Closed);
    assert_eq!(
        restored.closed_at,
        Some(closed_at),
        "closed_at preserved — the CHECK satisfier for status='closed'"
    );
    assert_eq!(restored.original_type, None);
    assert!(restored.deleted_at.is_none());
    assert!(restored.deleted_by.is_none());
}

/// A soft tombstone keeps deps + labels, so a restored issue re-enters the dependency graph with its
/// surviving edges (the inverse of close's newly-unblocked) and keeps its labels.
#[tokio::test]
async fn restore_keeps_surviving_deps_and_labels() {
    let storage = fresh().await;
    storage
        .create_issue(&issue("ub-1", "blocked"), "a")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-2", "blocker"), "a")
        .await
        .unwrap();
    // ub-1 is blocked-by ub-2.
    storage
        .add_dependency(&dep("ub-1", "ub-2", DependencyType::Blocks), "a")
        .await
        .unwrap();
    // Give ub-1 a label.
    storage
        .update_issue(
            "ub-1",
            &IssuePatch {
                labels_add: vec!["urgent".to_string()],
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .unwrap();

    let plan = DeletePlan {
        mode: DeleteMode::Tombstone,
        targets: vec!["ub-1".to_string()],
        cascade_children: Vec::new(),
    };
    storage.delete_issue(&plan, "admin").await.unwrap();

    let restored = storage
        .restore_issue("ub-1", "admin")
        .await
        .expect("restore");
    assert_eq!(restored.status, Status::Open);
    // Label survived the tombstone → restore.
    assert_eq!(restored.labels, vec!["urgent".to_string()]);
    // The blocking edge survived → the restored issue is still blocked-by ub-2.
    let deps = storage.list_dependencies("ub-1").await.unwrap();
    assert_eq!(deps.len(), 1, "the blocking edge survived the restore");
    assert_eq!(deps[0].depends_on_id, "ub-2");
}

/// Restore recomputes `content_hash` (status is hashed) and bumps `updated_at`.
#[tokio::test]
async fn restore_recomputes_hash_and_bumps_updated_at() {
    let storage = fresh().await;
    storage
        .create_issue(&issue("ub-1", "t"), "a")
        .await
        .unwrap();
    let plan = DeletePlan {
        mode: DeleteMode::Tombstone,
        targets: vec!["ub-1".to_string()],
        cascade_children: Vec::new(),
    };
    storage.delete_issue(&plan, "admin").await.unwrap();
    let tombstoned = storage.get_issue("ub-1").await.unwrap().unwrap();
    let tomb_updated = tombstoned.updated_at;

    let restored = storage
        .restore_issue("ub-1", "admin")
        .await
        .expect("restore");
    assert!(
        restored.updated_at >= tomb_updated,
        "updated_at bumped on restore"
    );
    // content_hash is recomputed from the restored (status=open) fields; it must equal the hash of an
    // equivalent freshly-loaded active issue (status is part of the hash, so it differs from the
    // tombstone hash).
    let reloaded = storage.get_issue("ub-1").await.unwrap().unwrap();
    assert_eq!(
        reloaded.content_hash,
        Some(reloaded.compute_content_hash()),
        "stored content_hash matches the recomputed hash of the restored row"
    );
    assert_ne!(
        reloaded.content_hash, tombstoned.content_hash,
        "restored hash differs from the tombstone hash (status is hashed)"
    );
}

/// A `Hard`-deleted row is gone: restore cannot resurrect it → `IssueNotFound` (restore is bounded to
/// SOFT deletes).
#[tokio::test]
async fn restore_of_hard_deleted_is_issue_not_found() {
    let storage = fresh().await;
    storage
        .create_issue(&issue("ub-1", "t"), "a")
        .await
        .unwrap();
    let plan = DeletePlan {
        mode: DeleteMode::Hard,
        targets: vec!["ub-1".to_string()],
        cascade_children: Vec::new(),
    };
    storage.delete_issue(&plan, "admin").await.unwrap();
    assert!(storage.get_issue("ub-1").await.unwrap().is_none());

    match storage.restore_issue("ub-1", "admin").await {
        Err(StorageError::IssueNotFound { id }) => assert_eq!(id, "ub-1"),
        other => panic!("expected IssueNotFound for a hard-deleted id, got {other:?}"),
    }
}

// --------------------------------------------------------------------------------------------------
// Cascade + Hard delete (FR-1c) — never-run production paths exercised here
// --------------------------------------------------------------------------------------------------

/// Build the 6-issue authoritative corpus for Cascade/Hard tests (§2 of the T1.4 LOCKED spec).
///
/// The corpus uses **dotted ids** because `resolve_cascade_children` keys on the `{target}.%`
/// dotted-id prefix (crud.rs:835), NOT on parent-child edges. The reparent corpus uses flat ids
/// and edges; these two corpora are intentionally separate (D-DRIFT-A).
///
/// Returns: (storage, ids in insertion order)
async fn seed_cascade_corpus(actor: &str) -> LibsqlStorage {
    let storage = fresh().await;
    // ub-1: the delete target (root, Open)
    storage
        .create_issue(&issue("ub-1", "root"), actor)
        .await
        .unwrap();
    // ub-1.1: child (depth 1, Open) — non-terminal cascade member
    storage
        .create_issue(&issue("ub-1.1", "child"), actor)
        .await
        .unwrap();
    // ub-1.1.1: grandchild (depth 2, Open) — proves recursive prefix match
    storage
        .create_issue(&issue("ub-1.1.1", "grandchild"), actor)
        .await
        .unwrap();
    // ub-1.2: sibling child (Closed/terminal) — proves Deleted-event guard
    storage
        .create_issue(&issue("ub-1.2", "closed-child"), actor)
        .await
        .unwrap();
    storage
        .update_issue(
            "ub-1.2",
            &IssuePatch {
                status: Some(Status::Closed),
                ..IssuePatch::default()
            },
            actor,
        )
        .await
        .unwrap();
    // ub-10: dot-boundary decoy (shares "ub-1" prefix WITHOUT a dot — must be UNTOUCHED by cascade)
    storage
        .create_issue(&issue("ub-10", "decoy-prefix"), actor)
        .await
        .unwrap();
    // ub-2: unrelated root — bounded blast-radius witness
    storage
        .create_issue(&issue("ub-2", "unrelated"), actor)
        .await
        .unwrap();
    storage
}

/// Gap 1a — Cascade tombstones target + all `{target}.%` descendants.
///
/// Assertions per spec §A/1a:
///  (1) `resolved.cascade_children` == `["ub-1.1","ub-1.1.1","ub-1.2"]` (sorted, NON-empty)
///  (2) all four get `status==Tombstone` + `original_type.is_some()`
///  (3) Deleted event count: ub-1/ub-1.1/ub-1.1.1 each have one; ub-1.2 (Closed) has zero →
///      total across the 4 == 3
///  (4) `deleted_by == Some("admin")` for all 4 (including the terminal ub-1.2)
///  (5) ub-10 (dot-boundary decoy) and ub-2 (unrelated) are UNTOUCHED (status Open, no `deleted_by`)
#[tokio::test]
async fn delete_cascade_tombstones_self_and_descendants() {
    let storage = seed_cascade_corpus("alice").await;

    // Caller passes cascade_children: Vec::new() — storage RESOLVES them internally.
    let plan = DeletePlan {
        mode: DeleteMode::Cascade,
        targets: vec!["ub-1".to_string()],
        cascade_children: Vec::new(),
    };
    let resolved = storage.delete_issue(&plan, "admin").await.unwrap();

    // (1) resolved plan shows the full blast radius (sorted, non-empty).
    assert_eq!(resolved.targets, vec!["ub-1".to_string()]);
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

    // (2) All four affected issues are now Tombstone with original_type preserved.
    for id in ["ub-1", "ub-1.1", "ub-1.1.1", "ub-1.2"] {
        let fetched = storage.get_issue(id).await.unwrap().unwrap();
        assert_eq!(fetched.status, Status::Tombstone, "{id} must be Tombstone");
        assert!(
            fetched.original_type.is_some(),
            "{id} original_type must be preserved"
        );
        // (4) Actor propagation: deleted_by set on ALL four, including the terminal ub-1.2.
        assert_eq!(
            fetched.deleted_by.as_deref(),
            Some("admin"),
            "{id} deleted_by must be the actor"
        );
    }

    // (3) Event count: non-terminal members each have exactly one "deleted" event;
    //     ub-1.2 (was Closed/terminal) has zero — tombstone_one skips the event for terminal.
    let mut deleted_event_count = 0usize;
    for id in ["ub-1", "ub-1.1", "ub-1.1.1", "ub-1.2"] {
        deleted_event_count += event_types(&storage, id)
            .await
            .into_iter()
            .filter(|e| e == "deleted")
            .count();
    }
    // 3 non-terminal members (ub-1, ub-1.1, ub-1.1.1) each emit one Deleted event; ub-1.2 emits 0.
    assert_eq!(
        deleted_event_count, 3,
        "total Deleted events across all 4 affected issues must be 3 (terminal ub-1.2 gets none)"
    );

    // (5) Dot-boundary decoy ub-10 is UNTOUCHED (shares "ub-1" prefix but has no dot after "ub-1").
    let decoy = storage.get_issue("ub-10").await.unwrap().unwrap();
    assert_eq!(
        decoy.status,
        Status::Open,
        "ub-10 (dot-boundary decoy) must be untouched"
    );
    assert!(decoy.deleted_by.is_none(), "ub-10 must have no deleted_by");
    assert_eq!(
        event_types(&storage, "ub-10").await,
        vec!["created".to_string()],
        "ub-10 events must be unchanged"
    );

    // (5) Unrelated ub-2 is also UNTOUCHED.
    let unrelated = storage.get_issue("ub-2").await.unwrap().unwrap();
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

/// Gap 1b — Hard delete removes ONLY the target's own row + its FK-child rows + inbound dep rows.
///
/// Key: Hard ≠ Cascade. `issues` has NO self-parent FK (schema.rs:34-78) — child *issue rows*
/// `ub-1.1`/`ub-1.1.1` SURVIVE. Only the deleted target's labels/deps(`issue_id`)/events/comments
/// (FK CASCADE) and inbound `depends_on_id` dep rows (explicit DELETE) are removed.
///
/// `PRAGMA foreign_keys = ON` is verified by `pragmas_readback_in_memory` (mod.rs:702-727) and
/// `foreign_keys_enforced` (mod.rs:853) — see mod.rs:404. The FK CASCADE assertion here relies on
/// that guarantee.
///
/// Assertions per spec §A/1b:
///  (1) `get_issue("ub-1") == Ok(None)` (row gone)
///  (2) inbound dep from ub-4 cleaned (no orphan `depends_on_id = "ub-1"`)
///  (3) FK CASCADE removed ub-1's OWN child rows: `list_dependencies("ub-1")` empty, `list_events` empty
///  (4) `dependency_graph(&[])` has no edge from or to "ub-1"
///  (5) Hard ≠ Cascade: ub-1.1 and ub-2 SURVIVE (M1 fix)
///  (6) Hard writes no spurious Deleted event on a SURVIVING issue (S9). NOTE: ub-1's OWN events are
///      unobservable post-delete (its events rows are gone via FK CASCADE), so we cannot — and do
///      not — assert "ub-1 emitted no Deleted event"; we only check that ub-1's rows are gone and
///      that a surviving issue (ub-4) has no spurious Deleted event from the Hard op.
#[tokio::test]
async fn delete_hard_removes_target_rows_and_inbound_deps_no_orphans() {
    let storage = fresh().await;

    // Core corpus: ub-1 (target), ub-1.1 (child — must survive Hard), ub-2 (unrelated).
    storage
        .create_issue(&issue("ub-1", "target"), "alice")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-1.1", "child"), "alice")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-2", "unrelated"), "alice")
        .await
        .unwrap();

    // Outbound edge: ub-1 --blocks--> ub-3  (ub-1's own issue_id dep row — FK CASCADE removes it).
    storage
        .create_issue(&issue("ub-3", "outbound-target"), "alice")
        .await
        .unwrap();
    storage
        .add_dependency(&dep("ub-1", "ub-3", DependencyType::Blocks), "alice")
        .await
        .unwrap();

    // Inbound edge: ub-4 --blocks--> ub-1  (depends_on_id = "ub-1", NO FK — explicit DELETE).
    storage
        .create_issue(&issue("ub-4", "inbound-blocker"), "alice")
        .await
        .unwrap();
    storage
        .add_dependency(&dep("ub-4", "ub-1", DependencyType::Blocks), "alice")
        .await
        .unwrap();

    // Execute Hard delete of ub-1 only (cascade_children is ignored for Hard — _ => targets.clone()).
    let plan = DeletePlan {
        mode: DeleteMode::Hard,
        targets: vec!["ub-1".to_string()],
        cascade_children: Vec::new(),
    };
    storage.delete_issue(&plan, "admin").await.unwrap();

    // (1) ub-1 row is gone.
    assert!(
        storage.get_issue("ub-1").await.unwrap().is_none(),
        "Hard delete must remove the target row"
    );

    // (2) Inbound orphan cleaned: ub-4's dependency list must no longer contain depends_on_id="ub-1".
    let ub4_deps = storage.list_dependencies("ub-4").await.unwrap();
    assert!(
        !ub4_deps.iter().any(|d| d.depends_on_id == "ub-1"),
        "inbound dep from ub-4 to ub-1 must be cleaned (explicit DELETE on depends_on_id)"
    );

    // (3) FK CASCADE removed ub-1's OWN child rows: outbound deps + events.
    //     list_dependencies("ub-1") is empty (issue_id FK rows gone).
    //     (We can't call list_events("ub-1") after row deletion since event rows are also gone.)
    let ub1_deps = storage.list_dependencies("ub-1").await.unwrap();
    assert!(
        ub1_deps.is_empty(),
        "ub-1's own dep rows must be gone via FK CASCADE"
    );

    // (4) Global no-orphan: whole dependency graph has no edge involving ub-1.
    let graph = storage.dependency_graph(&[]).await.unwrap();
    assert!(
        !graph
            .edges
            .iter()
            .any(|e| e.from == "ub-1" || e.to == "ub-1"),
        "no edge in the whole graph may reference the deleted ub-1"
    );

    // (5) Hard ≠ Cascade: child issue rows SURVIVE.  (M1: `issues` has NO self-parent FK.)
    assert!(
        storage.get_issue("ub-1.1").await.unwrap().is_some(),
        "ub-1.1 (child issue row) must SURVIVE a Hard delete — Hard ≠ Cascade (no self-parent FK)"
    );
    assert!(
        storage.get_issue("ub-2").await.unwrap().is_some(),
        "ub-2 (unrelated) must SURVIVE"
    );
    assert!(
        storage.get_issue("ub-3").await.unwrap().is_some(),
        "ub-3 (outbound target) must SURVIVE"
    );
    assert!(
        storage.get_issue("ub-4").await.unwrap().is_some(),
        "ub-4 (inbound blocker) must SURVIVE"
    );
    // (6) Hard writes no spurious Deleted event on a SURVIVING issue. tombstone_one is never called
    //     (Hard takes the row-delete path, not the tombstone path). ub-1's own events are gone via
    //     FK CASCADE and thus unobservable, so we only assert what is actually checkable: ub-4
    //     (which still exists) has no "deleted" event from the Hard operation.
    assert!(
        !event_types(&storage, "ub-4")
            .await
            .contains(&"deleted".to_string()),
        "Hard delete must not emit a Deleted event on any surviving issue"
    );
}

// --------------------------------------------------------------------------------------------------
// Reparent move / detach / no-op (FR-1b) — storage edge substrate
// --------------------------------------------------------------------------------------------------
//
// IMPORTANT: reparent keys on the parent-child EDGE (apply_reparent, crud.rs:625-668), NOT on the
// dotted-id prefix. This corpus uses FLAT ids (ub-p1, ub-p2, ub-child) with explicit edges —
// entirely separate from the Cascade corpus. (D-DRIFT-A in the T1.4 LOCKED spec.)

/// Gap 4a — Reparent moves the parent-child edge from one parent to another, advancing `updated_at`
/// and emitting the dependency audit events.
///
/// A real reparent is a genuine modification of the issue (FR-1b): it advances `updated_at` even
/// when no other field changed (crud.rs gates the `updated_at` stamp on `!builder.is_empty() ||
/// parent_changed`) and emits the same dependency events as `add_dependency`/`remove_dependency`
/// (`DependencyAdded`/`DependencyRemoved`) — NOT an `Updated` event. The first reparent (attach to
/// ub-p1) emits one `dependency_added`; the second (move to ub-p2) emits `dependency_removed`
/// (old ub-p1 edge dropped) + `dependency_added` (new ub-p2 edge). After the move the first edge
/// must be GONE (moved, not duplicated).
#[tokio::test]
async fn reparent_success_moves_edge_and_advances_updated_at() {
    let storage = fresh().await;
    storage
        .create_issue(&issue("ub-p1", "parent-1"), "a")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-p2", "parent-2"), "a")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-child", "child"), "a")
        .await
        .unwrap();
    let created_at = storage
        .get_issue("ub-child")
        .await
        .unwrap()
        .unwrap()
        .created_at;

    // First reparent: ub-child → ub-p1.
    let patch1 = IssuePatch {
        parent: Some(Some("ub-p1".to_string())),
        ..IssuePatch::default()
    };
    storage
        .update_issue("ub-child", &patch1, "a")
        .await
        .unwrap();

    // A real reparent advances updated_at. Strict `>` is safe: created_at is frozen to ts(2026,1,1)
    // while the reparent stamps Utc::now() (S7).
    let after_1 = storage.get_issue("ub-child").await.unwrap().unwrap();
    assert!(
        after_1.updated_at > created_at,
        "first reparent must advance updated_at past the frozen created_at"
    );

    // Exactly one parent-child edge and it points to ub-p1.
    let deps_after_1 = storage.list_dependencies("ub-child").await.unwrap();
    let parent_edges_after_1: Vec<_> = deps_after_1
        .iter()
        .filter(|d| d.dep_type == DependencyType::ParentChild)
        .collect();
    assert_eq!(
        parent_edges_after_1.len(),
        1,
        "exactly one parent-child edge after first reparent"
    );
    assert_eq!(parent_edges_after_1[0].depends_on_id, "ub-p1");

    // The attach emitted exactly one dependency_added event (mirroring add_dependency) — NOT updated.
    assert_eq!(
        event_types(&storage, "ub-child").await,
        vec!["created".to_string(), "dependency_added".to_string()],
        "attach reparent emits dependency_added (the edge event), not an updated event"
    );

    // Second reparent: ub-child → ub-p2.
    let patch2 = IssuePatch {
        parent: Some(Some("ub-p2".to_string())),
        ..IssuePatch::default()
    };
    storage
        .update_issue("ub-child", &patch2, "a")
        .await
        .unwrap();

    // The move advances updated_at again past the first reparent's stamp.
    let after_2 = storage.get_issue("ub-child").await.unwrap().unwrap();
    assert!(
        after_2.updated_at > created_at,
        "second reparent must advance updated_at past the frozen created_at"
    );

    // The ub-p1 edge must be GONE; only the ub-p2 edge survives.
    let deps_after_2 = storage.list_dependencies("ub-child").await.unwrap();
    let parent_edges_after_2: Vec<_> = deps_after_2
        .iter()
        .filter(|d| d.dep_type == DependencyType::ParentChild)
        .collect();
    assert_eq!(
        parent_edges_after_2.len(),
        1,
        "exactly one parent-child edge after second reparent"
    );
    assert_eq!(
        parent_edges_after_2[0].depends_on_id, "ub-p2",
        "the ub-p1 edge must be replaced by ub-p2 (moved, not duplicated)"
    );

    // The move dropped the ub-p1 edge (dependency_removed) and set the ub-p2 edge (dependency_added),
    // mirroring remove_dependency + add_dependency.
    assert_eq!(
        event_types(&storage, "ub-child").await,
        vec![
            "created".to_string(),
            "dependency_added".to_string(),
            "dependency_removed".to_string(),
            "dependency_added".to_string(),
        ],
        "moving the parent emits dependency_removed (old edge) + dependency_added (new edge)"
    );
}

/// Gap 4b — Reparent to the CURRENT parent is a no-op (`updated_at` unchanged, no new event).
///
/// `apply_reparent` (crud.rs:636-639) returns Ok(false) when the requested parent equals the
/// current one; the caller then advances NOTHING and emits NO dependency event. The baseline is
/// captured AFTER a real attach (which DID advance `updated_at` + emit `dependency_added`), so this
/// proves the no-op is silent on top of a prior real change.
#[tokio::test]
async fn reparent_to_current_parent_is_noop() {
    let storage = fresh().await;
    storage
        .create_issue(&issue("ub-p2", "parent-2"), "a")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-child", "child"), "a")
        .await
        .unwrap();

    // Set parent to ub-p2.
    storage
        .update_issue(
            "ub-child",
            &IssuePatch {
                parent: Some(Some("ub-p2".to_string())),
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .unwrap();

    let before = storage.get_issue("ub-child").await.unwrap().unwrap();
    let events_before = event_types(&storage, "ub-child").await;

    // Reparent to the same parent again → no-op.
    storage
        .update_issue(
            "ub-child",
            &IssuePatch {
                parent: Some(Some("ub-p2".to_string())),
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .unwrap();

    let after = storage.get_issue("ub-child").await.unwrap().unwrap();
    assert_eq!(
        after.updated_at, before.updated_at,
        "no-op reparent must not advance updated_at"
    );
    assert_eq!(
        event_types(&storage, "ub-child").await,
        events_before,
        "no-op reparent must write no new event"
    );

    // Edge is unchanged.
    let deps = storage.list_dependencies("ub-child").await.unwrap();
    let parent_edges: Vec<_> = deps
        .iter()
        .filter(|d| d.dep_type == DependencyType::ParentChild)
        .collect();
    assert_eq!(parent_edges.len(), 1);
    assert_eq!(parent_edges[0].depends_on_id, "ub-p2");
}

/// Gap 4c — Detach removes the parent-child edge (a real change: advances `updated_at` + emits
/// `DependencyRemoved`); a second detach (already parentless) is a no-op.
///
/// A detach (`parent: Some(None)`) that actually drops an edge is a genuine modification (FR-1b):
/// it advances `updated_at` and emits `dependency_removed` (mirroring `remove_dependency`). The
/// second detach is a no-op — `apply_reparent` returns `Ok(false)` (crud.rs:636-639) — so it advances
/// NOTHING and emits NO event.
#[tokio::test]
async fn reparent_detach_then_redetach() {
    let storage = fresh().await;
    storage
        .create_issue(&issue("ub-p2", "parent-2"), "a")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-child", "child"), "a")
        .await
        .unwrap();
    let created_at = storage
        .get_issue("ub-child")
        .await
        .unwrap()
        .unwrap()
        .created_at;

    // Attach to ub-p2 first.
    storage
        .update_issue(
            "ub-child",
            &IssuePatch {
                parent: Some(Some("ub-p2".to_string())),
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .unwrap();

    // Detach: parent: Some(None) → removes the edge (a real change).
    storage
        .update_issue(
            "ub-child",
            &IssuePatch {
                parent: Some(None),
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .unwrap();

    // No parent-child edge remains after the detach.
    let deps_detached = storage.list_dependencies("ub-child").await.unwrap();
    assert!(
        deps_detached
            .iter()
            .all(|d| d.dep_type != DependencyType::ParentChild),
        "detach must remove the parent-child edge"
    );

    // The detach advanced updated_at (real change) and emitted dependency_removed (the dropped edge)
    // on top of the attach's dependency_added. Strict `>` safe: created_at frozen (S7).
    let after_detach = storage.get_issue("ub-child").await.unwrap().unwrap();
    assert!(
        after_detach.updated_at > created_at,
        "a real detach must advance updated_at"
    );
    assert_eq!(
        event_types(&storage, "ub-child").await,
        vec![
            "created".to_string(),
            "dependency_added".to_string(),
            "dependency_removed".to_string(),
        ],
        "detach emits dependency_removed (the dropped parent edge)"
    );

    // Second detach (already parentless) → no-op: apply_reparent returns Ok(false) (crud.rs:636-639).
    let updated_at_after_detach = after_detach.updated_at;
    let events_after_detach = event_types(&storage, "ub-child").await;
    storage
        .update_issue(
            "ub-child",
            &IssuePatch {
                parent: Some(None),
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .unwrap();

    let after_redetach = storage.get_issue("ub-child").await.unwrap().unwrap();
    assert_eq!(
        after_redetach.updated_at, updated_at_after_detach,
        "re-detach (already parentless) must not advance updated_at"
    );
    assert_eq!(
        event_types(&storage, "ub-child").await,
        events_after_detach,
        "re-detach must write no new event"
    );
}

// --------------------------------------------------------------------------------------------------
// Claim (assignee-only guard) + contention
// --------------------------------------------------------------------------------------------------

#[tokio::test]
async fn same_actor_reclaim_is_idempotent_no_event() {
    let storage = fresh().await;
    storage
        .create_issue(&issue("ub-1", "t"), "a")
        .await
        .unwrap();

    storage.claim_issue("ub-1", "bob", "bob").await.unwrap();
    let before = event_types(&storage, "ub-1").await;
    // Re-claim by the same actor: idempotent, no new event.
    storage.claim_issue("ub-1", "bob", "bob").await.unwrap();
    let after = event_types(&storage, "ub-1").await;
    assert_eq!(before, after, "same-actor re-claim writes no event");
}

#[tokio::test]
async fn claim_by_different_actor_fails_already_claimed() {
    let storage = fresh().await;
    storage
        .create_issue(&issue("ub-1", "t"), "a")
        .await
        .unwrap();
    storage.claim_issue("ub-1", "alice", "alice").await.unwrap();

    let err = storage.claim_issue("ub-1", "bob", "bob").await;
    match err {
        Err(StorageError::AlreadyClaimed { by, .. }) => assert_eq!(by, "alice"),
        other => panic!("expected AlreadyClaimed, got {other:?}"),
    }
}

#[tokio::test]
async fn concurrent_claim_exactly_one_winner() {
    use std::sync::Arc;

    let storage: Arc<dyn Storage> = Arc::new(fresh().await);
    storage
        .create_issue(&issue("ub-1", "t"), "a")
        .await
        .unwrap();

    let mut handles = Vec::new();
    for n in 0..8 {
        let storage = Arc::clone(&storage);
        handles.push(tokio::spawn(async move {
            let actor = format!("agent-{n}");
            storage.claim_issue("ub-1", &actor, &actor).await
        }));
    }

    let mut wins = 0;
    let mut already = 0;
    for handle in handles {
        match handle.await.expect("join") {
            Ok(_) => wins += 1,
            Err(StorageError::AlreadyClaimed { .. }) => already += 1,
            Err(other) => panic!("unexpected error: {other:?}"),
        }
    }
    assert_eq!(wins, 1, "exactly one claim must win");
    assert_eq!(already, 7, "the rest must lose with AlreadyClaimed");
}

// --------------------------------------------------------------------------------------------------
// Ready / blocked / search
// --------------------------------------------------------------------------------------------------

#[tokio::test]
async fn ready_excludes_blocked_deferred_closed() {
    let storage = fresh().await;
    storage
        .create_issue(&issue("ub-open", "open"), "a")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-blocker", "blocker"), "a")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-blocked", "blocked"), "a")
        .await
        .unwrap();

    // ub-blocked depends on ub-blocker (blocks).
    storage
        .add_dependency(
            &Dependency {
                issue_id: "ub-blocked".to_string(),
                depends_on_id: "ub-blocker".to_string(),
                dep_type: DependencyType::Blocks,
                created_at: ts(2026, 1, 1),
                created_by: None,
                metadata: None,
                thread_id: None,
            },
            "a",
        )
        .await
        .unwrap();

    // Defer ub-open into the future.
    storage
        .create_issue(&issue("ub-deferred", "deferred"), "a")
        .await
        .unwrap();
    storage
        .defer_issue("ub-deferred", ts(2099, 1, 1), "a")
        .await
        .unwrap();

    let ready: Vec<String> = storage
        .ready_issues(&ListFilters::default())
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();

    assert!(ready.contains(&"ub-open".to_string()));
    assert!(ready.contains(&"ub-blocker".to_string()));
    assert!(
        !ready.contains(&"ub-blocked".to_string()),
        "blocked excluded"
    );
    assert!(
        !ready.contains(&"ub-deferred".to_string()),
        "deferred excluded"
    );
}

#[tokio::test]
async fn blocked_includes_in_progress() {
    let storage = fresh().await;
    storage
        .create_issue(&issue("ub-blocker", "blocker"), "a")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-wip", "wip"), "a")
        .await
        .unwrap();
    storage
        .add_dependency(
            &Dependency {
                issue_id: "ub-wip".to_string(),
                depends_on_id: "ub-blocker".to_string(),
                dep_type: DependencyType::Blocks,
                created_at: ts(2026, 1, 1),
                created_by: None,
                metadata: None,
                thread_id: None,
            },
            "a",
        )
        .await
        .unwrap();
    // Move the blocked issue to in_progress (claim it).
    storage.claim_issue("ub-wip", "bob", "bob").await.unwrap();

    let blocked: Vec<String> = storage
        .blocked_issues(&ListFilters::default())
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert!(
        blocked.contains(&"ub-wip".to_string()),
        "in_progress blocked issue must appear"
    );
}

/// Pass 3 (transitive): a child of a directly-blocked **non-epic** parent is itself blocked and is
/// absent from `ready_issues` — the new transitive down-propagation behaviour.
#[tokio::test]
async fn transitive_child_of_blocked_parent_is_blocked() {
    let storage = fresh().await;
    storage
        .create_issue(&issue("ub-blocker", "blocker"), "a")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-parent", "parent"), "a")
        .await
        .unwrap(); // plain task, not epic
    storage
        .create_issue(&issue("ub-child", "child"), "a")
        .await
        .unwrap();

    // ub-parent is directly blocked by ub-blocker.
    storage
        .add_dependency(&dep("ub-parent", "ub-blocker", DependencyType::Blocks), "a")
        .await
        .unwrap();
    // ub-child is a parent-child of ub-parent (child depends_on parent).
    storage
        .add_dependency(
            &dep("ub-child", "ub-parent", DependencyType::ParentChild),
            "a",
        )
        .await
        .unwrap();

    let blocked = blocked_ids(&storage).await;
    assert!(
        blocked.contains(&"ub-parent".to_string()),
        "parent directly blocked"
    );
    assert!(
        blocked.contains(&"ub-child".to_string()),
        "child of a blocked parent must be transitively blocked"
    );

    let ready = ready_ids(&storage).await;
    assert!(
        !ready.contains(&"ub-child".to_string()),
        "blocked child excluded from ready"
    );
    assert!(
        !ready.contains(&"ub-parent".to_string()),
        "blocked parent excluded from ready"
    );
}

/// Pass 3 (deep transitivity): a grandchild of a blocked ancestor is blocked.
#[tokio::test]
async fn deep_grandchild_of_blocked_ancestor_is_blocked() {
    let storage = fresh().await;
    storage
        .create_issue(&issue("ub-blocker", "blocker"), "a")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-gp", "grandparent"), "a")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-p", "parent"), "a")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-c", "child"), "a")
        .await
        .unwrap();

    storage
        .add_dependency(&dep("ub-gp", "ub-blocker", DependencyType::Blocks), "a")
        .await
        .unwrap();
    storage
        .add_dependency(&dep("ub-p", "ub-gp", DependencyType::ParentChild), "a")
        .await
        .unwrap();
    storage
        .add_dependency(&dep("ub-c", "ub-p", DependencyType::ParentChild), "a")
        .await
        .unwrap();

    let blocked = blocked_ids(&storage).await;
    for id in ["ub-gp", "ub-p", "ub-c"] {
        assert!(
            blocked.contains(&id.to_string()),
            "{id} must be transitively blocked"
        );
    }
}

/// Pass 2 (epic-rollup): an EPIC parent with an open child IS blocked and excluded from ready.
#[tokio::test]
async fn epic_parent_with_open_child_is_blocked() {
    let storage = fresh().await;
    let mut epic = issue("ub-epic", "epic");
    epic.issue_type = IssueType::Epic;
    storage.create_issue(&epic, "a").await.unwrap();
    storage
        .create_issue(&issue("ub-kid", "kid"), "a")
        .await
        .unwrap();

    // kid is a parent-child of the epic (kid depends_on epic).
    storage
        .add_dependency(&dep("ub-kid", "ub-epic", DependencyType::ParentChild), "a")
        .await
        .unwrap();

    let blocked = blocked_ids(&storage).await;
    assert!(
        blocked.contains(&"ub-epic".to_string()),
        "an epic with an open child must be blocked (rollup)"
    );
    let ready = ready_ids(&storage).await;
    assert!(
        !ready.contains(&"ub-epic".to_string()),
        "blocked epic excluded from ready"
    );
}

/// Pass 2 negative: a NON-epic parent with an open child is NOT blocked (it is not directly blocked,
/// nor a child of any blocked issue) — only epics roll up open children.
#[tokio::test]
async fn non_epic_parent_with_open_child_is_not_blocked() {
    let storage = fresh().await;
    storage
        .create_issue(&issue("ub-parent", "parent"), "a")
        .await
        .unwrap(); // plain task
    storage
        .create_issue(&issue("ub-kid", "kid"), "a")
        .await
        .unwrap();

    storage
        .add_dependency(
            &dep("ub-kid", "ub-parent", DependencyType::ParentChild),
            "a",
        )
        .await
        .unwrap();

    let blocked = blocked_ids(&storage).await;
    assert!(
        !blocked.contains(&"ub-parent".to_string()),
        "a non-epic parent does not roll up its open children"
    );
    // And the child of an unblocked parent is not blocked either.
    assert!(
        !blocked.contains(&"ub-kid".to_string()),
        "child of an unblocked parent is not blocked"
    );
    let ready = ready_ids(&storage).await;
    assert!(
        ready.contains(&"ub-parent".to_string()),
        "unblocked non-epic parent is ready"
    );
}

#[tokio::test]
async fn search_matches_title_description_id_substring() {
    let storage = fresh().await;
    let mut issue1 = issue("ub-needle", "Parser bug");
    issue1.description = Some("fix the lexer".to_string());
    storage.create_issue(&issue1, "a").await.unwrap();
    storage
        .create_issue(&issue("ub-other", "Unrelated"), "a")
        .await
        .unwrap();

    // Title substring (case-insensitive).
    let by_title = storage
        .search_issues("parser", &ListFilters::default())
        .await
        .unwrap();
    assert_eq!(by_title.len(), 1);
    assert_eq!(by_title[0].id, "ub-needle");

    // Description substring.
    let by_desc = storage
        .search_issues("LEXER", &ListFilters::default())
        .await
        .unwrap();
    assert_eq!(by_desc.len(), 1);

    // Id substring.
    let by_id = storage
        .search_issues("needle", &ListFilters::default())
        .await
        .unwrap();
    assert_eq!(by_id.len(), 1);
}

// --------------------------------------------------------------------------------------------------
// Dependencies + cycle detection
// --------------------------------------------------------------------------------------------------

#[tokio::test]
async fn gating_cycle_is_rejected_with_path() {
    let storage = fresh().await;
    storage
        .create_issue(&issue("ub-a", "a"), "x")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-b", "b"), "x")
        .await
        .unwrap();

    // a -> b (blocks).
    storage
        .add_dependency(&dep("ub-a", "ub-b", DependencyType::Blocks), "x")
        .await
        .unwrap();
    // b -> a would close a cycle.
    let cyclic = storage
        .add_dependency(&dep("ub-b", "ub-a", DependencyType::Blocks), "x")
        .await;
    match cyclic {
        Err(StorageError::CycleDetected { path }) => {
            // The path is the REAL ordered cycle naming BOTH nodes (`ub-b -> ub-a -> ub-b`), starting
            // and ending at the same node — NOT a synthetic placeholder.
            assert!(
                path.contains("ub-a") && path.contains("ub-b"),
                "path names both nodes: {path}"
            );
            let nodes: Vec<&str> = path.split(" -> ").collect();
            assert_eq!(
                nodes.first(),
                nodes.last(),
                "ordered cycle [start, …, start]"
            );
            assert!(nodes.len() >= 3, "a 2-cycle witness is `[a, b, a]`: {path}");
        }
        other => panic!("expected CycleDetected, got {other:?}"),
    }
}

/// M-S1 — a ≥3-node gating cycle `a -> b -> c -> a` is rejected with the **REAL ordered path naming
/// the interior node `c`** (NOT the old `a -> b -> … -> c` placeholder, which never named `c`).
#[tokio::test]
async fn three_node_gating_cycle_path_names_interior_node() {
    let storage = fresh().await;
    for id in ["ub-a", "ub-b", "ub-c"] {
        storage.create_issue(&issue(id, id), "x").await.unwrap();
    }
    // a -> b -> c (blocks), acyclic so far.
    storage
        .add_dependency(&dep("ub-a", "ub-b", DependencyType::Blocks), "x")
        .await
        .unwrap();
    storage
        .add_dependency(&dep("ub-b", "ub-c", DependencyType::Blocks), "x")
        .await
        .unwrap();
    // c -> a closes the 3-node cycle.
    match storage
        .add_dependency(&dep("ub-c", "ub-a", DependencyType::Blocks), "x")
        .await
    {
        Err(StorageError::CycleDetected { path }) => {
            assert!(
                path.contains("ub-a") && path.contains("ub-b") && path.contains("ub-c"),
                "the path NAMES every node incl. the interior `ub-c`: {path}"
            );
            let nodes: Vec<&str> = path.split(" -> ").collect();
            assert!(
                nodes.len() >= 4,
                "a 3-node cycle path has >= 4 entries (start repeated): {path}"
            );
            assert_eq!(
                nodes.first(),
                nodes.last(),
                "ordered cycle [start, …, start]"
            );
            assert!(
                !path.contains('…'),
                "no synthetic ellipsis placeholder: {path}"
            );
        }
        other => panic!("expected CycleDetected, got {other:?}"),
    }
}

/// M-S3 — mixed `parent-child` + `blocks` (and `parent-child` + `waits-for`) gating cycles are
/// rejected. MUST fail against the old forward-all orientation: gates D4. Plus each non-`blocks`
/// gating type cycles in isolation.
#[tokio::test]
async fn mixed_parent_child_and_blocking_cycle_is_rejected() {
    // parent-child + blocks over THREE nodes (distinct `(issue_id, depends_on_id)` pairs so the
    // duplicate guard never fires): `pc(b->a)` → reversed graph `a -> b`; `blocks(b->c)` → `b -> c`;
    // `blocks(c->a)` → `c -> a` closes `a -> b -> c -> a`. A uniform-forward graph would orient the
    // parent-child edge `b -> a` instead, leaving NO cycle — so this gates the D4 reversal.
    let storage = fresh().await;
    for id in ["ub-a", "ub-b", "ub-c"] {
        storage.create_issue(&issue(id, id), "x").await.unwrap();
    }
    storage
        .add_dependency(&dep("ub-b", "ub-a", DependencyType::ParentChild), "x")
        .await
        .unwrap();
    storage
        .add_dependency(&dep("ub-b", "ub-c", DependencyType::Blocks), "x")
        .await
        .expect("b -> c does not close a cycle yet");
    match storage
        .add_dependency(&dep("ub-c", "ub-a", DependencyType::Blocks), "x")
        .await
    {
        Err(StorageError::CycleDetected { path }) => {
            assert!(
                path.contains("ub-a") && path.contains("ub-b") && path.contains("ub-c"),
                "the mixed cycle names every node: {path}"
            );
        }
        other => panic!("mixed parent-child + blocks cycle must be rejected, got {other:?}"),
    }

    // parent-child + waits-for over three nodes (same structure, waits-for as the blocking type).
    let storage = fresh().await;
    for id in ["ub-x", "ub-y", "ub-z"] {
        storage.create_issue(&issue(id, id), "x").await.unwrap();
    }
    storage
        .add_dependency(&dep("ub-y", "ub-x", DependencyType::ParentChild), "x")
        .await
        .unwrap(); // reversed graph: x -> y
    storage
        .add_dependency(&dep("ub-y", "ub-z", DependencyType::WaitsFor), "x")
        .await
        .expect("y -> z does not close a cycle yet");
    match storage
        .add_dependency(&dep("ub-z", "ub-x", DependencyType::WaitsFor), "x")
        .await
    {
        Err(StorageError::CycleDetected { .. }) => {}
        other => panic!("mixed parent-child + waits-for cycle must be rejected, got {other:?}"),
    }
}

/// M-S3 (cont.) — a `conditional-blocks` cycle and a `waits-for` cycle are each rejected in
/// isolation (untested before despite being in the gating set).
#[tokio::test]
async fn conditional_blocks_and_waits_for_cycles_are_rejected() {
    for ty in [DependencyType::ConditionalBlocks, DependencyType::WaitsFor] {
        let storage = fresh().await;
        for id in ["ub-1", "ub-2"] {
            storage.create_issue(&issue(id, id), "x").await.unwrap();
        }
        storage
            .add_dependency(&dep("ub-1", "ub-2", ty.clone()), "x")
            .await
            .unwrap();
        match storage
            .add_dependency(&dep("ub-2", "ub-1", ty.clone()), "x")
            .await
        {
            Err(StorageError::CycleDetected { .. }) => {}
            other => panic!("a {ty:?} cycle must be rejected, got {other:?}"),
        }
    }
}

/// A pure `parent-child` cycle is rejected at add-time (the original `check_cycle` missed this; D4
/// orientation-consistency fixes it — gates the prospective-edge reversal rule).
#[tokio::test]
async fn pure_parent_child_cycle_is_rejected_at_add_time() {
    let storage = fresh().await;
    for id in ["ub-a", "ub-b", "ub-c"] {
        storage.create_issue(&issue(id, id), "x").await.unwrap();
    }
    // b's parent = a, c's parent = b (reversed graph: a -> b -> c).
    storage
        .add_dependency(&dep("ub-b", "ub-a", DependencyType::ParentChild), "x")
        .await
        .unwrap();
    storage
        .add_dependency(&dep("ub-c", "ub-b", DependencyType::ParentChild), "x")
        .await
        .unwrap();
    // a's parent = c (reversed graph: c -> a) closes a -> b -> c -> a.
    match storage
        .add_dependency(&dep("ub-a", "ub-c", DependencyType::ParentChild), "x")
        .await
    {
        Err(StorageError::CycleDetected { path }) => {
            assert!(
                path.contains("ub-a") && path.contains("ub-b") && path.contains("ub-c"),
                "names every node on the parent-child cycle: {path}"
            );
        }
        other => panic!("a pure parent-child cycle must be rejected at add-time, got {other:?}"),
    }
}

/// `detect_cycles(false)` (all dep types) detects a cycle formed by a NON-gating edge that
/// `detect_cycles(true)` (gating-only) does NOT — proving the `blocking_only` param. (The
/// parent-child-under-both-values + ordered-witness-shape legs need the raw-edge seam; see the
/// `testkit_backed` module below.)
#[tokio::test]
async fn detect_cycles_blocking_only_param() {
    // Non-gating (`related`) cycle: present under false, absent under true.
    let storage = fresh().await;
    for id in ["ub-a", "ub-b"] {
        storage.create_issue(&issue(id, id), "x").await.unwrap();
    }
    storage
        .add_dependency(&dep("ub-a", "ub-b", DependencyType::Related), "x")
        .await
        .unwrap();
    storage
        .add_dependency(&dep("ub-b", "ub-a", DependencyType::Related), "x")
        .await
        .unwrap();
    assert!(
        storage.detect_cycles(true).await.unwrap().is_empty(),
        "a related cycle is invisible to the gating-only view"
    );
    assert!(
        !storage.detect_cycles(false).await.unwrap().is_empty(),
        "a related cycle IS visible to the all-types view"
    );
}

/// M-S6 — a representative non-gating type beyond `Related` (`Duplicates`) added both directions
/// never raises `CycleDetected` and never removes an issue from `ready`.
#[tokio::test]
async fn non_gating_duplicates_edge_never_cycles_or_blocks() {
    let storage = fresh().await;
    for id in ["ub-a", "ub-b"] {
        storage.create_issue(&issue(id, id), "x").await.unwrap();
    }
    storage
        .add_dependency(&dep("ub-a", "ub-b", DependencyType::Duplicates), "x")
        .await
        .expect("duplicates never cycles");
    storage
        .add_dependency(&dep("ub-b", "ub-a", DependencyType::Duplicates), "x")
        .await
        .expect("the reverse duplicates edge never cycles");
    // Neither issue is blocked by a non-gating edge — both remain ready.
    let ready: Vec<String> = storage
        .ready_issues(&ListFilters::default())
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert!(
        ready.contains(&"ub-a".to_string()) && ready.contains(&"ub-b".to_string()),
        "a duplicates edge keeps both issues ready: {ready:?}"
    );
}

/// M-S5 — `dependency_tree(root)` over a small fan-out: the reachable edge set + deterministic
/// `(from, to, dep_type)` edge ordering.
#[tokio::test]
async fn dependency_tree_fan_out_shape_is_deterministic() {
    let storage = fresh().await;
    for id in ["ub-r", "ub-c1", "ub-c2", "ub-g"] {
        storage.create_issue(&issue(id, id), "x").await.unwrap();
    }
    // r -> c2, r -> c1 (added out of order), c1 -> g. (Related so no cycle gating in play.)
    storage
        .add_dependency(&dep("ub-r", "ub-c2", DependencyType::Related), "x")
        .await
        .unwrap();
    storage
        .add_dependency(&dep("ub-r", "ub-c1", DependencyType::Related), "x")
        .await
        .unwrap();
    storage
        .add_dependency(&dep("ub-c1", "ub-g", DependencyType::Related), "x")
        .await
        .unwrap();

    let tree = storage.dependency_tree("ub-r").await.unwrap();
    assert_eq!(tree.root, "ub-r");
    let edges: Vec<(&str, &str)> = tree
        .edges
        .iter()
        .map(|e| (e.from.as_str(), e.to.as_str()))
        .collect();
    // Reachable edge set {r->c1, r->c2, c1->g}, sorted by (from, to, dep_type) regardless of insert
    // order — deterministic for snapshots (NFR-14).
    assert_eq!(
        edges,
        vec![("ub-c1", "ub-g"), ("ub-r", "ub-c1"), ("ub-r", "ub-c2")]
    );
}

/// M-S4 — `dependency_graph` with a NON-EMPTY root set returns only the subgraph reachable from the
/// root, excluding a disjoint component, with the root recorded.
#[tokio::test]
async fn dependency_graph_non_empty_root_excludes_disjoint() {
    let storage = fresh().await;
    for id in ["ub-a", "ub-b", "ub-c", "ub-x", "ub-y"] {
        storage.create_issue(&issue(id, id), "x").await.unwrap();
    }
    // a -> b -> c (the queried component) and a disjoint x -> y.
    storage
        .add_dependency(&dep("ub-a", "ub-b", DependencyType::Related), "x")
        .await
        .unwrap();
    storage
        .add_dependency(&dep("ub-b", "ub-c", DependencyType::Related), "x")
        .await
        .unwrap();
    storage
        .add_dependency(&dep("ub-x", "ub-y", DependencyType::Related), "x")
        .await
        .unwrap();

    let tree = storage
        .dependency_graph(&["ub-a".to_string()])
        .await
        .unwrap();
    assert_eq!(tree.root, "ub-a");
    let edges: Vec<(&str, &str)> = tree
        .edges
        .iter()
        .map(|e| (e.from.as_str(), e.to.as_str()))
        .collect();
    assert_eq!(
        edges,
        vec![("ub-a", "ub-b"), ("ub-b", "ub-c")],
        "only the subgraph reachable from ub-a; the disjoint x -> y is excluded"
    );
}

#[tokio::test]
async fn non_gating_edge_never_cycles() {
    let storage = fresh().await;
    storage
        .create_issue(&issue("ub-a", "a"), "x")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-b", "b"), "x")
        .await
        .unwrap();

    storage
        .add_dependency(&dep("ub-a", "ub-b", DependencyType::Related), "x")
        .await
        .unwrap();
    // The reverse `related` edge is fine — `related` never gates ready work.
    storage
        .add_dependency(&dep("ub-b", "ub-a", DependencyType::Related), "x")
        .await
        .expect("related edges never cycle");
}

#[tokio::test]
async fn self_dependency_rejected() {
    let storage = fresh().await;
    storage
        .create_issue(&issue("ub-a", "a"), "x")
        .await
        .unwrap();
    let err = storage
        .add_dependency(&dep("ub-a", "ub-a", DependencyType::Blocks), "x")
        .await;
    assert!(matches!(err, Err(StorageError::SelfDependency)));
}

#[tokio::test]
async fn dependency_graph_empty_roots_is_whole_graph() {
    let storage = fresh().await;
    storage
        .create_issue(&issue("ub-a", "a"), "x")
        .await
        .unwrap();
    storage
        .create_issue(&issue("ub-b", "b"), "x")
        .await
        .unwrap();
    storage
        .add_dependency(&dep("ub-a", "ub-b", DependencyType::Blocks), "x")
        .await
        .unwrap();

    let tree = storage.dependency_graph(&[]).await.unwrap();
    assert_eq!(tree.edges.len(), 1);
    assert_eq!(tree.edges[0].from, "ub-a");
    assert_eq!(tree.edges[0].to, "ub-b");
}

// --------------------------------------------------------------------------------------------------
// Diagnostics
// --------------------------------------------------------------------------------------------------

#[tokio::test]
async fn integrity_check_clean_db_is_empty() {
    let storage = fresh().await;
    let problems = storage.integrity_check().await.unwrap();
    assert!(problems.is_empty(), "a healthy DB returns no problems");
}

#[tokio::test]
async fn closed_since_returns_closed_issues() {
    let storage = fresh().await;
    storage
        .create_issue(&issue("ub-1", "t"), "a")
        .await
        .unwrap();
    storage
        .update_issue(
            "ub-1",
            &IssuePatch {
                status: Some(Status::Closed),
                ..IssuePatch::default()
            },
            "a",
        )
        .await
        .unwrap();

    let closed = storage.closed_since(None).await.unwrap();
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].id, "ub-1");
}

#[tokio::test]
async fn orphan_candidates_match_commit_hash_external_refs() {
    let storage = fresh().await;
    let mut commit = issue("ub-commit", "has commit ref");
    commit.external_ref = Some("a1b2c3d4e5f6".to_string()); // hex commit-ish
    storage.create_issue(&commit, "a").await.unwrap();

    let mut jira = issue("ub-jira", "has jira ref");
    jira.external_ref = Some("jira-1234".to_string());
    storage.create_issue(&jira, "a").await.unwrap();

    let orphans: Vec<String> = storage
        .orphan_candidates()
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert!(orphans.contains(&"ub-commit".to_string()));
    assert!(!orphans.contains(&"ub-jira".to_string()));
}

/// `epic_child_rollup` (D26/T2.7): per-epic `(child_total, child_closed_or_tombstone)` over
/// `parent-child` edges, non-template children only, non-`parent-child` edges ignored, id-sorted.
#[tokio::test]
async fn epic_child_rollup_counts_children_by_status_excluding_templates() {
    let storage = fresh().await;

    // Two epics so we can assert the id-sorted output over more than one group.
    let mut epic_a = issue("ub-epic-a", "epic A");
    epic_a.issue_type = IssueType::Epic;
    storage.create_issue(&epic_a, "a").await.unwrap();
    let mut epic_b = issue("ub-epic-b", "epic B");
    epic_b.issue_type = IssueType::Epic;
    storage.create_issue(&epic_b, "a").await.unwrap();

    // epic-a: 3 real children (2 closed / 1 open) + 1 TEMPLATE child (EXCLUDED from the rollup).
    for (id, closed) in [("ub-c1", true), ("ub-c2", true), ("ub-c3", false)] {
        let mut child = issue(id, "child");
        if closed {
            child.status = Status::Closed;
            child.closed_at = Some(ts(2026, 1, 2));
        }
        storage.create_issue(&child, "a").await.unwrap();
        // Parent-child edge: issue_id = child, depends_on_id = epic (stored orientation).
        storage
            .add_dependency(&dep(id, "ub-epic-a", DependencyType::ParentChild), "a")
            .await
            .unwrap();
    }
    let mut template_child = issue("ub-tmpl", "template child");
    template_child.is_template = true;
    storage.create_issue(&template_child, "a").await.unwrap();
    storage
        .add_dependency(
            &dep("ub-tmpl", "ub-epic-a", DependencyType::ParentChild),
            "a",
        )
        .await
        .unwrap();

    // A NON-parent-child edge onto epic-a (Blocks) must be IGNORED by the rollup.
    let mut blocker = issue("ub-blk", "blocker child (wrong edge type)");
    blocker.status = Status::Closed;
    blocker.closed_at = Some(ts(2026, 1, 2));
    storage.create_issue(&blocker, "a").await.unwrap();
    storage
        .add_dependency(&dep("ub-blk", "ub-epic-a", DependencyType::Blocks), "a")
        .await
        .unwrap();

    // epic-b: 1 tombstone child (counts as closed_or_tombstone) + 1 open child.
    let mut tomb = issue("ub-d1", "tombstone child");
    tomb.status = Status::Tombstone;
    storage.create_issue(&tomb, "a").await.unwrap();
    storage
        .add_dependency(&dep("ub-d1", "ub-epic-b", DependencyType::ParentChild), "a")
        .await
        .unwrap();
    let open_child = issue("ub-d2", "open child");
    storage.create_issue(&open_child, "a").await.unwrap();
    storage
        .add_dependency(&dep("ub-d2", "ub-epic-b", DependencyType::ParentChild), "a")
        .await
        .unwrap();

    let rollup = storage.epic_child_rollup().await.unwrap();

    // Id-sorted (SQL ORDER BY), template child excluded, wrong-edge-type child ignored.
    assert_eq!(
        rollup,
        vec![
            ("ub-epic-a".to_string(), (3, 2)), // 3 real children, 2 closed (template + Blocks ignored)
            ("ub-epic-b".to_string(), (2, 1)), // tombstone counts as closed_or_tombstone
        ],
    );
}

/// An empty store yields an empty rollup (no panics, no git).
#[tokio::test]
async fn epic_child_rollup_is_empty_on_empty_store() {
    let storage = fresh().await;
    assert!(storage.epic_child_rollup().await.unwrap().is_empty());
}

// --------------------------------------------------------------------------------------------------
// schema_version (D27/AF-2, T3.1) — PRAGMA user_version read via the public Storage trait
// --------------------------------------------------------------------------------------------------

/// A fresh, **un-migrated** store reports `0` (unstamped `PRAGMA user_version`); after `migrate()`
/// it reports the current stamped baseline (a positive version).
#[tokio::test]
async fn schema_version_zero_before_migrate_positive_after() {
    // Un-migrated: open_in_memory does NOT stamp a version.
    let storage = LibsqlStorage::open_in_memory().await.expect("open");
    assert_eq!(
        storage
            .schema_version()
            .await
            .expect("schema_version fresh"),
        0,
        "a fresh, never-migrated DB reports user_version 0"
    );

    // Migrating bootstraps + stamps the baseline.
    storage.migrate().await.expect("migrate");
    let migrated = storage
        .schema_version()
        .await
        .expect("schema_version migrated");
    assert!(
        migrated >= 1,
        "a migrated DB reports its stamped baseline (>= 1), got {migrated}"
    );

    // Pure read: re-reading (and re-migrating idempotently) does not change the reported version.
    storage.migrate().await.expect("re-migrate no-op");
    assert_eq!(
        storage
            .schema_version()
            .await
            .expect("schema_version stable"),
        migrated,
        "schema_version is a stable pure read; an idempotent re-migrate does not advance it"
    );
}

// --------------------------------------------------------------------------------------------------
// helpers
// --------------------------------------------------------------------------------------------------

fn dep(from: &str, to: &str, dep_type: DependencyType) -> Dependency {
    Dependency {
        issue_id: from.to_string(),
        depends_on_id: to.to_string(),
        dep_type,
        created_at: ts(2026, 1, 1),
        created_by: None,
        metadata: None,
        thread_id: None,
    }
}

/// The current blocked-set ids.
async fn blocked_ids(storage: &LibsqlStorage) -> Vec<String> {
    storage
        .blocked_issues(&ListFilters::default())
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect()
}

/// The current ready-set ids.
async fn ready_ids(storage: &LibsqlStorage) -> Vec<String> {
    storage
        .ready_issues(&ListFilters::default())
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect()
}

/// The total row count (active + closed + tombstone) over `id LIKE 'ub-%'`, via the public read path.
async fn count_all(storage: &LibsqlStorage) -> usize {
    let filters = ListFilters {
        include_closed: true,
        include_deferred: true,
        ..ListFilters::default()
    };
    storage.list_issues(&filters).await.unwrap().len()
}

// --------------------------------------------------------------------------------------------------
// create_issues — the ATOMIC bulk INSERT (D22/T2.3, spine §3.2.1)
// --------------------------------------------------------------------------------------------------

/// A clean N-record batch inserts all N rows + their `Event(Created)` in one tx, round-tripping via
/// `get_issues`.
#[tokio::test]
async fn create_issues_inserts_whole_batch_in_one_tx() {
    let storage = fresh().await;
    let batch = vec![
        issue("ub-a", "alpha"),
        issue("ub-b", "beta"),
        issue("ub-c", "gamma"),
    ];

    storage
        .create_issues(&batch, "alice")
        .await
        .expect("bulk create");

    let ids: Vec<String> = batch.iter().map(|i| i.id.clone()).collect();
    let loaded = storage.get_issues(&ids).await.expect("get_issues");
    assert_eq!(loaded.len(), 3, "all three rows persisted");
    for id in &ids {
        assert_eq!(
            event_types(&storage, id).await,
            vec!["created".to_string()],
            "each row gets exactly one Created event",
        );
    }
}

/// A sibling-to-sibling dependency edge (record B depends on record A, BOTH in the same batch)
/// resolves and persists — both rows live in the one uncommitted tx.
#[tokio::test]
async fn create_issues_resolves_intra_batch_sibling_edge() {
    let storage = fresh().await;
    let mut b = issue("ub-b", "beta");
    b.dependencies = vec![dep("ub-b", "ub-a", DependencyType::Blocks)];
    let batch = vec![issue("ub-a", "alpha"), b];

    storage
        .create_issues(&batch, "alice")
        .await
        .expect("bulk create with sibling edge");

    let edges = storage.list_dependencies("ub-b").await.expect("deps");
    assert_eq!(edges.len(), 1, "the sibling edge persisted");
    assert_eq!(edges[0].depends_on_id, "ub-a");
}

/// FAULT-INJECTION ROLLBACK: a batch whose record #k's id duplicates a pre-existing COMMITTED row
/// fails with `IdCollision` and leaves ZERO rows from the batch — records 1..k-1 are NOT persisted
/// (the all-or-nothing atomicity proof, spine §3.2.1).
#[tokio::test]
async fn create_issues_rolls_back_whole_batch_on_mid_batch_collision() {
    let storage = fresh().await;
    // Pre-seed a COMMITTED row so the batch's 3rd record collides with it.
    storage
        .create_issue(&issue("ub-c", "pre-existing"), "alice")
        .await
        .expect("seed");
    let pre_count = count_all(&storage).await;
    assert_eq!(pre_count, 1);

    // The batch: two fresh records, then a record that collides with the committed `ub-c`.
    let batch = vec![
        issue("ub-a", "alpha"),
        issue("ub-b", "beta"),
        issue("ub-c", "collides"),
    ];
    let err = storage
        .create_issues(&batch, "alice")
        .await
        .expect_err("mid-batch collision must fail the whole batch");
    assert!(
        matches!(err, StorageError::IdCollision { id } if id == "ub-c"),
        "the failure is the IdCollision on record #k",
    );

    // ZERO rows from the batch persisted: the count is unchanged from before the call, and the two
    // records staged before #k (ub-a, ub-b) are absent.
    assert_eq!(
        count_all(&storage).await,
        pre_count,
        "row count unchanged — no partial batch",
    );
    assert!(
        storage.get_issue("ub-a").await.unwrap().is_none(),
        "record #1 staged before the failure must NOT persist",
    );
    assert!(
        storage.get_issue("ub-b").await.unwrap().is_none(),
        "record #2 staged before the failure must NOT persist",
    );
}

/// An EMPTY batch is a no-op `Ok` (one trivially-committed tx, no rows).
#[tokio::test]
async fn create_issues_empty_batch_is_noop_ok() {
    let storage = fresh().await;
    storage
        .create_issues(&[], "alice")
        .await
        .expect("empty batch is Ok");
    assert_eq!(count_all(&storage).await, 0);
}

/// **Batch-hydration chunk boundary (T3.5.1):** the batched read path chunks its `WHERE id IN (…)`
/// list at `HYDRATE_ID_CHUNK` (900). With a corpus larger than one chunk, an issue whose labels +
/// dependencies live in the SECOND chunk must still hydrate correctly, and the global result order
/// must be preserved across the chunk boundary — the single-accumulator + ordered-reconstruct proof
/// (a bug that processed only the first chunk, or reset the accumulator per chunk, would be caught).
///
/// Runs against a **file-backed** DB (a unique temp dir, like `scale.rs`) — NOT the shared-cache
/// `open_in_memory` path: a 900+-row bulk insert on the process-global shared-cache registry would
/// aggravate its documented parallel-open contention (`mod.rs::memory_open_lock`) and flake OTHER
/// in-memory tests' opens. A file DB has no shared cache, so it stays isolated under `cargo test`.
#[tokio::test]
async fn batch_hydration_second_chunk_hydrates_and_order_preserved() {
    // 902 issues > the 900-id chunk size → two chunks. All share the default priority + fixed
    // created_at, so the `list` order is a pure `id ASC` over a dense zero-padded sequence.
    const N: usize = 902;
    // The 901st id (index 900) is the FIRST row of the SECOND chunk.
    const SECOND_CHUNK_IDX: usize = 900;

    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("unblock.db");
    let storage =
        LibsqlStorage::open_local(&db_path, unblock_storage::DEFAULT_WRITE_LOCK_TIMEOUT_MS)
            .await
            .expect("open_local");
    storage.migrate().await.expect("migrate");
    let target_id = format!("ub-{SECOND_CHUNK_IDX:07}");

    let mut batch: Vec<Issue> = Vec::with_capacity(N);
    for i in 0..N {
        let mut it = issue(&format!("ub-{i:07}"), &format!("issue {i}"));
        if i == SECOND_CHUNK_IDX {
            // Distinct labels (inserted UNSORTED) + an external (non-gating) blocks-dep to hydrate.
            it.labels = vec!["z-label".to_string(), "a-label".to_string()];
            it.dependencies = vec![dep(&target_id, "external:blk", DependencyType::Blocks)];
        }
        batch.push(it);
    }
    storage
        .create_issues(&batch, "seed")
        .await
        .expect("bulk create");

    let listed = storage
        .list_issues(&ListFilters::default())
        .await
        .expect("list");
    assert_eq!(listed.len(), N, "every issue is present across BOTH chunks");

    // Global order preserved across the boundary: a dense id-ascending sequence.
    for (i, it) in listed.iter().enumerate() {
        assert_eq!(it.id, format!("ub-{i:07}"), "order preserved at index {i}");
    }

    // The second-chunk issue carries ITS OWN hydrated labels (sorted `label ASC`) + deps.
    let second = &listed[SECOND_CHUNK_IDX];
    assert_eq!(second.id, target_id);
    assert_eq!(
        second.labels,
        vec!["a-label".to_string(), "z-label".to_string()],
        "second-chunk issue labels hydrate sorted"
    );
    let dep_targets: Vec<&str> = second
        .dependencies
        .iter()
        .map(|d| d.depends_on_id.as_str())
        .collect();
    assert_eq!(
        dep_targets,
        ["external:blk"],
        "second-chunk issue dep hydrates"
    );

    // A first-chunk issue has NO labels/deps — no cross-chunk bleed.
    assert!(
        listed[0].labels.is_empty(),
        "first-chunk issue has no labels"
    );
    assert!(
        listed[0].dependencies.is_empty(),
        "first-chunk issue has no deps"
    );
}

// --------------------------------------------------------------------------------------------------
// detect_cycles ordered-witness shape (needs the raw-edge seam to plant a STORED cycle the public
// guard rejects) — gated on the `testkit` feature (the seam is unreachable from an integration test
// without it; an integration crate cannot see the lib's `#[cfg(test)]` items).
// --------------------------------------------------------------------------------------------------

#[cfg(feature = "testkit")]
mod testkit_backed {
    use super::{dep, fresh, issue};
    use unblock_model::DependencyType;
    use unblock_storage::{Storage, StorageTestkit};

    /// M-S2 — `detect_cycles(true)` returns the ordered-witness shape (`[start, …, start]`) for a
    /// planted multi-node gating cycle, `[]` on acyclic; a self-loop reports `[node, node]`.
    #[tokio::test]
    async fn detect_cycles_ordered_witness_shape() {
        let storage = fresh().await;
        for id in ["ub-a", "ub-b", "ub-c"] {
            storage.create_issue(&issue(id, id), "x").await.unwrap();
        }
        // Acyclic chain a -> b -> c.
        storage
            .add_dependency(&dep("ub-a", "ub-b", DependencyType::Blocks), "x")
            .await
            .unwrap();
        storage
            .add_dependency(&dep("ub-b", "ub-c", DependencyType::Blocks), "x")
            .await
            .unwrap();
        assert!(
            storage.detect_cycles(true).await.unwrap().is_empty(),
            "an acyclic gating graph yields []"
        );

        // Plant the closing edge c -> a RAW (the public guard would reject it) → 3-node gating cycle.
        storage
            .testkit_insert_raw_edge(&dep("ub-c", "ub-a", DependencyType::Blocks))
            .await
            .expect("raw edge planted");
        let cycles = storage.detect_cycles(true).await.unwrap();
        assert_eq!(cycles.len(), 1, "exactly one cyclic component: {cycles:?}");
        let witness = &cycles[0];
        assert_eq!(
            witness.first(),
            witness.last(),
            "ordered witness [start, …, start]: {witness:?}"
        );
        assert_eq!(
            witness.len(),
            4,
            "3-node cycle witness has 4 entries: {witness:?}"
        );
        let names: std::collections::HashSet<&str> = witness.iter().map(String::as_str).collect();
        assert!(
            names.contains("ub-a") && names.contains("ub-b") && names.contains("ub-c"),
            "every node is named: {witness:?}"
        );

        // Self-loop planted raw → witness [node, node].
        let storage = fresh().await;
        storage
            .create_issue(&issue("ub-self", "s"), "x")
            .await
            .unwrap();
        storage
            .testkit_insert_raw_edge(&dep("ub-self", "ub-self", DependencyType::Blocks))
            .await
            .expect("raw self-edge planted");
        let cycles = storage.detect_cycles(true).await.unwrap();
        assert_eq!(
            cycles,
            vec![vec!["ub-self".to_string(), "ub-self".to_string()]],
            "a self-loop reports [node, node]"
        );
    }

    /// M-S2 (cont.) — a stored `parent-child` cycle is caught under BOTH `blocking_only` values
    /// (reversed regardless), complementing the `related`-only leg in
    /// `detect_cycles_blocking_only_param`.
    #[tokio::test]
    async fn detect_cycles_parent_child_under_both_values() {
        let storage = fresh().await;
        for id in ["ub-x", "ub-y"] {
            storage.create_issue(&issue(id, id), "x").await.unwrap();
        }
        // Plant a stored 2-node parent-child cycle (the public add-time guard now rejects this).
        storage
            .testkit_insert_raw_edge(&dep("ub-y", "ub-x", DependencyType::ParentChild))
            .await
            .unwrap();
        storage
            .testkit_insert_raw_edge(&dep("ub-x", "ub-y", DependencyType::ParentChild))
            .await
            .unwrap();
        assert!(
            !storage.detect_cycles(true).await.unwrap().is_empty(),
            "parent-child cycle visible to the gating view"
        );
        assert!(
            !storage.detect_cycles(false).await.unwrap().is_empty(),
            "parent-child cycle visible to the all-types view"
        );
    }
}

// --------------------------------------------------------------------------------------------------
// Row <-> domain round-trip (proptest)
// --------------------------------------------------------------------------------------------------

mod roundtrip {
    use super::{fresh, ts};
    use proptest::prelude::*;
    use unblock_model::{Issue, Priority, Status};
    use unblock_storage::Storage;

    /// An arbitrary issue with a valid id + non-empty title + bounded priority + a handful of
    /// optional text fields. The content hash is recomputed on load, so a create→get round-trip must
    /// preserve every hashed field (and the recomputed hash must match).
    fn arb_issue() -> impl Strategy<Value = Issue> {
        // Optional text fields use a NON-empty inner string: the storage layer coalesces an empty
        // string to `None` on load (the documented `''<->None` bd-compatibility rule), so `Some("")`
        // is semantically `None` and would not round-trip to itself by design.
        (
            "[a-z]{1,6}",                        // hash portion (lowercase alnum subset)
            "[ -~]{1,40}",                       // title (printable ASCII)
            0i32..=4,                            // priority
            prop::option::of("[ -~]{1,30}"),     // description (non-empty)
            prop::option::of("[a-z0-9_]{1,20}"), // assignee
            prop::option::of("[a-z0-9_]{1,20}"), // owner
            any::<bool>(),                       // pinned
        )
            .prop_map(
                |(hash, title, priority, description, assignee, owner, pinned)| Issue {
                    id: format!("ub-{hash}"),
                    title: format!("t{title}"), // guarantee non-empty/non-whitespace
                    description,
                    assignee,
                    owner,
                    priority: Priority(priority),
                    pinned,
                    status: Status::Open,
                    created_at: ts(2026, 1, 1),
                    updated_at: ts(2026, 1, 1),
                    ..Issue::default()
                },
            )
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]

        #[test]
        fn create_then_get_round_trips(issue in arb_issue()) {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async {
                let storage = fresh().await;
                let expected_hash = issue.compute_content_hash();
                storage.create_issue(&issue, "tester").await.unwrap();
                let loaded = storage.get_issue(&issue.id).await.unwrap().unwrap();

                prop_assert_eq!(&loaded.title, &issue.title);
                prop_assert_eq!(&loaded.description, &issue.description);
                prop_assert_eq!(&loaded.assignee, &issue.assignee);
                prop_assert_eq!(&loaded.owner, &issue.owner);
                prop_assert_eq!(loaded.priority, issue.priority);
                prop_assert_eq!(loaded.pinned, issue.pinned);
                // content_hash is recomputed on load and must equal the model's hash.
                prop_assert_eq!(loaded.content_hash, Some(expected_hash));
                Ok(())
            })?;
        }
    }
}
