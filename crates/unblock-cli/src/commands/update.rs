//! `unblock update` (FR-25/D17, NFR-17) — self-update via the `axoupdater` LIBRARY, behind the
//! default-on `self-update` feature. The ONLY network surface in the whole binary (confined here).
//!
//! The updater loads THIS binary's dist install **receipt** (`unblock-cli-receipt.json`, App-name =
//! the `unblock-cli` package, ci-cd §3.1) to learn its release source + installed version; a copy with
//! no receipt (`cargo install` / raw download) is not eligible for self-update and the command refuses
//! (`NoReceipt` → `CliError::Update`). `--dry-run` checks for + reports an available version WITHOUT
//! swapping. A real run downloads + runs `dist`'s installer, which verifies each artifact's SHA256
//! checksum (from `dist-manifest.json`) before the binary is swapped (`self_replace`); a
//! checksum-mismatched/tampered download surfaces as a `CliError::Update` (→ `InternalError`, exit 1).
//! GitHub artifact attestations are publish-side provenance (`gh attestation verify`), NOT consulted on
//! the update path (NFR-17). The Cargo feature name (`self-update`) deliberately differs from the
//! command token (`unblock update`) — CF-K/G-18. `--no-default-features` drops both.

use axoupdater::AxoUpdater;

use crate::cli::UpdateArgs;
use crate::exit::CliError;
use crate::output;

/// The dist **App-name** — the `unblock-cli` PACKAGE name (NOT the `unblock` binary name): `dist` derives
/// the release App from the package, so the install receipt is `unblock-cli-receipt.json` and the
/// release-source lookup keys off `unblock-cli` (ci-cd §3.1 "P2 corrective"; Miguel's GA branding ruling).
const APP_NAME: &str = "unblock-cli";

/// Run `unblock update`.
///
/// # Errors
/// - [`CliError::Update`] on any `axoupdater` failure: no install receipt (a `cargo install` / raw-download
///   copy is not eligible — self-update is only defined for a dist-installed binary), a network error, a
///   checksum-mismatched/tampered download, or an install failure.
pub async fn run(args: &UpdateArgs) -> Result<Option<u8>, CliError> {
    // Build an updater for the shipped App and load ITS dist install receipt. The receipt (written by the
    // shell/powershell installer as `unblock-cli-receipt.json`) supplies THIS binary's release source +
    // installed version + install prefix — no hardcoded owner/repo (NFR-17 provenance). A copy with NO
    // receipt (installed via `cargo install` or a raw download) yields `NoReceipt` here → the command
    // REFUSES: self-update is only defined for a dist-installed binary (honest scope, ci-cd §4 / NFR-17).
    let mut updater = AxoUpdater::new_for(APP_NAME);
    updater.load_receipt().map_err(|e| update_error(&e))?;

    if args.dry_run {
        // query_new_version() fetches + caches the latest release and returns its version;
        // is_update_needed() then REUSES that cached release (no 2nd network call) and applies the
        // current<latest comparison + the receipt-eligibility check that dry-run must not skip.
        let latest = updater
            .query_new_version()
            .await
            .map_err(|e| update_error(&e))?
            .map(ToString::to_string); // own it: releases the &updater borrow before is_update_needed
        if updater
            .is_update_needed()
            .await
            .map_err(|e| update_error(&e))?
        {
            if let Some(version) = latest {
                output::diag(&format!("update available: {version}"));
            }
        } else {
            output::diag("already up to date");
        }
        return Ok(None);
    }

    // Real run: download + run the dist installer, which verifies each artifact's SHA256 checksum (from
    // `dist-manifest.json`) before `self_replace` swaps the binary (NFR-17). A checksum-mismatched /
    // tampered download aborts the installer non-zero → `InstallFailed` → `CliError::Update`, no swap.
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
