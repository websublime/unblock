//! Integration discovery suite on real `tempfile` directory trees (FR-13 / FORK-2 / FORK-3).
//!
//! Covers: nearest `.unblock`/`_unblock` walk-up; the explicit `--dir`/`--db` overrides (no walk-up,
//! MF-2); `--db` derivation; symlink canonicalization + confinement (FORK-3); not-found. The env
//! `UNBLOCK_DIR` override path is exercised through `CliOverrides::dir` (the env is parsed into the
//! same explicit-dir slot by `EnvOverrides`; tests stay parallel-safe by injecting via `cli.dir`
//! rather than mutating the process env).

use std::fs;

use unblock_config::{
    CliOverrides, ConfigError, discover_optional_unblock_dir, discover_unblock_dir,
};

#[test]
fn walks_up_to_dot_unblock() {
    let root = tempfile::tempdir().expect("tempdir");
    let ws = root.path().join("proj");
    fs::create_dir_all(ws.join(".unblock")).expect("mkdir");
    let nested = ws.join("a").join("b").join("c");
    fs::create_dir_all(&nested).expect("mkdir nested");

    let found = discover_unblock_dir(Some(&nested), &CliOverrides::default()).expect("discover");
    assert_eq!(found, ws.join(".unblock").canonicalize().expect("canon"));
}

#[test]
fn walks_up_to_underscore_unblock_alias() {
    // FORK-2: `_unblock` is a valid workspace dir name (dot-dir-hostile monorepos).
    let root = tempfile::tempdir().expect("tempdir");
    let ws = root.path().join("proj");
    fs::create_dir_all(ws.join("_unblock")).expect("mkdir");
    let nested = ws.join("nested");
    fs::create_dir_all(&nested).expect("mkdir nested");

    let found = discover_unblock_dir(Some(&nested), &CliOverrides::default()).expect("discover");
    assert_eq!(found, ws.join("_unblock").canonicalize().expect("canon"));
}

#[test]
fn explicit_dir_override_is_used_directly_no_walk_up() {
    // A `.unblock` exists at root, but the explicit --dir points elsewhere; walk-up must NOT fire.
    let root = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(root.path().join(".unblock")).expect("root ws");
    let other = root.path().join("other");
    fs::create_dir_all(other.join(".unblock")).expect("other ws");

    let cli = CliOverrides::new().with_dir(&other);
    let found = discover_unblock_dir(None, &cli).expect("explicit dir");
    assert_eq!(found, other.join(".unblock").canonicalize().expect("canon"));
}

#[test]
fn db_under_unblock_derives_the_dir() {
    let root = tempfile::tempdir().expect("tempdir");
    let unblock = root.path().join(".unblock");
    fs::create_dir_all(&unblock).expect("mkdir");
    let cli = CliOverrides::new().with_db(unblock.join("unblock.db"));
    let found = discover_unblock_dir(None, &cli).expect("derive from db");
    assert_eq!(found, unblock.canonicalize().expect("canon"));
}

#[test]
fn not_found_yields_workspace_not_found() {
    let root = tempfile::tempdir().expect("tempdir");
    let nested = root.path().join("x").join("y");
    fs::create_dir_all(&nested).expect("mkdir");

    let err = discover_unblock_dir(Some(&nested), &CliOverrides::default()).expect_err("not found");
    match err {
        ConfigError::WorkspaceNotFound { .. } => {}
        other => panic!("expected WorkspaceNotFound, got {other:?}"),
    }
}

#[test]
fn optional_discovery_returns_none_without_db() {
    let root = tempfile::tempdir().expect("tempdir");
    let nested = root.path().join("z");
    fs::create_dir_all(&nested).expect("mkdir");
    let result =
        discover_optional_unblock_dir(Some(&nested), &CliOverrides::default()).expect("optional");
    assert!(result.is_none());
}

#[cfg(unix)]
#[test]
fn symlinked_workspace_dir_is_canonicalized_and_confined() {
    // FORK-3: a symlinked workspace dir is ALLOWED but resolved to its canonical path so the db/jsonl
    // paths are confined within the canonical subtree (NFR-18) — not rejected.
    let root = tempfile::tempdir().expect("tempdir");
    let real = root.path().join("real");
    fs::create_dir_all(real.join(".unblock")).expect("real ws");
    let link = root.path().join("link");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");

    let cli = CliOverrides::new().with_dir(&link);
    let found = discover_unblock_dir(None, &cli).expect("discover via symlink");
    // The discovered dir resolves through `real`, never `link`.
    let canon_real = real.join(".unblock").canonicalize().expect("canon real");
    assert_eq!(found, canon_real);
    assert!(found.starts_with(real.canonicalize().expect("canon real root")));
}
