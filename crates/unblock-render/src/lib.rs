//! `unblock-render` (L6) — output formatting behind a single object-safe [`Renderer`] trait.
//!
//! Turns engine/domain result types into byte-stable serialized output in five formats —
//! `json` / `robot` / `plain` / `csv` / `markdown` (`toon` is feature-gated, v1.1). The crate is
//! **model + error only** (NFR-15): the §1.10 display DTOs live in `unblock-model` and are
//! re-exported, never redefined.
//!
//! Properties guaranteed by this crate:
//! - **No I/O** (NFR-14): every method returns a [`RenderOutput`] (`String` + [`ContentType`]); the
//!   caller owns the stdout/stderr split and any file write. The crate never touches a stream.
//! - **Byte-deterministic** for a fixed input + fixed [`RenderOptions`]: caller `Vec` order is
//!   preserved (never re-sorted), maps use ordered iteration, and every timestamp goes through the
//!   single [`fmt_ts`] helper.
//! - **Untrusted-string hygiene** (NFR-18): `plain`/`markdown`/`csv` route every user-controlled
//!   string — including open-enum `Custom` labels, embedded ids, and every `StructuredError`
//!   context value — through [`sanitize_inline`] before embedding; `json`/`robot` rely on serde
//!   escaping.
//! - **Always-valid JSON even on error** (FR-11): `structured_error` on the JSON backend serializes
//!   the `StructuredError` payload directly.
//!
//! # Example
//!
//! ```
//! use unblock_render::{renderer_for, OutputFormat, RenderOptions};
//! use unblock_model::Issue;
//! use chrono::{TimeZone, Utc};
//!
//! let issue = Issue {
//!     id: "ub-abc123".to_string(),
//!     title: "Write the docs".to_string(),
//!     created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
//!     updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
//!     ..Issue::default()
//! };
//!
//! let renderer = renderer_for(OutputFormat::Json, RenderOptions::default());
//! let out = renderer.issues(std::slice::from_ref(&issue), &RenderOptions::default()).unwrap();
//!
//! // The JSON parses back to the same issue list.
//! let parsed: Vec<Issue> = serde_json::from_str(&out.stdout).unwrap();
//! assert_eq!(parsed.len(), 1);
//! assert_eq!(parsed[0].id, "ub-abc123");
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod backend;
mod error;
mod format;
mod options;
mod renderer;
mod sanitize;

pub use error::RenderError;
pub use format::{
    FormatName, OutputFormat, OutputFormatExt, fmt_ts, format_as_str, parse_env_value,
    parse_format, pick_format,
};
pub use options::{ContentType, RenderOptions, RenderOutput};
pub use renderer::{Renderer, renderer_for};
pub use sanitize::{sanitize_inline, sanitize_text};
