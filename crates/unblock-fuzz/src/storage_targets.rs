//! The storage fuzz cores (`run_*_case`) — the query, cycle-detection, and id-allocation surfaces.
//!
//! Each drives the async [`Storage`](unblock_storage::Storage) trait from the synchronous core via
//! [`crate::tokio_block_on`], over a **fresh file-backed** `LibsqlStorage` (real WAL). They exercise
//! the storage seams the `unblock-storage` contract suite pins (it is reused here for the heavy
//! invariants), under fuzzer-chosen shapes. The gated `StorageTestkit` seam (raw-edge insert +
//! child-counter high-water) is reachable because this crate enables `unblock-storage/testkit`.

// The cores assert their invariants (a breach = the bug libFuzzer reports). The `# Panics` section is
// noise here, so the lint is scoped off for this module.
#![allow(clippy::missing_panics_doc)]

use std::collections::HashSet;

use unblock_model::{Dependency, DependencyType, Issue, ListFilters, Priority, Status};
use unblock_storage::{LibsqlStorage, Storage, StorageError, StorageTestkit};

use crate::FuzzError;
use crate::cursor::{ByteCursor, CursorExt};
use crate::tokio_block_on;
use crate::workspace::FuzzWorkspace;

/// **`query_filters`** — `list/ready/blocked/search/count/stale` never panic under a fuzzed
/// `ListFilters`; every result is a subset of the seeded ids; `ready` and `blocked` are disjoint and
/// exclude closed/deferred; the grouped count buckets stay sum-consistent.
///
/// # Errors
///
/// Returns [`FuzzError`] if the temp store cannot be opened/migrated (an environment problem, not an
/// input bug).
pub fn run_query_filters_case(data: &[u8]) -> Result<(), FuzzError> {
    let ws = FuzzWorkspace::new()?;
    tokio_block_on(async move {
        let storage = ws.open_local_storage().await?;
        let mut cursor = ByteCursor::new(data);

        // Seed a small, fixed set of issues so results are subset-checkable.
        let seeded = seed_issues(&storage, &mut cursor).await?;

        let filters = arbitrary_filters(&mut cursor);
        let needle = cursor.text(8);

        // None of these may panic; each result is a subset of the seeded ids.
        let list = storage.list_issues(&filters).await?;
        assert_subset(&list, &seeded, "list_issues");
        let ready = storage.ready_issues(&filters).await?;
        assert_subset(&ready, &seeded, "ready_issues");
        let blocked = storage.blocked_issues(&filters).await?;
        assert_subset(&blocked, &seeded, "blocked_issues");
        let searched = storage.search_issues(&needle, &filters).await?;
        assert_subset(&searched, &seeded, "search_issues");
        let stale = storage.stale_issues(chrono::Utc::now(), &filters).await?;
        assert_subset(&stale, &seeded, "stale_issues");

        // ready and blocked are disjoint.
        let ready_ids: HashSet<&str> = ready.iter().map(|i| i.id.as_str()).collect();
        let blocked_ids: HashSet<&str> = blocked.iter().map(|i| i.id.as_str()).collect();
        assert!(
            ready_ids.is_disjoint(&blocked_ids),
            "ready and blocked must be disjoint"
        );

        // count: each scalar grouped sum == the ungrouped total; the Label sum == the (issue, label)
        // pair count (the trait-doc Label exception).
        assert_count_consistency(&storage, &filters).await?;

        Ok::<(), FuzzError>(())
    })
}

/// **`cycle_detect`** — the cycle detector **always terminates** on a fuzzer-built dependency graph
/// (including planted gating cycles via the raw-edge seam), and `add_dependency` never lets a gating
/// cycle through the public path.
///
/// # Errors
///
/// Returns [`FuzzError`] on a store setup failure.
pub fn run_cycle_detect_case(data: &[u8]) -> Result<(), FuzzError> {
    let ws = FuzzWorkspace::new()?;
    tokio_block_on(async move {
        let storage = ws.open_local_storage().await?;
        let mut cursor = ByteCursor::new(data);

        // A handful of nodes.
        let node_count = 2 + cursor.next_usize(6); // 2..=7
        let mut ids = Vec::with_capacity(node_count);
        for n in 0..node_count {
            let id = format!("ub-n{n}");
            create(&storage, issue(&id, "node")).await?;
            ids.push(id);
        }

        // Add fuzzer-chosen edges through the PUBLIC path (gating cycles are rejected, never created).
        let edge_count = cursor.next_usize(12);
        for _ in 0..edge_count {
            let from = ids[cursor.next_usize(ids.len())].clone();
            let to = ids[cursor.next_usize(ids.len())].clone();
            let dep_type = cursor.dep_type();
            let edge = dep(&from, &to, dep_type);
            match storage.add_dependency(&edge, "fuzz").await {
                // Expected outcomes: success, a self/duplicate edge rejection, or a gating-cycle
                // rejection with a path (the public path NEVER creates a cycle).
                Ok(())
                | Err(
                    StorageError::SelfDependency
                    | StorageError::DuplicateDependency
                    | StorageError::CycleDetected { .. },
                ) => {}
                Err(other) => return Err(FuzzError::from(other)),
            }
        }

        // The public path never created a gating cycle.
        assert!(
            storage.detect_cycles().await?.is_empty(),
            "the public add_dependency path must never create a gating cycle"
        );

        // Now PLANT a raw gating cycle via the seam, on TWO DEDICATED fresh nodes the fuzz-edge loop
        // never touched — so the 2-cycle exists regardless of the prior edges. `add(a->b)` is clean
        // (no prior edge on these nodes), then the back-edge is planted raw (bypassing the guard).
        create(&storage, issue("ub-cyc-a", "cyc a")).await?;
        create(&storage, issue("ub-cyc-b", "cyc b")).await?;
        storage
            .add_dependency(&dep("ub-cyc-a", "ub-cyc-b", DependencyType::Blocks), "fuzz")
            .await?;
        storage
            .testkit_insert_raw_edge(&dep("ub-cyc-b", "ub-cyc-a", DependencyType::Blocks))
            .await?;
        let cycles = storage.detect_cycles().await?;
        assert!(
            !cycles.is_empty(),
            "a planted gating cycle must be detected"
        );
        let nodes: HashSet<&str> = cycles
            .iter()
            .flat_map(|path| path.iter().map(String::as_str))
            .collect();
        assert!(
            nodes.contains("ub-cyc-a") && nodes.contains("ub-cyc-b"),
            "the detected cycle names both planted nodes: {cycles:?}"
        );

        Ok::<(), FuzzError>(())
    })
}

/// **`id_alloc`** — creating hierarchical children advances the id child-counter high-water mark
/// monotonically (never regresses), exercised via the `StorageTestkit` seam.
///
/// # Errors
///
/// Returns [`FuzzError`] on a store setup failure.
pub fn run_id_alloc_case(data: &[u8]) -> Result<(), FuzzError> {
    let ws = FuzzWorkspace::new()?;
    tokio_block_on(async move {
        let storage = ws.open_local_storage().await?;
        let mut cursor = ByteCursor::new(data);

        let parent = "ub-root";
        create(&storage, issue(parent, "root")).await?;
        assert_eq!(
            storage.testkit_child_high_water(parent).await?,
            None,
            "no child allocated yet"
        );

        // Create children at fuzzer-chosen segments; the high-water must never regress and must reach
        // the max segment created.
        let mut max_created = 0u32;
        let mut last_hw = 0u32;
        let child_count = 1 + cursor.next_usize(8); // 1..=8
        for _ in 0..child_count {
            // A 1-based child segment in 1..=64 (avoid 0; valid hierarchical ids use positive ints).
            let segment = 1 + (cursor.next_u32() % 64);
            let child_id = format!("{parent}.{segment}");
            // A duplicate id collides — that is fine; we only care about the counter monotonicity.
            // Any create failure (collision / validation) is ignored; only a successful create counts
            // toward the max segment.
            if create(&storage, issue(&child_id, "child")).await.is_ok() {
                max_created = max_created.max(segment);
            }

            if let Some(hw) = storage.testkit_child_high_water(parent).await? {
                assert!(
                    hw >= last_hw,
                    "high-water must not regress: {hw} < {last_hw}"
                );
                last_hw = hw;
            }
        }

        if max_created > 0 {
            let hw = storage
                .testkit_child_high_water(parent)
                .await?
                .expect("a child exists");
            assert!(
                hw >= max_created,
                "high-water ({hw}) must reach the max child segment created ({max_created})"
            );
        }

        Ok::<(), FuzzError>(())
    })
}

// --------------------------------------------------------------------------------------------------
// Helpers
// --------------------------------------------------------------------------------------------------

/// A minimal valid issue at the fixed epoch.
fn issue(id: &str, title: &str) -> Issue {
    Issue {
        id: id.to_string(),
        title: title.to_string(),
        created_at: epoch(),
        updated_at: epoch(),
        ..Issue::default()
    }
}

/// A dependency edge.
fn dep(from: &str, to: &str, dep_type: DependencyType) -> Dependency {
    Dependency {
        issue_id: from.to_string(),
        depends_on_id: to.to_string(),
        dep_type,
        created_at: epoch(),
        created_by: None,
        metadata: None,
        thread_id: None,
    }
}

/// The fixed epoch for deterministic seeds.
fn epoch() -> chrono::DateTime<chrono::Utc> {
    use chrono::TimeZone;
    chrono::Utc
        .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
        .single()
        .unwrap_or_else(chrono::Utc::now)
}

/// Create an issue, mapping its result into [`FuzzError`].
async fn create(storage: &LibsqlStorage, issue: Issue) -> Result<(), FuzzError> {
    storage.create_issue(&issue, "fuzz").await?;
    Ok(())
}

/// Seed a fixed, predictable issue set (open / closed / deferred / blocked) and return their ids.
async fn seed_issues(
    storage: &LibsqlStorage,
    cursor: &mut ByteCursor<'_>,
) -> Result<HashSet<String>, FuzzError> {
    // A couple of plain open issues with fuzzer-chosen priority/assignee/labels. Every seeded value
    // must be VALID (these are setup, not the surface under test), so the assignee is drawn from a
    // small safe pool (not free text) and labels are well-formed `l{k}`.
    const ASSIGNEES: &[&str] = &["alice", "bob", "carol"];

    let mut ids = HashSet::new();
    for n in 0..2 {
        let mut i = issue(&format!("ub-open{n}"), "open");
        i.priority = Priority(i32::from(cursor.next_byte()).clamp(0, 4));
        i.assignee = if cursor.next_bool() {
            Some(ASSIGNEES[cursor.next_usize(ASSIGNEES.len())].to_string())
        } else {
            None
        };
        i.labels = (0..cursor.next_usize(3)).map(|k| format!("l{k}")).collect();
        create(storage, i.clone()).await?;
        ids.insert(i.id);
    }

    // A blocker + a blocked issue (so ready/blocked are exercised).
    create(storage, issue("ub-blocker", "blocker")).await?;
    ids.insert("ub-blocker".to_string());
    create(storage, issue("ub-blocked", "blocked")).await?;
    ids.insert("ub-blocked".to_string());
    storage
        .add_dependency(
            &dep("ub-blocked", "ub-blocker", DependencyType::Blocks),
            "fuzz",
        )
        .await?;

    // A deferred issue.
    create(storage, issue("ub-deferred", "deferred")).await?;
    ids.insert("ub-deferred".to_string());
    storage
        .defer_issue("ub-deferred", future_ts(), "fuzz")
        .await?;

    // A closed issue.
    create(storage, issue("ub-closed", "closed")).await?;
    ids.insert("ub-closed".to_string());
    storage
        .update_issue(
            "ub-closed",
            &unblock_storage::IssuePatch {
                status: Some(Status::Closed),
                ..Default::default()
            },
            "fuzz",
        )
        .await?;

    Ok(ids)
}

/// A far-future timestamp for deferral.
fn future_ts() -> chrono::DateTime<chrono::Utc> {
    use chrono::TimeZone;
    chrono::Utc
        .with_ymd_and_hms(2099, 1, 1, 0, 0, 0)
        .single()
        .unwrap_or_else(chrono::Utc::now)
}

/// Build a fuzzer-chosen `ListFilters`.
fn arbitrary_filters(cursor: &mut ByteCursor) -> ListFilters {
    let mut filters = ListFilters::default();
    // status OR facet.
    let status_count = cursor.next_usize(3);
    for _ in 0..status_count {
        filters.status.push(cursor.status());
    }
    let type_count = cursor.next_usize(3);
    for _ in 0..type_count {
        filters.issue_type.push(cursor.issue_type());
    }
    if cursor.next_bool() {
        filters.assignee = cursor.optional_text(8);
    }
    let all_count = cursor.next_usize(3);
    for k in 0..all_count {
        filters.labels_all.push(format!("l{k}"));
    }
    let any_count = cursor.next_usize(3);
    for k in 0..any_count {
        filters.labels_any.push(format!("l{k}"));
    }
    if cursor.next_bool() {
        filters.priority_min = Some(Priority(i32::from(cursor.next_byte()).clamp(0, 4)));
    }
    if cursor.next_bool() {
        filters.priority_max = Some(Priority(i32::from(cursor.next_byte()).clamp(0, 4)));
    }
    if cursor.next_bool() {
        filters.text_contains = Some(cursor.text(8));
    }
    filters.include_deferred = cursor.next_bool();
    filters.include_closed = cursor.next_bool();
    if cursor.next_bool() {
        filters.limit = Some(cursor.next_usize(10));
    }
    if cursor.next_bool() {
        filters.offset = Some(cursor.next_usize(10));
    }
    filters
}

/// Assert every returned issue id is in the seeded set.
fn assert_subset(result: &[Issue], seeded: &HashSet<String>, label: &str) {
    for issue in result {
        assert!(
            seeded.contains(&issue.id),
            "{label} returned an unseeded id: {}",
            issue.id
        );
    }
}

/// Assert count-bucket sum consistency: the ungrouped total equals the Status/Type/Assignee/Priority
/// grouped sums; the Label grouped sum equals the total number of (issue, label) pairs among the
/// matching issues (a multi-label issue counts once per label, a label-less issue counts zero — so
/// it is **not** simply `>= total`, which only holds when every issue has a label).
async fn assert_count_consistency(
    storage: &LibsqlStorage,
    filters: &ListFilters,
) -> Result<(), FuzzError> {
    use unblock_model::CountGroupBy;

    // `count_issues` ignores limit/offset (it counts the whole matching set), so compare against an
    // unlimited filter for the independent label-pair count.
    let unlimited = ListFilters {
        limit: None,
        offset: None,
        ..filters.clone()
    };

    let total: usize = storage
        .count_issues(filters, None)
        .await?
        .into_iter()
        .map(|b| b.count)
        .sum();

    for group in [
        CountGroupBy::Status,
        CountGroupBy::Type,
        CountGroupBy::Assignee,
        CountGroupBy::Priority,
    ] {
        let sum: usize = storage
            .count_issues(filters, Some(group))
            .await?
            .into_iter()
            .map(|b| b.count)
            .sum();
        assert_eq!(
            sum, total,
            "{group:?} buckets must sum to the ungrouped total"
        );
    }

    // The Label grouped sum = the count of (issue, label) pairs over the matching issue set. Derive
    // that independently from the hydrated label lists of `list_issues` over the same (unlimited)
    // filter.
    let label_sum: usize = storage
        .count_issues(&unlimited, Some(CountGroupBy::Label))
        .await?
        .into_iter()
        .map(|b| b.count)
        .sum();
    let independent_pairs: usize = storage
        .list_issues(&unlimited)
        .await?
        .into_iter()
        .map(|i| i.labels.len())
        .sum();
    assert_eq!(
        label_sum, independent_pairs,
        "Label group sum ({label_sum}) must equal the (issue, label) pair count ({independent_pairs})"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{run_cycle_detect_case, run_id_alloc_case, run_query_filters_case};

    #[test]
    fn storage_cores_never_panic_on_empty_and_garbage() {
        let inputs: &[&[u8]] = &[
            b"",
            b"\0",
            &[0x01u8, 0x02, 0x03],
            &[0xffu8; 64],
            &[0u8, 13, 1],
        ];
        for input in inputs {
            run_query_filters_case(input).expect("query_filters core ok");
            run_cycle_detect_case(input).expect("cycle_detect core ok");
            run_id_alloc_case(input).expect("id_alloc core ok");
        }
    }
}
