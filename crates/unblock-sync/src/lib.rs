//! `unblock-sync` (L3) — light JSONL export/import (D5): atomic temp+fsync+rename (NFR-4),
//! per-line validation, conflict-marker + path-confinement rejection (FR-7/FR-8/NFR-7/NFR-8),
//! and the one-shot `bd` import (FR-26). No git, no merge, no network.
//! See `docs/plans/crates/unblock-sync.md`.
#![forbid(unsafe_code)]
