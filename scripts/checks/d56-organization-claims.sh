#!/bin/sh
# d56-organization-claims.sh — the REQUIRED-LANDING gate for D56, THE FR-6 REMAINDER: a label RENAME
# and a label CATALOG, an EPIC ROLLUP READ and an EXPLICIT close of every eligible epic, shipped as
# four new arms on the existing `issue` and `query` tools (PRD §4 D56, tracked as `ub-lp9.1`). Spec:
# docs/plans/ci-cd-and-distribution.md §2.1, the paragraph beginning "Named sub-check (its D56
# sibling…)". Runs as a step of the required `doc-lint` job, immediately after its `d55` sibling.
#
# THAT PARAGRAPH IS NORMATIVE OVER THIS FILE. Every landing enforced here is named there; a rule that
# exists in one and not the other is a defect to be fixed in the SAME change. Do not "tidy" a row away
# as unspecified — read the spec paragraph first.
#
# WHY THIS ONE IS POSITIVE-ONLY. Every behaviour D56 adds is a cell: the storage contract cells, the
# libsql committed-mutation counter, the `organization_` engine cells, the MCP cells and the
# `capabilities`/`schema_bundle` goldens with `CONTRACT_HASH`. What no test carries is the mechanism's
# own survival, the documentation edits this decision makes across the tree, and the published
# artefacts outside every lint corpus (`README.md`'s tool table and `docs/roadmap.html`). A negative
# prose sweep is dodged by a claim REWRAPPED across two lines, so every row is a positive landing.
# The ONE NEGATIVE row (Q28, no non-comment line under `crates/unblock-storage/src/libsql/` spells
# `OR IGNORE`, case-insensitively) is a code rule, not a prose sweep: the merge-before-rename rule of
# D56 clause (5) must not be replaced by an ignore-on-conflict insert. The strict label INSERT is the
# spine §3.2.1 tripwire, and no line test can prove the ORDER of the merge DELETE and the rename —
# the `contract_rename_label_` merge cell does.
#
# WHAT NO ROW HERE CAN SEE, stated so no row is over-read. A line grep cannot prove that
# `close_issues` reuses `update_issue`'s in-transaction body rather than a second copy, that the
# cascade set is computed under the guard, that `close_issues` skips an already-closed row before the
# shared body, that `RecordingStorage` records the two new mutations, that a merged issue gets no
# `LabelAdded`, or that a cell RUNS rather than is `#[ignore]`d. Those are behaviour, and the contract
# and engine cells carry them. This file carries NO structural check.
#
# SEQUENCING, the same discipline every sibling states. The prose that NAMES this script rides the
# D56 SPEC commit (the ci-cd paragraph, §2.1(a)'s enumeration, `docs/PROCESS.md` §3's list entry and
# NEWEST pointer, and the seven retargeted pointer rows), so the specification leads the script. The
# IMPLEMENTATION commit carries the code, the doc comments, the contract bump with its re-blessed
# goldens, README and every sibling's `CONTRACT_RE` knob; the TEST commit carries the new cells. This
# script and its workflow step belong to the `ci(d56)` commit that mints the file, after both,
# because the code and cell rows assert lines that do not exist until those commits. The D-range knob
# is the inverse coupling: it guards PROSE the spec commit already moved, so it is LIVE from this
# file's first commit.
#
# NO ROW IS KNOWN-RED ON THE COMMIT THAT ADDS THIS FILE. `ub-lp9.1` already has a record in
# `.unblock/issues.jsonl` on `main`, so no tracker re-export is needed before this file exists. Every
# row pins a landing of the spec, implementation, test or `ci(d56)` commit — none pins the issue's
# CLOSED state, the run-report, the pull-request number or any other Track-step landing. Rows pin
# PRESENCE, never state, and the export keeps closed rows. D56 names `ub-bnb` (bd's open-children
# close guard) as OUT of scope, and no row pins it.
#
# PRE-FIX TREE: `main` at b73749a, the tree before D56's spec commit. Every row asserting a NEW
# landing FAILS there. Exactly two rows pass, by design, because they pin what already holds: P1
# (`ub-lp9.1` is already exported) and the negative row Q28 (no non-comment libsql line spells
# `OR IGNORE` today).
#
# THE CONTRACT KNOB MOVES HERE, stated affirmatively. D56 adds four arms and two descriptions to the
# published documents, so `unblock.mcp.v1.10` becomes `unblock.mcp.v1.11` in the implementation
# commit (spine §5.4 D56 entry), and this file pins the LIVE id from its first commit.
#
# TWO RULE KINDS
#   P-n   REQUIRED landing — a presence predicate over the GENERATED export `.unblock/issues.jsonl`.
#   Q-n   ROW-ANCHORED landing — at least ONE line must match the anchor, and EVERY line matching the
#         anchor must ALSO match the requirement. A vanished anchor is a FAILURE, never a pass, except
#         for the negative row Q28, where no `OR IGNORE` line at all is the pass. Every code anchor
#         starts `^` followed by the line's own indentation and then code, so no `///` or `//!` line
#         can satisfy it; in every production file that carries a `mod tests {`, the production rows
#         read only the lines before it, so no test body can satisfy them either (a duplicated or
#         missing `mod tests {` line in such a file is exit 2).
#
# Exit: 0 = pass · 1 = BLOCK (a required landing is missing) · 2 = cannot evaluate (fail-closed).
set -u

# PORTABILITY (2 of 2). Every variable expansion goes through `printf`, never `echo`. POSIX-mode `echo`
# interprets backslash escapes, so a `\b` inside a regex literal becomes a BACKSPACE byte and the
# pattern silently stops matching. Not a style choice.
say() { printf 'd56-claims: %s\n' "$*" >&2; }

git rev-parse --show-toplevel >/dev/null 2>&1 || { say "not a git repository"; exit 2; }
cd "$(git rev-parse --show-toplevel)" || { say "cannot cd to the repo root"; exit 2; }

# The LIVE D-id range, in its TWO spellings. It tracks the LIVE range, never a frozen historical one:
# the day a D58 is minted, every file `docs/PROCESS.md` §3 enumerates moves with it or a required step
# goes red. §3 deliberately states that cascade as a LIST WITH NO COUNT — a derived count rotted there
# five times — and the Q rows below are what make the list self-checking.
#
# WHY TWO SPELLINGS. `xtask/src/doc_lint.rs`'s bump site is ONE physical line carrying BOTH halves, the
# prose range `(D1..D57)` and the tokenizer's regex ALTERNATION `\bD(57|56|55|…)\b`. Pinning only the
# prose is exactly how that site rots into an undefined-D58 finding — the lint would stop tokenizing
# the id it is being told exists.
RANGE_RE='D1\.\.D57'
RANGE_ALT_RE='D\(57\|56\|'

# The LIVE published contract version. D56 MOVES it (`unblock.mcp.v1.10` -> `unblock.mcp.v1.11`), so
# this knob is LIVE from this file's first commit. It tracks the LIVE id, so a later decision's bump
# moves it in that decision's implementation commit.
CONTRACT_RE='unblock\.mcp\.v1\.12'

# The SAME spellings as they appear INSIDE a sibling script's knob line, where each backslash is a
# literal byte rather than a regex operator. DERIVED, never hand-written a second time. `printf '%s'`
# never interprets its ARGUMENT; `sed` then turns each `\` into `\\\`, i.e. ERE for "a literal
# backslash followed by the escaped char".
knob_re() { printf '%s' "$1" | sed 's/\\/\\\\\\/g'; }
RANGE_KNOB_RE="$(knob_re "$RANGE_RE")"
RANGE_KNOB_ALT_RE="$(knob_re "$RANGE_ALT_RE")"
CONTRACT_KNOB_RE="$(knob_re "$CONTRACT_RE")"

# The negative row's requirement, held in a variable for the reason d47's Q12 gives (`$@` in the
# double-quoted `@`-separated tables below would expand to the positional parameters). It runs over
# `git grep -n` output (`path:lineno:text`), so it spells the prefix before the comment marker: a
# comment line is one whose first non-blank characters are `//`.
COMMENT_LINE_RE='^[^:]+:[0-9]+:[[:space:]]*//'
# `OR IGNORE` in any letter case, as a word: no `-i` flag is needed, and `error ignored` cannot match.
OR_IGNORE_RE='(^|[^[:alnum:]_])[Oo][Rr][[:space:]]+[Ii][Gg][Nn][Oo][Rr][Ee]'

# =================================================================================================
# REQUIRED LANDINGS — `code@path@regex@what it proves`
#
# P1 is the work's own id. D56 names `ub-bnb` as OUT of scope, and no row pins it.
# =================================================================================================
REQUIRE="
P1@.unblock/issues.jsonl@\"id\":\"ub-lp9\.1\"@the tracker record names the work D56 implements. The export keeps closed rows, so the row survives the merge-time close
"

# =================================================================================================
# ROW-ANCHORED LANDINGS — `code@path@anchor@regex@what it proves`
#
# Q1..Q27 are THE MECHANISM, each anchored on its own production line: the three unblock-model
#      declarations, the three Storage trait declarations, the two src/epic.rs functions and their
#      CALL in diagnostics.rs, the four Session methods with the close_reason literal, the four input
#      arms, the four output arms, the epic_ids field, and BOTH copies of each new tool description.
#      Where the requirement merely restates the anchor it is said plainly — the anchor is the teeth.
# Q28 is the NEGATIVE row: every line under crates/unblock-storage/src/libsql/ spelling OR IGNORE is
#      a comment.
# Q29..Q51 are the cells, each on its own declaration: the nine storage contract cells, their nine
#      runner registrations, the libsql committed-mutation-counter cell and the organization_ engine
#      cells.
# Q52..Q55 are the CONTRACT_RE knob's sites: CONTRACT_VERSION, the public_api.rs pin, README.md and
#      the unblock-mcp plan's declaring row.
# Q56..Q75 are the SIBLING SCRIPTS' knobs — the NEWEST script pins the OLDER ones, and THIS script's
#      own knobs have NO row, deliberately: they are the REFERENCE the other rows are compared
#      against, and a self-row could never fail. Q57 is d44's CONTRACT_RE knob, which ub-lp9.25's RC6
#      also pins. Q74/Q75 are the marginal pair: d55's two knobs were pinned by nothing in the tree
#      until this script landed.
# Q76..Q79 are the THREE PROSE bump sites over FOUR rows.
# Q80..Q97 are the normative and published texts: the D56 row and the [D56] notes on FR-6, FR-20 and
#      the PRD §7 EpicStatus row; the spine's §3.2.1 paragraph, its two EventType rows, the §4.1
#      organization block and the §5.4 ledger entry; the engine, storage and mcp crate plans; both
#      roadmaps; README's tool-table lines; the T4.1 task.
# Q98..Q102 are this gate's wiring: SPECIFIED, RUNNING, LISTED in PROCESS.md §3, and in ci-cd §2.1(a);
#      Q101 pins PROCESS.md §3's newest-reference sentence naming the NEWEST gate, which pins this
#      script's knobs (retargeted when D57 took the pointer), and Q102 no longer requires §2.1(a) to call
#      this script the newest.
# Q103..Q109 are the SIBLING ROWS the D56 spec commit retargeted to this script: d53's Q53, d54's Q67
#      and Q72, and d55's Q78, Q80, Q81 and Q82 — and which the D57 spec commit retargeted again, to
#      D57's gate.
# =================================================================================================
REQUIRE_ROW="
Q1@crates/unblock-model/src/results.rs@^pub struct LabelCount \{@pub struct LabelCount \{@the label-catalog entry is still declared; the anchor is the teeth
Q2@crates/unblock-model/src/results.rs@^pub struct LabelRename \{@pub struct LabelRename \{@the rename result is still declared; the anchor is the teeth
Q3@crates/unblock-model/src/results.rs@^pub struct CloseEligibleOutcome \{@pub struct CloseEligibleOutcome \{@the close_eligible result is still declared; the anchor is the teeth
Q4@crates/unblock-storage/src/trait_def.rs@^    async fn list_all_labels\(@async fn list_all_labels\(@the Storage trait still declares the label catalog read
Q5@crates/unblock-storage/src/trait_def.rs@^    async fn rename_label\(@async fn rename_label\(@the Storage trait still declares the one-transaction label rename
Q6@crates/unblock-storage/src/trait_def.rs@^    async fn close_issues\(@async fn close_issues\(@the Storage trait still declares the one-transaction close of a given id list
Q7@crates/unblock-engine/src/epic.rs@^pub\(crate\) fn is_eligible_for_close\(@pub\(crate\) fn is_eligible_for_close\(@the ONE epic eligibility predicate is still declared in its own module
Q8@crates/unblock-engine/src/epic.rs@^pub\(crate\) fn cascade_close_set\(@pub\(crate\) fn cascade_close_set\(@the ONE nested-epic cascade over that predicate is still declared
Q9@crates/unblock-engine/src/diagnostics.rs@^ +crate::epic::is_eligible_for_close\(@crate::epic::is_eligible_for_close\(@stats.epics_eligible still CALLS the shared predicate, so the inline filter cannot come back
Q10@crates/unblock-engine/src/session/organization.rs@^    pub async fn list_all_labels\(@pub async fn list_all_labels\(@the Session label-catalog read is still declared
Q11@crates/unblock-engine/src/session/organization.rs@^    pub async fn rename_label\(@pub async fn rename_label\(@the Session label rename is still declared
Q12@crates/unblock-engine/src/session/organization.rs@^    pub async fn epic_statuses\(@pub async fn epic_statuses\(@the Session epic rollup read is still declared
Q13@crates/unblock-engine/src/session/organization.rs@^    pub async fn close_eligible\(@pub async fn close_eligible\(@the Session explicit close of eligible epics is still declared
Q14@crates/unblock-engine/src/session/organization.rs@^const CLOSE_ELIGIBLE_REASON: &str =@\"All children completed\";@every epic close_eligible closes still carries the bd-parity close_reason
Q15@crates/unblock-mcp/src/tools/query.rs@^    Labels \{@Labels \{\},@the query input still carries the parameterless labels arm
Q16@crates/unblock-mcp/src/tools/query.rs@^    Epics \{@Epics \{@the query input still carries the epics arm; the anchor is the teeth
Q17@crates/unblock-mcp/src/tools/issue.rs@^    Relabel \{@Relabel \{@the issue input still carries the relabel arm; the anchor is the teeth
Q18@crates/unblock-mcp/src/tools/issue.rs@^    CloseEligible \{@CloseEligible \{@the issue input still carries the close_eligible arm; the anchor is the teeth
Q19@crates/unblock-mcp/src/tools/output.rs@^    Labels\(@Labels\(LabelList\),@the query output still answers labels with the object-wrapped LabelList
Q20@crates/unblock-mcp/src/tools/output.rs@^    Epics\(@Epics\(EpicStatusList\),@the query output still answers epics with the object-wrapped EpicStatusList
Q21@crates/unblock-mcp/src/tools/output.rs@^    Relabel\(@Relabel\(LabelRename\),@the issue output still answers relabel with LabelRename
Q22@crates/unblock-mcp/src/tools/output.rs@^    CloseEligible\(@CloseEligible\(CloseEligibleOutcome\),@the issue output still answers close_eligible with CloseEligibleOutcome
Q23@crates/unblock-model/src/results.rs@^    pub epic_ids: @pub epic_ids: Vec<String>,@CloseEligibleOutcome still keys its ids epic_ids, so the key can never fall back to the closed key of close's outcome
Q24@crates/unblock-mcp/src/tools/query.rs@^ +description = \"Query issues: @\"Query issues: list, ready, blocked, search, count, stale, labels, or epics\.\"@the query tool description names both new kinds (tool-router copy)
Q25@crates/unblock-mcp/src/resources/capabilities.rs@^ +\"Query issues: @\"Query issues: list, ready, blocked, search, count, stale, labels, or epics\.\"@…and its capabilities() twin says the same
Q26@crates/unblock-mcp/src/tools/issue.rs@^ +description = \"Create, show, update, close, @restore, relabel, or close_eligible issues[.,]@the issue tool description names both new actions (tool-router copy); re-pointed by D57, whose config clause continues the description past issues
Q27@crates/unblock-mcp/src/resources/capabilities.rs@^ +\"Create, show, update, close, @restore, relabel, or close_eligible issues[.,]@…and its capabilities() twin says the same; re-pointed by D57 for the same continuation
Q28@crates/unblock-storage/src/libsql/@$OR_IGNORE_RE@$COMMENT_LINE_RE@NEGATIVE: no non-comment libsql line spells OR IGNORE, so the merge-before-rename rule of D56 clause (5) cannot be replaced by an ignore-on-conflict insert; the strict label INSERT is the spine §3.2.1 tripwire
Q29@crates/unblock-storage/src/testkit.rs@^pub async fn contract_list_all_labels_counts_live_issues_in_byte_order<@pub async fn contract_list_all_labels_counts_live_issues_in_byte_order<@the label-catalog contract cell is still declared
Q30@crates/unblock-storage/src/testkit.rs@^pub async fn contract_rename_label_pair<@pub async fn contract_rename_label_pair<@the rename-pair contract cell is still declared
Q31@crates/unblock-storage/src/testkit.rs@^pub async fn contract_rename_label_merge<@pub async fn contract_rename_label_merge<@the merge contract cell is still declared — the only proof of the merge-DELETE-before-rename ORDER
Q32@crates/unblock-storage/src/testkit.rs@^pub async fn contract_rename_label_tombstone_skip<@pub async fn contract_rename_label_tombstone_skip<@the tombstone-skip contract cell is still declared
Q33@crates/unblock-storage/src/testkit.rs@^pub async fn contract_rename_label_noop<@pub async fn contract_rename_label_noop<@the no-op contract cell is still declared
Q34@crates/unblock-storage/src/testkit.rs@^pub async fn contract_rename_label_missing_label<@pub async fn contract_rename_label_missing_label<@the missing-label contract cell is still declared
Q35@crates/unblock-storage/src/testkit.rs@^pub async fn contract_close_issues_closes_in_given_order<@pub async fn contract_close_issues_closes_in_given_order<@the close-in-given-order contract cell is still declared
Q36@crates/unblock-storage/src/testkit.rs@^pub async fn contract_close_issues_skips_closed_and_tombstoned<@pub async fn contract_close_issues_skips_closed_and_tombstoned<@the skip-closed-and-tombstoned contract cell is still declared
Q37@crates/unblock-storage/src/testkit.rs@^pub async fn contract_close_issues_missing_id_rolls_back<@pub async fn contract_close_issues_missing_id_rolls_back<@the missing-id rollback contract cell is still declared
Q38@crates/unblock-storage/src/testkit.rs@^    contract_list_all_labels_counts_live_issues_in_byte_order\(@contract_list_all_labels_counts_live_issues_in_byte_order\(factory\(\)\.await\)\.await;@the label-catalog cell is REGISTERED in run_storage_contract_suite, so every backend runs it
Q39@crates/unblock-storage/src/testkit.rs@^    contract_rename_label_pair\(@contract_rename_label_pair\(factory\(\)\.await\)\.await;@the rename-pair cell is registered in the runner
Q40@crates/unblock-storage/src/testkit.rs@^    contract_rename_label_merge\(@contract_rename_label_merge\(factory\(\)\.await\)\.await;@the merge cell is registered in the runner
Q41@crates/unblock-storage/src/testkit.rs@^    contract_rename_label_tombstone_skip\(@contract_rename_label_tombstone_skip\(factory\(\)\.await\)\.await;@the tombstone-skip cell is registered in the runner
Q42@crates/unblock-storage/src/testkit.rs@^    contract_rename_label_noop\(@contract_rename_label_noop\(factory\(\)\.await\)\.await;@the no-op cell is registered in the runner
Q43@crates/unblock-storage/src/testkit.rs@^    contract_rename_label_missing_label\(@contract_rename_label_missing_label\(factory\(\)\.await\)\.await;@the missing-label cell is registered in the runner
Q44@crates/unblock-storage/src/testkit.rs@^    contract_close_issues_closes_in_given_order\(@contract_close_issues_closes_in_given_order\(factory\(\)\.await\)\.await;@the close-in-given-order cell is registered in the runner
Q45@crates/unblock-storage/src/testkit.rs@^    contract_close_issues_skips_closed_and_tombstoned\(@contract_close_issues_skips_closed_and_tombstoned\(factory\(\)\.await\)\.await;@the skip-closed-and-tombstoned cell is registered in the runner
Q46@crates/unblock-storage/src/testkit.rs@^    contract_close_issues_missing_id_rolls_back\(@contract_close_issues_missing_id_rolls_back\(factory\(\)\.await\)\.await;@the missing-id rollback cell is registered in the runner
Q47@crates/unblock-storage/src/libsql/mod.rs@^    async fn rename_label_commits_exactly_one_transaction\(@async fn rename_label_commits_exactly_one_transaction\(\)@the libsql cell reading the committed-mutation counter across a rename is still declared
Q48@crates/unblock-engine/tests/organization.rs@^async fn organization_one_predicate_matches_stats_epics_eligible\(@async fn organization_one_predicate_matches_stats_epics_eligible\(\)@the one-predicate equality cell with stats.epics_eligible is still declared
Q49@crates/unblock-engine/tests/organization.rs@^async fn organization_dry_run_holds_permit_and_lock_and_writes_nothing\(@async fn organization_dry_run_holds_permit_and_lock_and_writes_nothing\(\)@the dry-run cell holding the permit and the .write.lock while writing nothing is still declared
Q50@crates/unblock-engine/tests/organization.rs@^async fn organization_cascade_closes_child_before_parent_in_one_run\(@async fn organization_cascade_closes_child_before_parent_in_one_run\(\)@the A to B to leaf cascade cell is still declared
Q51@crates/unblock-engine/tests/organization.rs@^async fn organization_real_close_holds_permit_and_lock\(@async fn organization_real_close_holds_permit_and_lock\(\)@the real-close-under-the-guard cell is still declared
Q52@crates/unblock-mcp/src/options.rs@^pub const CONTRACT_VERSION@$CONTRACT_RE@D56 adds four arms and two descriptions to the published documents, so the code constant carries the LIVE id
Q53@crates/unblock-mcp/tests/public_api.rs@^ +assert_eq!\(CONTRACT_VERSION, \"@$CONTRACT_RE@the second, independent CONTRACT_VERSION pin moved with it
Q54@README.md@unblock\.mcp\.v1\.[0-9]@$CONTRACT_RE@every README line naming a contract id names the LIVE one; README keeps no bump history
Q55@docs/plans/crates/unblock-mcp.md@^\| .pub const CONTRACT_VERSION@\*\*.$CONTRACT_RE.\*\* \(bumped from@the OWNING crate plan's CONTRACT_VERSION row DECLARES the live id as its value; the bump chain after it keeps the retired ones
Q56@scripts/checks/d44-create-deps-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D44 sibling's live-range knob carries the SAME range as this script — anchored on the knob line, so the file's own prose about the knob cannot satisfy the pin
Q57@scripts/checks/d44-create-deps-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@…and that sibling's contract knob carries the SAME live id as this script
Q58@scripts/checks/ub-lp9.25-dangling-blocker-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D45 sibling's live-range knob, same anchoring and same reason
Q59@scripts/checks/ub-lp9.25-dangling-blocker-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, a separate line and so a separate way to be half-bumped
Q60@scripts/checks/d46-schema-migration-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D46 sibling's live-range knob, same anchoring and same reason
Q61@scripts/checks/d46-schema-migration-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q62@scripts/checks/d47-envelope-id-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D47 sibling's live-range knob, same anchoring and same reason
Q63@scripts/checks/d47-envelope-id-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q64@scripts/checks/d48-stdout-channel-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D48 sibling's live-range knob, same anchoring and same reason
Q65@scripts/checks/d48-stdout-channel-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q66@scripts/checks/d49-startup-failure-render-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D49 sibling's live-range knob, same anchoring and same reason
Q67@scripts/checks/d49-startup-failure-render-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q68@scripts/checks/d50-pre-handshake-gate-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D50 sibling's live-range knob, same anchoring and same reason
Q69@scripts/checks/d50-pre-handshake-gate-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q70@scripts/checks/d53-request-integrity-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D53 sibling's live-range knob, same anchoring and same reason
Q71@scripts/checks/d53-request-integrity-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q72@scripts/checks/d54-parse-error-id-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D54 sibling's live-range knob, same anchoring and same reason
Q73@scripts/checks/d54-parse-error-id-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q74@scripts/checks/d55-update-refusal-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D55 sibling's live-range knob — the PREVIOUS newest script, which by the self-row rule pins everyone except itself and so needs this row
Q75@scripts/checks/d55-update-refusal-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, the other of the two knobs d55 cannot pin itself
Q76@CLAUDE.md@^\| .docs/PRD\.md. \| Product truth@$RANGE_RE@PROSE D-range bump site, LOCATED on the document-map row that states the range
Q77@docs/plans/ci-cd-and-distribution.md@\*\*\(a\) D-id coherence\*\*@$RANGE_RE@PROSE D-range bump site, LOCATED on the class-(a) statement — the ONE place in that file allowed to quote the live range
Q78@xtask/src/doc_lint.rs@Spec tokenizes@$RANGE_RE@PROSE D-range bump site, half 1 of 2: the PROSE range on the tokenizer comment line
Q79@xtask/src/doc_lint.rs@Spec tokenizes@$RANGE_ALT_RE@…half 2 of 2: the TOKENIZER ALTERNATION on that same line
Q80@docs/PRD.md@^\| \*\*D56\*\* \|@Tracked as .ub-lp9\.1.@the D56 decision row is in PRD §4 and names the work it tracks
Q81@docs/PRD.md@^- \*\*FR-6 \[v1\.1\] — Organization\.\*\*@\*\*\[D56\] The v1\.1 remainder is specified by D56\*\*@FR-6 carries the reciprocal note naming the decision that specifies its remainder
Q82@docs/PRD.md@^- \*\*FR-20 \[must\] — MCP stdio server@\*\*\[D56\]\*\* label rename/catalog and epic rollups add NO tool@FR-20 carries the correction: the FR-6 remainder rides existing tools, never a new one
Q83@docs/PRD.md@^\| \*\*EpicStatus\*\* \|@\*\*\[D56\]\*\* corrected from \"epic id\"@the PRD §7 EpicStatus row carries the correction to the full embedded Issue
Q84@docs/plans/01-design-spine.md@^\*\*Organization \(D56@three FR-6-remainder methods@spine §3.2.1's normative Organization paragraph is present
Q85@docs/plans/01-design-spine.md@^\| label rename \(.relabel., D56\)@LabelRemoved\(old\).*LabelAdded\(new\)@the EventType-per-mutation table carries the relabel row and its value-carrying pair
Q86@docs/plans/01-design-spine.md@^\| close eligible epics \(.close_eligible., D56\)@StatusChanged. \+ .Closed.@the EventType-per-mutation table carries the close_eligible row
Q87@docs/plans/01-design-spine.md@^ +// --- organization \(FR-6 remainder, D56, v1\.1\) — .src/session/organization\.rs.@\(NEW\)@spine §4.1's organization block is present; the anchor is the teeth
Q88@docs/plans/01-design-spine.md@^\*\*D56 \(v1\.1\) — the contract bumps to@bumps to .unblock\.mcp\.v1\.11.@the spine §5.4 ledger carries the D56 entry
Q89@docs/plans/crates/unblock-engine.md@^\| .src/session/organization\.rs. @\*\*PRD §4 D56\*\*, .ub-lp9\.1.@the engine plan's src/session/organization.rs row names the decision and the work
Q90@docs/plans/crates/unblock-engine.md@^\| .src/epic\.rs. @\*\*PRD §4 D56\*\* clause \(2\)@the engine plan's src/epic.rs row names the decision clause it implements
Q91@docs/plans/crates/unblock-storage.md@^- Trait grows \(additive\):@\*\*\[D56\] — the FR-6 remainder is specified@the storage plan's trait-growth bullet carries the D56 methods
Q92@docs/plans/crates/unblock-mcp.md@^- \*\*OQ-3 \(label rename/list-all placement\)@RESOLVED by PRD §4 D56@the unblock-mcp plan's OQ-3 is RESOLVED by D56
Q93@docs/plans/00-roadmap.md@^\| FR-6 \| \*\*Organization \(v1\.1 remainder\):\*\*@\*\*\[D56\]\*\* Specified by PRD §4 D56 \(.ub-lp9\.1.@the markdown roadmap §2 FR-6 row carries the D56 note and names the tracked work
Q94@docs/roadmap.html@^ +<li>Labels \(rename/list-all\) \+ epic rollups@never automatic \(D56\)@the RENDERED v1.1 card's FR-6 bullet names D56 — that file is OUTSIDE the doc-lint corpus
Q95@README.md@^\| .issue. \|@· .relabel. .*· .close_eligible.@the README tool table's issue line names both new actions — README is OUTSIDE the doc-lint corpus
Q96@README.md@^\| .query. \|@· .labels. .*· .epics.@the README tool table's query line names both new kinds
Q97@docs/plans/implementation-plan.md@^- \*\*T4\.1 — @FR-6 remainder@the task checklist exists; its next line names D56
Q98@docs/plans/ci-cd-and-distribution.md@Named sub-check \(its D56 sibling@d56-organization-claims@this gate is SPECIFIED in its own paragraph — §2.1(a) of that same file also carries this filename, so a bare token stays green with the paragraph deleted
Q99@.github/workflows/ci.yml@d56-organization-claims@:[0-9]+: +- run: scripts/checks/d56-organization-claims\.sh@this gate actually RUNS in the required doc-lint job; every line naming it must BE the run step, so a commented-out step fails
Q100@docs/PROCESS.md@d56-organization-claims\.sh. .RANGE_RE@RANGE_ALT_RE@the count-free LIST that IS the rule carries this script's own entry with BOTH knob names
Q101@docs/PROCESS.md@always the NEWEST script@d57-config-layers-claims\.sh@PROCESS.md §3 names D57's gate as the newest reference — a script that pins THIS one's knobs, which this script cannot pin itself (retargeted when D57 took the pointer; the next mint retargets it again)
Q102@docs/plans/ci-cd-and-distribution.md@\*\*\(a\) D-id coherence\*\*@the .ci\(d56\). commit that mints it, .scripts/checks/d56-organization-claims\.sh. .RANGE_RE. \*\*and\*\* .RANGE_ALT_RE.@ci-cd §2.1(a)'s enumeration carries this script's knobs (it stopped naming this script the newest when D57's gate took the pointer)
Q103@scripts/checks/d53-request-integrity-claims.sh@^Q53@d57-config-layers-claims@the D53 sibling's newest-pointer row names the NEWEST gate (this one until D57 took the pointer)
Q104@scripts/checks/d54-parse-error-id-claims.sh@^Q67@d57-config-layers-claims@the D54 sibling's row pinning d53's Q53 follows the retarget (to D57's gate since D57 took the pointer)
Q105@scripts/checks/d54-parse-error-id-claims.sh@^Q72@d57-config-layers-claims@the D54 sibling's newest-pointer row names the NEWEST gate (this one until D57 took the pointer)
Q106@scripts/checks/d55-update-refusal-claims.sh@^Q78@d57-config-layers-claims@the D55 sibling's newest-pointer row names the NEWEST gate (this one until D57 took the pointer)
Q107@scripts/checks/d55-update-refusal-claims.sh@^Q80@d57-config-layers-claims@the D55 sibling's row pinning d53's Q53 follows the retarget (to D57's gate since D57 took the pointer)
Q108@scripts/checks/d55-update-refusal-claims.sh@^Q81@d57-config-layers-claims@the D55 sibling's row pinning d54's Q67 follows the retarget (to D57's gate since D57 took the pointer)
Q109@scripts/checks/d55-update-refusal-claims.sh@^Q82@d57-config-layers-claims@the D55 sibling's row pinning d54's Q72 follows the retarget (to D57's gate since D57 took the pointer)
"

blocked=0

# PORTABILITY (1 of 2), deliberate. Every `$( … )` in this file substitutes a FUNCTION CALL, never an
# inline loop containing a `case`. macOS `/bin/sh` (bash 3.2) mis-parses a `case` arm's `)` inside
# `$( )` and silently produces garbage instead of failing. Every sibling avoids it the same way.
#
# PRODUCTION PREFIXES. A row on one of the files below reads ONLY the lines before that file's
# `mod tests {`, so no test body can satisfy it. Each of these files carries exactly one such line;
# a missing or duplicated one is fail-closed (exit 2), never a pass. A file absent from the tree is
# not evaluated here: its rows fail as missing targets below.
PROD_FILES='crates/unblock-model/src/results.rs
crates/unblock-storage/src/trait_def.rs
crates/unblock-engine/src/epic.rs
crates/unblock-engine/src/diagnostics.rs
crates/unblock-mcp/src/tools/output.rs
crates/unblock-mcp/src/resources/capabilities.rs
crates/unblock-mcp/src/options.rs'
tests_line() { git grep -n -E '^mod tests \{$' -- "$1" 2>/dev/null | cut -d: -f2; }
is_prod_file() { printf '%s\n' "$PROD_FILES" | grep -q -x -F "$1"; }
for f in $PROD_FILES; do
  [ -f "$f" ] || continue
  n="$(tests_line "$f")"
  case "$n" in
    ''|*[!0-9]*) say "cannot find exactly one '^mod tests {' line in $f"; exit 2 ;;
  esac
done

check_landings() {
  printf '%s\n' "$REQUIRE" | while IFS='@' read -r code path re reason; do
    [ -n "$code" ] || continue
    if [ ! -f "$path" ]; then
      printf '%s\n' "$path: [$code] REQUIRED D56 target is missing from the tree ($reason)"
      continue
    fi
    git grep -q -I -E "$re" -- "$path" 2>/dev/null \
      || printf '%s\n' "$path: [$code] the landing is GONE — no line matches /$re/ ($reason)"
  done
  printf '%s\n' "$REQUIRE_ROW" | while IFS='@' read -r code path anchor re reason; do
    [ -n "$code" ] || continue
    # Q28's target is a DIRECTORY; every other target is one file.
    if [ ! -e "$path" ]; then
      printf '%s\n' "$path: [$code] REQUIRED D56 target is missing from the tree ($reason)"
      continue
    fi
    rows="$(git grep -n -I -E "$anchor" -- "$path" 2>/dev/null)"
    if is_prod_file "$path"; then
      rows="$(printf '%s\n' "$rows" | awk -F: -v end="$(tests_line "$path")" 'NF && $2 + 0 < end + 0')"
    fi
    if [ -z "$rows" ]; then
      # The negative row passes with no `OR IGNORE` line at all; every other row needs its anchor.
      case "$code" in
        Q28) continue ;;
      esac
      printf '%s\n' "$path: [$code] the anchor line /$anchor/ no longer exists, so this pin now proves NOTHING ($reason)"
      continue
    fi
    # `git grep -n` prints `path:lineno:text`, so `path:lineno` is fields 1-2.
    bad="$(printf '%s\n' "$rows" | grep -v -E "$re" | cut -d: -f1,2)"
    if [ -n "$bad" ]; then
      printf '%s\n' "$path: [$code] the anchored line(s) $(printf '%s' "$bad" | tr '\n' ' ') do not match /$re/ ($reason)"
    fi
  done
}

missing="$(check_landings)"
if [ -n "$missing" ]; then
  printf '%s\n' "$missing" >&2
  say "BLOCKED — the D56 organization surface or its cascade is incomplete; the sites above are missing."
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
check_table_floor 'row-anchored (Q)' "$q_count" 109 || blocked=1

[ "$blocked" = "0" ] || exit 1
say "OK — the model results, the Storage trait methods, the shared epic predicate and cascade with the stats call, the four Session methods and the close_reason, the four input and four output arms, the epic_ids key and both description twins are in the tree, no code line under libsql spells OR IGNORE, every contract, libsql and organization_ cell is declared and every contract cell is registered, the tracker names ub-lp9.1, the PRD row and its notes, the spine, the crate plans, both roadmaps, README and the task carry D56, the retargeted sibling rows are in place, this gate is both specified and wired, the contract reads the live id, and the live D-range is current at every prose site and every sibling script's knob"
exit 0
