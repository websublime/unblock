//! JSONL export orchestration (FR-7).
//!
//! Preflight the path → pull the FULL non-ephemeral corpus (incl. closed + tombstones via
//! `ListFilters.include_tombstone`, FORK-1/D23) → exclude ephemeral / `-wisp-` rows in-crate →
//! **widen that corpus back out to the transitive closure of its blockers (D45)** → order `id ASC` →
//! serialize each line (canonical timestamps, CF-TS) → atomic durable write.
//!
//! **The exporter DROPS NOTHING** (D45, spine §1.10): it carries no edge filter and never has. The
//! D23 corpus filter drops ROWS, and until D45 it did not drop the EDGES pointing at them — an edge
//! from a kept issue to an ephemeral or `-wisp-` issue was serialized on the kept issue's line while
//! its target's line was gone, so unblock's own exporter could emit a file the D45 write guard
//! refuses. The repair is the CORPUS widening in [`corpus_closed_under_blockers`], never an edge
//! filter: dropping the edge would silently convert BLOCKED work into READY work in the destination
//! workspace, which a data-integrity tool may not do.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use unblock_model::{ExportReport, Issue, ListFilters, is_external_target};
use unblock_storage::Storage;

use crate::atomic::write_atomic;
use crate::error::SyncError;
use crate::jsonl::serialize_issue_line;
use crate::path::validate_sync_path;

/// Options for [`export_jsonl`].
#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    /// Opt-in to write outside `confine_root` (NFR-7); default `false`.
    pub allow_external: bool,
    /// A written reason REQUIRED when `allow_external` is set (NFR-5/D30 forward seam). An
    /// `allow_external` override WITHOUT a reason is [`SyncError::ExternalOverrideWithoutReason`];
    /// when present, it rides the NFR-13 force-override INFO. Unreachable in v1 (`allow_external` is
    /// forced `false` by the engine).
    pub external_reason: Option<String>,
}

/// Export the store to `path` atomically (FR-7), returning the [`ExportReport`].
///
/// Pulls the full corpus (`include_closed=true, include_deferred=true, include_tombstone=true`),
/// excludes ephemeral rows + `-wisp-` ids, **re-widens that set to the transitive closure of its
/// blockers in BOTH directions** ([`corpus_closed_under_blockers`], D45), orders `id ASC`, and
/// serializes each with canonical timestamps. This is a READ + atomic write; the engine acquires no
/// write permit for it.
///
/// # Errors
///
/// [`SyncError::ExternalOverrideWithoutReason`] when `allow_external` is set without a written
/// reason (NFR-5/D30); [`SyncError::PathTraversal`] on a rejected path; [`SyncError::JsonEncode`] on
/// serialization; [`SyncError::Io`] on the atomic write; the transparent `Storage` source on a
/// backend read failure.
pub async fn export_jsonl(
    storage: &dyn Storage,
    path: &Path,
    confine_root: &Path,
    opts: &ExportOptions,
) -> Result<ExportReport, SyncError> {
    // NFR-5/D30 (forward seam): an `allow_external` override MUST carry a written reason. The reject
    // lives in the orchestrator (it holds the opts + knows the operation label).
    crate::path::reject_external_without_reason(
        path,
        opts.allow_external,
        opts.external_reason.as_deref(),
    )?;
    let canonical: PathBuf = validate_sync_path(path, confine_root, opts.allow_external)?;
    if opts.allow_external && !canonical.starts_with(confine_root) {
        // NFR-13 (D30): ONE INFO covering BOTH external-path use AND force-override (they coincide in
        // v1 — the only external-path mechanism IS the reason-gated `allow_external`). The reject
        // above guarantees `external_reason` is `Some` here.
        crate::reliability::reliability_guard!(
            operation = "export",
            path = canonical.display(),
            result = "external-path-force-override",
            reason = opts.external_reason.as_deref().unwrap_or_default(),
        );
    }

    // Full non-ephemeral corpus incl. closed + tombstones (FORK-1/D23).
    let filters = ListFilters {
        include_closed: true,
        include_deferred: true,
        include_tombstone: true,
        ..ListFilters::default()
    };
    let all = storage.list_issues(&filters).await?;

    // D23 row retain (ephemeral + `-wisp-` excluded IN-CRATE — storage has no ephemeral filter on
    // `list_issues`), THEN the D45 blocker closure widening it back out. Both live in one fn so the
    // retain can never ship without the closure that makes it safe.
    let (mut issues, widened) = corpus_closed_under_blockers(all);

    // NFR-13/D30 observability, contract-neutral: `ExportReport` gains NO field (it is a spine §1.10
    // DTO surfaced through `SyncOutput`, so a field change moves schema bytes). The widening is
    // reported on the existing `unblock.reliability` target instead — **CONDITIONAL on `n > 0`**,
    // exactly like the sibling `external-path-force-override` emission above. An unconditional
    // emitter would write a `0 row(s)` INFO on every export, and this repository re-exports
    // `.unblock/issues.jsonl` on every commit (spine §1.10, "FIRE CONDITION, PINNED").
    if widened > 0 {
        crate::reliability::reliability_guard!(
            operation = "export",
            path = canonical.display(),
            result = "blocker-closure-widened",
            reason = format!(
                "{widened} row(s) outside the corpus filter retained as dependency targets"
            ),
        );
    }

    // Deterministic byte order.
    issues.sort_by(|a, b| a.id.cmp(&b.id));

    let mut lines = Vec::with_capacity(issues.len());
    for issue in &issues {
        lines.push(serialize_issue_line(issue)?);
    }

    let written = write_atomic(&canonical, confine_root, lines.into_iter()).await?;
    Ok(ExportReport {
        written,
        path: canonical,
    })
}

/// Apply the D23 row retain to `all` and then **widen the retained set back out to the transitive
/// closure of its blockers** (D45, NORMATIVE — spine §1.10 / PRD §4 D45 clause (5)), returning the
/// final corpus plus the NUMBER OF ROWS THE CLOSURE ADDED (`0` when it added none — the fire
/// condition of the reliability emission).
///
/// # Why this exists at all
///
/// The D23 filter drops ROWS; it never dropped the EDGES pointing at them. **Dropping the edge was
/// examined and REJECTED on measured evidence** (Miguel's ruling, 2026-08-01): `live_blocked_ids`
/// pass 1 (`crates/unblock-storage/src/libsql/query.rs:288-294`) is a LEFT JOIN with **no ephemeral
/// exclusion**, so an issue blocked by an ephemeral row is BLOCKED today — an exporter that dropped
/// the edge would hand the destination workspace a READY issue that is not ready. So the CORPUS
/// widens instead, and the exporter drops nothing.
///
/// # The rule, stated over ROWS — and the four ways to get it wrong
///
/// A row the retain excluded **stops counting as excluded** the moment it stands in a NON-EXTERNAL
/// dependency relation with a row in the working set, **in EITHER DIRECTION**, transitively.
///
/// - **BOTH directions, and an OUT-only closure is NON-CONFORMING.** An edge is stored on exactly
///   ONE row (`dependencies.issue_id`) and hydrated `WHERE issue_id = ?1`
///   (`crates/unblock-storage/src/libsql/crud.rs:408`), so it is serialized only on THAT row's line
///   — but `live_blocked_ids` **pass 2** (`query.rs:305-317`) marks the epic **PARENT** blocked
///   because it has a non-terminal CHILD, with no ephemeral exclusion, while the `parent-child` edge
///   lives on the **CHILD's** line. A kept epic with an excluded open child is therefore BLOCKED
///   today and would arrive READY after an OUT-only round trip, with pass 3 propagating that
///   ready-ness down to every kept child. Hence **OUT** (a row in the working set names the excluded
///   row) *and* **IN** (the excluded row names a row in the working set). Both are UNIFORM over
///   every dependency type — a gating-only carve-out would recreate the per-edge-type special-casing
///   §1.9 abolishes and would still lose a non-gating edge stored on the excluded row's line.
/// - **A WORKLIST RE-SCANNED ON GROWTH, not an out-edge queue drain.** The shape below is a worklist
///   over the STILL-EXCLUDED rows, re-scanned whenever the working set grows. An OUT-only queue
///   drained once per newly-added id is not sufficient: under the IN direction a row becomes
///   eligible because some OTHER row was just pulled in, not because its own edges were visited.
/// - **Termination is structural.** The working set only GROWS over a finite row set and a pass that
///   adds nothing ends the walk, so a dependency CYCLE through excluded rows terminates by
///   construction — no visited-set bookkeeping and no recursion is needed or wanted.
/// - **A pulled-in row travels VERBATIM.** The closure changes WHICH rows are written, never WHAT a
///   row says: its `ephemeral` flag is serialized exactly as stored. Rewriting it would make the
///   destination workspace disagree with the source about a stored field.
/// - **An `external:` id pulls NOTHING and is pulled by NOTHING** in either direction, because it is
///   not a row at all (spine §1.9). The closure is over ids that could denote rows, which is why
///   every edge walked below is filtered through [`is_external_target`].
///
/// A DANGLING target (one naming no row anywhere) likewise pulls nothing — there is no row to pull —
/// and its edge is still WRITTEN: the exporter does not repair, and the guarded import then refuses
/// that file whole-batch. That refusal is the disclosed consequence, not a regression (spine §1.10).
fn corpus_closed_under_blockers(all: Vec<Issue>) -> (Vec<Issue>, usize) {
    // (1) The D23 row retain. `kept[i]` is the working set, by index into `all`.
    let mut kept: Vec<bool> = all
        .iter()
        .map(|issue| !issue.ephemeral && !issue.id.contains("-wisp-"))
        .collect();

    // (2) The D45 closure. Scoped so the borrows of `all` end before it is consumed below.
    let added = {
        // Row id -> index, for the IN direction (does this excluded row name a row in the set?).
        let index_of: HashMap<&str, usize> = all
            .iter()
            .enumerate()
            .map(|(i, issue)| (issue.id.as_str(), i))
            .collect();
        // Target id -> the rows naming it, for the OUT direction (does a row in the set name this
        // excluded row?). External targets are omitted: they can never denote a row.
        let mut targeted_by: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, issue) in all.iter().enumerate() {
            for dep in &issue.dependencies {
                if is_external_target(&dep.depends_on_id) {
                    continue;
                }
                targeted_by
                    .entry(dep.depends_on_id.as_str())
                    .or_default()
                    .push(i);
            }
        }

        // The worklist: the still-excluded rows. Re-scanned in full whenever the working set grew.
        let mut pending: Vec<usize> = (0..all.len()).filter(|&i| !kept[i]).collect();
        let mut added = 0usize;
        while !pending.is_empty() {
            let mut still_excluded: Vec<usize> = Vec::with_capacity(pending.len());
            let mut grew = false;
            for &i in &pending {
                // IN — this excluded row names a row in the working set.
                let names_kept = all[i].dependencies.iter().any(|dep| {
                    !is_external_target(&dep.depends_on_id)
                        && index_of
                            .get(dep.depends_on_id.as_str())
                            .is_some_and(|&j| kept[j])
                });
                // OUT — a row in the working set names this excluded row.
                let named_by_kept = targeted_by
                    .get(all[i].id.as_str())
                    .is_some_and(|sources| sources.iter().any(|&j| kept[j]));
                if names_kept || named_by_kept {
                    kept[i] = true;
                    added += 1;
                    grew = true;
                } else {
                    still_excluded.push(i);
                }
            }
            pending = still_excluded;
            if !grew {
                // A full pass added nothing: the fixed point is reached (and a cycle among the
                // remaining excluded rows exits HERE rather than looping).
                break;
            }
        }
        added
    };

    // (3) The final corpus, each surviving row VERBATIM (`ephemeral` included).
    let issues: Vec<Issue> = all
        .into_iter()
        .zip(kept)
        .filter_map(|(issue, keep)| keep.then_some(issue))
        .collect();
    (issues, added)
}

#[cfg(test)]
mod tests {
    use super::{ExportOptions, export_jsonl};
    use crate::error::SyncError;
    use crate::testutil::FakeStorage;
    use crate::testutil::sample_issue;

    #[tokio::test]
    async fn exports_n_lines_and_report_matches() {
        let dir = tempfile::tempdir().unwrap();
        let storage = FakeStorage::with_issues(vec![
            sample_issue("ub-2"),
            sample_issue("ub-1"),
            sample_issue("ub-3"),
        ]);
        let target = dir.path().join("issues.jsonl");
        let report = export_jsonl(&storage, &target, dir.path(), &ExportOptions::default())
            .await
            .expect("export");
        assert_eq!(report.written, 3);
        let content = std::fs::read_to_string(&target).unwrap();
        // 3 lines, id ASC order (ub-1, ub-2, ub-3).
        let ids: Vec<String> = content
            .lines()
            .map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).unwrap();
                v["id"].as_str().unwrap().to_string()
            })
            .collect();
        assert_eq!(ids, vec!["ub-1", "ub-2", "ub-3"]);
    }

    #[tokio::test]
    async fn excludes_ephemeral_and_wisp() {
        let dir = tempfile::tempdir().unwrap();
        let mut ephemeral = sample_issue("ub-eph");
        ephemeral.ephemeral = true;
        let storage = FakeStorage::with_issues(vec![
            sample_issue("ub-1"),
            ephemeral,
            sample_issue("ub-wisp-x"),
        ]);
        let target = dir.path().join("issues.jsonl");
        let report = export_jsonl(&storage, &target, dir.path(), &ExportOptions::default())
            .await
            .expect("export");
        assert_eq!(report.written, 1, "only the non-ephemeral non-wisp row");
        let content = std::fs::read_to_string(&target).unwrap();
        assert!(content.contains("ub-1"));
        assert!(!content.contains("ub-eph"));
        assert!(!content.contains("ub-wisp-x"));
    }

    #[tokio::test]
    async fn empty_db_exports_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let storage = FakeStorage::with_issues(vec![]);
        let target = dir.path().join("issues.jsonl");
        let report = export_jsonl(&storage, &target, dir.path(), &ExportOptions::default())
            .await
            .expect("export");
        assert_eq!(report.written, 0);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "");
    }

    #[tokio::test]
    async fn external_path_without_allow_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let storage = FakeStorage::with_issues(vec![sample_issue("ub-1")]);
        let target = other.path().join("issues.jsonl");
        let err = export_jsonl(&storage, &target, dir.path(), &ExportOptions::default())
            .await
            .expect_err("external rejected");
        assert!(matches!(err, SyncError::PathTraversal { .. }));
    }

    // ---- D45: the export corpus is CLOSED UNDER ITS BLOCKERS ------------------------------------
    //
    // Each cell below names the MUTANT it kills. A coverage claim is worth nothing until a mutant
    // proves it, and the five mutants the acceptance criteria name (no closure / an OUT-only closure
    // / a single-pass closure / a normalized `ephemeral` flag / a re-walk that hangs on a cycle) each
    // have their OWN cell here — a cell that passes in both the old and the new world is worse than
    // no cell at all.

    use unblock_model::{Dependency, DependencyType, Issue};

    /// An edge `source -> target` of `dep_type`, timestamped like [`sample_issue`].
    fn dep(source: &str, target: &str, dep_type: DependencyType) -> Dependency {
        Dependency {
            issue_id: source.to_string(),
            depends_on_id: target.to_string(),
            dep_type,
            created_at: chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 1, 1, 0, 0, 0)
                .unwrap(),
            created_by: Some("t".to_string()),
            metadata: None,
            thread_id: None,
        }
    }

    /// A sample issue carrying `deps` (the hydrated out-edge list, `WHERE issue_id = <id>`).
    fn issue_with_deps(id: &str, deps: Vec<Dependency>) -> Issue {
        Issue {
            dependencies: deps,
            ..sample_issue(id)
        }
    }

    /// An EPHEMERAL sample issue carrying `deps`.
    fn ephemeral_with_deps(id: &str, deps: Vec<Dependency>) -> Issue {
        Issue {
            ephemeral: true,
            ..issue_with_deps(id, deps)
        }
    }

    /// Export `issues` to a temp dir and return `(written, the exported lines as JSON values)`.
    async fn export_lines(issues: Vec<Issue>) -> (usize, Vec<serde_json::Value>) {
        let dir = tempfile::tempdir().unwrap();
        let storage = FakeStorage::with_issues(issues);
        let target = dir.path().join("issues.jsonl");
        let report = export_jsonl(&storage, &target, dir.path(), &ExportOptions::default())
            .await
            .expect("export");
        let content = std::fs::read_to_string(&target).unwrap();
        let values: Vec<serde_json::Value> = content
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        (report.written, values)
    }

    /// The exported ids, in file order.
    fn ids(lines: &[serde_json::Value]) -> Vec<String> {
        lines
            .iter()
            .map(|v| v["id"].as_str().unwrap().to_string())
            .collect()
    }

    /// The `depends_on_id`s serialized on the line for `id`.
    fn targets_of(lines: &[serde_json::Value], id: &str) -> Vec<String> {
        lines
            .iter()
            .find(|v| v["id"].as_str() == Some(id))
            .and_then(|v| v["dependencies"].as_array())
            .map(|deps| {
                deps.iter()
                    .map(|d| d["depends_on_id"].as_str().unwrap().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// **OUT direction.** A kept issue blocked by an EPHEMERAL row: the edge survives on the kept
    /// line AND the ephemeral target's own row is now a LINE in the file.
    ///
    /// MUTANT KILLED: deleting the closure pass entirely (`corpus_closed_under_blockers` reduced to
    /// the bare D23 retain) — the file then carries `ub-1`'s edge with no `ub-eph` line, i.e. exactly
    /// the un-importable export D45 exists to abolish.
    #[tokio::test]
    async fn an_ephemeral_blocker_of_a_kept_issue_is_exported() {
        let (written, lines) = export_lines(vec![
            issue_with_deps("ub-1", vec![dep("ub-1", "ub-eph", DependencyType::Blocks)]),
            ephemeral_with_deps("ub-eph", vec![]),
        ])
        .await;
        assert_eq!(written, 2, "the ephemeral blocker travels with its edge");
        assert_eq!(ids(&lines), vec!["ub-1", "ub-eph"]);
        assert_eq!(targets_of(&lines, "ub-1"), vec!["ub-eph"]);
    }

    /// The same rule for a `-wisp-` id, which the D23 retain excludes by a DIFFERENT predicate.
    ///
    /// MUTANT KILLED: a closure applied to the `ephemeral` flag only, leaving the `-wisp-` half of
    /// the retain unclosed.
    #[tokio::test]
    async fn a_wisp_blocker_of_a_kept_issue_is_exported() {
        let (written, lines) = export_lines(vec![
            issue_with_deps(
                "ub-1",
                vec![dep("ub-1", "ub-wisp-x", DependencyType::Blocks)],
            ),
            issue_with_deps("ub-wisp-x", vec![]),
        ])
        .await;
        assert_eq!(written, 2);
        assert_eq!(ids(&lines), vec!["ub-1", "ub-wisp-x"]);
    }

    /// **IN direction — the cell every other closure cell here passes WITHOUT.** A kept EPIC whose
    /// non-terminal `parent-child` CHILD is ephemeral: the edge is stored on the CHILD's row
    /// (`WHERE issue_id = ?1`), so the epic's own line names nothing and an OUT-only closure has
    /// nothing to follow — yet `live_blocked_ids` pass 2 blocks the epic PARENT through that very
    /// edge, so the epic would arrive READY in the destination.
    ///
    /// MUTANT KILLED: an OUT-only closure (delete the `names_kept` arm of
    /// `corpus_closed_under_blockers`, keeping `named_by_kept`).
    #[tokio::test]
    async fn an_ephemeral_child_of_a_kept_epic_is_exported_through_the_incoming_edge() {
        let (written, lines) = export_lines(vec![
            issue_with_deps("ub-epic", vec![]),
            ephemeral_with_deps(
                "ub-eph-child",
                vec![dep("ub-eph-child", "ub-epic", DependencyType::ParentChild)],
            ),
        ])
        .await;
        assert_eq!(
            written, 2,
            "the excluded child travels because it BLOCKS the kept epic"
        );
        assert_eq!(ids(&lines), vec!["ub-eph-child", "ub-epic"]);
        assert_eq!(targets_of(&lines, "ub-eph-child"), vec!["ub-epic"]);
    }

    /// The closure is TRANSITIVE over the OUT direction: a kept row depends on an ephemeral row that
    /// depends on a SECOND ephemeral row — both lines appear.
    ///
    /// **ORDER-HOSTILE BY CONSTRUCTION, and that is not decoration.** `list_issues` returns rows in
    /// id ASC order, so the naive id choice (`ub-e1 -> ub-e2`) puts the chain in FORWARD order and a
    /// single pass would pull both — a cell that passes in BOTH worlds, which is worse than no cell.
    /// The chain here runs `ub-1 -> ub-e-zzz -> ub-e-aaa`, so the tail is VISITED FIRST (while its
    /// only sponsor is still excluded) and can only join on a LATER pass. Verified by mutation: with
    /// the forward-ordered ids this cell stayed GREEN under the single-pass mutant.
    ///
    /// MUTANT KILLED: a single-pass closure (`if true || !grew { break }` — one pass over the
    /// still-excluded rows). `ub-e-aaa` is then dropped.
    #[tokio::test]
    async fn the_blocker_closure_is_transitive() {
        let (written, lines) = export_lines(vec![
            issue_with_deps(
                "ub-1",
                vec![dep("ub-1", "ub-e-zzz", DependencyType::Blocks)],
            ),
            ephemeral_with_deps(
                "ub-e-aaa",
                vec![], // the tail: reachable ONLY through ub-e-zzz, which is visited AFTER it
            ),
            ephemeral_with_deps(
                "ub-e-zzz",
                vec![dep("ub-e-zzz", "ub-e-aaa", DependencyType::Blocks)],
            ),
        ])
        .await;
        assert_eq!(written, 3);
        assert_eq!(ids(&lines), vec!["ub-1", "ub-e-aaa", "ub-e-zzz"]);
    }

    /// The same RE-SCAN-ON-GROWTH obligation through the IN direction: the row that becomes eligible
    /// sits EARLIER in the pre-retain list than the row that makes it eligible.
    ///
    /// MUTANT KILLED: an out-edge queue drained once (no re-scan of the still-excluded set).
    #[tokio::test]
    async fn the_closure_re_scans_the_still_excluded_rows_on_growth() {
        // `list_issues` returns id ASC, so `ub-e-aaa` is visited BEFORE `ub-e-zzz`; only `ub-e-zzz`
        // is directly reachable from the kept row, and `ub-e-aaa` hangs off it.
        let (written, lines) = export_lines(vec![
            issue_with_deps(
                "ub-1",
                vec![dep("ub-1", "ub-e-zzz", DependencyType::Blocks)],
            ),
            ephemeral_with_deps(
                "ub-e-aaa",
                vec![dep("ub-e-aaa", "ub-e-zzz", DependencyType::Blocks)],
            ),
            ephemeral_with_deps("ub-e-zzz", vec![]),
        ])
        .await;
        assert_eq!(written, 3);
        assert_eq!(ids(&lines), vec!["ub-1", "ub-e-aaa", "ub-e-zzz"]);
    }

    /// The walk terminates on a REACHABLE dependency CYCLE between two excluded rows, pulling BOTH
    /// in exactly once rather than re-adding them forever.
    ///
    /// **Honest scoping, verified by mutation:** this cell alone does NOT prove termination. Deleting
    /// the fixed-point exit (`if !grew { break }`) leaves it GREEN, because both cycle members join
    /// the working set and the worklist drains to empty. The termination proof is its sibling
    /// [`an_unreachable_cycle_among_excluded_rows_stays_excluded_and_terminates`], where the worklist
    /// NEVER empties — that is the cell the non-terminating mutant hangs.
    #[tokio::test]
    async fn the_closure_terminates_on_a_cycle_between_excluded_rows() {
        let (written, lines) = export_lines(vec![
            issue_with_deps("ub-1", vec![dep("ub-1", "ub-e1", DependencyType::Blocks)]),
            ephemeral_with_deps("ub-e1", vec![dep("ub-e1", "ub-e2", DependencyType::Blocks)]),
            ephemeral_with_deps("ub-e2", vec![dep("ub-e2", "ub-e1", DependencyType::Blocks)]),
        ])
        .await;
        assert_eq!(written, 3);
        assert_eq!(ids(&lines), vec!["ub-1", "ub-e1", "ub-e2"]);
    }

    /// **THE TERMINATION CELL.** A cycle among excluded rows that NOTHING kept reaches stays
    /// excluded, and the walk still ends — the fixed point exits on the first pass that adds nothing.
    ///
    /// MUTANT KILLED: any shape without that structural exit — a naive recursive re-walk of the whole
    /// set, or simply `if false && !grew { break }`. Here the worklist NEVER drains (nothing is ever
    /// added), so the walk spins forever and this cell HANGS. A hang IS the failure signal; verified
    /// by mutation (the process had to be killed after 60s, while its reachable-cycle sibling above
    /// stayed green).
    #[tokio::test]
    async fn an_unreachable_cycle_among_excluded_rows_stays_excluded_and_terminates() {
        let (written, lines) = export_lines(vec![
            issue_with_deps("ub-1", vec![]),
            ephemeral_with_deps("ub-e1", vec![dep("ub-e1", "ub-e2", DependencyType::Blocks)]),
            ephemeral_with_deps("ub-e2", vec![dep("ub-e2", "ub-e1", DependencyType::Blocks)]),
        ])
        .await;
        assert_eq!(written, 1);
        assert_eq!(ids(&lines), vec!["ub-1"]);
    }

    /// A pulled-in row is serialized VERBATIM — its `ephemeral` flag included. The closure changes
    /// WHICH rows are written, never WHAT a row says.
    ///
    /// MUTANT KILLED: normalizing the flag on the way out (`issue.ephemeral = false` for a pulled-in
    /// row). `ephemeral` is `skip_serializing_if = "is_false"`, so the mutant DROPS the key entirely
    /// and both assertions below go red.
    #[tokio::test]
    async fn a_pulled_in_row_keeps_its_ephemeral_flag_verbatim() {
        let (_, lines) = export_lines(vec![
            issue_with_deps("ub-1", vec![dep("ub-1", "ub-eph", DependencyType::Blocks)]),
            ephemeral_with_deps("ub-eph", vec![]),
        ])
        .await;
        let pulled = lines
            .iter()
            .find(|v| v["id"].as_str() == Some("ub-eph"))
            .expect("the pulled-in row has a line");
        assert!(
            pulled.get("ephemeral").is_some(),
            "the `ephemeral` key must survive — normalizing it to false drops it (is_false)"
        );
        assert_eq!(pulled["ephemeral"].as_bool(), Some(true));
    }

    /// An `external:` id pulls NOTHING and is pulled by NOTHING — it is not a row at all (spine
    /// §1.9). The edge itself still travels on the kept line.
    ///
    /// **NO MUTANT — and saying so is the point.** Removing the
    /// [`is_external_target`](unblock_model::is_external_target) filter from the closure walk is an
    /// EQUIVALENT mutant: an `external:`-prefixed string can never BE a row id, so with the filter
    /// gone neither closure lookup can hit, the walk pulls the same rows, and no executable can
    /// distinguish the two builds. A "MUTANT KILLED" line here would be a false coverage claim, and
    /// this repository has shipped those before. What the filter buys is intent and cost, not
    /// behaviour — it states in code that an external id is not a candidate row, and it skips a
    /// lookup per external edge.
    ///
    /// What this cell DOES pin, and it is not nothing: the exporter DROPS NOTHING. Both external
    /// edges survive verbatim on the kept line, in BOTH spellings, and the corpus gains no line —
    /// so a build that "helpfully" filtered external edges out of the serialized set, or that
    /// invented a row for one, goes red here.
    #[tokio::test]
    async fn an_external_target_survives_as_an_edge_and_pulls_no_line() {
        let (written, lines) = export_lines(vec![issue_with_deps(
            "ub-1",
            vec![
                dep("ub-1", "external:jira-1", DependencyType::Blocks),
                dep("ub-1", "EXTERNAL:jira-2", DependencyType::Blocks),
            ],
        )])
        .await;
        assert_eq!(written, 1, "an external target has no row to serialize");
        assert_eq!(
            targets_of(&lines, "ub-1"),
            vec!["external:jira-1", "EXTERNAL:jira-2"],
            "the exporter drops NOTHING — both external edges survive verbatim"
        );
    }

    /// **The DISCLOSED CONSEQUENCE, pinned as such.** An edge whose target names no row ANYWHERE is
    /// still WRITTEN: the exporter does not repair, so it can emit a file its own guarded import
    /// refuses. The named remedy is the `dangling` diagnostic, not a silent edge-drop.
    ///
    /// MUTANT KILLED: ANY edge filter at all (the "obvious" repair D45 rejects) — the target list
    /// below empties and the cell goes red.
    #[tokio::test]
    async fn an_already_dangling_edge_is_still_written() {
        let (written, lines) = export_lines(vec![issue_with_deps(
            "ub-1",
            vec![dep("ub-1", "ub-ghost", DependencyType::Blocks)],
        )])
        .await;
        assert_eq!(written, 1, "a dangling target is no row — nothing to pull");
        assert_eq!(targets_of(&lines, "ub-1"), vec!["ub-ghost"]);
    }

    /// A kept row's edge to ANOTHER KEPT row changes nothing (the closure adds no row), and an
    /// excluded row nothing relates to stays excluded — the corpus is still NARROWER than "every
    /// row", which is why the `dangling` diagnostic's corpus must never be conflated with it.
    #[tokio::test]
    async fn an_unrelated_excluded_row_stays_excluded() {
        let (written, lines) = export_lines(vec![
            issue_with_deps("ub-1", vec![dep("ub-1", "ub-2", DependencyType::Blocks)]),
            issue_with_deps("ub-2", vec![]),
            ephemeral_with_deps("ub-eph", vec![]),
        ])
        .await;
        assert_eq!(written, 2);
        assert_eq!(ids(&lines), vec!["ub-1", "ub-2"]);
    }
}
