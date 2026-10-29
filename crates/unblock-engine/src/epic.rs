//! The ONE epic eligibility predicate and the ONE cascade over it (PRD §4 D56 clause (2) and (4),
//! spine §4.1 organization block).
//!
//! Three callers share these definitions and none carries a second copy: `diagnostics::stats`
//! (`stats.epics_eligible`, D26 — single-pass), `Session::epic_statuses`
//! (`EpicStatus.eligible_for_close`, `query kind: epics` — single-pass) and
//! `Session::close_eligible` (through [`cascade_close_set`] — the cascade). No `unblock-policy`
//! dependency is involved.
//!
//! Two consequences are disclosed rather than hidden (D56 clause (2)): a tombstoned child counts as
//! closed, so an epic whose children were all deleted is eligible (bd-faithful); and eligibility
//! ignores the epic's OWN blockers, so a `blocks` edge on the epic itself can keep it blocked while
//! it is eligible (bd-faithful).

use std::collections::HashMap;

use unblock_model::{DependencyType, EpicStatus, Issue, IssueType};

/// Whether `epic` is ELIGIBLE FOR CLOSE (D56 clause (2)) given its child rollup.
///
/// All five must hold: its `issue_type` is `epic`; it is not a template; its status is neither
/// `closed` nor `tombstone`; it has at least one child; and every child is `closed` or
/// `tombstone`. `total_children`/`closed_children` are the D26 `Storage::epic_child_rollup` counts
/// (a child holds a `parent-child` edge to the epic, has a row and is not a template).
pub(crate) fn is_eligible_for_close(
    epic: &Issue,
    total_children: usize,
    closed_children: usize,
) -> bool {
    epic.issue_type == IssueType::Epic
        && !epic.is_template
        && !epic.status.is_terminal()
        && total_children > 0
        && closed_children == total_children
}

/// The CASCADE CLOSE SET over `candidates` (D56 clause (4)): [`is_eligible_for_close`] applied to a
/// FIXPOINT.
///
/// Round 1 is every eligible candidate. An epic placed in the set counts as one more closed child
/// of every candidate it holds a `parent-child` edge to (read from its hydrated
/// `Issue.dependencies`), so a parent whose last open child was an epic placed in round k becomes
/// eligible in round k+1. The walk stops at the first round that adds nothing; it terminates because
/// each round only moves candidates out of a finite set. The ORDER is fixed: by round, then by epic
/// id ascending within a round, so every child epic precedes its parent.
pub(crate) fn cascade_close_set(candidates: &[EpicStatus]) -> Vec<String> {
    let index: HashMap<&str, usize> = candidates
        .iter()
        .enumerate()
        .map(|(i, status)| (status.epic.id.as_str(), i))
        .collect();
    let mut closed: Vec<usize> = candidates.iter().map(|s| s.closed_children).collect();
    let mut placed = vec![false; candidates.len()];
    let mut order = Vec::new();

    loop {
        // Every round reads the counts as they stood at the END of the previous round.
        let mut round: Vec<usize> = (0..candidates.len())
            .filter(|&i| {
                !placed[i]
                    && is_eligible_for_close(
                        &candidates[i].epic,
                        candidates[i].total_children,
                        closed[i],
                    )
            })
            .collect();
        if round.is_empty() {
            return order;
        }
        round.sort_unstable_by(|&a, &b| candidates[a].epic.id.cmp(&candidates[b].epic.id));
        for &i in &round {
            placed[i] = true;
            for dep in &candidates[i].epic.dependencies {
                if dep.dep_type == DependencyType::ParentChild
                    && let Some(&parent) = index.get(dep.depends_on_id.as_str())
                {
                    closed[parent] += 1;
                }
            }
            order.push(candidates[i].epic.id.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{cascade_close_set, is_eligible_for_close};
    use chrono::Utc;
    use unblock_model::{Dependency, DependencyType, EpicStatus, Issue, IssueType, Status};

    fn epic(id: &str) -> Issue {
        Issue {
            id: id.to_string(),
            title: id.to_string(),
            issue_type: IssueType::Epic,
            ..Issue::default()
        }
    }

    fn child_of(mut issue: Issue, parent: &str) -> Issue {
        issue.dependencies.push(Dependency {
            issue_id: issue.id.clone(),
            depends_on_id: parent.to_string(),
            dep_type: DependencyType::ParentChild,
            created_at: Utc::now(),
            created_by: None,
            metadata: None,
            thread_id: None,
        });
        issue
    }

    fn status(epic: Issue, total: usize, closed: usize) -> EpicStatus {
        let eligible_for_close = is_eligible_for_close(&epic, total, closed);
        EpicStatus {
            epic,
            total_children: total,
            closed_children: closed,
            eligible_for_close,
        }
    }

    #[test]
    fn predicate_requires_all_five_conditions() {
        assert!(is_eligible_for_close(&epic("ub-e"), 2, 2));
        // At least one child.
        assert!(!is_eligible_for_close(&epic("ub-e"), 0, 0));
        // Every child closed.
        assert!(!is_eligible_for_close(&epic("ub-e"), 2, 1));
        // Not a template.
        let mut template = epic("ub-e");
        template.is_template = true;
        assert!(!is_eligible_for_close(&template, 1, 1));
        // Not closed / tombstoned itself.
        for terminal in [Status::Closed, Status::Tombstone] {
            let mut done = epic("ub-e");
            done.status = terminal;
            assert!(!is_eligible_for_close(&done, 1, 1));
        }
        // An epic: a task with every child closed is not eligible.
        let mut task = epic("ub-e");
        task.issue_type = IssueType::Task;
        assert!(!is_eligible_for_close(&task, 1, 1));
    }

    #[test]
    fn cascade_closes_child_epic_before_parent() {
        // A's only child is epic B; B's only child is a closed leaf.
        let a = status(epic("ub-a"), 1, 0);
        let b = status(child_of(epic("ub-b"), "ub-a"), 1, 1);
        assert_eq!(cascade_close_set(&[a, b]), vec!["ub-b", "ub-a"]);
    }

    #[test]
    fn cascade_orders_one_round_by_id_and_stops_at_a_still_open_parent() {
        // Two independent eligible epics come out in id order; a parent with a second, still-open
        // non-epic child never enters the set.
        let z = status(child_of(epic("ub-z"), "ub-p"), 1, 1);
        let m = status(epic("ub-m"), 3, 3);
        let p = status(epic("ub-p"), 2, 0);
        assert_eq!(cascade_close_set(&[m, p, z]), vec!["ub-m", "ub-z"]);
    }

    #[test]
    fn cascade_of_nothing_eligible_is_empty() {
        assert!(cascade_close_set(&[status(epic("ub-a"), 0, 0)]).is_empty());
        assert!(cascade_close_set(&[]).is_empty());
    }
}
