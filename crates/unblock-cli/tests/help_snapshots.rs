//! `--help` snapshots for the top-level command + every subcommand (D9 stable-clap surface, D3).
//!
//! These pin the CLI's public usage surface so a drift (a renamed flag, a new/removed subcommand, a
//! changed about-line) is a deliberate re-bless, not a silent change. Help output is byte-stable (no
//! paths/timestamps), so no insta filters are needed. Each subcommand's help is snapshotted via
//! `<sub> --help` (clap prints to stdout with exit 0).

mod common;

use common::unblock;

/// Capture a command's `--help` stdout (asserting the help path exits 0 via clap).
fn help(args: &[&str]) -> String {
    let out = unblock().args(args).output().expect("run --help");
    assert_eq!(out.status.code(), Some(0), "help exits 0");
    assert!(!out.stdout.is_empty(), "help writes to stdout");
    String::from_utf8(out.stdout).expect("utf8 help")
}

#[test]
fn top_level_help() {
    insta::assert_snapshot!("help_top_level", help(&["--help"]));
}

#[test]
fn serve_help() {
    insta::assert_snapshot!("help_serve", help(&["serve", "--help"]));
}

#[test]
fn migrate_help() {
    insta::assert_snapshot!("help_migrate", help(&["migrate", "--help"]));
}

#[test]
fn doctor_help() {
    insta::assert_snapshot!("help_doctor", help(&["doctor", "--help"]));
}

#[test]
fn version_help() {
    insta::assert_snapshot!("help_version", help(&["version", "--help"]));
}

#[test]
fn init_help() {
    insta::assert_snapshot!("help_init", help(&["init", "--help"]));
}

#[test]
fn agents_help() {
    insta::assert_snapshot!("help_agents", help(&["agents", "--help"]));
}

#[cfg(feature = "self-update")]
#[test]
fn update_help() {
    insta::assert_snapshot!("help_update", help(&["update", "--help"]));
}
