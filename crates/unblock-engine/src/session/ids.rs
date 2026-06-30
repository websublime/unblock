//! The engine **id allocator** (D21/T1.8) — the **stateful** half of the id scheme.
//!
//! The pure candidate compute (seed/hash/adaptive-length/slug-normalize/prefix-normalize/`child_id`)
//! lives in `unblock-model` (`id.rs`); this module drives the **stateful** collision-retry loop over
//! those pure pieces, probing storage for existence. It is called by
//! [`Session::create_issue`](crate::Session::create_issue) **under the write permit**, so the
//! mint→probe→insert is atomic (a candidate that races in still surfaces `IdCollision` from
//! `storage.create_issue`, which the engine retries).
//!
//! This is a faithful re-home of the original `temp/beads_rust-main/src/util/id.rs` STATEFUL
//! `IdGenerator::generate` / `generate_with_slug` caller loop (adaptive length + slug + collision
//! retry + the saturated fallback). The loop is stateful **because it probes storage**; the allocator
//! itself holds no counter — the `parent.N` high-water mark is storage's
//! ([`Storage::next_child_number`](unblock_storage::Storage::next_child_number)).

use chrono::{DateTime, Utc};
use unblock_model::{
    MAX_HASH_LENGTH, child_id, compute_id_hash, generate_id_seed, normalize_slug_for_prefix,
    optimal_hash_length,
};

use crate::error::Result;
use crate::session::Session;

/// The 12-char hash length the original saturated fallback uses once the adaptive ladder maxes out
/// (faithful to `util/id.rs:230`). Bounded well under [`MAX_ID_HASH_LEN`] so the id still parses.
const FALLBACK_HASH_LENGTH: usize = 12;

/// The nonce at which the desperate fallback starts appending the nonce to the hash, and the hard
/// stop (faithful to `util/id.rs:241`/`:249`).
const DESPERATE_NONCE: u32 = 1000;
const HARD_STOP_NONCE: u32 = 2000;

impl Session {
    /// Mint a fresh id for `new` by probing storage for collisions (D21).
    ///
    /// - **child** (`new.parent` is `Some`): `child_id(parent, storage.next_child_number(parent))` —
    ///   the `parent.N` form (the in-tx counter bump happens inside `storage.create_issue`, under the
    ///   same permit, so the counter cannot race).
    /// - **root** (`new.parent` is `None`): the adaptive `<prefix>-<hash>` ladder, or — with
    ///   `new.slug` — the `<prefix>-<slug>-<hash>` form (falling back to hash-only on an empty slug or
    ///   an exhausted prefix budget).
    ///
    /// `created_at` and `creator` feed the hash seed; the prefix is config-derived
    /// (`self.config.id_prefix`, already `normalize_prefix`-normalized by config).
    ///
    /// # Errors
    /// - The transparent storage source if a `get_issue` probe or the `next_child_number` read fails.
    pub(crate) async fn allocate_id(&self, new: &NewIssueSeed<'_>) -> Result<String> {
        // Child id: delegate the number to storage's high-water counter.
        if let Some(parent) = new.parent {
            let n = self.storage.next_child_number(parent).await?;
            return Ok(child_id(parent, n));
        }

        // Root id: the adaptive collision-retry ladder.
        let issue_count = self.issue_count().await?;
        match new.slug {
            Some(slug) if !slug.is_empty() => {
                self.allocate_slug_id(new, slug, issue_count).await
            }
            _ => self.allocate_hash_id(new, issue_count).await,
        }
    }

    /// The number of issues currently in the store — the adaptive-length input (`optimal_hash_length`).
    ///
    /// Uses the existing ungrouped `count_issues` read; the default filter still excludes
    /// closed/tombstone, which is acceptable for sizing the hash (the original counted the live row
    /// set the same way), and the storage `IdCollision` guard is the hard correctness backstop.
    async fn issue_count(&self) -> Result<usize> {
        let buckets = self
            .storage
            .count_issues(&unblock_model::ListFilters::default(), None)
            .await?;
        Ok(buckets.iter().map(|b| b.count).sum())
    }

    /// The hash-only ladder `<prefix>-<hash>` (faithful to `IdGenerator::generate`, `util/id.rs:197`).
    ///
    /// Tries nonces `0..10` at the adaptive length, then grows the length, then the saturated 12-char
    /// fallback with a desperate `…{nonce}` tail — probing each candidate against storage.
    async fn allocate_hash_id(&self, new: &NewIssueSeed<'_>, issue_count: usize) -> Result<String> {
        let mut length = optimal_hash_length(issue_count);
        loop {
            // Try nonces 0..10 at this length.
            for nonce in 0..10 {
                let id = self.hash_candidate(new, nonce, length);
                if !self.exists(&id).await? {
                    return Ok(id);
                }
            }

            // All nonces collided — grow the length, or take the saturated fallback once maxed out.
            if length < MAX_HASH_LENGTH {
                length += 1;
            } else {
                return self.allocate_saturated_fallback(new).await;
            }
        }
    }

    /// The slug ladder `<prefix>-<slug>-<hash>` (faithful to `generate_with_slug`, `util/id.rs:137`).
    ///
    /// The slug is `normalize_slug_for_prefix`'d to fit `<prefix>-<slug>` within the prefix budget;
    /// the hash suffix is always appended. On an exhausted budget (empty drop-signal) — **or** once
    /// the hash length saturates without a free id — it **drops the slug** and falls back to the
    /// hash-only ladder (a single doc names BOTH drop triggers).
    async fn allocate_slug_id(
        &self,
        new: &NewIssueSeed<'_>,
        slug: &str,
        issue_count: usize,
    ) -> Result<String> {
        let normalized = normalize_slug_for_prefix(slug, &self.config.id_prefix);
        if normalized.is_empty() {
            // Budget-exhausted drop trigger #1: the prefix alone leaves no room for any slug.
            return self.allocate_hash_id(new, issue_count).await;
        }

        let mut length = optimal_hash_length(issue_count);
        loop {
            for nonce in 0..10 {
                let hash = compute_id_hash(&self.seed(new, nonce), length);
                let id = format!("{}-{normalized}-{hash}", self.config.id_prefix);
                if !self.exists(&id).await? {
                    return Ok(id);
                }
            }
            if length < MAX_HASH_LENGTH {
                length += 1;
            } else {
                // Drop trigger #2: hash-saturation under the slug. Drop the slug and fall back to the
                // hash-only ladder (preserves uniqueness without producing an oversized prefix).
                return self.allocate_hash_id(new, issue_count).await;
            }
        }
    }

    /// The original's saturated fallback (`util/id.rs:225-252`): a full 12-char hash, then a desperate
    /// `…{nonce}` tail past 1000 collisions, with a hard stop at 2000 to prevent an infinite loop.
    async fn allocate_saturated_fallback(&self, new: &NewIssueSeed<'_>) -> Result<String> {
        let mut nonce = 0u32;
        loop {
            let hash = compute_id_hash(&self.seed(new, nonce), FALLBACK_HASH_LENGTH);
            let id = format!("{}-{hash}", self.config.id_prefix);
            if !self.exists(&id).await? {
                return Ok(id);
            }

            nonce = nonce.saturating_add(1);

            // Past 1000 collisions, append the nonce to force uniqueness.
            if nonce > DESPERATE_NONCE {
                let desperate = format!("{}-{hash}{nonce}", self.config.id_prefix);
                if !self.exists(&desperate).await? {
                    return Ok(desperate);
                }
            }

            // Hard stop at 2000 (a broken existence probe must not spin forever).
            if nonce > HARD_STOP_NONCE {
                return Ok(format!("{}-{hash}{nonce}", self.config.id_prefix));
            }
        }
    }

    /// Build a `<prefix>-<hash>` candidate at `(nonce, length)`.
    fn hash_candidate(&self, new: &NewIssueSeed<'_>, nonce: u32, length: usize) -> String {
        let hash = compute_id_hash(&self.seed(new, nonce), length);
        format!("{}-{hash}", self.config.id_prefix)
    }

    /// The length-prefixed seed for `new` at `nonce`, carrying the resolved actor as `creator` (D21).
    fn seed(&self, new: &NewIssueSeed<'_>, nonce: u32) -> String {
        generate_id_seed(
            new.title,
            new.description,
            Some(&self.actor),
            new.created_at,
            nonce,
        )
    }

    /// Probe storage for an existing id via `get_issue(id).await?.is_some()` (there is no
    /// `Storage::exists` — the existence probe reuses the `get_issue` read).
    async fn exists(&self, id: &str) -> Result<bool> {
        Ok(self.storage.get_issue(id).await?.is_some())
    }
}

/// The borrowed seed inputs the allocator needs from a `NewIssue` (so the loop borrows rather than
/// clones the create inputs).
pub(crate) struct NewIssueSeed<'a> {
    /// The issue title (hashed into the seed).
    pub title: &'a str,
    /// The optional description (hashed into the seed).
    pub description: Option<&'a str>,
    /// `Some(parent)` mints `parent.N`; `None` mints a root id.
    pub parent: Option<&'a str>,
    /// `Some(slug)` mints `<prefix>-<slug>-<hash>` (subject to the budget); `None` mints hash-only.
    pub slug: Option<&'a str>,
    /// The creation timestamp (hashed into the seed).
    pub created_at: DateTime<Utc>,
}
