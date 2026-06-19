# unblock-sync — L3

Light JSONL export/import (D5): atomic write (NFR-4), per-line validation, conflict-marker +
path-confinement rejection (FR-7/FR-8/NFR-7/NFR-8), one-shot `bd` import (FR-26). No git, no merge,
no network; consumes only the `Storage` trait + model types (no libsql type crosses in).

- **Plan (authoritative):** [`docs/plans/crates/unblock-sync.md`](../../docs/plans/crates/unblock-sync.md)
- **Interface SSOT:** `docs/plans/01-design-spine.md` · **Product:** `docs/PRD.md`
- **Depends on:** `storage`, `model`, `error`.
