//! `unblock-health` (L3) — workspace health/integrity diagnostics. v1: libsql `integrity_check`
//! (rows arrive as `Vec<String>`, backend-agnostic) + a small `doctor` set. v1.1: the full
//! Healthy/Drifted/Recoverable/Unsafe taxonomy. No libsql type, no git, no network.
//! See `docs/plans/crates/unblock-health.md`.
#![forbid(unsafe_code)]
