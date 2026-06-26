//! Guard: legacy `BD_`/`BR_`/`BEADS_` env keys and YAML config are **not** recognized (D8/D10
//! regression guard). Locks the rename + single-prefix decision.

use std::collections::HashMap;
use std::fs;

use unblock_config::{
    CliOverrides, EnvOverrides, EnvSource, OutputFormat, ProjectConfig, WorkspaceConfig,
};

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
fn legacy_env_prefixes_are_ignored() {
    // BD_/BR_/BEADS_ must NOT influence resolution (D10 — single UNBLOCK_ prefix).
    let map = MapEnv::new(&[
        ("BD_ACTOR", "legacy-bd"),
        ("BR_ACTOR", "legacy-br"),
        ("BEADS_ACTOR", "legacy-beads"),
        ("BD_OUTPUT_FORMAT", "robot"),
        ("BEADS_JSONL", "true"),
        ("BEADS_DIR", "/legacy/dir"),
    ]);
    let env = EnvOverrides::from_source(&map).expect("env parse");
    // The parsed env layer is empty (no legacy keys recognized).
    assert_eq!(env, EnvOverrides::default());

    // Resolution falls through to defaults / $USER, never the legacy actor.
    let wc = WorkspaceConfig::resolve(
        &CliOverrides::default(),
        &env,
        &ProjectConfig::default(),
        &map,
    )
    .expect("resolve");
    assert_ne!(wc.actor(), "legacy-bd");
    assert_ne!(wc.actor(), "legacy-br");
    assert_ne!(wc.actor(), "legacy-beads");
    // Output format is the default (the legacy BD_OUTPUT_FORMAT="robot" was ignored).
    assert_eq!(wc.output_format(), OutputFormat::Json);
}

#[test]
fn yaml_config_is_not_loaded() {
    // A `config.yaml` next to a (present) `config.toml` is never read (D10 — TOML only). Here only a
    // config.yaml exists, so load must treat the workspace as having no config (all defaults).
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(
        dir.path().join("config.yaml"),
        "actor: yaml-actor\noutput_format: robot\n",
    )
    .expect("write yaml");

    let project = ProjectConfig::load(dir.path()).expect("load (no config.toml present)");
    // No config.toml -> all-None ProjectConfig (the YAML is ignored entirely).
    assert_eq!(project, ProjectConfig::default());
}
