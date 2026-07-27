//! Terminal-control sanitization for human-readable error messages (spine §2.4; NFR-14), plus the
//! shared attacker-echo bound [`clip`] (D43).
//!
//! This crate ships **only** the `\n`/`\t`-preserving variant ([`sanitize_message`], ported from
//! the original `format/text.rs::sanitize_terminal_text`). The stricter inline variant that also
//! escapes `\n`/`\t` (`sanitize_terminal_inline`) belongs to `unblock-render` for single-line
//! display fields — see the render crate plan. The split is intentional: do not collapse them.
//!
//! [`clip`] is the SECOND boundary helper: it bounds how much attacker-controlled text an error
//! payload may echo. It lives here (D43) rather than in one consumer crate because BOTH untrusted-
//! JSON boundaries now echo attacker text — `unblock-mcp`'s argument seam and `unblock-sync`'s `bd`
//! line parser — and two copies of a security helper is exactly the drift this repo's rules forbid.

use std::borrow::Cow;

/// The maximum number of bytes of attacker-controlled text echoed back into an error payload.
///
/// **This is a SOFT bound.** [`sanitize_message`] runs *after* the clip and escapes control
/// characters at up to ~6 bytes each (`\x1b` → `\u{1b}`), so a final `message` is bounded at
/// roughly `6 * MAX_ECHOED_BYTES` ≈ 768 B, not 128 B. Clipping BEFORE sanitizing is deliberate:
/// clipping after could cut inside an escape sequence and yield a misleading fragment.
pub const MAX_ECHOED_BYTES: usize = 128;

/// The marker appended to clipped text.
pub const TRUNCATION_MARKER: &str = "…[truncated]";

/// Clip attacker-controlled text to [`MAX_ECHOED_BYTES`] on a char boundary.
///
/// Returns the input borrowed when it already fits, so the common path allocates nothing.
///
/// Call this **before** any [`crate::StructuredError`] builder that sanitizes, and **before**
/// [`crate::StructuredError::with_context`], which performs no sanitization of its own — the
/// caller is the only bound on a `context` value.
///
/// # Examples
///
/// ```
/// use unblock_error::{MAX_ECHOED_BYTES, TRUNCATION_MARKER, clip};
///
/// assert_eq!(clip("short"), "short");
///
/// let long = "x".repeat(4096);
/// let clipped = clip(&long);
/// assert!(clipped.ends_with(TRUNCATION_MARKER));
/// assert!(clipped.len() <= MAX_ECHOED_BYTES + TRUNCATION_MARKER.len());
/// ```
#[must_use]
pub fn clip(s: &str) -> Cow<'_, str> {
    if s.len() <= MAX_ECHOED_BYTES {
        return Cow::Borrowed(s);
    }
    let mut end = MAX_ECHOED_BYTES;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    Cow::Owned(format!("{}{TRUNCATION_MARKER}", &s[..end]))
}

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
    use super::{MAX_ECHOED_BYTES, TRUNCATION_MARKER, clip, sanitize_message};
    use std::borrow::Cow;

    #[test]
    fn clip_leaves_short_text_untouched() {
        assert_eq!(clip("short"), "short");
        assert!(matches!(clip("short"), Cow::Borrowed(_)));
    }

    #[test]
    fn clip_truncates_on_a_char_boundary() {
        let long = "é".repeat(500);
        let clipped = clip(&long);
        assert!(clipped.ends_with(TRUNCATION_MARKER));
        assert!(clipped.len() <= MAX_ECHOED_BYTES + TRUNCATION_MARKER.len());
        // Truncating mid-`é` would have produced invalid UTF-8 (a panic on the slice); reaching
        // here at all proves the boundary walk worked.
        let kept = clipped.strip_suffix(TRUNCATION_MARKER).expect("marker");
        assert!(
            kept.chars().all(|c| c == 'é'),
            "kept prefix must be whole chars"
        );
        assert_eq!(
            kept.len() % 2,
            0,
            "`é` is 2 bytes; a mid-char cut would be odd"
        );
    }

    #[test]
    fn clip_bounds_an_adversarial_payload() {
        let huge = "x".repeat(64 * 1024);
        let clipped = clip(&huge);
        assert!(clipped.len() <= MAX_ECHOED_BYTES + TRUNCATION_MARKER.len());
    }

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
