---
name: 2026-07-24-memory-migration
description: Migrating the Miguel-stamped subset of the assistant's private memory store into the public .knowledge/memories/ tree — the first real population of the memories layer, privacy-gated.
type: run
date: 2026-07-24
branch: e4s.3-memory-migration
pr: -
issues: [ub-knowledge-layer-e4s.3]
---

# Run — populate .knowledge/memories/ from the triaged private store

## Context

Task ub-knowledge-layer-e4s.3 (an unblock task id; unblock is this repo's MCP issue tracker) — the third
child of the `.knowledge` layer epic (unblock issue ub-knowledge-layer-e4s). Its dependency (the scaffold,
task .2) landed with the knowledge-layer PR, and the privacy-triage stamp was already recorded on the
task, so this task was unblocked. This is the FIRST real population of `.knowledge/memories/`, which until
now held only its index skeleton. Branch `e4s.3-memory-migration`, worked in an isolated worktree off the
current `main`. Both required gates were met: the Decide was the maintainer's stamped privacy triage plus
his live decisions this session on the borderline items; the Verify quality gate (three adversarial
lenses — a privacy-leak hunt, schema/lint, and content fidelity — plus a coordinator) returned PASS with
the privacy verdict CLEAR.

## What & why

The assistant's private memory store held ~54 atomic memories. Per the stamped triage plus the
maintainer's decisions this session, a small number are HELD PRIVATE and do NOT enter the public repo;
their itemized list stays out of this public record (the epic's public-repo rule), so this report records
only their aggregate character — redundant-with-public-docs, personal/machine-scoped tooling, an
out-of-scope strategic idea, an internal tracking note, and go-to-market/commercial framing. The remaining
49 memories were migrated.

Because this is a PUBLIC repository, a privacy leak was treated as the single worst outcome. The migration
is a TRANSFORM, not a copy: each memory's frontmatter is reshaped to the `.knowledge` memory schema (flat,
exactly `name`/`description`/`type`, with `type` drawn from the descriptive set gotcha / recipe /
reference / environment); slugs are kebab-cased and the `name` field is made identical to the filename
stem; the memories index is rebuilt so each entry equals its file's description and type. Privacy scrubs
applied during the transform: personal absolute paths, session identifiers and session-scoped metadata
removed; a private personal tool's name and an internal agent codename genericized; every cross-link that
pointed at a held-private memory removed (repointed to the public epic issue id in plain text where the
referent was still needed), and every dangling link to a non-migrated memory dropped, so no surviving
`[[wikilink]]` is broken; two memories neutrally reformulated for a public audience; and one memory's
roadmap-iteration record softened of competitive/commercial editorializing. A follow-up polish round then
genericized deprecated-stack (Go/Encore predecessor) internal incident details in one memory to the plain
technical lesson, removed a personal informal quote from another, and fixed a cosmetic token that read as
a broken link.

Exclusion was enforced by construction: the orchestrator staged only the 49 migrating files as the
implementer's source, so a held-private item could not be picked up by mistake. The implementer ran in an
isolated worktree; the orchestrator then independently re-ran the leak-scans and the lint rather than
trusting the self-report, and the Verify gate did an adversarial third pass.

## Outcome

49 memories plus a rebuilt index landed on the branch; `.knowledge/memories/` is populated for the first
time. `cargo xtask knowledge-lint` is green (all structural checks clean) and every privacy leak-scan
comes back empty (no personal paths, no session identifiers, no private-tool or private-idea names, no
commercial framing, none of the held-private items present or referenced). The epic's one remaining child
after this is the knowledge gardener (task .5, a recurring consolidation sweep).

## Gotchas

- The `.knowledge` memory `type` set (gotcha / recipe / reference / environment) is deliberately
  descriptive and does NOT match the private store's own categories, so migrating requires a per-file type
  mapping — a lesson/pitfall becomes `gotcha`, a how-to becomes `recipe`, a durable fact becomes
  `reference`, a harness/setup fact becomes `environment`.
- The memories index is checked as data: each entry must be exactly `- [slug](slug.md) ` + the type in
  backticks + ` — ` + a one-liner that is byte-identical to the file's frontmatter description, and the
  backtick type must equal the file's type. Changing a description means changing its index entry in
  lockstep.
- Public-repo hygiene needed a second pass even after a clean main transform: an old deprecated-stack
  incident write-up carried internal deployment specifics, and a status memory carried a personal informal
  quote — both are the kind of thing a grep for the obvious private tokens does NOT catch, so the fidelity
  lens reading the actual prose is what surfaced them.
- The held-private itemization must stay out of BOTH the public issue comments and this public report;
  naming which items are private, in a public artifact, is itself the leak the exclusion exists to prevent.

## Glossary

No session-local id codes (short mutation-testing or must-fix labels — an uppercase letter immediately
followed by a number) appear in this report. The table records the durable references it leans on.

| id | what it is (in words) | where it lives |
|----|-----------------------|----------------|
| ub-knowledge-layer-e4s | the `.knowledge` layer epic (memories + wiki + day-1 enforcement) | the unblock tracker; git record `.unblock/issues.jsonl` |
| ub-knowledge-layer-e4s.3 | this task — migrate the triaged private memories into `.knowledge/memories/` | the unblock tracker; git record `.unblock/issues.jsonl` |
| ub-knowledge-layer-e4s.5 | the epic's remaining child — the recurring knowledge gardener sweep | the unblock tracker |
| memory types | the four canonical `.knowledge` memory `type` values | `xtask/src/knowledge_lint.rs` (`MEMORY_TYPES`) |
| PR #428 | the pull request that landed the `.knowledge` layer scaffold + enforcement | GitHub pull request #428 |

## Links

- ub-knowledge-layer-e4s.3 — the migration task; its comment thread carries the per-phase narrative
  (Understand + Decide + Implement-start; the Verify verdict and Track/merge follow).
- ub-knowledge-layer-e4s — the parent epic; on this merge only the gardener child remains.
- `.knowledge/memories/index.md` — the rebuilt curated index (49 entries).
- `xtask/src/knowledge_lint.rs` — the k1..k6 checks the migrated tree satisfies.
- Pull request: opened by the orchestrator after this Implement + Verify; not yet created at report time.
- Prior run-reports: `.knowledge/wiki/runs/2026-07-23-knowledge-layer-landing.md` and
  `.knowledge/wiki/runs/2026-07-24-acp-hook-coverage-removal.md`.
