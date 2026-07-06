//! `unblock migrate` (D27/AF-2) — ensure the schema is current and report the real from→to delta.
//!
//! Opens the context (the config facade already migrates on open), opens the `Session`, calls the NEW
//! `Session::migrate() -> MigrateOutcome` (under the write permit), and renders a CLI-local
//! `MigrateReport`. Idempotent — a second run reports the current DB with `applied = false` (honest,
//! since open already migrated). A newer-than-build DB surfaces the transparent `SchemaMismatch`
//! (→ exit 2) via the engine/storage error, never a fake success.

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
        schema_from: outcome.from,
        schema_to: outcome.to,
        applied: outcome.applied,
    };

    output::emit_report(&report.to_report(), fmt).map(|()| None)
}
