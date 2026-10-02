---
name: 2026-10-02-parse-error-recovered-id
description: Answering the -32700 parse error on the id recovered from a readable line (decision D54, tracker ub-788) — the residual D47 disclosed turned out to be a whole class of valid-JSON, non-Request lines with a readable id, of which duplicated method or jsonrpc were only two shapes; the fix keeps rmcp's code and bytes and inserts only the id, a byte-identity tier proves it against simulated rmcp bumps with hand-written literals as the absolute oracle, both gates passed with must-fixes that were all text, and two commit messages were rewritten for stating the predicate without its strict leg.
type: run
date: 2026-10-02
branch: ub788-parse-error-recovered-id
pr: '-'
issues: [ub-788, ub-a3w, ub-f1k]
---

# Run — the -32700 parse error answered on the recovered id (D54)

## Context

Task `ub-788` is the residual D47 disclosed when it was scoped to un-decodable ids: a frame whose
typed parse fails (a duplicated `method` or `jsonrpc` was the recorded shape) is answered with the
out-of-band `-32700` parse error with the id omitted, which an rmcp client drops, so its pending
request hangs. Two new issues, `ub-a3w` and `ub-f1k`, were filed at the Verify gate for
pre-existing items this run did not fix.

The lifecycle ran 2026-10-01 to 2026-10-02, each phase in an isolated clone or worktree.
Understand reproduced the defect on raw bytes against the `main` binary. Decide was Miguel ruling
the scope and error-code forks. The orchestrator then called a new decision id, D54, open to
countermand. Spec/Plan ran three planners (code design, spec text and cascade, verification plan)
and an integrating coordinator. The design Review gate ran three lenses (ReviewBypass,
ReviewContract, ReviewVerification) and a coordinator, one round. Implement was one implementer in
an isolated worktree. The Verify gate ran four lenses (VerifyWire, VerifyCode, VerifyMutation,
VerifyGates) and a coordinator, then a text-only fix pass and a delta check. Track is this report
and the tracker re-export.

The branch is `ub788-parse-error-recovered-id`, four work commits on `main` 835d30c plus the Track
commit. No pull request is open at the time of writing.

## What & why

The change was written against these sections, read rather than restated here.

- `docs/PRD.md` §4 rows D47 (the residual it named, clause 8(i), and clause (10), the CD-7 tiers),
  D43, D50 (the pre-handshake gate) and D53, and NFR-18.
- `docs/plans/crates/unblock-mcp.md`, the `wire.rs` and `envelope_id.rs` rows.
- `docs/plans/implementation-plan.md`, task T3.16.
- `docs/PROCESS.md` §3 for the D-id and D-range rules.
- JSON-RPC 2.0 §5 and §5.1, MCP 2025-11-25 Error Responses and MCP 2025-06-18 Responses, quoted in
  the Understand comment.

Understand found the class is wider than the issue said. Every valid-JSON line that is not a valid
Request object but carries a readable id is answered id-less: duplicated `method` or `jsonrpc`,
`jsonrpc` missing, numeric or `"1.0"`, `method` missing or numeric, scalar `params` on
`tools/call`, a duplicated `params`, a duplicated equal id, an id written last. Duplication was
again the minority route, as in the D47 run. The cited specs all agree for a readable id; they
differ only for an unreadable one, which D47 already settled as "omitted". A duplicated-method
`tools/call` carrying a create payload created nothing, so the defect is availability only.

Miguel's rulings at Decide: the whole readable-id class, not the two named shapes; keep `-32700`
and add only the id, so the sole byte that differs from rmcp is the `id` member; and reconcile the
byte-identity contract structurally rather than re-blessing it. The orchestrator called D54 as a
new row that closes D47's residual, following the D49 and D50 precedent, which brings the D1..D54
range bump and a new newest claims script.

Spec/Plan settled the strict-JSON leg. `envelope_id::scan` deliberately skips `end()`, and
`end()` or an `IgnoredAny` pass is not strict enough: measured on 21 lines, both still recover an
id past a non-UTF-8 byte or lone surrogate in a skipped string, 128 or more nesting levels, or
`1e400`. The design reuses the `Value` parse rmcp's compatibility filter already runs on every
failed line, so the check costs nothing extra. Miguel ruled one new finding at this phase: a
response-shaped client line with a readable id is answered on that id, disclosed rather than
excluded, since unblock sends the client no requests.

## Outcome

### What landed

Four commits on `main` 835d30c, each atomic by file ownership.

- `docs(spec)` 80408af — the D54 row with reciprocal notes on D47 and its siblings, NFR-18, the
  spine, the crate plan, both roadmaps, task T3.16, ci-cd, PROCESS, and the D1..D54 range bump at
  every PROCESS §3 site, including the older scripts' range knobs.
- `fix(mcp)` 79b29ad — the parse-error arm in `crates/unblock-mcp/src/wire.rs` classifies the
  failure (not JSON, or JSON that is no message) and takes the id from `scan` only for the second
  kind; the parse-error corpus data in `envelope_id_corpus.rs`; the `Tier::IdInserted` tier with
  F20 and NS2 moved into it and the W-R1 cell deleted; the rewritten d47 P8 and d50 P3 rows.
- `test(mcp)` 7fecaf9 — the corpus cells, the pre-handshake cell in `server.rs`, and
  `crates/unblock-mcp/tests/parse_error_id_duplex.rs` with C-E1 and C-E2.
- `ci(d54)` 5c55bbf — `scripts/checks/d54-parse-error-id-claims.sh`, wired as a required
  `doc-lint` step.

No `ErrorCode` was minted, no published byte moved for a line outside the class, and
`CONTRACT_VERSION` stays `unblock.mcp.v1.10`. A real rmcp 1.7 client now gets `McpError -32700`
where on `main` it is still pending after 3 s.

### The gate record

- Design Review (2026-10-01): PASS WITH MUST-FIXES, 6 must-fix (A1-A6), 9 notes (A7-A15), 7
  refused. ReviewBypass ran 112 named frames and 160k fuzzed lines on `main` and the fix against an
  independent model of the predicate: 0 mismatches. ReviewVerification killed 26 planned mutants
  and 5 of its own, none hung; ReviewContract applied 34 mutations to the d54 script and 31 turned
  their row red. The must-fixes were a too-narrow disclosure of the compatibility drop (new corpus
  row X13), an understated memory cost for a huge id, Q1-Q12 satisfiable by a test line, a C-E2
  guard that hung instead of failing, seven corpus rows deletable with the suite green, and a false
  uniqueness label. All were applied before Implement.
- Verify (2026-10-02): PASS WITH MUST-FIXES, 7 must-fix (V1-V7), 5 notes, 7 refused, all
  text: a cost bound of 30 % where the row's own figures give 33.75 %, five sites stating the rule
  without the compatibility-drop exception, a stale corpus note, a D43 sentence that ignored D47's
  `-32600`, a witness doc claiming sole kills, and two commit messages. VerifyWire checked 40
  hand-built frames and a 3,000-line seeded fuzz against an independent oracle with zero flags;
  VerifyMutation ran 38 production mutants, 36 killed by tests and 2 behaviour-equivalent ones
  caught by the d54 script; VerifyGates re-ran every required job, 1854 tests passed, and turned a
  named cell red under every simulated rmcp bump. After the fix pass and a delta check the result
  is PASS.
- Routed rather than fixed, all pre-existing: `ub-a3w` (scan allocates a `String` per root key),
  `ub-f1k` (the `unblock-mcp` duplex harness read has no deadline), a comment on `ub-46o` (the CLI
  half of the same hang) and a comment on `ub-fh5` (an insta timing flake in `contention_lab`).

## Gotchas

- An orphaned test binary from the Spec/Plan coordinator's scratch run hung for 3 h on the pre-fix
  C-E2 shape before it was found and killed at the design Review gate; the cell's guard awaited a
  oneshot before the sentinel ping (fixed as A4).
- `git archive` restores old mtimes, so building `main` and the branch in one target directory
  let cargo reuse a stale binary across trees. The Verify protocol gave each tree its own
  `CARGO_TARGET_DIR` and compared the two binaries with `cmp` before trusting any before/after.
- A differential tier that shares rmcp's codec with the code under test cannot see drift in that
  codec. The hand-written literals in the test-only module are the absolute oracle; VerifyGates
  showed they catch what the codec-sharing cells miss.
- Two commit messages stated the predicate without its strict-JSON leg, or credited a witness
  block to the wrong cell. They were rewritten by autosquash under the `ub-cnv` precedent that a
  commit message stating something false is rewritten.
- The MCP sync export only writes in the main checkout, so the record was exported there, copied
  into the branch worktree, and the main checkout restored and confirmed by checksum.
- A mutant that turns a correlated reply into silence hangs the D47 id-correlating cells rather
  than failing them (M16, M13p); this run's own cells use sentinel follow and did not hang. Filed
  as `ub-f1k` and a comment on `ub-46o`.

## Glossary

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-----------------------------------------------|
| E01..E24 | the parse-error corpus rows inside the class: each a failed line with a readable id that must now be answered on it | `crates/unblock-mcp/src/envelope_id_corpus.rs`, the D54 corpus |
| X01..X13 | the parse-error corpus rows outside the class, which keep rmcp's bytes or get no reply | `envelope_id_corpus.rs`, the D54 corpus |
| X13 | the out-of-class row whose duplicated `method` ends in a non-standard `notifications/*`, dropped silently as in rmcp | `envelope_id_corpus.rs`, added at the design Review gate (A1) |
| F17 | the D47 framing-corpus entry for an id-carrying `notifications/*` frame with untypeable `params`, dropped at parity with rmcp | `crates/unblock-mcp/src/wire.rs`, CD-7 corpus |
| F20 | the D47 framing-corpus entry with a duplicated `method`, whose reply changed and which moved into the D54 tier | `wire.rs`, CD-7 corpus |
| NS2 | the CLI raw-stdio cell that sends a duplicated `params` key, retargeted to the correlated reply | `crates/unblock-cli/tests/duplicate_key_frames.rs` |
| W-R1 | the deleted cell that pinned the duplicated-method reply as still id-less | formerly `wire.rs` tests, `the_duplicated_method_residual_is_still_id_less` |
| W-G3′, W-G4′, W-G5′ | the parse-error representation cells: rmcp-bump detection, every parse-error kind and witness row represented, and the strict-JSON gate halves | `wire.rs` `mod tests` |
| `Tier::IdInserted` | the CD-7 tier where our reply must equal rmcp's bytes with exactly the declared id inserted, and rmcp's own reply the id-less `-32700` | `wire.rs` CD-7 harness |
| C-E1, C-E2 | the duplex cells: the parse-error reply on the serve path, and a real rmcp client released by it | `crates/unblock-mcp/tests/parse_error_id_duplex.rs` |
| S-E1, P-E1 | the serve-loop and pre-handshake cells for the class, named in the `ub-46o` comment | `server.rs` and `wire.rs` tests |
| AC1, AC3 | `ub-788`'s acceptance criteria for the id form per case and for the byte-identity reconciliation | `ub-788` description |
| Q1-Q12 | the d54 claims-script rows anchored on `wire.rs` production lines | `scripts/checks/d54-parse-error-id-claims.sh` |
| Q12 (d47) | the d47 claims-script row anchored on D47's own `Some(id)` line, which D54's anchor must not also match | `scripts/checks/d47-envelope-id-claims.sh` |
| Q53 | the d53 claims-script row that went red once D54 became the newest script, retargeted | `scripts/checks/d53-request-integrity-claims.sh` |
| P8, P3 | the d47 and d50 rows that pinned `ub-788` as an open residual, rewritten | `d47-envelope-id-claims.sh`, `d50-pre-handshake-gate-claims.sh` |
| A1-A15 | the design Review gate's admitted items, A1-A6 must-fix and A7-A15 notes | `ub-788` Review comment; the gate verdict (session artifact, not committed) |
| A1 | the Review must-fix widening the compatibility-drop disclosure and adding corpus row X13 | `ub-788` Review comment, item (1) |
| A4 | the Review must-fix making C-E2's guard fail after the sentinel ping instead of hanging | `ub-788` Review comment, item (4) |
| A7 | the first Review note: the response-shaped residual holds before the handshake too | `ub-788` Review comment, notes |
| V1-V12 | the Verify gate's admitted items, V1-V7 must-fix, V8-V11 notes, V12 the post-fix checks | `ub-788` Verify comment; the gate verdict (session artifact, not committed) |
| M13 | the Verify mutant (written M13p in the `ub-46o` comment) that drops the BOM strip in `try_parse`; it hung a CLI id-correlating cell | `ub-46o` comment |
| M16 | the Verify mutant that stringifies ids in `scan`; it hung D47 id-correlating cells in both crates | `ub-46o` comment, `ub-f1k` |
| ReviewBypass, ReviewContract, ReviewVerification | the design Review lenses: bypass hunt, contract and cascade, verification non-vacuity | `ub-788` Review comment |
| VerifyWire, VerifyCode, VerifyMutation, VerifyGates | the Verify lenses: shipped-binary wire probes, code and prose truth, mutation, independent gate re-run | `ub-788` Verify comment |

## Links

- `ub-788` — the `-32700` parse error omitted the id of a line whose id was readable, hanging an
  rmcp client.
- `ub-a3w` — `envelope_id::scan` heap-allocates a `String` for every root key.
- `ub-f1k` — the `unblock-mcp` duplex harness read has no deadline.
- `ub-46o`, `ub-fh5` — existing issues that received comments from the Verify gate.
- Pull request — pending.
- Key files touched —
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-mcp/src/wire.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-mcp/src/envelope_id_corpus.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-mcp/tests/parse_error_id_duplex.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/scripts/checks/d54-parse-error-id-claims.sh`,
  `/Users/ramosmig/Public/WS-Labs/unblock/docs/PRD.md` (the D47 and D54 rows, NFR-18).
- Prior related run-reports — `runs/2026-08-06-envelope-id-reject.md`, the D47 run that recorded
  this residual, and `runs/2026-09-30-receive-cancellation.md`, the D53 run whose parked reply path
  the parse-error reply uses.
