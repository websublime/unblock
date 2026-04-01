//! # unblock-core
//!
//! Domain types, dependency graph engine, and cache layer for the unblock system.
//!
//! This crate is pure Rust with zero network dependencies. It provides:
//!
//! - **Types** — `Issue`, `Status`, `Priority`, `ReadyState`, `BlockingEdge`, `BodySections`
//! - **Graph** — petgraph-based dependency graph with ready set computation and cascade
//! - **Cache** — in-memory graph cache with TTL and invalidation
//! - **Config** — environment-based configuration
//! - **Errors** — domain error types with snafu
//!
//! # Re-exports
//!
//! [`Arc`] is re-exported for convenience because [`cache::GraphCache`] accessor
//! methods (`get_ready_set`, `get_graph`) return `Arc`-wrapped values. Callers
//! can import it directly from this crate instead of pulling in `std::sync::Arc`
//! separately.

/// [`std::sync::Arc`] re-exported for convenience.
///
/// Cache accessor methods such as [`cache::GraphCache::get_ready_set`] and
/// [`cache::GraphCache::get_graph`] return `Arc`-wrapped values. This re-export
/// allows downstream crates to use `unblock_core::Arc` without an additional
/// `std::sync` import.
pub use std::sync::Arc;

/// Domain types: `Issue`, `IssueComment`, `RelatedIssue`, `Status`, `Priority`, `ReadyState`, `BlockingEdge`, `BodySections`.
pub mod types;

/// Dependency graph engine: build, ready set, cascade, cycle detection.
pub mod graph;

/// In-memory graph cache with TTL and invalidation.
pub mod cache;

/// Environment-based configuration.
pub mod config;

/// Domain error types with HTTP status code mapping.
pub mod errors;

/// Reconciliation drift types: [`reconcile::DriftKind`] and [`reconcile::DriftReport`].
pub mod reconcile;
