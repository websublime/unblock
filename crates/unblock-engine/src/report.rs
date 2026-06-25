//! Engine-owned interchange options + the re-exported model report/outcome DTOs (CF-A, spine §1.10).
//!
//! [`CloseOutcome`], [`ImportReport`], and [`ExportReport`] are **defined in `unblock-model` §1.10**
//! and re-exported here (relocated so `unblock-render` — model + error only — can format them). This
//! module *defines* only the engine-local [`ImportOptions`] input that `import_jsonl` takes.
//!
//! # `ImportOptions` name-collision (resolved at T2.4)
//!
//! The engine's **public** `ImportOptions { dry_run }` is the **spine-owned** one (spine §4.1, the
//! type `import_jsonl` takes). `unblock-sync`'s plan defines a *different, internal*
//! `ImportOptions { dry_run, allow_external, on_collision }`. They are distinct types — when the
//! sync body is wired (T2.4) the engine **maps** its public `ImportOptions` into sync's internal
//! type at the call site (the extra knobs default). The engine's public surface stays
//! `{ dry_run }`; this is resolved fully at T2.4, not now.

// Re-export the model-owned report/outcome DTOs (CF-A) — NOT redefined here.
pub use unblock_model::{CloseOutcome, ExportReport, ImportReport};

/// Engine-owned options for [`crate::Session::import_jsonl`] (spine §4.1).
///
/// The only v1 knob is `dry_run`. The richer import knobs (`allow_external`, `on_collision`) live in
/// `unblock-sync`'s internal options type; the engine maps into it at the T2.4 call site.
#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    /// When `true`, the import is planned but applies no DB mutation.
    pub dry_run: bool,
}

#[cfg(test)]
mod tests {
    use super::ImportOptions;

    #[test]
    fn import_options_default_is_not_dry_run() {
        assert!(!ImportOptions::default().dry_run);
    }

    #[test]
    fn import_options_is_clone_debug() {
        let opts = ImportOptions { dry_run: true };
        let cloned = opts.clone();
        assert!(cloned.dry_run);
        assert!(format!("{opts:?}").contains("dry_run"));
    }
}
