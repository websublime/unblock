//! `unblock` — process entry point for the lifecycle/ops CLI. Real routing (a tokio runtime
//! driving `unblock_cli::run()` returning an `ExitCode`) lands at T3.1. See
//! `docs/plans/crates/unblock-cli.md`.
#![forbid(unsafe_code)]

fn main() {
    // Lifecycle/ops surface (serve/migrate/doctor/version/init/agents/update) is implemented at T3.1.
}
