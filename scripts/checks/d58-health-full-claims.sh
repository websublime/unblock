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
N3@crates/unblock-cli/src/commands/doctor.rs@integrity_check\(@NEGATIVE: the cli doctor command makes no second integrity_check call — the exit derives from HealthReport.level, one scan per classification (criterion (d), the cli half no in-process counter can observe)
N4@crates/unblock-health/src/*.rs@<{7}|>{7}@NEGATIVE: unblock-health spells no conflict-marker literal — its export scan is unblock-sync's ONE scanner, so health has no scanner of its own (criterion (j))
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
# Q181..   (IMPLEMENTATION commit) the mechanism rows on production lines: the two trait methods and
#      RecordingStorage's recording repair, RecoveryPathOutsideWorkspace, the four pre-open primitives
#      with the no-create probe and the locking_mode=EXCLUSIVE statement, the read-only open, classify
#      and classify_integrity_row with the four index-only shapes, the one scanner call,
#      DatabaseMissing onto DATABASE_NOT_FOUND, StorageOpenMode, create_db and the no_db reader.
#      Q204..Q210 the engine pre-open functions, HealthReport, the typed doctor signature and the cli
#      --repair flag.
# Q211..   (TEST commit) one cell declaration per criterion (a)..(o), the contract cells REGISTERED in
#      run_storage_contract_suite, and the exclusivity-macos job (criterion (q)). N3 is the cli half of
#      criterion (d): no integrity_check( call in the cli doctor command.
# Q243..Q245 (the docs(d58) AMENDMENT commit, tracker comment 385) the inline amendment measured on
#      2026-11-13: SQLite unlinks the rolled-back -journal at close, so the rollback is the repair and
#      its raw/ copy the evidence — PRD clause (6)(e), criterion (g) and spine §3.3 step (e).
# Q258..Q259 pin criterion (j)'s exporter path; N4 its no-own-scanner rule. Q260..Q267 pin the Unsafe gate
#      (engine and cli lines and cells) and the bounded capture (tracker comments 389, 390).
# Q268..Q272 (docs(d58), tracker comment 393) the hot-journal exception, the 64 MiB cap, the uncaptured
#      journal and the original-facts plan; Q273..Q277 their code line and cells.
# Q246..Q257 (the docs(d58) amendment for tracker comments 389, 388 and 390: Unsafe gate, capture before touch) the gate in PRD clause (6),
#      criteria (a), (b) and (h), spine §4.1 recover() and §5b, and the clause (9) confinement hardening.
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
Q142@docs/PRD.md@^\| \*\*D58\*\* \|@must fail with EXACTLY .SQLITE_NOTADB.@clause (6) record (withdrawn by the Unsafe gate, ruling 389): a file would be quarantined only after exactly SQLITE_NOTADB
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
Q181@crates/unblock-storage/src/trait_def.rs@^    async fn health_facts\(@    async fn health_facts\(&self\) -> Result<DbHealthFacts, StorageError>;\$@clause (5): the trait declares the PURE health read with NO default body (a body here would let a delegating double silently stop forwarding)
Q182@crates/unblock-storage/src/trait_def.rs@^    async fn repair\(@    async fn repair\(&self, request: &RepairRequest\) -> Result<RepairOutcome, StorageError>;\$@clause (5): the trait declares the one-call repair with NO default body
Q183@crates/unblock-mcp/tests/common/mod.rs@^        // D58: .repair. is a mutation@so it records@clause (5): RecordingStorage's repair delegation is declared a recording mutation — the line grep pins the declared intent; the mcp zero-write cells carry the record() call itself
Q184@crates/unblock-storage/src/error.rs@^    RecoveryPathOutsideWorkspace \{@    RecoveryPathOutsideWorkspace \{\$@clause (5): the refusal for a snapshot or run directory outside <db dir>/.recovery/ is a typed StorageError variant
Q185@crates/unblock-storage/src/libsql/health.rs@^pub fn lost_database\(@pub fn lost_database\(db_path: &Path\) -> io::Result<bool> \{\$@clause (7): the ONE lost-database predicate is a stat-only local-file primitive
Q186@crates/unblock-storage/src/libsql/health.rs@^pub fn live_connections_to\(@pub fn live_connections_to\(db_path: &Path\) -> io::Result<usize> \{\$@clause (5)/(6): the per-file connection registry the window refuses on (howtocorrupt.html section 2.2)
Q187@crates/unblock-storage/src/libsql/health.rs@^pub fn probe_write_lock\(@pub fn probe_write_lock\(db_path: &Path\) -> Result<bool, StorageError> \{\$@clause (2): the write_lock_held probe exists as a local-file primitive
Q188@crates/unblock-storage/src/libsql/health.rs@^    let file = match OpenOptions::new\(\)@    let file = match OpenOptions::new\(\)\.read\(true\)\.open\(&lock_path\) \{\$@clause (2): the probe opens .write.lock WITHOUT create, so an absent marker stays absent
Q189@crates/unblock-storage/src/libsql/health.rs@^pub async fn exclusive_window\(@pub async fn exclusive_window\(\$@clause (6): the pre-open exclusive window is a local-file primitive outside the trait
Q190@crates/unblock-storage/src/libsql/health.rs@^    let mut rows = conn\.query\(\"PRAGMA locking_mode=@\"PRAGMA locking_mode=EXCLUSIVE\", \(\)@clause (6): the window's first executed statement is locking_mode=EXCLUSIVE — BEGIN EXCLUSIVE alone proves nothing in WAL mode
Q191@crates/unblock-storage/src/libsql/mod.rs@^    pub async fn open_local_read_only\(@\(path: &Path\) -> Result<Self, StorageError> \{\$@clause (8): the read-only, non-migrating open plain doctor uses exists on LibsqlStorage
Q192@crates/unblock-health/src/classify.rs@^pub fn classify\(@pub fn classify\(\$@clause (4): the ONE classifier is a plain (non-async) pub fn in unblock-health
Q193@crates/unblock-health/src/integrity.rs@^pub fn classify_integrity_row\(@\(row: &str, index_names: &\[String\]\) -> AnomalyClass \{\$@clause (3): the integrity-row classifier takes the sqlite_schema index names
Q194@crates/unblock-health/src/integrity.rs@^    let index = if let Some\(rest\) = message\.strip_prefix\(@\"wrong # of entries in index \"@clause (3): the index-only shape wrong # of entries in index <I>
Q195@crates/unblock-health/src/integrity.rs@^    \} else if let Some\(rest\) = message\.strip_prefix\(\"rowid@\"rowid not at end-of-record for row \"@clause (3): the index-only shape rowid not at end-of-record for row <N> of index <I>
Q196@crates/unblock-health/src/integrity.rs@^        after_row_number\(rest, \" missing@\" missing from index \"@clause (3): the index-only shape row <N> missing from index <I>
Q197@crates/unblock-health/src/integrity.rs@^            \.or_else\(\|\| after_row_number\(rest, \" values@\" values differ from index \"@clause (3): the index-only shape row <N> values differ from index <I>
Q198@crates/unblock-health/src/export.rs@^    let scan = match scan_conflict_markers\(@scan_conflict_markers\(path\)@clause (4): health's export scan calls unblock-sync's ONE conflict-marker scanner
Q199@crates/unblock-config/src/error.rs@^    DatabaseMissing \{@    DatabaseMissing \{\$@clause (7): the lost-database refusal is a typed ConfigError variant
Q200@crates/unblock-config/src/error.rs@^            Self::DatabaseMissing \{ \.\. \} => ErrorCode::@=> ErrorCode::DatabaseNotFound,\$@clause (7): DatabaseMissing rides the EXISTING DATABASE_NOT_FOUND code — no ErrorCode minted
Q201@crates/unblock-config/src/cli.rs@^pub enum StorageOpenMode \{@pub enum StorageOpenMode \{\$@clause (8): the open modes (Migrate, ReadOnly, NoMigrate) exist on the config facade
Q202@crates/unblock-config/src/cli.rs@^    pub create_db: @    pub create_db: bool,\$@clause (7): init's create-over-the-marker flag exists on CliOverrides
Q203@crates/unblock-config/src/context.rs@^    if cli\.no_db \{@    if cli\.no_db \{\$@clause (8): open_with_cli is the FIRST reader of the reserved no_db seam
Q204@crates/unblock-engine/src/preopen.rs@^pub fn gather_preopen\(@pub fn gather_preopen\(paths: &ConfigPaths\) -> Result<PreOpenFacts> \{\$@clause (11): the session-less pre-open fact gatherer takes config's ConfigPaths
Q205@crates/unblock-engine/src/preopen.rs@^pub fn classify_preopen\(@pub fn classify_preopen\(facts: &PreOpenFacts\) -> HealthReport \{\$@clause (8): the session-less classification the cli renders alone when the facts forbid a read-only open
Q206@crates/unblock-engine/src/preopen.rs@^pub async fn repair_preopen\(@pub async fn repair_preopen\(\$@clause (11): the pre-open repair entry runs the exclusive window outside any Session
Q207@crates/unblock-engine/src/preopen.rs@^pub struct HealthReport \{@pub struct HealthReport \{\$@clause (8): the engine-local typed report carries the level the cli exits by
Q208@crates/unblock-engine/src/session/lifecycle.rs@^    pub async fn doctor\(@pub async fn doctor\(&self, preopen: &PreOpenFacts\) -> Result<HealthReport> \{\$@clause (8): doctor takes the pre-open facts and returns the typed HealthReport
Q209@crates/unblock-cli/src/cli.rs@^pub struct DoctorArgs \{@pub struct DoctorArgs \{\$@clause (8): the doctor command carries arguments
Q210@crates/unblock-cli/src/cli.rs@^    pub repair: bool,@pub repair: bool,\$@clause (8): --repair is the doctor flag (D3: a lifecycle flag, nothing reaches MCP)
Q211@crates/unblock-health/tests/classify_props.rs@fn class_table_golden\(@fn class_table_golden\(\)@criterion (a): the golden class table cell
Q212@crates/unblock-health/tests/classify_props.rs@fn adding_a_class_never_lowers_the_level\(@fn adding_a_class_never_lowers_the_level\(@criterion (a): the composite never drops when a class is added
Q213@crates/unblock-health/tests/classify_props.rs@fn no_unsafe_class_plans_an_executable_action\(@fn no_unsafe_class_plans_an_executable_action\(@criterion (a) as amended (tracker comment 389): no Unsafe class plans any executable action
Q214@crates/unblock-health/tests/integrity_rows.rs@fn each_index_only_shape_with_and_without_prefix_is_index_corruption\(@fn each_index_only_shape_with_and_without_prefix_is_index_corruption\(@criterion (b): the four index-only shapes, prefixed and bare
Q215@crates/unblock-health/tests/integrity_rows.rs@fn every_other_row_is_page_corruption_and_unsafe\(@fn every_other_row_is_page_corruption_and_unsafe\(@criterion (b): the fail-safe Unsafe for every other row
Q216@crates/unblock-cli/tests/migrate_doctor.rs@fn doctor_index_entry_removed_reports_recoverable_exit_2_and_repair_leaves_integrity_clean_exit_0\(@fn doctor_index_entry_removed_reports_recoverable_exit_2_and_repair_leaves_integrity_clean_exit_0\(@criterion (b) end to end: a removed index entry is Recoverable and --repair leaves integrity clean
Q217@crates/unblock-engine/tests/health.rs@fn drift_child_counter_reports_drifted_and_recover_raises_it_then_next_create_mints_the_following_number\(@fn drift_child_counter_reports_drifted_and_recover_raises_it_then_next_create_mints_the_following_number\(@criterion (c): the child-counter drift cell
Q218@crates/unblock-engine/tests/health.rs@fn duplicate_metadata_key_recover_keeps_the_latest_and_snapshot_holds_both\(@fn duplicate_metadata_key_recover_keeps_the_latest_and_snapshot_holds_both\(@criterion (c): the greatest-rowid metadata keep rule cell
Q219@crates/unblock-engine/tests/health.rs@fn schema_stamp_behind_recover_migrates_after_the_snapshot_which_keeps_the_old_stamp\(@fn schema_stamp_behind_recover_migrates_after_the_snapshot_which_keeps_the_old_stamp\(@criterion (c): the stamp-behind cell (migrate after the snapshot)
Q220@crates/unblock-engine/tests/health.rs@fn doctor_makes_exactly_one_health_facts_call_and_no_integrity_check_call\(@fn doctor_makes_exactly_one_health_facts_call_and_no_integrity_check_call\(@criterion (d): one scan per doctor, through the testkit counters
Q221@crates/unblock-engine/tests/health.rs@fn recover_makes_exactly_one_health_facts_call\(@fn recover_makes_exactly_one_health_facts_call\(@criterion (d): one health_facts call per recover
Q222@crates/unblock-cli/tests/migrate_doctor.rs@fn doctor_recoverable_exits_2\(@fn doctor_recoverable_exits_2\(@criterion (d): the exit by level, Recoverable -> 2
Q223@crates/unblock-engine/tests/dangling.rs@fn doctor_folds_in_the_same_dangling_findings_after_the_file_state_rows\(@fn doctor_folds_in_the_same_dangling_findings_after_the_file_state_rows\(@criterion (e): the D45 dangling block stays the trailing suffix of the typed report
Q224@crates/unblock-storage/tests/exclusive_window.rs@fn f1_second_process_reader_refuses_database_open_elsewhere\(@fn f1_second_process_reader_refuses_database_open_elsewhere\(@criterion (f1): a live reader in another process is refused, MEASURED
Q225@crates/unblock-storage/tests/exclusive_window.rs@fn f5_control_begin_exclusive_without_locking_mode_misses_a_wal_reader\(@fn f5_control_begin_exclusive_without_locking_mode_misses_a_wal_reader\(@criterion (f5): the control cell proving BEGIN EXCLUSIVE alone misses a WAL reader
Q226@crates/unblock-storage/tests/exclusive_window.rs@fn f4_same_process_store_refuses_process_holds_connection_and_creates_no_raw\(@fn f4_same_process_store_refuses_process_holds_connection_and_creates_no_raw\(@criteria (f4)/(n): a same-process connection refuses before any raw copy
Q227@crates/unblock-cli/tests/migrate_doctor.rs@fn repair_lone_shm_quarantined_final_healthy\(@fn repair_lone_shm_quarantined_final_healthy\(@criterion (g): a lone -shm is quarantined and the final level is Healthy
Q228@crates/unblock-cli/tests/init_agents.rs@fn init_database_only_recreates_only_the_database_under_a_custom_db_filename\(@fn init_database_only_recreates_only_the_database_under_a_custom_db_filename\(@criterion (h): init's database-only mode under a custom db_filename
Q229@crates/unblock-cli/tests/migrate_doctor.rs@fn doctor_lost_database_reports_unsafe_exit_2_opens_nothing_names_recovery_path\(@fn doctor_lost_database_reports_unsafe_exit_2_opens_nothing_names_recovery_path\(@criterion (h): the lost database is Unsafe, opens nothing and names the recovery path
Q230@crates/unblock-engine/tests/health.rs@fn recover_creates_exactly_one_run_dir_with_manifest_hashes_and_a_passing_snapshot\(@fn recover_creates_exactly_one_run_dir_with_manifest_hashes_and_a_passing_snapshot\(@criterion (i): one run directory per phase, hashed manifest, a snapshot that passes integrity_check
Q231@crates/unblock-cli/tests/migrate_doctor.rs@fn plain_doctor_creates_no_recovery_entry_and_leaves_db_and_write_lock_untouched\(@fn plain_doctor_creates_no_recovery_entry_and_leaves_db_and_write_lock_untouched\(@criterion (i): plain doctor is a dry run that writes nothing
Q232@crates/unblock-engine/tests/health.rs@fn conflict_markers_recover_copies_regenerates_and_reports_not_imported_rows\(@fn conflict_markers_recover_copies_regenerates_and_reports_not_imported_rows\(@criterion (j): the conflicted export is copied, regenerated and its unimported rows reported
Q233@crates/unblock-engine/tests/health.rs@fn recover_failure_in_second_action_leaves_first_unapplied_records_failed_and_returns_the_error\(@fn recover_failure_in_second_action_leaves_first_unapplied_records_failed_and_returns_the_error\(@criterion (k): one action transaction, failed recorded, the error returned
Q234@crates/unblock-engine/tests/health.rs@fn recover_holds_permit_and_write_lock_through_classification_and_repair\(@fn recover_holds_permit_and_write_lock_through_classification_and_repair\(@criterion (k): recover holds the permit and .write.lock from classification to the end
Q235@crates/unblock-storage/src/testkit.rs@^    contract_repair_never_lowers_a_counter\(@contract_repair_never_lowers_a_counter\(factory\(\)\.await\)\.await;\$@criterion (l): the never-lowering repair cell is REGISTERED in run_storage_contract_suite
Q236@crates/unblock-storage/src/testkit.rs@^    contract_health_facts_reports_drift_and_duplicates_in_key_order\(@contract_health_facts_reports_drift_and_duplicates_in_key_order\(factory\(\)\.await\)\.await;\$@criterion (l): the key-ordered health facts cell is REGISTERED in run_storage_contract_suite
Q237@crates/unblock-health/tests/classify_props.rs@^\) -> WorkspaceClassification = classify;@WorkspaceClassification = classify;\$@criterion (m): classify is a non-async fn, pinned by a const function-pointer binding
Q238@crates/unblock-engine/tests/health.rs@fn gather_preopen_with_a_live_session_is_refused_pre_open_with_live_connection\(@fn gather_preopen_with_a_live_session_is_refused_pre_open_with_live_connection\(@criterion (n): gather_preopen refuses while a session holds a connection
Q239@crates/unblock-engine/tests/scale.rs@^    seed_health_shape\(@seed_health_shape\(&storage, HEALTH_PARENTS, CHILDREN_PER_PARENT\)@criterion (o): the 250k scale corpus is seeded with the health shape
Q240@.github/workflows/ci.yml@^  exclusivity-macos:@exclusivity-macos:\$@criterion (q): the exclusivity-macos job exists
Q241@.github/workflows/ci.yml@^    runs-on: macos-15@runs-on: macos-15\$@criterion (q): it runs on the pinned macos-15 image
Q242@.github/workflows/ci.yml@--test exclusive_window@run: cargo test -p unblock-storage --features testkit --locked --test exclusive_window\$@criterion (q): every step naming the exclusive_window file RUNS it (storage-testkit on Linux, exclusivity-macos on macOS)
Q243@docs/PRD.md@^\| \*\*D58\*\* \|@\[D58 amendment, measured 2026-11-13 on macOS with libsql's SQLite 3\.45\.1 and SQLite 3\.50\.4: SQLite unlinks the rolled-back journal at close@clause (6)(e) as amended (tracker comment 385): the hot-journal rollback is the quarantine_journal repair, its evidence the raw/ copy, and no journal_mode=PERSIST
Q244@docs/PRD.md@^\| \*\*D58\*\* \|@the \`-journal\` is ABSENT after the close, like the \`-wal\` \*\*\[D58 amendment@criterion (g) as amended: the rolled-back -journal is absent after the close
Q245@docs/plans/01-design-spine.md@^- \*\*Pre-open health primitives and the read-only open@SQLite likewise unlinks a hot \`-journal\` it rolled back in \(c\), so \`quarantine_journal\` renames nothing@spine §3.3 step (e) as amended: the rolled-back journal is never renamed
Q246@docs/PRD.md@^\| \*\*D58\*\* \|@THE UNSAFE GATE \[D58 amendment, Miguel's ruling 2026-11-16, tracker comment 389\]:\*\* when ANY .Unsafe. class is present@clause (6) as amended (tracker comment 389): any Unsafe class makes doctor --repair repair and write nothing
Q247@docs/PRD.md@^\| \*\*D58\*\* \|@the .quarantine_database. exception is withdrawn@criterion (a) as amended: no Unsafe class has any executable action
Q248@docs/PRD.md@^\| \*\*D58\*\* \|@THE UNSAFE GATE:\*\* page corruption together with child-counter drift, and .schema_stamp_ahead. together with drift@criterion (b) as amended: the two Unsafe-gate cells the amendment names
Q249@docs/PRD.md@^\| \*\*D58\*\* \|@and .--repair. moves nothing, creates no run directory and exits 2@criterion (h) as amended: a non-SQLite database is never moved by --repair
Q250@docs/plans/01-design-spine.md@\(3\) classify the ONLINE classes@\(3b\) \*\*\[D58 amendment, tracker comment 389\] THE UNSAFE GATE:\*\*@spine §4.1 recover() as amended: the online Unsafe gate before any run directory
Q251@docs/plans/01-design-spine.md@^\*\*doctor — FR-16 full \(D58@\*\*\(i-b\) \[D58 amendment, tracker comment 389\] THE UNSAFE GATE\.\*\*@spine §5b as amended: --repair runs the plain-doctor classification first and stops on any Unsafe class
Q252@docs/PRD.md@^\| \*\*D58\*\* \|@\[D58 amendment, Verify round 1, tracker comment 388:\*\* .\.unblock/\.recovery/. and every directory under it must be a real directory@clause (9) as amended: the lstat/identity confinement, 0700/0600 evidence and the guarded export copy
Q253@docs/PRD.md@^\| \*\*D58\*\* \|@\(i-a\) \*\*\[D58 amendment, Miguel's ruling 2026-11-16, tracker comment 390\] CAPTURE BEFORE TOUCH:\*\* with NO libsql connection open@clause (8) as amended (tracker comment 390): --repair captures the anomalous sidecars in memory before the gate's read-only open
Q254@docs/PRD.md@^\| \*\*D58\*\* \|@the window quarantines the REBUILT copy and the manifest records the original bytes and the rebuild@clause (8) as amended: a lone -shm is resolved by SQLite's wal-index rebuild, with the original kept as evidence
Q255@docs/PRD.md@^\| \*\*D58\*\* \|@whose SHA-256 equals the .-shm. as it was before the run, and the cell fails if the capture moves after that open@criterion (g) as amended: the lone -shm original is the pre-run bytes
Q256@docs/plans/01-design-spine.md@^    //   pub async fn capture_sidecars\(@capture_sidecars\(paths: &ConfigPaths, facts: &PreOpenFacts\) -> Result<SidecarCapture, EngineError>;@spine §4.1 as amended: the capture-before-touch entry
Q257@docs/plans/01-design-spine.md@^    //   pub async fn repair_preopen\(@repair_preopen\(paths: &ConfigPaths, planned: &\[PlannedRepair\], capture: SidecarCapture\)@spine §4.1 as amended: repair_preopen takes the pre-gate capture
Q258@crates/unblock-engine/src/session/interchange.rs@^        let report = unblock_sync::export_jsonl\(@unblock_sync::export_jsonl\(\$@criterion (j): the export regeneration goes through unblock-sync's atomic exporter
Q259@crates/unblock-engine/src/session/lifecycle.rs@^            match self\.write_export\(@match self\.write_export\(&self\.jsonl_path\)\.await \{\$@criterion (j): recover() regenerates the export through that one exporter path
Q260@crates/unblock-engine/src/session/lifecycle.rs@^        if classification\.level == HealthLevel::Unsafe \{@if classification\.level == HealthLevel::Unsafe \{\$@spine §4.1 (3b) as amended (tracker comment 389): recover() stops on any Unsafe class before any run directory
Q261@crates/unblock-cli/src/commands/doctor.rs@^fn unsafe_gate_stops_repair\(@fn unsafe_gate_stops_repair\(level: HealthLevel\) -> bool \{\$@spine §5b (i-b) as amended: the cli Unsafe gate decision
Q262@crates/unblock-engine/src/preopen.rs@^pub async fn capture_sidecars\(@pub async fn capture_sidecars\(paths: &ConfigPaths, facts: &PreOpenFacts\)@spine §4.1 as amended (tracker comment 390): the capture-before-touch entry exists
Q263@crates/unblock-engine/tests/health.rs@fn recover_with_page_corruption_and_drift_repairs_nothing_and_creates_no_run_dir\(@fn recover_with_page_corruption_and_drift_repairs_nothing_and_creates_no_run_dir\(@criterion (b) as amended: the engine Unsafe-gate cell for page corruption plus drift
Q264@crates/unblock-engine/tests/health.rs@fn recover_with_schema_stamp_ahead_and_drift_repairs_nothing_and_creates_no_run_dir\(@fn recover_with_schema_stamp_ahead_and_drift_repairs_nothing_and_creates_no_run_dir\(@criterion (b) as amended: the engine Unsafe-gate cell for schema_stamp_ahead plus drift
Q265@crates/unblock-cli/tests/migrate_doctor.rs@fn repair_with_page_corruption_and_drift_repairs_nothing_exit_2_no_run_dir\(@fn repair_with_page_corruption_and_drift_repairs_nothing_exit_2_no_run_dir\(@criterion (b) as amended: the cli Unsafe-gate cell for page corruption plus drift
Q266@crates/unblock-cli/tests/migrate_doctor.rs@fn repair_with_schema_stamp_ahead_and_drift_repairs_nothing_exit_2_no_run_dir\(@fn repair_with_schema_stamp_ahead_and_drift_repairs_nothing_exit_2_no_run_dir\(@criterion (b) as amended: the cli Unsafe-gate cell for schema_stamp_ahead plus drift
Q267@crates/unblock-engine/tests/health.rs@fn capture_sidecars_refuses_a_shm_above_the_cap\(@fn capture_sidecars_refuses_a_shm_above_the_cap\(@clause (8) as amended (tracker comments 390, 393): the capture is bounded at 64 MiB and refuses above it
Q268@docs/PRD.md@^\| \*\*D58\*\* \|@no mutation beyond SQLite's own rollback of a hot journal\. A hot .-journal. is the documented exception@clause (6) as amended (tracker comment 393): the hot-journal exception to the Unsafe gate — raw/ evidence, then the rollback, then recover()'s gate
Q269@docs/PRD.md@^\| \*\*D58\*\* \|@the .-journal. is NOT captured in memory@clause (8) as amended (tracker comment 393): only the anomalous -shm/-wal are captured; the journal's evidence is raw/
Q270@docs/PRD.md@^\| \*\*D58\*\* \|@the capture cap is 64 MiB \(.preopen::CAPTURE_CAP_BYTES.\)@clause (8) as amended (tracker comment 393): the 64 MiB capture cap
Q271@docs/plans/01-design-spine.md@^\*\*doctor — FR-16 full \(D58@Step \(ii\) plans the exclusive actions from the ORIGINAL phase \(i\) facts and does NOT gather them again@spine §5b as amended (tracker comment 393): step (ii) plans from the original facts
Q272@docs/plans/01-design-spine.md@^    //   pub const CAPTURE_CAP_BYTES@pub const CAPTURE_CAP_BYTES: u64 = 64 \* 1024 \* 1024;@spine §4.1 as amended (tracker comment 393): the capture cap constant
Q273@crates/unblock-engine/src/preopen.rs@^pub const CAPTURE_CAP_BYTES@pub const CAPTURE_CAP_BYTES: u64 = 64 \* 1024 \* 1024;\$@the 64 MiB capture cap in code
Q274@crates/unblock-cli/tests/migrate_doctor.rs@fn repair_hot_journal_beside_page_corruption_rolls_back_and_repairs_no_sql_exit_2\(@fn repair_hot_journal_beside_page_corruption_rolls_back_and_repairs_no_sql_exit_2\(@criterion (b) as amended (tracker comment 393): hot journal + page corruption is rolled back with raw/ evidence and no SQL repair, exit 2
Q275@crates/unblock-cli/tests/migrate_doctor.rs@fn repair_refuses_a_sidecar_it_cannot_capture_exit_2_and_writes_nothing\(@fn repair_refuses_a_sidecar_it_cannot_capture_exit_2_and_writes_nothing\(@clause (8) as amended: a capture refusal is DATABASE_ERROR, exit 2, with nothing written
Q276@crates/unblock-engine/tests/health.rs@fn capture_sidecars_leaves_a_hot_journal_to_the_raw_copy\(@fn capture_sidecars_leaves_a_hot_journal_to_the_raw_copy\(@clause (8) as amended: the -journal is not captured in memory; raw/ holds it
Q277@crates/unblock-engine/tests/health.rs@fn capture_sidecars_keeps_a_truncated_wal_as_its_original\(@fn capture_sidecars_keeps_a_truncated_wal_as_its_original\(@clause (8) as amended: a truncated -wal is captured as its original
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
check_table_floor 'negative (N)' "$n_count" 4 || blocked=1
check_table_floor 'row-anchored (Q)' "$q_count" 277 || blocked=1

[ "$blocked" = "0" ] || exit 1
say "OK — the D58 row, its reciprocal notes on D29, D27, D14 and D31, FR-16 and PRD §7/§8.2/§12/§13/RK-5, the spine's health types, trait methods, CF-E withdrawal, pre-open primitives, facade, doctor/recover and §5b command, the crate plans, both roadmaps and the T4.3 task are in the tree; unblock-health names no storage dependency and no source spells VACUUM; the contract is unmoved; the tracker names ub-lp9.3; the live range sits at every prose site and every SIBLING script's knob — including d57's two — and every newest-pointer row names this gate."
exit 0
