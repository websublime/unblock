//! One-shot, best-effort `bd`/beads → unblock import (FR-26/D16/D24).
//!
//! The input is a `bd sync --flush-only`-produced `issues.jsonl` that the **user** generates with
//! their existing `bd` install; this crate runs NO external command (D13/NFR-6). unblock's
//! `Issue`/`Dependency`/`Comment`/enum serde is a byte-faithful field-for-field port of bd's, so the
//! struct-level field map is **identity** — a bd-export line deserializes directly into
//! [`unblock_model::Issue`] with zero renames.
//!
//! The real work is the **7-step bd import-normalize repair pass** (a faithful port of bd's
//! `normalize_issue`, `temp/beads_rust-main/src/sync/mod.rs:3813-3918`) — the repairs unblock's shared
//! [`crate::jsonl::normalize`] lacks — applied INSIDE this file **in bd's SOURCE ORDER** BEFORE the
//! `content_hash` recompute:
//!
//! 1. dep-type legacy-underscore repair — a **GENERAL** rule: for any `Custom` `dep_type`,
//!    `replace('_','-')` and adopt iff it parses to a known (non-`Custom`) [`DependencyType`] variant
//!    (`parent_child`/`conditional_blocks`/`waits_for`/`discovered_from`/`replies_to`/`relates_to`/
//!    `caused_by`; `parent_child`→`ParentChild` is the canonical EXAMPLE, not the whole rule);
//! 2. dependency dedup keep-latest per `(issue_id, depends_on_id, dep_type)`;
//! 3. terminal-status text aliases `done`/`complete`/`completed`/`finished`/`resolved`
//!    (`Status::Custom`) → `Closed` — BEFORE the `closed_at` repair (which tests `is_terminal()`);
//! 4. `-wisp-` id → `ephemeral = true`;
//! 5. `closed_at = updated_at` when terminal & `closed_at.is_none()`;
//! 6. `closed_at` cleared when non-terminal;
//! 7. `external_ref` blank/whitespace → `None` else trimmed.
//!
//! After the 7 repairs, [`map_bd_record`] composes with the SHARED [`crate::jsonl::normalize`]
//! (labels sort/dedup + `updated_at >= created_at` clamp + the `content_hash` recompute) so the
//! recomputed hash equals bd's stored hash byte-for-byte (bd folds those 3 shared steps into the SAME
//! `normalize_issue` pass) — FR-26 idempotency + cross-tool dedup (spine §1.8 NOTE, SF-4).
//!
//! `dropped_fields` = unknown TOP-LEVEL keys, via [`map_bd_record`] diffing the raw JSON keys against
//! the known `Issue`/`Dependency`/`Comment` key set BEFORE `from_value::<Issue>` (serde silently
//! discards unknowns — neither struct sets `deny_unknown_fields`). **bd ids are preserved verbatim**
//! (no remap/reject; `content_hash` excludes `id`, so preserving is idempotency-safe;
//! [`SyncError::PrefixMismatch`] stays a reserved v1 seam — remap is deferred to v1.1).
//!
//! FR-8 preflight (MF-4): because this path reads records via `map_bd_record(Value)` (NOT
//! `validate_records`), it re-runs every FR-8 guard on its own path BEFORE the shared
//! [`crate::import::apply_records`] tail: path confinement + conflict-marker rejection + the bounded
//! read (via [`crate::import::preflight_source`]), then per-line `IssueValidator::validate` + in-file
//! dup-id — ALL failures abort with ZERO DB writes.

use std::collections::HashSet;
use std::path::Path;

use serde_json::Value;
use unblock_model::{DependencyType, ImportReport, Issue, IssueValidator, Status};
use unblock_storage::Storage;

use crate::error::SyncError;
use crate::import::{ImportOptions, apply_records, preflight_source};
use crate::jsonl;

/// The terminal-status text aliases bd wrote as `Status::Custom` that map to `Closed` (repair 3).
///
/// Faithful to bd `normalize_issue` (`temp/beads_rust-main/src/sync/mod.rs:3873-3881`).
const TERMINAL_STATUS_ALIASES: &[&str] = &["done", "complete", "completed", "finished", "resolved"];

/// The serde-visible TOP-LEVEL keys of [`unblock_model::Issue`] (SF-6).
///
/// DERIVED from the serde field set INCLUDING the 3 relation-container keys
/// (`labels`/`dependencies`/`comments`), MINUS the `#[serde(skip)]` `content_hash`. A guard test
/// (`known_issue_keys_match_serde_field_set`) pins this to the actual wire shape so a future `Issue`
/// field cannot surface as a spurious `dropped_field`.
const KNOWN_ISSUE_KEYS: &[&str] = &[
    "id",
    "title",
    "description",
    "design",
    "acceptance_criteria",
    "notes",
    "status",
    "priority",
    "issue_type",
    "assignee",
    "owner",
    "estimated_minutes",
    "created_at",
    "created_by",
    "updated_at",
    "closed_at",
    "close_reason",
    "closed_by_session",
    "due_at",
    "defer_until",
    "external_ref",
    "source_system",
    "source_repo",
    "source_repo_path",
    "agent_context",
    "deleted_at",
    "deleted_by",
    "delete_reason",
    "original_type",
    "compaction_level",
    "compacted_at",
    "compacted_at_commit",
    "original_size",
    "sender",
    "ephemeral",
    "pinned",
    "is_template",
    "labels",
    "dependencies",
    "comments",
];

/// The serde-visible keys of [`unblock_model::Dependency`] (SF-6). `dep_type` serializes as `type`.
///
/// `dropped_fields` reports unknown TOP-LEVEL keys ONLY (D24/F4), so this set is not consulted at
/// runtime — it exists to PIN the derivation via the SF-6 drift guard, hence `#[cfg(test)]`.
#[cfg(test)]
const KNOWN_DEP_KEYS: &[&str] = &[
    "issue_id",
    "depends_on_id",
    "type",
    "created_at",
    "created_by",
    "metadata",
    "thread_id",
];

/// The serde-visible keys of [`unblock_model::Comment`] (SF-6). `body` serializes as `text`.
///
/// Test-only (see [`KNOWN_DEP_KEYS`]): `dropped_fields` is top-level keys only, so the comment key
/// set only backs the SF-6 drift guard.
#[cfg(test)]
const KNOWN_COMMENT_KEYS: &[&str] = &["id", "issue_id", "author", "text", "created_at"];

/// One-shot, best-effort `bd` → unblock import (FR-26/D16). **Skip-only** production semantics.
///
/// The bd-export `issues.jsonl` at `path` is imported into `storage`: every line is
/// [`map_bd_record`]-mapped (unknown top-level keys collected as `dropped_fields`, the 7 bd repairs
/// applied, the hash recomputed via the shared `normalize`), validated (`IssueValidator` + in-file
/// dup-id), then funneled through the shared atomic [`apply_records`] tail (tombstone-guard-first →
/// `sync_equals`-Skip → one `create_issues` tx). bd ids are preserved verbatim.
///
/// Synthesizes `ImportOptions { dry_run: false, allow_external: false, on_collision: Skip }` (no
/// `opts` — Skip-only, always applies). The engine holds the D14 write permit across the whole call.
/// `dependencies`/`comments` on the report count the relations/comments of the issues ACTUALLY
/// inserted (the applied subset), matching bd's applied-subset scoping (MF-2) — so an idempotent rerun
/// (all records Skipped) reports `dependencies=0, comments=0`.
///
/// # Errors
///
/// [`SyncError::PathTraversal`]/[`SyncError::ConflictMarkers`] at preflight; [`SyncError::JsonlParse`]
/// on a malformed line; [`SyncError::ValidationFailed`]/[`SyncError::DuplicateId`] on a bad/dup
/// record; the transparent `Storage` source if the atomic `create_issues` tx fails (rollback → ZERO
/// rows). ALL failures abort with ZERO DB writes.
pub async fn import_bd(
    storage: &dyn Storage,
    path: &Path,
    confine_root: &Path,
    actor: &str,
) -> Result<ImportReport, SyncError> {
    // (1)/(2) shared FR-8 preflight: path confinement + conflict-marker rejection (ZERO DB writes,
    //         Skip-only production semantics → allow_external is always false).
    let canonical = preflight_source(path, confine_root, false)?;

    // (3) per-line map + repair + validate + in-file dup-id — ALL failures abort with ZERO writes.
    let mapped = read_bd_records(&canonical)?;

    // (4)/(5) funnel into the shared atomic classify+create tail (Skip-only, always applies). The
    //         tail returns the relation/comment sums over the APPLIED subset (the records it actually
    //         inserts), not over every mapped record: it counts the relations/comments of the issues
    //         ACTUALLY inserted, matching bd's applied-subset scoping (bd's
    //         `record_imported_relation_counts`, mod.rs:4611-4614, runs ONLY on an applied
    //         Insert/Update, mod.rs:4563/4579 — NEVER on a Skip, mod.rs:4581). So an idempotent rerun
    //         (all Skipped) reports `dependencies=0, comments=0`.
    let opts = ImportOptions {
        dry_run: false,
        allow_external: false,
        on_collision: crate::import::CollisionPolicy::Skip,
    };
    let (mut report, applied) =
        apply_records(storage, mapped.records, mapped.dropped_fields, actor, &opts).await?;

    // Finalize the two relation counts on the report from the APPLIED subset (MF-2).
    report.dependencies = applied.dependencies;
    report.comments = applied.comments;
    Ok(report)
}

/// The mapped, repaired, validated bd records + the collected unknown-top-level `dropped_fields`.
struct MappedBd {
    records: Vec<Issue>,
    dropped_fields: Vec<String>,
}

/// Read every line of the (preflighted) bd export, mapping + repairing + validating each.
///
/// The reader mirrors [`crate::jsonl::validate_records`]'s bounded per-line read (MF-3) + the
/// fd-metadata file-size guard against [`crate::conflict::MAX_IMPORT_FILE_BYTES`] (the FORK-3/NFR-18
/// ingestion bound). Per-line: [`map_bd_record`] (unknown-key diff + `from_value` + 7 repairs + shared
/// `normalize`), then `IssueValidator::validate` + in-file dup-id — the FIRST surviving failure aborts
/// with ZERO DB writes (faithful to the generic path).
///
/// NOTE (V1): the fd-metadata guard here re-checks size/regular-file that `preflight_source` →
/// `ensure_no_conflict_markers` already ran on this same path. This double-check is DELIBERATE — it
/// mirrors the generic path exactly ([`crate::jsonl::validate_records`] re-runs the identical guard
/// after preflight, `jsonl.rs`). Both readers open their OWN guarded fd rather than threading a reader
/// out of `preflight_source`, keeping the FR-8 preflight a pure boolean gate (no fd handed across the
/// classify boundary). Deduping would break that symmetry for a micro-opt, so it is left as-is.
fn read_bd_records(path: &Path) -> Result<MappedBd, SyncError> {
    use std::io::BufReader;

    let meta = std::fs::metadata(path).map_err(|source| SyncError::Io {
        path: path.to_path_buf(),
        action: "reading metadata for",
        source,
    })?;
    if !meta.is_file() {
        return Err(SyncError::PathTraversal {
            path: path.to_path_buf(),
            reason: crate::path::PathReject::NonRegularFile,
        });
    }
    if meta.len() > crate::conflict::MAX_IMPORT_FILE_BYTES {
        return Err(SyncError::FileTooLarge {
            path: path.to_path_buf(),
            size: meta.len(),
            cap: crate::conflict::MAX_IMPORT_FILE_BYTES,
        });
    }
    let file = std::fs::File::open(path).map_err(|source| SyncError::Io {
        path: path.to_path_buf(),
        action: "opening",
        source,
    })?;
    let mut reader = BufReader::with_capacity(2 * 1024 * 1024, file);

    let mut records: Vec<Issue> = Vec::new();
    let mut dropped: Vec<String> = Vec::new();
    let mut seen_dropped: HashSet<String> = HashSet::new();
    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut line_no = 0usize;
    loop {
        line_no += 1;
        let read = crate::conflict::read_line_bounded(&mut reader, &mut buf, line_no, path)?;
        if read == 0 {
            break;
        }
        let Ok(text) = std::str::from_utf8(&buf) else {
            return Err(SyncError::ValidationFailed {
                line: line_no,
                detail: "line is not valid UTF-8".to_string(),
            });
        };
        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue; // blank lines are skipped.
        }

        let value: Value =
            serde_json::from_str(trimmed).map_err(|source| SyncError::JsonlParse {
                line: line_no,
                source,
            })?;
        let (issue, line_dropped) = map_bd_record(value).map_err(|err| match err {
            SyncError::JsonEncode { source } => SyncError::JsonlParse {
                line: line_no,
                source,
            },
            other => other,
        })?;
        // Collect unknown-top-level keys (deduped, stable insertion order).
        for key in line_dropped {
            if seen_dropped.insert(key.clone()) {
                dropped.push(key);
            }
        }
        // Per-line validation (FR-8): IssueValidator + in-file dup-id, ZERO writes on failure.
        IssueValidator::validate(&issue).map_err(|err| SyncError::ValidationFailed {
            line: line_no,
            detail: err.to_string(),
        })?;
        if !seen_ids.insert(issue.id.clone()) {
            return Err(SyncError::DuplicateId {
                line: line_no,
                id: issue.id,
            });
        }
        records.push(issue);
    }
    Ok(MappedBd {
        records,
        dropped_fields: dropped,
    })
}

/// Map one bd-export JSON `Value` into a repaired [`Issue`] + its unknown-top-level `dropped_fields`.
///
/// The key-diff runs BEFORE `from_value` (serde silently discards unknowns). After deserialization the
/// 7 bd repairs run in bd's SOURCE ORDER, then the shared [`crate::jsonl::normalize`] recomputes the
/// hash. `dropped_fields` are the raw top-level `Value` keys not in [`KNOWN_ISSUE_KEYS`] (nested
/// `Dependency`/`Comment` unknown keys are NOT surfaced — only unknown TOP-LEVEL keys per D24/F4).
///
/// # Errors
///
/// [`SyncError::JsonEncode`] if the `Value` fails to deserialize as an [`Issue`] (the caller re-maps
/// it to a line-numbered [`SyncError::JsonlParse`]).
///
/// Crate-internal (`import_bd` is the only public entry): the sole in-crate caller
/// ([`read_bd_records`]) feeds a `from_str`-bounded `Value`, so exposing the unbounded `from_value`
/// recursion surface (no 128-level limit) to external callers is intentionally avoided (V1/V4).
pub(crate) fn map_bd_record(value: Value) -> Result<(Issue, Vec<String>), SyncError> {
    // (a) diff the raw TOP-LEVEL keys against the known Issue key set BEFORE `from_value` (serde
    //     silently discards unknowns — Issue does not set `deny_unknown_fields`).
    let known: HashSet<&str> = KNOWN_ISSUE_KEYS.iter().copied().collect();
    let mut dropped: Vec<String> = Vec::new();
    if let Value::Object(map) = &value {
        for key in map.keys() {
            if !known.contains(key.as_str()) {
                dropped.push(key.clone());
            }
        }
    }

    // (b) deserialize into the domain model (identity field map).
    let mut issue: Issue =
        serde_json::from_value(value).map_err(|source| SyncError::JsonEncode { source })?;

    // (c) the 7 bd repairs, in bd's SOURCE ORDER (mod.rs:3813-3918).
    repair_dependency_types(&mut issue); // repair 1 (general underscore → kebab, adopt-if-parses).
    dedup_dependencies(&mut issue); // repair 2 (keep-latest per (issue_id, depends_on_id, dep_type)).
    repair_terminal_status_alias(&mut issue); // repair 3 — BEFORE the closed_at repairs (SF-2).
    repair_wisp_ephemeral(&mut issue); // repair 4.
    repair_closed_at_invariant(&mut issue); // repairs 5+6 (set when terminal & none / clear otherwise).
    repair_external_ref(&mut issue); // repair 7 (blank → None else trim).

    // (d) compose with the SHARED normalize (labels + updated_at>=created_at clamp + hash recompute)
    //     so the recomputed hash equals bd's stored hash byte-for-byte (SF-4). NOT a standalone
    //     recompute — bd folds these 3 steps into the same normalize_issue pass.
    jsonl::normalize(&mut issue);

    Ok((issue, dropped))
}

/// Repair 1 — general legacy-underscore dep-type repair (bd `mod.rs:3822-3832`).
///
/// For any `Custom` `dep_type`, `replace('_','-')` and ADOPT the result iff it parses to a known
/// (non-`Custom`) [`DependencyType`] variant. Repairs every legacy underscore form
/// (`parent_child`/`conditional_blocks`/`waits_for`/`discovered_from`/`replies_to`/`relates_to`/
/// `caused_by`) — `parse_lowercased` recognizes only kebab, so each lands in `Custom` first.
fn repair_dependency_types(issue: &mut Issue) {
    for dep in &mut issue.dependencies {
        if let DependencyType::Custom(custom) = &dep.dep_type {
            let candidate = custom.replace('_', "-");
            // `parse` is infallible for the open enum (unknown → Custom); adopt ONLY a known
            // (non-`Custom`) variant, faithful to bd `mod.rs:3826-3830`.
            if let Ok(normalized) = candidate.parse::<DependencyType>()
                && !matches!(normalized, DependencyType::Custom(_))
            {
                dep.dep_type = normalized;
            }
        }
    }
}

/// Repair 2 — dependency dedup keep-latest per `(issue_id, depends_on_id, dep_type)` (bd
/// `mod.rs:3834-3865`). Keeps the entry with the latest `created_at` per triple, preserving the
/// original relative order of the kept entries (stable by original index).
fn dedup_dependencies(issue: &mut Issue) {
    use std::collections::HashMap;

    if issue.dependencies.len() <= 1 {
        return;
    }
    // key → index of the currently-best (latest created_at) entry for that triple.
    let mut best: HashMap<(String, String, String), usize> = HashMap::new();
    for (i, dep) in issue.dependencies.iter().enumerate() {
        let key = (
            dep.issue_id.clone(),
            dep.depends_on_id.clone(),
            dep.dep_type.as_str().to_string(),
        );
        match best.get(&key) {
            // An existing entry is newer-or-equal → keep it (faithful to bd's `>=`).
            Some(&prev) if issue.dependencies[prev].created_at >= dep.created_at => {}
            _ => {
                best.insert(key, i);
            }
        }
    }
    if best.len() < issue.dependencies.len() {
        let mut keep: Vec<usize> = best.into_values().collect();
        keep.sort_unstable();
        issue.dependencies = keep
            .into_iter()
            .map(|i| issue.dependencies[i].clone())
            .collect();
    }
}

/// Repair 3 — terminal-status text aliases → `Closed` (bd `mod.rs:3873-3881`).
///
/// Runs BEFORE the `closed_at` repair (which tests `is_terminal()`, false for `Custom`) — SF-2.
fn repair_terminal_status_alias(issue: &mut Issue) {
    if let Status::Custom(raw) = &issue.status {
        let key = raw.trim().to_ascii_lowercase();
        if TERMINAL_STATUS_ALIASES.contains(&key.as_str()) {
            issue.status = Status::Closed;
        }
    }
}

/// Repair 4 — a `-wisp-` id marks the issue `ephemeral` (bd `mod.rs:3883-3886`).
fn repair_wisp_ephemeral(issue: &mut Issue) {
    if issue.id.contains("-wisp-") {
        issue.ephemeral = true;
    }
}

/// Repairs 5+6 — `closed_at` invariant (bd `mod.rs:3888-3896`).
///
/// Terminal (`Closed`/`Tombstone`) with `closed_at.is_none()` → `closed_at = updated_at`; a
/// non-terminal status clears `closed_at`. Tests `is_terminal()`, so it MUST run after repair 3.
fn repair_closed_at_invariant(issue: &mut Issue) {
    if issue.status.is_terminal() {
        if issue.closed_at.is_none() {
            issue.closed_at = Some(issue.updated_at);
        }
    } else {
        issue.closed_at = None;
    }
}

/// Repair 7 — `external_ref` blank/whitespace → `None`, else TRIM (bd `mod.rs:3898-3906`).
fn repair_external_ref(issue: &mut Issue) {
    if let Some(ext) = &issue.external_ref {
        if ext.trim().is_empty() {
            issue.external_ref = None;
        } else {
            issue.external_ref = Some(ext.trim().to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        KNOWN_COMMENT_KEYS, KNOWN_DEP_KEYS, KNOWN_ISSUE_KEYS, dedup_dependencies, map_bd_record,
        repair_closed_at_invariant, repair_dependency_types, repair_external_ref,
        repair_terminal_status_alias, repair_wisp_ephemeral,
    };
    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use std::collections::HashSet;
    use unblock_model::{Comment, Dependency, DependencyType, Issue, Status};

    fn ts(secs: i64) -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).unwrap()
    }

    fn base(id: &str) -> Issue {
        Issue {
            id: id.to_string(),
            title: format!("issue {id}"),
            status: Status::Open,
            created_at: ts(1_700_000_000),
            updated_at: ts(1_700_000_000),
            ..Issue::default()
        }
    }

    fn dep(target: &str, ty: DependencyType, secs: i64) -> Dependency {
        Dependency {
            issue_id: "bd-1".to_string(),
            depends_on_id: target.to_string(),
            dep_type: ty,
            created_at: ts(secs),
            created_by: None,
            metadata: None,
            thread_id: None,
        }
    }

    // ---- SF-6 drift guard: the hand-listed key sets equal the serde-visible field set. ----

    /// Serialize a FULLY-populated instance and collect its top-level object keys — the authoritative
    /// serde wire shape (`skip_serializing_if` fields are all non-empty here; `content_hash` is
    /// `#[serde(skip)]` so it never appears).
    fn serde_keys(value: &serde_json::Value) -> HashSet<String> {
        value.as_object().expect("object").keys().cloned().collect()
    }

    #[test]
    fn known_issue_keys_match_serde_field_set() {
        // Populate EVERY optional field so `skip_serializing_if` omits nothing.
        let mut issue = base("bd-1");
        issue.description = Some("d".into());
        issue.design = Some("de".into());
        issue.acceptance_criteria = Some("ac".into());
        issue.notes = Some("n".into());
        issue.assignee = Some("a".into());
        issue.owner = Some("o".into());
        issue.estimated_minutes = Some(1);
        issue.created_by = Some("c".into());
        issue.closed_at = Some(ts(1_700_000_100));
        issue.close_reason = Some("cr".into());
        issue.closed_by_session = Some("s".into());
        issue.due_at = Some(ts(1_700_000_200));
        issue.defer_until = Some(ts(1_700_000_300));
        issue.external_ref = Some("er".into());
        issue.source_system = Some("ss".into());
        issue.source_repo = Some("sr".into());
        issue.source_repo_path = Some("srp".into());
        issue.agent_context = Some("agc".into());
        issue.deleted_at = Some(ts(1_700_000_400));
        issue.deleted_by = Some("db".into());
        issue.delete_reason = Some("dr".into());
        issue.original_type = Some("ot".into());
        issue.compaction_level = Some(1);
        issue.compacted_at = Some(ts(1_700_000_500));
        issue.compacted_at_commit = Some("cac".into());
        issue.original_size = Some(2);
        issue.sender = Some("se".into());
        issue.ephemeral = true;
        issue.pinned = true;
        issue.is_template = true;
        issue.labels = vec!["l".into()];
        issue.dependencies = vec![dep("bd-2", DependencyType::Blocks, 1_700_000_000)];
        issue.comments = vec![Comment {
            id: 1,
            issue_id: "bd-1".into(),
            author: "auth".into(),
            body: "b".into(),
            created_at: ts(1_700_000_000),
        }];
        let value = serde_json::to_value(&issue).unwrap();
        let expected: HashSet<String> = KNOWN_ISSUE_KEYS.iter().map(|k| (*k).to_string()).collect();
        assert_eq!(
            serde_keys(&value),
            expected,
            "KNOWN_ISSUE_KEYS drifted from the serde field set"
        );
    }

    #[test]
    fn known_dep_keys_match_serde_field_set() {
        let d = Dependency {
            issue_id: "bd-1".into(),
            depends_on_id: "bd-2".into(),
            dep_type: DependencyType::Blocks,
            created_at: ts(1_700_000_000),
            created_by: Some("c".into()),
            metadata: Some("{}".into()),
            thread_id: Some("t".into()),
        };
        let value = serde_json::to_value(&d).unwrap();
        let expected: HashSet<String> = KNOWN_DEP_KEYS.iter().map(|k| (*k).to_string()).collect();
        assert_eq!(serde_keys(&value), expected);
    }

    #[test]
    fn known_comment_keys_match_serde_field_set() {
        let c = Comment {
            id: 1,
            issue_id: "bd-1".into(),
            author: "a".into(),
            body: "b".into(),
            created_at: ts(1_700_000_000),
        };
        let value = serde_json::to_value(&c).unwrap();
        let expected: HashSet<String> = KNOWN_COMMENT_KEYS
            .iter()
            .map(|k| (*k).to_string())
            .collect();
        assert_eq!(serde_keys(&value), expected);
    }

    // ---- per-repair unit tests (non-vacuous: each asserts the POST-repair field value). ----

    #[test]
    fn repair1_general_underscore_dep_types_adopt_kebab() {
        // Every legacy underscore form is repaired to its kebab variant (not just parent_child).
        let mut issue = base("bd-1");
        for (raw, expected) in [
            ("parent_child", DependencyType::ParentChild),
            ("conditional_blocks", DependencyType::ConditionalBlocks),
            ("waits_for", DependencyType::WaitsFor),
            ("discovered_from", DependencyType::DiscoveredFrom),
            ("replies_to", DependencyType::RepliesTo),
            ("relates_to", DependencyType::RelatesTo),
            ("caused_by", DependencyType::CausedBy),
        ] {
            issue.dependencies = vec![dep(
                "bd-2",
                DependencyType::Custom(raw.to_string()),
                1_700_000_000,
            )];
            repair_dependency_types(&mut issue);
            assert_eq!(issue.dependencies[0].dep_type, expected, "raw {raw}");
        }
    }

    #[test]
    fn repair1_unknown_underscore_stays_custom() {
        // A truly-unknown underscore form has no KNOWN kebab variant → the candidate parses back to
        // `Custom`, so the repair does NOT adopt it: the ORIGINAL value is preserved unchanged
        // (faithful to bd's adopt-only-if-non-Custom guard).
        let mut issue = base("bd-1");
        issue.dependencies = vec![dep(
            "bd-2",
            DependencyType::Custom("mentions_thing".to_string()),
            1_700_000_000,
        )];
        repair_dependency_types(&mut issue);
        assert_eq!(
            issue.dependencies[0].dep_type,
            DependencyType::Custom("mentions_thing".to_string()),
            "unknown underscore form is left untouched (not adopted)"
        );
    }

    #[test]
    fn repair2_dedup_keeps_latest_per_triple() {
        let mut issue = base("bd-1");
        // Two entries for the SAME triple (older + newer) + one distinct triple.
        issue.dependencies = vec![
            dep("bd-2", DependencyType::Blocks, 1_700_000_000),
            dep("bd-2", DependencyType::Blocks, 1_700_009_999), // newer, same triple.
            dep("bd-3", DependencyType::Blocks, 1_700_000_000), // distinct triple.
        ];
        dedup_dependencies(&mut issue);
        assert_eq!(issue.dependencies.len(), 2, "one duplicate removed");
        // The kept bd-2 entry is the LATEST.
        let bd2 = issue
            .dependencies
            .iter()
            .find(|d| d.depends_on_id == "bd-2")
            .unwrap();
        assert_eq!(bd2.created_at, ts(1_700_009_999));
    }

    #[test]
    fn repair3_terminal_status_aliases_map_to_closed() {
        for alias in [
            "done",
            "complete",
            "completed",
            "finished",
            "resolved",
            "DONE",
        ] {
            let mut issue = base("bd-1");
            issue.status = Status::Custom(alias.to_string());
            repair_terminal_status_alias(&mut issue);
            assert_eq!(issue.status, Status::Closed, "alias {alias}");
        }
    }

    #[test]
    fn repair3_non_alias_custom_status_unchanged() {
        let mut issue = base("bd-1");
        issue.status = Status::Custom("triage".to_string());
        repair_terminal_status_alias(&mut issue);
        assert_eq!(issue.status, Status::Custom("triage".to_string()));
    }

    #[test]
    fn repair4_wisp_id_marks_ephemeral() {
        let mut issue = base("bd-wisp-abc");
        assert!(!issue.ephemeral);
        repair_wisp_ephemeral(&mut issue);
        assert!(issue.ephemeral);
    }

    #[test]
    fn repair5_terminal_without_closed_at_sets_updated_at() {
        let mut issue = base("bd-1");
        issue.status = Status::Closed;
        issue.updated_at = ts(1_700_005_000);
        issue.closed_at = None;
        repair_closed_at_invariant(&mut issue);
        assert_eq!(issue.closed_at, Some(ts(1_700_005_000)));
    }

    #[test]
    fn repair6_non_terminal_clears_closed_at() {
        let mut issue = base("bd-1");
        issue.status = Status::Open;
        issue.closed_at = Some(ts(1_700_005_000));
        repair_closed_at_invariant(&mut issue);
        assert_eq!(issue.closed_at, None);
    }

    #[test]
    fn repair7_external_ref_blank_to_none_and_padded_to_trim() {
        // Blank → None.
        let mut blank = base("bd-1");
        blank.external_ref = Some("   ".to_string());
        repair_external_ref(&mut blank);
        assert_eq!(blank.external_ref, None);
        // Padded non-blank → trimmed (SF-3).
        let mut padded = base("bd-2");
        padded.external_ref = Some("  GH-42  ".to_string());
        repair_external_ref(&mut padded);
        assert_eq!(padded.external_ref.as_deref(), Some("GH-42"));
    }

    // ---- map_bd_record: unknown-key drop + repair-order composition. ----

    #[test]
    fn map_bd_record_collects_unknown_top_level_keys() {
        let value = json!({
            "id": "bd-1",
            "title": "t",
            "status": "open",
            "priority": 2,
            "issue_type": "task",
            "created_at": "2023-11-14T22:13:20Z",
            "updated_at": "2023-11-14T22:13:20Z",
            "some_future_bd_field": 123,
            "another_unknown": "x"
        });
        let (issue, dropped) = map_bd_record(value).unwrap();
        assert_eq!(issue.id, "bd-1");
        let set: HashSet<&str> = dropped.iter().map(String::as_str).collect();
        assert!(set.contains("some_future_bd_field"));
        assert!(set.contains("another_unknown"));
        assert_eq!(set.len(), 2, "only unknown keys dropped: {dropped:?}");
    }

    #[test]
    fn map_bd_record_runs_status_alias_before_closed_at_repair() {
        // A `done`-status record with closed_at absent: alias→Closed THEN closed_at←updated_at (SF-2).
        let value = json!({
            "id": "bd-1",
            "title": "t",
            "status": "done",
            "priority": 2,
            "issue_type": "task",
            "created_at": "2023-11-14T22:13:20Z",
            "updated_at": "2023-11-14T22:13:20Z"
        });
        let (issue, _) = map_bd_record(value).unwrap();
        assert_eq!(issue.status, Status::Closed);
        assert_eq!(
            issue.closed_at,
            Some(issue.updated_at),
            "closed_at repair must see the aliased terminal status"
        );
    }

    #[test]
    fn map_bd_record_recomputes_hash_via_shared_normalize() {
        let value = json!({
            "id": "bd-1",
            "title": "t",
            "status": "open",
            "priority": 2,
            "issue_type": "task",
            "created_at": "2023-11-14T22:13:20Z",
            "updated_at": "2023-11-14T22:13:20Z",
            "labels": ["b", "a", "a"]
        });
        let (issue, _) = map_bd_record(value).unwrap();
        // shared normalize sorts+dedups labels and sets content_hash.
        assert_eq!(issue.labels, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(issue.content_hash, Some(issue.compute_content_hash()));
    }
}
