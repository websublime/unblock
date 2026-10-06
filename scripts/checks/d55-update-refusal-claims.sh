#!/bin/sh
# d55-update-refusal-claims.sh — the REQUIRED-LANDING gate for D55, `unblock update` CLASSIFIES
# GITHUB'S REFUSAL OF THE RELEASE QUERY (PRD §4 D55, tracked as `ub-e47`). Spec:
# docs/plans/ci-cd-and-distribution.md §2.1, the paragraph beginning "Named sub-check (its D55
# sibling…)". Runs as a step of the required `doc-lint` job, immediately after its `d54` sibling.
#
# THAT PARAGRAPH IS NORMATIVE OVER THIS FILE. Every landing enforced here is named there; a rule that
# exists in one and not the other is a defect to be fixed in the SAME change. Do not "tidy" a row away
# as unspecified — read the spec paragraph first.
#
# WHY THIS ONE IS POSITIVE-ONLY. A negative sweep for the retired framing ("every update failure is
# INTERNAL_ERROR") is dodged by a claim REWRAPPED across two lines, and that wording stays CORRECT for
# every failure D55 leaves unchanged (a 404, a 5xx, a tampered download), so it cannot be forbidden
# at all. Every row is a positive landing. The one NEGATIVE row (Q13/Q14, no non-comment line of
# update.rs or exit.rs spells `reqwest`) is a code rule, not a prose sweep: `cargo xtask no-network`
# skips comment lines and exempts `#[cfg(feature = "self-update")]`-gated regions, so it does not
# enforce that rule in these two files.
#
# WHAT NO ROW HERE CAN SEE, stated so no row is read as covering it. A line grep cannot prove that the
# token never reaches the mapper, that the exit code ignores the token state, that `update_error` is
# the only mapper on every `map_err` site, or that a cell RUNS rather than is `#[ignore]`d. Those are
# behaviour, and the cells this file pins by name go red for the first three. Q29 proves the published
# `RateLimited` text is PRESENT, not that it is byte-identical; byte identity is the `CONTRACT_HASH`
# gate's job.
#
# SEQUENCING, the same discipline every sibling states. This script and its workflow step belong to
# the `ci(d55)` commit that mints the file, because the code and cell rows assert lines that do not
# exist until the implementation and test commits. The D-range knob is the inverse coupling: it
# guards PROSE the spec commit already moved, so it is LIVE from this file's first commit.
#
# NO ROW IS KNOWN-RED ON THE COMMIT THAT ADDS THIS FILE. `ub-e47` already has a record in
# `.unblock/issues.jsonl` on `main`, so no tracker re-export is needed before this file exists. Every
# row pins a landing of the spec, implementation, test or `ci(d55)` commit — none pins the issue's
# CLOSED state, the run-report, the pull-request number or any other Track-step landing. Rows pin
# PRESENCE, never state, and the export keeps closed rows.
#
# PRE-FIX TREE: `main` at 03489ec. Every row asserting a NEW landing FAILS there. The ones that pass
# assert an unchanged state on purpose: P1 (`ub-e47` already has a record), Q12 (the unchanged
# `Update` arm), Q26/Q27 (the token-gated and tampered-download cell declarations), Q29 (the published
# `RateLimited` text), Q13/Q14 (no code line spells `reqwest`) and Q31 (the contract version this
# decision does not move).
#
# THE CONTRACT KNOB IS PINNED BUT DOES NOT MOVE HERE. D55 mints no `ErrorCode`, moves no published
# byte and keeps the `RateLimited` description word for word, so `unblock.mcp.v1.10` stands; an
# unstated "we didn't bump" is indistinguishable from an oversight.
#
# TWO RULE KINDS
#   P-n   REQUIRED landing — a presence predicate over the GENERATED export `.unblock/issues.jsonl`.
#   Q-n   ROW-ANCHORED landing — at least ONE line must match the anchor, and EVERY line matching the
#         anchor must ALSO match the requirement. A vanished anchor is a FAILURE, never a pass, except
#         for the negative pair Q13/Q14, where no `reqwest` line at all is the pass. Every code anchor
#         starts `^` or `^ +` followed by code, so no `///` or `//!` line can satisfy it; Q1..Q7 read
#         only update.rs's lines before its `mod tests {` and Q8..Q12 only exit.rs's, so no test body
#         can satisfy them either (a missing or duplicated `mod tests {` line is exit 2).
#
# Exit: 0 = pass · 1 = BLOCK (a required landing is missing) · 2 = cannot evaluate (fail-closed).
set -u

# PORTABILITY (2 of 2). Every variable expansion goes through `printf`, never `echo`. POSIX-mode `echo`
# interprets backslash escapes, so a `\b` inside a regex literal becomes a BACKSPACE byte and the
# pattern silently stops matching. Not a style choice.
say() { printf 'd55-claims: %s\n' "$*" >&2; }

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

# The LIVE published contract version. D55 does NOT move it, so this row is the affirmative record of
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

# Q2's anchor and the negative pair's requirement, held in variables for the reason d47's Q12 gives:
# an END-ANCHOR cannot be written inline in the double-quoted `@`-separated tables below (`$@` expands
# to the positional parameters and eats the anchor). The negative requirement runs over `git grep -n`
# output (`path:lineno:text`), so it spells the prefix before the comment marker.
STATUS_READ_LINE_RE='^ +&& let Some\(status\) = e\.status\(\)\.map\(u16::from\)$'
COMMENT_LINE_RE='^[^:]+:[0-9]+:[[:space:]]*//'

# =================================================================================================
# REQUIRED LANDINGS — `code@path@regex@what it proves`
#
# P1 is the work's own id. D55 names no NEW residual id: its residuals (clause (10)) are disclosed in
# the PRD row and carry no issue.
# =================================================================================================
REQUIRE="
P1@.unblock/issues.jsonl@\"id\":\"ub-e47\"@the tracker record names the work D55 implements. The export keeps closed rows, so the row survives the merge-time close
"

# =================================================================================================
# ROW-ANCHORED LANDINGS — `code@path@anchor@regex@what it proves`
#
# Q1..Q12 are THE MECHANISM, each anchored on its own production line: in update.rs the variant match
#      and the status read inside `update_error`, the declarations of `refusal_for_status`,
#      `refusal_message` and `TokenState`, and the `403 | 429` and `401` arms; in exit.rs the two
#      variant declarations and the three `CliError::code` arms, the `Update` arm UNCHANGED. Where the
#      requirement merely restates the anchor it is said plainly — the anchor is the teeth.
# Q13/Q14 are the NEGATIVE pair: every line of update.rs and exit.rs spelling `reqwest` is a comment.
# Q15..Q27 are the cells, each on its own `fn` declaration: the unit cells over `refusal_for_status`
#      and `refusal_message`, every `update_refusal_` cell, the re-pinned token cell and the UNCHANGED
#      tampered-download cell.
# Q28..Q30 are `unblock-error`: the `//` producer comment, the published `///` text, the test comment.
# Q31 is the contract knob. Q32..Q48 are the SIBLING SCRIPTS' live-range knobs — the NEWEST script
#      pins the OLDER ones, and THIS script's own knob has NO row, deliberately: it is the REFERENCE the
#      other rows are compared against, and a self-row could never fail. Q47/Q48 are the marginal
#      pair: `d54`'s two knobs were pinned by nothing in the tree until this script landed.
# Q49..Q52 are the THREE PROSE bump sites over FOUR rows.
# Q53..Q74 are the normative and published texts: the D55 row and the `[D55]` note on D34, the spine
#      (§2.2, §2.3, the exit boundary), both crate plans, ci-cd §4, `RELEASING.md`, the two smoke
#      headers, the `update-smoke.yml` env comment, `README.md`, both roadmaps and the T3.17 task.
# Q75..Q79 are this gate's wiring: SPECIFIED, RUNNING, LISTED in PROCESS.md §3 with its NEWEST
#      pointer, and in ci-cd §2.1(a).
# Q80..Q82 are the SIBLING ROWS the spec commit retargeted to this script: `d53`'s Q53 and `d54`'s
#      Q67 and Q72.
# Q83 is the tokenless-401 `update_refusal_` cell, a cell row kept at the end so no row is renumbered.
# =================================================================================================
REQUIRE_ROW="
Q1@crates/unblock-cli/src/commands/update.rs@^ +if let AxoupdateError::Reqwest\(e\) = err@if let AxoupdateError::Reqwest\(e\) = err@update_error still matches the HTTP variant DIRECTLY; the transparent variant forwards a None source for a status error, so source() cannot reach the status. The anchor is the teeth
Q2@crates/unblock-cli/src/commands/update.rs@$STATUS_READ_LINE_RE@e\.status\(\)\.map\(u16::from\)@the status is still read as a plain number and handed on; the anchor is the teeth
Q3@crates/unblock-cli/src/commands/update.rs@^(const )?fn refusal_for_status\(@fn refusal_for_status\(status: u16\) -> Option<Refusal>@the pure status classifier is still declared, so the mapping stays unit-testable without a reqwest dev-dependency
Q4@crates/unblock-cli/src/commands/update.rs@^fn refusal_message\(@fn refusal_message\(refusal: Refusal, token_state: TokenState, status: u16, url: &str\) -> String@the pure message function still takes the refusal, the token STATE, the status and the URL, and never the token text
Q5@crates/unblock-cli/src/commands/update.rs@^enum TokenState \{@enum TokenState \{@the two-valued token state is still declared; the anchor is the teeth
Q6@crates/unblock-cli/src/commands/update.rs@^ +403 [|] 429 =>@=> Some\(Refusal::RateLimited\),@a 403 or 429 still classifies as a rate limit. Mutant: dropping 429, which GitHub documents for a secondary limit
Q7@crates/unblock-cli/src/commands/update.rs@^ +401 =>@=> Some\(Refusal::Unauthorized\),@a 401 still classifies as a rejected token, never a rate limit
Q8@crates/unblock-cli/src/exit.rs@^ +UpdateRateLimited \{@UpdateRateLimited \{@the rate-limited variant is still declared; the anchor is the teeth
Q9@crates/unblock-cli/src/exit.rs@^ +UpdateUnauthorized \{@UpdateUnauthorized \{@the unauthorized variant is still declared; the anchor is the teeth
Q10@crates/unblock-cli/src/exit.rs@^ +Self::UpdateRateLimited \{ \.\. \} =>@Self::UpdateRateLimited \{ \.\. \} => ErrorCode::RateLimited,@a 403 or 429 still renders RATE_LIMITED, exit 2, retryable
Q11@crates/unblock-cli/src/exit.rs@^ +Self::UpdateUnauthorized \{ \.\. \} =>@Self::UpdateUnauthorized \{ \.\. \} => ErrorCode::ConfigError,@a 401 still renders CONFIG_ERROR, exit 7, not retryable
Q12@crates/unblock-cli/src/exit.rs@^ +Self::Update \{ \.\. \} =>@Self::Update \{ \.\. \} => ErrorCode::InternalError,@every other update failure stays INTERNAL_ERROR, exit 1. Unchanged on purpose: it passes on the pre-fix tree
Q13@crates/unblock-cli/src/commands/update.rs@reqwest@$COMMENT_LINE_RE@NEGATIVE: no non-comment line of update.rs spells reqwest; cargo xtask no-network skips comments and exempts self-update-gated regions, so it does not enforce this here
Q14@crates/unblock-cli/src/exit.rs@reqwest@$COMMENT_LINE_RE@NEGATIVE: no non-comment line of exit.rs spells reqwest, for the same reason
Q15@crates/unblock-cli/src/commands/update.rs@^ +fn refusal_for_status_classifies_401_403_429_only@fn refusal_for_status_classifies_401_403_429_only\(\)@the status truth-table cell is still declared
Q16@crates/unblock-cli/src/commands/update.rs@^ +fn refusal_message_renders_the_four_d55_texts@fn refusal_message_renders_the_four_d55_texts\(\)@the four-text cell is still declared — the only coverage of the tokenless-401 text, which needs a GitHub Enterprise override end to end
Q17@crates/unblock-cli/src/commands/update.rs@^ +fn refusal_message_names_the_env_on_one_line@fn refusal_message_names_the_env_on_one_line_and_tells_set_from_unset\(\)@the env-named, one-line, set-differs-from-unset cell is still declared
Q18@crates/unblock-cli/tests/update_verify.rs@^async fn update_refusal_403_unset_is_rate_limited\(@async fn update_refusal_403_unset_is_rate_limited\(\)@the anonymous 403 cell is still declared
Q19@crates/unblock-cli/tests/update_verify.rs@^async fn update_refusal_403_blank_reads_as_unset\(@async fn update_refusal_403_blank_reads_as_unset\(\)@the blank and whitespace token cell is still declared
Q20@crates/unblock-cli/tests/update_verify.rs@^async fn update_refusal_403_set_is_rate_limited@async fn update_refusal_403_set_is_rate_limited_and_names_the_token_env\(\)@the token-set 403 cell is still declared
Q21@crates/unblock-cli/tests/update_verify.rs@^async fn update_refusal_429_unset@async fn update_refusal_429_unset_is_rate_limited_with_the_whole_message\(\)@the anonymous 429 cell, which pins the WHOLE message, is still declared
Q22@crates/unblock-cli/tests/update_verify.rs@^async fn update_refusal_401_set@async fn update_refusal_401_set_is_config_error_with_the_whole_message\(\)@the token-set 401 cell, which pins the WHOLE message, is still declared
Q23@crates/unblock-cli/tests/update_verify.rs@^async fn update_refusal_404_set@async fn update_refusal_404_set_stays_internal_error\(\)@the 404 boundary cell is still declared — the one new cell green on the pre-fix tree, by design
Q24@crates/unblock-cli/tests/update_verify.rs@^async fn update_refusal_403_unset_plain@async fn update_refusal_403_unset_plain_is_one_stderr_line\(\)@the -o plain cell is still declared
Q25@crates/unblock-cli/tests/update_verify.rs@^async fn update_refusal_403_set_on_a_real_run@async fn update_refusal_403_set_on_a_real_run_is_rate_limited_and_swaps_nothing\(\)@the real-run cell is still declared, so --dry-run and a real run classify alike
Q26@crates/unblock-cli/tests/update_verify.rs@^async fn update_dry_run_authenticates_with_the_github_token_env\(@async fn update_dry_run_authenticates_with_the_github_token_env\(\)@the re-pinned token cell is still declared
Q27@crates/unblock-cli/tests/update_verify.rs@^async fn update_rejects_a_tampered_download_and_swaps_nothing\(@async fn update_rejects_a_tampered_download_and_swaps_nothing\(\)@the UNCHANGED tampered-download cell is still declared, still INTERNAL_ERROR
Q28@crates/unblock-error/src/code.rs@^ +// D55: the CLI also emits this code@D55@the plain comment beside RateLimited records the CLI producer, which nothing publishes
Q29@crates/unblock-error/src/code.rs@^ +/// The MCP request-rate cap fired@The MCP request-rate cap fired@the published RateLimited description is still PRESENT; byte identity is the CONTRACT_HASH gate's job
Q30@crates/unblock-error/tests/exit_code_table.rs@^ +// Two producers emit it@\(D34, D55\)@the rate_limited_is_exit_two_and_retryable comment names both producers
Q31@crates/unblock-mcp/src/options.rs@^pub const CONTRACT_VERSION@$CONTRACT_RE@D55 mints no ErrorCode and moves no published byte, so the contract stands; an unstated 'we didn't bump' is indistinguishable from an oversight
Q32@scripts/checks/d44-create-deps-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D44 sibling's live-range knob carries the SAME range as this script — anchored on the knob line, so the file's own prose about the knob cannot satisfy the pin
Q33@scripts/checks/ub-lp9.25-dangling-blocker-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D45 sibling's live-range knob, same anchoring and same reason
Q34@scripts/checks/ub-lp9.25-dangling-blocker-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, a separate line and so a separate way to be half-bumped
Q35@scripts/checks/d46-schema-migration-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D46 sibling's live-range knob, same anchoring and same reason
Q36@scripts/checks/d46-schema-migration-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q37@scripts/checks/d47-envelope-id-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D47 sibling's live-range knob, same anchoring and same reason
Q38@scripts/checks/d47-envelope-id-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q39@scripts/checks/d48-stdout-channel-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D48 sibling's live-range knob, same anchoring and same reason
Q40@scripts/checks/d48-stdout-channel-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q41@scripts/checks/d49-startup-failure-render-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D49 sibling's live-range knob, same anchoring and same reason
Q42@scripts/checks/d49-startup-failure-render-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q43@scripts/checks/d50-pre-handshake-gate-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D50 sibling's live-range knob, same anchoring and same reason
Q44@scripts/checks/d50-pre-handshake-gate-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q45@scripts/checks/d53-request-integrity-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D53 sibling's live-range knob, same anchoring and same reason
Q46@scripts/checks/d53-request-integrity-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q47@scripts/checks/d54-parse-error-id-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D54 sibling's live-range knob — the PREVIOUS newest script, which by the self-row rule pins everyone except itself and so needs this row
Q48@scripts/checks/d54-parse-error-id-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, the other of the two knobs d54 cannot pin itself
Q49@CLAUDE.md@^\| .docs/PRD\.md. \| Product truth@$RANGE_RE@PROSE D-range bump site, LOCATED on the document-map row that states the range
Q50@docs/plans/ci-cd-and-distribution.md@\*\*\(a\) D-id coherence\*\*@$RANGE_RE@PROSE D-range bump site, LOCATED on the class-(a) statement — the ONE place in that file allowed to quote the live range
Q51@xtask/src/doc_lint.rs@Spec tokenizes@$RANGE_RE@PROSE D-range bump site, half 1 of 2: the PROSE range on the tokenizer comment line
Q52@xtask/src/doc_lint.rs@Spec tokenizes@$RANGE_ALT_RE@…half 2 of 2: the TOKENIZER ALTERNATION on that same line
Q53@docs/PRD.md@^\| \*\*D55\*\* \|@Tracked as .ub-e47.@the D55 decision row is in PRD §4 and names the work it tracks
Q54@docs/PRD.md@^\| \*\*D34\*\* \|@\*\*\[D55\]\*\* D55 \(this table\) adds a second producer@D34 clause (3) carries the reciprocal note: RateLimited is no longer emitted at the L7 MCP handler only
Q55@docs/plans/01-design-spine.md@^ +RateLimited, //@D55@spine §2.2's RateLimited comment names the CLI producer
Q56@docs/plans/01-design-spine.md@^\*\(exit 2 also carries the retryable transient-busy@\*\*\[D55\]\*\*@spine §2.3's exit-2 note carries the D55 producer
Q57@docs/plans/01-design-spine.md@^\*\*error boundary \(D27/AF-4\)\.\*\*@UpdateRateLimited.*UpdateUnauthorized@the spine's exit-boundary paragraph names the two new variants
Q58@docs/plans/crates/unblock-cli.md@^\| .pub enum CliError. \|@UpdateRateLimited.*UpdateUnauthorized@the crate plan's CliError row names the two new variants
Q59@docs/plans/crates/unblock-cli.md@^\| .src/exit\.rs. \|@UpdateRateLimited@the crate plan's src/exit.rs row carries the new code arms
Q60@docs/plans/crates/unblock-cli.md@^\| .src/commands/update\.rs. \|@refusal_for_status.*refusal_message@the crate plan's src/commands/update.rs row carries the mechanism and its unit cells
Q61@docs/plans/crates/unblock-cli.md@^\| .tests/update_verify\.rs. \|@update_refusal_@the crate plan's tests/update_verify.rs row lists the update_refusal_ cells
Q62@docs/plans/crates/unblock-error.md@^- .ErrorCode. \(enum@D55@the unblock-error crate plan's ErrorCode bullet records the second producer
Q63@docs/plans/ci-cd-and-distribution.md@^- \*\*When GitHub refuses the release query \(D55@RATE_LIMITED@ci-cd §4's D55 bullet states the reclassification
Q64@docs/plans/ci-cd-and-distribution.md@^- .AXOUPDATER_GITHUB_TOKEN. is a \*\*client-runtime\*\* env@It is optional\.@ci-cd §4's AXOUPDATER_GITHUB_TOKEN bullet, beside which the D55 bullet sits, still states the token is an optional client-runtime env
Q65@RELEASING.md@^ +- A v1\.0\.2 or later binary fails with code@RATE_LIMITED.*D55@RELEASING.md §5 step 3 keys RATE_LIMITED on the querying binary's version
Q66@scripts/release/update-smoke.sh@^# AXOUPDATER_GITHUB_TOKEN and report a 403 or 429@RATE_LIMITED, exit 2@the POSIX smoke header names RATE_LIMITED for a v1.0.2-or-later binary
Q67@scripts/release/update-smoke.ps1@^# and give RATE_LIMITED, exit 2@RATE_LIMITED, exit 2, for a 403 or 429@the PowerShell smoke header names RATE_LIMITED for a v1.0.2-or-later binary
Q68@.github/workflows/update-smoke.yml@^ +# v1\.0\.0 and v1\.0\.1 ignore the variable\.@RATE_LIMITED@the update-smoke.yml env comment keys RATE_LIMITED on the step's binary version
Q69@README.md@^.AXOUPDATER_GITHUB_TOKEN. is optional\.@AXOUPDATER_GITHUB_TOKEN@the README Self-update section names AXOUPDATER_GITHUB_TOKEN
Q70@README.md@\(D55, from v1\.0\.2\)@401@the README Self-update section states the refusal codes and the release that ships them
Q71@docs/plans/00-roadmap.md@^### v1\.0\.2 — maintenance patch@v1\.0\.2@the markdown roadmap carries the v1.0.2 subsection
Q72@docs/plans/00-roadmap.md@^- \*\*.unblock update. reported GitHub.s refusal of the release query@ub-e47@the markdown roadmap's D55 bullet names the tracked work
Q73@docs/roadmap.html@^ +<li>Fix: <span class=\"mono\">unblock update</span> reported GitHub&rsquo;s refusal@\(D55\)@the RENDERED v1.0.2 card's own D55 bullet — that file is OUTSIDE the doc-lint corpus
Q74@docs/plans/implementation-plan.md@^- \*\*T3\.17 @D55@the task checklist exists and names the decision
Q75@docs/plans/ci-cd-and-distribution.md@Named sub-check \(its D55 sibling@d55-update-refusal-claims@this gate is SPECIFIED in its own paragraph — §2.1(a) of that same file also carries this filename, so a bare token stays green with the paragraph deleted
Q76@.github/workflows/ci.yml@d55-update-refusal-claims@:[0-9]+: +- run: scripts/checks/d55-update-refusal-claims\.sh@this gate actually RUNS in the required doc-lint job; every line naming it must BE the run step, so a commented-out step fails
Q77@docs/PROCESS.md@d55-update-refusal-claims\.sh. .RANGE_RE@RANGE_ALT_RE@the count-free LIST that IS the rule carries this script's own entry with BOTH knob names
Q78@docs/PROCESS.md@always the NEWEST script@d55-update-refusal-claims\.sh@PROCESS.md §3 names this gate as the newest reference, the one script that pins every older sibling's knobs (the next mint retargets it)
Q79@docs/plans/ci-cd-and-distribution.md@\*\*\(a\) D-id coherence\*\*@the .ci\(d55\). commit that mints it, .scripts/checks/d55-update-refusal-claims\.sh.@ci-cd §2.1(a)'s enumeration carries this script's knobs as the newest
Q80@scripts/checks/d53-request-integrity-claims.sh@^Q53@d55-update-refusal-claims@the D53 sibling's newest-pointer row is retargeted to this gate
Q81@scripts/checks/d54-parse-error-id-claims.sh@^Q67@d55-update-refusal-claims@the D54 sibling's row pinning d53's Q53 follows the retarget
Q82@scripts/checks/d54-parse-error-id-claims.sh@^Q72@d55-update-refusal-claims@the D54 sibling's newest-pointer row is retargeted to this gate
Q83@crates/unblock-cli/tests/update_verify.rs@^async fn update_refusal_401_unset@async fn update_refusal_401_unset_is_config_error\(\)@the tokenless 401 cell is still declared, pinning CONFIG_ERROR and the whole tokenless text end to end. Mutant: a tokenless 401 falling through to INTERNAL_ERROR
"

blocked=0

# PORTABILITY (1 of 2), deliberate. Every `$( … )` in this file substitutes a FUNCTION CALL, never an
# inline loop containing a `case`. macOS `/bin/sh` (bash 3.2) mis-parses a `case` arm's `)` inside
# `$( )` and silently produces garbage instead of failing. Every sibling avoids it the same way.
# Q1..Q7 read ONLY update.rs's production prefix and Q8..Q12 ONLY exit.rs's: lines before each file's
# `mod tests {`, so no test body can satisfy them. A missing or duplicated `mod tests {` line is
# fail-closed (exit 2), never a pass.
tests_line() { git grep -n -E '^mod tests \{$' -- "$1" 2>/dev/null | cut -d: -f2; }
UPDATE_TESTS_LINE="$(tests_line crates/unblock-cli/src/commands/update.rs)"
case "$UPDATE_TESTS_LINE" in
  ''|*[!0-9]*) say "cannot find exactly one '^mod tests {' line in crates/unblock-cli/src/commands/update.rs"; exit 2 ;;
esac
EXIT_TESTS_LINE="$(tests_line crates/unblock-cli/src/exit.rs)"
case "$EXIT_TESTS_LINE" in
  ''|*[!0-9]*) say "cannot find exactly one '^mod tests {' line in crates/unblock-cli/src/exit.rs"; exit 2 ;;
esac

check_landings() {
  printf '%s\n' "$REQUIRE" | while IFS='@' read -r code path re reason; do
    [ -n "$code" ] || continue
    if [ ! -f "$path" ]; then
      printf '%s\n' "$path: [$code] REQUIRED D55 target is missing from the tree ($reason)"
      continue
    fi
    git grep -q -I -E "$re" -- "$path" 2>/dev/null \
      || printf '%s\n' "$path: [$code] the landing is GONE — no line matches /$re/ ($reason)"
  done
  printf '%s\n' "$REQUIRE_ROW" | while IFS='@' read -r code path anchor re reason; do
    [ -n "$code" ] || continue
    if [ ! -f "$path" ]; then
      printf '%s\n' "$path: [$code] REQUIRED D55 target is missing from the tree ($reason)"
      continue
    fi
    rows="$(git grep -n -I -E "$anchor" -- "$path" 2>/dev/null)"
    case "$code" in
      Q[1-7]) rows="$(printf '%s\n' "$rows" | awk -F: -v end="$UPDATE_TESTS_LINE" '$2 + 0 < end + 0')" ;;
      Q[89]|Q1[0-2]) rows="$(printf '%s\n' "$rows" | awk -F: -v end="$EXIT_TESTS_LINE" '$2 + 0 < end + 0')" ;;
    esac
    if [ -z "$rows" ]; then
      # The negative pair passes with no `reqwest` line at all; every other row needs its anchor.
      case "$code" in
        Q13|Q14) continue ;;
      esac
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
  say "BLOCKED — the D55 refusal classification or its cascade is incomplete; the sites above are missing."
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
check_table_floor 'row-anchored (Q)' "$q_count" 83 || blocked=1

[ "$blocked" = "0" ] || exit 1
say "OK — update_error's variant match and status read, the pure classifier with its 403|429 and 401 arms, the message function, the token state, both variants and the three code arms are in the tree, no code line of update.rs or exit.rs spells reqwest, every unit and update_refusal_ cell is declared, the unblock-error comments and the published RateLimited text stand, the tracker names ub-e47, the PRD row and its D34 note, the spine, both crate plans, ci-cd §4, RELEASING.md, the smoke headers, the update-smoke env comment, README, both roadmaps and the task carry D55, the retargeted sibling rows are in place, this gate is both specified and wired, the contract stays unblock.mcp.v1.10, and the live D-range is current at every prose site and every sibling script's knob"
exit 0
