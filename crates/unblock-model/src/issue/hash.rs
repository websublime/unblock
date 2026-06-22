//! Canonical content hashing for issue deduplication and sync (spine §1.8).
//!
//! SHA-256 over a stable ordered, null-separated field set. The byte stream is **frozen** and
//! matches classic Go `bd` for export/import compatibility (FR-26 / D16 idempotent one-shot `bd`
//! import) — in particular it appends a 17-field Go-bd zero-value padding tail after `is_template`
//! (spine §1.8, Q4 = KEEP). The golden hash is locked by `tests/golden_hash.rs`.

use std::fmt::Write;

use sha2::{Digest, Sha256};

use super::Issue;
use crate::enums::{IssueType, Priority, Status};

/// Lowercase hex encoding for digest bytes.
///
/// `sha2` 0.11 no longer implements `LowerHex` on its digest array, so we format bytes directly.
/// Writing to a `String` is infallible, so the `write!` result is discarded (no `expect`/panic).
#[must_use]
pub fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(&mut s, "{b:02x}");
    }
    s
}

/// Compute the SHA-256 content hash for an issue (spine §1.8).
///
/// Fields included, in order (each appended as `bytes ++ 0x00`):
/// `title, description, design, acceptance_criteria, notes, status.as_str(), priority.0,
/// issue_type.as_str(), assignee, owner, created_by, external_ref, source_system, pinned,
/// is_template`, followed by the frozen 17-field Go-bd zero-value padding tail.
///
/// Excluded: `id`, `content_hash` (circular), relations (labels/deps/comments), all timestamps,
/// tombstone fields, `estimated_minutes`, `due_at`, `defer_until`, `close_reason`,
/// `closed_by_session`.
#[must_use]
pub fn content_hash(issue: &Issue) -> String {
    content_hash_from_parts(
        &issue.title,
        issue.description.as_deref(),
        issue.design.as_deref(),
        issue.acceptance_criteria.as_deref(),
        issue.notes.as_deref(),
        &issue.status,
        &issue.priority,
        &issue.issue_type,
        issue.assignee.as_deref(),
        issue.owner.as_deref(),
        issue.created_by.as_deref(),
        issue.external_ref.as_deref(),
        issue.source_system.as_deref(),
        issue.pinned,
        issue.is_template,
    )
}

/// Compute a content hash from raw components (for import/validation).
///
/// This is the canonical byte-stream implementation (spine §1.8). See [`content_hash`] for the
/// field set; the 17-field Go-bd zero-value padding tail is appended verbatim after `is_template`.
#[must_use]
#[allow(clippy::too_many_arguments)] // mirrors the frozen Go-bd field order; a struct would obscure it.
pub fn content_hash_from_parts(
    title: &str,
    description: Option<&str>,
    design: Option<&str>,
    acceptance_criteria: Option<&str>,
    notes: Option<&str>,
    status: &Status,
    priority: &Priority,
    issue_type: &IssueType,
    assignee: Option<&str>,
    owner: Option<&str>,
    created_by: Option<&str>,
    external_ref: Option<&str>,
    source_system: Option<&str>,
    pinned: bool,
    is_template: bool,
) -> String {
    let mut writer = HashFieldWriter::new();

    writer.field(title);
    writer.field_opt(description);
    writer.field_opt(design);
    writer.field_opt(acceptance_criteria);
    writer.field_opt(notes);
    writer.field(status.as_str());
    writer.field(&priority.0.to_string());
    writer.field(issue_type.as_str());
    writer.field_opt(assignee);
    writer.field_opt(owner);
    writer.field_opt(created_by);
    writer.field_opt(external_ref);
    writer.field_opt(source_system);
    writer.field_flag(pinned, "pinned");
    writer.field_flag(is_template, "template");

    // Go bd hashes several newer fields that Rust does not model. Hash their Go zero values so a
    // Rust content_hash stays byte-for-byte compatible with a bd-exported hash (FR-26 / D16).
    // FROZEN — changing this tail breaks `bd` import idempotency (spine §1.8, golden-pinned).
    writer.field(""); // quality_score nil
    writer.field_flag(false, "crystallizes");
    writer.field(""); // await_type
    writer.field(""); // await_id
    writer.field("0"); // timeout duration
    writer.field(""); // holder
    writer.field(""); // hook_bead
    writer.field(""); // role_bead
    writer.field(""); // agent_state
    writer.field(""); // role_type
    writer.field(""); // rig
    writer.field(""); // mol_type
    writer.field(""); // work_type
    writer.field(""); // event_kind
    writer.field(""); // actor
    writer.field(""); // target
    writer.field(""); // payload

    writer.finalize()
}

/// Streaming SHA-256 field writer: each field is `bytes ++ 0x00`.
struct HashFieldWriter {
    hasher: Sha256,
}

impl HashFieldWriter {
    fn new() -> Self {
        Self {
            hasher: Sha256::new(),
        }
    }

    fn field(&mut self, value: &str) {
        self.hasher.update(value.as_bytes());
        self.hasher.update(b"\x00");
    }

    fn field_opt(&mut self, value: Option<&str>) {
        self.field(value.unwrap_or(""));
    }

    fn field_flag(&mut self, value: bool, label: &str) {
        self.field(if value { label } else { "" });
    }

    fn finalize(self) -> String {
        hex_encode(&self.hasher.finalize())
    }
}

#[cfg(test)]
mod tests {
    use super::{content_hash, content_hash_from_parts, hex_encode};
    use crate::enums::{Priority, Status};
    use crate::issue::Issue;
    use chrono::{TimeZone, Utc};

    fn make_test_issue() -> Issue {
        Issue {
            id: "ub-test123".to_string(),
            title: "Test Issue".to_string(),
            description: Some("A test description".to_string()),
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            ..Issue::default()
        }
    }

    #[test]
    fn content_hash_deterministic() {
        let issue = make_test_issue();
        assert_eq!(content_hash(&issue), content_hash(&issue));
    }

    #[test]
    fn content_hash_is_64_lowercase_hex() {
        let hash = content_hash(&make_test_issue());
        assert_eq!(hash.len(), 64);
        assert!(
            hash.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn content_hash_changes_with_included_fields() {
        let base = content_hash(&make_test_issue());

        let mut t = make_test_issue();
        t.title = "Different".to_string();
        assert_ne!(base, content_hash(&t));

        let mut p = make_test_issue();
        p.pinned = true;
        assert_ne!(base, content_hash(&p));

        let mut c = make_test_issue();
        c.created_by = Some("tester@example.com".to_string());
        assert_ne!(base, content_hash(&c));

        let mut s = make_test_issue();
        s.source_system = Some("imported".to_string());
        assert_ne!(base, content_hash(&s));
    }

    #[test]
    fn content_hash_ignores_excluded_fields() {
        let base = content_hash(&make_test_issue());

        let mut ts = make_test_issue();
        ts.updated_at = Utc.with_ymd_and_hms(2027, 6, 6, 6, 6, 6).unwrap();
        assert_eq!(base, content_hash(&ts));

        let mut est = make_test_issue();
        est.estimated_minutes = Some(99);
        assert_eq!(base, content_hash(&est));

        let mut due = make_test_issue();
        due.due_at = Some(Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap());
        assert_eq!(base, content_hash(&due));
    }

    #[test]
    fn from_parts_equals_compute() {
        let issue = make_test_issue();
        let from_parts = content_hash_from_parts(
            &issue.title,
            issue.description.as_deref(),
            issue.design.as_deref(),
            issue.acceptance_criteria.as_deref(),
            issue.notes.as_deref(),
            &issue.status,
            &issue.priority,
            &issue.issue_type,
            issue.assignee.as_deref(),
            issue.owner.as_deref(),
            issue.created_by.as_deref(),
            issue.external_ref.as_deref(),
            issue.source_system.as_deref(),
            issue.pinned,
            issue.is_template,
        );
        assert_eq!(content_hash(&issue), from_parts);
    }

    #[test]
    fn priority_hashed_as_decimal_not_label() {
        // priority.0 is hashed as the decimal int ("2"), never "P2".
        let mut a = make_test_issue();
        a.priority = Priority::MEDIUM;
        let mut b = make_test_issue();
        b.priority = Priority::CRITICAL;
        assert_ne!(content_hash(&a), content_hash(&b));
    }

    #[test]
    fn status_custom_changes_hash() {
        let mut a = make_test_issue();
        a.status = Status::Custom("review".to_string());
        let mut b = make_test_issue();
        b.status = Status::Custom("triage".to_string());
        assert_ne!(content_hash(&a), content_hash(&b));
    }

    #[test]
    fn hex_encode_known_vectors() {
        assert_eq!(hex_encode(&[]), "");
        assert_eq!(hex_encode(&[0x00]), "00");
        assert_eq!(hex_encode(&[0x0a]), "0a");
        assert_eq!(hex_encode(&[0xff]), "ff");
        assert_eq!(hex_encode(&[0x80, 0xff, 0xfe, 0x7f]), "80fffe7f");
    }

    #[test]
    fn hex_encode_length_invariant() {
        for len in [0usize, 1, 16, 32, 64] {
            let bytes: Vec<u8> = (0..len)
                .map(|i| u8::try_from(i % 256).unwrap_or(0))
                .collect();
            assert_eq!(hex_encode(&bytes).len(), bytes.len() * 2);
        }
    }

    #[test]
    fn hex_encode_matches_sha256_digest() {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"hello world");
        let hex = hex_encode(&hasher.finalize());
        assert_eq!(
            hex,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }
}
