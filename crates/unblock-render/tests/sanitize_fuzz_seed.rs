//! Seed corpus + proptest bridge for the sanitize + CSV-escape boundary (NFR-18).
//!
//! The `cargo-fuzz` targets `render_sanitize` and `render_csv_escape` live in the workspace
//! `unblock-fuzz` crate (PRD §8.1) and are wired in a later task; this proptest bridge exercises
//! the same invariants in CI without a nightly fuzz toolchain:
//! - `sanitize_inline` never panics and **never** emits a raw control byte (it escapes `\n`/`\t`
//!   too);
//! - `sanitize_text` never panics and never emits a raw control byte except the allowed `\n`/`\t`;
//! - both are idempotent on already-sanitized input;
//! - the csv `diagnostics` view writes one physical line and two cells per record, whatever the
//!   label and detail hold.

use proptest::prelude::*;
use unblock_model::{DiagnosticFinding, DiagnosticKind, DiagnosticReport, OutputFormat};
use unblock_render::{RenderOptions, renderer_for, sanitize_inline, sanitize_text};

/// A seed corpus of adversarial inputs that must always sanitize cleanly.
const SEEDS: &[&str] = &[
    "",
    "plain ascii",
    "\x1b[2J",            // ANSI clear screen
    "\x1b]52;c;evil\x07", // OSC 52 clipboard write + BEL
    "tab\there",
    "new\nline",
    "del\x7fchar",
    "c1\u{9b}control",
    "\u{1f980}", // 4-byte emoji
    "back\x08space",
    "carriage\rreturn",
];

#[test]
fn seed_corpus_inline_escapes_all_controls() {
    for &seed in SEEDS {
        let out = sanitize_inline(seed);
        assert!(
            !out.chars().any(char::is_control),
            "inline must escape every control byte; seed = {seed:?} -> {out:?}"
        );
    }
}

#[test]
fn seed_corpus_text_preserves_only_layout() {
    for &seed in SEEDS {
        let out = sanitize_text(seed);
        assert!(
            !out.chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\t')),
            "text may keep only \\n/\\t; seed = {seed:?} -> {out:?}"
        );
    }
}

proptest! {
    #[test]
    fn inline_never_emits_raw_control(input in ".*") {
        let out = sanitize_inline(&input);
        prop_assert!(!out.chars().any(char::is_control));
    }

    #[test]
    fn text_keeps_only_layout_controls(input in ".*") {
        let out = sanitize_text(&input);
        prop_assert!(
            !out.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
        );
    }

    #[test]
    fn inline_is_idempotent(input in ".*") {
        let once = sanitize_inline(&input).into_owned();
        let twice = sanitize_inline(&once).into_owned();
        prop_assert_eq!(once, twice);
    }

    #[test]
    fn text_is_idempotent(input in ".*") {
        let once = sanitize_text(&input).into_owned();
        let twice = sanitize_text(&once).into_owned();
        prop_assert_eq!(once, twice);
    }

    /// `(?s)` lets `.` yield `\n`, which a bare `.*` never does.
    #[test]
    fn csv_diagnostics_is_one_line_and_two_cells_per_record(
        rows in proptest::collection::vec(("(?s).{0,24}", "(?s).{0,24}"), 0..8)
    ) {
        let report = DiagnosticReport {
            kind: DiagnosticKind::Info,
            findings: rows
                .iter()
                .map(|(label, detail)| DiagnosticFinding {
                    label: label.clone(),
                    detail: detail.clone(),
                })
                .collect(),
        };
        let opts = RenderOptions::default();
        let out = renderer_for(OutputFormat::Csv, opts.clone())
            .diagnostics(&report, &opts)
            .expect("csv renders diagnostics");

        prop_assert_eq!(out.stdout.split('\n').count(), rows.len() + 1);
        prop_assert!(!out.stdout.chars().any(|c| c.is_control() && c != '\n'));

        let mut reader = csv::ReaderBuilder::new()
            .has_headers(true)
            .from_reader(out.stdout.as_bytes());
        let mut records = 0usize;
        for record in reader.records() {
            let record = record.expect("emitted CSV must be RFC-4180 well-formed");
            prop_assert_eq!(record.len(), 2);
            records += 1;
        }
        prop_assert_eq!(records, rows.len());
    }
}
