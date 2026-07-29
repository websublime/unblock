//! Byte-level DUPLICATE JSON KEY detection (D43) — the shared scanner both untrusted-JSON
//! ingestion paths run **before** the bytes are handed to a `serde_json::Map`.
//!
//! # Why this exists
//!
//! `serde_json`'s map visitor collapses a duplicated object key **last-wins** while it builds the
//! `Map`. Any consumer that receives an already-parsed `Map`/`Value` therefore cannot observe the
//! collapse at all — the evidence is gone before the type exists. A frame whose text reads as one
//! action consequently *executes* a different one:
//!
//! ```text
//! {"name":"issue","arguments":{"action":"create","ids":["ub-a"],"action":"delete"}}
//! ```
//!
//! reads as a create and executes a delete. The only place the ambiguity is still visible is the
//! **raw bytes**, which is what this module scans.
//!
//! # Contract in one line
//!
//! [`scan`] answers "does the subtree at `at` contain a duplicated object key at ANY depth?" with a
//! three-state [`DupScan`] verdict, and **`Indeterminate` is never equivalent to `Clean`** — every
//! caller must treat a non-`Clean` verdict as a rejection (fail-closed).
//!
//! # Implementation note (normative — do NOT hand-roll a tokenizer)
//!
//! The scan is a [`serde::de::DeserializeSeed`] driven by `serde_json` itself. That is deliberate:
//! the *oracle* is `serde_json`'s own map visitor, so reusing its parser makes divergence in string
//! decoding, escape handling, surrogate pairs, BOM treatment and the recursion limit **structurally
//! impossible** rather than test-dependent. A raw-span (byte-offset) comparator is a **silent
//! bypass** — a `\u0061`-escaped key and a bare `a` key collapse in `serde_json` but have
//! byte-different spans — and every hand-written test still passes against it. Keys are therefore
//! compared **DECODED**.
//!
//! Per-object detection uses a pooled `HashSet` of decoded keys — O(k) per object with exact
//! string equality, so there is no hash-collision arm and no probabilistic verdict. The set's
//! element type ([`ObjectKey`]) counts the key comparisons its own `PartialEq` performs, which is
//! what makes the complexity-shape unit cell a real guard rather than a tautology (see
//! [`ScanStats`]).

use std::cell::Cell;
use std::collections::HashSet;
use std::fmt;
use std::hash::{Hash, Hasher};

use serde::de::{self, DeserializeSeed, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};

/// UTF-8 byte order mark. RFC 8259 §8.1 lets a parser ignore a leading BOM; rmcp's transport
/// strips exactly one, prefix-only, before parsing — so this scanner must strip exactly the same
/// one, or the scanner and the parser would see different documents.
const UTF8_BOM: &[u8; 3] = b"\xEF\xBB\xBF";

/// The `serde` error payload used to unwind out of the visitor on the FIRST duplicate.
///
/// It is never surfaced to a caller: [`scan`] inspects the recorded verdict, not the error text.
/// It exists only because a `serde` visitor's sole short-circuit channel is `Err`.
const SHORT_CIRCUIT: &str = "unblock: duplicate object key (short-circuit)";

/// The verdict of a [`scan`].
///
/// **`Indeterminate` is NEVER equivalent to `Clean`.** Both non-`Clean` variants must be treated as
/// a rejection by every caller (fail-closed, D43).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DupScan {
    /// No duplicated object key exists anywhere inside the scanned subtree.
    Clean,
    /// A duplicated object key was found.
    Duplicate {
        /// The **DECODED** duplicated key (escapes resolved, exactly as `serde_json` would build
        /// it).
        key: String,
        /// An RFC 6901 JSON Pointer to the object that carries the duplicate, **relative to the
        /// scan root** — `""` for a duplicate directly inside the root object. Under
        /// `at = &["params"]` a nested example is `/arguments/deps/0` (a duplicate inside an object
        /// nested in an array) or `/_meta/trace`.
        path: String,
    },
    /// The bytes could not be tokenized to a decision (malformed JSON, non-UTF-8, nesting deeper
    /// than `serde_json`'s 128-level recursion limit, trailing characters).
    ///
    /// **NEVER equivalent to [`DupScan::Clean`].**
    Indeterminate,
}

/// Scan `bytes` for a duplicate object key inside the subtree at `at`, at ANY depth.
///
/// `at` is a path of object keys resolved **structurally from the document root** — never
/// textually. `at = &["params"]` is the MCP path (the WHOLE `tools/call` `params` value, the
/// reserved `_meta` member included); `at = &[]` scans the whole document (the `bd` importer path).
///
/// # Edge semantics (pinned fail-closed)
///
/// * A path that does not resolve (the document root is not an object, or a key along `at` is
///   absent) ⇒ [`DupScan::Clean`] — there is nothing in scope to be ambiguous about.
/// * The scan root being an **array** ⇒ descended: every object element is duplicate-checked at
///   any depth.
/// * The scan root being a **scalar** ⇒ [`DupScan::Clean`] — a scalar has no key to duplicate.
/// * Anything the parser cannot tokenize ⇒ [`DupScan::Indeterminate`], never `Clean`.
/// * If the key named by `at` appears MORE THAN ONCE at its level, **every** occurrence is scanned
///   (such a frame is rejected upstream anyway; scanning all of them is the fail-closed direction).
///
/// Keys OUTSIDE the scan root are deliberately not duplicate-checked: the scan root is the whole
/// contract, and a duplicate elsewhere in the envelope is a different (already non-executing) class.
///
/// # Examples
///
/// ```
/// use unblock_error::dup_key::{DupScan, scan};
///
/// // A duplicate directly inside the scan root: an empty pointer.
/// let frame = br#"{"method":"tools/call","params":{"a":1,"a":2}}"#;
/// assert_eq!(
///     scan(frame, &["params"]),
///     DupScan::Duplicate { key: "a".to_string(), path: String::new() }
/// );
///
/// // Keys are compared DECODED: the `\u0061` escape IS the bare `a`.
/// let escaped = br#"{"params":{"\u0061":1,"a":2}}"#;
/// assert!(matches!(scan(escaped, &["params"]), DupScan::Duplicate { .. }));
///
/// // A duplicate OUTSIDE the scan root is out of scope by construction.
/// let outside = br#"{"extra":{"a":1,"a":2},"params":{"ok":1}}"#;
/// assert_eq!(scan(outside, &["params"]), DupScan::Clean);
///
/// // Malformed input is INDETERMINATE — never `Clean`.
/// assert_eq!(scan(b"{not json", &["params"]), DupScan::Indeterminate);
/// ```
#[must_use]
pub fn scan(bytes: &[u8], at: &[&str]) -> DupScan {
    scan_with_stats(bytes, at).0
}

/// Work counters recorded by one [`scan`], for complexity-shape assertions.
///
/// Crate-internal: the public contract is the verdict, not the counters. They exist so the unit
/// suite can assert the scan's complexity CLASS deterministically (no wall-clock bench).
///
/// **Which counter carries that claim matters.** [`ScanStats::keys_examined`] cannot: it is
/// incremented once per key DECODED, *before* the membership probe, so it equals the key count
/// under ANY membership algorithm — a regression from the pooled `HashSet` (O(1) comparisons per
/// probe) to a pairwise scan (O(k) per probe) leaves it byte-for-byte identical. The complexity
/// claim therefore rides [`ScanStats::key_comparisons`], which counts the key EQUALITY comparisons
/// the probes actually perform ([`ObjectKey`]'s `PartialEq`, i.e. inside the probe rather than at
/// its call site), so a pairwise regression shows up as O(k²) comparison work and fails the cell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "the counters are read by the complexity-shape unit cells in this module"
)]
pub(crate) struct ScanStats {
    /// Object keys DECODED — one per `next_key`, counted before the membership probe. A pure
    /// measure of the INPUT: it is `k` under any algorithm and says nothing about probe work.
    pub(crate) keys_examined: usize,
    /// Key EQUALITY comparisons performed inside the per-object membership probes and inserts.
    /// THIS is the complexity-shape counter (see the type doc).
    pub(crate) key_comparisons: usize,
    /// Object frames entered.
    pub(crate) objects_examined: usize,
    /// The deepest container nesting reached inside the scan root.
    pub(crate) max_depth: usize,
    /// Input bytes the parser consumed (BOM excluded). Never exceeds the input length.
    pub(crate) bytes_examined: usize,
}

thread_local! {
    /// Every key equality comparison [`ObjectKey`] has performed on this thread.
    ///
    /// `PartialEq::eq` cannot reach the active [`ScanState`], so the count rides a thread-local and
    /// [`scan_with_stats`] records the DELTA across one scan — a delta rather than a reset, so a
    /// nested or interleaved scan on the same thread cannot corrupt an outer one's reading.
    static KEY_COMPARISONS: Cell<usize> = const { Cell::new(0) };
}

/// The running per-thread key-comparison count.
fn key_comparisons() -> usize {
    KEY_COMPARISONS.with(Cell::get)
}

/// A decoded object key whose EQUALITY COMPARISONS ARE COUNTED.
///
/// The newtype exists for exactly one reason: it makes the complexity-shape guard real. A hash-set
/// probe compares a bounded number of keys per lookup and a linear scan compares O(k) — and **no
/// counter incremented at the call site can tell those apart**, because both perform exactly one
/// probe per decoded key. Counting inside `eq` measures the work the probe itself does, so
/// swapping the pooled `HashSet` for a pairwise container turns the unit cell RED instead of
/// leaving it vacuously green.
struct ObjectKey(String);

impl ObjectKey {
    /// Unwrap the decoded key (used on the short-circuit path, for the verdict and the pointer).
    fn into_inner(self) -> String {
        self.0
    }
}

impl PartialEq for ObjectKey {
    fn eq(&self, other: &Self) -> bool {
        KEY_COMPARISONS.with(|count| count.set(count.get().saturating_add(1)));
        self.0 == other.0
    }
}

impl Eq for ObjectKey {}

/// Delegated to the inner `String`, by hand: DERIVING `Hash` beside a manual `PartialEq` is the
/// `derived_hash_with_manual_eq` defect — the two must stay consistent by construction, because
/// `HashSet` relies on `a == b ⇒ hash(a) == hash(b)`.
impl Hash for ObjectKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

/// One RFC 6901 pointer segment on the path from the scan root to a duplicate's container.
enum Segment {
    /// An object member name (DECODED — escaped on the way into the pointer).
    Key(String),
    /// An array index.
    Index(usize),
}

/// The recorded first duplicate, plus the reversed segment path built as the error unwinds.
struct Found {
    /// The decoded duplicated key.
    key: String,
    /// Segments from the duplicate's container UP to the scan root (innermost first).
    segments: Vec<Segment>,
}

/// Mutable scan state threaded through every seed/visitor.
struct ScanState {
    /// The first duplicate, once found. Its presence is what distinguishes a short-circuit
    /// from a genuine parse failure.
    found: Option<Found>,
    /// Reused per-object key sets — one allocation amortised across every object in the document.
    pool: Vec<HashSet<ObjectKey>>,
    /// The current container nesting depth inside the scan root.
    depth: usize,
    /// Work counters.
    stats: ScanStats,
}

impl ScanState {
    fn new() -> Self {
        Self {
            found: None,
            pool: Vec::new(),
            depth: 0,
            stats: ScanStats::default(),
        }
    }

    /// Record the first duplicate. Later calls are ignored (first-wins short-circuit).
    fn record(&mut self, key: String) {
        if self.found.is_none() {
            self.found = Some(Found {
                key,
                segments: Vec::new(),
            });
        }
    }

    /// Append the enclosing container's segment as the short-circuit error unwinds.
    fn push_segment(&mut self, segment: Segment) {
        if let Some(found) = self.found.as_mut() {
            found.segments.push(segment);
        }
    }

    fn enter(&mut self) {
        self.depth += 1;
        if self.depth > self.stats.max_depth {
            self.stats.max_depth = self.depth;
        }
    }

    fn leave(&mut self) {
        self.depth -= 1;
    }
}

/// Render the reversed segment stack as an RFC 6901 JSON Pointer relative to the scan root.
///
/// `~` and `/` are escaped as `~0`/`~1` in EVERY key segment before joining. Skipping that is a
/// real defect, not a cosmetic one: a key literally containing `/` would otherwise forge a pointer
/// that reads as two segments.
fn pointer_of(segments: &[Segment]) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    for segment in segments.iter().rev() {
        out.push('/');
        match segment {
            Segment::Key(key) => {
                for ch in key.chars() {
                    match ch {
                        '~' => out.push_str("~0"),
                        '/' => out.push_str("~1"),
                        other => out.push(other),
                    }
                }
            }
            Segment::Index(index) => {
                // `write!` into a `String` is infallible.
                let _ = write!(out, "{index}");
            }
        }
    }
    out
}

/// Walk the document down `at`, then hand the resolved subtree to [`ScanSeed`].
struct PathSeed<'a, 'p> {
    state: &'a mut ScanState,
    at: &'p [&'p str],
}

impl<'de> DeserializeSeed<'de> for PathSeed<'_, '_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<(), D::Error>
    where
        D: Deserializer<'de>,
    {
        if self.at.is_empty() {
            return ScanSeed { state: self.state }.deserialize(deserializer);
        }
        deserializer.deserialize_any(PathVisitor {
            state: self.state,
            at: self.at,
        })
    }
}

/// Ignores every shape except an object, in which case it descends the matching key(s).
struct PathVisitor<'a, 'p> {
    state: &'a mut ScanState,
    at: &'p [&'p str],
}

impl<'de> Visitor<'de> for PathVisitor<'_, '_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("any JSON value")
    }

    fn visit_bool<E: de::Error>(self, _v: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: de::Error>(self, _v: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_u64<E: de::Error>(self, _v: u64) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E: de::Error>(self, _v: f64) -> Result<(), E> {
        Ok(())
    }
    fn visit_str<E: de::Error>(self, _v: &str) -> Result<(), E> {
        Ok(())
    }
    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_none<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<(), A::Error>
    where
        A: SeqAccess<'de>,
    {
        // A non-object at a path position means the path does not resolve — but the value must
        // still be fully drained, or the parser reports trailing characters.
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(())
    }

    fn visit_map<A>(self, mut map: A) -> Result<(), A::Error>
    where
        A: MapAccess<'de>,
    {
        let state = self.state;
        let head = self.at[0];
        let tail = &self.at[1..];
        while let Some(key) = map.next_key::<String>()? {
            if key == head {
                // Every occurrence is descended (fail-closed), not just the first.
                map.next_value_seed(PathSeed {
                    state: &mut *state,
                    at: tail,
                })?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(())
    }
}

/// Duplicate-checks every object in the subtree it is applied to, at any depth.
struct ScanSeed<'a> {
    state: &'a mut ScanState,
}

impl<'de> DeserializeSeed<'de> for ScanSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<(), D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(ScanVisitor { state: self.state })
    }
}

/// The duplicate-detecting visitor. Scalars are ignored; containers are descended.
struct ScanVisitor<'a> {
    state: &'a mut ScanState,
}

impl<'de> Visitor<'de> for ScanVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("any JSON value")
    }

    fn visit_bool<E: de::Error>(self, _v: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: de::Error>(self, _v: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_u64<E: de::Error>(self, _v: u64) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E: de::Error>(self, _v: f64) -> Result<(), E> {
        Ok(())
    }
    fn visit_str<E: de::Error>(self, _v: &str) -> Result<(), E> {
        Ok(())
    }
    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_none<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<(), A::Error>
    where
        A: SeqAccess<'de>,
    {
        let state = self.state;
        state.enter();
        let mut index = 0usize;
        loop {
            match seq.next_element_seed(ScanSeed { state: &mut *state }) {
                Ok(Some(())) => index += 1,
                Ok(None) => break,
                Err(err) => {
                    state.push_segment(Segment::Index(index));
                    return Err(err);
                }
            }
        }
        state.leave();
        Ok(())
    }

    fn visit_map<A>(self, mut map: A) -> Result<(), A::Error>
    where
        A: MapAccess<'de>,
    {
        let state = self.state;
        state.enter();
        state.stats.objects_examined += 1;
        // One reusable key set per object frame, taken from (and returned to) the pool.
        let mut seen = state.pool.pop().unwrap_or_default();
        seen.clear();

        loop {
            let next = match map.next_key::<String>() {
                Ok(next) => next,
                Err(err) => {
                    state.pool.push(seen);
                    return Err(err);
                }
            };
            let Some(key) = next else { break };
            state.stats.keys_examined += 1;
            // The comparisons this probe performs are counted INSIDE `ObjectKey::eq`, which is the
            // only place that can distinguish an O(1) hash probe from an O(k) pairwise scan.
            let key = ObjectKey(key);

            if seen.contains(&key) {
                // THE defect: exact equality on the DECODED key — this is where `serde_json`'s own
                // `Map::insert` would silently overwrite the earlier member.
                state.record(key.into_inner());
                state.pool.push(seen);
                return Err(de::Error::custom(SHORT_CIRCUIT));
            }

            // Descend BEFORE inserting so `key` is still owned here and can be attached to the
            // pointer if the duplicate turns out to be nested inside this member's value.
            if let Err(err) = map.next_value_seed(ScanSeed { state: &mut *state }) {
                state.push_segment(Segment::Key(key.into_inner()));
                state.pool.push(seen);
                return Err(err);
            }
            seen.insert(key);
        }

        state.pool.push(seen);
        state.leave();
        Ok(())
    }
}

/// [`scan`] plus the work counters (crate-internal; see [`ScanStats`]).
pub(crate) fn scan_with_stats(bytes: &[u8], at: &[&str]) -> (DupScan, ScanStats) {
    // Exactly one BOM, prefix only — so the scanner and the downstream parser see the SAME document.
    let body = bytes.strip_prefix(UTF8_BOM.as_slice()).unwrap_or(bytes);

    let comparisons_before = key_comparisons();
    let mut state = ScanState::new();
    let mut deserializer = serde_json::Deserializer::from_slice(body);
    let mut outcome = PathSeed {
        state: &mut state,
        at,
    }
    .deserialize(&mut deserializer);
    if outcome.is_ok() {
        // Match the downstream parser's own strictness: `serde_json::from_slice` rejects trailing
        // characters, so a document this scanner accepted but the parser would not must not be
        // reported `Clean` on the strength of a partial read.
        outcome = deserializer.end();
    }

    let mut counters = state.stats;
    // The DELTA over this scan (see `KEY_COMPARISONS`): saturating, because the running per-thread
    // total is itself saturating and a saturated total would otherwise underflow the subtraction.
    counters.key_comparisons = key_comparisons().saturating_sub(comparisons_before);
    let verdict = match outcome {
        Ok(()) => {
            counters.bytes_examined = body.len();
            DupScan::Clean
        }
        Err(err) => {
            // A `serde` custom error is created with line 0 and re-positioned by `serde_json` as it
            // propagates, so `column` is the byte column the parse stopped at on this (single-line)
            // document. Clamped, because a re-position is documented as possibly off by one.
            counters.bytes_examined = err.column().min(body.len());
            match state.found {
                Some(found) => DupScan::Duplicate {
                    path: pointer_of(&found.segments),
                    key: found.key,
                },
                None => DupScan::Indeterminate,
            }
        }
    };
    (verdict, counters)
}

#[cfg(test)]
mod tests {
    use super::{DupScan, ScanStats, scan, scan_with_stats};

    /// The MCP scan root (D43): the WHOLE `tools/call` `params` value, `_meta` included.
    const PARAMS: &[&str] = &["params"];

    fn frame(params: &str) -> String {
        format!(r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{params}}}"#)
    }

    fn dup(bytes: &str, at: &[&str]) -> (String, String) {
        match scan(bytes.as_bytes(), at) {
            DupScan::Duplicate { key, path } => (key, path),
            other => panic!("expected Duplicate, got {other:?} for {bytes}"),
        }
    }

    // -- the headline defect ------------------------------------------------------------------

    #[test]
    fn the_live_exploit_frame_is_a_duplicate() {
        // The frame that tombstoned a live issue on GA: reads `create`, executed `delete`.
        let line = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"issue","arguments":{"action":"create","ids":["ub-vog"],"action":"delete"}}}"#;
        assert_eq!(
            dup(line, PARAMS),
            ("action".to_string(), "/arguments".to_string())
        );
    }

    #[test]
    fn clean_frame_is_clean() {
        let line = frame(r#"{"name":"issue","arguments":{"action":"create","title":"x"}}"#);
        assert_eq!(scan(line.as_bytes(), PARAMS), DupScan::Clean);
    }

    // -- §4.5 escape equivalence (E1-E3) ------------------------------------------------------
    //
    // Each pair is spelled with BYTE-DIFFERENT key spans that DECODE equal. A raw-span comparator
    // passes every one of these vacuously — which is exactly the silent-bypass class they exist to
    // catch.

    #[test]
    fn e1_unicode_escape_equals_bare_key() {
        let raw = r#"{"params":{"\u0061":1,"a":2}}"#;
        assert_ne!(
            r"\u0061".as_bytes(),
            "a".as_bytes(),
            "the RAW key spans must differ — otherwise this cell is vacuous"
        );
        assert_eq!(dup(raw, PARAMS), ("a".to_string(), String::new()));
    }

    #[test]
    fn e2_escaped_solidus_equals_bare_solidus() {
        let raw = r#"{"params":{"a\/b":1,"a/b":2}}"#;
        assert_ne!(
            r"a\/b".as_bytes(),
            "a/b".as_bytes(),
            "the RAW key spans must differ — otherwise this cell is vacuous"
        );
        // The duplicate sits at the scan root, so the pointer is empty; the KEY carries the literal
        // solidus and is reported DECODED.
        assert_eq!(dup(raw, PARAMS), ("a/b".to_string(), String::new()));
    }

    #[test]
    fn e3_surrogate_pair_escape_equals_literal_glyph() {
        let raw = "{\"params\":{\"\\ud83d\\ude00\":1,\"\u{1f600}\":2}}";
        assert_eq!("\u{1f600}".len(), 4, "the literal glyph is 4 raw bytes");
        assert_eq!(dup(raw, PARAMS), ("\u{1f600}".to_string(), String::new()));
    }

    // -- §4.5 over-rejection negatives (E4/E5) ------------------------------------------------

    #[test]
    fn e4_nfc_and_nfd_are_not_duplicates() {
        // U+00E9 vs `e` + U+0301 render identically but are DIFFERENT strings. Key comparison is
        // plain `==` on the decoded `String`, never Unicode normalization.
        let raw = "{\"params\":{\"\u{e9}\":1,\"e\u{301}\":2}}";
        assert_eq!(scan(raw.as_bytes(), PARAMS), DupScan::Clean);
    }

    #[test]
    fn e5_case_differing_keys_are_not_duplicates() {
        let raw = r#"{"params":{"a":1,"A":2}}"#;
        assert_eq!(scan(raw.as_bytes(), PARAMS), DupScan::Clean);
    }

    // -- nesting / containers -----------------------------------------------------------------

    #[test]
    fn n2_object_nested_in_an_array_is_scanned() {
        let line = frame(
            r#"{"name":"issue","arguments":{"action":"create","title":"x","deps":[{"issue_id":"a","depends_on_id":"b","dep_type":"blocks","dep_type":"discovered-from"}]}}"#,
        );
        assert_eq!(
            dup(&line, PARAMS),
            ("dep_type".to_string(), "/arguments/deps/0".to_string())
        );
    }

    #[test]
    fn n4_duplicate_inside_the_meta_value_is_scanned() {
        let line = frame(
            r#"{"name":"issue","arguments":{"action":"show","id":"ub-a"},"_meta":{"trace":{"span":"x","span":"y"}}}"#,
        );
        assert_eq!(
            dup(&line, PARAMS),
            ("span".to_string(), "/_meta/trace".to_string())
        );
    }

    #[test]
    fn depth_three_nesting_is_scanned() {
        let line = frame(r#"{"a":{"b":{"c":{"d":1,"d":2}}}}"#);
        assert_eq!(dup(&line, PARAMS), ("d".to_string(), "/a/b/c".to_string()));
    }

    #[test]
    fn pointer_segments_are_rfc6901_escaped() {
        // A key literally containing `/` and `~` must not forge extra pointer segments.
        let line = frame(r#"{"a/b":{"c~d":{"x":1,"x":2}}}"#);
        assert_eq!(
            dup(&line, PARAMS),
            ("x".to_string(), "/a~1b/c~0d".to_string())
        );
    }

    #[test]
    fn array_valued_scan_root_is_descended() {
        let line = r#"{"params":[{"ok":1},{"a":1,"a":2}]}"#;
        assert_eq!(dup(line, PARAMS), ("a".to_string(), "/1".to_string()));
    }

    // -- scope: the root is resolved STRUCTURALLY, never textually ----------------------------

    #[test]
    fn the_text_params_appearing_as_a_value_is_not_a_scan_root() {
        // `text.find("\"params\"")` would misfire on the NAME here. The real root is clean.
        let line = r#"{"name":"params","params":{"ok":1}}"#;
        assert_eq!(scan(line.as_bytes(), PARAMS), DupScan::Clean);
    }

    #[test]
    fn a_string_value_containing_a_duplicate_is_not_a_duplicate() {
        let line = frame(r#"{"note":"{\"a\":1,\"a\":2}"}"#);
        assert_eq!(scan(line.as_bytes(), PARAMS), DupScan::Clean);
    }

    #[test]
    fn ns5_a_duplicate_outside_the_scan_root_is_invisible() {
        let line = r#"{"jsonrpc":"2.0","params":{"ok":1},"extra":{"a":1,"a":2}}"#;
        assert_eq!(scan(line.as_bytes(), PARAMS), DupScan::Clean);
    }

    #[test]
    fn an_absent_scan_root_is_clean() {
        assert_eq!(scan(br#"{"method":"ping"}"#, PARAMS), DupScan::Clean);
    }

    #[test]
    fn a_non_object_document_root_is_clean() {
        assert_eq!(scan(b"[1,2,3]", PARAMS), DupScan::Clean);
        assert_eq!(scan(b"\"just a string\"", PARAMS), DupScan::Clean);
    }

    #[test]
    fn a_scalar_scan_root_is_clean() {
        assert_eq!(scan(br#"{"params":7}"#, PARAMS), DupScan::Clean);
        assert_eq!(scan(br#"{"params":null}"#, PARAMS), DupScan::Clean);
        assert_eq!(scan(br#"{"params":"text"}"#, PARAMS), DupScan::Clean);
    }

    #[test]
    fn every_occurrence_of_a_repeated_scan_root_key_is_scanned() {
        // The FIRST `params` is clean; the SECOND carries the duplicate. A scanner that stopped at
        // the first match would report `Clean` — the fail-open direction.
        let line = r#"{"params":{"ok":1},"params":{"a":1,"a":2}}"#;
        assert_eq!(dup(line, PARAMS), ("a".to_string(), String::new()));
    }

    // -- the whole-document root (`at = &[]`, the bd path) ------------------------------------

    #[test]
    fn whole_document_root_scans_the_top_level() {
        assert_eq!(
            dup(r#"{"id":"a","id":"b"}"#, &[]),
            ("id".to_string(), String::new())
        );
    }

    #[test]
    fn whole_document_root_scans_nested_arrays() {
        let line = r#"{"id":"a","comments":[{"id":"c1","text":"x","text":"y"}]}"#;
        assert_eq!(
            dup(line, &[]),
            ("text".to_string(), "/comments/0".to_string())
        );
    }

    // -- framing / normalization ---------------------------------------------------------------

    #[test]
    fn a_leading_bom_is_stripped_exactly_once() {
        let mut bytes = b"\xEF\xBB\xBF".to_vec();
        bytes.extend_from_slice(br#"{"params":{"a":1,"a":2}}"#);
        assert!(matches!(scan(&bytes, PARAMS), DupScan::Duplicate { .. }));

        // TWO BOMs: only the first is stripped, so the rest is not valid JSON — INDETERMINATE, and
        // that matches what the downstream parser does with the same bytes.
        let mut doubled = b"\xEF\xBB\xBF\xEF\xBB\xBF".to_vec();
        doubled.extend_from_slice(br#"{"params":{"a":1}}"#);
        assert_eq!(scan(&doubled, PARAMS), DupScan::Indeterminate);
    }

    // -- INDETERMINATE (the fail-closed arm) ---------------------------------------------------

    #[test]
    fn malformed_input_is_indeterminate() {
        for bad in [
            &b"{not json"[..],
            &b""[..],
            &br#"{"params":{"a":}}"#[..],
            &br#"{"params":{"a":1}} trailing"#[..],
        ] {
            assert_eq!(
                scan(bad, PARAMS),
                DupScan::Indeterminate,
                "input {bad:?} must be INDETERMINATE, never Clean"
            );
        }
    }

    #[test]
    fn non_utf8_bytes_are_indeterminate() {
        let bad = b"{\"params\":{\"a\":\"\xff\xfe\"}}";
        assert_eq!(scan(bad, PARAMS), DupScan::Indeterminate);
    }

    #[test]
    fn nesting_past_the_serde_json_recursion_limit_is_indeterminate() {
        // §5 row 6.6: a DIRECT unit pin of the `Indeterminate` verdict, invoked programmatically —
        // the wire path cannot reach this arm for a depth-130 frame (the downstream parse fails
        // first with `-32700`), so this is the arm's only scanner-side pin.
        for depth in [130usize, 100_000] {
            let mut line = String::from(r#"{"params":"#);
            line.push_str(&"[".repeat(depth));
            line.push_str(&"]".repeat(depth));
            line.push('}');
            assert_eq!(
                scan(line.as_bytes(), PARAMS),
                DupScan::Indeterminate,
                "depth {depth} must be INDETERMINATE (bounded work, no stack overflow)"
            );
        }
    }

    #[test]
    fn shallow_nesting_below_the_recursion_limit_still_scans() {
        // The complement of the cell above: 120 levels are UNDER `serde_json`'s 128-level limit, so
        // the scan must still reach the duplicate rather than degrade to `Indeterminate`.
        let depth = 120usize;
        let mut line = String::from(r#"{"params":"#);
        line.push_str(&"[".repeat(depth));
        line.push_str(r#"{"a":1,"a":2}"#);
        line.push_str(&"]".repeat(depth));
        line.push('}');
        assert!(matches!(
            scan(line.as_bytes(), PARAMS),
            DupScan::Duplicate { ref key, .. } if key == "a"
        ));
    }

    // -- complexity shape ----------------------------------------------------------------------

    fn stats_for_distinct_keys(count: usize) -> ScanStats {
        let mut line = String::from(r#"{"params":{"#);
        for i in 0..count {
            if i > 0 {
                line.push(',');
            }
            line.push('"');
            line.push('k');
            line.push_str(itoa(i).as_str());
            line.push_str("\":0");
        }
        line.push_str("}}");
        let (verdict, stats) = scan_with_stats(line.as_bytes(), PARAMS);
        assert_eq!(verdict, DupScan::Clean);
        stats
    }

    fn itoa(value: usize) -> String {
        value.to_string()
    }

    /// A comparison budget per decoded key. A pooled `HashSet` spends O(1) comparisons per probe
    /// (empirically well under one, since a probe only compares on a control-byte match); this
    /// leaves generous room for that while sitting three orders of magnitude below the ~k²/2 a
    /// pairwise container would spend.
    const COMPARISONS_PER_KEY_CEILING: usize = 4;

    #[test]
    fn key_membership_work_is_linear_not_quadratic() {
        // WHICH COUNTER IS ASSERTED IS THE WHOLE CELL. `keys_examined` is incremented once per key
        // DECODED, before the probe, so it equals k under any algorithm — asserting a linear ratio
        // on it is true by construction and a `HashSet` -> `Vec` regression stays green. The real
        // guard is `key_comparisons`, counted inside `ObjectKey::eq` (i.e. inside the probe).
        let small = stats_for_distinct_keys(10_000);
        let large = stats_for_distinct_keys(20_000);
        assert_eq!(small.keys_examined, 10_000, "sanity: every key is decoded");
        assert_eq!(large.keys_examined, 20_000, "sanity: every key is decoded");

        // A pairwise regression spends ~k²/2 comparisons — 5.0e7 and 2.0e8 at these two sizes,
        // against ceilings of 4.0e4 and 8.0e4. The arm dies by three orders of magnitude, not by a
        // tuned constant. A ratio between the two sizes is deliberately NOT asserted: with a hash
        // set the absolute counts are small and driven by 7-bit control-byte collisions, so their
        // ratio is noise, while the per-key BUDGET below is exactly the linear/quadratic
        // discriminator.
        for (label, stats) in [("10k", small), ("20k", large)] {
            // ORDER MATTERS: the ALGORITHM bound is asserted first, so a pairwise regression is
            // reported as the quadratic blow-up it is. The INSTRUMENT bound below sits inside this
            // one, and reversing them would make every quadratic failure print the instrument's
            // diagnosis instead of the real cause.
            assert!(
                stats.key_comparisons <= COMPARISONS_PER_KEY_CEILING * stats.keys_examined,
                "{label}: membership work must be LINEAR in the key count — {} comparisons for {} \
                 keys exceeds the {COMPARISONS_PER_KEY_CEILING}x budget (a pairwise scan would \
                 spend ~{})",
                stats.key_comparisons,
                stats.keys_examined,
                stats.keys_examined * stats.keys_examined / 2
            );
            // THE INSTRUMENT ITSELF, pinned — the budget above cannot do it. That budget is also
            // satisfied by a counter MOVED OUT of `ObjectKey::eq` to the probe's call site (one
            // plausible refactor), which records exactly 1.0 comparisons per decoded key and
            // re-creates the very vacuity this counter exists to kill: before the assertion below
            // it passed every cell, INCLUDING when compounded with a pairwise container — the
            // relocated counter reads ~k (one per decoded key) whatever the container does, so the
            // budget above sees k, not ~k²/2, and never fires. A real hash probe compares FEWER
            // keys than it decodes (measured: ~0.1 per key), so strict inequality separates a
            // probe-work counter from a decode-work one and kills both the relocation alone and
            // the compounded case.
            assert!(
                stats.key_comparisons < stats.keys_examined,
                "{label}: the counter must measure PROBE work, not DECODE work — {} comparisons \
                 for {} decoded keys is at least one per key, which is what counting at the \
                 probe's CALL SITE (instead of inside `ObjectKey::eq`) produces; a hash probe \
                 compares fewer keys than it decodes",
                stats.key_comparisons,
                stats.keys_examined
            );
        }
    }

    #[test]
    fn the_comparison_counter_is_wired_to_the_probe() {
        // NON-VACUITY for the budget above: a counter stuck at zero satisfies any linear bound. A
        // duplicate can only be detected BY a successful comparison, so this scan must record one.
        let (verdict, stats) = scan_with_stats(br#"{"params":{"a":1,"a":2}}"#, PARAMS);
        assert!(matches!(verdict, DupScan::Duplicate { .. }));
        assert!(
            stats.key_comparisons >= 1,
            "detecting a duplicate REQUIRES comparing the two keys, so the counter cannot be 0: \
             {stats:?}"
        );
    }

    #[test]
    fn bytes_examined_tracks_the_parse_and_short_circuits_at_the_first_duplicate() {
        let clean = frame(r#"{"name":"issue","arguments":{"action":"list"}}"#);
        let (verdict, stats) = scan_with_stats(clean.as_bytes(), PARAMS);
        assert_eq!(verdict, DupScan::Clean);
        assert_eq!(stats.bytes_examined, clean.len());

        // A 100 KiB pad AFTER the duplicate. Asserting `bytes_examined <= frame_len` here would be
        // TAUTOLOGICAL — the recording site clamps the error column with `.min(body.len())` — so
        // the real property is asserted instead: the scan stops AT the second `a`, before the pad,
        // rather than tokenizing the rest of the document.
        let pad = "x".repeat(100 * 1024);
        let padded = frame(&format!(r#"{{"a":1,"a":2,"pad":"{pad}"}}"#));
        let up_to_the_pad = frame(r#"{"a":1,"a":2,"pad":""}"#);
        assert_eq!(
            padded.len(),
            up_to_the_pad.len() + pad.len(),
            "the two frames must differ ONLY by the pad, else the bound below is arbitrary"
        );

        let (verdict, stats) = scan_with_stats(padded.as_bytes(), PARAMS);
        assert!(matches!(verdict, DupScan::Duplicate { .. }));
        assert!(
            stats.bytes_examined <= up_to_the_pad.len(),
            "the scan must SHORT-CIRCUIT at the duplicate (byte {}), not read the 100 KiB pad \
             behind it: examined {} of {} bytes",
            up_to_the_pad.len(),
            stats.bytes_examined,
            padded.len()
        );
    }

    #[test]
    fn the_key_set_pool_is_reused_across_sibling_objects() {
        let line = frame(r#"{"a":{"x":1},"b":{"x":1},"c":{"x":1},"d":{"x":1}}"#);
        let (verdict, stats) = scan_with_stats(line.as_bytes(), PARAMS);
        assert_eq!(verdict, DupScan::Clean);
        assert_eq!(stats.objects_examined, 5);
        assert_eq!(stats.max_depth, 2);
    }
}
