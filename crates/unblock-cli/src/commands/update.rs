//! `unblock update` (FR-25/D17, NFR-17) — self-update via the `axoupdater` LIBRARY, behind the
//! default-on `self-update` feature. The ONLY network surface in the whole binary (confined here).
//!
//! `--dry-run` checks for + reports an available version WITHOUT swapping. A real run downloads +
//! runs `dist`'s installer, which verifies each artifact's SHA256 checksum (from `dist-manifest.json`)
//! before the binary is swapped (`self_replace`); a checksum-mismatched/tampered download surfaces as
//! a `CliError::Update` (→ `InternalError`, exit 1). GitHub artifact attestations are publish-side
//! provenance (`gh attestation verify`), NOT consulted on the update path (NFR-17). The Cargo feature
//! name (`self-update`) deliberately differs from the command token (`unblock update`) — CF-K/G-18.
//! `--no-default-features` drops both.

use axoupdater::AxoUpdater;

use crate::cli::UpdateArgs;
use crate::exit::CliError;
use crate::output;

/// Run `unblock update`.
///
/// # Errors
/// - [`CliError::Update`] on any axoupdater failure (network, unverifiable/tampered artifact, install).
pub async fn run(args: &UpdateArgs) -> Result<Option<u8>, CliError> {
    // `new_for_updater_executable` reads the release source baked in by `dist` at build time so the
    // update targets THIS binary's own release channel (no hardcoded owner/repo, NFR-17 provenance).
    let mut updater = AxoUpdater::new_for_updater_executable().map_err(|e| update_error(&e))?;

    if args.dry_run {
        // Check-only: report the available newer version (if any) without swapping.
        if let Some(version) = updater
            .query_new_version()
            .await
            .map_err(|e| update_error(&e))?
        {
            output::diag(&format!("update available: {version}"));
        } else {
            output::diag("already up to date");
        }
        return Ok(None);
    }

    // Real run: download + run the dist installer (verifies each artifact's SHA256 checksum, NFR-17) + swap.
    if let Some(result) = updater.run().await.map_err(|e| update_error(&e))? {
        output::diag(&format!("updated to {}", result.new_version_tag));
    } else {
        output::diag("already up to date");
    }
    Ok(None)
}

/// Map an `axoupdater` error to the CLI-local `Update` variant (→ `InternalError`, exit 1). The
/// message is surfaced verbatim (the boundary sanitizes it via `StructuredError::from_code`); the
/// concrete `AxoupdateError` type never escapes past this boundary (spine §6 rule 2 spirit).
fn update_error(err: &axoupdater::AxoupdateError) -> CliError {
    CliError::Update {
        message: err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::update_error;
    use crate::exit::CliError;
    use unblock_error::ErrorCode;

    #[test]
    fn update_error_maps_to_internal_error_exit_1() {
        // Any axoupdater error → CliError::Update → InternalError (exit 1). Build a representative one.
        let err = axoupdater::AxoupdateError::NoAppName {};
        let cli = update_error(&err);
        assert!(matches!(cli, CliError::Update { .. }));
        assert_eq!(cli.code(), ErrorCode::InternalError);
        assert_eq!(cli.code().exit_code(), 1);
    }
}
