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
/// occurrence must be reachable ONLY when `self-update` is enabled (i.e. the vetted `axoupdater` symbol
/// in `commands/update.rs`, or on a `#[cfg(feature = "self-update")]`-guarded line). No un-gated network
/// symbol may leak. Kept IDENTICAL to the workspace-wide `xtask::no_network` scanner's 6-symbol list.
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

// Symbol-scoped confinement (defense-in-depth): `commands/update.rs` is NOT skipped as a whole file —
// only its vetted `axoupdater` symbol is exempt there; every OTHER network symbol (in ANY file) must be
// a comment or sit inside a `#[cfg(feature = "self-update")]`-gated region. The soundness of the
// `axoupdater`-in-`update.rs` exemption (that `mod update;` stays `self-update`-gated) is enforced by
// `xtask::no_network::assert_update_mod_gated` + the `--no-default-features` feature-matrix build; this
// crate-scoped tripwire does not re-assert the `mod` gate locally (parallel to its prior behavior).
#[test]
fn network_symbols_are_confined_behind_self_update() {
    let src = src_dir();
    for file in source_files(&src) {
        let text = std::fs::read_to_string(&file).expect("read source file");
        let is_update = file.file_name().is_some_and(|n| n == "update.rs");
        for symbol in NETWORK_SYMBOLS {
            if !text.contains(symbol) {
                continue;
            }
            for (idx, line) in text.lines().enumerate() {
                if line.contains(symbol) {
                    assert!(
                        line_is_confined(&text, line, idx, symbol, is_update),
                        "NFR-6/NFR-17: network symbol `{symbol}` appears un-confined in {}:\n  {line}\n\
                         it must be a comment, the vetted `axoupdater` symbol in the self-update-gated \
                         updater module, or inside a `#[cfg(feature = \"self-update\")]`-gated region",
                        file.display()
                    );
                }
            }
        }
    }
}

/// Whether a `line` (at index `idx`) containing network `symbol` (in file `is_update`) is a CONFINED
/// reference rather than a leak: a comment, the vetted `axoupdater` symbol in the `self-update`-gated
/// updater module, or a line inside a `#[cfg(feature = "self-update")]`-gated region. Anything else is a
/// confinement leak. Extracted so the confinement policy is unit-testable (the
/// `network_confinement_self_tests` below).
fn line_is_confined(text: &str, line: &str, idx: usize, symbol: &str, is_update: bool) -> bool {
    if line.trim_start().starts_with("//") {
        return true; // comments are inert
    }
    if is_update && symbol == "axoupdater" {
        return true; // update.rs may reference ONLY the vetted, module-gated axoupdater symbol
    }
    is_cfg_gated_region(text, idx)
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

/// Self-tests for the hardened confinement policy — proving `is_cfg_gated_region` (a) survives blank
/// lines and doc-comment preludes between a gate and its symbol AND (b) still catches a genuinely
/// un-gated network symbol (defense-in-depth non-vacuity: the scan must not be a rubber stamp), the
/// H1/H2/H3 lexer-hardening edge cases, and that `line_is_confined` closes both false-green escape
/// vectors while still exempting the legit self-update path. These are the tripwire's own regression pins.
#[cfg(test)]
mod network_confinement_self_tests {
    use super::{is_cfg_gated_region, line_is_confined};

    // ---- `is_cfg_gated_region` — blank-line survival + ungated-sibling + no-gate (line-index targets). ----

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
        // line 0 gate, 1 blank, 2 doc comment, 3 fn, 4 the target symbol.
        assert!(
            is_cfg_gated_region(text, 4),
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
        // line 2 the gated symbol, line 6 the un-gated sibling twin.
        assert!(
            is_cfg_gated_region(text, 2),
            "the symbol inside the gated fn is gated"
        );
        assert!(
            !is_cfg_gated_region(text, 6),
            "a symbol in an UN-gated sibling item must be seen as a leak (non-vacuous scan)"
        );
    }

    /// A file with NO gate never reports gated (the short-circuit) — a bare network symbol is a leak.
    #[test]
    fn no_gate_in_file_is_never_gated() {
        let text = "fn f() {\n    let x = std::net::TcpStream::connect(\"h:1\");\n}\n";
        // line 1 the target symbol.
        assert!(
            !is_cfg_gated_region(text, 1),
            "a network symbol with no gate anywhere is a leak"
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

    // ---- `line_is_confined` — the two closed escape vectors + the legit self-update path. ----

    // VECTOR 1: a std::net line with a self-update substring is NOT confined (no substring escape).
    #[test]
    fn self_update_substring_does_not_confine_a_raw_socket() {
        let text =
            "fn f() {\n    let _ = std::net::TcpStream::connect(\"self-update-host:1\");\n}\n";
        let line = "    let _ = std::net::TcpStream::connect(\"self-update-host:1\");";
        // the socket is on line index 1.
        assert!(!line_is_confined(text, line, 1, "std::net::", false));
        assert!(!line_is_confined(text, line, 1, "std::net::", true)); // not even in update.rs (not axoupdater)
    }

    // VECTOR 2: a raw socket in update.rs under a NON-self-update cfg is NOT confined (symbol-scoped).
    #[test]
    fn raw_socket_in_update_rs_is_not_confined() {
        let text = "\
#[cfg(feature = \"telemetry\")]
fn beacon() {
    let _ = std::net::TcpStream::connect(\"h:1\");
}
";
        let line = "    let _ = std::net::TcpStream::connect(\"h:1\");";
        // the socket is on line index 2.
        assert!(!line_is_confined(text, line, 2, "std::net::", true));
    }

    // The vetted axoupdater symbol in update.rs IS confined (module-gated, symbol-scoped).
    #[test]
    fn axoupdater_in_update_rs_is_confined() {
        let text = "use axoupdater::AxoUpdater;\n";
        let line = "use axoupdater::AxoUpdater;";
        assert!(line_is_confined(text, line, 0, "axoupdater", true));
        assert!(!line_is_confined(text, line, 0, "axoupdater", false)); // NOT exempt outside update.rs w/o a gate
    }

    // A genuinely self-update-gated symbol is confined in ANY file.
    #[test]
    fn self_update_gated_symbol_is_confined_anywhere() {
        let text = "\
#[cfg(feature = \"self-update\")]
fn updater() {
    let _ = reqwest::Client::new();
}
";
        let line = "    let _ = reqwest::Client::new();";
        // the symbol is on line index 2.
        assert!(line_is_confined(text, line, 2, "reqwest", false));
    }

    // R2 — a genuinely self-update-gated `axoupdater` line is confined too (symbol-specific, ANY file).
    #[test]
    fn self_update_gated_axoupdater_is_confined_anywhere() {
        let text = "\
#[cfg(feature = \"self-update\")]
fn updater() {
    let _ = axoupdater::AxoUpdater::new_for(\"x\");
}
";
        let line = "    let _ = axoupdater::AxoUpdater::new_for(\"x\");";
        // the symbol is on line index 2.
        assert!(line_is_confined(text, line, 2, "axoupdater", false));
    }
}
