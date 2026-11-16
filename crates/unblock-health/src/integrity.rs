//! The D58 clause (3) split of `integrity_check` rows into index-only and page-level corruption.
//!
//! A row is index-only when, after one leading `*** in database main ***` line is stripped, it
//! fully matches one of the four shapes `SQLite` 3.45.1 emits when a table row and an index
//! disagree, and its index name is one the probe found in `sqlite_schema`. Every other row is
//! page-level corruption. The rule is fail-safe, so a reworded message classifies `Unsafe`.

use crate::anomaly::AnomalyClass;

/// The line `SQLite` puts in front of a multi-line b-tree row.
const MAIN_DATABASE_LINE: &str = "*** in database main ***\n";

/// Classify one `integrity_check` row.
///
/// Returns [`AnomalyClass::IndexCorruption`] for an index-only row naming an index in
/// `index_names`, and [`AnomalyClass::PageCorruption`] for every other row.
///
/// ```
/// use unblock_health::classify_integrity_row;
///
/// let names = vec!["idx_issues_status".to_string()];
/// let class = classify_integrity_row("row 7 missing from index idx_issues_status", &names);
/// assert_eq!(class.code(), "index_corruption");
///
/// let class = classify_integrity_row("Page 7: never used", &names);
/// assert_eq!(class.code(), "page_corruption");
/// ```
#[must_use]
pub fn classify_integrity_row(row: &str, index_names: &[String]) -> AnomalyClass {
    let message = row.strip_prefix(MAIN_DATABASE_LINE).unwrap_or(row);
    match index_only_target(message) {
        Some(index) if index_names.iter().any(|name| name == index) => {
            AnomalyClass::IndexCorruption {
                index: index.to_string(),
                row: message.to_string(),
            }
        }
        _ => AnomalyClass::PageCorruption {
            row: message.to_string(),
        },
    }
}

/// The index name of a message that fully matches one of the four index-only shapes.
fn index_only_target(message: &str) -> Option<&str> {
    let index = if let Some(rest) = message.strip_prefix("wrong # of entries in index ") {
        rest
    } else if let Some(rest) = message.strip_prefix("rowid not at end-of-record for row ") {
        after_row_number(rest, " of index ")?
    } else if let Some(rest) = message.strip_prefix("row ") {
        after_row_number(rest, " missing from index ")
            .or_else(|| after_row_number(rest, " values differ from index "))?
    } else {
        return None;
    };
    (!index.is_empty()).then_some(index)
}

/// Strip a run of one or more ASCII digits and then `infix`, returning what follows.
fn after_row_number<'a>(text: &'a str, infix: &str) -> Option<&'a str> {
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    text.get(digits..)?.strip_prefix(infix)
}
