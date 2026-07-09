//! NFR-13 reliability-emission SSOT (D30).
//!
//! Every `unblock.reliability` event in this crate is emitted through the `reliability_guard!` /
//! `reliability_warn!` / `reliability_detail!` trio below, so a non-conforming emitter cannot exist
//! by construction: each macro pins the target and the four fields `operation`/`path`/`result`/
//! `reason` in ONE body. They are **macros, not functions**, so each emission keeps `tracing`'s static
//! callsite at its true guard site.
//!
//! The three levels cover every live emission level in the crate:
//! - [`reliability_guard!`] — INFO for guard activations (external-path use / force-override,
//!   conflict-marker rejection);
//! - [`reliability_warn!`] — WARN for the best-effort perms guard (`atomic.rs`);
//! - [`reliability_detail!`] — DEBUG for per-file / per-issue detail (import skips,
//!   non-unix parent-fsync-skip).
//!
//! The WARN arm is REQUIRED so the live unix-perms warn routes through the SSOT WITHOUT re-leveling
//! (re-leveling a live WARN to fit a two-arm SSOT would be a forbidden silent simplification).
//!
//! The tracing-target const is IMPORTED from the L0 crate `unblock-error` (NO by-value copy, D30/
//! FIX-7): `init_tracing`'s `EnvFilter` directive references the SAME one const, so the filter-target
//! can never diverge from this emit-target.

// Re-exported at `crate::reliability::RELIABILITY_TARGET` so the macros can pin `target:
// $crate::reliability::RELIABILITY_TARGET` — the ONE const, shared with the engine's `init_tracing`.
pub(crate) use unblock_error::RELIABILITY_TARGET;

/// Emit an INFO reliability GUARD activation (NFR-13/D30).
///
/// Pins `target: RELIABILITY_TARGET` and the four fields `operation`/`path`/`result`/`reason` (all
/// recorded via their `Display`). Used at the honored-`allow_external` external-path/force-override
/// site and the conflict-marker-rejection site.
macro_rules! reliability_guard {
    (
        operation = $operation:expr,
        path = $path:expr,
        result = $result:expr,
        reason = $reason:expr $(,)?
    ) => {
        ::tracing::info!(
            target: $crate::reliability::RELIABILITY_TARGET,
            operation = %$operation,
            path = %$path,
            result = %$result,
            reason = %$reason,
        );
    };
}

/// Emit a WARN reliability event (NFR-13/D30) — the best-effort guard level.
///
/// Same four-field body as [`reliability_guard!`] at WARN. Used by the best-effort unix-perms guard
/// (`atomic.rs`), which must NOT be re-leveled to fit a narrower SSOT.
macro_rules! reliability_warn {
    (
        operation = $operation:expr,
        path = $path:expr,
        result = $result:expr,
        reason = $reason:expr $(,)?
    ) => {
        ::tracing::warn!(
            target: $crate::reliability::RELIABILITY_TARGET,
            operation = %$operation,
            path = %$path,
            result = %$result,
            reason = %$reason,
        );
    };
}

/// Emit a DEBUG reliability detail event (NFR-13/D30) — per-file / per-issue detail.
///
/// Same four-field body as [`reliability_guard!`] at DEBUG. Per-issue events carry the record id in
/// the `path` field, keeping the four-key set uniform.
macro_rules! reliability_detail {
    (
        operation = $operation:expr,
        path = $path:expr,
        result = $result:expr,
        reason = $reason:expr $(,)?
    ) => {
        ::tracing::debug!(
            target: $crate::reliability::RELIABILITY_TARGET,
            operation = %$operation,
            path = %$path,
            result = %$result,
            reason = %$reason,
        );
    };
}

pub(crate) use {reliability_detail, reliability_guard, reliability_warn};
