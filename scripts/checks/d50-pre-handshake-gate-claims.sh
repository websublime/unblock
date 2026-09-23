#!/bin/sh
# d50-pre-handshake-gate-claims.sh — the REQUIRED-LANDING gate for D50, the PRE-HANDSHAKE FRAME GATE
# that intercepts a first frame which is neither `initialize` nor `ping` before rmcp's handshake loop
# can die on it (PRD §4 D50, tracked as `ub-kp7`; spec: docs/plans/ci-cd-and-distribution.md §2.1, the
# paragraph beginning "Named sub-check (its D50 sibling…)"). Runs as a step of the required `doc-lint`
# job, immediately after its `d49` sibling.
#
# THAT PARAGRAPH IS NORMATIVE OVER THIS FILE. Every landing enforced here is named there; a rule that
# exists in one and not the other is a defect to be fixed in the SAME change. Do not "tidy" a row away
# as unspecified — read the spec paragraph first.
#
# WHY THIS ONE IS POSITIVE-ONLY
# -----------------------------
# D50 retires the framing that called the pre-handshake death correct, and the tempting gate is a
# NEGATIVE sweep for that retired wording. That is the same defect in disguise, and this project has
# already paid for it, because a claim REWRAPPED across two lines becomes unfindable in principle and
# the sweep goes green while the false sentence is still in the tree. Every row here is therefore a
# spelling-INDEPENDENT POSITIVE landing — the corrected text, or the code, must be PRESENT — and no row
# enumerates a spelling a reformatting could dodge.
#
# The other half of the reason is the shape of what D50 adds. Its teeth are executable — the gate's own
# in-module cells and the repointed and inverted `mcp_lifecycle.rs` cells make a behavioural regression
# a red `test`. The RESIDUAL IDS the PRD row names are unreachable from any test at all, each one
# beside the work's own id, and `docs/roadmap.html` is a rendered document sitting outside every lint
# corpus here. The COMPOSITION LINE is the row this gate is built around, and the claim about it is
# narrow. NO IN-MODULE CELL OF THE GATE READS THE COMPOSITION — deleting the layer at
# `crates/unblock-mcp/src/server.rs:468` leaves `cargo test -p unblock-mcp --lib pre_handshake` GREEN,
# MEASURED. The gate's own cells build the decorator directly, and the one cell that name filter also
# runs from outside the module (`crates/unblock-mcp/src/error.rs:528`) builds none. What does go red
# for that deletion is the stdio lifecycle suite in `crates/unblock-cli/tests/mcp_lifecycle.rs` and
# workspace clippy, which reports the unreached layer as dead code and fails under `-D warnings`. The
# row earns its place twice over. It is the doc-lint catch, which runs in seconds where the stdio
# suite spawns a child per cell, and it still catches a subtler deletion that keeps the type
# constructed elsewhere, where clippy has nothing to report and that stdio suite is the other catch.
#
# SEQUENCING, the same discipline every sibling script states. This script and its workflow step belong
# to the IMPLEMENTATION commit, because the code rows below assert against files that do not exist until
# then; a spec-only commit that shipped it would turn the required `doc-lint` job red against its own
# tree. The D-range knob is the inverse coupling. It guards PROSE, which the spec commit already moved,
# so it is LIVE from this file's first commit.
#
# NO ROW IS KNOWN-RED ON THE COMMIT THAT ADDS THIS FILE, and the state is disclosed here rather than
# assumed. All seven ids the P table rows already have a record in `.unblock/issues.jsonl`, because the
# D47, D48 and D49 cascades each exported theirs. What the TRACK re-export changes is `ub-kp7`'s STATE,
# and these rows pin PRESENCE rather than state — the export keeps closed rows, so the re-export that
# `docs/PROCESS.md` §6 places after both gates on the same branch leaves every P row green. Satisfy a
# red one by updating the issue over the issue tool and re-exporting in the same PR, NEVER by
# hand-editing the generated file (D5 model B).
#
# THE CONTRACT KNOB IS PINNED BUT DOES NOT MOVE HERE. D50 mints no `ErrorCode` (`ErrorCode::ALL` stays
# 36) and moves no published byte, because `-32600` is rmcp's own code and appears in neither
# `capabilities()` nor `schema_bundle()`, so `unblock.mcp.v1.9` stands. The knob tracks the
# IMPLEMENTATION commit that changes the code constant — there is no such commit in this work, which is
# exactly why the row exists. An unstated "we didn't bump" is indistinguishable from an oversight.
#
# TWO RULE KINDS (both positive — see above)
#   P-n   REQUIRED landing — a presence predicate over a NAMED file. A landing that a COMMENT can
#         satisfy is NOT a P row; it is a Q row, which is why every code landing here is a Q row and
#         why all THREE self-wiring landings are Q rows as well. The P rows read one file, the
#         GENERATED export `.unblock/issues.jsonl`, which carries no commentary about this decision at
#         all. The three wiring files each do — MEASURED, each one with the mutation that a bare token
#         would have to catch. A commented-out `- run:` line in `.github/workflows/ci.yml` still
#         carries the filename; `docs/PROCESS.md` names this script twice, so dropping it from the
#         count-free LIST leaves the other mention standing; and `docs/plans/ci-cd-and-distribution.md`
#         names it in §2.1(a) as well as in its specification paragraph.
#   Q-n   ROW-ANCHORED landing — at least ONE line must match the anchor, and EVERY line matching the
#         anchor must ALSO match the requirement. A vanished anchor is a FAILURE and never a pass,
#         because an anchor that proves nothing while looking green is how a pin rots.
#         Used wherever a bare token would also match the file's own prose about the thing.
#
# THIS SCRIPT CARRIES NO STRUCTURAL CHECK, and the absence is stated rather than left to be noticed.
# Its D48 sibling has two because D48 rests on RELATIONS — a call-site count and an exhaustive
# classification. D50 rests on a decorator that is either installed or not, which `Q1` reads directly
# off the composition line, and on dispositions its cells assert frame by frame.
#
# Exit: 0 = pass · 1 = BLOCK (a required landing is missing) · 2 = cannot evaluate (fail-closed).
set -u

# PORTABILITY (2 of 2). Every variable expansion goes through `printf`, never `echo`. POSIX-mode `echo`
# interprets backslash escapes, so a `\b` inside a regex literal becomes a BACKSPACE byte and the
# pattern silently stops matching. Not a style choice.
say() { printf 'd50-claims: %s\n' "$*" >&2; }

git rev-parse --show-toplevel >/dev/null 2>&1 || { say "not a git repository"; exit 2; }
cd "$(git rev-parse --show-toplevel)" || { say "cannot cd to the repo root"; exit 2; }

# The LIVE D-id range, in its TWO spellings. It tracks the LIVE range, never a frozen historical one:
# the day a D52 is minted, every file `docs/PROCESS.md` §3 enumerates moves with it or a required step
# goes red. §3 deliberately states that cascade as a LIST WITH NO COUNT — a derived count rotted there
# five times — and the Q rows below are what make the list self-checking.
#
# WHY TWO SPELLINGS. `xtask/src/doc_lint.rs`'s bump site is ONE physical line carrying BOTH halves, the
# prose range `(D1..D51)` and the tokenizer's regex ALTERNATION `\bD(51|50|49|…)\b`. Pinning only the
# prose is exactly how that site rots into an undefined-D52 finding — the lint would stop tokenizing
# the id it is being told exists.
RANGE_RE='D1\.\.D51'
RANGE_ALT_RE='D\(51\|50\|'

# The LIVE published contract version. D50 does NOT move it (see the header), so this row is the
# affirmative record of that and a silent bump riding this decision goes red.
CONTRACT_RE='unblock\.mcp\.v1\.9'

# The SAME two spellings as they appear INSIDE a sibling script's knob line, where each backslash is a
# literal byte rather than a regex operator. DERIVED, never hand-written a second time, because a second
# copy of the range in this file would be a second thing to bump, which is the rot this whole clause
# exists to stop. `printf '%s'` never interprets its ARGUMENT, so the value passes through untouched; `sed` then
# turns each `\` into `\\\`, i.e. ERE for "a literal backslash followed by the escaped char".
knob_re() { printf '%s' "$1" | sed 's/\\/\\\\\\/g'; }
RANGE_KNOB_RE="$(knob_re "$RANGE_RE")"
RANGE_KNOB_ALT_RE="$(knob_re "$RANGE_ALT_RE")"

# =================================================================================================
# REQUIRED LANDINGS — `code@path@regex@what it proves`
#
# Every row asserting a NEW landing was evaluated against the PRE-FIX TREE, `main` at 11ea156, and
# FAILS there. The rows that pass on that tree each assert an unchanged state on purpose —
# the seven ids of P1..P7 already had records, the repointed cell of Q22 kept the name it shipped
# with, and Q24 pins a contract version this decision does not move. No other row can pass
# vacuously, because the landing it names did not exist before this work.
#
# P1..P7 are the WORK's own id plus every residual id the PRD row names. One row EACH and named
#      rather than numbered, so a failure says WHICH id vanished — a single row matching any of them
#      would go green with the rest dangling, and an "n of m" description rots at the next mint as
#      the d48 sibling's P9 row did. P2..P4 are the residuals the gate's own reply and read path
#      INHERIT; P5..P7 are the sibling rows' residuals, unmoved by D50 and rowed so none reads as
#      closed by omission.
#
# Every CODE landing and every WIRING landing is a Q row.
# =================================================================================================
REQUIRE="
P1@.unblock/issues.jsonl@ub-kp7@the tracker record names the work this implements (PROCESS.md §6 — re-export in the SAME PR as the work). The export keeps closed rows, so the row survives the state flip that closes it
P2@.unblock/issues.jsonl@ub-nbz@inherited residual — the -32600 is LOST whenever rmcp cancels the receive() future, the seam the gate writes its reply inside. OPEN, and the PRD row names it
P3@.unblock/issues.jsonl@ub-788@inherited residual — the -32700 arm still omits a readable id, so a duplicated method or jsonrpc member still leaves an rmcp client pending. OPEN, and the PRD row names it
P4@.unblock/issues.jsonl@ub-o8s@inherited residual — the stdio read still carries no maximum line length, so an oversized premature frame is read and scanned before the gate drops it. OPEN, and the PRD row names it
P5@.unblock/issues.jsonl@ub-c5o@sibling residual, unmoved by D50 — output emit_report still writes to stdout unconditionally with no classification. OPEN, and the PRD row names it
P6@.unblock/issues.jsonl@ub-5v5@sibling residual, unmoved by D50 — an oversized response can still leave a TRUNCATED frame on the framing channel. OPEN, and the PRD row names it
P7@.unblock/issues.jsonl@ub-wx3@sibling residual, unmoved by D50 — rmcp's post-handshake tracing still Debug-dumps frames at a single -v. OPEN, and the PRD row names it
"

# =================================================================================================
# ROW-ANCHORED LANDINGS — `code@path@anchor@regex@what it proves`
#
# Q1   is THE INSTALLATION, and it is the row this whole gate exists for. The decorator is a layer of
#      the composed transport, so deleting it from the composition still builds, ships a server that
#      dies on a premature frame again, and leaves every in-module cell green — the cells build the
#      decorator directly and never read the composition, and the dead-code report left behind names
#      an unused item rather than the defect. The row anchors on the OUTER wrapper's
#      constructor, which survives that deletion, and REQUIRES the gate's own constructor on the same
#      line, so the deletion fails the row while the anchor stands.
# Q2..Q8 are the MECHANISM, each anchored on its own declaration or its own production line. Q2 and Q3
#      are the type and its Transport impl, which are two ways to lose the layer — a struct with no
#      impl is not a transport. Q4 and Q5 are the two crate-private functions the decision makes
#      normative, the receive-side classifier and the send-side latch predicate, anchored on LINE-START
#      `fn` declarations so no doc comment can satisfy either half; a doc-comment line begins `///`
#      after its indentation, and this module names both functions in its own prose.
# Q6   is the EXHAUSTIVE four-variant match, pinned through the Response arm of the classifier's
#      or-pattern. A `_` wildcard compiles, passes every cell and silently reopens the hole an rmcp
#      variant bump would drive through, and it makes this ANCHOR vanish, which this table counts as a
#      failure. The anchor is what carries the teeth here, because a bare token over the file is
#      satisfied by the in-module cell that names the same variant inside a `matches!` — MEASURED.
# Q7   is the LATCH KEY. It pins that the send-side predicate matches the InitializeResult VARIANT, so
#      a rewrite onto the protocol version string, which the clamp above this layer may rewrite, fails
#      the row. Anchored on the production `matches!` line, because this module also names that
#      variant in its own doc prose and in a test helper, either of which satisfies a bare token.
# Q8   is the compile-time reply constant, required BY ITS TEXT. The message is normative in D50 clause
#      (2), and the in-module cells compare against their own copy of it, so a coordinated edit of both
#      would stay green — this row is what pins the shipped sentence.
# Q9..Q19 are the gate's in-module cells and Q20 the composition cell that drives a real duplex through
#      the server's own entry point, each anchored on its own `fn` declaration. No grep of any shape
#      can tell a RUNNING cell from an `#[ignore]`d one; that is the required `test` job's business,
#      and these rows do not claim it. What they do claim is that the cell still EXISTS under its own
#      name, which nothing else in the tree notices.
# Q21..Q22 are the two shipped `mcp_lifecycle.rs` cells the decision moves. Q21 is the INVERTED one,
#      which now asserts the handshake completing where it used to assert exit 1. Q22 is the REPOINTED
#      one, which keeps the name it shipped with and is therefore the one row here that matched on the
#      pre-fix tree — it pins that the last wire-reachable unsignalled run-loop error still has a
#      witness after the gate made the old provocation survivable.
# Q23  is the stdout-closing harness method Q22's provocation needs. It is the only method that closes
#      the READ end of the child's stdout, and the two neighbouring methods hold that end open, so a
#      revert to either leaves the repointed cell asserting nothing about a broken pipe.
# Q24  is the contract knob, anchored on the constant's own definition line so this file's prose about
#      the version cannot satisfy it. It pins that D50 did NOT bump the contract.
# Q25..Q35 are the SIBLING SCRIPTS' live-range knobs and Q36..Q39 the PROSE sites, and TOGETHER they
#      are what makes `docs/PROCESS.md` §3's count-free ENUMERATION self-checking, because every
#      file that list names is pinned against the range this script holds, so a bump that skips one
#      file, or a list that omits one, goes red instead of rotting silently. Following the precedent
#      its D46/D47/D48/D49 siblings state, the NEWEST script pins the OLDER ones; THIS script's own
#      knob has NO row, deliberately and not by oversight, because it is the REFERENCE the other rows
#      are compared against and a self-row could never fail. Do not "restore" one; it would be vacuous
#      by construction. Q34/Q35 are the marginal pair — `d49`'s two knobs were unpinned by anything in
#      the tree until this script landed.
# Q36..Q39 are the THREE PROSE bump sites over FOUR rows, in the shape its D48 and D49 siblings ship.
#      Each is anchored on its OWN normative line, and `xtask/src/doc_lint.rs` contributes TWO rows
#      because one physical line carries both halves, the prose range and the tokenizer alternation. A
#      file-level token check would prove only that the literal sits SOMEWHERE in the file, so a
#      document that ALSO discusses the range in explanatory prose passes with its NORMATIVE statement
#      still carrying the retired literal — a defect D45 actually hit on
#      `docs/plans/ci-cd-and-distribution.md`.
# Q40  is the RENDERED roadmap, anchored on the v1.0.1 card's own D50 bullet. That file sits OUTSIDE
#      the 19-file doc-lint corpus, so this row is the only thing in CI that can notice the published
#      card listing a fix set that is missing a fix — and anchoring it on the bullet is what makes it
#      say that, where a bare `D50` token over the file is satisfied by any mention anywhere in it.
# Q41  is this gate's SPECIFICATION, anchored on the D50 named sub-check paragraph's OWN opening line.
#      A file-level token over `docs/plans/ci-cd-and-distribution.md` is satisfied by §2.1(a), which
#      names this script for an unrelated reason, so the whole specification paragraph could be deleted
#      with the row still green — MEASURED on its D49 sibling.
# Q42  is this gate actually RUNNING in the workflow. A script that exists but is unwired proves
#      nothing, and a bare token cannot say it runs — commenting the step out leaves the filename in
#      the file, MEASURED. The row therefore anchors on every line of that workflow naming this script
#      and requires the matched line to BE the run step, which `git grep -n`'s own
#      `path:lineno:text` prefix makes expressible.
# Q43  is the count-free LIST in `docs/PROCESS.md` §3 naming this script — the enumeration that makes
#      the whole D-range cascade self-checking. It anchors on the LIST's own entry, the line pairing
#      this filename with `RANGE_RE`, and requires the `RANGE_ALT_RE` half beside it. A bare token is
#      satisfied by §3's other mention of this script, so dropping the entry leaves it green —
#      MEASURED. Anchoring on the entry rather than on the list's last line keeps the row correct the
#      day a D52 entry is appended after it.
# Q44  is the `d48` HALF of the SIBLING ROW TEXT clause (14) corrects, and it is the only executable
#      pin that clause has. `d48`'s `P9` reason string said the premature frame still kills the
#      server, which this decision makes false; nothing else in the tree would notice a later edit
#      restoring that wording. The row anchors on that `P9` row and requires the corrected text, so
#      the restore fails here. The clause's OTHER half — `d49`'s own reason strings at
#      `scripts/checks/d49-startup-failure-render-claims.sh:148` and `:196`, which stop calling an
#      end-to-end cell vacuous the day `ub-kp7` lands — gets no row, because the two rows that read
#      that file (Q34, Q35) read its range knobs.
# =================================================================================================
REQUIRE_ROW="
Q1@crates/unblock-mcp/src/server.rs@VersionClampingTransport::new@PreHandshakeGateTransport::new@the gate is still INSTALLED in the composed transport. Deleting the layer compiles, revives the death this decision removes and leaves every in-module cell green, so the anchor is the surviving outer wrapper and the requirement is the gate's own constructor beside it
Q2@crates/unblock-mcp/src/pre_handshake.rs@^pub\(crate\) struct PreHandshakeGate@pub\(crate\) struct PreHandshakeGateTransport<T>@the decorator type is still declared under its own name, anchored at column 0 so no doc comment naming it can satisfy the row
Q3@crates/unblock-mcp/src/pre_handshake.rs@^impl<T> Transport<RoleServer> for@PreHandshakeGateTransport<T>@the type is still the Transport impl, which is the half that makes it a layer — a struct that lost its impl is no longer in the stack at all
Q4@crates/unblock-mcp/src/pre_handshake.rs@^fn classify_pre_handshake_frame@fn classify_pre_handshake_frame\(message: &RxJsonRpcMessage<RoleServer>\) -> Disposition@the receive-side classifier is still declared with its rmcp signature. A rename keeping the prefix matches the anchor and fails the requirement, and any other rename or a deletion makes the anchor vanish
Q5@crates/unblock-mcp/src/pre_handshake.rs@^fn completes_the_handshake@fn completes_the_handshake\(item: &TxJsonRpcMessage<RoleServer>\) -> bool@the send-side latch predicate is still declared with its rmcp signature, both ways of losing it failing for the same two reasons as Q4
Q6@crates/unblock-mcp/src/pre_handshake.rs@^ +\| JsonRpcMessage::Response@\| JsonRpcMessage::Response\(_\)@the classifier's variant match is still EXHAUSTIVE with no wildcard arm. A fifth rmcp frame variant must be a COMPILE error rather than a silent hole, and a wildcard makes this anchor vanish, which this table counts as a failure
Q7@crates/unblock-mcp/src/pre_handshake.rs@^ +matches!\(response\.result@ServerResult::InitializeResult\(_\)@the latch still keys on the InitializeResult VARIANT. Keying it on the protocol version string desynchronises this layer from the clamp that wraps it, and leaves the latch shut on a supported non-latest result
Q8@crates/unblock-mcp/src/pre_handshake.rs@^const PRE_HANDSHAKE_REJECTION_MESSAGE@the server has not completed the initialize handshake and accepts only initialize and ping until it has@the reply message is still the compile-time sentence D50 clause 2 makes normative. The cells compare against their own copy, so a coordinated edit of both would stay green without this row
Q9@crates/unblock-mcp/src/pre_handshake.rs@^ +async fn a_premature_request_is_answered@async fn a_premature_request_is_answered_on_its_own_id_and_dropped\(\)@the premature-Request cell is still declared under its own name — it is the one shape that writes bytes, answered on the frame's own id and dropped
Q10@crates/unblock-mcp/src/pre_handshake.rs@^ +async fn a_premature_notification_is_dropped@async fn a_premature_notification_is_dropped_and_leaves_the_latch_shut\(\)@the premature-Notification cell is still declared under its own name — the cheapest kill an unauthenticated peer had, now dropped with no reply
Q11@crates/unblock-mcp/src/pre_handshake.rs@^ +async fn a_premature_response_frame_is_dropped@async fn a_premature_response_frame_is_dropped_with_no_reply\(\)@the premature-Response cell is still declared under its own name — a reply to a reply is meaningless, so this shape writes nothing
Q12@crates/unblock-mcp/src/pre_handshake.rs@^ +async fn a_premature_error_frame_is_dropped@async fn a_premature_error_frame_is_dropped_with_no_reply\(\)@the premature-Error cell is still declared under its own name, the fourth shape of the class D50 clause 1 defines
Q13@crates/unblock-mcp/src/pre_handshake.rs@^ +async fn a_ping_then_an_initialize@async fn a_ping_then_an_initialize_both_pass_through\(\)@the ping-then-initialize cell is still declared under its own name — ping is the single pre-handshake exception and both frames must reach rmcp
Q14@crates/unblock-mcp/src/pre_handshake.rs@^ +async fn the_latch_opens_on_the_initialize_result@async fn the_latch_opens_on_the_initialize_result\(\)@the latch-opening cell is still declared under its own name — the gate opens on the server's own InitializeResult and on nothing else
Q15@crates/unblock-mcp/src/pre_handshake.rs@^ +async fn a_supported_non_latest_initialize_result@async fn a_supported_non_latest_initialize_result_still_opens_the_latch\(\)@the supported-but-non-latest cell is still declared under its own name — it is the cell a latch keyed on the protocol version string turns red, which is why D50 clause 4 names it
Q16@crates/unblock-mcp/src/pre_handshake.rs@^ +async fn the_ping_reply_leaves_the_latch_shut@async fn the_ping_reply_leaves_the_latch_shut\(\)@the EmptyResult cell is still declared under its own name — the pre-handshake ping reply must leave the gate shut, or a ping-first client opens it for the whole class
Q17@crates/unblock-mcp/src/pre_handshake.rs@^ +async fn a_request_arriving_before_the_initialized_notification@async fn a_request_arriving_before_the_initialized_notification_is_served\(\)@the post-response cell is still declared under its own name — it pins that the gate never waits for notifications initialized, which rmcp does not wait for either
Q18@crates/unblock-mcp/src/pre_handshake.rs@^ +async fn a_method_initialize_frame_with_untypeable_params@async fn a_method_initialize_frame_with_untypeable_params_is_answered_and_dropped\(\)@the untypeable-params cell is still declared under its own name — it is what tells a variant-matched classifier from a method-string one, since rmcp decodes that frame as CustomRequest
Q19@crates/unblock-mcp/src/pre_handshake.rs@^ +async fn a_failed_reply_write_ends_the_receive@async fn a_failed_reply_write_ends_the_receive\(\)@the failed-write cell is still declared under its own name — a reply that cannot be written ends the read with None, which is D47's answer_error contract and reaches D40's teardown delegation
Q20@crates/unblock-mcp/src/server.rs@^ +async fn a_d47_frame_before_the_handshake@async fn a_d47_frame_before_the_handshake_still_gets_its_recovered_id_answer\(\)@the composition cell is still declared under its own name — it drives a real duplex through the server entry point and is the only cell that proves the gate runs ABOVE the scanner, which D50 clause 3 makes a correctness requirement
Q21@crates/unblock-cli/tests/mcp_lifecycle.rs@^fn an_id_less_notification_before_initialize@fn an_id_less_notification_before_initialize_is_dropped_and_the_handshake_still_completes\(\)@the INVERTED cell carries its new name. It asserted the fatality as correct and now asserts the handshake completing, so the old name surviving here would mean the inversion never landed
Q22@crates/unblock-cli/tests/mcp_lifecycle.rs@^fn a_no_signal_run_loop_error@fn a_no_signal_run_loop_error_exits_1_and_never_hangs\(\)@the REPOINTED cell keeps the name it shipped with, the one row here that also matched the pre-fix tree. It is the witness for the last unsignalled run-loop error still reachable from the wire, a broken pipe on the pre-handshake ping reply
Q23@crates/unblock-cli/tests/common/mod.rs@^ +pub fn close_stdout@pub fn close_stdout\(&mut self\)@the stdout-closing harness method is still declared. It is the only method that closes the READ end of the child's stdout, and its two neighbours hold that end open in a drain thread, so a revert to either leaves the repointed cell provoking nothing
Q24@crates/unblock-mcp/src/options.rs@^pub const CONTRACT_VERSION@$CONTRACT_RE@D50 mints no ErrorCode and bumps NO contract — the -32600 is rmcp's own transport-level code and appears in neither capabilities() nor schema_bundle(). An unstated 'we didn't bump' is indistinguishable from an oversight, so it is stated here
Q25@scripts/checks/d44-create-deps-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D44 sibling's live-range knob carries the SAME range as this script — anchored on the knob line, so the file's own prose about the knob cannot satisfy the pin
Q26@scripts/checks/ub-lp9.25-dangling-blocker-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D45 sibling's live-range knob, same anchoring and same reason
Q27@scripts/checks/ub-lp9.25-dangling-blocker-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, which is a separate line and therefore a separate way to be half-bumped
Q28@scripts/checks/d46-schema-migration-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D46 sibling's live-range knob, same anchoring and same reason
Q29@scripts/checks/d46-schema-migration-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and THAT sibling's ALTERNATION knob, a separate way the enumeration can be half-bumped
Q30@scripts/checks/d47-envelope-id-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D47 sibling's live-range knob, same anchoring and same reason
Q31@scripts/checks/d47-envelope-id-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, same reason again
Q32@scripts/checks/d48-stdout-channel-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D48 sibling's live-range knob, same anchoring and same reason
Q33@scripts/checks/d48-stdout-channel-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, the same pairing every sibling with two knobs gets here
Q34@scripts/checks/d49-startup-failure-render-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D49 sibling's live-range knob — the PREVIOUS newest script, which by the self-row rule pins everyone except itself and so needs this row
Q35@scripts/checks/d49-startup-failure-render-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, the other of the two knobs d49 cannot pin itself
Q36@CLAUDE.md@^\| .docs/PRD\.md. \| Product truth@$RANGE_RE@PROSE D-range bump site, LOCATED on the document-map row that states the range
Q37@docs/plans/ci-cd-and-distribution.md@\*\*\(a\) D-id coherence\*\*@$RANGE_RE@PROSE D-range bump site, LOCATED on the class-(a) statement — the ONE place in that file allowed to quote the live range
Q38@xtask/src/doc_lint.rs@Spec tokenizes@$RANGE_RE@PROSE D-range bump site, half 1 of 2: the PROSE range on the tokenizer comment line
Q39@xtask/src/doc_lint.rs@Spec tokenizes@$RANGE_ALT_RE@…half 2 of 2: the TOKENIZER ALTERNATION on that same line
Q40@docs/roadmap.html@^ +<li>Fix: a first frame that is neither@D50@the RENDERED roadmap's v1.0.1 card still lists D50 on its own bullet — that file is OUTSIDE the 19-file doc-lint corpus, so nothing else in CI can catch a card missing a fix
Q41@docs/plans/ci-cd-and-distribution.md@Named sub-check \(its D50 sibling@d50-pre-handshake-gate-claims@this gate is SPECIFIED in its own paragraph rather than merely named in passing — §2.1(a) of that same file also carries this filename, so a bare token over the file stays green with the whole specification paragraph deleted
Q42@.github/workflows/ci.yml@d50-pre-handshake-gate-claims@:[0-9]+: +- run: scripts/checks/d50-pre-handshake-gate-claims\.sh@this gate actually RUNS in the required doc-lint job. Every line of that workflow naming this script must BE the run step, so commenting the step out fails the row where a bare token stays green
Q43@docs/PROCESS.md@d50-pre-handshake-gate-claims\.sh. .RANGE_RE@RANGE_ALT_RE@the count-free LIST that IS the rule carries this script's own entry with BOTH knob names, so the enumeration cannot rot silently. Anchored on the entry itself, because that file names this script elsewhere too and a bare token survives the entry's deletion
Q44@scripts/checks/d48-stdout-channel-claims.sh@^P9.*ub-kp7@CLOSED by D50@the D48 sibling's P9 reason string carries the correction D50 clause 14 mandates. That row said the premature frame still kills the server, which this decision makes false, so an edit restoring the retired wording goes red here — the only executable pin clause 14 has
"

blocked=0

# PORTABILITY (1 of 2), deliberate. Every `$( … )` in this file substitutes a FUNCTION CALL, never an
# inline loop containing a `case`. macOS `/bin/sh` (bash 3.2) mis-parses a `case` arm's `)` inside
# `$( )` and silently produces garbage instead of failing — this script would then "pass" vacuously on
# a developer machine. Every sibling avoids it the same way.
check_landings() {
  printf '%s\n' "$REQUIRE" | while IFS='@' read -r code path re reason; do
    [ -n "$code" ] || continue
    if [ ! -f "$path" ]; then
      printf '%s\n' "$path: [$code] REQUIRED D50 target is missing from the tree ($reason)"
      continue
    fi
    git grep -q -I -E "$re" -- "$path" 2>/dev/null \
      || printf '%s\n' "$path: [$code] the D50 landing is GONE — no line matches /$re/ ($reason)"
  done
  printf '%s\n' "$REQUIRE_ROW" | while IFS='@' read -r code path anchor re reason; do
    [ -n "$code" ] || continue
    if [ ! -f "$path" ]; then
      printf '%s\n' "$path: [$code] REQUIRED D50 target is missing from the tree ($reason)"
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
  say "BLOCKED — the D50 gate or its cascade is incomplete; the sites above are missing. A wildcard arm replacing the four-variant match, a dangling residual id and a roadmap card missing its fix are the cases nothing else in the tree goes red for. A decorator dropped from the composed transport turns the stdio lifecycle cells and workspace clippy red as well. This gate catches that one in seconds, where the stdio suite spawns a child per cell, and it still catches it when the type stays constructed elsewhere and clippy falls silent."
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
check_table_floor 'required-landing (P)' "$p_count" 7 || blocked=1
check_table_floor 'row-anchored (Q)' "$q_count" 44 || blocked=1

[ "$blocked" = "0" ] || exit 1
say "OK — the gate is still installed in the composed transport, its type, its Transport impl, its classifier and its latch predicate are all still declared as items, the variant match is still exhaustive and the latch still keys on InitializeResult, the reply sentence is still the shipped one, every in-module cell and the composition cell are still declared under their own names, the inverted and repointed lifecycle cells and the stdout-closing harness method they need are still there, the tracker names ub-kp7 and all six residual ids the PRD row cites, the rendered roadmap's v1.0.1 card still lists the decision, the D48 sibling's P9 reason string still carries the correction clause 14 mandates, this gate is both specified and wired, and the live D-range is current at every prose site and every sibling script knob the PROCESS.md §3 list enumerates while the contract version stands unmoved."
exit 0
