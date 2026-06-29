//! Backend implementations of the [`crate::Renderer`] trait, one module per format family.
//!
//! `json` covers both `Json` (pretty) and `Robot` (compact); `plain`/`csv_fmt`/`markdown` are the
//! human/export formats. `toon` is `#[cfg(feature = "toon")]`-gated and cfg-empty in v1.

pub(crate) mod csv_fmt;
pub(crate) mod json;
pub(crate) mod markdown;
pub(crate) mod plain;
#[cfg(feature = "toon")]
pub(crate) mod toon;
