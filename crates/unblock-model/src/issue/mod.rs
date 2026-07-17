//! The [`Issue`] entity (spine §1.6) and its inherent behaviour, split per concern.
//!
//! The struct lives here; `content_hash`, `sync_equals`, and the tombstone helpers are inherent
//! `impl Issue` blocks in the sibling modules (`hash`, `sync_eq`, `tombstone`). The free hash
//! functions are re-exported through this module's `pub use`.

mod hash;
mod sync_eq;
mod tombstone;

pub use hash::{content_hash, content_hash_from_parts, hex_encode};
pub use tombstone::MAX_SAFE_TOMBSTONE_DAYS;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::enums::{IssueType, Priority, Status};
use crate::relations::{Comment, Dependency};
use crate::serde_helpers::{is_false, serialize_compaction_level};

/// The primary issue entity (spine §1.6).
///
/// `content_hash` is `#[serde(skip)]` — it never appears in JSONL and is recomputed on load (it is
/// the import idempotency key, FR-26). `compaction_level` always serializes as an integer (`0` when
/// `None`) for `bd` conformance. All other optional fields are omitted when empty.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct Issue {
    /// Unique id (prefix + optional slug + hash, e.g. `"ub-abc123"`).
    pub id: String,

    /// Canonical content hash for dedup/sync. **Never** serialized to JSONL; recomputed on load.
    #[serde(skip)]
    pub content_hash: Option<String>,

    /// Title (1..=500 chars; see [`crate::IssueValidator`]).
    pub title: String,

    /// Detailed description (unbounded by design).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Technical design notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub design: Option<String>,

    /// Acceptance criteria.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance_criteria: Option<String>,

    /// Additional notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,

    /// Workflow status.
    #[serde(default)]
    pub status: Status,

    /// Priority (0=Critical, 4=Backlog).
    #[serde(default)]
    pub priority: Priority,

    /// Issue type.
    #[serde(default)]
    pub issue_type: IssueType,

    /// Assigned actor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,

    /// Issue owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,

    /// Estimated effort in minutes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_minutes: Option<i32>,

    /// Creation timestamp.
    pub created_at: DateTime<Utc>,

    /// Creator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,

    /// Last-update timestamp.
    pub updated_at: DateTime<Utc>,

    /// Closure timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<DateTime<Utc>>,

    /// Reason for closure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close_reason: Option<String>,

    /// Session id that closed this issue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_by_session: Option<String>,

    /// Due date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_at: Option<DateTime<Utc>>,

    /// Defer-until date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defer_until: Option<DateTime<Utc>>,

    /// External reference (orphans derive from this; FR-15).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_ref: Option<String>,

    /// Source system for imported issues.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_system: Option<String>,

    /// Source repository basename (multi-repo support).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_repo: Option<String>,

    /// Absolute canonical path of the source repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_repo_path: Option<String>,

    /// Canonical-JSON governance document inherited by descendants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_context: Option<String>,

    /// Tombstone: deletion timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<DateTime<Utc>>,

    /// Tombstone: who deleted the issue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_by: Option<String>,

    /// Tombstone: reason for deletion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete_reason: Option<String>,

    /// Tombstone: the issue type before deletion (preserved across the tombstone transition).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_type: Option<String>,

    /// Compaction level (kept for JSONL round-trip fidelity, D12). `None` serializes as `0`.
    #[serde(default, serialize_with = "serialize_compaction_level")]
    pub compaction_level: Option<i32>,

    /// Compaction timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compacted_at: Option<DateTime<Utc>>,

    /// The commit at which the issue was compacted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compacted_at_commit: Option<String>,

    /// The pre-compaction size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_size: Option<i32>,

    /// Message sender (messaging surface).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<String>,

    /// Whether the issue is ephemeral.
    #[serde(default, skip_serializing_if = "is_false")]
    pub ephemeral: bool,

    /// Whether the issue is pinned.
    #[serde(default, skip_serializing_if = "is_false")]
    pub pinned: bool,

    /// Whether the issue is a template.
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_template: bool,

    /// Labels (hydrated for export/display).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,

    /// Dependencies (hydrated for export/display).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<Dependency>,

    /// Comments (hydrated for export/display on all read paths — D37).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<Comment>,
}

impl Default for Issue {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            id: String::new(),
            content_hash: None,
            title: String::new(),
            description: None,
            design: None,
            acceptance_criteria: None,
            notes: None,
            status: Status::default(),
            priority: Priority::default(),
            issue_type: IssueType::default(),
            assignee: None,
            owner: None,
            estimated_minutes: None,
            created_at: now,
            created_by: None,
            updated_at: now,
            closed_at: None,
            close_reason: None,
            closed_by_session: None,
            due_at: None,
            defer_until: None,
            external_ref: None,
            source_system: None,
            source_repo: None,
            source_repo_path: None,
            agent_context: None,
            deleted_at: None,
            deleted_by: None,
            delete_reason: None,
            original_type: None,
            compaction_level: None,
            compacted_at: None,
            compacted_at_commit: None,
            original_size: None,
            sender: None,
            ephemeral: false,
            pinned: false,
            is_template: false,
            labels: Vec::new(),
            dependencies: Vec::new(),
            comments: Vec::new(),
        }
    }
}

impl Issue {
    /// Compute the deterministic content hash for this issue (spine §1.8).
    ///
    /// SHA-256 over the canonical ordered, null-separated field set (delegates to
    /// [`content_hash`]). Excludes `id`, `content_hash`, relations, all timestamps, tombstone
    /// fields, and `estimated_minutes`/`due_at`/`defer_until`/`close_reason`/`closed_by_session`.
    #[must_use]
    pub fn compute_content_hash(&self) -> String {
        content_hash(self)
    }
}

#[cfg(test)]
mod tests {
    use super::Issue;
    use crate::enums::Status;
    use chrono::{TimeZone, Utc};

    fn fixed_issue() -> Issue {
        Issue {
            id: "ub-abc123".to_string(),
            title: "Test".to_string(),
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            ..Issue::default()
        }
    }

    #[test]
    fn content_hash_field_absent_from_json() {
        let mut issue = fixed_issue();
        issue.content_hash = Some("deadbeef".to_string());
        let value = serde_json::to_value(&issue).unwrap();
        assert!(value.get("content_hash").is_none());
    }

    #[test]
    fn skip_serializing_if_omits_empties() {
        let issue = fixed_issue();
        let value = serde_json::to_value(&issue).unwrap();
        assert!(value.get("description").is_none());
        assert!(value.get("labels").is_none());
        assert!(value.get("dependencies").is_none());
        assert!(value.get("comments").is_none());
        // ephemeral/pinned/is_template are false and omitted.
        assert!(value.get("ephemeral").is_none());
    }

    #[test]
    fn compaction_level_none_serializes_as_zero() {
        let issue = fixed_issue();
        let value = serde_json::to_value(&issue).unwrap();
        assert_eq!(value["compaction_level"], 0);
    }

    #[test]
    fn deserialize_defaults_missing_fields() {
        let json = r#"{
            "id": "ub-123",
            "title": "Test issue",
            "status": "open",
            "priority": 2,
            "issue_type": "task",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        }"#;
        let issue: Issue = serde_json::from_str(json).unwrap();
        assert!(issue.description.is_none());
        assert!(issue.labels.is_empty());
        assert_eq!(issue.status, Status::Open);
        // content_hash is #[serde(skip)] -> always None on load.
        assert!(issue.content_hash.is_none());
    }
}
