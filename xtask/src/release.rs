//! `release` — interactive version-bump + tag + push helper (`cargo xtask release`).
//!
//! Automates the `dist` release trigger safely. The `dist` release pipeline
//! (`.github/workflows/release.yml`) is dormant on `main` and fires when a version tag
//! (`vX.Y.Z` / `vX.Y.Z-rc.N`) is pushed; `dist` REQUIRES that tag version to equal the workspace
//! `Cargo.toml` `[workspace.package]` version. This command chains: pre-flight → prompt → compute
//! new version → bump `Cargo.toml` + refresh `Cargo.lock` → commit → annotated tag → push.
//!
//! Run: `cargo xtask release [--dry-run]`. Authoritative spec:
//! `docs/plans/ci-cd-and-distribution.md` §3.
//!
//! # Safety model
//! - The tag push is an **IRREVERSIBLE public publish**: it triggers the `dist` release workflow.
//!   The tool is HUMAN-operated and demands the operator TYPE THE TAG twice (once before any change,
//!   once before the push); any mismatch aborts with no push.
//! - `--dry-run` runs every read-only step (pre-flight, prompts, compute, guard, plan) and stops
//!   before any mutation, leaving the working tree untouched.
//! - Pre-flight refuses to proceed unless HEAD is `main`, the working tree is clean, and local `main`
//!   equals `origin/main` (after `git fetch`).
//! - The publish step needs a `WS_GH_TOKEN` repo secret with `contents: write` (the org restricts the
//!   default token, see ci-cd §3.1). A secret cannot be verified from here, so the tool only warns.
//!
//! The version-compute is a set of PURE, unit-tested functions ([`bump_core`], [`with_rc`],
//! [`tag_of`], [`compute_target`]); the git / cargo / filesystem effects go through the
//! [`ReleaseEnv`] seam so the whole flow is testable against a fake (no real repo mutation).

use std::ffi::OsStr;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use anstyle::{AnsiColor, Color, Style};
use semver::{Prerelease, Version};

/// The kind of release the operator chose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// A pre-release cut: attaches an `rc.N` pre-release to the (bumped) core.
    PreRelease,
    /// A final release: the (bumped) core with no pre-release.
    Final,
}

/// The semver core increment to apply before (optionally) attaching a pre-release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bump {
    /// Keep the current core (release it as-is; strips any existing pre-release).
    None,
    /// `x.y.z` → `x.y.(z+1)`.
    Patch,
    /// `x.y.z` → `x.(y+1).0`.
    Minor,
    /// `x.y.z` → `(x+1).0.0`.
    Major,
}

// ---------------------------------------------------------------------------------------------
// Pure version compute (unit-tested; no git / IO).
// ---------------------------------------------------------------------------------------------

/// The pre-release-free, build-metadata-free core of a version (`1.2.3-rc.2` → `1.2.3`).
fn core_of(v: &Version) -> Version {
    Version::new(v.major, v.minor, v.patch)
}

/// Apply a [`Bump`] to the CORE of `current` (any existing pre-release is stripped first).
///
/// `None` keeps the core; `Patch`/`Minor`/`Major` do the usual semver increment, zeroing every
/// lower part. Returns a core-only [`Version`] (no pre-release, no build metadata).
fn bump_core(current: &Version, bump: Bump) -> Version {
    let core = core_of(current);
    match bump {
        Bump::None => core,
        Bump::Patch => Version::new(core.major, core.minor, core.patch + 1),
        Bump::Minor => Version::new(core.major, core.minor + 1, 0),
        Bump::Major => Version::new(core.major + 1, 0, 0),
    }
}

/// Attach the `rc.<n>` pre-release to `core` (any existing pre-release/build metadata is dropped).
///
/// # Errors
/// Returns `Err` if `rc.<n>` is not a valid semver pre-release (it always is for a `u64`, but the
/// fallible `Prerelease::new` boundary is threaded rather than unwrapped).
fn with_rc(core: &Version, n: u64) -> Result<Version, String> {
    let mut v = core_of(core);
    v.pre =
        Prerelease::new(&format!("rc.{n}")).map_err(|e| format!("invalid rc pre-release: {e}"))?;
    Ok(v)
}

/// The git tag for a version: `v` + the version's `Display` (`1.1.0` → `v1.1.0`,
/// `1.0.0-rc.1` → `v1.0.0-rc.1`).
fn tag_of(v: &Version) -> String {
    format!("v{v}")
}

/// The next `rc` number for `core`: `(max existing v<core>-rc.<N> tag) + 1`, else `1`.
///
/// Scans `existing_tags` (a union of local + remote tags) for `v<core>-rc.<N>` and returns one past
/// the highest `N`, so a fresh core starts at `rc.1` and an existing `rc.k` advances to `rc.(k+1)`.
fn next_rc_number(existing_tags: &[String], core: &Version) -> u64 {
    let mut max_n: Option<u64> = None;
    for tag in existing_tags {
        let Some(stripped) = tag.strip_prefix('v') else {
            continue;
        };
        let Ok(v) = Version::parse(stripped) else {
            continue;
        };
        if v.major != core.major || v.minor != core.minor || v.patch != core.patch {
            continue;
        }
        if let Some(n) = rc_number_of(&v.pre) {
            max_n = Some(max_n.map_or(n, |m| m.max(n)));
        }
    }
    max_n.map_or(1, |m| m + 1)
}

/// The `N` in an `rc.N` pre-release, if the pre-release is exactly that shape.
fn rc_number_of(pre: &Prerelease) -> Option<u64> {
    pre.as_str().strip_prefix("rc.")?.parse::<u64>().ok()
}

/// Compute the target version from the current version, the chosen kind + bump, and the known tags.
///
/// Strips any existing pre-release, applies the core bump, and — for a pre-release — attaches the
/// next `rc.N` (numbered against `existing_tags`).
///
/// # Errors
/// Returns `Err` only if the internal `rc.<n>` pre-release cannot be constructed (see [`with_rc`]).
fn compute_target(
    current: &Version,
    kind: Kind,
    bump: Bump,
    existing_tags: &[String],
) -> Result<Version, String> {
    let core = bump_core(current, bump);
    match kind {
        Kind::Final => Ok(core),
        Kind::PreRelease => {
            let n = next_rc_number(existing_tags, &core);
            with_rc(&core, n)
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Pure Cargo.toml version edit (unit-tested; no IO).
// ---------------------------------------------------------------------------------------------

/// Locate the `version = "..."` line inside the `[workspace.package]` table of a `Cargo.toml`.
///
/// Returns `(line_index, current_value)`. Only the `[workspace.package]` table is inspected, so a
/// `version` key in any other table (or a dependency pin) is never touched.
///
/// # Errors
/// Returns `Err` if there is no `[workspace.package]` `version` key.
fn find_workspace_version_line(toml: &str) -> Result<(usize, String), String> {
    let mut in_section = false;
    for (i, line) in toml.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_section = trimmed == "[workspace.package]";
            continue;
        }
        if in_section
            && let Some(rest) = trimmed.strip_prefix("version")
            && let Some(after_eq) = rest.trim_start().strip_prefix('=')
        {
            let val = after_eq.trim().trim_matches('"').to_owned();
            return Ok((i, val));
        }
    }
    Err("no `version` key found in the [workspace.package] table of Cargo.toml".to_owned())
}

/// Parse the `[workspace.package]` version of a `Cargo.toml` as semver.
///
/// # Errors
/// Returns `Err` if the version key is absent or not valid semver.
fn parse_workspace_version(toml: &str) -> Result<Version, String> {
    let (_, raw) = find_workspace_version_line(toml)?;
    Version::parse(&raw).map_err(|e| format!("workspace version {raw:?} is not valid semver: {e}"))
}

/// Rewrite the `[workspace.package]` version line to `new`, preserving all other content + trailing
/// newline.
///
/// # Errors
/// Returns `Err` if there is no `[workspace.package]` `version` key to replace.
fn replace_workspace_version(toml: &str, new: &str) -> Result<String, String> {
    let (idx, _) = find_workspace_version_line(toml)?;
    let mut lines: Vec<String> = toml.lines().map(str::to_owned).collect();
    let indent: String = lines[idx]
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect();
    lines[idx] = format!("{indent}version = \"{new}\"");
    let mut result = lines.join("\n");
    if toml.ends_with('\n') {
        result.push('\n');
    }
    Ok(result)
}

// ---------------------------------------------------------------------------------------------
// Prompt parsing (pure).
// ---------------------------------------------------------------------------------------------

/// Parse a release-type answer (number or word); `None` for an unrecognised answer.
fn parse_kind(s: &str) -> Option<Kind> {
    match s.trim().to_ascii_lowercase().as_str() {
        "1" | "pre-release" | "prerelease" | "pre" | "rc" => Some(Kind::PreRelease),
        "2" | "final" => Some(Kind::Final),
        _ => None,
    }
}

/// Parse a bump answer (number or word); `None` for an unrecognised answer.
fn parse_bump(s: &str) -> Option<Bump> {
    match s.trim().to_ascii_lowercase().as_str() {
        "1" | "none" => Some(Bump::None),
        "2" | "patch" => Some(Bump::Patch),
        "3" | "minor" => Some(Bump::Minor),
        "4" | "major" => Some(Bump::Major),
        _ => None,
    }
}

const KIND_PROMPT: &str = "Release type:\n  1) pre-release (rc)\n  2) final\nChoose [1/2]: ";
const BUMP_PROMPT: &str = "Version bump:\n  1) none (release current version as-is)\n  \
     2) patch\n  3) minor\n  4) major\nChoose [1/2/3/4]: ";

// ---------------------------------------------------------------------------------------------
// The effect seam (git / cargo / filesystem). Real impl uses `Command`; tests use a fake.
// ---------------------------------------------------------------------------------------------

/// The git / cargo / filesystem effects the release flow needs, abstracted so the orchestration is
/// unit-testable against a fake (every irreversible mutation is funnelled through one seam).
trait ReleaseEnv {
    // ---- reads (pre-flight + guards) ----
    /// The checked-out branch name (`git rev-parse --abbrev-ref HEAD`).
    fn current_branch(&self) -> Result<String, String>;
    /// Whether the working tree is clean (`git status --porcelain` empty).
    fn working_tree_clean(&self) -> Result<bool, String>;
    /// `git fetch origin` (so the divergence check compares against the freshest remote).
    fn fetch_origin(&self) -> Result<(), String>;
    /// Whether local `main` equals `origin/main` (neither ahead nor behind).
    fn main_matches_origin(&self) -> Result<bool, String>;
    /// Local tags (`git tag -l`).
    fn local_tags(&self) -> Result<Vec<String>, String>;
    /// Remote tags (`git ls-remote --tags origin`, deref lines dropped).
    fn remote_tags(&self) -> Result<Vec<String>, String>;
    /// The current `[workspace.package]` version from the root `Cargo.toml`.
    fn current_version(&self) -> Result<Version, String>;

    // ---- writes (REAL run only) ----
    /// Rewrite the `[workspace.package]` version in the root `Cargo.toml`.
    fn set_workspace_version(&self, new: &Version) -> Result<(), String>;
    /// `cargo update --workspace` (refreshes `Cargo.lock` to the new version).
    fn cargo_update_workspace(&self) -> Result<(), String>;
    /// `git add -- Cargo.toml Cargo.lock && git commit -m <message>` (only the two release files are
    /// staged, so a stray untracked/dirty path can never enter the public release commit).
    fn git_commit_all(&self, message: &str) -> Result<(), String>;
    /// `git tag -a <tag> -m <message>` (annotated).
    fn git_annotated_tag(&self, tag: &str, message: &str) -> Result<(), String>;
    /// `git push --atomic origin <branch> <tag>` — both refs advance or NEITHER does. A
    /// non-fast-forward on `<branch>` (e.g. a race with another pusher since the pre-flight fetch)
    /// aborts the WHOLE push, so `origin/main` can never advance without its tag. No `--force`, so the
    /// branch is still rejected if it diverged.
    fn git_push_atomic(&self, branch: &str, tag: &str) -> Result<(), String>;
}

/// The real environment: runs git/cargo in the workspace root and edits `Cargo.toml` on disk.
struct GitEnv {
    /// Workspace root (holds `Cargo.toml`, the `.git` worktree).
    root: PathBuf,
    /// The once-decided STDERR color choice (from `run()`, keyed on `stderr().is_terminal()`),
    /// threaded into the stderr progress spinner.
    color: bool,
}

impl GitEnv {
    /// Run `git <args>` in the workspace root, capturing output.
    fn git(&self, args: &[&str]) -> Result<std::process::Output, String> {
        Command::new("git")
            .current_dir(&self.root)
            .args(args)
            .output()
            .map_err(|e| format!("failed to spawn `git {}`: {e}", args.join(" ")))
    }

    /// Run `git <args>`, requiring success, returning trimmed stdout.
    fn git_ok(&self, args: &[&str]) -> Result<String, String> {
        let out = self.git(args)?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
        } else {
            Err(format!(
                "`git {}` failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }

    /// Path to the root `Cargo.toml`.
    fn cargo_toml(&self) -> PathBuf {
        self.root.join("Cargo.toml")
    }
}

impl ReleaseEnv for GitEnv {
    fn current_branch(&self) -> Result<String, String> {
        self.git_ok(&["rev-parse", "--abbrev-ref", "HEAD"])
    }

    fn working_tree_clean(&self) -> Result<bool, String> {
        Ok(self.git_ok(&["status", "--porcelain"])?.is_empty())
    }

    fn fetch_origin(&self) -> Result<(), String> {
        // A slow blocking step — show a stderr spinner (TTY) / static line (non-TTY) while it runs.
        with_progress(
            io::stderr(),
            "fetching origin",
            Palette { color: self.color },
            io::stderr().is_terminal(),
            || self.git_ok(&["fetch", "origin"]).map(|_| ()),
        )
    }

    fn main_matches_origin(&self) -> Result<bool, String> {
        let local = self.git_ok(&["rev-parse", "refs/heads/main"])?;
        let remote = self.git_ok(&["rev-parse", "refs/remotes/origin/main"])?;
        Ok(local == remote)
    }

    fn local_tags(&self) -> Result<Vec<String>, String> {
        Ok(self
            .git_ok(&["tag", "-l"])?
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect())
    }

    fn remote_tags(&self) -> Result<Vec<String>, String> {
        let raw = self.git_ok(&["ls-remote", "--tags", "origin"])?;
        Ok(raw
            .lines()
            .filter_map(|l| l.split_whitespace().nth(1))
            .filter_map(|r| r.strip_prefix("refs/tags/"))
            // Drop the annotated-tag deref lines (`refs/tags/v1.0.0^{}`).
            .filter(|t| !t.ends_with("^{}"))
            .map(str::to_owned)
            .collect())
    }

    fn current_version(&self) -> Result<Version, String> {
        let toml = std::fs::read_to_string(self.cargo_toml())
            .map_err(|e| format!("cannot read {}: {e}", self.cargo_toml().display()))?;
        parse_workspace_version(&toml)
    }

    fn set_workspace_version(&self, new: &Version) -> Result<(), String> {
        let path = self.cargo_toml();
        let toml = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let updated = replace_workspace_version(&toml, &new.to_string())?;
        std::fs::write(&path, updated).map_err(|e| format!("cannot write {}: {e}", path.display()))
    }

    fn cargo_update_workspace(&self) -> Result<(), String> {
        // A slow blocking step — show a stderr spinner (TTY) / static line (non-TTY) while it runs.
        with_progress(
            io::stderr(),
            "updating Cargo.lock",
            Palette { color: self.color },
            io::stderr().is_terminal(),
            || {
                let out = Command::new("cargo")
                    .current_dir(&self.root)
                    .args(["update", "--workspace"])
                    .output()
                    .map_err(|e| format!("failed to spawn `cargo update --workspace`: {e}"))?;
                if out.status.success() {
                    Ok(())
                } else {
                    Err(format!(
                        "`cargo update --workspace` failed: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    ))
                }
            },
        )
    }

    fn git_commit_all(&self, message: &str) -> Result<(), String> {
        // Stage ONLY the two release files (defense-in-depth: even though the pre-flight requires a
        // clean tree, a scoped `add` guarantees a stray path can never enter the public commit).
        self.git_ok(&["add", "--", "Cargo.toml", "Cargo.lock"])?;
        self.git_ok(&["commit", "-m", message]).map(|_| ())
    }

    fn git_annotated_tag(&self, tag: &str, message: &str) -> Result<(), String> {
        self.git_ok(&["tag", "-a", tag, "-m", message]).map(|_| ())
    }

    fn git_push_atomic(&self, branch: &str, tag: &str) -> Result<(), String> {
        // No `--force`: a non-fast-forward on `branch` is rejected, and `--atomic` then aborts the
        // tag too — both-or-neither, so `origin/main` can never be published without its tag.
        self.git_ok(&["push", "--atomic", "origin", branch, tag])
            .map(|_| ())
    }
}

// ---------------------------------------------------------------------------------------------
// Presentation (styling primitives over `anstyle` + a stderr progress spinner).
//
// Color is an EXPLICIT decision made ONCE in `run()` from the `NO_COLOR` / `CLICOLOR` /
// `CLICOLOR_FORCE` env signals plus the SINK's own TTY-ness — and THREADED through the flow; it is
// never auto-detected mid-write. Two INDEPENDENT decisions are made: the `out` (stdout) palette from
// `stdout().is_terminal()`, and a SEPARATE stderr palette from `stderr().is_terminal()` for the
// progress spinner + top-level error lines, so a redirected stderr (`2>log`) stays plain even when
// stdout is a TTY. With color off, `Painted` writes plain text (no ESC byte), so the captured test
// output stays deterministic. `anstyle` is already in the lock via clap, so this adds no new
// transitive surface (ci-cd §3.3). All stdout styling flows through the same `out` writer; the
// spinner is the sole stderr writer (diagnostics → stderr, NFR-14).
// ---------------------------------------------------------------------------------------------

/// A green "check passed" style.
const STYLE_OK: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green)));
/// A red "check failed" / error style.
const STYLE_ERR: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Red)));
/// A yellow "warning / cannot verify" style.
const STYLE_WARN: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow)));
/// A bold-cyan header / progress-glyph style.
const STYLE_HEADER: Style = Style::new()
    .bold()
    .fg_color(Some(Color::Ansi(AnsiColor::Cyan)));
/// A bold style used to emphasize the version diff and the tag inside the plan box.
const STYLE_EMPHASIS: Style = Style::new().bold();
/// A LOUD bold white-on-red style for the irreversible-publish banner.
const STYLE_LOUD: Style = Style::new()
    .bold()
    .fg_color(Some(Color::Ansi(AnsiColor::BrightWhite)))
    .bg_color(Some(Color::Ansi(AnsiColor::Red)));

/// The animation frames for the stderr progress spinner (Braille dots — each a single visible column).
const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// The spinner redraw interval.
const SPIN_INTERVAL_MS: u64 = 90;

/// A once-decided color choice, threaded through the flow. `Copy` so it is cheap to pass by value.
#[derive(Clone, Copy)]
struct Palette {
    /// Whether ANSI styling is emitted (decided once in `run()`; forced `false` in tests).
    color: bool,
}

impl Palette {
    /// Wrap `text` in `style`'s ANSI SGR codes when color is enabled; otherwise render it verbatim.
    fn paint(self, style: Style, text: &str) -> Painted<'_> {
        Painted {
            style,
            text,
            color: self.color,
        }
    }
}

/// A `Display` fragment that emits `style` around `text` only when `color` is set. With color off it
/// writes the bytes of `text` unchanged, guaranteeing NO ESC (`0x1b`) byte reaches the sink.
struct Painted<'a> {
    /// The style to apply when `color` is set.
    style: Style,
    /// The text to render.
    text: &'a str,
    /// Whether to emit the ANSI codes.
    color: bool,
}

impl std::fmt::Display for Painted<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.color {
            // `{style}` renders the enable sequence, `{style:#}` the reset (anstyle's Display).
            write!(f, "{}{}{:#}", self.style, self.text, self.style)
        } else {
            f.write_str(self.text)
        }
    }
}

/// Resolve the color decision from the standard env signals + whether stdout is a TTY. PURE (so it is
/// unit-tested): `NO_COLOR` disables unconditionally; `CLICOLOR_FORCE` then forces on; `CLICOLOR=0`
/// disables; otherwise color follows the TTY (the informal `NO_COLOR` / `CLICOLOR` conventions).
fn resolve_color(
    no_color: bool,
    clicolor_force: bool,
    clicolor: Option<bool>,
    is_tty: bool,
) -> bool {
    if no_color {
        return false;
    }
    if clicolor_force {
        return true;
    }
    match clicolor {
        Some(false) => false,
        Some(true) | None => is_tty,
    }
}

/// Map the raw process env values (as `var_os` yields them) + a sink's TTY-ness to the color
/// decision. PURE glue over [`resolve_color`] (so the env→color parsing is unit-tested apart from the
/// process env): `NO_COLOR` counts only when NON-EMPTY; `CLICOLOR_FORCE` forces on only for a
/// non-empty, non-`0` value; `CLICOLOR` maps `0`→off and any other non-empty value→on, while empty
/// (or absent) is "unset" and falls through to the TTY. `OsStr` (not `str`) is kept so a non-UTF-8
/// value maps exactly as the process reads it.
fn color_from_env(
    no_color: Option<&OsStr>,
    clicolor_force: Option<&OsStr>,
    clicolor: Option<&OsStr>,
    is_terminal: bool,
) -> bool {
    let no_color = no_color.is_some_and(|v| !v.is_empty());
    let clicolor_force = clicolor_force.is_some_and(|v| !v.is_empty() && v.to_str() != Some("0"));
    let clicolor = match clicolor {
        Some(v) if v.is_empty() => None,
        Some(v) => Some(v.to_str() != Some("0")),
        None => None,
    };
    resolve_color(no_color, clicolor_force, clicolor, is_terminal)
}

/// Read the env signals + `stdout().is_terminal()` and decide the STDOUT color once (the `out` sink);
/// thin process-env glue over [`color_from_env`].
fn detect_color() -> bool {
    color_from_env(
        std::env::var_os("NO_COLOR").as_deref(),
        std::env::var_os("CLICOLOR_FORCE").as_deref(),
        std::env::var_os("CLICOLOR").as_deref(),
        io::stdout().is_terminal(),
    )
}

/// Read the env signals + `stderr().is_terminal()` and decide the STDERR color once (the progress
/// spinner + top-level error lines). Keyed on STDERR's OWN TTY-ness — not stdout's — so a redirected
/// stderr (`2>log`) stays plain even when stdout is a TTY. Thin process-env glue over
/// [`color_from_env`].
fn detect_err_color() -> bool {
    color_from_env(
        std::env::var_os("NO_COLOR").as_deref(),
        std::env::var_os("CLICOLOR_FORCE").as_deref(),
        std::env::var_os("CLICOLOR").as_deref(),
        io::stderr().is_terminal(),
    )
}

/// The styled command banner.
fn show_header(out: &mut dyn Write, palette: Palette) -> Result<(), String> {
    writeln!(
        out,
        "{}",
        palette.paint(STYLE_HEADER, "═══ unblock release ═══")
    )
    .map_err(|e| io_err(&e))
}

/// Emit a pre-flight check line: a green check or a red cross followed by `label`.
fn check_line(out: &mut dyn Write, palette: Palette, ok: bool, label: &str) -> Result<(), String> {
    let (glyph, style) = if ok {
        ("✓", STYLE_OK)
    } else {
        ("✗", STYLE_ERR)
    };
    writeln!(out, "  {} {label}", palette.paint(style, glyph)).map_err(|e| io_err(&e))
}

/// A row inside the boxed release plan: `(label, value, emphasize?)`.
type PlanRow<'a> = (&'a str, String, bool);

/// Render the release plan as a boxed, aligned block; emphasized values are bold. Column widths are
/// computed on the PLAIN text (visible columns), so the borders align even with color on.
fn render_plan_box(out: &mut dyn Write, palette: Palette, rows: &[PlanRow]) -> Result<(), String> {
    let label_w = rows
        .iter()
        .map(|(l, _, _)| l.chars().count())
        .max()
        .unwrap_or(0);
    let plain: Vec<String> = rows
        .iter()
        .map(|(l, v, _)| format!("{l:<label_w$} : {v}"))
        .collect();
    let content_w = plain.iter().map(|p| p.chars().count()).max().unwrap_or(0);
    let span = content_w + 2;
    let title = " Release plan ";
    let title_fill = span.saturating_sub(title.chars().count() + 1);
    writeln!(out, "┌─{title}{}┐", "─".repeat(title_fill)).map_err(|e| io_err(&e))?;
    for ((label, value, emphasize), plain_row) in rows.iter().zip(&plain) {
        let style = if *emphasize {
            STYLE_EMPHASIS
        } else {
            Style::new()
        };
        let pad = content_w - plain_row.chars().count();
        writeln!(
            out,
            "│ {label:<label_w$} : {}{} │",
            palette.paint(style, value),
            " ".repeat(pad)
        )
        .map_err(|e| io_err(&e))?;
    }
    writeln!(out, "└{}┘", "─".repeat(span)).map_err(|e| io_err(&e))?;
    Ok(())
}

/// The LOUD irreversible-publish banner shown before the typed confirmation gate (real runs only).
fn irreversible_banner(out: &mut dyn Write, palette: Palette, tag: &str) -> Result<(), String> {
    writeln!(out).map_err(|e| io_err(&e))?;
    writeln!(
        out,
        "{}",
        palette.paint(
            STYLE_LOUD,
            &format!("  !! IRREVERSIBLE PUBLISH — {tag} !!  ")
        )
    )
    .map_err(|e| io_err(&e))?;
    writeln!(
        out,
        "  This pushes the tag and triggers the public dist release; it CANNOT be undone."
    )
    .map_err(|e| io_err(&e))?;
    writeln!(out).map_err(|e| io_err(&e))
}

/// The static line rendered for a slow step when stderr is NOT a TTY (the spinner's fallback).
fn progress_static_line(label: &str, palette: Palette) -> String {
    format!("{} {label}…", palette.paint(STYLE_HEADER, "•"))
}

/// One animated spinner frame for `tick`.
fn spinner_frame(tick: usize, label: &str, palette: Palette) -> String {
    let frame = SPINNER_FRAMES[tick % SPINNER_FRAMES.len()];
    format!("{} {label}…", palette.paint(STYLE_HEADER, frame))
}

/// The green "step done" success line — printed only after a slow step returns `Ok`.
fn progress_done_line(label: &str, palette: Palette) -> String {
    format!("{} {label} done", palette.paint(STYLE_OK, "✓"))
}

/// The red "step failed" line — printed when a slow step returns `Err`, NEVER the green ✓ "done"
/// success line (which would misreport a failed `git fetch` / `cargo update` as a success).
fn progress_fail_line(label: &str, palette: Palette) -> String {
    format!("{} {label} failed", palette.paint(STYLE_ERR, "✗"))
}

/// A background-thread spinner that renders frames to a writer until stopped, then clears its line
/// and prints a "done" line. Owns the writer (moved onto the thread), so a test can inject a buffer
/// while the real path passes `io::stderr()`. `#![forbid(unsafe_code)]` holds — pure std threading,
/// no raw terminal mode.
struct Spinner {
    /// Signals the render thread to stop.
    stop: Arc<AtomicBool>,
    /// The step OUTCOME, read by the render thread after it stops to choose the end line (green ✓
    /// "done" vs red ✗ "failed"). Set by [`Spinner::stop`] BEFORE the stop flag is raised.
    success: Arc<AtomicBool>,
    /// The render-thread handle, joined on stop / drop.
    handle: Option<thread::JoinHandle<()>>,
}

impl Spinner {
    /// Spawn the render thread, drawing `label` frames to `writer` until [`Spinner::stop`], which also
    /// reports the step OUTCOME (done vs failed) on the cleared line.
    fn start<W: Write + Send + 'static>(
        mut writer: W,
        label: String,
        palette: Palette,
        clear_width: usize,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        // Default `true`: the `Drop` safety-net path (no explicit `stop`) reports "done", matching the
        // prior behaviour; the normal path always calls `stop(success)` with the real outcome first.
        let success = Arc::new(AtomicBool::new(true));
        let flag = Arc::clone(&stop);
        let ok_flag = Arc::clone(&success);
        let handle = thread::spawn(move || {
            let mut tick = 0usize;
            // `Acquire` pairs with the `Release` store in `join`, so once the loop observes the stop
            // flag it is guaranteed to also observe the `success` value stored before it.
            while !flag.load(Ordering::Acquire) {
                let _ = write!(writer, "\r{}", spinner_frame(tick, &label, palette));
                let _ = writer.flush();
                tick = tick.wrapping_add(1);
                thread::sleep(Duration::from_millis(SPIN_INTERVAL_MS));
            }
            // Erase the spinner line, then report the step OUTCOME — a failed step gets the red ✗
            // "failed" line, NEVER the green ✓ "done" success line — so the flow continues clean.
            let _ = write!(writer, "\r{}\r", " ".repeat(clear_width));
            let end = if ok_flag.load(Ordering::Relaxed) {
                progress_done_line(&label, palette)
            } else {
                progress_fail_line(&label, palette)
            };
            let _ = writeln!(writer, "{end}");
            let _ = writer.flush();
        });
        Spinner {
            stop,
            success,
            handle: Some(handle),
        }
    }

    /// Record the step `success`, then signal stop and join the render thread.
    fn stop(mut self, success: bool) {
        self.success.store(success, Ordering::Relaxed);
        self.join();
    }

    /// Idempotent stop+join used by both [`Spinner::stop`] and `Drop`.
    fn join(&mut self) {
        // `Release` publishes the `success` store (in `stop`) to the render thread's `Acquire` load.
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.join();
    }
}

/// Run a slow blocking step while presenting progress on `err` (stderr in production), returning the
/// step's `Result`. Animated when `is_tty` (a background thread renders frames, then an OUTCOME line
/// on stop), else a static in-progress line + an outcome line around the step. The outcome line is the
/// green ✓ "done" line ONLY when the step returns `Ok`; on `Err` it is the red ✗ "failed" line, so a
/// failed step is NEVER shown as succeeded. STDERR-only — never touches the `out` sink (NFR-14).
fn with_progress<W: Write + Send + 'static, T, E>(
    mut err: W,
    label: &str,
    palette: Palette,
    is_tty: bool,
    slow: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    if !is_tty {
        // Non-TTY (redirected / CI): a static in-progress line, the step, then an outcome line — a
        // failed step gets the red ✗ "failed" line, NEVER a green ✓, mirroring the animated branch.
        let _ = writeln!(err, "{}", progress_static_line(label, palette));
        let _ = err.flush();
        let result = slow();
        let end = if result.is_ok() {
            progress_done_line(label, palette)
        } else {
            progress_fail_line(label, palette)
        };
        let _ = writeln!(err, "{end}");
        let _ = err.flush();
        return result;
    }
    let clear_width = label.chars().count() + 3;
    let spinner = Spinner::start(err, label.to_owned(), palette, clear_width);
    let result = slow();
    // Thread the OUTCOME into the stop path so the "done" / "failed" line reflects the real result.
    spinner.stop(result.is_ok());
    result
}

// ---------------------------------------------------------------------------------------------
// Orchestration (pure over the `ReleaseEnv` seam + an input reader / output writer).
// ---------------------------------------------------------------------------------------------

/// Map an I/O error on the prompt/output stream to a message.
fn io_err(e: &io::Error) -> String {
    format!("I/O error on the prompt stream: {e}")
}

/// Prompt with `prompt`, read one line, and parse it; re-prompt until `parse` accepts or input ends.
fn ask<T>(
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    prompt: &str,
    parse: impl Fn(&str) -> Option<T>,
) -> Result<T, String> {
    loop {
        write!(out, "{prompt}").map_err(|e| io_err(&e))?;
        out.flush().map_err(|e| io_err(&e))?;
        let mut line = String::new();
        if input.read_line(&mut line).map_err(|e| io_err(&e))? == 0 {
            return Err("unexpected end of input while awaiting a prompt answer".to_owned());
        }
        if let Some(v) = parse(line.trim()) {
            return Ok(v);
        }
        writeln!(out, "  invalid choice {:?}; please try again", line.trim())
            .map_err(|e| io_err(&e))?;
    }
}

/// Prompt with `prompt` and return the single trimmed line (used for the typed-tag confirmations).
fn ask_line(input: &mut dyn BufRead, out: &mut dyn Write, prompt: &str) -> Result<String, String> {
    write!(out, "{prompt}").map_err(|e| io_err(&e))?;
    out.flush().map_err(|e| io_err(&e))?;
    let mut line = String::new();
    if input.read_line(&mut line).map_err(|e| io_err(&e))? == 0 {
        return Err("unexpected end of input while awaiting a confirmation".to_owned());
    }
    Ok(line.trim().to_owned())
}

/// Pre-flight guard: HEAD is `main`, the working tree is clean, and local `main` == `origin/main`
/// (after `git fetch`). Also prints the `WS_GH_TOKEN` reminder (a secret cannot be verified here).
///
/// # Errors
/// Returns `Err` on the FIRST failed check (branch / cleanliness / divergence), before any mutation.
fn preflight(env: &dyn ReleaseEnv, out: &mut dyn Write, palette: Palette) -> Result<(), String> {
    let branch = env.current_branch()?;
    let on_main = branch == "main";
    check_line(
        out,
        palette,
        on_main,
        &format!("on branch `main` (HEAD is `{branch}`)"),
    )?;
    if !on_main {
        return Err(format!(
            "must be on `main` to cut a release, but HEAD is `{branch}` — checkout main first"
        ));
    }
    let clean = env.working_tree_clean()?;
    check_line(out, palette, clean, "working tree is clean")?;
    if !clean {
        return Err(
            "working tree is not clean (`git status --porcelain` is non-empty) — commit or stash first"
                .to_owned(),
        );
    }
    env.fetch_origin()?;
    let synced = env.main_matches_origin()?;
    check_line(
        out,
        palette,
        synced,
        "local `main` is in sync with `origin/main`",
    )?;
    if !synced {
        return Err(
            "local `main` has diverged from `origin/main` (behind or ahead) — sync before releasing"
                .to_owned(),
        );
    }
    writeln!(
        out,
        "  {} reminder: the publish step needs a `WS_GH_TOKEN` repo secret with `contents: write` \
         (cannot be verified from here — see ci-cd §3.1).",
        palette.paint(STYLE_WARN, "⚠")
    )
    .map_err(|e| io_err(&e))?;
    Ok(())
}

/// Render the release plan (current → new, tag, pre-release?, files) as a boxed, aligned block with
/// the version diff + tag emphasized, followed by the irreversibility notice.
fn show_plan(
    out: &mut dyn Write,
    current: &Version,
    new: &Version,
    tag: &str,
    kind: Kind,
    palette: Palette,
) -> Result<(), String> {
    let pre = if kind == Kind::PreRelease {
        "yes (rc)"
    } else {
        "no (final)"
    };
    // Emphasized (bold) rows: the version diff and the tag. Plain rows: pre-release + files.
    let rows: Vec<PlanRow> = vec![
        ("version", format!("{current} → {new}"), true),
        ("tag", tag.to_owned(), true),
        ("pre-release", pre.to_owned(), false),
        ("files changed", "Cargo.toml, Cargo.lock".to_owned(), false),
    ];
    writeln!(out).map_err(|e| io_err(&e))?;
    render_plan_box(out, palette, &rows)?;
    writeln!(
        out,
        "{}",
        palette.paint(
            STYLE_ERR,
            &format!(
                "IRREVERSIBLE: pushing {tag} triggers the public dist release and CANNOT be undone."
            )
        )
    )
    .map_err(|e| io_err(&e))?;
    writeln!(out).map_err(|e| io_err(&e))?;
    Ok(())
}

/// Remediation for a failure BEFORE the release commit exists (a partial version bump / lock refresh):
/// restore the two tracked release files from `HEAD`.
const RECOVER_FILES: &str = "restore the working tree with `git checkout -- Cargo.toml Cargo.lock`";
/// Remediation for a failure AFTER the release commit exists but before/at the tag: drop the commit
/// (this also restores the two files).
const RECOVER_COMMIT: &str = "undo the release commit with `git reset --hard HEAD~1`";

/// Append a remediation hint to a mutating effect's error, so a mutate-then-fail surfaces a CLEAR
/// recovery path (mirroring the push-gate-mismatch message) instead of a raw backend error.
fn with_recovery<T>(step: Result<T, String>, recovery: &str) -> Result<T, String> {
    step.map_err(|e| format!("{e} — the release did NOT complete; {recovery}"))
}

/// The REAL mutation path: two typed-tag confirmations gating the version bump, commit, tag, push.
///
/// The effects run in order — set version → refresh `Cargo.lock` → commit → annotated tag → a SINGLE
/// atomic push — and each mutating step carries a remediation hint on failure, so a mutate-then-fail
/// surfaces a CLEAR recovery path (never a raw backend error). The reachable intermediate half-states
/// and their remediation:
/// - fail at set-version / cargo-update / commit (files changed, no release commit yet):
///   `git checkout -- Cargo.toml Cargo.lock`.
/// - fail at the annotated tag (the release commit exists, no tag yet): `git reset --hard HEAD~1`.
/// - stop at the push gate, or fail during the push (commit + tag exist, nothing pushed):
///   `git tag -d <tag>` then `git reset --hard HEAD~1`.
///
/// The push is a single `git push --atomic origin main <tag>` (both refs advance or neither does), so
/// `origin/main` can never be published without its tag, and a non-fast-forward race aborts both.
///
/// # Errors
/// Returns `Err` if either typed confirmation does not match `tag` (mismatch = abort), or if any
/// git/cargo/filesystem effect fails. On a first-gate mismatch NOTHING has changed; every later
/// failure carries the matching recovery hint listed above.
fn execute(
    env: &dyn ReleaseEnv,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    new: &Version,
    tag: &str,
    palette: Palette,
) -> Result<(), String> {
    irreversible_banner(out, palette, tag)?;
    let typed = ask_line(
        input,
        out,
        &format!("Type the tag `{tag}` exactly to CONFIRM (anything else aborts): "),
    )?;
    if typed != tag {
        return Err(format!(
            "confirmation {typed:?} != {tag} — aborted; NO changes were made"
        ));
    }

    let message = format!("release {tag}");
    with_recovery(env.set_workspace_version(new), RECOVER_FILES)?;
    with_recovery(env.cargo_update_workspace(), RECOVER_FILES)?;
    with_recovery(
        env.git_commit_all(&format!("release: {tag}")),
        RECOVER_FILES,
    )?;
    with_recovery(env.git_annotated_tag(tag, &message), RECOVER_COMMIT)?;

    let push_confirm = ask_line(
        input,
        out,
        &format!(
            "Local commit + tag `{tag}` created. Type `{tag}` AGAIN to PUSH (irreversible), anything else stops: "
        ),
    )?;
    if push_confirm != tag {
        return Err(format!(
            "push confirmation {push_confirm:?} != {tag} — STOPPED before push; the local commit + \
             tag remain (undo with `git tag -d {tag}` then `git reset --hard HEAD~1`)"
        ));
    }
    with_recovery(
        env.git_push_atomic("main", tag),
        &format!("undo with `git tag -d {tag}` then `git reset --hard HEAD~1`"),
    )?;
    writeln!(
        out,
        "{}",
        palette.paint(
            STYLE_OK,
            &format!("pushed {tag} to origin — the dist release workflow is now running.")
        )
    )
    .map_err(|e| io_err(&e))?;
    Ok(())
}

/// Drive the full release flow over the effect seam and the input/output streams.
///
/// `--dry-run` runs every read-only step and stops before [`execute`], printing the `[dry-run]`
/// plan; a real run proceeds into [`execute`]'s typed-confirmation gates.
///
/// # Errors
/// Returns `Err` from pre-flight, prompting, compute, the tag-existence guard, or (real run) the
/// mutation path.
fn orchestrate(
    env: &dyn ReleaseEnv,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    dry_run: bool,
    palette: Palette,
) -> Result<(), String> {
    show_header(out, palette)?;
    preflight(env, out, palette)?;

    let kind = ask(input, out, KIND_PROMPT, parse_kind)?;
    let bump = ask(input, out, BUMP_PROMPT, parse_bump)?;

    let current = env.current_version()?;
    let mut tags = env.local_tags()?;
    tags.extend(env.remote_tags()?);

    let new = compute_target(&current, kind, bump, &tags)?;
    let tag = tag_of(&new);

    if tags.iter().any(|t| t == &tag) {
        return Err(format!(
            "computed tag {tag} already exists (local or origin) — aborting"
        ));
    }

    show_plan(out, &current, &new, &tag, kind, palette)?;

    if dry_run {
        writeln!(
            out,
            "[dry-run] would: set [workspace.package] version = \"{new}\" in Cargo.toml"
        )
        .map_err(|e| io_err(&e))?;
        writeln!(out, "[dry-run] would: cargo update --workspace").map_err(|e| io_err(&e))?;
        writeln!(
            out,
            "[dry-run] would: git add -- Cargo.toml Cargo.lock && git commit -m \"release: {tag}\""
        )
        .map_err(|e| io_err(&e))?;
        writeln!(
            out,
            "[dry-run] would: git tag -a {tag} -m \"release {tag}\""
        )
        .map_err(|e| io_err(&e))?;
        writeln!(out, "[dry-run] would: git push --atomic origin main {tag}")
            .map_err(|e| io_err(&e))?;
        writeln!(out, "[dry-run] no changes made; working tree untouched")
            .map_err(|e| io_err(&e))?;
        return Ok(());
    }

    execute(env, input, out, &new, &tag, palette)
}

// ---------------------------------------------------------------------------------------------
// Entry point.
// ---------------------------------------------------------------------------------------------

/// Entry point for `cargo xtask release [--dry-run]`.
#[must_use]
pub fn run() -> ExitCode {
    let dry_run = std::env::args().any(|a| a == "--dry-run");
    // STDOUT styling: decided ONCE from stdout's TTY-ness + the env signals, threaded through the
    // flow's `out` sink.
    let palette = Palette {
        color: detect_color(),
    };
    // STDERR styling is a SEPARATE decision keyed on stderr's OWN TTY-ness, so a redirected stderr
    // (`2>log`) stays plain even when stdout is a TTY. Used for the spinner + the error lines below.
    let err_palette = Palette {
        color: detect_err_color(),
    };

    let root = match workspace_root() {
        Ok(root) => root,
        Err(err) => {
            eprintln!(
                "{}",
                err_palette.paint(
                    STYLE_ERR,
                    &format!("release: could not locate workspace root: {err}")
                )
            );
            return ExitCode::FAILURE;
        }
    };
    let env = GitEnv {
        root,
        color: err_palette.color,
    };

    let stdin = io::stdin();
    let mut input = stdin.lock();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    match orchestrate(&env, &mut input, &mut out, dry_run, palette) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!(
                "{}",
                err_palette.paint(STYLE_ERR, &format!("release: {err}"))
            );
            ExitCode::FAILURE
        }
    }
}

/// Resolve the workspace root from `CARGO_MANIFEST_DIR` (xtask sits one level under the root).
fn workspace_root() -> Result<PathBuf, String> {
    let manifest = std::env::var("CARGO_MANIFEST_DIR")
        .map_err(|_| "CARGO_MANIFEST_DIR not set (run via `cargo xtask release`)".to_owned())?;
    Path::new(&manifest)
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("CARGO_MANIFEST_DIR {manifest:?} has no parent"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn v(s: &str) -> Version {
        Version::parse(s).expect("valid test version")
    }

    // ---- Pure compute: bump_core from 1.0.0 and 1.2.3. ----

    #[test]
    fn bump_core_from_1_0_0() {
        let base = v("1.0.0");
        assert_eq!(bump_core(&base, Bump::None), v("1.0.0"));
        assert_eq!(bump_core(&base, Bump::Patch), v("1.0.1"));
        assert_eq!(bump_core(&base, Bump::Minor), v("1.1.0"));
        assert_eq!(bump_core(&base, Bump::Major), v("2.0.0"));
        // Non-vacuity: the four bumps are genuinely distinct.
        assert_ne!(bump_core(&base, Bump::Patch), bump_core(&base, Bump::Minor));
        assert_ne!(bump_core(&base, Bump::Minor), bump_core(&base, Bump::Major));
    }

    #[test]
    fn bump_core_from_1_2_3() {
        let base = v("1.2.3");
        assert_eq!(bump_core(&base, Bump::None), v("1.2.3"));
        assert_eq!(bump_core(&base, Bump::Patch), v("1.2.4"));
        assert_eq!(bump_core(&base, Bump::Minor), v("1.3.0")); // zeroes patch
        assert_eq!(bump_core(&base, Bump::Major), v("2.0.0")); // zeroes minor + patch
    }

    #[test]
    fn bump_core_strips_existing_pre_release() {
        // A `-rc.N` core is stripped BEFORE the bump: 1.2.3-rc.2 finalises to 1.2.3, patches to 1.2.4.
        let rc = v("1.2.3-rc.2");
        assert_eq!(bump_core(&rc, Bump::None), v("1.2.3"));
        assert_eq!(bump_core(&rc, Bump::Patch), v("1.2.4"));
        assert_eq!(bump_core(&rc, Bump::Minor), v("1.3.0"));
    }

    // ---- Pure compute: rc numbering. ----

    #[test]
    fn first_rc_is_one() {
        let core = v("1.0.0");
        assert_eq!(next_rc_number(&[], &core), 1);
        // A tag for a DIFFERENT core does not count.
        assert_eq!(next_rc_number(&["v0.9.0-rc.5".to_owned()], &core), 1);
    }

    #[test]
    fn rc_increments_from_existing() {
        let core = v("1.0.0");
        assert_eq!(next_rc_number(&["v1.0.0-rc.1".to_owned()], &core), 2);
        // Highest existing wins, order-independent, non-contiguous gaps allowed.
        let tags = [
            "v1.0.0-rc.1".to_owned(),
            "v1.0.0-rc.3".to_owned(),
            "v1.0.0".to_owned(), // a final tag has no rc number → ignored
        ];
        assert_eq!(next_rc_number(&tags, &core), 4);
    }

    #[test]
    fn compute_target_pre_and_final() {
        // Final none from 1.0.0 → 1.0.0; final minor → 1.1.0.
        assert_eq!(
            compute_target(&v("1.0.0"), Kind::Final, Bump::None, &[]).unwrap(),
            v("1.0.0")
        );
        assert_eq!(
            compute_target(&v("1.0.0"), Kind::Final, Bump::Minor, &[]).unwrap(),
            v("1.1.0")
        );
        // Pre-release none from 1.0.0 (no tags) → 1.0.0-rc.1.
        assert_eq!(
            compute_target(&v("1.0.0"), Kind::PreRelease, Bump::None, &[]).unwrap(),
            v("1.0.0-rc.1")
        );
        // Pre-release minor from 1.0.0 with an existing rc for the SAME target core → rc.2.
        assert_eq!(
            compute_target(
                &v("1.0.0"),
                Kind::PreRelease,
                Bump::Minor,
                &["v1.1.0-rc.1".to_owned()]
            )
            .unwrap(),
            v("1.1.0-rc.2")
        );
    }

    // ---- Pure: tag formatting + with_rc. ----

    #[test]
    fn tag_formatting() {
        assert_eq!(tag_of(&v("1.1.0")), "v1.1.0");
        assert_eq!(tag_of(&v("2.0.0")), "v2.0.0");
        assert_eq!(tag_of(&v("1.0.0-rc.1")), "v1.0.0-rc.1");
        assert_eq!(tag_of(&with_rc(&v("1.2.3"), 7).unwrap()), "v1.2.3-rc.7");
    }

    // ---- Pure: Cargo.toml version edit. ----

    const SAMPLE_TOML: &str = "\
[workspace]
resolver = \"2\"

[workspace.package]
version = \"1.0.0\"
edition = \"2024\"

[workspace.dependencies]
serde = { version = \"1\" }
libsql = { version = \"0.9.30\" }
";

    #[test]
    fn parses_workspace_version() {
        assert_eq!(parse_workspace_version(SAMPLE_TOML).unwrap(), v("1.0.0"));
    }

    #[test]
    fn replaces_only_workspace_package_version() {
        let out = replace_workspace_version(SAMPLE_TOML, "1.1.0").unwrap();
        assert!(out.contains("[workspace.package]\nversion = \"1.1.0\""));
        // Dependency versions are untouched (only the [workspace.package] key changes).
        assert!(out.contains("serde = { version = \"1\" }"));
        assert!(out.contains("libsql = { version = \"0.9.30\" }"));
        assert!(!out.contains("version = \"1.0.0\""));
        assert!(out.ends_with('\n'));
        // Round-trips: the rewritten TOML parses back to the new version.
        assert_eq!(parse_workspace_version(&out).unwrap(), v("1.1.0"));
    }

    #[test]
    fn version_edit_errors_without_workspace_package() {
        assert!(find_workspace_version_line("[workspace]\nresolver = \"2\"\n").is_err());
    }

    // ---- Prompt parsing. ----

    #[test]
    fn parse_prompts() {
        assert_eq!(parse_kind("1"), Some(Kind::PreRelease));
        assert_eq!(parse_kind("pre-release"), Some(Kind::PreRelease));
        assert_eq!(parse_kind("RC"), Some(Kind::PreRelease));
        assert_eq!(parse_kind("2"), Some(Kind::Final));
        assert_eq!(parse_kind("final"), Some(Kind::Final));
        assert_eq!(parse_kind("bogus"), None);
        assert_eq!(parse_bump("1"), Some(Bump::None));
        assert_eq!(parse_bump("patch"), Some(Bump::Patch));
        assert_eq!(parse_bump("3"), Some(Bump::Minor));
        assert_eq!(parse_bump("MAJOR"), Some(Bump::Major));
        assert_eq!(parse_bump(""), None);
    }

    // ---- Orchestration over a FAKE env (no real repo mutation). ----

    /// A fake [`ReleaseEnv`] with configurable reads that RECORDS every mutating call, so a test can
    /// assert dry-run performed zero mutations and a real run performed exactly the expected ones.
    ///
    /// `fail_on` names a single mutating effect (`set_version` / `cargo_update` / `commit` / `tag` /
    /// `push`) that returns `Err` WITHOUT recording — so a fail-path test can assert the sequence
    /// STOPS there (no later effect fires) and the surfaced error carries the step's recovery hint.
    struct FakeEnv {
        branch: String,
        clean: bool,
        matches: bool,
        version: Version,
        local: Vec<String>,
        remote: Vec<String>,
        fail_on: Option<String>,
        calls: RefCell<Vec<String>>,
    }

    impl FakeEnv {
        fn ok() -> Self {
            FakeEnv {
                branch: "main".to_owned(),
                clean: true,
                matches: true,
                version: v("1.0.0"),
                local: Vec::new(),
                remote: Vec::new(),
                fail_on: None,
                calls: RefCell::new(Vec::new()),
            }
        }
        fn record(&self, s: &str) {
            self.calls.borrow_mut().push(s.to_owned());
        }
        fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
        /// Run a mutating effect: if `name` is the injected `fail_on`, return `Err` (recording
        /// nothing, so the effect did NOT "succeed"); otherwise record `label` and succeed.
        fn effect(&self, label: &str, name: &str) -> Result<(), String> {
            if self.fail_on.as_deref() == Some(name) {
                return Err(format!("injected fake failure at `{name}`"));
            }
            self.record(label);
            Ok(())
        }
    }

    impl ReleaseEnv for FakeEnv {
        fn current_branch(&self) -> Result<String, String> {
            Ok(self.branch.clone())
        }
        fn working_tree_clean(&self) -> Result<bool, String> {
            Ok(self.clean)
        }
        fn fetch_origin(&self) -> Result<(), String> {
            Ok(())
        }
        fn main_matches_origin(&self) -> Result<bool, String> {
            Ok(self.matches)
        }
        fn local_tags(&self) -> Result<Vec<String>, String> {
            Ok(self.local.clone())
        }
        fn remote_tags(&self) -> Result<Vec<String>, String> {
            Ok(self.remote.clone())
        }
        fn current_version(&self) -> Result<Version, String> {
            Ok(self.version.clone())
        }
        fn set_workspace_version(&self, new: &Version) -> Result<(), String> {
            self.effect(&format!("set_version {new}"), "set_version")
        }
        fn cargo_update_workspace(&self) -> Result<(), String> {
            self.effect("cargo_update", "cargo_update")
        }
        fn git_commit_all(&self, message: &str) -> Result<(), String> {
            self.effect(&format!("commit {message}"), "commit")
        }
        fn git_annotated_tag(&self, tag: &str, _message: &str) -> Result<(), String> {
            self.effect(&format!("tag {tag}"), "tag")
        }
        fn git_push_atomic(&self, branch: &str, tag: &str) -> Result<(), String> {
            self.effect(&format!("push_atomic {branch} {tag}"), "push")
        }
    }

    /// Run `orchestrate` with scripted stdin `answers` and color FORCED OFF, returning
    /// `(result, stdout, mutation calls)`. Color off ⇒ the captured `Vec<u8>` stays plain +
    /// deterministic (no ESC bytes), so the behavioral asserts + snapshots below are stable.
    fn drive(
        env: &FakeEnv,
        answers: &str,
        dry_run: bool,
    ) -> (Result<(), String>, String, Vec<String>) {
        drive_colored(env, answers, dry_run, false)
    }

    /// [`drive`] with an explicit color choice (used to pin the styled ANSI shape with color ON).
    fn drive_colored(
        env: &FakeEnv,
        answers: &str,
        dry_run: bool,
        color: bool,
    ) -> (Result<(), String>, String, Vec<String>) {
        let mut input = io::Cursor::new(answers.as_bytes().to_vec());
        let mut out: Vec<u8> = Vec::new();
        let res = orchestrate(env, &mut input, &mut out, dry_run, Palette { color });
        (
            res,
            String::from_utf8(out).expect("utf8 output"),
            env.calls(),
        )
    }

    #[test]
    fn dry_run_pre_none_yields_rc1_no_mutation() {
        let env = FakeEnv::ok();
        let (res, out, calls) = drive(&env, "1\n1\n", true);
        assert!(res.is_ok(), "dry-run pre/none should succeed: {res:?}");
        assert!(
            out.contains("v1.0.0-rc.1"),
            "plan should target v1.0.0-rc.1:\n{out}"
        );
        assert!(
            out.contains("[dry-run]"),
            "should print dry-run lines:\n{out}"
        );
        assert!(calls.is_empty(), "dry-run must not mutate, got {calls:?}");
        // Pin the full reflowed (boxed) plan + dry-run shape, color OFF (plain + deterministic).
        insta::assert_snapshot!("plan_dry_run_pre_none", out);
    }

    #[test]
    fn dry_run_final_minor_yields_1_1_0_no_mutation() {
        let env = FakeEnv::ok();
        let (res, out, calls) = drive(&env, "2\n3\n", true);
        assert!(res.is_ok(), "dry-run final/minor should succeed: {res:?}");
        // A final release carries no rc pre-release on the computed version/tag.
        assert!(
            !out.contains("v1.1.0-rc"),
            "final tag must not be an rc:\n{out}"
        );
        assert!(calls.is_empty(), "dry-run must not mutate, got {calls:?}");
        // The reflowed plan alignment (formerly asserted as `tag             : v1.1.0` /
        // `pre-release     : no (final)` substrings) is now pinned by snapshot, color OFF.
        insta::assert_snapshot!("plan_dry_run_final_minor", out);
    }

    #[test]
    fn preflight_rejects_non_main() {
        let mut env = FakeEnv::ok();
        env.branch = "feature".to_owned();
        let (res, _out, calls) = drive(&env, "2\n3\n", true);
        assert!(res.unwrap_err().contains("main"));
        assert!(calls.is_empty());
    }

    #[test]
    fn preflight_rejects_dirty_tree() {
        let mut env = FakeEnv::ok();
        env.clean = false;
        let (res, _out, calls) = drive(&env, "2\n3\n", true);
        assert!(res.unwrap_err().contains("clean"));
        assert!(calls.is_empty());
    }

    #[test]
    fn preflight_rejects_diverged_main() {
        let mut env = FakeEnv::ok();
        env.matches = false;
        let (res, _out, calls) = drive(&env, "2\n3\n", true);
        assert!(res.unwrap_err().contains("diverged"));
        assert!(calls.is_empty());
    }

    #[test]
    fn guard_rejects_existing_tag() {
        let mut env = FakeEnv::ok();
        env.remote = vec!["v1.1.0".to_owned()]; // the computed final/minor tag already exists remotely
        let (res, _out, calls) = drive(&env, "2\n3\n", true);
        assert!(res.unwrap_err().contains("already exists"));
        assert!(calls.is_empty());
    }

    #[test]
    fn re_prompts_on_invalid_then_accepts() {
        let env = FakeEnv::ok();
        // First answers are invalid; the loop must re-prompt and then accept `2` / `3`.
        let (res, out, _calls) = drive(&env, "x\n2\nnope\n3\n", true);
        assert!(res.is_ok(), "should recover after invalid answers: {res:?}");
        assert!(
            out.matches("invalid choice").count() >= 2,
            "should re-prompt twice:\n{out}"
        );
        assert!(out.contains("v1.1.0"));
    }

    #[test]
    fn real_run_aborts_on_first_confirmation_mismatch_without_mutation() {
        let env = FakeEnv::ok();
        // final/minor → tag v1.1.0; the typed confirmation is WRONG → abort, zero mutations.
        let (res, _out, calls) = drive(&env, "2\n3\nWRONG\n", false);
        assert!(res.unwrap_err().contains("NO changes were made"));
        assert!(
            calls.is_empty(),
            "a mismatched confirmation must not mutate, got {calls:?}"
        );
    }

    #[test]
    fn real_run_stops_before_push_on_second_mismatch() {
        let env = FakeEnv::ok();
        // First gate matches (v1.1.0) so the bump/commit/tag run; the PUSH gate is wrong → no push.
        let (res, _out, calls) = drive(&env, "2\n3\nv1.1.0\nWRONG\n", false);
        assert!(res.unwrap_err().contains("STOPPED before push"));
        assert_eq!(
            calls,
            vec![
                "set_version 1.1.0".to_owned(),
                "cargo_update".to_owned(),
                "commit release: v1.1.0".to_owned(),
                "tag v1.1.0".to_owned(),
            ],
            "the local commit + tag are created but NOTHING is pushed"
        );
        assert!(
            !calls.iter().any(|c| c.starts_with("push")),
            "must not push"
        );
    }

    #[test]
    fn real_run_executes_full_sequence_on_both_confirmations() {
        let env = FakeEnv::ok();
        // Both typed gates match `v1.1.0` → the full mutation sequence runs (against the FAKE only).
        let (res, out, calls) = drive(&env, "2\n3\nv1.1.0\nv1.1.0\n", false);
        assert!(
            res.is_ok(),
            "both confirmations should complete the flow: {res:?}"
        );
        assert_eq!(
            calls,
            vec![
                "set_version 1.1.0".to_owned(),
                "cargo_update".to_owned(),
                "commit release: v1.1.0".to_owned(),
                "tag v1.1.0".to_owned(),
                // A SINGLE atomic push of both refs (main + tag) — never two separate pushes.
                "push_atomic main v1.1.0".to_owned(),
            ],
            "the full ordered mutation sequence must fire exactly once, ending in ONE atomic push"
        );
        // Pin the styled real-run stdout (color OFF): the loud IRREVERSIBLE banner, both typed-gate
        // prompts, and the green success line.
        insta::assert_snapshot!("real_run_full_stdout", out);
    }

    // ---- Fail-path coverage: an intermediate effect fails → the sequence STOPS with recovery guidance. ----
    //
    // Every case drives BOTH typed gates matching `v1.1.0` (so absent the injected failure the full
    // sequence would run), injects a failure at one effect, and asserts (1) the effects BEFORE it
    // already fired in order, (2) the failing effect and everything after it did NOT fire, and (3) the
    // surfaced error carries the step's recovery hint from `execute`. These are non-vacuous: drop the
    // recovery hint and the `contains` assertion goes red; drop the fail-fast `?` and the extra
    // recorded calls make the `assert_eq!` go red.

    const BOTH_GATES: &str = "2\n3\nv1.1.0\nv1.1.0\n";

    fn failing_at(effect: &str) -> FakeEnv {
        let mut env = FakeEnv::ok();
        env.fail_on = Some(effect.to_owned());
        env
    }

    #[test]
    fn fail_at_set_version_stops_immediately_with_file_recovery() {
        let env = failing_at("set_version");
        let (res, _out, calls) = drive(&env, BOTH_GATES, false);
        let err = res.expect_err("set_version failure must surface an error");
        assert!(
            err.contains("git checkout -- Cargo.toml Cargo.lock"),
            "must advise restoring the files, got: {err}"
        );
        assert!(
            calls.is_empty(),
            "nothing must have recorded when the FIRST effect fails, got {calls:?}"
        );
    }

    #[test]
    fn fail_at_cargo_update_stops_after_set_version_with_file_recovery() {
        let env = failing_at("cargo_update");
        let (res, _out, calls) = drive(&env, BOTH_GATES, false);
        let err = res.expect_err("cargo_update failure must surface an error");
        assert!(
            err.contains("git checkout -- Cargo.toml Cargo.lock"),
            "must advise restoring the files, got: {err}"
        );
        assert_eq!(
            calls,
            vec!["set_version 1.1.0".to_owned()],
            "only set_version should have fired before the cargo_update failure, got {calls:?}"
        );
    }

    #[test]
    fn fail_at_commit_stops_after_files_with_file_recovery() {
        let env = failing_at("commit");
        let (res, _out, calls) = drive(&env, BOTH_GATES, false);
        let err = res.expect_err("commit failure must surface an error");
        // No commit exists yet, so the guidance is still to restore the files (not `reset`).
        assert!(
            err.contains("git checkout -- Cargo.toml Cargo.lock"),
            "must advise restoring the files (no commit yet), got: {err}"
        );
        assert_eq!(
            calls,
            vec!["set_version 1.1.0".to_owned(), "cargo_update".to_owned()],
            "only the two file mutations should have fired before the commit failure, got {calls:?}"
        );
    }

    #[test]
    fn fail_at_tag_stops_after_commit_with_reset_recovery() {
        let env = failing_at("tag");
        let (res, _out, calls) = drive(&env, BOTH_GATES, false);
        let err = res.expect_err("tag failure must surface an error");
        // The commit already exists → the recovery is to drop it with a hard reset.
        assert!(
            err.contains("git reset --hard HEAD~1"),
            "must advise undoing the release commit, got: {err}"
        );
        assert_eq!(
            calls,
            vec![
                "set_version 1.1.0".to_owned(),
                "cargo_update".to_owned(),
                "commit release: v1.1.0".to_owned(),
            ],
            "the commit should have fired but not the tag, got {calls:?}"
        );
    }

    #[test]
    fn fail_at_push_stops_after_tag_with_tag_delete_and_reset_recovery() {
        let env = failing_at("push");
        let (res, _out, calls) = drive(&env, BOTH_GATES, false);
        let err = res.expect_err("push failure must surface an error");
        // Commit + tag exist, nothing pushed → advise removing BOTH (mirrors the push-gate message).
        assert!(
            err.contains("git tag -d v1.1.0"),
            "must advise deleting the tag, got: {err}"
        );
        assert!(
            err.contains("git reset --hard HEAD~1"),
            "must advise undoing the release commit, got: {err}"
        );
        assert_eq!(
            calls,
            vec![
                "set_version 1.1.0".to_owned(),
                "cargo_update".to_owned(),
                "commit release: v1.1.0".to_owned(),
                "tag v1.1.0".to_owned(),
            ],
            "all LOCAL mutations should have fired but the push must be absent, got {calls:?}"
        );
        assert!(
            !calls.iter().any(|c| c.starts_with("push")),
            "the atomic push must not have recorded on failure, got {calls:?}"
        );
    }

    // ---- Presentation: color decision, styling primitive, plan reflow, spinner. ----
    //
    // The seam + safety model is unchanged; these cover the T3.8 DX layer: color is decided once
    // (never from the sink), color OFF emits ZERO ESC bytes (so tests stay deterministic), color ON
    // pins the ANSI shape, and the stderr spinner is testable against an injected buffer.

    #[test]
    fn resolve_color_honours_the_standard_precedence() {
        // NO_COLOR disables unconditionally — even with CLICOLOR_FORCE + a TTY.
        assert!(!resolve_color(true, true, Some(true), true));
        // CLICOLOR_FORCE forces ON even without a TTY and with CLICOLOR=0.
        assert!(resolve_color(false, true, Some(false), false));
        // CLICOLOR=0 disables on a TTY (no force).
        assert!(!resolve_color(false, false, Some(false), true));
        // Otherwise color follows the TTY (default + CLICOLOR=1 both still need the TTY).
        assert!(resolve_color(false, false, None, true));
        assert!(!resolve_color(false, false, None, false));
        assert!(resolve_color(false, false, Some(true), true));
        assert!(!resolve_color(false, false, Some(true), false));
    }

    #[test]
    fn color_from_env_maps_the_process_signals() {
        // The env→color glue (`detect_color` / `detect_err_color` delegate here); `os` builds the
        // `OsStr` a `var_os` read would yield.
        let os = OsStr::new;
        // NO_COLOR: only a NON-EMPTY value disables; EMPTY or ABSENT is "unset" (falls to the TTY).
        assert!(!color_from_env(Some(os("1")), None, None, true)); // set, non-empty → off
        assert!(color_from_env(Some(os("")), None, None, true)); // set but EMPTY → unset → TTY on
        assert!(color_from_env(None, None, None, true)); // absent → follows TTY (on)
        // NO_COLOR dominates even a TTY + CLICOLOR_FORCE.
        assert!(!color_from_env(Some(os("1")), Some(os("1")), None, true));
        // CLICOLOR_FORCE: forces on even without a TTY; a `0` or EMPTY value does NOT force.
        assert!(color_from_env(None, Some(os("1")), None, false));
        assert!(!color_from_env(None, Some(os("0")), None, false));
        assert!(!color_from_env(None, Some(os("")), None, false));
        // CLICOLOR: `0` disables even on a TTY; a non-empty non-`0` and "unset" both defer to the TTY.
        assert!(!color_from_env(None, None, Some(os("0")), true));
        assert!(color_from_env(None, None, Some(os("1")), true));
        assert!(!color_from_env(None, None, Some(os("1")), false)); // clicolor=1 but no TTY → off
        assert!(color_from_env(None, None, Some(os("")), true)); // EMPTY CLICOLOR → unset → TTY on
        assert!(!color_from_env(None, None, Some(os("")), false));
        // Precedence: CLICOLOR_FORCE overrides CLICOLOR=0 (and the missing TTY).
        assert!(color_from_env(None, Some(os("1")), Some(os("0")), false));
        // The IsTerminal fallback with every signal unset (redirected stderr `2>log` ⇒ is_terminal
        // false ⇒ plain — the fix-4 guarantee).
        assert!(color_from_env(None, None, None, true));
        assert!(!color_from_env(None, None, None, false));
    }

    #[test]
    fn painted_emits_ansi_only_when_color_on() {
        let on = Palette { color: true };
        let off = Palette { color: false };
        // Color ON wraps the text in an SGR sequence (contains ESC) and preserves the text.
        let styled = on.paint(STYLE_ERR, "x").to_string();
        assert!(styled.as_bytes().contains(&0x1b), "color on must emit ESC");
        assert!(styled.contains('x'));
        // Color OFF is byte-for-byte the plain text — the deterministic-test guarantee.
        assert_eq!(off.paint(STYLE_ERR, "x").to_string(), "x");
    }

    #[test]
    fn spinner_and_progress_lines_track_color() {
        let on = Palette { color: true };
        let off = Palette { color: false };
        assert!(
            spinner_frame(0, "fetching origin", on)
                .as_bytes()
                .contains(&0x1b)
        );
        let plain = spinner_frame(3, "fetching origin", off);
        assert!(!plain.as_bytes().contains(&0x1b));
        assert!(plain.contains("fetching origin"));
        // Frame index wraps over the frame set (no panic / index-out-of-bounds).
        assert!(spinner_frame(SPINNER_FRAMES.len() + 2, "x", off).contains('x'));
        assert!(progress_static_line("updating Cargo.lock", off).contains("updating Cargo.lock"));
        assert!(
            progress_done_line("updating Cargo.lock", off).contains("updating Cargo.lock done")
        );
        // The failure line names the step as `failed` (never `done`) and is styled red when color on.
        let fail = progress_fail_line("updating Cargo.lock", off);
        assert!(fail.contains("updating Cargo.lock failed"));
        assert!(
            !fail.contains("done"),
            "a failure line must not say `done`: {fail}"
        );
        assert!(progress_fail_line("x", on).as_bytes().contains(&0x1b));
    }

    #[test]
    fn plan_has_no_ansi_escape_when_color_off() {
        // The invariant that keeps every snapshot + substring assert stable: color OFF ⇒ no ESC.
        let env = FakeEnv::ok();
        let (res, out, _calls) = drive(&env, "2\n3\n", true);
        assert!(res.is_ok());
        assert!(
            !out.as_bytes().contains(&0x1b),
            "color-off output must contain no ESC (0x1b) byte:\n{out:?}"
        );
    }

    #[test]
    fn real_run_success_has_no_ansi_escape_when_color_off() {
        let env = FakeEnv::ok();
        let (res, out, _calls) = drive(&env, "2\n3\nv1.1.0\nv1.1.0\n", false);
        assert!(res.is_ok());
        assert!(
            !out.as_bytes().contains(&0x1b),
            "color-off real-run output (banner + success) must contain no ESC byte:\n{out:?}"
        );
    }

    #[test]
    fn styled_plan_pins_the_ansi_shape_when_color_on() {
        let env = FakeEnv::ok();
        let (res, out, _calls) = drive_colored(&env, "2\n3\n", true, true);
        assert!(res.is_ok());
        assert!(
            out.as_bytes().contains(&0x1b),
            "color-on output must carry ANSI (ESC) bytes"
        );
        insta::assert_snapshot!("plan_dry_run_final_minor_color_on", out);
    }

    #[test]
    fn styled_real_run_pins_banner_and_success_ansi_when_color_on() {
        let env = FakeEnv::ok();
        // Both typed gates match `v1.1.0` and color is ON, so `execute()` renders the LOUD
        // white-on-red IRREVERSIBLE banner (STYLE_LOUD) and the green success line (STYLE_OK) — this
        // pins their ANSI so dropping the banner style/bg or the success color would go red.
        let (res, out, calls) = drive_colored(&env, "2\n3\nv1.1.0\nv1.1.0\n", false, true);
        assert!(res.is_ok(), "both confirmations should complete: {res:?}");
        assert!(
            out.as_bytes().contains(&0x1b),
            "color-on real run must carry ANSI (ESC) bytes"
        );
        // The safety model is unchanged under color: the full ordered mutation sequence still fires.
        assert_eq!(
            calls,
            vec![
                "set_version 1.1.0".to_owned(),
                "cargo_update".to_owned(),
                "commit release: v1.1.0".to_owned(),
                "tag v1.1.0".to_owned(),
                "push_atomic main v1.1.0".to_owned(),
            ],
            "color must not perturb the mutation sequence"
        );
        insta::assert_snapshot!("real_run_full_stdout_color_on", out);
    }

    /// A `Write` over a shared buffer so a test can capture BOTH the caller-thread (static) and the
    /// spinner-thread (animated) writes without touching the real stderr fd.
    struct SharedBuf(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let mut guard = self
                .0
                .lock()
                .map_err(|_| io::Error::other("poisoned shared buffer"))?;
            guard.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn with_progress_static_branch_runs_step_and_reports_ok_outcome() {
        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        // is_tty = false ⇒ the static-fallback branch runs inline on this thread.
        let got: Result<i32, String> = with_progress(
            SharedBuf(std::sync::Arc::clone(&buf)),
            "fetching origin",
            Palette { color: false },
            false,
            || Ok(5),
        );
        assert_eq!(got, Ok(5), "must return the slow step's Result");
        let s = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(
            s.contains("fetching origin"),
            "static line must name the step:\n{s}"
        );
        // An OK step reports the green "done" line — never the "failed" line.
        assert!(
            s.contains("fetching origin done"),
            "an OK static step reports done:\n{s}"
        );
        assert!(
            !s.contains("failed"),
            "an OK step must not report failed:\n{s}"
        );
        assert!(!s.as_bytes().contains(&0x1b), "color off ⇒ no ESC");
    }

    #[test]
    fn with_progress_animated_branch_runs_joins_and_reports_done() {
        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        // is_tty = true ⇒ the spinner thread renders frames, then (on join) a "done" line. The
        // injected buffer proves the thread started, was joined cleanly, and reported completion —
        // with NO real-stderr leak.
        let got: Result<i32, String> = with_progress(
            SharedBuf(std::sync::Arc::clone(&buf)),
            "updating Cargo.lock",
            Palette { color: false },
            true,
            || {
                thread::sleep(Duration::from_millis(5));
                Ok(9)
            },
        );
        assert_eq!(got, Ok(9), "must return the slow step's Result");
        let s = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(
            s.contains("updating Cargo.lock done"),
            "the animated path must clear + print a done line after joining:\n{s}"
        );
    }

    #[test]
    fn with_progress_failed_step_never_reports_success() {
        // Animated branch (is_tty = true): a FAILING step must NOT print the green "✓ … done" success
        // line — it prints the red ✗ "failed" line instead, and the `Err` propagates. Non-vacuous:
        // revert the spinner's outcome threading and the `!… done` assert goes red.
        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let got: Result<(), String> = with_progress(
            SharedBuf(std::sync::Arc::clone(&buf)),
            "fetching origin",
            Palette { color: false },
            true,
            || Err("offline".to_owned()),
        );
        assert_eq!(
            got,
            Err("offline".to_owned()),
            "the step error must propagate"
        );
        let s = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(
            !s.contains("fetching origin done"),
            "a FAILED animated step must not print the success `… done` line:\n{s}"
        );
        assert!(
            s.contains("fetching origin failed"),
            "a FAILED animated step should print the failure line:\n{s}"
        );

        // Static branch (is_tty = false): same guarantee.
        let buf2 = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let got2: Result<(), String> = with_progress(
            SharedBuf(std::sync::Arc::clone(&buf2)),
            "updating Cargo.lock",
            Palette { color: false },
            false,
            || Err("boom".to_owned()),
        );
        assert_eq!(got2, Err("boom".to_owned()));
        let s2 = String::from_utf8(buf2.lock().unwrap().clone()).unwrap();
        assert!(
            !s2.contains("updating Cargo.lock done"),
            "a FAILED static step must not print the success `… done` line:\n{s2}"
        );
        assert!(
            s2.contains("updating Cargo.lock failed"),
            "a FAILED static step should print the failure line:\n{s2}"
        );
    }

    #[test]
    fn spinner_starts_and_stops_without_deadlock() {
        // Directly exercise the thread lifecycle against a sink (no output assertion needed): start,
        // let it tick, stop → must join cleanly with no panic / deadlock. `Drop` is the safety net.
        let spinner = Spinner::start(
            io::sink(),
            "fetching origin".to_owned(),
            Palette { color: false },
            20,
        );
        thread::sleep(Duration::from_millis(5));
        spinner.stop(true);
    }
}
