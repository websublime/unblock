//! Issue relations and the derived epic-status rollup (spine §1.7).

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::enums::{DependencyType, EventType};
use crate::issue::Issue;
use crate::serde_helpers::deserialize_optional_metadata;

/// A relationship between two issues (spine §1.7).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct Dependency {
    /// The issue that has the dependency (source).
    pub issue_id: String,

    /// The issue being depended on (target).
    pub depends_on_id: String,

    /// The kind of dependency.
    #[serde(rename = "type")]
    pub dep_type: DependencyType,

    /// Creation timestamp.
    pub created_at: DateTime<Utc>,

    /// Who created the edge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,

    /// Optional JSON metadata. A degenerate empty (or whitespace-only) string is coerced to
    /// `None` on deserialization to tolerate legacy JSONL that wrote `"metadata":""`.
    #[serde(
        default,
        deserialize_with = "deserialize_optional_metadata",
        skip_serializing_if = "Option::is_none"
    )]
    pub metadata: Option<String>,

    /// Thread id for conversation linking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

/// A comment on an issue (spine §1.7; v1 surface — D37). Flat: no threading (FORK-T).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct Comment {
    /// Stable comment id.
    pub id: i64,
    /// The issue this comment belongs to.
    pub issue_id: String,
    /// The comment author (= the session actor at the MCP surface, FORK-M1b).
    pub author: String,
    /// The comment body (serialized under `"text"`); masked to `""` on redact.
    #[serde(rename = "text")]
    pub body: String,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Provenance-preserving edit instant (D37/D-D): `None` = never edited; `Some` = last edit.
    ///
    /// `add`'s own INSERT is create-time-only — see spine §3.2.1 MUST-1 SCOPE.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    /// Soft-redact marker (D37/D-E): `None` = live; `Some` = redacted.
    ///
    /// The PRESENCE is the "is redacted" flag (mirroring the tombstone `deleted_at`); on redact the
    /// row is KEPT and `body` is masked to `""`. The `CommentRedacted` audit event retains the
    /// original body (provenance — FORK-redact-wire).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redacted_at: Option<DateTime<Utc>>,
}

/// An append-only audit event (spine §1.7), written transactionally inside a mutation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct Event {
    /// Stable event id.
    pub id: i64,
    /// The issue this event belongs to.
    pub issue_id: String,
    /// The kind of event.
    pub event_type: EventType,
    /// The actor that caused the event.
    pub actor: String,
    /// The prior value (for change events).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_value: Option<String>,
    /// The new value (for change events).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_value: Option<String>,
    /// An optional free-form comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Tier-1 attribution: self-reported agent name (capture-only, never enforced).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    /// Tier-1 attribution: self-reported harness identifier (capture-only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    /// Tier-1 attribution: self-reported model identifier (capture-only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Epic completion status with child counts (spine §1.7; derived rollup, populated v1.1).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct EpicStatus {
    /// The epic issue.
    pub epic: Issue,
    /// Total number of child issues.
    pub total_children: usize,
    /// Number of closed child issues.
    pub closed_children: usize,
    /// Whether the epic is eligible to be closed.
    pub eligible_for_close: bool,
}

#[cfg(test)]
mod tests {
    use super::{Comment, Dependency};
    use crate::enums::DependencyType;
    use chrono::{TimeZone, Utc};

    fn ts() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
    }

    #[test]
    fn dependency_dep_type_serializes_under_type_key() {
        let dep = Dependency {
            issue_id: "ub-a".to_string(),
            depends_on_id: "ub-b".to_string(),
            dep_type: DependencyType::Blocks,
            created_at: ts(),
            created_by: None,
            metadata: None,
            thread_id: None,
        };
        let value = serde_json::to_value(&dep).unwrap();
        assert_eq!(value["type"], "blocks");
        assert!(value.get("created_by").is_none());
    }

    #[test]
    fn dependency_empty_metadata_coerced_to_none() {
        let json = r#"{
            "issue_id": "ub-a",
            "depends_on_id": "ub-b",
            "type": "blocks",
            "created_at": "2026-01-01T00:00:00Z",
            "metadata": ""
        }"#;
        let dep: Dependency = serde_json::from_str(json).unwrap();
        assert!(dep.metadata.is_none());
    }

    #[test]
    fn dependency_real_metadata_preserved() {
        let json = r#"{
            "issue_id": "ub-a",
            "depends_on_id": "ub-b",
            "type": "blocks",
            "created_at": "2026-01-01T00:00:00Z",
            "metadata": "{\"k\":1}"
        }"#;
        let dep: Dependency = serde_json::from_str(json).unwrap();
        assert_eq!(dep.metadata.as_deref(), Some("{\"k\":1}"));
    }

    #[test]
    fn comment_body_serializes_under_text_key() {
        let comment = Comment {
            id: 1,
            issue_id: "ub-a".to_string(),
            author: "tester".to_string(),
            body: "hello".to_string(),
            created_at: ts(),
            updated_at: None,
            redacted_at: None,
        };
        let value = serde_json::to_value(&comment).unwrap();
        assert_eq!(value["text"], "hello");
        // D37/FORK-M1: both new fields skip-when-None (a bd-shaped 5-field comment stays 5-field).
        assert!(value.get("updated_at").is_none());
        assert!(value.get("redacted_at").is_none());
    }

    #[test]
    fn comment_bd_shaped_five_field_json_roundtrips_absent_to_none() {
        // FR-26 / D37: a bd-exported comment carries no `updated_at`/`redacted_at` — both must
        // deserialize to None and round-trip back to a byte-identical 5-field document.
        let json = r#"{
            "id": 7,
            "issue_id": "ub-a",
            "author": "tester",
            "text": "hello",
            "created_at": "2026-01-01T00:00:00Z"
        }"#;
        let comment: Comment = serde_json::from_str(json).unwrap();
        assert_eq!(comment.updated_at, None);
        assert_eq!(comment.redacted_at, None);
        let back = serde_json::to_value(&comment).unwrap();
        assert_eq!(back.as_object().unwrap().len(), 5);
    }

    #[test]
    fn comment_redact_wire_form_is_redacted_at_present_plus_empty_text() {
        // D37/D-E: the PRESENCE of `redacted_at` is the flag — no extra top-level bool.
        let comment = Comment {
            id: 1,
            issue_id: "ub-a".to_string(),
            author: "tester".to_string(),
            body: String::new(),
            created_at: ts(),
            updated_at: None,
            redacted_at: Some(ts()),
        };
        let value = serde_json::to_value(&comment).unwrap();
        assert_eq!(value["text"], "");
        assert!(value.get("redacted_at").is_some());
        assert!(value.get("redacted").is_none());
    }
}
