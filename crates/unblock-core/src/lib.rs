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

/// Domain types: `Issue`, `Status`, `Priority`, `ReadyState`, `BlockingEdge`, `BodySections`.
pub mod types;

/// Dependency graph engine: build, ready set, cascade, cycle detection.
pub mod graph;

/// In-memory graph cache with TTL and invalidation.
pub mod cache;

/// Environment-based configuration.
pub mod config;

/// Domain error types.
pub mod errors;
