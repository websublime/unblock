//! `unblock-model` (L0) — pure domain types (no I/O): `Issue` + open enums, `Dependency`,
//! `Comment`, `Event`; `content_hash` / `sync_equals` / tombstone logic; `IssueValidator`;
//! and the shared §1.10 contract/display DTOs re-exported by storage/engine.
//! See `docs/plans/crates/unblock-model.md`.
#![forbid(unsafe_code)]
