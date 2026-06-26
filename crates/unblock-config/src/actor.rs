//! Actor resolution — the global precedence chain
//! `--actor` > `UNBLOCK_ACTOR` > `config.toml [actor]` > `$USER` > `"unblock"` literal
//! (FORK-4, D10, spine §4).
//!
//! The resolution reads from an injectable [`EnvSource`] (defined in [`crate::env`]) so tests do not
//! touch the process environment (parallel-safe). The literal default always resolves, so
//! [`ConfigError::ActorUnresolved`] is **unreachable** (reserved for a future strict mode). The
//! resolved actor is then bounded by [`unblock_model::validate_actor`] (Seam A) — that check lives
//! in [`crate::config`] where the full layer set is available.

use crate::env::EnvSource;
use crate::error::ConfigError;

/// The literal fallback actor when nothing else resolves (D10).
pub(crate) const DEFAULT_ACTOR: &str = "unblock";

/// Resolve the authoritative actor across the global precedence chain (FORK-4):
/// `cli_actor` > `UNBLOCK_ACTOR` > `config_actor` (`config.toml [actor]`) > `$USER` > `"unblock"`.
///
/// `cli_actor` is the `--actor` override, `config_actor` is the project-TOML `[actor]` key. An empty
/// (or whitespace-only) value at any layer is treated as **unset** (trimmed before use), matching the
/// original beads `filter(!is_empty)` behaviour, so a blank `--actor`/`UNBLOCK_ACTOR=` falls through.
/// The returned actor is **not** yet bounded — the caller routes it through
/// [`unblock_model::validate_actor`] (Seam A).
///
/// # Errors
///
/// Returns [`ConfigError::ActorUnresolved`] only if no layer yields a non-empty actor. With the
/// `"unblock"` literal default this is effectively unreachable — the variant is reserved for a
/// future strict-actor mode.
pub(crate) fn resolve_actor_layered(
    cli_actor: Option<&str>,
    config_actor: Option<&str>,
    env: &dyn EnvSource,
) -> Result<String, ConfigError> {
    // CLI override first.
    if let Some(actor) = cli_actor.and_then(trimmed_non_empty) {
        return Ok(actor);
    }
    // Then UNBLOCK_ACTOR env.
    if let Some(actor) = env
        .get("UNBLOCK_ACTOR")
        .as_deref()
        .and_then(trimmed_non_empty)
    {
        return Ok(actor);
    }
    // Then config.toml [actor].
    if let Some(actor) = config_actor.and_then(trimmed_non_empty) {
        return Ok(actor);
    }
    // Then $USER env.
    if let Some(actor) = env.get("USER").as_deref().and_then(trimmed_non_empty) {
        return Ok(actor);
    }
    // Finally the literal default.
    if DEFAULT_ACTOR.is_empty() {
        // Unreachable with the current constant; kept honest so a future empty default surfaces
        // the reserved error instead of an empty actor.
        return Err(ConfigError::ActorUnresolved);
    }
    Ok(DEFAULT_ACTOR.to_string())
}

/// Trim `value` and return it only if non-empty after trimming.
fn trimmed_non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_ACTOR, resolve_actor_layered};
    use crate::env::EnvSource;
    use std::collections::HashMap;

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
    fn cli_actor_wins_over_everything() {
        let env = MapEnv::new(&[("UNBLOCK_ACTOR", "envactor"), ("USER", "bob")]);
        assert_eq!(
            resolve_actor_layered(Some("cliactor"), Some("cfgactor"), &env).expect("actor"),
            "cliactor"
        );
    }

    #[test]
    fn unblock_actor_wins_over_config_user_and_default() {
        let env = MapEnv::new(&[("UNBLOCK_ACTOR", "alice"), ("USER", "bob")]);
        assert_eq!(
            resolve_actor_layered(None, Some("cfgactor"), &env).expect("actor"),
            "alice"
        );
    }

    #[test]
    fn config_actor_wins_over_user_and_default() {
        let env = MapEnv::new(&[("USER", "bob")]);
        assert_eq!(
            resolve_actor_layered(None, Some("cfgactor"), &env).expect("actor"),
            "cfgactor"
        );
    }

    #[test]
    fn falls_back_to_user_when_higher_layers_unset() {
        let env = MapEnv::new(&[("USER", "bob")]);
        assert_eq!(
            resolve_actor_layered(None, None, &env).expect("actor"),
            "bob"
        );
    }

    #[test]
    fn falls_back_to_literal_default_when_nothing_set() {
        let env = MapEnv::new(&[]);
        assert_eq!(
            resolve_actor_layered(None, None, &env).expect("actor"),
            DEFAULT_ACTOR
        );
        assert_eq!(
            resolve_actor_layered(None, None, &env).expect("actor"),
            "unblock"
        );
    }

    #[test]
    fn empty_at_any_layer_is_treated_as_unset() {
        let env = MapEnv::new(&[("UNBLOCK_ACTOR", "   "), ("USER", "bob")]);
        // Empty cli, empty env -> config wins.
        assert_eq!(
            resolve_actor_layered(Some("  "), Some("cfgactor"), &env).expect("actor"),
            "cfgactor"
        );
    }

    #[test]
    fn trims_surrounding_whitespace() {
        let env = MapEnv::new(&[]);
        assert_eq!(
            resolve_actor_layered(Some("  carol  "), None, &env).expect("actor"),
            "carol"
        );
    }
}
