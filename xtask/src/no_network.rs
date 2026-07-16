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
//! # Symbol-scoped self-update exemption
//! `commands/update.rs` is NOT exempt as a whole file — only its vetted `axoupdater` symbol is exempt
//! (any OTHER network symbol there, e.g. a raw socket, is STILL scanned, even behind the module gate).
//! That symbol-scoped exemption is justified by asserting the module's `mod` declaration stays
//! `#[cfg(feature = "self-update")]`-gated (`commands/mod.rs`, [`assert_update_mod_gated`]) — otherwise a
//! de-gated `mod update;` could reintroduce a live network path. This file (`xtask/src/no_network.rs`)
//! is itself fully exempt: it NAMES the banned symbols as search patterns.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Network / self-update symbols that must NOT appear un-gated in any crate source (NFR-17). The only
/// exempt live reference is the vetted `axoupdater` symbol in the `self-update`-gated
/// `commands/update.rs`; every other occurrence is a leak unless it is a comment or sits inside a
/// `#[cfg(feature = "self-update")]`-gated region.
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

/// The scanner's own source — fully exempt: it names the banned symbols as literal search patterns.
const SCANNER_SELF: &str = "xtask/src/no_network.rs";

/// The single whitelisted live network path: the `self-update`-gated updater module. ONLY the vetted
/// `axoupdater` symbol is exempt HERE (the module's `#[cfg(feature = "self-update")]` `mod` gate is
/// asserted separately, see `assert_update_mod_gated`, which JUSTIFIES this exemption). Every OTHER
/// network symbol in this file is still scanned — a raw socket / `reqwest` here would be a NEW un-vetted
/// network surface even behind the module gate, so it must still sit inside an inline
/// `#[cfg(feature = "self-update")]`-gated region (defense-in-depth; NOT a whole-file blanket skip).
const UPDATER_PATH: &str = "crates/unblock-cli/src/commands/update.rs";

/// The one network symbol the updater module is permitted to reference (the vetted self-update library).
const UPDATER_EXEMPT_SYMBOL: &str = "axoupdater";

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
/// Also asserts the `commands/update.rs` `mod` gate (self-update exemption integrity,
/// [`assert_update_mod_gated`]).
///
/// # Errors
/// Returns `Err` if the `commands/mod.rs` gate is missing, or no source files are found (vacuous-pass
/// guard).
pub fn scan_at(root: &Path) -> Result<(Vec<NetFinding>, usize), String> {
    // Integrity: the symbol-scoped `axoupdater` exemption of `commands/update.rs` (justified by the
    // asserted `mod` gate) may not hide a de-gated path — its `mod` declaration MUST stay
    // `self-update`-gated (P1 Verify should-fix).
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
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        findings.extend(scan_file(&rel, &text));
    }
    findings.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    Ok((findings, files.len()))
}

/// Scan one file's text for un-gated network symbols. Per-file policy is derived from the workspace-
/// relative path: the scanner's own source is fully exempt; the updater module may reference ONLY the
/// vetted `axoupdater` symbol; everywhere else every network symbol must be a comment or sit inside a
/// `#[cfg(feature = "self-update")]`-gated region.
///
/// The testable core: unit tests plant an un-gated `std::net::TcpStream` (must be REJECTED) and a
/// `self-update`-gated symbol (must be ACCEPTED), the non-vacuity control.
#[must_use]
pub fn scan_file(file: &str, text: &str) -> Vec<NetFinding> {
    // The scanner's own source names the banned symbols as literal search patterns.
    if file == SCANNER_SELF {
        return Vec::new();
    }
    // The updater module may reference ONLY the vetted `axoupdater` symbol (its `mod` is
    // `self-update`-gated, asserted separately); any OTHER network symbol there is still scanned.
    let is_updater = file == UPDATER_PATH;
    let mut findings = Vec::new();
    for (i, line) in text.lines().enumerate() {
        for symbol in NETWORK_SYMBOLS {
            if !line.contains(symbol) {
                continue;
            }
            // Comment lines are inert (a doc/gate reference, not a live network call).
            if line.trim_start().starts_with("//") {
                continue;
            }
            // The updater's vetted `axoupdater` symbol is exempt (module-gated). SYMBOL-scoped: a raw
            // `std::net::`/`TcpStream`/`reqwest` added to update.rs is NOT exempt by this clause.
            if is_updater && *symbol == UPDATER_EXEMPT_SYMBOL {
                continue;
            }
            // Everything else must sit inside a `#[cfg(feature = "self-update")]`-gated region.
            if is_cfg_gated_region(text, i) {
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
/// no-network `axoupdater`-symbol exemption of `commands/update.rs` cannot hide a de-gated network path.
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
                     `axoupdater`-symbol exemption of commands/update.rs would then hide a de-gated \
                     network path"
                ))
            };
        }
    }
    Err(
        "no `mod update;` declaration found in commands/mod.rs to verify the self-update gate"
            .to_owned(),
    )
}

/// Whether the line at `target_idx` sits within a `#[cfg(feature = "self-update")]`-gated item.
///
/// The gate applies to the item it immediately precedes and everything nested inside that item's block.
/// We pair the gate attribute with the brace-depth of the item it opens and track the innermost
/// enclosing gate. Hardened (v1.1) against three false-greens the earlier coarse scan carried:
///  - **H1** an un-braced gated item (`use`/`const`/`type`/`static`/a field) no longer leaks its gate
///    onto the next unrelated block: `armed` clears at the item's top-level `;` terminator, the item's
///    top-level `,` (a struct field / enum variant separator, told apart from a generic `<…>` comma by a
///    heuristic angle depth, and suppressed inside a `where`-clause whose bound commas are NOT item
///    terminators, W1), or when the block enclosing the un-braced item closes without the item
///    having opened its own block.
///  - **H2** the target is matched by LINE INDEX, not by text, so two byte-identical lines (one gated,
///    one not) are told apart.
///  - **H3** braces/terminators inside string/char literals and `//`/`/* */` comments are ignored (a
///    lightweight lexer blanks them before the brace scan), so a stray `{` in a string cannot inflate
///    the depth.
///
/// Coarse by design (a defense-in-depth tripwire, not a parser). Unrecognized gate forms over-flag (a
/// safe, spurious finding). The one known UNDER-flag is a **tuple-struct paren-field sibling**
/// (`struct S(#[cfg(feature = "self-update")] A, B)`): a tuple field comma sits inside `()`, textually
/// indistinguishable from an fn-param comma without a real parser, so a gated tuple field's arm leaks to
/// the next tuple field. Zero corpus exposure; the `--no-default-features` feature-matrix build is the
/// authoritative confinement gate that closes it.
fn is_cfg_gated_region(text: &str, target_idx: usize) -> bool {
    if !text.contains(SELF_UPDATE_GATE) {
        return false;
    }
    let mut depth: i32 = 0; // brace {} nesting
    let mut group: i32 = 0; // () and [] nesting — to find a TOP-LEVEL ';'
    let mut angle: i32 = 0; // generic <> nesting — to tell a generic ',' from a field/variant ','
    let mut gated_depths: Vec<i32> = Vec::new(); // brace depths at which open gated items began
    let mut armed = false; // a gate is pending, awaiting its item
    let mut in_where = false; // inside the armed item's `where`-clause (its commas are not terminators)
    let mut armed_depth: i32 = 0; // brace depth when the pending gate was armed
    let mut block_comment: i32 = 0; // /* */ nesting carried across lines

    for (i, line) in text.lines().enumerate() {
        if i == target_idx {
            return !gated_depths.is_empty() || armed;
        }
        let in_comment_at_start = block_comment > 0;
        let code = code_only(line, &mut block_comment);

        // Arm on the EXACT gate attribute (never when the line began inside a block comment).
        if !in_comment_at_start && line.trim_start() == SELF_UPDATE_GATE {
            armed = true;
            armed_depth = depth;
            in_where = false;
            continue; // an attribute line opens no braces and terminates no item
        }
        // A where-clause's bound commas are NOT item terminators — suppress the `,`-disarm until the
        // item's block opens, else a gated generic fn with a multi-line `where` is wrongly flagged (W1).
        if armed && !in_where && has_where_keyword(&code) {
            in_where = true;
        }
        let mut prev = ' '; // last significant code char — for <> generic detection (armed-window only)
        for ch in code.chars() {
            match ch {
                '(' | '[' => group += 1,
                ')' | ']' => group = (group - 1).max(0), // F4: floor at 0 (a mis-lex can't drive it negative)
                // Heuristic generic-angle depth. In the armed window (gate → item header) `<`/`>` are
                // generics or the `->` arrow; count `<` only in a type position (after an ident / `>`),
                // and `>` except in `->`. Purely to tell a generic `,` from a field/variant `,` (D1).
                '<' if prev.is_ascii_alphanumeric() || prev == '_' || prev == '>' => angle += 1,
                '>' if prev != '-' => angle = (angle - 1).max(0),
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
                    // The block enclosing an un-braced gated item closed → drop the stale arm (H1).
                    if armed && depth < armed_depth {
                        armed = false;
                    }
                }
                // An un-braced gated item ends at its top-level `;` (use/const/type/static) …
                ';' if armed && group == 0 => armed = false,
                // … or a top-level `,` that is a struct FIELD / enum VARIANT separator — NOT a generic
                // `<…>` comma (angle == 0) and NOT a fn-param comma (group == 0) (D1, H1 comma-sibling).
                ',' if armed && group == 0 && angle == 0 && !in_where => armed = false,
                _ => {}
            }
            if !ch.is_whitespace() {
                prev = ch;
            }
        }
    }
    false
}

/// Whether `code` contains the `where` keyword as a standalone word (a where-clause opener). Used to
/// suppress the field/variant `,`-disarm inside a where-clause: its bound commas are NOT item
/// terminators, so without this a gated generic fn with a `where` clause would disarm mid-signature and
/// be spuriously flagged (a false-RED). `where` is a reserved keyword, so a word-boundary match is exact.
fn has_where_keyword(code: &str) -> bool {
    code.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|w| w == "where")
}

/// Return `line` with `//`/`/* */` comment, `"…"`/raw-string, and `'…'` char-literal content removed,
/// so a following brace/terminator scan sees only real code punctuation (H3). `block_comment` carries
/// `/* */` nesting across lines. Lifetimes (`'a`) are emitted as harmless ticks (no braces/terminators).
///
/// Single-line coverage. The accepted lexer residuals are (a) a multi-line raw string and (b) a
/// `\`-continuation multi-line normal string — BOTH mis-count only in the over-flag (false-RED)
/// direction on the continuation line. The `group`/`angle` disarm counters are floored at 0 (D1/F4), so
/// a stray unbalanced `)`/`]`/`>` cannot drive a disarm guard permanently false. The
/// `--no-default-features` feature-matrix build remains the authoritative gate.
fn code_only(line: &str, block_comment: &mut i32) -> String {
    let b = line.as_bytes();
    let mut out = String::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if *block_comment > 0 {
            if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
                *block_comment += 1;
                i += 2;
            } else if b[i] == b'*' && i + 1 < b.len() && b[i + 1] == b'/' {
                *block_comment -= 1;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        match b[i] {
            b'/' if i + 1 < b.len() && b[i + 1] == b'/' => break, // line comment: drop the rest
            b'/' if i + 1 < b.len() && b[i + 1] == b'*' => {
                *block_comment += 1;
                i += 2;
            }
            b'r' if starts_raw_string(b, i) => i = skip_raw_string(b, i),
            b'b' if i + 1 < b.len() && b[i + 1] == b'r' && starts_raw_string(b, i + 1) => {
                i = skip_raw_string(b, i + 1);
            }
            b'"' => i = skip_string(b, i),
            b'b' if i + 1 < b.len() && b[i + 1] == b'"' => i = skip_string(b, i + 1),
            b'\'' => {
                if let Some(next) = skip_char_literal(b, i) {
                    i = next;
                } else {
                    out.push('\''); // a lifetime tick — harmless
                    i += 1;
                }
            }
            other if other.is_ascii() => {
                out.push(char::from(other));
                i += 1;
            }
            _ => i += 1, // non-ASCII (identifier bytes) — never structural punctuation
        }
    }
    out
}

/// Whether a raw-string prefix (`r`, `r#`, `r##`, …) begins at `b[i] == b'r'` (i.e. `r` then `#`* then `"`).
fn starts_raw_string(b: &[u8], i: usize) -> bool {
    let mut j = i + 1;
    while j < b.len() && b[j] == b'#' {
        j += 1;
    }
    j < b.len() && b[j] == b'"'
}

/// Skip a raw string starting at `b[i] == b'r'`; return the index just past the closing `"#*` (or
/// `b.len()` if unterminated on this line).
fn skip_raw_string(b: &[u8], i: usize) -> usize {
    let mut hashes: usize = 0;
    let mut j = i + 1;
    while j < b.len() && b[j] == b'#' {
        hashes += 1;
        j += 1;
    }
    j += 1; // past the opening '"'
    while j < b.len() {
        if b[j] == b'"' {
            let mut end = j + 1;
            let mut matched: usize = 0;
            while matched < hashes && end < b.len() && b[end] == b'#' {
                matched += 1;
                end += 1;
            }
            if matched == hashes {
                return end;
            }
        }
        j += 1;
    }
    b.len()
}

/// Skip a normal/byte string starting at `b[i] == b'"'`; return the index just past the closing `"`.
/// Honors `\"` escapes; returns `b.len()` if unterminated on this line.
fn skip_string(b: &[u8], i: usize) -> usize {
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            b'"' => return j + 1,
            _ => j += 1,
        }
    }
    b.len()
}

/// Skip a char literal at `b[i] == b'\''`; `Some(next)` past the closing `'` if it IS a char literal
/// (`'x'`, `'\n'`, `'\''`, `'{'`, `'\u{7f}'`, …), or `None` if it is a lifetime tick (`'a`, `'static`).
fn skip_char_literal(b: &[u8], i: usize) -> Option<usize> {
    if i + 1 >= b.len() {
        return None;
    }
    if b[i + 1] == b'\\' {
        // escaped: the byte at i+2 is escaped content (possibly a quote); the closing quote is the next
        // '\'' at or after i+3 (skips any `{`/`}` inside a `\u{..}` escape).
        let mut j = i + 3;
        while j < b.len() && b[j] != b'\'' {
            j += 1;
        }
        return (j < b.len()).then_some(j + 1);
    }
    if i + 2 < b.len() && b[i + 2] == b'\'' {
        return Some(i + 3); // plain 'x'
    }
    None // a lifetime
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
        // A planted `std::net::TcpStream` in a NON-exempt file is a leak — the primary non-vacuity
        // proof (mirrors the manual `plant → RED → restore` self-verify step). The line trips BOTH
        // `std::net::` and `TcpStream`, so it yields one finding per matched symbol.
        let text = "fn connect() {\n    let _ = std::net::TcpStream::connect(\"h:1\");\n}\n";
        let f = scan_file("crates/unblock-model/src/leak.rs", text);
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
            let f = scan_file("crates/unblock-x/src/lib.rs", &text);
            assert_eq!(f.len(), 1, "symbol `{sym}` must be detected, got {f:?}");
        }
    }

    #[test]
    fn comment_lines_are_inert() {
        // Commented symbols (a `//` line and a `///` doc line) must NOT fire — the comment PREFIX makes
        // them inert (not the `self-update` substring, whose blanket rule was removed as an escape).
        let text = "// uses reqwest under self-update\n/// axoupdater docs\nfn f() {}\n";
        let f = scan_file("crates/unblock-x/src/lib.rs", text);
        assert!(f.is_empty(), "comment lines must not fire, got {f:?}");
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
        let f = scan_file("crates/unblock-x/src/lib.rs", text);
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
        let f = scan_file("crates/unblock-x/src/lib.rs", text);
        assert_eq!(
            f.len(),
            1,
            "the un-gated sibling symbol must leak, got {f:?}"
        );
        assert_eq!(f[0].line, 7, "the leaked `reqwest` sits on line 7");
    }

    // ---- Symbol-scoped model: the two closed escape vectors + the legit self-update path. ----

    #[test]
    fn stdnet_with_self_update_substring_is_a_finding() {
        // VECTOR 1 (closed): a `std::net` line carrying a `self-update` substring is NO LONGER inert.
        let text =
            "fn f() {\n    let _ = std::net::TcpStream::connect(\"self-update-host:1\");\n}\n";
        let f = scan_file("crates/unblock-model/src/leak.rs", text);
        assert!(
            !f.is_empty(),
            "a std::net line with a self-update substring must be a finding, got {f:?}"
        );
        assert!(f.iter().any(|x| x.symbol == "std::net::"));
        assert!(f.iter().any(|x| x.symbol == "TcpStream"));
    }

    #[test]
    fn raw_socket_in_update_rs_is_a_finding() {
        // VECTOR 2 (closed): a NON-axoupdater network symbol in update.rs is NO LONGER blanket-exempt —
        // here under a NON-self-update cfg (a different feature), so is_cfg_gated_region is false → RED.
        let text = "\
#[cfg(feature = \"telemetry\")]
fn beacon() {
    let _ = std::net::TcpStream::connect(\"self-update-host:1\");
}
";
        let f = scan_file(UPDATER_PATH, text);
        assert!(
            !f.is_empty(),
            "a raw socket in update.rs must be a finding, got {f:?}"
        );
        assert!(f.iter().any(|x| x.symbol == "std::net::"));
    }

    #[test]
    fn axoupdater_in_update_rs_is_exempt() {
        // The vetted axoupdater symbol in update.rs stays exempt (symbol-scoped; module-gated).
        let text = "use axoupdater::AxoUpdater;\nfn run() { let _ = axoupdater::AxoUpdater::new_for(\"x\"); }\n";
        let f = scan_file(UPDATER_PATH, text);
        assert!(
            f.is_empty(),
            "the vetted axoupdater symbol in update.rs is exempt, got {f:?}"
        );
    }

    #[test]
    fn axoupdater_in_update_rs_but_a_raw_socket_beside_it_still_leaks() {
        // Mixed file: axoupdater exempt, but a raw std::net on another line still leaks (symbol-scoped).
        let text = "use axoupdater::AxoUpdater;\nfn sneaky() { let _ = std::net::TcpStream::connect(\"h:1\"); }\n";
        let f = scan_file(UPDATER_PATH, text);
        assert_eq!(
            f.len(),
            2,
            "std::net:: + TcpStream must leak even in update.rs, got {f:?}"
        );
        assert!(f.iter().all(|x| x.symbol != "axoupdater"));
    }

    #[test]
    fn genuinely_self_update_gated_symbol_is_exempt_anywhere() {
        // In ANY file, a network symbol inside a real `#[cfg(feature = "self-update")]` region is exempt.
        let text = "\
#[cfg(feature = \"self-update\")]
fn updater() {
    let _ = axoupdater::AxoUpdater::new_for(\"x\");
}
";
        let f = scan_file("crates/unblock-x/src/lib.rs", text);
        assert!(
            f.is_empty(),
            "a genuinely self-update-gated symbol is exempt, got {f:?}"
        );
    }

    #[test]
    fn scanner_self_source_is_exempt() {
        let text = "const S: &[&str] = &[\"reqwest\", \"std::net::\", \"axoupdater\"];\n";
        let f = scan_file("xtask/src/no_network.rs", text);
        assert!(
            f.is_empty(),
            "the scanner's own source is exempt (names symbols as patterns), got {f:?}"
        );
    }

    // ---- `is_cfg_gated_region` lexer hardening (H1/H2/H3) — line-index targets. ----

    // H1 — a gated UN-BRACED item does NOT leak its gate onto the next block.
    #[test]
    fn h1_gated_use_does_not_leak_to_next_block() {
        let text = "\
#[cfg(feature = \"self-update\")]
use reqwest::Client;
fn leaked() {
    let _ = std::net::TcpStream::connect(\"x\");
}
";
        // line 0 gate, 1 use (gated item), 2 fn, 3 the target socket (must be UN-gated → a leak).
        assert!(
            is_cfg_gated_region(text, 1),
            "the gated `use` line itself is gated"
        );
        assert!(
            !is_cfg_gated_region(text, 3),
            "the socket in the NEXT fn must NOT be gated (H1)"
        );
    }

    // H1 — a gated field inside a struct block does NOT leak past the struct's close.
    #[test]
    fn h1_gated_field_does_not_leak_past_struct_close() {
        let text = "\
struct S {
    #[cfg(feature = \"self-update\")]
    updater: reqwest::Client,
}
fn leaked() {
    let _ = std::net::TcpStream::connect(\"x\");
}
";
        assert!(
            !is_cfg_gated_region(text, 5),
            "the socket after the struct must NOT be gated (H1)"
        );
    }

    // H1 — a genuine MULTI-LINE gated fn signature stays gated (no false-RED regression from the H1 fix).
    #[test]
    fn h1_multiline_gated_signature_stays_gated() {
        let text = "\
#[cfg(feature = \"self-update\")]
pub fn updater(
    arg: u8,
) -> u8 {
    let _ = reqwest::Client::new();
    arg
}
";
        assert!(
            is_cfg_gated_region(text, 4),
            "a gated fn with a multi-line signature stays gated"
        );
    }

    // D1 — a sibling field after a gated field does NOT inherit the gate (H1 comma-sibling closed).
    #[test]
    fn h1_gated_field_does_not_leak_to_sibling_field() {
        let text = "\
struct S {
    #[cfg(feature = \"self-update\")]
    a: u8,
    b: reqwest::Client,
}
";
        assert!(
            !is_cfg_gated_region(text, 3),
            "a sibling field after a gated field must NOT be gated (H1 comma-sibling)"
        );
    }

    // D1 regression guard — a gated GENERIC fn signature stays gated (the angle guard protects it).
    #[test]
    fn h1_gated_generic_signature_stays_gated() {
        let text = "\
#[cfg(feature = \"self-update\")]
fn f<T: Trait, U>() {
    let _ = reqwest::Client::new();
}
";
        assert!(
            is_cfg_gated_region(text, 2),
            "a gated generic fn signature stays gated (no comma-disarm regression)"
        );
    }

    // W1 — a gated fn with a multi-line where-clause stays gated (the bound commas are suppressed).
    #[test]
    fn h1_gated_where_clause_signature_stays_gated() {
        let text = "\
#[cfg(feature = \"self-update\")]
fn f<T, U>() -> T
where
    T: Default,
    U: Clone,
{
    let _ = reqwest::Client::new();
    T::default()
}
";
        assert!(
            is_cfg_gated_region(text, 6),
            "a gated fn with a multi-line where-clause stays gated (W1, no false-RED)"
        );
    }

    // KNOWN, DOCUMENTED RESIDUAL (not a bug to fix without a real parser): a tuple-struct paren-field
    // sibling under-flags. Pinned so any future change to this behavior is surfaced, not silent. See the
    // `is_cfg_gated_region` doc. Backstopped by the --no-default-features feature-matrix build.
    #[test]
    fn known_residual_tuple_struct_field_under_flags() {
        let text = "\
struct S(
    #[cfg(feature = \"self-update\")]
    A,
    reqwest::Client,
);
";
        assert!(
            is_cfg_gated_region(text, 3),
            "documents the known tuple-struct paren-field under-flag (lightweight-heuristic limit)"
        );
    }

    // H2 — two byte-identical symbol lines (one gated, one not) are told apart by index.
    #[test]
    fn h2_identical_lines_are_distinguished_by_index() {
        // line 2 sits inside a `self-update`-gated fn, line 5 in an un-gated sibling; the two symbol
        // lines are BYTE-IDENTICAL. A text-keyed scan returns at the FIRST match and misclassifies
        // line 5 as gated; an INDEX-keyed scan tells the byte-identical twins apart (H2).
        let text = "\
#[cfg(feature = \"self-update\")]
fn gated() {
    let _ = reqwest::Client::new();
}
fn leaked() {
    let _ = reqwest::Client::new();
}
";
        assert!(
            is_cfg_gated_region(text, 2),
            "the gated identical line is gated"
        );
        assert!(
            !is_cfg_gated_region(text, 5),
            "the un-gated byte-identical twin is NOT gated (H2)"
        );
    }

    // H3 — a stray `{` inside a string in a gated block does not leak the gate past the block.
    #[test]
    fn h3_brace_in_string_does_not_miscount() {
        let text = "\
#[cfg(feature = \"self-update\")]
fn ok() { let _ = \"{\"; }
fn leaked() {
    let _ = std::net::TcpStream::connect(\"x\");
}
";
        assert!(
            !is_cfg_gated_region(text, 3),
            "a `{{` in a string must not inflate depth (H3)"
        );
    }

    // H3 — a `{` inside a char literal (unbalanced) must not inflate depth.
    #[test]
    fn h3_brace_in_char_literal_does_not_miscount() {
        let text = "\
#[cfg(feature = \"self-update\")]
fn ok() { let _ = '{'; }
fn leaked() {
    let _ = std::net::TcpStream::connect(\"x\");
}
";
        assert!(
            !is_cfg_gated_region(text, 3),
            "a `{{` in a char literal must not inflate depth (H3)"
        );
    }

    // H3 — a `{` inside a line comment WITHIN the gated block (unbalanced) must not inflate depth.
    #[test]
    fn h3_brace_in_line_comment_does_not_miscount() {
        let text = "\
#[cfg(feature = \"self-update\")]
fn ok() {
    let _ = 5; // {
}
fn leaked() {
    let _ = std::net::TcpStream::connect(\"x\");
}
";
        assert!(
            !is_cfg_gated_region(text, 5),
            "a `{{` in a line comment must not inflate depth (H3)"
        );
    }

    // ---- `mod update;` gate integrity (the symbol-scoped exemption may not hide a de-gated path). ----

    #[test]
    fn gated_mod_update_passes() {
        let text = "pub mod version;\n\n#[cfg(feature = \"self-update\")]\npub mod update;\n";
        assert!(assert_update_mod_gated(text).is_ok());
    }

    #[test]
    fn ungated_mod_update_fails() {
        // If someone de-gates `mod update;`, the `axoupdater`-symbol exemption would hide a live network
        // path — the assertion MUST fail (exemption-integrity non-vacuity).
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
