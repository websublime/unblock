//! In-memory graph cache with TTL and invalidation.
//!
//! `GraphCache` holds a cached graph state behind `RwLock` with configurable TTL.
//! Every write operation invalidates the cache, triggering a rebuild on the next read.
