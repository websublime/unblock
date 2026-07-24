---
name: project-comments-pull-forward-v1
description: MERGED 2026-07-17 — T3.9 comments (D37) landed on main (rebase, 823487d), CI 17/17 green; only open item = flip the STATUS.md T3.9 row ◐→☑ (blocked from a direct-to-main commit; fold into the next branch touching STATUS)
type: reference
---

**T3.9 = the D37 comment surface, GA-BLOCKING** (D-A: the human `v1.0.0` tag-push waits on it). ONE PR, ONE
vertical slice (`model→storage→engine→render→mcp→sync`); the atomic unit is the `unblock.mcp.v1.4`→`v1.5`
contract bump. Branch `t3.9-comments` off `main` @ `f77d92a`. Origin: 2026-07-16, Miguel parked dogfooding —
*"é fundamental ter toda a funcionalidade de comments antes de dogfooding."*
The D37 docs-only cascade is MERGED ([PR #421](https://github.com/websublime/unblock/pull/421), `f77d92a`).

**⚠️ T3.9 is NOT the only GA blocker.** A CONCURRENT session is landing **T3.2.1/D38** (FR-17 signal-exit hang),
also PRE-GA — see [[feedback-concurrent-session-shares-the-working-tree]]. Both branch off `f77d92a`; both edit
STATUS.md / PRD / spine / impl-plan → **expect a textual rebase**. Miguel (2026-07-17): T3.9 continues in
worktrees, rebase at the end. **CORRECTION to prior advice:** the C6 `c6_second_signal_escalation_never_hangs…`
failure is NOT a flake to re-run — D38 root-caused it as the same real defect as the handshake hang.

## Miguel's two fork resolutions (2026-07-17) — surfaced by the Understand sweep, MISSED by the D37 cascade
- **FORK 1** → ADD `StorageError::CommentNotFound { id: i64 }` → maps to the **existing** `ErrorCode::IssueNotFound`.
  FORK-E1 governs the *ErrorCode* taxonomy (stays 36; no exit-table/oneOf re-bless), NOT `StorageError`. Spine
  §3.1's "full v1 variant set" was a **gap** (the cascade never touched that list) → corrected spec-first. Reuse was
  rejected: `IssueNotFound{id: comment_id}` Displays "issue 42 not found" for a missing *comment* — lying to an
  agent in an agent-first tracker.
- **FORK 2** → NEW public `CommentValidator::validate_body(body)`. `add_comment`→`validate_comment`;
  `update_comment`→`validate_body` (it has only comment_id+body, and there is no `Storage::get_comment`).
  **NORMATIVE (spine §1.9:277): `validate_comment` must NOT call `validate_body`** — the latter returns an
  already-sealed `Result`, so `validate_body(body)?` is fail-fast and would emit a 1-entry FR-11
  `context["fields"]` where `IssueValidator` emits N, silently breaking the D-E1 uniform-aggregate carrier. BOTH
  public entries call a **private `body_rules(body, &mut Vec<FieldError>)`**, each sealing its own aggregate.

## Locked design (from the merged D37 cascade)
Dedicated **`comment` MCP tool** = the 8th (RK-3 ≤8 budget now FULL), superseding the old `issue comment` sketch.
CRUD add/list/update/delete. Hydration on **all 7** read paths via batch `hydrate_ids`/`hydrate` (the T3.5.1
pattern). `Comment` +`updated_at`/`redacted_at` (Option, skip-when-None). `update` = provenance-preserving
(`updated_at`=now + `EventType::CommentEdited`); **MUST-1: `add` leaves `updated_at` NULL**. `delete` =
**soft-redact** (KEEP row, mask `text=""`, `redacted_at`=now, `EventType::CommentRedacted` **retaining the original
body**); wire form = `redacted_at` present + `"text":""` (presence IS the flag); idempotent if already redacted.
FORK-3 existence-only guard (CLOSED issues still accept comments). Author = session actor (FORK-M1b). `sync_equals`
compares `redacted_at`, IGNORES `updated_at` (FORK-M2). `issues.updated_at` bumps on add+edit+redact (FORK-S1).
`content_hash` UNAFFECTED (FR-26 intact). Renders plain/markdown/json/robot; CSV comment-free; **NO CLI command**;
FLAT not threaded (FORK-T).

## Status
- **Spec-first pass DONE** — 10 docs-only commits (`a857e64`..`ab616f7`), **0 `.rs`**, doc-lint green. The design
  gate ran 2 adversarial iterations (7 must-fixes → 1 → closed; the last hand-fixed by the orchestrator with
  Miguel's authorisation, per PROCESS §4 trivial-edit + §5 escalate-at-2).
- **Implementation in flight** — SINGLE implementer, isolated worktree, incremental commits. The crates are ONE
  compile unit: the 4 trait methods break all **12** `impl Storage for` blocks at once (not 11 — `GateStorage` at
  unblock-mcp/tests/common/mod.rs:466 is the missed 12th), and the stub idiom is **per-site** (sync's 2
  FakeStorages use `unimplemented!`; the RaceInjector at contract.rs:322 must DELEGATE).

## The silent traps (compile clean + clippy clean + CI green, while shipping broken software)
Why the checklist itself was untrustworthy: [[feedback-task-checklists-rot-run-understand-first]].
- `sync_eq.rs:92` derived `PartialEq` → adding `updated_at` violates FORK-M2 with **zero** compile errors.
- `agents_digest.rs:106` `[(&str,&Value);7]` + `:124 unwrap_or_default()` → GA's AGENTS.md ships an **empty**
  `comment` actions table and no test fails (the ":124 caught by a test" comment is FALSE).
- `mappers.rs:254` `parse_event_type` catch-all `other => Custom(..)` → `comment_edited` reads back as `Custom`,
  breaking the 17-named oracle, clippy-clean.
- `crud.rs:173` 4-col seed INSERT (in `insert_issue_in_tx`, shared by create_issue AND create_issues) → a redacted
  comment imports back UN-redacted. MUST-1 scopes to `add_comment` ONLY — over-applying it here is the natural misread.
- `server.rs:175` embeds "7 tools, 5 resources, 3 prompts" in the LIVE handshake — agent-visible, unpinned.
- **Vacuous ACs:** export/render goldens use comment-less fixtures → a re-bless cannot diff; the green tick
  certifies ZERO coverage. Live+redacted fixtures must come first.
- **Timestamp precision is PER-TEST:** `serialize_issue_line` renders at SECOND precision and FORK-M2 puts
  `redacted_at` in the compare → a blanket sub-second fixture reddens `roundtrip_proptest` + `contract.rs:83`, and
  the tempting fix (drop `redacted_at`) is a silent FORK-M2 violation. Sub-second ONLY for the export_insta
  canonicalizer non-vacuity golden.

## Watch on Verify
- **Bench-gate likely REAL, not the known flake:** `hydrate_ids` goes `1+3·⌈N/900⌉`→`1+4·⌈N/900⌉` (~33% more
  round-trips on all 5 read paths) against ceilings T3.5.1 just re-tightened. Do NOT `rerun --failed` by reflex; do
  NOT widen a ceiling in this PR — cf. [[reference-bench-gate-cmp-ready-sort-250k-flaky]].
- **Goldens that must NOT move:** `golden_hash__canonical_content_hash.snap` (FR-26), the exit-code/error goldens
  (FORK-E1), the csv goldens + the 15-col pin (FORK-R1), help_snapshots (FORK-R2).
- **testkit gate:** `tests/contract.rs` is `#![cfg(feature="testkit")]` → a bare `-p unblock-storage` compiles the
  NFR-16 cases to NOTHING. Probe workspace-scoped `--all-features`, mirroring the **17**-job CI (11 is the stale M0
  count) — see [[feedback-implementer-probe-must-include-cargo-fmt]].
- **Zero-live-hits** must be scoped `git grep -n "unblock\.mcp\.v1\.4"`; a bare `-nw v1.4` collides with the product
  roadmap version + frozen history — see [[feedback-rename-zero-live-hits-needs-git-grep-w-allfiles]].
