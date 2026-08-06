#!/bin/sh
# ub-lp9.25-dangling-blocker-claims.sh — the EXECUTABLE done-gate for the D45 doc+code cascade
# (PRD §4 D45, tracked as `ub-lp9.25`; spec: docs/plans/ci-cd-and-distribution.md §2.1, the paragraph
# beginning "Named sub-check (its D45 sibling…)"). Runs as a step of the required `doc-lint` job,
# immediately after its D44 sibling.
#
# THAT PARAGRAPH IS NORMATIVE OVER THIS FILE. Every rule kind and every landing enforced here is named
# there; a rule that exists in one and not the other is a defect to be fixed in the SAME change. Do not
# "tidy" a row away as unspecified — read the spec paragraph first.
#
# SEQUENCING (stated first because it is the easiest thing to get wrong). The D45 cascade lands in TWO
# commits: a SPEC commit carrying only normative text, then an IMPLEMENTATION commit carrying the code.
# This script and its workflow step belong to the SECOND, because its contract-version rows and its
# code-side rows assert against the `CONTRACT_VERSION` constant, the re-blessed goldens and the guard
# bodies themselves. Wiring it earlier would turn a required job red for a change that has not happened.
#
# WHY THIS EXISTS
# ---------------
# A D-id cascade is "done" only when a grep for the retired framing returns zero live hits. Every
# retired claim below is a PREDICATE that is either fixed or explicitly allow-listed with a reason, and
# every landing site is its own NAMED presence predicate.
#
# It sweeps ALL TRACKED FILES with `git grep` on purpose. The doc-lint corpus is a fixed 19-file list
# (xtask/src/doc_lint.rs:31) that does NOT include docs/roadmap.html, README.md, AGENTS.md, CLAUDE.md,
# .github/ or any crates/** file — and this cascade lands in every one of those places.
#
# A NEGATIVE SWEEP CAN NEVER PROVE A COUNT IS GONE. A sentence wrapped between the numeral and the noun
# ("… only three\nedge-writing paths …") is unfindable in principle by ANY line-based regex. So every
# negative family below is PAIRED with a positive landing, and the strongest positives are deliberately
# SPELLING-INDEPENDENT: they key on DURABLE IDENTIFIERS (`ub-lp9.25`, `D45`, `create_bulk`, `reparent`,
# `insert_issue_in_tx`, a located table row) rather than on prose, because an identifier survives
# rewording and rewrapping while a sentence does not.
#
# FOUR RULE KINDS
#   N-n   FORBIDDEN framing — retired wording that must not survive.
#         mode `plain`  : every hit blocks (used where the retired words have no legitimate use left,
#                         THIS SPECIFICATION AND THIS FILE INCLUDED — which is why the retired adjective
#                         of the schema family is nowhere restated in prose, here or in ci-cd §2.1).
#         mode `escape` : a hit is clean when the SAME LINE also matches the escape regex (that is how a
#                         reciprocal "SUPERSEDED by D45" sentence stays legible without re-opening the
#                         claim). The escape match is CASE-SENSITIVE — D-ids are uppercase.
#         mode `subject`: a hit only counts when the SAME LINE also matches the subject regex, and the
#                         escape still applies. Used to keep a generic phrase from over-matching.
#   P-n   REQUIRED landing — a site the cascade MUST have reached, as a presence predicate over a named
#         file. A forbidden-thing REMOVAL is only proven when its replacement is pinned; without these
#         the check passes vacuously on a tree where the retired sentences were simply deleted and
#         nothing correct replaced them. A positive row ALSO cannot be satisfied by deleting the
#         sentence, which is the failure mode a negative family has.
#   PC-n  CODE-SIDE landing — the same predicate kind, aimed at `crates/**`. Kept under its own code
#         prefix (and its own count self-test) because these are the rows the spec calls "the code-side
#         teeth": they are the only reason this gate cannot go green on a tree where ONLY PROSE was
#         rewritten — i.e. the only reason the cascade cannot certify itself. Each row names the MUTANT
#         it kills, because a coverage claim is worth nothing until a mutant proves it.
#   Q-n   ROW-ANCHORED landing — at least ONE line must match the anchor, and EVERY line matching the
#         anchor must ALSO match the requirement. A vanished anchor is a FAILURE, never a pass. This is
#         the spelling-independent core: it cannot be dodged by rewrapping, and it cannot be satisfied
#         by deleting the sentence, since a deleted anchor is no longer a claim.
#   RC-n  CONTRACT-VERSION landing — a site that PUBLISHES the contract id, pinned against the one knob
#         below. Three selectors (presence / EXCLUSIVE / row-anchored); see that table's own header.
#
# KNOWN LIMITATION, stated rather than hidden: docs/PRD.md §4 is ONE PHYSICAL LINE per decision row, so
# a `D45` anywhere in the D44 row satisfies every `escape` family on that line. That is intended — a
# reciprocal cross-ref row IS a historical record.
#
# Exit: 0 = pass · 1 = BLOCK (retired framing survived, a required landing is missing, or an allow-list
#       entry rotted) · 2 = cannot evaluate (fail-closed).
set -u

# PORTABILITY (2 of 2): every variable expansion goes through `printf`, never `echo`. POSIX-mode `echo`
# interprets backslash escapes, so a `\b` inside a regex literal becomes a BACKSPACE byte and the
# pattern silently stops matching. Not a style choice.
say() { printf 'ub-lp9.25-claims: %s\n' "$*" >&2; }

git rev-parse --show-toplevel >/dev/null 2>&1 || { say "not a git repository"; exit 2; }
cd "$(git rev-parse --show-toplevel)" || { say "cannot cd to the repo root"; exit 2; }

SELF='scripts/checks/ub-lp9.25-dangling-blocker-claims.sh'

# The ONE knob in this file: the LIVE contract version. Change it here and nowhere else.
#
# D46 bumps `unblock.mcp.v1.8` -> `unblock.mcp.v1.9` (`ErrorCode::SchemaMismatch` moves off
# `HintShape::None` onto `ContextualText` — the stale-schema self-correction hint, a published byte in
# the `capabilities()` error map), with a `CONTRACT_HASH` re-pin; D45 bumped `unblock.mcp.v1.7` ->
# `unblock.mcp.v1.8` before it. Like its D44 sibling's knob this tracks the LIVE id, never
# a frozen historical one: the `EXCLUSIVE` rows below fail on a STALE literal, not merely a missing one.
#
# THE `\\?` IS LOAD-BEARING, not decoration. RC6 pins the D44 sibling gate's own knob LINE, and that line
# spells the id as a REGEX literal (`CONTRACT_RE='unblock\.mcp\.v1\.9'`) — i.e. with real backslash bytes
# between the segments. A pattern demanding a literal `.` there would never match it, and RC6 — the one
# row that exists to stop the sibling job going red in this very commit — would fail permanently for a
# reason having nothing to do with the id being current. `\\?` accepts BOTH spellings, prose and regex.
CONTRACT_RE='unblock\\?\.mcp\\?\.v1\\?\.9'

# The FAMILY the knob belongs to — ANY published contract id, current or retired. This is not a second
# copy of the knob: it is what makes an `EXCLUSIVE` row (below) able to say "no OTHER version literal
# may appear here", which is the only shape that catches a STALE version rather than a MISSING one. It
# carries the same `\\?` tolerance for the same reason.
CONTRACT_FAMILY_RE='unblock\\?\.mcp\\?\.v1\\?\.[0-9]+'

# The LIVE D-id range, in its TWO spellings, pinned ROW-ANCHORED at all 3 bump sites (Q4..Q7).
#
# WHY ROW-ANCHORED AND NOT FILE-LEVEL, at every one of the three. A file-level token check proves the
# literal is SOMEWHERE in the file — and a document that ALSO discusses the range in explanatory prose
# then passes with its NORMATIVE statement still carrying the retired literal. D45 hit exactly that on
# `docs/plans/ci-cd-and-distribution.md`. Anchoring each site on its own normative line closes the class
# everywhere instead of only where it was observed.
#
# WHY TWO SPELLINGS. `xtask/src/doc_lint.rs`'s bump site is ONE physical line carrying BOTH halves: the
# prose range `(D1..D48)` and the tokenizer's regex ALTERNATION `\bD(48|47|46|…)\b`. Pinning only the
# prose is exactly how that site rots into an undefined-D48 finding — the lint would stop tokenizing the
# id it is being told exists. Both halves are therefore separate Q rows on the same anchor.
RANGE_RE='D1\.\.D48'
RANGE_ALT_RE='D\(48\|47\|'

# =================================================================================================
# FORBIDDEN FRAMING — `code@mode@regex@second`
#
# `@` separates because several regexes contain `|`. All match regexes run CASE-INSENSITIVELY (a
# case-sensitive family is how the D43 sibling's predecessor passed vacuously on the very line it
# existed to catch). An empty `second` field is required for mode `plain`.
#
# N1 — THE UNDERCOUNT of the edge-writing paths. D45 closes FIVE; the framings this repo carried said
#      three (and, mid-Understand, four). The two the earlier framing never named are `issue update
#      {parent}` (the reparent, `apply_reparent`) and `issue create_bulk`. Numerals and their words are
#      both matched, and the family covers BOTH undercounts because "the undercount" is the claim being
#      retired, not one spelling of it. Its POSITIVE pair is P22/P23 (`.unblock/issues.jsonl` must name
#      five, and must name the two paths by identifier) plus P17/P18 (the roadmap co-occurrence rows).
# N2 — THE CLAIM THAT THIS REPAIR MINTS NO DECISION ID. It mints D45: Miguel's 2026-07-31 ruling puts
#      the guard in the SHARED per-record insert body, which REVERSES a clause D44 published, and
#      docs/PROCESS.md §3 makes a reversal ride a new id. Its POSITIVE pair is P1 (the PRD row) and Q1.
# N3 — THE RETRACTED PREMISE THAT THIS IS A CLASS THE SCHEMA SHOULD CLOSE. `dependencies.depends_on_id`
#      carries NO foreign key DELIBERATELY, because an external target is a legitimate blocker no
#      foreign key could ever satisfy; the repair is application-level and NO schema change is
#      authorised. `plain` mode: every hit blocks, this file and the ci-cd spec paragraph included,
#      which is why neither restates the retired adjective anywhere in prose. Positive pair: PC1..PC5.
# N4 — THE D44 SHARED-BODY HAZARD CLAUSE D45 REVERSES ("that body stays guard-free"). Escapable, so the
#      reciprocal record on the D44 PRD row and in the spine survives intact. Positive pair: PC3.
# N5 — see SUBJECT_FAMILIES: the spine forward-references that declare the class OPEN.
# =================================================================================================
FAMILIES="
N1@escape@(three|3|four|4) ((edge-writing|edge writing) (paths?|entry points?)|entry points|paths that write)@D45
N2@escape@(mints?|needs?|requires?) no (new )?(decision id|d-id)|no (new )?(decision id|d-id) is (minted|needed|required)@D45
N3@plain@missing (foreign[- ]?key|fk)|(foreign[- ]?key|fk) is missing|lacks? an? (foreign[- ]?key|fk)@
N4@escape@guard[- ]free@D45
"

# N5 is `subject` mode: the openness phrases are generic English, so a hit only counts on a line that is
# ABOUT this task. The subject is the DURABLE tracker id, not a sentence, so rewording the surrounding
# prose cannot dodge it. The escape is `D45`, which is how the two spine forward-references stay on the
# page as DISCHARGED history instead of being deleted (deleting them would lose the record of what D44
# shipped). Its positive pair is Q1..Q3 — the row-anchored rule that EVERY `ub-lp9.25` line in the PRD,
# the spine and the markdown roadmap also names `D45` — which closes the same class from the other side
# and, unlike this family, cannot be satisfied by deleting the sentence.
#   code@regex@subject@escape
SUBJECT_FAMILIES="
N5@(open|unresolved|outstanding) (follow-up|forward reference|question|sub-fork)|(stays|remains|is still|still) open@ub-lp9\.25@D45
"

# =================================================================================================
# ALLOW-LIST — `family|path-prefix|line-substring|reason`
#   family `*`         = every family
#   EMPTY line-substring = PATH-ONLY
#
# EXACTLY TWO ENTRIES, both PATH-ONLY, and the allow-list is consulted by the NEGATIVE scan only — the
# P/PC/Q/RC tables never read it, so neither path can dodge a required landing (and `.unblock/issues.jsonl`
# carries three P rows of its own, P21..P23).
#
# `.knowledge/wiki/runs/` — per docs/PROCESS.md §8 the wiki archive is DESCRIPTIVE and never normative.
# A run-report records how a PAST run went, so the D44 run-report legitimately still states that run's
# own scope, and a later correction never rewrites it.
#
# `.unblock/issues.jsonl` — the GENERATED tracker export (D5 model B), regenerated WHOLESALE by
# `sync export` and never hand-edited, so it is a RECORD of how the task was reasoned about, not a live
# claim about the product. It is ONE JSON object per LINE, so an entire issue record — description,
# every comment, the close reason — is a SINGLE physical line; docs/PROCESS.md §6 makes that comment
# thread the durable per-task narrative, and the thread for `ub-lp9.25` legitimately QUOTES the framings
# this cascade retires, including the record's own CORRECTIVE comment ("it is NOT three edge-writing
# paths: there are five") — which a negative family would itself flag. Demanding the retired sentences
# vanish from it would demand deleting the history the process exists to keep.
# =================================================================================================
ALLOW="
*|.knowledge/wiki/runs/||the DESCRIPTIVE wiki archive (PROCESS.md §8) — a past run-report records that run's own scope and is never rewritten
*|.unblock/issues.jsonl||the GENERATED tracker export (D5 model B), rewritten wholesale by \`sync export\`; one JSON object per LINE, and the thread legitimately quotes the framings this cascade retires
"

# =================================================================================================
# REQUIRED LANDINGS — `code@path@regex@what it proves`
#
# Every row is a NAMED predicate over a NAMED file. Row codes are `P` (documentation, tracker, CI) and
# `PC` (code-side teeth, in their own block below with their own count self-test).
# =================================================================================================
REQUIRE="
P1@docs/PRD.md@^\| \*\*D45\*\* \|@PRD §4 carries the D45 decision row (the id this repair was once claimed not to mint)
P2@docs/PRD.md@\*\*D44\*\*.*D45@the D44 row carries the reciprocal pointer: D45 SUPERSEDES its clause-(3) placement ruling and CORRECTS its entry-point count
P3@docs/PRD.md@\*\*D23\*\*.*D45@the D23 row carries the reciprocal pointer for clause (5): its unconditional ephemeral/-wisp- export exclusion is superseded by the blocker closure
P4@docs/plans/01-design-spine.md@D45@the interface SSOT records D45
P5@docs/plans/01-design-spine.md@ASCII-case-INSENSITIVE.*D45@the spine PINS the external: prefix AND its case rule (§1.9) — a concept NOTHING in the tree defined before D45, and the rule that keeps the write guard from ever being stricter than the read side
P6@docs/plans/implementation-plan.md@D45@the task DAG records D45
P7@docs/plans/00-roadmap.md@D45@the markdown roadmap NAMES D45 somewhere (that is all this predicate proves; the LOCATED roadmap pins are P8/P17/P18)
P8@docs/plans/00-roadmap.md@^\| .unblock-sync. \| ● \| ●@the §9 crate-impact table marks unblock-sync worked in v1.0.1 — that cell was BLANK before D45, and the exporter repair (clause 5) makes that crate gain CODE
P9@docs/roadmap.html@D45@the RENDERED roadmap lists D45 in its v1.0.1 card (OUTSIDE the 19-file doc-lint corpus, so nothing else in CI can catch it)
P10@docs/plans/crates/unblock-model.md@D45@the model crate plan records the L0 external: predicate
P11@docs/plans/crates/unblock-storage.md@insert_issue_in_tx.*D45|D45.*insert_issue_in_tx@LOCATED row: the storage crate plan names the SHARED per-record insert body as the guard's ONE home, not merely the decision id
P12@docs/plans/crates/unblock-sync.md@D45.*[Ee][Dd][Gg][Ee]@LOCATED row: the sync crate plan states the EDGE consequence (the corpus widens so a kept row's edge never outlives its target's line), not merely the id
P13@docs/plans/crates/unblock-engine.md@D45@the engine crate plan records the composed dangling findings + the doctor fold
P14@docs/plans/crates/unblock-mcp.md@D45@the mcp crate plan documents the new diagnostics action and the contract bump
P15@docs/plans/crates/unblock-cli.md@D45@the cli crate plan records the doctor fold
P16@docs/plans/ci-cd-and-distribution.md@ub-lp9\.25-dangling-blocker-claims@this gate is SPECIFIED, not merely wired
P17@docs/plans/00-roadmap.md@create_bulk.*(ub-lp9\.25|D45)|(ub-lp9\.25|D45).*create_bulk@CO-OCCURRENCE, not a token check: create_bulk already appeared in this file on rows unrelated to D45, so it must appear on a line that ALSO names this cascade or a bare token check passes vacuously
P18@docs/plans/00-roadmap.md@reparent.*(ub-lp9\.25|D45)|(ub-lp9\.25|D45).*reparent@CO-OCCURRENCE for the second never-named path, same reason as P17
P19@.github/workflows/ci.yml@ub-lp9\.25-dangling-blocker-claims@this gate actually RUNS in the required doc-lint job (so it can be neither wired-but-unspecified nor specified-but-unwired)
P20@.github/workflows/ci.yml@cargo test -p unblock-engine --features testkit --locked --test dangling@the deliverable is not green by NON-EXECUTION: every shipped testkit TEST step was storage-only, so a new ENGINE testkit cell would execute in NO job. This pins the step the required storage-testkit job gained
P21@.unblock/issues.jsonl@D45@the tracker record names the DECISION ID this repair mints (the record once claimed it minted none). Positive-only: the negative families are allow-listed on this path, so THIS is where its enforcement lives
P22@.unblock/issues.jsonl@(five|5) edge-writing@the tracker record carries the CORRECTED count. A token predicate is sound here and only here: a JSONL record is one physical line by construction, so no requirement can be split across a line break
P23@.unblock/issues.jsonl@create_bulk@the tracker record names the two paths the retired count never named, by DURABLE IDENTIFIER — this row for create_bulk, P24 for the reparent
P24@.unblock/issues.jsonl@reparent@the second half of P23 (see P23). Satisfy P21..P24 by updating the issue over the issue tool and re-exporting in the same commit — NEVER by hand-editing the generated file
P25@crates/unblock-cli/tests/create_deps_wire.rs@D45@EVERY REMOVAL PINS ITS REPLACEMENT, applied to crates/**: the one wire cell written to ANTICIPATE this change branches on is_error, so once the refusal lands it would PASS on its other branch — silently ceasing to pin anything while its docstring became false prose in a green suite. This pins the FILE, spelling-independently
"

# =================================================================================================
# CODE-SIDE TEETH — same format, same table semantics, own code prefix and own count self-test.
#
# WHY THEY EXIST, stated rather than left to inference: P1..P25 are almost all PROSE, and a prose-only
# table goes fully GREEN on a tree where only prose was rewritten — the cascade would certify itself.
# These rows are what make that impossible. Every one of them had ZERO matches before this change, so
# none can pass vacuously, and each names the MUTANT it kills.
#
# Coverage is one row per guarded SQL body (the three bodies the five wire entry points funnel through),
# plus the exporter, plus the shared L0 predicate and its case rule, plus its two non-storage callers,
# plus the new `dangling` action in all three of the places its bytes live, plus (since the 2026-08-02
# amendment) the two clauses of the SQL read that replaced the engine-side two-read composition.
#
# WHY THE AMENDMENT EARNED ITS OWN ROWS AND DID NOT JUST EDIT PC8's PROSE. PC8 keys on `fn
# dangling_findings`, which the amendment did not move — the ONE HOME is unchanged; only the WORK behind
# it moved. So PC8 alone would go green on a tree where the fn still exists and does the old, slow thing.
# PC13/PC14 are the rows that cannot: both had ZERO matches before the amendment, because this file
# carried no join and no ORDER BY over `dependencies` at all.
# =================================================================================================
REQUIRE_CODE="
PC1@crates/unblock-model/src/id.rs@pub fn is_external_target@the ONE shared external: predicate EXISTS at L0 — the only layer both unblock-storage (L2) and unblock-engine (L5) may depend on. MUTANT KILLED: deleting it and re-opening the two disagreeing dialects
PC2@crates/unblock-model/src/id.rs@eq_ignore_ascii_case@the predicate is ASCII-case-INSENSITIVE. MUTANT KILLED: reverting to the shipped case-SENSITIVE starts_with(\"external:\"), which would make the WRITE guard stricter than the read side — the ready/blocked SQL LIKE 'external:%' already folds case, so EXTERNAL:jira-1 would be refused on write yet treated as external on read
PC3@crates/unblock-storage/src/libsql/crud.rs@batch_ids\.contains\(target\)@GUARDED BODY 1 of 3 — the SHARED per-record insert body (insert_issue_in_tx), covering create_issue, create_issues, the bulk path and BOTH D5 import legs, with the BATCH arm present. MUTANT KILLED: deleting the guard, and separately deleting only the batch arm — which would ACCEPT a backward intra-file reference and REJECT a forward one, an ordering-dependent refusal of a legal import
PC4@crates/unblock-storage/src/libsql/crud.rs@is_external_target\(&parent_id\)@GUARDED BODY 2 of 3 — apply_reparent, the 'issue update {parent}' path the retired count never named. MUTANT KILLED: deleting the reparent guard (a phantom parent is written with isError:false and then LISTED by the very diagnostic this decision adds), and separately dropping the external: carve-out, which would make an external: PARENT illegal and re-fork the predicate per edge type
PC5@crates/unblock-storage/src/libsql/deps.rs@is_external_target\(&dep\.depends_on_id\)@GUARDED BODY 3 of 3 — add_dependency, the 'dep {action:\"add\"}' path. MUTANT KILLED: deleting the TARGET guard from the dependency-add body, the endpoint the original defect report reproduced live over the wire
PC6@crates/unblock-sync/src/export.rs@fn corpus_closed_under_blockers@the exporter DROPS NOTHING: the corpus widens to the transitive closure of its blockers. MUTANT KILLED: the 'obvious' repair D45 rejects — filtering the dangling EDGE out of the exported line, which would silently convert BLOCKED work into READY work in the destination workspace
PC7@crates/unblock-engine/src/session/bulk.rs@is_external_target@the BULK RESOLVER shares the ONE predicate. MUTANT KILLED: restoring the case-SENSITIVE starts_with(\"external:\") here, which is where the two dialects disagreed — EXTERNAL:jira-1 was an external blocker to the ready query and an ordinary id to this parser
PC8@crates/unblock-engine/src/diagnostics.rs@fn dangling_findings@the listing view has ONE HOME IN THE ENGINE, shared by the diagnostics action and the doctor fold, which is what keeps unblock-health's D29-F3 purity clause (run_doctor stays pure, non-async, storage-free) intact. MUTANT KILLED: making health storage-aware, i.e. reversing a second shipped clause. (Its 2026-08-02 amendment moved the WORK behind this fn into one SQL read — PC13/PC14 — but not the home; the anchor is unchanged because the home is)
PC13@crates/unblock-storage/src/libsql/diagnostics.rs@LEFT JOIN issues i ON i\.id = d\.depends_on_id@the 2026-08-02 amendment LANDED: the listing view is ONE query whose join tests target EXISTENCE ALONE. This is the code-side tooth for the amendment itself — before it, this file had no join at all. MUTANT KILLED: appending a status term to that ON clause (AND i.status NOT IN ('closed','tombstone')), which reports every CLOSED and TOMBSTONED blocker as dangling — the retired FULLY-INCLUSIVE-filters trap returning through a new door. The row pins the join SPELLING because that is the line the mutant edits; the BEHAVIOUR is pinned by two independent cells (crates/unblock-engine/tests/dangling.rs and the NFR-16 contract case), both measured RED under that mutant
PC14@crates/unblock-storage/src/libsql/diagnostics.rs@ORDER BY d\.issue_id ASC, d\.type ASC, d\.depends_on_id ASC@the PINNED finding order moved INTO the SQL when the engine-side re-sort was deleted, so the snapshot-stable output (NFR-14) now depends on this clause alone. MUTANT KILLED: dropping the ORDER BY, or keying it on (issue_id, depends_on_id) — the engine no longer re-sorts, deliberately, because a redundant sort would mask exactly this
PC9@crates/unblock-model/src/results.rs@Dangling@DiagnosticKind GREW a Dangling variant rather than reusing Lint. MUTANT KILLED: reusing an existing kind, so a response would declare a kind that lies about what it carries
PC10@crates/unblock-mcp/src/tools/diagnostics.rs@description = \"Diagnostics:[^\"]*dangling@the tool DESCRIPTION is contract bytes TWICE and this is copy 1 of 2 — the #\[tool(description)\] WIRE literal (rmcp requires a literal there, so the shared const cannot be used in the attribute). MUTANT KILLED: adding the action while leaving the published description naming only the seven GA kinds
PC11@crates/unblock-mcp/src/resources/capabilities.rs@DIAGNOSTICS_TOOL_DESCRIPTION@copy 2 of 2 — the capabilities() descriptor, which CONTRACT_HASH digests. It is SOURCED FROM THE SHARED CONST, which is the mechanism that makes the two copies AGREE. MUTANT KILLED: hand-copying the bytes here, after which the two copies drift independently and only one of them is what tools/list serves
PC12@crates/unblock-engine/tests/dangling.rs@D45@the new ENGINE testkit cell EXISTS at the path the workflow step in P20 executes. Without this row P20 could name a target that is not there, and the required job would fail for the wrong reason
"

# =================================================================================================
# ROW-ANCHORED LANDINGS — `code@path@anchor@regex@what it proves`
#
# Semantics: at least ONE line in `path` must match `anchor`, and EVERY line matching `anchor` must ALSO
# match `regex`. A vanished anchor is a FAILURE, never a pass — an anchor that no longer exists proves
# nothing, and silently proving nothing is how a pin rots.
#
# Q1..Q3 are the SPELLING-INDEPENDENT CORE of this gate. "Every line naming `ub-lp9.25` must also name
# `D45`" closes both spine forward-references and every roadmap citation at once; it cannot be dodged by
# rewrapping (the anchor is an identifier, not a sentence) and it cannot be satisfied by DELETING the
# sentence, since a deleted anchor is no longer a claim.
#
# Q4..Q7 are the three D-range bump sites. See the RANGE_RE header above for why all three are anchored
# and why doc_lint.rs needs both halves of its one line.
# =================================================================================================
REQUIRE_ROW="
Q1@docs/PRD.md@ub-lp9\.25@D45@every PRD line citing the tracker id also names the decision it minted
Q2@docs/plans/01-design-spine.md@ub-lp9\.25@D45@both spine forward-references are DISCHARGED, not left declaring the class open
Q3@docs/plans/00-roadmap.md@ub-lp9\.25@D45@every roadmap citation of the tracker id also names the decision
Q4@CLAUDE.md@^\| .docs/PRD\.md. \| Product truth@$RANGE_RE@D-range bump site 1 of 3, LOCATED on the document-map row that states the range
Q5@docs/plans/ci-cd-and-distribution.md@\*\*\(a\) D-id coherence\*\*@$RANGE_RE@D-range bump site 2 of 3, LOCATED on the class-(a) statement — the ONE place in that file allowed to quote the live range, so no explanatory prose can satisfy this pin
Q6@xtask/src/doc_lint.rs@Spec tokenizes@$RANGE_RE@D-range bump site 3 of 3, half 1 of 2: the PROSE range on the tokenizer comment line
Q7@xtask/src/doc_lint.rs@Spec tokenizes@$RANGE_ALT_RE@site 3 of 3, half 2 of 2: the TOKENIZER ALTERNATION on that same line. Pinning only the prose is how this site rots into an undefined-D45 finding — the lint would stop tokenizing the id it is being told exists
"

# =================================================================================================
# CONTRACT-VERSION LANDINGS — `code@path@selector@what it proves`
#
# Separate from REQUIRE only because every row shares one knob (`CONTRACT_RE`).
#
# The `selector` field chooses the PREDICATE, because "the contract id is current here" is three
# different questions in three different kinds of file:
#
#   (empty)      PRESENCE — the file must name the current version somewhere. Correct where the file
#                LEGITIMATELY also names retired ones: `options.rs` documents the whole bump chain in
#                the const's doc-comment, so an exclusive rule would block on its own history.
#
#   EXCLUSIVE    NO OTHER version literal may appear in the file at all, and the current one must. For
#                prose that makes PRESENT-TENSE claims and keeps no history: README.md, and the
#                GENERATED AGENTS.md, which publishes the id to every agent in the workspace.
#
#   <regex>      ROW-ANCHORED — at least one line matches the regex, and EVERY line that matches it also
#                names the current version. For a file whose DECLARING line sits among lines that
#                legitimately name older versions.
#
# RC6 IS A HARD BLOCKER POINTING AT ANOTHER GATE, and it is enumerated here so it is never rediscovered
# as a mystery failure. The shipped D44 sub-check hard-codes its OWN contract knob and demands that
# literal be present in four files — two of them EXCLUSIVE/ROW-ANCHORED, which fail on a STALE literal.
# So the instant D45 bumps those files, THAT required job turns red for a reason having nothing to do
# with D44 unless its one knob moves in the SAME commit. (The D-range couples the same way but lands
# EARLIER: the D44 sub-check's `RANGE_RE` guards PROSE, so it rides the SPEC commit that mints the id,
# while this contract knob waits for the implementation commit that moves the code constant.)
# =================================================================================================
REQUIRE_CONTRACT="
RC1@crates/unblock-mcp/src/options.rs@@the CONTRACT_VERSION const moved; PRESENCE, because this const's doc-comment documents the whole retired bump chain
RC2@crates/unblock-mcp/tests/public_api.rs@@the second, independent CONTRACT_VERSION pin moved with it
RC3@README.md@EXCLUSIVE@the most user-facing document in the repo publishes the contract id as a present-tense claim and keeps no bump history, so NO retired id may survive there
RC4@AGENTS.md@EXCLUSIVE@the GENERATED agent-facing file publishes the id to every agent in the workspace; it is regenerated by \`unblock agents\` (agents_digest() moves for D45, since it walks each tool's arms to publish actions), never hand-edited, and keeps no history
RC5@docs/plans/crates/unblock-mcp.md@^\| .pub const CONTRACT_VERSION@the OWNING crate plan's declaring row names the current id; row-anchored, because that row also records the bump CHAIN and the file names retired ids elsewhere
RC6@scripts/checks/d44-create-deps-claims.sh@^CONTRACT_RE=@HARD BLOCKER (see this table's header): the sibling D44 gate's own knob line must carry the SAME live id, or that required job goes red in this commit for a reason having nothing to do with D44
"

blocked=0

# -------------------------------------------------------------------------------------------------
allowed() { # $1 = family, $2 = path, $3 = line text -> 0 if allowed
  _f="$1"; _p="$2"; _t="$3"
  printf '%s\n' "$ALLOW" | while IFS='|' read -r fam prefix substr _reason; do
    [ -n "$fam" ] || continue
    [ "$fam" = "$_f" ] || [ "$fam" = '*' ] || continue
    case "$_p" in "$prefix"*) ;; *) continue ;; esac
    if [ -z "$substr" ]; then exit 9; fi
    case "$_t" in *"$substr"*) exit 9 ;; esac
  done
  [ "$?" = "9" ]
}

emit_hits() { # $1 code, $2 regex, $3 escape (may be empty), $4 subject (may be empty)
  _code="$1"; _re="$2"; _esc="$3"; _sub="$4"
  git grep -n -I -i -E "$_re" -- . 2>/dev/null | while IFS= read -r hit; do
    _path="${hit%%:*}"
    _rest="${hit#*:}"
    _line="${_rest%%:*}"
    _text="${_rest#*:}"
    # This check's own regex literals are the RULES, not claims about the product — without this
    # exclusion the file flags every family it defines. Specified, not incidental: do not delete it.
    case "$_path" in "$SELF") continue ;; esac
    if [ -n "$_sub" ]; then
      printf '%s' "$_text" | grep -q -i -E "$_sub" || continue
    fi
    if [ -n "$_esc" ] && printf '%s' "$_text" | grep -q -E "$_esc"; then continue; fi
    if allowed "$_code" "$_path" "$_text"; then continue; fi
    printf '%s\n' "$_path:$_line: retired dangling-blocker framing survived ($_code)"
  done
}

# PORTABILITY (1 of 2), deliberate: every `$( … )` in this file substitutes a FUNCTION CALL, never an
# inline loop containing a `case`. macOS `/bin/sh` (bash 3.2) mis-parses a `case` arm's `)` inside `$( )`
# and silently produces garbage instead of failing — this script would then "pass" vacuously on a
# developer machine. Both siblings avoid it the same way.
scan_all() {
  printf '%s\n' "$FAMILIES" | while IFS='@' read -r code mode re esc; do
    [ -n "$code" ] || continue
    if [ "$mode" = plain ]; then
      emit_hits "$code" "$re" '' ''
    elif [ "$mode" = escape ]; then
      emit_hits "$code" "$re" "$esc" ''
    else
      printf '%s\n' "FAMILY-TABLE: [$code] unknown mode '$mode'"
    fi
  done
  printf '%s\n' "$SUBJECT_FAMILIES" | while IFS='@' read -r code re sub esc; do
    [ -n "$code" ] || continue
    emit_hits "$code" "$re" "$esc" "$sub"
  done
}

findings="$(scan_all)"
if [ -n "$findings" ]; then
  printf '%s\n' "$findings" >&2
  say "BLOCKED — the framing above survived the D45 cascade. Rewrite it, add the reciprocal D45 cross-ref, or add an allow-list entry with a reason."
  blocked=1
fi

# -------------------------------------------------------------------------------------------------
# REQUIRED LANDINGS — each row must match at least one line in its own file; each ROW-ANCHORED row must
# find its anchor AND satisfy every anchored line.
# -------------------------------------------------------------------------------------------------
check_presence() { # $1 table text, $2 human label
  printf '%s\n' "$1" | while IFS='@' read -r code path re reason; do
    [ -n "$code" ] || continue
    if [ ! -f "$path" ]; then
      printf '%s\n' "$path: [$code] REQUIRED cascade target is missing from the tree ($reason)"
      continue
    fi
    git grep -q -I -E "$re" -- "$path" 2>/dev/null \
      || printf '%s\n' "$path: [$code] the D45 cascade never landed here — no line matches /$re/ ($reason)"
  done
}

check_landings() {
  check_presence "$REQUIRE" 'documentation'
  check_presence "$REQUIRE_CODE" 'code'
  printf '%s\n' "$REQUIRE_ROW" | while IFS='@' read -r code path anchor re reason; do
    [ -n "$code" ] || continue
    if [ ! -f "$path" ]; then
      printf '%s\n' "$path: [$code] REQUIRED cascade target is missing from the tree ($reason)"
      continue
    fi
    rows="$(git grep -n -I -E "$anchor" -- "$path" 2>/dev/null)"
    if [ -z "$rows" ]; then
      printf '%s\n' "$path: [$code] the anchor line /$anchor/ no longer exists, so this pin now proves NOTHING ($reason)"
      continue
    fi
    # `git grep -n -- <one path>` prints `path:lineno:text`, so the LINE NUMBER is field 2.
    bad="$(printf '%s\n' "$rows" | grep -v -E "$re" | cut -d: -f2)"
    if [ -n "$bad" ]; then
      printf '%s\n' "$path: [$code] the anchored line(s) $(printf '%s' "$bad" | tr '\n' ' ') do not match /$re/ ($reason)"
    fi
  done
  printf '%s\n' "$REQUIRE_CONTRACT" | while IFS='@' read -r code path selector reason; do
    [ -n "$code" ] || continue
    if [ ! -f "$path" ]; then
      printf '%s\n' "$path: [$code] REQUIRED cascade target is missing from the tree ($reason)"
      continue
    fi
    # Every kind first demands the CURRENT id be present, so no row can pass on an empty file.
    if ! git grep -q -I -E "$CONTRACT_RE" -- "$path" 2>/dev/null; then
      printf '%s\n' "$path: [$code] no line matches /$CONTRACT_RE/ ($reason)"
      continue
    fi
    if [ "$selector" = "EXCLUSIVE" ]; then
      # Any version literal that is NOT the current one is a stale published id.
      stale="$(git grep -h -I -E "$CONTRACT_FAMILY_RE" -- "$path" 2>/dev/null \
                 | grep -o -E "$CONTRACT_FAMILY_RE" | grep -v -E "^$CONTRACT_RE\$" | sort -u)"
      if [ -n "$stale" ]; then
        printf '%s\n' "$path: [$code] a RETIRED contract id survives here: $(printf '%s' "$stale" | tr '\n' ' ') ($reason)"
      fi
    elif [ -n "$selector" ]; then
      # Row-anchored: the declaring line(s) must themselves name the current id.
      rows="$(git grep -n -I -E "$selector" -- "$path" 2>/dev/null)"
      if [ -z "$rows" ]; then
        printf '%s\n' "$path: [$code] the anchor line /$selector/ no longer exists, so this pin now proves NOTHING ($reason)"
      else
        stale_rows="$(printf '%s\n' "$rows" | grep -v -E "$CONTRACT_RE" | cut -d: -f2)"
        if [ -n "$stale_rows" ]; then
          printf '%s\n' "$path: [$code] the declaring row(s) at line(s) $(printf '%s' "$stale_rows" | tr '\n' ' ') do not name /$CONTRACT_RE/ ($reason)"
        fi
      fi
    fi
  done
}

missing="$(check_landings)"
if [ -n "$missing" ]; then
  printf '%s\n' "$missing" >&2
  say "BLOCKED — the cascade is incomplete; the sites above were never updated."
  blocked=1
fi

# -------------------------------------------------------------------------------------------------
# SELF-TEST 1 — every allow-list entry must still match a real line. An entry that matches nothing means
# the line it excused was reworded or deleted, and the exemption is now silently widening this check's
# blind spot.
# -------------------------------------------------------------------------------------------------
# `code<TAB>regex` for EVERY family in both tables — the one place the two tables are unified.
family_pairs() {
  printf '%s\n' "$FAMILIES" | while IFS='@' read -r code mode re esc; do
    [ -n "$code" ] && printf '%s\t%s\n' "$code" "$re"
  done
  printf '%s\n' "$SUBJECT_FAMILIES" | while IFS='@' read -r code re sub esc; do
    [ -n "$code" ] && printf '%s\t%s\n' "$code" "$re"
  done
}

check_allow_rot() {
  printf '%s\n' "$ALLOW" | while IFS='|' read -r fam prefix substr reason; do
    [ -n "$fam" ] || continue
    hit=0
    while IFS="$(printf '\t')" read -r code re; do
      [ -n "$code" ] || continue
      [ "$fam" = '*' ] || [ "$fam" = "$code" ] || continue
      m="$(git grep -n -I -i -E "$re" -- "$prefix" 2>/dev/null \
            | { if [ -n "$substr" ]; then grep -F "$substr"; else cat; fi; })"
      if [ -n "$m" ]; then hit=1; break; fi
    done <<EOF
$(family_pairs)
EOF
    [ "$hit" = "1" ] || printf '%s\n' "$prefix: allow-list entry for $fam matches NOTHING (reason was:$reason)"
  done
}

rotted="$(check_allow_rot)"
if [ -n "$rotted" ]; then
  printf '%s\n' "$rotted" >&2
  say "BLOCKED — a rotted allow-list entry silently widens this check's blind spot. Delete it."
  blocked=1
fi

# -------------------------------------------------------------------------------------------------
# SELF-TEST 2 — no rule table may have SHRUNK below the counts it shipped with. A table silently emptied
# by a bad edit would make every scan above a vacuous pass, and the tables are counted SEPARATELY so
# that adding a prose row can never mask the deletion of a code-side one.
# -------------------------------------------------------------------------------------------------
count_rows() { # $1 = table text, $2 = code prefix regex
  printf '%s\n' "$1" | grep -c "$2"
}

check_table_floor() { # $1 label, $2 actual, $3 floor
  if [ "$2" -lt "$3" ]; then
    say "BLOCKED — the $1 table has $2 rows; it shipped with $3. A rule was dropped."
    return 1
  fi
  return 0
}

n_count="$(count_rows "$FAMILIES$SUBJECT_FAMILIES" '^N[0-9]')"
p_count="$(count_rows "$REQUIRE" '^P[0-9]')"
pc_count="$(count_rows "$REQUIRE_CODE" '^PC[0-9]')"
q_count="$(count_rows "$REQUIRE_ROW" '^Q[0-9]')"
rc_count="$(count_rows "$REQUIRE_CONTRACT" '^RC[0-9]')"

check_table_floor 'forbidden-framing (N)' "$n_count" 5 || blocked=1
check_table_floor 'required-landing (P)' "$p_count" 25 || blocked=1
check_table_floor 'code-side teeth (PC)' "$pc_count" 14 || blocked=1
check_table_floor 'row-anchored (Q)' "$q_count" 7 || blocked=1
check_table_floor 'contract-version (RC)' "$rc_count" 6 || blocked=1

[ "$blocked" = "0" ] || exit 1
say "OK — no retired dangling-blocker framing survived outside the allow-list, every required documentation, tracker, CI and CODE site landed, and every allow-list entry still matches."
exit 0
