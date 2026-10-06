#!/bin/sh
# d53-request-integrity-claims.sh — the REQUIRED-LANDING gate for D53, REQUEST INTEGRITY UNDER
# CANCELLATION (PRD §4 D53, tracked as `ub-zja`), and for the D47 clause 8(v) amendment that lands in
# the SAME pull request (tracked as `ub-nbz`; it rides D47 and takes no D-id of its own). Spec:
# docs/plans/ci-cd-and-distribution.md §2.1, the paragraph beginning "Named sub-check (its D53
# sibling…)". Runs as a step of the required `doc-lint` job, immediately after its `d52` sibling.
#
# THAT PARAGRAPH IS NORMATIVE OVER THIS FILE. Every landing enforced here is named there; a rule that
# exists in one and not the other is a defect to be fixed in the SAME change. Do not "tidy" a row away
# as unspecified — read the spec paragraph first.
#
# WHY ONE SCRIPT CARRIES TWO DECISIONS' LANDINGS. The D47 amendment mints no id, so it gets no script
# of its own, and editing the shipped `d47` table would also mean amending the D47 paragraph that is
# normative over it. Both changes edit the same `receive()` loop head in the same pull request, and
# the newest script already pins older decisions' text where a decision moves it (its `d50` sibling's
# Q44 pins `d48`'s P9). Every amendment row below says which decision it serves.
#
# WHY THIS ONE IS POSITIVE-ONLY. A negative sweep for the retired framing ("left OPEN", the one-harness
# figures, the retired helper name) is the defect its siblings describe: a claim REWRAPPED across two
# lines is unfindable in principle. Every row is a spelling-INDEPENDENT positive landing — the code,
# or the corrected text, must be PRESENT.
#
# WHAT NO ROW HERE CAN SEE, stated so no row is read as covering it. A line grep cannot tell WHERE in
# the loop a line sits, so moving the settle call below the read, or adding a second unconditional
# clear, keeps every Q row green. Those are behaviour, and the in-module cells this file pins by name
# go red for them. No grep can tell a RUNNING cell from an `#[ignore]`d one either; that is the
# required `test` job's business.
#
# SEQUENCING, the same discipline every sibling states. This script and its workflow step belong to
# the IMPLEMENTATION pull request's `ci(d53)` commit, the one that mints the file, because the code
# rows assert lines that do not exist until the fix commits. The D-range knob is the inverse coupling:
# it guards PROSE the spec commit already moved, so it is LIVE from this file's first commit.
#
# NO ROW IS KNOWN-RED ON THE COMMIT THAT ADDS THIS FILE, and the state is disclosed here rather than
# assumed. `ub-zja` was filed after the last tracker export on `main`, so the branch re-exports
# `.unblock/issues.jsonl` in its OWN first commit, BEFORE this file exists; the other three ids already
# had records. These rows pin PRESENCE rather than state, and the export keeps closed rows, so the
# Track re-export after both gates leaves every P row green. Satisfy a red one by updating the issue
# over the issue tool and re-exporting in the same PR, NEVER by hand-editing the generated file.
#
# THE CONTRACT KNOB IS PINNED BUT DOES NOT MOVE HERE. Neither change mints an `ErrorCode` or moves a
# published byte, so `unblock.mcp.v1.10` stands; an unstated "we didn't bump" is indistinguishable
# from an oversight.
#
# TWO RULE KINDS (both positive)
#   P-n   REQUIRED landing — a presence predicate over the GENERATED export `.unblock/issues.jsonl`,
#         the only file here that carries no commentary about these decisions.
#   Q-n   ROW-ANCHORED landing — at least ONE line must match the anchor, and EVERY line matching the
#         anchor must ALSO match the requirement. A vanished anchor is a FAILURE, never a pass. Every
#         code anchor starts `^ +` or `^ *` followed by code, so no `///` or `//!` line can satisfy it.
#
# Exit: 0 = pass · 1 = BLOCK (a required landing is missing) · 2 = cannot evaluate (fail-closed).
set -u

# PORTABILITY (2 of 2). Every variable expansion goes through `printf`, never `echo`. POSIX-mode `echo`
# interprets backslash escapes, so a `\b` inside a regex literal becomes a BACKSPACE byte and the
# pattern silently stops matching. Not a style choice.
say() { printf 'd53-claims: %s\n' "$*" >&2; }

git rev-parse --show-toplevel >/dev/null 2>&1 || { say "not a git repository"; exit 2; }
cd "$(git rev-parse --show-toplevel)" || { say "cannot cd to the repo root"; exit 2; }

# The LIVE D-id range, in its TWO spellings. It tracks the LIVE range, never a frozen historical one:
# the day a D56 is minted, every file `docs/PROCESS.md` §3 enumerates moves with it or a required step
# goes red. §3 deliberately states that cascade as a LIST WITH NO COUNT — a derived count rotted there
# five times — and the Q rows below are what make the list self-checking.
#
# WHY TWO SPELLINGS. `xtask/src/doc_lint.rs`'s bump site is ONE physical line carrying BOTH halves, the
# prose range `(D1..D55)` and the tokenizer's regex ALTERNATION `\bD(55|54|53|…)\b`. Pinning only the
# prose is exactly how that site rots into an undefined-D56 finding — the lint would stop tokenizing
# the id it is being told exists.
RANGE_RE='D1\.\.D55'
RANGE_ALT_RE='D\(55\|54\|'

# The LIVE published contract version. D53 does NOT move it, and neither does the D47 amendment, so
# this row is the affirmative record of that. It tracks the LIVE id, so a later decision's bump moves
# it in that decision's implementation commit.
CONTRACT_RE='unblock\.mcp\.v1\.10'

# The SAME two spellings as they appear INSIDE a sibling script's knob line, where each backslash is a
# literal byte rather than a regex operator. DERIVED, never hand-written a second time. `printf '%s'`
# never interprets its ARGUMENT; `sed` then turns each `\` into `\\\`, i.e. ERE for "a literal
# backslash followed by the escaped char".
knob_re() { printf '%s' "$1" | sed 's/\\/\\\\\\/g'; }
RANGE_KNOB_RE="$(knob_re "$RANGE_RE")"
RANGE_KNOB_ALT_RE="$(knob_re "$RANGE_ALT_RE")"

# =================================================================================================
# REQUIRED LANDINGS — `code@path@regex@what it proves`
#
# P1..P4 are the two works' ids plus every residual id the two PRD texts name. One row EACH, named
# rather than numbered, so a failure says WHICH id vanished.
# =================================================================================================
REQUIRE="
P1@.unblock/issues.jsonl@\"id\":\"ub-zja\"@the tracker record names the work D53 implements (PROCESS.md §6). The export keeps closed rows, so the row survives the state flip that closes it
P2@.unblock/issues.jsonl@\"id\":\"ub-nbz\"@the tracker record names the work the D47 clause 8(v) amendment implements, the id that amendment cites
P3@.unblock/issues.jsonl@\"id\":\"ub-o8s\"@residual D53 names — the stdio read still accepts a line of any length, which keeping a partial line across a dropped receive() neither widens nor closes
P4@.unblock/issues.jsonl@\"id\":\"ub-5v5\"@residual the D47 clause 8(v) amendment names as NOT reached — rmcp's own response sends aborted at its drain timeout are unchanged
"

# =================================================================================================
# ROW-ANCHORED LANDINGS — `code@path@anchor@regex@what it proves`
#
# Q1..Q5 are the D47 AMENDMENT's mechanism (`ub-nbz`): the parked handle, the spawn that fills it, the
# ONE write future both `send()` and the reply use, the settle at the loop head, and the settle in
# close(). Q6..Q9 are D53's (`ub-zja`): the completion flag, the CONDITIONAL clear, the EOF arm that
# keeps an unterminated final line, and the arm that marks a line complete. Each anchor is the
# production line itself, so a deletion makes the anchor vanish, which is a failure; where the
# requirement merely restates the anchor it is said plainly — the anchor is the teeth.
# Q10..Q20 are the cells and the harness they need, each on its own declaration.
# Q21 is the contract knob. Q22..Q34 are the SIBLING SCRIPTS' live-range knobs — the NEWEST script
#      pins the OLDER ones, and THIS script's own knob has NO row, deliberately: it is the REFERENCE the
#      other rows are compared against, and a self-row could never fail. Q33/Q34 are the marginal
#      pair: `d50`'s two knobs were pinned by nothing in the tree until this script landed.
# Q35..Q38 are the THREE PROSE bump sites over FOUR rows, each on its own normative line.
# Q39..Q43 are the normative texts: the D53 row, the D47 amendment, the D50 notes, the D43 note and
#      the crate plan's fork doctrine, each anchored on its own row or sentence.
# Q44..Q46 are the RENDERED roadmap, outside the 19-file doc-lint corpus: the new D53 bullet and the
#      corrected D47 and D50 bullets.
# Q47..Q49 are the SIBLING ROW TEXT this change corrects: `d47`'s P15 and `d50`'s P2 said the reply
#      loss was open, and `d50`'s Q19 named the removed helper.
# Q50..Q54 are this gate's wiring: SPECIFIED, RUNNING, LISTED in PROCESS.md §3, and in ci-cd §2.1(a);
#      Q53 pins PROCESS.md §3's newest-reference sentence naming the NEWEST gate, which pins this
#      script's knobs (retargeted when D54, then D55, took the pointer).
# =================================================================================================
REQUIRE_ROW="
Q1@crates/unblock-mcp/src/wire.rs@^ +parked_reply: Option<@parked_reply: Option<tokio::task::JoinHandle<std::io::Result<\(\)>>>,@[D47 8(v) amendment] the transport still PARKS the out-of-band reply's task handle, so a dropped receive() cannot take the reply along
Q2@crates/unblock-mcp/src/wire.rs@^ +\*slot = Some\(tokio::spawn\(@tokio::spawn\(write_owned\(Arc::clone\(write\), item\)\)\);@[D47 8(v) amendment] the reply is still SPAWNED on the same owned write future send() uses; awaiting it in place is the shape that lost it
Q3@crates/unblock-mcp/src/wire.rs@^ +write_owned\(self\.write\.clone\(\), item\)@write_owned\(self\.write\.clone\(\), item\)@[D47 8(v) amendment] send() and the parked reply still share ONE write future, so the bytes stay identical by construction and the three arms keep one behaviour
Q4@crates/unblock-mcp/src/wire.rs@^ +self\.settle_parked_reply\(\)\.await\?;@self\.settle_parked_reply\(\)\.await\?;@[D47 8(v) amendment] receive() still SETTLES the parked reply and returns None on its failure within the same call. The anchor is the teeth; the requirement restates it
Q5@crates/unblock-mcp/src/wire.rs@^ +let _ = self\.settle_parked_reply@let _ = self\.settle_parked_reply\(\)\.await;@[D47 8(v) amendment] close() still settles the parked reply before it takes the writer, so a reply answered before a close is not cut off by it
Q6@crates/unblock-mcp/src/wire.rs@^ +line_complete: bool,@line_complete: bool,@[D53] the transport still records whether line_buf holds a COMPLETE line. The anchor is the teeth
Q7@crates/unblock-mcp/src/wire.rs@^ +if self\.line_complete \{@if self\.line_complete \{@[D53] the line buffer is still cleared ONLY after a complete line (or once a read error abandons it, as rmcp does); an unconditional clear discards the bytes a cancelled read_until consumed
Q8@crates/unblock-mcp/src/wire.rs@^ +Ok\(0\) if @Ok\(0\) if self\.line_buf\.is_empty\(\) => return None,@[D53] EOF still ends the stream only when no partial line is held; a bare Ok(0) arm drops an unterminated final line that a dropped receive() left in the buffer
Q9@crates/unblock-mcp/src/wire.rs@^ +Ok\(_\) => self\.line_complete@Ok\(_\) => self\.line_complete = true,@[D53] a completed read still marks the line complete, which is the only thing that lets the next loop clear it
Q10@crates/unblock-mcp/src/wire.rs@^ +async fn an_out_of_band_reply_survives_a_dropped_receive@async fn an_out_of_band_reply_survives_a_dropped_receive\(\)@[D47 8(v) amendment] the deterministic cell (write lock held, receive() polled once and dropped, lock released, reply reaches the output) is still declared
Q11@crates/unblock-mcp/src/wire.rs@^ +async fn a_reply_that_cannot_be_written_ends_the_same_receive@async fn a_reply_that_cannot_be_written_ends_the_same_receive\(\)@[D47 8(v) amendment] the failed-write cell is still declared — a failed reply still ends the SAME receive() with None, which D40's teardown relies on
Q12@crates/unblock-mcp/src/wire.rs@^ +async fn a_panicked_reply_task_ends_the_receive@async fn a_panicked_reply_task_ends_the_receive\(\)@[D47 8(v) amendment] the abnormal-task cell is still declared — a JoinError maps to None exactly as a write error does
Q13@crates/unblock-mcp/src/wire.rs@^ +async fn consecutive_bad_frames_are_answered_in_arrival_order@async fn consecutive_bad_frames_are_answered_in_arrival_order\(\)@[D47 8(v) amendment] the ordering cell is still declared — it goes red if the settle moves below the read, which no line grep can see
Q14@crates/unblock-mcp/src/wire.rs@^ +async fn close_waits_for_a_parked_reply@async fn close_waits_for_a_parked_reply\(\)@[D47 8(v) amendment] the close cell is still declared
Q15@crates/unblock-mcp/src/wire.rs@^ +async fn a_line_split_by_a_dropped_receive_is_delivered_whole@async fn a_line_split_by_a_dropped_receive_is_delivered_whole\(\)@[D53] the deterministic mid-line drop cell is still declared — it goes red for an added unconditional clear, which no line grep can see
Q16@crates/unblock-mcp/src/wire.rs@^ +async fn an_unterminated_line_split_by_a_dropped_receive@async fn an_unterminated_line_split_by_a_dropped_receive_is_delivered_at_eof\(\)@[D53] the unterminated-final-line cell is still declared
Q17@crates/unblock-mcp/tests/receive_cancellation.rs@^ *async fn an_out_of_band_reply_survives_pings_in_flight@async fn an_out_of_band_reply_survives_pings_in_flight\(\)@[D47 8(v) amendment] the serve-loop cell with pings in flight over a small-capacity duplex is still declared
Q18@crates/unblock-mcp/tests/receive_cancellation.rs@^ *async fn a_frame_larger_than_one_read_survives@async fn a_frame_larger_than_one_read_survives_pings_in_flight\(\)@[D53] the serve-loop cell driving a frame larger than the 8 KiB read buffer with requests in flight is still declared
Q19@crates/unblock-mcp/tests/common/mod.rs@^pub async fn connect_raw_with_outbound_capacity@pub async fn connect_raw_with_outbound_capacity\(@the small-capacity connect helper is still declared; the 1 MiB helper beside it never fills, so the serve-loop reply cell would assert nothing
Q20@crates/unblock-mcp/tests/common/mod.rs@^ +pub (async )?fn write_raw_bytes@fn write_raw_bytes\(@the raw partial-write method the split-frame cell needs is still declared
Q21@crates/unblock-mcp/src/options.rs@^pub const CONTRACT_VERSION@$CONTRACT_RE@neither D53 nor the D47 amendment mints an ErrorCode or moves a published byte, so the contract stands; an unstated 'we didn't bump' is indistinguishable from an oversight
Q22@scripts/checks/d44-create-deps-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D44 sibling's live-range knob carries the SAME range as this script — anchored on the knob line, so the file's own prose about the knob cannot satisfy the pin
Q23@scripts/checks/ub-lp9.25-dangling-blocker-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D45 sibling's live-range knob, same anchoring and same reason
Q24@scripts/checks/ub-lp9.25-dangling-blocker-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, a separate line and so a separate way to be half-bumped
Q25@scripts/checks/d46-schema-migration-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D46 sibling's live-range knob, same anchoring and same reason
Q26@scripts/checks/d46-schema-migration-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q27@scripts/checks/d47-envelope-id-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D47 sibling's live-range knob, same anchoring and same reason
Q28@scripts/checks/d47-envelope-id-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q29@scripts/checks/d48-stdout-channel-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D48 sibling's live-range knob, same anchoring and same reason
Q30@scripts/checks/d48-stdout-channel-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q31@scripts/checks/d49-startup-failure-render-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D49 sibling's live-range knob, same anchoring and same reason
Q32@scripts/checks/d49-startup-failure-render-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q33@scripts/checks/d50-pre-handshake-gate-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D50 sibling's live-range knob — the PREVIOUS newest script, which by the self-row rule pins everyone except itself and so needs this row
Q34@scripts/checks/d50-pre-handshake-gate-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, the other of the two knobs d50 cannot pin itself
Q35@CLAUDE.md@^\| .docs/PRD\.md. \| Product truth@$RANGE_RE@PROSE D-range bump site, LOCATED on the document-map row that states the range
Q36@docs/plans/ci-cd-and-distribution.md@\*\*\(a\) D-id coherence\*\*@$RANGE_RE@PROSE D-range bump site, LOCATED on the class-(a) statement — the ONE place in that file allowed to quote the live range
Q37@xtask/src/doc_lint.rs@Spec tokenizes@$RANGE_RE@PROSE D-range bump site, half 1 of 2: the PROSE range on the tokenizer comment line
Q38@xtask/src/doc_lint.rs@Spec tokenizes@$RANGE_ALT_RE@…half 2 of 2: the TOKENIZER ALTERNATION on that same line
Q39@docs/PRD.md@^\| \*\*D53\*\* \|@Tracked as .ub-zja.@the D53 decision row is still in PRD §4 and still names the work it tracks
Q40@docs/PRD.md@^\| \*\*D47\*\* \|@AMENDED 2026-09-30 \(.ub-nbz.\)@the D47 row still carries the clause 8(v) amendment that closes the reply loss, riding D47 with no new id
Q41@docs/PRD.md@^\| \*\*D50\*\* \|@\[D47 clause 8\(v\) amendment, .ub-nbz.\]@the D50 row still carries the reciprocal notes that stop naming the reply loss as an inherited open residual
Q42@docs/PRD.md@^\| \*\*D43\*\* \|@\*\*\[D53\]\*\*@the D43 row still carries the reciprocal note that its fork now diverges from AsyncRwTransport under cancellation
Q43@docs/plans/crates/unblock-mcp.md@reproduces ALL of .AsyncRwTransport::receive.@\[v1\.0\.1/D53\] A SECOND deliberate divergence@the fork doctrine sentence still carries the D53 qualification on the same line, so 'reproduces ALL' cannot be read unqualified
Q44@docs/roadmap.html@^ +<li>Fix: a well-formed request was destroyed@D53@the RENDERED roadmap's v1.0.1 card still lists D53 on its own bullet — that file is OUTSIDE the doc-lint corpus
Q45@docs/roadmap.html@^ +<li>Fix: an un-decodable envelope@closed in this same cut by an amendment riding D47@the RENDERED D47 bullet still reports the reply loss CLOSED rather than open
Q46@docs/roadmap.html@^ +<li>Fix: a first frame that is neither@only the signal path can drop that read@the RENDERED D50 bullet still carries the rewritten disclosure of the gate's own reply
Q47@scripts/checks/d47-envelope-id-claims.sh@^P15@CLOSED by the D47 clause 8\(v\) amendment@the D47 sibling's P15 reason string still carries the correction — it called the reply loss open
Q48@scripts/checks/d50-pre-handshake-gate-claims.sh@^P2@CLOSED by the D47 clause 8\(v\) amendment@the D50 sibling's P2 reason string still carries the correction — it called the reply loss OPEN
Q49@scripts/checks/d50-pre-handshake-gate-claims.sh@^Q19@the contract the scanner.s own out-of-band replies keep@the D50 sibling's Q19 reason string no longer names the helper the D47 amendment removed
Q50@docs/plans/ci-cd-and-distribution.md@Named sub-check \(its D53 sibling@d53-request-integrity-claims@this gate is SPECIFIED in its own paragraph — §2.1(a) of that same file also carries this filename, so a bare token stays green with the paragraph deleted
Q51@.github/workflows/ci.yml@d53-request-integrity-claims@:[0-9]+: +- run: scripts/checks/d53-request-integrity-claims\.sh@this gate actually RUNS in the required doc-lint job; every line naming it must BE the run step, so a commented-out step fails
Q52@docs/PROCESS.md@d53-request-integrity-claims\.sh. .RANGE_RE@RANGE_ALT_RE@the count-free LIST that IS the rule carries this script's own entry with BOTH knob names
Q53@docs/PROCESS.md@always the NEWEST script@d55-update-refusal-claims\.sh@PROCESS.md §3 names D55's gate as the newest reference — a script that pins THIS one's knobs, which this script cannot pin itself (retargeted when D54, then D55, took the pointer; the next mint retargets it again)
Q54@docs/plans/ci-cd-and-distribution.md@\*\*\(a\) D-id coherence\*\*@since the D53 implementation commit, .scripts/checks/d53-request-integrity-claims\.sh.@ci-cd §2.1(a)'s enumeration carries this script's knobs
"

blocked=0

# PORTABILITY (1 of 2), deliberate. Every `$( … )` in this file substitutes a FUNCTION CALL, never an
# inline loop containing a `case`. macOS `/bin/sh` (bash 3.2) mis-parses a `case` arm's `)` inside
# `$( )` and silently produces garbage instead of failing. Every sibling avoids it the same way.
check_landings() {
  printf '%s\n' "$REQUIRE" | while IFS='@' read -r code path re reason; do
    [ -n "$code" ] || continue
    if [ ! -f "$path" ]; then
      printf '%s\n' "$path: [$code] REQUIRED D53 target is missing from the tree ($reason)"
      continue
    fi
    git grep -q -I -E "$re" -- "$path" 2>/dev/null \
      || printf '%s\n' "$path: [$code] the landing is GONE — no line matches /$re/ ($reason)"
  done
  printf '%s\n' "$REQUIRE_ROW" | while IFS='@' read -r code path anchor re reason; do
    [ -n "$code" ] || continue
    if [ ! -f "$path" ]; then
      printf '%s\n' "$path: [$code] REQUIRED D53 target is missing from the tree ($reason)"
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
  say "BLOCKED — the D53 read fix, the D47 clause 8(v) amendment or their cascade is incomplete; the sites above are missing."
  blocked=1
fi

# SELF-TEST — no rule table may have SHRUNK below the counts it shipped with; the two tables are
# counted SEPARATELY so adding a row to one can never mask the deletion of a row from the other.
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
check_table_floor 'required-landing (P)' "$p_count" 4 || blocked=1
check_table_floor 'row-anchored (Q)' "$q_count" 54 || blocked=1

[ "$blocked" = "0" ] || exit 1
say "OK — the parked reply, its spawn, the shared write future and both settles are in the tree, the line buffer is cleared only after a complete line (or a read error) and EOF keeps an unterminated one, every cell and the small-capacity harness are declared, the tracker names ub-zja, ub-nbz and both residual ids, the D53, D47, D50 and D43 rows and the fork doctrine carry their texts, the rendered roadmap carries all three bullets, the sibling reason strings are corrected, this gate is specified, wired, listed and named as the newest reference, and the live D-range is current at every prose site and every sibling knob while the contract version stands unmoved."
exit 0
