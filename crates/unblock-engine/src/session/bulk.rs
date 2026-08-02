//! The `create_bulk` 2-phase intra-file resolution + topological mint ordering (D22/T2.3).
//!
//! A faithful-but-STRICTER port of the original bulk-import resolution
//! (`temp/beads_rust-main/src/cli/commands/create.rs:855`–`1378`): the original SKIPPED each bad
//! record (`eprintln!` + `continue`) and created the rest; unblock refuses the WHOLE batch with one
//! `ValidationFailed` and ZERO writes (NFR-8 — the safe-import discipline wins over the port). The
//! symbolic-reference resolution ORDER is preserved verbatim: **stand-in id → title → pre-existing
//! storage id**, case-insensitive (`lookup_import_reference`, original `create.rs:1347`).
//!
//! This module owns the PURE / in-memory pieces (the maps, the topological order, the reference
//! resolution + the reject set); the stateful mint (which probes storage under the write permit) and
//! the single `storage.create_issues` tx live in `write.rs::create_bulk`, which drives this.

use std::collections::HashMap;

use unblock_error::FieldError;

use crate::session::write::NewIssue;

/// A symbolic reference resolves to exactly one batch record, several (ambiguous), or none.
pub(crate) enum RefResolution {
    /// Resolved to a single batch-record index.
    Resolved(usize),
    /// Matched more than one batch record (ambiguous — rejected).
    Ambiguous,
}

/// The case-insensitive title / stand-in indices over a batch (built once, consulted during mint +
/// dep resolution). Mirrors the original's `title_to_ids` / `standin_to_ids` (`create.rs:827`).
pub(crate) struct BatchMaps {
    title_to_indices: HashMap<String, Vec<usize>>,
    standin_to_indices: HashMap<String, Vec<usize>>,
}

impl BatchMaps {
    /// Build the title + stand-in indices over `records` (case-insensitive keys, trimmed; empty keys
    /// dropped — faithful to `create.rs:832`–`853`).
    pub(crate) fn build(records: &[NewIssue]) -> Self {
        let mut title_to_indices: HashMap<String, Vec<usize>> = HashMap::new();
        let mut standin_to_indices: HashMap<String, Vec<usize>> = HashMap::new();
        for (idx, record) in records.iter().enumerate() {
            let title_key = record.title.trim().to_lowercase();
            if !title_key.is_empty() {
                title_to_indices.entry(title_key).or_default().push(idx);
            }
            if let Some(sid) = record.stand_in_id.as_ref() {
                let sid_key = sid.trim().to_lowercase();
                if !sid_key.is_empty() {
                    standin_to_indices.entry(sid_key).or_default().push(idx);
                }
            }
        }
        Self {
            title_to_indices,
            standin_to_indices,
        }
    }

    /// Resolve a symbolic reference against the batch in the original ORDER **stand-in id → title**
    /// (case-insensitive). `None` means no intra-batch match (the caller then tries storage).
    pub(crate) fn lookup(&self, reference: &str) -> Option<RefResolution> {
        let key = reference.trim().to_lowercase();
        if key.is_empty() {
            return None;
        }
        let indices = self
            .standin_to_indices
            .get(&key)
            .or_else(|| self.title_to_indices.get(&key))?;
        match indices.as_slice() {
            [single] => Some(RefResolution::Resolved(*single)),
            _ => Some(RefResolution::Ambiguous),
        }
    }

    /// Whether a record's `parent` ref matches ANOTHER batch record (an intra-batch parent edge). A
    /// `None` parent or a parent matching no batch record (pre-existing storage parent) returns false.
    pub(crate) fn lookup_is_intra_batch(&self, record: &NewIssue) -> bool {
        record
            .parent
            .as_deref()
            .is_some_and(|parent_ref| self.lookup(parent_ref).is_some())
    }
}

/// The id-half of a dependency reference string (the storage-probe candidate). For `external:…` /
/// bare ids this is the whole string; for a valid `type:id` it is the id half; for an invalid type
/// prefix (a title with a colon) it is the whole string — faithful to `parse_dependency`.
pub(crate) fn dep_ref_id(dep_str: &str) -> String {
    let (_, id) = parse_dependency(dep_str);
    id
}

/// Order the records **parent-before-child** over the intra-batch parent edges (topological), or
/// reject the whole batch on a parent cycle / ambiguous parent ref (D22/T2.3).
///
/// A record's `parent` is an INTRA-BATCH edge only when it resolves (via [`BatchMaps::lookup`]) to
/// another batch record; a `parent` that matches no batch record is a pre-existing-storage parent
/// (no intra-batch edge — it can mint in any order). An AMBIGUOUS parent ref rejects the batch; a
/// PARENT CYCLE among intra-batch records has no valid mint order → reject.
///
/// Returns the record indices in a valid mint order (parents first). The reject errors map to the
/// engine `ValidationFailed` aggregate.
pub(crate) fn topological_mint_order(
    records: &[NewIssue],
    maps: &BatchMaps,
) -> Result<Vec<usize>, Vec<FieldError>> {
    let n = records.len();
    // child idx -> parent idx (intra-batch parent edges only).
    let mut parent_of: HashMap<usize, usize> = HashMap::new();
    for (idx, record) in records.iter().enumerate() {
        let Some(parent_ref) = record.parent.as_deref() else {
            continue;
        };
        match maps.lookup(parent_ref) {
            Some(RefResolution::Resolved(parent_idx)) => {
                if parent_idx == idx {
                    return Err(vec![FieldError::new(
                        "parent",
                        format!(
                            "record `{}` is its own parent (`{}`)",
                            display(&record.title),
                            display(parent_ref)
                        ),
                    )]);
                }
                parent_of.insert(idx, parent_idx);
            }
            Some(RefResolution::Ambiguous) => {
                return Err(vec![FieldError::new(
                    "parent",
                    format!(
                        "ambiguous parent reference `{}` for record `{}` (matches >1 record)",
                        display(parent_ref),
                        display(&record.title)
                    ),
                )]);
            }
            // No intra-batch match → a pre-existing-storage parent (no intra-batch edge).
            None => {}
        }
    }

    // Kahn's algorithm over the child->parent edges: a parent must come BEFORE its child. Edge
    // child -> parent means child depends on parent; we emit a node once its parent (if any) is done.
    // children_of[parent] = the records whose parent is this record.
    let mut children_of: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut indegree = vec![0usize; n];
    for (&child, &parent) in &parent_of {
        children_of.entry(parent).or_default().push(child);
        indegree[child] += 1; // each child has at most one intra-batch parent → indegree 0 or 1.
    }

    // Seed the queue with every record that has no intra-batch parent, in FILE ORDER (so records with
    // a pre-existing-storage parent or no parent keep their original order — faithful to the original
    // file-order creation, `create.rs:855`).
    let mut order = Vec::with_capacity(n);
    let mut queue: Vec<usize> = (0..n).filter(|&i| indegree[i] == 0).collect();
    let mut head = 0;
    while head < queue.len() {
        let node = queue[head];
        head += 1;
        order.push(node);
        if let Some(children) = children_of.get(&node) {
            // Emit children in file order for determinism.
            let mut sorted = children.clone();
            sorted.sort_unstable();
            for &child in &sorted {
                indegree[child] -= 1;
                if indegree[child] == 0 {
                    queue.push(child);
                }
            }
        }
    }

    if order.len() != n {
        // Some records never reached indegree 0 → a parent cycle among intra-batch records.
        return Err(vec![FieldError::new(
            "parent",
            "intra-batch parent cycle: the records cannot be ordered parent-before-child"
                .to_string(),
        )]);
    }
    Ok(order)
}

/// Resolve a record's symbolic `parent` ref to a concrete parent id for MINTING `parent.N`, using the
/// already-minted ids (intra-batch) then a pre-existing storage id. Returns `Ok(None)` for a root
/// record (no parent). An ambiguous/unresolved parent rejects the batch.
///
/// `minted_id_of[idx]` is the minted id for each batch record (populated as records mint in
/// topological order; a parent always mints before its child, so its entry is present here).
/// `storage_has(id)` reports whether a pre-existing storage row exists for a non-intra-batch parent.
pub(crate) fn resolve_parent_id(
    record: &NewIssue,
    maps: &BatchMaps,
    minted_id_of: &HashMap<usize, String>,
    storage_parent_id: Option<String>,
) -> Result<Option<String>, Vec<FieldError>> {
    let Some(parent_ref) = record.parent.as_deref() else {
        return Ok(None);
    };
    match maps.lookup(parent_ref) {
        Some(RefResolution::Resolved(parent_idx)) => {
            // The parent is a batch record minted earlier (topological order guarantees this).
            minted_id_of.get(&parent_idx).cloned().map_or_else(
                || {
                    Err(vec![FieldError::new(
                        "parent",
                        format!(
                            "intra-batch parent `{}` was not minted before its child",
                            display(parent_ref)
                        ),
                    )])
                },
                |id| Ok(Some(id)),
            )
        }
        Some(RefResolution::Ambiguous) => Err(vec![FieldError::new(
            "parent",
            format!(
                "ambiguous parent reference `{}` (matches >1 record)",
                display(parent_ref)
            ),
        )]),
        None => {
            // No intra-batch match → must resolve against pre-existing storage.
            storage_parent_id.map_or_else(
                || {
                    Err(vec![FieldError::new(
                        "parent",
                        format!(
                            "unresolved parent reference `{}` (no batch or storage match)",
                            display(parent_ref)
                        ),
                    )])
                },
                |id| Ok(Some(id)),
            )
        }
    }
}

/// Whether a dependency-type string is a recognized type (faithful to
/// `markdown_import.rs::validate_dependency_type`): a standard `DependencyType` is valid; the legacy
/// `blocked-by` alias is valid; any other `Custom` (unrecognized) is invalid. Because `from_str` is
/// infallible (open enum → `Custom`), a `Custom(_)` that is not `blocked-by` is the "invalid" case.
fn validate_dependency_type(dep_type: &str) -> bool {
    use unblock_model::DependencyType;
    if dep_type.eq_ignore_ascii_case("blocked-by") {
        return true;
    }
    !matches!(parse_dep_type(dep_type), DependencyType::Custom(_))
}

/// Parse a dependency-type string into a [`DependencyType`] (infallible — unknown → `Custom`).
fn parse_dep_type(dep_type: &str) -> unblock_model::DependencyType {
    use std::str::FromStr;
    // `DependencyType::from_str` is infallible (the Err type is fixed by the spine but never taken).
    unblock_model::DependencyType::from_str(dep_type)
        .unwrap_or_else(|_| unblock_model::DependencyType::Custom(dep_type.to_lowercase()))
}

/// Parse a dependency reference string into `(type, id)` — faithful to
/// `markdown_import.rs::parse_dependency`: `external:…` and bare ids default to `blocks`; a valid
/// `type:id` keeps the type; an INVALID type prefix is treated as part of the id (a title with a
/// colon) with the default `blocks` type. The `blocked-by` type string is PRESERVED verbatim here —
/// the engine flips it to `blocks` at the edge-build step (`resolve_dep_refs`).
///
/// **D45 — the external test is [`unblock_model::is_external_target`], the ONE predicate (spine
/// §1.9), never an open-coded case-SENSITIVE `starts_with("external:")`.** Stated precisely, because
/// an implementer who edits only this line ships nothing: for `EXTERNAL:jira-1` the shipped code
/// already fell to the `split_once` branch, found `validate_dependency_type("EXTERNAL")` FALSE (it
/// resolves to `DependencyType::Custom`) and returned `("blocks", "EXTERNAL:jira-1")` — byte-identical
/// to the external branch modulo `trim`. So the swap is observationally a NO-OP AT THIS SITE; it is
/// required because a single predicate is the only way to stop two dialects of one concept from
/// re-diverging. What actually delivers the relaxation is the carve-out in [`resolve_dep_refs`] and
/// the matching skip in the engine's pre-transaction probe.
fn parse_dependency(dep_str: &str) -> (String, String) {
    if unblock_model::is_external_target(dep_str) {
        ("blocks".to_string(), dep_str.to_string())
    } else if let Some((type_part, id_part)) = dep_str.split_once(':') {
        let type_part = type_part.trim();
        let id_part = id_part.trim();
        if validate_dependency_type(type_part) {
            (type_part.to_string(), id_part.to_string())
        } else {
            ("blocks".to_string(), dep_str.trim().to_string())
        }
    } else {
        ("blocks".to_string(), dep_str.to_string())
    }
}

/// A marker-only dependency token (a bare `-`/`*`/`+` after trim), faithful to
/// `is_marker_only_dependency` (`create.rs:1376`).
fn is_marker_only(dep_id: &str) -> bool {
    matches!(dep_id.trim(), "-" | "*" | "+")
}

/// A single resolved dependency edge built by [`resolve_dep_refs`].
pub(crate) struct ResolvedEdge {
    /// The resolved `depends_on_id` (a minted batch id or a pre-existing storage id).
    pub depends_on_id: String,
    /// The dependency type (the `blocked-by` alias already flipped to `blocks`).
    pub dep_type: unblock_model::DependencyType,
}

/// Resolve one record's verbatim `dep_refs` into concrete edges, or reject the WHOLE batch on any
/// ambiguous / unresolved / self-dependency / marker-only ref (the reject set, D22/T2.3).
///
/// The resolution mirrors the original (`create.rs:1164`–`1252`): first try the RAW string against
/// the intra-batch maps (handles titles containing colons); else `parse_dependency` it and resolve
/// the id half (stand-in → title → storage), flipping `blocked-by`→`blocks` at the edge-build step.
///
/// `own_id` is the resolving record's own minted id (for the self-dependency check). `minted_id_of`
/// maps batch-record index → minted id. `storage_resolve` reports the storage id for a non-intra-batch
/// reference (`Some(id)` if a pre-existing row resolves, `None` if it does not).
///
/// # D45 — the EXTERNAL carve-out (spine §5.2 rejection-set item (b), NORMATIVE)
///
/// A `dep_ref` for which [`unblock_model::is_external_target`] holds is **NOT resolved against
/// anything**: it is carried VERBATIM as the edge target and can never be "unresolved". The check
/// runs BEFORE the intra-batch map lookup for exactly that reason, and again on the id-half after
/// [`parse_dependency`] so `blocks:external:jira-1` is covered too.
///
/// **This is a stated RELAXATION of a GA-shipped, spine-pinned rejection, not a clarification.**
/// Until D45 `parse_dependency` kept the whole `external:…` string as the id, the engine's storage
/// probe probed it as an issue id, missed, and this resolver rejected the ENTIRE batch — so
/// `create_bulk` was the one path that refused a legitimate external blocker, contradicting the
/// external-targets-are-legitimate premise the rest of the system is built on. **No test covered that
/// behaviour, so nothing in CI would have gone red to announce the change**; the cells in
/// `tests/create_bulk.rs` are what now pin it in both spellings.
///
/// The remaining unresolved-reference rejection keeps `ValidationFailed` (a resolution fault),
/// distinct from D45's resolved-but-absent id, which is `BlockerNotFound`/`ISSUE_NOT_FOUND` at L2.
// One cohesive per-ref resolution ladder (external carve-out -> intra-batch -> parse -> storage ->
// the reject set), faithful to `create.rs:1164-1252`. Splitting it would scatter a rejection ORDER
// that is itself the contract.
#[allow(clippy::too_many_lines)]
pub(crate) fn resolve_dep_refs(
    record: &NewIssue,
    own_id: &str,
    maps: &BatchMaps,
    minted_id_of: &HashMap<usize, String>,
    storage_resolve: &HashMap<String, Option<String>>,
) -> Result<Vec<ResolvedEdge>, Vec<FieldError>> {
    let mut edges = Vec::new();
    for dep_str in &record.dep_refs {
        // (0) D45 — an EXTERNAL target is resolved against NOTHING and carried verbatim. It precedes
        // the intra-batch lookup because "not resolved against anything" is the literal rule.
        if unblock_model::is_external_target(dep_str) {
            edges.push(ResolvedEdge {
                depends_on_id: dep_str.clone(),
                dep_type: unblock_model::DependencyType::Blocks,
            });
            continue;
        }
        // (1) Raw string against the intra-batch maps (titles with colons resolve here).
        let (dep_type, resolved_id): (unblock_model::DependencyType, String) = if let Some(
            resolution,
        ) =
            maps.lookup(dep_str)
        {
            match resolution {
                RefResolution::Resolved(idx) => {
                    let id = minted_id_of.get(&idx).cloned().ok_or_else(|| {
                        vec![FieldError::new(
                            "dependencies",
                            format!(
                                "intra-batch dependency `{}` was not minted",
                                display(dep_str)
                            ),
                        )]
                    })?;
                    (unblock_model::DependencyType::Blocks, id)
                }
                RefResolution::Ambiguous => {
                    return Err(vec![FieldError::new(
                        "dependencies",
                        format!(
                            "ambiguous dependency reference `{}` (matches >1 record)",
                            display(dep_str)
                        ),
                    )]);
                }
            }
        } else {
            // (2) Parse as type:id / bare / external, then resolve the id (stand-in → title → storage).
            let (ty_str, dep_id) = parse_dependency(dep_str);
            // The `blocked-by` alias flip happens HERE (edge build), not in the parser.
            let ty = if ty_str.eq_ignore_ascii_case("blocked-by") {
                unblock_model::DependencyType::Blocks
            } else {
                parse_dep_type(&ty_str)
            };
            // D45 — the same carve-out on the ID HALF, so an explicitly typed
            // `waits-for:external:jira-1` is carried verbatim with ITS type. Reached only when the
            // whole ref was not already external (arm (0) above).
            if unblock_model::is_external_target(&dep_id) {
                edges.push(ResolvedEdge {
                    depends_on_id: dep_id,
                    dep_type: ty,
                });
                continue;
            }
            let resolved = match maps.lookup(&dep_id) {
                Some(RefResolution::Resolved(idx)) => {
                    minted_id_of.get(&idx).cloned().ok_or_else(|| {
                        vec![FieldError::new(
                            "dependencies",
                            format!(
                                "intra-batch dependency `{}` was not minted",
                                display(&dep_id)
                            ),
                        )]
                    })?
                }
                Some(RefResolution::Ambiguous) => {
                    return Err(vec![FieldError::new(
                        "dependencies",
                        format!(
                            "ambiguous dependency reference `{}` (matches >1 record)",
                            display(&dep_id)
                        ),
                    )]);
                }
                None => match storage_resolve.get(&dep_id) {
                    Some(Some(id)) => id.clone(),
                    _ => {
                        return Err(vec![FieldError::new(
                            "dependencies",
                            format!(
                                "unresolved dependency reference `{}` (no batch or storage match)",
                                display(&dep_id)
                            ),
                        )]);
                    }
                },
            };
            (ty, resolved)
        };

        // (3) The reject set: self-dependency + marker-only.
        if resolved_id == own_id {
            return Err(vec![FieldError::new(
                "dependencies",
                format!(
                    "self-dependency: record `{}` depends on itself",
                    display(own_id)
                ),
            )]);
        }
        if is_marker_only(&resolved_id) {
            return Err(vec![FieldError::new(
                "dependencies",
                format!("marker-only dependency `{resolved_id}` is not a valid reference"),
            )]);
        }

        edges.push(ResolvedEdge {
            depends_on_id: resolved_id,
            dep_type,
        });
    }
    Ok(edges)
}

/// Trim a label/title for an error message so a pathological value cannot blow up the message.
fn display(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() > 80 {
        let prefix: String = trimmed.chars().take(80).collect();
        format!("{prefix}…")
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{BatchMaps, RefResolution, topological_mint_order};
    use crate::session::write::NewIssue;

    fn rec(title: &str, stand_in: Option<&str>, parent: Option<&str>) -> NewIssue {
        NewIssue {
            title: title.to_string(),
            stand_in_id: stand_in.map(str::to_string),
            parent: parent.map(str::to_string),
            ..NewIssue::default()
        }
    }

    #[test]
    fn lookup_prefers_standin_over_title_case_insensitive() {
        let records = vec![rec("Build DB", Some("db-1"), None)];
        let maps = BatchMaps::build(&records);
        assert!(matches!(
            maps.lookup("DB-1"),
            Some(RefResolution::Resolved(0))
        ));
        assert!(matches!(
            maps.lookup("build db"),
            Some(RefResolution::Resolved(0))
        ));
        assert!(maps.lookup("missing").is_none());
    }

    #[test]
    fn lookup_ambiguous_title() {
        let records = vec![rec("Same", None, None), rec("Same", None, None)];
        let maps = BatchMaps::build(&records);
        assert!(matches!(
            maps.lookup("same"),
            Some(RefResolution::Ambiguous)
        ));
    }

    #[test]
    fn topological_order_parent_before_child() {
        // Child placed BEFORE its parent in file order; the order must still emit the parent first.
        let records = vec![
            rec("Child", None, Some("Parent")),
            rec("Parent", None, None),
        ];
        let maps = BatchMaps::build(&records);
        let order = topological_mint_order(&records, &maps).expect("orderable");
        let parent_pos = order.iter().position(|&i| i == 1).unwrap();
        let child_pos = order.iter().position(|&i| i == 0).unwrap();
        assert!(parent_pos < child_pos, "parent must mint before child");
    }

    #[test]
    fn topological_order_rejects_parent_cycle() {
        let records = vec![
            rec("A", Some("a"), Some("b")),
            rec("B", Some("b"), Some("a")),
        ];
        let maps = BatchMaps::build(&records);
        assert!(topological_mint_order(&records, &maps).is_err());
    }

    #[test]
    fn topological_order_rejects_self_parent() {
        let records = vec![rec("A", Some("a"), Some("a"))];
        let maps = BatchMaps::build(&records);
        assert!(topological_mint_order(&records, &maps).is_err());
    }

    #[test]
    fn storage_parent_keeps_file_order() {
        // A parent ref that matches NO batch record is a pre-existing-storage parent — no edge.
        let records = vec![rec("X", None, Some("ub-pre")), rec("Y", None, None)];
        let maps = BatchMaps::build(&records);
        let order = topological_mint_order(&records, &maps).expect("orderable");
        assert_eq!(order, vec![0, 1]);
    }
}
