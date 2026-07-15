//! `no-network` — workspace-wide network-confinement source-scan (NFR-17/NFR-10, D5/T3.6).
//!
//! A mechanical, offline scan over EVERY `.rs` file under `crates/*/src` + `xtask/src` that FAILS if a
//! networking symbol (`reqwest`, `hyper::`, `std::net::`, `TcpStream`, `rustls`, un-gated `axoupdater`)
//! appears OUTSIDE the single whitelisted, `self-update`-feature-gated `axoupdater` path in
//! `crates/unblock-cli/src/commands/update.rs`. This is the workspace-wide realization of the T3.1
//! crate-scoped tripwire (`crates/unblock-cli/tests/no_git_gate.rs`, kept as a fast per-crate check):
//! `unblock`'s ONLY network surface is `unblock update`, confined behind the default-on `self-update`
//! feature, so no other crate may link a network symbol.
//!
//! Run: `cargo xtask no-network`. The CI `no-network` job wires this in (ci-cd §2 / §5 NFR-17).
//!
//! # Distinct from the libsql `remote` stack (D15)
//! This is a SOURCE-symbol scan; the D15-banned libsql `remote` TLS stack (reqwest/hyper/rustls pulled
//! by `--all-features`) is kept off the build by the targeted-features policy + `cargo deny`, NOT by
//! this scan (no crate references those symbols in source).
//!
//! # The AUTHORITATIVE confinement gate is the feature-matrix build
//! The definitive proof is the CI `cargo build -p unblock-cli --no-default-features` job (`feature-matrix`,
//! ci-cd §2): with `self-update` OFF the crate compiles, so `axoupdater`/`reqwest`/`hyper` are provably
//! unreachable un-gated. This scan is DEFENSE-IN-DEPTH — a fast, human-readable tripwire that catches a
//! network symbol leaking into an un-gated line at review time, workspace-wide.
//!
//! # Whitelist integrity
//! `commands/update.rs` is exempt BY FILENAME, so this gate also asserts its `mod` declaration stays
//! `#[cfg(feature = "self-update")]`-gated (`commands/mod.rs`) — otherwise the by-filename exemption
//! could silently hide a de-gated network path (P1 Verify should-fix). This file
//! (`xtask/src/no_network.rs`) is itself exempt: it NAMES the banned symbols as search patterns.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Network / self-update symbols that must NOT appear un-gated in any crate source (NFR-17). Confined to
/// the whitelisted `self-update`-gated `commands/update.rs`; every other occurrence is a leak unless it
/// is a comment or sits inside a `#[cfg(feature = "self-update")]`-gated region.
const NETWORK_SYMBOLS: &[&str] = &[
    "reqwest",
    "hyper::",
    "std::net::",
    "TcpStream",
    "rustls",
    "axoupdater",
];

/// The `self-update` feature gate attribute (exact spelling produced by the crate).
const SELF_UPDATE_GATE: &str = "#[cfg(feature = \"self-update\")]";

/// The two files exempt from the symbol scan (workspace-relative, forward-slash normalized):
/// - the whitelisted live network path (its gate is asserted separately, see [`assert_update_mod_gated`]);
/// - this scanner's own source (it names the banned symbols as search patterns).
const WHITELISTED_FILES: &[&str] = &[
    "crates/unblock-cli/src/commands/update.rs",
    "xtask/src/no_network.rs",
];

/// A single un-gated network-symbol finding (`path:line: <symbol>`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetFinding {
    /// Workspace-relative path of the offending source file.
    pub file: String,
    /// 1-based line number of the offending symbol.
    pub line: usize,
    /// The offending network symbol (e.g. `reqwest`).
    pub symbol: String,
    /// The offending line, trimmed (for the report).
    pub snippet: String,
}

impl NetFinding {
    fn render(&self) -> String {
        format!(
            "{}:{}: [net] `{}` — {}",
            self.file, self.line, self.symbol, self.snippet
        )
    }
}

/// Entry point for `cargo xtask no-network`.
#[must_use]
pub fn no_network() -> ExitCode {
    let root = match workspace_root() {
        Ok(root) => root,
        Err(err) => {
            eprintln!("no-network: could not locate workspace root: {err}");
            return ExitCode::FAILURE;
        }
    };

    match scan_at(&root) {
        Ok((findings, scanned)) => report(&findings, scanned),
        Err(err) => {
            eprintln!("no-network: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Resolve the workspace root from `CARGO_MANIFEST_DIR` (xtask sits one level under root).
fn workspace_root() -> Result<PathBuf, String> {
    let manifest = std::env::var("CARGO_MANIFEST_DIR")
        .map_err(|_| "CARGO_MANIFEST_DIR not set (run via `cargo xtask no-network`)".to_owned())?;
    Path::new(&manifest)
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("CARGO_MANIFEST_DIR {manifest:?} has no parent"))
}

/// Scan every `.rs` under `<root>/crates/*/src` + `<root>/xtask/src`. Returns `(findings, files_scanned)`.
///
/// Also asserts the `commands/update.rs` `mod` gate (whitelist integrity, [`assert_update_mod_gated`]).
///
/// # Errors
/// Returns `Err` if the `commands/mod.rs` gate is missing, or no source files are found (vacuous-pass
/// guard).
pub fn scan_at(root: &Path) -> Result<(Vec<NetFinding>, usize), String> {
    // Whitelist integrity: the by-filename exemption of `commands/update.rs` may not hide a de-gated
    // path — its `mod` declaration MUST stay `self-update`-gated (P1 Verify should-fix).
    let mod_rs = root.join("crates/unblock-cli/src/commands/mod.rs");
    let mod_text = std::fs::read_to_string(&mod_rs)
        .map_err(|e| format!("cannot read {}: {e}", mod_rs.display()))?;
    assert_update_mod_gated(&mod_text)?;

    let files = source_files(root);
    if files.is_empty() {
        return Err(
            "no *.rs source files under crates/*/src or xtask/src — refusing a vacuous pass"
                .to_owned(),
        );
    }

    let mut findings = Vec::new();
    for path in &files {
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let whitelisted = WHITELISTED_FILES.contains(&rel.as_str());
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        findings.extend(scan_file(&rel, &text, whitelisted));
    }
    findings.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    Ok((findings, files.len()))
}

/// Scan one file's text for un-gated network symbols. Whitelisted files return no findings.
///
/// The testable core: unit tests plant an un-gated `std::net::TcpStream` (must be REJECTED) and a
/// `self-update`-gated symbol (must be ACCEPTED), the non-vacuity control.
#[must_use]
pub fn scan_file(file: &str, text: &str, whitelisted: bool) -> Vec<NetFinding> {
    if whitelisted {
        return Vec::new();
    }
    let mut findings = Vec::new();
    for (i, line) in text.lines().enumerate() {
        for symbol in NETWORK_SYMBOLS {
            if !line.contains(symbol) {
                continue;
            }
            // Comments are inert; a line mentioning the feature name is a doc/gate reference, not a live
            // network call. Everything else must sit inside a `self-update`-gated region.
            let inert = line.trim_start().starts_with("//") || line.contains("self-update");
            if inert || is_cfg_gated_region(text, line) {
                continue;
            }
            findings.push(NetFinding {
                file: file.to_owned(),
                line: i + 1,
                symbol: (*symbol).to_owned(),
                snippet: line.trim().to_owned(),
            });
        }
    }
    findings
}

/// Assert `commands/mod.rs` keeps `mod update;` behind `#[cfg(feature = "self-update")]` — so the
/// by-filename whitelist of `commands/update.rs` cannot hide a de-gated network path.
///
/// # Errors
/// Returns `Err` if `mod update;` is absent or its nearest preceding non-blank line is not the gate.
pub fn assert_update_mod_gated(mod_text: &str) -> Result<(), String> {
    let lines: Vec<&str> = mod_text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t == "pub mod update;" || t == "mod update;" {
            let gated = lines[..i]
                .iter()
                .rev()
                .find(|l| !l.trim().is_empty())
                .is_some_and(|l| l.trim() == SELF_UPDATE_GATE);
            return if gated {
                Ok(())
            } else {
                Err(format!(
                    "`{t}` in commands/mod.rs is NOT `{SELF_UPDATE_GATE}`-gated — the no-network \
                     by-filename exemption of commands/update.rs would then hide a de-gated network path"
                ))
            };
        }
    }
    Err(
        "no `mod update;` declaration found in commands/mod.rs to verify the self-update gate"
            .to_owned(),
    )
}

/// Whether `target` sits within a `#[cfg(feature = "self-update")]`-gated item — robust across blank
/// lines and doc-comment/attribute preludes. Ported from the T3.1 `no_git_gate.rs` scanner (SF-C model):
/// the gate applies to the item it immediately precedes and everything nested inside that item's block.
fn is_cfg_gated_region(text: &str, target: &str) -> bool {
    if !text.contains(SELF_UPDATE_GATE) {
        return false;
    }

    let mut depth: i32 = 0;
    let mut gated_depths: Vec<i32> = Vec::new();
    let mut armed = false;

    for line in text.lines() {
        let trimmed = line.trim_start();

        // The target may be un-braced (a match arm / a field): gated iff already inside a gated block OR
        // a gate is armed for the item this line opens.
        if line == target {
            return !gated_depths.is_empty() || armed;
        }

        if trimmed == SELF_UPDATE_GATE {
            armed = true;
        }

        for ch in line.chars() {
            match ch {
                '{' => {
                    if armed {
                        gated_depths.push(depth);
                        armed = false;
                    }
                    depth += 1;
                }
                '}' => {
                    depth -= 1;
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

/// Every `.rs` under `<root>/crates/*/src` + `<root>/xtask/src`, sorted for deterministic reporting.
fn source_files(root: &Path) -> Vec<PathBuf> {
    let mut scan_roots: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root.join("crates")) {
        for entry in entries.flatten() {
            let src = entry.path().join("src");
            if src.is_dir() {
                scan_roots.push(src);
            }
        }
    }
    let xtask_src = root.join("xtask").join("src");
    if xtask_src.is_dir() {
        scan_roots.push(xtask_src);
    }

    let mut files = Vec::new();
    for scan_root in scan_roots {
        let mut stack = vec![scan_root];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    files.push(path);
                }
            }
        }
    }
    files.sort();
    files
}

/// Emit findings and return the process exit code.
fn report(findings: &[NetFinding], scanned: usize) -> ExitCode {
    if findings.is_empty() {
        println!(
            "no-network OK: {scanned} source file(s), no un-gated network symbol \
             (only the whitelisted self-update axoupdater path)"
        );
        return ExitCode::SUCCESS;
    }
    eprintln!("UN-GATED NETWORK SYMBOLS (NFR-17):");
    for f in findings {
        eprintln!("{}", f.render());
    }
    eprintln!(
        "\nthe ONLY network surface is `unblock update`, confined to \
         crates/unblock-cli/src/commands/update.rs behind `#[cfg(feature = \"self-update\")]`; \
         gate or remove the symbol(s) above."
    );
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Symbol-scan non-vacuity (the scan must not be a rubber stamp). ----

    #[test]
    fn ungated_network_symbol_is_a_finding() {
        // A planted `std::net::TcpStream` in a NON-whitelisted file is a leak — the primary non-vacuity
        // proof (mirrors the manual `plant → RED → restore` self-verify step). The line trips BOTH
        // `std::net::` and `TcpStream`, so it yields one finding per matched symbol.
        let text = "fn connect() {\n    let _ = std::net::TcpStream::connect(\"h:1\");\n}\n";
        let f = scan_file("crates/unblock-model/src/leak.rs", text, false);
        assert_eq!(
            f.len(),
            2,
            "expected two symbol findings on the line, got {f:?}"
        );
        assert!(f.iter().all(|x| x.line == 2), "both on line 2, got {f:?}");
        assert!(f.iter().any(|x| x.symbol == "std::net::"));
        assert!(f.iter().any(|x| x.symbol == "TcpStream"));
    }

    #[test]
    fn every_symbol_class_is_detected() {
        for sym in ["reqwest", "hyper::", "TcpStream", "rustls", "axoupdater"] {
            let text = format!("fn f() {{\n    let _ = {sym};\n}}\n");
            let f = scan_file("crates/unblock-x/src/lib.rs", &text, false);
            assert_eq!(f.len(), 1, "symbol `{sym}` must be detected, got {f:?}");
        }
    }

    #[test]
    fn comment_and_feature_mention_are_inert() {
        // A commented symbol and a `self-update` doc mention must NOT fire (false-positive guard).
        let text = "// uses reqwest under self-update\n/// axoupdater docs\nfn f() {}\n";
        let f = scan_file("crates/unblock-x/src/lib.rs", text, false);
        assert!(
            f.is_empty(),
            "comment/feature-mention lines must not fire, got {f:?}"
        );
    }

    #[test]
    fn gated_symbol_is_accepted() {
        // A network symbol inside a `#[cfg(feature = "self-update")]`-gated item is confined by design.
        let text = "\
#[cfg(feature = \"self-update\")]
fn updater() {
    let _client = reqwest::Client::new();
}
";
        let f = scan_file("crates/unblock-x/src/lib.rs", text, false);
        assert!(f.is_empty(), "a gated symbol must be accepted, got {f:?}");
    }

    #[test]
    fn ungated_sibling_after_gated_block_is_a_leak() {
        // A symbol in an UN-gated sibling AFTER a gated block closes is still a leak (non-vacuity).
        let text = "\
#[cfg(feature = \"self-update\")]
fn gated() {
    let _ok = reqwest::Client::new();
}

fn leaked() {
    let _bad = reqwest::Client::new();
}
";
        let f = scan_file("crates/unblock-x/src/lib.rs", text, false);
        assert_eq!(
            f.len(),
            1,
            "the un-gated sibling symbol must leak, got {f:?}"
        );
        assert_eq!(f[0].line, 7, "the leaked `reqwest` sits on line 7");
    }

    #[test]
    fn whitelisted_file_is_skipped() {
        let text = "use axoupdater::AxoUpdater;\nfn f() { let _ = reqwest::Client::new(); }\n";
        let f = scan_file("crates/unblock-cli/src/commands/update.rs", text, true);
        assert!(
            f.is_empty(),
            "the whitelisted live path is exempt, got {f:?}"
        );
    }

    // ---- `mod update;` gate integrity (the by-filename exemption may not hide a de-gated path). ----

    #[test]
    fn gated_mod_update_passes() {
        let text = "pub mod version;\n\n#[cfg(feature = \"self-update\")]\npub mod update;\n";
        assert!(assert_update_mod_gated(text).is_ok());
    }

    #[test]
    fn ungated_mod_update_fails() {
        // If someone de-gates `mod update;`, the by-filename exemption would hide a live network path —
        // the assertion MUST fail (whitelist-integrity non-vacuity).
        let text = "pub mod version;\npub mod update;\n";
        assert!(
            assert_update_mod_gated(text).is_err(),
            "an un-gated `mod update;` must fail the gate assertion"
        );
    }

    #[test]
    fn missing_mod_update_fails() {
        let text = "pub mod version;\npub mod migrate;\n";
        assert!(
            assert_update_mod_gated(text).is_err(),
            "a missing `mod update;` must fail (cannot verify the gate)"
        );
    }
}
