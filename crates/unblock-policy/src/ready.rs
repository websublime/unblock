//! The v1 core: the pure **hybrid ready ranking** comparator + the ready/blocked predicate +
//! the gating-edge rule (plan §2 `ready.rs`; spine §3.2.1 / §4.1).
//!
//! # The hybrid ready comparator (PINNED)
//!
//! Byte-faithful to the original `sort_ready_hybrid` (`sqlite.rs:10444`) + `ready_hybrid_bucket`
//! (`sqlite.rs:10515`), the DEFAULT `ReadySortPolicy::Hybrid` every ready entry point uses:
//!
//! ```text
//! ready_hybrid_bucket(priority) = if priority.0 <= 1 { 0 } else { 1 }   // P0/P1 -> 0, P2/P3/P4 -> 1
//! order = bucket ASC, then created_at ASC, then id ASC
//! ```
//!
//! So `P0` and `P1` share the high bucket (`0`); the bucket boundary is between `P1` and `P2`;
//! within a bucket issues are oldest-first (`created_at` ASC) then tie-broken by `id` ASC. This is
//! **distinct** from the `list`-default order (`priority ASC, created_at DESC, id ASC` of
//! `sort_list_default`, `sqlite.rs:3464`): ready is `created_at` **ASC**, NOT `DESC`.
//!
//! The §3.2.1 storage `ready_issues` SQL pre-sorts candidates by `priority ASC, created_at ASC,
//! id ASC`; the **engine** then re-ranks that candidate set via [`cmp_ready`] (which buckets P0/P1
//! together, so the final hybrid order differs from the SQL pre-sort) per CF-11 / spine §4.1.
//!
//! # The ready/blocked predicate
//!
//! An incoming dependency edge gates ready-work iff its type
//! [`DependencyType::affects_ready_work`] (the four `Blocks` / `ParentChild` / `ConditionalBlocks`
//! / `WaitsFor` types) **and** its source is not terminal ([`Status::is_terminal`] — a
//! closed/tombstone blocker is resolved). `Related` and the other non-gating types never block.

use std::cmp::Ordering;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use unblock_model::{DependencyType, Issue, Priority, Status};

/// The four dependency types that gate ready-work (spine §1.4) — the exact set for which
/// [`DependencyType::affects_ready_work`] is `true`.
///
/// Re-exported so the engine can build its candidate-exclusion query over the same set the
/// predicate uses, keeping one definition of "what blocks ready work".
///
/// # Examples
///
/// ```
/// use unblock_policy::READY_GATING_TYPES;
/// use unblock_model::DependencyType;
///
/// assert_eq!(READY_GATING_TYPES.len(), 4);
/// assert!(READY_GATING_TYPES.contains(&DependencyType::Blocks));
/// assert!(!READY_GATING_TYPES.contains(&DependencyType::Related));
/// ```
pub const READY_GATING_TYPES: &[DependencyType] = &[
    DependencyType::Blocks,
    DependencyType::ParentChild,
    DependencyType::ConditionalBlocks,
    DependencyType::WaitsFor,
];

/// Compute the hybrid ready-sort bucket for a priority (`sqlite.rs:10515`).
///
/// `P0` and `P1` (`priority.0 <= 1`) share the high bucket `0`; `P2`/`P3`/`P4` fall into bucket
/// `1`. Lower bucket sorts first (bucket ASC).
///
/// # Examples
///
/// ```
/// use unblock_policy::ready_hybrid_bucket;
/// use unblock_model::Priority;
///
/// assert_eq!(ready_hybrid_bucket(Priority::CRITICAL), 0); // P0
/// assert_eq!(ready_hybrid_bucket(Priority::HIGH), 0);     // P1
/// assert_eq!(ready_hybrid_bucket(Priority::MEDIUM), 1);   // P2
/// assert_eq!(ready_hybrid_bucket(Priority::BACKLOG), 1);  // P4
/// ```
#[must_use]
pub const fn ready_hybrid_bucket(priority: Priority) -> i32 {
    if priority.0 <= 1 { 0 } else { 1 }
}

/// The ready-sort bucket of an issue — a transparent, totally-ordered newtype over the bucket id
/// produced by [`ready_hybrid_bucket`].
///
/// Lower buckets sort first; this is the most-significant key of [`ReadySortKey`].
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
pub struct ReadyBucket(pub i32);

/// The total-order sort key for the hybrid ready ranking.
///
/// The tuple `(bucket, created_at, id)` is ordered field-by-field, all ASC:
///
/// 1. [`ReadyBucket`] ASC (P0/P1 before P2/P3/P4),
/// 2. `created_at` ASC (oldest-first — **no** [`std::cmp::Reverse`]; this is NOT the `list`
///    `created_at DESC` order),
/// 3. `id` ASC (stable, unique tie-break).
///
/// Because all three fields are themselves totally ordered, the derived tuple `Ord` is a total
/// order without any `Reverse` wrapper. [`ReadySortKey::cmp`] is consistent with [`cmp_ready`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReadySortKey(pub ReadyBucket, pub DateTime<Utc>, pub String);

/// Build the [`ReadySortKey`] for an issue (its bucket, `created_at`, and `id`).
///
/// # Examples
///
/// ```
/// use unblock_policy::{ready_sort_key, ReadyBucket};
/// use unblock_model::{Issue, Priority};
/// use chrono::{TimeZone, Utc};
///
/// let issue = Issue {
///     id: "ub-1".to_string(),
///     priority: Priority::HIGH, // P1 -> bucket 0
///     created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
///     ..Issue::default()
/// };
/// let key = ready_sort_key(&issue);
/// assert_eq!(key.0, ReadyBucket(0));
/// assert_eq!(key.2, "ub-1");
/// ```
#[must_use]
pub fn ready_sort_key(issue: &Issue) -> ReadySortKey {
    ReadySortKey(
        ReadyBucket(ready_hybrid_bucket(issue.priority)),
        issue.created_at,
        issue.id.clone(),
    )
}

/// The canonical hybrid ready comparator (`sort_ready_hybrid`, `sqlite.rs:10444`).
///
/// Orders by **bucket ASC, then `created_at` ASC, then `id` ASC** — equivalent to comparing the
/// two issues' [`ReadySortKey`]s (see the consistency property test). This is the comparator the
/// engine `ready()` re-rank uses (spine §4.1, NORMATIVE).
///
/// # Examples
///
/// ```
/// use unblock_policy::cmp_ready;
/// use unblock_model::{Issue, Priority};
/// use chrono::{TimeZone, Utc};
/// use std::cmp::Ordering;
///
/// let p0 = Issue { id: "ub-a".into(), priority: Priority::CRITICAL,
///     created_at: Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap(), ..Issue::default() };
/// let p2 = Issue { id: "ub-b".into(), priority: Priority::MEDIUM,
///     created_at: Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(), ..Issue::default() };
/// // Bucket wins over age: P0 (bucket 0) sorts before an older P2 (bucket 1).
/// assert_eq!(cmp_ready(&p0, &p2), Ordering::Less);
/// ```
#[must_use]
pub fn cmp_ready(a: &Issue, b: &Issue) -> Ordering {
    ready_hybrid_bucket(a.priority)
        .cmp(&ready_hybrid_bucket(b.priority))
        .then_with(|| a.created_at.cmp(&b.created_at))
        .then_with(|| a.id.cmp(&b.id))
}

/// A single incoming dependency edge pointing **at** the issue under evaluation (the blocker is the
/// edge's `from`/source).
///
/// Caller-supplied DB-derived data: the engine walks storage and hands policy each incoming edge's
/// type and the live status of the source issue. Policy decides whether it gates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BlockingEdge {
    /// The id of the source/blocker issue (the edge's `from`).
    pub from_id: String,
    /// The relationship type of the edge.
    pub dep_type: DependencyType,
    /// The live status of the source/blocker issue.
    pub source_status: Status,
}

impl BlockingEdge {
    /// Whether this edge actively gates ready-work: a gating type
    /// ([`DependencyType::affects_ready_work`]) whose source is **not** terminal
    /// ([`Status::is_terminal`]). A `Related`/non-gating edge, or a closed/tombstone source, is
    /// resolved and never blocks.
    #[must_use]
    pub fn is_active_blocker(&self) -> bool {
        self.dep_type.affects_ready_work() && !self.source_status.is_terminal()
    }
}

/// All the inputs the ready/blocked predicate needs for one issue — caller-supplied plain data
/// (no I/O, no clock: `now` is a parameter for purity/testability).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadyContext {
    /// The issue's current status.
    pub status: Status,
    /// The defer-until timestamp, if any.
    pub defer_until: Option<DateTime<Utc>>,
    /// The issue's incoming dependency edges (its blockers).
    pub incoming_blocking: Vec<BlockingEdge>,
    /// The reference "now" the deferral is evaluated against.
    pub now: DateTime<Utc>,
}

/// The outcome of evaluating an issue's readiness ([`is_ready`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "verdict")]
pub enum ReadyVerdict {
    /// The issue is ready to be worked.
    Ready,
    /// The issue is blocked by unresolved incoming gating edges.
    Blocked {
        /// The ids of the active (unresolved) blockers, in input order.
        by: Vec<String>,
    },
    /// The issue is deferred until a future timestamp.
    Deferred {
        /// The defer-until timestamp (`> now`).
        until: DateTime<Utc>,
    },
    /// The issue is not actionable for a status reason (terminal, or not `Open`).
    NotActionable {
        /// A short, machine-friendly reason.
        reason: String,
    },
}

/// Evaluate whether an issue is ready, returning the precise [`ReadyVerdict`].
///
/// # Verdict precedence (faithful to storage `ready_issues`, spine §3.2.1)
///
/// The checks are applied in this fixed order (the first match wins):
///
/// 1. **Terminal status** (`Closed`/`Tombstone`) → [`ReadyVerdict::NotActionable`] (a terminal
///    issue is never ready, regardless of deferral or blockers).
/// 2. **Deferred to the future** (`defer_until > now`) → [`ReadyVerdict::Deferred`] (a past/`==
///    now` deferral does not defer; spine §3.2.1 `defer_until <= now`).
/// 3. **Active blocking edges** (any [`BlockingEdge::is_active_blocker`]) →
///    [`ReadyVerdict::Blocked`] carrying the blocker ids.
/// 4. **`Open`** with none of the above → [`ReadyVerdict::Ready`] (the storage filter is
///    `status = 'open'`).
/// 5. **Otherwise** (a non-terminal, non-`Open` status with no defer/blockers — e.g. `Blocked`,
///    `InProgress`, `Draft`, `Pinned`, `Custom`) → [`ReadyVerdict::NotActionable`].
///
/// `now` is a parameter (no clock dependency).
///
/// # Examples
///
/// ```
/// use unblock_policy::{is_ready, ReadyContext, ReadyVerdict};
/// use unblock_model::Status;
/// use chrono::{TimeZone, Utc};
///
/// let now = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
/// let ctx = ReadyContext { status: Status::Open, defer_until: None,
///     incoming_blocking: vec![], now };
/// assert_eq!(is_ready(&ctx), ReadyVerdict::Ready);
/// ```
#[must_use]
pub fn is_ready(ctx: &ReadyContext) -> ReadyVerdict {
    // 1. Terminal status is never ready.
    if ctx.status.is_terminal() {
        return ReadyVerdict::NotActionable {
            reason: format!("status `{}` is terminal", ctx.status.as_str()),
        };
    }

    // 2. A future deferral defers (a deferral `<= now` does not — spine §3.2.1).
    if let Some(until) = ctx.defer_until
        && until > ctx.now
    {
        return ReadyVerdict::Deferred { until };
    }

    // 3. Active (unresolved) gating edges block.
    let by: Vec<String> = ctx
        .incoming_blocking
        .iter()
        .filter(|edge| edge.is_active_blocker())
        .map(|edge| edge.from_id.clone())
        .collect();
    if !by.is_empty() {
        return ReadyVerdict::Blocked { by };
    }

    // 4. The storage ready filter is `status = 'open'`.
    if ctx.status == Status::Open {
        return ReadyVerdict::Ready;
    }

    // 5. A non-terminal, non-open status with no defer/blockers is not actionable as "ready".
    ReadyVerdict::NotActionable {
        reason: format!("status `{}` is not open", ctx.status.as_str()),
    }
}

/// Whether an issue is blocked — the [`is_ready`] complement used by `blocked()`.
///
/// Returns `true` iff [`is_ready`] yields [`ReadyVerdict::Blocked`]. A deferred, terminal, or
/// otherwise-not-actionable issue is **not** "blocked" in this sense (it is excluded for a
/// different reason); only an unresolved active gating edge makes an issue blocked.
///
/// # Examples
///
/// ```
/// use unblock_policy::{is_blocked, BlockingEdge, ReadyContext};
/// use unblock_model::{DependencyType, Status};
/// use chrono::{TimeZone, Utc};
///
/// let now = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
/// let edge = BlockingEdge { from_id: "ub-dep".into(), dep_type: DependencyType::Blocks,
///     source_status: Status::Open };
/// let ctx = ReadyContext { status: Status::Open, defer_until: None,
///     incoming_blocking: vec![edge], now };
/// assert!(is_blocked(&ctx));
/// ```
#[must_use]
pub fn is_blocked(ctx: &ReadyContext) -> bool {
    matches!(is_ready(ctx), ReadyVerdict::Blocked { .. })
}

#[cfg(test)]
mod tests {
    use super::{
        BlockingEdge, READY_GATING_TYPES, ReadyBucket, ReadyContext, ReadyVerdict, cmp_ready,
        is_blocked, is_ready, ready_hybrid_bucket, ready_sort_key,
    };
    use chrono::{DateTime, TimeZone, Utc};
    use std::cmp::Ordering;
    use unblock_model::{DependencyType, Issue, Priority, Status};

    fn at(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).single().expect("valid ts")
    }

    fn issue(id: &str, priority: Priority, created_secs: i64) -> Issue {
        Issue {
            id: id.to_string(),
            priority,
            created_at: at(created_secs),
            ..Issue::default()
        }
    }

    // --- bucket / comparator ---

    #[test]
    fn bucket_boundary_is_between_p1_and_p2() {
        assert_eq!(ready_hybrid_bucket(Priority::CRITICAL), 0); // P0
        assert_eq!(ready_hybrid_bucket(Priority::HIGH), 0); // P1
        assert_eq!(ready_hybrid_bucket(Priority::MEDIUM), 1); // P2
        assert_eq!(ready_hybrid_bucket(Priority::LOW), 1); // P3
        assert_eq!(ready_hybrid_bucket(Priority::BACKLOG), 1); // P4
    }

    #[test]
    fn p0_and_p1_share_bucket_and_tie_by_oldest_then_id() {
        // Same bucket (0): older created_at sorts first.
        let p0_new = issue("ub-z", Priority::CRITICAL, 200);
        let p1_old = issue("ub-a", Priority::HIGH, 100);
        assert_eq!(cmp_ready(&p1_old, &p0_new), Ordering::Less);

        // Same bucket + same created_at: id ASC breaks the tie.
        let a = issue("ub-a", Priority::CRITICAL, 100);
        let b = issue("ub-b", Priority::HIGH, 100);
        assert_eq!(cmp_ready(&a, &b), Ordering::Less);
        assert_eq!(cmp_ready(&b, &a), Ordering::Greater);
    }

    #[test]
    fn lower_bucket_sorts_before_higher_regardless_of_age() {
        // P2 (bucket 1) is older but the P1 (bucket 0) still wins on bucket.
        let p1_new = issue("ub-1", Priority::HIGH, 999);
        let p2_old = issue("ub-2", Priority::MEDIUM, 1);
        assert_eq!(cmp_ready(&p1_new, &p2_old), Ordering::Less);
    }

    #[test]
    fn within_bucket_is_oldest_first_not_newest() {
        // Guard against accidentally using the `list` DESC order.
        let old = issue("ub-a", Priority::MEDIUM, 100);
        let new = issue("ub-b", Priority::MEDIUM, 200);
        assert_eq!(cmp_ready(&old, &new), Ordering::Less);
    }

    #[test]
    fn cmp_ready_matches_ready_sort_key() {
        let a = issue("ub-a", Priority::CRITICAL, 100);
        let b = issue("ub-b", Priority::MEDIUM, 50);
        assert_eq!(
            cmp_ready(&a, &b),
            ready_sort_key(&a).cmp(&ready_sort_key(&b))
        );
    }

    #[test]
    fn ready_bucket_orders_numerically() {
        assert!(ReadyBucket(0) < ReadyBucket(1));
    }

    // --- gating set ---

    #[test]
    fn ready_gating_types_equals_affects_ready_work_set() {
        let all = [
            DependencyType::Blocks,
            DependencyType::ParentChild,
            DependencyType::ConditionalBlocks,
            DependencyType::WaitsFor,
            DependencyType::Related,
            DependencyType::DiscoveredFrom,
            DependencyType::RepliesTo,
            DependencyType::RelatesTo,
            DependencyType::Duplicates,
            DependencyType::Supersedes,
            DependencyType::CausedBy,
        ];
        for dep in all {
            assert_eq!(
                dep.affects_ready_work(),
                READY_GATING_TYPES.contains(&dep),
                "gating-set mismatch for {dep:?}"
            );
        }
        assert_eq!(READY_GATING_TYPES.len(), 4);
    }

    // --- predicate precedence ---

    #[test]
    fn open_with_no_edges_is_ready() {
        let ctx = ReadyContext {
            status: Status::Open,
            defer_until: None,
            incoming_blocking: vec![],
            now: at(1_000),
        };
        assert_eq!(is_ready(&ctx), ReadyVerdict::Ready);
        assert!(!is_blocked(&ctx));
    }

    #[test]
    fn deferred_future_defers_past_does_not() {
        let future = ReadyContext {
            status: Status::Open,
            defer_until: Some(at(2_000)),
            incoming_blocking: vec![],
            now: at(1_000),
        };
        assert_eq!(
            is_ready(&future),
            ReadyVerdict::Deferred { until: at(2_000) }
        );

        // Past deferral (< now) does NOT defer -> Ready.
        let past = ReadyContext {
            defer_until: Some(at(500)),
            ..future.clone()
        };
        assert_eq!(is_ready(&past), ReadyVerdict::Ready);

        // Exactly `== now` does NOT defer (spine: `defer_until <= now`).
        let eq = ReadyContext {
            defer_until: Some(at(1_000)),
            ..future
        };
        assert_eq!(is_ready(&eq), ReadyVerdict::Ready);
    }

    #[test]
    fn active_blocks_edge_blocks_related_does_not() {
        let blocks = BlockingEdge {
            from_id: "ub-dep".into(),
            dep_type: DependencyType::Blocks,
            source_status: Status::Open,
        };
        let ctx = ReadyContext {
            status: Status::Open,
            defer_until: None,
            incoming_blocking: vec![blocks],
            now: at(1_000),
        };
        assert_eq!(
            is_ready(&ctx),
            ReadyVerdict::Blocked {
                by: vec!["ub-dep".to_string()]
            }
        );
        assert!(is_blocked(&ctx));

        // A `Related` edge never gates.
        let related = BlockingEdge {
            from_id: "ub-rel".into(),
            dep_type: DependencyType::Related,
            source_status: Status::Open,
        };
        let ctx_rel = ReadyContext {
            incoming_blocking: vec![related],
            ..ctx
        };
        assert_eq!(is_ready(&ctx_rel), ReadyVerdict::Ready);
    }

    #[test]
    fn closed_or_tombstone_source_edge_is_resolved() {
        for terminal in [Status::Closed, Status::Tombstone] {
            let edge = BlockingEdge {
                from_id: "ub-dep".into(),
                dep_type: DependencyType::Blocks,
                source_status: terminal,
            };
            let ctx = ReadyContext {
                status: Status::Open,
                defer_until: None,
                incoming_blocking: vec![edge],
                now: at(1_000),
            };
            assert_eq!(is_ready(&ctx), ReadyVerdict::Ready);
            assert!(!is_blocked(&ctx));
        }
    }

    #[test]
    fn all_four_gating_types_gate_others_do_not() {
        let now = at(1_000);
        for dep in [
            DependencyType::Blocks,
            DependencyType::ParentChild,
            DependencyType::ConditionalBlocks,
            DependencyType::WaitsFor,
        ] {
            let edge = BlockingEdge {
                from_id: "ub-dep".into(),
                dep_type: dep.clone(),
                source_status: Status::Open,
            };
            let ctx = ReadyContext {
                status: Status::Open,
                defer_until: None,
                incoming_blocking: vec![edge],
                now,
            };
            assert!(is_blocked(&ctx), "{dep:?} should gate");
        }
        for dep in [
            DependencyType::Related,
            DependencyType::DiscoveredFrom,
            DependencyType::Duplicates,
        ] {
            let edge = BlockingEdge {
                from_id: "ub-dep".into(),
                dep_type: dep.clone(),
                source_status: Status::Open,
            };
            let ctx = ReadyContext {
                status: Status::Open,
                defer_until: None,
                incoming_blocking: vec![edge],
                now,
            };
            assert!(!is_blocked(&ctx), "{dep:?} should not gate");
        }
    }

    #[test]
    fn terminal_status_precedes_defer_and_block() {
        // A terminal status wins over a future deferral AND an active blocker.
        let edge = BlockingEdge {
            from_id: "ub-dep".into(),
            dep_type: DependencyType::Blocks,
            source_status: Status::Open,
        };
        let ctx = ReadyContext {
            status: Status::Closed,
            defer_until: Some(at(9_999)),
            incoming_blocking: vec![edge],
            now: at(1_000),
        };
        assert!(matches!(is_ready(&ctx), ReadyVerdict::NotActionable { .. }));
        assert!(!is_blocked(&ctx));
    }

    #[test]
    fn deferred_precedes_block() {
        // A future deferral wins over an active blocker.
        let edge = BlockingEdge {
            from_id: "ub-dep".into(),
            dep_type: DependencyType::Blocks,
            source_status: Status::Open,
        };
        let ctx = ReadyContext {
            status: Status::Open,
            defer_until: Some(at(2_000)),
            incoming_blocking: vec![edge],
            now: at(1_000),
        };
        assert_eq!(is_ready(&ctx), ReadyVerdict::Deferred { until: at(2_000) });
    }

    #[test]
    fn non_open_non_terminal_is_not_actionable() {
        for status in [
            Status::Blocked,
            Status::InProgress,
            Status::Draft,
            Status::Pinned,
            Status::Custom("qa".into()),
        ] {
            let ctx = ReadyContext {
                status,
                defer_until: None,
                incoming_blocking: vec![],
                now: at(1_000),
            };
            assert!(matches!(is_ready(&ctx), ReadyVerdict::NotActionable { .. }));
            assert!(!is_blocked(&ctx));
        }
    }

    #[test]
    fn blocked_by_preserves_input_order_and_only_active() {
        let edges = vec![
            BlockingEdge {
                from_id: "ub-1".into(),
                dep_type: DependencyType::Blocks,
                source_status: Status::Open,
            },
            BlockingEdge {
                from_id: "ub-resolved".into(),
                dep_type: DependencyType::Blocks,
                source_status: Status::Closed, // resolved -> skipped
            },
            BlockingEdge {
                from_id: "ub-2".into(),
                dep_type: DependencyType::WaitsFor,
                source_status: Status::InProgress,
            },
        ];
        let ctx = ReadyContext {
            status: Status::Open,
            defer_until: None,
            incoming_blocking: edges,
            now: at(1_000),
        };
        assert_eq!(
            is_ready(&ctx),
            ReadyVerdict::Blocked {
                by: vec!["ub-1".to_string(), "ub-2".to_string()]
            }
        );
    }
}
