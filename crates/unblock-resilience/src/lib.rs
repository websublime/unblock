//! Generic HTTP-resilience policy: composed circuit breaker + retry-with-backoff.
//!
//! Two consumer crates: `unblock-github` (this phase) and `unblock-indexer`
//! (Phase 03). Both are architecturally orthogonal — `unblock-resilience` carries
//! zero unblock-domain knowledge.
//!
//! See [`ResiliencePolicy::execute`] for the single entry point. State scope is
//! per-process: each `ResiliencePolicy` instance owns its own breaker and retry
//! configuration.

pub mod breaker;
pub mod policy;
pub mod retry;
pub mod snapshot;
pub mod traits;
