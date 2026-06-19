//! `unblock-engine` (L5) — the single mutation home (FR-9). A `Session` that consumes the
//! `WorkspaceContext` built by `unblock-config` (CF-D) and composes storage + policy (+ optional
//! sync/health) over one lifecycle, serializing in-process writes through a tokio `Semaphore(1)`
//! (D14) while reads bypass it (FR-10). Cooperative shutdown reads a flag installed by the cli
//! (OQ-4). No libsql/backend type crosses this boundary. See `docs/plans/crates/unblock-engine.md`.
#![forbid(unsafe_code)]
