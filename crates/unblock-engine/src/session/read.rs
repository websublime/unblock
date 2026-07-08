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
    CountBucket, CountGroupBy, DepTree, Dependency, DiagnosticKind, DiagnosticReport, Issue,
    ListFilters,
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
    /// `filters` **compose** (D18, spine §3.2.1): the same narrowing facets `list` applies
    /// (status-OR, `issue_type`-OR, priority range, `assignee`, `labels_all`/`labels_any`,
    /// `text_contains`) narrow the blocked set. The baseline is **deferred-inclusive**, so
    /// `include_closed`/`include_deferred` are **no-ops** here (closed/tombstone can never be
    /// blocked-visible; deferred is always shown).
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

    /// The direct dependency edges declared **by** `id` (backs the dep `list` action, spine
    /// §3.2/§4.1 — D1). A read path: **no write permit** (FR-10).
    ///
    /// # Errors
    /// Forwards any storage failure.
    pub async fn list_dependencies(&self, id: &str) -> Result<Vec<Dependency>> {
        Ok(self.storage.list_dependencies(id).await?)
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

    /// Detect every dependency cycle, each as an ordered traversal witness (backs the dep `cycles`
    /// action, spine §4.1 — D19). `blocking_only=true` restricts to the 4 gating types (the ready
    /// view); `=false` considers all dependency types (the integrity/lint view).
    ///
    /// # Errors
    /// Forwards any storage failure.
    pub async fn detect_cycles(&self, blocking_only: bool) -> Result<Vec<Vec<String>>> {
        Ok(self.storage.detect_cycles(blocking_only).await?)
    }

    /// Build the [`DiagnosticReport`] for `kind` (FR-15) — pure-DB, no git (NFR-6).
    ///
    /// The returned report's `kind` is the caller-supplied input (one of the seven constructible
    /// variants); this is the BUILD-now read path (contrast `doctor()`/`recover()`, the `health` seam —
    /// `doctor()` wired at T3.3/HEALTH-LITE, `recover()` at v1.1).
    ///
    /// `since` is the **changelog window** (D26/OQ-1): a bare method argument whose default lives
    /// only on the MCP wire (`DiagnosticsInput::Changelog{since}`), the D19 `detect_cycles(blocking_only)`
    /// precedent. It applies to the [`Changelog`](DiagnosticKind::Changelog) kind only; every other
    /// kind ignores it.
    ///
    /// # Errors
    /// Forwards any storage failure from the underlying probe.
    pub async fn diagnostics(
        &self,
        kind: DiagnosticKind,
        since: Option<DateTime<Utc>>,
    ) -> Result<DiagnosticReport> {
        let facts = WorkspaceFacts {
            actor: &self.actor,
            workspace_dir: &self.workspace_dir,
            unblock_dir: &self.unblock_dir,
            db_path: &self.db_path,
            jsonl_path: &self.jsonl_path,
        };
        diagnostics::diagnostics(self.storage.as_ref(), facts, kind, since).await
    }

    /// Run `PRAGMA integrity_check`, returning the raw problem rows (D27/AF-1, T3.1 — the doctor-lite
    /// input read, spine §4.1).
    ///
    /// Surfaces the existing [`Storage::integrity_check`](unblock_storage::Storage::integrity_check):
    /// a healthy database returns an empty `Vec`; any returned strings are integrity problems. This is
    /// the ONE corruption signal reachable at T3.1. At **T3.3 (HEALTH-LITE, D29)**
    /// [`Session::doctor`](Session::doctor) is wired (integrity + file-state) and the cli `doctor` routes
    /// through it; the full Healthy/Drifted/Recoverable/Unsafe taxonomy + `--repair` land additively at
    /// **v1.1** over the wired `doctor()`/`recover()` seam. A pure read: it **never** acquires the write
    /// permit (FR-10). At T3.1 the cli `doctor` command composes it with `diagnostics(Stats|Lint|Info)`
    /// into a doctor-lite report; a non-empty result maps to `ErrorCode::DatabaseError` (exit 2) at the
    /// cli boundary.
    ///
    /// # Errors
    /// Forwards any storage failure as the transparent `EngineError` source.
    pub async fn integrity_check(&self) -> Result<Vec<String>> {
        Ok(self.storage.integrity_check().await?)
    }
}
