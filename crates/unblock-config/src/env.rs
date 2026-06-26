//! The `UNBLOCK_*` environment layer (single prefix, D10) and its injectable source.
//!
//! Parses `UNBLOCK_ACTOR`, `UNBLOCK_DIR`, `UNBLOCK_JSONL`, and `UNBLOCK_OUTPUT_FORMAT` into a typed
//! [`EnvOverrides`] (the second-highest precedence layer, below CLI and above the project TOML). The
//! `UNBLOCK_JSONL` key is a **boolean export toggle** (SF-6) — NOT a path (unlike the original
//! `BEADS_JSONL`, which carried a path). `UNBLOCK_OUTPUT_FORMAT` deserializes via serde (SF-2):
//! [`OutputFormat`] has no `FromStr`, so an unknown value is [`ConfigError::InvalidValue`] and an
//! empty value is treated as **unset**. Empty-as-unset applies to **all** keys.
//!
//! The [`EnvSource`] trait is the single injection point for the environment (this module owns it;
//! `actor.rs` re-uses it) so the suite is deterministic and parallel-safe — tests never touch the
//! process-global env (`std::env::set_var` races, NFR-16). No legacy `BD_`/`BR_`/`BEADS_` keys are
//! recognized (D10).

use std::path::PathBuf;

use unblock_model::OutputFormat;

use crate::error::ConfigError;

/// A read-only view over environment variables, injected so resolution is deterministic and tests
/// avoid the process-global env (no `std::env::set_var` races).
pub trait EnvSource {
    /// Fetch the value of `key`, or `None` when it is unset.
    fn get(&self, key: &str) -> Option<String>;
}

/// The production [`EnvSource`] backed by [`std::env::var`].
pub struct ProcessEnv;

impl EnvSource for ProcessEnv {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

/// The parsed `UNBLOCK_*` layer (second precedence, below CLI / above project TOML).
///
/// Every field is `Option` (set-or-unset). An empty/whitespace value at any key is normalized to
/// `None` (unset), matching the original beads `filter(!is_empty)` behaviour.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvOverrides {
    /// `UNBLOCK_ACTOR` — the default actor (highest-but-one in the actor chain, FORK-4).
    pub actor: Option<String>,
    /// `UNBLOCK_DIR` — the EXPLICIT workspace dir override (no walk-up; MF-2).
    pub dir: Option<PathBuf>,
    /// `UNBLOCK_JSONL` — the boolean JSONL-export toggle (SF-6 — NOT a path).
    pub jsonl_export: Option<bool>,
    /// `UNBLOCK_OUTPUT_FORMAT` — the output format, deserialized via serde (SF-2).
    pub output_format: Option<OutputFormat>,
    /// `UNBLOCK_REMOTE_URL` — reserved for v1.2 (parsed but unused in v1).
    pub remote_url: Option<String>,
    /// `UNBLOCK_AUTH_TOKEN` — reserved for v1.2 (parsed but unused in v1; the sanctioned credential
    /// path, never `config.toml` — NFR-18).
    pub auth_token: Option<String>,
}

impl EnvOverrides {
    /// Parse `UNBLOCK_*` from an injected [`EnvSource`].
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::InvalidValue`] when `UNBLOCK_JSONL` is a non-boolean string or
    /// `UNBLOCK_OUTPUT_FORMAT` is an unrecognized format.
    pub fn from_source(src: &dyn EnvSource) -> Result<Self, ConfigError> {
        let actor = non_empty(src.get("UNBLOCK_ACTOR"));
        let dir = non_empty(src.get("UNBLOCK_DIR")).map(PathBuf::from);
        let jsonl_export = match non_empty(src.get("UNBLOCK_JSONL")) {
            Some(raw) => Some(parse_bool("UNBLOCK_JSONL", &raw)?),
            None => None,
        };
        let output_format = match non_empty(src.get("UNBLOCK_OUTPUT_FORMAT")) {
            Some(raw) => Some(parse_output_format("UNBLOCK_OUTPUT_FORMAT", &raw)?),
            None => None,
        };
        let remote_url = non_empty(src.get("UNBLOCK_REMOTE_URL"));
        let auth_token = non_empty(src.get("UNBLOCK_AUTH_TOKEN"));

        Ok(Self {
            actor,
            dir,
            jsonl_export,
            output_format,
            remote_url,
            auth_token,
        })
    }

    /// Parse `UNBLOCK_*` from the live process environment ([`ProcessEnv`]).
    ///
    /// # Errors
    ///
    /// As [`EnvOverrides::from_source`].
    pub fn from_process_env() -> Result<Self, ConfigError> {
        Self::from_source(&ProcessEnv)
    }
}

/// Trim a raw env value and treat empty/whitespace-only as unset.
fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|v| {
        let trimmed = v.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

/// Parse a boolean env toggle (`true`/`false`/`1`/`0`/`yes`/`no`/`on`/`off`, case-insensitive).
fn parse_bool(key: &str, raw: &str) -> Result<bool, ConfigError> {
    match raw.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(ConfigError::InvalidValue {
            key: key.to_string(),
            value: raw.to_string(),
            reason: "expected a boolean (true/false/1/0/yes/no/on/off)".to_string(),
        }),
    }
}

/// Parse an [`OutputFormat`] via serde (SF-2 — the enum has no `FromStr`).
fn parse_output_format(key: &str, raw: &str) -> Result<OutputFormat, ConfigError> {
    // Route through serde so the snake_case wire strings are the single source of truth (no second
    // string table). A bare string deserializes as a unit-variant enum via the value deserializer.
    serde::Deserialize::deserialize(
        serde::de::value::StrDeserializer::<serde::de::value::Error>::new(raw),
    )
    .map_err(|_| ConfigError::InvalidValue {
        key: key.to_string(),
        value: raw.to_string(),
        reason: "unrecognized output format (expected json/robot/plain/csv/markdown)".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::{EnvOverrides, EnvSource};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use unblock_model::OutputFormat;

    struct MapEnv(HashMap<String, String>);

    impl MapEnv {
        fn new(pairs: &[(&str, &str)]) -> Self {
            Self(
                pairs
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                    .collect(),
            )
        }
    }

    impl EnvSource for MapEnv {
        fn get(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
    }

    #[test]
    fn parses_all_keys() {
        let env = MapEnv::new(&[
            ("UNBLOCK_ACTOR", "alice"),
            ("UNBLOCK_DIR", "/ws/.unblock"),
            ("UNBLOCK_JSONL", "true"),
            ("UNBLOCK_OUTPUT_FORMAT", "robot"),
        ]);
        let ov = EnvOverrides::from_source(&env).expect("parse");
        assert_eq!(ov.actor.as_deref(), Some("alice"));
        assert_eq!(ov.dir, Some(PathBuf::from("/ws/.unblock")));
        assert_eq!(ov.jsonl_export, Some(true));
        assert_eq!(ov.output_format, Some(OutputFormat::Robot));
    }

    #[test]
    fn empty_values_are_unset() {
        let env = MapEnv::new(&[
            ("UNBLOCK_ACTOR", "   "),
            ("UNBLOCK_DIR", ""),
            ("UNBLOCK_JSONL", ""),
            ("UNBLOCK_OUTPUT_FORMAT", "  "),
        ]);
        let ov = EnvOverrides::from_source(&env).expect("parse");
        assert_eq!(ov, EnvOverrides::default());
    }

    #[test]
    fn unblock_jsonl_is_a_boolean_toggle() {
        for (raw, expected) in [("1", true), ("off", false), ("YES", true), ("False", false)] {
            let env = MapEnv::new(&[("UNBLOCK_JSONL", raw)]);
            let ov = EnvOverrides::from_source(&env).expect("parse bool");
            assert_eq!(ov.jsonl_export, Some(expected), "raw={raw}");
        }
        // A non-boolean is an error (not silently a path).
        let env = MapEnv::new(&[("UNBLOCK_JSONL", "/some/path.jsonl")]);
        let err = EnvOverrides::from_source(&env).expect_err("non-bool jsonl");
        assert!(matches!(
            err,
            crate::error::ConfigError::InvalidValue { .. }
        ));
    }

    #[test]
    fn output_format_unknown_is_invalid_value() {
        let env = MapEnv::new(&[("UNBLOCK_OUTPUT_FORMAT", "yaml")]);
        let err = EnvOverrides::from_source(&env).expect_err("unknown format");
        match err {
            crate::error::ConfigError::InvalidValue { key, .. } => {
                assert_eq!(key, "UNBLOCK_OUTPUT_FORMAT");
            }
            other => panic!("expected InvalidValue, got {other:?}"),
        }
    }

    #[test]
    fn legacy_prefixes_are_ignored() {
        // BD_/BR_/BEADS_ keys must NOT be recognized (D10).
        let env = MapEnv::new(&[
            ("BD_ACTOR", "legacy"),
            ("BR_DIR", "/legacy"),
            ("BEADS_JSONL", "true"),
            ("BEADS_OUTPUT_FORMAT", "plain"),
        ]);
        let ov = EnvOverrides::from_source(&env).expect("parse");
        assert_eq!(ov, EnvOverrides::default());
    }
}
