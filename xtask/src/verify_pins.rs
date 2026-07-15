//! `verify-pins` — GitHub Actions SHA-pinning gate (NFR-9).
//!
//! A mechanical, offline scan over every `.github/workflows/*.yml` (and `*.yaml`) that FAILS if any
//! third-party `uses:` reference is not pinned to a 40-character commit SHA. This backstops the
//! standing NFR-9 re-pin duty: `dist` CLOBBERS the pins it generates into `release.yml` on every
//! `dist generate`/upgrade, so a one-time manual pin rots silently. It MUST catch a floating
//! `actions/attest@v4` (a floating major in dist's template) just as it catches `@main`/`@v5.0.0`.
//!
//! Run: `cargo xtask verify-pins`. The CI `verify-pins` job wires this in (ci-cd §2, T3.6/D4/MF-5).
//!
//! ## What counts as a violation
//! A `uses:` YAML key whose value is a **third-party** action (`owner/repo@ref` or
//! `owner/repo/subdir@ref`) whose `ref` is not exactly `[0-9a-fA-F]{40}`. Local actions (`./…`,
//! `../…`) carry no ref and are exempt; `docker://…` references use a different (digest) pin scheme
//! and are reported as skipped rather than SHA-checked. A trailing `# vX.Y.Z` comment is advisory
//! (the repo convention) and is not required by this gate.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// A single unpinned-action finding (`path:line: <uses>`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinFinding {
    /// Workspace-relative path of the offending workflow file.
    pub file: String,
    /// 1-based line number of the offending `uses:`.
    pub line: usize,
    /// The offending action reference (e.g. `actions/attest@v4`).
    pub uses: String,
    /// Human-readable reason the reference is not accepted.
    pub reason: String,
}

impl PinFinding {
    fn render(&self) -> String {
        format!(
            "{}:{}: [pin] {} — {}",
            self.file, self.line, self.uses, self.reason
        )
    }
}

/// Entry point for `cargo xtask verify-pins`.
#[must_use]
pub fn verify_pins() -> ExitCode {
    // Workflows root = CARGO_MANIFEST_DIR/../.github/workflows (xtask sits one level under root).
    let root = match workspace_root() {
        Ok(root) => root,
        Err(err) => {
            eprintln!("verify-pins: could not locate workspace root: {err}");
            return ExitCode::FAILURE;
        }
    };

    match scan_at(&root) {
        Ok((findings, scanned)) => report(&findings, scanned),
        Err(err) => {
            eprintln!("verify-pins: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Resolve the workspace root from `CARGO_MANIFEST_DIR` (set by cargo for the running xtask crate).
fn workspace_root() -> Result<PathBuf, String> {
    let manifest = std::env::var("CARGO_MANIFEST_DIR")
        .map_err(|_| "CARGO_MANIFEST_DIR not set (run via `cargo xtask verify-pins`)".to_owned())?;
    Path::new(&manifest)
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("CARGO_MANIFEST_DIR {manifest:?} has no parent"))
}

/// Scan every workflow under `<root>/.github/workflows`. Returns `(findings, files_scanned)`.
///
/// Public so the integration/unit tests can drive it against a fixture tree and prove non-vacuity
/// (an empty workflows dir is an error, not a silent pass), mirroring how `doc-lint` guards its
/// corpus and `check-layering` guards its member set.
///
/// # Errors
/// Returns `Err` if the workflows directory is missing/unreadable or contains no `*.yml`/`*.yaml`
/// file (the vacuous-pass guard).
pub fn scan_at(root: &Path) -> Result<(Vec<PinFinding>, usize), String> {
    let dir = root.join(".github").join("workflows");
    let entries =
        std::fs::read_dir(&dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;

    // Deterministic order so findings sort stably across platforms.
    let mut files: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|e| format!("cannot read dir entry: {e}"))?
            .path();
        let is_workflow = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("yml") || e.eq_ignore_ascii_case("yaml"));
        if is_workflow {
            files.push(path);
        }
    }
    files.sort();

    if files.is_empty() {
        return Err(format!(
            "no *.yml/*.yaml workflow files under {} — refusing a vacuous pass",
            dir.display()
        ));
    }

    let mut findings = Vec::new();
    for path in &files {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        // Report a stable, workspace-relative path.
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();
        findings.extend(scan_workflow(&rel, &text));
    }

    findings.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    Ok((findings, files.len()))
}

/// Scan a single workflow's text and return any unpinned third-party `uses:` findings.
///
/// The testable core: unit tests plant a floating `@v4` / `@main` line (must be REJECTED) and a
/// 40-char SHA line (must be ACCEPTED), the non-vacuity control mirroring `doc-lint`'s planted
/// violations.
#[must_use]
pub fn scan_workflow(file: &str, text: &str) -> Vec<PinFinding> {
    let mut findings = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let Some(token) = uses_value(raw) else {
            continue;
        };
        match classify(token) {
            PinVerdict::Ok | PinVerdict::Skipped => {}
            PinVerdict::Unpinned(reason) => findings.push(PinFinding {
                file: file.to_owned(),
                line: i + 1,
                uses: token.to_owned(),
                reason,
            }),
        }
    }
    findings
}

/// Extract the action reference from a line iff it is a YAML `uses:` key (`uses: X` or `- uses: X`).
/// Returns `None` for prose/comment lines that merely contain the word `uses:`.
fn uses_value(raw: &str) -> Option<&str> {
    let trimmed = raw.trim_start();
    // A YAML comment line is never a directive, even if it mentions `uses:`.
    if trimmed.starts_with('#') {
        return None;
    }
    // Accept both `uses: X` and the list-item form `- uses: X`.
    let after_dash = trimmed.strip_prefix("- ").map_or(trimmed, str::trim_start);
    let rest = after_dash.strip_prefix("uses:")?;
    // The value is the first whitespace-delimited token (a trailing `# comment` is dropped here;
    // `split_whitespace` already skips the leading gap after `uses:`).
    let token = rest.split_whitespace().next()?;
    // Strip optional YAML quoting around the value.
    Some(token.trim_matches(|c| c == '"' || c == '\''))
}

/// The pin verdict for one `uses:` value.
enum PinVerdict {
    /// A 40-char SHA-pinned third-party action.
    Ok,
    /// A local (`./…`) or `docker://` reference — not a git-SHA pin surface.
    Skipped,
    /// A third-party action that is not SHA-pinned (the violation), with a reason.
    Unpinned(String),
}

/// Classify an action reference token.
fn classify(token: &str) -> PinVerdict {
    // Local composite actions carry no ref and live in-repo — exempt.
    if token.starts_with("./") || token.starts_with("../") || token.starts_with('.') {
        return PinVerdict::Skipped;
    }
    // Container actions use a digest pin scheme (`@sha256:…`), not a git SHA — out of this gate's scope.
    if token.starts_with("docker://") {
        return PinVerdict::Skipped;
    }
    match token.rsplit_once('@') {
        Some((_repo, git_ref)) => {
            if is_sha40(git_ref) {
                PinVerdict::Ok
            } else {
                PinVerdict::Unpinned(format!(
                    "ref {git_ref:?} is not a 40-char commit SHA (floating tag/branch)"
                ))
            }
        }
        // `uses: owner/repo` with no `@ref` resolves to the default branch — unpinned.
        None => PinVerdict::Unpinned("no `@<sha>` — resolves to the default branch".to_owned()),
    }
}

/// A 40-character hex commit SHA.
fn is_sha40(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Emit findings and return the process exit code.
fn report(findings: &[PinFinding], scanned: usize) -> ExitCode {
    if findings.is_empty() {
        println!("verify-pins OK: {scanned} workflow file(s), all third-party `uses:` SHA-pinned");
        return ExitCode::SUCCESS;
    }
    eprintln!("UNPINNED GITHUB ACTIONS (NFR-9):");
    for f in findings {
        eprintln!("{}", f.render());
    }
    eprintln!(
        "\npin every third-party `uses:` to a 40-char commit SHA (+ a trailing `# vX.Y.Z`); \
         `dist` re-clobbers `release.yml` on regen, so re-run after any `dist generate`."
    );
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Planted-violation tests (non-vacuity), mirroring doc_lint's one-per-class controls. ----

    #[test]
    fn floating_major_is_rejected() {
        // The exact dist-template risk MF-5 calls out: a floating `actions/attest@v4`.
        let f = scan_workflow(
            ".github/workflows/release.yml",
            "      - name: Attest\n        uses: actions/attest@v4\n",
        );
        assert_eq!(f.len(), 1, "expected one finding, got {f:?}");
        assert_eq!(f[0].uses, "actions/attest@v4");
        assert_eq!(f[0].line, 2);
    }

    #[test]
    fn floating_branch_is_rejected() {
        let f = scan_workflow("wf.yml", "      - uses: actions/checkout@main\n");
        assert_eq!(f.len(), 1, "expected @main rejected, got {f:?}");
        assert_eq!(f[0].uses, "actions/checkout@main");
    }

    #[test]
    fn semver_tag_is_rejected() {
        // A precise `# vX.Y.Z`-style TAG (not a SHA) is still floating and must be rejected.
        let f = scan_workflow("wf.yml", "      - uses: actions/checkout@v5.0.0\n");
        assert_eq!(f.len(), 1, "expected @v5.0.0 rejected, got {f:?}");
    }

    #[test]
    fn missing_ref_is_rejected() {
        let f = scan_workflow("wf.yml", "      - uses: actions/checkout\n");
        assert_eq!(f.len(), 1, "expected a no-`@ref` finding, got {f:?}");
        assert!(f[0].reason.contains("default branch"), "{:?}", f[0].reason);
    }

    #[test]
    fn sha40_is_accepted() {
        // The real repo pin — must be ACCEPTED (proves the gate is not vacuously red).
        let f = scan_workflow(
            "wf.yml",
            "      - uses: actions/checkout@08c6903cd8c0fde910a37f88322edcfb5dd907a8 # v5.0.0\n",
        );
        assert!(f.is_empty(), "40-char SHA must be accepted, got {f:?}");
    }

    #[test]
    fn sha40_without_comment_is_accepted() {
        let f = scan_workflow(
            "wf.yml",
            "        uses: actions/attest@a1948c3f048ba23858d222213b7c278aabede763\n",
        );
        assert!(
            f.is_empty(),
            "SHA without a comment must be accepted, got {f:?}"
        );
    }

    #[test]
    fn quoted_value_is_handled() {
        let ok = scan_workflow(
            "wf.yml",
            "      - uses: \"actions/checkout@08c6903cd8c0fde910a37f88322edcfb5dd907a8\"\n",
        );
        assert!(ok.is_empty(), "quoted SHA must be accepted, got {ok:?}");
        let bad = scan_workflow("wf.yml", "      - uses: 'actions/checkout@v6'\n");
        assert_eq!(
            bad.len(),
            1,
            "quoted floating tag must be rejected, got {bad:?}"
        );
    }

    #[test]
    fn local_action_is_skipped() {
        let f = scan_workflow("wf.yml", "      - uses: ./.github/actions/local\n");
        assert!(f.is_empty(), "local `./` action must be skipped, got {f:?}");
    }

    #[test]
    fn comment_and_prose_lines_are_not_directives() {
        // A commented-out `uses:` and a prose mention must NOT fire (would be a false positive).
        let f = scan_workflow(
            "wf.yml",
            "# Every `uses:` is pinned to a 40-char commit SHA (NFR-9).\n      # uses: actions/x@v1\n",
        );
        assert!(f.is_empty(), "comment/prose lines must not fire, got {f:?}");
    }

    #[test]
    fn subdir_action_ref_is_parsed() {
        let bad = scan_workflow("wf.yml", "      - uses: owner/repo/subdir@v2\n");
        assert_eq!(
            bad.len(),
            1,
            "subdir floating ref must be rejected, got {bad:?}"
        );
        let ok = scan_workflow(
            "wf.yml",
            "      - uses: owner/repo/subdir@08c6903cd8c0fde910a37f88322edcfb5dd907a8\n",
        );
        assert!(ok.is_empty(), "subdir SHA ref must be accepted, got {ok:?}");
    }

    #[test]
    fn is_sha40_boundaries() {
        assert!(is_sha40("08c6903cd8c0fde910a37f88322edcfb5dd907a8"));
        assert!(
            !is_sha40("08c6903cd8c0fde910a37f88322edcfb5dd907a"),
            "39 chars"
        );
        assert!(
            !is_sha40("08c6903cd8c0fde910a37f88322edcfb5dd907a8f"),
            "41 chars"
        );
        assert!(
            !is_sha40("08c6903cd8c0fde910a37f88322edcfb5dd907ag"),
            "non-hex g"
        );
    }
}
