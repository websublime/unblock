#!/bin/sh
# d58-health-full-claims.sh — the REQUIRED-LANDING gate for D58, FR-16 FULL: the four health levels
# defined by repair path, `unblock doctor --repair` with its pre-open exclusive window and online
# `Session::recover()`, the `.unblock/.recovery/` evidence, and the refusal to recreate a missing
# database (PRD §4 D58, tracked as `ub-lp9.3`). Spec: docs/plans/ci-cd-and-distribution.md §2.1, the
# paragraph beginning "Named sub-check (its D58 sibling…)". Runs as a step of the required `doc-lint`
# job, immediately after its `d57` sibling.
#
# THAT PARAGRAPH IS NORMATIVE OVER THIS FILE. Every landing enforced here is named there; a rule that
# exists in one and not the other is a defect to be fixed in the SAME change.
#
# MINTED IN THE SPEC COMMIT (Miguel's ruling, 2026-11-13) — unlike D57, whose gate landed in a
# `ci(d57)` commit after the code. So on its first commit this file pins only what the SPEC commit
# lands: the D58 row and its reciprocal notes, the spine, the crate plans, the roadmaps and the task,
# the D-range cascade, the newest-pointer rows retargeted to this script, every older sibling's knobs,
# and the unmoved contract. The IMPLEMENTATION and TEST commits EXTEND it with the mechanism and cell
# rows the ci-cd paragraph lists, each row landing in the SAME commit as the line it pins, and raise
# the floors below in that commit. A row is never added before its line exists.
#
# WHY THIS ONE IS POSITIVE-ONLY, save TWO negative rows. A negative prose sweep is dodged by a claim
# REWRAPPED across two lines, so every prose row is a positive landing. The two NEGATIVE rows are
# dependency and code rules, not prose sweeps: N1 — no non-comment line of unblock-health's manifest
# names `unblock-storage` (D29 clause F3, kept by D58 clause (4)); N2 — no non-comment line of any
# crate's production source spells a VACUUM statement, in any letter case, other than `VACUUM INTO`
# (D58 clause (3)). The IMPLEMENTATION commit adds a THIRD negative row with the code it pins: no
# non-comment line of `crates/unblock-cli/src/commands/doctor.rs` calls `integrity_check(` (criterion (d)).
# Both pass on the pre-fix tree by design: they pin what already holds and must keep holding.
#
# WHAT NO ROW HERE CAN SEE, stated so no row is over-read. A line grep cannot prove that the exclusive
# window takes its raw copies before any SQLite open, that `locking_mode=EXCLUSIVE` really refuses
# while another connection is open (criterion (f) MEASURES that), that `repair` runs its actions in one
# transaction, that a refusal never reports success, or that the classifier is pure. The cells D58
# clause (12) names carry those.
#
# THE CONTRACT KNOB DOES NOT MOVE. D58 is CLI-only (D3) and adds no wire byte, so this file pins the
# LIVE id `unblock.mcp.v1.12` at the code constant and in every sibling's knob (criterion (p)).
#
# PRE-FIX TREE: `main` at 121c8b7, the tree before D58's spec commit. Every row asserting a NEW
# landing FAILS there. Exactly the rows that pin what already holds pass: P1 (`ub-lp9.3` is already
# exported), N1, N2, the live-contract rows, and the sibling CONTRACT_RE rows.
#
# THREE RULE KINDS
#   P-n   REQUIRED landing — a presence predicate over the GENERATED export `.unblock/issues.jsonl`.
#   N-n   NEGATIVE landing — over a git PATHSPEC, no NON-COMMENT line may match. No line at all is
#         the pass.
#   Q-n   ROW-ANCHORED landing — at least ONE line must match the anchor, and EVERY line matching the
#         anchor must ALSO match the requirement. A vanished anchor is a FAILURE, never a pass.
#
# Exit: 0 = pass · 1 = BLOCK (a required landing is missing) · 2 = cannot evaluate (fail-closed).
set -u

# PORTABILITY (2 of 2). Every variable expansion goes through `printf`, never `echo`. POSIX-mode `echo`
# interprets backslash escapes, so a `\b` inside a regex literal becomes a BACKSPACE byte.
say() { printf 'd58-claims: %s\n' "$*" >&2; }

git rev-parse --show-toplevel >/dev/null 2>&1 || { say "not a git repository"; exit 2; }
cd "$(git rev-parse --show-toplevel)" || { say "cannot cd to the repo root"; exit 2; }

# The LIVE D-id range, in its TWO spellings. It tracks the LIVE range, never a frozen historical one:
# the day a D59 is minted, every file `docs/PROCESS.md` §3 enumerates moves with it or a required step
# goes red. `xtask/src/doc_lint.rs`'s bump site is ONE physical line carrying BOTH halves, the prose
# range `(D1..D58)` and the tokenizer's ALTERNATION `\bD(58|57|56|…)\b`; pinning only the prose is how
# that site rots into an undefined-D59 finding.
RANGE_RE='D1\.\.D58'
RANGE_ALT_RE='D\(58\|57\|'

# The LIVE published contract version. D58 does NOT move it.
CONTRACT_RE='unblock\.mcp\.v1\.12'

# The SAME spellings as they appear INSIDE a sibling script's knob line, where each backslash is a
# literal byte. DERIVED, never hand-written a second time.
knob_re() { printf '%s' "$1" | sed 's/\\/\\\\\\/g'; }
RANGE_KNOB_RE="$(knob_re "$RANGE_RE")"
RANGE_KNOB_ALT_RE="$(knob_re "$RANGE_ALT_RE")"
CONTRACT_KNOB_RE="$(knob_re "$CONTRACT_RE")"

# The negative rows' patterns, held in variables (`$@`-safe). Both run over `git grep -n` output
# (`path:lineno:text`). A comment line is one whose first non-blank characters are `#` (TOML) or `//`
# (Rust, doc comments included).
COMMENT_LINE_RE='^[^:]+:[0-9]+:[[:space:]]*(#|//)'
STORAGE_DEP_RE='(^|[^[:alnum:]_-])unblock-storage([^[:alnum:]_-]|$)'
VACUUM_STMT_RE='[Vv][Aa][Cc][Uu][Uu][Mm]([^ [:alnum:]_]| +[^Ii ]|$)'

# =================================================================================================
# REQUIRED LANDINGS — `code@path@regex@what it proves`
# =================================================================================================
REQUIRE="
P1@.unblock/issues.jsonl@\"id\":\"ub-lp9\.3\"@the tracker record names the work D58 implements. The export keeps closed rows, so the row survives the merge-time close
"

# =================================================================================================
# NEGATIVE LANDINGS — `code@pathspec@regex@what it proves`
# =================================================================================================
REQUIRE_NONE="
N1@crates/unblock-health/Cargo.toml@$STORAGE_DEP_RE@NEGATIVE: unblock-health declares no unblock-storage dependency — the classifier takes facts as data (D29 clause F3, kept by D58 clause (4))
N2@crates/*/src/*.rs@$VACUUM_STMT_RE@NEGATIVE: no production source spells a VACUUM statement other than VACUUM INTO — D58 clause (3) forbids VACUUM, which would erase the forensic trace of the corruption
"

# =================================================================================================
# ROW-ANCHORED LANDINGS — `code@path@anchor@regex@what it proves`
#
# Q1..Q23  are the SIBLING SCRIPTS' range knobs — the NEWEST script pins the OLDER ones, and THIS
#      script's own knobs have NO row, deliberately: they are the REFERENCE the other rows are compared
#      against, and a self-row could never fail. Q22/Q23 are the marginal pair: d57's two range knobs
#      were pinned by nothing in the tree until this script landed.
# Q24..Q35 are the UNMOVED contract: every sibling's CONTRACT_RE knob and the code constant.
# Q36..Q39 are the THREE PROSE bump sites over FOUR rows.
# Q40..Q44 are this gate's wiring: SPECIFIED, RUNNING, LISTED in PROCESS.md §3 with both knob names,
#      PROCESS.md §3's NEWEST sentence, and ci-cd §2.1(a)'s enumeration naming it the newest.
# Q45..Q77 are the SIBLING ROWS the D58 spec commit retargeted to this script: d53's Q53, d54's Q67
#      and Q72, d55's Q78, Q80, Q81 and Q82, d56's Q101 and Q103..Q109, and d57's Q149, Q150
#      (reworded so it stops calling d57 the newest) and Q151..Q166 except Q159.
# Q78..Q136 are the normative and published texts: the D58 row's load-bearing claims, the [D58] notes
#      on D29, D27, D14, D31, FR-16 and PRD §7, §8.2, §12.4, §12.5, §13 and RK-5; the spine's §2.3
#      note, §3.1 types, §3.2 methods and CF-E withdrawal, §3.3 primitives, §4 facade, §4.1 report,
#      doctor and recover, §5b command; the health, engine, cli, storage, config and sync plans and
#      docs/plans/README.md's CF-E note; both roadmaps; the T4.3 task, its letters, and the backlog.
# Q137..   (design Review round 1) the D58 row's evidence-never-deleted, never-a-success, lost-database
#      predicate and its accepted limit, read-only dry run, exact-NOTADB, live-connection refusal,
#      Windows-unmeasured and recovery-gap claims; the D46 note; the spine's no-default-body, open-mode
#      and two D31 notes; the repository .gitignore line; the exclusivity-macos CI job row.
# Q153..   (design Review round 2) the probe that never creates the marker, init's database-only
#      mode (D58 row, FR-14, spine §5b init, cli plan), the not-committed-lock assumption and limit, the
#      in-repair migrate on the held connection, the hot-journal routing, raw-code NOTADB, and the
#      REQUIRED macOS job.
# Q166..   (design Review round 3) T4.3's README duty, init's orphan-sidecar refusal, the D27/AF-3
#      reciprocal pair, the Windows registry key, why database_lost stays Unsafe, and the private-field
#      ExclusiveRequest with its testkit-only hook setter.
# Q175..   (Round 3 delta) the ConfigError::is_read_only_rollback seam (spine, config plan, D58 row),
#      the stop-every-process instruction in the OrphanSidecars message, and the Windows lock-byte skip.
# =================================================================================================
REQUIRE_ROW="
Q1@scripts/checks/d44-create-deps-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D44 sibling's live-range knob carries the SAME range as this script — anchored on the knob line, so the file's own prose about the knob cannot satisfy the pin
Q2@scripts/checks/ub-lp9.25-dangling-blocker-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D45 sibling's live-range knob carries the SAME range as this script — anchored on the knob line, so the file's own prose about the knob cannot satisfy the pin
Q3@scripts/checks/ub-lp9.25-dangling-blocker-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, a separate line and so a separate way to be half-bumped
Q4@scripts/checks/d46-schema-migration-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D46 sibling's live-range knob carries the SAME range as this script — anchored on the knob line, so the file's own prose about the knob cannot satisfy the pin
Q5@scripts/checks/d46-schema-migration-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, a separate line and so a separate way to be half-bumped
Q6@scripts/checks/d47-envelope-id-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D47 sibling's live-range knob carries the SAME range as this script — anchored on the knob line, so the file's own prose about the knob cannot satisfy the pin
Q7@scripts/checks/d47-envelope-id-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, a separate line and so a separate way to be half-bumped
Q8@scripts/checks/d48-stdout-channel-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D48 sibling's live-range knob carries the SAME range as this script — anchored on the knob line, so the file's own prose about the knob cannot satisfy the pin
Q9@scripts/checks/d48-stdout-channel-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, a separate line and so a separate way to be half-bumped
Q10@scripts/checks/d49-startup-failure-render-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D49 sibling's live-range knob carries the SAME range as this script — anchored on the knob line, so the file's own prose about the knob cannot satisfy the pin
Q11@scripts/checks/d49-startup-failure-render-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, a separate line and so a separate way to be half-bumped
Q12@scripts/checks/d50-pre-handshake-gate-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D50 sibling's live-range knob carries the SAME range as this script — anchored on the knob line, so the file's own prose about the knob cannot satisfy the pin
Q13@scripts/checks/d50-pre-handshake-gate-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, a separate line and so a separate way to be half-bumped
Q14@scripts/checks/d53-request-integrity-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D53 sibling's live-range knob carries the SAME range as this script — anchored on the knob line, so the file's own prose about the knob cannot satisfy the pin
Q15@scripts/checks/d53-request-integrity-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, a separate line and so a separate way to be half-bumped
Q16@scripts/checks/d54-parse-error-id-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D54 sibling's live-range knob carries the SAME range as this script — anchored on the knob line, so the file's own prose about the knob cannot satisfy the pin
Q17@scripts/checks/d54-parse-error-id-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, a separate line and so a separate way to be half-bumped
Q18@scripts/checks/d55-update-refusal-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D55 sibling's live-range knob carries the SAME range as this script — anchored on the knob line, so the file's own prose about the knob cannot satisfy the pin
Q19@scripts/checks/d55-update-refusal-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, a separate line and so a separate way to be half-bumped
Q20@scripts/checks/d56-organization-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D56 sibling's live-range knob carries the SAME range as this script — anchored on the knob line, so the file's own prose about the knob cannot satisfy the pin
Q21@scripts/checks/d56-organization-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, a separate line and so a separate way to be half-bumped
Q22@scripts/checks/d57-config-layers-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D57 sibling's live-range knob — the PREVIOUS newest script, which by the self-row rule pins everyone except itself and so needs this row
Q23@scripts/checks/d57-config-layers-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, the other of the two knobs d57 cannot pin itself
Q24@scripts/checks/d44-create-deps-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@that sibling's contract knob still carries the LIVE id — D58 moves no contract byte (PRD §4 D58 criterion (p))
Q25@scripts/checks/ub-lp9.25-dangling-blocker-claims.sh@^CONTRACT_RE=@mcp.*v1.*[^0-9]12'\$@the D45 sibling's contract knob spells the id with its \\? segments, so this row checks the LIVE minor at its end — D58 moves no contract byte
Q26@scripts/checks/d47-envelope-id-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@that sibling's contract knob still carries the LIVE id — D58 moves no contract byte (PRD §4 D58 criterion (p))
Q27@scripts/checks/d48-stdout-channel-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@that sibling's contract knob still carries the LIVE id — D58 moves no contract byte (PRD §4 D58 criterion (p))
Q28@scripts/checks/d49-startup-failure-render-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@that sibling's contract knob still carries the LIVE id — D58 moves no contract byte (PRD §4 D58 criterion (p))
Q29@scripts/checks/d50-pre-handshake-gate-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@that sibling's contract knob still carries the LIVE id — D58 moves no contract byte (PRD §4 D58 criterion (p))
Q30@scripts/checks/d53-request-integrity-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@that sibling's contract knob still carries the LIVE id — D58 moves no contract byte (PRD §4 D58 criterion (p))
Q31@scripts/checks/d54-parse-error-id-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@that sibling's contract knob still carries the LIVE id — D58 moves no contract byte (PRD §4 D58 criterion (p))
Q32@scripts/checks/d55-update-refusal-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@that sibling's contract knob still carries the LIVE id — D58 moves no contract byte (PRD §4 D58 criterion (p))
Q33@scripts/checks/d56-organization-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@that sibling's contract knob still carries the LIVE id — D58 moves no contract byte (PRD §4 D58 criterion (p))
Q34@scripts/checks/d57-config-layers-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@that sibling's contract knob still carries the LIVE id — D58 moves no contract byte (PRD §4 D58 criterion (p))
Q35@crates/unblock-mcp/src/options.rs@^pub const CONTRACT_VERSION@$CONTRACT_RE@the code constant still reads the live id: D58 is CLI-only and moves no contract byte (criterion (p))
Q36@CLAUDE.md@^\| .docs/PRD\.md. \| Product truth@$RANGE_RE@PROSE D-range bump site, LOCATED on the document-map row that states the range
Q37@docs/plans/ci-cd-and-distribution.md@\*\*\(a\) D-id coherence\*\*@$RANGE_RE@PROSE D-range bump site, LOCATED on the class-(a) statement — the ONE place in that file allowed to quote the live range
Q38@xtask/src/doc_lint.rs@Spec tokenizes@$RANGE_RE@PROSE D-range bump site, half 1 of 2: the PROSE range on the tokenizer comment line
Q39@xtask/src/doc_lint.rs@Spec tokenizes@$RANGE_ALT_RE@…half 2 of 2: the TOKENIZER ALTERNATION on that same line, which now LEADS with 58
Q40@docs/plans/ci-cd-and-distribution.md@Named sub-check \(its D58 sibling@d58-health-full-claims@this gate is SPECIFIED in its own paragraph — §2.1(a) also carries this filename, so a bare token would stay green with the paragraph deleted
Q41@.github/workflows/ci.yml@d58-health-full-claims@:[0-9]+: +- run: scripts/checks/d58-health-full-claims\.sh@this gate actually RUNS in the required doc-lint job; every line naming it must BE the run step, so a commented-out step fails
Q42@docs/PROCESS.md@d58-health-full-claims\.sh. .RANGE_RE@RANGE_ALT_RE@the count-free LIST that IS the rule carries this script's own entry with BOTH knob names
Q43@docs/PROCESS.md@always the NEWEST script@d58-health-full-claims\.sh@PROCESS.md §3 names this gate as the newest reference, which pins every OLDER script's knobs (the next mint retargets it)
Q44@docs/plans/ci-cd-and-distribution.md@\*\*\(a\) D-id coherence\*\*@since the D58 SPEC commit that mints it .Miguel.s ruling — the gate leads the implementation this time., .scripts/checks/d58-health-full-claims\.sh. .RANGE_RE. \*\*and\*\* .RANGE_ALT_RE., the newest of them@ci-cd §2.1(a)'s enumeration names this script as the newest
Q45@scripts/checks/d53-request-integrity-claims.sh@^Q53[^0-9]@d58-health-full-claims@the D53 sibling's newest-pointer row names this gate (retargeted by the D58 spec commit)
Q46@scripts/checks/d54-parse-error-id-claims.sh@^Q67[^0-9]@d58-health-full-claims@the D54 sibling's row pinning d53's Q53 follows the retarget
Q47@scripts/checks/d54-parse-error-id-claims.sh@^Q72[^0-9]@d58-health-full-claims@the D54 sibling's newest-pointer row names this gate
Q48@scripts/checks/d55-update-refusal-claims.sh@^Q78[^0-9]@d58-health-full-claims@the D55 sibling's newest-pointer row names this gate
Q49@scripts/checks/d55-update-refusal-claims.sh@^Q80[^0-9]@d58-health-full-claims@the D55 sibling's row pinning d53's Q53 follows the retarget
Q50@scripts/checks/d55-update-refusal-claims.sh@^Q81[^0-9]@d58-health-full-claims@the D55 sibling's row pinning d54's Q67 follows the retarget
Q51@scripts/checks/d55-update-refusal-claims.sh@^Q82[^0-9]@d58-health-full-claims@the D55 sibling's row pinning d54's Q72 follows the retarget
Q52@scripts/checks/d56-organization-claims.sh@^Q101[^0-9]@d58-health-full-claims@the D56 sibling's newest-pointer row names this gate
Q53@scripts/checks/d56-organization-claims.sh@^Q103[^0-9]@d58-health-full-claims@the D56 sibling's row pinning d53's Q53 follows the retarget
Q54@scripts/checks/d56-organization-claims.sh@^Q104[^0-9]@d58-health-full-claims@the D56 sibling's row pinning d54's Q67 follows the retarget
Q55@scripts/checks/d56-organization-claims.sh@^Q105[^0-9]@d58-health-full-claims@the D56 sibling's row pinning d54's Q72 follows the retarget
Q56@scripts/checks/d56-organization-claims.sh@^Q106[^0-9]@d58-health-full-claims@the D56 sibling's row pinning d55's Q78 follows the retarget
Q57@scripts/checks/d56-organization-claims.sh@^Q107[^0-9]@d58-health-full-claims@the D56 sibling's row pinning d55's Q80 follows the retarget
Q58@scripts/checks/d56-organization-claims.sh@^Q108[^0-9]@d58-health-full-claims@the D56 sibling's row pinning d55's Q81 follows the retarget
Q59@scripts/checks/d56-organization-claims.sh@^Q109[^0-9]@d58-health-full-claims@the D56 sibling's row pinning d55's Q82 follows the retarget
Q60@scripts/checks/d57-config-layers-claims.sh@^Q149[^0-9]@d58-health-full-claims@the D57 sibling's newest-pointer row names this gate
Q61@scripts/checks/d57-config-layers-claims.sh@^Q150[^0-9]@stopped naming this script the newest when D58@the D57 sibling's Q150 is reworded so it stops calling d57 the newest
Q62@scripts/checks/d57-config-layers-claims.sh@^Q151[^0-9]@d58-health-full-claims@the D57 sibling's row pinning d53's Q53 follows the retarget
Q63@scripts/checks/d57-config-layers-claims.sh@^Q152[^0-9]@d58-health-full-claims@the D57 sibling's row pinning d54's Q67 follows the retarget
Q64@scripts/checks/d57-config-layers-claims.sh@^Q153[^0-9]@d58-health-full-claims@the D57 sibling's row pinning d54's Q72 follows the retarget
Q65@scripts/checks/d57-config-layers-claims.sh@^Q154[^0-9]@d58-health-full-claims@the D57 sibling's row pinning d55's Q78 follows the retarget
Q66@scripts/checks/d57-config-layers-claims.sh@^Q155[^0-9]@d58-health-full-claims@the D57 sibling's row pinning d55's Q80 follows the retarget
Q67@scripts/checks/d57-config-layers-claims.sh@^Q156[^0-9]@d58-health-full-claims@the D57 sibling's row pinning d55's Q81 follows the retarget
Q68@scripts/checks/d57-config-layers-claims.sh@^Q157[^0-9]@d58-health-full-claims@the D57 sibling's row pinning d55's Q82 follows the retarget
Q69@scripts/checks/d57-config-layers-claims.sh@^Q158[^0-9]@d58-health-full-claims@the D57 sibling's row pinning d56's Q101 follows the retarget
Q70@scripts/checks/d57-config-layers-claims.sh@^Q160[^0-9]@d58-health-full-claims@the D57 sibling's row pinning d56's Q103 follows the retarget
Q71@scripts/checks/d57-config-layers-claims.sh@^Q161[^0-9]@d58-health-full-claims@the D57 sibling's row pinning d56's Q104 follows the retarget
Q72@scripts/checks/d57-config-layers-claims.sh@^Q162[^0-9]@d58-health-full-claims@the D57 sibling's row pinning d56's Q105 follows the retarget
Q73@scripts/checks/d57-config-layers-claims.sh@^Q163[^0-9]@d58-health-full-claims@the D57 sibling's row pinning d56's Q106 follows the retarget
Q74@scripts/checks/d57-config-layers-claims.sh@^Q164[^0-9]@d58-health-full-claims@the D57 sibling's row pinning d56's Q107 follows the retarget
Q75@scripts/checks/d57-config-layers-claims.sh@^Q165[^0-9]@d58-health-full-claims@the D57 sibling's row pinning d56's Q108 follows the retarget
Q76@scripts/checks/d57-config-layers-claims.sh@^Q166[^0-9]@d58-health-full-claims@the D57 sibling's row pinning d56's Q109 follows the retarget
Q77@docs/PRD.md@^\| \*\*D58\*\* \|@Tracked as .ub-lp9\.3.@the D58 decision row is in PRD §4 and names the work it tracks
Q78@docs/PRD.md@^\| \*\*D58\*\* \|@SUPERSEDES D29 clause F5.s severity map@the row records what it supersedes, reciprocal to D29's note
Q79@docs/PRD.md@^\| \*\*D58\*\* \|@.ub-p1h. is NOT a prerequisite@Drifted is database-internal, so the JSONL auto-export is not a prerequisite (clause (1))
Q80@docs/PRD.md@^\| \*\*D58\*\* \|@every other row is .page_corruption. and .Unsafe.|EVERY other row is .page_corruption. and .Unsafe.@clause (3): anything not index-only classifies Unsafe — the fail-safe rule
Q81@docs/PRD.md@^\| \*\*D58\*\* \|@.row <N> missing from index <I>., .rowid not at end-of-record for row <N> of index <I>., .row <N> values differ from index <I>., .wrong # of entries in index <I>.@clause (3): the four index-only message shapes, verified against the SQLite 3.45.1 libsql bundles
Q82@docs/PRD.md@^\| \*\*D58\*\* \|@\*\*NO .VACUUM.:\*\*@clause (3): no VACUUM — VACUUM INTO only for the snapshot
Q83@docs/PRD.md@^\| \*\*D58\*\* \|@THE METADATA KEEP RULE:\*\* the row with the GREATEST rowid survives@clause (5): the deterministic dedupe keep rule
Q84@docs/PRD.md@^\| \*\*D58\*\* \|@NEVER lowers a counter@clause (5): the child-counter raise never lowers a high-water mark (D21)
Q85@docs/PRD.md@^\| \*\*D58\*\* \|@FIRST EXECUTED statement is .PRAGMA locking_mode=EXCLUSIVE.@clause (6) step (c): the exclusivity proof is the locking mode
Q86@docs/PRD.md@^\| \*\*D58\*\* \|@\"EXCLUSIVE and IMMEDIATE are the same in WAL mode\"@clause (6): the cited reason BEGIN EXCLUSIVE alone proves nothing under WAL
Q87@docs/PRD.md@^\| \*\*D58\*\* \|@THE .-wal. IS NEVER RENAMED@clause (6): the open WAL is never renamed (howtocorrupt §2.5)
Q88@docs/PRD.md@^\| \*\*D58\*\* \|@howtocorrupt\.html. section 2\.2@clause (6) step (b): no descriptor on the db file coexists with a libsql connection (ub-2vp stays separate)
Q89@docs/PRD.md@^\| \*\*D58\*\* \|@NEW .ConfigError::DatabaseMissing \{ path \}. mapped onto the EXISTING .ErrorCode::DatabaseNotFound.@clause (7): a missing database is refused with the existing code, never recreated
Q90@docs/PRD.md@^\| \*\*D58\*\* \|@.Healthy. or .Drifted. → 0; .Recoverable. or .Unsafe. → 2@clause (8): the exit by level
Q91@docs/PRD.md@^\| \*\*D58\*\* \|@.Session::recover\(\). stays parameterless and returns .DiagnosticReport.@clause (8): recover() keeps its v1 signature
Q92@docs/PRD.md@^\| \*\*D58\*\* \|@\.unblock/\.recovery/<UTC-stamp>-<run-id>/@clause (9): the evidence directory layout
Q93@docs/PRD.md@^\| \*\*D58\*\* \|@LOCAL MODE ONLY@clause (10): recover(), --repair and .recovery/ are local-only
Q94@docs/PRD.md@^\| \*\*D58\*\* \|@the contract stays .unblock\.mcp\.v1\.12.@D35 not amended: no contract move
Q95@docs/PRD.md@^\| \*\*D58\*\* \|@\*\*\(f\) EXCLUSIVITY, MEASURED, on Linux and macOS\*\*@criterion (f): the exclusivity property is MEASURED before reliance
Q96@docs/PRD.md@^\| \*\*D29\*\* \|@\*\*\[D58\] SUPERSEDED IN PART by PRD §4 D58@D29 carries the reciprocal note on F5, F4 and the CF-E reservation
Q97@docs/PRD.md@^\| \*\*D27\*\* \|@\*\*\[D58\] SUPERSEDED by PRD §4 D58 clause \(8\)@D27 carries the reciprocal note on its AF-1 exit rule
Q98@docs/PRD.md@^\| \*\*D14\*\* \|@\*\*\[D58\] CORRECTED \(v1\.1\):\*\* no command assumes an inactive MCP server@D14 carries the correction of its inactive-server assumption
Q99@docs/PRD.md@^\| \*\*D31\*\* \|@\*\*\[D58\] AMENDED \(v1\.1\):\*\* .\.unblock\.lock. stays never-written@D31 carries the note on the replaced OrphanedLockFile detector
Q100@docs/PRD.md@^- \*\*FR-16 \[@\*\*\[D58\] CORRECTED:\*\* .Drifted. is derived-state drift INSIDE the database@FR-16 no longer ties Drifted to a JSONL export
Q101@docs/PRD.md@^- \*\*FR-16 \[@\*AC \(v1\.1, D58\): PRD §4 D58 clause \(12\) criteria \(a\)–\(r\)@FR-16 carries its v1.1 acceptance pointer, cited by letter
Q102@docs/PRD.md@^\| \*\*WorkspaceHealth / AnomalyClass\*\*@\*\*\[D58\]\*\* The levels are defined by repair path@PRD §7 WorkspaceHealth row carries the D58 definition
Q103@docs/PRD.md@^\| \*\*On-disk artifacts\*\*@\*\*\[D58\] \(v1\.1, LOCAL mode only\)\*\* .\.unblock/\.recovery/.@PRD §7 on-disk artifacts list .recovery/
Q104@docs/PRD.md@^Config dir .\.unblock/.@.\.recovery/. \(\*\*\[D58\]\*\*@PRD §12.5 locked names carry .recovery/
Q105@docs/PRD.md@^v1 ships .doctor. \+ libsql .integrity_check.@\*\*\[D58\] SPECIFIED@PRD §12.4 points at D58
Q106@docs/PRD.md@^\| \*\*v1\.1\+\*\* \|@FR-16\(full — D58, T4\.3\)@PRD §13 milestone row names D58
Q107@docs/PRD.md@^\| RK-5 \|@\*\*\[D58\]\*\* The v1\.1 scope is bounded@RK-5 carries the D58 scope bound
Q108@docs/PRD.md@^  exclusive lock \(timeout=0 fail-fast\) for the whole command@\*\*\[D58\] \(v1\.1\):\*\* plain .doctor. opens the database READ-ONLY without migrating@PRD §8.2 lock posture corrected for a writing doctor
Q109@docs/plans/01-design-spine.md@^\*\(\*\*\[D58\] \(v1\.1\) — .doctor. exits by its classified health level@No .ErrorCode. is minted@spine §2.3 note: exit by level, table unmoved
Q110@docs/plans/01-design-spine.md@^\*\*Health facts and repair requests \(storage-owned; D58@NEVER on the wire@spine §3.1 storage-owned health types, never on the wire
Q111@docs/plans/01-design-spine.md@^    RaiseChildCounter \{@NEVER lowers@spine §3.1: the counter raise never lowers
Q112@docs/plans/01-design-spine.md@^    async fn health_facts\(@-> Result<DbHealthFacts, StorageError>;@spine §3.2 declares health_facts
Q113@docs/plans/01-design-spine.md@^    async fn repair\(@request: &RepairRequest\) -> Result<RepairOutcome, StorageError>;@spine §3.2 declares repair
Q114@docs/plans/01-design-spine.md@^    // --- \[v1\.1\] CF-E .diagnostic_probe@WITHDRAWN by D58@spine §3.2: the CF-E probe reservation is withdrawn
Q115@docs/plans/01-design-spine.md@^- \*\*Pre-open health primitives and the read-only open \(D58@\"EXCLUSIVE and IMMEDIATE are the same in WAL mode\"@spine §3.3: the exclusive window and its cited semantics
Q116@docs/plans/01-design-spine.md@^pub async fn open_with_cli\(@-> Result<OpenedContext, ConfigError>;@spine §4: the no_db selector facade
Q117@docs/plans/01-design-spine.md@^    pub struct HealthReport \{@pub level: HealthLevel@spine §4.1: the engine-local typed report
Q118@docs/plans/01-design-spine.md@^    pub async fn doctor\(@preopen: &PreOpenFacts\) -> Result<HealthReport, EngineError>;@spine §4.1: the v1.1 doctor signature
Q119@docs/plans/01-design-spine.md@^    pub async fn recover\(&self\) -> Result<DiagnosticReport, EngineError>;@\*\*\[D58\] v1\.1 BODY \(signature UNCHANGED@spine §4.1: recover keeps its signature and gets its body
Q120@docs/plans/01-design-spine.md@^\*\*doctor — FR-16 full \(D58, v1\.1@DoctorArgs \{ repair: bool \}@spine §5b: the doctor command's phases and flag
Q121@docs/plans/crates/unblock-health.md@^- \*\*Status:\*\*@SPECIFIED by \*\*PRD §4 D58\*\*@the health plan is the REWRITTEN D58 plan
Q122@docs/plans/crates/unblock-health.md@^- \*\*Depends-on@\*\*Forbidden:\*\* any .unblock-storage. dependency, in v1 AND v1\.1@the health plan keeps D29 clause F3
Q123@docs/plans/crates/unblock-health.md@^\| .src/integrity\.rs. .\[v1\.1\].@wrong # of entries in index@the health plan carries the index-only row classifier
Q124@docs/plans/crates/unblock-health.md@^\| .src/file_state\.rs. \|@DELETED at v1\.1@the health plan deletes the in-session file-state probe
Q125@docs/plans/crates/unblock-engine.md@^\| .src/preopen\.rs. \+ .src/session/lifecycle\.rs. \(health\)@\*\*PRD §4 D58\*\*, .ub-lp9\.3.@the engine plan row names the decision and the work
Q126@docs/plans/crates/unblock-cli.md@^\| .src/commands/doctor\.rs. \|@\*\*\[D58\] SPECIFIED@the cli plan doctor row carries the phased D58 command
Q127@docs/plans/crates/unblock-storage.md@^- Trait grows \(additive\):@\*\*\[D58\] — FR-16 full adds TWO trait methods@the storage plan names the two trait methods
Q128@docs/plans/crates/unblock-config.md@^- \*\*\[D58\] \(v1\.1, .ub-lp9\.3.;@ConfigError::DatabaseMissing \{ path \}@the config plan carries the missing-database refusal and the no_db reader
Q129@docs/plans/crates/unblock-sync.md@^// \[D58\] \(v1\.1\) .scan_conflict_markers.@is the ONE conflict-marker scanner@the sync plan names its scanner the one health consumes
Q130@docs/plans/README.md@^- \*\*CF-E — \[RESOLVED\]@\*\*\[D58\] \(2026-11-13\): the full-taxonomy@docs/plans/README.md's CF-E resolution records the withdrawal
Q131@docs/plans/00-roadmap.md@^\| FR-16 \(full\) \|@\*\*\[D58\]\*\* Specified by PRD §4 D58 \(.ub-lp9\.3.@the markdown roadmap FR-16 row names D58 and the work
Q132@docs/plans/00-roadmap.md@^\| FR-16 \(full\) \|@\| health, engine, cli, storage, config, sync \|\$@the roadmap FR-16 crate list is corrected
Q133@docs/roadmap.html@^ +<li>Full health taxonomy \(FR-16\):@\(D58\)</li>@the RENDERED v1.1 card names D58 — that file is OUTSIDE the doc-lint corpus
Q134@docs/plans/implementation-plan.md@^- \*\*T4\.3 — @FR-16 full@the task exists and names FR-16 full
Q135@docs/plans/implementation-plan.md@^  \(a\) CLASS TABLE, \(b\) INTEGRITY ROWS@\(f\) EXCLUSIVITY, MEASURED@the task's acceptance list cites D58's criteria by letter
Q136@docs/plans/implementation-plan.md@^All M0–M3 exit gates green@FR-16\(full — T4\.3, D58\)@the backlog line names the task and the decision
Q137@docs/PRD.md@^\| \*\*D58\*\* \|@Evidence is NEVER auto-deleted@clause (9): evidence is never auto-deleted (R2-5)
Q138@docs/PRD.md@^\| \*\*D58\*\* \|@a run that repaired nothing is never reported as success@clause (6): a refusal is never a success (R1-4)
Q139@docs/PRD.md@^\| \*\*D58\*\* \|@ONE predicate, .unblock_storage::lost_database\(db_path\)., decides it for every caller@clause (7): one lost-database predicate keyed on the .write.lock marker (Miguel, 2026-11-13)
Q140@docs/PRD.md@^\| \*\*D58\*\* \|@THE ACCEPTED LIMITS\*\*, each with its in-product fix: deleting BOTH the database and .\.write\.lock. reads as a fresh clone@clause (7): the accepted limit of the marker is stated
Q141@docs/PRD.md@^\| \*\*D58\*\* \|@opens the database READ-ONLY WITHOUT migrating@plain doctor is a read-only, non-migrating dry run (Miguel, 2026-11-13)
Q142@docs/PRD.md@^\| \*\*D58\*\* \|@must fail with EXACTLY .SQLITE_NOTADB.@clause (6): a file is quarantined only after exactly SQLITE_NOTADB
Q143@docs/PRD.md@^\| \*\*D58\*\* \|@require .live_connections_to\(db_path\) == 0.@clause (6) step (a2): the window refuses while this process holds a connection to that database file, before any file is read
Q144@docs/PRD.md@^\| \*\*D58\*\* \|@WINDOWS, whose locking takes another VFS path, is DOCUMENTED AS UNMEASURED@criterion (f) is measured on Linux and macOS; Windows is documented as unmeasured
Q145@docs/PRD.md@^\| \*\*D58\*\* \|@.unblock init. gains a DATABASE-ONLY mode@clause (7): the in-product recovery path for a lost database or a changed db_filename (Miguel, 2026-11-13)
Q146@docs/PRD.md@^\| \*\*D46\*\* \|@\*\*\[D58\] AMENDED \(v1\.1, PRD §4 D58 clauses \(1\) and \(8\)\):\*\*@D46 carries the reciprocal note on its doctor exit rule and the stamp comparison
Q147@docs/plans/01-design-spine.md@^    // --- workspace health \(FR-16 full, D58, v1\.1\) — ALL TWELVE@with NO default body@spine §3.2: the two health methods carry no default body, so a delegating wrapper cannot silently stop forwarding
Q148@docs/plans/01-design-spine.md@^pub enum StorageOpenMode \{@ReadOnly@spine §4: the read-only open mode doctor uses
Q149@docs/plans/01-design-spine.md@^    //  never-written \(\[D58\]@REPLACED by the .write_lock_held. probe@spine §3.2 D31 note: the OrphanedLockFile detector is replaced
Q150@docs/plans/01-design-spine.md@^- \*\*Cross-process advisory write lock \(D31 — normative\)\.\*\*@\*\*\[D58\]\*\* its .OrphanedLockFile. detector and the F5 severity map are superseded@spine §3.3 D31 bullet carries the D58 note
Q151@.gitignore@^\.unblock/\.recovery/@:\.unblock/\.recovery/\$@this repository ignores the evidence directory (raw database copies never committed); unblock itself never writes .gitignore (D13)
Q152@docs/plans/ci-cd-and-distribution.md@^\| .exclusivity-macos. \|@locking_mode=EXCLUSIVE@the macOS CI job measuring the exclusivity proof is specified
Q153@docs/PRD.md@^\| \*\*D58\*\* \|@opens .\.unblock/\.write\.lock. WITHOUT .create.@clause (2): the write.lock probe never creates the lost-database marker
Q154@docs/PRD.md@^\| \*\*D58\*\* \|@a COMMITTED .\.write\.lock. makes every clone read as LOST@clause (7): the not-committed-lock assumption and its accepted limit
Q155@docs/PRD.md@^\| \*\*D58\*\* \|@\*\*THE ASSUMPTION:\*\* .\.write\.lock., like the database, is NOT committed@clause (7): the gitignore assumption is stated
Q156@docs/PRD.md@^\| \*\*D58\*\* \|@never through .Storage::migrate\(\).@clause (5): the in-repair migrate runs on the already-held write connection (no Mutex deadlock)
Q157@docs/PRD.md@^\| \*\*D58\*\* \|@SQLITE_READONLY_ROLLBACK@clause (8): a hot journal routes plain doctor to the session-less report
Q158@docs/PRD.md@^\| \*\*D58\*\* \|@.code & 0xff == 26.@clause (6): SQLITE_NOTADB is read on the raw libsql code
Q159@docs/PRD.md@^\| \*\*D58\*\* \|@becomes a REQUIRED status check on .main.@clause (14): the macOS exclusivity job becomes a required check
Q160@docs/PRD.md@^- \*\*FR-14 \[@\*\*\[D58\] \(v1\.1, PRD §4 D58 clause \(7\)\) — a DATABASE-ONLY .init. mode:\*\*@FR-14 carries the note for init's database-only mode
Q161@docs/plans/01-design-spine.md@^\*\*init \(D27/AF-3\)\.\*\*@\*\*\[D58\] \(v1\.1\) — DATABASE-ONLY MODE@spine §5b init describes the database-only mode
Q162@docs/plans/01-design-spine.md@^- \*\*Pre-open health primitives and the read-only open \(D58@opens .\.unblock/\.write\.lock. WITHOUT .create.@spine §3.3: the probe never creates the marker
Q163@docs/plans/crates/unblock-cli.md@^\| .src/commands/init\.rs. \|@DATABASE-ONLY MODE@the cli plan init row carries the database-only mode
Q164@docs/plans/ci-cd-and-distribution.md@^\| .exclusivity-macos. \|@a REQUIRED check\.\*\* .*runs-on: macos-15.*Swatinem/rust-cache@the macOS exclusivity job is required, on the pinned macos-15 image, with the SHA-pinned cache
Q165@docs/plans/implementation-plan.md@^- \*\*T4\.3 — @FR-16 full@(kept) the task names FR-16 full
Q166@docs/plans/implementation-plan.md@^  IMPLEMENTATION commit \(code, doc comments, README@advice that anyone committing .\.unblock/.@T4.3 owns the README ignore advice and recovery path (ruling 376 (4))
Q167@docs/plans/implementation-plan.md@^  ignores .\.unblock/\.write\.lock., .\*\.db.@\.unblock/\.recovery/.@…and the ignore list names the marker, the database files and the evidence directory
Q168@docs/PRD.md@^\| \*\*D58\*\* \|@REFUSED with the EXISTING .ErrorCode::DatabaseError.@clause (7) step (6): database-only init refuses while an orphan -wal/-shm/-journal sits beside the database (ruling 379)
Q169@docs/PRD.md@^\| \*\*D58\*\* \|@AMENDS D27/AF-3.s clobber guard@the D58 header names the AF-3 amendment it reciprocates
Q170@docs/PRD.md@^\| \*\*D27\*\* \|@\*\*\[D58\] AMENDED \(v1\.1, PRD §4 D58 clause \(7\)\):\*\* plain .init. over an existing .config\.toml.@D27/AF-3 carries the reciprocal note on its clobber guard
Q171@docs/PRD.md@^\| \*\*D58\*\* \|@the key is the CANONICALIZED path@clause (5): the Windows registry key, on stable std APIs only
Q172@docs/PRD.md@^\| \*\*D58\*\* \|@it stays .Unsafe. although .unblock init. is its recovery path@clause (1): why database_lost stays Unsafe
Q173@docs/plans/01-design-spine.md@^impl ExclusiveRequest \{@impl ExclusiveRequest \{@spine §3.1: ExclusiveRequest is built through methods (private fields), so it compiles with storage testkit on or off
Q174@docs/plans/01-design-spine.md@^    pub fn with_after_raw_copies\(@#\[cfg|with_after_raw_copies@spine §3.1: the (f6) hook setter exists only under testkit
Q175@docs/plans/01-design-spine.md@^impl ConfigError \{ pub fn is_read_only_rollback@-> bool;@spine §4: the config-side seam the cli (no storage dependency) uses to detect a hot-journal read-only open
Q176@docs/plans/crates/unblock-config.md@^- \*\*\[D58\] \(v1\.1@ConfigError::is_read_only_rollback\(&self\) -> bool.@the config plan specifies the is_read_only_rollback predicate
Q177@docs/PRD.md@^\| \*\*D58\*\* \|@detects it through the new config-side predicate .ConfigError::is_read_only_rollback\(&self\) -> bool.@clause (8): the cli detects ReadOnlyRollback through a config-side seam
Q178@docs/PRD.md@^\| \*\*D58\*\* \|@stop EVERY unblock process on the workspace \(every .unblock mcp. server and every CLI command\) FIRST@clause (7) step (6): the OrphanSidecars message tells the user to stop every unblock process before moving files
Q179@docs/plans/crates/unblock-cli.md@^\| .src/commands/init\.rs. \|@stop every unblock process \(every .unblock mcp. server and CLI\) BEFORE moving them@the cli plan carries the stop-first instruction in the OrphanSidecars message
Q180@docs/PRD.md@^\| \*\*D58\*\* \|@lock-byte range at offset .0x40000000.@clause (6): the Windows re-hash skips SQLite's lock-byte page
"

blocked=0

# PORTABILITY (1 of 2), deliberate. Every `$( … )` in this file substitutes a FUNCTION CALL, never an
# inline loop containing a `case` (macOS `/bin/sh`, bash 3.2, mis-parses a `case` arm's `)` there).
#
# PRODUCTION PREFIXES. A row on one of the files below reads ONLY the lines before that file's
# `mod tests {`, so no test body can satisfy it; a missing or duplicated one is exit 2.
PROD_FILES='crates/unblock-mcp/src/options.rs'
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
      printf '%s\n' "$path: [$code] REQUIRED D58 target is missing from the tree ($reason)"
      continue
    fi
    git grep -q -I -E -e "$re" -- "$path" 2>/dev/null \
      || printf '%s\n' "$path: [$code] the landing is GONE — no line matches /$re/ ($reason)"
  done
  printf '%s\n' "$REQUIRE_NONE" | while IFS='@' read -r code spec re reason; do
    [ -n "$code" ] || continue
    # A pathspec, not a file: no existence check. `git grep` exits 1 on no match, which is the pass.
    hits="$(git grep -n -I -E -e "$re" -- "$spec" 2>/dev/null | grep -v -E "$COMMENT_LINE_RE")"
    if [ -n "$hits" ]; then
      printf '%s\n' "$spec: [$code] forbidden line(s) $(printf '%s' "$hits" | cut -d: -f1,2 | tr '\n' ' ')($reason)"
    fi
  done
  printf '%s\n' "$REQUIRE_ROW" | while IFS='@' read -r code path anchor re reason; do
    [ -n "$code" ] || continue
    if [ ! -e "$path" ]; then
      printf '%s\n' "$path: [$code] REQUIRED D58 target is missing from the tree ($reason)"
      continue
    fi
    rows="$(git grep -n -I -E -e "$anchor" -- "$path" 2>/dev/null)"
    if is_prod_file "$path"; then
      rows="$(printf '%s\n' "$rows" | awk -F: -v end="$(tests_line "$path")" 'NF && $2 + 0 < end + 0')"
    fi
    if [ -z "$rows" ]; then
      printf '%s\n' "$path: [$code] the anchor line /$anchor/ no longer exists, so this pin now proves NOTHING ($reason)"
      continue
    fi
    bad="$(printf '%s\n' "$rows" | grep -v -E -e "$re" | cut -d: -f1,2)"
    if [ -n "$bad" ]; then
      printf '%s\n' "$path: [$code] the anchored line(s) $(printf '%s' "$bad" | tr '\n' ' ') do not match /$re/ ($reason)"
    fi
  done
}

missing="$(check_landings)"
if [ -n "$missing" ]; then
  printf '%s\n' "$missing" >&2
  say "BLOCKED — the D58 health-full specification or its cascade is incomplete; the sites above are missing."
  blocked=1
fi

# SELF-TEST — no rule table may have SHRUNK below the counts it shipped with; the three tables are
# counted SEPARATELY so adding a row to one can never mask the deletion of a row from another.
count_rows() { printf '%s\n' "$1" | grep -c "$2"; }
check_table_floor() { # $1 label, $2 actual, $3 floor
  if [ "$2" -lt "$3" ]; then
    say "BLOCKED — the $1 table has $2 rows; it shipped with $3. A rule was dropped."
    return 1
  fi
  return 0
}
p_count="$(count_rows "$REQUIRE" '^P[0-9]')"
n_count="$(count_rows "$REQUIRE_NONE" '^N[0-9]')"
q_count="$(count_rows "$REQUIRE_ROW" '^Q[0-9]')"

# The floors are the counts this file shipped with; the implementation and test commits RAISE them in
# the same commit as the rows they add. A floor never moves down because a rule became inconvenient.
check_table_floor 'required-landing (P)' "$p_count" 1 || blocked=1
check_table_floor 'negative (N)' "$n_count" 2 || blocked=1
check_table_floor 'row-anchored (Q)' "$q_count" 180 || blocked=1

[ "$blocked" = "0" ] || exit 1
say "OK — the D58 row, its reciprocal notes on D29, D27, D14 and D31, FR-16 and PRD §7/§8.2/§12/§13/RK-5, the spine's health types, trait methods, CF-E withdrawal, pre-open primitives, facade, doctor/recover and §5b command, the crate plans, both roadmaps and the T4.3 task are in the tree; unblock-health names no storage dependency and no source spells VACUUM; the contract is unmoved; the tracker names ub-lp9.3; the live range sits at every prose site and every SIBLING script's knob — including d57's two — and every newest-pointer row names this gate."
exit 0
