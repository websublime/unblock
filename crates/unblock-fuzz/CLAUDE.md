# unblock-fuzz — (unpublished; member, NOT a default member)

Fuzz harness over model/sync/storage ingestion (NFR-16). The libFuzzer targets live in a SEPARATE
nested `fuzz/` cargo-fuzz package (own `fuzz/Cargo.toml` + `fuzz/rust-toolchain.toml` pinning nightly
only for libFuzzer codegen), `exclude`d from the workspace so the stable-1.96 default build never
pulls libFuzzer (NFR-12). Added at the fuzzing task (T0.7+); CI `fuzz-smoke` is a scheduled nightly job.

- **Plan (authoritative):** [`docs/plans/crates/unblock-fuzz.md`](../../docs/plans/crates/unblock-fuzz.md)
- **Depends on:** `model`, `sync`, `storage`, `error`.
