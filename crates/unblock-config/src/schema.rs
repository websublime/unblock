//! The serde-deserializable raw `.unblock/config.toml` shape ([`ProjectConfig`]) + parse seam.
//!
//! Distinct from the merged [`crate::WorkspaceConfig`]: the raw layer is **all-`Option`** (every key
//! is "set or unset"), the merged value is resolved. The full known-key set is typed so a
//! **wrong-typed KNOWN key** (e.g. `search_cap = "abc"`) **hard-errors** at parse — it is never
//! swallowed by the extras map. Unknown keys are **captured** (not rejected) via a
//! `#[serde(flatten)]` [`BTreeMap`] and emitted as `tracing::warn!` (SF-4 — startup resilience over
//! `deny_unknown_fields`). A `[remote] auth_token` is **denied** at parse (NFR-18): credentials must
//! come from `UNBLOCK_*` env or the OS keychain, never `config.toml`.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use snafu::ResultExt;
use unblock_model::OutputFormat;

use crate::error::{ConfigError, IoSnafu, ParseSnafu};

/// The config filename inside `.unblock/` (locked, PRD §12.5).
pub(crate) const CONFIG_FILENAME: &str = "config.toml";

/// The raw, all-`Option` project config — the deserialized shape of `.unblock/config.toml`.
///
/// Every known key is a typed `Option`, so a present-but-wrong-typed value (e.g. a string where a
/// `usize` is expected) is a hard parse error (SF-4), while an absent key is `None` (the layer did
/// not set it). Unknown keys flow into [`ProjectConfig::extra`] for warn + credential inspection,
/// **not** a hard failure (forward-compat). `output_format` deserializes via serde (`snake_case`
/// wire strings) — [`OutputFormat`] has no `FromStr` (SF-2).
///
/// Public so callers (and the FR-13 integration precedence suite) can build a project layer for
/// [`WorkspaceConfig::resolve`](crate::WorkspaceConfig::resolve) without going through disk. `Eq` is
/// intentionally omitted: `extra`/`auth_token` carry `toml::Value`, which is not `Eq` (it can hold a
/// float). `PartialEq` is sufficient for the tests' `assert_eq!`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct ProjectConfig {
    /// `[actor]` default (runtime key).
    pub actor: Option<String>,
    /// `db_filename` (startup key) — the database filename inside `.unblock/`.
    pub db_filename: Option<String>,
    /// `jsonl_export_filename` (startup key) — projects to the merged `jsonl_filename`.
    pub jsonl_export_filename: Option<String>,
    /// `jsonl_export` (runtime key) — the auto-export toggle (FR-7).
    pub jsonl_export: Option<bool>,
    /// `output_format` (runtime key) — deserialized via serde, no `FromStr` (SF-2).
    pub output_format: Option<OutputFormat>,
    /// `search_cap` (runtime key) — the search-result cap (FR-4).
    pub search_cap: Option<usize>,
    /// `deletions_retention_days` (startup key) — reserved for v1.1, parsed + carried.
    pub deletions_retention_days: Option<u64>,
    /// `backend` (startup key) — parsed + validated (only `"libsql"` in v1; MF-3), reserved.
    pub backend: Option<String>,
    /// The `[remote]` table (v1.2 surface) — present here ONLY to **deny** an `auth_token` at
    /// parse (NFR-18). Any other `[remote]` keys flow into `RemoteTable::extra` (warn-only).
    pub remote: Option<RemoteTable>,
    /// Captured unknown top-level keys (SF-4): warned, never an error (forward-compat). A captured
    /// top-level `auth_token` is also a credential and is denied (NFR-18).
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

/// The `[remote]` table — typed only to deny `auth_token` (NFR-18). v1.2 will give it real fields.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct RemoteTable {
    /// A forbidden libsql auth token (NFR-18): credentials never live in `config.toml`. If present
    /// (even null/empty) the parse is rejected with [`ConfigError::InvalidValue`].
    pub auth_token: Option<toml::Value>,
    /// Other `[remote]` keys (e.g. a future `url`) — captured for warn, not an error.
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

impl ProjectConfig {
    /// Load + parse `<unblock_dir>/config.toml` into a [`ProjectConfig`].
    ///
    /// A **missing** file is not an error — it yields the all-`None` [`ProjectConfig::default`]
    /// (defaults apply, D2). A present file is read and parsed; unknown keys are warned, a
    /// `[remote] auth_token` (or a top-level `auth_token`) is denied (NFR-18), and a wrong-typed
    /// known key hard-errors.
    ///
    /// # Errors
    ///
    /// - [`ConfigError::Io`] if the file exists but cannot be read.
    /// - [`ConfigError::Parse`] if the file exists but is not valid TOML or a known key has the
    ///   wrong type.
    /// - [`ConfigError::InvalidValue`] if a forbidden credential key is present (NFR-18).
    pub fn load(unblock_dir: &Path) -> Result<Self, ConfigError> {
        let path = unblock_dir.join(CONFIG_FILENAME);
        if !path.exists() {
            // No config.toml is the common case (defaults apply); not an error (D2).
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(&path).context(IoSnafu { path: path.clone() })?;
        let config: Self = toml::from_str(&contents).context(ParseSnafu { path: path.clone() })?;
        config.deny_credentials()?;
        config.warn_unknown_keys();
        Ok(config)
    }

    /// Parse a TOML string directly (test seam for the deny/warn behaviour without touching disk).
    ///
    /// `label` names the source for diagnostics. Applies the same credential-deny + warn rules as
    /// [`ProjectConfig::load`].
    ///
    /// # Errors
    ///
    /// As [`ProjectConfig::load`], minus the I/O path.
    #[cfg(test)]
    pub(crate) fn parse_str(contents: &str, label: &Path) -> Result<Self, ConfigError> {
        let config: Self = toml::from_str(contents).context(ParseSnafu {
            path: label.to_path_buf(),
        })?;
        config.deny_credentials()?;
        config.warn_unknown_keys();
        Ok(config)
    }

    /// Deny any libsql credential in `config.toml` (NFR-18): a `[remote] auth_token` or a top-level
    /// `auth_token`. Credentials must come from env / keychain only.
    fn deny_credentials(&self) -> Result<(), ConfigError> {
        if let Some(remote) = &self.remote
            && remote.auth_token.is_some()
        {
            return Err(ConfigError::InvalidValue {
                key: "remote.auth_token".to_string(),
                value: "<redacted>".to_string(),
                reason: "libsql credentials must never be stored in config.toml (use UNBLOCK_* env or the OS keychain)".to_string(),
            });
        }
        if self.extra.contains_key("auth_token") {
            return Err(ConfigError::InvalidValue {
                key: "auth_token".to_string(),
                value: "<redacted>".to_string(),
                reason: "libsql credentials must never be stored in config.toml (use UNBLOCK_* env or the OS keychain)".to_string(),
            });
        }
        Ok(())
    }

    /// Emit a `tracing::warn!` for every captured unknown key (SF-4) — never an error.
    fn warn_unknown_keys(&self) {
        for key in self.extra.keys() {
            tracing::warn!(
                target: "unblock.config",
                key = %key,
                "unknown config.toml key ignored (forward-compatible)"
            );
        }
        if let Some(remote) = &self.remote {
            for key in remote.extra.keys() {
                tracing::warn!(
                    target: "unblock.config",
                    key = %format!("remote.{key}"),
                    "unknown config.toml key ignored (forward-compatible)"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ProjectConfig;
    use std::path::Path;
    use unblock_model::OutputFormat;

    fn parse(s: &str) -> Result<ProjectConfig, crate::error::ConfigError> {
        ProjectConfig::parse_str(s, Path::new("test://config.toml"))
    }

    #[test]
    fn parses_a_representative_config() {
        let cfg = parse(
            r#"
            actor = "alice"
            db_filename = "unblock.db"
            jsonl_export_filename = "issues.jsonl"
            jsonl_export = true
            output_format = "robot"
            search_cap = 100
            deletions_retention_days = 30
            backend = "libsql"
            "#,
        )
        .expect("parse");
        assert_eq!(cfg.actor.as_deref(), Some("alice"));
        assert_eq!(cfg.db_filename.as_deref(), Some("unblock.db"));
        assert_eq!(cfg.jsonl_export_filename.as_deref(), Some("issues.jsonl"));
        assert_eq!(cfg.jsonl_export, Some(true));
        assert_eq!(cfg.output_format, Some(OutputFormat::Robot));
        assert_eq!(cfg.search_cap, Some(100));
        assert_eq!(cfg.deletions_retention_days, Some(30));
        assert_eq!(cfg.backend.as_deref(), Some("libsql"));
    }

    #[test]
    fn empty_config_is_all_none() {
        let cfg = parse("").expect("parse empty");
        assert_eq!(cfg, ProjectConfig::default());
    }

    #[test]
    fn unknown_key_is_captured_not_rejected() {
        let cfg = parse("future_knob = \"value\"\nactor = \"bob\"").expect("parse with unknown");
        assert_eq!(cfg.actor.as_deref(), Some("bob"));
        assert!(cfg.extra.contains_key("future_knob"));
    }

    #[test]
    fn wrong_typed_known_key_hard_errors() {
        // search_cap is a usize; a string must NOT be swallowed by the extras map (SF-4).
        let err = parse("search_cap = \"abc\"").expect_err("must hard-error");
        assert!(
            matches!(err, crate::error::ConfigError::Parse { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn output_format_parses_via_serde_snake_case() {
        assert_eq!(
            parse("output_format = \"plain\"").unwrap().output_format,
            Some(OutputFormat::Plain)
        );
        assert_eq!(
            parse("output_format = \"markdown\"").unwrap().output_format,
            Some(OutputFormat::Markdown)
        );
        // An unknown output_format value is a hard parse error (not swallowed).
        let err = parse("output_format = \"yaml\"").expect_err("unknown format");
        assert!(
            matches!(err, crate::error::ConfigError::Parse { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn remote_auth_token_is_denied() {
        let err = parse("[remote]\nauth_token = \"secret\"").expect_err("credential denied");
        match err {
            crate::error::ConfigError::InvalidValue { key, .. } => {
                assert_eq!(key, "remote.auth_token");
            }
            other => panic!("expected InvalidValue, got {other:?}"),
        }
    }

    #[test]
    fn top_level_auth_token_is_denied() {
        let err = parse("auth_token = \"secret\"").expect_err("credential denied");
        match err {
            crate::error::ConfigError::InvalidValue { key, .. } => assert_eq!(key, "auth_token"),
            other => panic!("expected InvalidValue, got {other:?}"),
        }
    }

    #[test]
    fn remote_table_without_token_is_allowed_with_warn() {
        // A [remote] table that carries only forward-compat keys is allowed (warn-only).
        let cfg = parse("[remote]\nurl = \"libsql://example\"").expect("allowed");
        assert!(cfg.remote.is_some());
    }
}
