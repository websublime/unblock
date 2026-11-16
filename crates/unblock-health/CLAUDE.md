# unblock-health — L3

Workspace health classification (D58, FR-16 full): the four levels Healthy/Drifted/Recoverable/Unsafe
defined by repair path, the D58 class table, the planned repairs, and the `.unblock/.recovery/`
evidence layout. **Pure, non-async, storage-free** (D29 clause F3, kept by D58 clause (4)): the engine
gathers `PreOpenFacts`, `DbFacts` and `ExportFacts` and passes them to `classify` as data — health
holds NO `Storage` handle, has no `unblock-storage` dependency and **never** sees a libsql type
(NFR-15). The export scan is `unblock-sync`'s `scan_conflict_markers` (health has no scanner of its
own). No tokio, no chrono, no git (NFR-6), no network.

- **Plan (authoritative):** [`docs/plans/crates/unblock-health.md`](../../docs/plans/crates/unblock-health.md)
- **Interface SSOT:** `docs/plans/01-design-spine.md` · **Product:** `docs/PRD.md`
- **Depends on:** `model`, `error`, `sync`.
