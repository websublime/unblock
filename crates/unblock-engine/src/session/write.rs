//! `impl Session` mutation methods — each acquires the single write permit for the **entire**
//! storage transaction (D14, spine §4.2), then releases on drop.
//!
//! `create`/`update` run `IssueValidator::validate` (→ `ModelError`) **in the engine before** the
//! storage delegation, so validation failures surface as the `EngineError::Model` source (never via
//! `StorageError`, which is validation-free). The permit is held across the whole tx and is
//! cancel-safe: a dropped future before commit releases the permit and leaves the DB
//! committed-or-rolled-back (no partial state — spine §4.2, NFR-5).
//!
//! `close_with_suggestions` closes the issue then computes the **newly-unblocked** set via the
//! `unblock_policy` free functions over caller-built `ReadyContext`s (OQ-1; no policy handle).

use chrono::{DateTime, Utc};
use unblock_model::{
    Dependency, DependencyType, Issue, IssueType, IssueValidator, Priority, Status,
};
use unblock_policy::{BlockingEdge, ReadyContext, ReadyVerdict, is_ready};
use unblock_storage::{DeletePlan, IssuePatch};

use crate::error::{EngineError, Result};
use crate::permit::acquire_write;
use crate::report::CloseOutcome;
use crate::session::Session;
use crate::session::ids::NewIssueSeed;

/// The input to the MINTING create path [`Session::create_issue`] (engine-owned, D21).
///
/// Carries the domain fields of an **interactive** create MINUS the id — the engine mints the id
/// under the write permit. The fields mirror the MCP/CLI `Create` input minus the wire-only knobs
/// (`quick`/attribution are L7 adapter concerns). It is NOT a model DTO and NOT the import shape
/// (the import/internal path is [`Session::create`], which preserves caller ids for FR-26).
#[derive(Debug, Clone, Default)]
pub struct NewIssue {
    /// The issue title (required; also hashed into the id seed).
    pub title: String,
    /// The optional description/body (also hashed into the id seed).
    pub description: Option<String>,
    /// The issue type. `None` → the model default ([`IssueType::default`]).
    pub issue_type: Option<IssueType>,
    /// The priority. `None` → the model default ([`Priority::default`]).
    pub priority: Option<Priority>,
    /// Labels to seed on the new issue.
    pub labels: Vec<String>,
    /// `Some(parent)` mints the hierarchical `parent.N` id via `Storage::next_child_number`.
    pub parent: Option<String>,
    /// Dependency edges to add in/after the same write tx.
    pub deps: Vec<Dependency>,
    /// An optional due date.
    pub due_at: Option<DateTime<Utc>>,
    /// An optional defer-until date.
    pub defer_until: Option<DateTime<Utc>>,
    /// An optional estimate in minutes.
    pub estimated_minutes: Option<i32>,
    /// An optional user slug → the root id is `ub-<slug>-<hash>` (D21).
    pub slug: Option<String>,
    /// Whether the new issue is ephemeral (excluded from JSONL export).
    pub ephemeral: bool,
    // --- markdown-captured content fields (D22/T2.3) — set by the bulk-markdown parser and the MCP
    //     `Create` action so scalar create + bulk are full-fidelity. `create_issue` maps each onto the
    //     built `Issue` field of the same name (the domain `Issue` already carries all four — no model
    //     change). `notes`/`owner` are deliberately absent (no markdown section sets them).
    /// The `### Design` content (maps onto `Issue::design`).
    pub design: Option<String>,
    /// The `### Acceptance Criteria` / `### Acceptance` content (maps onto `Issue::acceptance_criteria`).
    pub acceptance_criteria: Option<String>,
    /// The `### Assignee` content (maps onto `Issue::assignee`).
    pub assignee: Option<String>,
    /// The `### Agent Context` content (maps onto `Issue::agent_context`).
    pub agent_context: Option<String>,
    // --- bulk symbolic-ref carriers (D22/T2.3) — populated ONLY by the bulk-markdown path; the engine
    //     `create_bulk` resolves them under the write permit. Single `create_issue` leaves them empty
    //     (`stand_in_id = None`, `dep_refs = []`) and uses the resolved `deps`/`parent` (byte-unchanged).
    /// The verbatim `### ID` symbolic intra-file handle (NOT the minted id). Bulk-only.
    pub stand_in_id: Option<String>,
    /// The verbatim `### Dependencies` reference strings (`type:id` / bare / `external:` / `blocked-by` /
    /// title / stand-in) the engine resolves at `create_bulk`. Bulk-only (empty for single create).
    pub dep_refs: Vec<String>,
}

impl Session {
    /// Acquire the single write permit, shutdown-aware (D14, spine §4.2).
    async fn acquire(&self) -> Result<crate::permit::WriteGuard> {
        acquire_write(&self.write_permit, &self.shutdown).await
    }

    /// Create an issue, returning its allocated id (FR-1a).
    ///
    /// Runs `IssueValidator::validate` in the engine first (→ `ModelError`), then delegates to
    /// `storage.create_issue` under the write permit. The audit `Event(Created)` is written
    /// transactionally by storage.
    ///
    /// # Errors
    /// - [`EngineError::Model`] if validation fails (aggregate `ValidationFailed`).
    /// - [`EngineError::ShutdownInProgress`] if a shutdown is in progress.
    /// - The transparent storage source on any backend failure (id/external-ref collision, etc.).
    pub async fn create(&self, issue: &Issue) -> Result<String> {
        IssueValidator::validate(issue)?;
        let _guard = self.acquire().await?;
        Ok(self.storage.create_issue(issue, &self.actor).await?)
    }

    /// Create an issue, **minting** its id under the write permit, and return the created `Issue`
    /// (FR-1a, D21 — the INTERACTIVE create path for MCP/CLI).
    ///
    /// Distinct from [`create`](Session::create) (the id-preserving import/internal path): this MINTS
    /// the id via the engine allocator ([`Session::allocate_id`]) — a root `ub-<hash>` / slug
    /// `ub-<slug>-<hash>` (config-derived prefix; the slug fits the prefix budget or drops to
    /// hash-only) or, with `new.parent`, the hierarchical `parent.N`. The whole mint→build→insert runs
    /// under one write permit, so two concurrent creates under one parent cannot mint the same
    /// `parent.N`.
    ///
    /// # Collision handling (no insert-level retry)
    ///
    /// The allocator AVOIDS collisions **before** the insert: its ladder probes each candidate with
    /// `get_issue` and only returns an id that is currently free (extending the hash / bumping the
    /// nonce until a free candidate is found). Because the probe and the insert both run under the
    /// **held** write permit, no other in-process writer can occupy the chosen id in between. A storage
    /// `IdCollision` can therefore only arise from an out-of-band writer that raced the row in after
    /// the probe; if it does, it **PROPAGATES** to the caller as the transparent storage source — this
    /// method does **not** catch it and re-mint. (The probe loop is collision-avoidance, not
    /// post-insert retry.)
    ///
    /// Steps under the permit: (1) mint the id (probing storage); (2) build the candidate `Issue`
    /// (minted id + `new` fields + engine defaults: `created_by = actor`,
    /// `created_at = updated_at = now`); (3) run the full [`IssueValidator::validate`] (the same gate
    /// `create` runs — `ModelError` surfaces as [`EngineError::Model`]); (4) `storage.create_issue`;
    /// (5) add each `new.deps` edge; (6) re-read via `get_issue` and return the hydrated `Issue`.
    ///
    /// # Errors
    /// - [`EngineError::Model`] if the built issue fails validation (aggregate `ValidationFailed`).
    /// - [`EngineError::ShutdownInProgress`] if a shutdown is in progress.
    /// - The transparent storage source on any backend failure (id collision after a probe race,
    ///   `external_ref` collision, a dependency cycle, a missing parent for the child counter, etc.).
    pub async fn create_issue(&self, new: NewIssue) -> Result<Issue> {
        let _guard = self.acquire().await?;

        let now = Utc::now();

        // (1) Mint the id, probing storage (under the held permit, so the probe→insert is atomic).
        let seed = NewIssueSeed {
            title: &new.title,
            description: new.description.as_deref(),
            parent: new.parent.as_deref(),
            slug: new.slug.as_deref(),
            created_at: now,
        };
        let id = self.allocate_id(&seed).await?;

        // (2) Build the candidate Issue (minted id + new fields + engine defaults). The D22
        //     markdown-captured fields map 1:1 onto the same-named built-`Issue` fields (field-wiring
        //     only; the domain `Issue` already carries them — NO model change).
        let issue = Issue {
            id: id.clone(),
            title: new.title,
            description: new.description,
            status: Status::default(),
            priority: new.priority.unwrap_or_default(),
            issue_type: new.issue_type.unwrap_or_default(),
            estimated_minutes: new.estimated_minutes,
            created_at: now,
            created_by: Some(self.actor.clone()),
            updated_at: now,
            due_at: new.due_at,
            defer_until: new.defer_until,
            ephemeral: new.ephemeral,
            labels: new.labels,
            design: new.design,
            acceptance_criteria: new.acceptance_criteria,
            assignee: new.assignee,
            agent_context: new.agent_context,
            ..Issue::default()
        };

        // (3) Validate the built issue the SAME way `create` validates (storage is validation-free).
        IssueValidator::validate(&issue)?;

        // (4) Insert the row + Event(Created) transactionally. The probe loop already chose a free id
        //     under the held permit, so storage's IdCollision guard only fires for an out-of-band race
        //     — and when it does, the `?` PROPAGATES it (no catch-and-re-mint at the insert).
        self.storage.create_issue(&issue, &self.actor).await?;

        // (5) Add the dependency edges in/after the same write tx (parent is already encoded in the
        //     id; `deps` are explicit edges). Each writes its own transactional Event(DependencyAdded).
        for dep in &new.deps {
            self.storage.add_dependency(dep, &self.actor).await?;
        }

        // (6) Re-read the hydrated issue (labels/deps populated) and return it.
        self.storage
            .get_issue(&id)
            .await?
            .ok_or(EngineError::Storage {
                source: unblock_storage::StorageError::IssueNotFound { id },
            })
    }

    /// Create a WHOLE batch atomically — the all-or-nothing bulk MINTING create path (FR-1a, D22/T2.3).
    ///
    /// This is the bulk sibling of [`create_issue`](Session::create_issue); it exists BECAUSE the
    /// minting path is non-idempotent (a loop of N independent `create_issue` calls that fails on
    /// record #k leaves a partial batch, and a re-run re-mints the survivors as DUPLICATES). So the
    /// whole batch MUST be one atomic unit. It acquires the write permit ONCE and:
    ///
    /// 1. orders the records **parent-before-child** topologically over the intra-batch parent edges
    ///    (a parent cycle / ambiguous parent ref → [`EngineError::Model`] `ValidationFailed`, ZERO
    ///    writes);
    /// 2. MINTS every id under the permit in that order via [`allocate_id_in_batch`](Session::allocate_id_in_batch)
    ///    — the existence probe consults committed storage AND an in-batch minted set (intra-batch
    ///    dedup); a same-parent sibling uses the in-batch per-parent counter so siblings get distinct
    ///    `parent.1, parent.2, …` (the committed `next_child_number` sees only committed state);
    /// 3. resolves each record's `dep_refs` + symbolic `parent` against the in-batch title/stand-in
    ///    maps + committed storage (the order **stand-in → title → storage**; `blocked-by`→`blocks`
    ///    flipped at the edge-build step), rejecting the WHOLE batch on ANY ambiguous / unresolved /
    ///    self-dependency / self-parent / marker-only ref ([`EngineError::Model`] `ValidationFailed`,
    ///    ZERO writes — faithful-but-STRICTER than the original per-record skip, NFR-8);
    /// 4. builds the N candidate [`Issue`]s (minted id + fields + the resolved + verbatim dependency
    ///    edges) and runs the FULL [`IssueValidator::validate`] on each (the same gate `create_issue`
    ///    runs);
    /// 5. inserts the whole batch in the ONE `storage.create_issues` tx — rollback-on-any-failure →
    ///    ZERO writes.
    ///
    /// # Errors
    /// - [`EngineError::Model`] (`ValidationFailed`) on a parent cycle, any rejection-set hit, or a
    ///   built-issue validation failure — ZERO writes.
    /// - [`EngineError::ShutdownInProgress`] if a shutdown is in progress.
    /// - The transparent storage source on any backend failure (a raced `IdCollision`, an
    ///   `external_ref` collision, etc.) — the whole tx rolls back → ZERO writes.
    pub async fn create_bulk(&self, records: Vec<NewIssue>) -> Result<Vec<Issue>> {
        use crate::session::bulk::{BatchMaps, topological_mint_order};

        let _guard = self.acquire().await?;
        let now = Utc::now();

        // The intra-batch title/stand-in indices (case-insensitive) consulted by mint + resolution.
        let maps = BatchMaps::build(&records);

        // (1) Parent-before-child topological order (rejects parent cycle / ambiguous parent ref).
        let order = topological_mint_order(&records, &maps).map_err(validation_failed)?;

        // (2) Mint every id in topological order under the held permit.
        let minted_id_of = self.mint_bulk_ids(&records, &maps, &order, now).await?;

        // (3) Probe committed storage once per distinct non-intra-batch dependency reference.
        let storage_resolve = self.probe_storage_dep_refs(&records, &maps).await?;

        // (4) Build + validate the N issues (in FILE order, so the returned Vec mirrors the input).
        let built =
            self.build_bulk_issues(&records, &maps, &minted_id_of, &storage_resolve, now)?;

        // (5) Insert the whole batch in the ONE atomic tx, in PARENT-BEFORE-CHILD topological order
        //     (a child's `parent.N` id bumps the `child_counters` FK → its parent row must exist
        //     first). `order` is the topological index sequence; `built` is file-indexed. The response
        //     is re-projected to FILE order below. Rollback-on-any-failure → ZERO writes.
        let insert_slice: Vec<Issue> = order.iter().map(|&idx| built[idx].clone()).collect();
        self.storage
            .create_issues(&insert_slice, &self.actor)
            .await?;

        // Re-read the hydrated issues (labels/deps populated) in FILE order for the response.
        let ids: Vec<String> = built.iter().map(|i| i.id.clone()).collect();
        let hydrated = self.storage.get_issues(&ids).await?;
        let mut by_id: std::collections::HashMap<String, Issue> =
            hydrated.into_iter().map(|i| (i.id.clone(), i)).collect();
        let mut out = Vec::with_capacity(ids.len());
        for id in &ids {
            if let Some(issue) = by_id.remove(id) {
                out.push(issue);
            }
        }
        Ok(out)
    }

    /// Mint every record's id in topological order under the held permit ([`create_bulk`] step 2).
    ///
    /// `minted` is the in-batch already-minted id set (intra-batch dedup); `child_counters` is the
    /// per-parent in-memory next-child counter; the returned map is `record index → minted id`.
    async fn mint_bulk_ids(
        &self,
        records: &[NewIssue],
        maps: &crate::session::bulk::BatchMaps,
        order: &[usize],
        now: DateTime<Utc>,
    ) -> Result<std::collections::HashMap<usize, String>> {
        use crate::session::bulk::resolve_parent_id;
        use crate::session::ids::NewIssueSeed;

        let mut minted: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut child_counters: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();
        let mut minted_id_of: std::collections::HashMap<usize, String> =
            std::collections::HashMap::new();

        for &idx in order {
            let record = &records[idx];

            // Resolve the symbolic parent for MINTING `parent.N`: an intra-batch parent (already minted
            // in this topological order) or a pre-existing storage id (probed once). A root has none.
            let storage_parent =
                match (record.parent.as_deref(), maps.lookup_is_intra_batch(record)) {
                    (Some(parent_ref), false) => {
                        // Not an intra-batch ref → resolve against committed storage.
                        if self.storage.get_issue(parent_ref).await?.is_some() {
                            Some(parent_ref.to_string())
                        } else {
                            None
                        }
                    }
                    _ => None,
                };
            let resolved_parent = resolve_parent_id(record, maps, &minted_id_of, storage_parent)
                .map_err(validation_failed)?;

            let seed = NewIssueSeed {
                title: &record.title,
                description: record.description.as_deref(),
                parent: resolved_parent.as_deref(),
                slug: record.slug.as_deref(),
                created_at: now,
            };
            let id = self
                .allocate_id_in_batch(&seed, &minted, &mut child_counters)
                .await?;
            minted.insert(id.clone());
            minted_id_of.insert(idx, id);
        }
        Ok(minted_id_of)
    }

    /// Build + validate the N candidate issues in FILE order ([`create_bulk`] step 4). Rejects the
    /// whole batch on any unresolved/ambiguous/self/marker dep ref or a built-issue validation failure.
    /// Pure (no `await`): the mint + storage probes already ran; this just resolves edges in memory.
    fn build_bulk_issues(
        &self,
        records: &[NewIssue],
        maps: &crate::session::bulk::BatchMaps,
        minted_id_of: &std::collections::HashMap<usize, String>,
        storage_resolve: &std::collections::HashMap<String, Option<String>>,
        now: DateTime<Utc>,
    ) -> Result<Vec<Issue>> {
        use crate::session::bulk::resolve_dep_refs;

        let mut built: Vec<Issue> = Vec::with_capacity(records.len());
        for (idx, record) in records.iter().enumerate() {
            let id = minted_id_of
                .get(&idx)
                .cloned()
                .ok_or_else(|| validation_failed(vec![]))?;

            let resolved_edges = resolve_dep_refs(record, &id, maps, minted_id_of, storage_resolve)
                .map_err(validation_failed)?;

            // Merge the already-resolved `deps` (verbatim) with the resolved `dep_refs` edges, deduped
            // by depends_on_id (storage also dedups, but keep the built Issue clean).
            let mut dependencies: Vec<Dependency> = record.deps.clone();
            let mut seen: std::collections::HashSet<String> = dependencies
                .iter()
                .map(|d| d.depends_on_id.clone())
                .collect();
            for edge in resolved_edges {
                if seen.insert(edge.depends_on_id.clone()) {
                    dependencies.push(Dependency {
                        issue_id: id.clone(),
                        depends_on_id: edge.depends_on_id,
                        dep_type: edge.dep_type,
                        created_at: now,
                        created_by: Some(self.actor.clone()),
                        metadata: None,
                        thread_id: None,
                    });
                }
            }

            let issue = Issue {
                id,
                title: record.title.clone(),
                description: record.description.clone(),
                status: Status::default(),
                priority: record.priority.unwrap_or_default(),
                issue_type: record.issue_type.clone().unwrap_or_default(),
                estimated_minutes: record.estimated_minutes,
                created_at: now,
                created_by: Some(self.actor.clone()),
                updated_at: now,
                due_at: record.due_at,
                defer_until: record.defer_until,
                ephemeral: record.ephemeral,
                labels: record.labels.clone(),
                design: record.design.clone(),
                acceptance_criteria: record.acceptance_criteria.clone(),
                assignee: record.assignee.clone(),
                agent_context: record.agent_context.clone(),
                dependencies,
                ..Issue::default()
            };

            // The SAME validation gate `create_issue` runs (storage stays validation-free).
            IssueValidator::validate(&issue)?;
            built.push(issue);
        }
        Ok(built)
    }

    /// Probe committed storage once per distinct non-intra-batch dependency reference, returning a
    /// `ref → Option<resolved_id>` map [`create_bulk`](Session::create_bulk) step 4 consults. A
    /// reference matching a batch record is excluded (it resolves intra-batch, not via storage).
    async fn probe_storage_dep_refs(
        &self,
        records: &[NewIssue],
        maps: &crate::session::bulk::BatchMaps,
    ) -> Result<std::collections::HashMap<String, Option<String>>> {
        // Collect the distinct candidate ids (the id-half of each non-intra-batch dep_ref).
        let mut candidates: std::collections::HashSet<String> = std::collections::HashSet::new();
        for record in records {
            for dep_str in &record.dep_refs {
                if maps.lookup(dep_str).is_some() {
                    continue; // resolves intra-batch (incl. titles with colons) — not a storage probe.
                }
                let dep_id = crate::session::bulk::dep_ref_id(dep_str);
                if maps.lookup(&dep_id).is_none() {
                    candidates.insert(dep_id);
                }
            }
        }
        let mut resolved: std::collections::HashMap<String, Option<String>> =
            std::collections::HashMap::new();
        for candidate in candidates {
            let found = self.storage.get_issue(&candidate).await?.map(|i| i.id);
            resolved.insert(candidate, found);
        }
        Ok(resolved)
    }

    /// Apply an [`IssuePatch`] to an issue, returning the updated issue (FR-1b).
    ///
    /// Validates the *post-patch* issue the **same way `create` validates** (the storage
    /// `update_issue` is validation-free, spine §3.2.1): under the write permit it **loads the
    /// current issue**, merges every patch field into a candidate [`Issue`], runs the **full**
    /// [`IssueValidator::validate`] on that merged candidate, and only then delegates to
    /// `storage.update_issue`. So a patch that would set a blank/whitespace `title`, introduce a NUL
    /// byte, or push an over-length/whitespace `external_ref` surfaces [`EngineError::Model`] (the
    /// aggregate `ValidationFailed`) and leaves the DB unchanged — closing the update-path
    /// data-integrity hole. The load + validate + update all run under one permit (linearizable).
    ///
    /// # Errors
    /// - [`EngineError::Model`] if the merged candidate fails validation (aggregate `ValidationFailed`).
    /// - The transparent storage source `IssueNotFound` if the issue does not exist.
    /// - [`EngineError::ShutdownInProgress`] / transparent storage source.
    pub async fn update(&self, id: &str, patch: &IssuePatch) -> Result<Issue> {
        let _guard = self.acquire().await?;
        // Load the current issue (under the permit, so the validate→update window is serialized).
        let Some(current) = self.storage.get_issue(id).await? else {
            return Err(EngineError::Storage {
                source: unblock_storage::StorageError::IssueNotFound { id: id.to_string() },
            });
        };
        // Merge the patch into a candidate and run the FULL validator (the same gate `create` runs);
        // storage trusts a validated row. A failure surfaces as the EngineError::Model aggregate.
        let candidate = apply_patch_for_validation(&current, patch);
        IssueValidator::validate(&candidate)?;
        Ok(self.storage.update_issue(id, patch, &self.actor).await?)
    }

    /// Execute (or, for `DeleteMode::DryRun`, plan) a delete (FR-1c).
    ///
    /// `DryRun` mutates nothing. Under the write permit (a non-`DryRun` mode writes rows + audit).
    ///
    /// # Errors
    /// - [`EngineError::ShutdownInProgress`] / transparent storage source.
    pub async fn delete(&self, plan: &DeletePlan) -> Result<DeletePlan> {
        let _guard = self.acquire().await?;
        Ok(self.storage.delete_issue(plan, &self.actor).await?)
    }

    /// Restore (un-tombstone) a SOFT-deleted issue (FR-1c "recoverable", D20).
    ///
    /// The dedicated inverse of a soft delete: it acquires the write permit for the whole storage tx
    /// and delegates to `storage.restore_issue` (the engine supplies the actor). An already-active id
    /// is an idempotent `Ok`; a missing/hard-deleted id surfaces the transparent `IssueNotFound`
    /// source. A single `Event(Restored)` is written transactionally by storage.
    ///
    /// This is **structurally distinct** from the reopen=update mapping (spine §5.2): a tombstone
    /// cannot be patched via `update` — the storage tombstone-patch guard fires first (spine §3.2.1
    /// `update_issue`) — so reopen=update never reaches a tombstone. `restore` is the dedicated
    /// terminal(tombstone)→active path; the two are not unified.
    ///
    /// # Errors
    /// - [`EngineError::ShutdownInProgress`] / transparent storage source (incl. `IssueNotFound` for
    ///   a missing or hard-deleted id).
    pub async fn restore(&self, id: &str) -> Result<Issue> {
        let _guard = self.acquire().await?;
        Ok(self.storage.restore_issue(id, &self.actor).await?)
    }

    /// Atomically claim an issue for `assignee` (FR-2).
    ///
    /// A single conditional `UPDATE` so concurrent claimers cannot both win; the loser surfaces
    /// `StorageError::AlreadyClaimed` (→ the transparent source). Under the write permit.
    ///
    /// # Errors
    /// - [`EngineError::ShutdownInProgress`] / transparent storage source (incl. `AlreadyClaimed`).
    pub async fn claim(&self, id: &str, assignee: &str) -> Result<Issue> {
        let _guard = self.acquire().await?;
        Ok(self.storage.claim_issue(id, assignee, &self.actor).await?)
    }

    /// Defer an issue until `until` (FR-3). Under the write permit.
    ///
    /// # Errors
    /// - [`EngineError::ShutdownInProgress`] / transparent storage source.
    pub async fn defer(&self, id: &str, until: DateTime<Utc>) -> Result<Issue> {
        let _guard = self.acquire().await?;
        Ok(self.storage.defer_issue(id, until, &self.actor).await?)
    }

    /// Undefer an issue (FR-3). Under the write permit.
    ///
    /// # Errors
    /// - [`EngineError::ShutdownInProgress`] / transparent storage source.
    pub async fn undefer(&self, id: &str) -> Result<Issue> {
        let _guard = self.acquire().await?;
        Ok(self.storage.undefer_issue(id, &self.actor).await?)
    }

    /// Add a dependency edge (FR-5). Rejects cycles with a path (→ transparent source). Under the
    /// write permit.
    ///
    /// # Errors
    /// - [`EngineError::ShutdownInProgress`] / transparent storage source (incl. `CycleDetected`).
    pub async fn add_dep(&self, dep: &Dependency) -> Result<()> {
        let _guard = self.acquire().await?;
        self.storage.add_dependency(dep, &self.actor).await?;
        Ok(())
    }

    /// Remove a dependency edge (FR-5). Under the write permit.
    ///
    /// # Errors
    /// - [`EngineError::ShutdownInProgress`] / transparent storage source.
    pub async fn remove_dep(&self, issue_id: &str, on: &str, ty: &DependencyType) -> Result<()> {
        let _guard = self.acquire().await?;
        self.storage
            .remove_dependency(issue_id, on, ty, &self.actor)
            .await?;
        Ok(())
    }

    /// Close an issue and report the issues it newly unblocked (FR-11).
    ///
    /// Closes by delegating a `status = Closed` patch (carrying the optional `reason`) to
    /// `storage.update_issue` (storage derives `closed_at`, **persists `close_reason`**, and writes
    /// the `StatusChanged`/`Closed` events transactionally), then computes the **newly-unblocked**
    /// set via the `unblock_policy` free functions: every issue that had a gating edge **to** the
    /// closed id is re-evaluated against its live incoming edges, and those that are now
    /// [`ReadyVerdict::Ready`] are returned (OQ-1 — no policy handle). The whole operation runs under
    /// one write permit.
    ///
    /// # `reason` persistence
    ///
    /// `reason` is persisted to the issue's `close_reason` column via the patch's
    /// `close_reason: Some(Some(reason))` field (spine §3.1/§4.1, T1.2 — no longer tracing-only). A
    /// `None` reason leaves `close_reason` unchanged (the patch field stays `None`). The
    /// `close_reason` column is not part of the frozen `content_hash` (spine §1.8), so persisting it
    /// does not perturb import idempotency (FR-26).
    ///
    /// # Errors
    /// - [`EngineError::ShutdownInProgress`] / transparent storage source.
    pub async fn close_with_suggestions(
        &self,
        id: &str,
        reason: Option<String>,
    ) -> Result<CloseOutcome> {
        let _guard = self.acquire().await?;

        // Close: status -> Closed, persisting the reason (Some => set; None => leave unchanged).
        // Storage derives closed_at + the Closed/StatusChanged events transactionally.
        let patch = IssuePatch {
            status: Some(Status::Closed),
            close_reason: reason.map(Some),
            ..IssuePatch::default()
        };
        let closed = self.storage.update_issue(id, &patch, &self.actor).await?;

        // Compute newly-unblocked via the policy free fns over the live dependents (OQ-1).
        let newly_unblocked = self.newly_unblocked_after_close(id).await?;

        Ok(CloseOutcome {
            closed,
            newly_unblocked,
        })
    }

    /// Re-evaluate every issue that had a gating edge **to** `closed_id` and return those now ready
    /// (FR-11). Uses the `unblock_policy` free functions over caller-built `ReadyContext`s (OQ-1).
    ///
    /// Reads only (the caller already holds the write permit for the close; this just queries the
    /// post-close state through the same `Storage`).
    async fn newly_unblocked_after_close(&self, closed_id: &str) -> Result<Vec<Issue>> {
        // The whole graph: edges are (from = dependent issue_id, to = blocker depends_on_id).
        let graph = self.storage.dependency_graph(&[]).await?;

        // Candidates = the `from` of every edge whose `to` is the just-closed issue (its dependents),
        // de-duplicated and excluding the closed issue itself.
        let mut candidate_ids: Vec<String> = graph
            .edges
            .iter()
            .filter(|edge| edge.to == closed_id && edge.from != closed_id)
            .map(|edge| edge.from.clone())
            .collect();
        candidate_ids.sort();
        candidate_ids.dedup();
        if candidate_ids.is_empty() {
            return Ok(Vec::new());
        }

        // Hydrate every candidate plus every blocker referenced by their incoming edges, so the
        // ReadyContext carries each blocker's LIVE status (post-close).
        let candidates = self.storage.get_issues(&candidate_ids).await?;

        // Blocker ids = the `to` of every edge whose `from` is a candidate.
        let candidate_set: std::collections::HashSet<&str> =
            candidate_ids.iter().map(String::as_str).collect();
        let mut blocker_ids: Vec<String> = graph
            .edges
            .iter()
            .filter(|edge| candidate_set.contains(edge.from.as_str()))
            .map(|edge| edge.to.clone())
            .collect();
        blocker_ids.sort();
        blocker_ids.dedup();
        let blockers = self.storage.get_issues(&blocker_ids).await?;
        let status_of: std::collections::HashMap<&str, Status> = blockers
            .iter()
            .map(|issue| (issue.id.as_str(), issue.status.clone()))
            .collect();

        let now = Utc::now();
        let mut newly_unblocked = Vec::new();
        for candidate in candidates {
            // Build the candidate's incoming gating edges with live source statuses.
            let incoming_blocking: Vec<BlockingEdge> = graph
                .edges
                .iter()
                .filter(|edge| edge.from == candidate.id)
                .map(|edge| BlockingEdge {
                    from_id: edge.to.clone(),
                    dep_type: edge.dep_type.clone(),
                    // A blocker not in the hydrated set (e.g. an `external:%` placeholder the storage
                    // graph omits) is treated as still-open (conservative — it would NOT mark the
                    // issue ready).
                    source_status: status_of
                        .get(edge.to.as_str())
                        .cloned()
                        .unwrap_or(Status::Open),
                })
                .collect();

            let ctx = ReadyContext {
                status: candidate.status.clone(),
                defer_until: candidate.defer_until,
                incoming_blocking,
                now,
            };
            if matches!(is_ready(&ctx), ReadyVerdict::Ready) {
                newly_unblocked.push(candidate);
            }
        }

        // Stable, deterministic order (id ASC) for snapshot-friendliness.
        newly_unblocked.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(newly_unblocked)
    }
}

/// Merge an [`IssuePatch`] into a clone of `current`, producing the candidate [`Issue`] that storage
/// would persist — so the engine can run the **full** [`IssueValidator::validate`] on it before
/// delegating (storage's `update_issue` is validation-free). It mirrors the storage apply rules for
/// every field the validator inspects, including the `status`-derived `closed_at` transition (set on
/// →`Closed`, cleared on reopen) so a legitimate close-via-update does not spuriously fail the
/// closed-state coherence rule.
///
/// `due_at`/`close_reason` and the `parent` reparent are not validated by `IssueValidator`, so they
/// are applied where they affect a validated field (none) and otherwise omitted from the candidate —
/// they cannot make a row invalid. Label ops (`labels_set`/`labels_add`/`labels_remove`) ARE merged
/// (the validator bounds the label count + charset).
fn apply_patch_for_validation(current: &Issue, patch: &IssuePatch) -> Issue {
    use std::collections::BTreeSet;

    let mut candidate = current.clone();

    if let Some(title) = &patch.title {
        candidate.title.clone_from(title);
    }
    apply_opt_text(&patch.description, &mut candidate.description);
    apply_opt_text(&patch.design, &mut candidate.design);
    apply_opt_text(
        &patch.acceptance_criteria,
        &mut candidate.acceptance_criteria,
    );
    apply_opt_text(&patch.notes, &mut candidate.notes);
    apply_opt_text(&patch.owner, &mut candidate.owner);
    apply_opt_text(&patch.external_ref, &mut candidate.external_ref);
    apply_opt_text(&patch.assignee, &mut candidate.assignee);
    apply_opt_text(&patch.close_reason, &mut candidate.close_reason);

    if let Some(priority) = patch.priority {
        candidate.priority = priority;
    }
    if let Some(issue_type) = &patch.issue_type {
        candidate.issue_type = issue_type.clone();
    }
    if let Some(minutes) = patch.estimated_minutes {
        candidate.estimated_minutes = Some(minutes);
    }
    if let Some(due) = patch.due_at {
        candidate.due_at = Some(due);
    }

    // Status transition: replicate storage's closed_at derivation so the closed-state coherence rule
    // matches what would actually be persisted (Closed sets closed_at; a non-terminal status clears
    // it; Tombstone leaves closed_at as-is — the validator permits a tombstone with any closed_at).
    if let Some(status) = &patch.status
        && status.as_str() != candidate.status.as_str()
    {
        match status {
            Status::Closed => {
                if candidate.closed_at.is_none() {
                    candidate.closed_at = Some(Utc::now());
                }
            }
            Status::Tombstone => {}
            _ => candidate.closed_at = None,
        }
        candidate.status = status.clone();
    }

    // Labels: set replaces, add inserts, remove deletes — order-independent, deduped (mirrors
    // storage's apply_labels reconciliation), so the count/charset validation sees the final set.
    let mut labels: BTreeSet<String> = candidate.labels.iter().cloned().collect();
    if let Some(set_labels) = &patch.labels_set {
        labels = set_labels.iter().cloned().collect();
    }
    for add in &patch.labels_add {
        labels.insert(add.clone());
    }
    for remove in &patch.labels_remove {
        labels.remove(remove);
    }
    candidate.labels = labels.into_iter().collect();

    candidate
}

/// Apply a nullable-text patch field (`None` leave / `Some(None)` clear / `Some(Some)` set) onto a
/// candidate field, mirroring the storage `push_opt_text` semantics. The nested `Option` and the
/// borrow mirror the [`IssuePatch`](unblock_storage::IssuePatch) field shape (outer=present-in-patch,
/// inner=clear-vs-set), so both pedantic lints are intentionally scoped here.
#[allow(clippy::option_option, clippy::ref_option)]
fn apply_opt_text(patch: &Option<Option<String>>, target: &mut Option<String>) {
    if let Some(new) = patch {
        target.clone_from(new);
    }
}

/// Map a non-empty set of bulk-resolution [`FieldError`](unblock_error::FieldError)s into the engine
/// `ValidationFailed` aggregate (D22/T2.3 — the whole-batch reject set surfaces as one
/// [`EngineError::Model`], ZERO writes).
fn validation_failed(fields: Vec<unblock_error::FieldError>) -> EngineError {
    EngineError::Model {
        source: unblock_error::ModelError::ValidationFailed { fields },
    }
}
