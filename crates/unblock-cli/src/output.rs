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

/// Resolve the output format for the NO-workspace path (`version`, top-level parse errors) — config
/// is never opened there, so read `UNBLOCK_OUTPUT_FORMAT` LENIENTLY (via render's `parse_env_value`)
/// so FR-13 precedence still holds uniformly: `--output > UNBLOCK_OUTPUT_FORMAT > Json` (SF-1). An
/// unknown env value falls through to `Json` (never a hard error on this path).
#[must_use]
pub fn pick_cli_format(global: &GlobalArgs) -> OutputFormat {
    global
        .output
        .or_else(|| {
            std::env::var(OUTPUT_FORMAT_ENV)
                .ok()
                .as_deref()
                .and_then(parse_env_value)
        })
        .unwrap_or(OutputFormat::Json)
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
    use std::fmt::Write as _;
    use std::path::PathBuf;

    use super::{
        InitReport, MigrateReport, ToDiagnosticReport, VersionReport, diag_to, pick_cli_format,
    };
    use crate::cli::GlobalArgs;
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

    #[test]
    fn pick_cli_format_prefers_flag_then_env_then_json() {
        // Flag wins.
        let with_flag = GlobalArgs {
            output: Some(OutputFormat::Csv),
            ..GlobalArgs::default()
        };
        assert_eq!(pick_cli_format(&with_flag), OutputFormat::Csv);

        // No flag, no env → Json default. (We avoid mutating process env in a parallel test run; the
        // env branch is covered by the render crate's `pick_format` precedence tests.)
        let bare = GlobalArgs::default();
        // SAFETY of test: only assert the flag-absent + env-absent default deterministically.
        if std::env::var("UNBLOCK_OUTPUT_FORMAT").is_err() {
            assert_eq!(pick_cli_format(&bare), OutputFormat::Json);
        }
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
