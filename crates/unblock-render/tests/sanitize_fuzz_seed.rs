//! Seed corpus + proptest bridge for the sanitize + CSV-escape boundary (NFR-18).
//!
//! The `cargo-fuzz` targets `render_sanitize` and `render_csv_escape` live in the workspace
//! `unblock-fuzz` crate (PRD §8.1) and are wired in a later task; this proptest bridge exercises
//! the same invariants in CI without a nightly fuzz toolchain:
//! - `sanitize_inline` never panics and **never** emits a raw control byte (it escapes `\n`/`\t`
//!   too);
//! - `sanitize_text` never panics and never emits a raw control byte except the allowed `\n`/`\t`;
//! - both are idempotent on already-sanitized input.

use proptest::prelude::*;
use unblock_render::{sanitize_inline, sanitize_text};

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
}
