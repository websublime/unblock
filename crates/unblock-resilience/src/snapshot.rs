//! Read-only observability snapshots for the breaker and retry policy.
//!
//! Bodies (`BreakerSnapshot`, `RetrySnapshot`, `BreakerState`) are added in
//! beads 02.A.3 and 02.A.4 per spec §4.5. Serialisation is gated behind the
//! `serde` feature.
