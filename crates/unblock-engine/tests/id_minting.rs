//! `Session::create_issue` (D21/T1.8) — the MINTING create path over a real in-memory libsql
//! `Session` (NOT a mock; the engine's contract is "identical behaviour through one path", FR-9).
//!
//! Covers: a root `ub-<hash>` round-trip; a child `parent.N`; a forced first-candidate collision that
//! retries to a longer hash; a slug `ub-<slug>-<hash>`; a slug-budget fallback to hash-only; a
//! config-derived prefix; the atomicity guard (concurrent children under one parent yield distinct
//! `parent.N` with no lost write); and that `create(&Issue)` (the import path) still works.

mod common;

use std::sync::Arc;

use unblock_engine::{NewIssue, Session, SessionConfig};
use unblock_model::{Priority, optimal_hash_length, parse_id};
use unblock_storage::{LibsqlStorage, Storage};

use common::collide::CollisionForcer;
use common::{issue, session};

/// A root mint produces a parseable `ub-<hash>` and round-trips through `get`.
#[tokio::test]
async fn root_mint_round_trips_a_hash_id() {
    let s = session().await;
    let created = s
        .create_issue(NewIssue {
            title: "a root issue".to_string(),
            ..NewIssue::default()
        })
        .await
        .expect("create_issue");

    let parsed = parse_id(&created.id).expect("minted id parses");
    assert_eq!(parsed.prefix, "ub");
    assert!(parsed.is_root(), "a root mint is non-hierarchical");
    assert!((3..=8).contains(&parsed.hash.len()), "adaptive hash length");

    // The issue is really in the store and hydrates.
    let fetched = s.get(&created.id).await.expect("get").expect("present");
    assert_eq!(fetched.id, created.id);
    assert_eq!(fetched.title, "a root issue");
    assert_eq!(fetched.created_by.as_deref(), Some("tester"));
}

/// A child mint (`parent` set) produces `parent.N` via `next_child_number`.
#[tokio::test]
async fn child_mint_produces_parent_dot_n() {
    let s = session().await;
    let parent = s
        .create_issue(NewIssue {
            title: "parent".to_string(),
            ..NewIssue::default()
        })
        .await
        .expect("parent");

    let child = s
        .create_issue(NewIssue {
            title: "child".to_string(),
            parent: Some(parent.id.clone()),
            ..NewIssue::default()
        })
        .await
        .expect("child");
    assert_eq!(
        child.id,
        format!("{}.1", parent.id),
        "first child is parent.1"
    );

    let child2 = s
        .create_issue(NewIssue {
            title: "child 2".to_string(),
            parent: Some(parent.id.clone()),
            ..NewIssue::default()
        })
        .await
        .expect("child 2");
    assert_eq!(
        child2.id,
        format!("{}.2", parent.id),
        "second child is parent.2"
    );

    // Both children parse as hierarchical ids of the parent.
    let parsed = parse_id(&child2.id).expect("child id parses");
    assert_eq!(parsed.parent().as_deref(), Some(parent.id.as_str()));
}

/// Minting the SAME title many times always yields DISTINCT, parseable ids — the collision-retry
/// probe loop never returns an id that already exists (it bumps the nonce / grows the hash).
#[tokio::test]
async fn repeated_mints_of_one_title_never_collide() {
    let s = session().await;

    let mut ids = std::collections::HashSet::new();
    for _ in 0..20 {
        let next = s
            .create_issue(NewIssue {
                title: "collide me".to_string(),
                ..NewIssue::default()
            })
            .await
            .expect("mint");
        assert!(parse_id(&next.id).is_ok(), "every minted id parses");
        assert!(
            ids.insert(next.id.clone()),
            "the probe loop must never return a colliding id ({})",
            next.id
        );
    }
}

/// A deterministic forced collision: pre-occupy the `parent.1` slot via the import path, then mint a
/// child and confirm the allocator's `next_child_number` read probed past it to `parent.2`.
#[tokio::test]
async fn collision_with_preoccupied_candidate_is_avoided() {
    let s = session().await;

    let parent = s
        .create_issue(NewIssue {
            title: "p".to_string(),
            ..NewIssue::default()
        })
        .await
        .expect("parent");

    // Pre-occupy parent.1 via the import path (caller-supplied id, no mint).
    let mut occupied = issue(&format!("{}.1", parent.id), Priority::MEDIUM, 10);
    occupied.title = "pre-occupied .1".to_string();
    s.create(&occupied).await.expect("seed parent.1");

    // Now a minted child must take parent.2 (next_child_number's high-water reflects the seeded .1).
    let child = s
        .create_issue(NewIssue {
            title: "minted child".to_string(),
            parent: Some(parent.id.clone()),
            ..NewIssue::default()
        })
        .await
        .expect("minted child");
    assert_eq!(
        child.id,
        format!("{}.2", parent.id),
        "the minted child must skip the pre-occupied .1 and take .2"
    );
}

/// **Forces the hash-length-EXTENSION branch (`session/ids.rs:93`) to fire, and asserts it did.**
///
/// The allocator probes nonces `0..10` at the adaptive base length (`optimal_hash_length(0) == 3`
/// for the empty store), and only when **all ten** collide does it grow the length by one and retry.
/// A [`CollisionForcer`] makes every root candidate whose hash segment is exactly the base length
/// appear already-occupied, so the loop must exhaust all ten base-length nonces and take the
/// extension branch — yielding a minted hash that is strictly LONGER than the base length.
///
/// Non-vacuous: we assert (a) the forcer was probed exactly **ten** times at the base length (proving
/// the whole base rung was tried, i.e. `>10` collisions were not silently skipped) and (b) the minted
/// hash is `base_len + 1` (the extension actually advanced the ladder). If the `length += 1` branch
/// were removed, the loop could never return a non-base-length id over this forcer (it would spin on
/// the saturated fallback or loop), so this test would fail.
///
/// **Timing-independent:** we count the base-rung PROBES (`base_rung_probes`), not the de-duplicated
/// SET of shadowed ids. The 10 base-length candidates are `created_at`-seeded; two seeds can hash to
/// the same 3-char base36 digits, which would shrink a distinct-id set below 10 at certain timestamps
/// (a real flake under heavy concurrent-build CPU contention). The probe count is exactly 10 on every
/// run because the loop always tries nonces `0..10` before it can extend.
#[tokio::test]
async fn hash_collision_extends_the_length() {
    // Real in-memory libsql, migrated, then wrapped so every base-length root candidate collides.
    let storage = LibsqlStorage::open_in_memory().await.expect("open");
    storage.migrate().await.expect("migrate");
    let inner: Arc<dyn Storage> = Arc::new(storage);

    // Empty store → base adaptive length is the minimum (3); shadow exactly that rung.
    let base_len = optimal_hash_length(0);
    assert_eq!(
        base_len, 3,
        "empty-store base hash length is the minimum (3)"
    );
    let forcer = CollisionForcer::new(inner, base_len);
    let storage: Arc<dyn Storage> = forcer.clone();

    let s = session_with_prefix(storage, "ub").await;

    let created = s
        .create_issue(NewIssue {
            title: "force a hash extension".to_string(),
            ..NewIssue::default()
        })
        .await
        .expect("mint over the collision forcer");

    let parsed = parse_id(&created.id).expect("the extended id still parses");
    assert_eq!(parsed.prefix, "ub");
    assert!(parsed.is_root(), "still a root id, just a longer hash");

    // (a) All ten base-length nonces were probed and reported occupied — the full base rung was tried.
    // Asserting the PROBE COUNT (not the de-duplicated shadowed set) makes this timing-independent:
    // the loop always probes nonces 0..10 at the base length, so this is exactly 10 on every run, even
    // when two `created_at`-seeded nonces collide to the same base-length hash.
    assert_eq!(
        forcer.base_rung_probes(),
        10,
        "the loop must exhaust all ten base-length nonces before extending the hash"
    );

    // (b) The extension branch advanced the length: the minted hash is base_len + 1 (length 4 here),
    // strictly longer than the base length — the EXTENSION branch fired.
    assert!(
        parsed.hash.len() > base_len,
        "the minted hash {} (len {}) must be LONGER than the base length {} — the extension branch fired",
        parsed.hash,
        parsed.hash.len(),
        base_len
    );
    assert_eq!(
        parsed.hash.len(),
        base_len + 1,
        "exactly one extension step was needed (length-4 nonce 0 is free over the forcer)"
    );
}

/// A slug mint produces `ub-<slug>-<hash>` (the slug embedded between prefix and hash).
#[tokio::test]
async fn slug_mint_embeds_the_slug() {
    let s = session().await;
    let created = s
        .create_issue(NewIssue {
            title: "whatever".to_string(),
            slug: Some("Survey My Thing!".to_string()),
            ..NewIssue::default()
        })
        .await
        .expect("slug mint");

    assert!(
        created.id.starts_with("ub-survey-my-thing-"),
        "expected ub-survey-my-thing-<hash>, got {}",
        created.id
    );
    let parsed = parse_id(&created.id).expect("slug id parses");
    assert_eq!(parsed.prefix, "ub-survey-my-thing");
    assert!(!parsed.hash.is_empty(), "the hash suffix is always present");
}

/// A slug that exhausts the prefix budget drops the slug and falls back to a parseable hash-only id.
#[tokio::test]
async fn slug_budget_fallback_drops_to_hash_only() {
    // A long config prefix leaves no room for any slug → the allocator drops to `<prefix>-<hash>`.
    let storage = LibsqlStorage::open_in_memory().await.expect("open");
    storage.migrate().await.expect("migrate");
    let storage: Arc<dyn Storage> = Arc::new(storage);

    // Build a session whose config prefix consumes the whole budget.
    let long_prefix = "p".repeat(unblock_model::MAX_ID_PREFIX_LEN);
    let s = session_with_prefix(storage, &long_prefix).await;

    let created = s
        .create_issue(NewIssue {
            title: "no room for a slug".to_string(),
            slug: Some("this-slug-will-be-dropped".to_string()),
            ..NewIssue::default()
        })
        .await
        .expect("budget fallback");

    let parsed = parse_id(&created.id).expect("fallback id parses");
    assert_eq!(
        parsed.prefix, long_prefix,
        "budget exhausted → hash-only, no slug"
    );
    assert!(!parsed.hash.is_empty());
}

/// An empty-after-normalization slug falls back to the hash-only ladder.
#[tokio::test]
async fn empty_slug_falls_back_to_hash_only() {
    let s = session().await;
    let created = s
        .create_issue(NewIssue {
            title: "title".to_string(),
            slug: Some("!!!".to_string()),
            ..NewIssue::default()
        })
        .await
        .expect("empty slug");
    let parsed = parse_id(&created.id).expect("hash-only id parses");
    assert_eq!(parsed.prefix, "ub", "empty-normalizing slug → hash-only");
}

/// The configured prefix is honoured (config-derived, not a constant).
#[tokio::test]
async fn config_prefix_is_honored() {
    let storage = LibsqlStorage::open_in_memory().await.expect("open");
    storage.migrate().await.expect("migrate");
    let storage: Arc<dyn Storage> = Arc::new(storage);
    let s = session_with_prefix(storage, "proj").await;

    let created = s
        .create_issue(NewIssue {
            title: "prefixed".to_string(),
            ..NewIssue::default()
        })
        .await
        .expect("create");
    let parsed = parse_id(&created.id).expect("parses");
    assert_eq!(parsed.prefix, "proj", "the config-derived prefix is used");
}

/// **Atomicity (non-vacuous):** N concurrent `create_issue` under the SAME parent through ONE shared
/// `Session` yield N DISTINCT `parent.N` ids with no lost write — proving the single write permit
/// serializes the `next_child_number` read→insert→bump (if the permit were removed, two tasks could
/// both read counter=k and mint `parent.k`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_children_under_one_parent_are_distinct() {
    let s = Arc::new(session().await);
    let parent = s
        .create_issue(NewIssue {
            title: "shared parent".to_string(),
            ..NewIssue::default()
        })
        .await
        .expect("parent");

    let n = 8usize;
    let mut handles = Vec::with_capacity(n);
    for i in 0..n {
        let s = Arc::clone(&s);
        let parent_id = parent.id.clone();
        handles.push(tokio::spawn(async move {
            s.create_issue(NewIssue {
                title: format!("child {i}"),
                parent: Some(parent_id),
                ..NewIssue::default()
            })
            .await
        }));
    }

    let mut ids = std::collections::HashSet::new();
    for handle in handles {
        let created = handle.await.expect("join").expect("create_issue");
        assert!(
            ids.insert(created.id.clone()),
            "duplicate minted child id {} — the permit failed to serialize the counter",
            created.id
        );
    }
    assert_eq!(
        ids.len(),
        n,
        "every concurrent child got a distinct parent.N"
    );

    // The minted ids are exactly parent.1..=parent.N (no gaps, no lost write).
    let expected: std::collections::HashSet<String> =
        (1..=n).map(|k| format!("{}.{k}", parent.id)).collect();
    assert_eq!(
        ids, expected,
        "the children are exactly parent.1..parent.{n}"
    );
}

/// The import/internal path `create(&Issue)` still inserts a caller-supplied id (never mints, D21).
#[tokio::test]
async fn create_with_caller_supplied_id_still_works() {
    let s = session().await;
    let id = s
        .create(&issue("ub-imported", Priority::MEDIUM, 100))
        .await
        .expect("import create");
    assert_eq!(
        id, "ub-imported",
        "create preserves the caller id (no mint)"
    );
    assert!(s.get("ub-imported").await.expect("get").is_some());
}

/// Build a `Session` over `storage` with a config whose `id_prefix` is `prefix` (the rest defaulted).
async fn session_with_prefix(storage: Arc<dyn Storage>, prefix: &str) -> Session {
    use std::path::PathBuf;
    use unblock_config::{ConfigPaths, ResolvedConfig, WorkspaceContext, WorkspaceSource};

    let workspace_dir = PathBuf::from("/tmp/unblock-test-ws");
    let unblock_dir = workspace_dir.join(".unblock");
    let config = ResolvedConfig {
        id_prefix: prefix.to_string(),
        ..ResolvedConfig::default()
    };
    let paths = ConfigPaths {
        db_path: unblock_dir.join(&config.db_filename),
        jsonl_path: unblock_dir.join(&config.jsonl_filename),
        unblock_dir,
    };
    let ctx = WorkspaceContext {
        storage,
        workspace_dir,
        actor: "tester".to_string(),
        config,
        paths,
        source: WorkspaceSource::WalkUp,
    };
    Session::open(ctx, SessionConfig::default())
        .await
        .expect("open session")
}
