//! Actor resolution — the precedence chain `UNBLOCK_ACTOR` env → `$USER` env → `"unblock"` literal
//! (spine §4 intro, D10).
//!
//! The resolution reads from an injectable [`EnvSource`] so tests do not touch the process
//! environment (parallel-safe). The literal default always resolves, so
//! [`ConfigError::ActorUnresolved`] is **unreachable** in T1.3a (reserved for a future strict mode).

use crate::error::ConfigError;

/// The literal fallback actor when neither env var resolves (D10).
pub(crate) const DEFAULT_ACTOR: &str = "unblock";

/// A read-only view over environment variables, injected so resolution is deterministic and tests
/// avoid the process-global env (no `std::env::set_var` races).
pub(crate) trait EnvSource {
    /// Fetch the value of `key`, or `None` when it is unset.
    fn get(&self, key: &str) -> Option<String>;
}

/// The production [`EnvSource`] backed by [`std::env::var`].
pub(crate) struct ProcessEnv;

impl EnvSource for ProcessEnv {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

/// Resolve the authoritative actor: `UNBLOCK_ACTOR` → `$USER` → `"unblock"` (D10).
///
/// An empty (or whitespace-only) value is treated as unset, matching the original beads
/// `filter(!is_empty)` behaviour, so a blank `UNBLOCK_ACTOR=` falls through to the next layer.
///
/// # Errors
///
/// Returns [`ConfigError::ActorUnresolved`] only if no layer yields a non-empty actor. With the
/// `"unblock"` literal default this is effectively unreachable in T1.3a — the variant is reserved
/// for a future strict-actor mode (T1.3).
pub(crate) fn resolve_actor(env: &dyn EnvSource) -> Result<String, ConfigError> {
    for key in ["UNBLOCK_ACTOR", "USER"] {
        if let Some(value) = env.get(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }
    }

    if DEFAULT_ACTOR.is_empty() {
        // Unreachable with the current constant; kept honest so a future empty default surfaces
        // the reserved error instead of an empty actor.
        return Err(ConfigError::ActorUnresolved);
    }
    Ok(DEFAULT_ACTOR.to_string())
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_ACTOR, EnvSource, resolve_actor};
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
    fn unblock_actor_wins_over_user_and_default() {
        let env = MapEnv::new(&[("UNBLOCK_ACTOR", "alice"), ("USER", "bob")]);
        assert_eq!(resolve_actor(&env).expect("actor"), "alice");
    }

    #[test]
    fn falls_back_to_user_when_unblock_actor_unset() {
        let env = MapEnv::new(&[("USER", "bob")]);
        assert_eq!(resolve_actor(&env).expect("actor"), "bob");
    }

    #[test]
    fn falls_back_to_literal_default_when_no_env() {
        let env = MapEnv::new(&[]);
        assert_eq!(resolve_actor(&env).expect("actor"), DEFAULT_ACTOR);
        assert_eq!(resolve_actor(&env).expect("actor"), "unblock");
    }

    #[test]
    fn empty_unblock_actor_is_treated_as_unset() {
        let env = MapEnv::new(&[("UNBLOCK_ACTOR", "   "), ("USER", "bob")]);
        assert_eq!(resolve_actor(&env).expect("actor"), "bob");
    }

    #[test]
    fn trims_surrounding_whitespace() {
        let env = MapEnv::new(&[("UNBLOCK_ACTOR", "  carol  ")]);
        assert_eq!(resolve_actor(&env).expect("actor"), "carol");
    }
}
