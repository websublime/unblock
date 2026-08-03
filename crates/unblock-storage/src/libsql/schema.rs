//! Embedded canonical DDL (`SCHEMA_SQL`) and the schema-version constants.
//!
//! **THE FROZEN-BASELINE DISCIPLINE (D46, v1.0.1 — read this before touching [`SCHEMA_SQL`]).**
//! [`SCHEMA_SQL`] is FROZEN at the shape it had when the migration ladder began — it corresponds to
//! [`BASELINE_SCHEMA_VERSION`], **not** to [`CURRENT_SCHEMA_VERSION`]. Every element added after that
//! baseline exists ONLY as a step in [`super::migrations::MIGRATIONS`]; adding a step does NOT permit
//! editing this DDL — the two are ALTERNATIVES, never a pair. A reader who needs the CURRENT shape
//! replays the ladder over this baseline. A fresh database is created at the baseline, stamped
//! `BASELINE_SCHEMA_VERSION`, and then FALLS THROUGH the ladder like any database found on disk, so
//! there is exactly ONE path to the current shape and every fresh install exercises every step.
//! The [`SCHEMA_CONTENT_DIGEST`] const assertion below turns an edit to this text into a red BUILD.
//!
//! The column order of `issues` is reproduced **verbatim** from the original
//! `temp/beads_rust-main/src/storage/schema.rs` (model-B minimal-v1 trims, crate plan §3.3): 38
//! columns ending `… ephemeral, pinned, is_template, source_repo_path, agent_context`, with the
//! `length(title) <= 500`, `priority 0..=4`, and closed-at-invariant `CHECK`s. The `idx_issues_ready`
//! composite index is copied verbatim (NFR-1, the original `workitems_ready_index` perf lesson).
//!
//! **model-B minimal-v1 trims vs the original (D5, crate plan §3.3):** `source_repo` is nullable
//! (the original's `NOT NULL DEFAULT '.'` is dropped); the `close_metadata`, `gate_results`,
//! `config`, `dirty_issues`, `export_hashes`, and `blocked_issues_cache` tables and the wisp
//! machinery are all dropped (JSONL is a light export in `unblock-sync`; blocked/ready are computed
//! live; `config/gate_results` land at v1.1). `ephemeral`/`pinned`/`is_template` are KEPT (ready
//! gating reads them). `depends_on_id` has **no** foreign key (intentional — external refs).

/// The on-disk schema version this build expects, stamped into `PRAGMA user_version`.
///
/// **`dependencies.metadata` and `dependencies.thread_id` are BASELINE-v1** — present in the
/// original `SCHEMA_SQL` since the first shipped release, so every database ever created by any
/// shipped `unblock` already has them. A future migration must NOT `ALTER TABLE ADD COLUMN` either
/// one: it would hard-error on every existing database. (They were merely never BOUND by the write
/// side until D42; that was a code defect, not a schema gap.)
///
/// **D46 (v1.0.1) — `1` → `2`.** The premise this constant carried until v1.0.1 ("`MIGRATIONS` is
/// empty: v1.0.0 is the first shipped schema, so there is no prior on-disk `user_version` to migrate
/// from") was true of published RELEASES and false of DATABASES IN THE FIELD: D37 added the two
/// `comments` columns by editing the baseline `CREATE TABLE` in place, so a database written before
/// 2026-07-17 is stamped `1` exactly like a GA one and still carries a five-column `comments`. The
/// ladder now carries its first real step and this constant names the version that step reaches;
/// [`BASELINE_SCHEMA_VERSION`] names the version [`SCHEMA_SQL`] itself creates. Any database whose
/// `user_version` is **greater** than this is rejected with [`crate::StorageError::SchemaMismatch`].
pub(crate) const CURRENT_SCHEMA_VERSION: i32 = 2;

/// The version the embedded [`SCHEMA_SQL`] tables correspond to (D46, v1.0.1).
///
/// Introduced so the literal `1` stops carrying two unrelated meanings. Under the frozen-baseline
/// discipline the correspondence is true BY CONSTRUCTION and stays true: the DDL never gains a
/// post-baseline element, so a database created from it is exactly at this version and reaches
/// [`CURRENT_SCHEMA_VERSION`] by running [`super::migrations::MIGRATIONS`] — the same ladder a
/// database found on disk runs. The covered range is `BASELINE_SCHEMA_VERSION + 1 ..=
/// CURRENT_SCHEMA_VERSION`, asserted contiguous and non-empty in `migrations.rs`.
pub(crate) const BASELINE_SCHEMA_VERSION: i32 = 1;

/// The complete canonical SQL schema at [`BASELINE_SCHEMA_VERSION`] (model-B minimal-v1). Applied
/// wholesale on a fresh database, which is then stamped at the BASELINE and run through the ladder.
///
/// **FROZEN (D46, v1.0.1).** This text no longer describes the CURRENT schema and must not be edited
/// to make it do so: post-baseline columns live only in migration steps (see the module docs). The
/// [`SCHEMA_CONTENT_DIGEST`] assertion makes an edit here a compile error — on ANY edit, not only on
/// one that forgot a version bump or a step.
///
/// Every statement is `CREATE … IF NOT EXISTS`, so re-applying is a no-op — and, precisely because
/// of that, **re-applying it can never ADD a column**: re-application is not a repair mechanism, a
/// step is. Statement boundaries are plain `;` at the top level — there are no string literals
/// containing semicolons here, so `execute_batch` runs the whole script.
pub(crate) const SCHEMA_SQL: &str = r"
    -- Issues table.
    -- Column order is FROZEN to match the original bd schema (model-B trims applied): the
    -- PRAGMA table_info(issues) ordinal sequence is golden-pinned (insta) so a fresh and a
    -- migrated DB stay column-compatible.
    -- TEXT body columns use DEFAULT '' (the mapper coalesces '' -> None on load).
    CREATE TABLE IF NOT EXISTS issues (
        id TEXT PRIMARY KEY,
        content_hash TEXT,
        title TEXT NOT NULL CHECK(length(title) <= 500),
        description TEXT NOT NULL DEFAULT '',
        design TEXT NOT NULL DEFAULT '',
        acceptance_criteria TEXT NOT NULL DEFAULT '',
        notes TEXT NOT NULL DEFAULT '',
        status TEXT NOT NULL DEFAULT 'open',
        priority INTEGER NOT NULL DEFAULT 2 CHECK(priority >= 0 AND priority <= 4),
        issue_type TEXT NOT NULL DEFAULT 'task',
        assignee TEXT,
        owner TEXT DEFAULT '',
        estimated_minutes INTEGER,
        created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
        created_by TEXT DEFAULT '',
        updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
        closed_at DATETIME,
        close_reason TEXT DEFAULT '',
        closed_by_session TEXT DEFAULT '',
        due_at DATETIME,
        defer_until DATETIME,
        external_ref TEXT,
        source_system TEXT DEFAULT '',
        source_repo TEXT DEFAULT '',
        deleted_at DATETIME,
        deleted_by TEXT DEFAULT '',
        delete_reason TEXT DEFAULT '',
        original_type TEXT DEFAULT '',
        compaction_level INTEGER DEFAULT 0,
        compacted_at DATETIME,
        compacted_at_commit TEXT,
        original_size INTEGER,
        sender TEXT DEFAULT '',
        ephemeral INTEGER NOT NULL DEFAULT 0,
        pinned INTEGER NOT NULL DEFAULT 0,
        is_template INTEGER NOT NULL DEFAULT 0,
        source_repo_path TEXT,
        agent_context TEXT,
        CHECK (
            (status = 'closed' AND closed_at IS NOT NULL) OR
            (status = 'tombstone') OR
            (status NOT IN ('closed', 'tombstone') AND closed_at IS NULL)
        )
    );

    -- Primary access patterns.
    CREATE INDEX IF NOT EXISTS idx_issues_status ON issues(status);
    CREATE INDEX IF NOT EXISTS idx_issues_priority ON issues(priority);
    CREATE INDEX IF NOT EXISTS idx_issues_issue_type ON issues(issue_type);
    CREATE INDEX IF NOT EXISTS idx_issues_assignee ON issues(assignee) WHERE assignee IS NOT NULL;
    CREATE INDEX IF NOT EXISTS idx_issues_created_at ON issues(created_at);
    CREATE INDEX IF NOT EXISTS idx_issues_updated_at ON issues(updated_at);

    -- Export/sync patterns.
    CREATE INDEX IF NOT EXISTS idx_issues_content_hash ON issues(content_hash);
    CREATE UNIQUE INDEX IF NOT EXISTS idx_issues_external_ref_unique ON issues(external_ref) WHERE external_ref IS NOT NULL;

    -- Special states.
    CREATE INDEX IF NOT EXISTS idx_issues_ephemeral ON issues(ephemeral) WHERE ephemeral = 1;
    CREATE INDEX IF NOT EXISTS idx_issues_pinned ON issues(pinned) WHERE pinned = 1;
    CREATE INDEX IF NOT EXISTS idx_issues_tombstone ON issues(status) WHERE status = 'tombstone';

    -- Time-based.
    CREATE INDEX IF NOT EXISTS idx_issues_due_at ON issues(due_at) WHERE due_at IS NOT NULL;
    CREATE INDEX IF NOT EXISTS idx_issues_defer_until ON issues(defer_until) WHERE defer_until IS NOT NULL;

    -- Ready-work composite index (most important for performance; copied verbatim, NFR-1).
    CREATE INDEX IF NOT EXISTS idx_issues_ready
        ON issues(status, priority, created_at)
        WHERE status = 'open'
        AND ephemeral = 0
        AND pinned = 0
        AND is_template = 0;

    -- Common active-list path: non-terminal issues ordered by priority/created_at.
    CREATE INDEX IF NOT EXISTS idx_issues_list_active_order
        ON issues(priority, created_at)
        WHERE status NOT IN ('closed', 'tombstone')
        AND (is_template = 0 OR is_template IS NULL);

    -- Dependencies. issue_id CASCADE; depends_on_id has NO FK (external refs allowed).
    CREATE TABLE IF NOT EXISTS dependencies (
        issue_id TEXT NOT NULL,
        depends_on_id TEXT NOT NULL,
        type TEXT NOT NULL DEFAULT 'blocks',
        created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
        created_by TEXT NOT NULL DEFAULT '',
        metadata TEXT DEFAULT '{}',
        thread_id TEXT DEFAULT '',
        PRIMARY KEY (issue_id, depends_on_id),
        FOREIGN KEY (issue_id) REFERENCES issues(id) ON DELETE CASCADE
    );
    CREATE INDEX IF NOT EXISTS idx_dependencies_issue ON dependencies(issue_id);
    CREATE INDEX IF NOT EXISTS idx_dependencies_depends_on ON dependencies(depends_on_id);
    CREATE INDEX IF NOT EXISTS idx_dependencies_type ON dependencies(type);
    CREATE INDEX IF NOT EXISTS idx_dependencies_depends_on_type ON dependencies(depends_on_id, type);
    CREATE INDEX IF NOT EXISTS idx_dependencies_blocking
        ON dependencies(depends_on_id, issue_id)
        WHERE (type = 'blocks' OR type = 'parent-child' OR type = 'conditional-blocks' OR type = 'waits-for');

    -- Labels.
    CREATE TABLE IF NOT EXISTS labels (
        issue_id TEXT NOT NULL,
        label TEXT NOT NULL,
        PRIMARY KEY (issue_id, label),
        FOREIGN KEY (issue_id) REFERENCES issues(id) ON DELETE CASCADE
    );
    CREATE INDEX IF NOT EXISTS idx_labels_label ON labels(label);
    CREATE INDEX IF NOT EXISTS idx_labels_issue ON labels(issue_id);

    -- Comments — the BASELINE five-column shape (D46: `updated_at`/`redacted_at` live in step 2).
    CREATE TABLE IF NOT EXISTS comments (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        issue_id TEXT NOT NULL,
        author TEXT NOT NULL,
        text TEXT NOT NULL,
        created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
        FOREIGN KEY (issue_id) REFERENCES issues(id) ON DELETE CASCADE
    );
    CREATE INDEX IF NOT EXISTS idx_comments_issue ON comments(issue_id);
    CREATE INDEX IF NOT EXISTS idx_comments_created_at ON comments(created_at);

    -- Events (append-only audit + Tier-1 attribution, capture-only).
    CREATE TABLE IF NOT EXISTS events (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        issue_id TEXT NOT NULL,
        event_type TEXT NOT NULL,
        actor TEXT NOT NULL DEFAULT '',
        old_value TEXT,
        new_value TEXT,
        comment TEXT,
        created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
        agent_name TEXT,
        harness TEXT,
        model TEXT,
        FOREIGN KEY (issue_id) REFERENCES issues(id) ON DELETE CASCADE
    );
    CREATE INDEX IF NOT EXISTS idx_events_issue ON events(issue_id);
    CREATE INDEX IF NOT EXISTS idx_events_type ON events(event_type);
    CREATE INDEX IF NOT EXISTS idx_events_created_at ON events(created_at);
    CREATE INDEX IF NOT EXISTS idx_events_actor ON events(actor) WHERE actor != '';

    -- Metadata (key/value; application enforces key replacement).
    CREATE TABLE IF NOT EXISTS metadata (
        key TEXT NOT NULL,
        value TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_metadata_key ON metadata(key);

    -- Child counters (hierarchical ids like ub-abc.1, ub-abc.2).
    CREATE TABLE IF NOT EXISTS child_counters (
        parent_id TEXT PRIMARY KEY,
        last_child INTEGER NOT NULL DEFAULT 0,
        FOREIGN KEY (parent_id) REFERENCES issues(id) ON DELETE CASCADE
    );
";

// ---------------------------------------------------------------------------------------------
// D46 clause (6) — THE CLASS GUARD: a const-evaluated CONTENT PIN.
// ---------------------------------------------------------------------------------------------

/// FNV-1a 64-bit offset basis.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a 64-bit prime.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Fold `bytes` into an FNV-1a 64-bit accumulator (`const fn` so the whole digest is const-evaluated).
const fn fold_bytes(mut hash: u64, bytes: &[u8]) -> u64 {
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
        i += 1;
    }
    hash
}

/// Fold an `i32` (little-endian) into the accumulator.
const fn fold_i32(hash: u64, value: i32) -> u64 {
    fold_bytes(hash, &value.to_le_bytes())
}

/// Fold a single byte into the accumulator.
const fn fold_u8(hash: u64, value: u8) -> u64 {
    fold_bytes(hash, &[value])
}

/// The const-evaluated digest over [`SCHEMA_SQL`] + [`CURRENT_SCHEMA_VERSION`] + the ladder.
///
/// **What the LADDER half hashes (D46 clause (6), settled by the decision — not an implementer
/// choice):** each step's VERSION and its step-KIND discriminant, plus the SQL TEXT of any
/// [`MigrationKind::Sql`](super::migrations::MigrationKind::Sql) step. The one-time historical step
/// is UNPARAMETERISED — its `ALTER`s live in a function body — so today the ladder half is version +
/// discriminant only. The clause is written for the FIRST future `Sql` step, whose in-place edit
/// must then redden exactly as a DDL edit does: the "never edit an applied step in place" rule this
/// crate already claimed, finally executable.
///
/// It is deliberately NOT generated by a build script, an `include!`, an `env!` or any codegen
/// reading these same constants — that would make the pin self-satisfying.
const SCHEMA_CONTENT_DIGEST: u64 = {
    let hash = fold_bytes(FNV_OFFSET_BASIS, SCHEMA_SQL.as_bytes());
    let hash = fold_i32(hash, CURRENT_SCHEMA_VERSION);
    let hash = fold_i32(hash, BASELINE_SCHEMA_VERSION);
    let mut hash = hash;
    let steps = super::migrations::MIGRATIONS;
    let mut i = 0;
    while i < steps.len() {
        hash = fold_i32(hash, steps[i].version);
        hash = fold_u8(hash, steps[i].kind.discriminant());
        hash = match steps[i].kind {
            super::migrations::MigrationKind::Sql(sql) => fold_bytes(hash, sql.as_bytes()),
            super::migrations::MigrationKind::CommentsColumnsReconcile => hash,
        };
        i += 1;
    }
    hash
};

/// The HAND-BLESSED digest literal (D46 clause (6)).
///
/// **Re-blessing this is the mechanism, not a side effect.** Any edit to [`SCHEMA_SQL`], to
/// [`CURRENT_SCHEMA_VERSION`]/[`BASELINE_SCHEMA_VERSION`], or to the ladder's shape moves
/// [`SCHEMA_CONTENT_DIGEST`] and turns the BUILD red — including a bump that DID add a step, because
/// a step and a DDL edit are ALTERNATIVES, never a pair. Reaching green after touching the DDL
/// therefore requires deliberately re-blessing this number, which is exactly the moment a reviewer
/// sees the frozen baseline being broken.
///
/// **How to obtain a new value** (a `const` assertion takes a string LITERAL and formats nothing, so
/// it can only ever print "assertion failed", never the computed digest): add a TEMPORARY,
/// UNCOMMITTED `#[test]` in this module that prints [`SCHEMA_CONTENT_DIGEST`] at runtime, read it
/// under `cargo test -- --nocapture`, transcribe it here, then DELETE the test. Leaving any readout
/// in the tree — a build script, an `include!`, an `env!`, or a committed test that writes the
/// literal — would make the pin self-satisfying.
///
/// Blessed ONCE, at the D46 implementation commit, over the POST-revert state of that same commit:
/// the five-column baseline `comments` table, `CURRENT_SCHEMA_VERSION = 2`, and the one-step ladder.
const BLESSED_SCHEMA_CONTENT_DIGEST: u64 = 0xf764_6f13_a7e9_95ba;

const _: () = assert!(
    SCHEMA_CONTENT_DIGEST == BLESSED_SCHEMA_CONTENT_DIGEST,
    "D46 clause (6): SCHEMA_SQL / the schema-version constants / the MIGRATIONS ladder changed \
     without re-blessing BLESSED_SCHEMA_CONTENT_DIGEST. SCHEMA_SQL is FROZEN at the baseline: a new \
     column belongs in a forward step, never in this DDL. If the change is legitimate, read the new \
     digest out with a throwaway #[test] (see BLESSED_SCHEMA_CONTENT_DIGEST's docs) and transcribe it."
);

#[cfg(test)]
mod tests {
    use super::{BASELINE_SCHEMA_VERSION, CURRENT_SCHEMA_VERSION};

    /// The stamp this build writes (D46: `1` → `2`). A by-VALUE pin — moving it is deliberate.
    #[test]
    fn schema_version_is_two() {
        assert_eq!(CURRENT_SCHEMA_VERSION, 2);
    }

    /// The DDL's own version (D46 clause (1)) — the version `SCHEMA_SQL` creates, distinct from the
    /// version this build expects. The non-empty covered range they imply is asserted at COMPILE time
    /// by the ladder-contiguity `const` block in `migrations.rs`, which no annotation can silence.
    #[test]
    fn baseline_is_one() {
        assert_eq!(BASELINE_SCHEMA_VERSION, 1);
    }

    /// **D46 clause (1) — the FROZEN BASELINE, asserted POSITIVELY on the shape rather than by a
    /// spelling sweep.** The embedded `comments` CREATE TABLE has exactly FIVE columns and the two
    /// D37 columns appear NOWHERE in this DDL: they exist only in step 2.
    ///
    /// MUTANT KILLED: re-adding `updated_at DATETIME` / `redacted_at DATETIME` to the baseline
    /// `comments` table (the D37 in-place edit D46 reverts) — which would also hard-error
    /// `duplicate column name` on every fresh install once step 2 ran.
    #[test]
    fn baseline_comments_table_is_the_five_column_shape() {
        let ddl = super::SCHEMA_SQL;
        let start = ddl
            .find("CREATE TABLE IF NOT EXISTS comments")
            .expect("the baseline DDL creates `comments`");
        let body = &ddl[start..];
        let end = body.find(");").expect("the CREATE TABLE is terminated");
        let create = &body[..end];

        for column in ["id", "issue_id", "author", "text", "created_at"] {
            assert!(
                create.contains(column),
                "the baseline `comments` table must keep its column `{column}`"
            );
        }
        for post_baseline in ["updated_at", "redacted_at"] {
            assert!(
                !create.contains(post_baseline),
                "`{post_baseline}` is a POST-baseline column: it belongs in step 2, never in \
                 SCHEMA_SQL (D46 clause (1) — the frozen-baseline discipline)"
            );
        }
    }

    #[test]
    fn schema_sql_mentions_all_v1_tables() {
        for table in [
            "issues",
            "dependencies",
            "labels",
            "comments",
            "events",
            "metadata",
            "child_counters",
        ] {
            assert!(
                super::SCHEMA_SQL.contains(&format!("CREATE TABLE IF NOT EXISTS {table}")),
                "missing table {table}"
            );
        }
        // The dropped tables must NOT be present (model-B trims).
        for dropped in [
            "close_metadata",
            "gate_results",
            "config",
            "dirty_issues",
            "export_hashes",
            "blocked_issues_cache",
        ] {
            assert!(
                !super::SCHEMA_SQL.contains(dropped),
                "dropped table {dropped} leaked into SCHEMA_SQL"
            );
        }
    }
}
