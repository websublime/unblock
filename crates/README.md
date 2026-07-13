# crates — the `unblock` Rust workspace

This directory holds the multi-crate Rust workspace for **unblock**, an agent-first, **MCP-first**
issue tracker. Every domain feature is an MCP tool/resource/prompt over stdio; the CLI is
lifecycle/ops only. libsql is the source of truth; JSONL is an optional export (D5).

The single shipped binary is **`unblock`** (from `unblock-cli`); all `unblock-*` library crates are
workspace-internal (`publish = false`) and never published to crates.io.

## Crates & layering (NFR-15, acyclic, enforced)

Layer order — edges point downward only; no back-edges, no cycles:

```
L0  unblock-model | unblock-error        (pure types / error vocabulary)
L1  unblock-policy                       (pure decision contracts)
L2  unblock-storage                      (Storage trait + libsql impl)
L3  unblock-sync | unblock-health        (JSONL export/import · integrity)
L4  unblock-config                       (layered TOML + workspace-open facade)
L5  unblock-engine                       (the single mutation home — Session, D14)
L6  unblock-render                       (output formatting)
L7  unblock-mcp | unblock-cli            (rmcp stdio server · lifecycle binary)
```

The only intra-L7 edge is **`unblock-cli → unblock-mcp`** (the cli owns the binary; mcp is a
library exposing `run_mcp_server(...)`). `unblock-storage` depends on `model + error` only (the engine composes
storage + policy). `unblock-fuzz` is an unpublished member (fuzz harness over ingestion).

| Crate | Layer | Role |
|---|---|---|
| `unblock-model` | L0 | Pure domain types, `content_hash`/`sync_equals`, validation, §1.10 DTOs |
| `unblock-error` | L0 | `ErrorCode`, 0–8 exit-code table, `StructuredError`, `CodedError` |
| `unblock-policy` | L1 | Ready-sort, gating, scheduler, cache-key — pure, side-effect-free |
| `unblock-storage` | L2 | `Storage` trait + libsql impl (WAL + `busy_timeout`); remote behind a feature (D15) |
| `unblock-sync` | L3 | Light JSONL export/import + `bd` import; atomic write; path confinement |
| `unblock-health` | L3 | libsql `integrity_check` + `doctor` (full taxonomy v1.1) |
| `unblock-config` | L4 | Layered TOML, `.unblock/` discovery, builds the `Arc<dyn Storage>` (CF-D) |
| `unblock-engine` | L5 | `Session`; in-process write `Semaphore` (D14); reads bypass it (FR-10) |
| `unblock-render` | L6 | json/robot/plain/csv/markdown (TOON v1.1); stdout/stderr discipline |
| `unblock-mcp` | L7 | **Primary** surface: rmcp stdio server, 7-tool taxonomy + resources + prompts |
| `unblock-cli` | L7 | The `unblock` binary: mcp/migrate/doctor/version/init/agents/update |
| `unblock-fuzz` | — | Unpublished; `cargo-fuzz` targets over model/sync/storage ingestion |

`xtask` (repo root) is workspace tooling: `cargo xtask check-layering` enforces the matrix above.

## Authoritative docs (read before working)

- Product truth: [`docs/PRD.md`](../docs/PRD.md) · Interface SSOT: [`docs/plans/01-design-spine.md`](../docs/plans/01-design-spine.md)
- Per-crate plans: [`docs/plans/crates/`](../docs/plans/crates/) · Task DAG: [`docs/plans/implementation-plan.md`](../docs/plans/implementation-plan.md)
- Live status: [`docs/plans/STATUS.md`](../docs/plans/STATUS.md) · Workspace contract: [`CLAUDE.md`](../CLAUDE.md)

## Build

```
cargo build --workspace            # stable 1.96, edition 2024
cargo xtask check-layering         # NFR-15 acyclic-graph gate
cargo test --workspace
```
