---
name: 2026-10-27-ub-m07-ready-filters
description: Making `query kind:ready` honour its filters (tracker ub-m07) — ready_issues now composes every narrowing facet like blocked, orders by the cmp_ready key mirrored as SQL and pages after the blocked-set filter, and blocked honours offset; three design rounds settled the paging location, the include_deferred no-op and the parity-test home after a solo start on main was moved onto a branch.
type: run
date: 2026-10-27
branch: ub-m07-ready-filters
pr: '465'
issues: [ub-m07]
---

# Run — `query kind:ready` honours its filters and offset

## Context

Issue `ub-m07`: over MCP, `query {kind:"ready"}` ignored `labels_any`, `labels_all`, `text_contains`,
`status` and `offset`, while `list` applied them. Full lifecycle (Understand, Decide, Design Review,
Spec/Plan, Implement, Verify, Track) on branch `ub-m07-ready-filters` in the isolated worktree
`../unblock-ub-m07`; team run — an orchestrator, one implementer, and parallel read-only reviewers per
gate (design Review: architecture/spec, Rust-correctness and QA lenses; Verify: code review, QA mutation,
Rust engineering and security). Design Review passed after round 3 by Miguel's delegated approval;
Verify passed in round 2 (final 4/4). PR: https://github.com/websublime/unblock/pull/465.

## What & why

The run started with a process violation: the orchestrator edited a fix solo on `main` in the shared
tree. The work moved, uncommitted, to the branch, the earlier issue comment was marked SUPERSEDED, and
the draft counted only as Understand evidence. Understand (graph unavailable, grep over `crates/`)
traced the fault to storage: `ready_issues` built its `WHERE` with a facet helper that applied only
`issue_type`, the priority range and `assignee`, while `list`/`search`/`count`/`stale` and `blocked`
applied every narrowing filter. Round 1 also found that ready paged in its `priority ASC` pre-sort
before the engine's `cmp_ready` re-sort (which buckets P0 with P1), so `limit=1` could return the wrong
top item, and that `blocked` ignored `offset`.

Decide (Miguel, 2026-10-27): ready pages in storage, with storage ordering candidates by the same key
as `unblock_policy::cmp_ready` (the bd approach) while taking no policy dependency (CF-11 intact; the
engine re-sort stays and must agree); `include_deferred`/`include_closed` are ratified no-ops on ready,
a deliberate departure from bd. Both ride an inline D18 addendum — no new D-id. Authoritative text: PRD
§4 D18 addendum and FR-4 AC, spine §3.2 / §3.2.1 (`ready_issues`, `blocked_issues`) / §3.3
(`issues.created_at` encoding invariant) / §4.1, storage plan OQ-3, policy plan Q1, implementation-plan
T1.5. The report does not restate them.

## Outcome

- Design Review round 1 FAIL 3/3 (not spec-first; paging before the re-sort; include_deferred
  unratified; test gaps; blocked ignoring offset) — looped to Decide and Spec/Plan. Round 2 FAIL 3/3
  (no viable home for a storage-vs-policy parity test; the `created_at` byte order equals chrono order
  only for the `to_rfc3339` `+00:00` form; fixtures unnamed; stale doc comments; the bd departure
  unrecorded; no roadmap bullet) — escalated. Round 3 FAIL 1 pass / 2 fail (the timestamp rule
  contradicted the new `created_at` bullet; the invariant overclaimed scope; F-BLOCKED-OFFSET could not
  catch SQL-side paging) — escalated again; after the fixes Miguel delegated approval: PASS.
- Implement: `ready_issues` composes `compose_facets`, orders by
  `CASE WHEN priority <= 1 THEN 0 ELSE 1 END ASC, created_at ASC, id ASC` and applies offset then limit
  after the in-Rust blocked filter; `blocked_issues` applies offset after its membership test; the old
  facet helper is gone. Six storage contract cases, three engine tests and one MCP wire test land.
- Verify round 1: 3 PASS, 1 FAIL — the wire offset check (`offset=1,limit=1`) matched what the pre-fix
  code also returned; `offset=1,limit=2 ⇒ [ub-plain, ub-text]` was added, which fails on the pre-fix
  code, on a `priority ASC` order and on an ignored offset. Round 2 (code review) PASS; final 4/4.
  Executed proof: against the `main` `query.rs` the wire test fails; an ignored-offset mutant fails it;
  with the fix the storage contract (`--features testkit`), engine reads 35/35, the MCP wire test and
  `contract_suite` pass; clippy, doc-lint and fmt clean.
- D35: no schema byte moves, so `CONTRACT_HASH` and `unblock.mcp.v1.10` are unchanged.
- Commits: f99c584 fix(storage), a4c0313 test(storage), 7df40ec test(engine), 5e50c7a test(mcp),
  cfec903 docs(plan), then this chore(tracker) commit with the run-report and the re-export.

## Gotchas

- Never draft a fix solo on `main` in the shared tree (PROCESS §4); the work had to move to the branch
  and the solo comment was superseded.
- An unbounded `cargo test --workspace --all-features` froze the machine and timed out after 40
  minutes. Run one command at a time with `CARGO_BUILD_JOBS=4` and `--test-threads=4`, targeted tests only.
- The storage contract suite only runs with `--features testkit`
  (`crates/unblock-storage/tests/contract.rs:20`); a plain `cargo test -p unblock-storage` skips it
  silently.
- The design Review needed three rounds and two escalations; Miguel decided the paging location, the
  include_deferred no-op and the parity-test home, and approved after round 3.
- Fixture bug caught in Implement: the `labels_all` single-facet filter also excluded the `labels_any`
  decoy, so the "unfiltered minus its own decoy" assertion was wrong for that facet
  (the F-DECOY decoy builders `facet_decoys`, `crates/unblock-storage/src/testkit.rs:1942` and
  `crates/unblock-engine/tests/reads.rs:2213`).
- The MCP `sync` export refuses paths outside its confine root, so `.unblock/issues.jsonl` was exported
  in the main tree and copied into the worktree.
- The codebase-memory graph index held 21 nodes (effectively empty), so the graph was unavailable and
  Understand used grep.

## Glossary

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-----------------------------------------------|
| F-DECOY | the fixture with one single-field decoy row per narrowing facet plus one hit; for every query kind the unfiltered call returns the hit plus every eligible decoy, and each single-facet filter drops exactly its own decoy; the search needle sits in the shared description | `crates/unblock-storage/src/testkit.rs:2057` (`contract_every_kind_applies_every_facet`, decoys from `facet_decoys` :1942); `crates/unblock-engine/tests/reads.rs:2331` (`assert_f_decoy_every_kind`); spec `docs/plans/crates/unblock-storage.md:79` |
| F-READY-STATUS | the fixture proving ready's status facet intersects the `open` baseline (an unblocked `in_progress` row never appears; `status=[open,in_progress]` gives only the open row) | `crates/unblock-storage/src/testkit.rs:2238`; `crates/unblock-engine/tests/reads.rs:2432` |
| F-READY-NOOP | the fixture proving `include_deferred`/`include_closed` leave ready unchanged | `crates/unblock-storage/src/testkit.rs:2281`; `crates/unblock-engine/tests/reads.rs:2471` |
| F-PAGE | the paging fixture: an `in_progress` blocker and blocked rows sharing their neighbours' bucket and sorting ahead of the paged target; checks limit, offset+limit, concatenated pages, offset without limit and `offset = len + 1 ⇒ []` | `crates/unblock-storage/src/testkit.rs:2342`; `crates/unblock-engine/tests/reads.rs:2534` (asserts at `Storage::ready_issues` and `Session::ready`) |
| F-PARITY | the ordering fixture pinning the SQL key to `cmp_ready`: a P1-older/P0-newer pair, P4 < P3 < P2 by age, an id tie, and a whole-second vs half-second pair whose ids sort the opposite way | `crates/unblock-storage/src/testkit.rs:2418` (literal order); `crates/unblock-engine/tests/reads.rs:2639` (raw storage vs `sort_by(cmp_ready)`); spec `docs/plans/crates/unblock-storage.md:122` |
| F-BLOCKED-OFFSET | the fixture proving blocked applies offset after the membership test: a non-blocked P0 blocker sorts between the two blocked rows, so SQL-side paging would shift the page | `crates/unblock-storage/src/testkit.rs:2455`; `crates/unblock-engine/tests/reads.rs:2685`; spec `docs/plans/01-design-spine.md:1566-1569` |
| CF-11 | the cross-crate finding that removed storage's dependency on policy; storage may mirror a policy order but never imports policy, and the engine composes the two | `docs/PRD.md:301`; `docs/plans/README.md:111`; `crates/unblock-storage/src/trait_def.rs:43-45` |
| OQ-3 | the storage plan's open question on ready-ordering ownership, resolved earlier and amended here so storage mirrors the `cmp_ready` key | `docs/plans/crates/unblock-storage.md:122` |
| policy Q1 | the policy plan's question on who owns the ready sort; policy keeps `cmp_ready`, now amended so storage mirrors its key | `docs/plans/crates/unblock-policy.md:110`; `crates/unblock-policy/src/ready.rs:156` |
| D18 addendum | the inline amendment to decision D18 (filter composition on `blocked`) that extends it to ready: facets, status ∩ open, include_* no-ops, the mirrored key, paging after the blocked filter, blocked offset | `docs/PRD.md:108`; spine `docs/plans/01-design-spine.md:1515-1569` (the `ready_issues` and `blocked_issues` entries) |
| D35 | the GA semver/contract freeze decision; it decides whether a change moves `CONTRACT_HASH` / `CONTRACT_VERSION` (here: no) | `docs/PRD.md:125` |
| CF-TS | the canonical-timestamp rule (`fmt_ts_secs`), now scoped to output/export; storage persistence never uses it | `docs/plans/01-design-spine.md:479`, scope sentence :495 |
| `issues.created_at` encoding invariant | the spine rule that `bind_issue` always writes `to_rfc3339()` (`+00:00`, AutoSi fractions), so the stored text sorts in chrono order | `docs/plans/01-design-spine.md:1977`; `crates/unblock-storage/src/libsql/mappers.rs:186` |
| bd departure | the port source `bd` honours `include_deferred` on ready; unblock deliberately does not (Miguel, 2026-10-27) | `temp/beads_rust-main/src/storage/sqlite.rs:5019-5036` (out-of-tree port source); `docs/PRD.md:108` |
| round 1/2/3 | successive iterations of the design Review or Verify gate | `ub-m07` issue comments |
| must-fix (1)–(5), (a)–(f), (1)–(3) | the numbered must-fix lists of design Review rounds 1, 2 and 3 respectively | `ub-m07` issue comments (rounds 1–3) |
| Z writer / Secs writer | named mutants: a `created_at` writer that emits a `Z` suffix (`'Z' > '.'` breaks byte order) or truncates to `SecondsFormat::Secs` (the half-second pair ties and the id order flips); F-PARITY catches both | `crates/unblock-storage/src/testkit.rs:2418` |
| SQL-side paging mutant | applying OFFSET/LIMIT in SQL before the in-Rust blocked-set (ready) or membership (blocked) filter; F-PAGE and F-BLOCKED-OFFSET catch it | `crates/unblock-storage/src/libsql/query.rs:97-102`, :150-155 |
| `DECOY_NEEDLE` | the shared search needle (`"zork"`) placed in every decoy's description so the `text_contains` and `search` decoys are not vacuous | `crates/unblock-engine/tests/reads.rs:2194` |
| wire repro | the MCP duplex test sending `query {kind:"ready",labels_any:["x"]}` and the offset/status cases against the real server | `crates/unblock-mcp/tests/query_ready_filters_duplex.rs:38` |

## Links

- `ub-m07` — `query` kind `ready` ignores its filters (`labels_any`, `labels_all`, `text_contains`,
  `status`, `offset`).
- Files: `crates/unblock-storage/src/libsql/query.rs`, `crates/unblock-storage/src/trait_def.rs`,
  `crates/unblock-storage/src/testkit.rs`, `crates/unblock-engine/src/session/read.rs`,
  `crates/unblock-engine/tests/reads.rs`, `crates/unblock-policy/src/ready.rs`,
  `crates/unblock-mcp/tests/query_ready_filters_duplex.rs`, `docs/PRD.md` (D18 addendum),
  `docs/plans/01-design-spine.md`, `docs/plans/crates/unblock-storage.md`.
- Prior: [2026-10-23-ub-q2v-agents-discriminant](2026-10-23-ub-q2v-agents-discriminant.md) (the
  same solo-on-main reset).
