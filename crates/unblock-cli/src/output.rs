//! CLI-local lifecycle report structs + the to-`DiagnosticReport` adapter + the render/emit path
//! (D27/AD-2) and the format resolver for the no-workspace path (D27/AD-3, SF-1).
//!
//! The three report structs (`VersionReport`/`MigrateReport`/`InitReport`) are
//! CLI-PRIVATE (deriving `serde::Serialize`; NOT spine §1.10 contract types — §6.1 binds only
//! re-exported §1.10 DTOs; the T2.1 render private-type precedent). Each maps onto a
//! `DiagnosticReport { kind, findings }` via [`ToDiagnosticReport`] and is rendered by
//! `Renderer::diagnostics` — the ONE live lifecycle-render path, all five formats, FR-11 uniform
//! (NOT a generic `render<T>`; the `Renderer` trait has no such method). *(The `doctor` command has NO
//! cli-local report at T3.3: it renders the wired `Session::doctor()` `DiagnosticReport` directly —
//! D29/F4.)*

use std::io::Write;
use std::path::PathBuf;

use serde::Serialize;
use unblock_config::EnvSource;
use unblock_engine::{DiagnosticFinding, DiagnosticKind, DiagnosticReport};
use unblock_render::{OutputFormat, RenderOptions, parse_env_value, renderer_for};

use crate::cli::GlobalArgs;
use crate::exit::{CliError, IoSnafu, RenderSnafu};
use snafu::ResultExt;

/// The `UNBLOCK_OUTPUT_FORMAT` env var name (config owns the strict parse for workspace commands; the
/// no-workspace path reads it leniently — SF-1).
const OUTPUT_FORMAT_ENV: &str = "UNBLOCK_OUTPUT_FORMAT";

/// `unblock version` report (bd-fidelity field set; `branch` intentionally dropped — SF-3).
#[derive(Debug, Clone, Serialize)]
pub struct VersionReport {
    /// The package version (`CARGO_PKG_VERSION`).
    pub version: String,
    /// The build profile (`debug`/`release`; from `build.rs`).
    pub build: String,
    /// The git commit sha (from `build.rs` `option_env!`; `None` when absent).
    pub commit: Option<String>,
    /// The rustc semver (from `build.rs` `option_env!`; `None` when absent).
    pub rustc: Option<String>,
    /// The target triple (from `build.rs`; `None` when absent).
    pub target: Option<String>,
    /// The enabled Cargo features (e.g. `self-update`).
    pub features: Vec<String>,
}

/// `unblock migrate` report — the real schema delta (D27/AF-2).
#[derive(Debug, Clone, Serialize)]
pub struct MigrateReport {
    /// The database file the migration ran against.
    pub database: PathBuf,
    /// The on-disk schema version BEFORE this migrate call.
    pub schema_from: i64,
    /// The on-disk schema version AFTER this migrate call.
    pub schema_to: i64,
    /// Whether the migrate advanced the schema (`schema_from != schema_to`).
    pub applied: bool,
}

/// The `unblock init` report names what `init` scaffolded (AF-3) and, under `--agents`, the
/// `AGENTS.md` it wrote.
#[derive(Debug, Clone, Serialize)]
pub struct InitReport {
    /// The project root that contains `.unblock/`.
    pub workspace_dir: PathBuf,
    /// The created `.unblock/` directory.
    pub unblock_dir: PathBuf,
    /// The migrated empty database path.
    pub db_path: PathBuf,
    /// The normalized issue-id prefix seeded into `config.toml`.
    pub id_prefix: String,
    /// The written `config.toml` path.
    pub config_path: PathBuf,
    /// The `AGENTS.md` path `init --agents` wrote, and `None` for a bare `init` (v1.1).
    pub agents_path: Option<PathBuf>,
}

/// Map a CLI-local report onto a `DiagnosticReport` for rendering (D27/AD-2). Each report reuses an
/// existing `DiagnosticKind` (no spine §1.10 change): `Version` → `DiagnosticKind::Version`; the rest
/// → `DiagnosticKind::Info` (model has no migrate/doctor/init kind).
pub trait ToDiagnosticReport {
    /// Build the `DiagnosticReport` this report renders as.
    fn to_report(&self) -> DiagnosticReport;
}

/// Build one finding row from a label + detail.
fn finding(label: impl Into<String>, detail: impl Into<String>) -> DiagnosticFinding {
    DiagnosticFinding {
        label: label.into(),
        detail: detail.into(),
    }
}

impl ToDiagnosticReport for VersionReport {
    fn to_report(&self) -> DiagnosticReport {
        let mut findings = vec![
            finding("version", self.version.clone()),
            finding("build", self.build.clone()),
        ];
        if let Some(commit) = &self.commit {
            findings.push(finding("commit", commit.clone()));
        }
        if let Some(rustc) = &self.rustc {
            findings.push(finding("rustc", rustc.clone()));
        }
        if let Some(target) = &self.target {
            findings.push(finding("target", target.clone()));
        }
        findings.push(finding("features", self.features.join(",")));
        DiagnosticReport {
            kind: DiagnosticKind::Version,
            findings,
        }
    }
}

impl ToDiagnosticReport for MigrateReport {
    fn to_report(&self) -> DiagnosticReport {
        DiagnosticReport {
            kind: DiagnosticKind::Info,
            findings: vec![
                finding("database", self.database.display().to_string()),
                finding("schema_from", self.schema_from.to_string()),
                finding("schema_to", self.schema_to.to_string()),
                finding("applied", self.applied.to_string()),
            ],
        }
    }
}

impl ToDiagnosticReport for InitReport {
    fn to_report(&self) -> DiagnosticReport {
        let mut findings = vec![
            finding("workspace_dir", self.workspace_dir.display().to_string()),
            finding("unblock_dir", self.unblock_dir.display().to_string()),
            finding("db_path", self.db_path.display().to_string()),
            finding("config_path", self.config_path.display().to_string()),
            finding("id_prefix", self.id_prefix.clone()),
        ];
        if let Some(agents_path) = &self.agents_path {
            findings.push(finding("agents_path", agents_path.display().to_string()));
        }
        DiagnosticReport {
            kind: DiagnosticKind::Info,
            findings,
        }
    }
}

/// Render `report` in `fmt` and write the structured payload to STDOUT (all five formats via
/// `Renderer::diagnostics`).
///
/// **NORMATIVE CONSTRAINT (D48): only a command that owns stdout as its OWN report channel may call
/// this.** It writes to stdout UNCONDITIONALLY and takes no `StdoutRole`, so a command whose stdout
/// is a wire-protocol framing channel (`unblock mcp`) would leak a report onto that channel with no
/// compile error, no lint and no test. The hazard is LATENT, not live: `commands/mcp.rs` makes no
/// `output::` call, and all four callers here — `version`, `doctor`, `migrate`, `init` — own stdout
/// as reports. Threading the classification through this second seam is deliberately OUT of D48's
/// scope and is tracked as its own open issue, `ub-c5o` (PRD §4 D48 clause 6(iii)).
///
/// # Errors
/// - [`CliError::Render`] if the renderer fails, which only JSON serialization or the
///   feature-gated TOON placeholder can do;
/// - [`CliError::Io`] if writing to stdout fails.
pub fn emit_report(report: &DiagnosticReport, fmt: OutputFormat) -> Result<(), CliError> {
    let opts = RenderOptions::default();
    let out = renderer_for(fmt, opts.clone())
        .diagnostics(report, &opts)
        .context(RenderSnafu)?;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(out.stdout.as_bytes()).context(IoSnafu)?;
    // Diagnostic reports render without a trailing newline; add one so shell output is clean.
    stdout.write_all(b"\n").context(IoSnafu)?;
    Ok(())
}

/// Which pass of [`pick_cli_format`] is running (PRD §4 D57 clause (6)) — a named type, deliberately
/// NOT a "role", which in this crate already means [`StdoutRole`](crate::exit::StdoutRole).
///
/// Only one pass may warn about a user file that fails to load, so the failure is reported exactly
/// once on every path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FormatPass {
    /// The pre-pass `run_with` (`lib.rs`) runs before dispatching EVERY command, to pick the format
    /// its structured error would use. It skips a failing user tier SILENTLY, always: a workspace
    /// command then fails on the same file with exit 7 and the full error.
    ErrorPrePass,
    /// A command's pass for its OWN report — today only `unblock version`, which never fails on the
    /// user file. It prints ONE stderr warning and skips the tier.
    OwnReport,
}

/// Resolve the output format for the NO-workspace path (`version`, the error pre-pass) — no
/// workspace is opened there, so FR-13 precedence is applied here: `--output` >
/// `UNBLOCK_OUTPUT_FORMAT` (read LENIENTLY via render's `parse_env_value`, SF-1) > the user file's
/// `output_format` (through `unblock_config::user_output_format`, so the cli never parses TOML) >
/// `Json` (PRD §4 D57 clause (6)). An unknown env value falls through to the next tier (never a hard
/// error on this path). A user file that fails to load is skipped, silently under
/// [`FormatPass::ErrorPrePass`] and with ONE stderr warning under [`FormatPass::OwnReport`].
#[must_use]
pub(crate) fn pick_cli_format(
    global: &GlobalArgs,
    env: &dyn EnvSource,
    pass: FormatPass,
) -> OutputFormat {
    pick_cli_format_to(global, env, pass, &mut std::io::stderr().lock())
}

/// [`pick_cli_format`] with the warning writer injected, so a unit test can read the warning.
fn pick_cli_format_to(
    global: &GlobalArgs,
    env: &dyn EnvSource,
    pass: FormatPass,
    warn_out: &mut impl Write,
) -> OutputFormat {
    if let Some(fmt) = global.output.or_else(|| {
        env.get(OUTPUT_FORMAT_ENV)
            .as_deref()
            .and_then(parse_env_value)
    }) {
        return fmt;
    }
    match unblock_config::user_output_format(env) {
        Ok(user) => user,
        Err(err) => {
            if pass == FormatPass::OwnReport {
                diag_to(&user_file_warning(&err), warn_out);
            }
            None
        }
    }
    .unwrap_or(OutputFormat::Json)
}

/// The ONE stderr line the [`FormatPass::OwnReport`] pass prints for a user file that fails to load.
/// The error's message is sanitised and its whitespace folded, so the warning is always exactly one
/// line whatever the path or message holds, and it never carries `error[`, because the command it
/// accompanies succeeds.
fn user_file_warning(err: &unblock_config::ConfigError) -> String {
    // NFR-14: through the ONE sanitising chokepoint before the fold, so no control byte from the
    // file's path or the parser's message reaches the terminal.
    let message = unblock_error::sanitize_message(&err.to_string()).into_owned();
    let folded: Vec<&str> = message.split_whitespace().collect();
    format!(
        "warning: the user config file was ignored for the output format: {}",
        folded.join(" ")
    )
}

/// Write a terse human note to STDERR (NFR-14). A failing stderr is ignored, so a note never
/// panics and never changes the exit code.
///
/// Its callers are the "wrote X" note of the shared `AGENTS.md` write (under `agents` and
/// `init --agents`), the next-step hint of a bare `init` and `update`'s notes. `diag` itself ignores
/// `-q`, so the "wrote X" note prints even under `-q`. `init` checks `-q` before it prints the hint.
pub fn diag(message: &str) {
    diag_to(message, &mut std::io::stderr().lock());
}

/// Writes `message` and one newline to `out`, ignoring a write error.
fn diag_to(message: &str, out: &mut impl Write) {
    let _ignored = writeln!(out, "{message}");
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fmt::Write as _;
    use std::path::PathBuf;

    use super::{
        FormatPass, InitReport, MigrateReport, ToDiagnosticReport, VersionReport, diag_to,
        pick_cli_format, pick_cli_format_to,
    };
    use crate::cli::GlobalArgs;
    use unblock_config::EnvSource;
    use unblock_engine::{DiagnosticFinding, DiagnosticKind, DiagnosticReport};
    use unblock_render::{OutputFormat, RenderOptions, renderer_for};

    /// A writer whose every write fails, as a stderr whose reader has exited does.
    struct ClosedWriter;

    impl std::io::Write for ClosedWriter {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
    }

    #[test]
    fn diag_to_writes_one_line_and_survives_a_failing_writer() {
        let mut out = Vec::new();
        diag_to("wrote /ws/AGENTS.md", &mut out);
        assert_eq!(out, b"wrote /ws/AGENTS.md\n", "one newline-terminated line");

        diag_to("wrote /ws/AGENTS.md", &mut ClosedWriter);
    }

    fn version_report() -> VersionReport {
        VersionReport {
            version: "0.1.0".to_string(),
            build: "debug".to_string(),
            commit: Some("abc123".to_string()),
            rustc: None,
            target: Some("aarch64-apple-darwin".to_string()),
            features: vec!["self-update".to_string()],
        }
    }

    #[test]
    fn version_report_maps_to_version_kind_with_findings() {
        let report = version_report().to_report();
        assert_eq!(report.kind, DiagnosticKind::Version);
        let labels: Vec<&str> = report.findings.iter().map(|f| f.label.as_str()).collect();
        assert!(labels.contains(&"version"));
        assert!(labels.contains(&"commit"));
        // `rustc` is None so it is omitted; `target` is Some so it is present.
        assert!(!labels.contains(&"rustc"));
        assert!(labels.contains(&"target"));
        assert!(labels.contains(&"features"));
    }

    #[test]
    fn migrate_report_maps_to_info_kind() {
        let report = MigrateReport {
            database: "/ws/.unblock/unblock.db".into(),
            schema_from: 1,
            schema_to: 1,
            applied: false,
        }
        .to_report();
        assert_eq!(report.kind, DiagnosticKind::Info);
        let applied = report
            .findings
            .iter()
            .find(|f| f.label == "applied")
            .expect("applied finding");
        assert_eq!(applied.detail, "false");
    }

    /// The labels of `report`'s findings, in order.
    fn labels(report: &DiagnosticReport) -> Vec<&str> {
        report.findings.iter().map(|f| f.label.as_str()).collect()
    }

    /// An `InitReport` for the workspace at `/ws`, with `agents_path` as given.
    fn init_report(agents_path: Option<&str>) -> InitReport {
        InitReport {
            workspace_dir: "/ws".into(),
            unblock_dir: "/ws/.unblock".into(),
            db_path: "/ws/.unblock/unblock.db".into(),
            id_prefix: "ub".to_string(),
            config_path: "/ws/.unblock/config.toml".into(),
            agents_path: agents_path.map(PathBuf::from),
        }
    }

    #[test]
    fn init_report_without_agents_keeps_its_five_rows() {
        let report = init_report(None).to_report();
        assert_eq!(
            labels(&report),
            [
                "workspace_dir",
                "unblock_dir",
                "db_path",
                "config_path",
                "id_prefix"
            ]
        );
    }

    #[test]
    fn init_report_with_agents_appends_agents_path_last() {
        let report = init_report(Some("/ws/AGENTS.md")).to_report();
        assert_eq!(
            labels(&report),
            [
                "workspace_dir",
                "unblock_dir",
                "db_path",
                "config_path",
                "id_prefix",
                "agents_path"
            ]
        );
        assert_eq!(
            report.findings.last().map(|f| f.detail.as_str()),
            Some("/ws/AGENTS.md"),
            "the last row names the AGENTS.md path"
        );
    }

    #[test]
    fn init_report_maps_to_info_kind() {
        let report = InitReport {
            workspace_dir: "/ws".into(),
            unblock_dir: "/ws/.unblock".into(),
            db_path: "/ws/.unblock/unblock.db".into(),
            id_prefix: "ub".to_string(),
            config_path: "/ws/.unblock/config.toml".into(),
            agents_path: None,
        }
        .to_report();
        assert_eq!(report.kind, DiagnosticKind::Info);
        let prefix = report
            .findings
            .iter()
            .find(|f| f.label == "id_prefix")
            .expect("id_prefix finding");
        assert_eq!(prefix.detail, "ub");
    }

    /// An injected environment (NFR-16): the format resolver never reads the process env here.
    struct MapEnv(HashMap<String, String>);

    impl MapEnv {
        fn new(pairs: &[(&str, &str)]) -> Self {
            Self(
                pairs
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                    .collect(),
            )
        }
    }

    impl EnvSource for MapEnv {
        fn get(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
    }

    /// A config home holding `unblock/config.toml` with `contents`.
    fn config_home(contents: &str) -> tempfile::TempDir {
        let home = tempfile::tempdir().expect("tempdir");
        let dir = home.path().join("unblock");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("config.toml"), contents).expect("write user file");
        home
    }

    /// Runs both passes over `env` and returns each pass's format and stderr.
    fn both_passes(global: &GlobalArgs, env: &MapEnv) -> [(OutputFormat, String); 2] {
        [FormatPass::ErrorPrePass, FormatPass::OwnReport].map(|pass| {
            let mut warn = Vec::new();
            let fmt = pick_cli_format_to(global, env, pass, &mut warn);
            (fmt, String::from_utf8(warn).expect("utf-8"))
        })
    }

    /// D57 clause (6): `--output` > `UNBLOCK_OUTPUT_FORMAT` > the user file's `output_format` >
    /// `Json`, each tier read from an injected environment.
    #[test]
    fn pick_cli_format_prefers_flag_then_env_then_user_then_json() {
        let home = config_home("output_format = \"markdown\"\n");
        let xdg = home.path().to_str().expect("utf-8 path");
        let with_flag = GlobalArgs {
            output: Some(OutputFormat::Csv),
            ..GlobalArgs::default()
        };
        let bare = GlobalArgs::default();
        let everything =
            MapEnv::new(&[("XDG_CONFIG_HOME", xdg), ("UNBLOCK_OUTPUT_FORMAT", "plain")]);
        let user_only = MapEnv::new(&[("XDG_CONFIG_HOME", xdg)]);
        // An unknown env value falls through to the next tier, never a hard error.
        let bad_env = MapEnv::new(&[("XDG_CONFIG_HOME", xdg), ("UNBLOCK_OUTPUT_FORMAT", "nope")]);

        for pass in [FormatPass::ErrorPrePass, FormatPass::OwnReport] {
            assert_eq!(
                pick_cli_format(&with_flag, &everything, pass),
                OutputFormat::Csv
            );
            assert_eq!(
                pick_cli_format(&bare, &everything, pass),
                OutputFormat::Plain
            );
            assert_eq!(
                pick_cli_format(&bare, &user_only, pass),
                OutputFormat::Markdown
            );
            assert_eq!(
                pick_cli_format(&bare, &bad_env, pass),
                OutputFormat::Markdown
            );
            assert_eq!(
                pick_cli_format(&bare, &MapEnv::new(&[]), pass),
                OutputFormat::Json
            );
        }
    }

    /// D57 clause (6): a user file that fails to load is skipped by both passes; the error pre-pass
    /// is SILENT and the own-report pass prints EXACTLY one `warning:` line with no `error[`. Every
    /// `load_user_config` refusal is covered: malformed, a startup key, a credential, `jsonl_export`.
    #[test]
    fn a_failing_user_file_warns_once_under_own_report_and_never_under_the_pre_pass() {
        for contents in [
            "output_format = [unclosed\n",
            "db_filename = \"x.db\"\n",
            "auth_token = \"secret\"\n",
            "jsonl_export = true\n",
        ] {
            let home = config_home(contents);
            let env = MapEnv::new(&[("XDG_CONFIG_HOME", home.path().to_str().expect("utf-8"))]);
            let [(pre_fmt, pre_err), (own_fmt, own_err)] =
                both_passes(&GlobalArgs::default(), &env);
            assert_eq!(pre_fmt, OutputFormat::Json, "{contents}");
            assert_eq!(own_fmt, OutputFormat::Json, "{contents}");
            assert_eq!(pre_err, "", "the pre-pass is silent: {contents}");
            assert_eq!(own_err.lines().count(), 1, "exactly one line: {own_err:?}");
            assert!(own_err.starts_with("warning: "), "{own_err:?}");
            assert!(!own_err.contains("error["), "{own_err:?}");
        }
    }

    /// A higher tier short-circuits the user file: with `--output` or a valid env value set, even
    /// the own-report pass never reads (or warns about) a broken user file.
    #[test]
    fn a_higher_tier_never_reads_the_user_file() {
        let home = config_home("output_format = [unclosed\n");
        let xdg = home.path().to_str().expect("utf-8");
        let with_flag = GlobalArgs {
            output: Some(OutputFormat::Csv),
            ..GlobalArgs::default()
        };
        let env = MapEnv::new(&[("XDG_CONFIG_HOME", xdg)]);
        let [(_, pre), (fmt, own)] = both_passes(&with_flag, &env);
        assert_eq!(
            (fmt, pre.as_str(), own.as_str()),
            (OutputFormat::Csv, "", "")
        );
        let env = MapEnv::new(&[("XDG_CONFIG_HOME", xdg), ("UNBLOCK_OUTPUT_FORMAT", "robot")]);
        let [(_, pre), (fmt, own)] = both_passes(&GlobalArgs::default(), &env);
        assert_eq!(
            (fmt, pre.as_str(), own.as_str()),
            (OutputFormat::Robot, "", "")
        );
    }

    /// Renders `report` in all five formats, one payload under each `--- <format>` delimiter, and
    /// closes the document with `--- end`. A payload that ends in a newline shows as a blank line
    /// before the next delimiter.
    fn render_in_every_format(report: &DiagnosticReport) -> String {
        let mut document = String::new();
        for (name, format) in [
            ("json", OutputFormat::Json),
            ("robot", OutputFormat::Robot),
            ("plain", OutputFormat::Plain),
            ("csv", OutputFormat::Csv),
            ("markdown", OutputFormat::Markdown),
        ] {
            let opts = RenderOptions::default();
            let out = renderer_for(format, opts.clone())
                .diagnostics(report, &opts)
                .unwrap_or_else(|e| panic!("{name} renders a lifecycle report: {e}"));
            let _ = writeln!(document, "--- {name}\n{}", out.stdout);
        }
        document.push_str("--- end\n");
        document
    }

    /// A report shaped like the one `Session::doctor()` returns. Its integrity problem holds a
    /// newline, and its sidecar detail holds a comma.
    fn doctor_report() -> DiagnosticReport {
        let rows = [
            ("health", "recoverable"),
            ("integrity", "1 problem(s)"),
            (
                "integrity_problem",
                "*** in database main ***\nPage 3 is never used",
            ),
            ("sidecar_mismatch", "sidecar mismatch (WAL=false, SHM=true)"),
            ("schema_version", "2"),
            ("schema_expected", "2"),
            ("ub-a1", "blocks -> ub-ghost"),
        ];
        DiagnosticReport {
            kind: DiagnosticKind::Info,
            findings: rows
                .into_iter()
                .map(|(label, detail)| DiagnosticFinding {
                    label: label.to_string(),
                    detail: detail.to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn lifecycle_reports_render_in_all_five_formats() {
        let version = VersionReport {
            version: "1.0.1".to_string(),
            build: "release".to_string(),
            commit: Some("abc1234".to_string()),
            rustc: Some("1.96.0".to_string()),
            target: Some("x86_64-unknown-linux-gnu".to_string()),
            features: vec!["remote".to_string(), "self-update".to_string()],
        };
        let migrate = MigrateReport {
            database: "/ws/.unblock/unblock.db".into(),
            schema_from: 1,
            schema_to: 2,
            applied: true,
        };
        let reports = [
            ("lifecycle_version", version.to_report()),
            ("lifecycle_migrate", migrate.to_report()),
            ("lifecycle_doctor", doctor_report()),
            ("lifecycle_init", init_report(None).to_report()),
            (
                "lifecycle_init_with_agents",
                init_report(Some("/ws/AGENTS.md")).to_report(),
            ),
        ];
        for (name, report) in reports {
            insta::assert_snapshot!(name, render_in_every_format(&report));
        }
    }
}
