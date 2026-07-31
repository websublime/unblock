//! `Session::create_issue(NewIssue { deps })` — the D44 one-transaction create with IMPLICIT edge
//! ownership (PRD §4 D44; spine §3.2.1 `create_issue` / §4.1 `Session::create_issue`).
//!
//! Runs over a real in-memory libsql `Session` (never a mock — FR-9 "identical behaviour through one
//! path"), except where a counting `Storage` decorator is the only way to observe a NEGATIVE.
//!
//! # The defect these cells exist to keep dead
//!
//! Pre-D44 the engine built the `Issue` with `..Issue::default()`, so `Issue.dependencies` was ALWAYS
//! empty; it then wrote each declared edge in its OWN independent `storage.add_dependency`
//! transaction, anchored on a client-supplied source the server never reconciled with the id it had
//! just minted. Live over the JSON-RPC wire that produced three outcomes: a committed edgeless orphan
//! behind a foreign-key error, a SILENT edge planted on an unrelated third party, and a new issue
//! that reported READY because its declared blocker never landed.
//!
//! # The standard every cell here is held to
//!
//! Each test names, in its own doc-comment, the concrete wrong implementation (the MUTANT) it kills.
//! A cell whose mutant cannot be named is decoration and does not belong here.

mod common;

use std::sync::Arc;

use common::race::RaceInjector;
use common::{dep, session, session_over};
use unblock_engine::{EngineError, NewDep, NewIssue, Session, SessionConfig};
use unblock_error::{CodedError, ErrorCode};
use unblock_model::{Dependency, DependencyType, Issue, ListFilters};
use unblock_storage::{LibsqlStorage, Storage};

// ------------------------------------------------------------------------------------------------
// Fixtures
// ------------------------------------------------------------------------------------------------

/// A minimal `NewIssue` carrying only a title.
fn record(title: &str) -> NewIssue {
    NewIssue {
        title: title.to_string(),
        ..NewIssue::default()
    }
}

/// A declared edge. Note the SHAPE itself: [`NewDep`] takes no source — that is the structural half
/// of D44, pinned by `new_dep_cannot_carry_an_edge_source` below.
fn edge(target: &str, dep_type: DependencyType) -> NewDep {
    NewDep {
        depends_on_id: target.to_string(),
        dep_type,
        metadata: None,
    }
}

/// A pre-built `Issue` with a caller-chosen id, for the id-preserving `Session::create` path.
fn carrier(id: &str, dependencies: Vec<Dependency>) -> Issue {
    Issue {
        id: id.to_string(),
        title: format!("carrier {id}"),
        dependencies,
        ..Issue::default()
    }
}

/// Every issue in the store (active + closed + deferred).
async fn count_all(session: &Session) -> usize {
    session
        .list(&ListFilters {
            include_closed: true,
            include_deferred: true,
            ..ListFilters::default()
        })
        .await
        .expect("list")
        .len()
}

/// The ids currently in the ready set.
async fn ready_ids(session: &Session) -> Vec<String> {
    session
        .ready(&ListFilters::default())
        .await
        .expect("ready")
        .into_iter()
        .map(|i| i.id)
        .collect()
}

/// A `Session` over a counting [`RaceInjector`] decorator, plus the decorator itself.
///
/// The out-of-band racer inside that decorator arms only on `create_issues` (bulk), which no cell in
/// this file calls — so here it is a pure counting delegate over real libsql storage.
async fn counting_session() -> (Session, Arc<RaceInjector>) {
    let inner = LibsqlStorage::open_in_memory().await.expect("open");
    inner.migrate().await.expect("migrate");
    let inner: Arc<dyn Storage> = Arc::new(inner);
    let counting = RaceInjector::new(inner, "ub-never-raced");
    let counting_dyn: Arc<dyn Storage> = counting.clone();
    let session = session_over(counting_dyn, SessionConfig::default()).await;
    (session, counting)
}

// ------------------------------------------------------------------------------------------------
// (1) THE CARRIER IS SOURCE-LESS — the structural half of D44
// ------------------------------------------------------------------------------------------------

/// `NewDep` has EXACTLY three fields and none of them is an edge source.
///
/// A compile-time assertion written as a runtime test: the destructuring pattern is exhaustive (no
/// `..` rest), so this file stops compiling the moment the struct gains or loses a field.
///
/// MUTANT KILLED: someone re-adds a source field to `NewDep` — `issue_id: Option<String>`, a
/// self-referential sentinel, anything — so that a client-chosen anchor can reach L5 again. D44
/// claims the misattachment class is UNREPRESENTABLE below layer 7, not merely unreached, and that
/// claim holds only while this type cannot spell a source. Adding a field compiles everywhere else
/// in the workspace (`NewDep` is built by name in two places, both of which would simply ignore it),
/// so nothing but an exhaustive pattern notices.
#[test]
fn new_dep_cannot_carry_an_edge_source() {
    let NewDep {
        depends_on_id,
        dep_type,
        metadata,
    } = edge("ub-target", DependencyType::Blocks);

    assert_eq!(depends_on_id, "ub-target");
    assert_eq!(dep_type, DependencyType::Blocks);
    assert!(metadata.is_none());
}

// ------------------------------------------------------------------------------------------------
// (2) CALL SHAPE — one storage create, zero follow-up edge writes
// ------------------------------------------------------------------------------------------------

/// A create declaring THREE edges makes EXACTLY ONE `Storage::create_issue` call and ZERO
/// `Storage::add_dependency` calls.
///
/// MUTANT KILLED — the follow-up-pass shape, in both of its forms:
///   (a) the pre-D44 engine, which left `Issue.dependencies` empty and wrote each declared edge in
///       its own independent `add_dependency` transaction: this counter would read 3, not 0;
///   (b) the half-repair that SEEDS the edges and ALSO keeps the per-edge pass: the counter reads 3
///       again, and each of those calls would additionally fail as a duplicate of the edge the
///       seeded insert had just committed — while the issue row stayed committed.
///
/// Form (b) reaches the same persisted graph as the correct implementation on a happy input, so no
/// assertion about the final state can separate them. Only the call count can, which is the whole
/// reason this cell exists.
#[tokio::test]
async fn a_create_with_deps_makes_one_storage_call_and_no_separate_edge_writes() {
    let (session, counting) = counting_session().await;

    for title in ["blocker one", "blocker two", "blocker three"] {
        session
            .create_issue(record(title))
            .await
            .expect("seed create");
    }
    let targets: Vec<String> = session
        .list(&ListFilters::default())
        .await
        .expect("list")
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert_eq!(targets.len(), 3, "three seeds");

    let creates_before = counting.single_calls();
    let dep_writes_before = counting.dep_calls();

    let created = session
        .create_issue(NewIssue {
            deps: targets
                .iter()
                .map(|t| edge(t, DependencyType::Blocks))
                .collect(),
            ..record("declares three edges")
        })
        .await
        .expect("create with three declared edges");

    assert_eq!(
        counting.single_calls() - creates_before,
        1,
        "the row and ALL THREE edges must reach storage in ONE `create_issue` call"
    );
    assert_eq!(
        counting.dep_calls() - dep_writes_before,
        0,
        "the post-insert per-edge pass is DELETED: `add_dependency` must not be reached at all from \
         the create path (it was called once per declared edge pre-D44, each in its own tx)"
    );
    assert_eq!(
        created.dependencies.len(),
        3,
        "and all three edges did land: {:?}",
        created.dependencies
    );
}

// ------------------------------------------------------------------------------------------------
// (3) ATOMICITY — a create whose declared edge cannot be satisfied persists NOTHING
// ------------------------------------------------------------------------------------------------

/// A create whose declared edges close a gating cycle persists ZERO rows: no issue, no edge — not
/// even the edges that precede the offending one.
///
/// The fixture is the MIXED-ORIENTATION cycle the spec calls out (spine §3.2.1 guard (b)): the new
/// issue N declares a `parent-child` edge to P — an IN-edge `P -> N` under the D4 reversal — AND a
/// `blocks` edge to X, an OUT-edge `N -> X`, while `X -> P` already exists. Neither declared element
/// closes a cycle on its own; together they close `N -> X -> P -> N`.
///
/// MUTANT KILLED (atomicity): the pre-D44 shape — an engine that commits the issue row first and
/// then writes each declared edge in its own follow-up transaction. Under it the row commits, the
/// `parent-child` edge commits, and only the SECOND edge is refused: the caller gets an error while
/// a half-built issue plus a prefix of its edges are already durable and already visible to the
/// ready query. The three unchanged-state assertions below are what that shape cannot satisfy.
///
/// MUTANT KILLED (guard ordering): the per-element pre-check the spec names NON-CONFORMING — a loop
/// that calls `would_cycle_in_tx` once per declared edge BEFORE the row and its edges are staged.
/// Each call reloads the graph from the transaction, so no element ever sees another: element one
/// finds no out-edge on P, element two finds no `P -> N`, and the create SUCCEEDS, persisting a
/// genuine gating cycle. Verified by building that mutant and watching this cell go red.
///
/// NOT killed by this cell, and said plainly rather than claimed: merely MOVING the shipped guard
/// ahead of the staging step leaves it green. The shipped guard loads the graph once and threads it
/// through the loop, so `would_cycle_in_graph` accumulates each prospective edge into that one map;
/// element two then sees element one even with nothing staged. What the staging step actually buys
/// is the PRECEDENCE — that a self edge reports `SelfDependency` rather than a self-loop cycle — and
/// `self_dependency_still_wins_over_duplicate_on_the_create_path` below is the cell that pins it
/// (it goes red under exactly that move).
#[tokio::test]
async fn a_create_that_closes_a_gating_cycle_persists_nothing() {
    let session = session().await;

    let parent = session.create_issue(record("P")).await.expect("P");
    let far = session.create_issue(record("X")).await.expect("X");

    // Pre-existing: X blocks-depends on P  =>  gating-graph edge `X -> P`.
    session
        .add_dep(&dep(&far.id, &parent.id, DependencyType::Blocks))
        .await
        .expect("seed X to P");

    let count_before = count_all(&session).await;
    let far_edges_before = session.list_dependencies(&far.id).await.expect("edges");

    let err = session
        .create_issue(NewIssue {
            deps: vec![
                edge(&parent.id, DependencyType::ParentChild),
                edge(&far.id, DependencyType::Blocks),
            ],
            ..record("N closes the cycle")
        })
        .await
        .expect_err("the two declared edges together close a gating cycle");

    assert_eq!(
        err.code(),
        ErrorCode::CycleDetected,
        "the create-specific gating guard must fire: {err:?}"
    );
    // FR-5 AC: the REAL ordered path, naming every node — never a synthetic placeholder.
    let rendered = format!("{err}");
    for node in [&parent.id, &far.id] {
        assert!(
            rendered.contains(node.as_str()),
            "the cycle path must name every node on the cycle ({node} missing): {rendered}"
        );
    }

    // ZERO rows: no orphan issue...
    assert_eq!(
        count_all(&session).await,
        count_before,
        "a rejected create must leave NO issue row behind — not even an edgeless one"
    );
    // ...and no prefix of edges anywhere in the graph.
    assert_eq!(
        session.list_dependencies(&far.id).await.expect("edges"),
        far_edges_before,
        "no declared edge may survive a rejected create"
    );
    assert!(
        session
            .list_dependencies(&parent.id)
            .await
            .expect("edges")
            .is_empty(),
        "the `parent-child` element must not have been written before the cycle was detected"
    );
}

// ------------------------------------------------------------------------------------------------
// (4) GUARD PARITY — the duplicate rejection still fires on the create path
// ------------------------------------------------------------------------------------------------

/// A create declaring the SAME target twice is rejected with `DuplicateDependency` and persists
/// nothing. The two elements carry DIFFERENT `dep_type`s on purpose: the key is the
/// `(source, target)` pair, type-INSENSITIVE, exactly as `add_dependency` keys it in SQL.
///
/// MUTANT KILLED (silent skip): routing create edges through the shared per-record insert body as
/// it stands. That body `continue`s past a repeated target, so the create would SUCCEED and persist
/// ONE edge where the client declared two — the guard that fires on the `dep` add action today
/// would have been silently removed by the very change that fixed atomicity, with a green suite.
///
/// MUTANT KILLED (key shape): a type-SENSITIVE duplicate key. `blocks` plus `waits-for` to one
/// target would then be accepted, diverging from `add_dependency`, whose SQL key names no type.
#[tokio::test]
async fn a_create_declaring_one_target_twice_is_rejected_and_persists_nothing() {
    let session = session().await;
    let target = session
        .create_issue(record("target"))
        .await
        .expect("target");
    let count_before = count_all(&session).await;

    let err = session
        .create_issue(NewIssue {
            deps: vec![
                edge(&target.id, DependencyType::Blocks),
                edge(&target.id, DependencyType::WaitsFor),
            ],
            ..record("declares the same target twice")
        })
        .await
        .expect_err("a repeated target is a duplicate edge");

    assert_eq!(
        err.code(),
        ErrorCode::DuplicateDependency,
        "the repeated target must be REJECTED, never silently skipped: {err:?}"
    );
    assert_eq!(
        count_all(&session).await,
        count_before,
        "a duplicate-rejected create persists ZERO rows"
    );
}

/// `SelfDependency` still precedes `DuplicateDependency` on the create path.
///
/// The payload is both self-referential AND duplicated, so only one of the two codes can be
/// reported — and the published precedence (`IdCollision`, `external_ref`, `SelfDependency`,
/// `DuplicateDependency`, `CycleDetected`) says which. It is driven through the id-preserving
/// `Session::create`, because naming the created issue own id is exactly what the minting path
/// deliberately makes impossible.
///
/// MUTANT KILLED (duplicate hoisted): a repair that moves the new duplicate scan ahead of the
/// shared body staging step. That scan reads no transaction state, so it runs happily first — and
/// this payload then reports `DUPLICATE_DEPENDENCY`, changing the error a client sees for an
/// unchanged input on a GA-frozen surface. Verified: built, cell went red.
///
/// MUTANT KILLED (cycle hoisted): moving the gating-cycle guard ahead of staging. The self edge is
/// gating, so against an unstaged graph it reads as a self-loop and reports `CYCLE_DETECTED` instead.
/// This is the cell that actually pins the post-staging placement, which is why the atomicity cell
/// above points here rather than claiming the ordering for itself. Verified: built, cell went red.
#[tokio::test]
async fn self_dependency_still_wins_over_duplicate_on_the_create_path() {
    let session = session().await;
    let self_edge = |dep_type| Dependency {
        issue_id: "ub-self-1".to_string(),
        depends_on_id: "ub-self-1".to_string(),
        dep_type,
        created_at: chrono::Utc::now(),
        created_by: Some("tester".to_string()),
        metadata: None,
        thread_id: None,
    };

    let err = session
        .create(&carrier(
            "ub-self-1",
            vec![
                self_edge(DependencyType::Blocks),
                self_edge(DependencyType::WaitsFor),
            ],
        ))
        .await
        .expect_err("a self edge is rejected");

    assert_eq!(
        err.code(),
        ErrorCode::SelfDependency,
        "`SelfDependency` precedes `DuplicateDependency` in the published order: {err:?}"
    );
    assert_eq!(count_all(&session).await, 0, "ZERO rows persist");
}

// ------------------------------------------------------------------------------------------------
// (5) THE HAPPY PATH — the edge round-trips anchored on the MINTED id, metadata survives
// ------------------------------------------------------------------------------------------------

/// The returned issue carries its declared edge, anchored on the id the SERVER minted, with the
/// declared `metadata` intact and the session actor stamped as `created_by`.
///
/// MUTANT KILLED (the headline defect): an engine that leaves `Issue.dependencies` empty
/// (`.take(0).collect()` at `crates/unblock-engine/src/session/write.rs:221`). The create then
/// succeeds and returns an edgeless issue — precisely the GA behaviour, and precisely what
/// `dependencies.len() == 1` refuses. Verified: applied, this cell went red.
///
/// MUTANT KILLED (metadata): a mapping that drops `NewDep.metadata` on the way to `Dependency`
/// (`metadata: None` at `crates/unblock-engine/src/session/write.rs:218`). That loss is DOUBLY
/// masked at L2 — a DEFAULT empty-object on write, a matching coercion on read — so nothing but an
/// explicit round-trip assertion can see it, which is how the same class survived to GA once
/// already. Verified: applied, this cell was the ONLY one of the nine here that went red.
///
/// NO MUTANT KILLED (anchoring) — and this file previously claimed otherwise. The assertion
/// `dependencies[0].issue_id == created.id` below CANNOT fail, for a structural reason: `created`
/// is a RE-READ, and the re-read hydrates edges with
/// `SELECT … FROM dependencies WHERE issue_id = ?1` bound to the issue's own id
/// (`crates/unblock-storage/src/libsql/crud.rs:408`), then reads the `issue_id` column back off
/// that very row. Every hydrated edge is therefore anchored on the issue that hydrated it BY
/// CONSTRUCTION, whatever L5 or L2 stamped. Proven, not argued: all three anchoring mutants at
/// `write.rs:213` (`String::new()`, `dep.depends_on_id.clone()`, a literal `"ub-not-me-1"`) leave
/// this cell GREEN — each killed exactly one test workspace-wide, and it was
/// [`the_engine_stamps_the_minted_id_and_the_session_actor_before_storage_sees_the_edges`], never
/// this one. What survives here is the `len() == 1` half: it is a genuine end-to-end pin that the
/// edge is REACHABLE under the minted id, which is why the assertion is kept — but it is not an
/// anchoring pin, and calling it "the whole D44 anchoring contract in one line" was false.
///
/// NO MUTANT KILLED (actor) — likewise previously claimed here. `created_by: None` at
/// `write.rs:217` leaves this cell GREEN, because L2 binds
/// `dep.created_by.as_deref().unwrap_or(actor)` (`crud.rs:252`) and so supplies the very actor the
/// engine omitted. Verified workspace-wide: that mutant killed exactly one test, the sibling cell
/// named above. The `created_by == "tester"` assertion below pins the END-TO-END attribution
/// (either layer may deliver it); the ENGINE's own stamp is pinned only by that sibling.
#[tokio::test]
async fn a_declared_edge_round_trips_anchored_on_the_minted_id() {
    let session = session().await;
    let blocker = session.create_issue(record("blocker")).await.expect("seed");

    let created = session
        .create_issue(NewIssue {
            deps: vec![NewDep {
                depends_on_id: blocker.id.clone(),
                dep_type: DependencyType::Blocks,
                metadata: Some("{\"why\":\"KEEP-ME\"}".to_string()),
            }],
            ..record("depends on the blocker")
        })
        .await
        .expect("create with one declared edge");

    assert_eq!(created.dependencies.len(), 1, "the declared edge landed");
    let landed = &created.dependencies[0];
    assert_eq!(
        landed.issue_id, created.id,
        "the edge came back REACHABLE under the minted id. NB this comparison is structurally \
         unfailable on a re-read (hydration filters by `issue_id`, crud.rs:408) — the ENGINE's \
         anchor is pinned by \
         `the_engine_stamps_the_minted_id_and_the_session_actor_before_storage_sees_the_edges`"
    );
    assert_eq!(landed.depends_on_id, blocker.id);
    assert_eq!(landed.dep_type, DependencyType::Blocks);
    assert_eq!(
        landed.metadata.as_deref(),
        Some("{\"why\":\"KEEP-ME\"}"),
        "`metadata` must round-trip on the create path too (D42 bound all seven columns; this path \
         only reaches that bind since D44)"
    );
    assert_eq!(
        landed.created_by.as_deref(),
        Some("tester"),
        "the SESSION actor is attributed END-TO-END (D44 moved the stamp from L7 to L5). NB L2's \
         `unwrap_or(actor)` fallback (crud.rs:252) also satisfies this, so it does NOT pin the \
         engine's own stamp — that is the sibling cell's job"
    );

    // And it is DURABLE, not merely present on the returned object.
    let reread = session
        .get(&created.id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(reread.dependencies, created.dependencies);
}

// ------------------------------------------------------------------------------------------------
// (5b) THE STAMP ITSELF — observed at the layer that is normatively required to make it
// ------------------------------------------------------------------------------------------------

/// The `Issue` the ENGINE hands to `Storage::create_issue` already carries every declared edge
/// anchored on the MINTED id and authored by the SESSION actor — before storage sees it.
///
/// # Why this cell is not redundant with the happy path above
///
/// Every other assertion in this file (and in `unblock-mcp/tests/dep_metadata.rs`) observes a
/// PERSISTED, re-read edge, and the persisted edge cannot see layer 5's stamp at all, because layer
/// 2 overwrites both fields on the way down:
///   * `crates/unblock-storage/src/libsql/crud.rs:248` binds `issue.id` into `dependencies.issue_id`
///     and never reads `dep.issue_id` — the engine's anchor is DISCARDED and silently replaced with
///     the correct one;
///   * `crud.rs:252` binds `dep.created_by.as_deref().unwrap_or(actor)` — an absent engine stamp
///     falls back to the very actor the engine would have written.
/// `Session::create_issue` then returns a re-read of that repaired row — and the re-read cannot
/// expose a bad anchor even in principle, because it hydrates edges with
/// `… FROM dependencies WHERE issue_id = ?1` bound to the issue's own id (`crud.rs:408`) and then
/// reads the `issue_id` column back off that row, so a hydrated edge is anchored on its own issue BY
/// CONSTRUCTION. So an engine that stamped an empty string, a foreign id, a literal wrong id, or no
/// author at all produces byte-identical observable output. The guarantee would still hold in
/// practice — but it would hold one layer BELOW where the spec places it, and the normative clause
/// itself would be untested. Capturing the argument is the only vantage point that sees layer 5.
///
/// Each mutant below was applied to a pristine tree and run against the WHOLE workspace
/// (`cargo test --workspace --all-targets --no-fail-fast`). Every one produced exactly the same
/// result: `1539 passed; 1 failed`, and the single failure was THIS cell. That is the evidence for
/// both halves of the claim — the mutant dies here, and it dies NOWHERE ELSE, which is precisely why
/// the happy-path cell above no longer claims to catch it.
///
/// MUTANT KILLED (empty anchor): `issue_id: String::new()` at
/// `crates/unblock-engine/src/session/write.rs:213` (the `unwrap_or_default` shape).
///
/// MUTANT KILLED (foreign anchor): `issue_id: dep.depends_on_id.clone()` at the same line — the edge
/// anchored on its own target, which is the misattachment class D44 exists to make unrepresentable.
///
/// MUTANT KILLED (literal wrong anchor): `issue_id: "ub-not-me-1".to_string()` at the same line.
///
/// MUTANT KILLED (unstamped author): `created_by: None` at `write.rs:217`.
///
/// NOT COVERED HERE, stated so the next reader does not assume it: the `create_bulk` path stamps the
/// same two fields at `write.rs:437`/`:441` (and `:453`/`:457` for resolved `dep_refs`). Those are a
/// DIFFERENT clause with their own cells; no mutant of them was run for this test, so nothing in this
/// comment should be read as covering them.
#[tokio::test]
async fn the_engine_stamps_the_minted_id_and_the_session_actor_before_storage_sees_the_edges() {
    let (session, counting) = counting_session().await;

    let mut targets = Vec::new();
    for title in ["blocker one", "blocker two"] {
        targets.push(session.create_issue(record(title)).await.expect("seed").id);
    }

    let created = session
        .create_issue(NewIssue {
            deps: vec![
                edge(&targets[0], DependencyType::Blocks),
                edge(&targets[1], DependencyType::WaitsFor),
            ],
            ..record("declares two edges")
        })
        .await
        .expect("create with two declared edges");

    let handed = counting
        .captured_creates()
        .pop()
        .expect("the create reached storage");

    // The chain the clause actually asserts: the id the CALLER is handed is the id on the row the
    // ENGINE built, and that same id is the anchor the ENGINE put on every declared edge.
    assert_eq!(
        handed.id, created.id,
        "the row handed to storage carries the id the caller is returned"
    );
    assert_eq!(
        handed.dependencies.len(),
        2,
        "both declared edges were SEEDED onto the built issue — this cell is vacuous without them: \
         {:?}",
        handed.dependencies
    );

    for seeded in &handed.dependencies {
        assert_eq!(
            seeded.issue_id, created.id,
            "the ENGINE anchors each declared edge on the MINTED id, before storage re-anchors it: \
             {seeded:?}"
        );
        assert_eq!(
            seeded.created_by.as_deref(),
            Some("tester"),
            "the ENGINE stamps the session actor as the edge author, before storage's `unwrap_or` \
             fallback can supply it: {seeded:?}"
        );
        // The remaining two fields of the same spine clause. These are NOT masked by layer 2 (it
        // binds them straight through), so they are pinned here only to cover the clause whole.
        assert_eq!(
            seeded.created_at, handed.created_at,
            "the edge carries the create's OWN `now`, not a second clock read: {seeded:?}"
        );
        assert!(
            seeded.thread_id.is_none(),
            "a create-declared edge belongs to no comment thread: {seeded:?}"
        );
    }

    // And the targets are the declared ones, in declaration order — so "anchored correctly" is not
    // being satisfied by some degenerate seeding that lost the payload.
    let landed_targets: Vec<&str> = handed
        .dependencies
        .iter()
        .map(|d| d.depends_on_id.as_str())
        .collect();
    assert_eq!(landed_targets, vec![&targets[0], &targets[1]]);
}

// ------------------------------------------------------------------------------------------------
// (6) THE READY-SET CONSEQUENCE
// ------------------------------------------------------------------------------------------------

/// An issue created with a `blocks` edge on an OPEN blocker is NOT in the ready set, and becomes
/// ready exactly when that blocker closes.
///
/// This is the acceptance criterion in its operational form. `query ready` is computed FROM the
/// dependency graph, so a dropped edge does not merely lose data — it hands a genuinely-blocked
/// issue to the next agent that asks for work. That is what happened at GA, with no race involved.
///
/// MUTANT KILLED: any implementation that drops a declared edge on the floor — the pre-D44 engine
/// (empty `Issue.dependencies`), an L7 adapter that maps `deps` to an empty vector, or a storage
/// body that skips the seeded list. Every one of them leaves the new issue edgeless and therefore
/// READY, which the first assertion refuses. The second assertion (ready AFTER the blocker closes)
/// is what makes the first non-vacuous: without it, an issue absent from the ready set for any
/// unrelated reason would pass.
#[tokio::test]
async fn a_create_with_a_declared_blocker_is_not_ready_until_the_blocker_closes() {
    let session = session().await;
    let blocker = session.create_issue(record("blocker")).await.expect("seed");

    let created = session
        .create_issue(NewIssue {
            deps: vec![edge(&blocker.id, DependencyType::Blocks)],
            ..record("blocked on creation")
        })
        .await
        .expect("create");

    let ready = ready_ids(&session).await;
    assert!(
        !ready.contains(&created.id),
        "an issue whose declared blocker is OPEN must NOT be offered as ready — a dropped edge is \
         exactly what made it ready at GA: ready={ready:?}"
    );
    assert!(
        ready.contains(&blocker.id),
        "the blocker itself IS ready: ready={ready:?}"
    );

    session
        .close_with_suggestions(&blocker.id, None)
        .await
        .expect("close the blocker");

    let ready_after = ready_ids(&session).await;
    assert!(
        ready_after.contains(&created.id),
        "closing the blocker unblocks it — proving the edge was REAL, not merely that the issue was \
         missing from the ready set for some unrelated reason: ready={ready_after:?}"
    );
}

// ------------------------------------------------------------------------------------------------
// (7) THE ID-PRESERVING SIBLING — `Session::create(&Issue)` shares the seam and the guards
// ------------------------------------------------------------------------------------------------

/// `Session::create(&Issue)` — the id-preserving path — carries the same create-specific gating
/// guard, because both engine callers share `Storage::create_issue` (spine §3.2.1, SCOPE clause).
///
/// MUTANT KILLED: placing the guards in `Session::create_issue` (the ENGINE method) rather than in
/// the storage `create_issue` wrapper. That placement satisfies every minting-path cell above while
/// leaving the id-preserving path free to commit a gating cycle — a real divergence between two
/// methods the spec binds to one seam, and one no minting-path test could ever notice.
#[tokio::test]
async fn the_id_preserving_create_carries_the_same_gating_guard() {
    let session = session().await;

    session
        .create(&carrier("ub-cyc-a", vec![]))
        .await
        .expect("a");
    // b blocks-depends on a, giving the gating edge `b -> a`.
    session
        .create(&carrier(
            "ub-cyc-b",
            vec![dep("ub-cyc-b", "ub-cyc-a", DependencyType::Blocks)],
        ))
        .await
        .expect("b");

    let count_before = count_all(&session).await;

    // c declares `c -> b` (blocks, forward) AND `a -> c` (parent-child, REVERSED under D4), which
    // closes `a -> c -> b -> a`.
    let err = session
        .create(&carrier(
            "ub-cyc-c",
            vec![
                dep("ub-cyc-c", "ub-cyc-b", DependencyType::Blocks),
                dep("ub-cyc-c", "ub-cyc-a", DependencyType::ParentChild),
            ],
        ))
        .await
        .expect_err("c closes the mixed-orientation cycle");

    assert_eq!(err.code(), ErrorCode::CycleDetected, "{err:?}");
    assert!(
        matches!(err, EngineError::Storage { .. }),
        "the guard is a STORAGE-layer property of the shared `create_issue` seam, so it surfaces as \
         the transparent storage source: {err:?}"
    );
    assert_eq!(
        count_all(&session).await,
        count_before,
        "ZERO rows persist on the id-preserving path too"
    );
}
