#!/bin/sh
# d49-startup-failure-render-claims.sh — the REQUIRED-LANDING gate for D49, the BOUNDED STRUCTURAL
# SUMMARY that replaces rmcp's `Debug` blob in the `unblock mcp` startup-failure message
# (PRD §4 D49, tracked as `ub-b1a`; spec: docs/plans/ci-cd-and-distribution.md §2.1, the paragraph
# beginning "Named sub-check (its D49 sibling…)"). Runs as a step of the required `doc-lint` job,
# immediately after its `d48` sibling.
#
# THAT PARAGRAPH IS NORMATIVE OVER THIS FILE. Every landing enforced here is named there; a rule that
# exists in one and not the other is a defect to be fixed in the SAME change. Do not "tidy" a row away
# as unspecified — read the spec paragraph first.
#
# WHY THIS ONE IS POSITIVE-ONLY
# -----------------------------
# D49 retires framing — the message no longer quotes the frame — and the tempting gate is a NEGATIVE
# sweep for the retired wording. That is the same defect in disguise, and this project has already paid
# for it, because a claim that gets REWRAPPED across two lines becomes unfindable in principle and the
# sweep goes green while the false sentence is still in the tree. Every row here is therefore a
# spelling-INDEPENDENT POSITIVE landing — the corrected text, or the code, must be PRESENT — and no row
# enumerates a spelling a reformatting could dodge.
#
# The other half of the reason is the shape of what D49 adds. Its teeth are executable — the cells
# behind the `test-util` seam render every arm the decision makes normative. What NO test can carry
# is ITS OWN SURVIVAL, and three of this decision's obligations are unreachable from any test at all —
# the MARKER FOLD in `bulk_markdown.rs` (the literal and the constant are equal bytes, so reverting the
# fold leaves every cell green), the RESIDUAL IDS the PRD row names — three of them, two rowed here
# and the third pinned by the D48 sibling, as the P table below states — and a rendered document
# (`docs/roadmap.html`) that sits outside every lint corpus in this repo.
#
# SEQUENCING, the same discipline all five siblings state. This script and its workflow step belong to
# the IMPLEMENTATION commit, because the code rows below assert against files that do not exist until
# then; a spec-only commit that shipped it would turn the required `doc-lint` job red against its own
# tree. The D-range knob is the inverse coupling: it guards PROSE, which the spec commit already moved,
# so it is LIVE from this file's first commit.
#
# TWO ROWS ARE KNOWN-RED ON THE COMMIT THAT ADDS THIS FILE, and the sequencing is disclosed here rather
# than discovered in CI. `ub-o8s` and `ub-wx3` reach `.unblock/issues.jsonl` only through the TRACK
# re-export, which `docs/PROCESS.md` §6 places AFTER both gates on the same branch — so those two rows
# go green in the same PR, whose head is what the required job evaluates. Do not weaken them to make an
# intermediate commit green.
#
# THE CONTRACT KNOB IS PINNED BUT DOES NOT MOVE HERE. D49 mints no `ErrorCode` (`ErrorCode::ALL` stays
# 36) and moves no published byte: `McpServerError`'s `Display` appears in neither `capabilities()` nor
# `schema_bundle()`, so `unblock.mcp.v1.9` stands. The knob tracks the IMPLEMENTATION commit that
# changes the code constant — there is no such commit in this work, which is exactly why the row exists:
# an unstated "we didn't bump" is indistinguishable from an oversight.
#
# TWO RULE KINDS (both positive — see above)
#   P-n   REQUIRED landing — a presence predicate over a NAMED file. A landing that can be satisfied by
#         a doc comment is NOT a P row; it is a Q row, which is why EVERY code landing here is a Q row.
#         The P rows read the GENERATED export `.unblock/issues.jsonl` and the hand-written
#         `.github/workflows/ci.yml` and `docs/PROCESS.md`. The fourth required-landing file,
#         `docs/plans/ci-cd-and-distribution.md`, is `Q21` instead, because that file names this script
#         in two places and only one of them is its specification. None of the four carries Rust doc
#         comments, so no bare token over them is satisfied by a file's own commentary about the thing.
#   Q-n   ROW-ANCHORED landing — at least ONE line must match the anchor, and EVERY line matching the
#         anchor must ALSO match the requirement. A vanished anchor is a FAILURE, never a pass: an
#         anchor that no longer exists proves nothing, and silently proving nothing is how a pin rots.
#         Used wherever a bare token would also match the file's own prose about the thing.
#
# THIS SCRIPT CARRIES NO STRUCTURAL CHECK. Its D48 sibling has two, because D48 rests on a RELATION —
# a call-site count and an exhaustive classification. D49 rests on renders, which its cells assert
# directly, so the rows below are the whole gate.
#
# Exit: 0 = pass · 1 = BLOCK (a required landing is missing) · 2 = cannot evaluate (fail-closed).
set -u

# PORTABILITY (2 of 2): every variable expansion goes through `printf`, never `echo`. POSIX-mode `echo`
# interprets backslash escapes, so a `\b` inside a regex literal becomes a BACKSPACE byte and the
# pattern silently stops matching. Not a style choice.
say() { printf 'd49-claims: %s\n' "$*" >&2; }

git rev-parse --show-toplevel >/dev/null 2>&1 || { say "not a git repository"; exit 2; }
cd "$(git rev-parse --show-toplevel)" || { say "cannot cd to the repo root"; exit 2; }

# The LIVE D-id range, in its TWO spellings. It tracks the LIVE range, never a frozen historical one:
# the day a D51 is minted, every file `docs/PROCESS.md` §3 enumerates moves with it or a required step
# goes red. §3 deliberately states that cascade as a LIST WITH NO COUNT — a derived count rotted there
# five times — and the Q rows below are what make the list self-checking.
#
# WHY TWO SPELLINGS. `xtask/src/doc_lint.rs`'s bump site is ONE physical line carrying BOTH halves: the
# prose range `(D1..D50)` and the tokenizer's regex ALTERNATION `\bD(50|49|48|…)\b`. Pinning only the
# prose is exactly how that site rots into an undefined-D51 finding — the lint would stop tokenizing
# the id it is being told exists.
RANGE_RE='D1\.\.D50'
RANGE_ALT_RE='D\(50\|49\|'

# The LIVE published contract version. D49 does NOT move it (see the header): this row is the
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
# Every row had ZERO matches before the D49 commits — `main` at 4f0ab59 — except the two the header
# names as deliberate unchanged-state exceptions (P1 and Q15), so none can pass vacuously.
#
# P1..P3 are the WORK's own id plus TWO of the THREE residuals the PRD row leaves OPEN. One row EACH,
#      so a failure says WHICH id vanished — a single row matching any of them would go green with two
#      dangling. The third residual, `ub-kp7`, is already pinned by the D48 sibling's own `P9` row
#      (`scripts/checks/d48-stdout-channel-claims.sh:131`), so the missing row here is coverage that
#      exists elsewhere rather than an oversight; D50 CLOSED that residual, and the row still
#      pins the presence of the id. Satisfy these rows by updating the issue over the
#      issue tool and re-exporting in the same PR — NEVER by hand-editing the generated file (D5 model B).
# P4   is this gate actually RUNNING in the workflow. A script that exists but is unwired fails on its
#      own rows rather than passing silently. The SPECIFIED landing is `Q21`.
# P5   is the count-free LIST in PROCESS.md §3 naming this script — the enumeration that makes the
#      whole D-range cascade self-checking, and which can otherwise rot silently.
#
# Every CODE landing is a Q row.
# =================================================================================================
REQUIRE="
P1@.unblock/issues.jsonl@ub-b1a@the tracker record names the work this implements (PROCESS.md §6 — re-export in the SAME PR as the work)
P2@.unblock/issues.jsonl@ub-o8s@residual (ii) of the PRD row's three — the stdio transport still reads an unbounded line, so the multi-megabyte frame is parsed before a short message describes it. OPEN, and the PRD row names it
P3@.unblock/issues.jsonl@ub-wx3@residual (iii) of the PRD row's three — rmcp still Debug-dumps POST-handshake frames under its own tracing, live at a single -v. OPEN, and the PRD row names it
P4@.github/workflows/ci.yml@d49-startup-failure-render-claims@this gate actually RUNS in the required doc-lint job
P5@docs/PROCESS.md@d49-startup-failure-render-claims@the count-free LIST that IS the rule names this script, so the enumeration cannot rot silently
"

# =================================================================================================
# ROW-ANCHORED LANDINGS — `code@path@anchor@regex@what it proves`
#
# Q1..Q3 are the three fn-anchored CODE landings. Each ANCHORS on a LINE-START `fn` declaration and
#      REQUIRES the item's full spelling on that line, so both ways of losing it fail — deleting the
#      item makes the anchor vanish, and a rename either vanishes with it or leaves the anchor matching
#      a line the requirement rejects. No comment can satisfy either half, because a doc-comment line
#      begins `///` after its indentation — the hole a bare presence grep leaves, MEASURED on this
#      very file, which names
#      `describe_initialize_error` in a doc comment at `:36`. The anchor's `^` is applied by `git
#      grep` to the FILE's line; the requirement is matched against `path:lineno:text`, so a
#      requirement can never carry a `^` of its own.
#      What NO grep of any shape can tell apart is a RUNNING cell from an `#[ignore]`d one. That is
#      the required `test` job's business, and these rows do not claim it.
# Q1   is THE MECHANISM. ONE description function renders every `ServerInitializeError` variant, so
#      the arm that echoed client bytes and any variant a future rmcp adds are both bounded at one
#      site. If it vanishes, the `Transport` display falls back to interpolating rmcp's own blob and
#      the whole decision is undone in one edit. It is declared at column 0, so its anchor is `^fn`;
#      the two cells sit inside `mod tests` and theirs is `^ +fn`.
# Q2   is the REGRESSION PIN — the four-shape cell. It is what proves the grammar and the forbidden
#      members, and clause (7) makes it the pin rather than an end-to-end cell, which went vacuous
#      once D50 gated that class.
# Q3   is the `Cancelled` BYTE-IDENTITY cell. D38's diagnostic routing was measured against the exact
#      line `failed to start the MCP server: Cancelled`, so the wildcard must not quote or pad it.
# Q1..Q3 anchor on an IDENTIFIER and never on a rendered message. The messages are asserted by the
#      cells, to the byte; a gate that grepped them too would go red on a legitimate reword.
# Q4   is the CLIP over the render's `clipped…` BINDINGS, which is the class an anchor can express. A
#      binding renamed OUT of that class leaves this row green — MEASURED — so the pin against a
#      dropped call is Q2's four-shape cell, which compares whole messages byte for byte. What this row
#      adds is that no surviving `clipped…` binding takes its value from anywhere but `clip`. Both
#      files in this table are heavily doc-commented and name `clip` in prose, so a bare token would
#      pass over a render that had dropped the call — the defect its D48 sibling's own Q15 row records
#      having MEASURED.
# Q5   is the marker FOLD, anchored on the `clip_header` line that appends it. This row is the ONLY
#      thing in the tree that can catch a revert to the hard-coded literal, because the constant and
#      the literal are equal bytes — every cell stays green through it, including the one that asserts
#      that literal at the clipped header's end. Clause (7) names that revert as the one mutation no
#      cell catches.
# Q6..Q14 are the SIBLING SCRIPTS' live-range knobs and Q16..Q19 the PROSE sites, and TOGETHER they
#      are what makes `docs/PROCESS.md` §3's count-free ENUMERATION self-checking, because every
#      file that list names is pinned against the range this script holds, so a bump that skips one
#      file, or a list that omits one, goes red instead of rotting silently. Following the precedent
#      its D46/D47/D48 siblings state, the NEWEST script pins the OLDER ones; THIS script's own knob
#      has NO row, deliberately and not by oversight, because it is the REFERENCE the other rows are
#      compared against and a self-row could never fail. Do not "restore" one; it would be vacuous
#      by construction. Q13/Q14 are the marginal pair — `d48`'s two knobs were unpinned by anything
#      in the tree until this script landed.
# Q15  is the contract knob, anchored on the constant's own definition line so this file's prose about
#      the version cannot satisfy it. It pins that D49 did NOT bump the contract.
# Q16..Q19 are the THREE PROSE bump sites, in the shape its D48 sibling ships at
#      `scripts/checks/d48-stdout-channel-claims.sh:171-174`. Each is anchored on its OWN normative
#      line, and `xtask/src/doc_lint.rs` contributes TWO rows because one physical line carries both
#      halves, the prose range and the tokenizer alternation. A file-level token check would prove
#      only that the literal sits SOMEWHERE in the file, so a document that ALSO discusses the range
#      in explanatory prose passes with its NORMATIVE statement still carrying the retired literal —
#      a defect D45 actually hit on `docs/plans/ci-cd-and-distribution.md`.
# Q20  is the RENDERED roadmap, anchored on the v1.0.1 card's own D49 bullet. That file sits OUTSIDE
#      the 19-file doc-lint corpus, so this row is the only thing in CI that can notice the published
#      card listing a fix set that is missing a fix — and anchoring it on the bullet is what makes it
#      say that, where a bare `D49` token over the file is satisfied by any mention anywhere in it.
# Q21  is this gate's SPECIFICATION, anchored on the D49 named sub-check paragraph's OWN opening line.
#      A file-level token over `docs/plans/ci-cd-and-distribution.md` is satisfied by §2.1(a), which
#      names this script for an unrelated reason, so the whole specification paragraph could be deleted
#      with the row still green — MEASURED. That is why this landing is a Q row here while the D48
#      sibling still pins the same file with a bare token (`d48`'s `P14`).
# =================================================================================================
REQUIRE_ROW="
Q1@crates/unblock-mcp/src/error.rs@^fn describe_initialize@fn describe_initialize_error\(err: &rmcp::service::ServerInitializeError\) -> String@the ONE description function is still declared at column 0 with its rmcp signature — without it the Transport display falls back to rmcp's own blob and D49 is undone in a single edit. No doc comment can match the anchor, and both ways of losing the item fail — a rename keeping the describe_initialize prefix matches the anchor and fails the requirement, and any other rename or a deletion makes the anchor vanish
Q2@crates/unblock-mcp/src/error.rs@^ +fn the_four_frame_shapes@fn the_four_frame_shapes_render_bounded_summaries\(\)@the four-shape regression pin is still declared under its own name — clause (7) makes this cell the pin, because an end-to-end cell went vacuous once D50 gated that class
Q3@crates/unblock-mcp/src/error.rs@^ +fn cancelled_renders@fn cancelled_renders_byte_identical_to_the_measured_line\(\)@the Cancelled byte-identity cell is still declared under its own name — D38's diagnostic routing was measured against that exact line, and the wildcard must neither quote nor pad it
Q4@crates/unblock-mcp/src/error.rs@^ +let clipped@= clip\(@every binding this render names clipped… takes its value from unblock_error::clip, the client member and the transport members alike. A binding renamed out of that class leaves this row green, so the pin against a DROPPED call is Q2's four-shape cell. Anchored on the production lines, so this file's own doc comments about clip cannot satisfy it
Q5@crates/unblock-mcp/src/tools/bulk_markdown.rs@^ +format!\(\"\{kept\}@TRUNCATION_MARKER@clip_header appends the CONSTANT and not a second hand-written copy of the literal. The two are equal bytes, so no cell can catch a revert — this row is the only thing that can
Q6@scripts/checks/d44-create-deps-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D44 sibling's live-range knob carries the SAME range as this script — anchored on the knob line, so the file's own prose about the knob cannot satisfy the pin
Q7@scripts/checks/ub-lp9.25-dangling-blocker-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D45 sibling's live-range knob, same anchoring and same reason
Q8@scripts/checks/ub-lp9.25-dangling-blocker-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, which is a separate line and therefore a separate way to be half-bumped
Q9@scripts/checks/d46-schema-migration-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D46 sibling's live-range knob, same anchoring and same reason
Q10@scripts/checks/d46-schema-migration-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and THAT sibling's ALTERNATION knob, a separate way the enumeration can be half-bumped
Q11@scripts/checks/d47-envelope-id-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D47 sibling's live-range knob, same anchoring and same reason
Q12@scripts/checks/d47-envelope-id-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, same reason again
Q13@scripts/checks/d48-stdout-channel-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D48 sibling's live-range knob — the PREVIOUS newest script, which by the self-row rule pins everyone except itself and so needs this row
Q14@scripts/checks/d48-stdout-channel-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, the other of the two knobs d48 cannot pin itself
Q15@crates/unblock-mcp/src/options.rs@^pub const CONTRACT_VERSION@$CONTRACT_RE@D49 mints no ErrorCode and bumps NO contract — it bounds the CONTENT of a message that appears in neither capabilities() nor schema_bundle(). An unstated 'we didn't bump' is indistinguishable from an oversight, so it is stated here
Q16@CLAUDE.md@^\| .docs/PRD\.md. \| Product truth@$RANGE_RE@PROSE D-range bump site, LOCATED on the document-map row that states the range
Q17@docs/plans/ci-cd-and-distribution.md@\*\*\(a\) D-id coherence\*\*@$RANGE_RE@PROSE D-range bump site, LOCATED on the class-(a) statement — the ONE place in that file allowed to quote the live range
Q18@xtask/src/doc_lint.rs@Spec tokenizes@$RANGE_RE@PROSE D-range bump site, half 1 of 2: the PROSE range on the tokenizer comment line
Q19@xtask/src/doc_lint.rs@Spec tokenizes@$RANGE_ALT_RE@…half 2 of 2: the TOKENIZER ALTERNATION on that same line
Q20@docs/roadmap.html@^ +<li>Fix: the mcp startup-failure message@D49@the RENDERED roadmap's v1.0.1 card still lists D49 on its own bullet — that file is OUTSIDE the 19-file doc-lint corpus, so nothing else in CI can catch a card missing a fix
Q21@docs/plans/ci-cd-and-distribution.md@Named sub-check \(its D49 sibling@d49-startup-failure-render-claims@this gate is SPECIFIED in its own paragraph rather than merely named in passing — §2.1(a) of that same file also carries this filename, so a bare token over the file stays green with the whole specification paragraph deleted
"

blocked=0

# PORTABILITY (1 of 2), deliberate: every `$( … )` in this file substitutes a FUNCTION CALL, never an
# inline loop containing a `case`. macOS `/bin/sh` (bash 3.2) mis-parses a `case` arm's `)` inside
# `$( )` and silently produces garbage instead of failing — this script would then "pass" vacuously on
# a developer machine. All five siblings avoid it the same way.
check_landings() {
  printf '%s\n' "$REQUIRE" | while IFS='@' read -r code path re reason; do
    [ -n "$code" ] || continue
    if [ ! -f "$path" ]; then
      printf '%s\n' "$path: [$code] REQUIRED D49 target is missing from the tree ($reason)"
      continue
    fi
    git grep -q -I -E "$re" -- "$path" 2>/dev/null \
      || printf '%s\n' "$path: [$code] the D49 landing is GONE — no line matches /$re/ ($reason)"
  done
  printf '%s\n' "$REQUIRE_ROW" | while IFS='@' read -r code path anchor re reason; do
    [ -n "$code" ] || continue
    if [ ! -f "$path" ]; then
      printf '%s\n' "$path: [$code] REQUIRED D49 target is missing from the tree ($reason)"
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
  say "BLOCKED — the D49 render or its cascade is incomplete; the sites above are missing. A description function replaced by rmcp's own blob, a dropped clip, a marker fold reverted to its literal, a dangling residual id, or a roadmap card missing its fix are the cases this gate exists for: nothing else in the tree goes red for them."
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
# it counts moving somewhere the other floor counts them — the P-to-Q move its D48 sibling records —
# never because a rule became inconvenient.
check_table_floor 'required-landing (P)' "$p_count" 5 || blocked=1
check_table_floor 'row-anchored (Q)' "$q_count" 21 || blocked=1

[ "$blocked" = "0" ] || exit 1
say "OK — the description function, the four-shape regression pin and the Cancelled byte-identity cell are all still declared as items, every binding the render names clipped… still takes its value from clip, clip_header still appends the shared marker constant rather than a second copy of the literal, the tracker names ub-b1a and the two residuals this gate rows (ub-o8s, ub-wx3), the rendered roadmap's v1.0.1 card still lists the decision, this gate is both specified and wired, and the live D-range is current at every prose site and every sibling script knob the PROCESS.md §3 list enumerates while the contract version stands unmoved."
exit 0
