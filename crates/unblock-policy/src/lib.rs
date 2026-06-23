//! `unblock-policy` (L1) — pure, side-effect-free, versioned **decision contracts** consumed by
//! `unblock-engine` (L5) and exposed read-only through `unblock-mcp` (L7).
//!
//! This crate is a pure leaf above `unblock-model` / `unblock-error` (its **only** dependencies —
//! no storage, no config, no `tokio`, no I/O, no `petgraph`; spine §0 / PRD §8.1 / NFR-15). Every
//! input is caller-supplied plain data and every output is plain data; nothing here touches a clock
//! or a database (functions that need a "now" take a `now: DateTime<Utc>` parameter). The crate is
//! `#![forbid(unsafe_code)]` and `#![warn(missing_docs)]`, clippy-pedantic clean.
//!
//! # v1 surface (M1 walking skeleton)
//!
//! - **Ready ranking** — the pinned hybrid comparator [`cmp_ready`] / [`ready_sort_key`] /
//!   [`ready_hybrid_bucket`] (P0/P1 in bucket `0`, P2..P4 in bucket `1`; then `created_at` ASC,
//!   then `id` ASC — byte-faithful to the original `sort_ready_hybrid`, spine §4.1 NORMATIVE), plus
//!   the [`ReadyBucket`] / [`ReadySortKey`] keys.
//! - **Ready/blocked predicate** — [`is_ready`] / [`is_blocked`] over a [`ReadyContext`] of
//!   incoming [`BlockingEdge`]s, returning a [`ReadyVerdict`]; [`READY_GATING_TYPES`] is the four
//!   gating dependency types.
//! - **Cache-key minting** — [`cache_key_ready`] / [`cache_key_blocked`] over a canonical,
//!   order-independent [`filters_fingerprint`].
//! - **Contract versioning** — the [`Contract`] trait + [`ContractEnvelope`] +
//!   [`contract_versions`] + [`POLICY_CONTRACT_VERSION`].
//! - **Inheritance** — the minimal infallible [`select_inherited_blocks`] bookend selector.
//! - **Errors** — [`PolicyError`] (a v1 `Internal`-only forward-compat seam implementing
//!   `unblock_error::CodedError`).
//!
//! The scheduler/coordination/gate/saved-query/cache contracts are v1.1/v1.3 and are **not** part
//! of this v1 surface (see `docs/plans/crates/unblock-policy.md`).
//!
//! # Example
//!
//! ```
//! use unblock_policy::{cmp_ready, is_ready, ReadyContext, ReadyVerdict};
//! use unblock_model::{Issue, Priority, Status};
//! use chrono::{TimeZone, Utc};
//! use std::cmp::Ordering;
//!
//! // Hybrid ready sort: a P1 (bucket 0) outranks an older P2 (bucket 1).
//! let p1 = Issue { id: "ub-a".into(), priority: Priority::HIGH,
//!     created_at: Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap(), ..Issue::default() };
//! let p2 = Issue { id: "ub-b".into(), priority: Priority::MEDIUM,
//!     created_at: Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(), ..Issue::default() };
//! assert_eq!(cmp_ready(&p1, &p2), Ordering::Less);
//!
//! // An open issue with no blockers and no future deferral is ready.
//! let now = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
//! let ctx = ReadyContext { status: Status::Open, defer_until: None,
//!     incoming_blocking: vec![], now };
//! assert_eq!(is_ready(&ctx), ReadyVerdict::Ready);
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod cache_key;
mod contract;
mod error;
mod inheritance;
mod ready;

#[cfg(any(test, feature = "proptest-support"))]
pub mod proptest_support;

pub use cache_key::{cache_key_blocked, cache_key_ready, filters_fingerprint};
pub use contract::{Contract, ContractEnvelope, POLICY_CONTRACT_VERSION, contract_versions};
pub use error::PolicyError;
pub use inheritance::{AncestorNode, InheritanceConfig, InheritedBlock, select_inherited_blocks};
pub use ready::{
    BlockingEdge, READY_GATING_TYPES, ReadyBucket, ReadyContext, ReadySortKey, ReadyVerdict,
    cmp_ready, is_blocked, is_ready, ready_hybrid_bucket, ready_sort_key,
};
