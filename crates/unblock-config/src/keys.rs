//! The startup-vs-runtime key partition (FR-13 "startup-vs-runtime key partitioning").
//!
//! Every merged [`crate::WorkspaceConfig`] field is classified into exactly one of two classes:
//!
//! - **Startup** keys are read once when the workspace is opened (the on-disk artifact filenames,
//!   the retention window, the backend selector). Changing them needs a reopen.
//! - **Runtime** keys can be re-read while a session is live (the actor default, output format, the
//!   JSONL-export toggle, the search cap). v1.1 `reload_runtime` re-resolves only these.
//!
//! The lists here are the FR-13 contract surface; an `insta` snapshot of the two name lists is the
//! drift detector, and a unit test asserts every `WorkspaceConfig` field is classified exactly once
//! (no key in both classes, none missing).

/// A startup-only config key — read once at workspace open (FR-13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupKey {
    /// `db_filename` — the database filename inside `.unblock/`.
    DbFilename,
    /// `jsonl_filename` — the JSONL export filename inside `.unblock/`.
    JsonlFilename,
    /// `backend` — the storage backend selector (only `"libsql"` accepted in v1; MF-3).
    Backend,
    /// `deletions_retention_days` — the tombstone retention window (reserved for v1.1).
    DeletionsRetentionDays,
}

/// A runtime config key — re-readable while a session is live (FR-13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeKey {
    /// `actor` — the default actor for authored events.
    Actor,
    /// `output_format` — the rendered-output format selector.
    OutputFormat,
    /// `jsonl_export` — the auto-export-after-mutation toggle (FR-7).
    JsonlExport,
    /// `search_cap` — the search-result cap (FR-4).
    SearchCap,
}

/// Which partition a config key belongs to (FR-13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyClass {
    /// A [`StartupKey`] — read once at open.
    Startup(StartupKey),
    /// A [`RuntimeKey`] — re-readable at runtime.
    Runtime(RuntimeKey),
}

/// The startup-only key names (wire spelling), in `WorkspaceConfig` field order.
pub const STARTUP_KEYS: &[&str] = &[
    "db_filename",
    "jsonl_filename",
    "backend",
    "deletions_retention_days",
];

/// The runtime key names (wire spelling), in `WorkspaceConfig` field order.
pub const RUNTIME_KEYS: &[&str] = &["actor", "output_format", "jsonl_export", "search_cap"];

/// Classify a config key into its [`KeyClass`], or `None` for an unknown key.
///
/// The classification is total over the merged [`crate::WorkspaceConfig`] field set: every field is
/// classified into exactly one class (asserted in the unit tests). An unknown key returns `None` —
/// the resolver treats it as a warn-only forward-compat key (SF-4), never an error.
#[must_use]
pub fn classify(key: &str) -> Option<KeyClass> {
    match key {
        "db_filename" => Some(KeyClass::Startup(StartupKey::DbFilename)),
        "jsonl_filename" => Some(KeyClass::Startup(StartupKey::JsonlFilename)),
        "backend" => Some(KeyClass::Startup(StartupKey::Backend)),
        "deletions_retention_days" => Some(KeyClass::Startup(StartupKey::DeletionsRetentionDays)),
        "actor" => Some(KeyClass::Runtime(RuntimeKey::Actor)),
        "output_format" => Some(KeyClass::Runtime(RuntimeKey::OutputFormat)),
        "jsonl_export" => Some(KeyClass::Runtime(RuntimeKey::JsonlExport)),
        "search_cap" => Some(KeyClass::Runtime(RuntimeKey::SearchCap)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{KeyClass, RUNTIME_KEYS, STARTUP_KEYS, classify};
    use std::collections::HashSet;

    /// Every merged `WorkspaceConfig` field (its wire-key name), in field-declaration order. The
    /// exhaustiveness assertions below are keyed off this single list so adding a field without
    /// classifying it fails the test.
    const WORKSPACE_CONFIG_FIELDS: &[&str] = &[
        "actor",
        "output_format",
        "jsonl_export",
        "search_cap",
        "db_filename",
        "jsonl_filename",
        "deletions_retention_days",
        "backend",
    ];

    #[test]
    fn every_field_classified_exactly_once() {
        for field in WORKSPACE_CONFIG_FIELDS {
            let class = classify(field).unwrap_or_else(|| panic!("`{field}` is unclassified"));
            let in_startup = STARTUP_KEYS.contains(field);
            let in_runtime = RUNTIME_KEYS.contains(field);
            assert!(
                in_startup ^ in_runtime,
                "`{field}` must be in exactly one of STARTUP_KEYS / RUNTIME_KEYS"
            );
            match class {
                KeyClass::Startup(_) => assert!(
                    in_startup,
                    "`{field}` classified Startup but not in STARTUP_KEYS"
                ),
                KeyClass::Runtime(_) => assert!(
                    in_runtime,
                    "`{field}` classified Runtime but not in RUNTIME_KEYS"
                ),
            }
        }
    }

    #[test]
    fn key_lists_cover_exactly_the_fields_with_no_overlap() {
        let startup: HashSet<&str> = STARTUP_KEYS.iter().copied().collect();
        let runtime: HashSet<&str> = RUNTIME_KEYS.iter().copied().collect();
        // No key in both partitions.
        assert!(
            startup.is_disjoint(&runtime),
            "a key appears in both partitions"
        );
        // The union is exactly the field set (no extra, none missing).
        let union: HashSet<&str> = startup.union(&runtime).copied().collect();
        let fields: HashSet<&str> = WORKSPACE_CONFIG_FIELDS.iter().copied().collect();
        assert_eq!(
            union, fields,
            "STARTUP_KEYS ∪ RUNTIME_KEYS must equal the field set"
        );
    }

    #[test]
    fn unknown_key_is_none() {
        assert!(classify("not_a_real_key").is_none());
        assert!(classify("").is_none());
    }

    /// Golden snapshot of the two key lists (FR-13 startup-vs-runtime contract drift detector).
    #[test]
    fn key_lists_golden() {
        // `(startup, runtime)` as a serializable tuple of slices (no serde_json dep needed).
        insta::assert_json_snapshot!("startup_runtime_key_lists", (STARTUP_KEYS, RUNTIME_KEYS));
    }
}
