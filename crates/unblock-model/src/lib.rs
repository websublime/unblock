//! `unblock-model` (L0) — pure domain types (no I/O, no async, no backend).
//!
//! This crate owns the shared domain vocabulary: the [`Issue`] entity and the open enums
//! ([`Status`], [`Priority`], [`IssueType`], [`DependencyType`], [`EventType`]); the relation types
//! ([`Dependency`], [`Comment`], [`Event`], [`EpicStatus`]); the canonical content-hash /
//! sync-equality / tombstone semantics; id-format parsing/validation ([`parse_id`]); the
//! [`IssueValidator`]; and the cross-crate contract/display DTOs of spine §1.10 (re-exported, never
//! redefined, by `unblock-storage`/`unblock-engine`/`unblock-render`/`unblock-config`).
//!
//! Its only in-workspace dependency is `unblock-error` (for [`unblock_error::ModelError`] as the
//! `FromStr`/validation error type), the sanctioned `model → error` L0 edge.
//!
//! # Content-hash invariant
//!
//! [`Issue::compute_content_hash`] is `#[serde(skip)]`-recomputed on load and is the import
//! idempotency key (FR-26). It hashes a **frozen** ordered, null-separated field set (spine §1.8);
//! it **excludes** `id`, the hash itself, relations, all timestamps, tombstone fields, and
//! `estimated_minutes`/`due_at`/`defer_until`/`close_reason`/`closed_by_session`. The byte stream
//! is byte-for-byte compatible with classic `bd` (it appends a frozen 17-field Go-bd zero-value
//! padding tail) and is locked by a golden snapshot.
//!
//! # Example
//!
//! ```
//! use unblock_model::{Issue, IssueValidator, Status};
//! use chrono::{TimeZone, Utc};
//!
//! let issue = Issue {
//!     id: "ub-abc123".to_string(),
//!     title: "Write the parser".to_string(),
//!     status: Status::Open,
//!     created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
//!     updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
//!     ..Issue::default()
//! };
//!
//! // Validation passes.
//! assert!(IssueValidator::validate(&issue).is_ok());
//!
//! // The content hash is deterministic, 64 lowercase hex chars, and never serialized.
//! let hash = issue.compute_content_hash();
//! assert_eq!(hash.len(), 64);
//! let json = serde_json::to_value(&issue).unwrap();
//! assert!(json.get("content_hash").is_none());
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod cache;
mod enums;
mod filters;
mod id;
mod issue;
mod output;
mod relations;
mod results;
mod serde_helpers;
mod validation;

pub use cache::CacheKey;
pub use enums::{DependencyType, EventType, IssueType, Priority, Status};
pub use filters::{CountGroupBy, ListFilters};
pub use id::{
    MAX_ID_HASH_LEN, MAX_ID_LENGTH, MAX_ID_PREFIX_LEN, ParsedId, is_valid_id_format, parse_id,
};
pub use issue::{
    Issue, MAX_SAFE_TOMBSTONE_DAYS, content_hash, content_hash_from_parts, hex_encode,
};
pub use output::OutputFormat;
pub use relations::{Comment, Dependency, EpicStatus, Event};
pub use results::{
    CloseOutcome, CountBucket, DepTree, DiagnosticFinding, DiagnosticKind, DiagnosticReport,
    ExportReport, GraphEdge, ImportReport,
};
pub use validation::{
    ACTOR_MAX_CHARS, CUSTOM_VARIANT_MAX_CHARS, ESTIMATED_MINUTES_MAX, EXTERNAL_REF_MAX_CHARS,
    ISSUE_LABEL_MAX_COUNT, IssueValidator, LABEL_MAX_LEN, LabelValidator, TITLE_MAX_CHARS,
};
