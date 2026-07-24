---
name: project-knowledge-layer-docs-in-db-exploration
description: Exploring a docs-in-DB "planner" end-state for unblock (2026-07-08) — staged proposal, resolved objections, spike results; the roadmap §7 candidate row exists and the file-based .knowledge/ precursor has landed
type: reference
---

Exploring evolving unblock from issue tracker into a full planner where documents
(PRD/specs/plans, doc types) live in the DB with issues linking directly (2026-07-08 session).
**Formalized:** the roadmap now carries a `§7` "Docs-in-DB process-knowledge storage" v2+ candidate row
(`00-roadmap.md`), and the file-based `.knowledge/` precursor (memories + wiki run-reports/topics) has LANDED
(PROCESS.md §8) — see [[project-dogfood-unblock-is-the-tracker]]. The Stage-1 fork below (a native DB-backed
`Document` entity) still awaits its lock, deferred to whichever version takes up the roadmap candidate.

State of the debate:
- Agreed: attachments are NOT embedded in the DB — sidecar CAS (`.unblock/attachments/` by hash, metadata in DB).
- Successfully rebutted the "git PR review / CI doc-lint" objection: review/lint are workflows, not
  intrinsic to docs — in-DB equivalents are FR-19 gates + FR-6 comments + engine-enforced invariants (typed
  links can't dangle; FTS queries replace grep-lint). unblock is an agnostic product; this repo is just one usage.
- Residual hard core: (1) concurrent long-form text editing (revision field gives history, not merge);
  (2) docs that must version atomically WITH code (spine-class) — honest boundary, stays in git;
  (3) blobs (resolved via sidecar).
- Spike (scratchpad merge-spike, 2026-07-08) PROVED: `diffy` 3-way merge + `similar` unified diff, pure Rust,
  ~15 lines: concurrent edits to different sections auto-merge cleanly on stale base_rev; same-line edits
  produce explicit conflict markers (no lost intent); engine can serve review diffs between any revs.
  No git, no D13 violation (D13 bans git ops/lib, not diff algorithms).
- A prior-art link (git-diff-sqlite3): textconv `.dump` trick = read-only diff of a DB committed to git —
  wrong layer for unblock (we don't commit the DB); its principle = deterministic text projection, which
  unblock already institutionalizes as JSONL export (D5); generalizes to a `docs export` git-diffable bridge.
- Full-circle precedent: classic bd was built on Dolt ("git for data", row-level branch/merge) and the
  beads_rust→unblock lineage dropped that architecture (PRD §11). Lesson: don't rebuild a VCS under the DB;
  add diff/3-way/patch algorithms above it.

Staged proposal (sequencing maintained):
- Stage 1 (fork at v1.3 lock): doc registry — files as truth, DB as index: `Document` entity (path,
  doc_type open enum, content_hash, anchors) + typed issue↔doc link table (v1.3 Goal link-table pattern),
  staleness via hash mismatch, MCP resource serving content read-only, derived rebuildable FTS.
- Stage 2 (v2-ish, needs v1.2 shared DB + human surface in sight): native Document entity — revision chain,
  optimistic concurrency w/ 3-way auto-merge, draft/review/approved states riding FR-19+FR-6,
  **Change Requests as entities** (proposed revision = patch w/ states+gates; apply = 3-way merge) = in-DB PR.
- Stage 3 (later maturity stage, Turso Sync era): block-granular model and/or CRDT (Automerge/Loro/yrs) for
  real-time human co-editing; markdown stays the storage format until then (sections derived via parsing, not
  persisted).
- Doc process migrates per doc class as features mature: notes/decisions first, PRD later, spine last/never.

Extension (2026-07-22) — an OpenKB/PageIndex-inspired COMPILED-wiki service idea:
rewrite PageIndex (tree index, vectorless LLM retrieval; Python, MIT, core is genuinely small) + a
flattened OpenKB (Python+TS, Apache-2.0, 3.1k★; its "broad format support" is actually Microsoft's
markitdown dep) in Rust, LLM bundled via ollama in a Docker (rig = candidate framework, MIT, v0.40).
Assessment: fits as the DERIVED read/compile half (authored layer = truth, wiki+tree = rebuildable
derivations); must be a SIBLING service/repo — unblock core stays LLM-free (D13/determinism); Stage-1
content_hash registry doubles as the incremental-recompile change-feed; for agent clients the retrieval
LLM is optional (expose the tree over MCP, the client IS the LLM — local LLM only for compile + human
TUI query). Flatten formats first (markdown-first; PDF/multimodal is the swamp), not just features.
Google OKF v0.1 (markdown + YAML frontmatter, `type` required; Karpathy LLM-wiki-inspired; repo
GoogleCloudPlatform/knowledge-catalog) is the cheapest high-value piece: adopted as the format for
`.knowledge/` (the .knowledge layer epic, unblock issue `ub-knowledge-layer-e4s` — the file-based precursor has
landed; query the live tracker for the epic's current subtask state) and intended later as the doc-type format
of the DB knowledge layer; interop by FORMAT, not code parity with a moving upstream.

End-state vision sketch (2026-07-22) — a high-level final-product picture:
- CLI: init/tracker(know=new)/migrate/agents/update/version/ui(tui) — i.e. today's `mcp` command
  (D32 renamed serve→mcp PRE-GA; bare `serve` verb is RESERVED for the v2+ web server; current D3 set =
  mcp/migrate/doctor/version/init/agents/update, cli.rs:95) eventually splits into `tracker` + `know`
  (two MCP servers, one binary). Post-GA rename is breaking (D35) → recommended: additive `know` now-ish,
  `tracker` added later with `mcp` kept as invocable clap alias, canonical rename only at a 2.0; `ui` vs
  ratified `tui` naming resolved at v1.5 lock.
- `.unblock/knowledge/` = memories/ + wiki/ as FILES in the workspace. Semantics: memories/ =
  the agent memory that today lives in the harness home (`~/.claude/projects/<slug>/memory/`) MOVED to the
  project workspace (project-scoped, shared, MCP-served to any agent); wiki/ = knowledge provided directly
  plus knowledge acquired/distilled over time. Pipeline: raw per-fact memories → LLM consolidation (document
  service batch job) → curated wiki pages. Format: index.md + file-per-fact + OKF frontmatter.
- Services timeline: document service (kb compiler sibling) + libsqld service (v1.2) as separate boxes.
- Open design points for the spec: memory scope/privacy (personal vs project), multi-agent consolidation
  ownership, and disjoint write-domains (know→files, tracker→DB) so two MCP servers don't double-write.

Related: [[project-unblock-rust-rewrite]].
