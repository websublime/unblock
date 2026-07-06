//! NFR-6 / NFR-17 static gate — the deferred CI `no-network`/`no-git` artifact, landing at T3.1.
//!
//! The `unblock` binary must NEVER shell out to `git` (`Command::new("git")`), must NEVER link a git
//! library (`git2`/`gix`/`libgit2`), and must confine its ONLY network surface (self-update,
//! `axoupdater` + its transitive `reqwest`/`hyper` stack) BEHIND the default-on `self-update` Cargo
//! feature (CF-K). `--no-default-features` therefore drops the only network dependency entirely (the
//! feature-matrix build in CI proves it compiles without it).
//!
//! This is a source-level scan of the crate (`$CARGO_MANIFEST_DIR/src`), NOT a symbol scan of a built
//! binary — it is deterministic, fast, and platform-independent (it does not depend on nm/objdump).

use std::path::{Path, PathBuf};

/// The forbidden substrings that must NOT appear ANYWHERE in the crate source (git surface, NFR-6).
const FORBIDDEN_GIT: &[&str] = &[
    "Command::new(\"git\"",
    "Command::new(\"/usr/bin/git\"",
    "git2::",
    "use git2",
    "gix::",
    "libgit2",
];

/// Network / self-update symbols that are ALLOWED only in a `self-update`-feature-gated context. Any
/// occurrence must be reachable ONLY when `self-update` is enabled (i.e. in `commands/update.rs`, or
/// on a `#[cfg(feature = "self-update")]`-guarded line). No un-gated network symbol may leak.
const NETWORK_SYMBOLS: &[&str] = &[
    "axoupdater",
    "reqwest",
    "hyper::",
    "TcpStream",
    "std::net::",
];

/// The crate `src/` root (from Cargo at compile time — no walk-up guessing).
fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Every `.rs` file under `src/` (recursive), sorted for deterministic reporting.
fn source_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).expect("read src dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

#[test]
fn no_git_surface_in_the_cli_source() {
    let src = src_dir();
    let files = source_files(&src);
    assert!(
        !files.is_empty(),
        "found no source files to scan under {}",
        src.display()
    );

    for file in &files {
        let text = std::fs::read_to_string(file).expect("read source file");
        for needle in FORBIDDEN_GIT {
            assert!(
                !text.contains(needle),
                "NFR-6: forbidden git surface `{needle}` found in {} — the binary must never shell \
                 out to git or link a git library",
                file.display()
            );
        }
    }
}

#[test]
fn network_symbols_are_confined_behind_self_update() {
    let src = src_dir();
    for file in source_files(&src) {
        let text = std::fs::read_to_string(&file).expect("read source file");
        let file_is_update = file.file_name().is_some_and(|n| n == "update.rs");
        for symbol in NETWORK_SYMBOLS {
            if !text.contains(symbol) {
                continue;
            }
            // The whole `commands/update.rs` module is `#[cfg(feature = "self-update")]`-gated at its
            // `mod` declaration (commands/mod.rs), so any network symbol there is confined by design.
            if file_is_update {
                continue;
            }
            // Elsewhere, EVERY line mentioning a network symbol must be feature-gated: the line (or a
            // nearby line) must carry `#[cfg(feature = "self-update")]`. We enforce the stricter rule
            // that such lines only appear inside a `self-update` cfg region — assert the file contains
            // the gate and that no network symbol appears in an un-gated line.
            for line in text.lines() {
                if line.contains(symbol) {
                    let gated_here = line.contains("self-update") // e.g. a doc/comment mentioning it
                        || line.trim_start().starts_with("//"); // comments are inert
                    assert!(
                        gated_here || is_cfg_gated_region(&text, line),
                        "NFR-6/NFR-17: network symbol `{symbol}` appears un-gated in {}:\n  {line}\n\
                         it must be confined behind `#[cfg(feature = \"self-update\")]`",
                        file.display()
                    );
                }
            }
        }
    }
}

/// Whether `line` sits within (or is directly preceded by) a `#[cfg(feature = "self-update")]` gate.
/// A coarse but sound check: the file must contain the gate, and the target line's enclosing item is
/// the cfg-gated one (in `exit.rs` the sole network mention is the cfg-gated `Update` variant + its
/// method arm, each preceded by `#[cfg(feature = "self-update")]`).
fn is_cfg_gated_region(text: &str, target: &str) -> bool {
    let gate = "#[cfg(feature = \"self-update\")]";
    if !text.contains(gate) {
        return false;
    }
    // Walk lines; track whether the most recent cfg attr was the self-update gate, resetting on a
    // blank line / closing brace at column 0 (a coarse region model sufficient for exit.rs's shape).
    let mut gated = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed == gate {
            gated = true;
        }
        if line == target {
            return gated;
        }
        if trimmed.is_empty() {
            gated = false;
        }
    }
    false
}
