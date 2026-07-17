//! Embedded canonical DDL (`SCHEMA_SQL`) and the schema-version constant.
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

/// The current on-disk schema version stamped into `PRAGMA user_version`.
///
/// v1 baseline. `MIGRATIONS` (in `migrations.rs`) is empty: v1.0.0 is the first shipped schema, so
/// there is no prior on-disk `user_version` to migrate from in v1 (CLAUDE.md). Any database whose
/// `user_version` is **greater** than this is rejected with [`crate::StorageError::SchemaMismatch`].
pub(crate) const CURRENT_SCHEMA_VERSION: i32 = 1;

/// The complete canonical SQL schema (model-B minimal-v1). Applied wholesale on a fresh database.
///
/// Every statement is `CREATE … IF NOT EXISTS`, so re-applying is a no-op (the migration path stamps
/// `user_version` separately). Statement boundaries are plain `;` at the top level — there are no
/// string literals containing semicolons here, so `execute_batch` runs the whole script.
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

    -- Comments (rows exist v1; surfaced v1.1).
    CREATE TABLE IF NOT EXISTS comments (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        issue_id TEXT NOT NULL,
        author TEXT NOT NULL,
        text TEXT NOT NULL,
        created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
        -- D37 — provenance-preserving edit (D-D) / soft-redact (D-E). Both nullable and part of
        -- the BASELINE schema: CURRENT_SCHEMA_VERSION stays 1 and MIGRATIONS stays empty.
        updated_at DATETIME,
        redacted_at DATETIME,
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

#[cfg(test)]
mod tests {
    use super::CURRENT_SCHEMA_VERSION;

    #[test]
    fn schema_version_is_one() {
        assert_eq!(CURRENT_SCHEMA_VERSION, 1);
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
