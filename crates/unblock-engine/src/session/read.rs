//! `impl Session` read methods — **never acquire the write permit** (FR-10, conformance rule 4).
//!
//! Each delegates to `Storage`, mapping `StorageError → EngineError` transparently (the `?` on the
//! transparent source). `ready()` re-ranks `storage.ready_issues` (the candidate pre-sort, spine
//! §3.2.1) via the `unblock_policy::cmp_ready` **free function** (the pinned bucket-hybrid
//! comparator; no policy handle — OQ-1) — `issues.sort_by(unblock_policy::cmp_ready)` after the
//! storage call (spine §4.1 NORMATIVE). `diagnostics()` dispatches a `DiagnosticKind` over pure-DB
//! storage calls (FR-15; no git, NFR-6).

use chrono::{DateTime, Utc};
use unblock_model::{
    CountBucket, CountGroupBy, DepTree, DiagnosticKind, DiagnosticReport, Issue, ListFilters,
};

use crate::diagnostics::{self, WorkspaceFacts};
use crate::error::Result;
use crate::session::Session;

impl Session {
    /// Fetch a single issue by id (hydrated), or `None` if absent.
    ///
    /// # Errors
    /// Forwards any storage failure as the transparent `EngineError` source.
    pub async fn get(&self, id: &str) -> Result<Option<Issue>> {
        Ok(self.storage.get_issue(id).await?)
    }

    /// List issues matching `filters`.
    ///
    /// # Errors
    /// Forwards any storage failure.
    pub async fn list(&self, filters: &ListFilters) -> Result<Vec<Issue>> {
        Ok(self.storage.list_issues(filters).await?)
    }

    /// The ready set, **re-ranked by the hybrid policy comparator** (spine §4.1 NORMATIVE).
    ///
    /// Calls `storage.ready_issues` (the candidate set, pre-sorted `priority ASC, created_at ASC,
    /// id ASC`) then re-ranks **in the engine** via `unblock_policy::cmp_ready` (which buckets P0/P1
    /// together, so the final order differs from the SQL pre-sort) — CF-11. No write permit (FR-10).
    ///
    /// # Errors
    /// Forwards any storage failure.
    pub async fn ready(&self, filters: &ListFilters) -> Result<Vec<Issue>> {
        let mut issues = self.storage.ready_issues(filters).await?;
        issues.sort_by(unblock_policy::cmp_ready);
        Ok(issues)
    }

    /// The blocked set (issues with at least one unresolved gating edge).
    ///
    /// # Errors
    /// Forwards any storage failure.
    pub async fn blocked(&self, filters: &ListFilters) -> Result<Vec<Issue>> {
        Ok(self.storage.blocked_issues(filters).await?)
    }

    /// Search over `query` honouring `filters` (the engine applies the default cap of 50 when no
    /// `filters.limit` is set, via the configured `search_cap`).
    ///
    /// # Errors
    /// Forwards any storage failure.
    pub async fn search(&self, query: &str, filters: &ListFilters) -> Result<Vec<Issue>> {
        // Storage honours `filters.limit`; the engine fills the default cap (FR-4) when unset.
        let mut filters = filters.clone();
        if filters.limit.is_none() {
            filters.limit = Some(self.config.search_cap);
        }
        Ok(self.storage.search_issues(query, &filters).await?)
    }

    /// Count issues matching `filters`, optionally grouped.
    ///
    /// # Errors
    /// Forwards any storage failure.
    pub async fn count(
        &self,
        filters: &ListFilters,
        by: Option<CountGroupBy>,
    ) -> Result<Vec<CountBucket>> {
        Ok(self.storage.count_issues(filters, by).await?)
    }

    /// Stale issues (not updated since `older_than`) matching `filters`.
    ///
    /// # Errors
    /// Forwards any storage failure.
    pub async fn stale(
        &self,
        older_than: DateTime<Utc>,
        filters: &ListFilters,
    ) -> Result<Vec<Issue>> {
        Ok(self.storage.stale_issues(older_than, filters).await?)
    }

    /// The dependency subtree rooted at `id`.
    ///
    /// # Errors
    /// Forwards any storage failure.
    pub async fn dependency_tree(&self, id: &str) -> Result<DepTree> {
        Ok(self.storage.dependency_tree(id).await?)
    }

    /// The dependency graph for a root set (empty `roots` = the whole graph), backing the dep
    /// `graph` action (spine §3.2/§4.1, G-23c).
    ///
    /// # Errors
    /// Forwards any storage failure.
    pub async fn dependency_graph(&self, roots: &[String]) -> Result<DepTree> {
        Ok(self.storage.dependency_graph(roots).await?)
    }

    /// Detect every dependency cycle, each as a path of ids.
    ///
    /// # Errors
    /// Forwards any storage failure.
    pub async fn detect_cycles(&self) -> Result<Vec<Vec<String>>> {
        Ok(self.storage.detect_cycles().await?)
    }

    /// Build the [`DiagnosticReport`] for `kind` (FR-15) — pure-DB, no git (NFR-6).
    ///
    /// The returned report's `kind` is the caller-supplied input (one of the seven constructible
    /// variants); this is the BUILD-now read path (contrast `doctor`/`recover`, seamed to T3.3).
    ///
    /// # Errors
    /// Forwards any storage failure from the underlying probe.
    pub async fn diagnostics(&self, kind: DiagnosticKind) -> Result<DiagnosticReport> {
        let facts = WorkspaceFacts {
            actor: &self.actor,
            workspace_dir: &self.workspace_dir,
            unblock_dir: &self.unblock_dir,
            db_path: &self.db_path,
            jsonl_path: &self.jsonl_path,
        };
        diagnostics::diagnostics(self.storage.as_ref(), facts, kind).await
    }
}
