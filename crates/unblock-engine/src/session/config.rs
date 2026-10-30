//! `impl Session` runtime configuration — FR-13 full through the engine (PRD §4 D57, spine §4.1
//! "runtime configuration (FR-13 full, D57").
//!
//! The four operations behind `issue action: config`, which are also the clause (2a) REPAIR path:
//! the fail-closed gate NEVER applies to them, so a workspace whose `config` table holds a blocking
//! row can always be read and repaired.
//!
//! - [`Session::config_get`] / [`Session::config_list`] are READS (no permit, no `.write.lock`,
//!   FR-10): `value` and `fault` come from the rows read at call time — `fault` through the pure,
//!   infallible [`unblock_config::decode_db_rows`], so a row another process wrote since this session
//!   opened is reported with its real fault and a broken TOML file never fails them — while
//!   `effective`/`source` (layer keys only) are the LIVE snapshot. The two can transiently disagree
//!   (another process's write, or inside a concurrent `config_set`'s commit→swap window): accepted,
//!   because `effective` is by definition what THIS session resolves.
//! - [`Session::config_set`] / [`Session::config_unset`] hold the write permit AND the `.write.lock`
//!   for the WHOLE operation (D14/D31) and, under that guard, IN ORDER: `set` classifies the key and
//!   canonicalizes the value, then reads the rows; `unset` reads the rows FIRST and classifies the key
//!   only when no row holds it; both build the prospective row set and RELOAD against it (a reload
//!   failure — a malformed project or user file — returns with ZERO writes); then ONE spawned task
//!   that OWNS the guard, a storage clone and the snapshot cell runs the ONE `set_config` /
//!   `unset_config` transaction when there is anything to write (D44) and ALWAYS the swap, so a repair
//!   made by another process reaches this session at its next `config` call. The method awaits that
//!   task, so a caller future dropped after the commit began can never separate the commit from the
//!   swap or release the guard early (spine §4.2 cancel-safety); a panic in the task is re-raised
//!   with [`std::panic::resume_unwind`], and its cancellation by runtime shutdown maps to
//!   [`EngineError::ShutdownInProgress`].
//!
//! The ONLY reload trigger is a non-refused in-process `config_set`/`config_unset` (D57 clause (8)).
//! A `config` write records NO `Event` (events are per issue, spine §1.7).

use std::collections::BTreeSet;
use std::sync::Arc;

use unblock_config::keys::{KeyClass, LAYER_KEYS, RuntimeKey, classify};
use unblock_config::{
    RuntimeConfig, canonicalize_runtime_value, classify_runtime_key, decode_db_rows,
};
use unblock_model::{ConfigEntry, ConfigSource, DbConfigFault, OutputFormat};

use crate::error::{EngineError, Result};
use crate::permit::MutationGuard;
use crate::session::Session;

/// The one write a `config_set`/`config_unset` commits, if any.
enum ConfigWrite {
    /// Upsert `key` to the canonical value (`Storage::set_config`).
    Set { key: String, value: String },
    /// Delete the row holding `key` (`Storage::unset_config`).
    Unset { key: String },
}

impl Session {
    /// Read ONE configuration entry (`issue action: config, op: get`) — a read: no permit, no lock.
    ///
    /// `value` is the key's `config` row (`None` when absent) and `fault` that row's clause (2a)
    /// fault, both from the rows read NOW; `effective`/`source` are the live snapshot's when `key`
    /// is a LAYER key, else `None`. A key that HAS a row is reachable whatever its class (that is
    /// the clause (2a) recovery); a NON-layer key with no row is refused — with NO `db_faults`
    /// exception (that belongs to [`config_unset`](Session::config_unset) alone).
    ///
    /// # Errors
    /// - [`EngineError::Config`]: `StartupKeyInRuntimeLayer { layer: Db }` for a startup key,
    ///   `InvalidValue` naming `ub-p1h` for `jsonl_export`, `UnknownKey` for anything else — each
    ///   only when no row holds the key.
    /// - The transparent storage source if the rows cannot be read.
    pub async fn config_get(&self, key: &str) -> Result<ConfigEntry> {
        let rows = self.storage.read_config().await?;
        if !holds_row(&rows, key) {
            classify_runtime_key(key)?;
        }
        let faults = decode_db_rows(&rows);
        Ok(entry(key, &rows, &faults, &self.runtime()))
    }

    /// List the configuration (`issue action: config, op: list`) — a read: no permit, no lock.
    ///
    /// The three LAYER keys PLUS one entry per row whose key is not a layer key (a startup,
    /// unknown or `jsonl_export` row, each carrying its fault), all in key order (byte order), each
    /// built as [`config_get`](Session::config_get) builds one, from ONE `read_config()` call.
    ///
    /// # Errors
    /// The transparent storage source if the rows cannot be read.
    pub async fn config_list(&self) -> Result<Vec<ConfigEntry>> {
        let rows = self.storage.read_config().await?;
        let faults = decode_db_rows(&rows);
        let runtime = self.runtime();
        let keys: BTreeSet<&str> = LAYER_KEYS
            .iter()
            .copied()
            .chain(rows.iter().map(|(key, _)| key.as_str()))
            .collect();
        Ok(keys
            .into_iter()
            .map(|key| entry(key, &rows, &faults, &runtime))
            .collect())
    }

    /// SET a LAYER key in the database `config` table (`issue action: config, op: set`), then
    /// reload and swap the live snapshot.
    ///
    /// Under the write permit + `.write.lock`, in order: classify `key` (only the three layer keys)
    /// and canonicalize `value`; read the rows; upsert the canonical value into the prospective row
    /// set (a faulty row of that key is REPLACED); reload against it — a failure returns HERE with
    /// zero writes; then commit + swap in one owned task. Returns the entry after the write and the
    /// reload.
    ///
    /// # Errors
    /// - [`EngineError::Config`]: `InvalidValue` (a bad value, or `jsonl_export` naming `ub-p1h`),
    ///   `StartupKeyInRuntimeLayer { layer: Db }`, `UnknownKey`, or a reload failure — all with
    ///   ZERO writes and the snapshot unchanged.
    /// - [`EngineError::ShutdownInProgress`] if a shutdown is in progress (or the runtime cancelled
    ///   the commit task).
    /// - The transparent storage source on a `.write.lock`, read or write failure.
    pub async fn config_set(&self, key: &str, value: &str) -> Result<ConfigEntry> {
        let guard = self.acquire().await?;
        let runtime_key = classify_runtime_key(key)?;
        let canonical = canonicalize_runtime_value(runtime_key, value)?;
        let mut rows = self.storage.read_config().await?;
        match rows.binary_search_by(|(row_key, _)| row_key.as_str().cmp(key)) {
            Ok(at) => rows[at].1.clone_from(&canonical),
            Err(at) => rows.insert(at, (key.to_string(), canonical.clone())),
        }
        let next = Arc::new(self.reloader.reload_runtime(&rows)?);
        self.commit_and_swap(
            guard,
            Some(ConfigWrite::Set {
                key: key.to_string(),
                value: canonical,
            }),
            Arc::clone(&next),
        )
        .await?;
        Ok(entry(key, &rows, &next.db_faults, &next))
    }

    /// UNSET a key in the database `config` table (`issue action: config, op: unset`), then reload
    /// and swap the live snapshot — write or no write.
    ///
    /// Under the write permit + `.write.lock`: read the rows FIRST. When a row holds `key` it is
    /// deleted whatever the key's class (deleting a startup, unknown or `jsonl_export` row is how a
    /// clause (2a) fault is repaired). When NO row holds it, a LAYER key — or any key the session's
    /// snapshot lists among its `db_faults` (another process already removed the row: the
    /// stale-gate repair) — is a SUCCESS with no write; any other key is refused. Every non-refused
    /// call reloads against the rows minus `key` and swaps, so a repair made elsewhere reaches this
    /// session now. A non-layer key returns `{key, value: null, effective: null, source: null,
    /// fault: null}`.
    ///
    /// # Errors
    /// - [`EngineError::Config`]: the classify refusal for a non-layer key no row holds and no
    ///   snapshot fault names, or a reload failure — both with ZERO writes.
    /// - [`EngineError::ShutdownInProgress`] if a shutdown is in progress (or the runtime cancelled
    ///   the commit task).
    /// - The transparent storage source on a `.write.lock`, read or write failure.
    pub async fn config_unset(&self, key: &str) -> Result<ConfigEntry> {
        let guard = self.acquire().await?;
        let mut rows = self.storage.read_config().await?;
        let write = if let Some(at) = rows.iter().position(|(row_key, _)| row_key == key) {
            rows.remove(at);
            Some(ConfigWrite::Unset {
                key: key.to_string(),
            })
        } else {
            let stale_fault = self.runtime().db_faults.iter().any(|f| f.key == key);
            if !stale_fault {
                classify_runtime_key(key)?;
            }
            None
        };
        let next = Arc::new(self.reloader.reload_runtime(&rows)?);
        self.commit_and_swap(guard, write, Arc::clone(&next))
            .await?;
        Ok(entry(key, &rows, &next.db_faults, &next))
    }

    /// The commit + swap tail of `config_set`/`config_unset` (D57 clause (7)): ONE spawned task that
    /// OWNS `guard`, a storage clone and the snapshot cell runs `write` (the ONE transaction, if
    /// any) and then ALWAYS swaps in `next`; the caller awaits it. A task panic is re-raised; a
    /// cancellation (runtime shutdown) is [`EngineError::ShutdownInProgress`].
    async fn commit_and_swap(
        &self,
        guard: MutationGuard,
        write: Option<ConfigWrite>,
        next: Arc<RuntimeConfig>,
    ) -> Result<()> {
        let storage = Arc::clone(&self.storage);
        let cell = Arc::clone(&self.runtime);
        let task = tokio::spawn(async move {
            // Held until the swap is done: no mutation can observe the old snapshot after the commit.
            let _guard = guard;
            match write {
                Some(ConfigWrite::Set { key, value }) => storage.set_config(&key, &value).await?,
                Some(ConfigWrite::Unset { key }) => {
                    storage.unset_config(&key).await?;
                }
                None => {}
            }
            Session::swap_runtime(&cell, next);
            Ok::<(), EngineError>(())
        });
        match task.await {
            Ok(outcome) => outcome,
            Err(join) if join.is_panic() => std::panic::resume_unwind(join.into_panic()),
            Err(_cancelled) => Err(EngineError::ShutdownInProgress),
        }
    }
}

/// Whether a row holds `key` (`rows` are ordered by key, but a linear scan is exact on any order).
fn holds_row(rows: &[(String, String)], key: &str) -> bool {
    rows.iter().any(|(row_key, _)| row_key == key)
}

/// Build one [`ConfigEntry`]: `value` = the key's row, `fault` = its fault in `faults` (the decode
/// of THOSE rows), `effective`/`source` = `runtime`'s when `key` is a LAYER key, else `None`.
fn entry(
    key: &str,
    rows: &[(String, String)],
    faults: &[DbConfigFault],
    runtime: &RuntimeConfig,
) -> ConfigEntry {
    let value = rows
        .iter()
        .find(|(row_key, _)| row_key == key)
        .map(|(_, value)| value.clone());
    let fault = faults
        .iter()
        .find(|f| f.key == key)
        .map(|f| f.fault.clone());
    let (effective, source) = match effective(key, runtime) {
        Some((effective, source)) => (Some(effective), Some(source)),
        None => (None, None),
    };
    ConfigEntry {
        key: key.to_string(),
        value,
        effective,
        source,
        fault,
    }
}

/// The canonical string and source of the value `runtime` resolved for a LAYER key — the same
/// canonical spelling a `config` row stores (D57 clause (4)) — or `None` for any other key.
fn effective(key: &str, runtime: &RuntimeConfig) -> Option<(String, ConfigSource)> {
    match classify(key) {
        Some(KeyClass::Runtime(RuntimeKey::Actor)) => {
            Some((runtime.actor.clone(), runtime.sources.actor))
        }
        Some(KeyClass::Runtime(RuntimeKey::OutputFormat)) => Some((
            output_format_name(runtime.output_format),
            runtime.sources.output_format,
        )),
        Some(KeyClass::Runtime(RuntimeKey::SearchCap)) => {
            Some((runtime.search_cap.to_string(), runtime.sources.search_cap))
        }
        Some(KeyClass::Runtime(RuntimeKey::JsonlExport) | KeyClass::Startup(_)) | None => None,
    }
}

/// The wire name of an [`OutputFormat`] — its serde `snake_case` spelling, the one string table
/// the canonical `output_format` row value is produced from.
fn output_format_name(format: OutputFormat) -> String {
    match serde_json::to_value(format) {
        Ok(serde_json::Value::String(name)) => name,
        // A unit-variant `rename_all = "snake_case"` enum always serializes to a string; the debug
        // spelling is an unreachable, still-deterministic fallback.
        _ => format!("{format:?}").to_ascii_lowercase(),
    }
}
