//! [`EventType`] — audit event kind (spine §1.5).

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Audit event type (spine §1.5).
///
/// Serializes as a plain `snake_case` string (hand-rolled `Serialize`/`Deserialize`/`JsonSchema`).
/// An unrecognised string deserializes into [`EventType::Custom`] (original case preserved).
///
/// # Examples
///
/// ```
/// use unblock_model::EventType;
///
/// assert_eq!(EventType::StatusChanged.as_str(), "status_changed");
/// let ev: EventType = serde_json::from_str("\"status_changed\"").unwrap();
/// assert_eq!(ev, EventType::StatusChanged);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EventType {
    /// An issue was created.
    Created,
    /// An issue was updated.
    Updated,
    /// An issue's status changed.
    StatusChanged,
    /// An issue's priority changed.
    PriorityChanged,
    /// An issue's assignee changed.
    AssigneeChanged,
    /// A comment was added.
    Commented,
    /// An issue was closed.
    Closed,
    /// An issue was reopened.
    Reopened,
    /// A dependency edge was added.
    DependencyAdded,
    /// A dependency edge was removed.
    DependencyRemoved,
    /// A label was added.
    LabelAdded,
    /// A label was removed.
    LabelRemoved,
    /// An issue was compacted.
    Compacted,
    /// An issue was deleted (tombstoned).
    Deleted,
    /// An issue was restored.
    Restored,
    /// A comment was edited (D37/D-D — provenance-preserving update).
    CommentEdited,
    /// A comment was redacted (D37/D-E — soft-delete: row kept, body masked).
    CommentRedacted,
    /// An open-enum tail variant for any unrecognised event (original case preserved).
    Custom(String),
}

impl EventType {
    /// The stable `snake_case` wire string for this event type.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Created => "created",
            Self::Updated => "updated",
            Self::StatusChanged => "status_changed",
            Self::PriorityChanged => "priority_changed",
            Self::AssigneeChanged => "assignee_changed",
            Self::Commented => "commented",
            Self::Closed => "closed",
            Self::Reopened => "reopened",
            Self::DependencyAdded => "dependency_added",
            Self::DependencyRemoved => "dependency_removed",
            Self::LabelAdded => "label_added",
            Self::LabelRemoved => "label_removed",
            Self::Compacted => "compacted",
            Self::Deleted => "deleted",
            Self::Restored => "restored",
            Self::CommentEdited => "comment_edited",
            Self::CommentRedacted => "comment_redacted",
            Self::Custom(value) => value,
        }
    }
}

impl Serialize for EventType {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for EventType {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Ok(match value.as_str() {
            "created" => Self::Created,
            "updated" => Self::Updated,
            "status_changed" => Self::StatusChanged,
            "priority_changed" => Self::PriorityChanged,
            "assignee_changed" => Self::AssigneeChanged,
            "commented" => Self::Commented,
            "closed" => Self::Closed,
            "reopened" => Self::Reopened,
            "dependency_added" => Self::DependencyAdded,
            "dependency_removed" => Self::DependencyRemoved,
            "label_added" => Self::LabelAdded,
            "label_removed" => Self::LabelRemoved,
            "compacted" => Self::Compacted,
            "deleted" => Self::Deleted,
            "restored" => Self::Restored,
            "comment_edited" => Self::CommentEdited,
            "comment_redacted" => Self::CommentRedacted,
            _ => Self::Custom(value),
        })
    }
}

impl JsonSchema for EventType {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("EventType")
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        // Serialized as a string (see the hand-rolled Serialize/Deserialize above).
        generator.subschema_for::<String>()
    }
}

#[cfg(test)]
mod tests {
    use super::EventType;

    const ALL_KNOWN: [EventType; 17] = [
        EventType::Created,
        EventType::Updated,
        EventType::StatusChanged,
        EventType::PriorityChanged,
        EventType::AssigneeChanged,
        EventType::Commented,
        EventType::Closed,
        EventType::Reopened,
        EventType::DependencyAdded,
        EventType::DependencyRemoved,
        EventType::LabelAdded,
        EventType::LabelRemoved,
        EventType::Compacted,
        EventType::Deleted,
        EventType::Restored,
        EventType::CommentEdited,
        EventType::CommentRedacted,
    ];

    #[test]
    fn serde_string_roundtrip_all_known() {
        for ev in ALL_KNOWN {
            let json = serde_json::to_string(&ev).unwrap();
            // Serializes to a plain JSON string, not an object.
            assert!(json.starts_with('"') && json.ends_with('"'));
            let back: EventType = serde_json::from_str(&json).unwrap();
            assert_eq!(back, ev);
        }
    }

    #[test]
    fn unknown_becomes_custom() {
        let ev: EventType = serde_json::from_str("\"frobnicated\"").unwrap();
        assert_eq!(ev, EventType::Custom("frobnicated".to_string()));
        assert_eq!(serde_json::to_string(&ev).unwrap(), "\"frobnicated\"");
    }

    #[test]
    fn as_str_is_snake_case() {
        assert_eq!(EventType::DependencyAdded.as_str(), "dependency_added");
        assert_eq!(EventType::LabelRemoved.as_str(), "label_removed");
    }
}
