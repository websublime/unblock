# unblock-storage — L2

The `Storage` trait + the **only** backend-aware impl (libsql). Owns schema/migrations, prepared
queries, transactional mutate (rows + audit events), WAL + native `busy_timeout` (NFR-3).
`petgraph` is a **private** dep (cycle detection / dependency tree); no libsql/petgraph type in any
public signature (spine §6 rule 2). Remote/replica behind the non-default `remote` feature (D15).

- **Plan (authoritative):** [`docs/plans/crates/unblock-storage.md`](../../docs/plans/crates/unblock-storage.md)
- **Interface SSOT:** `docs/plans/01-design-spine.md` §3 · **Product:** `docs/PRD.md`
- **Depends on:** `model`, `error` only (CF-11 — NOT policy; the engine composes storage + policy).
