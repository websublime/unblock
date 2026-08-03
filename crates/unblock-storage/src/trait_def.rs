//! The backend-agnostic async [`Storage`] trait (spine §3.2) — the contract every backend
//! implements. This file is **pure declaration + doc-comments**: no backend, no I/O. The libsql
//! implementation lands at T0.6; the backend-independent contract suite that *verifies* these
//! preconditions lands at T0.7.
//!
//! The trait is **object-safe** (`Arc<dyn Storage>` is the shape `unblock-config` builds and
//! `unblock-engine` consumes, spine §4): every method takes `&self`, all are `async fn` lowered by
//! `#[async_trait]` to `Pin<Box<dyn Future>>`, and there are no generic methods.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use unblock_model::{
    Comment, CountBucket, CountGroupBy, DepTree, Dependency, DependencyType, Event, GraphEdge,
    Issue, ListFilters,
};

use crate::WriteLockGuard;
use crate::error::StorageError;
use crate::filters::{DeletePlan, IssuePatch};

// NOTE: the CF-E reserved seams (`read_config`/`diagnostic_probe`/`diagnostic_probes`) reference
// `DiagnosticKind`/`DiagnosticReport`. They are kept **commented** below per spine §3.2 (reserved
// for v1.1, NOT live default methods), so those DTOs are intentionally NOT imported here — importing
// them would trip the unused-import lint.

/// The backend-agnostic storage contract (spine §3.2).
///
/// Async throughout (`#[async_trait]`); `Send + Sync` so it can be shared as `Arc<dyn Storage>`
/// across tokio tasks. The only backend-aware implementation is libsql (T0.6); a future backend
/// reuses the T0.7 contract suite. **No backend type appears in any signature** — failures surface
/// as [`StorageError`] (spine §6 rule 2).
///
/// # General invariants (honoured by the T0.6 impl, verified by the T0.7 suite)
///
/// - **Transactional audit (FR-9):** every mutation writes its [`Event`](unblock_model::Event)(s)
///   in the **same transaction** as the row change — rows and audit commit together or not at all.
/// - **No git, no network (NFR-6):** no method shells to git or links a git library; reads are
///   plain WAL reads. The `remote` path (T0.6+, non-default) is the only network surface.
/// - **Reads never serialize:** the write-serialization permit lives in `unblock-engine` (D14);
///   storage reads run concurrently against WAL readers (FR-10).
/// - **Storage never imports policy (CF-11):** ready/blocked ordering is deterministic for stable
///   snapshots, but the hybrid re-rank is applied by the engine via policy.
#[async_trait]
pub trait Storage: Send + Sync {
    // ---------------------------------------------------------------------------------------------
    // lifecycle
    // ---------------------------------------------------------------------------------------------

    /// Bring the on-disk schema to the current version, idempotently, and stamp it.
    ///
    /// # THE MIGRATION CONTRACT (D46, v1.0.1 — NORMATIVE, spine §3.2)
    ///
    /// Ordered forward steps, lowest first. Note what cannot substitute for one: the embedded
    /// `SCHEMA_SQL` is `CREATE … IF NOT EXISTS` throughout, so **re-applying it can never ADD a
    /// column**. Re-application is not a repair mechanism; a step is.
    ///
    /// **(0) THE FROZEN-BASELINE DISCIPLINE — the rule a future step author needs, so read it
    /// first.** `SCHEMA_SQL` IS FROZEN at the shape it had when the ladder began: a 5-column
    /// `comments` table (`id, issue_id, author, text, created_at`). **ADDING A STEP DOES NOT PERMIT
    /// EDITING `SCHEMA_SQL` — the two are ALTERNATIVES, never a pair.** Every element added after the
    /// baseline exists ONLY as a step; the constant deliberately no longer describes the current
    /// schema, and a reader who needs that shape REPLAYS the ladder over it. A fresh database is
    /// created at the BASELINE, stamped `BASELINE_SCHEMA_VERSION`, and then FALLS THROUGH the ladder
    /// like any other, so **there is exactly ONE path to the current shape and every fresh install
    /// exercises every step.** That is the point: this defect existed because the fresh path worked
    /// while the ladder path was never exercised even once.
    ///
    /// **(i) THE INVARIANT — A STAMPED VERSION IMPLIES A KNOWN SHAPE.** From step 3 onward every step
    /// applies its DDL UNCONDITIONALLY, because the version it advances FROM denotes exactly one
    /// physical shape. **A step MUST NOT inspect the database to decide what to do.** This is the
    /// sentence whose absence caused the defect D46 repairs: stamp `1` was allowed to mean two
    /// different `comments` tables, so a binary could not tell a stale database from a current one.
    /// **(0) is what makes this PROVABLE rather than asserted:** since `SCHEMA_SQL` never carries a
    /// post-baseline column, no fresh database can already have one, and no other creation path
    /// exists — so a database stamped `N` has run exactly steps `BASELINE + 1 ..= N` and its shape IS
    /// their composition. Were the DDL allowed to grow in place alongside the ladder, the first step
    /// whose column was also added to the DDL would hard-error `duplicate column name` on every fresh
    /// install.
    ///
    /// **(ii) THE ONE-TIME EXCEPTION — step 2, and NO other step, ever.** Stamp `1` covers TWO
    /// physical shapes (`comments` with 5 columns before 2026-07-17; 7 from `v1.0.0-rc.4` on, because
    /// D37 edited the baseline `CREATE TABLE` in place instead of shipping a step). Step 2 therefore
    /// SENSES the shape before it acts — one `PRAGMA table_info(comments)`, adding only the columns
    /// actually absent — and it is carried by an UNPARAMETERISED, single-purpose step kind precisely
    /// so that a later step cannot reuse it. **The sensing is load-bearing on the LARGEST half of the
    /// population, not a historical courtesy:** an existing GA database carries all SEVEN columns and
    /// is stamped `1`, so under (0) it falls through the ladder and reaches step 2 with the columns
    /// ALREADY PRESENT. Step 2 is the one step whose FROM-version does not denote one shape — the debt
    /// it pays off. **Copying its shape into a step 3 or later is a contract violation, not a style
    /// choice.** Once every stamp-`1` database has become stamp-`2`, (i) holds for every version this
    /// product will ever write; there is no second exception and none may be minted.
    ///
    /// **(iii) ATOMICITY** — a step's DDL and its `PRAGMA user_version` write commit TOGETHER in ONE
    /// `BEGIN IMMEDIATE`. A step that applied DDL and stamped separately could crash between them and
    /// manufacture a third shape, which is exactly the ambiguity (i) exists to forbid.
    ///
    /// **(iv) WHEN IT RUNS (the D46 policy, binding on every future step).** IMPLICIT ON OPEN is
    /// permitted ONLY for an ADDITIVE/nullable step: the config open facade already migrates on open
    /// (FR-9 single open path) and already holds the D31 advisory `.write.lock` across the version
    /// read and the run, so real DDL there introduces no new lock discipline. A DESTRUCTIVE,
    /// DATA-REWRITING or LONG-RUNNING step is EXPLICIT-ONLY, gated behind the `unblock migrate`
    /// command — it may not run inside an `unblock mcp` startup an agent never asked for. **The
    /// STEP'S CLASS decides, never the caller's convenience.**
    ///
    /// **(v) A STAMP THAT LIES IS AN ERROR, never a silent read failure.** `migrate` ends on EVERY
    /// path — including the already-at-current early return — by witnessing the NEWEST step's own
    /// columns, returning [`StorageError::Migration`] (→ `SchemaMismatch`, exit 2) naming what is
    /// missing. It is a bounded per-step POSTCONDITION: its result never decides which DDL to run
    /// (only step 2 ever decides anything from a probe), and it is NOT a conformance comparison of
    /// the live schema against `SCHEMA_SQL` — that is deliberately out of scope (PRD §4, D46).
    /// **THE ERROR CARRIES A HINT (NORMATIVE).** "Actionable" is delivered literally: the failure
    /// attaches a `StructuredError.hint` saying WHAT HAPPENED (the stamp found, the version this
    /// build expects, the columns actually missing) and WHAT TO RUN. It is NOT a per-code constant,
    /// because this code also serves the opposite direction ((vi)), where the remedy is the opposite
    /// instruction. It is composed by `impl CodedError for StorageError`'s `hint()`
    /// (`crate::StorageError`, `src/error.rs`) from the failing variant's own fields, which
    /// `src/libsql/migrations.rs` populates. On the (iv) implicit-on-open path `ConfigError` FORWARDS
    /// `source.hint()` on its `DbOpenFailed`/`MigrationFailed` variants — without that arm the trait
    /// DEFAULT `hint() -> None` would drop it before `StructuredError` is built. The hint may NOT
    /// advise a recovery the product cannot perform in that state: "export and re-import" is invalid
    /// while the shape is stale, because `sync export` is itself among the tools a stale `comments`
    /// table breaks.
    ///
    /// **(vi) THE TWO ENDS.** A database stamped NEWER than this build → [`StorageError::SchemaMismatch`]
    /// — which, since D46 bumps the stamp, is a REACHABLE direction and not a hypothetical: a GA
    /// `1.0.0` binary meeting a database this ladder moved to `2` REFUSES it with exit 2. PRD §4 D46
    /// records that this is NOT a breaking change under D35 (D35's stable set is the MCP contract, the
    /// CLI lifecycle surface and the 0–8 exit codes — the ON-DISK SCHEMA is not among them), and
    /// refusing with a clear error is the CORRECT behaviour, strictly better than misreading. A
    /// database stamped `0` that ALREADY carries tables takes the BASELINE stamp and then FALLS
    /// THROUGH the ladder — never the current stamp directly, which would assert a shape nobody
    /// verified and put the database beyond the reach of the very step that repairs it. Under (0)
    /// that is ALSO the rule for a TRULY EMPTY database, so **a fresh initialisation genuinely
    /// applies a step**: `Session::migrate` on an unmigrated store reports `from: 0`, `to: CURRENT`,
    /// `applied: true`, and no caller may assume a fresh database applies nothing. What
    /// `Session::migrate` SEES is not what the `unblock migrate` COMMAND reports (D46 clause (10)):
    /// on a workspace the facade already migrated the engine still returns `from == to`,
    /// `applied: false`, while the command renders the PRE-OPEN delta the facade recorded on
    /// `WorkspaceContext::schema_version_before_migrate`.
    ///
    /// **(vii) THE OBLIGATION THIS CONTRACT DISCHARGES is NFR-19 (PRD §6):** a released binary MUST
    /// open a database written by any earlier released binary, by migrating it forward. Before D46
    /// nothing in the tree stated that, which is the root cause of the defect CLASS rather than of
    /// this instance — a numbered requirement is what a test suite and a gate can cite.
    ///
    /// Idempotent: re-running on an up-to-date database applies no DDL (but still witnesses (v)).
    async fn migrate(&self) -> Result<(), StorageError>;

    /// Run `PRAGMA integrity_check`, returning the raw problem rows.
    ///
    /// A healthy database returns an empty `Vec` (the `"ok"` sentinel is normalized away). Any
    /// returned strings are integrity problems to surface to the operator.
    async fn integrity_check(&self) -> Result<Vec<String>, StorageError>;

    /// Read the current on-disk schema version (`PRAGMA user_version`) — D27/AF-2 (T3.1, spine §3.2).
    ///
    /// A fresh, never-migrated database reports `0`; a migrated database reports
    /// [`CURRENT_SCHEMA_VERSION`](crate::CURRENT_SCHEMA_VERSION). **Since D46 that is NOT the same as
    /// "the version `SCHEMA_SQL` creates":** the DDL is frozen at the BASELINE and a freshly created
    /// database reaches CURRENT by running the ladder, so `0 -> CURRENT` is a real migration even on a
    /// brand-new file (see the [`Storage::migrate`] contract, clause (0)). This is a **pure read** (no
    /// write permit, no migration side-effect) so the engine can report `migrate`'s from→to delta
    /// without re-opening the
    /// database. Backend-agnostic `i64`: the on-disk value is a `PRAGMA` integer (i64 domain); the
    /// libsql impl widens its internal `i32` reader via `i64::from(..)` so no backend width leaks into
    /// the contract. Backs the engine `Session::migrate() -> MigrateOutcome` (spine §4.1).
    async fn schema_version(&self) -> Result<i64, StorageError>;

    /// Acquire the cross-process advisory write lock (`.unblock/.write.lock`) EXCLUSIVE for the WHOLE
    /// mutation — the restored beads serializer (D31, a D14 amendment).
    ///
    /// The engine (L5) calls this under the write permit and holds the returned guard across the
    /// `next_child_number` allocation READ **and** the write transaction (the same span as the permit),
    /// so two MCP-server processes on one workspace cannot mint the same `parent.N` or interleave writes
    /// across processes. It composes BELOW the in-process `Semaphore` (L5) and ABOVE the write-conn
    /// `Mutex` + `BEGIN IMMEDIATE`. Bounded + non-spinning (NFR-3): a native `try_lock` fast-path then
    /// an async sleep-poll to the store's `write_lock_timeout_ms`; a timeout surfaces the retryable
    /// [`StorageError::DatabaseLocked`] (no new `ErrorCode`). Returns a [`WriteLockGuard`] on the
    /// file-backed path; `Ok(None)` on the in-memory path (no cross-process sharing, so no lock). Reads
    /// take NO lock (WAL MVCC). Distinct from the vestigial `.unblock.lock` `OrphanedLockFile` target
    /// (unblock-health is unchanged by D31).
    async fn acquire_write_lock(&self) -> Result<Option<WriteLockGuard>, StorageError>;

    // ---------------------------------------------------------------------------------------------
    // issue CRUD (mutations carry the actor + optional Tier-1 attribution; write Event(s) in-tx)
    // ---------------------------------------------------------------------------------------------

    /// Create an issue **and its seeded relations**, returning its allocated id.
    ///
    /// Validates via the model `IssueValidator`, guards against id/`external_ref` collisions, inserts
    /// the row, and writes an `Event(Created)` in the same transaction. `actor` is the attributed
    /// author. There is **NO** content-hash dedup here — the hash is computed and stored (a dedup
    /// cache column) but never short-circuits an insert; FR-26 import idempotency lives in
    /// `unblock-sync` (get-then-skip via `sync_equals`), not in storage. (Matches `crud.rs`.)
    ///
    /// **It persists the SEEDED relations in that SAME transaction** — the labels, the comments and
    /// `Issue.dependencies`, each with its per-relation event. Saying only "inserts the row" is how
    /// the D44 defect was mis-sized, so the dependency contract is spelled out here: the edge INSERT
    /// **binds `issue.id` as the source column** and reads only
    /// `depends_on_id`/`dep_type`/`created_at`/`created_by`/`metadata`/`thread_id` from each element —
    /// a `Dependency.issue_id` carried on the object is IGNORED, never written, and can never reach
    /// another issue's graph. So the row and every declared edge commit as ONE indivisible act.
    ///
    /// **Create-specific guards (D44):** a `depends_on_id` repeated within the declared list is
    /// rejected with [`StorageError::DuplicateDependency`], and a declared gating edge that closes a
    /// cycle with [`StorageError::CycleDetected`] carrying the real ordered path. Any rejection —
    /// including [`StorageError::SelfDependency`] — rolls the whole transaction back to ZERO rows: no
    /// issue, no edges, no events. [`Storage::create_issues`] deliberately carries NEITHER guard.
    ///
    /// **Dependency-TARGET existence (D45)** is a guard of the SHARED per-record body, so unlike the
    /// two above it DOES reach [`Storage::create_issues`]: a `depends_on_id` that names no row and is
    /// not an `external:` target is rejected with [`StorageError::BlockerNotFound`] (mapped onto the
    /// existing `ErrorCode::IssueNotFound`). The full published precedence on this path is
    /// `IdCollision` → `external_ref` collision → `SelfDependency` → `BlockerNotFound` →
    /// `DuplicateDependency` → `CycleDetected`; the rank is FORCED by that placement, since the two
    /// D44 guards run in the wrapper AROUND the shared body.
    async fn create_issue(&self, issue: &Issue, actor: &str) -> Result<String, StorageError>;

    /// Create the WHOLE slice in **exactly ONE** `BEGIN IMMEDIATE` transaction (D22/T2.3, spine
    /// §3.2.1 — the ATOMIC bulk INSERT primitive).
    ///
    /// Inserts every `Issue` — its row + its `Event(Created)` + per-relation events + the seeded
    /// dependency edges + any `child_counters` bump — committed ONCE. It does **no minting and no
    /// validation** (the engine `Session::create_bulk` mints every id + runs the full
    /// `IssueValidator::validate` BEFORE calling this — storage receives fully-formed `Issue`s).
    ///
    /// **All-or-nothing:** ANY failure on ANY record (id/`external_ref` collision, FK/CHECK
    /// violation, backend error) rolls back the entire transaction — **ZERO rows persist** (never a
    /// partial batch). A dependency edge pointing at a sibling minted earlier in the SAME batch
    /// resolves because both rows live in the one uncommitted tx. It is **NEVER** a loop of
    /// `create_issue` (that would be N independent transactions = a partial-commit hole).
    ///
    /// **It shares the per-record body with [`Storage::create_issue`] but NOT that method's
    /// create-specific guards (D44):** a repeated `depends_on_id` keeps being deduped-and-skipped
    /// here and there is deliberately NO cycle check, because this body is also the JSONL/`bd` IMPORT
    /// body — a guard here could make an already-exported D5 record un-importable.
    ///
    /// **AMENDED by D45:** the two D44 guards above still do not reach this method, but the shared
    /// body now carries ONE that does — the dependency-TARGET existence check. A `depends_on_id` that
    /// names no row, does not belong to any record of THIS slice, and is not an `external:` target is
    /// rejected with [`StorageError::BlockerNotFound`], rolling the WHOLE batch back. The slice's own
    /// id set is the third arm of that predicate, so a record may name a sibling appearing LATER in
    /// the same slice (a forward reference) exactly as it may name an earlier one — record order
    /// never decides acceptance. The un-importability hazard is not waved away: D45 removes its CAUSE
    /// by closing the `unblock-sync` export corpus under its blockers, so any file the exporter
    /// produces from a workspace with no dangling edge satisfies this guard.
    async fn create_issues(&self, issues: &[Issue], actor: &str) -> Result<(), StorageError>;

    /// Fetch a single issue by id, hydrated with its labels and dependencies.
    ///
    /// Returns `Ok(None)` when no issue matches (a missing issue is **not** an error here; callers
    /// that require existence map `None` to [`StorageError::IssueNotFound`] themselves).
    async fn get_issue(&self, id: &str) -> Result<Option<Issue>, StorageError>;

    /// Fetch multiple issues by id (hydrated). Unknown ids are simply absent from the result. Ids are
    /// a lookup **set**: a duplicate id yields **at most one** result (no duplicate-preservation
    /// guarantee — the batch-hydration path, T3.5.1).
    async fn get_issues(&self, ids: &[String]) -> Result<Vec<Issue>, StorageError>;

    /// Apply an [`IssuePatch`] to an issue, returning the updated issue.
    ///
    /// Writes **one `Event` per changed field** in the same transaction (so the audit log records
    /// exactly what changed). A **no-op update** (a patch that changes nothing) writes **no
    /// `Event`** and leaves `updated_at` unchanged. A `parent` change is cycle-checked (rejected
    /// with [`StorageError::CycleDetected`] carrying the path) and, since D45, existence-checked: a
    /// parent that names no row and is not an `external:` target is rejected with
    /// [`StorageError::BlockerNotFound`] (the chain on this path is self → `BlockerNotFound` → cycle,
    /// there being no duplicate guard here). An `external:` PARENT stays legal — the carve-out is
    /// per-TARGET, never per-edge-type.
    async fn update_issue(
        &self,
        id: &str,
        patch: &IssuePatch,
        actor: &str,
    ) -> Result<Issue, StorageError>;

    /// Execute (or, for [`DeleteMode::DryRun`](crate::DeleteMode), plan) a delete.
    ///
    /// Returns the **resolved** [`DeletePlan`] (with `cascade_children` populated for every mode).
    /// Semantics by mode:
    /// - **`DryRun`** mutates nothing and returns the plan (the full blast radius).
    /// - **`Tombstone`** sets `status = Tombstone` + the `deleted_*` fields and **preserves
    ///   `original_type`**.
    /// - **`Cascade`** tombstones the targets and their children.
    /// - **`Hard`** permanently deletes the rows.
    ///
    /// Every non-`DryRun` mode writes an `Event(Deleted)` per affected issue in the same
    /// transaction.
    async fn delete_issue(
        &self,
        plan: &DeletePlan,
        actor: &str,
    ) -> Result<DeletePlan, StorageError>;

    /// Restore (un-tombstone) a SOFT-deleted issue — the audited live inverse of `delete_issue`'s
    /// soft tombstone (FR-1c "recoverable", D20). Single-target only (scalar; no cascade — see spine
    /// §3.2.1).
    ///
    /// Semantics (spine §3.2.1, one `BEGIN IMMEDIATE` tx, TOCTOU-safe — the row is loaded inside the
    /// tx):
    /// - **Missing / hard-deleted id** → [`StorageError::IssueNotFound`] (restore is bounded to soft
    ///   deletes; no new `ErrorCode` is minted).
    /// - **Not a tombstone** (already active) → **idempotent no-op `Ok(issue)`**: no event, no
    ///   `updated_at` bump (mirrors `delete_issue`'s already-tombstone no-op and `claim_issue`'s
    ///   same-actor short-circuit — retry-safe).
    /// - **Real tombstone** → one `UPDATE` delegating to the model `Issue::restore_from_tombstone`
    ///   (best-effort `status` via `closed_at`; `issue_type` untouched; `original_type` and the
    ///   tombstone fields cleared; `closed_at` kept on the Closed branch / cleared on Open), bumps
    ///   `updated_at` + recomputes `content_hash`, and writes a single transactional
    ///   `Event(Restored)` — **never** `StatusChanged`/`Reopened` (the §3.2.1 carve-out).
    ///
    /// Returns the hydrated restored [`Issue`].
    async fn restore_issue(&self, id: &str, actor: &str) -> Result<Issue, StorageError>;

    // ---------------------------------------------------------------------------------------------
    // atomic claim (FR-2)
    // ---------------------------------------------------------------------------------------------

    /// Atomically claim an issue for `assignee` (sets assignee + `in_progress`), with no race
    /// window (FR-2).
    ///
    /// The claim is a single conditional `UPDATE` so concurrent claimers cannot both win. There are
    /// exactly **three** outcomes:
    /// - **Unassigned** → succeeds: sets `assignee` + `status = in_progress` and writes a
    ///   transactional `Event`.
    /// - **Held by a *different* actor** → fails with [`StorageError::AlreadyClaimed`] whose `by`
    ///   field is the current holder, **re-read within the same transaction** (so the loser learns
    ///   who won).
    /// - **Re-claimed by the *same* assignee** → **idempotent `Ok`** (NOT an error): re-claiming
    ///   what you already hold returns the issue unchanged.
    async fn claim_issue(
        &self,
        id: &str,
        assignee: &str,
        actor: &str,
    ) -> Result<Issue, StorageError>;

    // ---------------------------------------------------------------------------------------------
    // defer / undefer (FR-3)
    // ---------------------------------------------------------------------------------------------

    /// Defer an issue until `until` (sets `defer_until`), writing a transactional `Event`.
    ///
    /// A deferred issue is excluded from [`ready_issues`](Storage::ready_issues) until `until`
    /// passes (or it is undeferred).
    async fn defer_issue(
        &self,
        id: &str,
        until: DateTime<Utc>,
        actor: &str,
    ) -> Result<Issue, StorageError>;

    /// Undefer an issue (clears `defer_until`), writing a transactional `Event`. The issue becomes
    /// ready-eligible again immediately (subject to its gating dependencies).
    async fn undefer_issue(&self, id: &str, actor: &str) -> Result<Issue, StorageError>;

    // ---------------------------------------------------------------------------------------------
    // queries (FR-4)
    // ---------------------------------------------------------------------------------------------

    /// List issues matching `filters` (status/type OR within, labels AND/OR, priority range, text
    /// LIKE, include-deferred/closed, limit/offset).
    async fn list_issues(&self, filters: &ListFilters) -> Result<Vec<Issue>, StorageError>;

    /// Return the **ready** candidate set: open, undeferred, and not blocked by any unresolved
    /// gating dependency.
    ///
    /// The set is **default-complete** (unlimited unless `filters.limit` is set) and returned in a
    /// **deterministic order** — `priority` ASC, then `created_at` ASC, then `id` ASC — so output
    /// snapshots are stable (NFR-14). Storage does **not** import policy (CF-11): the engine
    /// re-ranks this candidate set with the hybrid sort; storage only guarantees the stable order.
    async fn ready_issues(&self, filters: &ListFilters) -> Result<Vec<Issue>, StorageError>;

    /// Return the **blocked** set: non-terminal issues (`status NOT IN ('closed','tombstone')`,
    /// deferred-INCLUSIVE) with **at least one unresolved gating edge** (a
    /// `blocks`/`parent-child`/`conditional-blocks`/`waits-for` dependency on a not-yet-closed
    /// issue).
    ///
    /// `filters` **compose** (D18, spine §3.2.1): the same narrowing facets `list_issues` applies
    /// (status-OR, `issue_type`-OR, priority range, `assignee`, `labels_all`/`labels_any`,
    /// `text_contains`) narrow the candidate rows before the live membership test. The baseline is
    /// deferred-inclusive and does NOT inherit `list`'s default visibility, so
    /// `include_closed`/`include_deferred` are **no-ops** here.
    ///
    /// Ready and blocked are **disjoint** but not jointly exhaustive (a closed issue is neither; a
    /// deferred issue is blocked only if it has an unresolved gating edge).
    async fn blocked_issues(&self, filters: &ListFilters) -> Result<Vec<Issue>, StorageError>;

    /// Full-text-ish search over `query` honouring `filters`.
    ///
    /// v1 uses a `LIKE` scan over `title` + `description`, **`ESCAPE`-guarded** so `%`/`_`/the
    /// escape char in `query` are matched literally (no injection, no accidental wildcards). Honours
    /// `filters.limit`; the engine applies the default cap of 50 when no limit is set.
    async fn search_issues(
        &self,
        query: &str,
        filters: &ListFilters,
    ) -> Result<Vec<Issue>, StorageError>;

    /// Count issues matching `filters`, optionally grouped (by status/type/assignee/priority/label).
    ///
    /// With `group_by = None`, returns a single bucket with the total count. For
    /// `Status`/`Type`/`Assignee`/`Priority` the per-group counts **sum to the ungrouped total** over
    /// the same filter (each issue lands in exactly one bucket). **`Label` is the exception:** an
    /// issue is counted **once per label it carries** (the label JOIN), so the `Label` group sum
    /// equals the number of `(issue, label)` pairs among the matching issues — which can be greater
    /// than the total (a multi-label issue) **or** less than it (label-less issues contribute zero).
    /// It is therefore **not** related to the total by a simple `==` or `>=`.
    async fn count_issues(
        &self,
        filters: &ListFilters,
        group_by: Option<CountGroupBy>,
    ) -> Result<Vec<CountBucket>, StorageError>;

    /// Return issues not updated since `older_than` (i.e. `updated_at < older_than`) that match
    /// `filters`.
    async fn stale_issues(
        &self,
        older_than: DateTime<Utc>,
        filters: &ListFilters,
    ) -> Result<Vec<Issue>, StorageError>;

    // ---------------------------------------------------------------------------------------------
    // dependencies (FR-5)
    // ---------------------------------------------------------------------------------------------

    /// Add a dependency edge, writing a transactional `Event(DependencyAdded)`.
    ///
    /// Rejects [`StorageError::SelfDependency`] and [`StorageError::DuplicateDependency`]. Cycle
    /// gating uses **exactly** `DependencyType::affects_ready_work`
    /// (`Blocks` | `ParentChild` | `ConditionalBlocks` | `WaitsFor`); a new edge that would close a
    /// cycle over that gating set is rejected with [`StorageError::CycleDetected`] carrying the
    /// concrete `path`. A non-gating edge (e.g. `Related`) never creates a ready-gating cycle.
    ///
    /// **BOTH endpoints are guarded in-transaction (D45), and the ORDER is published:**
    /// `SelfDependency` → SOURCE existence → `BlockerNotFound` → `DuplicateDependency` →
    /// `CycleDetected`. A source that names no row yields the EXISTING
    /// [`StorageError::IssueNotFound`] (the missing thing genuinely IS the addressed issue); a target
    /// that names no row and is not an `external:` target yields [`StorageError::BlockerNotFound`].
    /// The target probe sits BEFORE the duplicate query so ONE chain describes every write path,
    /// which has one observable consequence: re-adding an ALREADY-PRESENT edge whose target is
    /// dangling now returns `IssueNotFound` where GA returned `DuplicateDependency` (reachable only
    /// on already-corrupt data).
    async fn add_dependency(&self, dep: &Dependency, actor: &str) -> Result<(), StorageError>;

    /// Remove a dependency edge, writing a transactional `Event(DependencyRemoved)`.
    async fn remove_dependency(
        &self,
        issue_id: &str,
        depends_on_id: &str,
        dep_type: &DependencyType,
        actor: &str,
    ) -> Result<(), StorageError>;

    /// List the dependencies declared *by* `id`.
    async fn list_dependencies(&self, id: &str) -> Result<Vec<Dependency>, StorageError>;

    // ---------------------------------------------------------------------------------------------
    // comments (FR-6, D37 — the analog of add_dependency/list_dependencies; spine §3.2/§3.2.1).
    // Every mutation runs inside ONE `with_immediate_tx` so the row and its `Event` commit together
    // (FR-9), and bumps `issues.updated_at` (FORK-S1 — feeds `stale`; NOT hashed).
    // ---------------------------------------------------------------------------------------------

    /// Add a comment, writing a transactional `Event(Commented)`.
    ///
    /// **Existence guard (FORK-3):** the target issue MUST exist — a non-existent or tombstoned id
    /// yields [`StorageError::IssueNotFound`]. A CLOSED issue is ALLOWED (post-mortem commentary).
    ///
    /// The created comment's `updated_at` stays NULL: this path is create-time only (**MUST-1** —
    /// only [`update_comment`](Storage::update_comment) ever sets `updated_at`). MUST-1 is scoped to
    /// THIS method; the create/bulk/import seed path replays caller-supplied `Comment` values
    /// verbatim and persists both `updated_at` and `redacted_at` (spine §3.2.1 MUST-1 SCOPE).
    ///
    /// `author` is threaded separately from `actor` for bd parity: the import/seed path carries the
    /// comment's own author, while the engine passes `author = self.actor` (FORK-M1b).
    async fn add_comment(
        &self,
        issue_id: &str,
        author: &str,
        body: &str,
        actor: &str,
    ) -> Result<Comment, StorageError>;

    /// List the comments on `issue_id` in canonical order (`created_at ASC, id ASC`).
    async fn list_comments(&self, issue_id: &str) -> Result<Vec<Comment>, StorageError>;

    /// Update a comment's body, **preserving provenance** (D-D).
    ///
    /// Guards that the comment row exists → else [`StorageError::CommentNotFound`] (which maps to
    /// the EXISTING `ErrorCode::IssueNotFound` at L7 — FORK-E1: the code is reused, the taxonomy
    /// does not grow). Sets `updated_at = now` and writes `Event(CommentEdited)` carrying the old
    /// and new bodies. In-place replacement WITHOUT provenance is forbidden: the `updated_at` bump
    /// and the event ARE the provenance.
    async fn update_comment(
        &self,
        comment_id: i64,
        body: &str,
        actor: &str,
    ) -> Result<Comment, StorageError>;

    /// **Soft-redact** a comment (D-E) — the single deletion op; never a hard delete.
    ///
    /// Guards that the comment row exists → else [`StorageError::CommentNotFound`]. KEEPS the row,
    /// masks `text` to `""`, sets `redacted_at = now`, and writes `Event(CommentRedacted)` RETAINING
    /// the original body for provenance (FORK-redact-wire). Idempotent: an already-redacted comment
    /// is returned unchanged with no new event (mirroring `restore_issue`'s already-active no-op).
    async fn delete_comment(&self, comment_id: i64, actor: &str) -> Result<Comment, StorageError>;

    // ---------------------------------------------------------------------------------------------
    // hierarchical-id child allocation (FR-1a, D21) — the READ-half the engine allocator consumes
    // ---------------------------------------------------------------------------------------------

    /// Return the next free child number for `parent.N` minting (the `child_counters` high-water
    /// mark + 1, falling back to a `LIKE`-escaped scan of existing `{parent}.N` ids).
    ///
    /// This is the PRODUCTION read-half (T1.8) the engine id-allocator consumes (spine §3.2, D21):
    /// the engine reads it under the SAME write permit as the in-tx counter bump performed by
    /// [`create_issue`](Storage::create_issue), so two concurrent creates under one parent cannot
    /// mint the same `parent.N`. It is **distinct** from the testkit-only `testkit_child_high_water`
    /// seam (which exposes the raw high-water mark for tests). Never panics: overflow saturates.
    async fn next_child_number(&self, parent_id: &str) -> Result<u32, StorageError>;

    /// Return the dependency subtree **rooted at `id`** as a [`DepTree`].
    async fn dependency_tree(&self, id: &str) -> Result<DepTree, StorageError>;

    /// Return the dependency graph for a **root set** as a [`DepTree`].
    ///
    /// Backs the `dep graph` action. An **empty `roots`** slice means the **whole graph** (every
    /// edge); a non-empty `roots` returns the union of the subgraphs reachable from those roots.
    async fn dependency_graph(&self, roots: &[String]) -> Result<DepTree, StorageError>;

    /// Detect every dependency cycle, returning each as an **ordered traversal witness**: a
    /// multi-node cycle is `[start, …, start]` (the start repeated at the end), a self-loop is
    /// `[node, node]`; an acyclic graph returns `[]`. The outer `Vec` is deterministically ordered
    /// (NFR-14). NOT a sorted SCC node set (spine §3.2.1, D3).
    ///
    /// `blocking_only=true` restricts the cycle graph to the 4 gating types
    /// (`DependencyType::affects_ready_work`) — the ready-work view; `=false` considers **all**
    /// dependency types — the integrity/lint view. `parent-child` is inserted reversed regardless
    /// (D4/D19). The trait takes a bare `bool`; the default-TRUE (gating-only) is a wire-only
    /// contract on the MCP `Cycles` input (spine §5.2).
    async fn detect_cycles(&self, blocking_only: bool) -> Result<Vec<Vec<String>>, StorageError>;

    // ---------------------------------------------------------------------------------------------
    // events (audit; append-only)
    // ---------------------------------------------------------------------------------------------

    /// List the append-only audit events for `issue_id`, oldest first.
    async fn list_events(&self, issue_id: &str) -> Result<Vec<Event>, StorageError>;

    // ---------------------------------------------------------------------------------------------
    // diagnostics support (FR-15, pure-DB; no git, no network — NFR-6)
    // ---------------------------------------------------------------------------------------------

    /// Per-epic `parent-child` child rollup — the ONE additive `stats` primitive (D26/T2.7,
    /// spine §3.2): each entry is `(epic_id, (child_total, child_closed_or_tombstone))`, the outer
    /// `Vec` **sorted by epic id in SQL** (`ORDER BY`, deterministic — NFR-14; bd's `get_epic_counts`
    /// returns a non-deterministic `HashMap`, `sqlite.rs:6978`, which unblock does NOT copy).
    ///
    /// bd's `get_epic_counts` ported 1:1: over every `type = 'parent-child'` edge (stored as
    /// `epic = depends_on_id`, `child = issue_id`) whose CHILD is non-template, count the child total
    /// and the children whose `status IN ('closed','tombstone')`, grouped by the epic id. The
    /// engine applies the epic-side active + non-template filter (`issue_type == Epic ∧ ¬terminal ∧
    /// ¬template`) IN-MEMORY — both filters live at their respective sites and are NOT conflated.
    ///
    /// Pure-DB; **never** shells to git (NFR-6). An empty store returns `Ok(Vec::new())`.
    async fn epic_child_rollup(&self) -> Result<Vec<(String, (usize, usize))>, StorageError>;

    /// Return issues closed since `since` (or all closed issues when `since` is `None`), by
    /// `closed_at` — the changelog source. Pure-DB; **never** shells to git (NFR-6).
    async fn closed_since(&self, since: Option<DateTime<Utc>>) -> Result<Vec<Issue>, StorageError>;

    /// Return orphan candidates: issues whose `external_ref` matches the commit-hash pattern.
    ///
    /// The pattern match runs in SQL/Rust — it **never** invokes git or the network (NFR-6); the
    /// caller (health/diagnostics) decides what to do with the candidates.
    async fn orphan_candidates(&self) -> Result<Vec<Issue>, StorageError>;

    /// Every stored dependency edge whose TARGET denotes **nothing** — the D45 `dangling` diagnostic's
    /// ONE read (spine §3.2 / §3.2.1 `dangling`, as AMENDED 2026-08-02).
    ///
    /// The returned [`GraphEdge`]s ARE the finding set: `from` = the dependent issue id (the row
    /// carrying the broken edge), `to` = the phantom target, `dep_type` = the edge type. The caller
    /// maps them to findings and does nothing else — no second read, no id set, no in-memory
    /// difference, and **no re-sort**.
    ///
    /// # Why this is a trait method at all (it was specified NOT to be)
    ///
    /// D45 originally composed this view in the engine from `dependency_graph(&[])` differenced
    /// against a fully-inclusive `list_issues` id set, precisely to avoid growing the trait. That
    /// reasoning was sound and unmeasured: at 250 000 rows the composition cost **10.72 s** — two full
    /// scans plus `O(rows)` peak memory, hydrating every row's labels, dependencies and comments
    /// merely to derive an id SET — and took `Session::doctor()` to **16.31 s** against the 15 s
    /// boundedness guard in `crates/unblock-engine/tests/scale.rs`, i.e. a RED required job. One query
    /// replaces it.
    ///
    /// # Contract (NORMATIVE)
    ///
    /// - **Selection is EXISTENCE ALONE.** An edge is returned iff no `issues` row carries its
    ///   `depends_on_id`. **Never** filter on the target's STATUS: a closed, deferred or tombstoned
    ///   blocker row EXISTS, so its edge is NOT dangling. This is the D45 trap in its SQL form — the
    ///   retired wording said "the id set MUST come from FULLY-INCLUSIVE filters, or every CLOSED
    ///   blocker is reported as dangling"; a status-aware join is the same self-fabricating diagnostic
    ///   through a new door.
    /// - **`external:` targets are EXCLUDED**, with the same ASCII-case-INSENSITIVE semantics as
    ///   [`unblock_model::is_external_target`] (spine §1.9 invariant 3 — the SQL twin is
    ///   `NOT LIKE 'external:%'`, and the two halves are kept honest by the NFR-16 contract suite's
    ///   equivalence cell). An external target names a ticket in another system: a legitimate blocker
    ///   no row could ever satisfy, never a finding.
    /// - **The corpus is EVERY row in the store**, deliberately WIDER than the export corpus — so an
    ///   edge into an ephemeral / `-wisp-` row is NOT dangling, because the row exists.
    /// - **ORDER is PINNED and produced by the implementation**, `(issue_id, dep_type, depends_on_id)`
    ///   ascending — snapshot-pinned output (NFR-14). The triple is a total order: the `dependencies`
    ///   primary key is `(issue_id, depends_on_id)`, so no two rows share all three components.
    ///
    /// Pure-DB; **never** shells to git (NFR-6). A store with no broken edge returns `Ok(Vec::new())`.
    async fn dangling_dependencies(&self) -> Result<Vec<GraphEdge>, StorageError>;

    // ---------------------------------------------------------------------------------------------
    // [v1.1] reserved seams (CF-E; spine §3.2) — additive, depended on by config db-layer +
    //         health full-taxonomy. Kept COMMENTED (not live default methods) so the seam is
    //         reserved without v1 behaviour and this file does not `use` the diagnostics DTOs.
    // ---------------------------------------------------------------------------------------------
    //
    // [v1.1] async fn read_config(&self) -> Result<Vec<(String, String)>, StorageError>;
    // [v1.1] async fn diagnostic_probe(&self, kind: DiagnosticKind) -> Result<DiagnosticReport, StorageError>;
    // [v1.1] async fn diagnostic_probes(&self) -> Result<Vec<DiagnosticReport>, StorageError>;
}

/// Object-safety guard: this signature only compiles if [`Storage`] is object-safe (i.e. usable as
/// `dyn Storage`). It is never called.
#[cfg(test)]
fn _assert_object_safe(_: &dyn Storage) {}

#[cfg(test)]
mod tests {
    use super::Storage;
    use crate::WriteLockGuard;
    use crate::error::StorageError;
    use crate::filters::{DeletePlan, IssuePatch};
    use async_trait::async_trait;
    use chrono::{DateTime, Utc};
    use std::sync::Arc;
    use unblock_model::{
        Comment, CountBucket, CountGroupBy, DepTree, Dependency, DependencyType, Event, GraphEdge,
        Issue, ListFilters,
    };

    /// A backend-free [`Storage`] used only to prove the trait is implementable and object-safe.
    ///
    /// Every method returns an explicitly-constructed value or `Err(StorageError::NotInitialized)`
    /// — never `Default::default()` on a type that has none (`DeletePlan`/`Issue`/`DepTree`).
    struct NoopStorage;

    #[async_trait]
    impl Storage for NoopStorage {
        async fn migrate(&self) -> Result<(), StorageError> {
            Err(StorageError::NotInitialized)
        }

        async fn integrity_check(&self) -> Result<Vec<String>, StorageError> {
            Ok(Vec::new())
        }

        async fn schema_version(&self) -> Result<i64, StorageError> {
            // An un-bootstrapped stub is honestly unstamped: `PRAGMA user_version` on a fresh DB is 0
            // (this stub's `migrate` returns `NotInitialized`, so it never advances the version).
            Ok(0)
        }

        async fn acquire_write_lock(&self) -> Result<Option<WriteLockGuard>, StorageError> {
            // A backend-free stub has no workspace file — no cross-process lock to take.
            Ok(None)
        }

        async fn create_issue(&self, _issue: &Issue, _actor: &str) -> Result<String, StorageError> {
            Err(StorageError::NotInitialized)
        }

        async fn create_issues(&self, _issues: &[Issue], _actor: &str) -> Result<(), StorageError> {
            Err(StorageError::NotInitialized)
        }

        async fn get_issue(&self, _id: &str) -> Result<Option<Issue>, StorageError> {
            Ok(None)
        }

        async fn get_issues(&self, _ids: &[String]) -> Result<Vec<Issue>, StorageError> {
            Ok(Vec::new())
        }

        async fn update_issue(
            &self,
            _id: &str,
            _patch: &IssuePatch,
            _actor: &str,
        ) -> Result<Issue, StorageError> {
            Err(StorageError::NotInitialized)
        }

        async fn delete_issue(
            &self,
            plan: &DeletePlan,
            _actor: &str,
        ) -> Result<DeletePlan, StorageError> {
            Ok(plan.clone())
        }

        async fn restore_issue(&self, _id: &str, _actor: &str) -> Result<Issue, StorageError> {
            // Like the other `Issue`-returning methods: a backend-free stub has no row to restore.
            Err(StorageError::NotInitialized)
        }

        async fn claim_issue(
            &self,
            _id: &str,
            _assignee: &str,
            _actor: &str,
        ) -> Result<Issue, StorageError> {
            Err(StorageError::NotInitialized)
        }

        async fn defer_issue(
            &self,
            _id: &str,
            _until: DateTime<Utc>,
            _actor: &str,
        ) -> Result<Issue, StorageError> {
            Err(StorageError::NotInitialized)
        }

        async fn undefer_issue(&self, _id: &str, _actor: &str) -> Result<Issue, StorageError> {
            Err(StorageError::NotInitialized)
        }

        async fn list_issues(&self, _filters: &ListFilters) -> Result<Vec<Issue>, StorageError> {
            Ok(Vec::new())
        }

        async fn ready_issues(&self, _filters: &ListFilters) -> Result<Vec<Issue>, StorageError> {
            Ok(Vec::new())
        }

        async fn blocked_issues(&self, _filters: &ListFilters) -> Result<Vec<Issue>, StorageError> {
            Ok(Vec::new())
        }

        async fn search_issues(
            &self,
            _query: &str,
            _filters: &ListFilters,
        ) -> Result<Vec<Issue>, StorageError> {
            Ok(Vec::new())
        }

        async fn count_issues(
            &self,
            _filters: &ListFilters,
            _group_by: Option<CountGroupBy>,
        ) -> Result<Vec<CountBucket>, StorageError> {
            Ok(Vec::new())
        }

        async fn stale_issues(
            &self,
            _older_than: DateTime<Utc>,
            _filters: &ListFilters,
        ) -> Result<Vec<Issue>, StorageError> {
            Ok(Vec::new())
        }

        async fn add_dependency(
            &self,
            _dep: &Dependency,
            _actor: &str,
        ) -> Result<(), StorageError> {
            Err(StorageError::NotInitialized)
        }

        async fn remove_dependency(
            &self,
            _issue_id: &str,
            _depends_on_id: &str,
            _dep_type: &DependencyType,
            _actor: &str,
        ) -> Result<(), StorageError> {
            Err(StorageError::NotInitialized)
        }

        async fn list_dependencies(&self, _id: &str) -> Result<Vec<Dependency>, StorageError> {
            Ok(Vec::new())
        }

        // --- comments (FR-6, D37) — the NoopStorage posture: reads are empty, mutations are
        // NotInitialized (exactly as it treats every other read/mutation pair).
        async fn add_comment(
            &self,
            _issue_id: &str,
            _author: &str,
            _body: &str,
            _actor: &str,
        ) -> Result<Comment, StorageError> {
            Err(StorageError::NotInitialized)
        }

        async fn list_comments(&self, _issue_id: &str) -> Result<Vec<Comment>, StorageError> {
            Ok(Vec::new())
        }

        async fn update_comment(
            &self,
            _comment_id: i64,
            _body: &str,
            _actor: &str,
        ) -> Result<Comment, StorageError> {
            Err(StorageError::NotInitialized)
        }

        async fn delete_comment(
            &self,
            _comment_id: i64,
            _actor: &str,
        ) -> Result<Comment, StorageError> {
            Err(StorageError::NotInitialized)
        }

        async fn next_child_number(&self, _parent_id: &str) -> Result<u32, StorageError> {
            // A backend-free stub has no child counters; the first child number is 1.
            Ok(1)
        }

        async fn dependency_tree(&self, id: &str) -> Result<DepTree, StorageError> {
            Ok(DepTree {
                root: id.to_string(),
                edges: Vec::new(),
            })
        }

        async fn dependency_graph(&self, roots: &[String]) -> Result<DepTree, StorageError> {
            Ok(DepTree {
                root: roots.first().cloned().unwrap_or_default(),
                edges: Vec::new(),
            })
        }

        async fn detect_cycles(
            &self,
            _blocking_only: bool,
        ) -> Result<Vec<Vec<String>>, StorageError> {
            Ok(Vec::new())
        }

        async fn list_events(&self, _issue_id: &str) -> Result<Vec<Event>, StorageError> {
            Ok(Vec::new())
        }

        async fn epic_child_rollup(&self) -> Result<Vec<(String, (usize, usize))>, StorageError> {
            Ok(Vec::new())
        }

        async fn closed_since(
            &self,
            _since: Option<DateTime<Utc>>,
        ) -> Result<Vec<Issue>, StorageError> {
            Ok(Vec::new())
        }

        async fn orphan_candidates(&self) -> Result<Vec<Issue>, StorageError> {
            Ok(Vec::new())
        }

        // An empty store HAS no edges, so `Ok(Vec::new())` is the HONEST answer here — not a stub
        // that happens to look clean (D45, spine §3.2 `dangling_dependencies`).
        async fn dangling_dependencies(&self) -> Result<Vec<GraphEdge>, StorageError> {
            Ok(Vec::new())
        }
    }

    /// Drive the `Arc<dyn Storage>` coercion: this only compiles/runs if `Storage` is object-safe
    /// and every method is implementable through a trait object.
    #[tokio::test]
    async fn arc_dyn_storage_coercion() {
        let storage: Arc<dyn Storage> = Arc::new(NoopStorage);

        // A read path returns an explicitly-constructed value.
        assert!(storage.integrity_check().await.expect("ok").is_empty());
        assert!(storage.get_issue("ub-1").await.expect("ok").is_none());

        // The DryRun plan round-trips through the trait object unchanged.
        let plan = DeletePlan {
            mode: crate::DeleteMode::DryRun,
            targets: vec!["ub-1".to_string()],
            cascade_children: Vec::new(),
        };
        let returned = storage.delete_issue(&plan, "tester").await.expect("ok");
        assert_eq!(returned.targets, plan.targets);

        // dependency_graph([]) over the whole graph is reachable through the trait object.
        let tree = storage.dependency_graph(&[]).await.expect("ok");
        assert!(tree.edges.is_empty());

        // An error path maps to the typed error (not a panic).
        assert!(matches!(
            storage.migrate().await,
            Err(StorageError::NotInitialized)
        ));
    }
}
