//! `OutputFormat` parse/precedence helpers + the single pinned timestamp helper.
//!
//! The [`OutputFormat`] enum itself is **re-exported** from `unblock-model` (G-7 / CF-J, spine
//! §1.10) — it is never redefined here. This module adds the render-local parse/precedence helpers
//! ([`OutputFormatExt::as_format_str`], [`parse_format`], [`parse_env_value`], [`pick_format`]) and
//! the canonical [`fmt_ts`] timestamp formatter.
//!
//! `OutputFormat` is an open contract enum owned by another crate, so render cannot `impl
//! Display`/`FromStr` on it directly (orphan rule). The parse/format helpers are therefore free
//! functions plus a sealed extension trait.

use std::str::FromStr;

use chrono::{DateTime, Utc};

pub use unblock_model::OutputFormat;

use crate::error::RenderError;

/// The stable lowercase wire string for an [`OutputFormat`].
///
/// Mirrors the model's `snake_case` serde representation so the `Display`/`FromStr` round-trip is
/// stable (`json`/`robot`/`plain`/`csv`/`markdown`/`toon`).
#[must_use]
pub fn format_as_str(format: OutputFormat) -> &'static str {
    match format {
        OutputFormat::Json => "json",
        OutputFormat::Robot => "robot",
        OutputFormat::Plain => "plain",
        OutputFormat::Csv => "csv",
        OutputFormat::Markdown => "markdown",
        #[cfg(feature = "toon")]
        OutputFormat::Toon => "toon",
    }
}

/// Sealed extension trait giving [`OutputFormat`] a render-local `format_str` accessor (the enum is
/// owned by `unblock-model`, so a direct inherent impl is not possible from this crate).
pub trait OutputFormatExt: private::Sealed {
    /// The stable lowercase wire string (see [`format_as_str`]).
    fn format_str(&self) -> &'static str;
}

impl OutputFormatExt for OutputFormat {
    fn format_str(&self) -> &'static str {
        format_as_str(*self)
    }
}

mod private {
    pub trait Sealed {}
    impl Sealed for super::OutputFormat {}
}

/// Parse an [`OutputFormat`] from its name (case-insensitive).
///
/// Accepts the canonical names plus the `text` alias for `plain`. An unrecognised value is a
/// [`RenderError::UnknownFormat`] carrying the raw (trimmed) offending name so the boundary can echo
/// exactly what the caller passed (D27/AF-4, T3.1).
///
/// # Errors
///
/// Returns [`RenderError::UnknownFormat`] when `value` is not a known format name.
pub fn parse_format(value: &str) -> Result<OutputFormat, RenderError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "json" => Ok(OutputFormat::Json),
        "robot" => Ok(OutputFormat::Robot),
        "plain" | "text" => Ok(OutputFormat::Plain),
        "csv" => Ok(OutputFormat::Csv),
        "markdown" | "md" => Ok(OutputFormat::Markdown),
        #[cfg(feature = "toon")]
        "toon" => Ok(OutputFormat::Toon),
        // Carry the raw (trimmed) name the caller passed — `UnknownFormat` is distinct from
        // `UnsupportedFormat` (a KNOWN format that cannot render a kind). `parse_env_value` below
        // still maps this to `None` (the env layer swallows an unknown value via `.ok()`).
        _ => Err(RenderError::UnknownFormat {
            name: value.trim().to_string(),
        }),
    }
}

/// Parse an environment-variable value (`UNBLOCK_OUTPUT_FORMAT`) into an [`OutputFormat`].
///
/// Unlike [`parse_format`], an unrecognised value yields `None` (the env layer is best-effort: an
/// unknown value falls through to the next precedence layer rather than erroring). Recognises the
/// same aliases (`text` → `Plain`, `md` → `Markdown`).
#[must_use]
pub fn parse_env_value(value: &str) -> Option<OutputFormat> {
    parse_format(value).ok()
}

/// Resolve the effective [`OutputFormat`] by precedence (D10): CLI > env > config > `Json`.
///
/// This is a **pure** function — render never reads env vars or files itself. The caller passes
/// the already-extracted CLI flag, the raw `UNBLOCK_OUTPUT_FORMAT` env string, and the config
/// default. An unparseable env string is ignored (falls through to config, then `Json`).
#[must_use]
pub fn pick_format(
    cli: Option<OutputFormat>,
    env: Option<&str>,
    cfg_default: Option<OutputFormat>,
) -> OutputFormat {
    cli.or_else(|| env.and_then(parse_env_value))
        .or(cfg_default)
        .unwrap_or(OutputFormat::Json)
}

/// `FromStr` shim so callers can `value.parse::<FormatName>()` ergonomically; unknown → error.
///
/// A newtype is used because `OutputFormat` is owned by `unblock-model` (orphan rule forbids an
/// `impl FromStr for OutputFormat` here).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatName(pub OutputFormat);

impl FromStr for FormatName {
    type Err = RenderError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_format(s).map(FormatName)
    }
}

impl std::fmt::Display for FormatName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(format_as_str(self.0))
    }
}

/// The single canonical timestamp helper (A.OQ-B / crate plan §5 item 5).
///
/// RFC-3339, UTC, **second precision**, `Z` suffix. This is the **only** path any backend uses to
/// render a [`DateTime<Utc>`]: no backend may call `to_rfc3339()` directly (it emits sub-seconds +
/// offset, breaking byte-determinism and the T2.4 export-byte coherence). This is an intentional
/// deviation from the original CSV/text bytes.
///
/// Since T2.4/FORK-4 the canonical formatter is LIFTED into `unblock-model` L0
/// ([`unblock_model::fmt_ts_secs`]) as the single source of truth so `unblock-sync` — which cannot
/// depend on render (L6) — shares it; `fmt_ts` is a thin, byte-identical delegate.
///
/// # Examples
///
/// ```
/// use chrono::{TimeZone, Utc};
/// use unblock_render::fmt_ts;
///
/// let dt = Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap();
/// assert_eq!(fmt_ts(dt), "2026-01-02T03:04:05Z");
/// ```
#[must_use]
pub fn fmt_ts(dt: DateTime<Utc>) -> String {
    unblock_model::fmt_ts_secs(dt)
}

#[cfg(test)]
mod tests {
    use super::{
        FormatName, OutputFormat, OutputFormatExt, fmt_ts, parse_env_value, parse_format,
        pick_format,
    };
    use chrono::{TimeZone, Utc};
    use std::str::FromStr;

    fn known_formats() -> Vec<OutputFormat> {
        let formats = vec![
            OutputFormat::Json,
            OutputFormat::Robot,
            OutputFormat::Plain,
            OutputFormat::Csv,
            OutputFormat::Markdown,
        ];
        #[cfg(feature = "toon")]
        let formats = {
            let mut formats = formats;
            formats.push(OutputFormat::Toon);
            formats
        };
        formats
    }

    #[test]
    fn display_from_str_round_trip_all_variants() {
        for fmt in known_formats() {
            let s = fmt.format_str();
            assert_eq!(parse_format(s).unwrap(), fmt);
            assert_eq!(FormatName::from_str(s).unwrap(), FormatName(fmt));
            assert_eq!(FormatName(fmt).to_string(), s);
        }
    }

    #[test]
    fn parse_is_case_insensitive_and_trims() {
        assert_eq!(parse_format("  JSON ").unwrap(), OutputFormat::Json);
        assert_eq!(parse_format("Robot").unwrap(), OutputFormat::Robot);
    }

    #[test]
    fn text_and_md_aliases() {
        assert_eq!(parse_format("text").unwrap(), OutputFormat::Plain);
        assert_eq!(parse_env_value("text"), Some(OutputFormat::Plain));
        assert_eq!(parse_format("md").unwrap(), OutputFormat::Markdown);
    }

    #[test]
    fn unknown_format_errors_and_env_is_none() {
        use crate::RenderError;
        // D27/AF-4: the fallthrough arm returns `UnknownFormat` carrying the raw (trimmed) name.
        match parse_format("  bogus ") {
            Err(RenderError::UnknownFormat { name }) => assert_eq!(name, "bogus"),
            other => panic!("expected UnknownFormat carrying the raw name, got {other:?}"),
        }
        // The env layer still swallows an unknown value (via `.ok()`).
        assert_eq!(parse_env_value("xml"), None);
    }

    #[test]
    fn precedence_cli_over_env_over_cfg_over_default() {
        // CLI wins.
        assert_eq!(
            pick_format(
                Some(OutputFormat::Csv),
                Some("plain"),
                Some(OutputFormat::Robot)
            ),
            OutputFormat::Csv
        );
        // env wins when no CLI.
        assert_eq!(
            pick_format(None, Some("plain"), Some(OutputFormat::Robot)),
            OutputFormat::Plain
        );
        // cfg wins when no CLI/env.
        assert_eq!(
            pick_format(None, None, Some(OutputFormat::Robot)),
            OutputFormat::Robot
        );
        // default Json when nothing set.
        assert_eq!(pick_format(None, None, None), OutputFormat::Json);
        // unparseable env falls through to cfg.
        assert_eq!(
            pick_format(None, Some("nonsense"), Some(OutputFormat::Markdown)),
            OutputFormat::Markdown
        );
    }

    #[test]
    fn fmt_ts_is_second_precision_utc_z() {
        let dt = Utc
            .with_ymd_and_hms(2026, 6, 29, 12, 30, 45)
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(fmt_ts(dt), "2026-06-29T12:30:45Z");
        // Sub-second input is truncated to seconds.
        let sub = Utc.timestamp_opt(1_000_000_000, 123_456_789).unwrap();
        let out = fmt_ts(sub);
        assert!(out.ends_with('Z'));
        assert!(!out.contains('.'), "no sub-second component: {out}");
    }
}
