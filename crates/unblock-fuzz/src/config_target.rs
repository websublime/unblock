//! The `config_toml` fuzz core (`run_config_toml_case`) — PRD §4 D57, implementation-plan T4.2 (7).
//!
//! The SAME bytes are parsed as BOTH TOML layers through `unblock-config`:
//!
//! - **as the project file** `.unblock/config.toml` — `ProjectConfig::load` and the resolve-only open
//!   (`open_workspace_with_env` over an injected environment, NFR-16) are total: `Ok`, or a
//!   non-empty structured error whose code is `CONFIG_PARSE_ERROR`, `CONFIG_ERROR` or `IO_ERROR`
//!   (bytes that are not UTF-8 fail the read). When the open succeeds, the layered precedence
//!   CLI > env (`UNBLOCK_*`) > project > defaults holds for `output_format`, and `actor` and
//!   `search_cap` name the project layer exactly when the file sets them;
//! - **as the user file** `$XDG_CONFIG_HOME/unblock/config.toml` — `load_user_config` agrees with
//!   the project parse DIFFERENTIALLY: the same parse/credential refusal with the same code; when the
//!   project parse accepts a file carrying any startup key (either `jsonl_filename` spelling) the
//!   user load refuses it with `StartupKeyInRuntimeLayer { layer: User }` — never a panic and never a
//!   silent accept; a `jsonl_export` is refused as `InvalidValue` naming it; a layer value outside
//!   its per-key bounds (a non-blank out-of-policy `actor`, a `search_cap` outside `1..=i64::MAX`) is
//!   refused as `InvalidValue` naming the key, so `unblock init`'s pre-check refuses every file the
//!   open would; and otherwise the user load accepts the file and parses it to the identical
//!   `ProjectConfig`.

// The core asserts its invariants (a breach = the bug libFuzzer reports); a `# Panics` section would
// be noise.
#![allow(clippy::missing_panics_doc)]

use std::collections::HashMap;
use std::path::Path;

use unblock_config::{
    CliOverrides, ConfigError, EnvSource, KeyClass, OutputFormat, ProjectConfig, ResolvedContext,
    canonicalize_runtime_value, classify, classify_runtime_key, load_user_config,
    open_workspace_with_env,
};
use unblock_error::{CodedError, ErrorCode};
use unblock_model::ConfigSource;

use crate::FuzzError;
use crate::invariants::{assert_config_precedence, assert_nonempty_structured_error};
use crate::tokio_block_on;

/// An injected environment (NFR-16): the core never reads the process environment.
#[derive(Debug, Default)]
struct MapEnv(HashMap<String, String>);

impl MapEnv {
    fn with(mut self, key: &str, value: &str) -> Self {
        self.0.insert(key.to_string(), value.to_string());
        self
    }
}

impl EnvSource for MapEnv {
    fn get(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
}

/// The codes a refused config file may map to: a TOML or typed-key failure, a refused value
/// (credential, startup key, `jsonl_export`, an out-of-bound value), or an unreadable (non-UTF-8)
/// file.
const CONFIG_FILE_CODES: [ErrorCode; 3] = [
    ErrorCode::ConfigParseError,
    ErrorCode::ConfigError,
    ErrorCode::IoError,
];

/// Assert `err` is a well-formed structured error with one of [`CONFIG_FILE_CODES`].
fn assert_config_file_error(err: &ConfigError) {
    assert_nonempty_structured_error(err);
    assert!(
        CONFIG_FILE_CODES.contains(&err.code()),
        "a config-file refusal must map to CONFIG_PARSE_ERROR/CONFIG_ERROR/IO_ERROR, got {:?}: {err}",
        err.code()
    );
}

/// **`config_toml`** — see the module doc for the invariants.
///
/// # Errors
///
/// Returns [`FuzzError::Io`] if the temp directories or files cannot be created (an environment
/// problem, not an input bug); every input reaches the parse and yields `Ok` unless an invariant
/// breaks (a panic).
pub fn run_config_toml_case(data: &[u8]) -> Result<(), FuzzError> {
    let root = tempfile::tempdir()?;
    let unblock_dir = root.path().join(".unblock");
    std::fs::create_dir_all(&unblock_dir)?;
    std::fs::write(unblock_dir.join("config.toml"), data)?;

    let xdg = root.path().join("xdg");
    let user_dir = xdg.join("unblock");
    std::fs::create_dir_all(&user_dir)?;
    std::fs::write(user_dir.join("config.toml"), data)?;
    let Some(xdg) = xdg.to_str() else {
        // A temp path that is not UTF-8 cannot be handed to an `EnvSource`; nothing to fuzz.
        return Ok(());
    };

    let project = ProjectConfig::load(&unblock_dir);
    if let Err(err) = &project {
        assert_config_file_error(err);
    }
    check_project_open(root.path(), project.as_ref().ok());
    check_user_layer(&MapEnv::default().with("XDG_CONFIG_HOME", xdg), &project);
    Ok(())
}

/// Resolve-only open of the project file: total, and when it succeeds the layered precedence holds.
fn check_project_open(root: &Path, project: Option<&ProjectConfig>) {
    let open = |cli: &CliOverrides, env: &MapEnv| -> Result<ResolvedContext, ConfigError> {
        tokio_block_on(open_workspace_with_env(Some(root), cli, env))
    };
    let base = match open(&CliOverrides::default(), &MapEnv::default()) {
        Ok(ctx) => ctx,
        Err(err) => {
            assert_config_file_error(&err);
            return;
        }
    };
    let project = project.expect("an open that succeeds has parsed the project file");

    // Project > defaults, for every key the file can carry at this layer.
    assert_config_precedence(
        &base.config.runtime,
        project.output_format.unwrap_or(OutputFormat::Json),
        if project.output_format.is_some() {
            ConfigSource::Project
        } else {
            ConfigSource::Default
        },
    );
    let expected_source = |set: bool| {
        if set {
            ConfigSource::Project
        } else {
            ConfigSource::Default
        }
    };
    assert_eq!(
        base.config.runtime.sources.search_cap,
        expected_source(project.search_cap.is_some()),
        "search_cap names the project layer exactly when the file sets it"
    );
    if let Some(cap) = project.search_cap {
        assert_eq!(
            base.config.runtime.search_cap, cap,
            "the project search_cap wins"
        );
    }
    let project_actor = project
        .actor
        .as_deref()
        .map(str::trim)
        .filter(|a| !a.is_empty());
    if let Some(actor) = project_actor {
        assert_eq!(base.config.runtime.actor, actor, "the project actor wins");
        assert_eq!(base.config.runtime.sources.actor, ConfigSource::Project);
    }

    // env > project: the overlay changes only `output_format`, so it cannot fail where the base
    // open succeeded.
    let env = MapEnv::default().with("UNBLOCK_OUTPUT_FORMAT", "csv");
    let with_env = open(&CliOverrides::default(), &env)
        .unwrap_or_else(|err| panic!("an env output_format overlay cannot fail the open: {err}"));
    assert_config_precedence(
        &with_env.config.runtime,
        OutputFormat::Csv,
        ConfigSource::Env,
    );

    // CLI > env > project.
    let cli = CliOverrides::new().with_output_format(OutputFormat::Plain);
    let with_cli = open(&cli, &env)
        .unwrap_or_else(|err| panic!("a CLI output_format overlay cannot fail the open: {err}"));
    assert_config_precedence(
        &with_cli.config.runtime,
        OutputFormat::Plain,
        ConfigSource::Cli,
    );
}

/// The user load agrees with the project parse (the module doc's differential).
fn check_user_layer(env: &MapEnv, project: &Result<ProjectConfig, ConfigError>) {
    let user = load_user_config(env);
    match (project, &user) {
        (Err(project_err), Err(user_err)) => {
            assert_config_file_error(user_err);
            assert_eq!(
                project_err.code(),
                user_err.code(),
                "the user file fails its shared parse/credential step with the project's code"
            );
        }
        (Err(project_err), Ok(_)) => {
            panic!(
                "the user loader accepted a file the shared project parse refuses: {project_err}"
            )
        }
        (Ok(parsed), _) => {
            if let Some(key) = startup_key(parsed) {
                match &user {
                    Err(ConfigError::StartupKeyInRuntimeLayer { layer, .. }) => assert_eq!(
                        *layer,
                        ConfigSource::User,
                        "a user-file startup key is refused naming the user layer"
                    ),
                    Err(err) => {
                        panic!(
                            "a user-file startup key `{key}` must be StartupKeyInRuntimeLayer: {err}"
                        )
                    }
                    Ok(_) => panic!("a user-file startup key `{key}` was silently accepted"),
                }
                assert_config_file_error(user.as_ref().expect_err("refused above"));
            } else if parsed.jsonl_export.is_some() {
                match &user {
                    Err(err @ ConfigError::InvalidValue { key, .. }) if key == "jsonl_export" => {
                        assert_config_file_error(err);
                    }
                    other => panic!("a user-file jsonl_export must be InvalidValue: {other:?}"),
                }
            } else if let Some(key) = out_of_bound_layer_key(parsed) {
                match &user {
                    Err(err @ ConfigError::InvalidValue { key: refused, .. }) if refused == key => {
                        assert_config_file_error(err);
                    }
                    other => panic!(
                        "a user-file `{key}` out of its layer bounds must be InvalidValue: {other:?}"
                    ),
                }
            } else {
                let accepted = user.as_ref().unwrap_or_else(|err| {
                    panic!("the user loader refused an in-bounds file with no startup key: {err}")
                });
                assert_eq!(accepted, parsed, "both layers parse the same grammar");
            }
        }
    }
}

/// The first LAYER key whose user-file value is out of the per-key bounds the user loader applies
/// (PRD §4 D57 clause (6)) — judged independently through the public `canonicalize_runtime_value`,
/// the same parser a `config set` value passes: `actor` first (a BLANK actor is unset, as at every
/// actor tier, so never out of bounds), then `search_cap`.
fn out_of_bound_layer_key(config: &ProjectConfig) -> Option<&'static str> {
    let bad = |key: &str, raw: String| {
        let rk = classify_runtime_key(key).expect("a layer key");
        canonicalize_runtime_value(rk, &raw).is_err()
    };
    if let Some(actor) = &config.actor
        && !actor.trim().is_empty()
        && bad("actor", actor.clone())
    {
        return Some("actor");
    }
    if let Some(cap) = config.search_cap
        && bad("search_cap", cap.to_string())
    {
        return Some("search_cap");
    }
    None
}

/// The first startup key a parsed file carries — a typed startup field, or a captured extra key
/// that classifies as startup (e.g. the `jsonl_filename` spelling).
fn startup_key(config: &ProjectConfig) -> Option<String> {
    let typed = [
        ("db_filename", config.db_filename.is_some()),
        (
            "jsonl_export_filename",
            config.jsonl_export_filename.is_some(),
        ),
        ("id_prefix", config.id_prefix.is_some()),
        (
            "write_lock_timeout_ms",
            config.write_lock_timeout_ms.is_some(),
        ),
        ("backend", config.backend.is_some()),
        (
            "deletions_retention_days",
            config.deletions_retention_days.is_some(),
        ),
    ];
    typed
        .into_iter()
        .find_map(|(key, present)| present.then(|| key.to_string()))
        .or_else(|| {
            config
                .extra
                .keys()
                .find(|k| matches!(classify(k), Some(KeyClass::Startup(_))))
                .cloned()
        })
}

/// The `ConfigCorpus` TOML layer builders (fuzz plan, v1.1 harness items): the key and value
/// vocabularies a grammar-shaped generator draws from, and the builder that renders the chosen
/// entries as one config-file document, so a smoke reaches the accept, precedence and
/// startup-refusal arms instead of leaving them to random bytes.
pub struct ConfigCorpus;

impl ConfigCorpus {
    /// Every key class a config file can carry: the three layer keys, `jsonl_export`, every startup
    /// key (both `jsonl_filename` spellings) and an unknown key.
    pub const KEYS: [&'static str; 12] = [
        "actor",
        "output_format",
        "search_cap",
        "jsonl_export",
        "db_filename",
        "jsonl_filename",
        "jsonl_export_filename",
        "id_prefix",
        "write_lock_timeout_ms",
        "backend",
        "deletions_retention_days",
        "future_key",
    ];

    /// Well- and ill-typed TOML values (strings, a blank actor, in- and out-of-range integers, a
    /// boolean).
    pub const VALUES: [&'static str; 9] = [
        "\"ann\"",
        "\"csv\"",
        "\"markdown\"",
        "7",
        "0",
        "-1",
        "true",
        "\"\"",
        "\"   \"",
    ];

    /// Render `entries` as one TOML document, one `key = value` line each; a repeated key keeps its
    /// FIRST value, so the document never fails on a duplicate key by construction.
    #[must_use]
    pub fn document(entries: &[(&str, &str)]) -> String {
        use std::fmt::Write as _;
        let mut seen = std::collections::BTreeSet::new();
        let mut document = String::new();
        for (key, value) in entries.iter().filter(|(key, _)| seen.insert(*key)) {
            // Writing to a `String` cannot fail.
            let _ = writeln!(document, "{key} = {value}");
        }
        document
    }
}

#[cfg(test)]
mod tests {
    use super::run_config_toml_case;

    /// Every class of input reaches the core's deep path without an invariant breach.
    #[test]
    fn representative_inputs_hold_every_invariant() {
        for input in [
            &b""[..],
            b"output_format = \"markdown\"\nsearch_cap = 7\nactor = \"  ann  \"\n",
            b"db_filename = \"x.db\"\n",
            b"jsonl_filename = \"x.jsonl\"\n",
            b"jsonl_export = true\n",
            b"auth_token = \"secret\"\n",
            b"future_key = 1\n",
            b"search_cap = 0\n",
            b"output_format = 3\n",
            b"[unclosed\n",
            b"\xff\xfe not utf-8",
        ] {
            run_config_toml_case(input).expect("the core runs");
        }
    }
}
