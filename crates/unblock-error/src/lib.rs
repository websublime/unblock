//! `unblock-error` (L0) — shared error vocabulary: `ErrorCode`, the 0–8 exit-code table,
//! `StructuredError`, the `CodedError` bridge, and `ModelError`. The deepest leaf: it has
//! **no** in-workspace dependencies. See `docs/plans/crates/unblock-error.md`.
#![forbid(unsafe_code)]
