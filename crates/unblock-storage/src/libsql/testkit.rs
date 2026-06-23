//! In-module `StorageTestkit` implementation for [`LibsqlStorage`] (gated; resolved-decision #1).
//!
//! Living **inside** the `libsql` module lets this impl reach the `pub(super)` connection accessors
//! ([`LibsqlStorage::read`]/[`LibsqlStorage::write`]) and [`super::ids::next_child_number`] without
//! widening any visibility at the crate root. The two seams it provides exist solely to make two
//! otherwise-unreachable contract paths testable:
//!
//! - [`testkit_insert_raw_edge`](StorageTestkit::testkit_insert_raw_edge) inserts a dependency edge
//!   **bypassing the cycle guard** in [`super::deps::add_dependency`] — the only way to plant a
//!   stored gating cycle so [`crate::Storage::detect_cycles`]' positive path is reachable (the public
//!   `add_dependency` rejects exactly such an edge with `CycleDetected`).
//! - [`testkit_child_high_water`](StorageTestkit::testkit_child_high_water) reads the
//!   `child_counters` high-water mark for a parent (via `ids::next_child_number`) so the suite can
//!   assert the counter advances monotonically past the children created through the public
//!   `create_issue`.
//!
//! Both are compiled only under `#[cfg(any(test, feature = "testkit"))]`; they never enter a
//! production build.

use async_trait::async_trait;
use libsql::params;

use unblock_model::Dependency;

use crate::error::{StorageError, map_libsql_err};
use crate::testkit::StorageTestkit;

use super::LibsqlStorage;
use super::ids::next_child_number;

#[async_trait]
impl StorageTestkit for LibsqlStorage {
    async fn testkit_insert_raw_edge(&self, dep: &Dependency) -> Result<(), StorageError> {
        // Raw INSERT on the write connection: NO cycle guard, NO duplicate guard, NO event. This is
        // the deliberate bypass of `deps::add_dependency` so a gating cycle can be planted in the
        // store for the `detect_cycles` positive-path contract case.
        let conn = self.write().await;
        conn.execute(
            "INSERT INTO dependencies (issue_id, depends_on_id, type, created_at, created_by) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                dep.issue_id.as_str(),
                dep.depends_on_id.as_str(),
                dep.dep_type.as_str(),
                dep.created_at.to_rfc3339(),
                dep.created_by.as_deref().unwrap_or(""),
            ],
        )
        .await
        .map_err(map_libsql_err)?;
        Ok(())
    }

    async fn testkit_child_high_water(&self, parent_id: &str) -> Result<Option<u32>, StorageError> {
        // `next_child_number` returns the NEXT free child number (high-water + 1), reading the
        // `child_counters` table first and falling back to a legacy id scan. The high-water mark is
        // therefore `next - 1`; `None` means no child has ever been allocated under `parent_id`.
        let next = next_child_number(self.read(), parent_id).await?;
        Ok(next.checked_sub(1).filter(|&hw| hw > 0))
    }

    // --- T0.8 contention-lab instrumentation seams -----------------------------------------------
    //
    // Thin async wrappers over the in-`mod.rs` `StorageInstrument` (reached via the `pub(super)`
    // `instrument()` accessor — no crate-root visibility is widened). They are intentionally
    // `async` to satisfy the `StorageTestkit: Storage` (`#[async_trait]`) shape, but do no I/O: the
    // counters are atomics.

    async fn testkit_busy_retry_count(&self) -> u64 {
        self.instrument().busy_retry_count()
    }

    async fn testkit_checkpoint_count(&self) -> u64 {
        self.instrument().checkpoint_count()
    }

    async fn testkit_mutation_count(&self) -> u64 {
        self.instrument().mutation_count()
    }

    async fn testkit_set_checkpoint_interval(&self, n: u64) {
        self.instrument().set_checkpoint_interval(n);
    }

    async fn testkit_set_busy_witness(&self, on: bool) {
        self.instrument().set_busy_witness(on);
    }
}
