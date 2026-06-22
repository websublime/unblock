//! Byte-driven builders for typed values + the `normalize_issue` repair pass.
//!
//! `arbitrary_issue` produces an `Issue` from raw bytes that explores the **typed** surface
//! (complementing the raw-bytes JSON targets). `normalize_issue` is the key tool: it **repairs** any
//! `Issue` into one that the model `IssueValidator` always accepts, clamping to the validator's own
//! bounds (the now-`pub` consts). It is **idempotent** — `normalize(normalize(x)) == normalize(x)` —
//! so a target can normalize once and rely on a stable validatable value.

use chrono::{DateTime, TimeZone, Utc};

use unblock_model::{
    ACTOR_MAX_CHARS, CUSTOM_VARIANT_MAX_CHARS, ESTIMATED_MINUTES_MAX, EXTERNAL_REF_MAX_CHARS,
    ISSUE_LABEL_MAX_COUNT, Issue, IssueType, LABEL_MAX_LEN, Priority, Status, TITLE_MAX_CHARS,
};

use crate::cursor::{ByteCursor, CursorExt};

/// A fixed epoch for deterministic builders (`created_at`).
fn epoch() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
        .single()
        .unwrap_or_else(Utc::now)
}

/// Build a single `Issue` from the cursor (typed-surface exploration).
///
/// The result is **not** guaranteed valid — pass it through [`normalize_issue`] when a validatable
/// issue is needed (e.g. before `create_issue` or the field-scoped hash assertions).
#[must_use]
pub fn arbitrary_issue(cursor: &mut ByteCursor) -> Issue {
    let prefix = cursor.prefix();
    let hash = cursor.text(8);
    let created = epoch();
    Issue {
        id: format!("{prefix}-{hash}"),
        title: cursor.text(40),
        description: cursor.optional_text(30),
        design: cursor.optional_text(20),
        acceptance_criteria: cursor.optional_text(20),
        notes: cursor.optional_text(20),
        status: cursor.status(),
        priority: Priority(i32::from(cursor.next_byte())),
        issue_type: cursor.issue_type(),
        assignee: cursor.optional_text(20),
        owner: cursor.optional_text(20),
        created_by: cursor.optional_text(20),
        estimated_minutes: if cursor.next_bool() {
            // Reinterpret the bytes as a possibly-negative i32 (to exercise normalize's clamp path);
            // an explicit bit-reinterpretation, not a wrapping numeric cast.
            Some(i32::from_ne_bytes(cursor.next_u32().to_ne_bytes()))
        } else {
            None
        },
        external_ref: cursor.optional_text(20),
        source_system: cursor.optional_text(20),
        pinned: cursor.next_bool(),
        is_template: cursor.next_bool(),
        labels: (0..cursor.next_usize(6)).map(|_| cursor.text(12)).collect(),
        created_at: created,
        updated_at: created,
        ..Issue::default()
    }
}

/// Build `n` issues from the cursor (each via [`arbitrary_issue`]).
#[must_use]
pub fn arbitrary_issues(cursor: &mut ByteCursor, n: usize) -> Vec<Issue> {
    (0..n).map(|_| arbitrary_issue(cursor)).collect()
}

/// Repair an arbitrary `Issue` into one the model `IssueValidator` **always** accepts.
///
/// Clamps every field to the validator's own bound (the now-`pub` model consts), so the post-
/// condition `IssueValidator::validate(&normalize_issue(x)).is_ok()` holds for ANY input, and the
/// pass is **idempotent**. See the unit test below, which sweeps a cursor asserting both properties.
#[must_use]
pub fn normalize_issue(mut issue: Issue) -> Issue {
    // --- id: force a syntactically valid `<lc-prefix>-<base36>` ---
    issue.id = repair_id(&issue.id);

    // --- title: non-empty, <= TITLE_MAX_CHARS chars, NUL-free ---
    issue.title = repair_title(&issue.title);

    // --- body text fields: NUL-free (unbounded otherwise) ---
    issue.description = issue.description.map(|s| strip_nul(&s));
    issue.design = issue.design.map(|s| strip_nul(&s));
    issue.acceptance_criteria = issue.acceptance_criteria.map(|s| strip_nul(&s));
    issue.notes = issue.notes.map(|s| strip_nul(&s));

    // --- status / issue_type: NUL-free; Custom <= CUSTOM_VARIANT_MAX_CHARS ---
    issue.status = repair_status(issue.status);
    issue.issue_type = repair_issue_type(issue.issue_type);

    // --- actor-style fields: <= ACTOR_MAX_CHARS chars, NUL-free ---
    issue.assignee = issue
        .assignee
        .map(|s| clamp_chars(&strip_nul(&s), ACTOR_MAX_CHARS));
    issue.owner = issue
        .owner
        .map(|s| clamp_chars(&strip_nul(&s), ACTOR_MAX_CHARS));
    issue.created_by = issue
        .created_by
        .map(|s| clamp_chars(&strip_nul(&s), ACTOR_MAX_CHARS));
    issue.source_system = issue
        .source_system
        .map(|s| clamp_chars(&strip_nul(&s), ACTOR_MAX_CHARS));

    // --- external_ref: <= EXTERNAL_REF_MAX_CHARS chars, NUL-free, NO whitespace ---
    issue.external_ref = issue.external_ref.map(|s| repair_external_ref(&s));

    // --- priority: 0..=4 ---
    issue.priority = Priority(issue.priority.0.clamp(0, 4));

    // --- estimated_minutes: 0..=ESTIMATED_MINUTES_MAX ---
    issue.estimated_minutes = issue
        .estimated_minutes
        .map(|m| m.clamp(0, ESTIMATED_MINUTES_MAX));

    // --- labels: deduped + sorted + charset-filtered + bounded length + bounded count ---
    issue.labels = repair_labels(issue.labels);

    // --- timestamps: created_at <= updated_at ---
    if issue.updated_at < issue.created_at {
        issue.updated_at = issue.created_at;
    }

    // --- closed-state coherence ---
    repair_closed_state(&mut issue);

    issue
}

/// Repair an id to a valid `<lc-prefix>-<base36>` (always parses; idempotent for already-valid ids).
fn repair_id(raw: &str) -> String {
    // Already valid → pass through unchanged (this is what makes `normalize_issue` idempotent: a
    // second pass over a repaired id must not reshape it).
    if unblock_model::is_valid_id_format(raw) {
        return raw.to_string();
    }
    // Keep only id-charset-safe lowercase chars; rebuild a guaranteed-valid id from the survivors.
    let prefix: String = raw
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .take(8)
        .collect();
    let prefix = if prefix.is_empty() {
        "ub".to_string()
    } else {
        prefix
    };
    // A short base36 hash from the raw bytes (always non-empty, lowercase-alnum).
    let mut hash: String = raw
        .bytes()
        .filter(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        .map(|b| b as char)
        .take(8)
        .collect();
    if hash.is_empty() {
        hash.push('1');
    }
    format!("{prefix}-{hash}")
}

/// Repair a title: trim NUL, ensure non-empty (non-whitespace), clamp to `TITLE_MAX_CHARS` chars.
fn repair_title(raw: &str) -> String {
    let cleaned = strip_nul(raw);
    let cleaned = if cleaned.trim().is_empty() {
        "issue".to_string()
    } else {
        cleaned
    };
    clamp_chars(&cleaned, TITLE_MAX_CHARS)
}

/// Repair `external_ref`: NUL-free, whitespace-free, clamped to `EXTERNAL_REF_MAX_CHARS` chars.
fn repair_external_ref(raw: &str) -> String {
    let no_ws: String = strip_nul(raw)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    clamp_chars(&no_ws, EXTERNAL_REF_MAX_CHARS)
}

/// Clamp a `Status::Custom` payload to the variant cap; known variants pass through.
fn repair_status(status: Status) -> Status {
    match status {
        Status::Custom(value) => {
            let cleaned = clamp_chars(&strip_nul(&value), CUSTOM_VARIANT_MAX_CHARS);
            // A cleared custom value would be an empty wire string; fall back to a known variant.
            if cleaned.is_empty() {
                Status::Open
            } else {
                Status::Custom(cleaned)
            }
        }
        other => other,
    }
}

/// Clamp an `IssueType::Custom` payload to the variant cap; known variants pass through.
fn repair_issue_type(issue_type: IssueType) -> IssueType {
    match issue_type {
        IssueType::Custom(value) => {
            let cleaned = clamp_chars(&strip_nul(&value), CUSTOM_VARIANT_MAX_CHARS);
            if cleaned.is_empty() {
                IssueType::Task
            } else {
                IssueType::Custom(cleaned)
            }
        }
        other => other,
    }
}

/// Repair the label set: charset-filter each label, drop empties, clamp length, dedup, sort, and cap
/// the count.
fn repair_labels(labels: Vec<String>) -> Vec<String> {
    let mut cleaned: Vec<String> = labels
        .into_iter()
        .filter_map(|label| {
            let filtered: String = label
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':'))
                .collect();
            // LabelValidator measures length in bytes; the charset is ASCII so bytes == chars.
            let bounded: String = filtered.chars().take(LABEL_MAX_LEN).collect();
            if bounded.is_empty() {
                None
            } else {
                Some(bounded)
            }
        })
        .collect();
    cleaned.sort();
    cleaned.dedup();
    cleaned.truncate(ISSUE_LABEL_MAX_COUNT);
    cleaned
}

/// Force closed-state coherence (the three validator rules around `closed_at`).
fn repair_closed_state(issue: &mut Issue) {
    match issue.status {
        Status::Closed => {
            // Closed requires closed_at, and closed_at >= created_at.
            let at = issue.closed_at.unwrap_or(issue.created_at);
            issue.closed_at = Some(at.max(issue.created_at));
        }
        Status::Tombstone => {
            // Tombstone may carry closed_at, but if present it must be >= created_at.
            if let Some(at) = issue.closed_at {
                issue.closed_at = Some(at.max(issue.created_at));
            }
        }
        _ => {
            // Non-terminal status must not carry closed_at.
            issue.closed_at = None;
        }
    }
}

/// Drop NUL bytes from a string (the validator rejects them in every text field).
fn strip_nul(s: &str) -> String {
    s.replace('\0', "")
}

/// Clamp a string to at most `max_chars` `char`s.
fn clamp_chars(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::{arbitrary_issue, normalize_issue};
    use crate::cursor::ByteCursor;
    use unblock_model::IssueValidator;

    /// Sweep a cursor: `validate(normalize(x))` is always Ok AND `normalize` is idempotent.
    #[test]
    fn normalize_always_validates_and_is_idempotent() {
        // A spread of byte patterns to exercise the repair branches (empties, all-0xff Custom tails,
        // NUL injection, oversized fields).
        let seeds: &[Vec<u8>] = &[
            vec![],
            vec![0u8; 1],
            vec![0xffu8; 256],
            (0u8..=255).collect(),
            b"ub-abc123\0\0title with nul\0".to_vec(),
            std::iter::repeat_n(0x41u8, 1024).collect(), // long 'A' run
            (0u8..200).map(|n| n.wrapping_mul(7)).collect(),
        ];

        for seed in seeds {
            let mut cursor = ByteCursor::new(seed);
            // A handful of issues per seed (the cursor advances each call).
            for _ in 0..4 {
                let raw = arbitrary_issue(&mut cursor);
                let normalized = normalize_issue(raw);
                assert!(
                    IssueValidator::validate(&normalized).is_ok(),
                    "normalize must always produce a validatable issue: {normalized:?}"
                );
                let twice = normalize_issue(normalized.clone());
                assert_eq!(
                    normalized, twice,
                    "normalize must be idempotent for {normalized:?}"
                );
            }
        }
    }
}
