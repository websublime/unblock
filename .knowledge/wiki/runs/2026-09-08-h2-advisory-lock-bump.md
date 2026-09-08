---
name: 2026-09-08-h2-advisory-lock-bump
description: Clearing RUSTSEC-2026-0258 with a Cargo.lock-only bump of h2 to 0.4.19 (tracker ub-5xl) — an advisory that reddened the required deny and audit jobs on every pull request while the shipped binary never linked the vulnerable code, and a tracker re-export that made a twelve-line lock diff substantive.
type: run
date: 2026-09-08
branch: ub-5xl-h2-rustsec-2026-0258
pr: -
issues: [ub-5xl]
---

# Run — the h2 advisory lock bump

## Context

Task `ub-5xl` — RUSTSEC-2026-0258 turned the required `deny` and `audit` jobs red on every pull
request, and the fix moves one entry of `Cargo.lock`. The orchestrator ran Understand and Decide off
`main` at b562570, one implementer made the bump in an isolated worktree, one code-reviewer lens ran
the Verify gate in its own checkout at the implementer's commit, and Track landed the tracker
re-export and this report. Proportionality sized the run down to that one implementer and that one
lens, with no design Review gate, under `docs/PROCESS.md` section 4. Branch
`ub-5xl-h2-rustsec-2026-0258`; no pull request is open at the time of writing.

## What & why

RUSTSEC-2026-0258, published 2026-08-17, reports that h2 0.4.15 accepts and queues empty HTTP/2 DATA
frames without limit, so an undrained stream grows memory without bound or panics once a length
overflows. The advisory sets the patched floor at 0.4.16 and rates the severity low. The last CI run
on `main` predates publication and passed, so the advisory first showed up on a pull request — the CI
run of #441, whose own diff touches no manifest and no lock.

h2 reaches the lock through hyper, and both entry paths end at `unblock-cli`. The self-update runtime
path runs axoupdater to axoasset to reqwest to hyper-rustls to hyper, behind the default-on
`self-update` feature. The dev-dependency path runs wiremock to hyper-util to hyper, and stands up the
mock release source that `crates/unblock-cli/tests/update_verify.rs` drives `unblock update` against.

The fix moves `Cargo.lock` alone. No workspace crate declares h2 or carries a version requirement on
it, so no manifest had anything to change. `cargo update -p h2` resolves 0.4.19, and the branch takes
that version so the lock stays reproducible from that one command.

Sections read rather than restated: the CI job table in `docs/plans/ci-cd-and-distribution.md`
section 2 (the `deny` and `audit` rows that went red), section 2.3.3 (the substantive-pull-request
predicate, whose rule 1a made this report due), and `docs/PROCESS.md` sections 4 and 6 (team sizing,
and who owns the re-export and the report).

## Outcome

Verify passed the tree, and its evidence sits in the gate comment on `ub-5xl`. The lens re-derived the
diff rather than reading it off the implementer. On a pristine copy of `main`, `cargo update -p h2`
reproduces the committed lock byte for byte. Exactly one name-version pair changes, h2 0.4.15 to
0.4.19; both sides hold 385 packages and identical checksums on every shared entry. `cargo audit` and
`cargo deny check` are red on `main` and green at the commit, and `cargo build --workspace --locked`,
`cargo test --workspace --locked` (1714 passed), `cargo xtask no-network`, the four feature-matrix
steps and `cargo fmt --check` are green.

The gate then sent the commit message back, and the orchestrator amended it without touching the tree.
The message had credited the advisory with naming `cargo update -p h2` as its remedy, so the
attribution now stops at cargo-deny's rendering, where that command actually appears. A quantifier
over every range requirement covered crates whose requirement never moved, so it is now scoped to the
edges that did move. One colon-hinged clause became two sentences.

The shipped binary never linked the vulnerable code. On the runtime edges hyper resolves with
`client`, `default` and `http1` and leaves its optional h2 dependency off, so `cargo build
--workspace` compiles no h2 unit at all. wiremock enables hyper's `http2` feature, and that
dev-dependency edge is the only place h2 is built. cargo-deny and cargo-audit read the lockfile rather
than the unit graph, so both jobs were red regardless, and the bump remains the correct fix. The cost
landed on CI, and user-facing severity was nil.

Re-resolution has one visible side effect. windows-sys 0.60.2 was reachable on `main` only through
quinn-udp, outside the default feature graph, and the re-pointed edges make it reachable inside that
graph, so windows-targets 0.53.5 and its eight arch crates enter the graph too and cargo-deny reports
twenty-three duplicate-version warnings where `main` reports fourteen. `deny.toml` sets
`multiple-versions` to warn, so the gate stays green. No CI job compiles the re-pointed Windows edges,
because every job in `ci.yml` runs on `ubuntu-latest`; the release pipeline's `x86_64-pc-windows-msvc`
build is their first real compile.

## Gotchas

- A branch that re-exports the whole tracker inherits every other open task's coined codes. Rule 1a of
  the gate scans added `.unblock/issues.jsonl` lines for a session-local code absent from the paired
  removed record, and this branch's re-export carries another open task's comment thread, which coins
  one. A twelve-line lock diff that rule 2 would otherwise pass as a pure dependency bump therefore
  classifies as substantive, and this report is the consequence. Run
  `scripts/knowledge/run-report-gate.sh` before assuming a bump is trivial; the pattern it scans with
  lives at `scripts/knowledge/run-report-gate.sh:11`.
- The advisory names no remedy command. `cargo update -p h2` appears in cargo-deny's rendering, while
  the advisory file and cargo-audit say only to upgrade to 0.4.16 or later. A commit message that
  credits the advisory with that command claims more than the advisory says.
- The resolver re-pointed ten windows-sys edges onto LOWER versions. A crate declaring a range
  requirement gets re-unified onto a version already activated in the graph, so those edges moved from
  0.61.2 down to 0.60.2 and 0.52.0 while the set of windows-sys versions in the lock stayed identical
  on both sides. A lock diff wider than the crate you bumped can still be exactly what the one command
  produces.
- A foreground `cd` into a worktree persists for the rest of the session and silently re-anchors later
  commands, including a bare `git status`. Pass `git -C <path>`, or check `pwd` before each command.

## Glossary

No session-local ids were used in this run.

## Links

- `ub-5xl` — RUSTSEC-2026-0258 reddens the required `deny` and `audit` jobs on every pull request, so
  bump the lock to h2 0.4.16 or later; the task this run tracks.
- `ub-wvh` — `scripts/knowledge/tests/landing-verify.sh` has been red since it landed and runs in no
  CI job; the task whose pull request surfaced the advisory.
- Pull request #441 — the `ub-wvh` change, whose CI run failed the `deny` and `audit` jobs and raised
  `ub-5xl`. It is rebased onto `main` after this branch lands.
- The advisory — https://rustsec.org/advisories/RUSTSEC-2026-0258
- Key files: `Cargo.lock` (the only file the fix touches), `deny.toml` (the `multiple-versions` warn
  setting that keeps the extra duplicate-version warnings out of the gate).
