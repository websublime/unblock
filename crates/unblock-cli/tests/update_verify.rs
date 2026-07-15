//! `unblock update` no-swap safety gates (FR-25/D17/NFR-17, CF-K) — the client-side half of the
//! self-update integrity story.
//!
//! ## What these tests PROVE (unblock's client-side half ONLY)
//!
//! `unblock update` calls `AxoUpdater::new_for("unblock-cli")` then `load_receipt()` (G1). The receipt
//! (`unblock-cli-receipt.json`, App-name = the `unblock-cli` package, ci-cd §3.1) supplies THIS binary's
//! release source + installed version. Two invariants are pinned here:
//!
//! 1. **Refusal / no-swap (any platform).** A copy with NO install receipt (`cargo install` / raw
//!    download) is not eligible: `load_receipt()` → `NoReceipt` → the command REFUSES **before any
//!    network I/O**, surfacing as `CliError::Update → InternalError` (exit 1) with a valid structured
//!    error on stdout (FR-11), and the on-disk `unblock` binary is **byte-identical** (nothing swapped).
//!    The refusal suites force `NoReceipt` deterministically with an EMPTY `AXOUPDATER_CONFIG_PATH` dir
//!    (hermeticity — a dev machine with a real receipt would otherwise make them non-hermetic).
//!
//! 2. **Tampered-download rejection / no-swap (`#[cfg(unix)]`).** With a FABRICATED receipt the updater
//!    is driven PAST refusal to actual install-execution against a mock release source. The mock serves
//!    a newer release whose installer asset is a TAMPERED script that stands in for the dist installer
//!    aborting on a SHA256 mismatch: it writes a checksum-failure signal to stderr, `touch`es a sentinel
//!    (proving install-execution was REACHED, not the earlier `NoReceipt` refusal), then exits non-zero.
//!    axoupdater surfaces that non-zero abort as `InstallFailed` → `CliError::Update` (exit 1) and the
//!    on-disk `unblock` binary stays byte-identical (no swap).
//!
//! ## The BOUNDARY (OQ-1 — read before trusting the assertion)
//!
//! Test (2) is a faithful STAND-IN that proves **unblock's client-side no-swap-on-installer-abort half
//! ONLY**: it drives the real `unblock update` CLI to install-execution and asserts a non-zero installer
//! abort → exit 1 → no swap. It does **NOT** re-test the dist installer's own SHA256 verification — that
//! is dist's code, covered by dist's own suite, plus `gh attestation verify` out-of-band on real signed
//! releases (publish-side provenance, NOT on the auto-update path, NFR-17). The real dist installer needs
//! real signed artifacts that exist only post-release, so it is not runnable hermetically pre-cut; the
//! live download+swap end-to-end is the human release runbook (a future v1.0.1 exercises it).
//!
//! ## Hermeticity
//!
//! Every case runs against per-CHILD `Command::env` overrides (parallel-safe; the suite never mutates
//! process-global env via the `unsafe`/edition-2024-forbidden `std::env::set_var`). The GitHub-Enterprise
//! API base is redirected to a wiremock via `UNBLOCK_CLI_INSTALLER_GHE_BASE_URL` — the env-var prefix
//! axoupdater derives from the receipt's `source.app_name` (`app_name_to_env_var("unblock-cli") ==
//! "UNBLOCK_CLI"`, verified against axoupdater 0.10.0 `release/github.rs::github_api`) — so no request can
//! ever reach real github.com. `--no-default-features` drops the whole command; the CI feature-matrix
//! build proves no network dependency is compiled without `self-update`.
//!
//! Gated on `self-update` (the default-on feature that carries the whole update surface).
#![cfg(feature = "self-update")]

mod common;

use common::unblock;
use serde_json::Value;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The dist App-name = the `unblock-cli` PACKAGE name (ci-cd §3.1 P2 corrective). It fixes the receipt
/// filename (`unblock-cli-receipt.json`) AND — via `source.app_name` → `app_name_to_env_var` — the
/// `UNBLOCK_CLI_*` env-var prefix axoupdater keys the GHE/GitHub base-URL overrides off.
const APP_NAME: &str = "unblock-cli";

/// The env var (correct prefix for App-name `unblock-cli`) that redirects axoupdater's GitHub-Enterprise
/// API base so a releases query — IF the code path reached the network — hits the mock, never real
/// github.com (axoupdater joins `api/v3` onto it, GHE style).
const GHE_BASE_URL_ENV: &str = "UNBLOCK_CLI_INSTALLER_GHE_BASE_URL";

/// Stand up a mock "release source" that returns a non-release payload for every releases query — kept
/// as a hermeticity guard for the REFUSAL suites (which refuse at `load_receipt()` before any network
/// I/O, so it is never actually queried on that path; the redirect just guarantees the child can never
/// reach real github.com even if the refusal ever regressed).
async fn untrusted_release_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(wiremock::matchers::path_regex(r".*/releases.*"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({ "tampered": true })),
        )
        .mount(&server)
        .await;
    server
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_refuses_a_receiptless_binary_and_swaps_nothing() {
    let server = untrusted_release_server().await;
    // An EMPTY receipt dir → `load_receipt()` finds no `unblock-cli-receipt.json` → deterministic
    // `NoReceipt`, independent of any real receipt on the dev/CI machine (hermeticity).
    let empty_config = tempfile::tempdir().expect("empty config tempdir");

    // Snapshot the on-disk binary bytes BEFORE the update attempt (the no-swap safety invariant).
    let bin_path = assert_cmd::cargo::cargo_bin("unblock");
    let before = std::fs::read(&bin_path).expect("read the unblock binary before");

    // Drive the REAL `unblock update` child. With no receipt it refuses at `load_receipt()` BEFORE any
    // network I/O — no swap can occur.
    let out = unblock()
        .args(["update", "--output", "json"])
        .env("AXOUPDATER_CONFIG_PATH", empty_config.path())
        .env_remove("AXOUPDATER_CONFIG_WORKING_DIR")
        .env(GHE_BASE_URL_ENV, server.uri())
        .output()
        .expect("run `unblock update`");

    // Refused: exit 1 with a VALID structured error on stdout (FR-11). The whole update surface maps to
    // InternalError (exit 1) — never a silent success from a non-eligible (receiptless) binary.
    assert_eq!(
        out.status.code(),
        Some(1),
        "a receiptless binary must be REFUSED (exit 1); stderr: {}",
        String::from_utf8_lossy(&out.stderr)
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
async fn update_dry_run_refuses_a_receiptless_binary_and_swaps_nothing() {
    let server = untrusted_release_server().await;
    let empty_config = tempfile::tempdir().expect("empty config tempdir");
    let bin_path = assert_cmd::cargo::cargo_bin("unblock");
    let before = std::fs::read(&bin_path).expect("read binary before");

    // `--dry-run` must also never swap; against a receiptless binary it is refused (exit 1) and the
    // binary is untouched — a dry-run can only ever report, never act.
    let out = unblock()
        .args(["update", "--dry-run", "--output", "json"])
        .env("AXOUPDATER_CONFIG_PATH", empty_config.path())
        .env_remove("AXOUPDATER_CONFIG_WORKING_DIR")
        .env(GHE_BASE_URL_ENV, server.uri())
        .output()
        .expect("run `unblock update --dry-run`");
    assert_eq!(
        out.status.code(),
        Some(1),
        "dry-run against a receiptless binary is refused; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let after = std::fs::read(&bin_path).expect("read binary after");
    assert_eq!(before, after, "--dry-run never swaps the binary");
}

/// The tampered-download rejection test (D7). Drives `unblock update` PAST refusal to install-execution
/// and proves the client-side no-swap-on-installer-abort invariant. `#[cfg(unix)]`: the tampered
/// installer is a `#!/bin/sh` script (axoupdater execs the downloaded script path DIRECTLY via
/// `Cmd::new(path)`, not `sh script`, so the shebang is load-bearing — a missing one ENOEXECs before the
/// failure branch). See the module BOUNDARY note: this does NOT re-test dist's own SHA256.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_rejects_a_tampered_download_and_swaps_nothing() {
    let config_dir = tempfile::tempdir().expect("config tempdir");

    // The running `unblock` binary + the receipt's install_prefix. `check_receipt_is_for_this_executable`
    // compares the canonicalized parent dir of the running exe against the canonicalized receipt prefix;
    // they MUST match or the run reports "up to date" without reaching install-exec (sentinel absent).
    let bin_path = assert_cmd::cargo::cargo_bin("unblock");
    let canonical_bin = std::fs::canonicalize(&bin_path).expect("canonicalize unblock binary");
    let install_prefix = canonical_bin.parent().expect("binary parent dir");

    // A FABRICATED dist install receipt: a LOW installed version (so the mock's newer release is
    // "needed"), a github source pointing at the mock's repo coordinates, and provider `cargo-dist`
    // 0.32.0 (> 0.15.0, so axoupdater's historical `bin`-strip workaround does NOT apply). Field set
    // verified against axoupdater 0.10.0 `receipt.rs::InstallReceipt`.
    let receipt = serde_json::json!({
        "install_prefix": install_prefix.to_string_lossy(),
        "binaries": ["unblock"],
        "cdylibs": [],
        "source": {
            "release_type": "github",
            "owner": "websublime",
            "name": "unblock",
            "app_name": APP_NAME,
        },
        "version": "0.0.1",
        "provider": { "source": "cargo-dist", "version": "0.32.0" },
        "modify_path": false,
    });
    let receipt_path = config_dir.path().join(format!("{APP_NAME}-receipt.json"));
    std::fs::write(
        &receipt_path,
        serde_json::to_vec(&receipt).expect("serialize receipt"),
    )
    .expect("write receipt");

    // The mock release source. axoupdater (GHE style) queries
    // `<base>/api/v3/repos/<owner>/<name>/releases/latest`, so mount that exact path.
    let server = MockServer::start().await;

    // A sentinel the tampered installer `touch`es to PROVE it reached install-execution (an absolute
    // path — the installer's cwd is unspecified).
    let sentinel = config_dir.path().join("installer-ran.sentinel");
    // The tampered installer: exec'd DIRECTLY by axoupdater, so it carries a `#!/bin/sh` shebang
    // (axoupdater sets the exec bit on the downloaded temp file). It writes the checksum-failure signal
    // to STDERR ONLY — the child `unblock`'s stdout is INHERITED, so writing there would corrupt the
    // CLI's own JSON stdout — touches the sentinel, then aborts non-zero (as the real installer does on
    // a SHA256 mismatch).
    let installer_script = format!(
        "#!/bin/sh\n\
         echo 'checksum mismatch: refusing to install a tampered artifact' 1>&2\n\
         : > '{}'\n\
         exit 1\n",
        sentinel.display()
    );

    let download_url = format!("{}/download/{APP_NAME}-installer.sh", server.uri());
    // A newer release (v1.0.0 > receipt 0.0.1) whose single installer asset points back at the mock.
    let release_body = serde_json::json!({
        "tag_name": "v1.0.0",
        "name": "1.0.0",
        "url": format!("{}/releases/v1.0.0", server.uri()),
        "assets": [{
            "url": download_url,
            "browser_download_url": download_url,
            "name": format!("{APP_NAME}-installer.sh"),
        }],
        "prerelease": false,
    });
    Mock::given(method("GET"))
        .and(path("/api/v3/repos/websublime/unblock/releases/latest"))
        .respond_with(ResponseTemplate::new(200).set_body_json(release_body))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/download/{APP_NAME}-installer.sh")))
        .respond_with(ResponseTemplate::new(200).set_body_string(installer_script))
        .mount(&server)
        .await;

    // Snapshot the on-disk binary bytes BEFORE (the NO-SWAP invariant).
    let before = std::fs::read(&bin_path).expect("read the unblock binary before");

    // Drive the REAL `unblock update` child PAST refusal to install-execution.
    let out = unblock()
        .args(["update", "--output", "json"])
        // Where axoupdater reads `unblock-cli-receipt.json`.
        .env("AXOUPDATER_CONFIG_PATH", config_dir.path())
        .env_remove("AXOUPDATER_CONFIG_WORKING_DIR")
        // Redirect the GHE API base to the mock (prefix derived from the receipt's `source.app_name`).
        .env(GHE_BASE_URL_ENV, server.uri())
        // Guard against the mutually-exclusive plain-GitHub override leaking from the environment.
        .env_remove("UNBLOCK_CLI_INSTALLER_GITHUB_BASE_URL")
        // Not consulted on our path (only `new_for_updater_executable` reads it, and G1 does not use
        // that); set for documentation/robustness so the App-name is unambiguous end-to-end.
        .env("AXOUPDATER_APP_NAME", APP_NAME)
        .output()
        .expect("run `unblock update`");

    // (1) REACHED install-execution — the sentinel exists (proving this is NOT the earlier `NoReceipt`
    // refusal, which never runs an installer).
    assert!(
        sentinel.exists(),
        "the tampered installer must have RUN (sentinel present) — proving the update reached \
         install-execution, not the earlier NoReceipt refusal.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    // (2) REFUSED: exit 1 with a valid structured error on stdout (FR-11). The installer's non-zero abort
    // surfaces as `InstallFailed` → `CliError::Update` → InternalError (exit 1). The installer wrote ONLY
    // to stderr, so stdout carries just the CLI's JSON.
    assert_eq!(
        out.status.code(),
        Some(1),
        "a tampered-download install failure maps to InternalError (exit 1)"
    );
    let value: Value =
        serde_json::from_slice(&out.stdout).expect("valid JSON structured error on stdout (FR-11)");
    assert_eq!(value["code"], "INTERNAL_ERROR", "self-update failure code");

    // (3) The checksum-failure signal reached the child's INHERITED stderr. axoupdater inherits the
    // installer's stderr by default (`print_installer_stderr = true` → `Stdio::inherit()`), so
    // `InstallFailed { stderr: None }` and the text is NOT in the `CliError` message (MF-3) — assert on
    // the child's captured stderr instead.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("checksum mismatch"),
        "the installer's checksum-mismatch signal must reach the child's stderr; got: {stderr}"
    );

    // (4) The safety invariant: NO binary was swapped — the on-disk bytes are byte-identical.
    let after = std::fs::read(&bin_path).expect("read the unblock binary after");
    assert_eq!(
        before, after,
        "no unverified binary may be swapped in — the on-disk `unblock` must be byte-identical"
    );
}

/// Write a FABRICATED dist install receipt for the version-comparison dry-run cases. `install_prefix`
/// = the running exe's canonicalized parent so `check_receipt_is_for_this_executable()` PASSES (the
/// receipt-eligibility guard `is_update_needed()` applies, which dry-run must NOT skip); provider
/// cargo-dist 0.32.0 (> 0.15.0, so axoupdater's historical `bin`-strip workaround does not apply);
/// github source pointing at the mock's repo coordinates. Only `version` differs between cases,
/// driving the `current<latest` comparison. Field set verified against axoupdater 0.10.0
/// `receipt.rs::InstallReceipt` (mirrors the tampered-download fixture above).
#[cfg(unix)]
fn write_version_receipt(
    config_dir: &std::path::Path,
    install_prefix: &std::path::Path,
    version: &str,
) {
    let receipt = serde_json::json!({
        "install_prefix": install_prefix.to_string_lossy(),
        "binaries": ["unblock"],
        "cdylibs": [],
        "source": {
            "release_type": "github",
            "owner": "websublime",
            "name": "unblock",
            "app_name": APP_NAME,
        },
        "version": version,
        "provider": { "source": "cargo-dist", "version": "0.32.0" },
        "modify_path": false,
    });
    let receipt_path = config_dir.join(format!("{APP_NAME}-receipt.json"));
    std::fs::write(
        &receipt_path,
        serde_json::to_vec(&receipt).expect("serialize receipt"),
    )
    .expect("write receipt");
}

/// Stand up a mock GHE release source that serves a single latest stable release at `tag_name`,
/// carrying the `{APP_NAME}-installer.sh` asset `get_latest_stable_release` REQUIRES to treat the
/// release as installable (axoupdater 0.10.0 `release/github.rs:102`). The dry-run path only QUERIES
/// this release (`query_new_version()` → `fetch_release()`); it never downloads the asset, so no
/// download endpoint is mounted. axoupdater (GHE style) hits
/// `<base>/api/v3/repos/<owner>/<name>/releases/latest`, so mount that exact path.
#[cfg(unix)]
async fn latest_release_server(tag_name: &str) -> MockServer {
    let server = MockServer::start().await;
    let download_url = format!("{}/download/{APP_NAME}-installer.sh", server.uri());
    let release_body = serde_json::json!({
        "tag_name": tag_name,
        "name": tag_name.trim_start_matches('v'),
        "url": format!("{}/releases/{tag_name}", server.uri()),
        "assets": [{
            "url": download_url,
            "browser_download_url": download_url,
            "name": format!("{APP_NAME}-installer.sh"),
        }],
        "prerelease": false,
    });
    Mock::given(method("GET"))
        .and(path("/api/v3/repos/websublime/unblock/releases/latest"))
        .respond_with(ResponseTemplate::new(200).set_body_json(release_body))
        .mount(&server)
        .await;
    server
}

/// Drive a `--dry-run` against a mock whose latest release is `tag_name`, with the receipt fabricated
/// at `receipt_version`. Returns the child's captured stderr (where `output::diag` writes, NFR-14),
/// after asserting exit 0 and the NO-SWAP safety invariant (the on-disk `unblock` is byte-identical —
/// a dry-run can only ever report, never act). `#[cfg(unix)]`: reuses the receipt-fixture + wiremock
/// setup and the running exe's parent as `install_prefix`.
#[cfg(unix)]
async fn dry_run_stderr(receipt_version: &str, tag_name: &str) -> String {
    let config_dir = tempfile::tempdir().expect("config tempdir");

    // The running `unblock` binary + the receipt's install_prefix. `check_receipt_is_for_this_executable`
    // compares the canonicalized parent dir of the running exe against the canonicalized receipt prefix;
    // they MUST match or `is_update_needed()` short-circuits to "not needed" WITHOUT reaching the version
    // comparison — so the verdict here comes from `current<latest`, not a prefix mismatch.
    let bin_path = assert_cmd::cargo::cargo_bin("unblock");
    let canonical_bin = std::fs::canonicalize(&bin_path).expect("canonicalize unblock binary");
    let install_prefix = canonical_bin.parent().expect("binary parent dir");

    write_version_receipt(config_dir.path(), install_prefix, receipt_version);
    let server = latest_release_server(tag_name).await;

    // Snapshot the on-disk binary bytes BEFORE (the NO-SWAP invariant).
    let before = std::fs::read(&bin_path).expect("read the unblock binary before");

    let out = unblock()
        .args(["update", "--dry-run", "--output", "json"])
        // Where axoupdater reads `unblock-cli-receipt.json`.
        .env("AXOUPDATER_CONFIG_PATH", config_dir.path())
        .env_remove("AXOUPDATER_CONFIG_WORKING_DIR")
        // Redirect the GHE API base to the mock (prefix derived from the receipt's `source.app_name`).
        .env(GHE_BASE_URL_ENV, server.uri())
        // Guard against the mutually-exclusive plain-GitHub override leaking from the environment.
        .env_remove("UNBLOCK_CLI_INSTALLER_GITHUB_BASE_URL")
        .env("AXOUPDATER_APP_NAME", APP_NAME)
        .output()
        .expect("run `unblock update --dry-run`");

    // A dry-run against an ELIGIBLE receipt succeeds (exit 0) whether or not an update exists — it only
    // reports, it never fails the process on the happy path.
    assert_eq!(
        out.status.code(),
        Some(0),
        "an eligible `--dry-run` exits 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // The safety invariant: NO binary was swapped — the on-disk bytes are byte-identical.
    let after = std::fs::read(&bin_path).expect("read the unblock binary after");
    assert_eq!(before, after, "--dry-run never swaps the binary");

    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Dry-run "already up to date" (D7 hermetic). The fabricated receipt's `version` EQUALS the mock's
/// latest release (1.0.0) → `query_new_version()` caches that release, then `is_update_needed()` reuses
/// it and evaluates `1.0.0 < 1.0.0 == false` → `--dry-run` reports "already up to date" on STDERR
/// (`output::diag` → stderr, NFR-14) and exits 0 with NO swap. This covers the previously-DEAD
/// up-to-date branch: before MF-P2-1, `query_new_version()` ALWAYS returned `Some(..)` (it never
/// compared against the current version), so the happy path always LIED "update available".
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_dry_run_reports_up_to_date_when_receipt_matches_latest() {
    let stderr = dry_run_stderr("1.0.0", "v1.0.0").await;
    assert!(
        stderr.contains("already up to date"),
        "dry-run on the latest version must report 'already up to date' on stderr (NFR-14); got: {stderr}"
    );
    assert!(
        !stderr.contains("update available"),
        "dry-run on the latest version must NOT claim an update is available; got: {stderr}"
    );
}

/// Dry-run "update available" (D7 hermetic). The fabricated receipt's `version` (0.0.1) is BEHIND the
/// mock's latest release (1.0.0) → `is_update_needed()` evaluates `0.0.1 < 1.0.0 == true` → `--dry-run`
/// reports "update available: 1.0.0" on STDERR (the semver parsed from tag `v1.0.0`) and exits 0 with
/// NO swap. Together with the up-to-date case this proves dry-run now honors `is_update_needed()`'s
/// eligibility + `current<latest` guard instead of unconditionally claiming an update.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_dry_run_reports_available_when_receipt_is_behind_latest() {
    let stderr = dry_run_stderr("0.0.1", "v1.0.0").await;
    assert!(
        stderr.contains("update available: 1.0.0"),
        "dry-run behind the latest must report 'update available: 1.0.0' on stderr (NFR-14); got: {stderr}"
    );
    assert!(
        !stderr.contains("already up to date"),
        "dry-run behind the latest must NOT report 'already up to date'; got: {stderr}"
    );
}
