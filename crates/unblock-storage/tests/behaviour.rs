//! Behavioural contract tests for the libsql backend (T0.6).
//!
//! These exercise the spine §3.2.1 semantics directly against `LibsqlStorage::open_in_memory`:
//! schema shape (insta column-order + index-list goldens, CHECK rejection), migration stamping,
//! per-mutation `EventType` emission, ready/blocked/search behaviour, claim contention, and cycle
//! detection. The reusable backend-independent contract suite (NFR-16) lands at T0.7; this file is
//! the libsql-specific behavioural floor.

use chrono::{DateTime, TimeZone, Utc};

use unblock_model::{Dependency, DependencyType, Issue, ListFilters, Priority, Status};
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

    let events = event_types(&storage, "ub-1").await;
    assert!(events.contains(&"status_changed".to_string()));
    assert!(events.contains(&"priority_changed".to_string()));
    assert!(events.contains(&"assignee_changed".to_string()));
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

    let events = event_types(&storage, "ub-1").await;
    assert!(events.contains(&"closed".to_string()));
    assert!(events.contains(&"reopened".to_string()));
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
    assert!(
        event_types(&storage, "ub-1")
            .await
            .contains(&"deleted".to_string())
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
        Err(StorageError::CycleDetected { path }) => assert!(path.contains("ub-b")),
        other => panic!("expected CycleDetected, got {other:?}"),
    }
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
