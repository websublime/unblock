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
