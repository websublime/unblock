//! Helpers shared by the unblock-sync integration tests (PRD §4 D5 amendment, `ub-c15`).

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod fd_witness;
