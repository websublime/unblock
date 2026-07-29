# Wiki index

Descriptive only — never normative (PRD > spine > crate plans is unchanged).

## Runs

- [2026-07-23-knowledge-layer-landing](runs/2026-07-23-knowledge-layer-landing.md) — The knowledge layer lands self-hosting — the spec-gate saga (two rounds, an authorized third iteration) and the one-PR landing of scaffold, lint, gate, hooks and this first report.
- [2026-07-24-acp-hook-coverage-removal](runs/2026-07-24-acp-hook-coverage-removal.md) — First live session after the knowledge-layer landing pull request — the hook-wiring canaries fired for real, branch protection was activated, and the unused ACP tool coverage was removed from the memories write-guard.
- [2026-07-24-memory-migration](runs/2026-07-24-memory-migration.md) — Migrating the Miguel-stamped subset of the assistant's private memory store into the public .knowledge/memories/ tree — the first real population of the memories layer, privacy-gated.
- [2026-07-24-knowledge-gardener-sweep](runs/2026-07-24-knowledge-gardener-sweep.md) — The inaugural knowledge-gardener sweep — the first semantic consolidation pass over .knowledge/, plus standing up the gardener runbook and its weekly reminder.
- [2026-07-29-duplicate-key-execution-flip](runs/2026-07-29-duplicate-key-execution-flip.md) — Closing the duplicate-JSON-key execution flip — a frame that read as one action executed another; three design-gate rounds, an owned scanning transport, and a fix proven against the original exploit.

## Topics

### orchestration

- [knowledge-gardener](topics/knowledge-gardener.md) — The recurring consolidation sweep over .knowledge/ — the semantic layer above the structural knowledge-lint, run periodically to keep memories and wiki pages timeless and non-contradictory.
