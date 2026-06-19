# unblock-engine — L5

The single mutation home (FR-9): a `Session` over `open → import? → mutate → export? → recover`,
serializing writes through a tokio `Semaphore(1)` (D14); reads bypass the permit (FR-10). Consumes
the `WorkspaceContext` (with `Arc<dyn Storage>`) from `unblock-config` — it does **not** build
storage (CF-D). Reads the cooperative shutdown flag; the cli installs the signal handler (OQ-4).
No backend type leaks (spine §6 rule 2).

- **Plan (authoritative):** [`docs/plans/crates/unblock-engine.md`](../../docs/plans/crates/unblock-engine.md)
- **Interface SSOT:** `docs/plans/01-design-spine.md` §4 · **Product:** `docs/PRD.md`
- **Depends on:** `config`, `sync`, `storage`, `policy`, `health`, `model`, `error`.
