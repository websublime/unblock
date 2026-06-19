# unblock-model — L0

Pure domain types, no I/O: `Issue` + open enums, relations, `content_hash`/`sync_equals`/tombstone,
`IssueValidator`, and the §1.10 contract/display DTOs (re-exported by storage & engine — never redefined).

- **Plan (authoritative):** [`docs/plans/crates/unblock-model.md`](../../docs/plans/crates/unblock-model.md)
- **Interface SSOT:** `docs/plans/01-design-spine.md` §1 / §1.10 · **Product:** `docs/PRD.md`
- **Depends on:** `error` only (the sanctioned `model → error` L0 edge, CF-G). No edge above L0.
