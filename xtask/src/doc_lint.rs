//! `doc-lint` — the doc-corpus consistency lint (ci-cd §2.1).
//!
//! A mechanical, offline, sub-second lint over a **fixed 19-file** documentation corpus that fails
//! CI on the six drift classes seen in the consolidated review (a..f). It is deterministic: a single
//! pass per file, findings sorted by `(file, line, class)`, no network, no `cargo metadata`.
//!
//! Run: `cargo xtask doc-lint`. T0.9 wires this into the CI `doc-lint` job.
//! Authoritative spec: `docs/plans/ci-cd-and-distribution.md` §2.1 (the six classes a..f).
//!
//! ## Global guards (built once per file, shared by every class)
//! 1. **Block-fence mask** — `CommonMark` fenced code blocks (backtick or tilde runs, matched by char
//!    and run length); every class skips fenced lines.
//! 2. **Inline-code-span index** — backtick-delimited runs per line; class (c) fires ONLY in-code,
//!    while a/b/d/e count in-code tokens for existence resolution.
//! 3. **Never-finding glyph set** `{● ◐ — ☑ ⊘ ☐}` — status/legend glyphs are never a violation.
//! 4. **Approximate-number guard** — `≈`/`~`-prefixed numbers are approximate, never asserted exact.
//!
//! The lint is the machine-enforcement half of the PROCESS.md decision-change checklist: PRD §4 owns
//! D-ids, PRD §5/§6 own FR/NFR tiers, the spine owns interface §-refs, the README owns its own counts.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use regex::Regex;

/// The fixed, ordered 19-file corpus (paths relative to the workspace root). The existence-guard
/// FAILs on any missing **or** extra file — a corpus smaller than this is a vacuous pass.
const CORPUS: &[&str] = &[
    "docs/PRD.md",
    "docs/plans/00-roadmap.md",
    "docs/plans/01-design-spine.md",
    "docs/plans/README.md",
    "docs/plans/STATUS.md",
    "docs/plans/ci-cd-and-distribution.md",
    "docs/plans/implementation-plan.md",
    "docs/plans/crates/unblock-model.md",
    "docs/plans/crates/unblock-error.md",
    "docs/plans/crates/unblock-policy.md",
    "docs/plans/crates/unblock-storage.md",
    "docs/plans/crates/unblock-sync.md",
    "docs/plans/crates/unblock-health.md",
    "docs/plans/crates/unblock-config.md",
    "docs/plans/crates/unblock-engine.md",
    "docs/plans/crates/unblock-render.md",
    "docs/plans/crates/unblock-mcp.md",
    "docs/plans/crates/unblock-cli.md",
    "docs/plans/crates/unblock-fuzz.md",
];

/// Status/legend glyphs that are never a finding regardless of class.
const NEVER_FINDING_GLYPHS: &[char] = &['●', '◐', '—', '☑', '⊘', '☐'];

/// A single finding (`path:line: [class] message`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// Corpus-relative path of the offending doc.
    pub file: String,
    /// 1-based line number of the offending token.
    pub line: usize,
    /// The drift class (`a`..`f`) that fired.
    pub class: char,
    /// Human-readable description of the violation.
    pub message: String,
}

impl Finding {
    fn render(&self) -> String {
        format!(
            "{}:{}: [{}] {}",
            self.file, self.line, self.class, self.message
        )
    }
}

/// Entry point for `cargo xtask doc-lint`.
#[must_use]
pub fn doc_lint() -> ExitCode {
    // Corpus root = CARGO_MANIFEST_DIR/.. (xtask manifest sits one level under the workspace root).
    let root = match workspace_root() {
        Ok(root) => root,
        Err(err) => {
            eprintln!("doc-lint: could not locate workspace root: {err}");
            return ExitCode::FAILURE;
        }
    };

    let docs = match load_corpus(&root) {
        Ok(docs) => docs,
        Err(err) => {
            eprintln!("doc-lint: {err}");
            return ExitCode::FAILURE;
        }
    };

    let findings = run(&docs);
    report(&findings, docs.len())
}

/// Lint the corpus rooted at `root` and return the sorted findings. Public so the corpus-green
/// integration test (`xtask/tests/doc_lint_corpus.rs`) can assert the real docs are clean AND that
/// the existence-guard FAILs on a truncated corpus (non-vacuity), mirroring how T0.2 proved the
/// layering check by injecting a back-edge.
///
/// # Errors
/// Returns `Err` if the corpus is incomplete (the existence / vacuous-pass guard).
pub fn lint_at(root: &Path) -> Result<Vec<Finding>, String> {
    let docs = load_corpus(root)?;
    Ok(run(&docs))
}

/// Resolve the workspace root from `CARGO_MANIFEST_DIR` (set by cargo for the running xtask crate).
fn workspace_root() -> Result<PathBuf, String> {
    let manifest = std::env::var("CARGO_MANIFEST_DIR")
        .map_err(|_| "CARGO_MANIFEST_DIR not set (run via `cargo xtask doc-lint`)".to_owned())?;
    Path::new(&manifest)
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("CARGO_MANIFEST_DIR {manifest:?} has no parent"))
}

/// A loaded corpus file: its corpus path plus its line-by-line content.
struct Doc {
    /// The corpus-relative path (also the short-name resolution key).
    path: String,
    /// Lines, 0-indexed in the vector; finding line numbers are `index + 1`.
    lines: Vec<String>,
}

/// Load exactly the 19-file corpus; FAIL if any file is missing (existence-guard / vacuous-pass guard).
fn load_corpus(root: &Path) -> Result<Vec<Doc>, String> {
    let mut docs = Vec::with_capacity(CORPUS.len());
    let mut missing = Vec::new();
    for rel in CORPUS {
        let full = root.join(rel);
        match std::fs::read_to_string(&full) {
            Ok(content) => docs.push(Doc {
                path: (*rel).to_owned(),
                lines: content.lines().map(str::to_owned).collect(),
            }),
            Err(_) => missing.push(*rel),
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "corpus incomplete — {} of {} expected docs unreadable: {} \
             (a corpus smaller than expected is a vacuous pass; FAIL)",
            missing.len(),
            CORPUS.len(),
            missing.join(", ")
        ));
    }
    if docs.len() != CORPUS.len() {
        return Err(format!(
            "corpus size {} != expected {} (vacuous-pass guard)",
            docs.len(),
            CORPUS.len()
        ));
    }
    Ok(docs)
}

/// Per-file global guards, computed once and shared by every class.
struct Guards {
    /// `fenced[i]` is true when line `i` is inside a fenced code block (the opener/closer lines
    /// themselves are also masked — they carry no lintable content).
    fenced: Vec<bool>,
}

impl Guards {
    fn build(doc: &Doc) -> Self {
        Guards {
            fenced: fence_mask(&doc.lines),
        }
    }
}

/// `CommonMark` block-fence mask: a fence opens on a line whose first non-space run is a run of >= 3
/// backticks or tildes, and closes on the next line with a matching char and a run length >= the
/// opener's. Opener + closer + the body are all masked.
fn fence_mask(lines: &[String]) -> Vec<bool> {
    let mut mask = vec![false; lines.len()];
    let mut open: Option<(char, usize)> = None;
    for (i, raw) in lines.iter().enumerate() {
        let trimmed = raw.trim_start();
        let fence = fence_run(trimmed);
        match (open, fence) {
            (None, Some((ch, len))) => {
                // Open a fence; mask the opener line.
                open = Some((ch, len));
                mask[i] = true;
            }
            (Some((open_ch, open_len)), Some((ch, len))) => {
                // Inside a fence: mask the line; a matching, long-enough run of the SAME char and
                // NO info string closes it.
                mask[i] = true;
                if ch == open_ch && len >= open_len && info_string(trimmed, ch).is_empty() {
                    open = None;
                }
            }
            (Some(_), None) => {
                // Inside a fence, ordinary body line.
                mask[i] = true;
            }
            (None, None) => {}
        }
    }
    mask
}

/// If `s` begins with a `CommonMark` fence run (>= 3 identical backticks or tildes), return
/// `(char, run_len)`.
fn fence_run(s: &str) -> Option<(char, usize)> {
    let first = s.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let len = s.chars().take_while(|&c| c == first).count();
    if len >= 3 { Some((first, len)) } else { None }
}

/// The info string after a fence run (everything past the leading run of `ch`), trimmed.
fn info_string(s: &str, ch: char) -> &str {
    s.trim_start_matches(ch).trim()
}

/// Inline-code spans on a line: byte ranges delimited by matched backtick runs (`CommonMark`: a run of
/// N backticks opens a span closed by the next run of exactly N backticks). Returns the spans as
/// `(start, end)` byte offsets covering the code content (delimiters excluded).
fn code_spans(line: &str) -> Vec<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let open_start = i;
            let mut run = 0;
            while i < bytes.len() && bytes[i] == b'`' {
                run += 1;
                i += 1;
            }
            let content_start = i;
            // Find a closing run of exactly `run` backticks.
            let mut j = content_start;
            let mut closed = None;
            while j < bytes.len() {
                if bytes[j] == b'`' {
                    let close_start = j;
                    let mut k = j;
                    let mut crun = 0;
                    while k < bytes.len() && bytes[k] == b'`' {
                        crun += 1;
                        k += 1;
                    }
                    if crun == run {
                        closed = Some((close_start, k));
                        break;
                    }
                    j = k;
                } else {
                    j += 1;
                }
            }
            if let Some((close_start, close_end)) = closed {
                spans.push((content_start, close_start));
                i = close_end;
            } else {
                // Unterminated run — not a span; resume scanning after the opener run.
                i = open_start + run;
            }
        } else {
            i += 1;
        }
    }
    spans
}

/// Is byte offset `off` within any inline-code span?
fn in_code_span(spans: &[(usize, usize)], off: usize) -> bool {
    spans.iter().any(|&(s, e)| off >= s && off < e)
}

/// Run all six classes over the corpus, returning findings sorted by `(file, line, class)`.
fn run(docs: &[Doc]) -> Vec<Finding> {
    let index = CorpusIndex::build(docs);
    let mut findings = Vec::new();

    for doc in docs {
        let guards = Guards::build(doc);
        class_a_d_ids(doc, &guards, &index, &mut findings);
        class_b_fr_nfr(doc, &guards, &index, &mut findings);
        class_c_commands(doc, &guards, &mut findings);
        class_d_stamp(doc, &guards, &index, &mut findings);
        class_e_cross_refs(doc, &guards, &index, &mut findings);
        if doc.path == "docs/plans/README.md" {
            class_f_readme(doc, &guards, &mut findings);
        }
    }

    findings.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.line.cmp(&b.line))
            .then(a.class.cmp(&b.class))
    });
    findings
}

/// Pre-computed cross-corpus indexes the classes resolve against.
struct CorpusIndex {
    /// D-ids defined in PRD §4 (`| **Dx** |` rows), e.g. {1..=19}.
    defined_d_ids: BTreeSet<u32>,
    /// FR/NFR ids defined in PRD §5/§6, with their canonical release marker (if any).
    /// Key = full id (`FR-1a`, `NFR-3`); value = `Some("must"|"v1.1"|"wont"|...)` or `None`.
    fr_nfr_defs: BTreeMap<String, Option<String>>,
    /// FR/NFR umbrella prefixes that have at least one sub-id (`FR-1` satisfied by `FR-1a`).
    fr_nfr_umbrellas: BTreeSet<String>,
    /// Canonical PRD revision, e.g. "1.1" (from PRD.md line 3).
    prd_revision: String,
    /// Per-file heading-number index: file path -> set of dotted heading numbers ("5", "5.3", ...).
    headings: BTreeMap<String, BTreeSet<String>>,
}

impl CorpusIndex {
    fn build(docs: &[Doc]) -> Self {
        let prd = docs
            .iter()
            .find(|d| d.path == "docs/PRD.md")
            .expect("PRD.md is in the fixed corpus");

        let (defined_d_ids, fr_nfr_defs, fr_nfr_umbrellas) = build_prd_defs(prd);
        let prd_revision = prd_revision(prd);
        let headings = docs
            .iter()
            .map(|d| (d.path.clone(), heading_index(d)))
            .collect();

        CorpusIndex {
            defined_d_ids,
            fr_nfr_defs,
            fr_nfr_umbrellas,
            prd_revision,
            headings,
        }
    }
}

/// Parse PRD §4 D-id rows and PRD §5/§6 FR/NFR definition lines.
fn build_prd_defs(
    prd: &Doc,
) -> (
    BTreeSet<u32>,
    BTreeMap<String, Option<String>>,
    BTreeSet<String>,
) {
    let d_row = Regex::new(r"^\s*\|\s*\*\*D(\d+)\*\*\s*\|").expect("valid D-row regex");
    // A definition line: a leading bullet/cell, then `FR-25 [must]` / `NFR-3 [performance]` etc.
    let def =
        Regex::new(r"(?m)^\W*(FR|NFR)-(\d+)([a-z]?)\s*\[([^\]]+)\]").expect("valid def regex");

    let mut d_ids = BTreeSet::new();
    let mut defs: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut umbrellas = BTreeSet::new();
    let mask = fence_mask(&prd.lines);

    for (i, line) in prd.lines.iter().enumerate() {
        if mask[i] {
            continue;
        }
        if let Some(c) = d_row.captures(line)
            && let Ok(n) = c[1].parse::<u32>()
        {
            d_ids.insert(n);
        }
        if let Some(c) = def.captures(line) {
            let kind = &c[1];
            let num = &c[2];
            let sub = &c[3];
            let marker = c[4].trim().to_owned();
            let id = format!("{kind}-{num}{sub}");
            // Release marker only (must / v1 / v1.x / wont); §6 category tags ([performance] etc.)
            // are not on the release axis.
            let release = if is_release_marker(&marker) {
                Some(marker)
            } else {
                None
            };
            defs.insert(id, release);
            if !sub.is_empty() {
                umbrellas.insert(format!("{kind}-{num}"));
            }
        }
    }
    (d_ids, defs, umbrellas)
}

/// Is a `[...]` marker a release marker (vs a §6 category tag)?
fn is_release_marker(marker: &str) -> bool {
    // Normalize: a release marker is `must`, `wont`, or a `v<major>.<minor?>` token; markers may be
    // compound (`must (subset) / v1.1 (full)`), so any release token present makes it a release axis.
    marker
        .split(|c: char| c == '/' || c == '(' || c == ')' || c.is_whitespace())
        .any(is_release_token)
}

/// One whitespace/`/`-separated token from a marker.
fn is_release_token(tok: &str) -> bool {
    let t = tok.trim().to_ascii_lowercase();
    if t == "must" || t == "wont" {
        return true;
    }
    // v1, v1.1, v1.2, v1.3, v2 ...
    if let Some(rest) = t.strip_prefix('v') {
        return rest
            .split('.')
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    }
    false
}

/// Canonical PRD revision from PRD.md (the `APPROVED (vX.Y)` stamp on line ~3).
fn prd_revision(prd: &Doc) -> String {
    let re = Regex::new(r"APPROVED\s*\(?v(\d+\.\d+)\)?").expect("valid revision regex");
    prd.lines
        .iter()
        .find_map(|l| re.captures(l).map(|c| c[1].to_owned()))
        .unwrap_or_default()
}

/// Heading-number index for a file: dotted section numbers from `^#{1,6}\s+N(.M)*`.
fn heading_index(doc: &Doc) -> BTreeSet<String> {
    let re = Regex::new(r"^#{1,6}\s+(\d+(?:\.\d+)*)").expect("valid heading regex");
    let mask = fence_mask(&doc.lines);
    let mut set = BTreeSet::new();
    for (i, line) in doc.lines.iter().enumerate() {
        if mask[i] {
            continue;
        }
        if let Some(c) = re.captures(line) {
            set.insert(c[1].to_owned());
        }
    }
    set
}

// ---------------------------------------------------------------------------------------------
// Class (a) — D-id existence (deterministic; no co-keyword heuristic).
// ---------------------------------------------------------------------------------------------

fn class_a_d_ids(doc: &Doc, guards: &Guards, index: &CorpusIndex, out: &mut Vec<Finding>) {
    // Spec tokenizes `\bD(2[01]|1[0-9]|[1-9])\b` for the in-range ids (D1..D21), but an undefined ref
    // (D98, D99) is *also* a violation — so we match every `\bDn\b` and resolve membership against
    // the PRD §4 definition set. Range-awareness lives in the definition set, not the regex.
    let any = Regex::new(r"\bD(\d+)\b").expect("valid Dn regex");

    for (i, line) in doc.lines.iter().enumerate() {
        if guards.fenced[i] || line_is_only_glyphs(line) {
            continue;
        }
        for c in any.captures_iter(line) {
            let n: u32 = match c[1].parse() {
                Ok(n) => n,
                Err(_) => continue,
            };
            if !index.defined_d_ids.contains(&n) {
                out.push(Finding {
                    file: doc.path.clone(),
                    line: i + 1,
                    class: 'a',
                    message: format!("D{n} referenced but not defined in PRD §4"),
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Class (b) — FR/NFR tier coherence.
// ---------------------------------------------------------------------------------------------

fn class_b_fr_nfr(doc: &Doc, guards: &Guards, index: &CorpusIndex, out: &mut Vec<Finding>) {
    // Token: FR-25 / NFR-3 / FR-1a. Capture an OPTIONAL immediately-adjacent `[marker]`.
    let re =
        Regex::new(r"\b(FR|NFR)-(\d+)([a-z]?)\b(\s*\[([^\]]+)\])?").expect("valid FR/NFR regex");

    for (i, line) in doc.lines.iter().enumerate() {
        if guards.fenced[i] || line_is_only_glyphs(line) {
            continue;
        }
        for c in re.captures_iter(line) {
            let kind = &c[1];
            let num = &c[2];
            let sub = &c[3];
            let id = format!("{kind}-{num}{sub}");

            // (1) Existence: resolve to a PRD definition, or (bare umbrella) to any sub-id.
            let defined = index.fr_nfr_defs.contains_key(&id)
                || (sub.is_empty() && index.fr_nfr_umbrellas.contains(&id));
            if !defined {
                // The PRD itself is the definition home; an undefined ref anywhere (incl. PRD) is a
                // violation. (A def line in the PRD self-resolves, so this only fires on dangling
                // refs.)
                out.push(Finding {
                    file: doc.path.clone(),
                    line: i + 1,
                    class: 'b',
                    message: format!("{id} referenced but not defined in PRD §5/§6"),
                });
                continue;
            }

            // (2) Tier: compare only when a RELEASE marker is immediately adjacent.
            let Some(raw_marker) = c.get(5).map(|m| m.as_str().trim()) else {
                continue; // existence-only
            };
            // Exclude §6 category tags from the release axis.
            if !is_release_marker(raw_marker) {
                continue;
            }
            // The canonical tier: prefer the sub-id def; for a bare umbrella with no def, skip tier.
            let Some(canonical) = index.fr_nfr_defs.get(&id).and_then(Option::as_ref) else {
                continue;
            };
            if !markers_equal(raw_marker, canonical) {
                let section = if kind == "FR" { "§5" } else { "§6" };
                out.push(Finding {
                    file: doc.path.clone(),
                    line: i + 1,
                    class: 'b',
                    message: format!(
                        "{id} marked [{raw_marker}] but PRD {section} defines it [{canonical}]"
                    ),
                });
            }
        }
    }
}

/// Compare two release markers for equality on the release axis (case-insensitive; ignores the
/// non-release descriptive parens so `must (subset) / v1.1 (full)` matches itself).
fn markers_equal(a: &str, b: &str) -> bool {
    release_tokens(a) == release_tokens(b)
}

/// The sorted set of release tokens within a marker (drops descriptive parenthetical text).
fn release_tokens(marker: &str) -> BTreeSet<String> {
    marker
        .split(|c: char| c == '/' || c == '(' || c == ')' || c.is_whitespace())
        .map(|t| t.trim().to_ascii_lowercase())
        .filter(|t| is_release_token(t))
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Class (c) — command-token spelling (code-span only + rejection guard).
// ---------------------------------------------------------------------------------------------

const CANONICAL_VERBS: &[&str] = &[
    "serve", "migrate", "doctor", "version", "init", "agents", "update",
];

fn class_c_commands(doc: &Doc, guards: &Guards, out: &mut Vec<Finding>) {
    let re = Regex::new(r"\bunblock\s+([a-z][a-z-]*)\b").expect("valid command regex");

    for (i, line) in doc.lines.iter().enumerate() {
        if guards.fenced[i] {
            continue;
        }
        // Rejection-context guard: a line demonstrating a NEGATIVE/usage-error example is skipped.
        if is_rejection_context(line) {
            continue;
        }
        let spans = code_spans(line);
        for c in re.captures_iter(line) {
            let m = c.get(1).expect("group 1 present");
            let verb = m.as_str();
            // Fire ONLY inside an inline code span (or a carved-out command block — fenced lines are
            // already skipped, so a code-span is the in-prose signal we require).
            if !in_code_span(&spans, m.start()) {
                continue;
            }
            if CANONICAL_VERBS.contains(&verb) {
                continue;
            }
            // GUARD (i): the Cargo FEATURE `self-update` is feature context, not a verb — allowed.
            if verb == "self-update" {
                continue;
            }
            out.push(Finding {
                file: doc.path.clone(),
                line: i + 1,
                class: 'c',
                message: format!(
                    "non-canonical command 'unblock {verb}' (canonical: {})",
                    CANONICAL_VERBS.join("|")
                ),
            });
        }
    }
}

/// Rejection-context guard: the line carries an explicit negative-example marker, so a non-canonical
/// `unblock <verb>` on it is a deliberate "this is rejected" illustration, not a spelling drift.
///
/// `e.g.` is deliberately NOT a marker — it is too broad and would silence a genuine future typo on an
/// "e.g." example line. The remaining markers all name a rejection/usage-error context explicitly
/// (cli.md's negative example carries `reject` + `unknown` + `usage error` regardless).
fn is_rejection_context(line: &str) -> bool {
    const MARKERS: &[&str] = &[
        "reject",
        "unknown",
        "usage error",
        "not a command",
        "→ exit",
    ];
    let lower = line.to_ascii_lowercase();
    MARKERS.iter().any(|m| lower.contains(m))
}

// ---------------------------------------------------------------------------------------------
// Class (d) — source-of-truth stamp.
// ---------------------------------------------------------------------------------------------

fn class_d_stamp(doc: &Doc, guards: &Guards, index: &CorpusIndex, out: &mut Vec<Finding>) {
    // `PRD APPROVED v1.1` literal. The `regex` crate has no look-around, but the RIGHT boundary the
    // spec wants is implicit: `\d+\.\d+` is greedy and stops at the second dotted group, so a
    // sentence-final `PRD APPROVED v1.1.` captures `1.1` (the period is not a digit) and a `v1.10`
    // captures `1.10` — neither over-absorbs the trailing punctuation.
    let re = Regex::new(r"PRD APPROVED v(\d+\.\d+)").expect("valid stamp regex");
    let canonical = &index.prd_revision;
    if canonical.is_empty() {
        return;
    }

    for (i, line) in doc.lines.iter().enumerate() {
        if guards.fenced[i] {
            continue;
        }
        // The PRD header line that DEFINES the revision is the source, not a violation.
        if doc.path == "docs/PRD.md"
            && line.contains("APPROVED")
            && !line.contains("PRD APPROVED v")
        {
            continue;
        }
        for c in re.captures_iter(line) {
            let ver = &c[1];
            if ver != canonical {
                out.push(Finding {
                    file: doc.path.clone(),
                    line: i + 1,
                    class: 'd',
                    message: format!("stamp 'PRD APPROVED v{ver}' != PRD revision v{canonical}"),
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Class (e) — cross-ref resolution.
// ---------------------------------------------------------------------------------------------

fn class_e_cross_refs(doc: &Doc, guards: &Guards, index: &CorpusIndex, out: &mut Vec<Finding>) {
    // A doc-qualifier token: an explicit filename (`01-design-spine.md`, optional trailing backtick)
    // or a short-name (`spine`/`PRD`/`impl-plan`/`roadmap`/`ci-cd`). The matched form is captured so
    // we can resolve it and decide whether it is KNOWN (an unknown short-name is reported on the ref
    // it governs).
    // Convention: word-style short-names accept an OPTIONAL leading capital (a sentence-start
    // `Spine §4.1`) but nothing else — `[Ss]pine` etc., NOT a fully case-insensitive match (which
    // would catch unrelated prose like "SPINE"). `PRD` is an acronym, matched exactly. This MUST
    // mirror `short_name_to_file`, which lowercases the word-style names before resolving (and matches
    // `PRD` exactly) — the two are kept in lock-step so a captured qualifier always resolves.
    let qualifier = Regex::new(
        r"(?:(01-design-spine|PRD|implementation-plan|00-roadmap|ci-cd-and-distribution)\.md`?)|\b([Ss]pine|PRD|[Ii]mpl-plan|[Rr]oadmap|[Cc]i-cd)\b",
    )
    .expect("valid qualifier regex");
    // A `§N` ref. The section number is a dotted run that NEVER ends on a dot (so a sentence-final
    // `§3.1.` captures `3.1`, not `3.1.`), matching the class-(d) right-boundary convention.
    let sec = Regex::new(r"§(\d+(?:\.\d+)*)").expect("valid section regex");
    // Inheritance barriers: doc-name-like tokens that are NOT one of the five recognised qualifiers —
    // a `*.md` filename (e.g. a crate-plan `unblock-render.md`) or a bare crate name (`unblock-foo`).
    // A barrier between a qualifier and a `§N` BREAKS the carry-forward, so the `§N` cannot silently
    // inherit a far-away qualifier across an intervening "other doc" mention — it falls back to self
    // (and a genuinely ambiguous bare ref then surfaces as unresolved against the containing file).
    let barrier = Regex::new(r"\b[A-Za-z0-9][A-Za-z0-9_-]*\.md\b|\bunblock-[a-z]+\b")
        .expect("valid barrier regex");

    for (line_idx, line) in doc.lines.iter().enumerate() {
        if guards.fenced[line_idx] {
            continue;
        }

        // Every qualifier occurrence on the line with its byte span + resolved target.
        let mut quals: Vec<QualHit> = Vec::new();
        for c in qualifier.captures_iter(line) {
            let m = c.get(0).expect("group 0 present");
            let (display, target) = if let Some(fname) = c.get(1) {
                let short = fname.as_str();
                (format!("{short}.md"), qualified_filename(short))
            } else if let Some(sn) = c.get(2) {
                let short = sn.as_str();
                (short.to_owned(), short_name_to_file(short))
            } else {
                continue;
            };
            quals.push(QualHit {
                start: m.start(),
                end: m.end(),
                display,
                target,
            });
        }

        // Barrier END offsets: an unrecognised doc-name token whose span does NOT coincide with a
        // recognised qualifier (a recognised `01-design-spine.md` is a qualifier, not a barrier).
        let barriers: Vec<(usize, usize)> = barrier
            .find_iter(line)
            .map(|m| (m.start(), m.end()))
            .filter(|&(bs, be)| !quals.iter().any(|q| q.start <= bs && be <= q.end))
            .collect();

        for c in sec.captures_iter(line) {
            let whole = c.get(0).expect("group 0 present");
            let section = &c[1];
            let ref_start = whole.start();

            // The governing qualifier is the LAST one ending at-or-before this `§N` — UNLESS an
            // inheritance barrier (an other-doc mention) sits between that qualifier and the ref, in
            // which case the carry-forward is broken and the ref falls back to self. This implements
            // the spec's "small preceding window": `spine §1.8 ... §1.1` carries `spine` forward to a
            // bare sibling, but `spine §3.1 vs unblock-render.md §6` does NOT leak `spine` onto `§6`.
            let governing = quals.iter().rev().find(|q| {
                q.end <= ref_start
                    && !barriers
                        .iter()
                        .any(|&(bs, _)| q.end <= bs && bs < ref_start)
            });

            match governing {
                Some(q) => {
                    resolve_or_report(doc, line_idx, index, q.target, section, &q.display, out);
                }
                None => {
                    // Bare `§N` with no qualifier in the line → the CONTAINING file.
                    resolve_or_report(
                        doc,
                        line_idx,
                        index,
                        Some(doc.path.as_str()),
                        section,
                        "<self>",
                        out,
                    );
                }
            }
        }
    }
}

/// A qualifier occurrence on a line: its byte span, how it was written, and its resolved target
/// (`None` = an unknown doc-name, reported on the ref it governs).
struct QualHit {
    start: usize,
    end: usize,
    display: String,
    target: Option<&'static str>,
}

/// Map a filename short-name (without `.md`) to its corpus path.
fn qualified_filename(short: &str) -> Option<&'static str> {
    Some(match short {
        "01-design-spine" => "docs/plans/01-design-spine.md",
        "PRD" => "docs/PRD.md",
        "implementation-plan" => "docs/plans/implementation-plan.md",
        "00-roadmap" => "docs/plans/00-roadmap.md",
        "ci-cd-and-distribution" => "docs/plans/ci-cd-and-distribution.md",
        _ => return None,
    })
}

/// Map a short-name (`spine`, `PRD`, `impl-plan`, `roadmap`, `ci-cd`) to its corpus path. `PRD` is
/// matched exactly; the word-style names are accepted with an optional leading capital.
fn short_name_to_file(name: &str) -> Option<&'static str> {
    if name == "PRD" {
        return Some("docs/PRD.md");
    }
    Some(match name.to_ascii_lowercase().as_str() {
        "spine" => "docs/plans/01-design-spine.md",
        "impl-plan" => "docs/plans/implementation-plan.md",
        "roadmap" => "docs/plans/00-roadmap.md",
        "ci-cd" => "docs/plans/ci-cd-and-distribution.md",
        _ => return None,
    })
}

/// Resolve `§section` against `target`'s heading set; push a class-(e) finding if it does not resolve.
/// `display` is how the ref was written (for the message: `spine §9.9`, `<self>`).
fn resolve_or_report(
    doc: &Doc,
    line_idx: usize,
    index: &CorpusIndex,
    target: Option<&str>,
    section: &str,
    display: &str,
    out: &mut Vec<Finding>,
) {
    let Some(target) = target else {
        // Unknown doc-qualifier is reported.
        out.push(Finding {
            file: doc.path.clone(),
            line: line_idx + 1,
            class: 'e',
            message: format!("cross-ref '{display} §{section}' names an unknown doc"),
        });
        return;
    };
    let Some(headings) = index.headings.get(target) else {
        out.push(Finding {
            file: doc.path.clone(),
            line: line_idx + 1,
            class: 'e',
            message: format!("cross-ref '{display} §{section}' targets an unknown doc {target}"),
        });
        return;
    };
    if !section_resolves(headings, section) {
        let label = if display == "<self>" {
            format!("§{section}")
        } else {
            format!("{display} §{section}")
        };
        out.push(Finding {
            file: doc.path.clone(),
            line: line_idx + 1,
            class: 'e',
            message: format!("cross-ref '{label}' does not resolve"),
        });
    }
}

/// A section resolves if it equals a heading number OR is a prefix-parent of one (`§5` resolves when
/// only `### 5.3` exists).
fn section_resolves(headings: &BTreeSet<String>, section: &str) -> bool {
    if headings.contains(section) {
        return true;
    }
    let prefix = format!("{section}.");
    headings.iter().any(|h| h.starts_with(&prefix))
}

// ---------------------------------------------------------------------------------------------
// Class (f) — doc-count & RESOLVED claims (README.md only).
// ---------------------------------------------------------------------------------------------

fn class_f_readme(doc: &Doc, guards: &Guards, out: &mut Vec<Finding>) {
    class_f_doc_count(doc, guards, out);
    class_f_resolved(doc, guards, out);
}

/// (f.1) DOC-COUNT — count `^\| \[` rows ONLY between the `## 1.` heading and the next `## `.
fn class_f_doc_count(doc: &Doc, guards: &Guards, out: &mut Vec<Finding>) {
    let section1 = Regex::new(r"^##\s+1\.").expect("valid §1 regex");
    let next_section = Regex::new(r"^##\s").expect("valid next-section regex");
    let table_row = Regex::new(r"^\|\s*\[").expect("valid table-row regex");
    let total_literal = Regex::new(r"Total plan docs:\s*(\d+)").expect("valid total regex");
    let crosschecked =
        Regex::new(r"All\s*(\d+)\s*plan docs cross-checked").expect("valid xcheck regex");

    // Count rows between `## 1.` and the next `## `.
    let mut in_section = false;
    let mut row_count: usize = 0;
    for (i, line) in doc.lines.iter().enumerate() {
        if guards.fenced[i] {
            continue;
        }
        if section1.is_match(line) {
            in_section = true;
            continue;
        }
        if in_section && next_section.is_match(line) {
            break;
        }
        if in_section && table_row.is_match(line) {
            row_count += 1;
        }
    }

    // Assert the literals match the counted set.
    for (i, line) in doc.lines.iter().enumerate() {
        if guards.fenced[i] {
            continue;
        }
        if let Some(c) = total_literal.captures(line) {
            let claimed: usize = c[1].parse().unwrap_or(usize::MAX);
            if claimed != row_count {
                out.push(Finding {
                    file: doc.path.clone(),
                    line: i + 1,
                    class: 'f',
                    message: format!(
                        "README claims 'Total plan docs: {claimed}' but {row_count} index rows found"
                    ),
                });
            }
        }
        if let Some(c) = crosschecked.captures(line) {
            let claimed: usize = c[1].parse().unwrap_or(usize::MAX);
            if claimed != row_count {
                out.push(Finding {
                    file: doc.path.clone(),
                    line: i + 1,
                    class: 'f',
                    message: format!(
                        "README claims 'All {claimed} plan docs cross-checked' but {row_count} index rows found"
                    ),
                });
            }
        }
    }
}

/// (f.2) RESOLVED — count CF-A..CF-K entries under `### HIGH/MEDIUM/LOW`; assert the headline tallies
/// and that no CF-x lacks a RESOLVED marker under an "all RESOLVED" headline.
fn class_f_resolved(doc: &Doc, guards: &Guards, out: &mut Vec<Finding>) {
    let sev_header = Regex::new(r"^###\s+(HIGH|MEDIUM|LOW)\b").expect("valid sev header regex");
    let other_h3 = Regex::new(r"^###\s").expect("valid h3 regex");
    // A CF entry line: `- **CF-A — ...`. Capture the letter and whether it carries `[RESOLVED]`.
    let cf_entry = Regex::new(r"^\s*-\s*\*\*CF-([A-K])\b").expect("valid CF entry regex");
    // The CF-structure headline `... all 11 RESOLVED` (vs the separate `All 24 findings RESOLVED`
    // narrative tally of the six-lens gap review, which is NOT a CF count and is left unchecked).
    let headline = Regex::new(r"all\s+(\d+)\s+RESOLVED").expect("valid headline regex");

    let mut in_sev = false;
    let mut cf_letters: BTreeSet<char> = BTreeSet::new();
    let mut cf_missing_resolved: Vec<(usize, char)> = Vec::new();

    for (i, line) in doc.lines.iter().enumerate() {
        if guards.fenced[i] {
            continue;
        }
        if sev_header.is_match(line) {
            in_sev = true;
            continue;
        }
        if in_sev && other_h3.is_match(line) && !sev_header.is_match(line) {
            in_sev = false;
        }
        if in_sev && let Some(c) = cf_entry.captures(line) {
            let letter = c[1].chars().next().expect("one letter");
            cf_letters.insert(letter);
            if !line.contains("RESOLVED") {
                cf_missing_resolved.push((i + 1, letter));
            }
        }
    }

    let cf_count = cf_letters.len();

    // Headline tally: `... all 11 RESOLVED`.
    for (i, line) in doc.lines.iter().enumerate() {
        if guards.fenced[i] {
            continue;
        }
        if let Some(c) = headline.captures(line) {
            let claimed: usize = c[1].parse().unwrap_or(usize::MAX);
            if claimed != cf_count {
                out.push(Finding {
                    file: doc.path.clone(),
                    line: i + 1,
                    class: 'f',
                    message: format!(
                        "README claims 'all {claimed} RESOLVED' but {cf_count} CF-entries found"
                    ),
                });
            }
            // Under an "all RESOLVED" headline every CF entry must carry a RESOLVED marker.
            for (cf_line, letter) in &cf_missing_resolved {
                out.push(Finding {
                    file: doc.path.clone(),
                    line: *cf_line,
                    class: 'f',
                    message: format!(
                        "CF-{letter} lacks a RESOLVED marker under an 'all RESOLVED' headline"
                    ),
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Shared helpers.
// ---------------------------------------------------------------------------------------------

/// True when a line, stripped of whitespace and the never-finding glyph set, is empty (so it carries
/// no lintable token — a legend/status row).
fn line_is_only_glyphs(line: &str) -> bool {
    let stripped: String = line
        .chars()
        .filter(|c| !c.is_whitespace() && !NEVER_FINDING_GLYPHS.contains(c))
        .collect();
    stripped.is_empty() && line.chars().any(|c| NEVER_FINDING_GLYPHS.contains(&c))
}

/// Emit findings (stderr) + the final tally; return the process exit code.
fn report(findings: &[Finding], corpus_len: usize) -> ExitCode {
    if findings.is_empty() {
        println!("doc-lint OK: {corpus_len} docs, 6 classes clean");
        return ExitCode::SUCCESS;
    }
    let mut per_class: BTreeMap<char, usize> = BTreeMap::new();
    for f in findings {
        eprintln!("{}", f.render());
        *per_class.entry(f.class).or_default() += 1;
    }
    let tally: Vec<String> = ['a', 'b', 'c', 'd', 'e', 'f']
        .iter()
        .map(|c| format!("{c}:{}", per_class.get(c).copied().unwrap_or(0)))
        .collect();
    eprintln!(
        "doc-lint: {} findings ({})",
        findings.len(),
        tally.join(" ")
    );
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;

    /// Build a `Doc` from inline text (newline-split), for planted-violation unit tests.
    fn doc(path: &str, text: &str) -> Doc {
        Doc {
            path: path.to_owned(),
            lines: text.lines().map(str::to_owned).collect(),
        }
    }

    /// A minimal PRD that defines D1..D3, FR-25 [must], NFR-3 [reliability], and revision v1.1, so the
    /// corpus index has real definitions for the planted-violation tests to reference.
    fn prd_fixture() -> Doc {
        doc(
            "docs/PRD.md",
            "# unblock — PRD\n\
             - **Status:** APPROVED (v1.1)\n\
             ## 4. Decisions\n\
             | **D1** | a | b |\n\
             | **D2** | a | b |\n\
             | **D3** | a | b |\n\
             ## 5. Functional Requirements\n\
             - **FR-25 [must] — Self-update.** body\n\
             ## 6. Non-Functional Requirements\n\
             - **NFR-3 [reliability]** body\n\
             ### 6.1 sub\n",
        )
    }

    fn index_with(extra: &Doc) -> (Vec<Doc>, CorpusIndex) {
        let prd = prd_fixture();
        // Two-doc corpus: PRD + the doc under test. CorpusIndex::build only requires PRD.md present.
        let docs = vec![prd, doc(&extra.path, &extra.lines.join("\n"))];
        let index = CorpusIndex::build(&docs);
        (docs, index)
    }

    fn run_one(text_path: &str, text: &str) -> Vec<Finding> {
        let under = doc(text_path, text);
        let (docs, index) = index_with(&under);
        let target = docs
            .iter()
            .find(|d| d.path == text_path)
            .expect("doc present");
        let guards = Guards::build(target);
        let mut out = Vec::new();
        class_a_d_ids(target, &guards, &index, &mut out);
        class_b_fr_nfr(target, &guards, &index, &mut out);
        class_c_commands(target, &guards, &mut out);
        class_d_stamp(target, &guards, &index, &mut out);
        class_e_cross_refs(target, &guards, &index, &mut out);
        if target.path == "docs/plans/README.md" {
            class_f_readme(target, &guards, &mut out);
        }
        out
    }

    // ---- Planted-violation tests, one per class ----

    #[test]
    fn class_a_dangling_d_id() {
        let f = run_one(
            "docs/plans/README.md",
            "References D99 which is undefined.\n",
        );
        assert!(
            f.iter()
                .any(|x| x.class == 'a' && x.message.contains("D99")),
            "expected a class-a D99 finding, got {f:?}"
        );
    }

    #[test]
    fn class_b_wrong_tier_fr() {
        // FR-25 is canonically [must]; a `[v1.1]` ref must be flagged.
        let f = run_one("docs/plans/00-roadmap.md", "We ship FR-25 [v1.1] later.\n");
        assert!(
            f.iter().any(|x| x.class == 'b'
                && x.message.contains("FR-25")
                && x.message.contains("must")),
            "expected a class-b FR-25 tier finding, got {f:?}"
        );
    }

    #[test]
    fn class_c_non_canonical_command() {
        let f = run_one(
            "docs/plans/crates/unblock-cli.md",
            "Run `unblock upgrade` to update.\n",
        );
        assert!(
            f.iter()
                .any(|x| x.class == 'c' && x.message.contains("unblock upgrade")),
            "expected a class-c non-canonical command finding, got {f:?}"
        );
    }

    #[test]
    fn class_d_wrong_stamp() {
        let f = run_one(
            "docs/plans/STATUS.md",
            "Source of truth: PRD APPROVED v9.9 here.\n",
        );
        assert!(
            f.iter()
                .any(|x| x.class == 'd' && x.message.contains("v9.9")),
            "expected a class-d stamp finding, got {f:?}"
        );
    }

    #[test]
    fn class_e_unresolved_cross_ref() {
        let f = run_one(
            "docs/plans/crates/unblock-storage.md",
            "See spine §99 for details.\n",
        );
        assert!(
            f.iter()
                .any(|x| x.class == 'e' && x.message.contains("§99")),
            "expected a class-e unresolved cross-ref finding, got {f:?}"
        );
    }

    #[test]
    fn class_f_doc_count_mismatch() {
        // 17 index rows but the literal claims 16 — count drift.
        let mut body =
            String::from("# README\n\n## 1. Document index\n\n| Doc | Desc |\n|---|---|\n");
        for n in 0..17 {
            let _ = writeln!(body, "| [`f{n}.md`](f{n}.md) | desc |");
        }
        body.push_str("\n**Total plan docs: 16**\n\n## 2. Next\n");
        let f = run_one("docs/plans/README.md", &body);
        assert!(
            f.iter()
                .any(|x| x.class == 'f' && x.message.contains("16") && x.message.contains("17")),
            "expected a class-f doc-count finding, got {f:?}"
        );
    }

    // ---- Guard / no-false-positive tests ----

    #[test]
    fn fenced_lines_are_skipped() {
        // A non-canonical command INSIDE a fence must not fire class (c).
        let text = "```sh\nunblock upgrade\n```\n";
        let f = run_one("docs/plans/crates/unblock-cli.md", text);
        assert!(
            !f.iter().any(|x| x.class == 'c'),
            "fenced command must not fire class-c, got {f:?}"
        );
    }

    #[test]
    fn bare_command_in_prose_is_not_class_c() {
        // `unblock upgrade` in bare prose (no code span) must NOT fire — class (c) is code-span only.
        let f = run_one(
            "docs/plans/crates/unblock-cli.md",
            "We never call unblock upgrade.\n",
        );
        assert!(
            !f.iter().any(|x| x.class == 'c'),
            "bare-prose command must not fire class-c, got {f:?}"
        );
    }

    #[test]
    fn rejection_context_command_is_skipped() {
        // A negative example: `unblock create` → usage error. Must be skipped by the rejection guard.
        let f = run_one(
            "docs/plans/crates/unblock-cli.md",
            "`unblock create` → usage error (not a command).\n",
        );
        assert!(
            !f.iter().any(|x| x.class == 'c'),
            "rejection-context command must not fire class-c, got {f:?}"
        );
    }

    #[test]
    fn self_update_feature_is_allowed() {
        let f = run_one(
            "docs/plans/crates/unblock-cli.md",
            "Behind the `unblock self-update` feature gate.\n",
        );
        assert!(
            !f.iter().any(|x| x.class == 'c'),
            "self-update feature must be allowed, got {f:?}"
        );
    }

    #[test]
    fn sentence_final_stamp_does_not_overmatch() {
        // `PRD APPROVED v1.1.` (sentence-final period) must equal canonical v1.1, NOT read as v1.1.<>.
        let f = run_one("docs/plans/STATUS.md", "Truth: PRD APPROVED v1.1. Done.\n");
        assert!(
            !f.iter().any(|x| x.class == 'd'),
            "sentence-final v1.1. must not over-match, got {f:?}"
        );
    }

    #[test]
    fn bare_section_resolves_via_prefix_parent() {
        // `§6` in the PRD resolves because `### 6.1` exists (prefix-parent rule).
        let f = run_one("docs/PRD.md", "See §6 for the NFRs.\n");
        assert!(
            !f.iter().any(|x| x.class == 'e'),
            "§6 should resolve via prefix-parent 6.1, got {f:?}"
        );
    }

    /// Build a 2-doc corpus (a spine with the given headings + the doc under test) and run class (e).
    fn class_e_on(spine_headings: &str, under_path: &str, under_text: &str) -> Vec<Finding> {
        let prd = doc(
            "docs/PRD.md",
            "# PRD\n- APPROVED (v1.1)\n## 4. D\n| **D1** | a | b |\n",
        );
        let spine = doc(
            "docs/plans/01-design-spine.md",
            &format!("# spine\n{spine_headings}"),
        );
        let under = doc(under_path, under_text);
        let docs = vec![prd, spine, under];
        let index = CorpusIndex::build(&docs);
        let target = docs.iter().find(|d| d.path == under_path).expect("present");
        let guards = Guards::build(target);
        let mut out = Vec::new();
        class_e_cross_refs(target, &guards, &index, &mut out);
        out
    }

    #[test]
    fn class_e_sibling_refs_inherit_qualifier() {
        // `spine §3.1 ... §1.10` — the bare `§1.10` inherits the `spine` context (no barrier).
        let f = class_e_on(
            "### 3.1 a\n### 1.10 b\n",
            "docs/plans/crates/unblock-storage.md",
            "Per spine §3.1 the type lives in model §1.10 here.\n",
        );
        assert!(
            f.is_empty(),
            "both §3.1 and the inherited §1.10 should resolve against the spine, got {f:?}"
        );
    }

    #[test]
    fn class_e_barrier_breaks_inheritance() {
        // `spine §3.1 vs unblock-render.md §6` — the intervening other-doc mention is a barrier, so
        // `§6` must NOT inherit `spine`; it falls back to self (the storage plan, no §6) and is flagged.
        let f = class_e_on(
            "### 3.1 a\n### 6 b\n",
            "docs/plans/crates/unblock-storage.md",
            "Compare spine §3.1 vs unblock-render.md §6 here.\n",
        );
        assert!(
            f.iter().any(|x| x.class == 'e' && x.message.contains("§6")),
            "the barrier must stop `spine` leaking onto §6, surfacing it as unresolved, got {f:?}"
        );
        assert!(
            !f.iter().any(|x| x.message.contains("§3.1")),
            "§3.1 still resolves against the spine, got {f:?}"
        );
    }

    #[test]
    fn umbrella_fr_resolves_via_sub_id() {
        // Define an FR-1a in PRD, then a bare `FR-1` umbrella ref must resolve.
        let prd = doc(
            "docs/PRD.md",
            "# PRD\n- APPROVED (v1.1)\n## 4. D\n| **D1** | a | b |\n## 5. FR\n- **FR-1a [must] — Create.** x\n",
        );
        let under = doc("docs/plans/00-roadmap.md", "FR-1 umbrella reference.\n");
        let docs = vec![prd, under];
        let index = CorpusIndex::build(&docs);
        let target = &docs[1];
        let guards = Guards::build(target);
        let mut out = Vec::new();
        class_b_fr_nfr(target, &guards, &index, &mut out);
        assert!(
            !out.iter().any(|x| x.class == 'b'),
            "bare FR-1 umbrella should resolve via FR-1a, got {out:?}"
        );
    }
}
