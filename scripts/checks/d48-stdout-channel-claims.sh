#!/bin/sh
# d48-stdout-channel-claims.sh — the REQUIRED-LANDING gate for D48, the PROTOCOL-CHANNEL carve-out
# (PRD §4 D48, tracked as `ub-og3`; spec: docs/plans/ci-cd-and-distribution.md §2.1, the paragraph
# beginning "Named sub-check (its D48 sibling…)"). Runs as a step of the required `doc-lint` job,
# immediately after its `d47` sibling.
#
# THAT PARAGRAPH IS NORMATIVE OVER THIS FILE. Every landing enforced here is named there; a rule that
# exists in one and not the other is a defect to be fixed in the SAME change. Do not "tidy" a row away
# as unspecified — read the spec paragraph first.
#
# WHY THIS ONE IS POSITIVE-ONLY
# -----------------------------
# D48 retires framing in several files — every "structured output strictly on stdout" sentence gains a
# carve-out — and the tempting gate is a NEGATIVE sweep for the retired wording. That is the same
# defect in disguise, and this project has already paid for it twice: a claim that gets REWRAPPED
# across two lines becomes unfindable in principle, so the sweep goes green while the false sentence is
# still in the tree. Every row here is therefore a spelling-INDEPENDENT POSITIVE landing — the
# corrected text, or the code, must be PRESENT — and no row enumerates a spelling a reformatting could
# dodge.
#
# The other half of the reason is the shape of what D48 adds. Its teeth are executable: the injected
# sink cells, the spawning suite, and the two INVERTED lifecycle cells. What NO test can carry is ITS
# OWN SURVIVAL, and three of this decision's obligations are unreachable from any test at all — the
# single-caller invariant its blast-radius argument rests on, the four residual ids the PRD row names,
# and a rendered document (`docs/roadmap.html`) that sits outside every lint corpus in this repo.
#
# SEQUENCING, the same discipline all four siblings state. This script and its workflow step belong to
# the IMPLEMENTATION commit, because the code rows below assert against files that do not exist until
# then; a spec-only commit that shipped it would turn the required `doc-lint` job red against its own
# tree. The D-range knob is the inverse coupling: it guards PROSE, which the spec commit already moved,
# so it is LIVE from this file's first commit.
#
# THE CONTRACT KNOB IS PINNED BUT DOES NOT MOVE HERE. D48 mints no `ErrorCode` (`ErrorCode::ALL` stays
# 36) and moves no published byte: it changes which STREAM an already-formed document is written to.
# `unblock.mcp.v1.9` stands. The knob tracks the IMPLEMENTATION commit that changes the code constant —
# there is no such commit in this work, which is exactly why the row exists: an unstated "we didn't
# bump" is indistinguishable from an oversight.
#
# TWO RULE KINDS (both positive — see above)
#   P-n   REQUIRED landing — a presence predicate over a NAMED file. A landing that can be satisfied by
#         a doc comment is NOT a P row; it is a Q row.
#   Q-n   ROW-ANCHORED landing — at least ONE line must match the anchor, and EVERY line matching the
#         anchor must ALSO match the requirement. A vanished anchor is a FAILURE, never a pass: an
#         anchor that no longer exists proves nothing, and silently proving nothing is how a pin rots.
#         Used wherever a bare token would also match the file's own prose about the thing.
#
# AND TWO STRUCTURAL CHECKS THAT NO TABLE ROW CAN EXPRESS (S1/S2 below), because both are about a
# RELATION between two places rather than the presence of one string.
#
# Exit: 0 = pass · 1 = BLOCK (a required landing is missing) · 2 = cannot evaluate (fail-closed).
set -u

# PORTABILITY (2 of 2): every variable expansion goes through `printf`, never `echo`. POSIX-mode `echo`
# interprets backslash escapes, so a `\b` inside a regex literal becomes a BACKSPACE byte and the
# pattern silently stops matching. Not a style choice.
say() { printf 'd48-claims: %s\n' "$*" >&2; }

git rev-parse --show-toplevel >/dev/null 2>&1 || { say "not a git repository"; exit 2; }
cd "$(git rev-parse --show-toplevel)" || { say "cannot cd to the repo root"; exit 2; }

# The LIVE D-id range, in its TWO spellings. It tracks the LIVE range, never a frozen historical one:
# the day a D49 is minted, every file `docs/PROCESS.md` §3 enumerates moves with it or a required step
# goes red. §3 deliberately states that cascade as a LIST WITH NO COUNT — a derived count rotted there
# five times — and the Q rows below are what make the list self-checking.
#
# WHY TWO SPELLINGS. `xtask/src/doc_lint.rs`'s bump site is ONE physical line carrying BOTH halves: the
# prose range `(D1..D48)` and the tokenizer's regex ALTERNATION `\bD(48|47|46|…)\b`. Pinning only the
# prose is exactly how that site rots into an undefined-D49 finding — the lint would stop tokenizing
# the id it is being told exists.
RANGE_RE='D1\.\.D48'
RANGE_ALT_RE='D\(48\|47\|'

# The LIVE published contract version. D48 does NOT move it (see the header): this row is the
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

# =================================================================================================
# REQUIRED LANDINGS — `code@path@regex@what it proves`
#
# Every row had ZERO matches before the D48 commits, so none can pass vacuously.
#
# P1/P2 and Q15 are THE MECHANISM, and they are THREE rows rather than one because the classification
#       can be broken in three independent places: the TYPE can lose a variant (collapsing the
#       carve-out), the CLASSIFIER can vanish from `cli.rs`, or the classification can be produced and
#       then DROPPED instead of threaded into the boundary. P2 and Q15 are the two-file co-occurrence:
#       `stdout_role` must appear in `cli.rs` (produced) AND in `lib.rs` (consumed), because either
#       half alone is a fix that does nothing. The CONSUMER half is a Q row and not a P one — see
#       Q15 below for the measurement that moved it.
# P4   is the SINK-INJECTED core. Without it the stream choice is unobservable in-process and the whole
#       unit layer of clause (7) collapses back to the spawning cells it was written to complement.
# P5   is the spawning regression FILE. Its cells are what cover the one thing no unit cell can reach:
#       the classification's call site and the wrapper's binding of the two real streams.
# P6/P7 are the HARNESS hardening. The framing predicate is what stops a blob that merely PARSES as JSON
#       from passing the stdout guard — the exact hole the defect lived in — and the env scrub is what
#       stops a host shell making every frame-only assertion vacuous.
# P8   is the tracker record. Satisfy it by updating the issue over the issue tool and re-exporting in
#       the same commit — NEVER by hand-editing the generated file (D5 model B).
# P9..P12 are the FOUR residuals the PRD row NAMES and deliberately leaves OPEN. One row EACH, so a
#       failure says WHICH id vanished: a single row matching any of them would go green with three
#       dangling. Without them the top document of the hierarchy carries dangling ids, which is worse
#       than the vagueness they replaced.
# P13  is the RENDERED roadmap. It sits OUTSIDE the 19-file doc-lint corpus, so this row is the only
#       thing in CI that can notice the published v1.0.1 card listing a fix set that is missing a fix.
# P14/P15 are this gate's own wiring: SPECIFIED in ci-cd, and actually RUNNING in the workflow. A
#       script that exists but is unwired fails on its own rows rather than passing silently.
# P16  is the count-free LIST in PROCESS.md §3 naming this script — the enumeration that makes the
#       whole D-range cascade self-checking, and which can otherwise rot silently.
#
# THERE IS NO `P3`, DELIBERATELY. It became `Q15` (see the Q table) and the remaining rows were NOT
# renumbered: a row code is how a failure names itself, so shifting every row that follows it to close one gap
# would silently repoint every reference to this table. A gap costs one comment; a renumber costs a
# reader who trusts an old citation.
# =================================================================================================
REQUIRE="
P1@crates/unblock-cli/src/exit.rs@enum StdoutRole@the two-valued classification TYPE still exists — a bool, or a collapse to one variant, is what this row notices
P2@crates/unblock-cli/src/cli.rs@fn stdout_role@the classifier over the PARSED command still exists where the subcommand enum lives
P4@crates/unblock-cli/src/exit.rs@fn into_exit_to@the sink-injected core survives: without it the stream CHOICE is unobservable in-process
P5@crates/unblock-cli/tests/mcp_stdout_channel.rs@assert_diagnostic_on_stderr@the spawning regression file still drives its shared oracle — the layer that covers the call site no unit cell can see
P6@crates/unblock-cli/tests/common/mod.rs@fn is_jsonrpc_framing@the hardened stdout guard still requires FRAMING, not merely valid JSON — the blob IS valid JSON, which is how it passed for the life of the suite
P7@crates/unblock-cli/tests/common/mod.rs@env_remove\(\"UNBLOCK_OUTPUT_FORMAT\"\)@the format env is still scrubbed at the ONE spawn root: inherited, it makes every frame-only assertion vacuous
P8@.unblock/issues.jsonl@ub-og3@the tracker record names the work this implements (PROCESS.md §6: re-export in the SAME commit as the work)
P9@.unblock/issues.jsonl@ub-kp7@residual 1 of 4: a first frame that is neither initialize NOR ping still kills the server — OPEN, and the PRD row names it
P10@.unblock/issues.jsonl@ub-b1a@residual 2 of 4: the relocated message still embeds an unbounded Debug rendering of attacker-controlled bytes — OPEN
P11@.unblock/issues.jsonl@ub-c5o@residual 3 of 4: output::emit_report still writes to stdout unconditionally with no classification — OPEN
P12@.unblock/issues.jsonl@ub-5v5@residual 4 of 4: an oversized response could leave a TRUNCATED frame on the same channel — reasoned from source, never reproduced, OPEN
P13@docs/roadmap.html@D48@the RENDERED roadmap lists D48 in its v1.0.1 card — it is OUTSIDE the 19-file doc-lint corpus, so nothing else in CI can catch its absence
P14@docs/plans/ci-cd-and-distribution.md@d48-stdout-channel-claims@this gate is SPECIFIED, not merely wired
P15@.github/workflows/ci.yml@d48-stdout-channel-claims@this gate actually RUNS in the required doc-lint job
P16@docs/PROCESS.md@d48-stdout-channel-claims@the count-free LIST that IS the rule names this script, so the enumeration cannot rot silently
"

# =================================================================================================
# ROW-ANCHORED LANDINGS — `code@path@anchor@regex@what it proves`
#
# Q1..Q4 are the PROSE D-range bump sites, all anchored on their own normative line. A file-level token
#       check proves the literal is SOMEWHERE in the file, so a document that ALSO discusses the range
#       in explanatory prose passes with its NORMATIVE statement still carrying the retired literal —
#       a defect D45 actually hit on ci-cd-and-distribution.md.
# Q5..Q11 are the SIBLING SCRIPTS' live-range knobs, and they are what makes `docs/PROCESS.md` §3's
#       count-free ENUMERATION self-checking: every file that list names is pinned against the range
#       this script holds, so a bump that skips one file, or a list that omits one, goes red instead of
#       rotting silently. Following the precedent its D46/D47 siblings state, the NEWEST script pins the
#       OLDER ones; THIS script's own knob has NO row, deliberately and not by oversight, because it is
#       the REFERENCE the other rows are compared against and a self-row could never fail. Do not
#       "restore" one; it would be vacuous by construction.
# Q12  is the contract knob, anchored on the constant's own definition line so this file's prose about
#       the version cannot satisfy it. It pins that D48 did NOT bump the contract.
# Q13  is the CARVE-OUT itself, anchored on the machine arm's own `match role` line: the arm must route
#       the Protocol case to the stderr sink. Deleting the branch (the headline mutation) makes the
#       anchor vanish, and a vanished anchor is a failure.
# Q14  is the classifier's Mcp arm, anchored on its own production line. A blanket classifier that
#       returned Reports for everything would rewrite exactly this line.
# Q15  is the CONSUMER half of the two-file co-occurrence, and it is a Q row because it shipped as a
#       P one and MEASURABLY proved nothing: a bare `stdout_role` token over `lib.rs` is also matched
#       by that file's own doc comment about `Command::stdout_role`, so deleting the production line
#       and hard-coding a role at the call site compiled and left this gate silent — the exact state
#       the row says it catches. That is the case the two-rule header calls out ("a landing that can
#       be satisfied by a doc comment is NOT a P row"), so the row now anchors on the production
#       line itself and requires the call that produces the value.
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
Q9@scripts/checks/d46-schema-migration-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and THAT sibling's ALTERNATION knob, a separate way the enumeration can be half-bumped
Q10@scripts/checks/d47-envelope-id-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D47 sibling's live-range knob — the PREVIOUS newest script, which by the self-row rule pins everyone except itself and so needs this row
Q11@scripts/checks/d47-envelope-id-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, the last of the seven ways the enumeration can be half-bumped
Q12@crates/unblock-mcp/src/options.rs@^pub const CONTRACT_VERSION@$CONTRACT_RE@D48 mints no ErrorCode and bumps NO contract: it moves an already-formed document to another stream. An unstated 'we didn't bump' is indistinguishable from an oversight, so it is stated here
Q13@crates/unblock-cli/src/exit.rs@^ +StdoutRole::Protocol =>@write_payload\(&out\.stdout, stderr\)@the CARVE-OUT itself: the machine arm routes a protocol-channel command's document to the STDERR sink. Anchored on the arm's own line, so the module doc that describes the rule cannot satisfy it
Q14@crates/unblock-cli/src/cli.rs@^ +Self::Mcp\(_\) =>@StdoutRole::Protocol@mcp is classified as a PROTOCOL channel. Anchored on the arm's own production line, which a blanket classifier would rewrite
Q15@crates/unblock-cli/src/lib.rs@^ +let stdout_role =@cli\.command\.stdout_role\(\)@the classification is CONSUMED at the exit boundary, not merely produced. Anchored on the production LINE — as a bare token over the file it was satisfied by lib.rs's own doc comment, so dropping the line and hard-coding a role passed
"

blocked=0

# PORTABILITY (1 of 2), deliberate: every `$( … )` in this file substitutes a FUNCTION CALL, never an
# inline loop containing a `case`. macOS `/bin/sh` (bash 3.2) mis-parses a `case` arm's `)` inside
# `$( )` and silently produces garbage instead of failing — this script would then "pass" vacuously on
# a developer machine. All four siblings avoid it the same way.
check_landings() {
  printf '%s\n' "$REQUIRE" | while IFS='@' read -r code path re reason; do
    [ -n "$code" ] || continue
    if [ ! -f "$path" ]; then
      printf '%s\n' "$path: [$code] REQUIRED D48 target is missing from the tree ($reason)"
      continue
    fi
    git grep -q -I -E "$re" -- "$path" 2>/dev/null \
      || printf '%s\n' "$path: [$code] the D48 landing is GONE — no line matches /$re/ ($reason)"
  done
  printf '%s\n' "$REQUIRE_ROW" | while IFS='@' read -r code path anchor re reason; do
    [ -n "$code" ] || continue
    if [ ! -f "$path" ]; then
      printf '%s\n' "$path: [$code] REQUIRED D48 target is missing from the tree ($reason)"
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
  say "BLOCKED — the D48 mechanism or its cascade is incomplete; the sites above are missing. A deleted role branch, a classification produced and dropped, a dangling residual id, or a roadmap card missing its fix are the cases this gate exists for: nothing else in the tree goes red for them."
  blocked=1
fi

# -------------------------------------------------------------------------------------------------
# S1 — THE SINGLE-CALLER INVARIANT, which no table row can express because it is about a COUNT of
# call sites rather than the presence of one.
#
# D48 clause (2)'s whole blast-radius argument is "`into_exit` has exactly ONE caller", which is what
# makes ONE edited arm cover every `commands/mcp.rs` return site. A SECOND caller passing a literal
# `Reports` would re-open ub-og3 with every unit and end-to-end cell of clause (7) still green, and
# nothing else in the tree would notice. So the invariant is asserted executably rather than merely
# observed. `into_exit_to(` cannot match here: the character after `into_exit` is `_`, not `(`.
#
# **It counts OCCURRENCES, not matching LINES, and that distinction was MEASURED rather than assumed:**
# a first draft counted lines and stayed GREEN against a mutant that put a second call on the SAME line
# as the first. A rule that a one-line edit can dodge is not a rule. The definition is excluded by
# dropping its whole LINE, which rustfmt guarantees carries nothing else.
# -------------------------------------------------------------------------------------------------
call_sites() {
  git grep -n -I -E 'into_exit\(' -- crates/unblock-cli/src 2>/dev/null \
    | grep -v -E 'fn into_exit\('
}

sites="$(call_sites)"
site_count="$(printf '%s\n' "$sites" | grep -o -E 'into_exit\(' | grep -c . )"
if [ "$site_count" != "1" ]; then
  printf '%s\n' "$sites" >&2
  say "BLOCKED — [S1] \`into_exit\` must have EXACTLY ONE caller (found $site_count). D48 clause (2)'s blast-radius argument rests on it: a second caller passing a literal Reports re-opens ub-og3 with every U- and E-cell still green."
  blocked=1
fi

# -------------------------------------------------------------------------------------------------
# S2 — EVERY `Command` VARIANT IS CLASSIFIED, which no table row can express either: it is a relation
# between two regions of one file.
#
# D48 clause (2) makes the no-`_`-arm exhaustiveness NORMATIVE — a new subcommand must fail to COMPILE
# until its author classifies it. That is enforced by rustc only while no wildcard exists, and the day
# someone "fixes" a build by adding `_ => Reports` the compiler stops asking and nothing else does. So
# this check reads the variant identifiers out of `enum Command` and requires each to appear inside
# `stdout_role`'s body. Added by the design Review gate, which observed that `Self::Mcp(_) => Protocol,
# _ => Reports` satisfies the classifier's own unit cell COMPLETELY.
# -------------------------------------------------------------------------------------------------
CLI_RS='crates/unblock-cli/src/cli.rs'

command_variants() { # the `Ident` of each variant declared in `pub enum Command { … }`
  awk '/^pub enum Command \{/ {inside=1; next}
       inside && /^\}/ {exit}
       inside && /^    [A-Z][A-Za-z0-9]*\(/ {sub(/\(.*/, "", $0); gsub(/^ +/, "", $0); print}' "$CLI_RS"
}

stdout_role_body() { # the lines of `fn stdout_role`, up to its closing brace at that indent
  awk '/fn stdout_role/ {inside=1}
       inside {print}
       inside && /^    \}$/ {exit}' "$CLI_RS"
}

check_exhaustive() {
  [ -f "$CLI_RS" ] || { printf '%s\n' "$CLI_RS: [S2] missing from the tree"; return; }
  body="$(stdout_role_body)"
  [ -n "$body" ] || { printf '%s\n' "$CLI_RS: [S2] \`fn stdout_role\` has no readable body"; return; }
  variants="$(command_variants)"
  [ -n "$variants" ] || { printf '%s\n' "$CLI_RS: [S2] \`enum Command\` declares no variants — this check would prove nothing"; return; }
  printf '%s\n' "$variants" | while read -r variant; do
    [ -n "$variant" ] || continue
    printf '%s\n' "$body" | grep -q -E "Self::$variant\b" \
      || printf '%s\n' "$CLI_RS: [S2] the subcommand \`$variant\` is NOT classified inside \`stdout_role\` — a \`_\` arm silently classifies the next subcommand instead of forcing its author to (D48 clause 2)"
  done
}

unclassified="$(check_exhaustive)"
if [ -n "$unclassified" ]; then
  printf '%s\n' "$unclassified" >&2
  say "BLOCKED — the classification is no longer EXHAUSTIVE over the subcommand enum. Clause (2) makes that normative: the compiler asks the question only while there is no wildcard arm."
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

# The floors are the counts this file shipped with. A floor may only move down together with the rows
# it counts moving somewhere the other floor counts them — never because a rule became inconvenient.
# That is exactly what happened once, in the D48 Verify repair round: the consumer landing moved from
# `P3` to `Q15` because a bare token was satisfiable by a doc comment, so the P floor dropped by one
# and the Q floor rose by one IN THE SAME EDIT. The pair still totals what it shipped with.
check_table_floor 'required-landing (P)' "$p_count" 15 || blocked=1
check_table_floor 'row-anchored (Q)' "$q_count" 15 || blocked=1

[ "$blocked" = "0" ] || exit 1
say "OK — the classification type, its classifier and its consumer are all still in the tree with exactly one caller and no unclassified subcommand, the carve-out arm still routes a protocol-channel command to stderr, the sink-injected core and the spawning regression file still exist, the hardened framing guard and the format-env scrub still stand, the tracker names ub-og3 and all four residual ids the PRD row cites (ub-kp7, ub-b1a, ub-c5o, ub-5v5), the rendered roadmap lists the decision, this gate is both specified and wired, and the live D-range is current at every prose site and every sibling script knob the PROCESS.md §3 list enumerates while the contract version stands unmoved."
exit 0
