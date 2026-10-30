//! Property tests (NFR-16): precedence totality (FR-13 invariant), schema raw→merged never panics,
//! and the startup/runtime key-classification exhaustiveness (a public re-check of `keys.rs`).

use std::collections::HashMap;

use proptest::prelude::*;
use unblock_config::{
    CliOverrides, EnvOverrides, EnvSource, OutputFormat, ProjectConfig, RUNTIME_KEYS, STARTUP_KEYS,
    WorkspaceConfig, classify,
};

struct MapEnv(HashMap<String, String>);
impl EnvSource for MapEnv {
    fn get(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
}

/// A small, safe output-format strategy (the five v1 formats).
fn any_output_format() -> impl Strategy<Value = OutputFormat> {
    prop_oneof![
        Just(OutputFormat::Json),
        Just(OutputFormat::Robot),
        Just(OutputFormat::Plain),
        Just(OutputFormat::Csv),
        Just(OutputFormat::Markdown),
    ]
}

/// The `snake_case` wire string for an [`OutputFormat`] (matches the serde rename — no `serde_json`
/// dep). Exhaustive in BOTH feature states: the five v1 formats are always present, and the
/// `#[cfg(feature = "toon")]` arm covers the model's feature-gated `Toon` variant so this `match`
/// compiles under `--all-features` (the `any_output_format` strategy never yields `Toon`, but the
/// arm keeps the mapping total and future-proof).
fn output_format_wire(f: OutputFormat) -> &'static str {
    match f {
        OutputFormat::Json => "json",
        OutputFormat::Robot => "robot",
        OutputFormat::Plain => "plain",
        OutputFormat::Csv => "csv",
        OutputFormat::Markdown => "markdown",
        #[cfg(feature = "toon")]
        OutputFormat::Toon => "toon",
    }
}

/// A bounded, valid actor string (ASCII, no control chars, <= 200 chars) — so actor validation
/// never rejects it; the property under test is precedence, not validation.
fn any_valid_actor() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_-]{1,40}".prop_map(|s| s)
}

proptest! {
    /// Precedence totality: for any combination of (cli, env, project) settings for `output_format`,
    /// the merged value equals the HIGHEST layer that set it (cli > env > project > default Json).
    #[test]
    fn output_format_precedence_is_total(
        cli_set in proptest::option::of(any_output_format()),
        env_set in proptest::option::of(any_output_format()),
        proj_set in proptest::option::of(any_output_format()),
    ) {
        let mut cli = CliOverrides::default();
        if let Some(f) = cli_set { cli = cli.with_output_format(f); }

        let mut env_map = HashMap::new();
        if let Some(f) = env_set {
            env_map.insert("UNBLOCK_OUTPUT_FORMAT".to_string(), output_format_wire(f).to_string());
        }
        let map = MapEnv(env_map);
        let env = EnvOverrides::from_source(&map).unwrap();

        let project = ProjectConfig { output_format: proj_set, ..ProjectConfig::default() };

        let wc = WorkspaceConfig::resolve(&cli, &env, &project, &map).unwrap();

        let expected = cli_set.or(env_set).or(proj_set).unwrap_or(OutputFormat::Json);
        prop_assert_eq!(wc.output_format(), expected);
    }

    /// Actor precedence totality (FORK-4): cli > UNBLOCK_ACTOR > config > $USER > "unblock".
    #[test]
    fn actor_precedence_is_total(
        cli_set in proptest::option::of(any_valid_actor()),
        env_set in proptest::option::of(any_valid_actor()),
        proj_set in proptest::option::of(any_valid_actor()),
        user_set in proptest::option::of(any_valid_actor()),
    ) {
        let mut cli = CliOverrides::default();
        if let Some(a) = &cli_set { cli = cli.with_actor(a.clone()); }

        let mut env_map = HashMap::new();
        if let Some(a) = &env_set { env_map.insert("UNBLOCK_ACTOR".to_string(), a.clone()); }
        if let Some(u) = &user_set { env_map.insert("USER".to_string(), u.clone()); }
        let map = MapEnv(env_map);
        let env = EnvOverrides::from_source(&map).unwrap();

        let project = ProjectConfig { actor: proj_set.clone(), ..ProjectConfig::default() };

        let wc = WorkspaceConfig::resolve(&cli, &env, &project, &map).unwrap();

        let expected = cli_set
            .or(env_set)
            .or(proj_set)
            .or(user_set)
            .unwrap_or_else(|| "unblock".to_string());
        prop_assert_eq!(wc.actor(), expected);
    }

    /// schema raw->merged never panics for any partial ProjectConfig (using arbitrary valid subsets).
    #[test]
    fn raw_to_merged_never_panics(
        actor in proptest::option::of(any_valid_actor()),
        search_cap in proptest::option::of(any::<usize>()),
        jsonl in proptest::option::of(any::<bool>()),
        retention in proptest::option::of(any::<u64>()),
    ) {
        let project = ProjectConfig {
            actor,
            search_cap,
            jsonl_export: jsonl,
            deletions_retention_days: retention,
            ..ProjectConfig::default()
        };
        let map = MapEnv(HashMap::new());
        let env = EnvOverrides::from_source(&map).unwrap();
        // Must never panic; may legitimately Err only on validation (none of these inputs trigger it).
        let _ = WorkspaceConfig::resolve(&CliOverrides::default(), &env, &project, &map);
    }

    /// Every known key classifies into exactly one of STARTUP_KEYS / RUNTIME_KEYS; unknown -> None.
    #[test]
    fn classification_is_consistent(key in "[a-z_]{1,20}") {
        let in_startup = STARTUP_KEYS.contains(&key.as_str());
        let in_runtime = RUNTIME_KEYS.contains(&key.as_str());
        if classify(&key).is_some() {
            // A classified key is in exactly one partition list.
            prop_assert!(in_startup ^ in_runtime);
        } else {
            // An unknown key is in neither partition list.
            prop_assert!(!in_startup && !in_runtime);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// **D57 criterion (a) — the six-layer fold is TOTAL**: over random stacks of CLI, env,
    /// project file, user file, DB row and `$USER` values for `output_format` and `actor`, the
    /// resolution never fails and returns the HIGHEST layer that set each key, with its `source`.
    #[test]
    fn six_layer_fold_is_total(
        fmt in proptest::array::uniform5(proptest::option::of(any_output_format())),
        actor in proptest::array::uniform6(proptest::option::of(any_valid_actor())),
    ) {
        use unblock_model::ConfigSource as S;
        let dir = tempfile::tempdir().unwrap();
        let toml = |f: Option<OutputFormat>, a: &Option<String>| {
            let format_line = f.map_or(String::new(), |f| {
                format!("output_format = \"{}\"\n", output_format_wire(f))
            });
            let actor_line = a.as_ref().map_or(String::new(), |a| format!("actor = \"{a}\"\n"));
            format_line + &actor_line
        };
        let project = dir.path().join("p.toml");
        std::fs::write(&project, toml(fmt[2], &actor[2])).unwrap();
        let user = dir.path().join("u.toml");
        std::fs::write(&user, toml(fmt[3], &actor[3])).unwrap();
        let cli = CliOverrides { output_format: fmt[0], actor: actor[0].clone(), ..CliOverrides::default() };
        let env = EnvOverrides { output_format: fmt[1], actor: actor[1].clone(), ..EnvOverrides::default() };
        let mut rows = Vec::new();
        if let Some(a) = &actor[4] { rows.push(("actor".to_string(), a.clone())); }
        if let Some(f) = fmt[4] { rows.push(("output_format".to_string(), output_format_wire(f).to_string())); }
        let runtime = unblock_config::ConfigReloader::new(cli, env, actor[5].clone(), Some(project), Some(user))
            .reload_runtime(&rows)
            .expect("the fold is total");

        let fmt_sources = [S::Cli, S::Env, S::Project, S::User, S::Db];
        let want_fmt = fmt.iter().zip(fmt_sources).find_map(|(f, s)| f.map(|f| (f, s)))
            .unwrap_or((OutputFormat::Json, S::Default));
        prop_assert_eq!((runtime.output_format, runtime.sources.output_format), want_fmt);

        let actor_sources = [S::Cli, S::Env, S::Project, S::User, S::Db, S::OsUser];
        let want_actor = actor.iter().zip(actor_sources).find_map(|(a, s)| a.clone().map(|a| (a, s)))
            .unwrap_or(("unblock".to_string(), S::Default));
        prop_assert_eq!((runtime.actor, runtime.sources.actor), want_actor);
        prop_assert!(runtime.db_faults.is_empty());
    }
}
