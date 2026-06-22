//! Temp `.unblock/` workspace builder for the storage targets.
//!
//! Each storage core opens a **fresh file-backed** `LibsqlStorage` (real WAL + native `busy_timeout`)
//! under a unique temp directory, so a target exercises the on-disk path the in-memory shared-cache
//! store cannot. The directory is removed when the [`FuzzWorkspace`] drops.
//!
//! The JSONL/`bd` path-confinement sentinel machinery (NFR-7/8) belongs to the `unblock-sync`
//! targets, which are **post-T0.7** — this T0.7 workspace is deliberately storage-only.

use unblock_storage::{LibsqlStorage, StorageError};

/// A throwaway workspace directory holding one libsql DB file.
pub struct FuzzWorkspace {
    /// The temp directory (removed on drop). Kept alive for the lifetime of any opened store.
    _dir: tempfile::TempDir,
    db_path: std::path::PathBuf,
}

impl FuzzWorkspace {
    /// Create a fresh temp workspace with an `.unblock/` directory.
    ///
    /// # Errors
    ///
    /// Returns the underlying I/O error as a string if the temp dir cannot be created.
    pub fn new() -> Result<Self, std::io::Error> {
        let dir = tempfile::tempdir()?;
        let unblock_dir = dir.path().join(".unblock");
        std::fs::create_dir_all(&unblock_dir)?;
        let db_path = unblock_dir.join("unblock.db");
        Ok(Self { _dir: dir, db_path })
    }

    /// Open (and migrate) a file-backed `LibsqlStorage` in this workspace.
    ///
    /// # Errors
    ///
    /// Returns a [`StorageError`] if the database cannot be opened or migrated.
    pub async fn open_local_storage(&self) -> Result<LibsqlStorage, StorageError> {
        use unblock_storage::Storage;
        let storage = LibsqlStorage::open_local(&self.db_path).await?;
        storage.migrate().await?;
        Ok(storage)
    }
}

#[cfg(test)]
mod tests {
    use super::FuzzWorkspace;
    use crate::tokio_block_on;

    #[test]
    fn open_local_storage_round_trips() {
        let ws = FuzzWorkspace::new().expect("workspace");
        tokio_block_on(async {
            use unblock_storage::Storage;
            let storage = ws.open_local_storage().await.expect("open");
            // A trivial read proves the store is live + migrated.
            assert!(
                storage
                    .integrity_check()
                    .await
                    .expect("integrity")
                    .is_empty()
            );
        });
    }
}
