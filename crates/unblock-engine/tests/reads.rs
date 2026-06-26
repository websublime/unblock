//! Read-path integration tests: ready semantics (FR-4/FR-5), hybrid sort (spine §4.1), diagnostics
//! dispatch (FR-15), reads-succeed-while-a-write-holds-the-permit (FR-10), and the FR-4 query AC
//! (filters compose on list/count/search/stale/blocked — D18; count dims; ordering determinism).

mod common;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use common::parked::ParkedStorage;
use common::{add_blocks, issue, seed_open, session, session_over};
use unblock_engine::{DiagnosticKind, IssuePatch};
use unblock_model::{CountGroupBy, Issue, IssueType, ListFilters, Priority, Status};

#[tokio::test]
async fn ready_excludes_blocked_deferred_closed_and_reflects_edge_change() {
    let session = session().await;

    // Three open issues.
    for (id, secs) in [("ub-a", 1000), ("ub-b", 1001), ("ub-c", 1002)] {
        session
            .create(&issue(id, Priority::MEDIUM, secs))
            .await
            .expect("create");
    }

    // ub-b is blocked by ub-a; ub-c is deferred.
    add_blocks(&session, "ub-b", "ub-a").await;
    session
        .defer("ub-c", chrono::Utc::now() + chrono::Duration::days(7))
        .await
        .expect("defer");

    let ready_ids: Vec<String> = session
        .ready(&unblock_model::ListFilters::default())
        .await
        .expect("ready")
        .into_iter()
        .map(|i| i.id)
        .collect();
    // Only ub-a is ready (ub-b blocked, ub-c deferred).
    assert_eq!(ready_ids, vec!["ub-a".to_string()]);

    // Close ub-a -> ub-b's only blocker is resolved -> ub-b becomes ready immediately (FR-5).
    session
        .close_with_suggestions("ub-a", None)
        .await
        .expect("close");
    let ready_after: Vec<String> = session
        .ready(&unblock_model::ListFilters::default())
        .await
        .expect("ready")
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert_eq!(ready_after, vec!["ub-b".to_string()]);
}

#[tokio::test]
async fn ready_hybrid_sort_equals_policy_order_on_a_fixed_corpus() {
    let session = session().await;

    // Corpus crossing the P1/P2 bucket boundary with mixed ages.
    // bucket 0 (P0/P1): ub-p1-old (older) then ub-p0-new (newer) -> oldest-first within bucket.
    // bucket 1 (P2/P3): ub-p2-old then ub-p3-new.
    session
        .create(&issue("ub-p0-new", Priority::CRITICAL, 2000))
        .await
        .expect("c");
    session
        .create(&issue("ub-p1-old", Priority::HIGH, 1000))
        .await
        .expect("c");
    session
        .create(&issue("ub-p3-new", Priority::LOW, 2000))
        .await
        .expect("c");
    session
        .create(&issue("ub-p2-old", Priority::MEDIUM, 1000))
        .await
        .expect("c");

    let ready = session
        .ready(&unblock_model::ListFilters::default())
        .await
        .expect("ready");
    let order: Vec<String> = ready.iter().map(|i| i.id.clone()).collect();

    // Independently re-rank the same set via the policy free fn; the engine order must match.
    let mut expected = ready.clone();
    expected.sort_by(unblock_policy::cmp_ready);
    let expected_order: Vec<String> = expected.iter().map(|i| i.id.clone()).collect();
    assert_eq!(order, expected_order);

    // And it is the byte-faithful hybrid order: bucket 0 oldest-first, then bucket 1 oldest-first.
    assert_eq!(
        order,
        vec![
            "ub-p1-old".to_string(), // bucket 0, created 1000
            "ub-p0-new".to_string(), // bucket 0, created 2000
            "ub-p2-old".to_string(), // bucket 1, created 1000
            "ub-p3-new".to_string(), // bucket 1, created 2000
        ]
    );
}

#[tokio::test]
async fn reads_succeed_while_a_write_holds_the_permit() {
    use unblock_storage::Storage;
    // FR-10: a write parked mid-tx holds the engine's single permit; a concurrent read must still
    // return (reads never touch the permit).
    let inner = unblock_storage::LibsqlStorage::open_in_memory()
        .await
        .expect("open");
    inner.migrate().await.expect("migrate");
    // Seed an issue so the concurrent read has something to return.
    inner
        .create_issue(&issue("ub-seed", Priority::MEDIUM, 1000), "tester")
        .await
        .expect("seed");

    let parked: Arc<ParkedStorage> = ParkedStorage::new(Arc::new(inner));
    let storage: Arc<dyn Storage> = parked.clone();
    let session = Arc::new(session_over(storage, unblock_engine::SessionConfig::default()).await);

    // Spawn a write that will park inside create_issue (holding the engine permit).
    let writer_session = session.clone();
    let writer = tokio::spawn(async move {
        writer_session
            .create(&issue("ub-parked", Priority::MEDIUM, 2000))
            .await
    });

    // Wait until the write is parked mid-tx (permit held).
    parked.wait_until_parked().await;

    // A concurrent read must complete WHILE the write holds the permit (FR-10).
    let read = tokio::time::timeout(Duration::from_secs(2), session.get("ub-seed"))
        .await
        .expect("read must not block on the write permit")
        .expect("read ok");
    assert!(read.is_some(), "the seeded issue is readable mid-write");

    // Release the parked write; it completes its tx.
    parked.release();
    let created = writer.await.expect("join").expect("parked write completes");
    assert_eq!(created, "ub-parked");
}

#[tokio::test]
async fn search_applies_default_cap_when_limit_unset_and_honours_an_explicit_limit() {
    // FR-4: with no `filters.limit`, the engine fills the default `search_cap` (50). Seed 55 matching
    // issues so the uncapped result would be 55 — the cap must clamp it to 50; an explicit small
    // limit must be honoured verbatim.
    let session = session().await;
    seed_open(&session, 55).await; // titles are "issue ub-XXXX" — all match the needle "issue".

    // No limit set -> the engine applies the default cap of 50.
    let capped = session
        .search("issue", &ListFilters::default())
        .await
        .expect("search");
    assert_eq!(
        capped.len(),
        50,
        "default search_cap (50) must clamp the result"
    );

    // An explicit limit is honoured verbatim (the engine does not override a set limit).
    let limited = session
        .search(
            "issue",
            &ListFilters {
                limit: Some(7),
                ..ListFilters::default()
            },
        )
        .await
        .expect("search");
    assert_eq!(limited.len(), 7, "an explicit limit must be honoured");
}

#[tokio::test]
async fn count_group_by_status_buckets_sum_to_total() {
    // FR-4: count with a group-by returns per-key buckets; a separate close moves one issue into a
    // distinct status bucket. The buckets must sum to the grand total.
    let session = session().await;
    for (id, secs) in [("ub-a", 1000), ("ub-b", 1001), ("ub-c", 1002)] {
        session
            .create(&issue(id, Priority::MEDIUM, secs))
            .await
            .expect("create");
    }
    // Close one so two statuses exist (open + closed).
    session
        .close_with_suggestions("ub-c", None)
        .await
        .expect("close");

    let filters = ListFilters {
        include_closed: true,
        ..ListFilters::default()
    };
    let buckets = session
        .count(&filters, Some(CountGroupBy::Status))
        .await
        .expect("count");
    let total: usize = buckets.iter().map(|b| b.count).sum();
    assert_eq!(total, 3, "buckets must sum to the 3 issues");
    // At least two distinct status keys are present (open + closed).
    assert!(
        buckets.len() >= 2,
        "a closed issue creates a second status bucket, got {buckets:?}"
    );
}

#[tokio::test]
async fn stale_returns_only_issues_older_than_the_cutoff() {
    // FR-4: `stale(older_than, filters)` returns issues whose `updated_at` predates the cutoff.
    let session = session().await;
    // Two issues created at fixed deterministic timestamps (1000s and 5000s epoch via the corpus
    // builder), so a cutoff between them isolates exactly the older one.
    session
        .create(&issue("ub-old", Priority::MEDIUM, 1000))
        .await
        .expect("create old");
    session
        .create(&issue("ub-new", Priority::MEDIUM, 5000))
        .await
        .expect("create new");

    // Cutoff at 3000s: only ub-old (updated_at == 1000s) is older.
    let cutoff = common::t(3000);
    let stale: Vec<String> = session
        .stale(cutoff, &ListFilters::default())
        .await
        .expect("stale")
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert_eq!(
        stale,
        vec!["ub-old".to_string()],
        "only the older issue is stale"
    );
}

#[tokio::test]
async fn diagnostics_dispatch_covers_every_kind() {
    let session = session().await;
    session
        .create(&issue("ub-0001", Priority::MEDIUM, 1000))
        .await
        .expect("create");

    for kind in [
        DiagnosticKind::Stats,
        DiagnosticKind::Info,
        DiagnosticKind::Where,
        DiagnosticKind::Version,
        DiagnosticKind::Lint,
        DiagnosticKind::Changelog,
        DiagnosticKind::Orphans,
    ] {
        let report = session.diagnostics(kind).await.expect("diagnostics");
        // The report's kind is the INPUT kind (never an integrity placeholder).
        assert_eq!(report.kind, kind);
    }

    // Stats reflects the one created issue in the total.
    let stats = session
        .diagnostics(DiagnosticKind::Stats)
        .await
        .expect("stats");
    let total = stats
        .findings
        .iter()
        .find(|f| f.label == "total")
        .expect("total finding");
    assert_eq!(total.detail, "1");
}

#[tokio::test]
async fn diagnostics_report_shapes_are_golden_pinned() {
    // Golden-pin the v1 DiagnosticReport SHAPE per kind (label set + per-kind detail policy) over a
    // fixed, deterministic corpus, so T2.7 (changelog `since`, richer lint) cannot silently drift the
    // wire shapes. Volatile details (absolute paths in Info/Where, the crate version, env-specific
    // workspace facts) are REDACTED — we pin the structure, not the machine.
    let session = session().await;
    // A deterministic corpus: one open issue + one closed issue (so Stats/Changelog are non-empty).
    session
        .create(&issue("ub-open", Priority::MEDIUM, 1000))
        .await
        .expect("create open");
    session
        .create(&issue("ub-done", Priority::HIGH, 2000))
        .await
        .expect("create done");
    session
        .close_with_suggestions("ub-done", Some("finished".to_string()))
        .await
        .expect("close");

    // For each kind, render a stable "kind: [label=detail, ...]" line, redacting the volatile labels.
    let volatile = [
        "workspace_dir",
        "db_path",
        "jsonl_path",
        "unblock_dir",
        "version",
    ];
    let mut lines = Vec::new();
    for kind in [
        DiagnosticKind::Stats,
        DiagnosticKind::Info,
        DiagnosticKind::Where,
        DiagnosticKind::Version,
        DiagnosticKind::Lint,
        DiagnosticKind::Changelog,
        DiagnosticKind::Orphans,
    ] {
        let report = session.diagnostics(kind).await.expect("diagnostics");
        let rendered: Vec<String> = report
            .findings
            .iter()
            .map(|f| {
                if volatile.contains(&f.label.as_str()) {
                    format!("{}=<redacted>", f.label)
                } else {
                    format!("{}={}", f.label, f.detail)
                }
            })
            .collect();
        lines.push(format!("{kind:?}: [{}]", rendered.join(", ")));
    }

    insta::assert_snapshot!("diagnostics_report_shapes", lines.join("\n"));
}

// --------------------------------------------------------------------------------------------------
// FR-4 query AC (T1.5): filters compose on list/count/search/stale/blocked (D18); count dims;
// ordering determinism (NFR-14 ordering half). All over a real in-memory `Session` (no mock).
// --------------------------------------------------------------------------------------------------

/// Build a full [`Issue`] from the minimal corpus builder, overriding the FR-4 facet fields.
// One positional parameter per FR-4 facet dimension keeps the discriminating corpora compact and
// readable in-line; a builder struct would add noise without aiding the tests.
#[allow(clippy::too_many_arguments)]
fn full_issue(
    id: &str,
    title: &str,
    issue_type: IssueType,
    priority: Priority,
    assignee: Option<&str>,
    labels: &[&str],
    description: Option<&str>,
    secs: i64,
) -> Issue {
    Issue {
        title: title.to_string(),
        issue_type,
        assignee: assignee.map(str::to_string),
        labels: labels.iter().map(|l| (*l).to_string()).collect(),
        description: description.map(str::to_string),
        ..issue(id, priority, secs)
    }
}

/// The id set of a `list` result under `filters`.
async fn list_ids(session: &unblock_engine::Session, filters: &ListFilters) -> HashSet<String> {
    session
        .list(filters)
        .await
        .expect("list")
        .into_iter()
        .map(|i| i.id)
        .collect()
}

/// The id set of a `blocked` result under `filters`.
async fn blocked_ids(session: &unblock_engine::Session, filters: &ListFilters) -> HashSet<String> {
    session
        .blocked(filters)
        .await
        .expect("blocked")
        .into_iter()
        .map(|i| i.id)
        .collect()
}

/// #1 (HEADLINE) — `list` composes facets as an INTERSECTION; each facet narrows (non-vacuity:
/// flip/drop each facet alone and the matching decoy enters). This IS the FR-4 "filters compose" AC.
// The 6-decoy corpus + five flip-each non-vacuity legs are a single coherent AC; splitting would
// fragment the "filters compose" proof.
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn list_composes_multiple_facets_as_intersection() {
    let session = session().await;
    // ub-hit satisfies every facet; each decoy differs on EXACTLY one.
    for spec in [
        (
            "ub-hit",
            "fix the parser",
            IssueType::Task,
            Priority::HIGH,
            Some("alice"),
            &["api"][..],
            1000,
        ),
        (
            "ub-wrongtype",
            "fix the parser",
            IssueType::Bug,
            Priority::HIGH,
            Some("alice"),
            &["api"][..],
            1001,
        ),
        (
            "ub-wrongprio",
            "fix the parser",
            IssueType::Task,
            Priority::LOW,
            Some("alice"),
            &["api"][..],
            1002,
        ),
        (
            "ub-wrongassignee",
            "fix the parser",
            IssueType::Task,
            Priority::HIGH,
            Some("bob"),
            &["api"][..],
            1003,
        ),
        (
            "ub-wronglabel",
            "fix the parser",
            IssueType::Task,
            Priority::HIGH,
            Some("alice"),
            &["ui"][..],
            1004,
        ),
        (
            "ub-wrongtext",
            "tidy the lexer",
            IssueType::Task,
            Priority::HIGH,
            Some("alice"),
            &["api"][..],
            1005,
        ),
    ] {
        let (id, title, ty, prio, assignee, labels, secs) = spec;
        session
            .create(&full_issue(
                id, title, ty, prio, assignee, labels, None, secs,
            ))
            .await
            .expect("create");
    }

    let combined = ListFilters {
        issue_type: vec![IssueType::Task],
        priority_min: Some(Priority::HIGH),
        priority_max: Some(Priority::HIGH),
        assignee: Some("alice".to_string()),
        labels_all: vec!["api".to_string()],
        text_contains: Some("parser".to_string()),
        ..ListFilters::default()
    };
    assert_eq!(
        list_ids(&session, &combined).await,
        HashSet::from(["ub-hit".to_string()]),
        "every facet intersects to exactly ub-hit"
    );

    // Flip-each non-vacuity core: relaxing one facet admits exactly its decoy.
    let drop_type = ListFilters {
        issue_type: Vec::new(),
        ..combined.clone()
    };
    assert!(
        list_ids(&session, &drop_type)
            .await
            .contains("ub-wrongtype"),
        "drop type ⇒ ub-wrongtype enters"
    );

    let widen_prio = ListFilters {
        priority_max: Some(Priority::LOW),
        ..combined.clone()
    };
    assert!(
        list_ids(&session, &widen_prio)
            .await
            .contains("ub-wrongprio"),
        "widen prio ⇒ ub-wrongprio enters"
    );

    let drop_assignee = ListFilters {
        assignee: None,
        ..combined.clone()
    };
    assert!(
        list_ids(&session, &drop_assignee)
            .await
            .contains("ub-wrongassignee"),
        "drop assignee ⇒ ub-wrongassignee enters"
    );

    let drop_labels = ListFilters {
        labels_all: Vec::new(),
        ..combined.clone()
    };
    assert!(
        list_ids(&session, &drop_labels)
            .await
            .contains("ub-wronglabel"),
        "drop labels ⇒ ub-wronglabel enters"
    );

    let drop_text = ListFilters {
        text_contains: None,
        ..combined.clone()
    };
    assert!(
        list_ids(&session, &drop_text)
            .await
            .contains("ub-wrongtext"),
        "drop text ⇒ ub-wrongtext enters"
    );
}

/// #2 — `labels_all` (AND) and `labels_any` (OR) are distinct; together they intersect (AND ∩ OR).
#[tokio::test]
async fn list_label_and_vs_or_discriminates() {
    let session = session().await;
    for (id, labels, secs) in [
        ("ub-a", &["a"][..], 1000),
        ("ub-b", &["b"][..], 1001),
        ("ub-ab", &["a", "b"][..], 1002),
        ("ub-c", &["c"][..], 1003), // the "neither" control
    ] {
        session
            .create(&full_issue(
                id,
                id,
                IssueType::Task,
                Priority::MEDIUM,
                None,
                labels,
                None,
                secs,
            ))
            .await
            .expect("create");
    }

    let and = list_ids(
        &session,
        &ListFilters {
            labels_all: vec!["a".into(), "b".into()],
            ..ListFilters::default()
        },
    )
    .await;
    assert_eq!(
        and,
        HashSet::from(["ub-ab".to_string()]),
        "labels_all=[a,b] ⇒ only the both-carrier"
    );

    let or = list_ids(
        &session,
        &ListFilters {
            labels_any: vec!["a".into(), "b".into()],
            ..ListFilters::default()
        },
    )
    .await;
    assert_eq!(
        or,
        HashSet::from(["ub-a".to_string(), "ub-b".to_string(), "ub-ab".to_string()]),
        "labels_any=[a,b] ⇒ any carrier; ub-c absent"
    );

    let both = list_ids(
        &session,
        &ListFilters {
            labels_all: vec!["a".into(), "b".into()],
            labels_any: vec!["a".into(), "b".into()],
            ..ListFilters::default()
        },
    )
    .await;
    assert_eq!(
        both, and,
        "AND ∩ OR = the AND result (intersection, not replacement)"
    );
    assert_ne!(
        and, or,
        "AND and OR over the same labels are different sets"
    );
}

/// #3 — the `priority` range is inclusive on BOTH ends and direction-pinned (CRITICAL=0 is the
/// LOWEST number — a min/max swap cannot pass).
#[tokio::test]
async fn list_priority_range_is_inclusive_and_direction_pinned() {
    let session = session().await;
    for (id, prio, secs) in [
        ("ub-p0", Priority::CRITICAL, 1000),
        ("ub-p1", Priority::HIGH, 1001),
        ("ub-p2", Priority::MEDIUM, 1002),
        ("ub-p3", Priority::LOW, 1003),
        ("ub-p4", Priority::BACKLOG, 1004),
    ] {
        session
            .create(&issue(id, prio, secs))
            .await
            .expect("create");
    }

    let range = list_ids(
        &session,
        &ListFilters {
            priority_min: Some(Priority::HIGH),
            priority_max: Some(Priority::MEDIUM),
            ..ListFilters::default()
        },
    )
    .await;
    assert_eq!(
        range,
        HashSet::from(["ub-p1".to_string(), "ub-p2".to_string()]),
        "[HIGH,MEDIUM] inclusive on both ends; excludes the lower-numbered P0 and P3/P4"
    );

    let only_p0 = list_ids(
        &session,
        &ListFilters {
            priority_max: Some(Priority::CRITICAL),
            ..ListFilters::default()
        },
    )
    .await;
    assert_eq!(
        only_p0,
        HashSet::from(["ub-p0".to_string()]),
        "priority_max=CRITICAL ⇒ only P0 (lowest number)"
    );

    let all_from_p0 = list_ids(
        &session,
        &ListFilters {
            priority_min: Some(Priority::CRITICAL),
            ..ListFilters::default()
        },
    )
    .await;
    assert_eq!(
        all_from_p0.len(),
        5,
        "priority_min=CRITICAL, max=None ⇒ all five buckets"
    );
}

/// #4 (NEW PROD AC, D18) — `blocked` composes facets AND preserves the blocked set; the
/// deferred-status blocked issue survives a default filter (the A.7 regression guard).
// The 5-issue corpus + the seven a–g sub-assertions are one coherent D18/A.7 proof; splitting it
// would obscure the deferred-preservation invariant it pins.
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn blocked_composes_facets_and_preserves_blocked_set() {
    let session = session().await;

    // A shared blocker so each candidate has an unresolved gating edge.
    session
        .create(&issue("ub-blocker", Priority::MEDIUM, 900))
        .await
        .expect("create");

    // ub-wip: Bug, P0, {x}, blocked + claimed → in_progress.
    session
        .create(&full_issue(
            "ub-wip",
            "wip",
            IssueType::Bug,
            Priority::CRITICAL,
            None,
            &["x"],
            None,
            1000,
        ))
        .await
        .expect("create");
    add_blocks(&session, "ub-wip", "ub-blocker").await;
    session.claim("ub-wip", "bob").await.expect("claim");

    // ub-defer-blocked: Task, P2, {y}, blocked, then STATUS set to deferred (the regression pin).
    session
        .create(&full_issue(
            "ub-defer-blocked",
            "deferred",
            IssueType::Task,
            Priority::MEDIUM,
            None,
            &["y"],
            None,
            1001,
        ))
        .await
        .expect("create");
    add_blocks(&session, "ub-defer-blocked", "ub-blocker").await;
    session
        .update(
            "ub-defer-blocked",
            &IssuePatch {
                status: Some(Status::Deferred),
                ..IssuePatch::default()
            },
        )
        .await
        .expect("set deferred");

    // ub-closed-blocked: blocked then closed (never blocked-visible).
    session
        .create(&issue("ub-closed-blocked", Priority::MEDIUM, 1002))
        .await
        .expect("create");
    add_blocks(&session, "ub-closed-blocked", "ub-blocker").await;
    session
        .close_with_suggestions("ub-closed-blocked", None)
        .await
        .expect("close");

    // ub-tomb-blocked: blocked then tombstoned (should-fix decoy).
    session
        .create(&issue("ub-tomb-blocked", Priority::MEDIUM, 1003))
        .await
        .expect("create");
    add_blocks(&session, "ub-tomb-blocked", "ub-blocker").await;
    session
        .delete(&unblock_engine::DeletePlan {
            mode: unblock_engine::DeleteMode::Tombstone,
            targets: vec!["ub-tomb-blocked".to_string()],
            cascade_children: Vec::new(),
        })
        .await
        .expect("tombstone");

    // ub-free: open, no edge — never blocked.
    session
        .create(&issue("ub-free", Priority::MEDIUM, 1004))
        .await
        .expect("create");

    // (a) default: in_progress + deferred-status blocked present; closed/tombstone/free absent.
    let default = blocked_ids(&session, &ListFilters::default()).await;
    assert!(
        default.contains("ub-wip"),
        "(a) in_progress blocked appears"
    );
    // (b) REQUIRED visibility-preserved pin (A.7): deferred-status blocked issue STILL present.
    assert!(
        default.contains("ub-defer-blocked"),
        "(b) deferred-status blocked survives default (deferred-inclusive)"
    );
    assert!(
        !default.contains("ub-closed-blocked"),
        "(a) closed never blocked-visible"
    );
    assert!(
        !default.contains("ub-tomb-blocked"),
        "(g) tombstone never blocked-visible"
    );
    assert!(!default.contains("ub-free"), "(a) unblocked issue absent");

    // (c) labels_all=[x] narrows to ub-wip (ub-defer-blocked dropped though still blocked).
    let by_label = blocked_ids(
        &session,
        &ListFilters {
            labels_all: vec!["x".into()],
            ..ListFilters::default()
        },
    )
    .await;
    assert_eq!(
        by_label,
        HashSet::from(["ub-wip".to_string()]),
        "(c) labels_all=[x] narrows to ub-wip"
    );

    // (d) priority_max=CRITICAL: ub-wip=P0 survives, ub-defer-blocked=P2 drops (non-vacuous).
    let by_prio = blocked_ids(
        &session,
        &ListFilters {
            priority_max: Some(Priority::CRITICAL),
            ..ListFilters::default()
        },
    )
    .await;
    assert_eq!(
        by_prio,
        HashSet::from(["ub-wip".to_string()]),
        "(d) priority_max=CRITICAL keeps only the P0"
    );

    // (e) REQUIRED no-op pins: include_deferred=false keeps the deferred-blocked; include_closed=true
    //     does NOT add the closed-blocked.
    let no_defer = blocked_ids(
        &session,
        &ListFilters {
            include_deferred: false,
            ..ListFilters::default()
        },
    )
    .await;
    assert!(
        no_defer.contains("ub-defer-blocked"),
        "(e) include_deferred is a no-op on blocked"
    );
    let with_closed = blocked_ids(
        &session,
        &ListFilters {
            include_closed: true,
            ..ListFilters::default()
        },
    )
    .await;
    assert!(
        !with_closed.contains("ub-closed-blocked"),
        "(e) include_closed is a no-op on blocked"
    );

    // (f) ORDER BY unchanged: priority ASC, created_at DESC, id ASC over the default set.
    let ordered: Vec<String> = session
        .blocked(&ListFilters::default())
        .await
        .expect("blocked")
        .into_iter()
        .map(|i| i.id)
        .collect();
    // ub-wip is P0 (sorts first); ub-defer-blocked is P2 (after). Both are the only default members.
    assert_eq!(
        ordered,
        vec!["ub-wip".to_string(), "ub-defer-blocked".to_string()],
        "(f) priority ASC order preserved"
    );

    // (g) should-fix: status=[Closed] ∩ deferred-inclusive base = ∅.
    let status_closed = blocked_ids(
        &session,
        &ListFilters {
            status: vec![Status::Closed],
            ..ListFilters::default()
        },
    )
    .await;
    assert!(
        status_closed.is_empty(),
        "(g) blocked(status=[Closed]) is empty"
    );
}

/// #5 — `count` over every group-by dim plus `None`; the Label group double-counts (per-(issue,label)
/// pair), and the Assignee group carries the COALESCE empty-string key for the assignee-less issue.
// All five count dimensions + the label-double-count derivation are one coherent count-AC proof.
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn count_group_by_all_dims_plus_none_and_label_double_count() {
    let session = session().await;
    session
        .create(&full_issue(
            "ub-1",
            "one",
            IssueType::Task,
            Priority::HIGH,
            Some("alice"),
            &[],
            None,
            1000,
        ))
        .await
        .expect("c");
    session
        .create(&full_issue(
            "ub-2",
            "two",
            IssueType::Bug,
            Priority::HIGH,
            Some("bob"),
            &["x", "y"],
            None,
            1001,
        ))
        .await
        .expect("c");
    session
        .create(&full_issue(
            "ub-3",
            "three",
            IssueType::Task,
            Priority::MEDIUM,
            Some("alice"),
            &["x"],
            None,
            1002,
        ))
        .await
        .expect("c");
    session
        .create(&full_issue(
            "ub-4",
            "four",
            IssueType::Task,
            Priority::MEDIUM,
            None,
            &[],
            None,
            1003,
        ))
        .await
        .expect("c");

    let count_map =
        |buckets: Vec<unblock_model::CountBucket>| -> std::collections::HashMap<String, usize> {
            buckets.into_iter().map(|b| (b.key, b.count)).collect()
        };

    // None → single total bucket.
    let none = session
        .count(&ListFilters::default(), None)
        .await
        .expect("count");
    assert_eq!(none.len(), 1);
    assert_eq!(none[0].key, "total");
    assert_eq!(none[0].count, 4);

    // Each grouped dim sums to 4.
    for dim in [
        CountGroupBy::Status,
        CountGroupBy::Type,
        CountGroupBy::Assignee,
        CountGroupBy::Priority,
    ] {
        let buckets = session
            .count(&ListFilters::default(), Some(dim))
            .await
            .expect("count");
        let sum: usize = buckets.iter().map(|b| b.count).sum();
        assert_eq!(sum, 4, "{dim:?} buckets must sum to 4");
    }

    let by_type = count_map(
        session
            .count(&ListFilters::default(), Some(CountGroupBy::Type))
            .await
            .expect("count"),
    );
    assert_eq!(by_type.get("task"), Some(&3));
    assert_eq!(by_type.get("bug"), Some(&1));

    let by_prio = count_map(
        session
            .count(&ListFilters::default(), Some(CountGroupBy::Priority))
            .await
            .expect("count"),
    );
    assert_eq!(by_prio.get("1"), Some(&2), "two P1 issues");
    assert_eq!(by_prio.get("2"), Some(&2), "two P2 issues");

    // Assignee includes the COALESCE empty-string key for the assignee-less ub-4 (render T2.1 pins it).
    let by_assignee = count_map(
        session
            .count(&ListFilters::default(), Some(CountGroupBy::Assignee))
            .await
            .expect("count"),
    );
    assert_eq!(
        by_assignee.get(""),
        Some(&1),
        "the empty-string assignee key (ub-4) is present"
    );
    assert_eq!(by_assignee.get("alice"), Some(&2));
    assert_eq!(by_assignee.get("bob"), Some(&1));

    // Label double-counts: x:2, y:1 → sum 3 ≠ total 4. Derive expected independently from list labels.
    let label_buckets = session
        .count(&ListFilters::default(), Some(CountGroupBy::Label))
        .await
        .expect("count");
    let label_sum: usize = label_buckets.iter().map(|b| b.count).sum();
    let pairs: usize = session
        .list(&ListFilters::default())
        .await
        .expect("list")
        .into_iter()
        .map(|i| i.labels.len())
        .sum();
    assert_eq!(
        label_sum, pairs,
        "Label group sum equals the (issue,label) pair count"
    );
    assert_ne!(label_sum, 4, "Label double-counts ⇒ sum ≠ total");
}

/// #6 (OQ-C proof) — `count` default visibility matches `list` (both exclude closed + deferred);
/// `include_*` widen both. No code change — pure AC proof.
#[tokio::test]
async fn count_default_visibility_matches_list() {
    let session = session().await;
    session
        .create(&issue("ub-open-1", Priority::MEDIUM, 1000))
        .await
        .expect("c");
    session
        .create(&issue("ub-open-2", Priority::MEDIUM, 1001))
        .await
        .expect("c");
    session
        .create(&issue("ub-closed", Priority::MEDIUM, 1002))
        .await
        .expect("c");
    session
        .close_with_suggestions("ub-closed", None)
        .await
        .expect("close");
    session
        .create(&issue("ub-deferred", Priority::MEDIUM, 1003))
        .await
        .expect("c");
    session
        .update(
            "ub-deferred",
            &IssuePatch {
                status: Some(Status::Deferred),
                ..IssuePatch::default()
            },
        )
        .await
        .expect("set deferred");

    let total = |buckets: Vec<unblock_model::CountBucket>| -> usize {
        buckets.iter().map(|b| b.count).sum()
    };

    let default_count = total(
        session
            .count(&ListFilters::default(), None)
            .await
            .expect("count"),
    );
    let default_list = session
        .list(&ListFilters::default())
        .await
        .expect("list")
        .len();
    assert_eq!(
        default_count, default_list,
        "count default == list default (both exclude closed+deferred)"
    );
    assert_eq!(default_count, 2, "only the two open issues");

    let with_closed = total(
        session
            .count(
                &ListFilters {
                    include_closed: true,
                    ..ListFilters::default()
                },
                None,
            )
            .await
            .expect("count"),
    );
    assert!(
        with_closed > default_count,
        "include_closed grows the count (closed re-enters)"
    );

    let with_deferred = total(
        session
            .count(
                &ListFilters {
                    include_deferred: true,
                    ..ListFilters::default()
                },
                None,
            )
            .await
            .expect("count"),
    );
    assert!(
        with_deferred > default_count,
        "include_deferred grows the count (deferred re-enters)"
    );
}

/// #7 — `ready` is default-complete ABOVE the search cap (no implicit limit), contrasting `search`'s
/// default cap of 50.
#[tokio::test]
async fn ready_is_default_complete_above_the_search_cap() {
    let session = session().await;
    seed_open(&session, 60).await; // 60 open P2 unblocked issues.

    let ready = session.ready(&ListFilters::default()).await.expect("ready");
    assert_eq!(
        ready.len(),
        60,
        "ready is uncapped (default-complete), above the 50 search cap"
    );
}

/// #8 — `text_contains` is title-ONLY and ESCAPE-guarded; it differs from `search` (which scans
/// title + description + id).
#[tokio::test]
async fn text_contains_is_title_only_and_escape_guarded() {
    let session = session().await;
    session
        .create(&full_issue(
            "ub-titlematch",
            "find the WIDGET",
            IssueType::Task,
            Priority::MEDIUM,
            None,
            &[],
            None,
            1000,
        ))
        .await
        .expect("c");
    session
        .create(&full_issue(
            "ub-descmatch",
            "unrelated",
            IssueType::Task,
            Priority::MEDIUM,
            None,
            &[],
            Some("the WIDGET lives here"),
            1001,
        ))
        .await
        .expect("c");
    session
        .create(&full_issue(
            "ub-pct",
            "50% off",
            IssueType::Task,
            Priority::MEDIUM,
            None,
            &[],
            None,
            1002,
        ))
        .await
        .expect("c");
    session
        .create(&full_issue(
            "ub-plain",
            "all clear",
            IssueType::Task,
            Priority::MEDIUM,
            None,
            &[],
            None,
            1003,
        ))
        .await
        .expect("c");

    // text_contains is title-only: only ub-titlematch (NOT the description carrier).
    let tc = list_ids(
        &session,
        &ListFilters {
            text_contains: Some("widget".to_string()),
            ..ListFilters::default()
        },
    )
    .await;
    assert_eq!(
        tc,
        HashSet::from(["ub-titlematch".to_string()]),
        "text_contains is title-only"
    );

    // search scans title + description: BOTH carriers match (pins text_contains ≠ search).
    let found: HashSet<String> = session
        .search("widget", &ListFilters::default())
        .await
        .expect("search")
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert!(
        found.contains("ub-titlematch") && found.contains("ub-descmatch"),
        "search scans title+description"
    );

    // ESCAPE guard: a literal "%" matches only the "50% off" title, not everything.
    let pct = list_ids(
        &session,
        &ListFilters {
            text_contains: Some("%".to_string()),
            ..ListFilters::default()
        },
    )
    .await;
    assert_eq!(
        pct,
        HashSet::from(["ub-pct".to_string()]),
        "literal % is ESCAPE-guarded (not a wildcard)"
    );
}

/// #9 (OQ-A / NFR-14 ordering half) — a repeated identical query is byte-identical in order. The
/// corpus seeds a same-priority + same-created_at pair so the final `id ASC` tiebreak is the ONLY
/// discriminator (else the determinism passes vacuously).
#[tokio::test]
async fn repeated_identical_query_is_byte_identical_order() {
    let session = session().await;
    // ub-tie-a and ub-tie-b: SAME priority AND SAME created_at → only `id ASC` separates them.
    session
        .create(&issue("ub-tie-a", Priority::MEDIUM, 1000))
        .await
        .expect("c");
    session
        .create(&issue("ub-tie-b", Priority::MEDIUM, 1000))
        .await
        .expect("c");
    session
        .create(&issue("ub-other", Priority::HIGH, 1001))
        .await
        .expect("c");
    // A blocked member for the blocked-determinism leg.
    session
        .create(&issue("ub-blocker", Priority::MEDIUM, 999))
        .await
        .expect("c");
    session
        .create(&issue("ub-blocked", Priority::MEDIUM, 1002))
        .await
        .expect("c");
    add_blocks(&session, "ub-blocked", "ub-blocker").await;

    let order = |v: Vec<Issue>| -> Vec<String> { v.into_iter().map(|i| i.id).collect() };

    let list1 = order(session.list(&ListFilters::default()).await.expect("list"));
    let list2 = order(session.list(&ListFilters::default()).await.expect("list"));
    assert_eq!(list1, list2, "list order is byte-identical across runs");
    // The tie pair is ordered by id ASC (the only discriminator) — non-vacuous determinism.
    let a = list1
        .iter()
        .position(|x| x == "ub-tie-a")
        .expect("a present");
    let b = list1
        .iter()
        .position(|x| x == "ub-tie-b")
        .expect("b present");
    assert!(
        a < b,
        "the same-priority same-created_at tie is broken by id ASC (ub-tie-a < ub-tie-b)"
    );

    let ready1 = order(session.ready(&ListFilters::default()).await.expect("ready"));
    let ready2 = order(session.ready(&ListFilters::default()).await.expect("ready"));
    assert_eq!(ready1, ready2, "ready order is byte-identical across runs");

    let blocked1 = order(
        session
            .blocked(&ListFilters::default())
            .await
            .expect("blocked"),
    );
    let blocked2 = order(
        session
            .blocked(&ListFilters::default())
            .await
            .expect("blocked"),
    );
    assert_eq!(
        blocked1, blocked2,
        "blocked order is byte-identical across runs"
    );
}
