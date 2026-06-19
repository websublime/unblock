//! `unblock-policy` (L1) — pure, versioned decision contracts: ready/hybrid-sort ranking,
//! dependency→ready gating, validation/inheritance helpers, cache-key contract. Side-effect-free;
//! no storage, no I/O, no `petgraph`. See `docs/plans/crates/unblock-policy.md`.
#![forbid(unsafe_code)]
