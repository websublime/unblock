//! `unblock update` refusal + no-swap safety gate (FR-25/D17/NFR-17, CF-K).
//!
//! ## What this test PROVES (the safety-critical invariant)
//!
//! `unblock update` is wired to `axoupdater::AxoUpdater::new_for_updater_executable()`, which trusts
//! ONLY the release source baked into a genuine `dist` install. A test harness is NOT such an install,
//! so the updater is "not properly configured" for THIS executable and REFUSES **before any network
//! request** — surfacing as a `CliError::Update → InternalError` (exit 1) with a valid structured
//! error on stdout (FR-11). The load-bearing assertions here are:
//!   - the update is REFUSED with exit 1 (never a silent success), and
//!   - the on-disk `unblock` binary is **byte-identical** afterwards (the NO-SWAP invariant — no
//!     unverified artifact was ever swapped in).
//!
//! ## What this test does NOT (yet) prove — and why the wiremock scaffold is kept
//!
//! The refusal fires at `new_for_updater_executable()`, **before** any network I/O, so the wiremock
//! server + the `<PREFIX>_INSTALLER_GHE_BASE_URL` redirect below are **never actually hit** on this
//! path — they are retained only to DOCUMENT INTENT (they stand up the untrusted release source a
//! future test will drive) and to keep the child hermetic (a redirected base can never reach real
//! github.com even if the code path changed). They do NOT exercise an attestation/download-tampering
//! rejection today; the names/comments are written to say exactly that, not to overclaim.
//!
//! TODO(T3.6 / v1 GA): add a genuine tampered-DOWNLOAD attestation test. That needs a `dist`-receipt
//! fixture so `new_for_updater_executable()` succeeds and the run reaches the download/verify stage,
//! where a tampered artifact from the mock source must be REJECTED at attestation. Until that fixture
//! exists, this suite proves the earlier (still safety-critical) unconfigured-updater refusal + no-swap.
//!
//! The mock base is redirected per-CHILD via `Command::env` (parallel-safe; the suite never calls the
//! `unsafe`/edition-2024-forbidden `std::env::set_var`). `--no-default-features` drops the command
//! entirely — proven by the CI feature-matrix build (`cargo build -p unblock-cli
//! --no-default-features`), so no network dependency is even compiled without `self-update`.
//!
//! Gated on `self-update` (the default-on feature that carries the whole update surface).
#![cfg(feature = "self-update")]

mod common;

use common::unblock;
use serde_json::Value;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The env-var prefix axoupdater derives from the app name `unblock`
/// (`app_name_to_env_var("unblock") == "UNBLOCK"`); `<PREFIX>_INSTALLER_GHE_BASE_URL` redirects the
/// GitHub-Enterprise API base so — IF the code path ever reached the network — a request would go to
/// the mock, not real github.com. On the refusal path exercised here it is never actually queried; it
/// is a hermeticity guard + intent marker (see the module TODO).
const GHE_BASE_URL_ENV: &str = "UNBLOCK_INSTALLER_GHE_BASE_URL";

/// Stand up a mock "release source" that returns a non-release payload for every releases query — an
/// untrusted source a future download-tampering test will drive. On the current refusal path it is
/// never hit (the updater refuses before any network I/O); kept to document intent (see module TODO).
async fn untrusted_release_server() -> MockServer {
    let server = MockServer::start().await;
    // Every GHE releases endpoint would return a body that is NOT a valid, attested release (a bare
    // object with no installer assets) — the untrusted artifact a future test must refuse at verify.
    Mock::given(method("GET"))
        .and(path_regex(r".*/releases.*"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({ "tampered": true })),
        )
        .mount(&server)
        .await;
    server
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_refuses_an_unconfigured_updater_and_swaps_nothing() {
    // The untrusted source is stood up to document intent + keep the child hermetic; the refusal below
    // fires BEFORE it would ever be queried (see module docs).
    let server = untrusted_release_server().await;

    // Snapshot the on-disk binary bytes BEFORE the update attempt (the no-swap safety invariant).
    let bin_path = assert_cmd::cargo::cargo_bin("unblock");
    let before = std::fs::read(&bin_path).expect("read the unblock binary before");

    // Drive the REAL `unblock update` child. The updater is NOT configured for THIS
    // (non-dist-installed) executable, so it refuses at `new_for_updater_executable()` BEFORE any
    // network I/O — no swap can occur.
    let out = unblock()
        .args(["update", "--output", "json"])
        .env(GHE_BASE_URL_ENV, server.uri())
        .output()
        .expect("run `unblock update`");

    // Refused: a non-zero exit with a VALID structured error on stdout (FR-11). The whole update
    // surface maps to InternalError (exit 1) — never a silent success from an unconfigured updater.
    assert_ne!(
        out.status.code(),
        Some(0),
        "an unconfigured updater must be REFUSED"
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "the self-update failure maps to InternalError (exit 1)"
    );
    let value: Value =
        serde_json::from_slice(&out.stdout).expect("valid JSON structured error on stdout (FR-11)");
    assert_eq!(value["code"], "INTERNAL_ERROR", "self-update failure code");

    // The safety invariant: NO binary was swapped — the on-disk bytes are unchanged.
    let after = std::fs::read(&bin_path).expect("read the unblock binary after");
    assert_eq!(
        before, after,
        "no unverified binary may be swapped in — the on-disk `unblock` must be byte-identical"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_dry_run_refuses_and_swaps_nothing() {
    let server = untrusted_release_server().await;
    let bin_path = assert_cmd::cargo::cargo_bin("unblock");
    let before = std::fs::read(&bin_path).expect("read binary before");

    // `--dry-run` must also never swap; against an unconfigured updater it is refused (exit 1) and the
    // binary is untouched — a dry-run can only ever report, never act.
    let out = unblock()
        .args(["update", "--dry-run", "--output", "json"])
        .env(GHE_BASE_URL_ENV, server.uri())
        .output()
        .expect("run `unblock update --dry-run`");
    assert_ne!(
        out.status.code(),
        Some(0),
        "dry-run against an unconfigured updater is refused"
    );

    let after = std::fs::read(&bin_path).expect("read binary after");
    assert_eq!(before, after, "--dry-run never swaps the binary");
}
