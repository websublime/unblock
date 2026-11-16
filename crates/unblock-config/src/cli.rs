//! [`CliOverrides`] — the typed top precedence layer the CLI binary fills and passes down.
//!
//! Highest precedence in the FR-13 chain (`CLI > env UNBLOCK_* > project config.toml > defaults`).
//! Decoupled from `clap` (clap lives in `unblock-cli`; this is a plain struct so the engine and the
//! config tests do not pull clap). `dir` is the EXPLICIT `--dir`/`UNBLOCK_DIR` override (NO walk-up,
//! MF-2) — distinct from the `&Path` facade's walk-up `start`. There is **no** `--jsonl` CLI flag
//! (SF-6): `jsonl_export` is here only for programmatic callers / env-and-TOML resolution.

use std::path::PathBuf;

use unblock_model::OutputFormat;

/// The typed top precedence layer (highest in the FR-13 chain).
///
/// Every override is `Option` (set-or-unset) except the open-mode knobs: `no_db` and `create_db`
/// (plain flags, default `false`) and `storage_open` (default [`StorageOpenMode::Migrate`]).
/// `Default` is all-unset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CliOverrides {
    /// `--dir` / `UNBLOCK_DIR` — the EXPLICIT workspace dir (used directly, NO walk-up; MF-2).
    pub dir: Option<PathBuf>,
    /// `--db` — an explicit database path (confined within the resolved `unblock_dir`; Seam B).
    pub db: Option<PathBuf>,
    /// `--actor` — the highest-precedence actor override (FORK-4).
    pub actor: Option<String>,
    /// `--output-format` — the output format override.
    pub output_format: Option<OutputFormat>,
    /// The JSONL-export toggle (programmatic only; no clap flag — SF-6).
    pub jsonl_export: Option<bool>,
    /// Whether to skip opening the DB: `open_with_cli` then returns the resolve-only context (the
    /// `doctor` pre-open phase, PRD §4 D58 clause (8)).
    pub no_db: bool,
    /// How `open_with_cli` opens the database (PRD §4 D58 clause (8)).
    pub storage_open: StorageOpenMode,
    /// Whether a storage-bearing open may create a LOST database (absent or 0 bytes beside the
    /// `.write.lock` marker) instead of refusing it. Only `unblock init` sets it (PRD §4 D58 clause
    /// (7)).
    pub create_db: bool,
}

/// How a storage-bearing open treats the database (PRD §4 D58 clause (8)).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StorageOpenMode {
    /// Open read-write and migrate — every command except `doctor`.
    #[default]
    Migrate,
    /// Open read-only: no migrate, no `.write.lock`, no write (plain `doctor`, the final `--repair`
    /// pass).
    ReadOnly,
    /// Open read-write without migrating, so `Session::recover()` snapshots before it migrates (the
    /// `--repair` online phase).
    NoMigrate,
}

impl CliOverrides {
    /// Construct an all-unset [`CliOverrides`] (same as [`CliOverrides::default`]).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the explicit workspace dir (`--dir`/`UNBLOCK_DIR`; no walk-up).
    #[must_use]
    pub fn with_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.dir = Some(dir.into());
        self
    }

    /// Set the explicit database path (`--db`).
    #[must_use]
    pub fn with_db(mut self, db: impl Into<PathBuf>) -> Self {
        self.db = Some(db.into());
        self
    }

    /// Set the actor override (`--actor`).
    #[must_use]
    pub fn with_actor(mut self, actor: impl Into<String>) -> Self {
        self.actor = Some(actor.into());
        self
    }

    /// Set the output-format override (`--output-format`).
    #[must_use]
    pub fn with_output_format(mut self, format: OutputFormat) -> Self {
        self.output_format = Some(format);
        self
    }

    /// Set the JSONL-export toggle (programmatic only).
    #[must_use]
    pub fn with_jsonl_export(mut self, enabled: bool) -> Self {
        self.jsonl_export = Some(enabled);
        self
    }

    /// Set the `no_db` flag (skip opening the DB).
    #[must_use]
    pub fn with_no_db(mut self, no_db: bool) -> Self {
        self.no_db = no_db;
        self
    }

    /// Set how a storage-bearing open treats the database.
    #[must_use]
    pub fn with_storage_open(mut self, mode: StorageOpenMode) -> Self {
        self.storage_open = mode;
        self
    }

    /// Set the `create_db` flag (create a lost database instead of refusing it; `unblock init`
    /// only).
    #[must_use]
    pub fn with_create_db(mut self, create_db: bool) -> Self {
        self.create_db = create_db;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{CliOverrides, StorageOpenMode};
    use std::path::PathBuf;
    use unblock_model::OutputFormat;

    #[test]
    fn default_is_all_unset() {
        let cli = CliOverrides::default();
        assert!(cli.dir.is_none());
        assert!(cli.db.is_none());
        assert!(cli.actor.is_none());
        assert!(cli.output_format.is_none());
        assert!(cli.jsonl_export.is_none());
        assert!(!cli.no_db);
        assert_eq!(cli.storage_open, StorageOpenMode::Migrate);
        assert!(!cli.create_db);
        assert_eq!(cli, CliOverrides::new());
    }

    #[test]
    fn setters_compose() {
        let cli = CliOverrides::new()
            .with_dir("/ws/.unblock")
            .with_db("/ws/.unblock/unblock.db")
            .with_actor("alice")
            .with_output_format(OutputFormat::Robot)
            .with_jsonl_export(true)
            .with_no_db(true)
            .with_storage_open(StorageOpenMode::ReadOnly)
            .with_create_db(true);
        assert_eq!(cli.dir, Some(PathBuf::from("/ws/.unblock")));
        assert_eq!(cli.db, Some(PathBuf::from("/ws/.unblock/unblock.db")));
        assert_eq!(cli.actor.as_deref(), Some("alice"));
        assert_eq!(cli.output_format, Some(OutputFormat::Robot));
        assert_eq!(cli.jsonl_export, Some(true));
        assert!(cli.no_db);
        assert_eq!(cli.storage_open, StorageOpenMode::ReadOnly);
        assert!(cli.create_db);
        assert_eq!(
            CliOverrides::new()
                .with_storage_open(StorageOpenMode::NoMigrate)
                .storage_open,
            StorageOpenMode::NoMigrate
        );
    }
}
