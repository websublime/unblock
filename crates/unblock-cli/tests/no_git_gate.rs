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
//!
//! # The AUTHORITATIVE network-confinement gate is the feature-matrix build
//!
//! The definitive proof that the ONLY network surface is confined behind the default-on `self-update`
//! feature is the CI `cargo build -p unblock-cli --no-default-features` job (ci-cd §2): it compiles
//! the crate with `self-update` OFF, so if ANY network dependency (axoupdater/reqwest/hyper) were
//! reachable un-gated, the build would fail to resolve it. This source scan is DEFENSE-IN-DEPTH: a
//! fast, human-readable tripwire that catches a network symbol leaking into an un-gated line at review
//! time (before the feature-matrix build even runs), not a replacement for that build.

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

/// Whether `target` sits within a `#[cfg(feature = "self-update")]`-gated item — robust across BLANK
/// LINES and doc-comment/attribute preludes (the earlier model reset on any blank line, so a gate
/// separated from its symbol by a blank line was wrongly seen as un-gated).
///
/// # The model
///
/// The gate applies to the **item it immediately precedes** and to everything nested inside that
/// item's block. We track the innermost enclosing gate by pairing the `#[cfg(feature =
/// "self-update")]` attribute with the brace-depth of the item it opens:
///
/// - When we see the gate attribute, we ARM it: the next `{` that opens a block records the
///   depth at which the gated item begins (a doc comment or other attributes may sit between the gate
///   and the `{` — blank lines included — and the arm survives them, unlike the old blank-line reset).
/// - A `target` line is gated iff we are currently inside a block opened while a gate was armed (depth
///   is at or below a recorded gate-open depth), so a following blank line no longer clears it.
/// - When a gated block closes (`}` returns below its open depth), the gate stops applying — a later
///   un-gated item is correctly seen as un-gated (so the scan still catches a genuine leak).
///
/// Brace counting on raw source is coarse (it ignores braces inside strings/comments), but the
/// network symbols are confined to `commands/update.rs` (whole-module-gated, short-circuited before
/// this fn) + inert doc comments, so this is a sound tripwire. The AUTHORITATIVE gate remains the
/// `--no-default-features` build (see the module header).
fn is_cfg_gated_region(text: &str, target: &str) -> bool {
    let gate = "#[cfg(feature = \"self-update\")]";
    if !text.contains(gate) {
        return false;
    }

    let mut depth: i32 = 0;
    // The brace-depths at which currently-open gated items began (a stack — nested gates possible).
    let mut gated_depths: Vec<i32> = Vec::new();
    // Whether the most recent attribute prelude armed a gate awaiting its opening `{`.
    let mut armed = false;

    for line in text.lines() {
        let trimmed = line.trim_start();

        // The target may itself be un-braced (e.g. a match arm / a field): it is gated iff we are
        // already inside a gated item's block OR a gate is armed for the item this very line opens.
        if line == target {
            return !gated_depths.is_empty() || armed;
        }

        if trimmed == gate {
            armed = true;
        }

        for ch in line.chars() {
            match ch {
                '{' => {
                    if armed {
                        // This block belongs to the gated item; record the depth it opened at.
                        gated_depths.push(depth);
                        armed = false;
                    }
                    depth += 1;
                }
                '}' => {
                    depth -= 1;
                    // Leaving a gated item's block clears its gate.
                    if gated_depths.last().is_some_and(|&d| depth <= d) {
                        gated_depths.pop();
                    }
                }
                _ => {}
            }
        }
    }
    false
}

/// Self-tests for the hardened `is_cfg_gated_region` scanner — proving it (a) survives blank lines and
/// doc-comment preludes between a gate and its symbol (the fragility SF-C fixes) AND (b) still catches
/// a genuinely un-gated network symbol (defense-in-depth non-vacuity: the scan must not be a rubber
/// stamp). These are the tripwire's own regression pins.
#[cfg(test)]
mod scanner_self_tests {
    use super::is_cfg_gated_region;

    /// A gate separated from the symbol line by a BLANK LINE + a doc comment is still seen as gated —
    /// the exact case the old blank-line-reset model got wrong.
    #[test]
    fn gate_survives_blank_line_and_doc_comment() {
        let text = "\
#[cfg(feature = \"self-update\")]

/// A doc comment sitting between the gate and the item.
fn updater() {
    let client = reqwest::Client::new();
}
";
        let target = "    let client = reqwest::Client::new();";
        assert!(
            is_cfg_gated_region(text, target),
            "a gate must still apply across a blank line + doc comment (SF-C hardening)"
        );
    }

    /// A network symbol in an item that is NOT gated (a sibling AFTER a gated item's block closes) is
    /// correctly seen as UN-gated — so the scan still fails on a genuine leak (non-vacuity).
    #[test]
    fn ungated_sibling_symbol_is_not_gated() {
        let text = "\
#[cfg(feature = \"self-update\")]
fn gated() {
    let ok = reqwest::Client::new();
}

fn leaked() {
    let bad = reqwest::Client::new();
}
";
        let gated_line = "    let ok = reqwest::Client::new();";
        let leaked_line = "    let bad = reqwest::Client::new();";
        assert!(
            is_cfg_gated_region(text, gated_line),
            "the symbol inside the gated fn is gated"
        );
        assert!(
            !is_cfg_gated_region(text, leaked_line),
            "a symbol in an UN-gated sibling item must be seen as a leak (non-vacuous scan)"
        );
    }

    /// A file with NO gate never reports gated (the short-circuit) — a bare network symbol is a leak.
    #[test]
    fn no_gate_in_file_is_never_gated() {
        let text = "fn f() {\n    let x = std::net::TcpStream::connect(\"h:1\");\n}\n";
        let target = "    let x = std::net::TcpStream::connect(\"h:1\");";
        assert!(
            !is_cfg_gated_region(text, target),
            "a network symbol with no gate anywhere is a leak"
        );
    }
}
