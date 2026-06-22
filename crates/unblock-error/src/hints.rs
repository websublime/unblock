//! Agent self-correction helpers (FR-11): Levenshtein "did you mean" id suggestions and
//! status/type/priority intent detection. Pure, no I/O — callers pass the candidate set; this
//! crate never queries storage. Ported from the original `error/structured.rs`.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// Detailed priority hint (full mapping).
pub const PRIORITY_DETAIL_HINT: &str =
    "Priority must be 0-4 (or P0-P4): 0=critical, 1=high, 2=medium, 3=low, 4=backlog";
/// Short priority hint.
pub const PRIORITY_SHORT_HINT: &str = "Priority must be 0-4 (0=critical, 4=backlog).";
/// Valid status values hint.
pub const VALID_STATUS_HINT: &str =
    "Valid statuses: open, in_progress, blocked, deferred, draft, closed, tombstone, pinned";
/// Valid issue-type values hint.
pub const VALID_TYPE_HINT: &str = "Valid types: task, bug, feature, epic, chore, docs, question";

/// Maximum Levenshtein distance for an id to be offered as a suggestion.
pub const MAX_SUGGESTION_DISTANCE: usize = 3;

/// Valid status values (O(1) membership lookup).
static VALID_STATUSES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "open",
        "in_progress",
        "blocked",
        "deferred",
        "draft",
        "closed",
        "tombstone",
        "pinned",
    ]
    .into_iter()
    .collect()
});

/// Valid issue-type values (O(1) membership lookup).
static VALID_TYPES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    ["task", "bug", "feature", "epic", "chore", "docs", "question"]
        .into_iter()
        .collect()
});

/// Status synonyms → canonical status, for intent detection.
static STATUS_SYNONYMS: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    [
        ("done", "closed"),
        ("complete", "closed"),
        ("completed", "closed"),
        ("finished", "closed"),
        ("resolved", "closed"),
        ("wontfix", "closed"),
        ("wip", "in_progress"),
        ("working", "in_progress"),
        ("active", "in_progress"),
        ("started", "in_progress"),
        ("new", "open"),
        ("todo", "open"),
        ("pending", "open"),
        ("waiting", "blocked"),
        ("hold", "deferred"),
        ("later", "deferred"),
        ("postponed", "deferred"),
    ]
    .into_iter()
    .collect()
});

/// Type synonyms → canonical issue type, for intent detection.
static TYPE_SYNONYMS: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    [
        ("story", "feature"),
        ("enhancement", "feature"),
        ("improvement", "feature"),
        ("issue", "bug"),
        ("defect", "bug"),
        ("problem", "bug"),
        ("ticket", "task"),
        ("item", "task"),
        ("work", "task"),
        ("documentation", "docs"),
        ("doc", "docs"),
        ("readme", "docs"),
        ("cleanup", "chore"),
        ("refactor", "chore"),
        ("maintenance", "chore"),
        ("parent", "epic"),
        ("initiative", "epic"),
        ("ask", "question"),
        ("help", "question"),
    ]
    .into_iter()
    .collect()
});

/// Priority synonyms → canonical priority digit, for intent detection.
static PRIORITY_SYNONYMS: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    [
        ("critical", "0"),
        ("crit", "0"),
        ("urgent", "0"),
        ("highest", "0"),
        ("high", "1"),
        ("important", "1"),
        ("medium", "2"),
        ("normal", "2"),
        ("default", "2"),
        ("low", "3"),
        ("minor", "3"),
        ("backlog", "4"),
        ("lowest", "4"),
        ("trivial", "4"),
    ]
    .into_iter()
    .collect()
});

/// Detect the status the user likely meant (case-insensitive direct match → synonym → prefix).
///
/// # Examples
///
/// ```
/// use unblock_error::detect_status_intent;
/// assert_eq!(detect_status_intent("done"), Some("closed"));
/// assert_eq!(detect_status_intent("op"), Some("open")); // prefix
/// assert_eq!(detect_status_intent("xyz"), None);
/// ```
#[must_use]
pub fn detect_status_intent(input: &str) -> Option<&'static str> {
    let lower = input.to_lowercase();

    if let Some(known) = VALID_STATUSES.get(lower.as_str()) {
        return Some(known);
    }
    if let Some(&canonical) = STATUS_SYNONYMS.get(lower.as_str()) {
        return Some(canonical);
    }
    if lower.is_empty() {
        return None;
    }
    VALID_STATUSES
        .iter()
        .find(|status| status.starts_with(&lower))
        .copied()
}

/// Detect the issue type the user likely meant (case-insensitive direct match → synonym → prefix).
///
/// # Examples
///
/// ```
/// use unblock_error::detect_type_intent;
/// assert_eq!(detect_type_intent("story"), Some("feature"));
/// assert_eq!(detect_type_intent("xyz"), None);
/// ```
#[must_use]
pub fn detect_type_intent(input: &str) -> Option<&'static str> {
    let lower = input.to_lowercase();

    if let Some(known) = VALID_TYPES.get(lower.as_str()) {
        return Some(known);
    }
    if let Some(&canonical) = TYPE_SYNONYMS.get(lower.as_str()) {
        return Some(canonical);
    }
    if lower.is_empty() {
        return None;
    }
    VALID_TYPES
        .iter()
        .find(|t| t.starts_with(&lower))
        .copied()
}

/// Detect the priority the user likely meant (digits, `P0`–`P4`, or synonyms).
///
/// # Examples
///
/// ```
/// use unblock_error::detect_priority_intent;
/// assert_eq!(detect_priority_intent("high"), Some("1"));
/// assert_eq!(detect_priority_intent("P2"), Some("2"));
/// assert_eq!(detect_priority_intent("p5"), None);
/// ```
#[must_use]
pub fn detect_priority_intent(input: &str) -> Option<&'static str> {
    let lower = input.to_lowercase();

    if let Some(digit) = digit_str(lower.as_str()) {
        return Some(digit);
    }

    // `P0`–`P4` format.
    if lower.len() == 2 {
        let mut chars = lower.chars();
        if chars.next() == Some('p')
            && let Some(digit) = chars.next()
            && digit.is_ascii_digit()
            && digit <= '4'
        {
            return digit_str(&digit.to_string());
        }
    }

    PRIORITY_SYNONYMS.get(lower.as_str()).copied()
}

/// Map a single `"0".."4"` digit string to its `'static` form (`None` otherwise).
fn digit_str(s: &str) -> Option<&'static str> {
    match s {
        "0" => Some("0"),
        "1" => Some("1"),
        "2" => Some("2"),
        "3" => Some("3"),
        "4" => Some("4"),
        _ => None,
    }
}

/// The Levenshtein edit distance between two strings (counted in `char`s, not bytes).
///
/// # Examples
///
/// ```
/// use unblock_error::levenshtein_distance;
/// assert_eq!(levenshtein_distance("kitten", "sitting"), 3);
/// assert_eq!(levenshtein_distance("abc", "abc"), 0);
/// ```
#[must_use]
pub fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let a_len = a_chars.len();
    let b_len = b_chars.len();

    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    let mut matrix = vec![vec![0usize; b_len + 1]; a_len + 1];

    for (i, row) in matrix.iter_mut().enumerate().take(a_len + 1) {
        row[0] = i;
    }
    for (j, item) in matrix[0].iter_mut().enumerate().take(b_len + 1) {
        *item = j;
    }

    for (i, a_char) in a_chars.iter().enumerate() {
        for (j, b_char) in b_chars.iter().enumerate() {
            let cost = usize::from(a_char != b_char);
            matrix[i + 1][j + 1] = std::cmp::min(
                std::cmp::min(matrix[i][j + 1] + 1, matrix[i + 1][j] + 1),
                matrix[i][j] + cost,
            );
        }
    }

    matrix[a_len][b_len]
}

/// Find ids similar to `searched`, ranked by Levenshtein distance.
///
/// Candidates with distance greater than [`MAX_SUGGESTION_DISTANCE`] are filtered out **before**
/// the `max` cap is applied; the surviving candidates are sorted deterministically (by distance,
/// then lexicographically) and truncated to `max`.
///
/// # Examples
///
/// ```
/// use unblock_error::find_similar_ids;
/// let existing = vec!["ub-abc123".to_string(), "ub-abc124".to_string(), "ub-xyz789".to_string()];
/// let hits = find_similar_ids("ub-abc12", &existing, 3);
/// assert!(hits.contains(&"ub-abc123".to_string()));
/// assert!(!hits.contains(&"ub-xyz789".to_string()));
/// ```
#[must_use]
pub fn find_similar_ids(searched: &str, existing: &[String], max: usize) -> Vec<String> {
    let mut candidates: Vec<(usize, &str)> = existing
        .iter()
        .map(|id| (levenshtein_distance(searched, id), id.as_str()))
        .filter(|(dist, _)| *dist <= MAX_SUGGESTION_DISTANCE)
        .collect();

    candidates.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));

    candidates
        .into_iter()
        .take(max)
        .map(|(_, id)| id.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        detect_priority_intent, detect_status_intent, detect_type_intent, find_similar_ids,
        levenshtein_distance,
    };

    #[test]
    fn levenshtein_table() {
        assert_eq!(levenshtein_distance("", ""), 0);
        assert_eq!(levenshtein_distance("abc", "abc"), 0);
        assert_eq!(levenshtein_distance("abc", "abd"), 1);
        assert_eq!(levenshtein_distance("abc", "abcd"), 1);
        assert_eq!(levenshtein_distance("kitten", "sitting"), 3);
    }

    #[test]
    fn find_similar_ranks_and_filters() {
        let existing = vec![
            "ub-abc123".to_string(),
            "ub-xyz789".to_string(),
            "ub-abc124".to_string(),
            "ub-def456".to_string(),
        ];
        let hits = find_similar_ids("ub-abc12", &existing, 3);
        assert!(!hits.is_empty());
        assert!(hits.contains(&"ub-abc123".to_string()));
        assert!(hits.contains(&"ub-abc124".to_string()));
        // far-away ids are filtered by the distance cap
        assert!(!hits.contains(&"ub-xyz789".to_string()));
    }

    #[test]
    fn find_similar_respects_max_after_distance_filter() {
        let existing = vec![
            "ub-aaa".to_string(),
            "ub-aab".to_string(),
            "ub-aac".to_string(),
        ];
        let hits = find_similar_ids("ub-aaa", &existing, 1);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0], "ub-aaa");
    }

    #[test]
    fn find_similar_empty_on_no_candidates() {
        let existing = vec!["completely-different".to_string()];
        assert!(find_similar_ids("ub-abc", &existing, 3).is_empty());
    }

    #[test]
    fn detect_status_vectors() {
        assert_eq!(detect_status_intent("done"), Some("closed"));
        assert_eq!(detect_status_intent("wip"), Some("in_progress"));
        assert_eq!(detect_status_intent("OPEN"), Some("open"));
        assert_eq!(detect_status_intent("draft"), Some("draft"));
        assert_eq!(detect_status_intent("op"), Some("open"));
        assert_eq!(detect_status_intent("xyz"), None);
    }

    #[test]
    fn detect_type_vectors() {
        assert_eq!(detect_type_intent("story"), Some("feature"));
        assert_eq!(detect_type_intent("defect"), Some("bug"));
        assert_eq!(detect_type_intent("TASK"), Some("task"));
        assert_eq!(detect_type_intent("docs"), Some("docs"));
        assert_eq!(detect_type_intent("xyz"), None);
    }

    #[test]
    fn detect_priority_vectors() {
        assert_eq!(detect_priority_intent("high"), Some("1"));
        assert_eq!(detect_priority_intent("critical"), Some("0"));
        assert_eq!(detect_priority_intent("P2"), Some("2"));
        assert_eq!(detect_priority_intent("p3"), Some("3"));
        assert_eq!(detect_priority_intent("2"), Some("2"));
        assert_eq!(detect_priority_intent("xyz"), None);
    }

    #[test]
    fn detect_priority_all_digits_and_p_prefixed() {
        for (input, expected) in [("0", "0"), ("1", "1"), ("2", "2"), ("3", "3"), ("4", "4")] {
            assert_eq!(detect_priority_intent(input), Some(expected));
        }
        for (input, expected) in [
            ("p0", "0"),
            ("P0", "0"),
            ("p4", "4"),
            ("P4", "4"),
        ] {
            assert_eq!(detect_priority_intent(input), Some(expected), "input: {input}");
        }
    }

    #[test]
    fn detect_priority_rejects_malformed() {
        for bad in ["p5", "P5", "px", "p10", "5", "9", "", "p", "P"] {
            assert_eq!(detect_priority_intent(bad), None, "input: {bad}");
        }
    }
}
