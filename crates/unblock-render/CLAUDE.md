# unblock-render — L6

Output formatting (json/robot/plain/csv/markdown; TOON feature-gated, v1.1) behind a `Renderer`
trait. Structured stdout / diagnostics stderr (NFR-14); byte-deterministic; always-valid-JSON on
error (FR-11). Stays **model + error only** — the §1.10 display DTOs live in `unblock-model`.

- **Plan (authoritative):** [`docs/plans/crates/unblock-render.md`](../../docs/plans/crates/unblock-render.md)
- **Interface SSOT:** `docs/plans/01-design-spine.md` §1.10 · **Product:** `docs/PRD.md`
- **Depends on:** `model`, `error` only (back-edges to engine/mcp/cli are forbidden — NFR-15).
