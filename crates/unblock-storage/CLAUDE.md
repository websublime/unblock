# unblock-storage — L2

The `Storage` trait + its backend-aware impls, one per product mode (D51) — libsql for LOCAL mode
(this default build), SQL-over-HTTP for REMOTE mode at v1.3 (PROPOSED). Owns schema/migrations, prepared
queries, transactional mutate (rows + audit events), WAL + native `busy_timeout` (NFR-3).
`petgraph` is a **private** dep (cycle detection / dependency tree); no libsql/petgraph type in any
public signature (spine §6 rule 2). REMOTE mode is a second impl behind the non-default `remote`
feature (D15/D51); libsql stays `core`-only.

- **Plan (authoritative):** [`docs/plans/crates/unblock-storage.md`](../../docs/plans/crates/unblock-storage.md)
- **Interface SSOT:** `docs/plans/01-design-spine.md` §3 · **Product:** `docs/PRD.md`
- **Depends on:** `model`, `error` only (CF-11 — NOT policy; the engine composes storage + policy).
