# unblock-cli — L7

The reduced binary (`unblock`): lifecycle/ops only (serve/migrate/doctor/version/init/agents/update,
D3) — thin routing over the engine + config flag-forwarding + tracing + the 0–8 exit-code boundary.
Owns cooperative-shutdown signal install (FR-17, OQ-4). `unblock update` is behind the default-on
`self-update` feature (CF-K); `--no-default-features` drops the only network surface.

- **Plan (authoritative):** [`docs/plans/crates/unblock-cli.md`](../../docs/plans/crates/unblock-cli.md)
- **Interface SSOT:** `docs/plans/01-design-spine.md` §5b / §0.1 · **Product:** `docs/PRD.md`
- **Depends on:** `engine`, `render`, `policy`, `mcp`, `error` (the settled `cli → mcp` edge, §0.1).
