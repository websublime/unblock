# unblock-health — L3

Workspace health/integrity. v1 (lite): libsql `integrity_check` + `doctor`; v1.1: full
Healthy/Drifted/Recoverable/Unsafe taxonomy. Receives integrity rows as `Vec<String>` and a
`&dyn Storage` handle — **never** a libsql type (NFR-15). No git (NFR-6), no network.

- **Plan (authoritative):** [`docs/plans/crates/unblock-health.md`](../../docs/plans/crates/unblock-health.md)
- **Interface SSOT:** `docs/plans/01-design-spine.md` · **Product:** `docs/PRD.md`
- **Depends on:** `model`, `error`, `sync`.
