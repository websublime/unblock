//! Property tests for the ready/blocked predicate (plan §3; NFR-16).
//!
//! - [`is_blocked`] is exactly the [`ReadyVerdict::Blocked`] arm of [`is_ready`];
//! - the verdict is deterministic for a fixed context;
//! - a terminal status is never `Ready`/`Blocked`/`Deferred`;
//! - any `by` blocker id reported corresponds to an actually-active incoming edge.

use proptest::prelude::*;

use unblock_model::Status;
use unblock_policy::proptest_support::arb_ready_context;
use unblock_policy::{ReadyVerdict, is_blocked, is_ready};

proptest! {
    /// `is_blocked` agrees with the `Blocked` verdict arm.
    #[test]
    fn is_blocked_matches_verdict(ctx in arb_ready_context()) {
        let blocked = matches!(is_ready(&ctx), ReadyVerdict::Blocked { .. });
        prop_assert_eq!(is_blocked(&ctx), blocked);
    }

    /// The predicate is deterministic for a fixed context.
    #[test]
    fn verdict_is_deterministic(ctx in arb_ready_context()) {
        prop_assert_eq!(is_ready(&ctx), is_ready(&ctx));
    }

    /// A terminal status is always `NotActionable` (never ready/blocked/deferred).
    #[test]
    fn terminal_status_is_not_actionable(ctx in arb_ready_context()) {
        if ctx.status.is_terminal() {
            prop_assert!(
                matches!(is_ready(&ctx), ReadyVerdict::NotActionable { .. }),
                "terminal status must be NotActionable"
            );
            prop_assert!(!is_blocked(&ctx));
        }
    }

    /// Every reported blocker id corresponds to an active (gating, non-terminal-source) edge.
    #[test]
    fn reported_blockers_are_active(ctx in arb_ready_context()) {
        if let ReadyVerdict::Blocked { by } = is_ready(&ctx) {
            for id in &by {
                let active = ctx
                    .incoming_blocking
                    .iter()
                    .any(|e| &e.from_id == id
                        && e.dep_type.affects_ready_work()
                        && !e.source_status.is_terminal());
                prop_assert!(active, "reported blocker {id} is not an active edge");
            }
        }
    }

    /// An `Open` issue with no future deferral and no active blockers is `Ready`.
    #[test]
    fn open_unblocked_undeferred_is_ready(ctx in arb_ready_context()) {
        let has_active_blocker = ctx
            .incoming_blocking
            .iter()
            .any(|e| e.dep_type.affects_ready_work() && !e.source_status.is_terminal());
        let deferred_future = ctx.defer_until.is_some_and(|u| u > ctx.now);
        if ctx.status == Status::Open && !has_active_blocker && !deferred_future {
            prop_assert_eq!(is_ready(&ctx), ReadyVerdict::Ready);
        }
    }
}
