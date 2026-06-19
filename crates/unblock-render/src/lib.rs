//! `unblock-render` (L6) — output formatting (json/robot/plain/csv/markdown; TOON feature-gated
//! at v1.1) behind a `Renderer` trait. Structured output to stdout, diagnostics to stderr
//! (NFR-14); byte-deterministic, snapshot-stable; always-valid-JSON even on error (FR-11).
//! See `docs/plans/crates/unblock-render.md`.
#![forbid(unsafe_code)]
