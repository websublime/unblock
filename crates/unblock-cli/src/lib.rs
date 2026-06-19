//! `unblock-cli` (L7) — the reduced lifecycle/ops CLI facade over the engine: `serve`, `migrate`,
//! `doctor`, `version`, `init`, `agents`, `update` (D3). Thin routing + config flag-forwarding +
//! tracing + the 0–8 exit-code boundary. Owns cooperative-shutdown signal install (FR-17, OQ-4).
//! The `unblock` binary entry point is `src/main.rs`. See `docs/plans/crates/unblock-cli.md`.
#![forbid(unsafe_code)]
