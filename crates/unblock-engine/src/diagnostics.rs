//! Engine-side dispatch over the [`DiagnosticKind`] taxonomy → [`DiagnosticReport`] (CF-B, spine
//! §4.1 / §5.3).
//!
//! The `DiagnosticKind`/`DiagnosticReport`/`DiagnosticFinding` types are **defined in
//! `unblock-model`** (spine §1.10, CF-B) and re-exported (never redefined here) so `unblock-render`
//! (model + error only) can format them. The seven landed kinds — `Stats|Info|Where|Version|Lint|
//! Changelog|Orphans` — are the caller-supplied **input** (this is the BUILD-now read path; contrast
//! `doctor`/`recover`, whose integrity `DiagnosticKind` is unconstructible in v1 and is therefore
//! seamed to `health`/T3.3).
//!
//! Every derivation is **pure-DB** (counts, `closed_since`, `orphan_candidates`) — it **never** shells
//! to git or touches the network (FR-15 / NFR-6). The `Changelog` window-threading (`since`) is a
//! **T2.7** concern (the engine signature takes only a `kind`, spine §4.1); v1 uses the full
//! all-closed window.

use unblock_model::{
    CountGroupBy, DiagnosticFinding, DiagnosticKind, DiagnosticReport, ListFilters,
};
use unblock_storage::Storage;

use crate::error::Result;

/// The engine's display version (the workspace package version), surfaced by the `Version` kind.
const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Build the [`DiagnosticReport`] for `kind` over the workspace, derived purely from `storage`
/// (FR-15) plus the static workspace facts (`actor`/`workspace_dir`/`db_path`/`jsonl_path`) needed by
/// the `Info`/`Where` kinds.
///
/// The returned report's `kind` is the **input** `kind` (never an integrity placeholder). Pure-DB:
/// no git, no network (NFR-6).
///
/// # Errors
///
/// Forwards any [`StorageError`](unblock_storage::StorageError) from the underlying probe as the
/// transparent [`EngineError`] source.
pub(crate) async fn diagnostics(
    storage: &dyn Storage,
    facts: WorkspaceFacts<'_>,
    kind: DiagnosticKind,
) -> Result<DiagnosticReport> {
    let findings = match kind {
        DiagnosticKind::Stats => stats(storage).await?,
        DiagnosticKind::Info => info(storage, facts).await?,
        DiagnosticKind::Where => where_(facts),
        DiagnosticKind::Version => version(),
        DiagnosticKind::Lint => lint(storage).await?,
        DiagnosticKind::Changelog => changelog(storage).await?,
        DiagnosticKind::Orphans => orphans(storage).await?,
    };
    Ok(DiagnosticReport { kind, findings })
}

/// The static workspace facts the `Info`/`Where` diagnostics surface (from the `WorkspaceContext`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct WorkspaceFacts<'a> {
    /// The authoritative actor (spine §4.1).
    pub actor: &'a str,
    /// The project root (the dir that contains `.unblock/`).
    pub workspace_dir: &'a std::path::Path,
    /// The discovered `.unblock/` directory.
    pub unblock_dir: &'a std::path::Path,
    /// The libsql database path.
    pub db_path: &'a std::path::Path,
    /// The JSONL export path.
    pub jsonl_path: &'a std::path::Path,
}

/// Aggregate statistics: per-status counts + the grand total.
async fn stats(storage: &dyn Storage) -> Result<Vec<DiagnosticFinding>> {
    // Include closed/deferred so the stats reflect the WHOLE store (a diagnostics view, not a
    // ready/list view).
    let filters = all_inclusive_filters();
    let buckets = storage
        .count_issues(&filters, Some(CountGroupBy::Status))
        .await?;
    let total: usize = buckets.iter().map(|b| b.count).sum();

    let mut findings: Vec<DiagnosticFinding> = buckets
        .into_iter()
        .map(|b| DiagnosticFinding {
            label: format!("status:{}", b.key),
            detail: b.count.to_string(),
        })
        .collect();
    findings.push(DiagnosticFinding {
        label: "total".to_string(),
        detail: total.to_string(),
    });
    Ok(findings)
}

/// General workspace info: actor + a live total-issue count.
async fn info(storage: &dyn Storage, facts: WorkspaceFacts<'_>) -> Result<Vec<DiagnosticFinding>> {
    let filters = all_inclusive_filters();
    let total: usize = storage
        .count_issues(&filters, None)
        .await?
        .iter()
        .map(|b| b.count)
        .sum();
    Ok(vec![
        DiagnosticFinding {
            label: "actor".to_string(),
            detail: facts.actor.to_string(),
        },
        DiagnosticFinding {
            label: "workspace_dir".to_string(),
            detail: facts.workspace_dir.display().to_string(),
        },
        DiagnosticFinding {
            label: "issues".to_string(),
            detail: total.to_string(),
        },
    ])
}

/// Where the workspace lives: the resolved `.unblock/` + db/jsonl paths.
fn where_(facts: WorkspaceFacts<'_>) -> Vec<DiagnosticFinding> {
    vec![
        DiagnosticFinding {
            label: "unblock_dir".to_string(),
            detail: facts.unblock_dir.display().to_string(),
        },
        DiagnosticFinding {
            label: "db_path".to_string(),
            detail: facts.db_path.display().to_string(),
        },
        DiagnosticFinding {
            label: "jsonl_path".to_string(),
            detail: facts.jsonl_path.display().to_string(),
        },
    ]
}

/// Version information.
fn version() -> Vec<DiagnosticFinding> {
    vec![DiagnosticFinding {
        label: "version".to_string(),
        detail: ENGINE_VERSION.to_string(),
    }]
}

/// Lint findings: v1 derives the count of issues with an unresolved gating dependency (a "blocked"
/// hygiene signal). The richer lint taxonomy lands additively (v1.1).
async fn lint(storage: &dyn Storage) -> Result<Vec<DiagnosticFinding>> {
    let filters = ListFilters::default();
    let blocked = storage.blocked_issues(&filters).await?;
    Ok(vec![DiagnosticFinding {
        label: "blocked".to_string(),
        detail: blocked.len().to_string(),
    }])
}

/// The changelog of closed issues (all-closed window; the `since` threading is T2.7).
async fn changelog(storage: &dyn Storage) -> Result<Vec<DiagnosticFinding>> {
    let closed = storage.closed_since(None).await?;
    Ok(closed
        .into_iter()
        .map(|issue| DiagnosticFinding {
            label: issue.id,
            detail: issue.title,
        })
        .collect())
}

/// Orphan candidates: issues whose `external_ref` matches the commit pattern (pure-DB; no git).
async fn orphans(storage: &dyn Storage) -> Result<Vec<DiagnosticFinding>> {
    let candidates = storage.orphan_candidates().await?;
    Ok(candidates
        .into_iter()
        .map(|issue| DiagnosticFinding {
            label: issue.id,
            detail: issue.external_ref.unwrap_or_default(),
        })
        .collect())
}

/// A `ListFilters` that counts the WHOLE store (closed + deferred included) — for `Stats`/`Info`.
fn all_inclusive_filters() -> ListFilters {
    ListFilters {
        include_closed: true,
        include_deferred: true,
        ..ListFilters::default()
    }
}

#[cfg(test)]
mod tests {
    use super::WorkspaceFacts;
    use std::path::Path;

    #[test]
    fn workspace_facts_is_copy() {
        let facts = WorkspaceFacts {
            actor: "alice",
            workspace_dir: Path::new("/ws"),
            unblock_dir: Path::new("/ws/.unblock"),
            db_path: Path::new("/ws/.unblock/unblock.db"),
            jsonl_path: Path::new("/ws/.unblock/issues.jsonl"),
        };
        let copy = facts; // Copy
        assert_eq!(copy.actor, facts.actor);
    }
}
