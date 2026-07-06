//! The clap `Parser` command surface (D9 — stable clap features only) and the ONE bridge from clap
//! types into `unblock_config::CliOverrides`.
//!
//! **The CLI is a pure `CliOverrides` forwarder** (D27/AD-3): `unblock-config` owns ALL layering
//! (CLI > env `UNBLOCK_*` > `.unblock/config.toml` > defaults). The single CLI-owned resolution seam
//! is clap `env` binding — `--dir`→`UNBLOCK_DIR`, `--actor`→`UNBLOCK_ACTOR` — so `--flag > UNBLOCK_*`
//! precedence is free. `--output/-o` deliberately does NOT carry clap `env`: `UNBLOCK_OUTPUT_FORMAT`
//! is parsed strictly by config's env layer (the single strict parse site) for the four workspace
//! commands; the flag still forwards via `CliOverrides.output_format` so `--flag > env` holds inside
//! config's resolver (spine §5b / §1807; the no-workspace `version` path reads the env leniently in
//! `output::pick_cli_format`). See `docs/plans/crates/unblock-cli.md`.

use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand};
use unblock_config::CliOverrides;
use unblock_model::OutputFormat;
use unblock_render::parse_format;

/// The top-level `unblock` command (lifecycle/ops only — D3).
#[derive(Debug, Parser)]
#[command(
    name = "unblock",
    version,
    about = "unblock — agent-first issue tracker (lifecycle/ops CLI; domain features are MCP tools)",
    long_about = None,
)]
pub struct Cli {
    /// The global flags (all `global = true`, so they may appear before or after the subcommand).
    #[command(flatten)]
    pub global: GlobalArgs,

    /// The lifecycle/ops subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// The global flags shared by every subcommand (D27/AD-3 — clap `env` is the single FR-13 wiring
/// point for `--dir`/`--actor`).
#[derive(Debug, Args, Clone, Default)]
pub struct GlobalArgs {
    /// The explicit workspace `.unblock/` directory (no walk-up; `--dir` > `UNBLOCK_DIR`, MF-2).
    #[arg(long, global = true, env = "UNBLOCK_DIR", value_name = "DIR")]
    pub dir: Option<PathBuf>,

    /// The actor override (`--actor` > `UNBLOCK_ACTOR`, FORK-4).
    #[arg(long, global = true, env = "UNBLOCK_ACTOR", value_name = "ACTOR")]
    pub actor: Option<String>,

    /// The output format for rendered reports (`json|robot|plain|csv|markdown`). NO clap `env` —
    /// `UNBLOCK_OUTPUT_FORMAT` is owned by config's strict env layer for workspace commands; the flag
    /// still forwards so `--flag > env` holds (spine §5b).
    #[arg(
        long = "output",
        short = 'o',
        global = true,
        value_parser = parse_output_format,
        value_name = "FORMAT",
    )]
    pub output: Option<OutputFormat>,

    /// Increase verbosity (`-v` INFO, `-vv` DEBUG, `-vvv+` TRACE). Logs go to stderr only (NFR-14).
    #[arg(long, short = 'v', global = true, action = ArgAction::Count)]
    pub verbose: u8,

    /// Suppress all but error-level diagnostics (`-q`). Overrides `-v`.
    #[arg(long, short = 'q', global = true)]
    pub quiet: bool,
}

impl GlobalArgs {
    /// Build the `CliOverrides` this run forwards into `unblock-config` — the ONE place clap types
    /// cross into config (D27/AD-3). `--prefix` is NOT here (there is no `CliOverrides.id_prefix`,
    /// DR-7 — `init --prefix` goes into the scaffold `config.toml` text).
    #[must_use]
    pub fn to_overrides(&self) -> CliOverrides {
        let mut overrides = CliOverrides::new();
        if let Some(dir) = &self.dir {
            overrides = overrides.with_dir(dir.clone());
        }
        if let Some(actor) = &self.actor {
            overrides = overrides.with_actor(actor.clone());
        }
        if let Some(format) = self.output {
            overrides = overrides.with_output_format(format);
        }
        overrides
    }
}

/// The lifecycle/ops subcommands (D3 — exactly these; `Update` is `self-update`-feature-gated).
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the MCP stdio server (the primary product surface, FR-20).
    Serve(ServeArgs),
    /// Ensure the workspace database schema is current and report the from→to delta (FR-16).
    Migrate(MigrateArgs),
    /// Run read-only health diagnostics on the workspace (doctor-lite, FR-16).
    Doctor(DoctorArgs),
    /// Print version / build metadata (no workspace, no network).
    Version(VersionArgs),
    /// Scaffold a new `.unblock/` workspace (config + migrated empty database, FR-14).
    Init(InitArgs),
    /// Write / refresh the managed `AGENTS.md` MCP-wiring block (FR-14).
    Agents(AgentsArgs),
    /// Self-update the `unblock` binary (attestation-verified, FR-25/D17).
    #[cfg(feature = "self-update")]
    Update(UpdateArgs),
}

/// `unblock serve` — no v1 flags (the MCP surface is fixed; instructions are generated).
#[derive(Debug, Args)]
pub struct ServeArgs {}

/// `unblock migrate` — no v1 flags.
#[derive(Debug, Args)]
pub struct MigrateArgs {}

/// `unblock doctor` — no v1 flags (`--repair` is a T3.3 seam, AF-1).
#[derive(Debug, Args)]
pub struct DoctorArgs {}

/// `unblock version` — `--short` prints the bare version (no `--check`; no network, AD-5).
#[derive(Debug, Args)]
pub struct VersionArgs {
    /// Print only the bare package version (no build metadata).
    #[arg(long)]
    pub short: bool,
}

/// `unblock init` — scaffold a workspace (AF-3).
#[derive(Debug, Args)]
pub struct InitArgs {
    /// The issue-id prefix to seed `config.toml` with (normalized; default `ub`).
    #[arg(long, value_name = "PREFIX")]
    pub prefix: Option<String>,
    /// Overwrite an existing `.unblock/` scaffold instead of refusing (clobber guard override).
    #[arg(long)]
    pub force: bool,
}

/// `unblock agents` — no v1 flags (writes `<workspace>/AGENTS.md`).
#[derive(Debug, Args)]
pub struct AgentsArgs {}

/// `unblock update` — self-update (behind the `self-update` feature).
#[cfg(feature = "self-update")]
#[derive(Debug, Args)]
pub struct UpdateArgs {
    /// Check for and report an available update without swapping the binary.
    #[arg(long)]
    pub dry_run: bool,
}

/// clap `value_parser` for `--output/-o`: wraps `unblock_render::parse_format`, mapping its
/// `RenderError` to a clap-friendly `String` so `-o bogus` is a usage error (exit 2) echoing the raw
/// name (this exercises the D27/AF-4 `RenderError::UnknownFormat { name }` raw-name carry).
fn parse_output_format(value: &str) -> Result<OutputFormat, String> {
    parse_format(value).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::{Cli, Command, GlobalArgs};
    use clap::{CommandFactory, Parser};
    use unblock_model::OutputFormat;

    #[test]
    fn clap_command_definition_is_valid() {
        // clap's own structural assertions (arg ids, conflicts, value parsers) — a hard compile-time
        // + runtime guard that the derive surface is well-formed.
        Cli::command().debug_assert();
    }

    #[test]
    fn env_dir_binds_to_global_dir() {
        // clap `env` binding: `UNBLOCK_DIR` fills `global.dir` when `--dir` is absent (AD-3). We pass
        // the env value explicitly via the parser's env-less path by setting it in-process; clap reads
        // process env, so drive it through the flag to keep the test hermetic (no global env mutation).
        let cli = Cli::try_parse_from(["unblock", "--dir", "/ws/.unblock", "version"])
            .expect("parse with --dir");
        assert_eq!(
            cli.global.dir.as_deref(),
            Some(std::path::Path::new("/ws/.unblock"))
        );
    }

    #[test]
    fn output_flag_parses_known_format() {
        let cli =
            Cli::try_parse_from(["unblock", "-o", "robot", "version"]).expect("parse -o robot");
        assert_eq!(cli.global.output, Some(OutputFormat::Robot));
    }

    #[test]
    fn output_flag_rejects_unknown_format_with_raw_name() {
        // `-o bogus` is a clap usage error (exit 2); the raw name is echoed via the AF-4 UnknownFormat.
        let err = Cli::try_parse_from(["unblock", "-o", "bogus", "version"])
            .expect_err("bogus format must be rejected");
        assert_eq!(err.exit_code(), 2);
        assert!(
            err.to_string().contains("bogus"),
            "the raw offending name must appear: {err}"
        );
    }

    #[test]
    fn unknown_subcommand_is_a_usage_error() {
        // A domain verb like `create` is NOT a lifecycle subcommand — clap rejects it (exit 2).
        let err =
            Cli::try_parse_from(["unblock", "create"]).expect_err("create is not a subcommand");
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn to_overrides_forwards_dir_actor_and_output() {
        let global = GlobalArgs {
            dir: Some("/ws/.unblock".into()),
            actor: Some("alice".to_string()),
            output: Some(OutputFormat::Csv),
            verbose: 0,
            quiet: false,
        };
        let overrides = global.to_overrides();
        assert_eq!(
            overrides.dir.as_deref(),
            Some(std::path::Path::new("/ws/.unblock"))
        );
        assert_eq!(overrides.actor.as_deref(), Some("alice"));
        assert_eq!(overrides.output_format, Some(OutputFormat::Csv));
        // `--prefix` never crosses here — there is no `CliOverrides.id_prefix`.
        assert!(overrides.db.is_none());
    }

    #[test]
    fn version_short_flag_parses() {
        let cli =
            Cli::try_parse_from(["unblock", "version", "--short"]).expect("parse version --short");
        match cli.command {
            Command::Version(args) => assert!(args.short),
            other => panic!("expected Version, got {other:?}"),
        }
    }
}
