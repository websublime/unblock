//! The DB layer (FR-13 full, PRD §4 D57 clause (4) and (2a)) — the database `config` table's rows,
//! decoded TOLERANTLY into a [`DbLayer`] plus the faulty rows.
//!
//! The rows arrive from `Storage::read_config()` (spine §3.2) — never raw libsql here (NFR-15): the
//! open facade reads them at open and the engine passes them to
//! [`crate::ConfigReloader::reload_runtime`], so this module is PURE over `&[(String, String)]`.
//!
//! Each row is classified ([`crate::keys::classify`]):
//! - a LAYER key (`actor`, `output_format`, `search_cap`) is decoded by the same per-key parser the
//!   env layer uses (the one [`crate::canonicalize_runtime_value`] runs); a value that decodes
//!   CONTRIBUTES, one that does not is an `invalid_value` fault;
//! - `jsonl_export` is a runtime key but NOT a layer key (its consumer is `ub-p1h`): an
//!   `invalid_value` fault;
//! - a STARTUP key (either `jsonl_filename` spelling) is a `startup_key` fault;
//! - anything else is an `unknown_key` fault (SF-4 forward compatibility — warned, never blocking).
//!
//! A faulty row NEVER contributes a value, and the layer never fails an open: the faults travel in
//! `RuntimeConfig::db_faults`, and a blocking one fail-closes the engine `Session` instead.

use unblock_model::{ConfigFault, ConfigFaultKind, DbConfigFault, OutputFormat};

use crate::keys::{KeyClass, RuntimeKey, classify};
use crate::reload::{RuntimeValue, parse_runtime_value};

/// The values the database `config` table contributes — only the three LAYER keys, each `None`
/// when no sound row holds it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DbLayer {
    /// A sound `actor` row (trimmed, `validate_actor`-bounded).
    pub(crate) actor: Option<String>,
    /// A sound `output_format` row.
    pub(crate) output_format: Option<OutputFormat>,
    /// A sound `search_cap` row (`1..=i64::MAX`).
    pub(crate) search_cap: Option<usize>,
}

/// Decode `rows` TOLERANTLY (PRD §4 D57 clause (2a)): the [`DbLayer`] the sound layer-key rows
/// contribute, and one [`DbConfigFault`] per faulty row, in row order. An `unknown_key` row is
/// warned on the `unblock.config` tracing target (SF-4).
pub(crate) fn db_layer(rows: &[(String, String)]) -> (DbLayer, Vec<DbConfigFault>) {
    let mut layer = DbLayer::default();
    let mut faults = Vec::new();
    for (key, value) in rows {
        match decode_row(key, value) {
            Ok(RuntimeValue::Actor(actor)) => layer.actor = Some(actor),
            Ok(RuntimeValue::OutputFormat(format)) => layer.output_format = Some(format),
            Ok(RuntimeValue::SearchCap(cap)) => layer.search_cap = Some(cap),
            Err(fault) => {
                if fault.kind == ConfigFaultKind::UnknownKey {
                    tracing::warn!(
                        target: "unblock.config",
                        key = %key,
                        "unknown key in the database config table (ignored; remove it with \
                         issue config unset)"
                    );
                }
                faults.push(DbConfigFault {
                    key: key.clone(),
                    value: value.clone(),
                    fault,
                });
            }
        }
    }
    (layer, faults)
}

/// The tolerant DB decode's FAULT half (PRD §4 D57 clause (2a)) — PURE and INFALLIBLE (no file
/// read): exactly the faults [`crate::ConfigReloader::reload_runtime`] and the open facade record for
/// `rows`. The engine's `config_get`/`config_list` call it on the rows they read, so a row another
/// process wrote since this session opened is reported with its real fault, and a broken TOML file
/// can never make the read-only repair diagnostics fail.
#[must_use]
pub fn decode_db_rows(rows: &[(String, String)]) -> Vec<DbConfigFault> {
    db_layer(rows).1
}

/// Classify and decode one row: its value when it is a sound layer-key row, else its fault.
fn decode_row(key: &str, value: &str) -> Result<RuntimeValue, ConfigFault> {
    let runtime_key = match classify(key) {
        Some(KeyClass::Runtime(RuntimeKey::JsonlExport)) => {
            return Err(ConfigFault {
                kind: ConfigFaultKind::InvalidValue,
                reason: "`jsonl_export` is not a database-layer key (nothing reads it yet; its \
                         consumer is tracked as ub-p1h)"
                    .to_string(),
            });
        }
        Some(KeyClass::Runtime(rk)) => rk,
        Some(KeyClass::Startup(_)) => {
            return Err(ConfigFault {
                kind: ConfigFaultKind::StartupKey,
                reason: format!(
                    "`{key}` is a startup-only key; set it on the command line, in the environment \
                     or in the project file .unblock/config.toml"
                ),
            });
        }
        None => {
            return Err(ConfigFault {
                kind: ConfigFaultKind::UnknownKey,
                reason: format!("`{key}` is not a key this build knows"),
            });
        }
    };
    parse_runtime_value(runtime_key, value).map_err(|e| ConfigFault {
        kind: ConfigFaultKind::InvalidValue,
        reason: e.to_string(),
    })
}
