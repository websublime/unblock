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

/// `unblock init` report — what was scaffolded (AF-3).
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
        DiagnosticReport {
            kind: DiagnosticKind::Info,
            findings: vec![
                finding("workspace_dir", self.workspace_dir.display().to_string()),
                finding("unblock_dir", self.unblock_dir.display().to_string()),
                finding("db_path", self.db_path.display().to_string()),
                finding("config_path", self.config_path.display().to_string()),
                finding("id_prefix", self.id_prefix.clone()),
            ],
        }
    }
}

/// Render `report` in `fmt` and write the structured payload to STDOUT (the CLI owns the stream —
/// NFR-14; all five formats via `Renderer::diagnostics`).
///
/// # Errors
/// - [`CliError::Render`] if the format cannot represent a diagnostic report;
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

/// Write a terse human note to STDERR (NFR-14) — used by `agents`/`init`/`update` for "wrote X".
pub fn diag(message: &str) {
    eprintln!("{message}");
}

#[cfg(test)]
mod tests {
    use super::{InitReport, MigrateReport, ToDiagnosticReport, VersionReport, pick_cli_format};
    use crate::cli::GlobalArgs;
    use unblock_engine::DiagnosticKind;
    use unblock_render::OutputFormat;

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

    #[test]
    fn init_report_maps_to_info_kind() {
        let report = InitReport {
            workspace_dir: "/ws".into(),
            unblock_dir: "/ws/.unblock".into(),
            db_path: "/ws/.unblock/unblock.db".into(),
            id_prefix: "ub".to_string(),
            config_path: "/ws/.unblock/config.toml".into(),
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
}
