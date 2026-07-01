//! Canonical RFC-3339 timestamp rendering (CF-TS, spine §1.10 / FORK-4 / D-OQ-B / D23).
//!
//! The single source of truth for stringifying a [`DateTime<Utc>`] lives here at L0 so BOTH
//! `unblock-render` (its `fmt_ts` helper) and `unblock-sync` (the JSONL export serializer) share ONE
//! formatter without a render↔sync layering edge. Pure, no I/O; reuses the model's existing `chrono`
//! dependency (adds no new dep).

use chrono::{DateTime, SecondsFormat, Utc};

/// Canonical RFC-3339 rendering: UTC, SECOND precision, `Z` suffix (e.g. `2026-01-02T03:04:05Z`).
///
/// This is the **only** path any crate may use to stringify a [`DateTime<Utc>`] for output/export.
/// No crate may call [`chrono::DateTime::to_rfc3339`] directly — it emits sub-seconds + a numeric
/// offset, breaking byte-determinism and the render↔export byte-coherence the T2.4 JSONL export
/// depends on (spine §1.10, D-OQ-B). `content_hash` is unaffected (spine §1.8 excludes all
/// timestamps from the hash), so this is hash-safe.
///
/// # Examples
///
/// ```
/// use chrono::{TimeZone, Utc};
/// use unblock_model::fmt_ts_secs;
///
/// let dt = Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap();
/// assert_eq!(fmt_ts_secs(dt), "2026-01-02T03:04:05Z");
/// ```
#[must_use]
pub fn fmt_ts_secs(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::fmt_ts_secs;
    use chrono::{TimeZone, Utc};

    #[test]
    fn second_precision_utc_z() {
        let dt = Utc.with_ymd_and_hms(2026, 6, 29, 12, 30, 45).unwrap();
        assert_eq!(fmt_ts_secs(dt), "2026-06-29T12:30:45Z");
    }

    #[test]
    fn sub_second_truncates_to_seconds() {
        // A sub-second input renders WITHOUT a fractional component.
        let sub = Utc.timestamp_opt(1_000_000_000, 123_456_789).unwrap();
        let out = fmt_ts_secs(sub);
        assert!(out.ends_with('Z'), "must end with Z: {out}");
        assert!(!out.contains('.'), "no sub-second component: {out}");
    }

    #[test]
    fn never_numeric_offset() {
        let dt = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let out = fmt_ts_secs(dt);
        // `Z` suffix, never `+00:00`.
        assert!(out.ends_with('Z'), "{out}");
        assert!(!out.contains("+00:00"), "{out}");
    }
}
