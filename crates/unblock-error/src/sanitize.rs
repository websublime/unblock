//! Terminal-control sanitization for human-readable error messages (spine §2.4; NFR-14).
//!
//! This crate ships **only** the `\n`/`\t`-preserving variant ([`sanitize_message`], ported from
//! the original `format/text.rs::sanitize_terminal_text`). The stricter inline variant that also
//! escapes `\n`/`\t` (`sanitize_terminal_inline`) belongs to `unblock-render` for single-line
//! display fields — see the render crate plan. The split is intentional: do not collapse them.

use std::borrow::Cow;

/// Escape terminal control characters in an error message, preserving layout.
///
/// Ordinary printable text is preserved verbatim; `\n` and `\t` are kept (multi-line messages
/// stay readable); every other control character (CR, BS, ESC, BEL, DEL, the C1 block, …) is
/// rendered as a visible Rust-style escape (e.g. `\u{1b}`) via [`char::escape_default`]. This is
/// the single sanitizer every [`crate::StructuredError`] constructor routes its message through
/// (spine §2.4 chokepoint), so a composed error carrying raw control bytes can never reach a
/// terminal unescaped.
///
/// Returns [`Cow::Borrowed`] with zero allocation when the input needs no escaping.
///
/// # Examples
///
/// ```
/// use std::borrow::Cow;
/// use unblock_error::sanitize_message;
///
/// // A clean string is borrowed, not copied.
/// assert!(matches!(sanitize_message("all good\nsecond line"), Cow::Borrowed(_)));
///
/// // ESC is escaped; the newline survives.
/// let out = sanitize_message("danger\x1b[2K\nok");
/// assert!(out.contains("\\u{1b}[2K"));
/// assert!(out.contains('\n'));
/// ```
#[must_use]
pub fn sanitize_message(text: &str) -> Cow<'_, str> {
    let mut escaped = String::new();
    let mut changed = false;

    for (idx, ch) in text.char_indices() {
        let allowed_layout = matches!(ch, '\n' | '\t');
        if allowed_layout || !ch.is_control() {
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
    use super::sanitize_message;
    use std::borrow::Cow;

    #[test]
    fn preserves_layout_but_escapes_controls() {
        let out = sanitize_message("line one\n\tline two\x1b]52;c;bad\x07\r");
        assert!(out.contains("line one\n\tline two"));
        assert!(out.contains("\\u{1b}]52"));
        assert!(out.contains("\\u{7}"));
        assert!(out.contains("\\r"));
        assert!(!out.contains('\x1b'));
        assert!(!out.contains('\x07'));
        assert!(!out.contains('\r'));
    }

    #[test]
    fn clean_string_borrows_without_alloc() {
        let out = sanitize_message("plain ascii\nwith tab\tand newline");
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    #[test]
    fn empty_string_borrows() {
        let out = sanitize_message("");
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out, "");
    }

    #[test]
    fn escapes_cr_bs_esc_bel_del_and_c1() {
        let out = sanitize_message("a\rb\x08c\x1bd\x07e\x7ff\u{9b}g");
        assert!(
            !out.chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
        );
        assert!(out.contains("\\r"));
        assert!(out.contains("\\u{8}"));
        assert!(out.contains("\\u{1b}"));
        assert!(out.contains("\\u{7}"));
        assert!(out.contains("\\u{7f}"));
        assert!(out.contains("\\u{9b}"));
    }

    #[test]
    fn idempotent_on_dirty_input() {
        let once = sanitize_message("x\x1by").into_owned();
        let twice = sanitize_message(&once).into_owned();
        assert_eq!(once, twice);
    }

    #[test]
    fn four_byte_emoji_survives_untouched() {
        // A 4-byte UTF-8 emoji is not a control char; it must pass through and borrow.
        let out = sanitize_message("\u{1f980}");
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out, "\u{1f980}");
    }
}
