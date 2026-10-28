//! The comment/string-aware line lexer shared by the `unblock-cli` source-scan gates
//! (`no_git_gate.rs`, `spawn_lock_gate.rs`).
//!
//! [`Lexer::line`] returns one source line with every `//`/`/* */` comment, `"…"`/raw-string and
//! `'…'` char-literal content removed, so a following scan sees only real code. The text after a
//! code-level `//` is returned separately ([`Lexed::line_comment`]), so a gate can read a marker
//! comment without ever mistaking one inside a string or a block comment for it.
//!
//! The lexer is stateful across lines: `/* */` nesting AND an open normal or raw string carry over
//! to the next line, so the line that CLOSES a multi-line string is read from inside the string and
//! the code after its closing quote is scanned. [`Lexer::finish`] reports a literal or block comment
//! still open at end of file, so a phantom opener cannot silently blank the rest of a file.
//!
//! Lifetimes (`'a`) are emitted as harmless ticks. Non-ASCII bytes are dropped from the code text:
//! they are identifier or literal bytes, never structural punctuation.

#![allow(dead_code)] // each gate uses a subset of the lexer API.

/// A literal still open at the end of a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Open {
    /// A normal or byte string (`"…"`, `b"…"`).
    Str,
    /// A raw string (`r"…"`, `r#"…"#`, `br##"…"##`), closed by `"` and `hashes` `#`.
    Raw { hashes: usize },
}

/// One lexed line.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Lexed {
    /// The line's code text, with comment, string and char-literal content removed.
    pub code: String,
    /// The text after a code-level `//` (`None` if the line has none).
    pub line_comment: Option<String>,
}

/// The stateful line lexer: feed every line of a file in order, then call [`Lexer::finish`].
#[derive(Debug, Default)]
pub struct Lexer {
    /// `/* */` nesting carried across lines.
    block_comment: i32,
    /// A string literal left open at the end of the previous line.
    open: Option<Open>,
}

impl Lexer {
    /// A lexer at the start of a file.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the NEXT line starts inside a `/* */` block comment.
    #[must_use]
    pub fn in_block_comment(&self) -> bool {
        self.block_comment > 0
    }

    /// Lex one line (without its line terminator).
    pub fn line(&mut self, line: &str) -> Lexed {
        let b = line.as_bytes();
        let mut out = String::with_capacity(b.len());
        let mut line_comment = None;
        let mut i = match self.open {
            Some(Open::Str) => self.close_string(b, 0),
            Some(Open::Raw { hashes }) => self.close_raw(b, 0, hashes),
            None => 0,
        };
        while i < b.len() {
            if self.block_comment > 0 {
                if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
                    self.block_comment += 1;
                    i += 2;
                } else if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                    self.block_comment -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
                continue;
            }
            match b[i] {
                b'/' if b.get(i + 1) == Some(&b'/') => {
                    // `i + 2` is a char boundary: both `/` are ASCII.
                    line_comment = Some(line[i + 2..].to_string());
                    break;
                }
                b'/' if b.get(i + 1) == Some(&b'*') => {
                    self.block_comment += 1;
                    i += 2;
                }
                b'r' if starts_raw_string(b, i) => i = self.open_raw(b, i),
                b'b' if b.get(i + 1) == Some(&b'r') && starts_raw_string(b, i + 1) => {
                    i = self.open_raw(b, i + 1);
                }
                b'"' => i = self.close_string(b, i + 1),
                b'b' if b.get(i + 1) == Some(&b'"') => i = self.close_string(b, i + 2),
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
        Lexed {
            code: out,
            line_comment,
        }
    }

    /// End of file: `Err` names whatever is still open (an unterminated literal or block comment).
    ///
    /// # Errors
    /// A string, raw string or block comment opened and never closed.
    pub fn finish(self) -> Result<(), String> {
        match (self.open, self.block_comment) {
            (Some(Open::Str), _) => Err("unterminated string literal at end of file".to_string()),
            (Some(Open::Raw { hashes }), _) => Err(format!(
                "unterminated raw string literal (r{}\") at end of file",
                "#".repeat(hashes)
            )),
            (None, depth) if depth > 0 => Err(format!(
                "unterminated block comment (depth {depth}) at end of file"
            )),
            (None, _) => Ok(()),
        }
    }

    /// Scan string content from `j` for the closing `"`, honouring `\` escapes. Returns the index
    /// just past it, or `b.len()` with the string left open for the next line.
    fn close_string(&mut self, b: &[u8], mut j: usize) -> usize {
        while j < b.len() {
            match b[j] {
                b'\\' => j += 2,
                b'"' => {
                    self.open = None;
                    return j + 1;
                }
                _ => j += 1,
            }
        }
        self.open = Some(Open::Str);
        b.len()
    }

    /// Open a raw string at `b[i] == b'r'` and scan it; see [`Lexer::close_raw`].
    fn open_raw(&mut self, b: &[u8], i: usize) -> usize {
        let mut hashes: usize = 0;
        let mut j = i + 1;
        while j < b.len() && b[j] == b'#' {
            hashes += 1;
            j += 1;
        }
        self.close_raw(b, j + 1, hashes) // past the opening '"'
    }

    /// Scan raw-string content from `j` for `"` followed by `hashes` `#`. Returns the index just
    /// past it, or `b.len()` with the raw string left open for the next line.
    fn close_raw(&mut self, b: &[u8], mut j: usize, hashes: usize) -> usize {
        while j < b.len() {
            if b[j] == b'"' {
                let mut end = j + 1;
                let mut matched: usize = 0;
                while matched < hashes && end < b.len() && b[end] == b'#' {
                    matched += 1;
                    end += 1;
                }
                if matched == hashes {
                    self.open = None;
                    return end;
                }
            }
            j += 1;
        }
        self.open = Some(Open::Raw { hashes });
        b.len()
    }
}

/// Whether a raw-string prefix (`r`, `r#`, `r##`, …) begins at `b[i] == b'r'` (i.e. `r` then `#`*
/// then `"`).
fn starts_raw_string(b: &[u8], i: usize) -> bool {
    let mut j = i + 1;
    while j < b.len() && b[j] == b'#' {
        j += 1;
    }
    j < b.len() && b[j] == b'"'
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
