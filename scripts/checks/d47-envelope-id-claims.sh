#!/bin/sh
# d47-envelope-id-claims.sh — the REQUIRED-LANDING gate for D47, the UN-DECODABLE ENVELOPE `id` class
# (PRD §4 D47, tracked as `ub-cnv`; spec: docs/plans/ci-cd-and-distribution.md §2.1, the paragraph
# beginning "Named sub-check (its D47 sibling…)"). Runs as a step of the required `doc-lint` job,
# immediately after its `d46` sibling.
#
# THAT PARAGRAPH IS NORMATIVE OVER THIS FILE. Every landing enforced here is named there; a rule that
# exists in one and not the other is a defect to be fixed in the SAME change. Do not "tidy" a row away
# as unspecified — read the spec paragraph first.
#
# WHY THIS ONE IS POSITIVE-ONLY
# -----------------------------
# D47 retires framing in several files, and the tempting gate is a NEGATIVE sweep for the retired
# wording. That is the same defect in disguise, and this project has already paid for it: a claim that
# gets REWRAPPED across two lines becomes unfindable in principle, so the sweep goes green while the
# false sentence is still in the tree. Every row here is therefore a spelling-INDEPENDENT POSITIVE
# landing — the corrected text, or the code, must be PRESENT — and no row enumerates a spelling that a
# reformatting could dodge.
#
# The other half of the reason is the shape of what D47 adds. Its teeth are executable: the transport
# arm, the predicate module, and roughly forty cells. What NO test can carry is ITS OWN SURVIVAL, and
# neither can a rendered document that sits outside every lint corpus. So this gate asserts the
# POSITIVE: the mechanism is still there, the DOC cascade landed, and the one published artefact that
# nothing else in CI can see (`docs/roadmap.html`) really names the decision.
#
# SEQUENCING, the same discipline all three siblings state. This script and its workflow step belong to
# the IMPLEMENTATION commit, because the code rows below assert against files that do not exist until
# then; a spec-only commit that shipped it would turn the required `doc-lint` job red against its own
# tree. The D-range knob is the inverse coupling: it guards PROSE, which the spec commit already moved,
# so it is LIVE from this file's first commit.
#
# THE CONTRACT KNOB IS PINNED BUT DOES NOT MOVE HERE. D47 mints no `ErrorCode` and bumps no contract:
# `-32600` is `rmcp::model::ErrorCode(i32)`, a transport-layer type in another crate. `unblock.mcp.v1.9`
# stands. The knob tracks the IMPLEMENTATION commit that changes the code constant — there is no such
# commit in this work, which is exactly why the row exists: an unstated "we didn't bump" is
# indistinguishable from an oversight.
#
# TWO RULE KINDS (both positive — see above)
#   P-n   REQUIRED landing — a presence predicate over a NAMED file. A landing that can be satisfied by
#         a doc comment is NOT a P row; it is a Q row.
#   Q-n   ROW-ANCHORED landing — at least ONE line must match the anchor, and EVERY line matching the
#         anchor must ALSO match the requirement. A vanished anchor is a FAILURE, never a pass: an
#         anchor that no longer exists proves nothing, and silently proving nothing is how a pin rots.
#         Used wherever a bare token would also match the file's own prose about the thing.
#
# THE P TABLE HAS A GAP AT P4/P5, DELIBERATELY. Both shipped as P rows and both were SATISFIABLE BY A
# DOC COMMENT — the decoded-key rule's token also matches two comments in `envelope_id.rs`, and the
# recovered-arm's `Some(id)` also matches a module doc line and a test body in `wire.rs` — so each
# stayed GREEN under the very mutant its own reason text named. They are RE-ANCHORED as Q11/Q12 on
# their PRODUCTION lines (the D46 sibling's remedy for exactly this shape). The codes are NOT reused
# and the survivors are NOT renumbered: a renumbering makes every prior reference to "P6" silently
# mean something else, which is a worse failure than a documented gap.
#
# Exit: 0 = pass · 1 = BLOCK (a required landing is missing) · 2 = cannot evaluate (fail-closed).
set -u

# PORTABILITY (2 of 2): every variable expansion goes through `printf`, never `echo`. POSIX-mode `echo`
# interprets backslash escapes, so a `\b` inside a regex literal becomes a BACKSPACE byte and the
# pattern silently stops matching. Not a style choice.
say() { printf 'd47-claims: %s\n' "$*" >&2; }

git rev-parse --show-toplevel >/dev/null 2>&1 || { say "not a git repository"; exit 2; }
cd "$(git rev-parse --show-toplevel)" || { say "cannot cd to the repo root"; exit 2; }

# The LIVE D-id range, in its TWO spellings. It tracks the LIVE range, never a frozen historical one:
# the day a D48 is minted, every file `docs/PROCESS.md` §3 enumerates moves with it or a required step
# goes red. §3 deliberately states that cascade as a LIST WITH NO COUNT — a derived count rotted there
# five times — and the Q rows below are what make the list self-checking.
#
# WHY TWO SPELLINGS. `xtask/src/doc_lint.rs`'s bump site is ONE physical line carrying BOTH halves: the
# prose range `(D1..D48)` and the tokenizer's regex ALTERNATION `\bD(48|47|46|…)\b`. Pinning only the
# prose is exactly how that site rots into an undefined-D48 finding — the lint would stop tokenizing
# the id it is being told exists.
RANGE_RE='D1\.\.D48'
RANGE_ALT_RE='D\(48\|47\|'

# The LIVE published contract version. D47 does NOT move it (see the header): this row is the
# affirmative record of that, so a silent bump riding this decision goes red.
CONTRACT_RE='unblock\.mcp\.v1\.9'

# The SAME two spellings as they appear INSIDE a sibling script's knob line, where each backslash is a
# literal byte rather than a regex operator. DERIVED, never hand-written a second time: a second copy of
# the range in this file would be a second thing to bump, which is the rot this whole clause exists to
# stop. `printf '%s'` never interprets its ARGUMENT, so the value passes through untouched; `sed` then
# turns each `\` into `\\\`, i.e. ERE for "a literal backslash followed by the escaped char".
knob_re() { printf '%s' "$1" | sed 's/\\/\\\\\\/g'; }
RANGE_KNOB_RE="$(knob_re "$RANGE_RE")"
RANGE_KNOB_ALT_RE="$(knob_re "$RANGE_ALT_RE")"

# Q12's two halves, held in variables for TWO independent reasons, both of which bite silently.
#   1. An END-ANCHOR cannot be written inline in the tables below: those are DOUBLE-QUOTED strings
#      whose field separator is `@`, so a regex ending in `$` puts `$@` in the source — the shell
#      expands that to the positional parameters (empty) and eats the anchor. The row then matches a
#      mangled pattern, and the failure looks like a missing landing rather than a quoting bug.
#   2. The two halves are matched against DIFFERENT texts and cannot be one variable: the ANCHOR runs
#      over the file's own lines, while the REQUIREMENT runs over `git grep -n` output, i.e.
#      `path:lineno:text` — so a `^` in a requirement can never match anything and would turn the row
#      into a permanent, meaningless BLOCK.
RECOVERED_ID_LINE_RE='^ +Some\(id\),$'
RECOVERED_ID_RE='Some\(id\),$'

# =================================================================================================
# REQUIRED LANDINGS — `code@path@regex@what it proves`
#
# Every row had ZERO matches before the D47 commits, so none can pass vacuously.
#
# P1/P2 are THE MECHANISM: the predicate module and the transport arm that consumes it. Either one
#       deleted leaves a tree whose docs describe a fix that is not there.
# P3    is the EXHAUSTIVE variant match, which is the entire mitigation for an rmcp bump adding a fifth
#       `JsonRpcMessage` variant. A `_` wildcard would compile, pass every cell, and silently reopen
#       the hole; this row is what notices.
# P4/P5 are GONE from this table on purpose — see the header. The DECODED-key rule is Q11 and the
#       recovered-id arm is Q12, both anchored on the production line the mutant edits.
# P6    is the SHARED corpus, gated `any(test, feature = ...)` so the in-lib cells cannot silently
#       compile away — this project's canonical vacuity failure.
# P7    is the store-EFFECT oracle, the only assertion that kills a "rebuild it as a Request and
#       deliver it" implementation.
# P8    is the DISCLOSED residual staying measured rather than prose: the -32700 arm still omits a
#       readable id. Closing it is a deliberate future change that turns that cell red.
# P9    is the tracker record. Satisfy it by updating the issue over the issue tool and re-exporting in
#       the same commit — NEVER by hand-editing the generated file (D5 model B).
# P10   is `ub-788`, the residual the PRD row NAMES. Without it the top document of the hierarchy
#       carries a DANGLING id, which is worse than the vagueness it replaced.
# P11   is the RENDERED roadmap. It sits OUTSIDE the 19-file doc-lint corpus, so this row is the only
#       thing in CI that can notice the published v1.0.1 card listing a fix set that is missing a fix.
# P12/P13 are this gate's own wiring: SPECIFIED in ci-cd, and actually RUNNING in the workflow. A
#       script that exists but is unwired fails on its own rows rather than passing silently.
# P14   is the count-free LIST in PROCESS.md §3 naming this script. No sibling has this row and nothing
#       else pins that line — which is precisely why the enumeration can rot silently, and this is the
#       first cascade that would notice.
# P15   is `ub-nbz`, the SECOND residual the PRD row names — the -32600 lost whenever rmcp cancels the
#       receive() future. It mirrors P10 for the same reason and is a SEPARATE row rather than a widened
#       one: a single row matching either id would go green with the other id dangling.
# =================================================================================================
REQUIRE="
P1@crates/unblock-mcp/src/envelope_id.rs@pub\(crate\) fn scan@the D47 predicate module still exists and still exposes its scan entry point
P2@crates/unblock-mcp/src/wire.rs@INVALID_REQUEST_ID_MESSAGE@the transport's -32600 arm is still wired to the compile-time reply constant
P3@crates/unblock-mcp/src/wire.rs@JsonRpcMessage::Response\(_\)@the variant match is still EXHAUSTIVE (no wildcard arm): a fifth rmcp variant that could carry a stray id must be a COMPILE error, not a silent hole
P6@crates/unblock-mcp/src/lib.rs@cfg\(any\(test, feature = \"test-util\"\)\)@the shared corpus is gated so the in-lib cells cannot silently compile away — a non-compiled cell is a vacuous pass
P7@crates/unblock-mcp/tests/envelope_id_duplex.rs@store_fingerprint@the store-EFFECT oracle survives: the one assertion that kills a rebuild-as-a-Request-and-deliver-it implementation
P8@crates/unblock-mcp/src/wire.rs@ub-788@the DISCLOSED -32700 residual is still named at the transport, so it stays tracked rather than quietly assumed closed
P9@.unblock/issues.jsonl@ub-cnv@the tracker record names the work this implements (PROCESS.md §6: re-export in the SAME commit as the work)
P10@.unblock/issues.jsonl@ub-788@the residual the PRD row names has a REAL issue, so the top document of the hierarchy carries no dangling id
P11@docs/roadmap.html@D47@the RENDERED roadmap lists D47 in its v1.0.1 card — it is OUTSIDE the 19-file doc-lint corpus, so nothing else in CI can catch its absence
P12@docs/plans/ci-cd-and-distribution.md@d47-envelope-id-claims@this gate is SPECIFIED, not merely wired
P13@.github/workflows/ci.yml@d47-envelope-id-claims@this gate actually RUNS in the required doc-lint job
P14@docs/PROCESS.md@d47-envelope-id-claims@the count-free LIST that IS the rule names this script, so the enumeration cannot rot silently
P15@.unblock/issues.jsonl@ub-nbz@the SECOND residual the PRD row names — the -32600 lost to a cancelled receive() — has a REAL issue, so that id does not dangle either
"

# =================================================================================================
# ROW-ANCHORED LANDINGS — `code@path@anchor@regex@what it proves`
#
# Q1..Q4 are the PROSE D-range bump sites, all anchored on their own normative line. A file-level token
#       check proves the literal is SOMEWHERE in the file, so a document that ALSO discusses the range
#       in explanatory prose passes with its NORMATIVE statement still carrying the retired literal —
#       a defect D45 actually hit on ci-cd-and-distribution.md.
# Q5..Q9 are the SIBLING SCRIPTS' live-range knobs, and they are what makes `docs/PROCESS.md` §3's
#       count-free ENUMERATION self-checking: every file that list names is pinned against the range
#       this script holds, so a bump that skips one file, or a list that omits one, goes red instead of
#       rotting silently. Following the precedent its D46 sibling states, the NEWEST script pins the
#       OLDER ones; THIS script's own knob has NO row, deliberately and not by oversight, because it is
#       the REFERENCE the other rows are compared against and a self-row could never fail. Do not
#       "restore" one; it would be vacuous by construction.
# Q10   is the contract knob, anchored on the constant's own definition line so this file's prose about
#       the version cannot satisfy it. It pins that D47 did NOT bump the contract.
# Q11   is the DECODED-key rule (ex-P4), anchored on the collector's own key loop. BOTH halves work
#       here: swap the key type for a raw-span one and the anchored line fails the requirement; delete
#       the loop and the anchor vanishes, which is also a failure. What NO positive row can catch is a
#       byte PREFILTER added in front of the seed (`M5b`) — added code leaves every landing intact —
#       and that mutant is graded by the cell `scan_decodes_keys` instead. Stated so this row is not
#       read as covering it.
# Q12   is the ANSWER-AND-DROP shape (ex-P5): the recovered arm must pass `Some(id)`. Passing `None`
#       there still answers, still recovers the connection, and still passes every cell that does not
#       correlate by id — while releasing no waiting peer at all, which is the whole point of D47.
#       The anchor is the argument's OWN production line (indentation + the value + a comma, whole
#       line), which no doc comment and no test body can spell; the named mutant rewrites that line to
#       `None,`, the anchor then matches NOTHING, and a vanished anchor is a failure. So on this row it
#       is the anchor half that kills the mutant and the requirement half that is nearly a restatement
#       — said plainly rather than dressed up, because a row whose reason overstates its own reach is
#       the exact defect this pair of rows was rewritten to remove.
# =================================================================================================
REQUIRE_ROW="
Q1@CLAUDE.md@^\| .docs/PRD\.md. \| Product truth@$RANGE_RE@PROSE D-range bump site, LOCATED on the document-map row that states the range
Q2@docs/plans/ci-cd-and-distribution.md@\*\*\(a\) D-id coherence\*\*@$RANGE_RE@PROSE D-range bump site, LOCATED on the class-(a) statement — the ONE place in that file allowed to quote the live range
Q3@xtask/src/doc_lint.rs@Spec tokenizes@$RANGE_RE@PROSE D-range bump site, half 1 of 2: the PROSE range on the tokenizer comment line
Q4@xtask/src/doc_lint.rs@Spec tokenizes@$RANGE_ALT_RE@…half 2 of 2: the TOKENIZER ALTERNATION on that same line
Q5@scripts/checks/d44-create-deps-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D44 sibling's live-range knob carries the SAME range as this script — anchored on the knob line, so the file's own prose about the knob cannot satisfy the pin
Q6@scripts/checks/ub-lp9.25-dangling-blocker-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D45 sibling's live-range knob, same anchoring and same reason
Q7@scripts/checks/ub-lp9.25-dangling-blocker-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, which is a separate line and therefore a separate way to be half-bumped
Q8@scripts/checks/d46-schema-migration-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D46 sibling's live-range knob, same anchoring and same reason
Q9@scripts/checks/d46-schema-migration-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and THAT sibling's ALTERNATION knob, the fifth separate way the enumeration can be half-bumped
Q10@crates/unblock-mcp/src/options.rs@^pub const CONTRACT_VERSION@$CONTRACT_RE@D47 mints no ErrorCode and bumps NO contract: -32600 is rmcp's transport-level code, not our taxonomy. An unstated 'we didn't bump' is indistinguishable from an oversight, so it is stated here
Q11@crates/unblock-mcp/src/envelope_id.rs@^ +while let Some\(key\) = map\.next_key@next_key::<String>\(\)@keys are still compared DECODED, never as raw spans — the escaped spelling IS a genuine envelope id. Anchored on the collector's key LOOP, so neither of this file's two comments about the rule can satisfy it
Q12@crates/unblock-mcp/src/wire.rs@$RECOVERED_ID_LINE_RE@$RECOVERED_ID_RE@the RECOVERED arm still answers ON the recovered id; passing None there answers nobody, since an rmcp client DROPS an id-less error. Anchored on the argument's own production line, so the module doc and the test body that also spell Some(id) cannot satisfy it
"

blocked=0

# PORTABILITY (1 of 2), deliberate: every `$( … )` in this file substitutes a FUNCTION CALL, never an
# inline loop containing a `case`. macOS `/bin/sh` (bash 3.2) mis-parses a `case` arm's `)` inside
# `$( )` and silently produces garbage instead of failing — this script would then "pass" vacuously on
# a developer machine. All three siblings avoid it the same way.
check_landings() {
  printf '%s\n' "$REQUIRE" | while IFS='@' read -r code path re reason; do
    [ -n "$code" ] || continue
    if [ ! -f "$path" ]; then
      printf '%s\n' "$path: [$code] REQUIRED D47 target is missing from the tree ($reason)"
      continue
    fi
    git grep -q -I -E "$re" -- "$path" 2>/dev/null \
      || printf '%s\n' "$path: [$code] the D47 landing is GONE — no line matches /$re/ ($reason)"
  done
  printf '%s\n' "$REQUIRE_ROW" | while IFS='@' read -r code path anchor re reason; do
    [ -n "$code" ] || continue
    if [ ! -f "$path" ]; then
      printf '%s\n' "$path: [$code] REQUIRED D47 target is missing from the tree ($reason)"
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
}

missing="$(check_landings)"
if [ -n "$missing" ]; then
  printf '%s\n' "$missing" >&2
  say "BLOCKED — the D47 mechanism or its cascade is incomplete; the sites above are missing. A deleted arm, a wildcard match arm, or a roadmap card missing its fix are the cases this gate exists for: nothing else in the tree goes red for them."
  blocked=1
fi

# -------------------------------------------------------------------------------------------------
# SELF-TEST — no rule table may have SHRUNK below the counts it shipped with. A table silently emptied
# by a bad edit would make every check above a vacuous pass, and the two tables are counted SEPARATELY
# so that adding a row to one can never mask the deletion of a row from the other.
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

p_count="$(count_rows "$REQUIRE" '^P[0-9]')"
q_count="$(count_rows "$REQUIRE_ROW" '^Q[0-9]')"

# The floors are 13 and 12. The P floor is still LOWER than the highest P code in use because P4/P5
# were RE-ANCHORED into the Q table (header) rather than dropped, and it rose by one when P15 pinned
# the second residual id the PRD row names. A floor may only move down together with the rows it counts
# moving somewhere the other floor counts them — never because a rule became inconvenient.
check_table_floor 'required-landing (P)' "$p_count" 13 || blocked=1
check_table_floor 'row-anchored (Q)' "$q_count" 12 || blocked=1

[ "$blocked" = "0" ] || exit 1
say "OK — the D47 predicate, its exhaustive-match guard, its decoded-key rule and its recovered-id arm are all still in the tree, the store-effect oracle and the disclosed -32700 residual are still pinned, the tracker names ub-cnv and both residual ids the PRD row cites (ub-788 and ub-nbz), the rendered roadmap lists the decision, this gate is both specified and wired, and the live D-range is current at every prose site and every sibling script knob the PROCESS.md §3 list enumerates while the contract version stands unmoved."
exit 0
