//! `unblock update` attestation / verification gate (FR-25/D17/NFR-17, CF-K), wiremock-backed.
//!
//! The self-update path must NEVER swap in an unattested / unverifiable binary. `unblock update` is
//! wired to `axoupdater::AxoUpdater::new_for_updater_executable`, which trusts ONLY the release source
//! baked into a genuine `dist` install — there is no such install in a test harness. So an update run
//! that is pointed at an UNTRUSTED release source (a `wiremock` mock standing in for GitHub Releases)
//! is REJECTED before any swap: the updater is "not properly configured" for THIS executable, which
//! surfaces as a `CliError::Update → InternalError` (exit 1) with a valid structured error on stdout.
//! The critical safety invariant asserted here: **the on-disk `unblock` binary is byte-identical
//! after the rejected update** — no unattested artifact was ever swapped in.
//!
//! The mock release source is redirected per-CHILD via `Command::env` (parallel-safe; the suite never
//! calls the `unsafe`/edition-2024-forbidden `std::env::set_var`). `--no-default-features` drops the
//! command entirely — proven by the CI feature-matrix build (`cargo build -p unblock-cli
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
/// GitHub-Enterprise API base so a request goes to the mock, not real github.com.
const GHE_BASE_URL_ENV: &str = "UNBLOCK_INSTALLER_GHE_BASE_URL";

/// Stand up a mock "release source" that returns a TAMPERED / non-release payload for every releases
/// query — an unverifiable source. Any real fetch against it must be refused, never trusted.
async fn tampered_release_server() -> MockServer {
    let server = MockServer::start().await;
    // Every GHE releases endpoint returns a body that is NOT a valid, attested release (a bare object
    // with no installer assets) — an untrusted artifact the updater must not act on.
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
async fn update_rejects_an_unverifiable_release_and_swaps_nothing() {
    let server = tampered_release_server().await;

    // Snapshot the on-disk binary bytes BEFORE the update attempt (the no-swap safety invariant).
    let bin_path = assert_cmd::cargo::cargo_bin("unblock");
    let before = std::fs::read(&bin_path).expect("read the unblock binary before");

    // Drive the REAL `unblock update` child, redirecting its release source to the mock. The updater
    // is not configured for THIS (non-dist-installed) executable, so it refuses BEFORE any swap.
    let out = unblock()
        .args(["update", "--output", "json"])
        .env(GHE_BASE_URL_ENV, server.uri())
        .output()
        .expect("run `unblock update`");

    // Rejected: a non-zero exit with a VALID structured error on stdout (FR-11). The whole update
    // surface maps to InternalError (exit 1) — never a silent success against an untrusted source.
    assert_ne!(
        out.status.code(),
        Some(0),
        "an unverifiable update must be REJECTED"
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
        "no unattested binary may be swapped in — the on-disk `unblock` must be byte-identical"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_dry_run_reports_without_swapping() {
    let server = tampered_release_server().await;
    let bin_path = assert_cmd::cargo::cargo_bin("unblock");
    let before = std::fs::read(&bin_path).expect("read binary before");

    // `--dry-run` must also never swap; against an unverifiable source it is refused (exit 1) and the
    // binary is untouched — a dry-run can only ever report, never act.
    let out = unblock()
        .args(["update", "--dry-run", "--output", "json"])
        .env(GHE_BASE_URL_ENV, server.uri())
        .output()
        .expect("run `unblock update --dry-run`");
    assert_ne!(
        out.status.code(),
        Some(0),
        "dry-run against an unverifiable source is refused"
    );

    let after = std::fs::read(&bin_path).expect("read binary after");
    assert_eq!(before, after, "--dry-run never swaps the binary");
}
