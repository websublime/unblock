#!/bin/sh
# d54-parse-error-id-claims.sh — the REQUIRED-LANDING gate for D54, A PARSE ERROR ON A READABLE ID IS
# ANSWERED ON THAT ID (PRD §4 D54, tracked as `ub-788`). Spec: docs/plans/ci-cd-and-distribution.md
# §2.1, the paragraph beginning "Named sub-check (its D54 sibling…)". Runs as a step of the required
# `doc-lint` job, immediately after its `d53` sibling.
#
# THAT PARAGRAPH IS NORMATIVE OVER THIS FILE. Every landing enforced here is named there; a rule that
# exists in one and not the other is a defect to be fixed in the SAME change. Do not "tidy" a row away
# as unspecified — read the spec paragraph first.
#
# WHY THIS ONE IS POSITIVE-ONLY. A negative sweep for the retired framing ("left open", "id OMITTED")
# is dodged by a claim REWRAPPED across two lines, and the id-less wording stays CORRECT for every
# out-of-class line, so it cannot be forbidden at all. Every row is a positive landing; the
# co-occurrence rows (every line naming `ub-788` must also name D54) are how a live line that still
# reads the residual as open is caught.
#
# WHAT NO ROW HERE CAN SEE, stated so no row is read as covering it. A line grep cannot prove that
# the recovered id REACHES `park_reply` (Q8 pins the call, not the data flow), that the `Err` arm
# stays the only site, or that a cell RUNS rather than is `#[ignore]`d. Those are behaviour, and the
# byte cells this file pins by name go red for them. d47's Q12 (`^ +Some\(id\),$`) is single-sited
# only because D54's arm is spelled `EnvelopeId::Recovered(id) => Some(id),` on ONE line; a reformat
# that put a bare `Some(id),` line anywhere in wire.rs would leave d47's Q12 green with D47's own
# argument deleted, and no row here can count lines. Nor can a row see that leg (b)'s `Value` parse
# stays STRICT: a parse that skips `end()` leaves every anchored line in place, and only the X01
# cell (required by name in W-G4′) kills it.
#
# SEQUENCING, the same discipline every sibling states. This script and its workflow step belong to
# the `ci(d54)` commit that mints the file, because the code and cell rows assert lines that do not
# exist until the implementation and test commits. The D-range knob is the inverse coupling: it
# guards PROSE the spec commit already moved, so it is LIVE from this file's first commit.
#
# NO ROW IS KNOWN-RED ON THE COMMIT THAT ADDS THIS FILE. `ub-788` already has a record in
# `.unblock/issues.jsonl` on `main`, so no tracker re-export is needed before this file exists. Every
# row pins a landing of the spec, implementation, test or `ci(d54)` commit — none pins the issue's
# CLOSED state, the run-report, the pull-request number or any other Track-step landing. Rows pin
# PRESENCE, never state, and the export keeps closed rows.
#
# PRE-FIX TREE: `main` at 835d30c. Every row asserting a NEW landing FAILS there. The three that pass
# assert an unchanged state on purpose: P1 (`ub-788` already has a record), Q7 (the `-32700` reply
# keeps rmcp's code, message and no data) and Q25 (the contract version this decision does not move).
#
# THE CONTRACT KNOB IS PINNED BUT DOES NOT MOVE HERE. D54 mints no `ErrorCode` and moves no published
# byte, so `unblock.mcp.v1.10` stands; an unstated "we didn't bump" is indistinguishable from an
# oversight.
#
# TWO RULE KINDS (both positive)
#   P-n   REQUIRED landing — a presence predicate over the GENERATED export `.unblock/issues.jsonl`.
#   Q-n   ROW-ANCHORED landing — at least ONE line must match the anchor, and EVERY line matching the
#         anchor must ALSO match the requirement. A vanished anchor is a FAILURE, never a pass. Every
#         code anchor starts `^`, `^ +` or `^\)` followed by code, so no `///` or `//!` line can
#         satisfy it; Q1..Q12 read only wire.rs's lines before its `mod tests {`, so no test body
#         can satisfy them either (a missing or duplicated `mod tests {` line is exit 2).
#
# Exit: 0 = pass · 1 = BLOCK (a required landing is missing) · 2 = cannot evaluate (fail-closed).
set -u

# PORTABILITY (2 of 2). Every variable expansion goes through `printf`, never `echo`. POSIX-mode `echo`
# interprets backslash escapes, so a `\b` inside a regex literal becomes a BACKSPACE byte and the
# pattern silently stops matching. Not a style choice.
say() { printf 'd54-claims: %s\n' "$*" >&2; }

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

# The LIVE published contract version. D54 does NOT move it, so this row is the affirmative record of
# that. It tracks the LIVE id, so a later decision's bump moves it in that decision's implementation
# commit.
CONTRACT_RE='unblock\.mcp\.v1\.10'

# The SAME two spellings as they appear INSIDE a sibling script's knob line, where each backslash is a
# literal byte rather than a regex operator. DERIVED, never hand-written a second time. `printf '%s'`
# never interprets its ARGUMENT; `sed` then turns each `\` into `\\\`, i.e. ERE for "a literal
# backslash followed by the escaped char".
knob_re() { printf '%s' "$1" | sed 's/\\/\\\\\\/g'; }
RANGE_KNOB_RE="$(knob_re "$RANGE_RE")"
RANGE_KNOB_ALT_RE="$(knob_re "$RANGE_ALT_RE")"

# Q5's two halves, held in variables for the reason d47's Q12 gives: an END-ANCHOR cannot be written
# inline in the double-quoted `@`-separated tables below (`$@` expands to the positional parameters and
# eats the anchor), and the requirement runs over `git grep -n` output (`path:lineno:text`), so it can
# carry no `^`.
RECOVERED_ARM_LINE_RE='^ +EnvelopeId::Recovered\(id\) => Some\(id\),$'
RECOVERED_ARM_RE='=> Some\(id\),$'

# =================================================================================================
# REQUIRED LANDINGS — `code@path@regex@what it proves`
#
# P1 is the work's own id. D54 names no NEW residual id: its residuals are `ub-o8s`, rowed by d50 and
# d53, and the response-shaped-line disclosure, which is a disclosed residual with no issue by ruling.
# =================================================================================================
REQUIRE="
P1@.unblock/issues.jsonl@\"id\":\"ub-788\"@the tracker record names the work D54 implements. The export keeps closed rows, so the row survives the merge-time close
"

# =================================================================================================
# ROW-ANCHORED LANDINGS — `code@path@anchor@regex@what it proves`
#
# Q1..Q12 are THE MECHANISM, each anchored on its own production line in `wire.rs`: the failure type
#      and its strict-JSON variant, the reply-id function and its three arms, the unchanged reply
#      constructor, the call in the `Err` arm, and the three lines of `try_parse_with_compatibility`
#      that CLASSIFY a failure (the compat `Value` parse accepted it, rejected it, or the line is not
#      UTF-8). Where the requirement merely restates the anchor it is said plainly — the anchor is the
#      teeth.
# Q13..Q24 are the cells, the tier, the corpus and the harness, each on its own declaration.
# Q25 is the contract knob. Q26..Q40 are the SIBLING SCRIPTS' live-range knobs — the NEWEST script
#      pins the OLDER ones, and THIS script's own knob has NO row, deliberately: it is the REFERENCE the
#      other rows are compared against, and a self-row could never fail. Q39/Q40 are the marginal
#      pair: `d53`'s two knobs were pinned by nothing in the tree until this script landed.
# Q41..Q44 are the THREE PROSE bump sites over FOUR rows.
# Q45..Q54 are the normative texts: the D54 row, the reciprocal notes on D47, D50, D53 and D43,
#      NFR-18, spine §5.6, and the crate plan's fork doctrine, `src/wire.rs` row and test list.
# Q55..Q59 are CO-OCCURRENCE rows: every line naming `ub-788` in these files must also name D54, so no
#      live line reads the residual as open. They are doc pins on purpose. `docs/PRD.md` is not rowed
#      this way: its D48 row cites `ub-788` historically.
# Q60..Q63 are the roadmaps: the markdown bullet, and the RENDERED card (outside the doc-lint corpus)
#      — its new bullet and the corrected D47 and D50 bullets.
# Q64..Q67 are the SIBLING ROW TEXT this change corrects: `d47`'s P8 and success line, `d50`'s P3, and
#      `d53`'s newest-pointer row, which names the NEWEST gate (retargeted when D54, then D55, took the
#      pointer).
# Q68 is the implementation-plan task.
# Q69..Q73 are this gate's wiring: SPECIFIED, RUNNING, LISTED in PROCESS.md §3, and in ci-cd §2.1(a);
#      Q72 pins PROCESS.md §3's newest-reference sentence naming the NEWEST gate, which pins this
#      script's knobs (retargeted when D55 took the pointer).
# =================================================================================================
REQUIRE_ROW="
Q1@crates/unblock-mcp/src/wire.rs@^enum ParseFailure \{@enum ParseFailure \{@the Err arm's failure is still CLASSIFIED; without the type the strict-JSON verdict cannot reach the reply. The anchor is the teeth
Q2@crates/unblock-mcp/src/wire.rs@^ +NotAMessage\(serde_json::Error\),@NotAMessage\(serde_json::Error\),@the strict-JSON-but-not-a-message variant, the ONLY one that can carry a recovered id, is still declared
Q3@crates/unblock-mcp/src/wire.rs@^fn parse_error_reply_id\(@fn parse_error_reply_id\(failure: &ParseFailure, line: &\[u8\]\) -> Option<RequestId>@the reply-id function is still declared with its two inputs and its optional id
Q4@crates/unblock-mcp/src/wire.rs@^ +ParseFailure::NotJson\(_\) =>@ParseFailure::NotJson\(_\) => None,@leg (b): a line that is not strict JSON yields NO id. Mutant: scanning NotJson too, which answers trailing-garbage, non-UTF-8, lone-surrogate and over-deep lines on an id read from text that is not JSON
Q5@crates/unblock-mcp/src/wire.rs@$RECOVERED_ARM_LINE_RE@$RECOVERED_ARM_RE@leg (c): a RECOVERED id is passed through. The anchor is the teeth: rewriting the arm to answer None makes it vanish. It is distinct from D47's arm, which opens a block, and from d47's Q12 anchor, which this line does not match
Q6@crates/unblock-mcp/src/wire.rs@^ +EnvelopeId::Absent [|] EnvelopeId::Unusable =>@=> None,@an absent or unusable id still yields the OMITTED reply, never a fabricated one. The anchor is this arm's own two-verdict spelling, so D47's one-verdict arms cannot satisfy or break it
Q7@crates/unblock-mcp/src/wire.rs@^ +ErrorData::parse_error\(@ErrorData::parse_error\(\"Parse error\", None\),@the code stays -32700 with rmcp's message and no data, so the id is the ONLY byte that differs from rmcp (D54 clause 5). Unchanged on purpose: it passes on the pre-fix tree
Q8@crates/unblock-mcp/src/wire.rs@^ +let id = parse_error_reply_id\(@let id = parse_error_reply_id\(&failure, line\);@the Err arm still CALLS the recovery. A grep cannot prove the result reaches park_reply; the byte cells do
Q9@crates/unblock-mcp/src/wire.rs@^\) -> Result<Option<T>, ParseFailure> \{@ParseFailure@the compatibility parse still RETURNS the classification instead of rmcp's bare serde error
Q10@crates/unblock-mcp/src/wire.rs@^ +Ok\(_\) => ParseFailure::NotAMessage\(e\),@Ok\(_\) => ParseFailure::NotAMessage\(e\),@a line the compat Value parse ACCEPTED is still the only source of NotAMessage
Q11@crates/unblock-mcp/src/wire.rs@^ +Err\(_\) => ParseFailure::NotJson\(e\),@Err\(_\) => ParseFailure::NotJson\(e\),@a line the compat Value parse REJECTED is still NotJson. Mutant: classing it NotAMessage, which reopens the gate for trailing bytes, lone surrogates and over-deep nesting
Q12@crates/unblock-mcp/src/wire.rs@^ +\.map_err\(ParseFailure::NotJson\)@\.map_err\(ParseFailure::NotJson\)@a NON-UTF-8 line is still NotJson. Mutant: NotAMessage there, which X03 alone kills in the cells
Q13@crates/unblock-mcp/src/wire.rs@^ +async fn the_parse_error_corpus_is_answered_byte_for_byte@async fn the_parse_error_corpus_is_answered_byte_for_byte\(\)@every parse-error corpus entry is answered with its EXACT hand-written bytes and never delivered
Q14@crates/unblock-mcp/src/wire.rs@^ +fn the_parse_error_corpus_is_not_vacuous@fn the_parse_error_corpus_is_not_vacuous\(\)@every entry still REACHES the arm it is declared to grade — a tidied row grades nothing while the byte cells stay green
Q15@crates/unblock-mcp/src/wire.rs@^ +fn every_parse_error_kind_is_represented@fn every_parse_error_kind_is_represented\(\)@every reply kind and both gate halves stay in the corpus, as a SET
Q16@crates/unblock-mcp/src/wire.rs@^ +async fn the_per_entry_differential_holds_for_every_tier@async fn the_per_entry_differential_holds_for_every_tier\(\)@the per-entry differential still grades the id-inserted tier against rmcp's LIVE bytes, rmcp's own reply asserted id-less
Q17@crates/unblock-mcp/src/wire.rs@^ +IdInserted\(ParseExpect\),@IdInserted\(ParseExpect\),@the CD-7 harness still carries the D54 tier, so the set-equality guard declares its entries
Q18@crates/unblock-mcp/src/wire.rs@^ +\"-32700 recovered id\",@\"-32700 recovered id\",@the cancellation cell still carries the recovered-id -32700 arm, the only cell that sees an inline (unparked) write of this reply
Q19@crates/unblock-mcp/src/envelope_id_corpus.rs@^pub fn parse_error_corpus\(\)@pub fn parse_error_corpus\(\) -> Vec<ParseErrorFrame>@the hand-written corpus is still declared in the module the shared any(test, feature) gate compiles
Q20@crates/unblock-mcp/src/envelope_id_corpus.rs@^pub fn parse_error_bytes\(@pub fn parse_error_bytes\(expect: &ParseExpect\) -> Vec<u8>@the hand-written expected-bytes function is still declared — every byte assertion grades through it, never through the production encoder
Q21@crates/unblock-mcp/src/server.rs@^ +async fn a_parse_error_frame_before_the_handshake@async fn a_parse_error_frame_before_the_handshake_is_answered_on_its_id_and_the_handshake_survives\(\)@the pre-handshake cell is still declared — the composed stack answers the class before the latch opens and the handshake completes
Q22@crates/unblock-mcp/tests/parse_error_id_duplex.rs@^async fn an_in_class_tools_call@async fn an_in_class_tools_call_is_answered_on_its_id_and_executes_nothing\(\)@the store-EFFECT oracle on the real serve path is still declared
Q23@crates/unblock-mcp/tests/parse_error_id_duplex.rs@^async fn an_rmcp_client_is_released@async fn an_rmcp_client_is_released_by_the_parse_error_on_its_recovered_id\(\)@the REAL rmcp client cell is still declared — the only cell that pins rmcp's id routing of errors
Q24@crates/unblock-cli/tests/duplicate_key_frames.rs@^ +c\.saw_response_for\(9001\),@c\.saw_response_for\(9001\),@the real-binary NS2 cell still requires a reply ON id 9001, observed by sentinel follow so a dropped id fails at once
Q25@crates/unblock-mcp/src/options.rs@^pub const CONTRACT_VERSION@$CONTRACT_RE@D54 mints no ErrorCode and moves no published byte, so the contract stands; an unstated 'we didn't bump' is indistinguishable from an oversight
Q26@scripts/checks/d44-create-deps-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D44 sibling's live-range knob carries the SAME range as this script — anchored on the knob line, so the file's own prose about the knob cannot satisfy the pin
Q27@scripts/checks/ub-lp9.25-dangling-blocker-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D45 sibling's live-range knob, same anchoring and same reason
Q28@scripts/checks/ub-lp9.25-dangling-blocker-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, a separate line and so a separate way to be half-bumped
Q29@scripts/checks/d46-schema-migration-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D46 sibling's live-range knob, same anchoring and same reason
Q30@scripts/checks/d46-schema-migration-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q31@scripts/checks/d47-envelope-id-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D47 sibling's live-range knob, same anchoring and same reason
Q32@scripts/checks/d47-envelope-id-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q33@scripts/checks/d48-stdout-channel-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D48 sibling's live-range knob, same anchoring and same reason
Q34@scripts/checks/d48-stdout-channel-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q35@scripts/checks/d49-startup-failure-render-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D49 sibling's live-range knob, same anchoring and same reason
Q36@scripts/checks/d49-startup-failure-render-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q37@scripts/checks/d50-pre-handshake-gate-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D50 sibling's live-range knob, same anchoring and same reason
Q38@scripts/checks/d50-pre-handshake-gate-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q39@scripts/checks/d53-request-integrity-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D53 sibling's live-range knob — the PREVIOUS newest script, which by the self-row rule pins everyone except itself and so needs this row
Q40@scripts/checks/d53-request-integrity-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, the other of the two knobs d53 cannot pin itself
Q41@CLAUDE.md@^\| .docs/PRD\.md. \| Product truth@$RANGE_RE@PROSE D-range bump site, LOCATED on the document-map row that states the range
Q42@docs/plans/ci-cd-and-distribution.md@\*\*\(a\) D-id coherence\*\*@$RANGE_RE@PROSE D-range bump site, LOCATED on the class-(a) statement — the ONE place in that file allowed to quote the live range
Q43@xtask/src/doc_lint.rs@Spec tokenizes@$RANGE_RE@PROSE D-range bump site, half 1 of 2: the PROSE range on the tokenizer comment line
Q44@xtask/src/doc_lint.rs@Spec tokenizes@$RANGE_ALT_RE@…half 2 of 2: the TOKENIZER ALTERNATION on that same line
Q45@docs/PRD.md@^\| \*\*D54\*\* \|@Tracked as .ub-788.@the D54 decision row is in PRD §4 and names the work it tracks
Q46@docs/PRD.md@^\| \*\*D47\*\* \|@CLOSED by D54 \(this table\)@D47 clause 8(i) carries the reciprocal closure of the residual it named
Q47@docs/PRD.md@^\| \*\*D50\*\* \|@\*\*\[D54\]\*\* The .-32700. arm.s id omission@D50 clause (9) no longer lists ub-788 as an inherited open residual
Q48@docs/PRD.md@^\| \*\*D53\*\* \|@\*\*\[D54\]\*\* D54 \(this table\) adds a THIRD divergence@D53 clause (4) records the third divergence
Q49@docs/PRD.md@^\| \*\*D43\*\* \|@\*\*\[D54\]\*\*@the D43 fork record carries the D54 note
Q50@docs/PRD.md@NFR-18 \[security@CLOSED by D54@NFR-18 no longer lists the residual as open
Q51@docs/plans/01-design-spine.md@Member by member, measured@Since D54@spine §5.6 states the id rule
Q52@docs/plans/crates/unblock-mcp.md@reproduces ALL of .AsyncRwTransport::receive.@\[v1\.0\.1/D54\] A THIRD deliberate divergence@the fork doctrine sentence carries the third divergence on the same line, so 'reproduces ALL' cannot be read unqualified
Q53@docs/plans/crates/unblock-mcp.md@^\| .src/wire\.rs.@\[v1\.0\.1/D54\]@the crate plan's wire.rs row carries the D54 mechanism
Q54@docs/plans/crates/unblock-mcp.md@^- .tests/parse_error_id_duplex\.rs.@\[v1\.0\.1/D54\]@the crate plan lists the new duplex suite
Q55@docs/plans/00-roadmap.md@ub-788@D54@every roadmap line naming ub-788 also names D54, so none reads the residual as open
Q56@docs/plans/01-design-spine.md@ub-788@D54@every spine line naming ub-788 also names D54
Q57@crates/unblock-mcp/src/wire.rs@ub-788@D54@every wire.rs line naming ub-788 also names D54 — the transport records the closure beside the id
Q58@crates/unblock-cli/tests/duplicate_key_frames.rs@ub-788@D54@every line of the real-binary suite naming ub-788 also names D54
Q59@docs/roadmap.html@ub-788@D54@every RENDERED roadmap line naming ub-788 also names D54
Q60@docs/plans/00-roadmap.md@^- \*\*A parse error on a READABLE id@D54@the markdown roadmap's v1.0.1 bullet for this decision
Q61@docs/roadmap.html@^ +<li>Fix: a parse error on a readable id@D54@the RENDERED card's own D54 bullet — that file is OUTSIDE the doc-lint corpus
Q62@docs/roadmap.html@^ +<li>Fix: an un-decodable envelope@closed by D54 below@the RENDERED D47 bullet reports ub-788 closed
Q63@docs/roadmap.html@^ +<li>Fix: a first frame that is neither@closed by D54 below@the RENDERED D50 bullet reports ub-788 closed
Q64@scripts/checks/d47-envelope-id-claims.sh@^P8@D54@the D47 sibling's P8 pins the closure beside the id, not the open residual
Q65@scripts/checks/d47-envelope-id-claims.sh@^say \"OK@closed by D54@the D47 sibling's success line no longer calls ub-788 still open
Q66@scripts/checks/d50-pre-handshake-gate-claims.sh@^P3@CLOSED by D54@the D50 sibling's P3 reason no longer calls ub-788 OPEN
Q67@scripts/checks/d53-request-integrity-claims.sh@^Q53@d55-update-refusal-claims@the D53 sibling's newest-pointer row is retargeted to the NEWEST gate, which pins its knobs (D54 retargeted it, then D55)
Q68@docs/plans/implementation-plan.md@^- \*\*T3\.16 @D54@the task checklist exists and names the decision
Q69@docs/plans/ci-cd-and-distribution.md@Named sub-check \(its D54 sibling@d54-parse-error-id-claims@this gate is SPECIFIED in its own paragraph — §2.1(a) of that same file also carries this filename, so a bare token stays green with the paragraph deleted
Q70@.github/workflows/ci.yml@d54-parse-error-id-claims@:[0-9]+: +- run: scripts/checks/d54-parse-error-id-claims\.sh@this gate actually RUNS in the required doc-lint job; every line naming it must BE the run step, so a commented-out step fails
Q71@docs/PROCESS.md@d54-parse-error-id-claims\.sh. .RANGE_RE@RANGE_ALT_RE@the count-free LIST that IS the rule carries this script's own entry with BOTH knob names
Q72@docs/PROCESS.md@always the NEWEST script@d55-update-refusal-claims\.sh@PROCESS.md §3 names D55's gate as the newest reference — a script that pins THIS one's knobs, which this script cannot pin itself (retargeted when D55 took the pointer; the next mint retargets it again)
Q73@docs/plans/ci-cd-and-distribution.md@\*\*\(a\) D-id coherence\*\*@since the D54 implementation commit, .scripts/checks/d54-parse-error-id-claims\.sh.@ci-cd §2.1(a)'s enumeration carries this script's knobs
"

blocked=0

# PORTABILITY (1 of 2), deliberate. Every `$( … )` in this file substitutes a FUNCTION CALL, never an
# inline loop containing a `case`. macOS `/bin/sh` (bash 3.2) mis-parses a `case` arm's `)` inside
# `$( )` and silently produces garbage instead of failing. Every sibling avoids it the same way.
# Q1..Q12 read ONLY wire.rs's production prefix: lines before its `mod tests {`, so no test body can
# satisfy them. A missing or duplicated `mod tests {` line is fail-closed (exit 2), never a pass.
WIRE_TESTS_LINE="$(git grep -n -E '^mod tests \{$' -- crates/unblock-mcp/src/wire.rs 2>/dev/null | cut -d: -f2)"
case "$WIRE_TESTS_LINE" in
  ''|*[!0-9]*) say "cannot find exactly one '^mod tests {' line in crates/unblock-mcp/src/wire.rs"; exit 2 ;;
esac

check_landings() {
  printf '%s\n' "$REQUIRE" | while IFS='@' read -r code path re reason; do
    [ -n "$code" ] || continue
    if [ ! -f "$path" ]; then
      printf '%s\n' "$path: [$code] REQUIRED D54 target is missing from the tree ($reason)"
      continue
    fi
    git grep -q -I -E "$re" -- "$path" 2>/dev/null \
      || printf '%s\n' "$path: [$code] the landing is GONE — no line matches /$re/ ($reason)"
  done
  printf '%s\n' "$REQUIRE_ROW" | while IFS='@' read -r code path anchor re reason; do
    [ -n "$code" ] || continue
    if [ ! -f "$path" ]; then
      printf '%s\n' "$path: [$code] REQUIRED D54 target is missing from the tree ($reason)"
      continue
    fi
    rows="$(git grep -n -I -E "$anchor" -- "$path" 2>/dev/null)"
    case "$code" in
      Q[1-9]|Q1[0-2]) rows="$(printf '%s\n' "$rows" | awk -F: -v end="$WIRE_TESTS_LINE" '$2 + 0 < end + 0')" ;;
    esac
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
  say "BLOCKED — the D54 recovered-id reply or its cascade is incomplete; the sites above are missing."
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
check_table_floor 'required-landing (P)' "$p_count" 1 || blocked=1
check_table_floor 'row-anchored (Q)' "$q_count" 73 || blocked=1

[ "$blocked" = "0" ] || exit 1
say "OK — the parse-failure classification, the reply-id function and its three arms, the unchanged -32700 reply and the Err-arm call are in the tree, every cell, the tier, the corpus and the harness are declared, the tracker names ub-788, the PRD row and its reciprocal notes, NFR-18, the spine, the crate plan, both roadmaps and the task carry the closure, every live line naming ub-788 names D54, the corrected sibling rows are in place, this gate is both specified and wired, and the live D-range is current at every prose site and every sibling script's knob"
exit 0
