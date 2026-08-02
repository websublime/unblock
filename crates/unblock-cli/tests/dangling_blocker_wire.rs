//! D45 - the DANGLING dependency-TARGET guard over the LIVE JSON-RPC stdio wire, against a real
//! `unblock mcp` CHILD PROCESS (PRD §4 D45; tracker `ub-lp9.25`).
//!
//! # Why this file exists at the wire level and not in-process
//!
//! The defect this closes was a call that returned `isError:false` while the store gained a blocker
//! that can never be created and therefore never closed. That is a claim about what a CLIENT
//! receives, so the regression is asserted where a client stands: a real server child, a real
//! workspace, real frames. An in-process test shares the harness assumptions of the code under test.
//!
//! It lives in `unblock-cli` because only this crate can spawn `unblock mcp`
//! (`CARGO_BIN_EXE_unblock`); an `unblock-engine` or `unblock-storage` test doing so would be a
//! back-edge the layering check (NFR-15) rejects. It reuses the `common::McpClient` spawn harness,
//! whose `initialize` handshake is the readiness barrier - the same shape `create_deps_wire.rs`
//! established for D44.
//!
//! # What is here, and what is deliberately elsewhere
//!
//! D45 closes FIVE edge-writing entry points. The create path is pinned by
//! `create_deps_wire.rs::a_create_whose_blocker_does_not_exist_is_refused_and_persists_nothing`
//! (the cell D44 wrote to anticipate this change, rewritten when it landed). The remaining four -
//! `dep {action:"add"}`, the `issue update {parent}` reparent, `issue create_bulk` and the D5
//! JSONL import leg - are here, one cell each, plus the `external:` carve-out that keeps all of them
//! from being a blanket refusal.
//!
//! **The codes are NOT uniform and these cells do not pretend they are.** `dep add`, the reparent
//! and the import leg return `ISSUE_NOT_FOUND` (the storage guard, riding the existing code - D45
//! mints none). `create_bulk` returns `VALIDATION_FAILED`, because its L5 resolver rejects an
//! unresolvable reference BEFORE storage is reached, and that is the shipped behaviour D45 KEEPS -
//! which makes that cell a REGRESSION cell, not a new-behaviour one.
//!
//! Unix-only: `unblock mcp` is a no-op EOF path on Windows (NFR-11), the `mcp_lifecycle.rs`
//! precedent.
//!
//! Every cell names the MUTANT it kills.
#![cfg(unix)]

mod common;

use common::{McpClient, Workspace, id_set, issue_id};
use serde_json::{Value, json};

/// Create a titled issue through the wire and return its minted id.
fn create(client: &mut McpClient, title: &str) -> String {
    let (is_error, created) =
        client.call_tool("issue", &json!({"action": "create", "title": title}));
    assert!(!is_error, "setup create must succeed: {created}");
    issue_id(&created)
}

/// How many issues exist right now.
fn count(client: &mut McpClient) -> usize {
    let (_, listed) = client.call_tool("query", &json!({"kind": "list"}));
    id_set(&listed).len()
}

/// Every edge in the store, as the whole-graph read returns them.
fn edges(client: &mut McpClient) -> Vec<Value> {
    let (is_error, graph) = client.call_tool("dep", &json!({"action": "graph"}));
    assert!(!is_error, "the graph read must succeed: {graph}");
    graph["edges"].as_array().cloned().unwrap_or_default()
}

/// Shut the child down over EOF and assert the clean exit, so a cell cannot pass while leaving a
/// wedged server behind.
fn close_clean(client: &mut McpClient) {
    client.close_stdin();
    let status = common::wait_for(&mut client.child, std::time::Duration::from_secs(20));
    assert_eq!(status.code(), Some(0), "clean EOF exit 0");
}

/// `dep {action:"add"}` naming a TARGET that does not exist is refused with `ISSUE_NOT_FOUND`,
/// naming both endpoints, and writes no edge.
///
/// This is the path an agent uses most, and at GA it returned `isError:false`: the edge landed, the
/// source issue left the ready set, and no id existed that could ever close it.
///
/// MUTANT KILLED: deleting the target-existence probe from `add_dependency`
/// (`crates/unblock-storage/src/libsql/deps.rs`). The call succeeds, `is_error` goes red and the
/// edge count goes red with it.
#[test]
fn a_dep_add_whose_target_does_not_exist_is_refused_and_writes_no_edge() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());
    client.initialize();

    let source = create(&mut client, "the dependent");

    let (is_error, payload) = client.call_tool(
        "dep",
        &json!({
            "action": "add",
            "issue_id": source,
            "depends_on_id": "ub-no-such-blocker",
            "dep_type": "blocks"
        }),
    );

    assert!(
        is_error,
        "a phantom TARGET must be refused (D45): {payload}"
    );
    assert_eq!(payload["code"], "ISSUE_NOT_FOUND", "{payload}");
    assert_eq!(payload["context"]["issue_id"], source, "{payload}");
    assert_eq!(
        payload["context"]["blocker_id"], "ub-no-such-blocker",
        "the refusal names the missing target: {payload}"
    );

    assert!(
        edges(&mut client).is_empty(),
        "no edge may survive the refusal"
    );
    // ...and the dependent is still offered as ready, which is the operational consequence: at GA
    // it silently dropped out of the ready set behind a blocker nobody could ever close.
    let (_, ready) = client.call_tool("query", &json!({"kind": "ready"}));
    assert!(
        id_set(&ready).contains(&source),
        "the dependent must not drop out of the ready set behind a phantom blocker: {ready}"
    );

    close_clean(&mut client);
}

/// `dep {action:"add"}` naming a SOURCE that does not exist returns the SAME code as a phantom
/// target - `ISSUE_NOT_FOUND` (exit 3), not the opaque `DATABASE_ERROR` (exit 2) the source-column
/// foreign key produced at GA.
///
/// D45 clause (11): shipping the asymmetry would mean ONE call with ONE typo'd id returns two
/// different codes depending on WHICH FIELD carries the typo, and the source's one looks
/// unretryable by accident rather than by design.
///
/// MUTANT KILLED: deleting the source probe from `add_dependency`. The insert then trips the
/// source-column foreign key and the code assertion goes red with `DATABASE_ERROR`.
#[test]
fn a_dep_add_whose_source_does_not_exist_is_issue_not_found_not_a_database_error() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());
    client.initialize();

    let target = create(&mut client, "a real blocker");

    let (is_error, payload) = client.call_tool(
        "dep",
        &json!({
            "action": "add",
            "issue_id": "ub-no-such-source",
            "depends_on_id": target,
            "dep_type": "blocks"
        }),
    );

    assert!(is_error, "a phantom SOURCE must be refused: {payload}");
    assert_eq!(
        payload["code"], "ISSUE_NOT_FOUND",
        "the SOURCE half returns the SAME code as the TARGET half - a single typo must not pick \
         its error class from the field it landed in (D45 clause 11): {payload}"
    );
    assert_eq!(
        payload["context"]["id"], "ub-no-such-source",
        "the missing thing genuinely IS the addressed issue, so the existing `id` key stays \
         honest: {payload}"
    );

    assert!(edges(&mut client).is_empty(), "no edge may be written");

    close_clean(&mut client);
}

/// The precedence chain is ASSERTED, not assumed: a `dep add` naming the SAME non-existent id in
/// BOTH fields returns `SELF_DEPENDENCY`, because the self check answers without a transaction and
/// runs before the two existence probes.
///
/// This pair is trivially constructible and wire-observable, which is exactly why the published rank
/// has to be the rank the code executes.
///
/// MUTANT KILLED: relocating either existence probe ahead of the self check - the code assertion
/// then reads `ISSUE_NOT_FOUND` and this cell goes red.
#[test]
fn the_self_check_still_wins_over_the_existence_probes() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());
    client.initialize();

    let (is_error, payload) = client.call_tool(
        "dep",
        &json!({
            "action": "add",
            "issue_id": "ub-ghost",
            "depends_on_id": "ub-ghost",
            "dep_type": "blocks"
        }),
    );

    assert!(is_error, "a self edge is refused: {payload}");
    assert_eq!(
        payload["code"], "SELF_DEPENDENCY",
        "the published chain is SelfDependency -> source existence -> missing target -> \
         DuplicateDependency -> CycleDetected: {payload}"
    );

    close_clean(&mut client);
}

/// `issue update {parent}` - the REPARENT, the 4th edge-writing entry point and one the earlier
/// three-path framing never named - is refused when the parent does not exist, and the issue keeps
/// the parent it had.
///
/// Honest scoping, stated so this cell is not oversold: a dangling `parent-child` edge does NOT
/// produce the never-ready symptom. It is nevertheless a real integrity defect - written with
/// `isError:false`, hydrated onto the issue, exported, and LISTED by the `dangling` diagnostic - and
/// leaving it open would let the tool that REPORTS the defect create the defects it reports.
///
/// MUTANT KILLED: deleting the target-existence guard from `apply_reparent`
/// (`crates/unblock-storage/src/libsql/crud.rs`). The update succeeds and both the refusal and the
/// unchanged-edge assertions go red.
#[test]
fn a_reparent_onto_a_parent_that_does_not_exist_is_refused() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());
    client.initialize();

    let child = create(&mut client, "the child");
    let real_parent = create(&mut client, "the real parent");
    let (is_error, updated) = client.call_tool(
        "issue",
        &json!({"action": "update", "ids": [child], "parent": real_parent}),
    );
    assert!(!is_error, "setup reparent must succeed: {updated}");
    let edges_before = edges(&mut client);

    let (is_error, payload) = client.call_tool(
        "issue",
        &json!({"action": "update", "ids": [child], "parent": "ub-no-such-parent"}),
    );

    assert!(
        is_error,
        "a parent that names no row and is not an `external:` target must be REFUSED: {payload}"
    );
    assert_eq!(payload["code"], "ISSUE_NOT_FOUND", "{payload}");
    assert_eq!(
        payload["context"]["blocker_id"], "ub-no-such-parent",
        "{payload}"
    );

    assert_eq!(
        edges(&mut client),
        edges_before,
        "the reparent rolled back whole - the child keeps the parent it had, and no phantom \
         parent-child edge was written"
    );

    close_clean(&mut client);
}

/// `issue create_bulk` naming an unknown dependency reference is refused WHOLE-BATCH with
/// `VALIDATION_FAILED`, and nothing is minted.
///
/// **This is a REGRESSION cell, not a new-behaviour cell, and saying so is the point.** This path
/// already refused an unknown reference before D45, from the L5 resolver (batch set ∪ storage) -
/// which is exactly the batch-aware predicate D45 generalises to every other path. So `create_bulk`
/// is D45's TEMPLATE, not a hole, and its user-visible code STAYS `VALIDATION_FAILED`: publishing
/// `ISSUE_NOT_FOUND` here would name a code this path cannot return, because the resolver runs
/// first. What D45 changed underneath is that the storage guard now closes the race between that
/// pre-transaction probe and the commit - unobservable from a single client, which is why it is not
/// asserted here.
///
/// MUTANT KILLED: removing the resolver's no-batch-no-storage-match rejection
/// (`crates/unblock-engine/src/session/bulk.rs`). The batch is then accepted and both the refusal
/// and the zero-mint assertions go red.
#[test]
fn a_create_bulk_naming_an_unknown_dependency_is_refused_whole_batch() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());
    client.initialize();

    let count_before = count(&mut client);

    let markdown = "## first\n\n## second\n\n### Dependencies\n\n- blocks: ub-no-such-issue\n";
    let (is_error, payload) = client.call_tool(
        "issue",
        &json!({"action": "create_bulk", "markdown": markdown}),
    );

    assert!(
        is_error,
        "an unresolvable dependency reference is refused WHOLE-BATCH: {payload}"
    );
    assert_eq!(
        payload["code"], "VALIDATION_FAILED",
        "the L5 resolver rejects before storage is reached - this path's code is UNCHANGED by \
         D45: {payload}"
    );

    assert_eq!(
        count(&mut client),
        count_before,
        "whole-batch means whole-batch: the sibling record that was perfectly valid is not \
         minted either"
    );

    close_clean(&mut client);
}

/// The D5 JSONL import leg refuses a file whose record declares a target present neither in the
/// file's own batch nor in the destination database - whole-batch, with `ISSUE_NOT_FOUND`, naming
/// the offending pair, with ZERO rows written.
///
/// **The exporter may WIDEN its corpus; the importer may never INVENT one.** Repairing such a file
/// on ingest would put a silent edge-dropper on a write path, which is the same silence D45 exists
/// to close. The cost is accepted openly - a foreign file can now fail on data the user cannot edit
/// inside unblock - which is exactly why the message must carry both ids.
///
/// The file is written BY HAND rather than exported, because the exporter (correctly) cannot produce
/// a dangling edge from a healthy workspace: this models the foreign `bd`/JSONL file the clause is
/// about.
///
/// MUTANT KILLED: the guard living in the `create_issue` WRAPPER instead of the SHARED per-record
/// insert body. The import leg calls `create_issues`, so a wrapper-only guard leaves this path open
/// and the refusal assertion goes red - this cell is what makes the "one home" placement observable
/// from outside.
#[test]
fn an_import_file_carrying_a_dangling_edge_is_refused_whole_batch() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());
    client.initialize();

    // Inside `.unblock/`: the sync paths are CONFINED to the workspace directory (NFR-18), so a file
    // dropped at the workspace ROOT is rejected with `PATH_TRAVERSAL` before the import ever runs -
    // which would make this cell pass for the wrong reason.
    let path = ws.unblock_dir().join("foreign.jsonl");
    let file = format!(
        "{}\n{}\n",
        json!({
            "id": "ub-import-1",
            "title": "a perfectly valid record",
            "status": "open",
            "issue_type": "task",
            "priority": 2,
            "created_at": "2026-08-01T00:00:00Z",
            "updated_at": "2026-08-01T00:00:00Z"
        }),
        json!({
            "id": "ub-import-2",
            "title": "declares a blocker no one can ever create",
            "status": "open",
            "issue_type": "task",
            "priority": 2,
            "created_at": "2026-08-01T00:00:00Z",
            "updated_at": "2026-08-01T00:00:00Z",
            "dependencies": [{
                "issue_id": "ub-import-2",
                "depends_on_id": "ub-import-ghost",
                "type": "blocks",
                "created_at": "2026-08-01T00:00:00Z",
                "created_by": "someone-else"
            }]
        })
    );
    std::fs::write(&path, file).expect("write the foreign import file");

    let count_before = count(&mut client);
    let (is_error, payload) = client.call_tool(
        "sync",
        &json!({"action": "import", "path": path.to_string_lossy()}),
    );

    assert!(
        is_error,
        "a file carrying a dangling edge is REFUSED, never repaired: {payload}"
    );
    assert_eq!(payload["code"], "ISSUE_NOT_FOUND", "{payload}");
    // The pair is asserted on the MESSAGE, not on `context`, and that is deliberate rather than a
    // weakening: on this leg the storage error travels through `SyncError::Storage`, whose
    // `CodedError::context()` is documented to stay empty ("sync owns a distinct, coarser
    // boundary" - a decision that predates D45 and is not this change's to reverse). The CODE
    // forwards, and `BlockerNotFound`'s `Display` carries both ids - which is exactly the promise
    // D45 makes for a foreign file: there is no `--repair` escape in this cut, so the source file
    // must be fixable FROM THE MESSAGE ALONE.
    let message = payload["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("ub-import-2"),
        "the refusal names the RECORD that declared the edge - on a 500-record file the target \
         alone does not say which one: {payload}"
    );
    assert!(
        message.contains("ub-import-ghost"),
        "...and the missing target: {payload}"
    );

    assert_eq!(
        count(&mut client),
        count_before,
        "ZERO rows written - the valid sibling record is not imported either"
    );

    close_clean(&mut client);
}

/// **The LISTING VIEW, at the wire: `diagnostics {kind:"dangling"}` returns the planted edge.**
///
/// This is the half of D45 that exists FOR AGENTS. `doctor` is a CLI command they cannot reach, so a
/// finding placed only there would be invisible to the primary consumers - which is why the
/// agent-reachable action is the requirement and the `doctor` fold is its companion, not the other
/// way round. The report DECLARES its own kind (`dangling`), which is the whole reason the model
/// grew a variant instead of reusing `lint`: a response whose declared kind misdescribes its rows is
/// a lie bought for nothing.
///
/// The edge is planted with a direct row insert, because since D45 no supported path writes one -
/// that is the point of the change. The server child is stopped first and a fresh one is spawned
/// after, so the planting connection never contends with the server's.
///
/// MUTANT KILLED: mapping the wire `dangling` arm to any other `DiagnosticKind` (the declared kind
/// assertion goes red, and with a kind that computes something else the finding vanishes too).
///
/// MUTANT KILLED: deleting the engine-side composition the arm dispatches to - the findings array
/// comes back empty.
#[test]
fn the_dangling_action_lists_a_planted_edge_at_the_wire() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());
    client.initialize();
    let dependent = create(&mut client, "carries an edge to nowhere");
    close_clean(&mut client);

    plant_dangling_edge(&ws, &dependent, "ub-ghost", "blocks");

    let mut client = McpClient::spawn(ws.root());
    client.initialize();
    let (is_error, report) = client.call_tool("diagnostics", &json!({"kind": "dangling"}));
    assert!(!is_error, "the dangling action must succeed: {report}");
    assert_eq!(
        report["kind"], "dangling",
        "the response DECLARES the kind it carries: {report}"
    );
    let findings = report["findings"].as_array().expect("findings array");
    assert_eq!(findings.len(), 1, "exactly the planted edge: {report}");
    assert_eq!(findings[0]["label"], dependent, "{report}");
    assert_eq!(findings[0]["detail"], "blocks -> ub-ghost", "{report}");

    close_clean(&mut client);
}

/// Insert a dependency row DIRECTLY into the workspace DB, bypassing every guard. The raw-libsql
/// precedent in this crate is `migrate_doctor.rs::stamp_user_version`; the connection is dropped
/// before the next server child opens the file, so there is no writer contention.
fn plant_dangling_edge(ws: &Workspace, source: &str, target: &str, dep_type: &str) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build a current-thread runtime");
    rt.block_on(async {
        let database = libsql::Builder::new_local(ws.db_path())
            .build()
            .await
            .expect("open the workspace db");
        let conn = database.connect().expect("connect");
        conn.execute(
            "INSERT INTO dependencies (issue_id, depends_on_id, type, created_at, created_by) \
             VALUES (?1, ?2, ?3, '2026-08-01T00:00:00Z', 'planted')",
            libsql::params![source, target, dep_type],
        )
        .await
        .expect("plant the dangling edge");
    });
}

/// An `external:` target is ACCEPTED on the wire, in BOTH spellings, on FOUR of the five guarded
/// paths - `dep add`, the create, the reparent and `create_bulk`. An external blocker is a
/// legitimate fact no row could ever satisfy, which is exactly why the column carries no foreign key
/// in the first place and why the repair had to be application-level.
///
/// (The fifth path, the JSONL/`bd` import leg, is covered at the storage level by the NFR-16
/// contract suite, which drives `create_issues` - the body that leg calls - directly. Stated rather
/// than left as an unremarked gap.)
///
/// The case rule is FORCED, not chosen: the ready/blocked SQL already matches `LIKE 'external:%'`,
/// which `SQLite` evaluates ASCII-case-insensitively, so a case-SENSITIVE write guard would be
/// STRICTER than the read side and would refuse writes the store is happy to serve.
///
/// The `create_bulk` leg is a RELAXATION, not a preserved behaviour: before D45 that path refused a
/// correctly-spelled external reference whole-batch, and no test covered it - so nothing in CI would
/// have gone red to announce the change. This leg is that announcement at the wire.
///
/// MUTANT KILLED: a guard with no `external:` carve-out at all (every call below is refused), and -
/// separately - a case-SENSITIVE `starts_with` in `unblock_model::is_external_target`, under which
/// the `EXTERNAL:` legs are refused while the lowercase ones pass.
#[test]
fn an_external_target_is_accepted_in_both_spellings_at_the_wire() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());
    client.initialize();

    let a = create(&mut client, "depends on a lowercase external ref");
    let b = create(&mut client, "depends on an uppercase external ref");
    let c = create(&mut client, "parented to an external ref");

    for (issue, target) in [(&a, "external:jira-1"), (&b, "EXTERNAL:jira-2")] {
        let (is_error, payload) = client.call_tool(
            "dep",
            &json!({
                "action": "add",
                "issue_id": issue,
                "depends_on_id": target,
                "dep_type": "blocks"
            }),
        );
        assert!(
            !is_error,
            "`{target}` is an EXTERNAL blocker and must be accepted: {payload}"
        );
    }

    // An `external:` PARENT stays legal too: ONE shared predicate, no per-edge-type special-casing
    // (a carve-out that applied to some edge types and not others would recreate the two-dialect
    // split D45 abolishes).
    let (is_error, payload) = client.call_tool(
        "issue",
        &json!({"action": "update", "ids": [c], "parent": "EXTERNAL:epic-9"}),
    );
    assert!(
        !is_error,
        "an `external:` PARENT is legal under the ONE shared predicate: {payload}"
    );

    // The CREATE path - the one whose guard lives in the shared per-record insert body.
    let (is_error, payload) = client.call_tool(
        "issue",
        &json!({
            "action": "create",
            "title": "created with an external blocker",
            "deps": [{"depends_on_id": "EXTERNAL:jira-3", "dep_type": "blocks"}]
        }),
    );
    assert!(
        !is_error,
        "a create declaring an `external:` blocker is accepted: {payload}"
    );

    // And `create_bulk`, where this is a stated RELAXATION of a pre-D45 whole-batch refusal.
    let markdown =
        "## bulk with an external blocker\n\n### Dependencies\n\n- blocks: EXTERNAL:jira-4\n";
    let (is_error, payload) = client.call_tool(
        "issue",
        &json!({"action": "create_bulk", "markdown": markdown}),
    );
    assert!(
        !is_error,
        "`create_bulk` no longer refuses a correctly-spelled external reference: {payload}"
    );

    let written = edges(&mut client);
    assert_eq!(
        written.len(),
        5,
        "all five external edges were written, across four distinct guarded paths: {written:?}"
    );

    close_clean(&mut client);
}
