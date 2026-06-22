# unblock-fuzz — (unpublished; member, NOT a default member)

Fuzz harness over model/error/storage ingestion (NFR-16). All target logic lives in `src/` as
`run_<t>_case(&[u8]) -> Result<(), FuzzError>` **cores** (stable Rust); the libFuzzer entry points are
5-line wrappers in a SEPARATE nested `fuzz/` cargo-fuzz package (own `fuzz/Cargo.toml` +
`fuzz/rust-toolchain.toml` pinning `nightly-2024-10-31` only for libFuzzer codegen), `exclude`d from
the workspace so the stable-1.96 default build never pulls libFuzzer (NFR-12). The stable gate is
`cargo test` (`tests/regression.rs` replays the committed corpus + a `proptest!` smoke per target);
CI `fuzz-smoke` is a scheduled nightly job.

- **Plan (authoritative):** [`docs/plans/crates/unblock-fuzz.md`](../../docs/plans/crates/unblock-fuzz.md)
- **Depends on (T0.7):** `model`, `error`, `storage` (with the `testkit` seam). `sync` is dropped at
  T0.7 — the JSONL/`bd` targets are post-T0.7.
- **T0.7 targets:** model+error `{content_hash, issue_ingest, parse_id, enum_deserialize, sanitize}` +
  storage `{query_filters, cycle_detect, id_alloc}`.
