---
name: project-roadmap-resequence-2026-07
description: Post-GA roadmap resequence (2026-07-20) — LANDED as PRD D41: v1.2↔v1.3 swap (planning before shared state), v1.0.1 patch slot, streamable-HTTP deferred, TUI pulled forward to v1.4
type: reference
---

**Post-GA roadmap iteration (2026-07-20, Miguel) — LANDED as D41.** v1.1 stayed as-is; the v1.2+ PROPOSED
sequence was reordered/clarified. The decisions landed together in one spec-first cascade (00-roadmap.md + PRD
§4 D41) — NOT piecemeal (avoids repeated edits to the same roadmap sections). Relates to
[[project-dogfood-unblock-is-the-tracker]] (this iteration happened while dogfooding surfaced the planning
need).

## DECIDED
- **SWAP v1.2 ↔ v1.3 (2026-07-20, Miguel):** the PLANNING layer (goals + milestones + the consolidated
  `planning` MCP tool, pure-local) becomes the **new v1.2**; SHARED STATE (libsql embedded replicas /
  Turso Sync) becomes the **new v1.3**. Decided via a 3-lens Decide workflow (effective consensus: default
  SWAP, flip to KEEP only on a firm next-release shared-state commitment — Miguel confirmed NONE, D28 =
  internal teams so timing is his). **Rationale:** (1) schema-before-distribution — settle the additive
  planning schema on one cheap local file before replication (migrating a schema across a primary +
  version-skewed replicas is far costlier; reinforced by the just-filed no-forward-migration bug); (2)
  vendor-tech timing — Turso's own guidance favors Turso Sync over embedded replicas for sync use cases;
  Turso Sync = the durable path but still BETA + a less-mature Rust SDK (`turso` 0.4.3 stable/0.7.0-pre) →
  SWAP buys ~1 release for the tech to mature, likely building shared-state directly on Turso Sync (which
  also unlocks offline writes embedded replicas can't); (3) dogfood planning demand felt NOW while
  shared-state solves a not-yet-felt (single-driver) problem. **GUARDRAIL (accepted):**
  the release AFTER planning is UNCONDITIONALLY shared-state — a one-time reorder, NOT a precedent for
  deferring the hard bet. Honest counter (product lens): the dogfood-driven urgency reflects the team's
  own usage pattern rather than confirmed external demand → the guardrail is the mitigation.

- **v1.0.1 patch slot (2026-07-20, Miguel = OK):** mint a v1.0.1 patch = the 2 dogfood bugs (`ub-lp9.12`
  silent comment-add no-op P0; `ub-lp9.13` no-forward-migration P1) + the never-run `unblock update` smoke.
  Orthogonal to the resequence.
- **streamable-HTTP transport OUT of v1.4 (2026-07-20, Miguel = OK):** it lost its "UI enabler" justification
  (the TUI runs over stdio; other clients get direct Turso access without it) → moves to the v2+ "other
  transports (unscheduled)" bucket (thin remaining rationale: clients that can't spawn a stdio child), NO
  committed slot.
- **TUI pulled forward to v1.4 (2026-07-20, Miguel = yes):** the guardrail pins v1.3=shared-state, so the TUI
  can't sit between planning and shared-state → it becomes **v1.4** (ahead of scale), and Scale slips to v1.5.
  At v1.4 the TUI has all it needs (swarm=v1.1 FR-18/22, milestone board=v1.2 planning, data=v1.3 shared-state).

## Final resequenced order (landed — D41)
v1.0.1 patch (2 bugs + smoke) · v1.1 org/ergonomics (unchanged) · **v1.2 Planning** · **v1.3 Shared state** ·
**v1.4 Local TUI** · **v1.5 Scale & swarm depth** (streamable-HTTP removed) · v2+ (+ streamable-HTTP unscheduled).
The whole resequence = ONE new PRD §4 **D41** + the roadmap rewrite. **✅ LANDED — D41 → main `de6450e`** (2026-07-20;
00-roadmap.md §3↔§4 + §5↔§6 swaps + v1.0.1 block + matrices + §10 + D-range D1..D41 at the 3 sites; doc-lint green;
v1/v1.1 byte-identical). Kept ATOMIC (roadmap+PRD only). **✅ Derived-docs reconciliation COMPLETE — `ub-lp9.15` merged 2026-07-21 (PR #426, main `01b97ac`).**
31 files, 5 atomic commits, both gates passed, full CI green (1366 tests). Miguel chose the MAXIMAL option on all 4
scoping forks: full sweep / zero live hits / D28 rewritten in-place (not a forward-amendment marker — he rejected the
dated-record convention twice) / v1.0.1 first-class incl. a column in BOTH roadmap matrices §8+§9.
**Two brief corrections worth remembering:** (1) the "atomic PR" left the **PRD itself stale in 19 sites** (not the
~11 estimated) — the authoritative source needed fixing FIRST; (2) `docs/roadmap.html` was ALREADY correct (`78760f1`),
so that tier was empty. Open follow-ups deliberately NOT absorbed: `ub-lp9.16` (README glyph divergence vs §9, P3 —
predates D41 and is the table-rebuild option Miguel declined), `ub-lp9.17` (roadmap §1:79-80 says the comment-bug
remedy is durable storage where `ub-lp9.12` says structured error — **opposite behaviours, needs a decision**, P2),
`ub-lp9.18` (storage stability range excludes v1.0.1, P3). See [[reference-d41-doc-cascade-trap-classes]].

## SUB-QUESTIONS deferred to each version's LOCK (not now)
- **RK-3 tool budget** is FULL at 8 (spine ~:1683) → the 9th `planning` tool forces an RK-3 amendment or
  consolidation. Order-independent; surfaces one release earlier under SWAP. Settle at the planning lock.
- **Turso Sync vs embedded replicas** — bake a MANDATORY "fresh Rust-SDK/engine maturity check" gate into
  the shared-state lock (either way). May now build directly on Turso Sync.

## DOC CHANGES the SWAP forced (landed in the spec-first cascade)
00-roadmap.md: swap §3 (shared-state) ↔ §4 (planning) incl. SUBSTANTIVE "why now" rewrites (planning's
justification flips from "with shared state real" → dogfood-demand + schema-before-distribution); update §5
(v1.4 "once shared state is real" pointer — the scheduler-v2-consumes-planning dep is UNAFFECTED), §6 (v1.5
"critical path v1.2→v1.3→v1.4" + the "(v1.2 shared-state first)" parenthetical), §8 feature-matrix column
swaps, §9 crate-impact column swaps, §10 sequencing-rationale narrative, header resequence date. PRD: D28
audience text UNCHANGED (only the delivery SEQUENCE moves); MINT a NEW D-id recording the reorder + the
one-time-exception guardrail. No spine change forced by the reorder itself (Storage trait is order-agnostic).
