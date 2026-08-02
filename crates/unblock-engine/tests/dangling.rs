//! D45 (v1.0.1) — the `dangling` diagnostics kind and the `doctor` fold, over a REAL in-memory
//! libsql `Session` (no `Storage` mock — FR-9 "identical behaviour through one path").
//!
//! **Why this file exists as its own target, and why it is feature-gated.** The only seam that can
//! PLANT a dangling edge is `StorageTestkit::testkit_insert_raw_edge`, which is compiled under
//! `#[cfg(any(test, feature = "testkit"))]` in `unblock-storage` — the D45 write guard now refuses to
//! create such an edge through every public path, and `DeleteMode::Hard` explicitly cleans
//! `depends_on_id` references, so an already-corrupt workspace is otherwise unreachable. The findings,
//! however, are composed in the ENGINE, so the shipped storage-only testkit steps
//! (`cargo test -p unblock-storage --features testkit --test contract` / `--test contention_lab`)
//! can never reach them. **This cell would therefore execute in NO CI job — be green by
//! non-execution — unless a job is named, so one is:** the required `storage-testkit` job carries
//! `cargo test -p unblock-engine --features testkit --locked --test dangling`
//! (`ci-cd-and-distribution.md` §2.1, the D45 sub-check, which is normative over that wiring).
//!
//! Without the feature this file compiles to zero tests.

#![cfg(feature = "testkit")]
#![allow(clippy::doc_markdown)] // prose cell docs, not API docs.

mod common;

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use unblock_engine::{Session, SessionConfig};
use unblock_model::{Dependency, DependencyType, DiagnosticFinding, DiagnosticKind, Issue, Status};
use unblock_storage::{DeleteMode, DeletePlan, LibsqlStorage, Storage, StorageTestkit};

use common::session_over;

/// A `Session` over a real in-memory libsql store, PLUS the concrete handle the testkit seam needs
/// (the `Session` only exposes `Arc<dyn Storage>`, which cannot be downcast to `StorageTestkit`).
async fn session_and_store() -> (Session, Arc<LibsqlStorage>) {
    let storage = LibsqlStorage::open_in_memory()
        .await
        .expect("open in-memory");
    storage.migrate().await.expect("migrate");
    let storage = Arc::new(storage);
    let session = session_over(
        storage.clone() as Arc<dyn Storage>,
        SessionConfig::default(),
    )
    .await;
    (session, storage)
}

/// A minimal valid issue at a fixed instant.
fn issue(id: &str) -> Issue {
    let ts = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    Issue {
        id: id.to_string(),
        title: format!("issue {id}"),
        status: Status::Open,
        created_at: ts,
        updated_at: ts,
        ..Issue::default()
    }
}

/// Plant an edge `source -> target` DIRECTLY, bypassing every guard — the only way to reach the
/// already-corrupt state D45's read view exists to enumerate.
async fn plant_edge(store: &Arc<LibsqlStorage>, source: &str, target: &str, ty: DependencyType) {
    let dep = Dependency {
        issue_id: source.to_string(),
        depends_on_id: target.to_string(),
        dep_type: ty,
        created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        created_by: Some("tester".to_string()),
        metadata: None,
        thread_id: None,
    };
    store
        .testkit_insert_raw_edge(&dep)
        .await
        .expect("plant raw edge");
}

/// `(label, detail)` pairs, in emission order.
fn rows(findings: &[DiagnosticFinding]) -> Vec<(String, String)> {
    findings
        .iter()
        .map(|f| (f.label.clone(), f.detail.clone()))
        .collect()
}

/// The `dangling` action lists a planted dangling edge with the PINNED finding shape: `label` = the
/// DEPENDENT issue id, `detail` = `"<dep_type> -> <missing target id>"` (spine §3.2.1).
///
/// MUTANT KILLED: any other encoding of the three facts — dropping the edge TYPE from `detail` (it is
/// what distinguishes a permanently-stuck issue from a merely phantom parent), or swapping `label`
/// for the TARGET.
#[tokio::test]
async fn the_dangling_action_lists_a_planted_edge_with_the_pinned_shape() {
    let (s, store) = session_and_store().await;
    s.create(&issue("ub-1")).await.expect("create");
    plant_edge(&store, "ub-1", "ub-ghost", DependencyType::Blocks).await;

    let report = s
        .diagnostics(DiagnosticKind::Dangling, None)
        .await
        .expect("diagnostics");
    assert_eq!(
        report.kind,
        DiagnosticKind::Dangling,
        "the response DECLARES what it is — the reason the variant was minted rather than reusing Lint"
    );
    assert_eq!(
        rows(&report.findings),
        vec![("ub-1".to_string(), "blocks -> ub-ghost".to_string())]
    );
}

/// The findings are ordered by `(issue_id, dep_type, depends_on_id)` — a DELIBERATE re-sort in the
/// engine, NOT the `(from, to, dep_type)` order `dependency_graph` returns, so a dependent's broken
/// edges group by KIND (NFR-14 — this is snapshot-pinned output).
///
/// NON-VACUOUS BY CONSTRUCTION: the planted set is chosen so the two orders DISAGREE. Under
/// `(from, to, dep_type)` `ub-1` would emit `parent-child -> ub-aaa` before `blocks -> ub-zzz`
/// (`ub-aaa` < `ub-zzz`); under the pinned order `blocks` precedes `parent-child`.
///
/// MUTANT KILLED: forwarding `dependency_graph`'s own order, or sorting on the rendered `detail`
/// string instead of the triple.
#[tokio::test]
async fn the_dangling_findings_are_ordered_by_issue_then_type_then_target() {
    let (s, store) = session_and_store().await;
    s.create(&issue("ub-2")).await.expect("create");
    s.create(&issue("ub-1")).await.expect("create");
    plant_edge(&store, "ub-1", "ub-aaa", DependencyType::ParentChild).await;
    plant_edge(&store, "ub-1", "ub-zzz", DependencyType::Blocks).await;
    plant_edge(&store, "ub-2", "ub-mmm", DependencyType::WaitsFor).await;

    let report = s
        .diagnostics(DiagnosticKind::Dangling, None)
        .await
        .expect("diagnostics");
    assert_eq!(
        rows(&report.findings),
        vec![
            ("ub-1".to_string(), "blocks -> ub-zzz".to_string()),
            ("ub-1".to_string(), "parent-child -> ub-aaa".to_string()),
            ("ub-2".to_string(), "waits-for -> ub-mmm".to_string()),
        ]
    );
}

/// **THE TRAP, pinned normatively.** The existing-id set MUST come from FULLY-INCLUSIVE filters
/// (`include_closed` + `include_deferred` + `include_tombstone`, all true). With the DEFAULT filters
/// — which exclude closed and tombstone — every CLOSED blocker would be reported as dangling: a
/// diagnostic that fabricates its own findings.
///
/// MUTANT KILLED: `all_visibility_filters()` swapped for `ListFilters::default()` in
/// `dangling_findings`. All THREE legitimate targets below then surface as false findings.
#[tokio::test]
async fn a_closed_deferred_or_tombstoned_blocker_is_not_dangling() {
    let (s, _store) = session_and_store().await;
    s.create(&issue("ub-1")).await.expect("create");
    s.create(&issue("ub-closed")).await.expect("create");
    s.create(&issue("ub-deferred")).await.expect("create");
    s.create(&issue("ub-tombstoned")).await.expect("create");

    // Three legitimate blockers, each invisible to the DEFAULT filters.
    for target in ["ub-closed", "ub-deferred", "ub-tombstoned"] {
        s.add_dep(&Dependency {
            issue_id: "ub-1".to_string(),
            depends_on_id: target.to_string(),
            dep_type: DependencyType::Blocks,
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            created_by: Some("tester".to_string()),
            metadata: None,
            thread_id: None,
        })
        .await
        .expect("edge to a live row");
    }
    s.close_with_suggestions("ub-closed", None)
        .await
        .expect("close");
    s.defer(
        "ub-deferred",
        Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
    )
    .await
    .expect("defer");
    s.delete(&DeletePlan {
        mode: DeleteMode::Tombstone,
        targets: vec!["ub-tombstoned".to_string()],
        cascade_children: Vec::new(),
    })
    .await
    .expect("tombstone");

    let report = s
        .diagnostics(DiagnosticKind::Dangling, None)
        .await
        .expect("diagnostics");
    assert!(
        report.findings.is_empty(),
        "a closed / deferred / tombstoned blocker EXISTS — reporting it would fabricate findings: {:?}",
        report.findings
    );
}

/// An `external:` target is NEVER a finding, in EITHER spelling (spine §1.9: it names a blocker
/// outside this workspace, which is legitimate and which no row could ever satisfy).
///
/// MUTANT KILLED: dropping the `is_external_target` filter from the composition — both edges below
/// become false findings — and, separately, a case-SENSITIVE predicate, which lets the `EXTERNAL:`
/// spelling through while the ready/blocked SQL already treats it as external.
#[tokio::test]
async fn an_external_target_is_never_a_dangling_finding() {
    let (s, store) = session_and_store().await;
    s.create(&issue("ub-1")).await.expect("create");
    plant_edge(&store, "ub-1", "external:jira-1", DependencyType::Blocks).await;
    plant_edge(&store, "ub-1", "EXTERNAL:jira-2", DependencyType::Blocks).await;

    let report = s
        .diagnostics(DiagnosticKind::Dangling, None)
        .await
        .expect("diagnostics");
    assert!(
        report.findings.is_empty(),
        "external targets are legitimate blockers, not findings: {:?}",
        report.findings
    );
}

/// **The dangling corpus is DELIBERATELY WIDER than the EXPORT corpus, and the two must NEVER be
/// conflated.** An edge whose target is an EPHEMERAL / `-wisp-` row is NOT dangling — the row
/// exists. Reading the id set as "what `sync export` writes" reports every such edge as a false
/// finding: the same self-fabrication as the default-filters mutant, from the other side.
///
/// MUTANT KILLED: sourcing the id set from the export corpus (the D23 retain + the D45 blocker
/// closure) instead of every row in the database.
#[tokio::test]
async fn an_edge_to_an_ephemeral_or_wisp_row_is_not_dangling() {
    let (s, _store) = session_and_store().await;
    let ephemeral = Issue {
        ephemeral: true,
        ..issue("ub-eph")
    };
    s.create(&ephemeral).await.expect("create ephemeral");
    s.create(&issue("ub-wisp-x")).await.expect("create wisp");
    let mut dependent = issue("ub-1");
    dependent.dependencies = vec![
        Dependency {
            issue_id: "ub-1".to_string(),
            depends_on_id: "ub-eph".to_string(),
            dep_type: DependencyType::Blocks,
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            created_by: Some("tester".to_string()),
            metadata: None,
            thread_id: None,
        },
        Dependency {
            issue_id: "ub-1".to_string(),
            depends_on_id: "ub-wisp-x".to_string(),
            dep_type: DependencyType::Blocks,
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            created_by: Some("tester".to_string()),
            metadata: None,
            thread_id: None,
        },
    ];
    s.create(&dependent).await.expect("create dependent");

    let report = s
        .diagnostics(DiagnosticKind::Dangling, None)
        .await
        .expect("diagnostics");
    assert!(
        report.findings.is_empty(),
        "an ephemeral / -wisp- row EXISTS, so the edge is not dangling: {:?}",
        report.findings
    );
}

/// A clean workspace reports NOTHING — the diagnostic does not invent findings out of a healthy
/// graph. Without this cell, a composition that returned an empty list unconditionally would pass the
/// three negative cells above.
#[tokio::test]
async fn a_clean_workspace_reports_no_dangling_edges() {
    let (s, _store) = session_and_store().await;
    s.create(&issue("ub-1")).await.expect("create");
    s.create(&issue("ub-2")).await.expect("create");
    common::add_blocks(&s, "ub-1", "ub-2").await;

    let report = s
        .diagnostics(DiagnosticKind::Dangling, None)
        .await
        .expect("diagnostics");
    assert!(report.findings.is_empty(), "{:?}", report.findings);
}

/// **The `doctor` FOLD, and the ONE-HOME property.** `Session::doctor()` appends the SAME findings
/// the `dangling` action returns, in the same pinned order, AFTER the health/integrity/file-state
/// rows — while the report's `kind` stays `Info` (the fold moves no spine §1.10 byte).
///
/// The equality against `diagnostics(Dangling)` is what pins ONE HOME: a second implementation in
/// `lifecycle.rs` would have to reproduce the composition, the fully-inclusive filters, the external
/// carve-out AND the sort to keep this green.
///
/// MUTANT KILLED: deleting the fold (the tail rows vanish); folding the rows in BEFORE the file-state
/// anomalies (the suffix assertion goes red); or flipping the report `kind` to `Dangling`.
#[tokio::test]
async fn doctor_folds_in_the_same_dangling_findings_after_the_file_state_rows() {
    let (s, store) = session_and_store().await;
    s.create(&issue("ub-1")).await.expect("create");
    plant_edge(&store, "ub-1", "ub-aaa", DependencyType::ParentChild).await;
    plant_edge(&store, "ub-1", "ub-zzz", DependencyType::Blocks).await;

    let action = s
        .diagnostics(DiagnosticKind::Dangling, None)
        .await
        .expect("diagnostics");
    let doctor = s.doctor().await.expect("doctor");

    assert_eq!(
        doctor.kind,
        DiagnosticKind::Info,
        "the fold REUSES Info — the Dangling KIND exists for the diagnostics arm, not for doctor"
    );
    assert!(
        !action.findings.is_empty(),
        "non-vacuity: the planted edges must actually be found"
    );
    let folded = rows(&doctor.findings);
    let expected = rows(&action.findings);
    assert_eq!(
        folded[folded.len() - expected.len()..],
        expected[..],
        "doctor appends the SAME rows, in the SAME order, as a SUFFIX"
    );
    assert!(
        folded.len() > expected.len(),
        "the health/integrity rows still precede them"
    );
}

/// A `doctor` run on a clean workspace folds in NOTHING, so the fold cannot pad a healthy report.
#[tokio::test]
async fn doctor_folds_in_nothing_on_a_clean_workspace() {
    let (s, _store) = session_and_store().await;
    s.create(&issue("ub-1")).await.expect("create");

    let doctor = s.doctor().await.expect("doctor");
    assert!(
        !doctor.findings.iter().any(|f| f.detail.contains(" -> ")),
        "no dangling row on a clean workspace: {:?}",
        doctor.findings
    );
}
