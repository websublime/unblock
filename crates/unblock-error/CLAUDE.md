# unblock-error — L0

Shared error boundary vocabulary (snafu, D4): `ErrorCode`, exit-code table, `StructuredError`,
`CodedError`, `ModelError`. Backend-agnostic; absorbs no other crate.

- **Plan (authoritative):** [`docs/plans/crates/unblock-error.md`](../../docs/plans/crates/unblock-error.md)
- **Interface SSOT:** `docs/plans/01-design-spine.md` §2 · **Product:** `docs/PRD.md`
- **Depends on:** *(nothing internal — deepest leaf).* No internal `unblock-*` dep may be added
  (would break the acyclic L0; NFR-15, enforced by `cargo xtask check-layering`).
