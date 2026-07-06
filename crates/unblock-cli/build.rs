//! Build-time metadata capture for `unblock version` (D27/AD-5, NFR-6).
//!
//! Emits `UNBLOCK_BUILD_*` env vars from cargo-provided build env only — **no git invocation, no
//! network, no `vergen` build-dep, no `rustc -V` subprocess**. The git sha / rustc semver come from
//! `option_env!("VERGEN_*")` (or a CI-injected `UNBLOCK_BUILD_*`) at compile time inside `version.rs`;
//! when absent they are simply `None` (never an error). Honoring the same `VERGEN_*` env names bd used
//! gives free CI compatibility.
//!
//! `#![forbid(unsafe_code)]` applies transitively; this file runs on the host at build time.

use std::env;

fn main() {
    // Cargo-provided build env (always present in a cargo build): the compile profile + target triple.
    // `PROFILE` is "debug"/"release"; `TARGET` is the target triple.
    if let Ok(profile) = env::var("PROFILE") {
        println!("cargo:rustc-env=UNBLOCK_BUILD_PROFILE={profile}");
    }
    if let Ok(target) = env::var("TARGET") {
        println!("cargo:rustc-env=UNBLOCK_BUILD_TARGET={target}");
    }

    // Optional metadata: re-inject the CI/vergen-provided values as `UNBLOCK_BUILD_*` so `version.rs`
    // can read them uniformly via `option_env!("UNBLOCK_BUILD_*")`. A missing source is NOT an error —
    // the pass-through only emits when the source env is set at build time (absent => `version.rs`
    // reads `None`). NO `Command::new("git")`, NO git crate, NO network (NFR-6/D27/AD-5).
    if let Some(commit) = first_env(&["UNBLOCK_BUILD_COMMIT", "VERGEN_GIT_SHA"]) {
        println!("cargo:rustc-env=UNBLOCK_BUILD_COMMIT={commit}");
    }
    if let Some(rustc) = first_env(&["UNBLOCK_BUILD_RUSTC", "VERGEN_RUSTC_SEMVER"]) {
        println!("cargo:rustc-env=UNBLOCK_BUILD_RUSTC={rustc}");
    }

    // Rebuild `version` when a CI re-injects any of the optional metadata env vars.
    for var in [
        "UNBLOCK_BUILD_COMMIT",
        "UNBLOCK_BUILD_RUSTC",
        "VERGEN_GIT_SHA",
        "VERGEN_RUSTC_SEMVER",
    ] {
        println!("cargo:rerun-if-env-changed={var}");
    }
}

/// Return the first non-empty value among `names` read from the build environment (or `None`).
fn first_env(names: &[&str]) -> Option<String> {
    names
        .iter()
        .filter_map(|name| env::var(name).ok())
        .find(|value| !value.trim().is_empty())
}
