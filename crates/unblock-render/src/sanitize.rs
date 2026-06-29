//! Terminal-control sanitization for untrusted strings rendered in human/text formats (NFR-18).
//!
//! Two variants, split by layout-control policy (the split is intentional — do **not** collapse
//! them):
//!
//! - [`sanitize_inline`] escapes **all** control characters, including `\n` and `\t`, for
//!   single-line display fields (titles, labels, assignees, embedded ids) and for
//!   `StructuredError` context values (spine §2.4:503). It is a verbatim behavioural port of the
//!   original `format/text.rs::sanitize_terminal_inline`.
//! - [`sanitize_text`] preserves `\n`/`\t` (multi-line descriptions/notes stay readable) and
//!   escapes the rest. It is **re-exported** from `unblock_error::sanitize_message` rather than
//!   copied, so the layout-preserving sanitizer has exactly one definition in the workspace
//!   (CLAUDE.md "defined once, re-exported, never redefined").

use std::borrow::Cow;

/// The layout-preserving sanitizer (`\n`/`\t` kept, everything else escaped).
///
/// Re-exported from `unblock-error` — this crate does **not** define a second copy. Use it for
/// multi-line content (descriptions, notes, comment bodies) where line structure must survive.
pub use unblock_error::sanitize_message as sanitize_text;

/// Escape **all** terminal control characters, including `\n` and `\t`, for single-line display.
///
/// Ordinary printable text passes through verbatim; every control character (CR, BS, ESC, BEL,
/// DEL, the C1 block, **and** `\n`/`\t`) is rendered as a visible Rust-style escape (e.g.
/// `\u{1b}`) via [`char::escape_default`]. Use this for any value that must stay on one line:
/// titles, labels, assignees, owners, embedded issue ids, open-enum `Custom` labels, and every
/// `StructuredError` context value.
///
/// Returns [`Cow::Borrowed`] with zero allocation when the input needs no escaping.
///
/// # Examples
///
/// ```
/// use std::borrow::Cow;
/// use unblock_render::sanitize_inline;
///
/// // A clean single-line string is borrowed, not copied.
/// assert!(matches!(sanitize_inline("all good"), Cow::Borrowed(_)));
///
/// // ESC is escaped — and so is the newline (unlike `sanitize_text`).
/// let out = sanitize_inline("danger\x1b[2K\nmore");
/// assert!(out.contains("\\u{1b}[2K"));
/// assert!(out.contains("\\n"));
/// assert!(!out.contains('\n'));
/// ```
#[must_use]
pub fn sanitize_inline(text: &str) -> Cow<'_, str> {
    let mut escaped = String::new();
    let mut changed = false;

    for (idx, ch) in text.char_indices() {
        // The inline variant allows NO layout exception: `\n`/`\t` are escaped like any control.
        if !ch.is_control() {
            if changed {
                escaped.push(ch);
            }
            continue;
        }

        if !changed {
            escaped.reserve(text.len());
            escaped.push_str(&text[..idx]);
            changed = true;
        }

        for escaped_char in ch.escape_default() {
            escaped.push(escaped_char);
        }
    }

    if changed {
        Cow::Owned(escaped)
    } else {
        Cow::Borrowed(text)
    }
}

#[cfg(test)]
mod tests {
    use super::{sanitize_inline, sanitize_text};
    use std::borrow::Cow;

    #[test]
    fn inline_escapes_newline_and_tab() {
        let out = sanitize_inline("a\nb\tc");
        assert!(out.contains("\\n"));
        assert!(out.contains("\\t"));
        assert!(!out.contains('\n'));
        assert!(!out.contains('\t'));
    }

    #[test]
    fn inline_escapes_esc_bel_del_and_c1() {
        let out = sanitize_inline("a\x1bb\x07c\x7fd\u{9b}e");
        assert!(out.contains("\\u{1b}"));
        assert!(out.contains("\\u{7}"));
        assert!(out.contains("\\u{7f}"));
        assert!(out.contains("\\u{9b}"));
        assert!(
            !out.chars().any(char::is_control),
            "no raw control byte may survive inline sanitization"
        );
    }

    #[test]
    fn inline_borrows_plain_ascii() {
        let out = sanitize_inline("plain ascii with spaces");
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    #[test]
    fn inline_borrows_empty() {
        assert!(matches!(sanitize_inline(""), Cow::Borrowed(_)));
    }

    #[test]
    fn inline_idempotent_on_dirty_input() {
        let once = sanitize_inline("x\x1b\ny").into_owned();
        let twice = sanitize_inline(&once).into_owned();
        assert_eq!(once, twice);
    }

    #[test]
    fn text_reexport_preserves_layout() {
        // `sanitize_text` is the re-exported `sanitize_message`: it KEEPS `\n`/`\t`.
        let out = sanitize_text("line one\n\tline two\x1bgone");
        assert!(out.contains("line one\n\tline two"));
        assert!(out.contains("\\u{1b}"));
        assert!(!out.contains('\x1b'));
    }

    #[test]
    fn inline_and_text_differ_on_newline() {
        let input = "a\nb";
        assert!(sanitize_inline(input).contains("\\n"));
        assert!(sanitize_text(input).contains('\n'));
    }
}
