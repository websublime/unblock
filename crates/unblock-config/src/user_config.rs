//! The USER layer (FR-13 full, PRD §4 D57 clause (3)) — the per-person `config.toml`.
//!
//! **Path.** `$XDG_CONFIG_HOME/unblock/config.toml` when `XDG_CONFIG_HOME` is set to a non-empty
//! ABSOLUTE path, else `<home>/.config/unblock/config.toml`, on EVERY operating system, where
//! `<home>` is the first of `HOME`, `USERPROFILE` that is non-empty and ABSOLUTE — all read through
//! the injected [`EnvSource`] (NFR-16). A relative `XDG_CONFIG_HOME` is ignored (as the XDG Base
//! Directory specification requires), and a relative `HOME`/`USERPROFILE` is treated the same way —
//! as no home — so the user file can never resolve against the process's working directory. No
//! `directories` dependency: its platform paths (`~/Library/Application Support`, `%APPDATA%`) are
//! NOT used. No home and no `XDG_CONFIG_HOME` is an empty layer, never an error; so is an absent file.
//!
//! **Content.** The project file's TOML grammar ([`ProjectConfig`]), READ ONLY (unblock never writes
//! it), carrying the three LAYER keys only — `actor`, `output_format`, `search_cap`.
//! [`load_user_config`] is the SINGLE HOME of every user-file refusal, applied in this order: the TOML
//! parse ([`ConfigError::Parse`] → `CONFIG_PARSE_ERROR`), the NFR-18 credential deny, the startup-key
//! refusal ([`ConfigError::StartupKeyInRuntimeLayer`] with `layer: User`, either `jsonl_filename`
//! spelling), the `jsonl_export` refusal ([`ConfigError::InvalidValue`] naming `ub-p1h`) and the
//! layer-value bounds (a blank or out-of-policy `actor`, a `search_cap` outside `1..=i64::MAX` —
//! [`ConfigError::InvalidValue`]), so `unblock init`'s pre-check refuses every user file the open
//! would (PRD §4 D57 clause (6)). An
//! unknown key keeps the SF-4 rule (warned on `unblock.config`, ignored). Every path that reads the
//! file — both open facades, the reloader and [`user_output_format`] — goes through it, so no path
//! can accept a user file another path refuses.

use std::path::{Path, PathBuf};

use snafu::ResultExt;
use unblock_model::{ConfigSource, OutputFormat};

use crate::env::EnvSource;
use crate::error::{ConfigError, IoSnafu};
use crate::keys::{KeyClass, RuntimeKey, STARTUP_KEYS, classify};
use crate::reload::{jsonl_export_refusal, parse_runtime_value};
use crate::schema::{CONFIG_FILENAME, ProjectConfig};

/// The directory under the user configuration root that holds unblock's user file.
const USER_CONFIG_DIR: &str = "unblock";

/// The user file's path for `env` (PRD §4 D57 clause (3)), or `None` when neither an absolute
/// `XDG_CONFIG_HOME` nor an absolute home is set. The file need not exist.
#[must_use]
pub fn user_config_path(env: &dyn EnvSource) -> Option<PathBuf> {
    if let Some(xdg) = absolute(env.get("XDG_CONFIG_HOME")) {
        return Some(xdg.join(USER_CONFIG_DIR).join(CONFIG_FILENAME));
    }
    ["HOME", "USERPROFILE"]
        .into_iter()
        .find_map(|key| absolute(env.get(key)))
        .map(|home| {
            home.join(".config")
                .join(USER_CONFIG_DIR)
                .join(CONFIG_FILENAME)
        })
}

/// `value` as a path when it is non-empty and ABSOLUTE; a relative value is treated as unset.
fn absolute(value: Option<String>) -> Option<PathBuf> {
    value
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

/// Load the user file for `env` — the SINGLE HOME of every user-file refusal (PRD §4 D57 clause
/// (3)/(6)). `unblock init` calls it BEFORE it creates `.unblock/` or writes any file, so EVERY user
/// file a later open would refuse is refused here first — which is why the layer-value bounds the
/// open applies are applied here too, not only after the merge.
///
/// # Errors
///
/// In order: [`ConfigError::Io`] (a file that cannot be stat'ed or read — never silently an empty
/// layer), [`ConfigError::Parse`] (malformed TOML or a wrong-typed known key, e.g. a non-wire
/// `output_format`), [`ConfigError::InvalidValue`] (a credential, NFR-18),
/// [`ConfigError::StartupKeyInRuntimeLayer`] (`layer: User` — any startup key, either
/// `jsonl_filename` spelling), [`ConfigError::InvalidValue`] (`jsonl_export`, naming `ub-p1h`),
/// then [`ConfigError::InvalidValue`] for a layer value out of bounds — an `actor` that is blank or
/// fails `validate_actor` after trimming, a `search_cap` outside `1..=i64::MAX` — by the same per-key
/// rules a `config set` value and a database row are held to.
pub fn load_user_config(env: &dyn EnvSource) -> Result<ProjectConfig, ConfigError> {
    load_user_file(user_config_path(env).as_deref())
}

/// The user file's `output_format` for `env` — `load_user_config(env)?.output_format` — the ONE
/// entry the cli's no-workspace format resolver uses, so the cli never parses TOML itself and
/// `version` warns on exactly the refusals [`load_user_config`] makes.
///
/// # Errors
///
/// As [`load_user_config`].
pub fn user_output_format(env: &dyn EnvSource) -> Result<Option<OutputFormat>, ConfigError> {
    Ok(load_user_config(env)?.output_format)
}

/// Load the user file at `path` (`None` or an absent file → the empty layer) with every user-file
/// refusal — the body [`load_user_config`] and the reloader share.
///
/// # Errors
///
/// As [`load_user_config`].
pub(crate) fn load_user_file(path: Option<&Path>) -> Result<ProjectConfig, ConfigError> {
    let Some(path) = path else {
        return Ok(ProjectConfig::default());
    };
    // `try_exists`, not `exists`: a stat error (EACCES on a parent, ELOOP) is an `Io` error, never
    // a silently empty layer that drops the user's `actor`/`output_format` from resolution.
    if !path.try_exists().context(IoSnafu {
        path: path.to_path_buf(),
    })? {
        return Ok(ProjectConfig::default());
    }
    // Parse + NFR-18 credential deny (the project file's own rules), unknown keys NOT yet warned:
    // a startup key spelled outside the typed fields (`jsonl_filename`) must be refused, not
    // warned as unknown.
    let user = ProjectConfig::parse_file(path)?;
    if let Some(key) = startup_key_in(&user) {
        return Err(ConfigError::StartupKeyInRuntimeLayer {
            key: key.to_string(),
            layer: ConfigSource::User,
        });
    }
    if let Some(value) = user.jsonl_export {
        return Err(jsonl_export_refusal(&value.to_string()));
    }
    check_layer_bounds(&user)?;
    user.warn_unknown_keys();
    Ok(user)
}

/// The layer-value bounds (PRD §4 D57 clauses (4)/(5)/(6)): the user file's `actor` and
/// `search_cap` must each pass the per-key parser a `config set` value and a database row pass, so a
/// user file the open would refuse is refused by [`load_user_config`] itself — before `init` writes
/// anything. (`output_format` is a closed enum, already enforced by the parse.)
fn check_layer_bounds(user: &ProjectConfig) -> Result<(), ConfigError> {
    if let Some(actor) = &user.actor {
        parse_runtime_value(RuntimeKey::Actor, actor)?;
    }
    if let Some(cap) = user.search_cap {
        parse_runtime_value(RuntimeKey::SearchCap, &cap.to_string())?;
    }
    Ok(())
}

/// The first startup key `user` carries, by its wire spelling ([`STARTUP_KEYS`] order for the typed
/// fields, then any captured extra key that classifies as startup — e.g. the `jsonl_filename`
/// spelling of the TOML key `jsonl_export_filename`).
fn startup_key_in(user: &ProjectConfig) -> Option<&str> {
    let typed = [
        ("db_filename", user.db_filename.is_some()),
        (
            "jsonl_export_filename",
            user.jsonl_export_filename.is_some(),
        ),
        ("id_prefix", user.id_prefix.is_some()),
        (
            "write_lock_timeout_ms",
            user.write_lock_timeout_ms.is_some(),
        ),
        ("backend", user.backend.is_some()),
        (
            "deletions_retention_days",
            user.deletions_retention_days.is_some(),
        ),
    ];
    debug_assert_eq!(typed.len(), STARTUP_KEYS.len());
    typed
        .into_iter()
        .find_map(|(key, present)| present.then_some(key))
        .or_else(|| {
            user.extra
                .keys()
                .find(|k| matches!(classify(k), Some(KeyClass::Startup(_))))
                .map(String::as_str)
        })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    use unblock_error::{CodedError, ErrorCode};
    use unblock_model::OutputFormat;

    use super::{load_user_config, user_config_path, user_output_format};
    use crate::env::EnvSource;
    use crate::error::ConfigError;

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

    /// An absolute path that is valid on every OS (a tempdir is absolute everywhere).
    fn abs(dir: &Path, leaf: &str) -> String {
        dir.join(leaf).to_string_lossy().into_owned()
    }

    /// **D57 criterion (f) — USER PATH.** An absolute `XDG_CONFIG_HOME` wins; a relative one is
    /// ignored; the first ABSOLUTE of `HOME`, `USERPROFILE` supplies `<home>/.config/unblock/
    /// config.toml`; a relative `HOME` is no home; no home is no path.
    ///
    /// MUTANT KILLED: accepting a relative `XDG_CONFIG_HOME`/`HOME` (the file would resolve
    /// against the process's working directory), or preferring `HOME` over `XDG_CONFIG_HOME`.
    #[test]
    fn user_path_rules() {
        let tmp = tempfile::tempdir().unwrap();
        let xdg = abs(tmp.path(), "xdg");
        let home = abs(tmp.path(), "home");
        let profile = abs(tmp.path(), "profile");

        let env = MapEnv::new(&[("XDG_CONFIG_HOME", &xdg), ("HOME", &home)]);
        assert_eq!(
            user_config_path(&env),
            Some(PathBuf::from(&xdg).join("unblock").join("config.toml"))
        );

        let env = MapEnv::new(&[("XDG_CONFIG_HOME", "relative/xdg"), ("HOME", &home)]);
        assert_eq!(
            user_config_path(&env),
            Some(
                PathBuf::from(&home)
                    .join(".config")
                    .join("unblock")
                    .join("config.toml")
            ),
            "a relative XDG_CONFIG_HOME is ignored"
        );

        let env = MapEnv::new(&[("HOME", "relative/home"), ("USERPROFILE", &profile)]);
        assert_eq!(
            user_config_path(&env),
            Some(
                PathBuf::from(&profile)
                    .join(".config")
                    .join("unblock")
                    .join("config.toml")
            ),
            "a relative HOME is no home; the next absolute candidate supplies it"
        );

        let env = MapEnv::new(&[("HOME", "relative/home"), ("XDG_CONFIG_HOME", "")]);
        assert_eq!(user_config_path(&env), None, "no absolute home → no path");
        assert_eq!(user_config_path(&MapEnv::new(&[])), None);
    }

    /// **D57 criterion (f) — "the FIRST absolute of `HOME`, `USERPROFILE`"** with BOTH absolute:
    /// `HOME` wins.
    ///
    /// MUTANT KILLED: probing `USERPROFILE` before `HOME`.
    #[test]
    fn home_beats_userprofile_when_both_are_absolute() {
        let tmp = tempfile::tempdir().unwrap();
        let home = abs(tmp.path(), "home");
        let profile = abs(tmp.path(), "profile");
        let env = MapEnv::new(&[("HOME", &home), ("USERPROFILE", &profile)]);
        assert_eq!(
            user_config_path(&env),
            Some(
                PathBuf::from(&home)
                    .join(".config")
                    .join("unblock")
                    .join("config.toml")
            )
        );
    }

    /// **D57 clause (6), the user-file half of "any user file the open would refuse"**: the
    /// layer-value bounds are applied by `load_user_config` itself — a `search_cap` of `0` or above
    /// `i64::MAX`, a blank actor and an over-long actor are each `InvalidValue` (`CONFIG_ERROR`)
    /// naming the key — so `unblock init`'s pre-check refuses them before it writes anything.
    ///
    /// MUTANT KILLED: dropping `check_layer_bounds` (the open would then refuse a file `init`'s
    /// pre-check accepted, leaving a half-made scaffold).
    #[test]
    fn out_of_bound_layer_values_are_refused() {
        let long_actor = format!("actor = \"{}\"\n", "a".repeat(201));
        for (contents, key) in [
            ("search_cap = 0\n", "search_cap"),
            ("search_cap = 9223372036854775807\n", ""),
            ("actor = \"   \"\n", "actor"),
            (long_actor.as_str(), "actor"),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let env = user_file(tmp.path(), contents);
            let loaded = load_user_config(&env);
            if key.is_empty() {
                // i64::MAX itself is IN bounds.
                assert!(loaded.is_ok(), "{contents}: {loaded:?}");
                continue;
            }
            let err = loaded.expect_err(contents);
            assert!(
                matches!(&err, ConfigError::InvalidValue { key: k, .. } if k == key),
                "{contents}: {err:?}"
            );
            assert_eq!(err.code(), ErrorCode::ConfigError);
        }
    }

    /// A user file that cannot be STAT'ed (its "directory" is a regular file, `ENOTDIR`) is a
    /// `ConfigError::Io`, never a silently empty layer — `try_exists`, not `exists`.
    ///
    /// MUTANT KILLED: `path.exists()`, which maps every stat error to "absent".
    #[test]
    fn an_unstatable_user_file_is_an_io_error() {
        let tmp = tempfile::tempdir().unwrap();
        let not_a_dir = tmp.path().join("xdg-is-a-file");
        std::fs::write(&not_a_dir, "").unwrap();
        let env = MapEnv::new(&[("XDG_CONFIG_HOME", &not_a_dir.to_string_lossy())]);
        let err = load_user_config(&env).expect_err("a stat error is not an empty layer");
        assert!(matches!(err, ConfigError::Io { .. }), "{err:?}");
        assert_eq!(err.code(), ErrorCode::IoError);
    }

    /// Write `contents` as the user file under an absolute `XDG_CONFIG_HOME` and return the env.
    fn user_file(tmp: &Path, contents: &str) -> MapEnv {
        let dir = tmp.join("xdg").join("unblock");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), contents).unwrap();
        MapEnv::new(&[("XDG_CONFIG_HOME", &abs(tmp, "xdg"))])
    }

    /// No home, or an absent file, is an EMPTY layer — never an error.
    #[test]
    fn no_home_or_absent_file_is_an_empty_layer() {
        assert_eq!(
            load_user_config(&MapEnv::new(&[])).unwrap(),
            crate::schema::ProjectConfig::default()
        );
        let tmp = tempfile::tempdir().unwrap();
        let env = MapEnv::new(&[("XDG_CONFIG_HOME", &abs(tmp.path(), "nothing-here"))]);
        assert_eq!(
            load_user_config(&env).unwrap(),
            crate::schema::ProjectConfig::default()
        );
    }

    /// The three LAYER keys load; `user_output_format` reads the one the cli needs.
    #[test]
    fn layer_keys_load() {
        let tmp = tempfile::tempdir().unwrap();
        let env = user_file(
            tmp.path(),
            "actor = \"alice\"\noutput_format = \"robot\"\nsearch_cap = 7\n",
        );
        let user = load_user_config(&env).unwrap();
        assert_eq!(user.actor.as_deref(), Some("alice"));
        assert_eq!(user.search_cap, Some(7));
        assert_eq!(user_output_format(&env).unwrap(), Some(OutputFormat::Robot));
    }

    /// **D57 criterion (f) — a malformed file is `CONFIG_PARSE_ERROR` and an `auth_token` is
    /// denied (NFR-18).**
    #[test]
    fn malformed_file_and_credential_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let env = user_file(tmp.path(), "actor = [unterminated");
        let err = load_user_config(&env).expect_err("malformed");
        assert_eq!(err.code(), ErrorCode::ConfigParseError);

        let tmp = tempfile::tempdir().unwrap();
        let env = user_file(tmp.path(), "auth_token = \"secret\"\n");
        let err = load_user_config(&env).expect_err("credential denied");
        assert!(
            matches!(&err, ConfigError::InvalidValue { key, .. } if key == "auth_token"),
            "{err:?}"
        );
    }

    /// **D57 criterion (c) — USER STARTUP REFUSAL**: every startup key, in BOTH `jsonl_filename`
    /// spellings, is `StartupKeyInRuntimeLayer { layer: User }` → `CONFIG_ERROR`, naming the key.
    ///
    /// MUTANT KILLED: missing the `jsonl_filename` spelling (it is not a typed field, so it would
    /// otherwise be warned as unknown and IGNORED).
    #[test]
    fn every_startup_key_in_either_spelling_is_refused() {
        for (toml, key) in [
            ("db_filename = \"x.db\"", "db_filename"),
            (
                "jsonl_export_filename = \"x.jsonl\"",
                "jsonl_export_filename",
            ),
            ("jsonl_filename = \"x.jsonl\"", "jsonl_filename"),
            ("id_prefix = \"zz\"", "id_prefix"),
            ("write_lock_timeout_ms = 5000", "write_lock_timeout_ms"),
            ("backend = \"libsql\"", "backend"),
            ("deletions_retention_days = 3", "deletions_retention_days"),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let env = user_file(tmp.path(), toml);
            let err = load_user_config(&env).expect_err("startup key refused");
            assert_eq!(err.code(), ErrorCode::ConfigError, "{toml}");
            let structured: unblock_error::StructuredError = (&err).into();
            assert_eq!(structured.context["key"], key, "{toml}");
            assert_eq!(structured.context["layer"], "user", "{toml}");
            assert!(
                err.to_string().contains(key),
                "the message names the key: {err}"
            );
        }
    }

    /// **D57 criterion (e) — NON-LAYER `jsonl_export`**: a user-file `jsonl_export` is the
    /// EXISTING `InvalidValue` (`CONFIG_ERROR`) whose reason names `ub-p1h`.
    #[test]
    fn jsonl_export_in_the_user_file_is_refused_naming_ub_p1h() {
        let tmp = tempfile::tempdir().unwrap();
        let env = user_file(tmp.path(), "jsonl_export = true\n");
        let err = load_user_config(&env).expect_err("refused");
        assert_eq!(err.code(), ErrorCode::ConfigError);
        assert!(err.to_string().contains("ub-p1h"), "{err}");
    }

    /// **D57 criterion (e2) — an unknown user-file key is warned and IGNORED** (SF-4).
    #[test]
    fn an_unknown_user_key_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let env = user_file(tmp.path(), "actor = \"a\"\nfuture_knob = 1\n");
        let user = load_user_config(&env).expect("unknown keys never fail the load");
        assert_eq!(user.actor.as_deref(), Some("a"));
    }
}
