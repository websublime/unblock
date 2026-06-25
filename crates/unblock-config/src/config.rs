//! [`ResolvedConfig`] — the resolved, validated config **VALUES** the engine/`Session` reads
//! (config-owned, spine §4 CF-D).
//!
//! This is **NOT** paths (those live in [`crate::ConfigPaths`]) and **NOT** the actor (that is the
//! top-level context field, spine §4.1). The minimal v1 shape is pinned by the crate plan
//! (`docs/plans/crates/unblock-config.md` §2/§3): it is **DEFAULTED** in the T1.3a minimal subset
//! and **RESOLVED** for real (layered TOML/env/CLI) at T1.3 — the shape does not change.

use unblock_model::OutputFormat;

/// The locked default database filename (PRD §12.5).
pub(crate) const DB_FILENAME: &str = "unblock.db";

/// The locked default JSONL export filename (PRD §12.5).
pub(crate) const JSONL_FILENAME: &str = "issues.jsonl";

/// The T1.3a default search-result cap (FR-4).
pub(crate) const DEFAULT_SEARCH_CAP: usize = 50;

/// The resolved, validated config values the engine/`Session` consumes (config-owned, spine §4
/// CF-D).
///
/// Embedded **by value** in both [`crate::ResolvedContext`] and [`crate::WorkspaceContext`]. In the
/// T1.3a minimal subset every field is the locked default ([`ResolvedConfig::default`]); the T1.3
/// layered resolver fills the values for real without changing this shape.
#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    /// The output format for rendered command output (re-exported from `unblock-model`, G-7/CF-J).
    /// T1.3a default = [`OutputFormat::Json`] (the model's own `Default`).
    pub output_format: OutputFormat,
    /// Auto-export JSONL after mutating ops (FR-7). T1.3a default = `false`.
    pub jsonl_export: bool,
    /// Search-result cap (FR-4). T1.3a default = `50`.
    pub search_cap: usize,
    /// The database filename inside `.unblock/`. T1.3a default = `"unblock.db"` (PRD §12.5).
    pub db_filename: String,
    /// The JSONL export filename inside `.unblock/`. T1.3a default = `"issues.jsonl"`
    /// (PRD §12.5).
    pub jsonl_filename: String,
}

impl Default for ResolvedConfig {
    /// The T1.3a defaults: `Json` / `false` / `50` / `"unblock.db"` / `"issues.jsonl"`.
    fn default() -> Self {
        Self {
            output_format: OutputFormat::default(),
            jsonl_export: false,
            search_cap: DEFAULT_SEARCH_CAP,
            db_filename: DB_FILENAME.to_string(),
            jsonl_filename: JSONL_FILENAME.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DB_FILENAME, DEFAULT_SEARCH_CAP, JSONL_FILENAME, ResolvedConfig};
    use unblock_model::OutputFormat;

    #[test]
    fn defaults_match_the_t1_3a_pins() {
        let cfg = ResolvedConfig::default();
        assert_eq!(cfg.output_format, OutputFormat::Json);
        assert!(!cfg.jsonl_export);
        assert_eq!(cfg.search_cap, DEFAULT_SEARCH_CAP);
        assert_eq!(cfg.search_cap, 50);
        assert_eq!(cfg.db_filename, DB_FILENAME);
        assert_eq!(cfg.db_filename, "unblock.db");
        assert_eq!(cfg.jsonl_filename, JSONL_FILENAME);
        assert_eq!(cfg.jsonl_filename, "issues.jsonl");
    }
}
