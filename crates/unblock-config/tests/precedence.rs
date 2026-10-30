//! Integration contract suite for FR-13 layer precedence — **CLI > env (`UNBLOCK_*`) > project
//! `config.toml` > defaults** — using injected env + tempfile project TOML (no process-global state).
//!
//! The precedence engine is driven via the public [`WorkspaceConfig::resolve`] seam with an injected
//! [`EnvSource`] (parallel-safe — never `std::env::set_var`) and a tempfile-loaded [`ProjectConfig`].
//! The end-to-end facade path (`open_workspace_with_cli`) is covered in `open_workspace.rs`.

use std::collections::HashMap;
use std::fs;

use unblock_config::{
    CliOverrides, EnvOverrides, EnvSource, OutputFormat, ProjectConfig, WorkspaceConfig,
};

/// An injected, parallel-safe environment source (no process env).
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

/// Load a `ProjectConfig` from a tempfile `config.toml` (exercises the real load + deny + warn path).
fn project_from_toml(contents: &str) -> (tempfile::TempDir, ProjectConfig) {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("config.toml"), contents).expect("write config.toml");
    let project = ProjectConfig::load(dir.path()).expect("load config.toml");
    (dir, project)
}

fn resolve(
    cli: &CliOverrides,
    env_pairs: &[(&str, &str)],
    project: &ProjectConfig,
) -> WorkspaceConfig {
    let map = MapEnv::new(env_pairs);
    let env = EnvOverrides::from_source(&map).expect("env parse");
    WorkspaceConfig::resolve(cli, &env, project, &map).expect("resolve")
}

#[test]
fn output_format_full_precedence_matrix() {
    // project sets csv; env sets plain; cli sets robot.
    let (_g, project) = project_from_toml("output_format = \"csv\"\n");

    // 1) cli wins over everything.
    let cli = CliOverrides::new().with_output_format(OutputFormat::Robot);
    let wc = resolve(&cli, &[("UNBLOCK_OUTPUT_FORMAT", "plain")], &project);
    assert_eq!(wc.output_format(), OutputFormat::Robot);

    // 2) env beats project.
    let wc = resolve(
        &CliOverrides::default(),
        &[("UNBLOCK_OUTPUT_FORMAT", "plain")],
        &project,
    );
    assert_eq!(wc.output_format(), OutputFormat::Plain);

    // 3) project beats defaults.
    let wc = resolve(&CliOverrides::default(), &[], &project);
    assert_eq!(wc.output_format(), OutputFormat::Csv);

    // 4) defaults when nothing set.
    let (_g2, empty) = project_from_toml("");
    let wc = resolve(&CliOverrides::default(), &[], &empty);
    assert_eq!(wc.output_format(), OutputFormat::Json);
}

#[test]
fn actor_fork4_global_order() {
    // cli > UNBLOCK_ACTOR > config.toml [actor] > $USER > "unblock".
    let (_g, project) = project_from_toml("actor = \"cfg\"\n");

    let cli = CliOverrides::new().with_actor("cli");
    assert_eq!(
        resolve(
            &cli,
            &[("UNBLOCK_ACTOR", "env"), ("USER", "user")],
            &project
        )
        .actor(),
        "cli"
    );
    assert_eq!(
        resolve(
            &CliOverrides::default(),
            &[("UNBLOCK_ACTOR", "env"), ("USER", "user")],
            &project
        )
        .actor(),
        "env"
    );
    assert_eq!(
        resolve(&CliOverrides::default(), &[("USER", "user")], &project).actor(),
        "cfg"
    );

    let (_g2, empty) = project_from_toml("");
    assert_eq!(
        resolve(&CliOverrides::default(), &[("USER", "user")], &empty).actor(),
        "user"
    );
    assert_eq!(
        resolve(&CliOverrides::default(), &[], &empty).actor(),
        "unblock"
    );
}

#[test]
fn jsonl_export_precedence() {
    let (_g, project) = project_from_toml("jsonl_export = false\n");
    // cli wins.
    let cli = CliOverrides::new().with_jsonl_export(true);
    assert!(resolve(&cli, &[("UNBLOCK_JSONL", "false")], &project).jsonl_export());
    // env beats project.
    assert!(
        resolve(
            &CliOverrides::default(),
            &[("UNBLOCK_JSONL", "true")],
            &project
        )
        .jsonl_export()
    );
    // project beats defaults (project=false, default=false here; flip project=true).
    let (_g2, p2) = project_from_toml("jsonl_export = true\n");
    assert!(resolve(&CliOverrides::default(), &[], &p2).jsonl_export());
    // default.
    let (_g3, empty) = project_from_toml("");
    assert!(!resolve(&CliOverrides::default(), &[], &empty).jsonl_export());
}

#[test]
fn empty_env_falls_through_to_project_layer() {
    // An EMPTY (whitespace-only or "") env var is normalized to unset, so the lower layer (project
    // config.toml) supplies the value — it is NEVER parsed as an empty value at the env layer.

    // (a) UNBLOCK_OUTPUT_FORMAT = "  " (whitespace) falls through to project output_format = "csv".
    let (_g, project) = project_from_toml("output_format = \"csv\"\n");
    let wc = resolve(
        &CliOverrides::default(),
        &[("UNBLOCK_OUTPUT_FORMAT", "  ")],
        &project,
    );
    assert_eq!(wc.output_format(), OutputFormat::Csv);

    // (b) UNBLOCK_JSONL = "" (empty) falls through to project jsonl_export = true.
    let (_g2, project) = project_from_toml("jsonl_export = true\n");
    let wc = resolve(&CliOverrides::default(), &[("UNBLOCK_JSONL", "")], &project);
    assert!(wc.jsonl_export());
}

#[test]
fn startup_keys_from_project_toml() {
    // db_filename / jsonl_export_filename / search_cap / deletions_retention_days / backend.
    let (_g, project) = project_from_toml(
        r#"
        db_filename = "alt.db"
        jsonl_export_filename = "alt.jsonl"
        search_cap = 17
        deletions_retention_days = 45
        backend = "libsql"
        "#,
    );
    let wc = resolve(&CliOverrides::default(), &[], &project);
    assert_eq!(wc.db_filename(), "alt.db");
    assert_eq!(wc.jsonl_filename(), "alt.jsonl");
    assert_eq!(wc.search_cap(), 17);
    assert_eq!(wc.deletions_retention_days(), Some(45));
    assert_eq!(wc.backend(), Some("libsql"));
}

#[test]
fn defaults_when_no_layer_sets_anything() {
    let (_g, empty) = project_from_toml("");
    let wc = resolve(&CliOverrides::default(), &[], &empty);
    assert_eq!(wc.output_format(), OutputFormat::Json);
    assert!(!wc.jsonl_export());
    assert_eq!(wc.search_cap(), 50);
    assert_eq!(wc.db_filename(), "unblock.db");
    assert_eq!(wc.jsonl_filename(), "issues.jsonl");
    assert_eq!(wc.actor(), "unblock");
    assert_eq!(wc.deletions_retention_days(), None);
    assert_eq!(wc.backend(), None);
}

#[test]
fn credential_in_config_toml_is_rejected() {
    // NFR-18: an auth token in config.toml is a hard error at load.
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(
        dir.path().join("config.toml"),
        "[remote]\nauth_token = \"secret\"\n",
    )
    .expect("write");
    let err = ProjectConfig::load(dir.path()).expect_err("credential denied");
    assert!(matches!(
        err,
        unblock_config::ConfigError::InvalidValue { .. }
    ));
}

// ================================================================================================
// FR-13 FULL (PRD §4 D57, v1.1) — SIX layers: CLI > env > project > user > DB > defaults.
// The runtime fold is driven through the public `ConfigReloader` (captured CLI/env/$USER, project
// and user FILES on disk, DB rows in memory) — the same resolution the open facade and a live
// session's reload run.
// ================================================================================================

/// One six-layer stack for a runtime key: which layers set it, each to its own value.
struct Stack<'a> {
    cli: Option<&'a str>,
    env: Option<&'a str>,
    project: Option<&'a str>,
    user: Option<&'a str>,
    db: Option<&'a str>,
    os_user: Option<&'a str>,
}

/// Resolve `key` over `stack` through a `ConfigReloader`; returns the runtime config.
fn resolve_six(key: &str, stack: &Stack<'_>) -> unblock_config::RuntimeConfig {
    let dir = tempfile::tempdir().expect("tempdir");
    let toml_line = |value: &str| -> String {
        if key == "search_cap" {
            format!("{key} = {value}\n")
        } else {
            format!("{key} = \"{value}\"\n")
        }
    };
    let project_file = dir.path().join("project.toml");
    fs::write(
        &project_file,
        stack.project.map(toml_line).unwrap_or_default(),
    )
    .unwrap();
    let user_file = dir.path().join("user.toml");
    fs::write(&user_file, stack.user.map(toml_line).unwrap_or_default()).unwrap();
    let mut cli = CliOverrides::default();
    let mut env = EnvOverrides::default();
    match key {
        "actor" => {
            cli.actor = stack.cli.map(str::to_string);
            env.actor = stack.env.map(str::to_string);
        }
        "output_format" => {
            let parse = |v: &str| serde_json::from_value(serde_json::json!(v)).unwrap();
            cli.output_format = stack.cli.map(parse);
            env.output_format = stack.env.map(parse);
        }
        _ => {}
    }
    let rows: Vec<(String, String)> = stack
        .db
        .map(|v| vec![(key.to_string(), v.to_string())])
        .unwrap_or_default();
    unblock_config::ConfigReloader::new(
        cli,
        env,
        stack.os_user.map(str::to_string),
        Some(project_file),
        Some(user_file),
    )
    .reload_runtime(&rows)
    .expect("resolves")
}

/// **D57 criterion (a) ORDER — every subset of the layers that CARRY each runtime key**: the
/// highest set layer wins and `source` names it (`actor`: CLI, env, project, user, DB, `$USER`, the
/// literal; `output_format`: CLI, env, project, user, DB, defaults; `search_cap`: project, user, DB,
/// defaults).
///
/// MUTANT KILLED: swapping the user and DB layers; letting `search_cap` read a CLI/env slot.
#[test]
fn six_layer_order_every_subset_per_runtime_key() {
    use unblock_model::ConfigSource as S;
    // (key, the layers that carry it in precedence order, a value per layer, default)
    type Carrier<'a> = (&'a str, &'a [(S, &'a str)], &'a str);
    let carriers: [Carrier<'_>; 3] = [
        (
            "actor",
            &[
                (S::Cli, "c"),
                (S::Env, "e"),
                (S::Project, "p"),
                (S::User, "u"),
                (S::Db, "d"),
                (S::OsUser, "o"),
            ],
            "unblock",
        ),
        (
            "output_format",
            &[
                (S::Cli, "robot"),
                (S::Env, "plain"),
                (S::Project, "csv"),
                (S::User, "markdown"),
                (S::Db, "plain"),
            ],
            "json",
        ),
        (
            "search_cap",
            &[(S::Project, "11"), (S::User, "22"), (S::Db, "33")],
            "50",
        ),
    ];
    for (key, layers, default) in carriers {
        for mask in 0u32..(1 << layers.len()) {
            let set = |s: S| {
                layers
                    .iter()
                    .enumerate()
                    .find(|(i, (l, _))| *l == s && mask & (1 << i) != 0)
                    .map(|(_, (_, v))| *v)
            };
            let stack = Stack {
                cli: set(S::Cli),
                env: set(S::Env),
                project: set(S::Project),
                user: set(S::User),
                db: set(S::Db),
                os_user: set(S::OsUser),
            };
            let runtime = resolve_six(key, &stack);
            let (value, source) = match key {
                "actor" => (runtime.actor.clone(), runtime.sources.actor),
                "output_format" => (
                    serde_json::to_value(runtime.output_format)
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .to_string(),
                    runtime.sources.output_format,
                ),
                _ => (runtime.search_cap.to_string(), runtime.sources.search_cap),
            };
            let expected = layers
                .iter()
                .enumerate()
                .find(|(i, _)| mask & (1 << i) != 0)
                .map_or((default.to_string(), S::Default), |(_, (s, v))| {
                    ((*v).to_string(), *s)
                });
            assert_eq!((value, source), expected, "{key} mask {mask:b}");
            assert!(runtime.db_faults.is_empty());
        }
    }
}

/// `jsonl_export` resolves over CLI > env > project > defaults ONLY (not a layer key, `ub-p1h`).
#[test]
fn jsonl_export_resolves_over_cli_env_project_defaults_only() {
    use unblock_model::ConfigSource as S;
    for (cli, env, project, expected) in [
        (Some(true), Some(false), Some(false), (true, S::Cli)),
        (None, Some(true), Some(false), (true, S::Env)),
        (None, None, Some(true), (true, S::Project)),
        (None, None, None, (false, S::Default)),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let project_file = dir.path().join("p.toml");
        let body = project.map_or(String::new(), |v| format!("jsonl_export = {v}\n"));
        fs::write(&project_file, body).unwrap();
        let runtime = unblock_config::ConfigReloader::new(
            CliOverrides {
                jsonl_export: cli,
                ..CliOverrides::default()
            },
            EnvOverrides {
                jsonl_export: env,
                ..EnvOverrides::default()
            },
            None,
            Some(project_file),
            None,
        )
        .reload_runtime(&[])
        .unwrap();
        assert_eq!(
            (runtime.jsonl_export, runtime.sources.jsonl_export),
            expected
        );
    }
}

/// **D57 criterion (a) — the GOLDEN six-layer resolution**: one representative stack with every
/// layer set for some key, pinned with its sources.
#[test]
fn six_layer_resolution_golden() {
    let dir = tempfile::tempdir().unwrap();
    let project_file = dir.path().join("project.toml");
    fs::write(&project_file, "jsonl_export = true\n").unwrap();
    let user_file = dir.path().join("user.toml");
    fs::write(&user_file, "output_format = \"markdown\"\nsearch_cap = 7\n").unwrap();
    let rows = vec![
        ("actor".to_string(), "db-actor".to_string()),
        ("search_cap".to_string(), "99".to_string()),
        ("db_filename".to_string(), "x.db".to_string()),
        ("zz_future".to_string(), "1".to_string()),
    ];
    let runtime = unblock_config::ConfigReloader::new(
        CliOverrides::default(),
        EnvOverrides::default(),
        Some("os-user".to_string()),
        Some(project_file),
        Some(user_file),
    )
    .reload_runtime(&rows)
    .unwrap();
    insta::assert_debug_snapshot!("six_layer_resolution", runtime);
}

/// **D57 criterion (i)** — a project `search_cap = 0` (accepted through v1) is now
/// `InvalidValue` (`CONFIG_ERROR`): the bound `1..=i64::MAX` holds on every layer.
#[test]
fn a_project_search_cap_of_zero_is_refused() {
    use unblock_error::{CodedError, ErrorCode};
    let (_dir, project) = project_from_toml("search_cap = 0\n");
    let map = MapEnv::new(&[]);
    let env = EnvOverrides::from_source(&map).unwrap();
    let err = WorkspaceConfig::resolve(&CliOverrides::default(), &env, &project, &map)
        .expect_err("0 is out of range");
    assert_eq!(err.code(), ErrorCode::ConfigError);
}

/// **D57 criterion (d)/(e)/(e2) at the open** — a startup-key row, an undecodable layer-key row, a
/// `jsonl_export` row and an unknown row: the storage-bearing open SUCCEEDS, each faulty row
/// contributes nothing and is recorded with its kind; only the unknown one is non-blocking.
#[tokio::test]
async fn faulty_db_rows_never_fail_the_open_and_are_recorded() {
    use unblock_model::ConfigFaultKind;
    let ws = tempfile::tempdir().unwrap();
    fs::create_dir(ws.path().join(".unblock")).unwrap();
    let env = MapEnv::new(&[]);
    let ctx =
        unblock_config::open_with_storage_with_env(Some(ws.path()), &CliOverrides::default(), &env)
            .await
            .expect("fresh open");
    for (k, v) in [
        ("db_filename", "x.db"),
        ("search_cap", "abc"),
        ("jsonl_export", "true"),
        ("zz", "1"),
    ] {
        ctx.storage.set_config(k, v).await.unwrap();
    }
    drop(ctx);
    let ctx =
        unblock_config::open_with_storage_with_env(Some(ws.path()), &CliOverrides::default(), &env)
            .await
            .expect("a faulty table never fails the open");
    let kinds: Vec<(&str, ConfigFaultKind)> = ctx
        .config
        .runtime
        .db_faults
        .iter()
        .map(|f| (f.key.as_str(), f.fault.kind))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("db_filename", ConfigFaultKind::StartupKey),
            ("jsonl_export", ConfigFaultKind::InvalidValue),
            ("search_cap", ConfigFaultKind::InvalidValue),
            ("zz", ConfigFaultKind::UnknownKey),
        ]
    );
    assert_eq!(
        ctx.config.runtime.search_cap, 50,
        "a faulty row contributes nothing"
    );
    assert_eq!(
        ctx.config.startup.db_filename, "unblock.db",
        "never read from the DB"
    );
    let err = ctx
        .config
        .runtime
        .blocking_fault_error()
        .expect("blocking faults gate");
    let structured: unblock_error::StructuredError = (&err).into();
    assert_eq!(
        structured.context["keys"],
        serde_json::json!(["db_filename", "jsonl_export", "search_cap"]),
        "the unknown row blocks nothing"
    );
}

/// **D57 criterion (c)** — a user-file startup key fails the open with `CONFIG_ERROR`,
/// `context.layer:"user"` and the key named.
#[tokio::test]
async fn a_user_file_startup_key_fails_the_open() {
    let ws = tempfile::tempdir().unwrap();
    fs::create_dir(ws.path().join(".unblock")).unwrap();
    let xdg = ws.path().join("xdg");
    fs::create_dir_all(xdg.join("unblock")).unwrap();
    fs::write(
        xdg.join("unblock").join("config.toml"),
        "id_prefix = \"zz\"\n",
    )
    .unwrap();
    let env = MapEnv::new(&[("XDG_CONFIG_HOME", xdg.to_str().unwrap())]);
    let err =
        unblock_config::open_workspace_with_env(Some(ws.path()), &CliOverrides::default(), &env)
            .await
            .expect_err("refused");
    let structured: unblock_error::StructuredError = (&err).into();
    assert_eq!(structured.code, unblock_error::ErrorCode::ConfigError);
    assert_eq!(structured.context["layer"], "user");
    assert_eq!(structured.context["key"], "id_prefix");
}
