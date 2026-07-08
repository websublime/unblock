# unblock-health — L3

Workspace health/integrity. v1 (lite, T3.3/D29): libsql `integrity_check` + file-state `doctor`;
v1.1: full Healthy/Drifted/Recoverable/Unsafe taxonomy + `--repair` + `.unblock/.recovery/` evidence.
**Storage-free** (F3/D29): the engine calls `Session::integrity_check()` and passes the integrity
rows in as `Vec<String>` — health holds NO `Storage` handle and **never** sees a libsql type
(NFR-15). No git (NFR-6), no network.

- **Plan (authoritative):** [`docs/plans/crates/unblock-health.md`](../../docs/plans/crates/unblock-health.md)
- **Interface SSOT:** `docs/plans/01-design-spine.md` · **Product:** `docs/PRD.md`
- **Depends on:** `model`, `error`, `sync`.
