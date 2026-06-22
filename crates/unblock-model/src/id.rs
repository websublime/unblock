//! Issue-ID format parsing and validation (pure; no I/O).
//!
//! This ports only the **format** side of the classic `bd` id scheme — `<prefix>-<hash>` with an
//! optional dot-separated child path (`<prefix>-<hash>.1.2`). Stateful id **generation** and
//! **resolution** (`IdGenerator` / `IdResolver`) are storage-coupled concerns and live in
//! `unblock-storage`/`unblock-engine`, not in this pure model crate.

use unblock_error::ModelError;

/// Maximum length of an id prefix.
pub const MAX_ID_PREFIX_LEN: usize = 64;
/// Maximum length of an id hash portion.
pub const MAX_ID_HASH_LEN: usize = 40;
/// Maximum total id length (`prefix` + `-` + `hash`).
pub const MAX_ID_LENGTH: usize = MAX_ID_PREFIX_LEN + 1 + MAX_ID_HASH_LEN;

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

#[cfg(test)]
mod tests {
    use super::{MAX_ID_LENGTH, is_valid_id_format, parse_id};

    #[test]
    fn consts() {
        assert_eq!(MAX_ID_LENGTH, 105);
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
}
