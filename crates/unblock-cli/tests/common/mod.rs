//! Shared harness for the `unblock-cli` integration suites (D27/T3.1).
//!
//! Every case runs against an **isolated** temp workspace (its own `.unblock/`) so the suites are
//! hermetic and parallel-safe: no case reads the repo's own `.unblock/` (walk-up discovery is pinned
//! by passing an explicit `--dir`), and no case mutates process-global env (`std::env::set_var` is
//! `unsafe` under edition 2024 and forbidden — per-child `Command::env` is used instead).
//!
//! The `unblock` binary is located via `assert_cmd`'s cargo integration (`cargo_bin`), so the suites
//! drive the SAME artifact the shipped build produces.

#![allow(dead_code)] // each test binary uses a subset of the harness.

use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::cargo::CommandCargoExt as _;
use tempfile::TempDir;

/// A freshly-scaffolded, isolated workspace: a tempdir whose `.unblock/` holds a migrated empty
/// `unblock.db` + a `config.toml`. The `TempDir` is retained so it outlives the case.
pub struct Workspace {
    /// The owning tempdir (the project root; `.unblock/` sits directly under it).
    pub root: TempDir,
}

impl Workspace {
    /// Scaffold a fresh workspace by running the real `unblock init` (FR-9 no-drift — the same code
    /// path `serve`/`migrate`/`doctor` open). Panics on failure (a harness precondition, not the SUT).
    #[must_use]
    pub fn init() -> Self {
        Self::init_with_prefix(None)
    }

    /// Like [`Workspace::init`] but seeds an explicit `--prefix`.
    #[must_use]
    pub fn init_with_prefix(prefix: Option<&str>) -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        let mut cmd = unblock();
        cmd.current_dir(root.path()).arg("init");
        if let Some(prefix) = prefix {
            cmd.args(["--prefix", prefix]);
        }
        let out = cmd.output().expect("run init");
        assert!(
            out.status.success(),
            "init must succeed; stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        Self { root }
    }

    /// The project root (contains `.unblock/`).
    #[must_use]
    pub fn root(&self) -> &Path {
        self.root.path()
    }

    /// The `.unblock/` directory.
    #[must_use]
    pub fn unblock_dir(&self) -> PathBuf {
        self.root.path().join(".unblock")
    }

    /// The workspace database path.
    #[must_use]
    pub fn db_path(&self) -> PathBuf {
        self.unblock_dir().join("unblock.db")
    }

    /// The scaffolded `config.toml` path.
    #[must_use]
    pub fn config_path(&self) -> PathBuf {
        self.unblock_dir().join("config.toml")
    }

    /// A `Command` for the `unblock` binary with `current_dir` set to this workspace root (so
    /// walk-up discovery finds this `.unblock/`, not the repo's).
    #[must_use]
    pub fn cmd(&self) -> Command {
        let mut cmd = unblock();
        cmd.current_dir(self.root.path());
        cmd
    }
}

/// A bare `Command` for the `unblock` binary (no cwd set). Callers set `current_dir`/`--dir`.
#[must_use]
pub fn unblock() -> Command {
    Command::cargo_bin("unblock").expect("locate the `unblock` binary")
}

/// A `Command` for the `unblock` binary anchored at `dir` (its cwd) — used when the case wants a
/// specific cwd but does NOT pre-scaffold a workspace (e.g. the no-workspace error paths).
#[must_use]
pub fn unblock_in(dir: &Path) -> Command {
    let mut cmd = unblock();
    cmd.current_dir(dir);
    cmd
}
