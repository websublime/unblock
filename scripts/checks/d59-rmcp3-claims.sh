#!/bin/sh
# d59-rmcp3-claims.sh — the REQUIRED-LANDING gate for D59: the MCP SDK moves from rmcp 1.7.0 to 3.10.0
# (RUSTSEC-2026-0462) with the semver-compatible quinn-udp 0.5.16 (RUSTSEC-2026-0538) and rustix 1.1.6
# (RUSTSEC-2026-0486) lock bumps; the wire converges on rmcp 3 except D54, which is KEPT; the server
# serves both MCP lifecycles, `initialize` and the stateless 2026-07-28 one with `server/discover`; the
# D47 arm and the CD-4 clamp are RETIRED and the D50 gate is redesigned (PRD §4 D59, tracked as
# `ub-9ov`). Spec: docs/plans/ci-cd-and-distribution.md §2.1, the paragraph beginning "Named sub-check
# (its D59 sibling…)". Runs as a step of the required `doc-lint` job, immediately after its `d58` sibling.
#
# THAT PARAGRAPH IS NORMATIVE OVER THIS FILE. Every landing enforced here is named there; a rule that
# exists in one and not the other is a defect to be fixed in the SAME change.
#
# MINTED IN THE SPEC COMMIT, as D58's gate was. So on its first commit this file pins only what the
# SPEC commit lands: the D59 row and its reciprocal notes, the spine, the crate plans, both roadmaps and
# the task, the D-range cascade, the newest-pointer rows retargeted to this script, every older
# sibling's knobs, and the contract id as it stands. The IMPLEMENTATION and TEST commits EXTEND it with
# the manifest, mechanism and cell rows the ci-cd paragraph lists, each row landing in the SAME commit
# as the line it pins, and raise the floors below in that commit. A row is never added before its line
# exists.
#
# WHY THIS ONE IS POSITIVE-ONLY, save TEN negative rows. A negative prose sweep is dodged by a claim
# REWRAPPED across two lines, so every prose row is a positive landing. The negative rows are
# configuration and code rules, not prose sweeps: N1 and N2 — no non-comment line of deny.toml or of
# any file under .cargo/ names one of the three advisories, so neither the audit job nor the deny job
# can go green through an ignore (criterion (b)); N3 and N4 — no non-comment line under
# crates/unblock-mcp or crates/unblock-fuzz spells an allow(…) or expect(…) attribute naming
# deprecated (a method call spelled .expect( is not an attribute and is excluded), N5..N7 — no
# deprecated lint level in the workspace manifest or those two crates' manifests, a [lints.rust.deprecated]
# table included, N8 and N9 — no bare `deprecated` entry line, which is how a multi-line attribute names
# it, and N10 — no rustflags under .cargo/ that allow deprecated or cap lints, so no rmcp 3 deprecation is
# silenced (criterion (p), widened by design Review rounds 1 and 2). All ten pass on the pre-fix tree by
# design: they pin what already holds and must keep holding.
#
# WHAT NO ROW HERE CAN SEE, stated so no row is over-read. A line grep cannot prove that rmcp 3.10 still
# writes nothing for invalid JSON, that the gate's start predicate agrees with rmcp's pre-lifecycle loop,
# that a cancelled request's mutation commits, or that the `initialize` answers equal the pre-D59 ones.
# The differential, lifecycle and wire cells D59 clause (17) names carry those.
#
# THE CONTRACT KNOB MOVED WITH THE IMPLEMENTATION COMMIT. The spec commit moved no contract byte and
# pinned `unblock.mcp.v1.12`; the implementation commit moved the constant, every sibling's CONTRACT_RE,
# this one and Q27's live minor to `unblock.mcp.v1.13` together (ci-cd §2.1).
#
# PRE-FIX TREE: `main` at 60d9551, the tree before D59's spec commit. Every row asserting a NEW landing
# FAILS there. Exactly the rows that pin what already holds pass: P1 and P2 (both ids are already
# exported), N1..N10, the sibling CONTRACT_RE rows and the code-constant row.
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
say() { printf 'd59-claims: %s\n' "$*" >&2; }

git rev-parse --show-toplevel >/dev/null 2>&1 || { say "not a git repository"; exit 2; }
cd "$(git rev-parse --show-toplevel)" || { say "cannot cd to the repo root"; exit 2; }

# The LIVE D-id range, in its TWO spellings. It tracks the LIVE range, never a frozen historical one:
# the day a D60 is minted, every file `docs/PROCESS.md` §3 enumerates moves with it or a required step
# goes red. `xtask/src/doc_lint.rs`'s bump site is ONE physical line carrying BOTH halves, the prose
# range `(D1..D59)` and the tokenizer's ALTERNATION `\bD(59|58|57|…)\b`; pinning only the prose is how
# that site rots into an undefined-D60 finding.
RANGE_RE='D1\.\.D59'
RANGE_ALT_RE='D\(59\|58\|'

# The LIVE published contract version. The D59 SPEC commit does NOT move it; its IMPLEMENTATION commit
# moves it to unblock\.mcp\.v1\.13 here and in every sibling's knob, with the code constant.
CONTRACT_RE='unblock\.mcp\.v1\.13'

# The SAME spellings as they appear INSIDE a sibling script's knob line, where each backslash is a
# literal byte. DERIVED, never hand-written a second time.
knob_re() { printf '%s' "$1" | sed 's/\\/\\\\\\/g'; }
RANGE_KNOB_RE="$(knob_re "$RANGE_RE")"
RANGE_KNOB_ALT_RE="$(knob_re "$RANGE_ALT_RE")"
CONTRACT_KNOB_RE="$(knob_re "$CONTRACT_RE")"

# The negative rows' patterns, held in variables (`$@`-safe). They run over `git grep -n` output
# (`path:lineno:text`). A comment line is one whose first non-blank characters are `//` (Rust, doc
# comments included) or a `#` that does NOT open a Rust attribute (`#[` / `#![`), so a TOML comment
# is skipped while `#[allow(deprecated)]` counts as code.
COMMENT_LINE_RE='^[^:]+:[0-9]+:[[:space:]]*(//|#([^[!]|$))'
ADVISORY_RE='RUSTSEC-2026-0462|RUSTSEC-2026-0538|RUSTSEC-2026-0486'
ALLOW_DEPRECATED_RE='(^|[^.[:alnum:]_])(allow|expect)\([^)]*deprecated'
DEPRECATED_ATTR_LINE_RE='^[[:space:]]*deprecated[[:space:]]*([,)]|$)'
DEPRECATED_LINT_RE='deprecated[[:space:]]*=|lints\.rust\.deprecated'
DEPRECATED_FLAG_RE='(-A|--allow)[^]]*deprecated|--cap-lints'
# Any RustSec id EXCEPT the one dated ignore D59's 2026-11-17 amendment permits (RUSTSEC-2026-0543):
# another number in any year, or any number in another year.
OTHER_ADVISORY_RE='RUSTSEC-[0-9]{4}-([1-9][0-9]{3}|0[0-46-9][0-9]{2}|05[0-35-9][0-9]|054[0-24-9])|RUSTSEC-([013-9][0-9]{3}|2[1-9][0-9]{2}|20[013-9][0-9]|202[0-57-9])-[0-9]{4}'
# An advisory-ignore input or flag in a CI workflow (audit-check's `ignore:`, `--ignore …`).
CI_IGNORE_RE='^ +ignore: |--ignore[ =]'

# =================================================================================================
# REQUIRED LANDINGS — `code@path@regex@what it proves`
# =================================================================================================
REQUIRE="
P1@.unblock/issues.jsonl@\"id\":\"ub-9ov\"@the tracker record names the work D59 implements. The export keeps closed rows, so the row survives the merge-time close
P3@Cargo.lock@^checksum = .b5949e504d5ab9944a168259afc0c39ac0230627b6ec5367ace194c59bcbe163.@criterion (a): Cargo.lock carries rmcp 3.10.0 (its registry checksum, a line no other version can carry)
P4@Cargo.lock@^checksum = .b98a686693ee91b665c6f89f182fca6e141c8a63dc113b11b38c1bf3491606b7.@criterion (a): Cargo.lock carries rmcp-macros 3.10.0
P5@Cargo.lock@^checksum = .af66907df18639dcf4db56ca65490cabc4b27a97dbadd96f2926cca73298f016.@criterion (a): Cargo.lock carries quinn-udp 0.5.16 (RUSTSEC-2026-0538)
P6@Cargo.lock@^checksum = .e5e76742e5294153d4dde1207dbec5bdd81eef6fabdc0d2118babadabfd858aa.@criterion (a): Cargo.lock carries rustix 1.1.6 (RUSTSEC-2026-0486)
P7@Cargo.lock@^checksum = .fdb5bc1ae2baa591800df16c9ca78619bf65c0488b41b96ccec5d11220d8c154.@criterion (a): rustix 0.38.44, outside the advisory, is KEPT
P8@deny.toml@^ +\{ id = .RUSTSEC-2026-0543., reason = .*ub-4db@[D59 amendment, 2026-11-17] criterion (b): deny.toml carries the ONE dated ignore, RUSTSEC-2026-0543 (chrono unmaintained, an unconditional rmcp 3.10 dependency), its reason naming ub-4db, which removes it
P2@.unblock/issues.jsonl@\"id\":\"ub-o8s\"@the 16 MiB line-bound residual D59 clause (12) discloses as OUT of scope is a real, exported issue
"

# =================================================================================================
# NEGATIVE LANDINGS — `code@pathspec@regex@what it proves`
# =================================================================================================
REQUIRE_NONE="
N1@deny.toml@$ADVISORY_RE@NEGATIVE: deny.toml ignores none of the three advisories D59 fixes, so cargo deny cannot go green through an ignore (criterion (b))
N2@.cargo@$ADVISORY_RE@NEGATIVE: no cargo or cargo-audit configuration under .cargo/ ignores one of the three advisories, so the audit job cannot go green through an ignore (criterion (b))
N3@crates/unblock-mcp@$ALLOW_DEPRECATED_RE@NEGATIVE: unblock-mcp silences no deprecation — no allow(…) or expect(…) attribute names deprecated, alone or in a list — so every rmcp 3 deprecation (ServerInfo, ClientInfo) is migrated rather than allowed (criterion (p))
N4@crates/unblock-fuzz@$ALLOW_DEPRECATED_RE@NEGATIVE: unblock-fuzz silences no deprecation either (criterion (p))
N5@Cargo.toml@$DEPRECATED_LINT_RE@NEGATIVE: the workspace manifest sets no deprecated lint level in [workspace.lints], so no table-level allow can stand in for the attribute N3 forbids (criterion (p))
N6@crates/unblock-mcp/Cargo.toml@$DEPRECATED_LINT_RE@NEGATIVE: unblock-mcp's manifest sets no deprecated lint level in its [lints] table (criterion (p))
N7@crates/unblock-fuzz/Cargo.toml@$DEPRECATED_LINT_RE@NEGATIVE: unblock-fuzz's manifest sets no deprecated lint level in its [lints] table (criterion (p))
N8@crates/unblock-mcp@$DEPRECATED_ATTR_LINE_RE@NEGATIVE: no line of unblock-mcp is a bare deprecated list entry, with a trailing comma, a closing paren or nothing after it (a final entry), which is how a multi-line allow( or expect( attribute names the lint on its own line (criterion (p), design Review rounds 2 and 3)
N9@crates/unblock-fuzz@$DEPRECATED_ATTR_LINE_RE@NEGATIVE: the same for unblock-fuzz (criterion (p))
N19@deny.toml@$OTHER_ADVISORY_RE@NEGATIVE: [D59 amendment, 2026-11-17] deny.toml names no advisory other than the one dated ignore, RUSTSEC-2026-0543 (criterion (b))
N20@.github/workflows@$CI_IGNORE_RE@NEGATIVE: [D59 amendment, 2026-11-17] no CI workflow passes an advisory ignore to the audit or deny step (criterion (b))
N11@Cargo.lock@0810a9f717d9828f475fe1f629f4c305c8464b7f496c3a854b58d29e65f4058e@NEGATIVE: criterion (a) — no rmcp 1.7.0 entry survives in Cargo.lock (its registry checksum)
N12@Cargo.lock@6aefac48c364756e97f04c0401ba3231e8607882c7c1d92da0437dc16307904d@NEGATIVE: criterion (a) — no rmcp-macros 1.7.0 entry survives
N13@Cargo.lock@addec6a0dcad8a8d96a771f815f0eaf55f9d1805756410b39f5fa81332574cbd@NEGATIVE: criterion (a) — no quinn-udp 0.5.14 entry survives (RUSTSEC-2026-0538)
N14@Cargo.lock@b6fe4565b9518b83ef4f91bb47ce29620ca828bd32cb7e408f0062e9930ba190@NEGATIVE: criterion (a) — no rustix 1.1.4 entry survives (RUSTSEC-2026-0486)
N15@crates/unblock-mcp/src/wire.rs@INVALID_REQUEST_ID_MESSAGE@NEGATIVE: criterion (d) — the D47 reply constant is gone from every non-comment line of wire.rs
N16@crates/unblock-mcp/src/pre_handshake.rs@PRE_HANDSHAKE_REJECTION_MESSAGE@NEGATIVE: criterion (g) — the gate declares no reply constant
N17@crates/unblock-mcp/src/pre_handshake.rs@ErrorData@NEGATIVE: criterion (g) — no non-comment line of pre_handshake.rs names ErrorData, so the gate constructs no reply of its own
N18@crates@VersionClampingTransport|clamp_unsupported_initialize_version|mcp_server_duplex_unclamped_for_test@NEGATIVE: criterion (h) — the CD-4 clamp, its rewrite and the old seam name are gone from every non-comment line under crates/
N10@.cargo@$DEPRECATED_FLAG_RE@NEGATIVE: no cargo configuration under .cargo/ passes rustflags that allow deprecated, or any --cap-lints (=warn and the spaced form included), which would silence the deprecation build-wide (criterion (p))
"

# =================================================================================================
# ROW-ANCHORED LANDINGS — `code@path@anchor@regex@what it proves`
#
# Q1..Q25  are the SIBLING SCRIPTS' range knobs — the NEWEST script pins the OLDER ones, and THIS
#      script's own knobs have NO row, deliberately: they are the REFERENCE the other rows are compared
#      against, and a self-row could never fail. Q24/Q25 are the marginal pair: d58's two range knobs
#      were pinned by nothing in the tree until this script landed.
# Q26..Q38 are the contract as it stands: every sibling's CONTRACT_RE knob (d58's included) and the
#      code constant. The IMPLEMENTATION commit moves all of them to unblock.mcp.v1.13 together.
# Q39..Q42 are the THREE PROSE bump sites over FOUR rows.
# Q43..Q47 are this gate's wiring: SPECIFIED, RUNNING, LISTED in PROCESS.md §3 with both knob names,
#      PROCESS.md §3's NEWEST sentence, and ci-cd §2.1(a)'s enumeration naming it the newest.
# Q48..Q111 are the SIBLING ROWS the D59 spec commit retargeted to this script: d53's Q53, d54's Q67
#      and Q72, d55's Q78 and Q80..Q82, d56's Q101 and Q103..Q109, d57's Q149 and Q151..Q166 but
#      Q159, d58's Q45..Q76 but Q61, and d58's Q43 and Q44, reworded so they stop calling d58 the
#      newest.
# Q112..   are the normative and published texts: the D59 row's load-bearing claims; the [D59] notes
#      on D2, D25, D27, D38, D40, D42, D43, D47, D48, D49, D50, D53 and D54; FR-20, NFR-18, PRD §9's
#      Lifecycle bullet, §10, §13 and RK-2; the spine's §0.1, §5, §5.2a, §5.4 and §5.6 notes, its D59
#      ledger entry, the quota note and the §5b gate note; the mcp, cli and fuzz plans and the plans
#      index; both roadmaps; the T4.4 task and its letters.
# Q183..Q197 (tracker comment 406) Miguel's rulings: clause (15) RULED additive with its four
#      documented client-visible narrowings and the AGENTS.md renderer duty, the three-rule parse reply
#      RULED, and a cancelled write's re-read duty with the ub-bj0 follow-up. Q131 now pins the ruling.
# Q198..Q221 (design Review round 1, tracker comment 408) the advisory scope, the deterministic hold
#      seams, the duplicated-equal-id and (f4)/(o) cell fixes, criteria (s) and (t), the D40 kept and
#      read-error convergence clauses, the instructions() sentence, the retired D50 clause (5), the
#      client-side 16 MiB and MRTR-type disclosures, the FR-17/D50/D53 notes, T4.4's commit plan, the cli
#      plan's renderer note and the spine's ledger and D40 notes.
# Q222..Q240 (design Review round 2, tracker comments 410 and 411) the -32601 CustomRequest fallback
#      for a wrongly typed MRTR member and the null-runs case, ruling 411's six send sites, io::ErrorKind
#      keying, the ping/InitializeResult exit flip and its D35 classification with the D40/D50 notes, the
#      from_parts unit cells, the no-sibling-row-red commit order, the EOF-drain no-reply proof, the
#      ledger, the required-check correction and the mcp plan's error.rs row.
#      Q239 was reworded by the ub-c15 spec commit (tracker comment 478, the required-check drift
#      ruling): main's required checks are run-report-gate and the macOS exclusivity job (D58 (14)).
# Q293..Q294 (Verify round 1, tracker comment 422) the sixth narrowing (a reused in-flight id) and the _meta
#      read ahead of the rate limit and quota, beside ub-o8s.
# Q291..Q292 and N19..N20 ([D59 amendment, 2026-11-17], tracker comment 420) the one dated deny ignore of
#      RUSTSEC-2026-0543 (chrono, an unconditional rmcp 3.10 dependency) and the ban on any other ignore;
#      numbered after the test commit's rows so no earlier code is reused.
# Q241..Q242 (orchestrator ruling on the X01 drift, option (A), measured 2026-11-17) the [D59 amendment]
#      to clause (12) and criterion (17)(c): the third divergence tier `Recoded` and the three-tier guard.
# Q243..Q270 (IMPLEMENTATION commit) the manifest pin, the three-rule Err arm, the EOF drop, the 3.10
#      standard lists, the gate's pass-through and stateless predicate, the clamp-free composition, the
#      RequestMetaObject quota, get_info's ServerConfig, the io::ErrorKind disconnect predicate, the
#      criterion-(s) AGENTS.md/instructions()/README landings and the cells the implementation commit
#      lands. Q271..Q290 (TEST commit) the cells criteria (e)-(m) and (s) add, the hold seam and the
#      returned signal. P3..P7 and N11..N14 pin the lockfile by registry checksum (a version string sits on a line
#      of its own and cannot be row-anchored to its package); N15..N18 are criteria (d), (g) and (h).
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
Q22@scripts/checks/d57-config-layers-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D57 sibling's live-range knob carries the SAME range as this script — anchored on the knob line, so the file's own prose about the knob cannot satisfy the pin
Q23@scripts/checks/d57-config-layers-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, a separate line and so a separate way to be half-bumped
Q24@scripts/checks/d58-health-full-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D58 sibling's live-range knob — the PREVIOUS newest script, which by the self-row rule pins everyone except itself and so needs this row
Q25@scripts/checks/d58-health-full-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, the other of the two knobs d58 cannot pin itself
Q26@scripts/checks/d44-create-deps-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@that sibling's contract knob carries the id the code constant carries — the SPEC commit moves no contract byte, and the IMPLEMENTATION commit moves every knob to unblock.mcp.v1.13 with the constant (ci-cd §2.1)
Q27@scripts/checks/ub-lp9.25-dangling-blocker-claims.sh@^CONTRACT_RE=@mcp.*v1.*[^0-9]13'\$@the D45 sibling's contract knob spells the id with its \\? segments, so this row checks the live minor at its end — the SPEC commit moves no contract byte; the IMPLEMENTATION commit moves this knob to .13 with the code constant
Q28@scripts/checks/d47-envelope-id-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@that sibling's contract knob carries the id the code constant carries — the SPEC commit moves no contract byte, and the IMPLEMENTATION commit moves every knob to unblock.mcp.v1.13 with the constant (ci-cd §2.1)
Q29@scripts/checks/d48-stdout-channel-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@that sibling's contract knob carries the id the code constant carries — the SPEC commit moves no contract byte, and the IMPLEMENTATION commit moves every knob to unblock.mcp.v1.13 with the constant (ci-cd §2.1)
Q30@scripts/checks/d49-startup-failure-render-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@that sibling's contract knob carries the id the code constant carries — the SPEC commit moves no contract byte, and the IMPLEMENTATION commit moves every knob to unblock.mcp.v1.13 with the constant (ci-cd §2.1)
Q31@scripts/checks/d50-pre-handshake-gate-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@that sibling's contract knob carries the id the code constant carries — the SPEC commit moves no contract byte, and the IMPLEMENTATION commit moves every knob to unblock.mcp.v1.13 with the constant (ci-cd §2.1)
Q32@scripts/checks/d53-request-integrity-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@that sibling's contract knob carries the id the code constant carries — the SPEC commit moves no contract byte, and the IMPLEMENTATION commit moves every knob to unblock.mcp.v1.13 with the constant (ci-cd §2.1)
Q33@scripts/checks/d54-parse-error-id-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@that sibling's contract knob carries the id the code constant carries — the SPEC commit moves no contract byte, and the IMPLEMENTATION commit moves every knob to unblock.mcp.v1.13 with the constant (ci-cd §2.1)
Q34@scripts/checks/d55-update-refusal-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@that sibling's contract knob carries the id the code constant carries — the SPEC commit moves no contract byte, and the IMPLEMENTATION commit moves every knob to unblock.mcp.v1.13 with the constant (ci-cd §2.1)
Q35@scripts/checks/d56-organization-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@that sibling's contract knob carries the id the code constant carries — the SPEC commit moves no contract byte, and the IMPLEMENTATION commit moves every knob to unblock.mcp.v1.13 with the constant (ci-cd §2.1)
Q36@scripts/checks/d57-config-layers-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@that sibling's contract knob carries the id the code constant carries — the SPEC commit moves no contract byte, and the IMPLEMENTATION commit moves every knob to unblock.mcp.v1.13 with the constant (ci-cd §2.1)
Q37@scripts/checks/d58-health-full-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@that sibling's contract knob carries the id the code constant carries — the SPEC commit moves no contract byte, and the IMPLEMENTATION commit moves every knob to unblock.mcp.v1.13 with the constant (ci-cd §2.1)
Q38@crates/unblock-mcp/src/options.rs@^pub const CONTRACT_VERSION@$CONTRACT_RE@the code constant still reads the id this script's knob carries; D59's bump to unblock.mcp.v1.13 rides the IMPLEMENTATION commit, never this spec commit
Q39@CLAUDE.md@^\| .docs/PRD\.md. \| Product truth@$RANGE_RE@PROSE D-range bump site, LOCATED on the document-map row that states the range
Q40@docs/plans/ci-cd-and-distribution.md@\*\*\(a\) D-id coherence\*\*@$RANGE_RE@PROSE D-range bump site, LOCATED on the class-(a) statement — the ONE place in that file allowed to quote the live range
Q41@xtask/src/doc_lint.rs@Spec tokenizes@$RANGE_RE@PROSE D-range bump site, half 1 of 2: the PROSE range on the tokenizer comment line
Q42@xtask/src/doc_lint.rs@Spec tokenizes@$RANGE_ALT_RE@…half 2 of 2: the TOKENIZER ALTERNATION on that same line, which now LEADS with 59
Q43@docs/plans/ci-cd-and-distribution.md@Named sub-check \(its D59 sibling@d59-rmcp3-claims@this gate is SPECIFIED in its own paragraph — §2.1(a) also carries this filename, so a bare token would stay green with the paragraph deleted
Q44@.github/workflows/ci.yml@d59-rmcp3-claims@:[0-9]+: +- run: scripts/checks/d59-rmcp3-claims\.sh@this gate actually RUNS in the required doc-lint job; every line naming it must BE the run step, so a commented-out step fails
Q45@docs/PROCESS.md@d59-rmcp3-claims\.sh. .RANGE_RE@RANGE_ALT_RE@the count-free LIST that IS the rule carries this script's own entry with BOTH knob names
Q46@docs/PROCESS.md@always the NEWEST script@d59-rmcp3-claims\.sh@PROCESS.md §3 names this gate as the newest reference, which pins every OLDER script's knobs (the next mint retargets it)
Q47@docs/plans/ci-cd-and-distribution.md@\*\*\(a\) D-id coherence\*\*@since the D59 SPEC commit that mints it .the D58 precedent., .scripts/checks/d59-rmcp3-claims\.sh. .RANGE_RE. \*\*and\*\* .RANGE_ALT_RE., the newest of them@ci-cd §2.1(a)'s enumeration names this script as the newest
Q48@scripts/checks/d53-request-integrity-claims.sh@^Q53[^0-9]@d59-rmcp3-claims@the D53 sibling's row Q53 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q49@scripts/checks/d54-parse-error-id-claims.sh@^Q67[^0-9]@d59-rmcp3-claims@the D54 sibling's row Q67 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q50@scripts/checks/d54-parse-error-id-claims.sh@^Q72[^0-9]@d59-rmcp3-claims@the D54 sibling's row Q72 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q51@scripts/checks/d55-update-refusal-claims.sh@^Q78[^0-9]@d59-rmcp3-claims@the D55 sibling's row Q78 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q52@scripts/checks/d55-update-refusal-claims.sh@^Q80[^0-9]@d59-rmcp3-claims@the D55 sibling's row Q80 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q53@scripts/checks/d55-update-refusal-claims.sh@^Q81[^0-9]@d59-rmcp3-claims@the D55 sibling's row Q81 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q54@scripts/checks/d55-update-refusal-claims.sh@^Q82[^0-9]@d59-rmcp3-claims@the D55 sibling's row Q82 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q55@scripts/checks/d56-organization-claims.sh@^Q101[^0-9]@d59-rmcp3-claims@the D56 sibling's row Q101 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q56@scripts/checks/d56-organization-claims.sh@^Q103[^0-9]@d59-rmcp3-claims@the D56 sibling's row Q103 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q57@scripts/checks/d56-organization-claims.sh@^Q104[^0-9]@d59-rmcp3-claims@the D56 sibling's row Q104 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q58@scripts/checks/d56-organization-claims.sh@^Q105[^0-9]@d59-rmcp3-claims@the D56 sibling's row Q105 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q59@scripts/checks/d56-organization-claims.sh@^Q106[^0-9]@d59-rmcp3-claims@the D56 sibling's row Q106 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q60@scripts/checks/d56-organization-claims.sh@^Q107[^0-9]@d59-rmcp3-claims@the D56 sibling's row Q107 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q61@scripts/checks/d56-organization-claims.sh@^Q108[^0-9]@d59-rmcp3-claims@the D56 sibling's row Q108 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q62@scripts/checks/d56-organization-claims.sh@^Q109[^0-9]@d59-rmcp3-claims@the D56 sibling's row Q109 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q63@scripts/checks/d57-config-layers-claims.sh@^Q149[^0-9]@d59-rmcp3-claims@the D57 sibling's row Q149 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q64@scripts/checks/d57-config-layers-claims.sh@^Q151[^0-9]@d59-rmcp3-claims@the D57 sibling's row Q151 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q65@scripts/checks/d57-config-layers-claims.sh@^Q152[^0-9]@d59-rmcp3-claims@the D57 sibling's row Q152 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q66@scripts/checks/d57-config-layers-claims.sh@^Q153[^0-9]@d59-rmcp3-claims@the D57 sibling's row Q153 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q67@scripts/checks/d57-config-layers-claims.sh@^Q154[^0-9]@d59-rmcp3-claims@the D57 sibling's row Q154 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q68@scripts/checks/d57-config-layers-claims.sh@^Q155[^0-9]@d59-rmcp3-claims@the D57 sibling's row Q155 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q69@scripts/checks/d57-config-layers-claims.sh@^Q156[^0-9]@d59-rmcp3-claims@the D57 sibling's row Q156 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q70@scripts/checks/d57-config-layers-claims.sh@^Q157[^0-9]@d59-rmcp3-claims@the D57 sibling's row Q157 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q71@scripts/checks/d57-config-layers-claims.sh@^Q158[^0-9]@d59-rmcp3-claims@the D57 sibling's row Q158 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q72@scripts/checks/d57-config-layers-claims.sh@^Q160[^0-9]@d59-rmcp3-claims@the D57 sibling's row Q160 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q73@scripts/checks/d57-config-layers-claims.sh@^Q161[^0-9]@d59-rmcp3-claims@the D57 sibling's row Q161 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q74@scripts/checks/d57-config-layers-claims.sh@^Q162[^0-9]@d59-rmcp3-claims@the D57 sibling's row Q162 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q75@scripts/checks/d57-config-layers-claims.sh@^Q163[^0-9]@d59-rmcp3-claims@the D57 sibling's row Q163 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q76@scripts/checks/d57-config-layers-claims.sh@^Q164[^0-9]@d59-rmcp3-claims@the D57 sibling's row Q164 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q77@scripts/checks/d57-config-layers-claims.sh@^Q165[^0-9]@d59-rmcp3-claims@the D57 sibling's row Q165 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q78@scripts/checks/d57-config-layers-claims.sh@^Q166[^0-9]@d59-rmcp3-claims@the D57 sibling's row Q166 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q79@scripts/checks/d58-health-full-claims.sh@^Q45[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q45 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q80@scripts/checks/d58-health-full-claims.sh@^Q46[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q46 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q81@scripts/checks/d58-health-full-claims.sh@^Q47[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q47 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q82@scripts/checks/d58-health-full-claims.sh@^Q48[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q48 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q83@scripts/checks/d58-health-full-claims.sh@^Q49[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q49 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q84@scripts/checks/d58-health-full-claims.sh@^Q50[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q50 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q85@scripts/checks/d58-health-full-claims.sh@^Q51[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q51 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q86@scripts/checks/d58-health-full-claims.sh@^Q52[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q52 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q87@scripts/checks/d58-health-full-claims.sh@^Q53[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q53 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q88@scripts/checks/d58-health-full-claims.sh@^Q54[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q54 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q89@scripts/checks/d58-health-full-claims.sh@^Q55[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q55 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q90@scripts/checks/d58-health-full-claims.sh@^Q56[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q56 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q91@scripts/checks/d58-health-full-claims.sh@^Q57[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q57 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q92@scripts/checks/d58-health-full-claims.sh@^Q58[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q58 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q93@scripts/checks/d58-health-full-claims.sh@^Q59[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q59 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q94@scripts/checks/d58-health-full-claims.sh@^Q60[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q60 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q95@scripts/checks/d58-health-full-claims.sh@^Q62[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q62 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q96@scripts/checks/d58-health-full-claims.sh@^Q63[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q63 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q97@scripts/checks/d58-health-full-claims.sh@^Q64[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q64 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q98@scripts/checks/d58-health-full-claims.sh@^Q65[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q65 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q99@scripts/checks/d58-health-full-claims.sh@^Q66[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q66 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q100@scripts/checks/d58-health-full-claims.sh@^Q67[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q67 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q101@scripts/checks/d58-health-full-claims.sh@^Q68[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q68 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q102@scripts/checks/d58-health-full-claims.sh@^Q69[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q69 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q103@scripts/checks/d58-health-full-claims.sh@^Q70[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q70 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q104@scripts/checks/d58-health-full-claims.sh@^Q71[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q71 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q105@scripts/checks/d58-health-full-claims.sh@^Q72[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q72 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q106@scripts/checks/d58-health-full-claims.sh@^Q73[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q73 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q107@scripts/checks/d58-health-full-claims.sh@^Q74[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q74 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q108@scripts/checks/d58-health-full-claims.sh@^Q75[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q75 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q109@scripts/checks/d58-health-full-claims.sh@^Q76[^0-9]@d59-rmcp3-claims@the D58 sibling's row Q76 names the NEWEST gate, retargeted to this one by the D59 spec commit
Q110@scripts/checks/d58-health-full-claims.sh@^Q43[^0-9]@d59-rmcp3-claims..sh@the D58 sibling's PROCESS.md newest-pointer row names this gate (reworded by the D59 spec commit)
Q111@scripts/checks/d58-health-full-claims.sh@^Q44[^0-9]@the newest of them until D59@the D58 sibling's §2.1(a) row stops requiring that enumeration to call d58 the newest (reworded by the D59 spec commit)
Q112@docs/PRD.md@^\| \*\*D59\*\* \|@Tracked as .ub-9ov.@the D59 decision row is in PRD §4 and names the work it tracks
Q113@docs/PRD.md@^\| \*\*D59\*\* \|@SUPERSEDES D47.s Notification-keyed class and its transport .-32600. arm@the row records the D47 supersession, reciprocal to D47's note
Q114@docs/PRD.md@^\| \*\*D59\*\* \|@\*\*\(2\) D54 IS KEPT, and the reason is restated@clause (2): D54 is KEPT, with its reason restated against rmcp 3
Q115@docs/PRD.md@^\| \*\*D59\*\* \|@A line that is NOT strict JSON .D54 leg .b. fails on it@clause (2) rule (α): a line that is not strict JSON keeps the id-less -32700
Q116@docs/PRD.md@^\| \*\*D59\*\* \|@A strict-JSON line whose id .envelope_id::scan. recovers gets .-32700 Parse error. ON that id@clause (2) rule (β): a readable id keeps -32700 on that id
Q117@docs/PRD.md@^\| \*\*D59\*\* \|@gets rmcp 3.10.s own reply, the id-less .ErrorData::invalid_request..Invalid request., None.., byte for byte@clause (2) rule (γ): an unreadable id converges on rmcp's id-less -32600
Q118@docs/PRD.md@^\| \*\*D59\*\* \|@\*\*FLOAT IDS RUN AS REQUESTS:\*\*@clause (3): float-form integer ids decode and execute
Q119@docs/PRD.md@^\| \*\*D59\*\* \|@\*\*NFR-18.s .per-request bytes. is REDEFINED as the re-serialized .CallToolRequestParams. plus .RequestContext\.meta.\*\*@clause (4): the quota's definition on 3.10, task excluded, the MRTR fields measured
Q120@docs/PRD.md@^\| \*\*D59\*\* \|@version outside .supported_protocol_versions... gets out-of-band .-32022.@clause (5): an unsupported _meta protocol version gets rmcp's out-of-band -32022
Q121@docs/PRD.md@^\| \*\*D59\*\* \|@\*\*This MOVES .CONTRACT_HASH.\*\*@clause (6): the stripped inputSchema roots move the hash, on the code's evidence
Q122@docs/PRD.md@^\| \*\*D59\*\* \|@\*\*\(7\) AN UNTERMINATED FINAL LINE AT EOF IS DROPPED\.\*\*@clause (7): the EOF drop amending D53
Q123@docs/PRD.md@^\| \*\*D59\*\* \|@a cancelled .tools/call. RUNS TO COMPLETION and a mutation it started COMMITS@clause (8): a cancelled request gets no reply while its mutation commits
Q124@docs/PRD.md@^\| \*\*D59\*\* \|@\*\*A STATELESS 2026-07-28 CLIENT\*\* sends no .initialize.@clause (9): what a stateless 2026-07-28 client sees
Q125@docs/PRD.md@^\| \*\*D59\*\* \|@\*\*A CLIENT THAT SENDS .initialize.\*\* gets its own version echoed@clause (9): what an initialize client sees
Q126@docs/PRD.md@^\| \*\*D59\*\* \|@\*\*\(10\) THE CD-4 CLAMP IS RETIRED\.\*\*@clause (10): the clamp is retired and its CD-6 pins re-pointed
Q127@docs/PRD.md@^\| \*\*D59\*\* \|@\*\*The latch opens on EITHER lifecycle.s start\.\*\*@clause (11): the gate opens on either lifecycle's start
Q128@docs/PRD.md@^\| \*\*D59\*\* \|@the gate DROPS every Notification, Response and Error frame with no reply .KEPT. and PASSES every Request upward .CHANGED.@clause (11): the gate keeps dropping non-Requests and passes Requests to rmcp
Q129@docs/PRD.md@^\| \*\*D59\*\* \|@the fork now carries exactly these divergences from rmcp 3\.10.s .AsyncRwTransport.|The fork now carries exactly these divergences from rmcp 3\.10.s .AsyncRwTransport.@clause (12): the fork's four divergences
Q130@docs/PRD.md@^\| \*\*D59\*\* \|@.ub-o8s. stays OPEN and OUT of scope by Miguel.s ruling@clause (12): the line bound stays out of scope
Q131@docs/PRD.md@^\| \*\*D59\*\* \|@\*\*RULED by Miguel \(2026-11-16, tracker comment 406\): the contract is .unblock\.mcp\.v1\.13., ADDITIVE, not .2\.0\.0.\.\*\*@clause (15): the v1.13-vs-2.0.0 question is RULED additive by Miguel (tracker comment 406), not decided silently
Q132@docs/PRD.md@^\| \*\*D59\*\* \|@.unblock\.mcp\.v1\.12. → .unblock\.mcp\.v1\.13., an ADDITIVE bump under D35@clause (15): the proposed contract bump and its D35 classification
Q133@docs/PRD.md@^\| \*\*D59\*\* \|@\*\*\(16\) THE .rmcp-1\.7\.0. CITATIONS\.\*\*@clause (16): the convention for the rmcp-1.7.0 citations
Q134@docs/PRD.md@^\| \*\*D59\*\* \|@\*\*\(c\) D54 KEPT AGAINST THE LIVE rmcp 3 READER\*\*@criterion (c): D54 is graded against rmcp 3.10's live reader
Q135@docs/PRD.md@^\| \*\*D59\*\* \|@\*\*\(q\) GATE\*\* — .scripts/checks/d59-rmcp3-claims\.sh. is executable@criterion (q): this gate
Q136@docs/PRD.md@^\| \*\*D59\*\* \|@RUSTSEC-2026-0538.*RUSTSEC-2026-0486@clause (1) names the two lock advisories beside rmcp's
Q137@docs/PRD.md@^\| \*\*D59\*\* \|@THE D-RANGE BUMP .D1\.\.D58. → .D1\.\.D59.\*\*@clause (18): the D-range bump
Q138@docs/PRD.md@^\| \*\*D2\*\* \|@\*\*\[D59\] AMENDED by PRD §4 D59 \(v1\.1\):\*\* the pinned SDK is .rmcp. 3\.10\.0@D2 carries the reciprocal [D59] AMENDED note
Q139@docs/PRD.md@^\| \*\*D25\*\* \|@\*\*\[D59\] AMENDED \(v1\.1\):\*\* the resource not-found mapping@D25 carries the reciprocal [D59] note
Q140@docs/PRD.md@^\| \*\*D27\*\* \|@\*\*\[D59\] \(v1\.1\):\*\* the AD-4 cancel mechanics above hold on rmcp 3\.10@D27 carries the reciprocal [D59] note
Q141@docs/PRD.md@^\| \*\*D38\*\* \|@\*\*\[D59\] \(v1\.1\):\*\* the .rmcp 1\.7. citations above are the record@D38 carries the reciprocal [D59] note
Q142@docs/PRD.md@^\| \*\*D40\*\* \|@\*\*\[D59\] KEPT \(v1\.1, PRD §4 D59 clause \(13\)\):\*\* this exit rule holds on rmcp 3\.10@D40 carries the reciprocal [D59] KEPT note, extended to the closed-stdout write failure
Q143@docs/PRD.md@^\| \*\*D42\*\* \|@\*\*\[D59\] AMENDED \(v1\.1, PRD §4 D59 clause \(4\)\):\*\*@D42 carries the reciprocal [D59] note
Q144@docs/PRD.md@^\| \*\*D43\*\* \|@\*\*\[D59\] AMENDED \(v1\.1\):\*\* the fork-divergence record@D43 carries the reciprocal [D59] note
Q145@docs/PRD.md@^\| \*\*D47\*\* \|@\*\*\[D59\] SUPERSEDED \(v1\.1, PRD §4 D59 clause \(3\)\):\*\*@D47 carries the reciprocal [D59] note
Q146@docs/PRD.md@^\| \*\*D48\*\* \|@\*\*\[D59\] \(v1\.1\):\*\* the .rmcp-1\.7\.0. pre-handshake citations above are the record@D48 carries the reciprocal [D59] note
Q147@docs/PRD.md@^\| \*\*D49\*\* \|@\*\*\[D59\] \(v1\.1\):\*\* the .rmcp-1\.7\.0. citations above are the record@D49 carries the reciprocal [D59] note
Q148@docs/PRD.md@^\| \*\*D50\*\* \|@\*\*\[D59\] REDESIGNED \(v1\.1, PRD §4 D59 clause \(11\)\):\*\*@D50 carries the reciprocal [D59] note
Q149@docs/PRD.md@^\| \*\*D53\*\* \|@\*\*\[D59\] AMENDED \(v1\.1, PRD §4 D59 clause \(7\)\):\*\*@D53 carries the reciprocal [D59] note
Q150@docs/PRD.md@^\| \*\*D54\*\* \|@\*\*\[D59\] KEPT \(v1\.1, PRD §4 D59 clause \(2\)\), with TWO clauses amended — \(4\) and \(6\)\.\*\*@D54 carries the reciprocal [D59] note, naming both amended clauses
Q151@docs/PRD.md@^- \*\*FR-20 \[must\]@\*\*\[D59\] \(v1\.1\):\*\* the server runs on rmcp 3\.10@FR-20 records the SDK, both lifecycles and the redefined quota
Q152@docs/PRD.md@^- \*\*NFR-18 \[security@\*\*\[D59\] \(v1\.1\):\*\* .per-request bytes. is now@NFR-18 records the redefined per-request bytes and the converged transport replies
Q153@docs/PRD.md@^- \*\*Lifecycle \[D59\]\*\*@stateless lifecycle with .server/discover.@PRD §9 names both MCP lifecycles
Q154@docs/PRD.md@^- \*\*Runtime/async:\*\*@\*\*MCP:\*\* .rmcp 3\.10.@PRD §10's stack line pins rmcp 3.10
Q155@docs/PRD.md@^\| \*\*v1\.1\+\*\* \|@the rmcp 3\.10 and MCP 2026-07-28 upgrade \(D59, T4\.4\)@PRD §13's v1.1+ row carries D59
Q156@docs/PRD.md@^\| RK-2 \|@\*\*\[D59\]\*\* Realised once@RK-2 records the realised churn
Q157@docs/plans/01-design-spine.md@^.unblock-cli. \*\*depends on\*\* .unblock-mcp.@PreHandshakeGateTransport<DupScanningTransport<R, W>>@spine §0.1: the clamp-free composition and the renamed raw seam
Q158@docs/plans/01-design-spine.md@^\*\*rmcp 3\.10\*\*@serving BOTH MCP lifecycles@spine §5 header: rmcp 3.10 and both lifecycles
Q159@docs/plans/01-design-spine.md@^jointly with the §5\.3 output change\)\. \*\*\[D59\]@no tool input ROOT carries .title. or .description.@spine §5.2a: no tool input root carries title or description
Q160@docs/plans/01-design-spine.md@^faults stay .INTERNAL_ERROR. \(-32603\)\.@\*\*\[D59\] \(v1\.1\):\*\* on a request that runs the MCP 2026-07-28 revision@spine §5.4: the not-found remap on the 2026-07-28 revision
Q161@docs/plans/01-design-spine.md@^\*\*D59 \(v1\.1\) — the contract bumps to .unblock\.mcp\.v1\.13.\.\*\*@A SIBLING entry appended to this ledger@spine §5.4 ledger: the D59 entry
Q162@docs/plans/01-design-spine.md@^Any .EngineError. → .StructuredError.@\*\*\[D59\] \(v1\.1, PRD §4 D59\) — on rmcp 3\.10 the out-of-band list above changes in five places@spine §5.6: the out-of-band list on rmcp 3.10
Q163@docs/plans/01-design-spine.md@^per-request SIZE caps stay .enforce_quota. checks@.inputResponses. \+ .requestState. \+ ._meta.@spine §5b quota note: the 3.10 per-request bytes
Q164@docs/plans/01-design-spine.md@^- \*\*Pre-handshake frame gate \(NORMATIVE — D50\)\.\*\*@\*\*\[D59\] REDESIGNED \(v1\.1, PRD §4 D59 clause \(11\)\):\*\*@spine §5b: the redesigned gate
Q165@docs/plans/crates/unblock-mcp.md@^## Pinned rmcp@## Pinned rmcp 3\.10 API contract@the mcp plan's pinned-API section is re-pinned to 3.10
Q166@docs/plans/crates/unblock-mcp.md@^- \*\*Protocol lifecycles and version negotiation \[D59\]\.\*\*@is RETIRED \(D59 clause \(10\)\)@the mcp plan's lifecycle bullet, with the retired clamp
Q167@docs/plans/crates/unblock-mcp.md@reproduces ALL of .AsyncRwTransport::receive.@Against that reader the fork carries exactly FOUR divergences@the mcp plan's fork doctrine line carries the four 3.10 divergences
Q168@docs/plans/crates/unblock-mcp.md@^\| .src/server\.rs. \|@\*\*\[D59\] \(v1\.1\):\*\* the composed transport becomes@the mcp plan's server.rs row: the clamp deleted
Q169@docs/plans/crates/unblock-mcp.md@^\| .src/wire\.rs.@\*\*\[D59\] \(v1\.1, PRD §4 D59 clauses \(2\), \(3\), \(7\) and \(12\)\):\*\*@the mcp plan's wire.rs row: the three-rule Err arm and the EOF drop
Q170@docs/plans/crates/unblock-mcp.md@^\| .src/pre_handshake\.rs.@\*\*\[D59\] REDESIGNED \(v1\.1, PRD §4 D59 clause \(11\)\):\*\*@the mcp plan's pre_handshake.rs row: the redesigned gate
Q171@docs/plans/crates/unblock-cli.md@^\| .src/commands/mcp\.rs. \|@\*\*\[D59\] \(v1\.1\):\*\* the server under this command runs on rmcp 3\.10@the cli plan's mcp.rs row
Q172@docs/plans/crates/unblock-fuzz.md@^- Dev/runtime deps:@\*\*\[D59\] \(v1\.1\):\*\* the oracle is rmcp 3\.10@the fuzz plan's oracle note
Q173@docs/plans/README.md@^\| \[.crates/unblock-mcp\.md.\]@rmcp 3\.10 stdio server@the plans index names rmcp 3.10
Q174@docs/plans/00-roadmap.md@^\| FR-20 \| \*\*MCP stdio server@\*\*\[D59\]\*\* 3\.10 from v1\.1@the markdown roadmap's v1 FR-20 row re-points the SDK
Q175@docs/plans/00-roadmap.md@^\| FR-20 \(SDK\) \|@Specified by PRD §4 D59 \(.ub-9ov., implementation-plan T4\.4\)@the markdown roadmap's v1.1 row for D59
Q176@docs/plans/00-roadmap.md@^  \*\*\[D59\] \(v1\.1\):\*\* this@this class is RETIRED on rmcp 3\.10@the markdown roadmap's D47 bullet carries its [D59] note
Q177@docs/plans/00-roadmap.md@^  \*\*\[D59\] \(v1\.1\):\*\* on@on rmcp 3\.10 the gate PASSES every premature Request@the markdown roadmap's D50 bullet carries its [D59] note
Q178@docs/plans/00-roadmap.md@^  \*\*\[D59\] \(v1\.1\):\*\* an@an unterminated final line at EOF is now DROPPED@the markdown roadmap's D53 bullet carries its [D59] note
Q179@docs/plans/00-roadmap.md@^  \*\*\[D59\] \(v1\.1\):\*\* D54@D54 is KEPT on rmcp 3\.10@the markdown roadmap's D54 bullet carries its [D59] note
Q180@docs/roadmap.html@^ +<li>MCP SDK rmcp 1\.7@contract <span class=.mono.>unblock\.mcp\.v1\.13</span> \(D59\)@the RENDERED v1.1 card carries the D59 bullet
Q181@docs/plans/implementation-plan.md@^- \*\*T4\.4 @moves to rmcp 3\.10 and the server adopts MCP 2026-07-28@the T4.4 task exists and names the move it implements
Q182@docs/plans/implementation-plan.md@^  \(d\) THE CONVERGED WRONG-SHAPE CLASS@\(h\) THE CLAMP IS@T4.4 cites D59's acceptance letters
Q183@docs/PRD.md@^\| \*\*D59\*\* \|@\*\*\(15\) CONTRACT — .unblock\.mcp\.v1\.12. → .unblock\.mcp\.v1\.13., an ADDITIVE bump under D35, RULED by Miguel \(tracker comment 406\)\.\*\*@ruling 406 (1): clause (15)'s heading records the ruling
Q184@docs/PRD.md@^\| \*\*D59\*\* \|@each affects only a client that does not follow the MCP specification, which is the reason for the ruling@ruling 406 (1): the reason for the additive ruling
Q185@docs/PRD.md@^\| \*\*D59\*\* \|@\(a\) A request whose ._meta. names an unsupported .io\.modelcontextprotocol/protocolVersion. gets out-of-band .-32022. and does not execute@ruling 406 (1): documented client-visible behaviour (a), the _meta -32022
Q186@docs/PRD.md@^\| \*\*D59\*\* \|@\(b\) An unterminated final line at EOF is dropped and does not execute@ruling 406 (1): documented client-visible behaviour (b), the EOF drop
Q187@docs/PRD.md@^\| \*\*D59\*\* \|@\(c\) A cancelled request gets no reply, and a write it started may still have happened@ruling 406 (1): documented client-visible behaviour (c), the unanswered cancelled request
Q188@docs/PRD.md@^\| \*\*D59\*\* \|@\(d\) The .inputSchema. root, live and in .unblock://schema., carries no .title. and no .description.@ruling 406 (1): documented client-visible behaviour (d), the root-less inputSchema
Q189@docs/PRD.md@^\| \*\*D59\*\* \|@\*\*.AGENTS\.md. is GENERATED\*\* .*to write a .### MCP client-visible behaviour. section@ruling 406 (1): AGENTS.md is generated, so the documented behaviour lines are the cli renderer's duty, in a named section
Q190@docs/PRD.md@^\| \*\*D59\*\* \|@\*\*The three rules are RULED by Miguel \(tracker comment 406\): invalid JSON gets an id-less .-32700.; valid JSON with a readable id gets .-32700. on that id \(D54 kept\); valid JSON with no readable id gets rmcp.s id-less .-32600.\.\*\*@ruling 406 (2): the three-rule parse reply is RULED
Q191@docs/PRD.md@^\| \*\*D59\*\* \|@\*\*RULED by Miguel \(tracker comment 406\): a client that cancels a WRITE request has NO guarantee that the write did not happen, and must re-read the state before acting on it\*\*@ruling 406 (3): a cancelling client must re-read the state
Q192@docs/PRD.md@^\| \*\*D59\*\* \|@Cooperative cancellation of a write before its commit is studied in the follow-up issue .ub-bj0.@ruling 406 (3): the cooperative-cancellation follow-up is named
Q193@docs/plans/implementation-plan.md@Clause \(15\) is RULED \(tracker comment 406\)@Clause \(15\) is RULED \(tracker comment 406\):@ruling 406 (1): T4.4 no longer waits on an open question
Q194@docs/plans/implementation-plan.md@^  writing the .### MCP client-visible behaviour. section@clause \(15\).s six lines@ruling 406 (1) and Review round 1: T4.4's implementation commit writes the six client-visible lines through the cli renderer
Q195@docs/plans/01-design-spine.md@^Miguel \(tracker comment 406, PRD §4 D59 clause \(15\)\)\.@tracker comment 406@ruling 406 (1): the spine's D59 ledger entry records the additive ruling
Q196@docs/plans/ci-cd-and-distribution.md@Named sub-check \(its D59 sibling@RULED additive \(tracker comment 406\)@ruling 406: the ci-cd paragraph names the ruling rows
Q197@docs/plans/00-roadmap.md@^\| FR-20 \(SDK\) \|@RULED by Miguel in tracker comment 406@ruling 406 (1): the markdown roadmap's D59 row records the additive ruling
Q198@docs/PRD.md@^\| \*\*D59\*\* \|@\*\*\(b\) ADVISORIES\*\* — .cargo audit. and .cargo deny check. pass on every commit of the branch FROM THE IMPLEMENTATION COMMIT ON@Review round 1 (ruling 408 (1)): the advisory jobs bind from the implementation commit on
Q199@docs/PRD.md@^\| \*\*D59\*\* \|@The docs-only SPEC commit still locks rmcp 1\.7\.0, .quinn-udp. 0\.5\.14 and .rustix. 1\.1\.4, so it INHERITS main.s known advisories@Review round 1: the spec commit's inherited audit/deny reds are documented
Q200@docs/PRD.md@^\| \*\*D59\*\* \|@\*\*HOLD SEAMS ARE DETERMINISTIC\.\*\*@Review round 1: the in-flight cells use deterministic hold seams, never a sleep
Q201@docs/PRD.md@^\| \*\*D59\*\* \|@a duplicated EQUAL id .*which D47 answered .-32600. on id 1@Review round 1: the duplicated-equal-id move from -32600 to -32700 has a named cell
Q202@docs/PRD.md@^\| \*\*D59\*\* \|@AND .clientCapabilities. .\{\}. gets .-32022.@Review round 1: (f4)'s frame carries complete metadata, so it reaches the version check
Q203@docs/PRD.md@^\| \*\*D59\*\* \|@drives an id-LESS .server/discover. frame with untypeable .params.@Review round 1: the compat-filter cell uses the id-less frame that bites on the 3.10 lists
Q204@docs/PRD.md@^\| \*\*D59\*\* \|@\*\*\(s\) DOCUMENTED CLIENT-VISIBLE BEHAVIOUR\*\*@Review round 1 (ruling 408 (3)): a criterion pins the AGENTS.md lines, the instructions() sentence and the README lifecycle sentence
Q205@docs/PRD.md@^\| \*\*D59\*\* \|@\*\*\(t\) D40 KEPT ON A CLOSED STDOUT\*\*@Review round 1 (ruling 408 (4)): the closed-stdout exit-0 cell
Q206@docs/PRD.md@^\| \*\*D59\*\* \|@\*\*D40 is KEPT, not converged, and Miguel.s ruling \(tracker comment 411\) widens it to every pre-lifecycle reply write\.\*\*@ruling 408 (4) and 411: clause (13) keeps D40 for every pre-lifecycle reply-write TransportError
Q207@docs/PRD.md@^\| \*\*D59\*\* \|@\*\*A READ ERROR CONVERGES too \(Miguel.s ruling 401, converge on rmcp 3\)\.\*\*@ruling 408 (5): clause (7) converges the read-error resume on rmcp 3.10
Q208@docs/PRD.md@^\| \*\*D59\*\* \|@Cancelling a write request does not guarantee the write did not happen; the client must re-read the state before acting on it\.@ruling 408 (3): clause (8) specifies the instructions() cancel sentence
Q209@docs/PRD.md@^\| \*\*D59\*\* \|@the client must re-read the state before acting on it \(clause \(8\)\)\.@ruling 406 (3) and 408 (3): documented line (c) carries the re-read duty
Q210@docs/PRD.md@^\| \*\*D59\*\* \|@RETIRES D50 clause \(5\)@Review round 1: the header names D50 clause (5) as retired
Q211@docs/PRD.md@^\| \*\*D59\*\* \|@\*\*The same bound bites on the CLIENT side, disclosed with it:\*\*@Review round 1: the client-side 16 MiB drop is disclosed
Q212@docs/PRD.md@^\| \*\*D59\*\* \|@\*\*A wrongly typed one is NOT ignored\*\*@Review round 1: the wrongly typed MRTR members are disclosed as a narrowing
Q213@docs/PRD.md@^\| \*\*D59\*\* \|@defined at .src/model/meta\.rs:393.@Review round 1: the RequestMetaObject citation points at its definition
Q214@docs/PRD.md@^\| \*\*D50\*\* \|@clause \(5\), which guarded that reply against traffic cancellation, is RETIRED with it@D50's note names clause (5) as retired
Q215@docs/PRD.md@^\| \*\*D53\*\* \|@Clause \(3\).s read-error sentence .*is AMENDED too@D53's note records the read-error convergence
Q216@docs/PRD.md@^- \*\*FR-17 \[must\]@\*\*\[D59\] \(v1\.1\):\*\* every .pre-.initialize.. above reads .before either MCP lifecycle starts.@FR-17 carries the lifecycle reading of pre-initialize
Q217@docs/plans/implementation-plan.md@The TEST commit only ADDS@\(iii\) The TEST commit only ADDS@ruling 408 (2): the test commit only adds cells; shipped cells and goldens move with the code
Q218@docs/plans/implementation-plan.md@^  so the workspace suite stays green on it@The IMPLEMENTATION commit carries the code AND everything the code turns red|so the workspace suite stays green on it@ruling 408 (2): the implementation commit carries the inverted shipped cells and the re-blessed goldens
Q219@docs/plans/crates/unblock-cli.md@^\| .src/commands/agents\.rs. \|@\*\*\[D59\] \(v1\.1, PRD §4 D59 clauses \(15\) and \(17\)\(s\)\):\*\*@the cli plan's agents.rs row carries the renderer duty
Q220@docs/plans/01-design-spine.md@^section of the six ruled lines PRD §4 D59 clause \(15\) specifies@written by the renderer in@the spine ledger says the managed block gains the ruled lines beside the contract line
Q221@docs/plans/01-design-spine.md@^\*\(\*\*Unsignalled pre-.initialize. disconnect@\*\*\[D59\] KEPT \(v1\.1, PRD §4 D59 clause \(13\)\):\*\*@spine §2.3's D40 note keeps exit 0 for the closed-stdout spelling
Q222@docs/PRD.md@^\| \*\*D59\*\* \|@falls back to its last variant .CustomRequest.,.*answers out-of-band .-32601. on the frame.s id@ruling 410/411 R2-M1: a wrongly typed MRTR member gets -32601 through the CustomRequest fallback, not -32700
Q223@docs/PRD.md@^\| \*\*D59\*\* \|@a call carrying .*inputResponses.*null.* RUNS@R2-S1: a null MRTR member decodes as absent and the call runs
Q224@docs/PRD.md@^\| \*\*D59\*\* \|@gets the JSON-RPC error -32601 and does not run\.@R2-M1: the fifth verbatim AGENTS line says -32601
Q225@docs/PRD.md@^\| \*\*D59\*\* \|@present and neither .null. nor an object, or whose .requestState. is present and neither .null. nor a string, gets out-of-band .-32601.@R2-M1/R2-S1: documented narrowing (e) is -32601 for a present non-null wrong type
Q226@docs/PRD.md@^\| \*\*D59\*\* \|@gets out-of-band .-32601. on its id with zero writes \(clause \(4\).s disclosed narrowing\), while one whose .inputResponses. and .requestState. are both .null. executes@R2-M1/R2-S1: criterion (i) pins the -32601 cells and the null-runs cell
Q227@docs/PRD.md@^\| \*\*D59\*\* \|@it has exactly SIX pre-lifecycle send sites@ruling 411: clause (13) names all six pre-lifecycle send sites
Q228@docs/PRD.md@^\| \*\*D59\*\* \|@The predicate keys on that .io::ErrorKind. and NEVER on rmcp.s .context. string@ruling 411: the predicate keys on our boxed io::ErrorKind, never on the context string
Q229@docs/PRD.md@^\| \*\*D59\*\* \|@\*\*This FLIPS two exits that v1\.12 ships\.\*\*@ruling 411: the ping and InitializeResult reply flip from exit 1 to exit 0 is recorded
Q230@docs/PRD.md@^\| \*\*D59\*\* \|@\*\*D35 classification, RULED by Miguel \(tracker comment 411\): NOT a breaking change\.\*\*@ruling 411: the flip's D35 classification
Q231@docs/PRD.md@^\| \*\*D59\*\* \|@with the public .DynamicTransportError::from_parts.@ruling 411: criterion (t) builds its unit cells with the public from_parts
Q232@docs/PRD.md@^\| \*\*D59\*\* \|@so a predicate that keys on the context text fails a cell@ruling 411: criterion (t) bites on a context-keyed predicate
Q233@docs/PRD.md@^\| \*\*D59\*\* \|@\*\*NO SIBLING ROW IS EVER RED ON ANY COMMIT, and this is how\.\*\*@R2-M3: clause (19) says how every sibling row stays green on every commit
Q234@docs/PRD.md@^\| \*\*D59\*\* \|@The NEGATIVE half of every no-reply cell first waits on the hold seam.s HELD-HANDLER-RETURNED signal@R2-S2 and round 3: no-reply cells wait on the held-handler-returned signal, the bounded EOF drain being the secondary proof
Q235@docs/PRD.md@^\| \*\*D40\*\* \|@\*\*Miguel.s ruling \(tracker comment 411\):\*\* that covers all six pre-lifecycle reply sends@D40 carries the reciprocal note for ruling 411
Q236@docs/PRD.md@^\| \*\*D50\*\* \|@which exit 1 on v1\.12, a flip ruled NOT a D35 break because the client has disconnected@D50 carries the reciprocal note for ruling 411
Q237@docs/plans/implementation-plan.md@which therefore ride THIS commit, not the test commit@which therefore ride THIS commit, not the test commit@R2-M3: T4.4 puts criterion (t)'s cells in the implementation commit, so d50 Q19 is never red
Q238@docs/plans/01-design-spine.md@^cancelled request, the fifth documented narrowing@present and neither .null. nor@spine ledger lists the fifth narrowing, the read-error resume and the D40 extension
Q239@docs/plans/ci-cd-and-distribution.md@so the feature-ON leg is invisible to both supply-chain jobs@neither is a required status check on .main., as PRD §4 D59 criterion \(b\) records; the required checks on .main. are .run-report-gate. and the macOS exclusivity job@ci-cd records that audit and deny are not required checks, and names main's required checks (reworded by the ub-c15 spec commit per PRD §4 D58 clause (14))
Q240@docs/plans/crates/unblock-mcp.md@^\| .src/error\.rs. \|@keyed on that kind and NEVER on rmcp.s .context. string@the mcp plan's error.rs row keys the predicate on io::ErrorKind
Q241@docs/PRD.md@^\| \*\*D59\*\* \|@\[D59 amendment, measured 2026-11-17\] A THIRD DIVERGENCE TIER, .Recoded.\..*exactly .Silenced. ∪ .Recoded. ∪ .IdInserted.@clause (12) records the measured Recoded tier (a not-strict-JSON line rmcp 3.10 answers with its id-less -32600 because its typed decode fails on shape first) and the three-tier set-equality guard
Q293@docs/PRD.md@^\| \*\*D59\*\* \|@\[D59 amendment, 2026-11-17\] \(f\) A SIXTH narrowing.*rmcp-3\.10\.0/src/service\.rs:1528-1531.*1 and 1\.0 are the same id@criterion (s) and clause (15): the duplicated in-flight id narrowing is disclosed with its verbatim AGENTS.md line (tracker comment 422)
Q294@docs/PRD.md@^\| \*\*D59\*\* \|@\[D59 amendment, 2026-11-17\] DISCLOSED beside that residual.*BEFORE .call_tool.*ub-o8s@clause (4): rmcp's _meta version read ahead of the rate limit and quota is disclosed beside ub-o8s (tracker comment 422)
Q291@docs/PRD.md@^\| \*\*D59\*\* \|@\[D59 amendment, 2026-11-17\] ONE DATED DENY IGNORE, RUSTSEC-2026-0543.*ub-4db.*rmcp-3\.10\.0/Cargo\.toml:1012-1025@clause (1) records the one dated deny ignore, its rationale and ub-4db (tracker comment 420)
Q292@docs/PRD.md@^\| \*\*D59\*\* \|@\[D59 amendment, 2026-11-17\]\*\* The one exception is the dated .deny\.toml. ignore of .RUSTSEC-2026-0543.@criterion (b) records the one exception
Q242@docs/PRD.md@^\| \*\*D59\*\* \|@\[D59 amendment, measured 2026-11-17\]\*\* A rule-\(α\) line whose typed decode fails on shape before .end\(\). reaches its trailing bytes@criterion (c) records that X01's shape draws rmcp's id-less -32600, graded by the Recoded tier
Q243@Cargo.toml@^rmcp = @rmcp = \\{ version = .3\\.10., features = \\[.server., .transport-io.\\] \\}@criterion (a): the workspace pins rmcp 3.10 with the server and transport-io features
Q244@crates/unblock-mcp/src/wire.rs@^ +ErrorData::invalid_request\\(@ErrorData::invalid_request\\(.Invalid request., None\\),@D59 clause (2) rule (γ): a strict-JSON line with no readable id gets rmcp 3.10's own id-less -32600, byte for byte
Q245@crates/unblock-mcp/src/wire.rs@^ +if answers_invalid_request\\(@answers_invalid_request\\(&failure, id\\.as_ref\\(\\)\\)@D59 clause (2): the three-rule Err arm selects rule (γ) on a strict-JSON line whose id is unreadable
Q246@crates/unblock-mcp/src/wire.rs@^ +Ok\\(_\\) if !self\\.line_buf\\.ends_with@=> return None,@D59 clause (7): the EOF arm drops an unterminated final line, as rmcp 3.10 does
Q247@crates/unblock-mcp/src/wire.rs@^ +[|] .server/discover.@server/discover@D59 clause (12): the fork's standard-method list carries rmcp 3.10's server/discover
Q248@crates/unblock-mcp/src/wire.rs@^ +[|] .subscriptions/listen.@subscriptions/listen@D59 clause (12): …and subscriptions/listen
Q249@crates/unblock-mcp/src/wire.rs@^ +[|] .elicitation/create.@elicitation/create@D59 clause (12): …and elicitation/create
Q250@crates/unblock-mcp/src/wire.rs@^ +[|] .notifications/subscriptions/acknowledged.@notifications/subscriptions/acknowledged@D59 clause (12): …and the standard notification notifications/subscriptions/acknowledged
Q251@crates/unblock-mcp/src/pre_handshake.rs@^ +Disposition::Pass => @Disposition::Pass => return Some\\(message\\),@D59 clause (11): the gate PASSES a premature Request upward and writes nothing
Q252@crates/unblock-mcp/src/pre_handshake.rs@^ +\\.missing_required_keys\\(@missing_required_keys\\(&ProtocolVersion::V_2026_07_28\\)@D59 clause (11): the stateless latch edge runs rmcp's own public predicate
Q253@crates/unblock-mcp/src/server.rs@^ +PreHandshakeGateTransport::new\\(@PreHandshakeGateTransport::new\\(DupScanningTransport::new\\(read, write\\), supported\\);@D59 clause (10): the composition is clamp-free, the gate wraps the scanner and is built with the server's supported versions
Q254@crates/unblock-mcp/src/server.rs@^ +meta: &RequestMetaObject,@meta: &RequestMetaObject,@D59 clauses (4) and (14): the quota measures RequestContext.meta as rmcp 3.10's RequestMetaObject
Q255@crates/unblock-mcp/src/server.rs@^ +fn get_info\\(&self\\)@fn get_info\\(&self\\) -> ServerConfig \\{@D59 clause (14): get_info returns ServerConfig, never the deprecated ServerInfo
Q256@crates/unblock-mcp/src/error.rs@^ +\\.is_some_and\\(\\|io\\| is_peer_gone@is_peer_gone\\(io\\.kind\\(\\)\\)@criterion (t): the disconnect predicate keys on the boxed io::ErrorKind, never on rmcp's context string (ruling 411)
Q257@crates/unblock-mcp/src/error.rs@^ +std::io::ErrorKind::BrokenPipe@std::io::ErrorKind::BrokenPipe@criterion (t): BrokenPipe is a peer-gone kind
Q258@crates/unblock-mcp/src/error.rs@^ +[|] std::io::ErrorKind::ConnectionReset@ConnectionReset@criterion (t): …and ConnectionReset
Q259@crates/unblock-mcp/src/error.rs@^ +[|] std::io::ErrorKind::NotConnected@NotConnected@criterion (t): …and NotConnected
Q260@AGENTS.md@^### MCP client-visible behaviour@### MCP client-visible behaviour@criterion (s): the regenerated managed block carries the client-visible-behaviour heading
Q295@AGENTS.md@^- A request that reuses the id of a request still in flight@may get no reply; 1 and 1\\.0 are the same id\\.@criterion (s) as amended 2026-11-17: the sixth client-visible line, the reused in-flight id (tracker comment 422)
Q261@AGENTS.md@^- A cancelled request gets no reply@the client must re-read the state before acting on it\\.@criterion (s): the cancelled-request line carries the re-read duty (ruling 406)
Q262@crates/unblock-cli/src/commands/mcp.rs@Cancelling a write request@Cancelling a write request does not guarantee the write did not happen;@criterion (s): instructions() carries clause (8)'s cancel sentence
Q263@README.md@^unblock mcp serves both MCP lifecycles@the initialize handshake \\(protocol revisions up to 2025-11-25\\) and the stateless 2026-07-28 lifecycle with server/discover\\.@criterion (s): README's MCP section carries clause (15)'s lifecycle sentence
Q264@crates/unblock-mcp/src/wire.rs@^ +async fn the_id_less_server_discover@async fn the_id_less_server_discover_frame_gets_rmcps_verdict\\(\\)@criterion (o): the id-less server/discover cell against rmcp 3.10's live reader
Q265@crates/unblock-mcp/src/wire.rs@^ +async fn a_read_error_resumes@async fn a_read_error_resumes_the_partial_line_like_rmcp\\(\\)@criterion (l): the inverted read-error cell
Q266@crates/unblock-mcp/src/wire.rs@^ +Recoded,@Recoded,@D59 clause (12) as amended: the Recoded tier exists in the differential
Q267@crates/unblock-mcp/tests/protocol_version.rs@^fn known_versions_latest@fn known_versions_latest_and_latest_with_initialize_are_pinned\\(\\)@criterion (h): the re-pointed CD-6 pins
Q268@crates/unblock-cli/tests/error_channel.rs@^fn a_task_param_is_ignored@fn a_task_param_is_ignored_and_the_call_runs\\(\\)@criterion (i): the inverted params.task cell
Q269@crates/unblock-cli/tests/mcp_lifecycle.rs@^fn a_closed_stdout_before_either_lifecycle@fn a_closed_stdout_before_either_lifecycle_exits_0\\(\\)@criterion (t): the closed-stdout real-binary cell
Q270@crates/unblock-mcp/src/error.rs@^ +fn the_disconnect_predicate_keys_on_the_error_kind@fn the_disconnect_predicate_keys_on_the_error_kind_never_on_the_context\\(\\)@criterion (t): the context-independence pair
Q271@crates/unblock-mcp/tests/lifecycle_2026.rs@^ *(pub )?(async )?fn a_first_frame_server_discover_is_answered_and_initialize_still_completes\\(@fn a_first_frame_server_discover_is_answered_and_initialize_still_completes\\(@criterion (f2): a first-frame server/discover gets the DiscoverResult and initialize still completes
Q272@crates/unblock-mcp/tests/lifecycle_2026.rs@^ *(pub )?(async )?fn a_first_frame_stateless_tools_call_starts_the_2026_session\\(@fn a_first_frame_stateless_tools_call_starts_the_2026_session\\(@criterion (f3): a complete-_meta first request starts the stateless 2026-07-28 session
Q273@crates/unblock-mcp/tests/lifecycle_2026.rs@^ *(pub )?(async )?fn an_unsupported_inline_version_gets_32022_and_leaves_the_latch_shut\\(@fn an_unsupported_inline_version_gets_32022_and_leaves_the_latch_shut\\(@criteria (f4) and (g): an unsupported inline version gets -32022 and the latch stays shut
Q274@crates/unblock-mcp/tests/lifecycle_2026.rs@^ *(pub )?(async )?fn a_premature_request_without_meta_gets_32602_and_the_handshake_completes\\(@fn a_premature_request_without_meta_gets_32602_and_the_handshake_completes\\(@criterion (g): a premature Request is answered -32602 by rmcp and the handshake completes
Q275@crates/unblock-mcp/tests/lifecycle_2026.rs@^ *(pub )?(async )?fn a_cancelled_write_commits_and_gets_no_reply\\(@fn a_cancelled_write_commits_and_gets_no_reply\\(@criterion (m): a cancelled write held in its storage write commits and gets no reply, on the returned signal plus the EOF drain
Q276@crates/unblock-mcp/src/server.rs@^ *(pub )?(async )?fn the_gate_predicate_agrees_with_rmcps_pre_lifecycle_loop\\(@fn the_gate_predicate_agrees_with_rmcps_pre_lifecycle_loop\\(@criterion (g): the gate predicate differential against rmcp 3.10s pre-lifecycle loop
Q277@crates/unblock-mcp/src/server.rs@^ *(pub )?(async )?fn mcp_server_duplex_with_returned_signal_for_test<@fn mcp_server_duplex_with_returned_signal_for_test<R, W>\\(@criterion (17): the held-handler-returned signal seam
Q278@crates/unblock-mcp/tests/common/mod.rs@^ *(pub )?(async )?fn session_create_gated\\(@fn session_create_gated\\(@criterion (17): the hold inside the storage write
Q279@crates/unblock-mcp/src/pre_handshake.rs@^ *(pub )?(async )?fn a_stateless_session_start_opens_the_latch\\(@fn a_stateless_session_start_opens_the_latch\\(@criterion (g): a stateless start opens the latch, so a later cancellation reaches rmcp
Q280@crates/unblock-mcp/src/pre_handshake.rs@^ *(pub )?(async )?fn an_unsupported_inline_version_leaves_the_latch_shut\\(@fn an_unsupported_inline_version_leaves_the_latch_shut\\(@criterion (g): an unsupported inline version leaves the latch shut
Q281@crates/unblock-mcp/src/pre_handshake.rs@^ *(pub )?(async )?fn a_server_discover_leaves_the_latch_shut\\(@fn a_server_discover_leaves_the_latch_shut\\(@criterion (g): server/discover leaves the latch shut
Q282@crates/unblock-cli/src/commands/agents.rs@^ *(pub )?(async )?fn managed_block_documents_the_client_visible_behaviour\\(@fn managed_block_documents_the_client_visible_behaviour\\(@criterion (s): the renderer unit cell
Q283@crates/unblock-cli/src/commands/mcp.rs@^ *(pub )?(async )?fn instructions_carry_the_cancel_sentence\\(@fn instructions_carry_the_cancel_sentence\\(@criterion (s): the instructions() cell
Q284@crates/unblock-cli/tests/error_channel.rs@^ *(pub )?(async )?fn oversized_mrtr_members_are_measured_by_the_quota\\(@fn oversized_mrtr_members_are_measured_by_the_quota\\(@criterion (i): the 300 KiB requestState and object inputResponses are measured
Q285@crates/unblock-cli/tests/error_channel.rs@^ *(pub )?(async )?fn wrongly_typed_mrtr_members_get_32601_and_null_runs\\(@fn wrongly_typed_mrtr_members_get_32601_and_null_runs\\(@criterion (i): the wrongly typed MRTR -32601 and the null-runs cells
Q286@crates/unblock-cli/tests/error_channel.rs@^ *(pub )?(async )?fn meta_protocol_versions_get_rmcps_out_of_band_errors\\(@fn meta_protocol_versions_get_rmcps_out_of_band_errors\\(@criterion (j): the _meta out-of-band cells
Q287@crates/unblock-cli/tests/mcp_lifecycle.rs@^ *(pub )?(async )?fn an_unterminated_final_tools_call_at_eof_is_dropped\\(@fn an_unterminated_final_tools_call_at_eof_is_dropped\\(@criterion (l): the real-binary EOF drop
Q288@crates/unblock-mcp/tests/contract_suite.rs@^ *(pub )?(async )?fn input_roots_carry_no_title_or_description_and_outputs_keep_theirs\\(@fn input_roots_carry_no_title_or_description_and_outputs_keep_theirs\\(@criterion (k): the root-annotation cell
Q289@crates/unblock-mcp/tests/envelope_id_duplex.rs@^ *(pub )?(async )?fn float_ids_run_and_unreadable_floats_get_rule_gamma\\(@fn float_ids_run_and_unreadable_floats_get_rule_gamma\\(@criterion (e): float ids run, unreadable floats get rule (γ)
Q290@crates/unblock-mcp/tests/envelope_id_duplex.rs@^ *(pub )?(async )?fn a_cancelled_notification_with_an_id_is_a_request_and_cancels_nothing\\(@fn a_cancelled_notification_with_an_id_is_a_request_and_cancels_nothing\\(@criterion (e): an id-carrying notifications/cancelled gets -32601 and cancels nothing
Q296@crates/unblock-mcp/tests/lifecycle_2026.rs@^ *(pub )?(async )?fn a_cancellation_after_a_stateless_start_reaches_rmcp\\(@fn a_cancellation_after_a_stateless_start_reaches_rmcp\\(@criterion (g): a cancellation after a stateless start reaches rmcp (hold seam, returned signal, EOF drain)
Q297@crates/unblock-cli/tests/mcp_lifecycle.rs@^ *(pub )?(async )?fn initialize_answers_every_version_with_the_pre_d59_bytes\\(@fn initialize_answers_every_version_with_the_pre_d59_bytes\\(@criterion (f1) over the real binary: the seven answers, the pre-D59 bytes, a legacy tools/list and ping
Q298@crates/unblock-cli/tests/mcp_lifecycle.rs@^ *(pub )?(async )?fn a_stateless_first_request_over_stdio_starts_the_2026_session\\(@fn a_stateless_first_request_over_stdio_starts_the_2026_session\\(@criterion (f3) over the real binary: the stateless lifecycle
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
      printf '%s\n' "$path: [$code] REQUIRED D59 target is missing from the tree ($reason)"
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
      printf '%s\n' "$path: [$code] REQUIRED D59 target is missing from the tree ($reason)"
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
  say "BLOCKED — the D59 rmcp 3 specification or its cascade is incomplete; the sites above are missing."
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
check_table_floor 'required-landing (P)' "$p_count" 8 || blocked=1
check_table_floor 'negative (N)' "$n_count" 20 || blocked=1
check_table_floor 'row-anchored (Q)' "$q_count" 298 || blocked=1

[ "$blocked" = "0" ] || exit 1
say "OK — the D59 row and its reciprocal notes on D2, D25, D27, D38, D40, D42, D43, D47, D48, D49, D50, D53 and D54, FR-20, NFR-18 and PRD §9/§10/§13/RK-2, the spine's rmcp 3.10 notes and D59 ledger entry, the crate plans, both roadmaps and the T4.4 task are in the tree; no advisory is ignored and no deprecation is allowed; the contract knobs agree with the code constant; the tracker names ub-9ov and ub-o8s; the live range sits at every prose site and every SIBLING script's knob — including d58's two — and every newest-pointer row names this gate."
exit 0
