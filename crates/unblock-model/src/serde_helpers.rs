//! Shared `#[serde(...)]` attribute helper functions referenced across the model structs.
//!
//! Kept `pub(crate)` — they are an implementation detail of the serde attributes on [`crate::Issue`]
//! and [`crate::Dependency`], not part of the public API.

use serde::{Deserialize, Serializer};

/// `skip_serializing_if` predicate: skip a `bool` field when it is `false`.
#[allow(clippy::trivially_copy_pass_by_ref)] // signature is dictated by serde's `skip_serializing_if`.
pub(crate) const fn is_false(b: &bool) -> bool {
    !*b
}

/// `serialize_with` for `compaction_level`: serialize `Option<i32>` as `0` when `None`.
///
/// `bd`'s Go SQL scanner cannot read `NULL` for the integer compaction-level column, so the
/// canonical JSONL form always writes an integer. `None` and `Some(0)` therefore both emit `0`.
// Signature is dictated by serde's `serialize_with`; `&Option<i32>` cannot be `Option<i32>`.
#[allow(clippy::ref_option, clippy::trivially_copy_pass_by_ref)]
pub(crate) fn serialize_compaction_level<S>(
    value: &Option<i32>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_i32(value.unwrap_or(0))
}

/// `deserialize_with` for `Dependency::metadata`: coerce a degenerate empty (or whitespace-only)
/// string to `None`.
///
/// Legacy JSONL written by older `br`/`bd` versions serialized absent dependency metadata as
/// `"metadata":""` rather than omitting the field. The empty string is not valid JSON, so a
/// downstream consumer that parses `metadata` as JSON would reject the record. Treating `""` (or
/// whitespace) as absent is lossless; any genuine (non-blank) string is preserved verbatim.
pub(crate) fn deserialize_optional_metadata<'de, D>(
    deserializer: D,
) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    Ok(match value {
        Some(s) if s.trim().is_empty() => None,
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use super::is_false;

    #[test]
    fn is_false_truth() {
        assert!(is_false(&false));
        assert!(!is_false(&true));
    }
}
