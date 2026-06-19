//! `unblock-storage` (L2) — the backend-agnostic async `Storage` trait and its only
//! backend-aware implementation (libsql: schema/migrations, queries, transactional mutate,
//! WAL + native `busy_timeout` non-spin discipline, NFR-3). No libsql type crosses the public
//! API; remote/replica behind the non-default `remote` feature (D15).
//! See `docs/plans/crates/unblock-storage.md`.
#![forbid(unsafe_code)]
