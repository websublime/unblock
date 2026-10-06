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
//! **D55:** when GitHub refuses the release query, the HTTP status picks the code: 403 or 429 is
//! `CliError::UpdateRateLimited` (→ `RateLimited`, exit 2, retryable) and 401 is
//! `CliError::UpdateUnauthorized` (→ `ConfigError`, exit 7). Every other failure stays `CliError::Update`.
//! The token state (set or unset) picks only the message words, and the token text never reaches the mapper.
//! GitHub artifact attestations are publish-side provenance (`gh attestation verify`), NOT consulted on
//! the update path (NFR-17). A non-empty `AXOUPDATER_GITHUB_TOKEN` is passed to axoupdater's
//! `set_github_token`, so the release query is authenticated (ci-cd §4). The axoupdater LIBRARY reads
//! no token env of its own, so without this call every query is unauthenticated and rate-limited per IP.
//! The Cargo feature name (`self-update`) deliberately differs from the command token (`unblock update`)
//! — CF-K/G-18. `--no-default-features` drops both.

use axoupdater::{AxoUpdater, AxoupdateError};

use crate::cli::UpdateArgs;
use crate::exit::CliError;
use crate::output;

/// The dist **App-name** — the `unblock-cli` PACKAGE name (NOT the `unblock` binary name): `dist` derives
/// the release App from the package, so the install receipt is `unblock-cli-receipt.json` and the
/// release-source lookup keys off `unblock-cli` (ci-cd §3.1 "P2 corrective"; Miguel's GA branding ruling).
const APP_NAME: &str = "unblock-cli";

/// The client-runtime env carrying a GitHub token for the release query (ci-cd §4). It is read HERE,
/// because axoupdater 0.10.0 reads no token env itself; the token reaches it only through
/// `set_github_token`, which sends it as a bearer header on the GitHub API requests. The token is never
/// rendered: axoupdater/reqwest errors name the URL and the status, never a request header.
const GITHUB_TOKEN_ENV: &str = "AXOUPDATER_GITHUB_TOKEN";

/// Whether `AXOUPDATER_GITHUB_TOKEN` holds a token. It carries no token text, so no message can render one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenState {
    /// The variable holds a non-blank token, which the release query sends.
    Set,
    /// The variable is unset, empty or whitespace-only; the release query is anonymous.
    Unset,
}

/// How GitHub refused the release query (D55).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// A 403 or 429: a rate limit, or a refusal that cannot be told apart from one.
    RateLimited,
    /// A 401: the release source rejected the credentials or requires some.
    Unauthorized,
}

/// Run `unblock update`.
///
/// # Errors
/// - [`CliError::UpdateRateLimited`] when GitHub answers the release query 403 or 429.
/// - [`CliError::UpdateUnauthorized`] when GitHub answers the release query 401.
/// - [`CliError::Update`] on any other `axoupdater` failure: no install receipt (a `cargo install` /
///   raw-download copy is not eligible — self-update is only defined for a dist-installed binary), any
///   other HTTP status, a network error, a checksum-mismatched/tampered download, or an install failure.
pub async fn run(args: &UpdateArgs) -> Result<Option<u8>, CliError> {
    // Build an updater for the shipped App and load ITS dist install receipt. The receipt (written by the
    // shell/powershell installer as `unblock-cli-receipt.json`) supplies THIS binary's release source +
    // installed version + install prefix — no hardcoded owner/repo (NFR-17 provenance). A copy with NO
    // receipt (installed via `cargo install` or a raw download) yields `NoReceipt` here → the command
    // REFUSES: self-update is only defined for a dist-installed binary (honest scope, ci-cd §4 / NFR-17).
    let token = github_token(std::env::var(GITHUB_TOKEN_ENV).ok());
    let token_state = if token.is_some() {
        TokenState::Set
    } else {
        TokenState::Unset
    };
    let mut updater = AxoUpdater::new_for(APP_NAME);
    updater
        .load_receipt()
        .map_err(|e| update_error(&e, token_state))?;
    if let Some(token) = token {
        updater.set_github_token(&token);
    }

    if args.dry_run {
        // query_new_version() fetches + caches the latest release and returns its version;
        // is_update_needed() then REUSES that cached release (no 2nd network call) and applies the
        // current<latest comparison + the receipt-eligibility check that dry-run must not skip.
        let latest = updater
            .query_new_version()
            .await
            .map_err(|e| update_error(&e, token_state))?
            .map(ToString::to_string); // own it: releases the &updater borrow before is_update_needed
        if updater
            .is_update_needed()
            .await
            .map_err(|e| update_error(&e, token_state))?
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
    if let Some(result) = updater
        .run()
        .await
        .map_err(|e| update_error(&e, token_state))?
    {
        output::diag(&format!("updated to {}", result.new_version_tag));
    } else {
        output::diag("already up to date");
    }
    Ok(None)
}

/// Map an `axoupdater` error to a `CliError`. A status-bearing HTTP error whose status
/// [`refusal_for_status`] classifies becomes `UpdateRateLimited` or `UpdateUnauthorized`, with the text
/// from [`refusal_message`]; anything else becomes `Update` (→ `InternalError`, exit 1) with the error
/// surfaced verbatim (the boundary sanitizes it via `StructuredError::from_code`). The status is read on
/// the variant itself, because the transparent variant forwards a `None` source for a status error. The
/// concrete `AxoupdateError` type never escapes past this boundary (spine §6 rule 2 spirit).
fn update_error(err: &AxoupdateError, token_state: TokenState) -> CliError {
    if let AxoupdateError::Reqwest(e) = err
        && let Some(status) = e.status().map(u16::from)
        && let Some(refusal) = refusal_for_status(status)
    {
        // A parsed `Url` serializes with no tab, newline or raw control character, so the message
        // stays one line.
        let url = e.url().map(ToString::to_string).unwrap_or_default();
        let message = refusal_message(refusal, token_state, status, &url);
        return match refusal {
            Refusal::RateLimited => CliError::UpdateRateLimited { message },
            Refusal::Unauthorized => CliError::UpdateUnauthorized { message },
        };
    }
    CliError::Update {
        message: err.to_string(),
    }
}

/// Classify the HTTP status GitHub answered the release query with. 403 and 429 are rate limits, 401
/// is a rejected or missing token, and every other status is no refusal D55 classifies.
const fn refusal_for_status(status: u16) -> Option<Refusal> {
    match status {
        403 | 429 => Some(Refusal::RateLimited),
        401 => Some(Refusal::Unauthorized),
        _ => None,
    }
}

/// The refusal text after the `self-update failed: ` prefix, which the variant's display owns. It
/// names the status, the query URL and `AXOUPDATER_GITHUB_TOKEN`, holds no newline, and carries the
/// remedy (PRD §4 D55 clause (5)).
fn refusal_message(refusal: Refusal, token_state: TokenState, status: u16, url: &str) -> String {
    match (refusal, token_state) {
        (Refusal::RateLimited, TokenState::Unset) => format!(
            "GitHub refused the release query with HTTP {status} ({url}). Anonymous queries share \
             GitHub's limit of 60 per hour per IP address. Set {GITHUB_TOKEN_ENV} to a GitHub token, \
             or retry later."
        ),
        (Refusal::RateLimited, TokenState::Set) => format!(
            "GitHub refused the release query with HTTP {status} ({url}) although {GITHUB_TOKEN_ENV} \
             is set. The authenticated rate limit may be used up, or GitHub may be refusing the token, \
             for example after repeated failed logins. Retry later, and check the token if it keeps \
             failing."
        ),
        (Refusal::Unauthorized, TokenState::Set) => format!(
            "GitHub rejected the token in {GITHUB_TOKEN_ENV} with HTTP {status} ({url}). Replace it \
             with a valid token, or unset it to query anonymously."
        ),
        (Refusal::Unauthorized, TokenState::Unset) => format!(
            "the release source refused the query with HTTP {status} ({url}) and requires a token. \
             Set {GITHUB_TOKEN_ENV} to a token it accepts."
        ),
    }
}

/// The token to authenticate the release query with, from the raw `AXOUPDATER_GITHUB_TOKEN` value.
/// Unset, empty or whitespace-only means NO token: an empty bearer header would turn GitHub's
/// unauthenticated answer into a 401 for everyone who exports the variable blank.
fn github_token(raw: Option<String>) -> Option<String> {
    raw.filter(|token| !token.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::{
        Refusal, TokenState, github_token, refusal_for_status, refusal_message, update_error,
    };
    use crate::exit::CliError;
    use unblock_error::ErrorCode;

    const URL: &str = "https://api.github.com/repos/websublime/unblock/releases";

    #[test]
    fn update_error_maps_to_internal_error_exit_1() {
        // A non-HTTP axoupdater error → CliError::Update → InternalError (exit 1), whatever the token state.
        let err = axoupdater::AxoupdateError::NoAppName {};
        for token_state in [TokenState::Set, TokenState::Unset] {
            let cli = update_error(&err, token_state);
            assert!(matches!(cli, CliError::Update { .. }));
            assert_eq!(cli.code(), ErrorCode::InternalError);
            assert_eq!(cli.code().exit_code(), 1);
        }
    }

    #[test]
    fn refusal_for_status_classifies_401_403_429_only() {
        assert_eq!(refusal_for_status(401), Some(Refusal::Unauthorized));
        assert_eq!(refusal_for_status(403), Some(Refusal::RateLimited));
        assert_eq!(refusal_for_status(429), Some(Refusal::RateLimited));
        assert_eq!(refusal_for_status(404), None);
        assert_eq!(refusal_for_status(500), None);
        assert_eq!(refusal_for_status(200), None);
    }

    #[test]
    fn refusal_message_renders_the_four_d55_texts() {
        assert_eq!(
            refusal_message(Refusal::RateLimited, TokenState::Unset, 403, URL),
            format!(
                "GitHub refused the release query with HTTP 403 ({URL}). Anonymous queries share \
                 GitHub's limit of 60 per hour per IP address. Set AXOUPDATER_GITHUB_TOKEN to a \
                 GitHub token, or retry later."
            )
        );
        assert_eq!(
            refusal_message(Refusal::RateLimited, TokenState::Set, 429, URL),
            format!(
                "GitHub refused the release query with HTTP 429 ({URL}) although \
                 AXOUPDATER_GITHUB_TOKEN is set. The authenticated rate limit may be used up, or \
                 GitHub may be refusing the token, for example after repeated failed logins. Retry \
                 later, and check the token if it keeps failing."
            )
        );
        assert_eq!(
            refusal_message(Refusal::Unauthorized, TokenState::Set, 401, URL),
            format!(
                "GitHub rejected the token in AXOUPDATER_GITHUB_TOKEN with HTTP 401 ({URL}). \
                 Replace it with a valid token, or unset it to query anonymously."
            )
        );
        assert_eq!(
            refusal_message(Refusal::Unauthorized, TokenState::Unset, 401, URL),
            format!(
                "the release source refused the query with HTTP 401 ({URL}) and requires a token. \
                 Set AXOUPDATER_GITHUB_TOKEN to a token it accepts."
            )
        );
    }

    #[test]
    fn refusal_message_names_the_env_on_one_line_and_tells_set_from_unset() {
        for refusal in [Refusal::RateLimited, Refusal::Unauthorized] {
            let status = if refusal == Refusal::RateLimited {
                403
            } else {
                401
            };
            let set = refusal_message(refusal, TokenState::Set, status, URL);
            let unset = refusal_message(refusal, TokenState::Unset, status, URL);
            assert_ne!(set, unset);
            for text in [&set, &unset] {
                assert!(text.contains("AXOUPDATER_GITHUB_TOKEN"), "{text}");
                assert!(text.contains(&format!("HTTP {status} ({URL})")), "{text}");
                assert!(!text.contains('\n'), "{text}");
                assert!(!text.starts_with("self-update failed"), "{text}");
            }
        }
    }

    #[test]
    fn a_blank_token_env_means_no_token() {
        assert_eq!(github_token(None), None);
        assert_eq!(github_token(Some(String::new())), None);
        assert_eq!(github_token(Some("  \t".to_owned())), None);
        assert_eq!(
            github_token(Some("ghp_x".to_owned())),
            Some("ghp_x".to_owned())
        );
    }
}
