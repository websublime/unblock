//! `unblock migrate` (D27/AF-2) — ensure the schema is current and report the real from→to delta.
//!
//! Opens the context (the config facade already migrates on open), opens the `Session`, calls
//! `Session::migrate() -> MigrateOutcome` (under the write permit), and renders a CLI-local
//! `MigrateReport`. A newer-than-build DB surfaces the transparent `SchemaMismatch` (→ exit 2) via
//! the engine/storage error, never a fake success.
//!
//! **D46 clause (10) — this command COMPOSES its report; it no longer renders `MigrateOutcome`
//! alone.** The blind spot it fixes was STRUCTURAL: the facade runs the migration AT OPEN
//! (`crates/unblock-config/src/context.rs`) and only afterwards does `Session::migrate` read `from`,
//! so rendering the outcome alone this command could only ever print `from == to`, `applied: false`
//! — on the very database D46 exists to repair. The facade now RECORDS the stamp it read BEFORE
//! migrating on `WorkspaceContext::schema_version_before_migrate`, and this command reports that
//! pre-open delta. `applied = false` therefore means EXACTLY "the stamp did not move across THIS
//! run's own open" — never "nothing was wrong". Three cases, given the same treatment: a STALE
//! workspace prints `1` → `2` `applied: true`; a NEVER-MIGRATED one `0` → `2` `applied: true`; an
//! ALREADY-CURRENT one (anything `unblock init` or an earlier command opened, and every second run)
//! `2` → `2` `applied: false`. A stamp that LIES still surfaces as `SchemaMismatch` → exit 2 on every
//! path, carrying the hint composed in `unblock-storage` and forwarded through `ConfigError::hint()`
//! — this command adds no hint text of its own.

use unblock_config::{CliOverrides, open_with_storage_with_cli};
use unblock_engine::{Session, SessionConfig};

use crate::cli::MigrateArgs;
use crate::exit::CliError;
use crate::output::{self, MigrateReport, ToDiagnosticReport};

/// Run `unblock migrate`.
///
/// # Errors
/// - [`CliError::Config`] if the workspace cannot be opened (discovery/DB/`SchemaMismatch` on open);
/// - [`CliError::Engine`] if the session cannot be opened or `migrate()` fails;
/// - [`CliError::Render`]/[`CliError::Io`] if rendering / writing the report fails.
pub async fn run(_args: &MigrateArgs, overrides: &CliOverrides) -> Result<Option<u8>, CliError> {
    let ctx = open_with_storage_with_cli(overrides).await?;
    let database = ctx.paths.db_path.clone();
    let fmt = ctx.config.output_format;
    // D46 clause (10): copy the PRE-OPEN stamp out before `Session::open` consumes the context —
    // exactly as `paths.db_path` and `config.output_format` are copied above.
    let schema_from = ctx.schema_version_before_migrate;

    let session = Session::open(
        ctx,
        SessionConfig {
            import_on_open: false,
            ..SessionConfig::default()
        },
    )
    .await?;

    let outcome = session.migrate().await?;
    let report = MigrateReport {
        database,
        // The stamp observed BEFORE the facade's open-time migration (D46 clause (10)) — NOT
        // `outcome.from`, which the facade has already advanced past by the time the engine reads it.
        schema_from,
        // Still verbatim from the engine outcome: the version re-read under the D14 write permit.
        schema_to: outcome.to,
        // "The stamp moved across this run's own open."
        applied: schema_from != outcome.to,
    };

    output::emit_report(&report.to_report(), fmt).map(|()| None)
}
