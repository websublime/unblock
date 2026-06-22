# unblock-fuzz — nested cargo-fuzz package

This directory is a **separate cargo-fuzz package, OUTSIDE the workspace** (excluded in the root
`Cargo.toml`). It holds only the libFuzzer entry points; all target logic lives in the `unblock-fuzz`
**member crate** (`../src/`) as `run_<t>_case(&[u8]) -> Result<(), FuzzError>` cores. Each
`fuzz_targets/<t>.rs` is a 5-line `fuzz_target!` wrapper over its core.

## Why it is its own package (and not locally buildable on the stable gate)

libFuzzer needs **nightly** sanitizer codegen, which must never reach the stable-1.96 default build
(NFR-12). So this package:

- is `exclude`d from the workspace (the root `cargo build --workspace` never compiles
  `libfuzzer-sys`/`arbitrary`);
- pins its toolchain in `rust-toolchain.toml` (`nightly-2024-10-31`), **scoped to this directory
  only** — the workspace root stays on stable `1.96.0`.

The **stable PR gate** is `cargo test` in the member crate (`../`): `tests/regression.rs` replays the
committed `corpus/` through the cores and runs a `proptest!` smoke per target. New crash artifacts get
their input committed under `corpus/<target>/` so the stable gate guards against regressions **without
the libFuzzer toolchain**.

## Install + run

```sh
cargo install cargo-fuzz                 # once
rustup toolchain install nightly-2024-10-31 --component rust-src

# Run a target (the scoped rust-toolchain.toml selects the nightly automatically inside fuzz/):
cargo +nightly-2024-10-31 fuzz run content_hash
cargo +nightly-2024-10-31 fuzz run content_hash -- -max_total_time=60   # timed smoke

# Coverage (periodic corpus-quality review; not gated):
cargo +nightly-2024-10-31 fuzz coverage content_hash
```

## Targets and what each proves

### model + error (L0)

| Target | Proves |
|---|---|
| `content_hash` | `compute_content_hash` is total, deterministic, **transport-independent** (compact/pretty/reordered/CRLF JSON round-trip preserves the hash), and field-scoped (spine §1.8). |
| `issue_ingest` | `serde_json::from_slice::<Issue>` over arbitrary bytes never panics; a surviving issue survives the full read-side surface (validate / hash / `sync_equals` / tombstone TTL). |
| `parse_id` | `parse_id` / `is_valid_id_format` over arbitrary UTF-8 never panic and **agree**; a parsed id re-parses to itself. |
| `enum_deserialize` | the hand-rolled open-enum `Deserialize` (Status/IssueType/DependencyType/EventType) never panics and **round-trips** its wire form. |
| `sanitize` | `unblock_error::sanitize_message` is total, leaks **no raw terminal-control byte**, and is idempotent; `find_similar_ids` stays bounded (NFR-14). |

### storage (L2)

| Target | Proves |
|---|---|
| `query_filters` | `list/ready/blocked/search/count/stale` never panic under a fuzzed `ListFilters`; results are a subset of the seeded ids; `ready`/`blocked` are disjoint; the grouped count buckets stay sum-consistent. |
| `cycle_detect` | the cycle detector **always terminates** on a fuzzer-built dependency graph; `add_dependency` never lets a gating cycle through the public path; a cycle planted via the testkit seam is detected. |
| `id_alloc` | the id child-counter high-water mark advances **monotonically** past the children created. |

The JSONL/`bd`/sync targets are **post-T0.7** (they need `unblock-sync`, which the member crate does
not yet depend on). See `docs/plans/crates/unblock-fuzz.md`.
