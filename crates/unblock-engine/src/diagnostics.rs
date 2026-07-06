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
//! Every derivation is **pure-DB** (counts, `epic_child_rollup`, `closed_since`, `orphan_candidates`,
//! `list`/`ready`/`blocked` reads) — it **never** shells to git or touches the network (FR-15 /
//! NFR-6). The `stats` and `lint` kinds are a **faithful port of bd's `StatsSummary` and template
//! lint** MINUS every git-derived block (D26/T2.7); `changelog` accepts the `since` window (OQ-1) and
//! excludes templates in this engine composition (faithful to bd's `list_changelog_issues`). Every
//! finding list has a **pinned deterministic order** (NFR-14 — insta stability).

use chrono::{DateTime, Utc};
use unblock_model::{
    CountGroupBy, DiagnosticFinding, DiagnosticKind, DiagnosticReport, IssueType, ListFilters,
    Status,
};
use unblock_storage::Storage;

use crate::error::Result;

/// The engine's display version (the workspace package version), surfaced by the `Version` kind.
const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Build the [`DiagnosticReport`] for `kind` over the workspace, derived purely from `storage`
/// (FR-15) plus the static workspace facts (`actor`/`workspace_dir`/`db_path`/`jsonl_path`) needed by
/// the `Info`/`Where` kinds.
///
/// The returned report's `kind` is the **input** `kind` (never an integrity placeholder). `since` is
/// the changelog window (D26/OQ-1): it applies to the [`Changelog`](DiagnosticKind::Changelog) kind
/// only; every other kind ignores it. Pure-DB: no git, no network (NFR-6).
///
/// # Errors
///
/// Forwards any [`StorageError`](unblock_storage::StorageError) from the underlying probe as the
/// transparent [`EngineError`] source.
pub(crate) async fn diagnostics(
    storage: &dyn Storage,
    facts: WorkspaceFacts<'_>,
    kind: DiagnosticKind,
    since: Option<DateTime<Utc>>,
) -> Result<DiagnosticReport> {
    let findings = match kind {
        DiagnosticKind::Stats => stats(storage).await?,
        DiagnosticKind::Info => info(storage, facts).await?,
        DiagnosticKind::Where => where_(facts),
        DiagnosticKind::Version => version(),
        DiagnosticKind::Lint => lint(storage).await?,
        DiagnosticKind::Changelog => changelog(storage, since).await?,
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

/// Aggregate statistics — a faithful port of bd's `StatsSummary`
/// (`temp/beads_rust-main/src/cli/commands/stats.rs:376-499`) MINUS every git-derived block
/// (`RecentActivity`/`git_recent_activity` — EXCLUDED per NFR-6). Every counter derives purely from
/// libsql state (D26/OQ-5).
///
/// **Emission order (PINNED for NFR-14 insta stability):**
/// `open, in_progress, blocked, closed, ready, deferred, draft, tombstone, pinned, epics_eligible,
/// [avg_lead_time_hours], total` — each a `{label,detail}` row (label = the counter name,
/// detail = the count; `avg_lead_time_hours` is an ABSENT row when there are no closed issues).
///
/// Sources: the per-status tally + tombstone count come from `count_issues` with
/// `include_tombstone:true` (a live read-path flag since T2.6/D25); `total` is the non-tombstone sum.
/// `pinned` is composed in-memory over a widest-visibility `list_issues` pass
/// (`issue.pinned || status == Pinned`, `stats.rs:436`). `blocked` is the id SET of the manual
/// `Status::Blocked` rows UNION the dependency-blocked active ids (`blocked_issues`), deduped by id.
/// `ready` is `ready_issues().len()`. `epics_eligible` counts each `epic_child_rollup` entry with
/// `total>0 && closed==total` whose epic passes the in-memory active-non-template Epic filter.
/// `avg_lead_time_hours` is the mean `(closed_at − created_at)` in hours over closed issues.
async fn stats(storage: &dyn Storage) -> Result<Vec<DiagnosticFinding>> {
    // Per-status tally over the WHOLE table INCLUDING tombstones (bd's WHERE-less scan): the
    // tombstone bucket is surfaced as a distinct counter and `total` is the non-tombstone sum.
    let buckets = storage
        .count_issues(&all_visibility_filters(), Some(CountGroupBy::Status))
        .await?;
    let count_of = |status: Status| -> usize {
        buckets
            .iter()
            .find(|b| b.key == status.as_str())
            .map_or(0, |b| b.count)
    };
    let tombstone = count_of(Status::Tombstone);
    let total: usize = buckets
        .iter()
        .filter(|b| b.key != Status::Tombstone.as_str())
        .map(|b| b.count)
        .sum();

    // ONE widest-visibility (tombstone-inclusive) hydration pass feeds the in-memory counters:
    // `pinned`, the manual `Status::Blocked` id set, the active-non-template Epic id set, and the
    // closed-issue lead-time sample. This is bd's single stats scan (`list_stats_summary_issues`).
    let all = storage.list_issues(&all_visibility_filters()).await?;

    let pinned = all
        .iter()
        .filter(|i| i.pinned || i.status == Status::Pinned)
        .count();

    // `blocked` = manual `Status::Blocked` rows ∪ the live dependency-blocked active id set, DEDUPED
    // by id (a manual-Blocked issue that is ALSO dependency-blocked counts once).
    let mut blocked_ids: std::collections::HashSet<&str> = all
        .iter()
        .filter(|i| i.status == Status::Blocked)
        .map(|i| i.id.as_str())
        .collect();
    let dependency_blocked = storage.blocked_issues(&ListFilters::default()).await?;
    for issue in &dependency_blocked {
        blocked_ids.insert(issue.id.as_str());
    }
    let blocked = blocked_ids.len();

    let ready = storage.ready_issues(&ListFilters::default()).await?.len();

    // `epics_eligible` — the SQL rollup supplies the per-epic child (total, closed_or_tombstone); the
    // engine gates the EPIC side in-memory (`issue_type == Epic ∧ ¬terminal ∧ ¬template`,
    // `stats.rs:441-446`). Both filters live at their respective sites (D26).
    let eligible_epic_ids: std::collections::HashSet<&str> = all
        .iter()
        .filter(|i| i.issue_type == IssueType::Epic && !i.status.is_terminal() && !i.is_template)
        .map(|i| i.id.as_str())
        .collect();
    let rollup = storage.epic_child_rollup().await?;
    let epics_eligible = rollup
        .iter()
        .filter(|(epic_id, (child_total, child_closed))| {
            *child_total > 0
                && child_closed == child_total
                && eligible_epic_ids.contains(epic_id.as_str())
        })
        .count();

    // `avg_lead_time_hours` — mean `(closed_at − created_at)` in hours over closed issues; `None`
    // (an ABSENT finding row) when there are no closed issues (bd's `skip_serializing_if`).
    let lead_times: Vec<i64> = all
        .iter()
        .filter(|i| i.status == Status::Closed)
        .filter_map(|i| {
            i.closed_at
                .map(|closed| (closed - i.created_at).num_hours())
        })
        .collect();
    #[allow(clippy::cast_precision_loss)]
    let avg_lead_time_hours: Option<f64> = if lead_times.is_empty() {
        None
    } else {
        let sum: i64 = lead_times.iter().sum();
        Some(sum as f64 / lead_times.len() as f64)
    };

    // Emit in the PINNED bd-parity order (NFR-14).
    let mut findings = vec![
        finding("open", count_of(Status::Open)),
        finding("in_progress", count_of(Status::InProgress)),
        finding("blocked", blocked),
        finding("closed", count_of(Status::Closed)),
        finding("ready", ready),
        finding("deferred", count_of(Status::Deferred)),
        finding("draft", count_of(Status::Draft)),
        finding("tombstone", tombstone),
        finding("pinned", pinned),
        finding("epics_eligible", epics_eligible),
    ];
    if let Some(avg) = avg_lead_time_hours {
        findings.push(DiagnosticFinding {
            label: "avg_lead_time_hours".to_string(),
            detail: avg.to_string(),
        });
    }
    findings.push(finding("total", total));
    Ok(findings)
}

/// A `{label, detail}` finding whose `detail` is a decimal count string.
fn finding(label: &str, count: usize) -> DiagnosticFinding {
    DiagnosticFinding {
        label: label.to_string(),
        detail: count.to_string(),
    }
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

/// Return the required template sections for an issue type — bd's `required_sections`
/// (`temp/beads_rust-main/src/cli/commands/lint.rs:454-461`), in DECLARATION order (the pinned inner
/// finding order, NFR-14):
/// - `Bug` ⇒ [`## Steps to Reproduce`, `## Acceptance Criteria`]
/// - `Task` | `Feature` ⇒ [`## Acceptance Criteria`]
/// - `Epic` ⇒ [`## Success Criteria`]
/// - every other type (`Chore`/`Docs`/`Question`/`Custom`) ⇒ `[]` (the issue is skipped)
fn required_sections(issue_type: &IssueType) -> &'static [&'static str] {
    match issue_type {
        IssueType::Bug => &["## Steps to Reproduce", "## Acceptance Criteria"],
        IssueType::Task | IssueType::Feature => &["## Acceptance Criteria"],
        IssueType::Epic => &["## Success Criteria"],
        _ => &[],
    }
}

/// Strip a leading markdown heading prefix (`## ` or `# `) from a section heading — bd's
/// `strip_heading_prefix` (`lint.rs:478-484`).
fn strip_heading_prefix(heading: &str) -> &str {
    heading
        .strip_prefix("## ")
        .or_else(|| heading.strip_prefix("# "))
        .unwrap_or(heading)
}

/// Lint findings: a faithful port of bd's template-section lint
/// (`temp/beads_rust-main/src/cli/commands/lint.rs`). For each active non-template candidate issue
/// whose type has required sections, emit ONE `{label:id, detail:"missing section: <heading>"}`
/// finding per MISSING section.
///
/// Detection is over the `description` field ONLY (bd deliberately ignores the structured
/// `design`/`acceptance_criteria` columns): a case-insensitive substring test of the heading TEXT
/// (the `## `/`# ` prefix stripped), so `# acceptance criteria` satisfies `## Acceptance Criteria`.
///
/// **Order (PINNED for NFR-14):** outer = issue id ASC; inner = the required-section DECLARATION
/// order (Bug: `Steps to Reproduce` THEN `Acceptance Criteria`). The candidate set is the active
/// non-template set (`ListFilters::default()` = `open`+`in_progress` minus closed/deferred/tombstone),
/// templates filtered in-memory (`list_issues` does not exclude them). This REPLACES the prior
/// `blocked=<n>`-lite finding (bd's `lint` never computes a blocked count).
async fn lint(storage: &dyn Storage) -> Result<Vec<DiagnosticFinding>> {
    let mut candidates = storage.list_issues(&ListFilters::default()).await?;
    // Templates are not a required recommended-section subject (bd excludes `is_template`).
    candidates.retain(|issue| !issue.is_template);
    // Outer order = issue id ASC (deterministic, NFR-14).
    candidates.sort_by(|a, b| a.id.cmp(&b.id));

    let mut findings = Vec::new();
    for issue in &candidates {
        let required = required_sections(&issue.issue_type);
        if required.is_empty() {
            continue;
        }
        let description_lower = issue.description.as_deref().unwrap_or("").to_lowercase();
        for heading in required {
            let heading_text = strip_heading_prefix(heading).to_lowercase();
            if !description_lower.contains(&heading_text) {
                findings.push(DiagnosticFinding {
                    label: issue.id.clone(),
                    detail: format!("missing section: {heading}"),
                });
            }
        }
    }
    Ok(findings)
}

/// The changelog of closed issues over the optional `since` window (D26/OQ-1: `since=None` ⇒ all
/// closed). Faithful to bd's `list_changelog_issues`, this engine composition FILTERS OUT template
/// rows after `closed_since(since)` (`closed_since` stays shared/unchanged — the template filter is
/// an engine-side composition step, spine §3.2.1). Thin `{label:id, detail:title}` rows (OQ-2).
///
/// Order: `closed_since` already orders `closed_at ASC, id ASC` (deterministic, NFR-14).
async fn changelog(
    storage: &dyn Storage,
    since: Option<DateTime<Utc>>,
) -> Result<Vec<DiagnosticFinding>> {
    let closed = storage.closed_since(since).await?;
    Ok(closed
        .into_iter()
        .filter(|issue| !issue.is_template)
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

/// A `ListFilters` that counts the WHOLE store (closed + deferred included) — for `Info`.
fn all_inclusive_filters() -> ListFilters {
    ListFilters {
        include_closed: true,
        include_deferred: true,
        ..ListFilters::default()
    }
}

/// A `ListFilters` at the WIDEST visibility — closed + deferred + tombstone included — for the bd
/// stats scan (`list_stats_summary_issues` sees the whole table). `include_tombstone:true` (with
/// `include_closed:true`) routes to the all-statuses branch, so the per-status tally carries a
/// tombstone bucket and the widest hydration sees every row (pinned/epic/lead-time in-memory).
fn all_visibility_filters() -> ListFilters {
    ListFilters {
        include_closed: true,
        include_deferred: true,
        include_tombstone: true,
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
