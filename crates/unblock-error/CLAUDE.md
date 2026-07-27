# unblock-error — L0

Shared error boundary vocabulary (snafu, D4): `ErrorCode`, exit-code table, `StructuredError`,
`CodedError`, `ModelError`. Backend-agnostic; absorbs no other crate.

Also the two SHARED untrusted-input helpers (D43), homed here because BOTH JSON ingestion boundaries
(`unblock-mcp`'s argument seam and `unblock-sync`'s `bd` line parser) need them and two copies of a
security helper is drift: `dup_key::scan` — the byte-level DUPLICATE-JSON-KEY detector, which every
caller must treat fail-closed (`Indeterminate` is never `Clean`) — and `clip` +
`MAX_ECHOED_BYTES`/`TRUNCATION_MARKER`, the bound on how much attacker-controlled text an error
payload may echo. Zero new dependencies: this crate stays the deepest leaf.

- **Plan (authoritative):** [`docs/plans/crates/unblock-error.md`](../../docs/plans/crates/unblock-error.md)
- **Interface SSOT:** `docs/plans/01-design-spine.md` §2 · **Product:** `docs/PRD.md`
- **Depends on:** *(nothing internal — deepest leaf).* No internal `unblock-*` dep may be added
  (would break the acyclic L0; NFR-15, enforced by `cargo xtask check-layering`).
