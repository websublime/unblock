//! Issue-ID format parsing/validation **plus the pure candidate-compute half of id generation**
//! (all pure; no I/O — D21).
//!
//! This ports the **format** side of the classic `bd` id scheme — `<prefix>-<hash>` with an optional
//! dot-separated child path (`<prefix>-<hash>.1.2`) — and, since T1.8 (D21), the **pure, deterministic
//! generation primitives**: the length-prefixed seed ([`generate_id_seed`]), the SHA-256 → base36
//! tail-slice hash ([`compute_id_hash`]), the birthday-heuristic adaptive length
//! ([`optimal_hash_length`]), the slug/prefix normalizers ([`normalize_slug`] / [`normalize_prefix`] /
//! [`normalize_slug_for_prefix`]), and the [`child_id`] formatter. These are I/O-free and share the
//! parser's home.
//!
//! Only the **stateful** allocator — the adaptive-count + storage-probe collision-retry loop driven by
//! the existence probe (`get_issue(id).await?.is_some()`) and the `Storage::next_child_number` read —
//! lives in `unblock-engine` (`src/session/ids.rs`). The stateful `IdGenerator` / `IdResolver` structs
//! of the original are **not** ported; only their pure inner functions are.

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};

use unblock_error::ModelError;

/// Maximum length of an id prefix.
pub const MAX_ID_PREFIX_LEN: usize = 64;
/// Maximum length of an id hash portion.
pub const MAX_ID_HASH_LEN: usize = 40;
/// Maximum total id length (`prefix` + `-` + `hash`).
pub const MAX_ID_LENGTH: usize = MAX_ID_PREFIX_LEN + 1 + MAX_ID_HASH_LEN;

/// Maximum length of a normalized slug (the user-supplied human-readable segment of an id). Capped
/// well below [`MAX_ID_PREFIX_LEN`] so the `<prefix>-<slug>` portion still fits within the parser's
/// prefix budget (D21).
pub const MAX_SLUG_LEN: usize = 48;

/// Minimum adaptive hash length (the birthday-heuristic floor — D21).
const MIN_HASH_LENGTH: usize = 3;
/// Maximum adaptive hash length the heuristic climbs to before [`optimal_hash_length`] saturates
/// (D21; bounded well under [`MAX_ID_HASH_LEN`] so the parser still accepts the id).
const MAX_HASH_LENGTH: usize = 8;
/// The birthday-problem collision probability ceiling: [`optimal_hash_length`] grows the hash until
/// `P(collision) < MAX_COLLISION_PROB` (D21).
const MAX_COLLISION_PROB: f64 = 0.25;

/// Parsed components of an issue id.
///
/// Supports root ids (`ub-abc123`) and hierarchical ids (`ub-abc123.1.2`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedId {
    /// The prefix (e.g. `"ub"`); may itself contain hyphens (`"bead-me-up"`).
    pub prefix: String,
    /// The base hash portion (e.g. `"abc123"`).
    pub hash: String,
    /// Child-path segments for a hierarchical id (e.g. `[1, 2]` for `.1.2`).
    pub child_path: Vec<u32>,
}

impl ParsedId {
    /// Whether this is a root (non-hierarchical) id.
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.child_path.is_empty()
    }

    /// The depth in the hierarchy (`0` for root).
    #[must_use]
    pub fn depth(&self) -> usize {
        self.child_path.len()
    }

    /// The parent id, or `None` for a root id.
    #[must_use]
    pub fn parent(&self) -> Option<String> {
        if self.child_path.is_empty() {
            return None;
        }
        let mut parent_path = self.child_path.clone();
        parent_path.pop();
        if parent_path.is_empty() {
            Some(format!("{}-{}", self.prefix, self.hash))
        } else {
            Some(format!(
                "{}-{}{}",
                self.prefix,
                self.hash,
                format_child_path(&parent_path)
            ))
        }
    }

    /// Reconstruct the full id string.
    #[must_use]
    pub fn to_id_string(&self) -> String {
        if self.child_path.is_empty() {
            format!("{}-{}", self.prefix, self.hash)
        } else {
            format!(
                "{}-{}{}",
                self.prefix,
                self.hash,
                format_child_path(&self.child_path)
            )
        }
    }

    /// Whether this id is a (direct or indirect) child of `potential_parent`.
    #[must_use]
    pub fn is_child_of(&self, potential_parent: &str) -> bool {
        let full = self.to_id_string();
        full.starts_with(potential_parent)
            && full.len() > potential_parent.len()
            && full[potential_parent.len()..].starts_with('.')
    }
}

fn format_child_path(path: &[u32]) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for segment in path {
        let _ = write!(out, ".{segment}");
    }
    out
}

/// Split an id into `(prefix, remainder)` at the last `-`, or `None` if either side is empty.
fn split_prefix_remainder(id: &str) -> Option<(&str, &str)> {
    let dash_pos = id.rfind('-')?;
    let (prefix, remainder_with_dash) = id.split_at(dash_pos);
    let remainder = remainder_with_dash.strip_prefix('-')?;
    if prefix.is_empty() || remainder.is_empty() {
        return None;
    }
    Some((prefix, remainder))
}

/// Parse an issue id into its components.
///
/// # Errors
///
/// Returns [`ModelError::InvalidId`] if the id format is invalid (bad prefix charset/length, empty
/// or non-base36 hash, or a non-numeric child-path segment).
pub fn parse_id(id: &str) -> Result<ParsedId, ModelError> {
    let invalid = || ModelError::InvalidId { id: id.to_string() };

    let Some((prefix, remainder)) = split_prefix_remainder(id) else {
        return Err(invalid());
    };

    if prefix.is_empty() || prefix.len() > MAX_ID_PREFIX_LEN {
        return Err(invalid());
    }

    if !prefix.chars().all(|c| {
        c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.' | ':' | '#')
    }) {
        return Err(invalid());
    }

    let parts: Vec<&str> = remainder.split('.').collect();
    let hash = parts[0].to_string();

    if hash.is_empty() || hash.len() > MAX_ID_HASH_LEN {
        return Err(invalid());
    }

    if !hash
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        return Err(invalid());
    }

    let mut child_path = Vec::new();
    for part in parts.iter().skip(1) {
        if part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()) {
            return Err(invalid());
        }
        match part.parse::<u32>() {
            Ok(n) => child_path.push(n),
            Err(_) => return Err(invalid()),
        }
    }

    Ok(ParsedId {
        prefix: prefix.to_string(),
        hash,
        child_path,
    })
}

/// Whether a string is a syntactically valid issue id.
#[must_use]
pub fn is_valid_id_format(id: &str) -> bool {
    parse_id(id).is_ok()
}

// ====================================================================================================
// Pure generation primitives (D21 — the I/O-free candidate compute; the stateful collision-retry loop
// lives in the engine allocator). Faithful port of `temp/beads_rust-main/src/util/id.rs`.
// ====================================================================================================

/// Generate the seed string for id generation (faithful to `util/id.rs:263-280`).
///
/// Inputs are length-prefixed as `len:value` fields so embedded separators in titles or descriptions
/// cannot collide with adjacent fields. The order is title, description, creator, the `created_at`
/// nanos timestamp, then the nonce.
#[must_use]
pub fn generate_id_seed(
    title: &str,
    description: Option<&str>,
    creator: Option<&str>,
    created_at: DateTime<Utc>,
    nonce: u32,
) -> String {
    let timestamp = created_at.timestamp_nanos_opt().unwrap_or(0).to_string();
    let nonce = nonce.to_string();

    let mut seed = String::new();
    append_seed_part(&mut seed, title);
    append_seed_part(&mut seed, description.unwrap_or(""));
    append_seed_part(&mut seed, creator.unwrap_or(""));
    append_seed_part(&mut seed, &timestamp);
    append_seed_part(&mut seed, &nonce);
    seed
}

/// Append one length-prefixed `len:value` field to the seed (`util/id.rs:282-286`).
fn append_seed_part(seed: &mut String, value: &str) {
    use std::fmt::Write;
    // Writing to a `String` is infallible; the result is intentionally discarded (no `.expect`).
    let _ = write!(seed, "{}:", value.len());
    seed.push_str(value);
}

/// Compute a base36 hash of the input string with a specific length (faithful to `util/id.rs:293-316`).
///
/// Uses SHA-256 to hash the input, converts the first 8 bytes to a `u64`, encodes as lowercase base36,
/// left-`'0'`-pads when shorter than `len`, and returns the **LAST `len` characters** — the
/// least-significant base36 digits, for full entropy from the tail.
#[must_use]
pub fn compute_id_hash(input: &str, length: usize) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();

    // Use the first 8 bytes for a 64-bit integer.
    let mut num = 0u64;
    for &byte in result.iter().take(8) {
        num = (num << 8) | u64::from(byte);
    }

    let encoded = base36_encode(num);

    // Pad with '0' if too short (unlikely for a u64, but possible).
    let mut s = encoded;
    if s.len() < length {
        s = format!("{s:0>length$}");
    }

    // Take the last `length` characters to ensure full entropy from the least-significant base36
    // digits of the encoding.
    let start = s.len().saturating_sub(length);
    s.chars().skip(start).collect()
}

/// Encode a `u64` as lowercase base36 (`0-9a-z`), most-significant digit first (`util/id.rs:318-329`).
fn base36_encode(mut num: u64) -> String {
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if num == 0 {
        return "0".to_string();
    }
    let mut chars = Vec::new();
    while num > 0 {
        chars.push(ALPHABET[(num % 36) as usize] as char);
        num /= 36;
    }
    chars.into_iter().rev().collect()
}

/// Compute the optimal hash length for a given issue count (faithful to `util/id.rs:97-111`).
///
/// Uses the birthday-problem approximation to estimate collision probability and returns the smallest
/// length in `MIN_HASH_LENGTH..=MAX_HASH_LENGTH` (3..=8) whose estimated `P(collision)` is below
/// [`MAX_COLLISION_PROB`] (0.25); saturates at [`MAX_HASH_LENGTH`] when none qualifies.
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap
)]
pub fn optimal_hash_length(issue_count: usize) -> usize {
    let n = issue_count as f64;

    for len in MIN_HASH_LENGTH..=MAX_HASH_LENGTH {
        // Base36 has 36^len possible values.
        let space = 36_f64.powi(len as i32);
        // Birthday problem: P(collision) ≈ 1 - e^(-n²/2d).
        let prob = 1.0 - (-n * n / (2.0 * space)).exp();
        if prob < MAX_COLLISION_PROB {
            return len;
        }
    }
    MAX_HASH_LENGTH
}

/// Normalize a configured issue prefix into a valid runtime form (faithful to `util/id.rs:575-599`).
///
/// Trims whitespace, lowercases ASCII letters, drops unsupported characters, clamps to
/// [`MAX_ID_PREFIX_LEN`], and strips trailing separators (so id generation never emits a double
/// hyphen). Falls back to `"ub"` when no usable characters remain (the unblock default prefix, D21).
#[must_use]
pub fn normalize_prefix(prefix: &str) -> String {
    let normalized: String = prefix
        .trim()
        .chars()
        .filter_map(|c| {
            let normalized = c.to_ascii_lowercase();
            (normalized.is_ascii_lowercase()
                || normalized.is_ascii_digit()
                || matches!(normalized, '_' | '-' | '.' | ':' | '#'))
            .then_some(normalized)
        })
        .take(MAX_ID_PREFIX_LEN)
        .collect();

    // Strip trailing separator chars to prevent double-hyphens during id generation.
    let normalized = normalized
        .trim_end_matches(['_', '-', '.', ':', '#'])
        .to_string();

    if normalized.is_empty() {
        "ub".to_string()
    } else {
        normalized
    }
}

/// Normalize a user-supplied slug for embedding in an issue id (faithful to `util/id.rs:616-642`).
///
/// The output is lowercase ASCII alphanumerics and single hyphens only — runs of any other characters
/// collapse to a single hyphen, leading/trailing hyphens are stripped, the result is capped at
/// [`MAX_SLUG_LEN`], and any trailing hyphen the cap may have left is re-trimmed.
///
/// Returns an empty string if no usable characters remain — callers must fall back to the hash-only id
/// path in that case (the empty drop-signal).
#[must_use]
pub fn normalize_slug(slug: &str) -> String {
    let mut out = String::with_capacity(slug.len());
    let mut prev_was_hyphen = false;
    for c in slug.chars() {
        let lc = c.to_ascii_lowercase();
        if lc.is_ascii_lowercase() || lc.is_ascii_digit() {
            out.push(lc);
            prev_was_hyphen = false;
        } else if !prev_was_hyphen && !out.is_empty() {
            out.push('-');
            prev_was_hyphen = true;
        }
    }

    // Trim a trailing hyphen any final non-alphanumeric run may have appended, then cap length.
    while out.ends_with('-') {
        out.pop();
    }
    if out.len() > MAX_SLUG_LEN {
        out.truncate(MAX_SLUG_LEN);
        while out.ends_with('-') {
            out.pop();
        }
    }
    out
}

/// Normalize a slug so the embedded `<prefix>-<slug>` segment fits the prefix budget
/// (faithful to `util/id.rs:644-660`).
///
/// First [`normalize_slug`]s the input, then fits `<prefix>-<slug>` within [`MAX_ID_PREFIX_LEN`] (=64):
/// the available slug budget is `MAX_ID_PREFIX_LEN - prefix.len() - 1` (the `-1` for the separator).
/// The slug is truncated to that budget (re-trimming a trailing hyphen). Returns the **EMPTY string as
/// the drop-signal** ("go hash-only") when the prefix alone exhausts the budget.
#[must_use]
pub fn normalize_slug_for_prefix(slug: &str, prefix: &str) -> String {
    let mut normalized = normalize_slug(slug);
    let Some(max_len) = MAX_ID_PREFIX_LEN
        .checked_sub(prefix.len())
        .and_then(|remaining| remaining.checked_sub(1))
    else {
        return String::new();
    };

    if normalized.len() > max_len {
        normalized.truncate(max_len);
        while normalized.ends_with('-') {
            normalized.pop();
        }
    }
    normalized
}

/// Generate a child id from its parent (faithful to `util/id.rs:339-341`).
///
/// Child ids have the format `<parent>.<n>` where `n` is the child number.
#[must_use]
pub fn child_id(parent_id: &str, child_number: u32) -> String {
    format!("{parent_id}.{child_number}")
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_ID_LENGTH, MAX_ID_PREFIX_LEN, MAX_SLUG_LEN, base36_encode, child_id, compute_id_hash,
        generate_id_seed, is_valid_id_format, normalize_prefix, normalize_slug,
        normalize_slug_for_prefix, optimal_hash_length, parse_id,
    };
    use chrono::Utc;
    use proptest::prelude::*;

    #[test]
    fn consts() {
        assert_eq!(MAX_ID_LENGTH, 105);
        assert_eq!(MAX_SLUG_LEN, 48);
    }

    #[test]
    fn parse_root() {
        let parsed = parse_id("ub-abc123").unwrap();
        assert_eq!(parsed.prefix, "ub");
        assert_eq!(parsed.hash, "abc123");
        assert!(parsed.is_root());
        assert_eq!(parsed.depth(), 0);
        assert_eq!(parsed.to_id_string(), "ub-abc123");
        assert_eq!(parsed.parent(), None);
    }

    #[test]
    fn parse_hyphenated_prefix() {
        let parsed = parse_id("bead-me-up-3e9").unwrap();
        assert_eq!(parsed.prefix, "bead-me-up");
        assert_eq!(parsed.hash, "3e9");
    }

    #[test]
    fn parse_child_and_grandchild() {
        let child = parse_id("ub-abc123.1").unwrap();
        assert_eq!(child.child_path, vec![1]);
        assert_eq!(child.depth(), 1);
        assert_eq!(child.parent(), Some("ub-abc123".to_string()));
        assert!(child.is_child_of("ub-abc123"));

        let grand = parse_id("ub-abc123.1.2").unwrap();
        assert_eq!(grand.child_path, vec![1, 2]);
        assert_eq!(grand.parent(), Some("ub-abc123.1".to_string()));
        assert!(grand.is_child_of("ub-abc123.1"));
    }

    #[test]
    fn parse_external_style() {
        let parsed = parse_id("external:jira-123").unwrap();
        assert_eq!(parsed.prefix, "external:jira");
        assert_eq!(parsed.hash, "123");
    }

    #[test]
    fn rejects_invalid_ids() {
        assert!(parse_id("nodash").is_err());
        assert!(parse_id("ub-").is_err());
        assert!(parse_id("ub-ABC123").is_err()); // uppercase hash
        assert!(parse_id("UB-abc").is_err()); // uppercase prefix
        assert!(parse_id("ub-abc.def").is_err()); // non-numeric child
    }

    #[test]
    fn is_valid_id_format_truth() {
        assert!(is_valid_id_format("ub-abc123"));
        assert!(is_valid_id_format("ub-abc123.1.2"));
        assert!(is_valid_id_format("ub-1"));
        // 40-char hash ok, 41 rejected.
        assert!(is_valid_id_format(&format!("ub-{}", "a".repeat(40))));
        assert!(!is_valid_id_format(&format!("ub-{}", "a".repeat(41))));
        assert!(!is_valid_id_format("invalid"));
        assert!(!is_valid_id_format("ub-ABC"));
    }

    // ================================================================================================
    // Pure generation primitives (D21) — ported from `temp/beads_rust-main/src/util/id.rs` tests.
    // ================================================================================================

    #[test]
    fn base36_encode_known_values() {
        assert_eq!(base36_encode(0), "0");
        assert_eq!(base36_encode(10), "a");
        assert_eq!(base36_encode(35), "z");
        assert_eq!(base36_encode(36), "10");
    }

    #[test]
    fn compute_id_hash_length_and_determinism() {
        let input = "test input";
        // Length is honoured.
        assert_eq!(compute_id_hash(input, 3).len(), 3);
        assert_eq!(compute_id_hash(input, 8).len(), 8);
        // Deterministic.
        assert_eq!(compute_id_hash(input, 5), compute_id_hash(input, 5));
        // Always base36-lowercase.
        assert!(
            compute_id_hash(input, 8)
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        );
    }

    #[test]
    fn compute_id_hash_takes_the_tail_not_the_head() {
        // Tail-slice direction: the length-3 hash is the LAST 3 chars of the full base36 encoding of
        // the first-8-bytes u64 — i.e. the suffix of the length-8 hash, NOT its prefix.
        let input = "deterministic seed for tail-slice check";
        let full = compute_id_hash(input, 8);
        let short = compute_id_hash(input, 3);
        assert!(
            full.ends_with(&short),
            "length-3 hash {short} must be the TAIL of the length-8 hash {full} (least-significant base36 digits)"
        );
        // The short hash equals exactly the last 3 chars of the long one.
        assert_eq!(short, full[full.len() - 3..]);
    }

    #[test]
    fn optimal_hash_length_small_and_large() {
        // Small DB → the minimum length (3).
        assert_eq!(optimal_hash_length(0), 3);
        assert_eq!(optimal_hash_length(10), 3);
        // Large DB → more characters, but bounded to 8.
        let big = optimal_hash_length(1_000_000);
        assert!((3..=8).contains(&big));
        // Monotone-ish: a huge count saturates at 8.
        assert_eq!(optimal_hash_length(usize::MAX), 8);
    }

    #[test]
    fn generate_id_seed_is_length_prefixed() {
        let now = Utc::now();
        let seed = generate_id_seed("title", Some("desc"), Some("me"), now, 0);
        assert!(seed.contains("5:title"));
        assert!(seed.contains("4:desc"));
        assert!(seed.contains("2:me"));
        assert!(seed.ends_with("1:0"));
    }

    #[test]
    fn normalize_slug_basic_cases() {
        assert_eq!(normalize_slug("survey-my-thing"), "survey-my-thing");
        assert_eq!(normalize_slug("Survey My Thing"), "survey-my-thing");
        assert_eq!(normalize_slug("---survey---"), "survey");
        assert_eq!(normalize_slug("a/b/c"), "a-b-c");
        assert_eq!(normalize_slug("a   b   c"), "a-b-c");
        assert_eq!(normalize_slug(""), "");
        assert_eq!(normalize_slug("!!!"), "");
        assert_eq!(normalize_slug("!@#$abc"), "abc");
        // Non-ASCII dropped (not transliterated).
        assert_eq!(normalize_slug("café-résumé"), "caf-r-sum");
    }

    #[test]
    fn normalize_slug_caps_at_max_len() {
        let long = "a".repeat(100);
        let out = normalize_slug(&long);
        assert_eq!(out.len(), MAX_SLUG_LEN);
        assert!(!out.ends_with('-'));
    }

    #[test]
    fn normalize_prefix_sanitizes_and_defaults_to_ub() {
        assert_eq!(normalize_prefix("  Project-Name_2!  "), "project-name_2");
        // No usable chars → the unblock default prefix.
        assert_eq!(normalize_prefix("!!!"), "ub");
        assert_eq!(normalize_prefix(""), "ub");
        // Trailing separators stripped (no double-hyphen leak).
        assert_eq!(normalize_prefix("proj-"), "proj");
    }

    /// Budget test (a): a normal slug + short prefix fits → the embedded `<prefix>-<slug>` ≤ 64 and
    /// the full `ub-<slug>-<hash>` round-trips through `parse_id`.
    #[test]
    fn normalize_slug_for_prefix_normal_slug_fits_and_round_trips() {
        let slug = normalize_slug_for_prefix("survey-my-thing", "ub");
        assert_eq!(slug, "survey-my-thing");
        // The embedded prefix segment fits the budget.
        let embedded = format!("ub-{slug}");
        assert!(embedded.len() <= MAX_ID_PREFIX_LEN);
        // The full id (with a hash suffix) parses, recovering the slug-bearing prefix.
        let now = Utc::now();
        let hash = compute_id_hash(&generate_id_seed("t", None, None, now, 0), 4);
        let id = format!("ub-{slug}-{hash}");
        let parsed = parse_id(&id).expect("slug-shaped id parses");
        assert_eq!(parsed.prefix, "ub-survey-my-thing");
        assert_eq!(parsed.hash, hash);
    }

    /// Budget test (b): a long prefix + long slug exceeds the budget → the empty drop-signal (the
    /// allocator then falls back to hash-only; NO over-budget/unparseable id).
    #[test]
    fn normalize_slug_for_prefix_exhausted_budget_drops_slug() {
        // A prefix that already consumes the whole budget leaves no room for any slug.
        let prefix = "p".repeat(MAX_ID_PREFIX_LEN);
        assert_eq!(normalize_slug_for_prefix("slug", &prefix), "");

        // A prefix that leaves only 3 chars truncates the slug to fit (and re-trims a trailing hyphen).
        let prefix = "p".repeat(MAX_ID_PREFIX_LEN - 4);
        let fitted = normalize_slug_for_prefix("abcd-efgh", &prefix);
        assert_eq!(fitted, "abc");
        assert!(format!("{prefix}-{fitted}").len() <= MAX_ID_PREFIX_LEN);
    }

    #[test]
    fn child_id_format() {
        assert_eq!(child_id("ub-abc123", 1), "ub-abc123.1");
        assert_eq!(child_id("ub-abc123.1", 2), "ub-abc123.1.2");
    }

    #[test]
    fn generated_root_and_slug_ids_round_trip_through_parse_id() {
        let now = Utc::now();
        let len = optimal_hash_length(10);
        let hash = compute_id_hash(&generate_id_seed("Title", Some("d"), Some("me"), now, 0), len);

        // ub-<hash> round-trips.
        let root = format!("ub-{hash}");
        let parsed = parse_id(&root).expect("root id parses");
        assert_eq!(parsed.prefix, "ub");
        assert_eq!(parsed.hash, hash);
        assert!(parsed.is_root());

        // ub-<slug>-<hash> round-trips.
        let slug = normalize_slug_for_prefix("my-feature", "ub");
        let with_slug = format!("ub-{slug}-{hash}");
        let parsed = parse_id(&with_slug).expect("slug id parses");
        assert_eq!(parsed.prefix, "ub-my-feature");
        assert_eq!(parsed.hash, hash);
    }

    // --- proptest: arbitrary input never panics; output shape invariants hold ---

    proptest::proptest! {
        #[test]
        fn compute_id_hash_is_always_base36_of_requested_len(seed in ".*", len in 1usize..=12) {
            let hash = compute_id_hash(&seed, len);
            prop_assert_eq!(hash.len(), len);
            prop_assert!(hash.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
        }

        #[test]
        fn compute_id_hash_tail_slice_direction(seed in ".*") {
            // The shorter hash is always the suffix of the longer one (tail-slice).
            let long = compute_id_hash(&seed, 8);
            let short = compute_id_hash(&seed, 3);
            prop_assert!(long.ends_with(&short));
        }

        #[test]
        fn normalize_slug_never_panics_and_is_well_formed(s in ".*") {
            let out = normalize_slug(&s);
            prop_assert!(out.len() <= MAX_SLUG_LEN);
            prop_assert!(!out.starts_with('-'));
            prop_assert!(!out.ends_with('-'));
            prop_assert!(out.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
        }

        #[test]
        fn normalize_prefix_never_panics_and_is_nonempty(s in ".*") {
            let out = normalize_prefix(&s);
            prop_assert!(!out.is_empty());
            prop_assert!(out.len() <= MAX_ID_PREFIX_LEN);
        }

        #[test]
        fn generate_id_seed_never_panics(title in ".*", nonce in any::<u32>()) {
            let _ = generate_id_seed(&title, None, None, Utc::now(), nonce);
        }
    }
}
