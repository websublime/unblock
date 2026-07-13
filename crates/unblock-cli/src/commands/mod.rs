//! Command handlers — one module per lifecycle/ops subcommand (D3). Each `run` returns
//! `Result<Option<u8>, CliError>` (`Some(128+signo)` = an mcp signal exit; `None` = success 0).

pub mod agents;
pub mod doctor;
pub mod init;
pub mod mcp;
pub mod migrate;
pub mod version;

#[cfg(feature = "self-update")]
pub mod update;
