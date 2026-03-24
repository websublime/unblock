//! Environment-based configuration.
//!
//! Reads configuration from environment variables:
//! - `GITHUB_TOKEN` (required)
//! - `GITHUB_API_URL` (optional, default: `https://api.github.com`)
//! - `UNBLOCK_REPO` (optional, auto-detect from git remote)
//! - `UNBLOCK_PROJECT` (optional, auto-detect from linked Projects V2)
//! - `UNBLOCK_AGENT` (optional, default: `agent`)
//! - `UNBLOCK_CACHE_TTL` (optional, default: `30` seconds)
//! - `UNBLOCK_LOG_LEVEL` (optional, default: `info`)
//! - `UNBLOCK_OTEL_ENDPOINT` (optional)

use std::env::VarError;

use crate::errors::ValidationSnafu;

/// Application configuration loaded from environment variables.
///
/// All fields are populated by [`Config::load`], which reads from the process
/// environment, or by [`Config::load_from`], which accepts a custom environment
/// reader (useful for testing without mutating process-global state).
///
/// # Required
///
/// - `GITHUB_TOKEN` — GitHub personal access token. Loading fails with
///   [`DomainError::Validation`](crate::errors::DomainError::Validation) if
///   this is absent or empty.
///
/// # Optional (with defaults)
///
/// | Env var | Field | Default |
/// |---------|-------|---------|
/// | `GITHUB_API_URL` | [`api_base_url`](Self::api_base_url) | `https://api.github.com` |
/// | `UNBLOCK_REPO` | [`repo`](Self::repo) | `None` (auto-detect) |
/// | `UNBLOCK_PROJECT` | [`project_number`](Self::project_number) | `None` (auto-detect) |
/// | `UNBLOCK_AGENT` | [`agent`](Self::agent) | `"agent"` |
/// | `UNBLOCK_CACHE_TTL` | [`cache_ttl`](Self::cache_ttl) | `30` |
/// | `UNBLOCK_LOG_LEVEL` | [`log_level`](Self::log_level) | `"info"` |
/// | `UNBLOCK_OTEL_ENDPOINT` | [`otel_endpoint`](Self::otel_endpoint) | `None` |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// GitHub personal access token (from `GITHUB_TOKEN`).
    pub token: String,

    /// GitHub API base URL (from `GITHUB_API_URL`).
    ///
    /// Defaults to `https://api.github.com`. For GitHub Enterprise Server,
    /// set to `https://<host>/api/v3`. Trailing slashes are stripped.
    pub api_base_url: String,

    /// Repository in `owner/repo` format (from `UNBLOCK_REPO`).
    ///
    /// When `None`, the GitHub client auto-detects from the git remote.
    pub repo: Option<String>,

    /// GitHub Projects V2 number (from `UNBLOCK_PROJECT`).
    ///
    /// When `None`, the GitHub client auto-detects from linked projects.
    pub project_number: Option<u64>,

    /// Default agent name for issue claims (from `UNBLOCK_AGENT`).
    ///
    /// Defaults to `"agent"`.
    pub agent: String,

    /// Graph cache time-to-live in seconds (from `UNBLOCK_CACHE_TTL`).
    ///
    /// Defaults to `30`.
    pub cache_ttl: u64,

    /// Tracing log level filter (from `UNBLOCK_LOG_LEVEL`).
    ///
    /// Defaults to `"info"`.
    pub log_level: String,

    /// OpenTelemetry collector endpoint (from `UNBLOCK_OTEL_ENDPOINT`).
    ///
    /// When `None`, OpenTelemetry export is disabled.
    pub otel_endpoint: Option<String>,
}

/// Default cache TTL in seconds.
const DEFAULT_CACHE_TTL: u64 = 30;

/// Default log level filter string.
const DEFAULT_LOG_LEVEL: &str = "info";

/// Default agent name.
const DEFAULT_AGENT: &str = "agent";

/// Default GitHub API base URL.
const DEFAULT_API_BASE_URL: &str = "https://api.github.com";

/// The set of valid tracing log-level strings.
///
/// These correspond to the five levels supported by the `tracing` crate.
/// Values are stored in lowercase; validation is case-insensitive.
const VALID_LOG_LEVELS: [&str; 5] = ["error", "warn", "info", "debug", "trace"];

/// Validates that a log-level string is one of the known tracing levels.
///
/// Comparison is case-insensitive. On failure, returns a
/// [`DomainError::Validation`](crate::errors::DomainError::Validation) listing
/// the accepted values.
fn validate_log_level(level: &str) -> Result<(), crate::errors::DomainError> {
    let lower = level.to_lowercase();
    if VALID_LOG_LEVELS.contains(&lower.as_str()) {
        Ok(())
    } else {
        Err(ValidationSnafu {
            message: format!(
                "UNBLOCK_LOG_LEVEL must be one of: error, warn, info, debug, trace; got: {level}"
            ),
        }
        .build())
    }
}

/// Validates that a repository string is in `owner/repo` format.
///
/// The string must contain exactly one `/`, with non-empty `owner` and `repo`
/// segments on each side.
fn validate_repo_format(repo: &str) -> Result<(), crate::errors::DomainError> {
    let parts: Vec<&str> = repo.split('/').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err(ValidationSnafu {
            message: format!("UNBLOCK_REPO must be in 'owner/repo' format, got: {repo}"),
        }
        .build());
    }
    Ok(())
}

impl Config {
    /// Loads configuration from the process environment.
    ///
    /// This is a convenience wrapper around [`Config::load_from`] that reads
    /// from [`std::env::var`].
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::Validation`](crate::errors::DomainError::Validation) if:
    /// - `GITHUB_TOKEN` is not set or is an empty string.
    /// - `UNBLOCK_REPO` is set but does not match the expected `owner/repo` format.
    /// - `UNBLOCK_PROJECT` is set but cannot be parsed as `u64`.
    /// - `UNBLOCK_CACHE_TTL` is set but cannot be parsed as `u64`.
    /// - `UNBLOCK_LOG_LEVEL` is set but is not one of: `error`, `warn`, `info`,
    ///   `debug`, `trace`.
    pub fn load() -> Result<Self, crate::errors::DomainError> {
        Self::load_from(|key| std::env::var(key))
    }

    /// Loads configuration from a custom environment reader.
    ///
    /// Accepts any function with the signature `Fn(&str) -> Result<String, VarError>`.
    /// This enables testing without mutating process-global environment variables,
    /// which is important in Rust edition 2024 where `std::env::set_var` is `unsafe`.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::Validation`](crate::errors::DomainError::Validation) if:
    /// - `GITHUB_TOKEN` is not set or is an empty string.
    /// - `UNBLOCK_REPO` is set but does not match the expected `owner/repo` format.
    /// - `UNBLOCK_PROJECT` is set but cannot be parsed as `u64`.
    /// - `UNBLOCK_CACHE_TTL` is set but cannot be parsed as `u64`.
    /// - `UNBLOCK_LOG_LEVEL` is set but is not one of: `error`, `warn`, `info`,
    ///   `debug`, `trace`.
    pub fn load_from(
        env: impl Fn(&str) -> Result<String, VarError>,
    ) -> Result<Self, crate::errors::DomainError> {
        // GITHUB_TOKEN is required and must be non-empty.
        let token = env("GITHUB_TOKEN")
            .ok()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                ValidationSnafu {
                    message: "GITHUB_TOKEN is required and must not be empty".to_owned(),
                }
                .build()
            })?;

        // GITHUB_API_URL with trailing-slash normalisation.
        let api_base_url = env("GITHUB_API_URL")
            .unwrap_or_else(|_| DEFAULT_API_BASE_URL.to_owned())
            .trim_end_matches('/')
            .to_owned();

        let repo = match env("UNBLOCK_REPO") {
            Ok(val) => {
                validate_repo_format(&val)?;
                Some(val)
            }
            Err(_) => None,
        };

        let project_number = match env("UNBLOCK_PROJECT") {
            Ok(val) => Some(val.parse::<u64>().map_err(|_| {
                ValidationSnafu {
                    message: format!("UNBLOCK_PROJECT must be a valid integer: {val}"),
                }
                .build()
            })?),
            Err(_) => None,
        };

        let agent = env("UNBLOCK_AGENT").unwrap_or_else(|_| DEFAULT_AGENT.to_owned());

        let cache_ttl = match env("UNBLOCK_CACHE_TTL") {
            Ok(val) => val.parse::<u64>().map_err(|_| {
                ValidationSnafu {
                    message: format!("UNBLOCK_CACHE_TTL must be a valid integer: {val}"),
                }
                .build()
            })?,
            Err(_) => DEFAULT_CACHE_TTL,
        };

        let log_level = env("UNBLOCK_LOG_LEVEL").unwrap_or_else(|_| DEFAULT_LOG_LEVEL.to_owned());
        validate_log_level(&log_level)?;
        let log_level = log_level.to_lowercase();

        let otel_endpoint = env("UNBLOCK_OTEL_ENDPOINT").ok();

        Ok(Self {
            token,
            api_base_url,
            repo,
            project_number,
            agent,
            cache_ttl,
            log_level,
            otel_endpoint,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::env::VarError;

    use super::*;

    /// Creates an env reader backed by a `HashMap`.
    fn make_env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Result<String, VarError> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |key: &str| map.get(key).cloned().ok_or(VarError::NotPresent)
    }

    #[test]
    fn missing_github_token_returns_validation_error() {
        let env = make_env(&[]);
        let err = Config::load_from(env).unwrap_err();
        assert!(err.to_string().contains("GITHUB_TOKEN"));
    }

    #[test]
    fn empty_github_token_returns_validation_error() {
        let env = make_env(&[("GITHUB_TOKEN", "")]);
        let err = Config::load_from(env).unwrap_err();
        assert!(err.to_string().contains("GITHUB_TOKEN"));
    }

    #[test]
    fn all_defaults_applied_when_only_token_set() {
        let env = make_env(&[("GITHUB_TOKEN", "ghp_test123")]);
        let config = Config::load_from(env).expect("should load");

        assert_eq!(config.token, "ghp_test123");
        assert_eq!(config.api_base_url, "https://api.github.com");
        assert_eq!(config.repo, None);
        assert_eq!(config.project_number, None);
        assert_eq!(config.agent, "agent");
        assert_eq!(config.cache_ttl, 30);
        assert_eq!(config.log_level, "info");
        assert_eq!(config.otel_endpoint, None);
    }

    #[test]
    fn all_overrides_applied_when_all_vars_set() {
        let env = make_env(&[
            ("GITHUB_TOKEN", "ghp_override"),
            ("GITHUB_API_URL", "https://ghe.example.com/api/v3/"),
            ("UNBLOCK_REPO", "acme/widgets"),
            ("UNBLOCK_PROJECT", "42"),
            ("UNBLOCK_AGENT", "my-bot"),
            ("UNBLOCK_CACHE_TTL", "120"),
            ("UNBLOCK_LOG_LEVEL", "debug"),
            ("UNBLOCK_OTEL_ENDPOINT", "http://otel:4317"),
        ]);
        let config = Config::load_from(env).expect("should load");

        assert_eq!(config.token, "ghp_override");
        // Trailing slash stripped:
        assert_eq!(config.api_base_url, "https://ghe.example.com/api/v3");
        assert_eq!(config.repo.as_deref(), Some("acme/widgets"));
        assert_eq!(config.project_number, Some(42));
        assert_eq!(config.agent, "my-bot");
        assert_eq!(config.cache_ttl, 120);
        assert_eq!(config.log_level, "debug");
        assert_eq!(config.otel_endpoint.as_deref(), Some("http://otel:4317"));
    }

    #[test]
    fn invalid_cache_ttl_returns_validation_error() {
        let env = make_env(&[
            ("GITHUB_TOKEN", "ghp_test"),
            ("UNBLOCK_CACHE_TTL", "not-a-number"),
        ]);
        let err = Config::load_from(env).unwrap_err();
        assert!(err.to_string().contains("UNBLOCK_CACHE_TTL"));
    }

    #[test]
    fn invalid_project_number_returns_validation_error() {
        let env = make_env(&[("GITHUB_TOKEN", "ghp_test"), ("UNBLOCK_PROJECT", "abc")]);
        let err = Config::load_from(env).unwrap_err();
        assert!(err.to_string().contains("UNBLOCK_PROJECT"));
    }

    #[test]
    fn api_base_url_trailing_slashes_stripped() {
        let env = make_env(&[
            ("GITHUB_TOKEN", "ghp_test"),
            ("GITHUB_API_URL", "https://api.example.com///"),
        ]);
        let config = Config::load_from(env).expect("should load");
        assert_eq!(config.api_base_url, "https://api.example.com");
    }

    #[test]
    fn validation_errors_have_correct_status_code() {
        let env = make_env(&[]);
        let err = Config::load_from(env).unwrap_err();
        assert_eq!(err.status_code(), 400);
    }

    #[test]
    fn valid_repo_format_accepted() {
        let env = make_env(&[
            ("GITHUB_TOKEN", "ghp_test"),
            ("UNBLOCK_REPO", "acme/widgets"),
        ]);
        let config = Config::load_from(env).expect("should load");
        assert_eq!(config.repo.as_deref(), Some("acme/widgets"));
    }

    #[test]
    fn repo_missing_slash_returns_validation_error() {
        let env = make_env(&[
            ("GITHUB_TOKEN", "ghp_test"),
            ("UNBLOCK_REPO", "just-a-name"),
        ]);
        let err = Config::load_from(env).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("UNBLOCK_REPO"),
            "error should mention UNBLOCK_REPO: {msg}"
        );
        assert!(
            msg.contains("owner/repo"),
            "error should mention expected format: {msg}"
        );
    }

    #[test]
    fn repo_extra_segments_returns_validation_error() {
        let env = make_env(&[
            ("GITHUB_TOKEN", "ghp_test"),
            ("UNBLOCK_REPO", "owner/repo/extra"),
        ]);
        let err = Config::load_from(env).unwrap_err();
        assert!(err.to_string().contains("UNBLOCK_REPO"));
    }

    #[test]
    fn repo_empty_owner_returns_validation_error() {
        let env = make_env(&[("GITHUB_TOKEN", "ghp_test"), ("UNBLOCK_REPO", "/repo")]);
        let err = Config::load_from(env).unwrap_err();
        assert!(err.to_string().contains("UNBLOCK_REPO"));
    }

    #[test]
    fn repo_empty_repo_returns_validation_error() {
        let env = make_env(&[("GITHUB_TOKEN", "ghp_test"), ("UNBLOCK_REPO", "owner/")]);
        let err = Config::load_from(env).unwrap_err();
        assert!(err.to_string().contains("UNBLOCK_REPO"));
    }

    #[test]
    fn repo_just_slash_returns_validation_error() {
        let env = make_env(&[("GITHUB_TOKEN", "ghp_test"), ("UNBLOCK_REPO", "/")]);
        let err = Config::load_from(env).unwrap_err();
        assert!(err.to_string().contains("UNBLOCK_REPO"));
    }

    #[test]
    fn repo_empty_string_returns_validation_error() {
        let env = make_env(&[("GITHUB_TOKEN", "ghp_test"), ("UNBLOCK_REPO", "")]);
        let err = Config::load_from(env).unwrap_err();
        assert!(err.to_string().contains("UNBLOCK_REPO"));
    }

    #[test]
    fn repo_not_set_is_none() {
        let env = make_env(&[("GITHUB_TOKEN", "ghp_test")]);
        let config = Config::load_from(env).expect("should load");
        assert_eq!(config.repo, None);
    }

    #[test]
    fn invalid_log_level_returns_validation_error() {
        let env = make_env(&[
            ("GITHUB_TOKEN", "ghp_test"),
            ("UNBLOCK_LOG_LEVEL", "verbose"),
        ]);
        let err = Config::load_from(env).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("UNBLOCK_LOG_LEVEL"),
            "error should mention UNBLOCK_LOG_LEVEL: {msg}"
        );
        assert!(
            msg.contains("verbose"),
            "error should echo the invalid value: {msg}"
        );
    }

    #[test]
    fn valid_log_levels_accepted() {
        for level in &["error", "warn", "info", "debug", "trace"] {
            let vars = [("GITHUB_TOKEN", "ghp_test"), ("UNBLOCK_LOG_LEVEL", level)];
            let env = make_env(&vars);
            let config = Config::load_from(env).unwrap_or_else(|e| {
                panic!("level '{level}' should be valid but got error: {e}");
            });
            assert_eq!(config.log_level, *level);
        }
    }

    #[test]
    fn case_insensitive_log_level() {
        for level in &["INFO", "Debug", "TRACE", "Warn", "ERROR"] {
            let vars = [("GITHUB_TOKEN", "ghp_test"), ("UNBLOCK_LOG_LEVEL", level)];
            let env = make_env(&vars);
            let config = Config::load_from(env).unwrap_or_else(|e| {
                panic!("level '{level}' should be valid (case-insensitive) but got error: {e}");
            });
            assert_eq!(
                config.log_level,
                level.to_lowercase(),
                "log_level should be normalized to lowercase for '{level}'"
            );
        }
    }
}
