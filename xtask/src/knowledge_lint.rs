//! `knowledge-lint` — the `.knowledge/` structural lint (checks k1..k6; ci-cd §2.3).
//!
//! A sibling of `doc_lint` with a **separate, dynamic corpus**: it walks `.knowledge/**` (memories +
//! wiki run-reports/topics), guarded by a fixed-skeleton structure guard, and enforces the six
//! knowledge checks k1..k6. It deliberately does NOT extend the 19-file normative corpus — doc-lint
//! classes a..f must never run over `.knowledge` (a run-report legitimately quotes superseded ids and
//! dead spellings; descriptive history is not drift), and a knowledge finding must never read as a
//! doc-lint a..f finding (hence the `k` namespace).
//!
//! Out-of-tree point-reads (both resolved against `lint_at`'s root, never the cwd):
//! - `.unblock/issues.jsonl` — the k4 issues-resolve check + the k6 comment token scan (top-level
//!   `id` FIELD parse via `serde_json`; a substring grep is forbidden — ids also appear in prose).
//! - the `CLAUDE.md` `@`-import closure (max depth 5) — the decision-10 no-import check.
//!
//! Absent or unreadable inputs are structure-guard `Err`s (hard FAIL): a k-check that cannot read
//! its inputs must block, never skip. Run: `cargo xtask knowledge-lint`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use regex::Regex;

use crate::doc_lint::{code_spans, fence_mask};

/// Valid `type:` values for memory pages (deliberately descriptive-only — no decision/constraint
/// kinds, which would invite normative content).
const MEMORY_TYPES: &[&str] = &["gotcha", "recipe", "reference", "environment"];
/// Valid `type:` for `wiki/runs/*` pages (the dir is the kind; frontmatter must agree).
const RUN_TYPE: &str = "run";
/// Valid `type:` for `wiki/topics/*` pages.
const TOPIC_TYPE: &str = "topic";
/// The exact H2 section set of `wiki/index.md`, in order.
const WIKI_SECTIONS: &[&str] = &["Runs", "Topics"];
/// Valid topic categories (H3 headings under `## Topics`).
const TOPIC_CATEGORIES: &[&str] = &[
    "orchestration",
    "git-and-worktrees",
    "ci-and-quality-gates",
    "testing-and-benches",
    "release-and-distribution",
    "mcp-and-agents",
    "environment-and-tooling",
];
/// Glossary empty-body sentinel (must equal the template literal).
const GLOSSARY_NONE: &str = "No session-local ids were used in this run.";
/// The session-local-id pattern — single-sourced with the sh const in
/// `scripts/knowledge/run-report-gate.sh` and the landed ci-cd §2.3.3 literal (both pinned by the
/// gate selftest; the Rust side is pinned by a unit test below).
const SESSION_LOCAL_ID_RE: &str = "(^|[^A-Za-z0-9-])(MF|CF|M|R|F|A)-?[0-9]+([^0-9]|$)";
/// `@`-import recursion cap (Claude Code's own import-hop limit).
const IMPORT_MAX_DEPTH: usize = 5;

/// A single knowledge-lint finding (`path:line: [kN] message`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// Root-relative path of the offending file.
    pub file: String,
    /// 1-based line number (line 1 for whole-file conditions).
    pub line: usize,
    /// The check that fired.
    pub check: Check,
    /// Human-readable description of the violation.
    pub message: String,
}

/// The six checks. Renders as "k1".."k6".
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Check {
    /// index→file resolution + index grammar/agreement.
    K1Index,
    /// file→index orphans + structural strays.
    K2Orphan,
    /// frontmatter validity (per-kind schema).
    K3Frontmatter,
    /// enum, category & value agreement + issues-resolve + the no-import check.
    K4Values,
    /// slug/filename rules.
    K5Slug,
    /// run-report mandatory sections + glossary token coverage.
    K6RunSections,
}

impl Check {
    /// The two-char check code ("k1".."k6").
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Check::K1Index => "k1",
            Check::K2Orphan => "k2",
            Check::K3Frontmatter => "k3",
            Check::K4Values => "k4",
            Check::K5Slug => "k5",
            Check::K6RunSections => "k6",
        }
    }
}

/// Entry point for `cargo xtask knowledge-lint`.
#[must_use]
pub fn knowledge_lint() -> ExitCode {
    let root = match workspace_root() {
        Ok(root) => root,
        Err(err) => {
            eprintln!("knowledge-lint: could not locate workspace root: {err}");
            return ExitCode::FAILURE;
        }
    };
    match lint_at(&root) {
        Ok(findings) => report(&findings, page_count(&root)),
        Err(err) => {
            eprintln!("knowledge-lint: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Resolve the workspace root from `CARGO_MANIFEST_DIR` (set by cargo for the running xtask crate).
fn workspace_root() -> Result<PathBuf, String> {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").map_err(|_| {
        "CARGO_MANIFEST_DIR not set (run via `cargo xtask knowledge-lint`)".to_owned()
    })?;
    Path::new(&manifest)
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("CARGO_MANIFEST_DIR {manifest:?} has no parent"))
}

/// Count content pages for the OK line (best-effort; the lint itself re-walks).
fn page_count(root: &Path) -> usize {
    ["memories", "wiki/runs", "wiki/topics"]
        .iter()
        .map(|d| {
            std::fs::read_dir(root.join(".knowledge").join(d)).map_or(0, |rd| {
                rd.flatten()
                    .filter(|e| {
                        let name = e.file_name();
                        let name = name.to_string_lossy();
                        Path::new(name.as_ref())
                            .extension()
                            .is_some_and(|x| x == "md")
                            && name != "index.md"
                    })
                    .count()
            })
        })
        .sum()
}

/// Emit findings (stderr) + the final tally; return the process exit code.
fn report(findings: &[Finding], pages: usize) -> ExitCode {
    if findings.is_empty() {
        println!("knowledge-lint OK: {pages} pages, 6 checks clean");
        return ExitCode::SUCCESS;
    }
    let mut per_check: BTreeMap<&'static str, usize> = BTreeMap::new();
    for f in findings {
        eprintln!("{}:{}: [{}] {}", f.file, f.line, f.check.code(), f.message);
        *per_check.entry(f.check.code()).or_default() += 1;
    }
    let tally: Vec<String> = ["k1", "k2", "k3", "k4", "k5", "k6"]
        .iter()
        .map(|c| format!("{c}:{}", per_check.get(c).copied().unwrap_or(0)))
        .collect();
    eprintln!(
        "knowledge-lint: {} findings ({})",
        findings.len(),
        tally.join(" ")
    );
    ExitCode::FAILURE
}

/// Testable core (the `lint_at` pattern). `Err` = the structure guard tripped.
///
/// # Errors
/// Returns `Err` when the `.knowledge` skeleton is missing/unreadable, or when an out-of-tree
/// point-read (`.unblock/issues.jsonl`, the `CLAUDE.md` `@`-import closure) is absent, unreadable,
/// or corrupt — a vacuous pass is not a pass.
pub fn lint_at(root: &Path) -> Result<Vec<Finding>, String> {
    guard_skeleton(root)?;
    let export = load_export(root)?;
    let mut findings = Vec::new();
    scan_import_closure(root, &mut findings)?;
    let tree = walk_knowledge(root, &mut findings)?;
    let pages = parse_pages(&tree, &mut findings);
    lint_memories_index(&tree, &pages, &mut findings);
    lint_wiki_index(&tree, &pages, &mut findings);
    for page in pages.values().filter(|p| p.kind == Kind::Run) {
        lint_run_sections(page, &mut findings);
        lint_run_tokens(page, &export, &mut findings)?;
        for id in &page.issues {
            if !export.ids.contains(id) {
                findings.push(Finding {
                    file: page.rel.clone(),
                    line: page.issues_line,
                    check: Check::K4Values,
                    message: format!("run cites issue '{id}' not present in .unblock/issues.jsonl"),
                });
            }
        }
    }
    findings.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.line.cmp(&b.line))
            .then(a.check.cmp(&b.check))
    });
    Ok(findings)
}

// ---------------------------------------------------------------------------------------------
// Structure guard + out-of-tree point-reads.
// ---------------------------------------------------------------------------------------------

/// The fixed skeleton: both index files + both wiki content dirs.
fn guard_skeleton(root: &Path) -> Result<(), String> {
    let mut missing = Vec::new();
    for rel in [".knowledge/memories/index.md", ".knowledge/wiki/index.md"] {
        if !root.join(rel).is_file() {
            missing.push(rel);
        }
    }
    for rel in [".knowledge/wiki/runs", ".knowledge/wiki/topics"] {
        if !root.join(rel).is_dir() {
            missing.push(rel);
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "knowledge structure incomplete — missing: {} (an absent skeleton is a vacuous pass; FAIL)",
            missing.join(", ")
        ))
    }
}

/// A guard error for an absent/unreadable out-of-tree read (a vacuous k4 pass is not a pass).
fn out_of_tree_err(path: &str) -> String {
    format!(
        "knowledge structure incomplete — missing: {path} (out-of-tree read; a vacuous k4 pass is not a pass; FAIL)"
    )
}

/// One issue comment as read from the export (only the fields the checks consume).
struct ExportComment {
    created_at: Option<String>,
    text: String,
}

/// The parsed `.unblock/issues.jsonl`: the top-level id set + per-record comments.
struct Export {
    ids: BTreeSet<String>,
    comments: BTreeMap<String, Vec<ExportComment>>,
}

/// Parse the export per the ci-cd §2.3.2 point-read spec: each non-empty line is a JSON object with
/// a string top-level `id`; `comments`, when present, must be an array of objects with string `text`.
fn load_export(root: &Path) -> Result<Export, String> {
    let rel = ".unblock/issues.jsonl";
    let raw = std::fs::read_to_string(root.join(rel)).map_err(|_| out_of_tree_err(rel))?;
    let mut ids = BTreeSet::new();
    let mut comments = BTreeMap::new();
    for (idx, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(line).map_err(|e| {
            format!(
                "knowledge structure incomplete — corrupt export {rel}:{}: not JSON ({e}); fail-closed",
                idx + 1
            )
        })?;
        let obj = value.as_object().ok_or_else(|| {
            format!(
                "knowledge structure incomplete — corrupt export {rel}:{}: not a JSON object; fail-closed",
                idx + 1
            )
        })?;
        let id = obj.get("id").and_then(serde_json::Value::as_str).ok_or_else(|| {
            format!(
                "knowledge structure incomplete — corrupt export {rel}:{}: no string top-level 'id'; fail-closed",
                idx + 1
            )
        })?;
        ids.insert(id.to_owned());
        if let Some(c) = obj.get("comments") {
            let arr = c.as_array().ok_or_else(|| {
                format!(
                    "knowledge structure incomplete — corrupt export {rel}:{}: 'comments' is not an array; fail-closed",
                    idx + 1
                )
            })?;
            let mut parsed = Vec::with_capacity(arr.len());
            for member in arr {
                let text = member
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        format!(
                            "knowledge structure incomplete — corrupt export {rel}:{}: a comment member lacks a string 'text'; fail-closed",
                            idx + 1
                        )
                    })?;
                let created_at = member
                    .get("created_at")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
                parsed.push(ExportComment {
                    created_at,
                    text: text.to_owned(),
                });
            }
            comments.insert(id.to_owned(), parsed);
        }
    }
    Ok(Export { ids, comments })
}

/// Scan the `CLAUDE.md` `@`-import closure (decision 10): CLAUDE.md + every repo-local file it
/// transitively imports; any member `@`-importing a target whose root-resolved path lies under
/// `root/.knowledge` is a k4 finding. Absent/unreadable closure members are guard `Err`s.
fn scan_import_closure(root: &Path, findings: &mut Vec<Finding>) -> Result<(), String> {
    let import_re =
        Regex::new(r"(?:^|[\s(])@([A-Za-z0-9_~./][A-Za-z0-9_./-]*)").expect("valid import regex");
    let mut visited: BTreeSet<String> = BTreeSet::new();
    let mut queue: Vec<(String, usize)> = vec![("CLAUDE.md".to_owned(), 0)];
    while let Some((rel, depth)) = queue.pop() {
        if !visited.insert(rel.clone()) {
            continue;
        }
        let raw = std::fs::read_to_string(root.join(&rel)).map_err(|_| out_of_tree_err(&rel))?;
        let lines: Vec<String> = raw.lines().map(str::to_owned).collect();
        let mask = fence_mask(&lines);
        for (i, line) in lines.iter().enumerate() {
            if mask[i] {
                continue;
            }
            let scrubbed = scrub_code_spans(line);
            for cap in import_re.captures_iter(&scrubbed) {
                let target = &cap[1];
                match resolve_import(root, target) {
                    ImportTarget::Knowledge => findings.push(Finding {
                        file: rel.clone(),
                        line: i + 1,
                        check: Check::K4Values,
                        message: format!(
                            "{rel} must not @-import .knowledge content (decision 10 — @-import closure)"
                        ),
                    }),
                    ImportTarget::RepoLocal(member) => {
                        if depth < IMPORT_MAX_DEPTH {
                            queue.push((member, depth + 1));
                        }
                    }
                    ImportTarget::Outside => {}
                }
            }
        }
    }
    Ok(())
}

/// Where an `@`-import points, after root-resolution.
enum ImportTarget {
    /// Under `root/.knowledge` (any spelling) — the decision-10 violation.
    Knowledge,
    /// A repo-local file (root-relative path) — a closure member to recurse into.
    RepoLocal(String),
    /// Home-dir / root-external — outside repo jurisdiction, skipped.
    Outside,
}

/// Root-resolve one import target (relative, `./`-relative, and root-internal absolute spellings).
fn resolve_import(root: &Path, target: &str) -> ImportTarget {
    if target.starts_with('~') {
        return ImportTarget::Outside;
    }
    let resolved: PathBuf = if Path::new(target).is_absolute() {
        PathBuf::from(target)
    } else {
        root.join(target)
    };
    let normal = normalize(&resolved);
    let Ok(rel) = normal.strip_prefix(normalize(root)) else {
        return ImportTarget::Outside;
    };
    if rel.starts_with(".knowledge") {
        return ImportTarget::Knowledge;
    }
    ImportTarget::RepoLocal(rel.to_string_lossy().replace('\\', "/"))
}

/// Lexical path normalization (collapses `.` and `..` components; no fs access).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Corpus walk (dynamic) + k2 structural strays.
// ---------------------------------------------------------------------------------------------

/// One content-page file: root-relative path + its lines.
struct PageFile {
    rel: String,
    lines: Vec<String>,
}

/// The walked `.knowledge` tree: content pages per dir + the two index files.
struct KnowledgeTree {
    memories: Vec<PageFile>,
    runs: Vec<PageFile>,
    topics: Vec<PageFile>,
    memories_index: PageFile,
    wiki_index: PageFile,
}

/// Read a file into a `PageFile`, mapping unreadability to a guard `Err`.
fn read_page(root: &Path, rel: &str) -> Result<PageFile, String> {
    let raw = std::fs::read_to_string(root.join(rel))
        .map_err(|_| format!("knowledge structure incomplete — unreadable file: {rel} (FAIL)"))?;
    Ok(PageFile {
        rel: rel.to_owned(),
        lines: raw.lines().map(str::to_owned).collect(),
    })
}

/// List one directory's entries (names + is-dir), mapping I/O errors to guard `Err`s.
fn list_dir(root: &Path, rel: &str) -> Result<Vec<(String, bool)>, String> {
    let mut out = Vec::new();
    let rd = std::fs::read_dir(root.join(rel))
        .map_err(|_| format!("knowledge structure incomplete — unreadable dir: {rel} (FAIL)"))?;
    for entry in rd {
        let entry = entry.map_err(|_| {
            format!("knowledge structure incomplete — unreadable dir: {rel} (FAIL)")
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_dir = entry.path().is_dir();
        out.push((name, is_dir));
    }
    out.sort();
    Ok(out)
}

/// Is this file name a markdown page name?
fn is_md(name: &str) -> bool {
    Path::new(name).extension().is_some_and(|e| e == "md")
}

/// Is the file at `root/rel` exactly zero bytes? An I/O error is a structure-guard `Err`
/// (fail-closed): a check that cannot read its input must block, never skip.
fn is_empty_file(root: &Path, rel: &str) -> Result<bool, String> {
    std::fs::metadata(root.join(rel))
        .map(|m| m.len() == 0)
        .map_err(|_| format!("knowledge structure incomplete — unreadable file: {rel} (FAIL)"))
}

/// Walk `.knowledge/`, emitting k2 stray/non-md/subdir findings and collecting content pages.
/// One narrow exception: exactly a ZERO-BYTE `.gitkeep` in `.knowledge/wiki/topics` is skipped —
/// git cannot track that empty seed dir, so the placeholder keeps the structure guard green on a
/// fresh clone. Anything else named `.gitkeep` (a non-empty one, or one in `memories/` or
/// `wiki/runs/`) falls through to the k2 stray-non-markdown finding, so the fail-closed k2 rule is
/// never softened (a `.gitkeep` cannot smuggle un-enforced bytes into the tree).
fn walk_knowledge(root: &Path, findings: &mut Vec<Finding>) -> Result<KnowledgeTree, String> {
    let stray = |rel: String, findings: &mut Vec<Finding>| {
        findings.push(Finding {
            file: rel.clone(),
            line: 1,
            check: Check::K2Orphan,
            message: format!("stray file '{rel}' outside the content dirs"),
        });
    };
    // .knowledge root: exactly memories/ + wiki/.
    for (name, is_dir) in list_dir(root, ".knowledge")? {
        let rel = format!(".knowledge/{name}");
        if is_dir {
            if name != "memories" && name != "wiki" {
                for (sub, _) in list_dir(root, &rel)? {
                    stray(format!("{rel}/{sub}"), findings);
                }
            }
        } else {
            stray(rel, findings);
        }
    }
    // .knowledge/wiki root: index.md + runs/ + topics/.
    for (name, is_dir) in list_dir(root, ".knowledge/wiki")? {
        let rel = format!(".knowledge/wiki/{name}");
        if is_dir {
            if name != "runs" && name != "topics" {
                for (sub, _) in list_dir(root, &rel)? {
                    stray(format!("{rel}/{sub}"), findings);
                }
            }
        } else if name != "index.md" {
            stray(rel, findings);
        }
    }
    let collect = |dir: &str, findings: &mut Vec<Finding>| -> Result<Vec<PageFile>, String> {
        let mut pages = Vec::new();
        for (name, is_dir) in list_dir(root, dir)? {
            let rel = format!("{dir}/{name}");
            if is_dir {
                findings.push(Finding {
                    file: rel.clone(),
                    line: 1,
                    check: Check::K2Orphan,
                    message: format!("subdirectory '{rel}' not allowed (flat layout)"),
                });
            } else if name == ".gitkeep"
                && dir == ".knowledge/wiki/topics"
                && is_empty_file(root, &rel)?
            {
                // Sanctioned zero-byte tree-preserving placeholder — and ONLY that: exactly a
                // zero-byte `.gitkeep` in `wiki/topics`. A non-empty `.gitkeep`, or one in
                // `memories/`/`wiki/runs/`, short-circuits past this arm and falls through to the
                // k2 stray-non-markdown finding below (fail-closed; see the function doc).
            } else if !is_md(&name) {
                findings.push(Finding {
                    file: rel.clone(),
                    line: 1,
                    check: Check::K2Orphan,
                    message: format!("unindexable non-markdown file '{rel}'"),
                });
            } else if name != "index.md" || dir != ".knowledge/memories" {
                pages.push(read_page(root, &rel)?);
            }
        }
        Ok(pages)
    };
    let memories = collect(".knowledge/memories", findings)?;
    let runs = collect(".knowledge/wiki/runs", findings)?;
    let topics = collect(".knowledge/wiki/topics", findings)?;
    Ok(KnowledgeTree {
        memories,
        runs,
        topics,
        memories_index: read_page(root, ".knowledge/memories/index.md")?,
        wiki_index: read_page(root, ".knowledge/wiki/index.md")?,
    })
}

// ---------------------------------------------------------------------------------------------
// Page parsing: frontmatter (k3) + slug (k5) + kind/type agreement (k4).
// ---------------------------------------------------------------------------------------------

/// The page kind, derived from the owning directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Memory,
    Run,
    Topic,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Memory => "memory",
            Kind::Run => "run",
            Kind::Topic => "topic",
        }
    }
    fn allowed_keys(self) -> &'static [&'static str] {
        match self {
            Kind::Memory => &["name", "description", "type"],
            Kind::Run => &[
                "name",
                "description",
                "type",
                "date",
                "branch",
                "pr",
                "issues",
            ],
            Kind::Topic => &["name", "description", "type", "category"],
        }
    }
}

/// A parsed content page (best-effort: `None` fields mean the frontmatter was too broken to read).
struct Page {
    rel: String,
    kind: Kind,
    stem: String,
    lines: Vec<String>,
    body_start: usize,
    fields: BTreeMap<String, (String, usize)>,
    issues: Vec<String>,
    issues_line: usize,
}

impl Page {
    fn field(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(|(v, _)| v.as_str())
    }
}

/// Parse every content page, emitting k3/k4/k5 findings; returns pages keyed by root-relative path.
fn parse_pages(tree: &KnowledgeTree, findings: &mut Vec<Finding>) -> BTreeMap<String, Page> {
    let mut pages = BTreeMap::new();
    for (kind, files) in [
        (Kind::Memory, &tree.memories),
        (Kind::Run, &tree.runs),
        (Kind::Topic, &tree.topics),
    ] {
        for pf in files {
            let page = parse_page(kind, pf, findings);
            pages.insert(page.rel.clone(), page);
        }
    }
    pages
}

/// Parse one page: filename/slug checks (k5), frontmatter schema (k3), value agreement (k4).
fn parse_page(kind: Kind, pf: &PageFile, findings: &mut Vec<Finding>) -> Page {
    let file_name = pf.rel.rsplit('/').next().unwrap_or(&pf.rel).to_owned();
    let stem = file_name
        .strip_suffix(".md")
        .unwrap_or(&file_name)
        .to_owned();
    check_slug(kind, &pf.rel, &file_name, &stem, findings);
    let (fields, body_start, issues, issues_line) = parse_frontmatter(kind, pf, findings);
    let page = Page {
        rel: pf.rel.clone(),
        kind,
        stem,
        lines: pf.lines.clone(),
        body_start,
        fields,
        issues,
        issues_line,
    };
    check_values(&page, findings);
    page
}

/// k5 — slug/filename rules.
fn check_slug(kind: Kind, rel: &str, file_name: &str, stem: &str, findings: &mut Vec<Finding>) {
    let slug_re = Regex::new(r"^[a-z0-9][a-z0-9-]*$").expect("valid slug regex");
    if !slug_re.is_match(stem) {
        findings.push(Finding {
            file: rel.to_owned(),
            line: 1,
            check: Check::K5Slug,
            message: format!("filename '{file_name}' is not a valid slug ([a-z0-9][a-z0-9-]*)"),
        });
    }
    if kind == Kind::Run {
        let date_re = Regex::new(r"^\d{4}-\d{2}-\d{2}-[a-z0-9]").expect("valid date-prefix regex");
        if !date_re.is_match(stem) {
            findings.push(Finding {
                file: rel.to_owned(),
                line: 1,
                check: Check::K5Slug,
                message: "run filename must be YYYY-MM-DD-<slug>.md".to_owned(),
            });
        }
    }
}

/// k3 — parse + validate the flat frontmatter block; returns (fields, body-start, issues, issues-line).
#[allow(clippy::type_complexity)]
fn parse_frontmatter(
    kind: Kind,
    pf: &PageFile,
    findings: &mut Vec<Finding>,
) -> (BTreeMap<String, (String, usize)>, usize, Vec<String>, usize) {
    let mut fields: BTreeMap<String, (String, usize)> = BTreeMap::new();
    let mut issues: Vec<String> = Vec::new();
    let mut issues_line = 1;
    let push = |line: usize, message: String, findings: &mut Vec<Finding>| {
        findings.push(Finding {
            file: pf.rel.clone(),
            line,
            check: Check::K3Frontmatter,
            message,
        });
    };
    if pf.lines.first().map(|l| l.trim_end()) != Some("---") {
        push(
            1,
            "missing frontmatter block ('---' ... '---')".to_owned(),
            findings,
        );
        return (fields, 0, issues, issues_line);
    }
    let Some(close) = pf.lines.iter().skip(1).position(|l| l.trim_end() == "---") else {
        push(1, "frontmatter not closed with '---'".to_owned(), findings);
        return (fields, pf.lines.len(), issues, issues_line);
    };
    let close_idx = close + 1;
    let kv = Regex::new(r"^([A-Za-z][A-Za-z0-9_-]*): ?(.*)$").expect("valid kv regex");
    for (i, line) in pf.lines[1..close_idx].iter().enumerate() {
        let line_no = i + 2;
        let Some(cap) = kv.captures(line) else {
            push(
                line_no,
                "malformed frontmatter line (expected flat 'key: value')".to_owned(),
                findings,
            );
            continue;
        };
        let key = cap[1].to_owned();
        let value = cap[2].trim().to_owned();
        if value.is_empty() {
            push(
                line_no,
                format!("frontmatter key '{key}' has an empty value"),
                findings,
            );
        }
        if !kind.allowed_keys().contains(&key.as_str()) {
            push(
                line_no,
                format!(
                    "frontmatter unknown key '{key}' (allowed for {}: {})",
                    kind.label(),
                    kind.allowed_keys().join(", ")
                ),
                findings,
            );
            continue;
        }
        if key == "issues" {
            issues_line = line_no;
            match parse_issues(&value) {
                Some(list) => issues = list,
                None => push(
                    line_no,
                    "'issues' must be a non-empty inline list of ub-* ids".to_owned(),
                    findings,
                ),
            }
        }
        fields.insert(key, (value, line_no));
    }
    for key in kind.allowed_keys() {
        if !fields.contains_key(*key) {
            push(
                1,
                format!("frontmatter missing required key '{key}'"),
                findings,
            );
        }
    }
    if kind == Kind::Run
        && let Some((date, line_no)) = fields.get("date")
    {
        let prefix: String = pf
            .rel
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .chars()
            .take(10)
            .collect();
        if date != &prefix {
            push(
                *line_no,
                format!("frontmatter date '{date}' != filename date prefix '{prefix}'"),
                findings,
            );
        }
    }
    (fields, close_idx + 1, issues, issues_line)
}

/// Parse the run `issues:` inline list; `None` = not a non-empty inline `[..]` list.
fn parse_issues(value: &str) -> Option<Vec<String>> {
    let inner = value.strip_prefix('[')?.strip_suffix(']')?;
    let ids: Vec<String> = inner
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    if ids.is_empty() || inner.split(',').any(|s| s.trim().is_empty()) {
        return None;
    }
    Some(ids)
}

/// k4/k5 — value agreement: type-per-dir, memory type enum, topic category enum, name == stem.
fn check_values(page: &Page, findings: &mut Vec<Finding>) {
    if let Some((name, line)) = page.fields.get("name")
        && name != &page.stem
    {
        findings.push(Finding {
            file: page.rel.clone(),
            line: *line,
            check: Check::K5Slug,
            message: format!("frontmatter name '{name}' != filename stem '{}'", page.stem),
        });
    }
    let type_line = page.fields.get("type").map_or(1, |(_, l)| *l);
    match (page.kind, page.field("type")) {
        (Kind::Memory, Some(t)) if !MEMORY_TYPES.contains(&t) => findings.push(Finding {
            file: page.rel.clone(),
            line: type_line,
            check: Check::K4Values,
            message: format!(
                "type '{t}' is not a canonical memory type (gotcha|recipe|reference|environment)"
            ),
        }),
        (Kind::Run, Some(t)) if t != RUN_TYPE => findings.push(Finding {
            file: page.rel.clone(),
            line: type_line,
            check: Check::K4Values,
            message: format!("page in wiki/runs must have type '{RUN_TYPE}', found '{t}'"),
        }),
        (Kind::Topic, Some(t)) if t != TOPIC_TYPE => findings.push(Finding {
            file: page.rel.clone(),
            line: type_line,
            check: Check::K4Values,
            message: format!("page in wiki/topics must have type '{TOPIC_TYPE}', found '{t}'"),
        }),
        _ => {}
    }
    if page.kind == Kind::Topic
        && let Some((cat, line)) = page.fields.get("category")
        && !TOPIC_CATEGORIES.contains(&cat.as_str())
    {
        findings.push(Finding {
            file: page.rel.clone(),
            line: *line,
            check: Check::K4Values,
            message: format!("category '{cat}' is not a canonical topic category"),
        });
    }
}

// ---------------------------------------------------------------------------------------------
// Index lints (k1 grammar/resolution, k4 agreement, k2 orphans).
// ---------------------------------------------------------------------------------------------

/// Replace inline-code-span content with spaces (offsets preserved) so token/entry scans skip it.
fn scrub_code_spans(line: &str) -> String {
    let spans = code_spans(line);
    if spans.is_empty() {
        return line.to_owned();
    }
    let mut bytes = line.as_bytes().to_vec();
    for (s, e) in spans {
        for b in &mut bytes[s..e] {
            if !b.is_ascii_whitespace() {
                *b = b' ';
            }
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// `.knowledge/memories/index.md` — flat list with an inline backticked type token.
fn lint_memories_index(
    tree: &KnowledgeTree,
    pages: &BTreeMap<String, Page>,
    findings: &mut Vec<Finding>,
) {
    let index = &tree.memories_index;
    let entry_shaped = Regex::new(r"^\s*-\s*\[").expect("valid entry-shape regex");
    let grammar = Regex::new(r"^\s*- \[([^\]]+)\]\(([^)]+)\) `([^`]+)` — (.+)$")
        .expect("valid memories-entry regex");
    let mask = fence_mask(&index.lines);
    let mut listed: BTreeMap<String, usize> = BTreeMap::new();
    for (i, line) in index.lines.iter().enumerate() {
        if mask[i] || !entry_shaped.is_match(line) {
            continue;
        }
        let Some(cap) = grammar.captures(line) else {
            findings.push(Finding {
                file: index.rel.clone(),
                line: i + 1,
                check: Check::K1Index,
                message: "malformed index entry (expected '- [slug](slug.md) `type` — one-liner')"
                    .to_owned(),
            });
            continue;
        };
        let (text, target, tok, one_liner) = (&cap[1], &cap[2], &cap[3], &cap[4]);
        check_entry(
            &EntryCtx {
                index_rel: &index.rel,
                line: i + 1,
                text,
                target,
                one_liner,
                owning_dir: ".knowledge/memories",
                expect_prefix: "",
            },
            pages,
            &mut listed,
            findings,
        );
        let page_rel = format!(".knowledge/memories/{target}");
        if let Some(page) = pages.get(&page_rel)
            && let Some(t) = page.field("type")
            && t != tok
        {
            findings.push(Finding {
                file: index.rel.clone(),
                line: i + 1,
                check: Check::K4Values,
                message: format!("'{target}' indexed with type '{tok}' but frontmatter says '{t}'"),
            });
        }
    }
    orphans(&tree.memories, pages, &listed, &index.rel, findings);
}

/// Shared per-entry fields for the k1 checks.
struct EntryCtx<'a> {
    index_rel: &'a str,
    line: usize,
    text: &'a str,
    target: &'a str,
    one_liner: &'a str,
    owning_dir: &'a str,
    expect_prefix: &'a str,
}

/// The k1 target checks shared by both indexes: escape, resolution, duplicates, link-text, one-liner.
fn check_entry(
    ctx: &EntryCtx<'_>,
    pages: &BTreeMap<String, Page>,
    listed: &mut BTreeMap<String, usize>,
    findings: &mut Vec<Finding>,
) {
    let push = |check: Check, message: String, findings: &mut Vec<Finding>| {
        findings.push(Finding {
            file: ctx.index_rel.to_owned(),
            line: ctx.line,
            check,
            message,
        });
    };
    let bare = ctx
        .target
        .strip_prefix(ctx.expect_prefix)
        .unwrap_or_default();
    if ctx.target.starts_with('/')
        || ctx.target.contains("..")
        || !ctx.target.starts_with(ctx.expect_prefix)
        || bare.contains('/')
        || !is_md(bare)
    {
        push(
            Check::K1Index,
            format!("index entry '{}' escapes its content dir", ctx.target),
            findings,
        );
        return;
    }
    let page_rel = format!("{}/{bare}", ctx.owning_dir);
    *listed.entry(page_rel.clone()).or_insert(0) += 1;
    if listed[&page_rel] > 1 {
        push(
            Check::K1Index,
            format!("duplicate index entry for '{}'", ctx.target),
            findings,
        );
        return;
    }
    let Some(page) = pages.get(&page_rel) else {
        push(
            Check::K1Index,
            format!("index entry '{}' does not resolve to a file", ctx.target),
            findings,
        );
        return;
    };
    if ctx.text != page.stem {
        push(
            Check::K1Index,
            format!(
                "index entry link-text '{}' != target stem '{}'",
                ctx.text, page.stem
            ),
            findings,
        );
    }
    if let Some(desc) = page.field("description")
        && desc.trim() != ctx.one_liner.trim()
    {
        push(
            Check::K1Index,
            "index one-liner differs from the page's frontmatter description".to_owned(),
            findings,
        );
    }
}

/// k2 — every content page must be listed in its index (duplicates are k1's business).
fn orphans(
    files: &[PageFile],
    pages: &BTreeMap<String, Page>,
    listed: &BTreeMap<String, usize>,
    index_rel: &str,
    findings: &mut Vec<Finding>,
) {
    for pf in files {
        if pages.contains_key(&pf.rel) && !listed.contains_key(&pf.rel) {
            findings.push(Finding {
                file: pf.rel.clone(),
                line: 1,
                check: Check::K2Orphan,
                message: format!("page not listed in {index_rel}"),
            });
        }
    }
}

/// `.knowledge/wiki/index.md` — exactly-two-H2 rule + `### <category>` subheads + entry grammar.
fn lint_wiki_index(
    tree: &KnowledgeTree,
    pages: &BTreeMap<String, Page>,
    findings: &mut Vec<Finding>,
) {
    let index = &tree.wiki_index;
    let entry_shaped = Regex::new(r"^\s*-\s*\[").expect("valid entry-shape regex");
    let grammar =
        Regex::new(r"^\s*- \[([^\]]+)\]\(([^)]+)\) — (.+)$").expect("valid wiki-entry regex");
    let mask = fence_mask(&index.lines);
    let mut listed: BTreeMap<String, usize> = BTreeMap::new();
    let mut seen_h2: Vec<&str> = Vec::new();
    let mut section = String::new();
    let mut category: Option<(String, usize)> = None;
    for (i, line) in index.lines.iter().enumerate() {
        if mask[i] {
            continue;
        }
        if let Some(h) = line.strip_prefix("## ") {
            let h = h.trim();
            h.clone_into(&mut section);
            category = None;
            if WIKI_SECTIONS.contains(&h) {
                seen_h2.push(if h == "Runs" { "Runs" } else { "Topics" });
            } else {
                findings.push(Finding {
                    file: index.rel.clone(),
                    line: i + 1,
                    check: Check::K4Values,
                    message: format!(
                        "wiki index section '## {h}' is not canonical (expected exactly: Runs, Topics)"
                    ),
                });
            }
            continue;
        }
        if let Some(h) = line.strip_prefix("### ") {
            let h = h.trim().to_owned();
            if section == "Topics" && !TOPIC_CATEGORIES.contains(&h.as_str()) {
                findings.push(Finding {
                    file: index.rel.clone(),
                    line: i + 1,
                    check: Check::K4Values,
                    message: format!("wiki index category heading '### {h}' is not canonical"),
                });
            }
            category = Some((h, i + 1));
            continue;
        }
        if !entry_shaped.is_match(line) {
            continue;
        }
        let Some(cap) = grammar.captures(line) else {
            findings.push(Finding {
                file: index.rel.clone(),
                line: i + 1,
                check: Check::K1Index,
                message: "malformed index entry (expected '- [name](path.md) — one-liner')"
                    .to_owned(),
            });
            continue;
        };
        wiki_entry(
            index,
            i,
            (&cap[1], &cap[2], &cap[3]),
            &section,
            category.as_ref(),
            pages,
            &mut listed,
            findings,
        );
    }
    for want in WIKI_SECTIONS {
        if !seen_h2.contains(want) {
            findings.push(Finding {
                file: index.rel.clone(),
                line: 1,
                check: Check::K4Values,
                message: format!(
                    "wiki index section '## {want}' is not canonical (expected exactly: Runs, Topics)"
                ),
            });
        }
    }
    orphans(&tree.runs, pages, &listed, &index.rel, findings);
    orphans(&tree.topics, pages, &listed, &index.rel, findings);
}

/// One wiki-index entry: k1 target checks + k4 section/category placement.
#[allow(clippy::too_many_arguments)]
fn wiki_entry(
    index: &PageFile,
    line_idx: usize,
    (text, target, one_liner): (&str, &str, &str),
    section: &str,
    category: Option<&(String, usize)>,
    pages: &BTreeMap<String, Page>,
    listed: &mut BTreeMap<String, usize>,
    findings: &mut Vec<Finding>,
) {
    let expect_prefix = if target.starts_with("topics/") {
        "topics/"
    } else {
        "runs/"
    };
    let owning_dir = if expect_prefix == "runs/" {
        ".knowledge/wiki/runs"
    } else {
        ".knowledge/wiki/topics"
    };
    check_entry(
        &EntryCtx {
            index_rel: &index.rel,
            line: line_idx + 1,
            text,
            target,
            one_liner,
            owning_dir,
            expect_prefix,
        },
        pages,
        listed,
        findings,
    );
    let lives_in = if expect_prefix == "runs/" {
        "wiki/runs"
    } else {
        "wiki/topics"
    };
    let belongs = if expect_prefix == "runs/" {
        "Runs"
    } else {
        "Topics"
    };
    if section != belongs {
        findings.push(Finding {
            file: index.rel.clone(),
            line: line_idx + 1,
            check: Check::K4Values,
            message: format!("'{target}' indexed under '## {section}' but lives in {lives_in}"),
        });
    }
    if expect_prefix == "topics/"
        && let Some((heading, _)) = category
        && let Some(page) = pages.get(&format!(
            ".knowledge/wiki/topics/{}",
            target.strip_prefix("topics/").unwrap_or_default()
        ))
        && let Some(cat) = page.field("category")
        && cat != heading
    {
        findings.push(Finding {
            file: index.rel.clone(),
            line: line_idx + 1,
            check: Check::K4Values,
            message: format!(
                "'{target}' indexed under '### {heading}' but frontmatter says category '{cat}'"
            ),
        });
    }
}

// ---------------------------------------------------------------------------------------------
// k6 — run-report mandatory sections + glossary token coverage (arm B, temporal).
// ---------------------------------------------------------------------------------------------

/// The six mandatory H2 sections of a run-report, in template order.
const RUN_SECTIONS: &[&str] = &[
    "Context",
    "What & why",
    "Outcome",
    "Gotchas",
    "Glossary",
    "Links",
];

/// Locate the H2 sections of a run-report body: name -> (heading line idx, body line range).
fn sections(page: &Page) -> BTreeMap<String, (usize, std::ops::Range<usize>)> {
    let mask = fence_mask(&page.lines);
    let mut heads: Vec<(usize, String)> = Vec::new();
    for (i, line) in page.lines.iter().enumerate().skip(page.body_start) {
        if !mask[i]
            && let Some(h) = line.strip_prefix("## ")
        {
            heads.push((i, h.trim().to_owned()));
        }
    }
    let mut out = BTreeMap::new();
    for (n, (idx, name)) in heads.iter().enumerate() {
        let end = heads.get(n + 1).map_or(page.lines.len(), |(j, _)| *j);
        out.insert(name.clone(), (*idx, (*idx + 1)..end));
    }
    out
}

/// k6 shape half: all six H2s present, each with a non-empty body, glossary row-or-sentinel.
fn lint_run_sections(page: &Page, findings: &mut Vec<Finding>) {
    let secs = sections(page);
    for name in RUN_SECTIONS {
        let Some((head, body)) = secs.get(*name) else {
            findings.push(Finding {
                file: page.rel.clone(),
                line: 1,
                check: Check::K6RunSections,
                message: format!("run-report missing mandatory section '## {name}'"),
            });
            continue;
        };
        if page.lines[body.clone()].iter().all(|l| l.trim().is_empty()) {
            findings.push(Finding {
                file: page.rel.clone(),
                line: head + 1,
                check: Check::K6RunSections,
                message: format!("section '## {name}' is empty"),
            });
        }
    }
    if let Some((head, body)) = secs.get("Glossary") {
        let rows = glossary_rows(page, body.clone());
        let has_sentinel = page.lines[body.clone()]
            .iter()
            .any(|l| l.trim() == GLOSSARY_NONE);
        if rows.is_empty() && !has_sentinel {
            findings.push(Finding {
                file: page.rel.clone(),
                line: head + 1,
                check: Check::K6RunSections,
                message: format!(
                    "'## Glossary' must contain >=1 glossary DATA row or exactly: \"{GLOSSARY_NONE}\""
                ),
            });
        }
    }
}

/// Glossary DATA rows (id-cell values): within each contiguous `|`-table block, a DATA row is a
/// `|`-delimited line that is not the block's first row (the header), not a separator row, and not
/// an all-placeholder row.
fn glossary_rows(page: &Page, body: std::ops::Range<usize>) -> BTreeSet<String> {
    let sep_cell = Regex::new(r"^\s*:?-+:?\s*$").expect("valid separator-cell regex");
    let placeholder_cell = Regex::new(r"^\s*<[^>]*>\s*$").expect("valid placeholder-cell regex");
    let mut rows = BTreeSet::new();
    let mut in_block = false;
    let mut first_row = false;
    for line in &page.lines[body] {
        let t = line.trim();
        if t.starts_with('|') {
            if !in_block {
                in_block = true;
                first_row = true;
                continue; // the block's first row is the header
            }
            if first_row {
                first_row = false;
            }
            let cells: Vec<&str> = t.trim_matches('|').split('|').collect();
            if cells.iter().all(|c| sep_cell.is_match(c)) {
                continue;
            }
            if cells
                .iter()
                .all(|c| c.trim().is_empty() || placeholder_cell.is_match(c))
            {
                continue;
            }
            if let Some(id) = cells.first() {
                rows.insert(id.trim().to_owned());
            }
        } else {
            in_block = false;
        }
    }
    rows
}

/// Scan a text's lines for session-local-id tokens (fence mask + code spans applied); returns
/// distinct tokens with the 1-based line of their first occurrence.
fn scan_tokens(lines: &[String], skip_before: usize) -> BTreeMap<String, usize> {
    let re = Regex::new(SESSION_LOCAL_ID_RE).expect("valid session-local-id regex");
    let mask = fence_mask(lines);
    let mut out: BTreeMap<String, usize> = BTreeMap::new();
    for (i, line) in lines.iter().enumerate().skip(skip_before) {
        if mask[i] {
            continue;
        }
        let scrubbed = scrub_code_spans(line);
        let mut pos = 0;
        while let Some(m) = re.find_at(&scrubbed, pos) {
            let tok: &str = m
                .as_str()
                .trim_start_matches(|c: char| !c.is_ascii_alphanumeric())
                .trim_end_matches(|c: char| !c.is_ascii_digit());
            if !tok.is_empty() {
                out.entry(tok.to_owned()).or_insert(i + 1);
            }
            pos = m.start() + 1;
            if pos >= scrubbed.len() {
                break;
            }
        }
    }
    out
}

/// The report's date (the k3-pinned filename prefix, falling back to the `date:` field).
fn report_date(page: &Page) -> Option<String> {
    let prefix: String = page.stem.chars().take(10).collect();
    let date_re = Regex::new(r"^\d{4}-\d{2}-\d{2}$").expect("valid date regex");
    if date_re.is_match(&prefix) {
        return Some(prefix);
    }
    page.field("date")
        .filter(|d| date_re.is_match(d))
        .map(str::to_owned)
}

/// Validate + extract the date part of a comment `created_at` (date or datetime shape).
fn created_at_date(raw: &str) -> Option<String> {
    let re = Regex::new(r"^(\d{4})-(\d{2})-(\d{2})([T ].+)?$").expect("valid created-at regex");
    let cap = re.captures(raw)?;
    let month: u32 = cap[2].parse().ok()?;
    let day: u32 = cap[3].parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(format!("{}-{}-{}", &cap[1], &cap[2], &cap[3]))
}

/// k6 token-coverage rules (arm B, hard + temporal): report-body tokens (rule 1) and the run's
/// issue-comment tokens with `created_at` <= the report's date (rule 2 — inclusive end-of-day UTC;
/// equal timestamps IN scope) must each have a glossary DATA row whose id cell equals the token.
fn lint_run_tokens(
    page: &Page,
    export: &Export,
    findings: &mut Vec<Finding>,
) -> Result<(), String> {
    let rows = sections(page)
        .get("Glossary")
        .map(|(_, body)| glossary_rows(page, body.clone()))
        .unwrap_or_default();
    for (tok, line) in scan_tokens(&page.lines, page.body_start) {
        if !rows.contains(&tok) {
            findings.push(Finding {
                file: page.rel.clone(),
                line,
                check: Check::K6RunSections,
                message: format!("session-local id '{tok}' has no glossary row"),
            });
        }
    }
    let Some(date) = report_date(page) else {
        return Ok(()); // the page is already red under k3/k5; no temporal anchor exists
    };
    for id in &page.issues {
        let Some(comments) = export.comments.get(id) else {
            continue; // an absent `comments` key = legitimately zero comments
        };
        for comment in comments {
            let Some(raw) = comment.created_at.as_deref() else {
                return Err(format!(
                    "knowledge structure incomplete — a scanned comment of '{id}' lacks a string 'created_at' (fail-closed; never a silent skip)"
                ));
            };
            let Some(comment_date) = created_at_date(raw) else {
                return Err(format!(
                    "knowledge structure incomplete — a scanned comment of '{id}' has an unparsable 'created_at' ('{raw}'); fail-closed"
                ));
            };
            if comment_date > date {
                continue; // later comments are owed by their own coining PR (gate rule 1a)
            }
            let lines: Vec<String> = comment.text.lines().map(str::to_owned).collect();
            for tok in scan_tokens(&lines, 0).into_keys() {
                if !rows.contains(&tok) {
                    findings.push(Finding {
                        file: page.rel.clone(),
                        line: 1,
                        check: Check::K6RunSections,
                        message: format!(
                            "session-local id '{tok}' (from issue '{id}' comments) has no glossary row"
                        ),
                    });
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// The fixture export line: mirrors the real record shape (nested numeric `comments[].id` +
    /// `issue_id` + `created_at`), so the tests prove the top-level-id parse ignores nested fields
    /// and the temporal comparison has real data. The comment text carries `ub-prose.7` so the
    /// field-anchored (never substring) resolution is provable.
    const EXPORT_LINE: &str = r#"{"id":"ub-fixture.1","status":"open","comments":[{"id":1,"issue_id":"ub-fixture.1","created_at":"2026-07-21T12:00:00Z","text":"baseline note (see ub-prose.7)"}]}"#;

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, content).expect("write");
    }

    /// A minimal green fixture root: skeleton + out-of-tree stubs + one valid indexed run-report.
    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write(root, "CLAUDE.md", "# fixture\n\n@docs/PROCESS.md\n");
        write(root, "docs/PROCESS.md", "# process stub\n");
        write(root, ".unblock/issues.jsonl", &format!("{EXPORT_LINE}\n"));
        write(
            root,
            ".knowledge/memories/index.md",
            "# Memory index\n\nOne line per memory; the line's one-liner equals the memory's frontmatter description.\n",
        );
        write(
            root,
            ".knowledge/wiki/index.md",
            "# Wiki index\n\nDescriptive only.\n\n## Runs\n\n- [2026-07-21-example](runs/2026-07-21-example.md) — Example run.\n\n## Topics\n",
        );
        write(
            root,
            ".knowledge/wiki/runs/2026-07-21-example.md",
            &run_page(
                "2026-07-21-example",
                "2026-07-21",
                "[ub-fixture.1]",
                GLOSSARY_NONE,
                "",
            ),
        );
        fs::create_dir_all(root.join(".knowledge/wiki/topics")).expect("topics dir");
        dir
    }

    /// Build a valid run-report body with the given glossary payload + extra body text.
    fn run_page(name: &str, date: &str, issues: &str, glossary: &str, extra: &str) -> String {
        format!(
            "---\nname: {name}\ndescription: Example run.\ntype: run\ndate: {date}\nbranch: -\npr: -\nissues: {issues}\n---\n\n# Run — example\n\n## Context\n\nSome context.{extra}\n\n## What & why\n\nWhy it ran.\n\n## Outcome\n\nWhat landed.\n\n## Gotchas\n\nNone.\n\n## Glossary\n\n{glossary}\n\n## Links\n\n- the tracker issue.\n"
        )
    }

    fn lint(dir: &tempfile::TempDir) -> Result<Vec<Finding>, String> {
        lint_at(dir.path())
    }

    fn has(findings: &[Finding], check: Check, needle: &str) -> bool {
        findings
            .iter()
            .any(|f| f.check == check && f.message.contains(needle))
    }

    #[test]
    fn green_fixture_passes() {
        let dir = fixture();
        let findings = lint(&dir).expect("guard clean");
        assert!(findings.is_empty(), "expected clean, got {findings:?}");
    }

    #[test]
    fn k1_unresolved_and_malformed_and_duplicate_entries() {
        let dir = fixture();
        write(
            dir.path(),
            ".knowledge/memories/index.md",
            "# Memory index\n\n- [ghost](ghost.md) `gotcha` — Missing target.\n- [broken](broken.md) no type token here\n",
        );
        let f = lint(&dir).expect("guard clean");
        assert!(has(&f, Check::K1Index, "does not resolve"), "{f:?}");
        assert!(has(&f, Check::K1Index, "malformed index entry"), "{f:?}");
    }

    #[test]
    fn k2_orphan_stray_subdir_and_non_md() {
        let dir = fixture();
        write(
            dir.path(),
            ".knowledge/memories/orphan-fact.md",
            "---\nname: orphan-fact\ndescription: A fact.\ntype: gotcha\n---\n\nBody.\n",
        );
        write(dir.path(), ".knowledge/stray.txt", "stray\n");
        write(dir.path(), ".knowledge/memories/notes.txt", "not md\n");
        write(dir.path(), ".knowledge/memories/nested/deep.md", "nested\n");
        let f = lint(&dir).expect("guard clean");
        assert!(has(&f, Check::K2Orphan, "page not listed"), "{f:?}");
        assert!(has(&f, Check::K2Orphan, "stray file"), "{f:?}");
        assert!(
            has(&f, Check::K2Orphan, "unindexable non-markdown"),
            "{f:?}"
        );
        assert!(has(&f, Check::K2Orphan, "subdirectory"), "{f:?}");
    }

    #[test]
    fn gitkeep_placeholder_is_not_a_stray() {
        // (d) The sole sanctioned case: a ZERO-BYTE `.gitkeep` in `wiki/topics/` is skipped.
        let dir = fixture();
        write(dir.path(), ".knowledge/wiki/topics/.gitkeep", "");
        let f = lint(&dir).expect("guard clean");
        assert!(
            f.is_empty(),
            "an empty wiki/topics/.gitkeep placeholder must be skipped, got {f:?}"
        );
    }

    #[test]
    fn nonempty_gitkeep_in_topics_is_rejected() {
        // (a) A `.gitkeep` in the sanctioned dir but carrying bytes must NOT be exempt — it falls
        // through to the k2 stray-non-markdown finding (a `.gitkeep` cannot smuggle content).
        let dir = fixture();
        write(
            dir.path(),
            ".knowledge/wiki/topics/.gitkeep",
            "a paragraph of un-enforced prose\n",
        );
        let f = lint(&dir).expect("guard clean");
        assert!(
            f.iter().any(|x| x.check == Check::K2Orphan
                && x.file == ".knowledge/wiki/topics/.gitkeep"
                && x.message.contains("unindexable non-markdown")),
            "a NON-EMPTY wiki/topics/.gitkeep must be a k2 finding: {f:?}"
        );
    }

    #[test]
    fn gitkeep_in_memories_is_rejected() {
        // (b) Even an EMPTY `.gitkeep` outside `wiki/topics/` is not exempt — the dir restriction
        // must hold (emptiness alone does not license the skip).
        let dir = fixture();
        write(dir.path(), ".knowledge/memories/.gitkeep", "");
        let f = lint(&dir).expect("guard clean");
        assert!(
            f.iter().any(|x| x.check == Check::K2Orphan
                && x.file == ".knowledge/memories/.gitkeep"
                && x.message.contains("unindexable non-markdown")),
            "a .gitkeep in memories/ must be a k2 finding: {f:?}"
        );
    }

    #[test]
    fn gitkeep_in_runs_is_rejected() {
        // (c) An EMPTY `.gitkeep` in `wiki/runs/` is likewise not exempt (dir restriction).
        let dir = fixture();
        write(dir.path(), ".knowledge/wiki/runs/.gitkeep", "");
        let f = lint(&dir).expect("guard clean");
        assert!(
            f.iter().any(|x| x.check == Check::K2Orphan
                && x.file == ".knowledge/wiki/runs/.gitkeep"
                && x.message.contains("unindexable non-markdown")),
            "a .gitkeep in wiki/runs/ must be a k2 finding: {f:?}"
        );
    }

    #[test]
    fn k3_missing_frontmatter_and_bad_lines() {
        let dir = fixture();
        write(
            dir.path(),
            ".knowledge/memories/index.md",
            "# Memory index\n\n- [no-fm](no-fm.md) `gotcha` — A fact.\n- [bad-fm](bad-fm.md) `gotcha` — A fact.\n",
        );
        write(dir.path(), ".knowledge/memories/no-fm.md", "just a body\n");
        write(
            dir.path(),
            ".knowledge/memories/bad-fm.md",
            "---\nname: bad-fm\ndescription: A fact.\ntype: gotcha\nextra: nope\n  nested: yes\n---\n\nBody.\n",
        );
        let f = lint(&dir).expect("guard clean");
        assert!(
            has(&f, Check::K3Frontmatter, "missing frontmatter block"),
            "{f:?}"
        );
        assert!(
            has(&f, Check::K3Frontmatter, "unknown key 'extra'"),
            "{f:?}"
        );
        assert!(
            has(&f, Check::K3Frontmatter, "malformed frontmatter line"),
            "{f:?}"
        );
    }

    #[test]
    fn k3_run_date_must_match_filename_prefix() {
        let dir = fixture();
        write(
            dir.path(),
            ".knowledge/wiki/runs/2026-07-21-example.md",
            &run_page(
                "2026-07-21-example",
                "2026-07-22",
                "[ub-fixture.1]",
                GLOSSARY_NONE,
                "",
            ),
        );
        let f = lint(&dir).expect("guard clean");
        assert!(
            has(&f, Check::K3Frontmatter, "filename date prefix"),
            "{f:?}"
        );
    }

    #[test]
    fn k4_bad_memory_type_and_bad_topic_category() {
        let dir = fixture();
        write(
            dir.path(),
            ".knowledge/memories/index.md",
            "# Memory index\n\n- [typed](typed.md) `feeling` — A fact.\n",
        );
        write(
            dir.path(),
            ".knowledge/memories/typed.md",
            "---\nname: typed\ndescription: A fact.\ntype: feeling\n---\n\nBody.\n",
        );
        write(
            dir.path(),
            ".knowledge/wiki/index.md",
            "# Wiki index\n\n## Runs\n\n- [2026-07-21-example](runs/2026-07-21-example.md) — Example run.\n\n## Topics\n\n### vibes\n\n- [howto](topics/howto.md) — A runbook.\n",
        );
        write(
            dir.path(),
            ".knowledge/wiki/topics/howto.md",
            "---\nname: howto\ndescription: A runbook.\ntype: topic\ncategory: vibes\n---\n\nBody.\n",
        );
        let f = lint(&dir).expect("guard clean");
        assert!(
            has(&f, Check::K4Values, "not a canonical memory type"),
            "{f:?}"
        );
        assert!(
            has(&f, Check::K4Values, "not a canonical topic category"),
            "{f:?}"
        );
        assert!(has(&f, Check::K4Values, "category heading"), "{f:?}");
    }

    #[test]
    fn k4_wiki_h2_set_is_exact() {
        let dir = fixture();
        write(
            dir.path(),
            ".knowledge/wiki/index.md",
            "# Wiki index\n\n## Runs\n\n- [2026-07-21-example](runs/2026-07-21-example.md) — Example run.\n\n## Topics\n\n## Extras\n",
        );
        let f = lint(&dir).expect("guard clean");
        assert!(
            has(&f, Check::K4Values, "'## Extras' is not canonical"),
            "{f:?}"
        );
    }

    #[test]
    fn k4_issues_resolve_is_field_anchored() {
        let dir = fixture();
        write(
            dir.path(),
            ".knowledge/wiki/runs/2026-07-21-example.md",
            &run_page(
                "2026-07-21-example",
                "2026-07-21",
                "[ub-fixture.1, ub-ghost.9, ub-prose.7]",
                GLOSSARY_NONE,
                "",
            ),
        );
        let f = lint(&dir).expect("guard clean");
        assert!(
            has(&f, Check::K4Values, "'ub-ghost.9' not present"),
            "{f:?}"
        );
        assert!(
            has(&f, Check::K4Values, "'ub-prose.7' not present"),
            "prose-only ids must NOT resolve (field-anchored parse): {f:?}"
        );
        assert!(
            !has(&f, Check::K4Values, "'ub-fixture.1' not present"),
            "{f:?}"
        );
    }

    #[test]
    fn k4_closure_import_is_flagged_and_attributed() {
        let dir = fixture();
        write(
            dir.path(),
            "docs/PROCESS.md",
            "# process stub\n\n@.knowledge/wiki/index.md\n",
        );
        let f = lint(&dir).expect("guard clean");
        assert!(
            f.iter().any(|x| x.check == Check::K4Values
                && x.file == "docs/PROCESS.md"
                && x.message.contains("must not @-import .knowledge content")),
            "{f:?}"
        );
    }

    #[test]
    fn k4_closure_import_inside_fence_is_skipped() {
        let dir = fixture();
        write(
            dir.path(),
            "docs/PROCESS.md",
            "# process stub\n\n```\n@.knowledge/wiki/index.md\n```\n",
        );
        let f = lint(&dir).expect("guard clean");
        assert!(f.is_empty(), "fenced import text must not fire, got {f:?}");
    }

    #[test]
    fn k5_bad_slug_and_name_mismatch() {
        let dir = fixture();
        write(
            dir.path(),
            ".knowledge/memories/index.md",
            "# Memory index\n\n- [Bad_Slug](Bad_Slug.md) `gotcha` — A fact.\n- [renamed](renamed.md) `gotcha` — A fact.\n",
        );
        write(
            dir.path(),
            ".knowledge/memories/Bad_Slug.md",
            "---\nname: Bad_Slug\ndescription: A fact.\ntype: gotcha\n---\n\nBody.\n",
        );
        write(
            dir.path(),
            ".knowledge/memories/renamed.md",
            "---\nname: other-name\ndescription: A fact.\ntype: gotcha\n---\n\nBody.\n",
        );
        let f = lint(&dir).expect("guard clean");
        assert!(has(&f, Check::K5Slug, "not a valid slug"), "{f:?}");
        assert!(has(&f, Check::K5Slug, "!= filename stem"), "{f:?}");
    }

    #[test]
    fn k6_missing_section_and_empty_section() {
        let dir = fixture();
        let body = "---\nname: 2026-07-21-example\ndescription: Example run.\ntype: run\ndate: 2026-07-21\nbranch: -\npr: -\nissues: [ub-fixture.1]\n---\n\n## Context\n\n## What & why\n\nWhy.\n\n## Outcome\n\nLanded.\n\n## Glossary\n\nNo session-local ids were used in this run.\n\n## Links\n\n- x\n";
        write(
            dir.path(),
            ".knowledge/wiki/runs/2026-07-21-example.md",
            body,
        );
        let f = lint(&dir).expect("guard clean");
        assert!(
            has(
                &f,
                Check::K6RunSections,
                "missing mandatory section '## Gotchas'"
            ),
            "{f:?}"
        );
        assert!(
            has(&f, Check::K6RunSections, "section '## Context' is empty"),
            "{f:?}"
        );
    }

    #[test]
    fn k6_glossary_header_separator_only_is_not_a_data_row() {
        let dir = fixture();
        let glossary = "| id | what it is (in words) | where it lives |\n|----|----|----|";
        write(
            dir.path(),
            ".knowledge/wiki/runs/2026-07-21-example.md",
            &run_page(
                "2026-07-21-example",
                "2026-07-21",
                "[ub-fixture.1]",
                glossary,
                "",
            ),
        );
        let f = lint(&dir).expect("guard clean");
        assert!(
            has(&f, Check::K6RunSections, ">=1 glossary DATA row"),
            "{f:?}"
        );
    }

    #[test]
    fn k6_glossary_all_placeholder_row_is_not_a_data_row() {
        let dir = fixture();
        let glossary = "| id | what it is (in words) | where it lives |\n|----|----|----|\n| <M10> | <…> | <…> |";
        write(
            dir.path(),
            ".knowledge/wiki/runs/2026-07-21-example.md",
            &run_page(
                "2026-07-21-example",
                "2026-07-21",
                "[ub-fixture.1]",
                glossary,
                "",
            ),
        );
        let f = lint(&dir).expect("guard clean");
        assert!(
            has(&f, Check::K6RunSections, ">=1 glossary DATA row"),
            "{f:?}"
        );
    }

    #[test]
    fn k6_one_real_data_row_passes() {
        let dir = fixture();
        let glossary = "| id | what it is (in words) | where it lives |\n|----|----|----|\n| MF-9 | a must-fix id from the gate round | the review verdict |";
        write(
            dir.path(),
            ".knowledge/wiki/runs/2026-07-21-example.md",
            &run_page(
                "2026-07-21-example",
                "2026-07-21",
                "[ub-fixture.1]",
                glossary,
                "",
            ),
        );
        let f = lint(&dir).expect("guard clean");
        assert!(f.is_empty(), "expected clean, got {f:?}");
    }

    #[test]
    fn k6_body_token_needs_a_row() {
        let dir = fixture();
        write(
            dir.path(),
            ".knowledge/wiki/runs/2026-07-21-example.md",
            &run_page(
                "2026-07-21-example",
                "2026-07-21",
                "[ub-fixture.1]",
                GLOSSARY_NONE,
                " The gate raised MF-9 against this.",
            ),
        );
        let f = lint(&dir).expect("guard clean");
        assert!(
            has(
                &f,
                Check::K6RunSections,
                "session-local id 'MF-9' has no glossary row"
            ),
            "{f:?}"
        );
    }

    #[test]
    fn k6_body_token_with_row_passes_and_fenced_examples_are_skipped() {
        let dir = fixture();
        let glossary = "| id | what it is (in words) | where it lives |\n|----|----|----|\n| MF-9 | a must-fix id from the gate round | the review verdict |";
        let extra = " The gate raised MF-9 against this.\n\n```\n---\nname: fenced-example\n---\nMF-77 inside a fence needs no row\n- [x](x.md) `gotcha` — fenced entry example\n```";
        write(
            dir.path(),
            ".knowledge/wiki/runs/2026-07-21-example.md",
            &run_page(
                "2026-07-21-example",
                "2026-07-21",
                "[ub-fixture.1]",
                glossary,
                extra,
            ),
        );
        let f = lint(&dir).expect("guard clean");
        assert!(f.is_empty(), "expected clean, got {f:?}");
    }

    #[test]
    fn k6_comment_token_in_scope_needs_a_row() {
        let dir = fixture();
        write(
            dir.path(),
            ".unblock/issues.jsonl",
            r#"{"id":"ub-fixture.1","status":"open","comments":[{"id":1,"issue_id":"ub-fixture.1","created_at":"2026-07-21T23:59:59Z","text":"round 1 coined MF-7 here"}]}"#,
        );
        let f = lint(&dir).expect("guard clean");
        assert!(
            has(
                &f,
                Check::K6RunSections,
                "session-local id 'MF-7' (from issue 'ub-fixture.1' comments) has no glossary row"
            ),
            "equal-date timestamps are IN scope: {f:?}"
        );
    }

    #[test]
    fn k6_comment_token_after_report_date_is_out_of_scope() {
        let dir = fixture();
        write(
            dir.path(),
            ".unblock/issues.jsonl",
            r#"{"id":"ub-fixture.1","status":"open","comments":[{"id":1,"issue_id":"ub-fixture.1","created_at":"2026-07-22T00:00:01Z","text":"a later phase coined MF-7 here"}]}"#,
        );
        let f = lint(&dir).expect("guard clean");
        assert!(
            f.is_empty(),
            "later comments must not redden a frozen report, got {f:?}"
        );
    }

    #[test]
    fn guard_scanned_comment_without_created_at_fails_closed() {
        let dir = fixture();
        write(
            dir.path(),
            ".unblock/issues.jsonl",
            r#"{"id":"ub-fixture.1","status":"open","comments":[{"id":1,"issue_id":"ub-fixture.1","text":"no timestamp"}]}"#,
        );
        let err = lint(&dir).expect_err("must fail closed");
        assert!(err.contains("created_at"), "{err}");
    }

    #[test]
    fn guard_absent_export_or_claude_md_fails_closed() {
        let dir = fixture();
        fs::remove_file(dir.path().join(".unblock/issues.jsonl")).expect("rm");
        let err = lint(&dir).expect_err("must fail closed");
        assert!(err.contains(".unblock/issues.jsonl"), "{err}");

        let dir2 = fixture();
        fs::remove_file(dir2.path().join("CLAUDE.md")).expect("rm");
        let err2 = lint(&dir2).expect_err("must fail closed");
        assert!(err2.contains("CLAUDE.md"), "{err2}");
    }

    #[test]
    fn guard_corrupt_export_line_fails_closed() {
        let dir = fixture();
        write(dir.path(), ".unblock/issues.jsonl", "not json at all\n");
        let err = lint(&dir).expect_err("must fail closed");
        assert!(err.contains("corrupt export"), "{err}");
    }

    #[test]
    fn guard_absent_closure_member_fails_closed() {
        let dir = fixture();
        write(dir.path(), "CLAUDE.md", "# fixture\n\n@docs/MISSING.md\n");
        let err = lint(&dir).expect_err("must fail closed");
        assert!(err.contains("docs/MISSING.md"), "{err}");
    }

    #[test]
    fn guard_missing_skeleton_fails_closed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = lint_at(dir.path()).expect_err("must fail closed");
        assert!(err.contains("knowledge structure incomplete"), "{err}");
    }

    #[test]
    fn index_files_need_no_frontmatter() {
        // The green fixture's indexes carry no frontmatter and no k3 finding fires — pinned here
        // explicitly so nobody "fixes" the exemption away.
        let dir = fixture();
        let f = lint(&dir).expect("guard clean");
        assert!(!f.iter().any(|x| x.check == Check::K3Frontmatter), "{f:?}");
    }

    #[test]
    fn session_local_id_re_is_single_sourced() {
        // Rust const == the normative literal (ci-cd §2.3.3) == the gate script's sh const.
        assert_eq!(
            SESSION_LOCAL_ID_RE,
            "(^|[^A-Za-z0-9-])(MF|CF|M|R|F|A)-?[0-9]+([^0-9]|$)"
        );
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .to_path_buf();
        let script = fs::read_to_string(root.join("scripts/knowledge/run-report-gate.sh"))
            .expect("gate script readable");
        let line = script
            .lines()
            .find(|l| l.starts_with("SESSION_LOCAL_ID_RE='"))
            .expect("gate script declares SESSION_LOCAL_ID_RE");
        let value = line
            .trim_start_matches("SESSION_LOCAL_ID_RE='")
            .trim_end_matches('\'');
        assert_eq!(
            value, SESSION_LOCAL_ID_RE,
            "script const drifted from the Rust const"
        );
    }

    #[test]
    fn token_scan_finds_adjacent_and_hyphenless_tokens() {
        let lines = vec!["M10, MF-2 and R8 (also CF-3; A-1..A-7)".to_owned()];
        let toks: Vec<String> = scan_tokens(&lines, 0).into_keys().collect();
        assert_eq!(toks, vec!["A-1", "A-7", "CF-3", "M10", "MF-2", "R8"]);
    }
}
