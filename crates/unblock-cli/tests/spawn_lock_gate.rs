//! ub-vcp static gate: every child process and every `std::io::pipe()` in this crate's integration
//! tests goes through the ONE process-wide `SPAWN_LOCK` in `tests/common/mod.rs` (see that module's
//! doc for the macOS `pipe()`→`FIOCLEX` race it closes).
//!
//! A source scan of every `*.rs` under `$CARGO_MANIFEST_DIR/tests` (recursive, sorted), run on the
//! CODE text the shared `source_scan::Lexer` leaves after blanking comments, strings and char
//! literals. A line is a finding when it holds:
//! 1. `.spawn()` with an empty argument list;
//! 2. `.output()` (which also covers `.output().await`);
//! 3. `.status()`;
//! 4. `.assert()`;
//! 5. `Command::spawn`, `Command::output` or `Command::status` (UFCS calls and fn-pointer uses such
//!    as `.map(Command::output)`);
//! 6. any `assert_cmd` path other than `assert_cmd::cargo::CommandCargoExt` and
//!    `assert_cmd::cargo::cargo_bin` (a strict allowlist: `OutputOkExt`/`OutputAssertExt` are
//!    reachable only by naming `assert_cmd::…`, so no `.ok()`/`.unwrap()` text rule is needed);
//! 7. `pipe(` after a word boundary, or the path `io::pipe`.
//!
//! The two sanctioned lines in `tests/common/mod.rs` carry a `// SPAWN_LOCK: sanctioned` line
//! comment. A marked line suppresses findings 1 and 7 only, and only in `tests/common/mod.rs`; a
//! marker anywhere else is itself a finding. `common/mod.rs` must hold exactly two marked lines:
//! one whose code is exactly `cmd.spawn()` and one whose code holds `io::pipe()` and no other
//! finding, each directly under `let _guard = spawn_guard();`.
//!
//! # Known blind spots
//! - Dispatch hidden behind a macro, or in a non-test helper crate.
//! - `use … as` aliasing of std items (`use std::process::Command as C;` + `C::output(&mut c)`, or
//!   `use std::io::{pipe as mk};` + `mk()`). The `.output()` method form is still caught.
//! - A command whose stdio was set in an earlier statement and is then passed to `output_locked`.
//!   Review and the `output_locked` doc enforce that precondition; the scan cannot.
//! - Third-party forks inside dependencies. For insta, `.cargo/config.toml`'s
//!   `INSTA_WORKSPACE_ROOT` closes this.
//! - Over-flag direction: a future non-process `.status()`/`.output()` method (e.g. an HTTP
//!   response) goes red and needs a rename or a wrapper. That is the safe direction.
//! - A call whose `(` and `)` are split across lines (`.spawn(\n)`): the scan is per line. rustfmt
//!   never formats an empty argument list that way, and CI runs `cargo fmt --check`.

mod source_scan;

use std::path::{Path, PathBuf};

use source_scan::Lexer;

/// The harness file that owns the lock and the two sanctioned lines (relative to `tests/`).
const COMMON: &str = "common/mod.rs";
/// The exact (trimmed) line-comment text that marks a sanctioned line.
const MARKER: &str = "SPAWN_LOCK: sanctioned";
/// The exact (trimmed) code line that must directly precede each sanctioned line.
const GUARD_LINE: &str = "let _guard = spawn_guard();";
/// The exact (trimmed) code of the sanctioned spawn.
const SANCTIONED_SPAWN: &str = "cmd.spawn()";

/// The two `assert_cmd` paths the corpus may name (each followed by a word boundary).
const ASSERT_CMD_ALLOWED: &[&str] = &["::cargo::CommandCargoExt", "::cargo::cargo_bin"];

/// One finding rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rule {
    /// 1. `.spawn()`.
    Spawn,
    /// 2. `.output()`.
    Output,
    /// 3. `.status()`.
    Status,
    /// 4. `.assert()`.
    Assert,
    /// 5. `Command::{spawn,output,status}`.
    CommandPath,
    /// 6. an `assert_cmd` path outside the allowlist.
    AssertCmd,
    /// 7. `pipe(` / `io::pipe`.
    Pipe,
}

impl Rule {
    /// Whether a sanctioned marker suppresses this rule (findings 1 and 7 only).
    fn sanctionable(self) -> bool {
        matches!(self, Self::Spawn | Self::Pipe)
    }

    fn describe(self) -> &'static str {
        match self {
            Self::Spawn => "bare `.spawn()` (use `common::spawn_locked`)",
            Self::Output => "bare `.output()` (use `common::output_locked`)",
            Self::Status => "bare `.status()` (use `common::status_locked`)",
            Self::Assert => "`.assert()` (assert_cmd forks; use `common::output_locked`)",
            Self::CommandPath => {
                "`Command::{spawn,output,status}` path (use the `common::*_locked` helpers)"
            }
            Self::AssertCmd => {
                "an `assert_cmd` path other than `cargo::CommandCargoExt`/`cargo::cargo_bin`"
            }
            Self::Pipe => "`io::pipe` (use `common::pipe_locked`)",
        }
    }
}

/// A line carrying the sanctioned marker.
#[derive(Debug)]
struct Marked {
    /// 1-based line number.
    line: usize,
    /// The line's code text.
    code: String,
    /// The previous non-empty code line, trimmed.
    prev_code: Option<String>,
}

/// The result of scanning one file.
#[derive(Debug, Default)]
struct Scan {
    /// `(1-based line, message)` for every finding.
    findings: Vec<(usize, String)>,
    /// The marked lines (only collected in `common/mod.rs`).
    marked: Vec<Marked>,
}

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Whether `code[at..]` starts a word, i.e. the byte before `at` is not an identifier byte.
fn boundary_before(code: &[u8], at: usize) -> bool {
    at == 0 || !is_ident(code[at - 1])
}

/// Whether the byte at `at` (end of a match) is not an identifier byte.
fn boundary_after(code: &[u8], at: usize) -> bool {
    code.get(at).is_none_or(|&c| !is_ident(c))
}

/// Every start index of `needle` in `hay`.
fn occurrences<'a>(hay: &'a str, needle: &'a str) -> impl Iterator<Item = usize> {
    hay.match_indices(needle).map(|(i, _)| i)
}

/// Whether `.name` followed by an empty argument list (`()`, whitespace allowed) occurs in `code`.
fn empty_call(code: &str, name: &str) -> bool {
    let b = code.as_bytes();
    let needle = format!(".{name}");
    occurrences(code, &needle).any(|i| {
        let mut j = i + needle.len();
        while b.get(j).is_some_and(u8::is_ascii_whitespace) {
            j += 1;
        }
        if b.get(j) != Some(&b'(') {
            return false;
        }
        j += 1;
        while b.get(j).is_some_and(u8::is_ascii_whitespace) {
            j += 1;
        }
        b.get(j) == Some(&b')')
    })
}

/// Every rule `code` (one scanned line) violates, in rule order.
fn line_findings(code: &str) -> Vec<Rule> {
    let b = code.as_bytes();
    let mut rules = Vec::new();
    for (rule, name) in [
        (Rule::Spawn, "spawn"),
        (Rule::Output, "output"),
        (Rule::Status, "status"),
        (Rule::Assert, "assert"),
    ] {
        if empty_call(code, name) {
            rules.push(rule);
        }
    }
    if ["Command::spawn", "Command::output", "Command::status"]
        .iter()
        .any(|p| {
            occurrences(code, p).any(|i| boundary_before(b, i) && boundary_after(b, i + p.len()))
        })
    {
        rules.push(Rule::CommandPath);
    }
    let assert_cmd_bad = occurrences(code, "assert_cmd").any(|i| {
        let end = i + "assert_cmd".len();
        if !boundary_before(b, i) || !boundary_after(b, end) {
            return false; // a longer identifier, not the crate
        }
        let rest = &code[end..];
        !ASSERT_CMD_ALLOWED.iter().any(|allowed| {
            rest.starts_with(allowed)
                && boundary_after(b, end + allowed.len())
                && b.get(end + allowed.len()) != Some(&b'!') // a macro is never the allowed item
        })
    });
    if assert_cmd_bad {
        rules.push(Rule::AssertCmd);
    }
    let pipe_call = occurrences(code, "pipe").any(|i| {
        if !boundary_before(b, i) {
            return false;
        }
        let mut j = i + "pipe".len();
        while b.get(j).is_some_and(u8::is_ascii_whitespace) {
            j += 1;
        }
        b.get(j) == Some(&b'(')
    });
    let pipe_path = occurrences(code, "io::pipe")
        .any(|i| boundary_before(b, i) && boundary_after(b, i + "io::pipe".len()));
    if pipe_call || pipe_path {
        rules.push(Rule::Pipe);
    }
    rules
}

/// Scan one file. `rel` is the path relative to `tests/` with `/` separators.
fn scan(rel: &str, text: &str) -> Scan {
    let is_common = rel == COMMON;
    let mut out = Scan::default();
    let mut lexer = Lexer::new();
    let mut prev_code: Option<String> = None;
    for (idx, line) in text.lines().enumerate() {
        let line_number = idx + 1;
        let scanned = lexer.line(line);
        let marked = scanned
            .line_comment
            .as_deref()
            .is_some_and(|c| c.trim() == MARKER);
        let mut rules = line_findings(&scanned.code);
        if marked {
            if is_common {
                rules.retain(|r| !r.sanctionable());
                out.marked.push(Marked {
                    line: line_number,
                    code: scanned.code.clone(),
                    prev_code: prev_code.clone(),
                });
            } else {
                out.findings.push((
                    line_number,
                    format!("`// {MARKER}` outside `tests/{COMMON}`"),
                ));
            }
        }
        for rule in rules {
            out.findings
                .push((line_number, rule.describe().to_string()));
        }
        let trimmed = scanned.code.trim();
        if !trimmed.is_empty() {
            prev_code = Some(trimmed.to_string());
        }
    }
    if let Err(err) = lexer.finish() {
        out.findings
            .push((text.lines().count(), format!("lexer desync: {err}")));
    }
    out
}

/// Check `common/mod.rs`'s marked lines: exactly two, one `cmd.spawn()` and one `io::pipe()`, each
/// directly under the guard line. Returns one named message per violation.
fn check_sanctioned(marked: &[Marked]) -> Vec<String> {
    let mut errors = Vec::new();
    if marked.len() != 2 {
        let lines: Vec<usize> = marked.iter().map(|m| m.line).collect();
        errors.push(format!(
            "expected exactly 2 `// {MARKER}` lines in tests/{COMMON}, found {} (lines {lines:?})",
            marked.len()
        ));
    }
    let spawns = marked
        .iter()
        .filter(|m| m.code.trim() == SANCTIONED_SPAWN)
        .count();
    let pipes = marked
        .iter()
        .filter(|m| {
            m.code.contains("io::pipe()") && line_findings(&m.code).iter().all(|&r| r == Rule::Pipe)
        })
        .count();
    if spawns != 1 {
        errors.push(format!(
            "expected exactly 1 sanctioned line whose code is `{SANCTIONED_SPAWN}`, found {spawns}"
        ));
    }
    if pipes != 1 {
        errors.push(format!(
            "expected exactly 1 sanctioned line whose code holds `io::pipe()` and no other finding, \
             found {pipes}"
        ));
    }
    for m in marked {
        let is_spawn = m.code.trim() == SANCTIONED_SPAWN;
        let is_pipe = m.code.contains("io::pipe()")
            && line_findings(&m.code).iter().all(|&r| r == Rule::Pipe);
        if !is_spawn && !is_pipe {
            errors.push(format!(
                "tests/{COMMON}:{}: sanctioned line's code `{}` is neither `{SANCTIONED_SPAWN}` nor \
                 an `io::pipe()` call",
                m.line,
                m.code.trim()
            ));
        }
        if m.prev_code.as_deref() != Some(GUARD_LINE) {
            errors.push(format!(
                "tests/{COMMON}:{}: sanctioned line is not directly under `{GUARD_LINE}` (previous \
                 code line: {:?})",
                m.line, m.prev_code
            ));
        }
    }
    errors
}

/// The `tests/` root (from Cargo at compile time).
fn tests_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

/// Every `.rs` file under `dir` (recursive), sorted for deterministic reporting.
fn corpus(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).expect("read tests dir") {
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
fn every_spawn_and_pipe_goes_through_the_spawn_lock() {
    let root = tests_dir();
    let files = corpus(&root);
    let rels: Vec<String> = files
        .iter()
        .map(|f| {
            f.strip_prefix(&root)
                .expect("corpus file under tests/")
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect();
    assert!(
        rels.iter().any(|r| r == COMMON),
        "non-vacuity: the corpus under {} must contain tests/{COMMON}",
        root.display()
    );

    let mut errors = Vec::new();
    for (file, rel) in files.iter().zip(&rels) {
        let text = std::fs::read_to_string(file).expect("read test source");
        let result = scan(rel, &text);
        for (line, msg) in &result.findings {
            errors.push(format!("tests/{rel}:{line}: {msg}"));
        }
        if rel == COMMON {
            errors.extend(check_sanctioned(&result.marked));
        }
    }
    assert!(
        errors.is_empty(),
        "ub-vcp SPAWN_LOCK gate: every child process and every `io::pipe()` must go through \
         `tests/common/mod.rs`'s `*_locked` helpers:\n  {}",
        errors.join("\n  ")
    );
}

#[cfg(test)]
mod spawn_lock_gate_self_tests {
    use super::{COMMON, check_sanctioned, scan};

    /// The 1-based lines with a finding when `src` is scanned as a non-common test file.
    fn flagged(src: &str) -> Vec<usize> {
        scan("x.rs", src).findings.iter().map(|&(l, _)| l).collect()
    }

    fn assert_flags(src: &str) {
        assert!(!flagged(src).is_empty(), "must be flagged:\n{src}");
    }

    fn assert_clean(src: &str) {
        let found = scan("x.rs", src).findings;
        assert!(
            found.is_empty(),
            "must NOT be flagged:\n{src}\nfindings: {found:?}"
        );
    }

    // ---- flagged ----

    #[test]
    fn a_multi_line_output_chain_is_flagged_at_the_dispatch_line() {
        assert_eq!(
            flagged("let o = ws.cmd()\n    .arg(\"x\")\n    .output()\n    .expect(\"x\");"),
            vec![3]
        );
    }

    #[test]
    fn spawn_status_assert_are_flagged() {
        assert_flags("let c = cmd.spawn().unwrap();");
        assert_flags("let s = cmd.status();");
        assert_flags("cmd.assert().success();");
        assert_flags("let c = cmd.spawn ( );");
        assert_flags("let o = cmd.output().await;");
    }

    #[test]
    fn io_pipe_is_flagged() {
        assert_flags("let (r, w) = std::io::pipe().unwrap();");
        assert_flags("use std::io::pipe;");
        assert_flags("let fds = libc::pipe(p);");
    }

    #[test]
    fn command_paths_are_flagged() {
        assert_flags("let o = Command::output(&mut c);");
        assert_flags("let o = cmds.iter_mut().map(Command::output);");
        assert_flags("let s = std::process::Command::status(&mut c);");
    }

    #[test]
    fn assert_cmd_paths_outside_the_allowlist_are_flagged() {
        assert_flags("use assert_cmd::prelude::*;");
        assert_flags("use assert_cmd::{cargo::CommandCargoExt, output::OutputOkExt};");
        assert_flags("use assert_cmd::{cargo::CommandCargoExt};");
        assert_flags("use assert_cmd::cargo::*;");
        assert_flags("use assert_cmd::Command;");
        assert_flags("use assert_cmd as ac;");
        assert_flags("extern crate assert_cmd;");
        assert_flags("let c = assert_cmd::cargo_bin_cmd!(\"unblock\");");
        assert_flags("let p = assert_cmd::cargo_bin!(\"unblock\");");
        assert_flags("let p = assert_cmd::cargo::cargo_bin!(\"unblock\");");
    }

    #[test]
    fn an_output_ok_ext_import_is_flagged_on_the_import_line() {
        assert_eq!(
            flagged("use assert_cmd::output::OutputOkExt;\nlet out = ws.cmd().unwrap();"),
            vec![1]
        );
    }

    #[test]
    fn the_marker_outside_common_is_flagged() {
        for src in [
            "fn f() {\n    let _guard = spawn_guard();\n    cmd.spawn() // SPAWN_LOCK: sanctioned\n}",
            "let n = 1; // SPAWN_LOCK: sanctioned",
        ] {
            let findings = scan("x.rs", src).findings;
            assert!(
                findings
                    .iter()
                    .any(|(_, m)| m.contains("outside `tests/common/mod.rs`")),
                "the marker itself must be a finding outside common:\n{src}\nfindings: {findings:?}"
            );
        }
    }

    // ---- lexer edge fixtures: each must STILL flag the forbidden call ----

    #[test]
    fn a_lifetime_does_not_hide_a_call() {
        assert_eq!(
            flagged("fn f<'a>(c: &'a mut Command) -> Output { c.output() }"),
            vec![1]
        );
    }

    #[test]
    fn an_escaped_quote_char_does_not_hide_a_call() {
        assert_eq!(flagged("let q = '\\''; c.output()"), vec![1]);
    }

    #[test]
    fn a_continued_string_does_not_hide_the_call_after_its_close() {
        assert_eq!(flagged("let s = \"a\\\nb\"; c.output()"), vec![2]);
    }

    #[test]
    fn a_hashless_raw_string_does_not_hide_a_call() {
        assert_eq!(flagged("let s = r\"x\"; c.output()"), vec![1]);
    }

    #[test]
    fn a_multi_line_raw_string_does_not_hide_the_call_after_its_close() {
        assert_eq!(flagged("let s = r#\"a\nb\"#; c.output()"), vec![2]);
    }

    #[test]
    fn a_byte_string_does_not_hide_a_call() {
        assert_eq!(flagged("let s = b\"x\"; c.status()"), vec![1]);
    }

    // ---- not flagged ----

    #[test]
    fn thread_and_runtime_spawns_are_not_dispatch() {
        assert_clean("let h = std::thread::spawn(move || {});");
        assert_clean(
            "let h = std::thread::Builder::new()\n    .name(\"drain\".into())\n    .spawn(move || {\n    });",
        );
        assert_clean("rt.spawn(fut);");
        assert_clean("let out = child.wait_with_output().expect(\"x\");");
        assert_clean("let code = out.status.code();");
    }

    #[test]
    fn pipe_lookalikes_are_not_flagged() {
        assert_clean("let (r, w) = pipe_locked();");
        assert_clean("if let Some(pipe) = child.stderr.take() {}");
    }

    #[test]
    fn tokens_in_comments_and_literals_are_not_flagged() {
        assert_clean("// cmd.output()");
        assert_clean("/// cmd.spawn()");
        assert_clean("/* cmd.status() */");
        assert_clean("/* a\ncmd.output()\n*/");
        assert_clean("let s = \"cmd.output() io::pipe()\";");
        assert_clean("let s = r#\"a\ncmd.output()\nuse assert_cmd::prelude::*;\n\"#;");
        assert_clean("let q = '\"'; let n = 1;");
    }

    #[test]
    fn the_allowed_assert_cmd_paths_are_not_flagged() {
        assert_clean("use assert_cmd::cargo::CommandCargoExt as _;");
        assert_clean("let p = assert_cmd::cargo::cargo_bin(\"unblock\");");
    }

    #[test]
    fn the_marker_inside_a_string_or_block_comment_is_not_a_marker() {
        assert_clean("let s = \"// SPAWN_LOCK: sanctioned\";");
        assert_clean("/* // SPAWN_LOCK: sanctioned */");
    }

    // ---- allowlist ----

    const SPAWN_OK: &str =
        "fn a() {\n    let _guard = spawn_guard();\n    cmd.spawn() // SPAWN_LOCK: sanctioned\n}\n";
    const PIPE_OK: &str = "fn b() {\n    let _guard = spawn_guard();\n    match io::pipe() { // SPAWN_LOCK: sanctioned\n        Ok(e) => e,\n        Err(e) => panic!(\"{e}\"),\n    }\n}\n";

    fn common_errors(src: &str) -> Vec<String> {
        let result = scan(COMMON, src);
        let mut errors: Vec<String> = result
            .findings
            .iter()
            .map(|(l, m)| format!("{l}: {m}"))
            .collect();
        errors.extend(check_sanctioned(&result.marked));
        errors
    }

    #[test]
    fn two_correct_marked_lines_pass() {
        let errors = common_errors(&format!("{SPAWN_OK}{PIPE_OK}"));
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn the_block_scoped_pipe_shape_passes() {
        // `pipe_locked`'s shape: the guard is scoped to the `pipe()` call alone.
        let pipe_scoped = "fn b() {\n    let ends = {\n        let _guard = spawn_guard();\n        io::pipe() // SPAWN_LOCK: sanctioned\n    };\n    match ends {\n        Ok(e) => e,\n        Err(e) => panic!(\"{e}\"),\n    }\n}\n";
        let errors = common_errors(&format!("{SPAWN_OK}{pipe_scoped}"));
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn one_marked_line_fails() {
        let errors = common_errors(SPAWN_OK);
        assert!(errors.iter().any(|e| e.contains("exactly 2")), "{errors:?}");
    }

    #[test]
    fn three_marked_lines_fail() {
        let errors = common_errors(&format!("{SPAWN_OK}{PIPE_OK}{SPAWN_OK}"));
        assert!(errors.iter().any(|e| e.contains("exactly 2")), "{errors:?}");
    }

    #[test]
    fn a_missing_guard_line_fails() {
        let unguarded = "fn a() {\n    cmd.spawn() // SPAWN_LOCK: sanctioned\n}\n";
        let errors = common_errors(&format!("{unguarded}{PIPE_OK}"));
        assert!(
            errors.iter().any(|e| e.contains("not directly under")),
            "{errors:?}"
        );
    }

    #[test]
    fn a_guard_moved_below_the_spawn_fails() {
        let moved = "fn a() {\n    cmd.spawn() // SPAWN_LOCK: sanctioned\n    let _guard = spawn_guard();\n}\n";
        let errors = common_errors(&format!("{moved}{PIPE_OK}"));
        assert!(
            errors.iter().any(|e| e.contains("not directly under")),
            "{errors:?}"
        );
    }

    #[test]
    fn a_marker_on_other_code_fails() {
        let other = "fn a() {\n    let _guard = spawn_guard();\n    cmd.output() // SPAWN_LOCK: sanctioned\n}\n";
        let errors = common_errors(&format!("{other}{PIPE_OK}"));
        assert!(errors.iter().any(|e| e.contains("neither")), "{errors:?}");
        assert!(
            errors.iter().any(|e| e.contains("bare `.output()`")),
            "{errors:?}"
        );
    }

    // ---- desync ----

    #[test]
    fn an_unterminated_string_at_eof_is_a_desync() {
        let result = scan("x.rs", "let s = \"abc;\nlet n = 1;");
        assert!(
            result
                .findings
                .iter()
                .any(|(_, m)| m.starts_with("lexer desync")),
            "{:?}",
            result.findings
        );
    }
}
