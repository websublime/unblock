#!/bin/sh
# d46-schema-migration-claims.sh — the REQUIRED-LANDING gate for D46, the `comments` forward
# migration (PRD §4 D46, tracked as `ub-lp9.13`; spec: docs/plans/ci-cd-and-distribution.md §2.1, the
# paragraph beginning "Named sub-check (its D46 sibling…)"). Runs as a step of the required `doc-lint`
# job, immediately after its `ub-lp9.25` sibling.
#
# THAT PARAGRAPH IS NORMATIVE OVER THIS FILE. Every landing enforced here is named there; a rule that
# exists in one and not the other is a defect to be fixed in the SAME change. Do not "tidy" a row away
# as unspecified — read the spec paragraph first.
#
# WHY THIS ONE IS SMALL, AND WHY IT IS POSITIVE-ONLY
# --------------------------------------------------
# Most of D46's teeth are not a shell gate at all: the class guard is a `const`-evaluated digest over
# the embedded DDL + the schema-version constants + the migration ladder, asserted against a
# hand-blessed literal, so editing the frozen baseline without a version bump or a forward step is a
# COMPILE error under every job that builds — no annotation, `cfg` or `#[ignore]` can silence it.
#
# What a compile-time assertion cannot carry is ITS OWN SURVIVAL. Nobody can write the negative
# question ("did anyone edit the DDL?"), and deleting the assertions themselves leaves a fully green
# tree. So this gate asserts the POSITIVE: the assertions are still there, and so is every landing
# whose absence would make them vacuous. There are deliberately NO forbidden-framing families and
# therefore no allow-list — D46 retires no wording, it adds a mechanism (contrast the D43/D44/D45
# siblings, which each retire a claim and carry a negative sweep plus its allow-list self-test).
#
# SEQUENCING, the same discipline both siblings state. This script and its workflow step belong to the
# IMPLEMENTATION commit, because every landing below asserts against code that does not exist until
# then; a spec-only commit that shipped it would turn the required `doc-lint` job red against its own
# tree. The D-range knob is the inverse coupling: it guards PROSE, which the spec commit already
# moved, so it is LIVE from this file's first commit.
#
# ONE PRECISION vs the spec paragraph's wording, stated rather than left as a silent divergence: it
# says "the storage migrations module still contains both assertions". The LADDER-CONTIGUITY assertion
# lives in `migrations.rs`; the CONTENT-DIGEST assertion lives beside the DDL it hashes, in
# `schema.rs`. Each row below pins the file the assertion is actually in, and the spec paragraph is
# corrected to name both files in this same commit.
#
# TWO RULE KINDS (both positive — see above)
#   P-n   REQUIRED landing — a presence predicate over a NAMED file. A landing that can be satisfied by
#         a doc comment is NOT a P row; it is a Q row.
#   Q-n   ROW-ANCHORED landing — at least ONE line must match the anchor, and EVERY line matching the
#         anchor must ALSO match the requirement. A vanished anchor is a FAILURE, never a pass: an
#         anchor that no longer exists proves nothing, and silently proving nothing is how a pin rots.
#         Used wherever a bare token would also match the file's own prose about the thing.
#
# Exit: 0 = pass · 1 = BLOCK (a required landing is missing) · 2 = cannot evaluate (fail-closed).
set -u

# PORTABILITY (2 of 2): every variable expansion goes through `printf`, never `echo`. POSIX-mode `echo`
# interprets backslash escapes, so a `\b` inside a regex literal becomes a BACKSPACE byte and the
# pattern silently stops matching. Not a style choice.
say() { printf 'd46-claims: %s\n' "$*" >&2; }

git rev-parse --show-toplevel >/dev/null 2>&1 || { say "not a git repository"; exit 2; }
cd "$(git rev-parse --show-toplevel)" || { say "cannot cd to the repo root"; exit 2; }

# The LIVE D-id range, in its TWO spellings. It tracks the LIVE range, never a frozen historical one:
# the day a D47 is minted, every file `docs/PROCESS.md` §3 enumerates moves with it or a required step
# goes red. §3 deliberately states that cascade as a LIST WITH NO COUNT — a derived count rotted there
# five times — and the Q rows below are what make the list self-checking.
#
# WHY TWO SPELLINGS. `xtask/src/doc_lint.rs`'s bump site is ONE physical line carrying BOTH halves: the
# prose range `(D1..D47)` and the tokenizer's regex ALTERNATION `\bD(47|46|45|…)\b`. Pinning only the
# prose is exactly how that site rots into an undefined-D47 finding — the lint would stop tokenizing
# the id it is being told exists.
RANGE_RE='D1\.\.D47'
RANGE_ALT_RE='D\(47\|46\|'

# The SAME two spellings as they appear INSIDE a sibling script's knob line, where each backslash is a
# literal byte rather than a regex operator. DERIVED, never hand-written a second time: a second copy of
# the range in this file would be a second thing to bump, which is the rot this whole clause exists to
# stop. `printf '%s'` never interprets its ARGUMENT, so the value passes through untouched; `sed` then
# turns each `\` into `\\\`, i.e. ERE for "a literal backslash followed by the escaped char".
knob_re() { printf '%s' "$1" | sed 's/\\/\\\\\\/g'; }
RANGE_KNOB_RE="$(knob_re "$RANGE_RE")"
RANGE_KNOB_ALT_RE="$(knob_re "$RANGE_ALT_RE")"

# =================================================================================================
# REQUIRED LANDINGS — `code@path@regex@what it proves`
#
# Every row had ZERO matches before the D46 implementation commit, so none can pass vacuously.
#
# P1/P2 are THE POINT OF THIS FILE: the two `const` assertions whose own survival nothing else can
#       guard. Delete either and the tree stays green — the digest stops guarding the frozen baseline,
#       or the ladder stops having to cover its own version range — with no test, lint or build to say so.
# P3    is the third `const` assertion, added by the 2026-08-03 Verify-gate ruling on the sentinel's
#       subject: it binds `witness_newest_step`'s column set to the ladder's NEWEST step, which is what
#       makes the `Storage::migrate` contract's "the NEWEST step's own columns" TRUE AS FACT rather
#       than as prose that a step 3 would quietly falsify.
# P4/P5 are the two version constants. Their VALUES are pinned, not merely their names: a revert to
#       `= 1`, or the deletion of the baseline constant that stops the literal `1` carrying two
#       unrelated meanings, is exactly the regression this decision exists to prevent.
# P6/P7 are the CONSUMPTION half of clause (10)'s pre-open stamp — a DURABLE-IDENTIFIER landing, spelled
#       the same in both files. The negative form ("nobody re-sourced `schema_from` from
#       `MigrateOutcome`") is the unwritable kind; the two-file co-occurrence is what proves the value
#       is both CAPTURED (config, L4) and CONSUMED (cli, L7) rather than captured and dropped.
# P8    is the tracker record. Satisfy it by updating the issue over the issue tool and re-exporting in
#       the same commit — NEVER by hand-editing the generated file (D5 model B).
# P9/P10 pin the STEP's own column set as TUPLE LITERALS, which is the POSITIVE form of "the frozen DDL
#       no longer carries them" (that absence is unwritable as a check; the step naming them is what
#       makes it observable). A bare `updated_at` token would be satisfied by any of this file's
#       several doc comments naming the columns — including one describing a step that had been
#       deleted — which is the vacuous-pass shape this project has watched rot twice. The tuple spelling
#       cannot appear in prose, and a half-deleted step fails exactly one of the two rows.
# =================================================================================================
REQUIRE="
P1@crates/unblock-storage/src/libsql/schema.rs@SCHEMA_CONTENT_DIGEST == BLESSED_SCHEMA_CONTENT_DIGEST@the CONTENT-PIN const assertion still exists: a DDL edit that ships without a version bump or a forward step is a red BUILD. Nothing else in the tree can notice its deletion
P2@crates/unblock-storage/src/libsql/migrations.rs@expected - 1 == CURRENT_SCHEMA_VERSION@the LADDER-CONTIGUITY const assertion still exists: bumping the version without a step, or adding a step without bumping, is a red BUILD
P3@crates/unblock-storage/src/libsql/migrations.rs@newest\.kind\.discriminant\(\) == MigrationKind::CommentsColumnsReconcile@the SENTINEL-SUBJECT const assertion still exists: the ladder's newest step IS the one the sentinel witnesses, so the migrate contract's promise cannot silently fall behind the ladder (Verify gate, 2026-08-03)
P4@crates/unblock-storage/src/libsql/schema.rs@CURRENT_SCHEMA_VERSION: i32 = 2@the version constant carries the D46 BUMP, by value — a revert to 1 fails this row rather than merely a test
P5@crates/unblock-storage/src/libsql/schema.rs@BASELINE_SCHEMA_VERSION: i32 = 1@the NEW baseline constant exists, by value: it is what stops the literal 1 carrying two unrelated meanings under the frozen-baseline discipline
P6@crates/unblock-config/src/context.rs@schema_version_before_migrate@clause (10)'s pre-open stamp is CAPTURED at the one shared open body (L4), between open_local and the facade's own migrate()
P7@crates/unblock-cli/src/commands/migrate.rs@schema_version_before_migrate@clause (10)'s pre-open stamp is CONSUMED by the command that reports it (L7) — the half that proves it is not captured and dropped
P8@.unblock/issues.jsonl@D46@the tracker record names the decision this work implements (PROCESS.md §6: re-export in the SAME commit as the work)
P9@crates/unblock-storage/src/libsql/migrations.rs@\(\"updated_at\", \"DATETIME\"\)@the ladder's step declares the FIRST post-baseline column as a TYPED STEP COLUMN
P10@crates/unblock-storage/src/libsql/migrations.rs@\(\"redacted_at\", \"DATETIME\"\)@…and the SECOND. Two rows, not one: a half-deleted step must fail on exactly one of them
"

# =================================================================================================
# ROW-ANCHORED LANDINGS — `code@path@anchor@regex@what it proves`
#
# Q1..Q4 are the PROSE D-range bump sites, all anchored on their own normative line. A file-level token
#       check proves the literal is SOMEWHERE in the file, so a document that ALSO discusses the range
#       in explanatory prose passes with its NORMATIVE statement still carrying the retired literal —
#       a defect D45 actually hit on ci-cd-and-distribution.md.
# Q5..Q7 are the SIBLING SCRIPTS' live-range knobs, and they are what makes `docs/PROCESS.md` §3's
#       count-free ENUMERATION self-checking: every file that list names is pinned against the range
#       this script holds, so a bump that skips one file, or a list that omits one, goes red instead of
#       rotting silently. THIS script's own knob has NO row, deliberately and not by oversight: it is
#       the REFERENCE the other rows are compared against, so a self-row could never fail. The cover is
#       complete either way — bump this knob alone and Q1..Q7 go red; bump everything but this knob and
#       Q1..Q7 go red too. Do not "restore" a self-row; it would be vacuous by construction.
# =================================================================================================
REQUIRE_ROW="
Q1@CLAUDE.md@^\| .docs/PRD\.md. \| Product truth@$RANGE_RE@PROSE D-range bump site, LOCATED on the document-map row that states the range
Q2@docs/plans/ci-cd-and-distribution.md@\*\*\(a\) D-id coherence\*\*@$RANGE_RE@PROSE D-range bump site, LOCATED on the class-(a) statement — the ONE place in that file allowed to quote the live range
Q3@xtask/src/doc_lint.rs@Spec tokenizes@$RANGE_RE@PROSE D-range bump site, half 1 of 2: the PROSE range on the tokenizer comment line
Q4@xtask/src/doc_lint.rs@Spec tokenizes@$RANGE_ALT_RE@…half 2 of 2: the TOKENIZER ALTERNATION on that same line
Q5@scripts/checks/d44-create-deps-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D44 sibling's live-range knob carries the SAME range as this script — anchored on the knob line, so the file's own prose about the knob cannot satisfy the pin
Q6@scripts/checks/ub-lp9.25-dangling-blocker-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D45 sibling's live-range knob, same anchoring and same reason
Q7@scripts/checks/ub-lp9.25-dangling-blocker-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, which is a separate line and therefore a separate way to be half-bumped
"

blocked=0

# PORTABILITY (1 of 2), deliberate: every `$( … )` in this file substitutes a FUNCTION CALL, never an
# inline loop containing a `case`. macOS `/bin/sh` (bash 3.2) mis-parses a `case` arm's `)` inside
# `$( )` and silently produces garbage instead of failing — this script would then "pass" vacuously on
# a developer machine. Both siblings avoid it the same way.
check_landings() {
  printf '%s\n' "$REQUIRE" | while IFS='@' read -r code path re reason; do
    [ -n "$code" ] || continue
    if [ ! -f "$path" ]; then
      printf '%s\n' "$path: [$code] REQUIRED D46 target is missing from the tree ($reason)"
      continue
    fi
    git grep -q -I -E "$re" -- "$path" 2>/dev/null \
      || printf '%s\n' "$path: [$code] the D46 landing is GONE — no line matches /$re/ ($reason)"
  done
  printf '%s\n' "$REQUIRE_ROW" | while IFS='@' read -r code path anchor re reason; do
    [ -n "$code" ] || continue
    if [ ! -f "$path" ]; then
      printf '%s\n' "$path: [$code] REQUIRED D46 target is missing from the tree ($reason)"
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
  say "BLOCKED — the D46 mechanism is incomplete; the sites above are missing. A deleted const assertion is the case this gate exists for: nothing else in the tree goes red for it."
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

check_table_floor 'required-landing (P)' "$p_count" 10 || blocked=1
check_table_floor 'row-anchored (Q)' "$q_count" 7 || blocked=1

[ "$blocked" = "0" ] || exit 1
say "OK — both D46 compile-time assertions and the sentinel-subject binding are still in the tree, the two schema-version constants carry their D46 values, the ladder's step still names both post-baseline columns, the pre-open stamp is captured AND consumed, the tracker record names the decision, and the live D-range is current at every prose site and every sibling script knob the PROCESS.md §3 list enumerates."
exit 0
