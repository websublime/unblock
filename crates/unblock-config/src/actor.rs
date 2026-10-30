//! Actor resolution — the global precedence chain, SEVEN tiers since D57 (D10 as amended, PRD §4
//! D57 clause (5)): `--actor` > `UNBLOCK_ACTOR` > project `[actor]` > user `actor` > DB `actor` >
//! `$USER` > `"unblock"`.
//!
//! The chain is folded by the ordinary highest-first merge (`merge.rs`): each layer contributes its
//! own TRIMMED actor (a blank value is unset), and the defaults layer carries the two-tier bottom
//! (`$USER` > `"unblock"`, built by `Defaults::with_os_user`), so the resolved actor's `source`
//! names the tier that supplied it (`os_user` for the `$USER` fallback). The literal default always
//! resolves, so [`crate::ConfigError::ActorUnresolved`] stays unreachable (reserved for a future
//! strict mode). The resolved actor is then bounded by [`unblock_model::validate_actor`] (Seam A).

/// The literal fallback actor when nothing else resolves (D10).
pub(crate) const DEFAULT_ACTOR: &str = "unblock";

/// Trim `value` and return it only if non-empty after trimming — the "blank is unset" rule every
/// actor tier shares (the original beads `filter(!is_empty)` behaviour).
pub(crate) fn trimmed_non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use unblock_model::ConfigSource;

    use super::DEFAULT_ACTOR;
    use crate::cli::CliOverrides;
    use crate::config::resolve_runtime;
    use crate::env::EnvOverrides;
    use crate::schema::ProjectConfig;

    /// One full stack: which tiers are set (each to its own name), resolved over the six layers.
    fn resolve(tiers: &[&str]) -> (String, ConfigSource) {
        let has = |t: &str| tiers.contains(&t);
        let cli = CliOverrides {
            actor: has("cli").then(|| "  cli  ".to_string()),
            ..CliOverrides::default()
        };
        let env = EnvOverrides {
            actor: has("env").then(|| "env".to_string()),
            ..EnvOverrides::default()
        };
        let project = ProjectConfig {
            actor: has("project").then(|| "project".to_string()),
            ..ProjectConfig::default()
        };
        let user = ProjectConfig {
            actor: has("user").then(|| "user".to_string()),
            ..ProjectConfig::default()
        };
        let rows: Vec<(String, String)> = if has("db") {
            vec![("actor".to_string(), "db".to_string())]
        } else {
            Vec::new()
        };
        let os_user = has("os_user").then_some("  os_user  ");
        let runtime =
            resolve_runtime(&cli, &env, &project, &user, &rows, os_user).expect("resolves");
        (runtime.actor, runtime.sources.actor)
    }

    /// **D57 criterion (g) — the seven-tier order holds tier by tier**, every tier trimmed, with
    /// `source` naming the tier (`os_user` for the `$USER` fallback, `default` for the literal).
    ///
    /// MUTANT KILLED: placing the DB layer above the user layer (the `user` step fails), dropping a
    /// tier, or tagging the `$USER` fallback `default`.
    #[test]
    fn seven_tier_actor_chain_holds_tier_by_tier() {
        let order = [
            ("cli", ConfigSource::Cli),
            ("env", ConfigSource::Env),
            ("project", ConfigSource::Project),
            ("user", ConfigSource::User),
            ("db", ConfigSource::Db),
            ("os_user", ConfigSource::OsUser),
        ];
        for (i, (winner, source)) in order.iter().enumerate() {
            let set: Vec<&str> = order[i..].iter().map(|(t, _)| *t).collect();
            assert_eq!(
                resolve(&set),
                ((*winner).to_string(), *source),
                "with tiers {set:?} set, `{winner}` wins"
            );
        }
        assert_eq!(
            resolve(&[]),
            (DEFAULT_ACTOR.to_string(), ConfigSource::Default),
            "nothing set → the literal `unblock`, source `default`"
        );
    }

    /// A blank value at any tier is UNSET (it falls through), and every tier is trimmed.
    #[test]
    fn a_blank_tier_falls_through_and_values_are_trimmed() {
        let cli = CliOverrides {
            actor: Some("   ".to_string()),
            ..CliOverrides::default()
        };
        let project = ProjectConfig {
            actor: Some("  carol  ".to_string()),
            ..ProjectConfig::default()
        };
        let runtime = resolve_runtime(
            &cli,
            &EnvOverrides::default(),
            &project,
            &ProjectConfig::default(),
            &[],
            Some("bob"),
        )
        .expect("resolves");
        assert_eq!(runtime.actor, "carol");
        assert_eq!(runtime.sources.actor, ConfigSource::Project);
    }
}
