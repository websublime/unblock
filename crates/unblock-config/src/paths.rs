//! [`ConfigPaths`] — the resolved `.unblock/` + db/jsonl paths (config-owned, spine §4 CF-D).
//!
//! **Config owns path resolution from T1.3a** (single source of truth): the paths are derived from
//! the discovered workspace + the [`crate::ResolvedConfig`] filenames. Embedded **by value** in
//! both [`crate::ResolvedContext`] and [`crate::WorkspaceContext`].

use std::path::{Path, PathBuf};

use crate::config::ResolvedConfig;

/// The name of the workspace metadata directory (locked, PRD §12.5 / D8).
pub(crate) const UNBLOCK_DIR_NAME: &str = ".unblock";

/// The resolved `.unblock/` + db/jsonl paths (config-owned, spine §4 CF-D).
///
/// `unblock_dir` is `workspace_dir.join(".unblock")`; `db_path` / `jsonl_path` are derived from
/// `unblock_dir` + the [`ResolvedConfig`] filenames. At T1.3a these are derived from the defaulted
/// config; the T1.3 layered resolver enriches *how* they are filled (custom filenames, `--db`
/// override) without changing the shape.
#[derive(Debug, Clone)]
pub struct ConfigPaths {
    /// The discovered `.unblock/` directory (`= workspace_dir.join(".unblock")`).
    pub unblock_dir: PathBuf,
    /// `unblock_dir.join(ResolvedConfig.db_filename)` (T1.3a default `"unblock.db"`).
    pub db_path: PathBuf,
    /// `unblock_dir.join(ResolvedConfig.jsonl_filename)` (T1.3a default `"issues.jsonl"`).
    pub jsonl_path: PathBuf,
}

impl ConfigPaths {
    /// Derive the paths from the discovered `workspace_dir` (the project root containing
    /// `.unblock/`) and the resolved [`ResolvedConfig`] filenames.
    ///
    /// `unblock_dir = workspace_dir/.unblock`; `db_path` / `jsonl_path` are `unblock_dir` joined
    /// with the config filenames. This is config's single source of truth for path resolution
    /// (spine §4 CF-D), from T1.3a.
    pub(crate) fn derive(workspace_dir: &Path, config: &ResolvedConfig) -> Self {
        let unblock_dir = workspace_dir.join(UNBLOCK_DIR_NAME);
        let db_path = unblock_dir.join(&config.db_filename);
        let jsonl_path = unblock_dir.join(&config.jsonl_filename);
        Self {
            unblock_dir,
            db_path,
            jsonl_path,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ConfigPaths, UNBLOCK_DIR_NAME};
    use crate::config::ResolvedConfig;
    use std::path::Path;

    #[test]
    fn derive_joins_unblock_dir_and_filenames() {
        let workspace = Path::new("/projects/demo");
        let paths = ConfigPaths::derive(workspace, &ResolvedConfig::default());

        assert_eq!(paths.unblock_dir, workspace.join(UNBLOCK_DIR_NAME));
        assert_eq!(paths.unblock_dir, Path::new("/projects/demo/.unblock"));
        assert_eq!(
            paths.db_path,
            Path::new("/projects/demo/.unblock/unblock.db")
        );
        assert_eq!(
            paths.jsonl_path,
            Path::new("/projects/demo/.unblock/issues.jsonl")
        );
    }
}
