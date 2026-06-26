//! [`ConfigPaths`] — the resolved `.unblock/` + db/jsonl paths (config-owned, spine §4 CF-D).
//!
//! **Config owns path resolution from T1.3a** (single source of truth): the paths are derived from
//! the discovered workspace + the [`crate::ResolvedConfig`] filenames. Embedded **by value** in
//! both [`crate::ResolvedContext`] and [`crate::WorkspaceContext`].
//!
//! **Seam B (NFR-18 / FORK-3):** [`ConfigPaths::resolve`] guards the config-resolved
//! `db_filename`/`jsonl_filename` (and the `--db` path) against path-separator / `..` injection,
//! **rejects ABSOLUTE filenames** (an absolute arg to [`Path::join`] replaces the base, escaping
//! `unblock_dir`), and enforces a post-join `starts_with(canonical unblock_dir)` confinement check so
//! a resolved artifact cannot escape the workspace subtree.

use std::path::{Component, Path, PathBuf};

use crate::cli::CliOverrides;
use crate::config::WorkspaceConfig;
use crate::error::ConfigError;

/// The resolved `.unblock/` + db/jsonl paths (config-owned, spine §4 CF-D).
///
/// `unblock_dir` is the canonicalized discovered `.unblock`/`_unblock` directory; `db_path` and
/// `jsonl_path` are derived from `unblock_dir` joined with the merged [`WorkspaceConfig`] filenames,
/// then validated and confined (Seam B). The TYPE shape is frozen from T1.3a; T1.3 only enriches
/// *how* it is filled (custom filenames, `--db` override, the FORK-3 canonicalize + confinement).
#[derive(Debug, Clone)]
pub struct ConfigPaths {
    /// The discovered `.unblock`/`_unblock` directory (canonicalized; Seam C).
    pub unblock_dir: PathBuf,
    /// `unblock_dir.join(db_filename)` (default `"unblock.db"`) or the confined `--db` override.
    pub db_path: PathBuf,
    /// `unblock_dir.join(jsonl_filename)` (default `"issues.jsonl"`).
    pub jsonl_path: PathBuf,
}

impl ConfigPaths {
    /// Resolve the concrete artifact paths from the **canonicalized** discovered `unblock_dir`, the
    /// merged config filenames, and the CLI `--db` override (FORK-3 / Seam B).
    ///
    /// `unblock_dir` must already be the **canonicalized** workspace metadata directory (the actual
    /// `.unblock` OR `_unblock` directory — discovery canonicalizes it, Seam C, and supports the
    /// monorepo alias FORK-2). The db/jsonl filenames are validated against path injection (no
    /// separators, no `..`, not absolute) and the resolved paths are confined within `unblock_dir`
    /// (`starts_with`). The `--db` override, when present, is itself confined the same way.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::InvalidValue`] if `db_filename` / `jsonl_filename` / the `--db` path is
    /// absolute, contains a path separator or a `..` component, or the post-join path escapes
    /// `unblock_dir`.
    pub(crate) fn resolve(
        unblock_dir: &Path,
        cfg: &WorkspaceConfig,
        cli: &CliOverrides,
    ) -> Result<Self, ConfigError> {
        let unblock_dir = unblock_dir.to_path_buf();

        // db_path: an explicit --db wins (confined); otherwise join the validated db_filename.
        let db_path = match &cli.db {
            Some(db) => confine_path("--db", db, &unblock_dir)?,
            None => safe_join("db_filename", cfg.db_filename(), &unblock_dir)?,
        };

        let jsonl_path = safe_join("jsonl_filename", cfg.jsonl_filename(), &unblock_dir)?;

        Ok(Self {
            unblock_dir,
            db_path,
            jsonl_path,
        })
    }
}

/// Reject a filename that is absolute or carries a path separator / `..` component, then join it onto
/// `unblock_dir` and confirm the result stays within `unblock_dir`.
fn safe_join(key: &str, filename: &str, unblock_dir: &Path) -> Result<PathBuf, ConfigError> {
    let candidate = Path::new(filename);

    // Reject absolute filenames (an absolute arg to `join` REPLACES the base, escaping unblock_dir).
    if candidate.is_absolute() {
        return Err(ConfigError::InvalidValue {
            key: key.to_string(),
            value: filename.to_string(),
            reason: "must be a bare filename, not an absolute path".to_string(),
        });
    }

    // Reject anything that is not a single, plain filename component (no `/`, `\`, `..`, `.`).
    let mut components = candidate.components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) => {}
        _ => {
            return Err(ConfigError::InvalidValue {
                key: key.to_string(),
                value: filename.to_string(),
                reason: "must be a bare filename (no path separators or `..`)".to_string(),
            });
        }
    }

    let joined = unblock_dir.join(candidate);
    confirm_within(key, &joined, unblock_dir, filename)?;
    Ok(joined)
}

/// Confine an explicit `--db` path: reject relative escapes / `..`, then ensure it is within
/// `unblock_dir` (a `--db` path must live under the workspace's `.unblock/`, NFR-18).
fn confine_path(key: &str, path: &Path, unblock_dir: &Path) -> Result<PathBuf, ConfigError> {
    // A `..` component is a clear escape attempt regardless of absoluteness.
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(ConfigError::InvalidValue {
            key: key.to_string(),
            value: path.display().to_string(),
            reason: "must not contain `..` components".to_string(),
        });
    }
    // Normalize relative --db against unblock_dir; an absolute --db is taken as-is then confined.
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        unblock_dir.join(path)
    };
    confirm_within(key, &candidate, unblock_dir, &path.display().to_string())?;
    Ok(candidate)
}

/// Confirm `candidate` is lexically within `unblock_dir` (post-join `starts_with` confinement).
fn confirm_within(
    key: &str,
    candidate: &Path,
    unblock_dir: &Path,
    value: &str,
) -> Result<(), ConfigError> {
    if candidate.starts_with(unblock_dir) {
        Ok(())
    } else {
        Err(ConfigError::InvalidValue {
            key: key.to_string(),
            value: value.to_string(),
            reason: format!(
                "resolved path escapes the workspace directory {}",
                unblock_dir.display()
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ConfigPaths;
    use crate::cli::CliOverrides;
    use crate::config::WorkspaceConfig;
    use crate::env::{EnvOverrides, EnvSource};
    use crate::schema::ProjectConfig;
    use std::path::{Path, PathBuf};

    struct NoEnv;
    impl EnvSource for NoEnv {
        fn get(&self, _key: &str) -> Option<String> {
            None
        }
    }

    fn wc(project: &ProjectConfig) -> WorkspaceConfig {
        let env = EnvOverrides::from_source(&NoEnv).expect("env");
        WorkspaceConfig::resolve(&CliOverrides::default(), &env, project, &NoEnv).expect("resolve")
    }

    #[test]
    fn resolve_default_filenames() {
        let unblock_dir = Path::new("/projects/demo/.unblock");
        let cfg = wc(&ProjectConfig::default());
        let paths =
            ConfigPaths::resolve(unblock_dir, &cfg, &CliOverrides::default()).expect("resolve");
        assert_eq!(
            paths.db_path,
            Path::new("/projects/demo/.unblock/unblock.db")
        );
        assert_eq!(
            paths.jsonl_path,
            Path::new("/projects/demo/.unblock/issues.jsonl")
        );
    }

    #[test]
    fn resolve_custom_filenames() {
        let unblock_dir = Path::new("/projects/demo/.unblock");
        let cfg = wc(&ProjectConfig {
            db_filename: Some("alt.db".to_string()),
            jsonl_export_filename: Some("alt.jsonl".to_string()),
            ..ProjectConfig::default()
        });
        let paths =
            ConfigPaths::resolve(unblock_dir, &cfg, &CliOverrides::default()).expect("resolve");
        assert_eq!(paths.db_path, Path::new("/projects/demo/.unblock/alt.db"));
        assert_eq!(
            paths.jsonl_path,
            Path::new("/projects/demo/.unblock/alt.jsonl")
        );
    }

    #[test]
    fn underscore_alias_dir_is_honored() {
        // FORK-2: a `_unblock` monorepo-alias dir is resolved as-is (discovery returns it directly).
        let unblock_dir = Path::new("/projects/demo/_unblock");
        let cfg = wc(&ProjectConfig::default());
        let paths =
            ConfigPaths::resolve(unblock_dir, &cfg, &CliOverrides::default()).expect("resolve");
        assert_eq!(paths.unblock_dir, unblock_dir);
        assert_eq!(
            paths.db_path,
            Path::new("/projects/demo/_unblock/unblock.db")
        );
    }

    #[test]
    fn db_override_under_unblock_dir_wins() {
        let unblock_dir = Path::new("/projects/demo/.unblock");
        let cfg = wc(&ProjectConfig::default());
        let cli = CliOverrides::new().with_db("/projects/demo/.unblock/override.db");
        let paths = ConfigPaths::resolve(unblock_dir, &cfg, &cli).expect("resolve");
        assert_eq!(
            paths.db_path,
            Path::new("/projects/demo/.unblock/override.db")
        );
    }

    #[test]
    fn separator_or_parent_filename_rejected() {
        let unblock_dir = Path::new("/projects/demo/.unblock");
        for bad in ["../escape.db", "sub/dir.db", ".."] {
            let cfg = wc(&ProjectConfig {
                db_filename: Some(bad.to_string()),
                ..ProjectConfig::default()
            });
            let err = ConfigPaths::resolve(unblock_dir, &cfg, &CliOverrides::default())
                .expect_err("must reject injection");
            match err {
                crate::error::ConfigError::InvalidValue { key, .. } => {
                    assert_eq!(key, "db_filename");
                }
                other => panic!("expected InvalidValue, got {other:?}"),
            }
        }
    }

    #[test]
    fn absolute_filename_rejected() {
        let unblock_dir = Path::new("/projects/demo/.unblock");
        let cfg = wc(&ProjectConfig {
            db_filename: Some("/etc/passwd".to_string()),
            ..ProjectConfig::default()
        });
        let err = ConfigPaths::resolve(unblock_dir, &cfg, &CliOverrides::default())
            .expect_err("absolute rejected");
        assert!(matches!(
            err,
            crate::error::ConfigError::InvalidValue { .. }
        ));
    }

    #[test]
    fn db_override_escaping_unblock_dir_rejected() {
        let unblock_dir = Path::new("/projects/demo/.unblock");
        let cfg = wc(&ProjectConfig::default());
        // Absolute --db outside .unblock/.
        let cli = CliOverrides::new().with_db("/tmp/elsewhere.db");
        let err =
            ConfigPaths::resolve(unblock_dir, &cfg, &cli).expect_err("escaping --db rejected");
        match err {
            crate::error::ConfigError::InvalidValue { key, .. } => assert_eq!(key, "--db"),
            other => panic!("expected InvalidValue, got {other:?}"),
        }
        // A `..`-bearing --db is rejected too.
        let cli = CliOverrides::new().with_db(PathBuf::from("../escape.db"));
        let err = ConfigPaths::resolve(unblock_dir, &cfg, &cli).expect_err("parent --db rejected");
        assert!(matches!(
            err,
            crate::error::ConfigError::InvalidValue { .. }
        ));
    }

    /// Golden snapshot of a resolved `ConfigPaths` for a fixed fixture dir (deterministic; pins the
    /// resolved-path shape). Uses a fixed absolute `unblock_dir` so the snapshot is platform-stable.
    #[test]
    fn resolved_config_paths_golden() {
        let unblock_dir = Path::new("/fixture/workspace/.unblock");
        let cfg = wc(&ProjectConfig::default());
        let paths =
            ConfigPaths::resolve(unblock_dir, &cfg, &CliOverrides::default()).expect("resolve");
        insta::assert_debug_snapshot!("resolved_config_paths_fixture", paths);
    }
}
