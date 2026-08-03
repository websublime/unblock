# Wiki index

Descriptive only — never normative (PRD > spine > crate plans is unchanged).

## Runs

- [2026-07-23-knowledge-layer-landing](runs/2026-07-23-knowledge-layer-landing.md) — The knowledge layer lands self-hosting — the spec-gate saga (two rounds, an authorized third iteration) and the one-PR landing of scaffold, lint, gate, hooks and this first report.
- [2026-07-24-acp-hook-coverage-removal](runs/2026-07-24-acp-hook-coverage-removal.md) — First live session after the knowledge-layer landing pull request — the hook-wiring canaries fired for real, branch protection was activated, and the unused ACP tool coverage was removed from the memories write-guard.
- [2026-07-24-memory-migration](runs/2026-07-24-memory-migration.md) — Migrating the Miguel-stamped subset of the assistant's private memory store into the public .knowledge/memories/ tree — the first real population of the memories layer, privacy-gated.
- [2026-07-24-knowledge-gardener-sweep](runs/2026-07-24-knowledge-gardener-sweep.md) — The inaugural knowledge-gardener sweep — the first semantic consolidation pass over .knowledge/, plus standing up the gardener runbook and its weekly reminder.
- [2026-07-29-duplicate-key-execution-flip](runs/2026-07-29-duplicate-key-execution-flip.md) — Closing the duplicate-JSON-key execution flip — a frame that read as one action executed another; three design-gate rounds, an owned scanning transport, and a fix proven against the original exploit.
- [2026-07-31-create-deps-atomicity](runs/2026-07-31-create-deps-atomicity.md) — Making a create with declared dependencies one indivisible act — a silent third-party graph corruption nobody had recorded, four spec-repair rounds against an unbounded prose sweep, and a mutation pass that caught a structurally unfailable assertion.
- [2026-08-01-dangling-blocker-spec](runs/2026-08-01-dangling-blocker-spec.md) — Specifying the dangling dependency-target guard (decision D45) — a design gate that failed TWICE (eleven blocking findings, then ten more), two Miguel rulings that first reversed the exporter repair into a corpus widening and then forced that widening to follow incoming edges as well, and two open questions closed rather than shipped half-open.
- [2026-08-01-dangling-blocker-impl](runs/2026-08-01-dangling-blocker-impl.md) — Implementing the dangling dependency-target guard (decision D45) — three parallel implementers behind a design gate that had already failed twice, seventeen mutation kills an independent lens reproduced one by one, three claimed kills honestly declared equivalent instead, the two costs the specification refused to accept an opinion about, and a second round in which the 250k scale gate rejected one of those costs and the listing view was amended into a single SQL query.
- [2026-08-03-comments-forward-migration](runs/2026-08-03-comments-forward-migration.md) — Repairing the missing forward migration for the comments columns (decision D46) — a database no shipped binary could read, an issue whose own proposed fix provably could not work, five design-gate rounds, and a Verify gate that caught the repair's own error message lying about the state it names.

## Topics

### orchestration

- [knowledge-gardener](topics/knowledge-gardener.md) — The recurring consolidation sweep over .knowledge/ — the semantic layer above the structural knowledge-lint, run periodically to keep memories and wiki pages timeless and non-contradictory.
