---
name: 2026-11-17-ub-9ov-rmcp3-advisories
description: Clearing the CI audit/deny advisories by moving the MCP server from rmcp 1.7 to 3.10 and onto MCP 2026-07-28 (tracker ub-9ov, T4.4, D59) — the only patched rmcp was two majors away, so the wire converges on rmcp 3 except the D54 parse-error reply, the contract moves additively to unblock.mcp.v1.13 with six documented narrowings for clients that break the MCP spec, a measured Recoded tier joins the parse differential, quinn-udp and rustix take semver-compatible bumps, and a planned chrono migration became one dated deny ignore (ub-4db) once rmcp 3.10 turned out to depend on chrono unconditionally.
type: run
date: 2026-11-17
branch: ub-9ov-rmcp-3-advisories
pr: '475'
issues: [ub-9ov, ub-4db, ub-bj0, ub-o8s]
---

# Run — rmcp 3.10, MCP 2026-07-28 and the CI advisories (D59)

## Context

Issue `ub-9ov`: CI `audit` and `deny` turned red on main after advisories were published against
crates already in the lockfile; the work became implementation-plan task T4.4 under the new decision
D59. The run went from 2026-11-16 to 2026-11-17 on branch `ub-9ov-rmcp-3-advisories`, cut from main
`60d9551` after PRs #473 and #474 merged, in the isolated worktree `../unblock-9ov`. PR: #475 (draft
for the Verify CI runs). Phases: Decide (Miguel chose to upgrade rather than ignore), Understand
(three read-only lenses), a second Decide on the wire forks, Spec/Plan in two iterations plus a final
fixup, design Review in two rounds that escalated to Miguel and closed with a delta confirmation,
Implement (one implementer, with a spec-first amendment for a measured drift and a mid-run chrono
detour), Verify in two rounds (round 1 FAIL from two lenses, round 2 delta PASS from all four), then
this Track step. Team: one coordinator (orchestrator), three Understand lenses, one spec writer, three
design-Review lenses, one implementer, two read-only chrono Understand lenses, four Verify lenses and
one Track agent.

## What & why

Three advisories hit the lockfile: RUSTSEC-2026-0538 (quinn-udp 0.5.14, reached through the
`unblock update` HTTP path), RUSTSEC-2026-0486 (rustix 1.1.4, unsound) and RUSTSEC-2026-0462 (rmcp
1.7.0). The first two had semver-compatible fixes (quinn-udp 0.5.16, rustix 1.1.6). rmcp was patched
only from 3.9, and although the vulnerable code sits in the HTTP client transport that unblock does
not compile, Miguel ruled an upgrade instead of an ignore. rmcp 2 tracks MCP 2025-11-25 and rmcp 3
tracks MCP 2026-07-28, whose stateless lifecycle can skip `initialize`, so the move also changed the
protocol surface. Miguel's rulings fixed the shape: converge on rmcp 3 wire behaviour everywhere
except D54, adopt MCP 2026-07-28 with `server/discover`, redesign the version clamp and the D50
pre-handshake gate, and bump the contract additively to `unblock.mcp.v1.13`. Authoritative text: PRD
§4 D59 (clauses (1)–(19) with its inline amendments) and its reciprocal notes on D2, D25, D27, D38,
D40, D42, D43, D47–D50, D53 and D54; FR-20, NFR-18 and RK-2; spine §0.1, §5, §5.2a, §5.6 and §5b; the
unblock-mcp crate plan ("Pinned rmcp 3.10 API contract"); ci-cd §2.1; implementation-plan T4.4; and
the new gate `scripts/checks/d59-rmcp3-claims.sh`.

## Outcome

Landed on the branch over `60d9551` as four atomic commits, spec first:
- `docs(d59)` `ae2757b` — the D59 spec, its downstream notes and the d59 claims gate.
- `docs(d59)` `123bb50` — inline amendments: the Recoded tier in clause (12) and criterion (c), the
  single dated deny ignore in clause (1) and criterion (b), and the sixth narrowing plus the
  `_meta`-before-quota note.
- `feat(mcp)` `cf8de49` — rmcp 3.10, the three-rule reply arm, the redesigned pre-handshake gate,
  the clamp removal, the closed-stdout exit-0 extension, contract v1.13, the regenerated AGENTS.md
  block, the deny.toml ignore and the quinn-udp/rustix bumps.
- `test(mcp)` `098fac6` — the criterion cells and their hold-seam helpers.

Design Review: round 1 PASS WITH CHANGES from all three lenses (five must-fixes), treated as a loop
back; round 2 PASS from two lenses and PASS WITH CHANGES from architecture (three must-fixes, one
needing Miguel); escalated under PROCESS.md §5, ruling 411 settled the exit-code fork and authorised
a delta confirmation, which passed on all three lenses. Verify: round 1 at `2ae90c3` (CI run
44082003668, every job green except `run-report-gate`) gave code-review and security PASS WITH NOTES,
qa and rust/mcp FAIL on two missing acceptance cells; iteration 1 added them; the round-2 delta at
`f7f12c2` passed on all four lenses. Follow-ups opened: `ub-4db` (chrono), `ub-bj0` (cooperative
cancellation of writes); `ub-o8s` (16 MiB line cap) stays out of scope. The commits were re-cut
after Verify to the head named above.

## Gotchas

- **Two majors for one advisory.** The rmcp fix exists only from 3.9, and rmcp 3 follows MCP
  2026-07-28, so a lockfile-only bump (the precedent of `ub-5xl`, `ub-xl6`) was impossible and the
  run carried a protocol change (Understand, comment 400).
- **Converge, except D54.** rmcp 3 ignores invalid JSON silently, so a client that sent one would
  wait forever for a reply; D54's -32700 with the recovered id was kept while everything else
  followed rmcp 3 (ruling 401). The parse rule became three labelled rules α/β/γ (ruling 406).
- **2.0.0 vs v1.13.** Miguel first chose 2.0.0; under D35 that would have meant a crate major
  (1.0.2 to 2.0.0), a v2.0.0 release carrying 1.x users across `unblock update`, a new contract-id
  convention, every check script's contract regex moving and the locked additive-only v1.1 roadmap
  absorbed. He ruled v1.13 because each narrowing reaches only clients that break the MCP spec
  (ruling 406).
- **The Recoded tier was measured, not predicted.** A live per-entry differential against rmcp
  3.10's transport showed parse-corpus entry X01 (duplicated `method` plus trailing bytes) gets
  rmcp's id-less -32600 rather than silence, because typed decoding fails before the trailing bytes
  are reached; the spec was amended first and the set guard became Silenced ∪ Recoded ∪ IdInserted
  (ruling 415).
- **Cancellation drops the reply, not the write.** A cancelled write may still commit, so D59
  documents "re-read the state" and `ub-bj0` studies cooperative cancellation. Verify's security lens
  found a sixth narrowing: a client reusing an in-flight id loses the second reply, and 1 and 1.0
  are the same id (comments 422–423).
- **The chrono turn.** `cargo deny` failed on RUSTSEC-2026-0543 (chrono unmaintained); Miguel first
  ruled "migrate now" and a D60 was planned, but rmcp-3.10.0/Cargo.toml declares chrono
  non-optional on every target, so migrating our crates could not clear it; the chrono Understand
  pass also found the migration would move stored and wire date bytes. Result: one dated,
  rationale-carrying ignore under D59, tracked in `ub-4db`, and no D60 (ruling 419).
- **Read-only fds on macOS.** Rust's stdout swallows EBADF there, so a write to a read-only fd
  "succeeds", and RLIMIT_FSIZE is not enforced; the d50 Q22 exit-1 cell moved to ENOSPC on
  `/dev/full`, gated to Linux, with io::ErrorKind unit cells covering every platform (comment 417).
- **audit-check fails only on critical advisories.** The pinned `rustsec/audit-check` logged the
  unmaintained chrono advisory as "not marking check as failed", so `audit` stayed green while
  `deny` was red (comment 422).
- **A write landed in the main checkout.** A spec writer's Python write edited the main checkout's
  `docs/PRD.md`; it was moved to the worktree and main restored. Absolute worktree paths avoid it.
- **Request budgets and HTTP 429.** The implementer hit its per-run request budget mid-commit and
  agents met provider 429s; each was resumed from the worktree state (comment 416).

## Glossary

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-----------------------------------------------|
| T4.4 | The implementation-plan task for the rmcp 3.10 / MCP 2026-07-28 move | docs/plans/implementation-plan.md T4.4 |
| α / β / γ | The three parse-reply rules: α invalid JSON gets an id-less -32700; β valid JSON with a readable id gets -32700 on that id (D54); γ valid JSON without a readable id gets rmcp's id-less -32600 | docs/PRD.md §4 D59 clause (2); ub-9ov comments 406, 415 |
| clause (1)–(19) | The numbered clauses of the D59 row; (12) the parse differential, (13) the closed-stdout exit 0, (15) the contract level and narrowings | docs/PRD.md §4 D59 |
| criterion (a)–(t) | The lettered D59 acceptance criteria in clause (17); e.g. (b) audit/deny green, (c) the parse differential, (e) float ids, (f)/(f1)/(f3) lifecycle cells, (g) and (m) no-reply cells, (i) quota, (s) AGENTS.md lines, (t) closed-stdout exit cells | docs/PRD.md §4 D59 clause (17) |
| narrowing (a)–(e), sixth | The documented client-visible behaviours of v1.13 for clients that break the MCP spec; (e) the wrongly typed inputResponses/requestState getting -32601; the sixth is the reused in-flight id | docs/PRD.md §4 D59 clause (15); AGENTS.md |
| Q1–Q298, N1–N20, P1–P8 | Numbered rows of the d59 claims gate: Q positive claims, N negative (forbidden) patterns such as N3–N10 deprecation-suppression and N19/N20 other-advisory ignores, P rows pinning files such as P8 the deny.toml ignore | scripts/checks/d59-rmcp3-claims.sh |
| Q22 | The d50 gate row pinning the no-signal run-loop error exit-1 cell, now Linux-only | scripts/checks/d50-pre-handshake-gate-claims.sh; ub-9ov comments 416–417 |
| Q8 / Q9 / Q18 / Q19 / Q20 | d50 rows retired or re-pointed by the commit plan | scripts/checks/d50-pre-handshake-gate-claims.sh; ub-9ov comment 412 |
| Q3 / Q4 / Q7 / Q21 | d54 rows kept on parse_error_reply_id, and the d54 row d50 Q20 re-points to | scripts/checks/d54-parse-error-id-claims.sh; ub-9ov comment 416 |
| Q89 / Q25 / Q27 | Rows in the d57, d58 and d59 gates that hard-code the live contract minor, moved to v1.13 | ub-9ov comment 419 |
| ruling 401 | Miguel: converge on rmcp 3 wire behaviour except D54; adopt MCP 2026-07-28; order after #474 and #473 | ub-9ov comment 401 |
| ruling 406 | Miguel: contract v1.13 additive, the α/β/γ parse rule, cancelled-write re-read guidance and a follow-up issue | ub-9ov comment 406 |
| ruling 411 | Miguel's escalation ruling: a failed ping or InitializeResult reply write to a closed stdout exits 0; delta-only confirmation | ub-9ov comment 411 |
| ruling 415 | Orchestrator: keep our X01 output, amend D59 spec-first with the Recoded tier, reshape X01 | ub-9ov comment 415 |
| ruling 419 | Miguel: one dated deny ignore for RUSTSEC-2026-0543 under D59, no chrono migration, no D60 | ub-9ov comment 419 |
| Silenced / Recoded / IdInserted | Divergence tiers of the parse differential: rmcp writes nothing / rmcp writes an id-less -32600 where we write -32700 / we add the recovered id to rmcp's bytes | docs/PRD.md §4 D59 clause (12) |
| X01 / X15 / X02–X05 / X12 | Parse-corpus entries: X01 reshaped to a well-typed object plus trailing bytes, X15 the old X01 shape kept as the Recoded witness, the others silenced entries | crates/unblock-mcp/src/envelope_id_corpus.rs |
| F6 | A parse-corpus entry silenced by rmcp, as specified | crates/unblock-mcp/src/envelope_id_corpus.rs; ub-9ov comment 415 |
| F10 | A parse-corpus entry silenced by rmcp, as specified | crates/unblock-mcp/src/envelope_id_corpus.rs; ub-9ov comment 415 |
| F11 | A parse-corpus entry silenced by rmcp, as specified | crates/unblock-mcp/src/envelope_id_corpus.rs; ub-9ov comment 415 |
| M1 | Design-Review round-2 must-fix 1: -32601 (not -32700) for a wrongly typed inputResponses/requestState | ub-9ov comments 410, 413 |
| M2 | Design-Review round-2 must-fix 2: the ping/InitializeResult exit 1 to 0 flip, escalated | ub-9ov comments 410, 411, 413 |
| M3 | Design-Review round-2 must-fix 3: the commit plan left d50 Q19 red at commit (ii) | ub-9ov comments 410, 413 |
| R2 | Prefix of R2-S1..S3: design-Review round-2 should-fixes (null counts as absent; no-reply cells drain to EOF; InitializeResult exit cell) | ub-9ov comment 410 |
| S4 / S5 / SF1 | Verify round-1 findings: security S4 the reused in-flight id, security S5 `_meta` read before rate limit and quota, qa SF1 a real-binary (f) cell | ub-9ov comment 422 |
| must-fix (1)–(5) / should-fix | Numbered blocking and non-blocking findings of a gate round | ub-9ov comments 408, 410, 422 |
| commit (i)–(iv) | The T4.4 commit plan: docs, implementation, tests, Track | docs/plans/implementation-plan.md T4.4 |
| api / contract / risk | The three read-only Understand lenses | ub-9ov comment 400 |
| architecture / rust-engineer-mcp / edge-cases-CI-client | The three design-Review lenses (rust/mcp and edge/CI/client in comments) | ub-9ov comments 407–413 |
| code-review / qa / rust-mcp / security | The four Verify lenses | ub-9ov comments 421–424 |
| round 1 / 2, delta | Passes of a gate; a delta re-verifies only the fixed items | ub-9ov comments 408–413, 422–424 |
| iteration 1 / 2 | Loop-backs counted against the two-iteration budget before escalation | docs/PROCESS.md §5; ub-9ov comments 408, 410, 422 |
| P3 notes | Lowest-severity non-blocking review notes folded in or left as they are | ub-9ov comments 413, 424 |
| hold seam | The test-util hook that holds a handler and signals its return, used by the no-reply cells | crates/unblock-mcp (cfg feature test-util); ub-9ov comments 414, 423 |
| CI run 44082003668 | Verify round-1 CI run at `2ae90c3` | PR #475 checks |

## Links

- `ub-9ov` — T4.4 / D59, rmcp 1.7 to 3.10, MCP 2026-07-28 and the CI advisories (this run);
  `ub-4db` — track chrono (RUSTSEC-2026-0543) until rmcp drops it, then remove the dated ignore;
  `ub-bj0` — study cooperative cancellation of writes before commit; `ub-o8s` — the 16 MiB line
  cap, kept out of scope.
- PR: https://github.com/websublime/unblock/pull/475
- Key files: `docs/PRD.md` (D59), `docs/plans/01-design-spine.md`,
  `docs/plans/crates/unblock-mcp.md`, `docs/plans/ci-cd-and-distribution.md` §2.1,
  `scripts/checks/d59-rmcp3-claims.sh`, `crates/unblock-mcp/src/`,
  `crates/unblock-cli/src/commands/agents.rs`, `AGENTS.md`, `deny.toml`, `Cargo.lock`.
- Prior related run-reports: [2026-10-02-parse-error-recovered-id](2026-10-02-parse-error-recovered-id.md),
  [2026-09-30-receive-cancellation](2026-09-30-receive-cancellation.md),
  [2026-11-16-ub-lp9-3-fr16-health-full](2026-11-16-ub-lp9-3-fr16-health-full.md).
