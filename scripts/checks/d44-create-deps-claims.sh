#!/bin/sh
# d44-create-deps-claims.sh — the EXECUTABLE done-gate for the D44 doc cascade
# (PRD §4 D44; spec: docs/plans/ci-cd-and-distribution.md §2.1). Runs as a step of the required
# `doc-lint` job, immediately after its D43 sibling.
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
# TWO RULE KINDS
#   F-n   FORBIDDEN framing — retired wording that must not survive.
#         mode `plain`  : every hit blocks.
#         mode `escape` : a hit is clean when the SAME LINE also matches the escape regex (that is how
#                         a reciprocal "SUPERSEDED by D44" sentence stays legible without re-opening
#                         the claim). The escape match is CASE-SENSITIVE — D-ids are uppercase.
#         mode `subject`: a hit only counts when the SAME LINE also matches the subject regex, and the
#                         escape still applies. Used to keep a generic phrase from over-matching.
#   R-n   REQUIRED landing — a site the cascade MUST have reached. A forbidden-thing REMOVAL is only
#         proven when its replacement is pinned; without these the check passes vacuously on a tree
#         where the retired sentences were simply deleted and nothing correct replaced them.
#   RC-n  CONTRACT-VERSION landing — a site that PUBLISHES the contract id, pinned against the one
#         knob below. Three predicate kinds (presence / EXCLUSIVE / row-anchored); see that table's
#         own header for which file needs which, and for the three green gates the two doc-side rows
#         were added to close.
#
# KNOWN LIMITATION, stated rather than hidden: docs/PRD.md §4 is ONE PHYSICAL LINE per decision row, so
# a `D44` anywhere in the D42 row satisfies every `escape` family on that line. That is intended — a
# reciprocal cross-ref row IS a historical record.
#
# Exit: 0 = pass · 1 = BLOCK (retired framing survived, a required landing is missing, or an allow-list
#       entry rotted) · 2 = cannot evaluate (fail-closed).
set -u

say() { echo "d44-claims: $*" >&2; }

git rev-parse --show-toplevel >/dev/null 2>&1 || { say "not a git repository"; exit 2; }
cd "$(git rev-parse --show-toplevel)" || { say "cannot cd to the repo root"; exit 2; }

SELF='scripts/checks/d44-create-deps-claims.sh'

# The ONE knob in this file: the LIVE contract version. Change it here and nowhere else.
#
# WHY IT MOVES WHEN A LATER DECISION BUMPS THE CONTRACT, even though this is the D44 gate. The four
# `REQUIRE_CONTRACT` rows below assert that the four PUBLISHING sites (the code constant, its
# independent test pin, README.md and the owning crate plan's declaring row) all name the SAME id —
# and two of them are EXCLUSIVE/ROW-ANCHORED, i.e. they fail on a STALE literal, not merely a missing
# one. So the moment any decision bumps the contract, those sites move and THIS required job goes red
# for a reason having nothing to do with D44 unless this one line moves in the SAME commit. It is
# therefore pinned to the LIVE id, never to a frozen historical one — the same discipline `RANGE_RE`
# below already uses. D46 (v1.0.1, the `comments` forward migration) bumped it
# `unblock.mcp.v1.8` -> `unblock.mcp.v1.9` (`ErrorCode::SchemaMismatch` moves off `HintShape::None`
# onto `ContextualText` — a published byte in the `capabilities()` error map), and this edit rode the
# D46 IMPLEMENTATION commit, which is where the code constant and the re-blessed goldens move
# (`RANGE_RE` differs: it guards prose, so it rides the SPEC commit that mints the id). D45 bumped it
# `unblock.mcp.v1.7` -> `unblock.mcp.v1.8` the same way. Both couplings are enumerated in
# `docs/plans/ci-cd-and-distribution.md` §2.1 so neither is rediscovered as a mystery failure.
CONTRACT_RE='unblock\.mcp\.v1\.9'

# The LIVE D-id range, pinned at its 3 bump sites (R16/R18 file-level, RW1 ROW-ANCHORED). It tracks the
# CURRENT range and never a frozen historical one, so minting a new D-id moves THIS ONE LINE instead of
# three table rows.
#
# WHY ONE OF THE THREE IS ROW-ANCHORED AND THE OTHER TWO ARE NOT. A file-level token check proves the
# literal is SOMEWHERE in the file. That is sufficient for `CLAUDE.md` and `xtask/src/doc_lint.rs`, which
# carry exactly one occurrence each (the bump site itself). It is NOT sufficient for
# `docs/plans/ci-cd-and-distribution.md`, which also DESCRIBES this pin in prose: while that prose quoted
# the live literal, reverting the normative class-(a) statement to the previous range left the file-level
# check GREEN — the prose satisfied it. D45 hit exactly that. Two things fix it together and both are
# required: the ci-cd prose now names the range instead of quoting it, and its row moved to REQUIRE_ROW
# below, anchored on the class-(a) statement itself. Neither alone is enough — prose drifts back, and an
# anchor on a line carrying two occurrences would be satisfied by the wrong one.
RANGE_RE='D1\.\.D47'

# The FAMILY the knob belongs to — ANY published contract id, current or retired. This is not a second
# copy of the knob: it is what makes an `EXCLUSIVE` row (below) able to say "no OTHER version literal
# may appear here", which is the only shape that catches a STALE version rather than a MISSING one.
CONTRACT_FAMILY_RE='unblock\.mcp\.v1\.[0-9]+'

# =================================================================================================
# FORBIDDEN FRAMING — `code@mode@regex@second`
#
# `@` separates because several regexes contain `|`. All match regexes run CASE-INSENSITIVELY (a
# case-sensitive family is how the D43 sibling's predecessor passed vacuously on the very line it
# existed to catch). An empty `second` field is required for mode `plain`.
# =================================================================================================
FAMILIES="
F1@escape@in/after the same( write)? tx@D44
F2@plain@resolves [^ ]{0,3}new\.deps@
F3@plain@NOT endorsed here@
F4@plain@(loops?[^.]{0,25}add_dependency|add_dependency\`? loop)@
F5@plain@comes verbatim from the client@
F6@plain@dependent issue id \(source\)@
F7@plain@Create\.deps.{0,2} element@
F8@plain@PINNED KNOWN FAILURE@
F9@escape@non[-_ ]?atomic@D44
F10@escape@FK[- ]fail@D44
F11@escape@round-trip is claimed@D44
F14@escape@cannot[- ](know|supply)[- ]the@D44
F15@escape@unblock-engine.{0,40}(not touched|untouched)@\bD4[0-9]\b
F16@escape@(single[-_ ]create[-_ ]paths?[-_ ]unchanged|the bulk path is additive)@D44
"

# F16 keys on the SAME retired D22 clause as F12 below, in verbless spellings. F12 keys on a verb
# (`paths ARE unchanged`), and a verb is exactly what the clause loses when it is written as a section
# header (`// Single-create paths unchanged`), as a Rust test-fn identifier
# (`async fn single_create_paths_unchanged`), or as the additive-bulk restatement it was coined to
# justify (`the bulk path is additive`). Each of those is FALSE after D44, which extends the seeded
# one-transaction insert to the single create. A test FUNCTION NAME asserting a retired claim is the
# worst shape of all: the suite goes green under a name that says the opposite of what shipped, and no
# verb-carrying regex can ever see it. The escape is plain `D44` (not the `D4[0-9]` of F15) because two
# DOCUMENTATION lines quote the clause verbatim WHILE superseding it — the PRD D44 row and the spec
# paragraph in `docs/plans/ci-cd-and-distribution.md` §2.1 — and a reciprocal record is not a live
# claim. The escape is therefore load-bearing at more than one site; do not assume it excuses only one.

# F13 and F12 are `subject` mode: the phrase alone is generic, so it only counts on a line that is ABOUT
# the create-with-deps path. Kept in their own table because they carry a third regex.
#   code@regex@subject@escape
#
# F12 keys on the D22 clause D44 SUPERSEDES in its VERB-CARRYING spellings ("paths STAY unchanged",
# "paths ARE unchanged"); F16 above keys on verbless spellings of the same clause.
#
# F12's verb is an alternation — but the phrase `path is UNCHANGED` on its own is a TRUE and unrelated
# statement elsewhere in the tree, the live example being `crates/unblock-engine/src/session/ids.rs:64`
# (about the id allocator threading empty in-batch context, which D44 does not touch). Hence the subject
# regex: the clause is only the clause when the SAME LINE names its object, the `create_issue`/`create`
# pair. A bare widening to `paths?` blocks on that line forever and teaches the next reader to add
# allow-list entries for true sentences. (`docs/plans/implementation-plan.md:55`, about `create(&Issue)`
# never MINTING, is the other true sentence of that shape, but it is no longer an argument for the
# subject regex: this cascade put the token `D44` on that line, so it now clears the escape anyway.)
SUBJECT_FAMILIES="
F13@(separately[- ]tracked|tracked separately)@(create ?\{deps|create\.deps|create-with-deps|issue create)@D44
F12@paths? (stay|stays|are|is|remain|remains) UNCHANGED@create_issue.\/.create@D44
"

# =================================================================================================
# ALLOW-LIST — `family|path-prefix|line-substring|reason`
#   family `*`         = every family
#   EMPTY line-substring = PATH-ONLY
#
# Path-only is deliberate for `.unblock/issues.jsonl`: it is the GENERATED tracker export, rewritten
# wholesale by every `sync export`, so keying it on line text would break on a routine re-export — and
# the export necessarily quotes this very defect's own issue text. Nothing else is path-only.
# =================================================================================================
ALLOW="
*|.unblock/issues.jsonl||the GENERATED tracker export (D5 model B), rewritten wholesale by \`sync export\`
"

# =================================================================================================
# REQUIRED LANDINGS — `code@path@regex@what it proves`
#
# Every row is a NAMED predicate over a NAMED file.
#
# THE OBLIGATIONS THIS TABLE CARRIES, all learned the hard way:
#
# 1. THE FIX ITSELF MUST BE REACHED. R1..R21 are almost all documentation. A table that never names
#    the two files the behaviour change lands in goes fully GREEN on a tree where only prose was
#    rewritten — the cascade would certify itself. R22 (`unblock-engine`, the source-less carrier) and
#    R23/R24 (`unblock-storage`, the restored guards) are the code-side teeth. Every one of them was
#    checked to have ZERO matches before the change, so none can pass vacuously.
#
# 2. EVERY REMOVAL MUST PIN ITS REPLACEMENT (the header rule, applied to crates/** too). Two files in
#    this cascade lose text, and DELETING each file clears all of its forbidden hits at once:
#      * `crates/unblock-mcp/tests/dep_metadata.rs` — deleting it removes D44's ONLY end-to-end
#        JSON-RPC coverage. R25/R26 pin the two assertions the rewritten test must carry.
#      * `crates/unblock-sync/tests/contract.rs` — its only hits are ESCAPABLE families, so inserting
#        the token `D44` clears them with the sentence still false. R27 pins the affirmative fact.
#
# 3. ONE LANDING IS KEYED TO A FILE, NOT TO A SPELLING. `crates/unblock-engine/tests/create_bulk.rs`
#    carries the superseded D22 clause several times over, including one occurrence split across a line
#    break (the subject on one line, the predicate on the next). R29 requires that FILE to name `D44`.
#    It had ZERO matches before the change.
# =================================================================================================
REQUIRE="
R1@docs/PRD.md@^\| \*\*D44\*\* \|@PRD §4 carries the D44 decision row
R2@docs/PRD.md@\*\*FR-1a \[must\].*D44@FR-1a's create AC names D44 (the deps round-trip on the MINTING path)
R3@docs/PRD.md@\*\*FR-5 \[must\].*D44@FR-5's dependency AC names D44 (a declared edge can no longer be dropped)
R4@docs/PRD.md@CLOSED by D44@FR-20 carve-out (a) is retired IN PLACE, in the shape item (h) used for D43
R5@docs/PRD.md@\*\*D22\*\*.*D44@the D22 row carries the reciprocal SUPERSEDED-by-D44 pointer
R6@docs/PRD.md@\*\*D42\*\*.*D44@the D42 row carries the reciprocal pointer on its BOUND clause
R7@docs/plans/01-design-spine.md@D44@the interface SSOT records D44
R8@docs/plans/01-design-spine.md@struct DepInput@the spine DEFINES DepInput's shape (pre-D44 it only REFERENCED it, at :1734)
R9@docs/plans/implementation-plan.md@D44@the task DAG records D44
R10@docs/plans/00-roadmap.md@D44@the markdown roadmap NAMES D44 somewhere (that is all this predicate proves; the LOCATED roadmap pin is R11, and no row here claims to pin the v1.0.1 slot itself)
R11@docs/plans/00-roadmap.md@^\| .unblock-engine. \| ● \| ●@the §9 crate table marks unblock-engine worked in v1.0.1
R12@docs/roadmap.html@D44@the RENDERED roadmap lists D44 in its v1.0.1 card (outside the doc-lint corpus)
R13@docs/plans/crates/unblock-engine.md@D44@the engine crate plan records the repaired create contract
R14@docs/plans/crates/unblock-storage.md@D44@the storage crate plan records the seeded-edge/anchoring rule
R15@docs/plans/crates/unblock-mcp.md@D44@the mcp crate plan documents the create-arm deps semantics
R16@CLAUDE.md@$RANGE_RE@D-range bump site 1 of 3 (the LIVE range — knob: RANGE_RE). File-level is sound here: this file carries exactly ONE occurrence, the bump site itself
R18@xtask/src/doc_lint.rs@$RANGE_RE@D-range bump site 3 of 3 (the LIVE range — knob: RANGE_RE). File-level is sound here for the same reason; site 2 is ROW-ANCHORED at RW1 below, because ci-cd also describes this pin in prose
R19@crates/unblock-storage/src/trait_def.rs@Issue\.dependencies@the Storage trait doc states create_issue persists the seeded edges
R20@docs/plans/ci-cd-and-distribution.md@d44-create-deps-claims@this gate is SPECIFIED, not merely wired
R21@.github/workflows/ci.yml@d44-create-deps-claims@this gate actually RUNS in the required doc-lint job
R22@crates/unblock-engine/src/session/write.rs@NewDep@L5 carries the SOURCE-LESS create-edge type (zero matches pre-D44 — the engine change cannot be skipped)
R23@crates/unblock-storage/src/libsql/crud.rs@DuplicateDependency@the RESTORED create-specific duplicate guard exists by CODE (zero matches pre-D44: the shared body dedups with a silent continue at crud.rs:145-147)
R24@crates/unblock-storage/src/libsql/crud.rs@D44@the create-specific guard block is marked in the file that hosts it (zero matches pre-D44; would_cycle_in_tx / CycleDetected alone would be VACUOUS here, apply_reparent already uses both)
R25@crates/unblock-mcp/tests/dep_metadata.rs@VALIDATION_FAILED@the end-to-end JSON-RPC test was REWRITTEN, not deleted: it now pins the rejection CODE (zero matches pre-D44, where the test asserted only is_error)
R26@crates/unblock-mcp/tests/dep_metadata.rs@\[.dependencies.\]@the same test asserts the CREATED issue hydrated edge set, the half that proves the edge landed on the MINTED id (zero matches pre-D44)
R27@crates/unblock-sync/tests/contract.rs@import leg (routes|enters).{0,60}create_issues@the import-leg pin names the ACTUAL entry point (import.rs:279 calls create_issues, NOT create_issue) instead of being cleared by a bare D44 token
R28@.unblock/issues.jsonl@ub-lp9\.25@the co-shipped dangling-blocker issue EXISTS in the committed tracker record: PRD/spine/roadmap all cite it as a 1.0.1 co-requisite (Miguel ruling), and a cited-but-nonexistent id is how a co-ship commitment evaporates
R29@crates/unblock-engine/tests/create_bulk.rs@D44@the bulk-create test file NAMES D44 — the SPELLING-INDEPENDENT pin (obligation 3 in the header above) over the one file where the retired D22 clause is dense and WRAPS across a line break; zero matches pre-D44, so it cannot pass vacuously
"

# =================================================================================================
# ROW-ANCHORED LANDINGS — `code@path@anchor@regex@what it proves`
#
# Semantics (the same predicate `REQUIRE_CONTRACT`'s row-anchored selector uses, generalised off the
# contract knob): at least ONE line in `path` must match `anchor`, and EVERY line matching `anchor` must
# ALSO match `regex`. A vanished anchor is a FAILURE, never a pass — an anchor that no longer exists
# proves nothing, and silently proving nothing is how a pin rots.
#
# This table exists because a file-level presence check is the WRONG predicate for a document that both
# CARRIES a normative literal and DESCRIBES the pin that reads it. See the RANGE_RE header above for the
# concrete miss it repairs.
# =================================================================================================
REQUIRE_ROW="
RW1@docs/plans/ci-cd-and-distribution.md@\*\*\(a\) D-id coherence\*\*@$RANGE_RE@D-range bump site 2 of 3, LOCATED on the class-(a) statement itself — the ONE place in that file allowed to quote the live range, so no explanatory prose can satisfy this pin
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
#   EXCLUSIVE    NO OTHER version literal may appear in the file at all, and the current one must.
#                For prose that makes PRESENT-TENSE claims to users and keeps no history: README.md
#                says "the contract id is X" twice, and a stale X there is a published lie.
#
#   <regex>      ROW-ANCHORED — at least one line matches the regex, and EVERY line that matches it
#                also names the current version. For a file whose DECLARING line sits among lines that
#                legitimately name older versions: `docs/plans/crates/unblock-mcp.md` records the bump
#                CHAIN inside its own CONTRACT_VERSION row, so only that row can be pinned, not the
#                file.
#
# WHY README.md AND THE CRATE PLAN ARE HERE — the reason this table was extended rather than left at
# its two code rows. Both files shipped this cascade still publishing `unblock.mcp.v1.6` while the code
# emitted `v1.7`, and THREE green gates missed it: this check pinned only the two code files; the
# doc-lint corpus is a fixed 19-file list that does not contain README.md at all
# (xtask/src/doc_lint.rs:31); and neither doc-lint nor knowledge-lint has any rule class that fires on
# a stale version literal. A version bump is a cascade like any other — every site that PUBLISHES the
# id needs a named predicate, or the sites nobody re-greps rot silently.
REQUIRE_CONTRACT="
RC1@crates/unblock-mcp/src/options.rs@@the CONTRACT_VERSION const moved (D44 changes DepInput's published shape); PRESENCE, because this const's doc-comment documents the whole retired bump chain
RC2@crates/unblock-mcp/tests/public_api.rs@@the second, independent CONTRACT_VERSION pin moved with it
RC3@README.md@EXCLUSIVE@the most user-facing document in the repo publishes the contract id twice (:197, :205) as a present-tense claim and keeps no bump history, so NO retired id may survive there
RC4@docs/plans/crates/unblock-mcp.md@^\| .pub const CONTRACT_VERSION@the OWNING crate plan's CONTRACT_VERSION row DECLARES the current id (the PROCESS.md §3 decision-change checklist names the owning crate plan explicitly); row-anchored, because that row also records the bump chain, and the file names v1.7 elsewhere — so a file-level presence check would pass on the stale row
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
    # This check's own regex literals are not claims about the product.
    case "$_path" in "$SELF") continue ;; esac
    if [ -n "$_sub" ]; then
      printf '%s' "$_text" | grep -q -i -E "$_sub" || continue
    fi
    if [ -n "$_esc" ] && printf '%s' "$_text" | grep -q -E "$_esc"; then continue; fi
    if allowed "$_code" "$_path" "$_text"; then continue; fi
    printf '%s\n' "$_path:$_line: retired create-with-deps framing survived ($_code)"
  done
}

# PORTABILITY, deliberate: every `$( … )` in this file substitutes a FUNCTION CALL, never an inline
# loop containing a `case`. macOS `/bin/sh` (bash 3.2) mis-parses a `case` arm's `)` inside `$( )` and
# silently produces garbage instead of failing — this script would then "pass" vacuously on a
# developer machine. The D43 sibling avoids it the same way (`findings="$(scan CLM1 …)"`).
scan_all() {
  printf '%s\n' "$FAMILIES" | while IFS='@' read -r code mode re esc; do
    [ -n "$code" ] || continue
    if [ "$mode" = plain ]; then
      emit_hits "$code" "$re" '' ''
    elif [ "$mode" = escape ]; then
      emit_hits "$code" "$re" "$esc" ''
    else
      echo "FAMILY-TABLE: [$code] unknown mode '$mode'"
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
  say "BLOCKED — the framing above survived the D44 cascade. Rewrite it, add the reciprocal D44 cross-ref, or add an allow-list entry with a reason."
  blocked=1
fi

# -------------------------------------------------------------------------------------------------
# REQUIRED LANDINGS — each row must match at least one line in its own file.
# -------------------------------------------------------------------------------------------------
check_landings() {
  printf '%s\n' "$REQUIRE" | while IFS='@' read -r code path re reason; do
    [ -n "$code" ] || continue
    if [ ! -f "$path" ]; then
      printf '%s\n' "$path: [$code] REQUIRED cascade target is missing from the tree ($reason)"
      continue
    fi
    git grep -q -I -E "$re" -- "$path" 2>/dev/null \
      || printf '%s\n' "$path: [$code] the D44 cascade never landed here — no line matches /$re/ ($reason)"
  done
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
# SELF-TEST 1 — every allow-list entry must still match a real line. An entry that matches nothing
# means the line it excused was reworded or deleted, and the exemption is now silently widening this
# check's blind spot.
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
# SELF-TEST 2 — the family table itself must be non-empty and well-formed. A table silently emptied by
# a bad edit would make every scan above a vacuous pass.
# -------------------------------------------------------------------------------------------------
fam_count="$(printf '%s\n' "$FAMILIES$SUBJECT_FAMILIES" | grep -c '^F[0-9]')"
if [ "$fam_count" -lt 16 ]; then
  say "BLOCKED — the forbidden-framing table has $fam_count families; it shipped with 16. A family was dropped."
  blocked=1
fi

# -------------------------------------------------------------------------------------------------
# SELF-TEST 3 — same guard for the contract-version table, which shipped with 4 rows. It is stated
# separately because that table was extended from 2 to 4 precisely BECAUSE two publishing sites had no
# predicate; silently dropping a row would restore the blind spot this check exists to close.
# -------------------------------------------------------------------------------------------------
rc_count="$(printf '%s\n' "$REQUIRE_CONTRACT" | grep -c '^RC[0-9]')"
if [ "$rc_count" -lt 4 ]; then
  say "BLOCKED — the contract-version table has $rc_count rows; it shipped with 4. A publishing site lost its pin."
  blocked=1
fi

# -------------------------------------------------------------------------------------------------
# SELF-TEST 4 — same guard for the ROW-ANCHORED table, which shipped with 1 row. Deleting that row is
# how the D-range pin on ci-cd would silently revert to the file-level check that passed VACUOUSLY.
# -------------------------------------------------------------------------------------------------
rw_count="$(printf '%s\n' "$REQUIRE_ROW" | grep -c '^RW[0-9]')"
if [ "$rw_count" -lt 1 ]; then
  say "BLOCKED — the row-anchored table has $rw_count rows; it shipped with 1. A LOCATED pin was downgraded or dropped."
  blocked=1
fi

[ "$blocked" = "0" ] || exit 1
say "OK — no forbidden-framing family matched outside the allow-list, every required cascade site landed, and every allow-list entry still matches."
exit 0
