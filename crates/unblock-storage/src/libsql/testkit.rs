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
//! - [`testkit_sql_matches_external_prefix`](StorageTestkit::testkit_sql_matches_external_prefix)
//!   (D45) evaluates the SQL twin of the `external:` predicate — `SELECT ?1 LIKE 'external:%'` — in
//!   the DATABASE, so the suite can assert it agrees with the Rust
//!   [`unblock_model::is_external_target`] the write guard calls. The two halves exist only because
//!   SQL cannot call Rust, and they agree by CONTRACT; this seam is what makes a future divergence
//!   go red instead of shipping.
//!
//! All are compiled only under `#[cfg(any(test, feature = "testkit"))]`; they never enter a
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

    async fn testkit_sql_matches_external_prefix(&self, probe: &str) -> Result<bool, StorageError> {
        // The LITERAL SQL twin of `unblock_model::is_external_target` (D45, spine §1.9): the same
        // `LIKE 'external:%'` the ready/blocked queries carry, evaluated by the DATABASE so the
        // contract suite can assert the two halves agree instead of hoping they do.
        let mut rows = self
            .read()
            .query("SELECT ?1 LIKE 'external:%'", params![probe])
            .await
            .map_err(map_libsql_err)?;
        let Some(row) = rows.next().await.map_err(map_libsql_err)? else {
            // A bare `SELECT <expr>` always yields exactly one row; absence is a backend fault.
            return Err(StorageError::Backend {
                source: crate::error::BackendOpaque::from_message(
                    "SELECT ?1 LIKE 'external:%' returned no row",
                ),
            });
        };
        // SQLite renders a boolean as INTEGER 0/1.
        Ok(row.get::<i64>(0).map_err(map_libsql_err)? != 0)
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
