---
name: 2026-09-21-pre-handshake-frame-gate
description: Gating the pre-handshake frame class in a transport decorator we own (decision D50, tracker ub-kp7) — one premature frame killed a server whose workspace was already open and healthy, both gates failed twice and closed by a Miguel-ruled escalation rather than a third round, a session that died with the volume full left the whole change as a single cumulative commit in a workflow worktree for six days, and a last should-fix round was folded back into the four atomic commits instead of riding as a fifth.
type: run
date: 2026-09-21
branch: ub-kp7-track
pr: '-'
issues: [ub-kp7]
---

# Run — the pre-handshake frame gate (D50)

## Context

Task `ub-kp7` is a bug in the v1.0.1 slot. rmcp 1.7.0 answers a pre-handshake `ping` and keeps
waiting, but every other frame it reads before accepting an `initialize` request returns
`ExpectedInitializeRequest` and kills `unblock mcp`. The workspace is already open and healthy at
that point, so one sloppy or hostile frame ended the session. Decision D50 closes that class. Three
shipped rows had each named it and left it open, which the D50 row in `docs/PRD.md` records as
D47 clause 8(ii), D48 clause 6(i) and D49 clause 6(i).

The work sits on `ub-kp7-pre-initialize-gate`, four commits on `main` at 11ea156. This Track commit
was written on `ub-kp7-track`, branched off that tip at dd14449, so the re-export and this report
reach `main` in the same pull request as the work. No pull request is open at the time of writing.

The lifecycle ran from 2026-09-11 to 2026-09-21, every phase in an isolated worktree. Understand
and Decide each ran three lenses with a coordinator. Spec/Plan drafted through an architect and a
project-manager, merged into one writer brief by the coordinator, and landed through a single
`rust-engineer`. The design Review gate ran four lenses in round 1 and three in round 2, failed
both, and closed under a Miguel-ruled escalation. Implement ran three writers and a coordinator.
The Verify gate ran four lenses in round 1 and three in round 2, failed both, and closed under the
same escalation shape. Track is this report, the tracker re-export and the commit that carries
them.

Neither gate closed on a full team round. Both closed with one repairer plus one independent
read-only recheck lens, which is weaker than a gate pass, and the issue comments record it as such.

## What & why

The change was written against these sections, read rather than restated here.

- `docs/PRD.md` §4 rows D47, D48 and D49, which each named this class and left it open; D38, which
  owns the end-to-end cell this change repoints; and D40, which decides what the class now exits
  with.
- `docs/plans/01-design-spine.md` §5.6 and §5b, where the pre-handshake frame class gained its
  normative sentences.
- `docs/plans/crates/unblock-mcp.md`, the `server.rs`, `wire.rs` and `error.rs` rows.
- `docs/plans/implementation-plan.md`, the T3.13 entry.
- `docs/PROCESS.md` §3 for the D-id and D-range rules, and §5 for escalation after two failed
  rounds.

Understand read rmcp 1.7.0 rather than trusting the issue text, and two of the issue's own claims
did not survive. The issue said any first frame other than `initialize` kills the server, and it
does not, because `ping` is answered and the loop continues. The issue said a parked server holds
the D31 write lock, and it does not, because `Session::acquire` takes the lock once per mutation
and the guard releases it. So the gate is justified by availability and by what the client
observes, never by contention relief, and no D50 prose repeats the lock framing.

Four ecosystem implementations were read for comparison. LSP 3.17 answers a premature request
`-32002`, drops premature notifications and survives. The TypeScript and C# SDKs survive too.
Killing the server is done only by rmcp and by the Python SDK's request arm.

Miguel resolved four forks at Decide. The gate covers all four JSON-RPC frame shapes rather than
requests alone. A premature request is answered `-32600` on its own id and dropped, while a
premature notification, response or error frame is dropped with no reply. The gate lives in a new
decorator between the version clamp and the duplicate-key scanner rather than inside either. The
latch opens on the server's own outbound `InitializeResult` passing through `send()`, which is the
edge the specification names.

## Outcome

### What landed

The branch carries four commits on `main` 11ea156, each atomic by file ownership.

- `docs(d50)` 03a6975 — 15 files, 207 insertions, 66 deletions. It lands the D50 row with
  reciprocal notes on the five amended rows, the spine paragraphs, the crate-plan rows, the T3.13
  entry, the roadmap and the rendered roadmap page. It also bumps the D-range from D1..D49 to
  D1..D50 at every live site, which reaches `CLAUDE.md`, the ci-cd document, the `xtask` tokenizer
  comment and the range knobs of six sibling check scripts.
- `feat(d50)` c143ab9 — 4 files, 866 insertions, 11 deletions. It adds
  `crates/unblock-mcp/src/pre_handshake.rs`, 737 lines holding `PreHandshakeGateTransport<T>`, a
  four-arm classifier with no wildcard, a send-side latch keyed on `ServerResult::InitializeResult`
  and the compile-time reply constant at line 116. `server.rs:468` now composes
  `VersionClampingTransport` over the gate over `DupScanningTransport`, so the receive order is
  scan, gate, clamp. The module carries eleven cells of its own, and `server.rs` adds one
  composition cell that drives a real duplex through `run_mcp_server_handler`.
- `test(d50)` d8e9d9d — 3 files, 470 insertions, 103 deletions. The id-less-notification cell
  inverts from exit 1 to exit 0 and is renamed, the unsignalled run-loop witness is repointed onto
  a broken pipe on the pre-handshake `ping` reply through a new `close_stdout` helper, and six
  stdio cells are added over the real binary.
- `ci(d50)` dd14449 — 4 files, 395 insertions, 23 deletions.
  `scripts/checks/d50-pre-handshake-gate-claims.sh` at 344 lines carries 7 required-landing rows
  and 44 row-anchored rows, and its `ci.yml` step is the eighth gate script the required `doc-lint`
  job runs.

The contract version did not move, no `ErrorCode` was minted, no snapshot moved and the layering is
unchanged.

### The gate record

- Design Review round 1 (2026-09-11) failed with 9 must-fixes, 12 should-fixes and 8 dismissals.
  All four lenses failed it independently. The finding that changed the design is that rmcp's
  `ClientRequest` is an untagged union, so the gate has to match by variant.
- Design Review round 2 (finished 2026-09-14 after a session-limit pause) failed on 2 must-fixes
  and 10 should-fixes. One must-fix is a sentence that predates the repair round and states the
  inverse of its own hazard. The other is a mutation example that no cell could kill.
- The escalation closed the gate after one repairer and two passes by a single recheck lens. The
  first recheck found one must-fix on an orchestrator-dictated sentence, that the measurement
  binary was built at the `v1.0.1-rc.3` tag, which that tag's own date falsifies.
- Verify round 1 (2026-09-14) failed with 7 must-fixes. Every one is prose or test scope and none
  is behaviour. The largest class is five branch-authored sentences claiming the `-32600` reply
  echoes no client bytes, when it echoes the frame's id verbatim and unbounded.
- Verify round 2 failed on 2 must-fixes, both sentences written by the repair between the two
  rounds, plus 8 should-fixes. One of the two called the d50 script's composition row the only
  catch for a gate deletion that keeps the type constructed elsewhere. A reviewer built that mutant
  and the stdio lifecycle suite went red on six cells, so the row is not the only catch.
- Verify closed by escalation on 2026-09-21. One read-only recheck lens re-derived every fact from
  the 11-file repair delta, confirmed both must-fixes and nine of the ten should-fixes, reproduced
  the demanded mutation proof, and left 6 should-fix prose items open.

### The last repair and the fold

Miguel asked for one short round before Track rather than shipping the six open sentences. Five
became edits in commit 930f584, which touches 4 files with 17 insertions and 14 deletions and is
prose only. The sixth needed no edit, because the row it asked about does not exist in the
pre-repair tree, and the repairer re-derived that reading itself rather than taking it on trust. A
narrow lens then built six mutants on private target directories, and each killed exactly the cell
its sentence names.

That repair was folded back into the four commits by rebuilding each from the repaired tree, so no
fifth commit rides. The branch tip `dd14449` and the repair commit `930f584` both point at tree
`53226c1`, which is how the fold was checked.

### The probe

The orchestrator probe on the folded branch left one step red. Green were `cargo fmt --check`,
three clippy invocations, the full workspace test run, `cargo insta test --check`, `doc-lint`,
`check-layering`, `knowledge-lint`, all eight gate scripts including the new d50 one, the
knowledge-layer invariants check and its fixture selftest. The red step is the run-report gate,
which blocks a substantive diff that adds no run-report, so it was red only until this Track
commit. An earlier probe run reddened once on the shared-cache in-memory open flake tracked as
`ub-q1u` and `ub-lp9.30`, and a full re-run of the suite was green at exit 0.

### This commit

This commit carries a fresh 66-row export of the tracker over `.unblock/issues.jsonl`, taken
through the `sync` tool after the Verify closure comment and copied byte for byte with a matching
md5, and this report indexed under the wiki index's Runs section. Four gates were run here and all
four are green — `cargo xtask knowledge-lint`, the knowledge-layer invariants script, the
run-report gate against `main`, and `cargo xtask doc-lint`.

## Gotchas

- The session that ran this task stopped on 2026-09-15 with the volume full, mid-recheck, and its
  scratchpad worktree was purged afterwards. The branch itself still stood at `main`, and the whole
  change survived only as one cumulative commit, 5189e1d, on a workflow worktree branch. The
  recheck lens's own worktree held that same tree uncommitted, which is what identified 5189e1d as
  the tree the escalated closure had been measuring.
- A `cargo test` name filter matches the whole test path as a substring. Filtering on
  `pre_handshake` in `unblock-mcp` reports 12 passed while the module holds 11 cells, because
  `error::tests::connection_closed_is_the_pre_handshake_disconnect_class_narrowly` carries the
  string in its name. One issue comment on this task records that total as "the pre_handshake cells
  at 12 passed".
- The Verify gate failed its second round on two sentences the previous repair had written. What
  converged both gates was a recheck lens that re-measured numbers against the tree instead of
  re-reading them.
- The Verify rerun reused the killed run's worktree paths, and one lens inherited a worktree where
  the dead run had left the composition line mutated with the gate deleted. It reported that the
  shipped binary has no gate. The coordinator dismissed it against the patch text and kept that
  lens's measurements as evidence for a different must-fix.
- One mutation survived the whole run, a behaviour-preserving wildcard over the three non-Request
  classifier arms that no cell can reach. Its only pin is the d50 script row anchored on the
  Response arm's own line, and the row says so.
- The repointed end-to-end witness hung once in twelve runs under the session's heaviest parallel
  load and never in 46 further runs during Verify. The recorded mechanism is macOS pipe creation
  without atomic close-on-exec, which lets a sibling spawn inherit the child's stdout read end.
  Miguel ruled that the process-wide spawn mutex rides a follow-up with its own issue.
- A frame spelled `"method":"initialize"` whose `params` do not type decodes as
  `ClientRequest::CustomRequest`, and rmcp keys on the variant at both handshake edges. A gate
  keyed on `method()` would pass that frame upward into the same death. Design Review round 1
  caught it, and the shipped gate matches `ClientRequest::InitializeRequest(_)` and
  `ClientRequest::PingRequest(_)` by variant.
- The knowledge lint's session-local-id scan matches only tokens beginning MF, CF, M, R, F or A
  (`xtask/src/knowledge_lint.rs:50`), so it found nothing in this run's issue comments. The
  session-local codes those comments carry are gate-script row names starting with P and Q, plus
  the conformance-defect ids CD-6 and CD-7. The glossary below defines them regardless.

## Glossary

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-----------------------------------------------|
| P9 | the row of the D48 gate script that pins the tracker id of the first residual that decision names, which is this task | `scripts/checks/d48-stdout-channel-claims.sh:131` |
| P10 | the row of the same script pinning the second residual, the unbounded `Debug` rendering that D49 later bounded | `scripts/checks/d48-stdout-channel-claims.sh:132` |
| Q41 | the D50 gate-script row asserting this gate is specified in its own paragraph of the ci-cd document rather than only named in passing | `scripts/checks/d50-pre-handshake-gate-claims.sh:268` |
| Q42 | the D50 gate-script row asserting the script actually runs as a step of the required `doc-lint` job, so an unwired script fails | `scripts/checks/d50-pre-handshake-gate-claims.sh:269` |
| Q44 | the D50 gate-script row asserting the D48 sibling's P9 reason string carries the correction D50 clause 14 mandates | `scripts/checks/d50-pre-handshake-gate-claims.sh:271` |
| CD-6 | the conformance-defect id for the version clamp's coupling to an undocumented rmcp internal, which carries a stated deletion endgame and is why the gate was not hosted there | `crates/unblock-mcp/src/server.rs:494` |
| CD-7 | the conformance-defect id for the wire module's fork of rmcp's private framing helpers, pinned by a differential corpus the gate had to leave byte-unchanged | `crates/unblock-mcp/src/wire.rs:87` |

## Links

- `ub-kp7` — this task; a first frame that was neither `initialize` nor `ping` killed the MCP
  server even with a healthy workspace open.
- `ub-og3` — the D48 channel move named this class as one of its four residuals and left it open.
- `ub-b1a` — the D49 render bound; this class was the live path into that message, and D50 makes
  the render defence-in-depth.
- `ub-nbz` — an out-of-band transport reply is lost when rmcp cancels the receive future under
  concurrent traffic. The gate writes its reply inside `receive()` and inherits that residual,
  which the harness measured at 0 of 40 losses in the pre-handshake regime.
- `ub-q1u` and `ub-lp9.30` — the shared-cache in-memory flake that reddens the required workspace
  test job for reasons unrelated to the change under test.
- `ub-fh5` — the `write_lock_two_process` non-vacuity control that fails under load, byte-unchanged
  on this branch.
- Pull request — none open at the time of writing.
- Key files touched —
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-mcp/src/pre_handshake.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-mcp/src/server.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-mcp/src/wire.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-cli/tests/mcp_lifecycle.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/scripts/checks/d50-pre-handshake-gate-claims.sh`,
  `/Users/ramosmig/Public/WS-Labs/unblock/docs/PRD.md` (the D50 row).
- Prior related run-reports — `runs/2026-09-11-startup-failure-render-bound.md`, the D49 render
  bound whose own report names this class as the path that reaches it, and
  `runs/2026-08-07-mcp-stdout-framing-channel.md`, the D48 channel move that left it open.
