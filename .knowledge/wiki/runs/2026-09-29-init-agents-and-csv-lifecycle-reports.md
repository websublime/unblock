---
name: 2026-09-29-init-agents-and-csv-lifecycle-reports
description: Adding init --agents and a next-step hint for unblock agents (tracker ub-lp9.14), with csv lifecycle reports riding the same pull request (tracker ub-q3k) — a one-flag onboarding change that grew three init bug fixes and a stderr panic fix, a design gate that passed with fixes six times because each round's rulings were new design until Miguel froze the design after round five, a retry command quoted for every target shell down to legacy Windows code pages, a Verify gate that failed once on a cell whose expected bytes a second guard also printed, and builds an endpoint antivirus scanner stretched past ten minutes.
type: run
date: 2026-09-29
branch: ub-lp9.14-init-agents
pr: -
issues: [ub-lp9.14, ub-q3k]
---

# Run — init --agents, the next-step hint and csv lifecycle reports

## Context

Two tracker tasks share this branch and one pull request.

- `ub-lp9.14` is a v1.1 onboarding finding from dogfooding the tracker. `unblock init` scaffolds a
  workspace but writes no managed `AGENTS.md` block, because PRD §4 D27 (the ratified lifecycle CLI
  surface) keeps `unblock agents` a separate command. A fresh workspace therefore has no
  `AGENTS.md` block until a second command runs.
- `ub-q3k` is a bug the Spec/Plan team found on 2026-09-24. D27 promises the `version`, `migrate`,
  `doctor` and `init` reports in all five output formats, but the csv renderer refused them, so
  `-o csv` did the work and then exited 4.

The run went from 2026-09-24 to 2026-09-29, off `main` at 253baf7, on branch
`ub-lp9.14-init-agents`. The branch carries eleven gated commits with tip 3e1f4c4, and this Track
commit sits on top of them. No pull request is open at the time of writing, so the `pr` field holds
the template's `-`. The change ships in v1.1, and Miguel ruled that the merge waits until 1.0.1 is
tagged final.

Understand and Decide ran in the orchestrator's session with Miguel, and the code graph answered at
253baf7. Every later phase ran as a Workflow, and every writer worked in this branch's worktree or in
a throwaway one.

| Phase | Team | Workflow run |
|---|---|---|
| Spec/Plan | Plan, rust-engineer and project-manager plus the coordinator | wf_26c6543d-cd7 |
| design Review | six rounds of three or four lenses plus the coordinator; a spec writer created the one `docs(spec)` commit before round 2 and amended it before each later round | one run per round, listed in the glossary |
| Implement | three sequential implementers in the branch worktree, then one loop-back after Verify round 1 | wf_baf689eb-28f, wf_a178ead8-3f9 |
| Verify | two rounds of code-reviewer, qa-expert, rust-engineer and security lenses plus the coordinator, each lens in its own throwaway worktree | wf_88205dba-602, wf_9f3cfe13-d21 |
| Track | a project-manager pass that filed the follow-ups, then this git-workflow-manager pass | wf_ecfefc6c-213, then this pass |

## What & why

The change was written against these sections, read rather than restated here.

- `docs/PRD.md` §4 D27, which gains an inline amendment dated 2026-09-24, and D33 (the managed
  `AGENTS.md` block decision), which gains a pointer back to it.
- `docs/PRD.md` §5 FR-14 (workspace bootstrap), which takes the tier marker
  `[must / v1.1 (init --agents)]`, plus the §11 deferred list and the §13 v1.1 milestone row.
- `docs/plans/01-design-spine.md` §4 (the D39 discovery tiers and the child probe), §5b (the `init`
  and `agents` commands and the CLI error boundary), §0.1 and §5.4.
- The crate plans of `unblock-cli`, `unblock-config`, `unblock-render` and `unblock-health`, the T3.1
  and T3.4.3 entries of `docs/plans/implementation-plan.md`, and roadmap §2 and §8.

At Decide, Miguel took both options the issue offered. `unblock init --agents` runs the same
`AGENTS.md` merge right after the scaffold, and a bare `init` prints one stderr hint naming
`unblock agents` as the next step. The two commands stay separate underneath, so the change rides D27
as an inline amendment, with no new D-id and no D-range move.

`ub-q3k` rides this pull request because of an ordering trap. On `main`, `init` ignored `-o` and
`--actor` on success, since `init.rs:58` opened the workspace with empty overrides, and that bug hid
the csv refusal for `init` alone. Fixing the forwarding first would have made `init -o csv` scaffold
and then exit 4. So the csv fix landed as its own `fix(render)` commit, ordered before the forwarding
fix.

The scope grew because the gates kept finding `init` paths the new flag would reach.

1. `init --dir <project root>` wrote a stray `<root>/config.toml`, then either exited 7 or bound the
   root's existing workspace and reported a scaffold that never happened.
2. A bare `init` in a root holding only `_unblock/` created a `.unblock/` that hid it from every later
   command.
3. `init` could scaffold a second workspace directory beside an existing one, and discovery then bound
   only one of the two. That is the silent wrong-database class the PRD risk register tracks as RK-7.
4. `output::diag` panicked with exit 101 when a write to stderr failed, and the new hint goes through
   it.

Each became its own fix commit. Discovery's name check and child probe also became public
`unblock-config` helpers, so `init` and discovery share one home for that rule, as spine CF-D (the
workspace-open ownership rule) requires.

The hint, the exit-8 merge-failure message and the refused-`--agents` message all end with the same
retry text, the command `unblock agents --dir '<dir>'`, so an agent can run it verbatim. That turned
the quoting of a directory name into a code-execution question, and most design rounds spent their
time there. The quoting moved from double quotes to POSIX single quotes, then dropped its Windows
fallback line for PowerShell's own quote doubling, and then gained documented residues for shells
outside the targets, interactive line editors and legacy Windows code pages.

## Outcome

### What landed

| Commit | Subject | Files | Lines |
|---|---|---|---|
| 21cd118 | docs(spec): specify init --agents, the init hint and csv lifecycle reports | 10 | +89 −50 |
| c0c0c7c | fix(render): render lifecycle reports as label,detail csv rows | 13 | +557 −36 |
| 14167f7 | fix(cli): forward the global flags into init's workspace open | 3 | +148 −4 |
| 95aaa0a | refactor(config): share discovery's workspace-dir probe with init | 2 | +167 −34 |
| 507b859 | fix(cli): probe init's target root the way discovery does | 4 | +329 −16 |
| 30b04bf | fix(cli): refuse init beside a sibling workspace | 4 | +560 −28 |
| 7b533be | fix(cli): write stderr notes without panicking | 2 | +58 −3 |
| 950beef | refactor(cli): share the AGENTS.md managed-block write between agents and init | 3 | +103 −17 |
| 3fbc0cb | feat(cli): add init --agents and a next-step hint for unblock agents | 11 | +1573 −71 |
| 63e92bc | test(cli): pin every lifecycle report in all five formats | 6 | +257 −2 |
| 3e1f4c4 | docs(health): record the end-to-end integrity-dirty doctor fixture | 1 | +1 −1 |

Together the eleven commits touch 41 files, with 3789 insertions and 209 deletions. No `ErrorCode`,
hint shape, `CONTRACT_VERSION` (still `unblock.mcp.v1.9`) or `CONTRACT_HASH` moved, and no
managed-block byte changed. The model, error, storage, engine and mcp crates are untouched.
`Cargo.lock` moves by one line, which adds `csv` to the dev-dependencies of `unblock-cli`.

A user sees these changes, and the pull request body carries them as release-note lines.

- `unblock init --agents` writes the managed `AGENTS.md` block after scaffolding.
- A successful bare `init` prints one new stderr line, a hint naming `unblock agents --dir '…'`,
  quoted for POSIX shells on unix and for PowerShell on Windows. `-q` silences it.
- `init -o plain`, `-o markdown` and `-o csv` now render the requested format, and an invalid
  `--actor` now exits 7 after `config.toml` is written. `init --force` recovers.
- `init` reads a root the way discovery does. It refuses a root or a sibling layout that already
  holds a workspace, and an `_unblock/` target that an empty `.unblock/` beside it would hide.
- `-o csv` works for `version`, `migrate`, `doctor` and `init`, which used to exit 4 after doing their
  work.
- When a write to stderr fails, `unblock agents` and `unblock update` now exit with their normal
  code, where they used to panic with exit 101.

### The gate record

The design gate passed with fixes in all six rounds, and only round 6 cleared Implement.

| Round | Date | Spec commit | Lenses | Must-fixes | What the round turned on |
|---|---|---|---|---|---|
| 1 | 2026-09-24 | none yet | Plan, rust-engineer, research-analyst | 26 | `init --dir <root>` bypassing the clobber guard; the hint's quoting; the side effects of forwarding `-o` and `--actor` |
| 2 | 2026-09-24 | 6dc6a1e | the same three | 13 | a new `.unblock/` shadowing an existing `_unblock/`; history expansion and Windows `$(…)` inside the double-quoted retry text |
| 3 | 2026-09-25 | 7ee4245 | security, Plan, rust-engineer, research-analyst | 17 | a Windows fallback line that could run code; a CLI copy of discovery's probe; a `.unblock/` scaffolded beside an initialized `_unblock/` |
| 4 | 2026-09-25 | 2939c5e | the same four | 8, plus 12 brief-only items | a symlinked sibling refusing its own target; an `_unblock/` scaffolded beside an empty `.unblock/`; PowerShell 5.1 reading a script without a byte-order mark in the ANSI code page |
| 5 | run 2026-09-25, ruled 2026-09-28 | 150119a | security, Plan, rust-engineer | 4 | layout L; the same decode reaching captured output and piped stdin; a target symlinked into another parent |
| 6 | 2026-09-28 | 765a3e6 | Plan, rust-engineer, research-analyst | 10 wording fixes, no blocker | round 5's rulings plus a focused recheck, under the freeze rule |

Round 5's security lens signed off on the retry text and found that none of its findings needed a
new quoting scheme. Round 6 marked the brief final and froze the design.

Implement ran on 2026-09-28 as three sequential implementers in the branch worktree. The first
amended the spec commit with round 6's five doc fixes and landed the csv and forwarding commits. The
second landed the shared probe, the target probe and the sibling guard. The third landed the panic
fix, the shared write, the flag with its hint, and the five-format pins. Every commit built and passed
`cargo test -p unblock-cli -p unblock-render -p unblock-config` on its own, and the full probe passed
with 1834 workspace tests. The first implementer also built the readable but integrity-dirty doctor
fixture the brief had left conditional, which the health plan had called impractical since T3.3 (the
v1 health-lite task). The orchestrator's `docs(health)` commit corrects that sentence.

Verify round 1 ran on 2026-09-28 at e307043 and failed, looping back to Implement. All four lenses
returned pass with fixes, and the coordinator failed the gate because a key cell's named mutant
survived. The cell for a bare `init` in a root holding only `_unblock/` (TR-7) expected exactly the
bytes the sibling guard also prints, so dropping the probe on the cwd path left it green, along with
all 300 `unblock-cli` cells. The round also measured these results.

- `cargo test --workspace` passed 1834 tests with none failed, and a per-commit bisect passed 143 of
  143 steps over the eleven commits.
- 159 of 163 table mutants died on their named cells, and the survivors included the TR-7 and TR-6
  masking cases.
- 32 cells went red under the mutation that treats every command as a protocol channel, matching the
  measured D48 list.
- `cargo audit` found no advisory, and `cargo deny` passed.
- The security lens ran 21 crafted directory names through sh, bash, zsh, dash and ksh, scripted and
  over a pty, and the printed retry text stayed inert and named the exact directory every time.

The round carried eight must-fixes, which the glossary lists. Miguel ruled two of them, as rulings 26
and 27. The loop-back kept eleven atomic commits by autosquashing its fixups, with tip 8890611. It
added the two cwd twin cells TR-8c and TR-8d, pinned the `_unblock` forms of the hint and of the
exit-8 message, routed two cells through a `..` segment so the canonicalization mutants die on Linux
too, redacted the init snapshots host-neutrally, and escaped U+2028 and U+2029 in the retry text.
Every one of its 24 compiling mutants went red, and the full probe passed with 1837 workspace tests.

Verify round 2 ran on 2026-09-29 at 8890611 and passed with fixes. The code-reviewer and
rust-engineer lenses returned pass with fixes, and the qa-expert and security lenses returned pass.

- All 43 CI-equivalent steps exited 0, with 1837 workspace tests passed and none failed.
- `cargo audit` reported no advisory across 386 crates, and the bench gate held all 25 operations
  within their ceilings.
- QA ran 186 table mutants plus 11 extras, and every table cell went red under its named mutants
  except UA-4, which only has to compile. One extra mutant is equivalent on unix and is recorded as
  such.
- The security lens ran 37 crafted names, 23 of them interactively over a pty. No code ran, and the
  decoy workspace was never written.

Its two must-fixes change no behaviour, cell or mutant. The orchestrator applied them as autosquashed
fixups, which moved the tip to 3e1f4c4 with a three-line tree diff. A focused recheck replaced a third
round, as the round-2 coordinator allowed. It ran `cargo fmt --check`, clippy with `-D warnings` and
the three-crate tests on each rewritten commit, which passed 510, 511 and 511 tests with none failed.
It then ran doc-lint and all eight check scripts at the tip. Last, it flipped every `#[cfg(unix)]` in
`crates/unblock-cli/tests/init_agents.rs` to `#[cfg(windows)]`, ran clippy on that test target clean,
and restored the file.

### Miguel's rulings

Miguel made 27 numbered rulings and one freeze rule, and the glossary defines each.

- Rulings 1 to 10 came on 2026-09-24. They cover the flag and the hint, the v1.1 release, the retry
  command, the exit-8 error, the report row, flag forwarding, csv for lifecycle reports, `--dir` as a
  project root, the retry text on a refused `--agents`, and accepting an invalid actor.
- Rulings 11 to 21 came on 2026-09-25. They cover the discovery-shaped probe, single quotes, the
  non-panicking note writer, PowerShell doubling with no fallback, the probe's home in
  `unblock-config`, the sibling refusal, the `--force` defaults, the PowerShell 5.1 residue, the
  refusal beside an empty `.unblock/`, the same-directory skip, and the Windows sign-off.
- Rulings 22 to 25 and the freeze rule came on 2026-09-28. They cover the check order in layout L, the
  message on the probe path there, the symlink residue and the wider decode residue.
- Rulings 26 and 27 came on 2026-09-28 after Verify round 1. They cover the separator escape and the
  host-neutral snapshots.

### Follow-ups

Track filed 21 new issues under the v1.1 epic `ub-lp9`, `ub-lp9.32` to `ub-lp9.52`. It also parented
`ub-q3k` and the three issues filed during the design rounds, `ub-nu0`, `ub-s82` and `ub-0p4`, to that
epic. This pull request closes four brief items. They are the second workspace directory at one root,
the end-to-end integrity-dirty doctor fixture, the cli plan's `Update` field name and the five-format
lifecycle snapshots. One item was left unfiled on purpose. An invalid actor or output format still
leaves `config.toml` without `unblock.db`, which ruling 10 accepted and `init --force` recovers. The
glossary maps every follow-up key to its issue.

### This commit

This Track commit carries four things.

- A 102-issue re-export of the tracker over `.unblock/issues.jsonl`, taken through the `sync` tool.
  It registers `ub-q3k`, `ub-nu0`, `ub-s82`, `ub-0p4` and the 21 new follow-ups, snapshots every
  comment on both tasks, and marks `ub-b07` and `ub-jv2` closed, which the record on `main` still
  showed as in progress and open.
- This report.
- Its entry under the Runs section of the wiki index.
- A curation of the memory `project-dogfood-unblock-is-the-tracker`, whose finding about the
  `init`/`agents` two-step was the last unexpected hit of the brief's first zero-live-hits sweep.

A second Track commit, made after the pull request opens, records the outcome and fills the `pr`
field.

## Gotchas

- **The design gate did not converge by itself.** Every round returned pass with fixes, so the
  two-iteration escalation of `docs/PROCESS.md` §5 never fired. Each round also raised forks,
  Miguel's rulings on them were new design, and each set of rulings sent the design back through a
  full new round. In rounds 2 to 5, each round's headline finding sat in design that an earlier
  round's ruling had just created. Must-fix counts ran 26, 13, 17, 8 and 4 without reaching zero.
  Miguel broke the loop with the freeze rule on 2026-09-28. Round 6 applied his round-5 rulings plus a
  focused recheck, and after it any new edge case short of a blocker became a follow-up issue. Verify
  used the same blocker definition twice, to grade the U+2028/U+2029 re-split and the ksh93u+
  line-editor corruption below blocker.
- **A cell whose expected bytes a second guard also prints cannot kill a mutant.** TR-7 expected the
  clobber guard's refusal. With the probe dropped at `crates/unblock-cli/src/commands/init.rs:146`,
  the sibling guard refused the un-probed `.unblock/` target and named the same `_unblock/` path byte
  for byte. The fix was two cells whose expected outcome only the probe produces, a re-scaffold in
  place and a scaffold inside an empty `_unblock/`.
- **Linux CI never sees the macOS `/private/var` prefix.** Two canonicalization mutants
  (`init.rs:173` and `:184`) died on macOS, where tempdirs sit under a symlinked `/var`, and survived
  on Linux, where `/tmp` is already canonical. Routing the cells through a `..` segment makes the
  formed path differ from the canonical one on every host. The memory
  `feedback-macos-probe-masks-linux-ci-path-confinement` records the same trap.
- **The endpoint antivirus scanner starved the builds.** On 2026-09-24, rebuilding the stale debug
  binary took about 20 minutes, from 10:31 to 10:51 UTC. Partway through, the two running `rustc`
  processes had used 4.7 and 2.3 seconds of CPU over 9 and 5 minutes of wall time, while the endpoint
  antivirus scanner ran at 188% CPU and the load average sat near 7. The recheck on 2026-09-29
  measured 13 and 8 minutes for incremental three-crate test builds, with no CPU snapshot taken then.
  A per-commit proof through `git rebase --exec` ran past the 10-minute tool limit, so the loop-back
  checked out each commit detached instead.
- **The session scratchpad does not survive.** It was retired once in the middle of the Verify
  round-1 loop-back, which then moved its mutation harness, results and logs to a durable directory
  outside `/tmp`. On 2026-09-29 the tracker harness was gone from the scratchpad when the round-2
  verdict was being recorded, and a durable copy made the evening before carried on.
- **A foreground `cd` into a worktree persists.** The harness reset the shell to the repository root
  42 times in this session, each time after a foreground `cd` that left the repository. A `cd` into
  the branch worktree stays inside the repository, under `.claude/worktrees/`, and it persisted with
  no notice. The orchestrator re-anchored with an explicit `cd` and `pwd` before launching the Verify
  round-1 workflow, because an earlier run saw a Workflow launched from a worktree cwd anchor itself
  to that worktree.
- **The native unblock MCP tools never connected.** `.mcp.json` starts the server through
  `cargo run`, the debug binary was stale, and the connection timed out after 30 seconds because
  cargo had to rebuild it first. Every tracker write in this run, and most reads, went through a
  line-delimited JSON-RPC harness over the binary's stdio, and the session made no native
  `mcp__unblock__*` call.
- **doc-lint class (c) reads a placeholder as a command.** A placeholder that puts a word after
  `unblock` inside a code span, such as `unblock path` or `unblock dir`, matches the class-(c) command
  tokenizer as a non-canonical subcommand. Round 1 caught it as a must-fix, and the round-5 writer
  tripped it again before amending. The landed placeholders avoid the pattern.
- **An older decision's check script can flag new prose.** The `ub-lp9.25` claims script sweeps every
  tracked file and retires a phrase about minting no decision id, to guard D45's history (the
  dangling-blocker decision). It flagged the round-2 writer's D27 note, which now says the note rides
  D27 inline and adds no decision of its own.
- **The knowledge bash-guard refuses a sweep that pairs `touch` with `.knowledge`.** The first
  zero-live-hits sweep's regex contains the word `touch`, and the second sweep names `.knowledge`.
  One combined command was refused, so the two sweeps run as separate commands.
- **Concurrent agents redden performance checks.** During Verify round 1 the contention lab's p99 read
  1.24 s against a 500 ms ceiling (`crates/unblock-storage/tests/contention_lab.rs:1452`), and the
  `engine_claim` bench read 18.98 ms against 15.0 ms. Both ran under the load of parallel lenses, both
  passed in isolation or on rerun, and the branch touches neither crate.
- **The first loop-back run stopped before its rebase.** It committed seven fixup commits and stopped.
  The run that finished checked each against the round-1 verdict, kept them, added two of its own and
  completed the rebase and the proofs.
- **Nothing executed on Windows.** The PowerShell quoting rule is checked against PowerShell's source,
  the code-page claims against Python's codec tables, and the Windows build of the test target only by
  flipping `cfg` attributes on macOS. `ub-nu0` carries the real-host probe.
- **No spawned agent reached the code graph.** Every spawned agent that reported a graph field
  reported it unavailable or stale at 253baf7 and answered with full reads and `git grep`. Only the
  orchestrator's Understand used the index.
- **`cargo test -p unblock-cli --no-default-features` fails two cells on `main` as well.** CI builds
  that leg without testing it, so a local probe that adds it meets two reds the branch did not cause
  (`ub-lp9.47`).

## Glossary

The implementation brief and the gate records live outside the repository, in the orchestrator's
durable session store. Every cell id below resolves to a named test in the repository, and every
ruling and must-fix is also recorded in words on the tracker issue named in its row.

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-----------------------------------------------|
| the brief | the final implementation brief for both tasks, frozen after design round 6, which pins the rulings, commit plan, cells, message templates and follow-up items | off-repo durable file; comments 195, 196 and 199 on ub-lp9.14 refer to it |
| freeze rule | Miguel's convergence rule of 2026-09-28. Round 6 applies the round-5 rulings plus a focused recheck, and after it a new edge case short of a blocker becomes a follow-up issue. A blocker is data loss, code execution in a target shell, or a wrong-database write `main` does not already make | comment 198 on ub-lp9.14, where it is called the convergence rule |
| layout L | an initialized `R/_unblock/` beside a distinct `R/.unblock/` directory holding neither `config.toml` nor `unblock.db`. `init --dir R/_unblock` on `main` can leave it behind, and discovery at `R` binds the empty `.unblock/` | comment 199 on ub-lp9.14; spine §5b, the `init` bullets |
| retry text | the command `unblock agents --dir '<dir>'` that ends the hint, the exit-8 message and the refused-`--agents` message, quoted for the host's shell family | built by `agents_retry` and `retry_for`, `crates/unblock-cli/src/commands/init.rs:253-285` |
| Posix family | the quoting rule of every non-Windows build, POSIX single quotes with an embedded quote written as `'\''` | `single_quote`, `crates/unblock-cli/src/commands/init.rs:270` |
| Windows family | the quoting rule of a Windows build, single quotes with each of the five single-quote characters doubled | `single_quote`, `crates/unblock-cli/src/commands/init.rs:270` |
| clobber guard | `init`'s refusal of a target that already holds `config.toml` or `unblock.db` unless `--force` is passed, unchanged from `main` | `crates/unblock-cli/src/commands/init.rs`, `run` |
| binds-first check | the check before the clobber guard that refuses a target when the pair's `.unblock/` is an empty directory discovery would bind first and the pair's `_unblock/` is the target or holds a workspace, with or without `--force` | `check_binds_first`, `crates/unblock-cli/src/commands/init.rs` |
| sibling guard | the check after the clobber guard that refuses a target beside a distinct sibling workspace directory holding a workspace, with or without `--force` | `check_sibling`, `crates/unblock-cli/src/commands/init.rs` |
| the measured D48 list | the cells that must go red when every command is treated as a protocol channel (D48, the stdout framing-channel decision); it holds 32 names at the tip | `crates/unblock-cli/tests/mcp_stdout_channel.rs:26-55`; comments 202, 204 and 206 call it the measured stdout-channel list |
| CF-D | a durable spine rule, glossed because a comment cites it bare. `unblock-config` owns workspace-dir discovery, so a second copy of the probe rule is a drift | `docs/plans/01-design-spine.md` §4, "Workspace-open ownership (CF-D)"; comment 195 on ub-lp9.14 |
| commit 1 to commit 12 | the brief's commit-plan numbering. Commits 1 to 10 are 21cd118, c0c0c7c, 14167f7, 95aaa0a, 507b859, 30b04bf, 7b533be, 950beef, 3fbc0cb and 63e92bc in order. Commit 11 is this Track commit, and commit 12 is the later Track commit that records the outcome and fills the `pr` field. The `docs(health)` commit 3e1f4c4 sits outside the numbering | comment 202 (commits 11 and 12) and comment 196 (a commit-5 build note) on ub-lp9.14 |
| design Review round-N must-fix K | item K of design round N's numbered must-fix list | comments 191, 193, 195, 196 and 198 on ub-lp9.14 for rounds 1 to 5; comments 192 and 194 on ub-q3k carry the csv items |
| implementer fixes 1 to 10 | round 6's ten wording fixes; fixes 1 to 5 amended the spec commit and fixes 6 to 10 touched only the brief | comment 199 on ub-lp9.14 |
| Verify round-1 must-fix 1 | the blocking item. TR-7's named mutant, the probe dropped on the cwd path, survived the whole suite, and the cwd twin cells TR-8c and TR-8d now kill it. Comment 206 calls items 1 to 8 "Must-fix 1" to "Must-fix 8" | comment 204 on ub-lp9.14, fixed per comment 206 |
| Verify round-1 must-fix 2 | TR-6's named mutant, the probe dropped on the `--dir` path, survived TR-6 by the same masking; TR-8 and TR-8b kill it, so only the proof record moved | comments 204 and 206 on ub-lp9.14 |
| Verify round-1 must-fix 3 | the `_unblock` form of the hint was unpinned; TR-8b gained a unix leg asserting the quoted directory | comments 204 and 206 on ub-lp9.14 |
| Verify round-1 must-fix 4 | the `_unblock` form of the exit-8 message was unpinned; IA-13 gained an `_unblock` leg | comments 204 and 206 on ub-lp9.14 |
| Verify round-1 must-fix 5 | two canonicalization mutants died only on macOS; TR-10 and TR-21 now form their paths through a `..` segment | comments 204 and 206 on ub-lp9.14 |
| Verify round-1 must-fix 6 | the two init-report snapshots hard-coded `/` and would mismatch on Windows; ruling 27 | comments 204 and 206 on ub-lp9.14 |
| Verify round-1 must-fix 7 | a module doc said every refusal under `--agents` carries the retry text, when only an already-initialized refusal does | comments 204 and 206 on ub-lp9.14 |
| Verify round-1 must-fix 8 | U+2028 and U+2029 passed `sanitize_inline`, so a reader that splits lines on them could see a forged hint, and the head of the refused-`--agents` message prints its path raw; ruling 26 | comments 204 and 206 on ub-lp9.14 |
| Verify round-2 must-fix 1 | two test constants read only by unix-gated cells gained `#[cfg(unix)]`, so a Windows build of the test target stays warning-free | comment 207 on ub-lp9.14 |
| Verify round-2 must-fix 2 | the cli crate plan's `init.rs` test list gained the U+2028/U+2029 escape | comment 207 on ub-lp9.14 |
| ruling 1 | `init --agents` runs the shared `agents` merge after a good scaffold, and a bare `init` prints a one-line stderr hint that `--agents` and `-q` suppress; recorded as an inline D27 amendment with no new D-id | comment 187 on ub-lp9.14; PRD §4 D27 |
| ruling 2 | the change ships in v1.1, built and gated now, and its merge waits until 1.0.1 is tagged final | comment 188 on ub-lp9.14 |
| ruling 3 | the hint always names the directory through `unblock agents --dir`, and the failure and refusal messages reuse that text | comment 188 on ub-lp9.14 |
| ruling 4 | a merge failure after a good scaffold is the CLI-local `InitAgentsWrite` error, exit 8, naming the workspace, the `AGENTS.md` path, the OS error and the retry text | comment 188 on ub-lp9.14 |
| ruling 5 | the init report gains an `agents_path` row, last, only under `--agents` | comment 188 on ub-lp9.14 |
| ruling 6 | `init` forwards the global flags into its workspace open, in its own commit, fixing the ignored `-o` and `--actor` | comment 188 on ub-lp9.14 |
| ruling 7 | csv renders lifecycle reports as `label,detail` rows, as `ub-q3k`, in its own commit ordered before the forwarding fix, inside this pull request | comments 188 and 189 on ub-lp9.14; the ub-q3k description and comment 190 |
| ruling 8 | an `init --dir` path whose last component is neither `.unblock` nor `_unblock` is a project root | comment 191 on ub-lp9.14 |
| ruling 9 | a refused `init --agents` extends only the `ALREADY_INITIALIZED` message with the retry text, keeping its code and hint shape | comment 191 on ub-lp9.14 |
| ruling 10 | an invalid `--actor` exiting 7 after `config.toml` is written is accepted, documented and pinned; `init --force` recovers | comment 191 on ub-lp9.14 |
| ruling 11 | `init` probes its root like discovery, taking an existing `.unblock`, else an existing `_unblock`, else a new `.unblock`, and refuses an initialized `_unblock` | comment 193 on ub-lp9.14 |
| ruling 12 | the retry text uses POSIX single quotes with the `'\''` idiom on unix; ruling 14 superseded its Windows half | comment 193 on ub-lp9.14 |
| ruling 13 | `output::diag` stops panicking for every caller, in its own commit | comment 193 on ub-lp9.14 |
| ruling 14 | on Windows the retry text is always the command, with the path in single quotes and each single-quote character doubled, PowerShell's own escaping, and no fallback line | comment 195 on ub-lp9.14 |
| ruling 15 | the name check and child probe become public `unblock-config` helpers that discovery and `init` share | comment 195 on ub-lp9.14 |
| ruling 16 | `init` beside a distinct sibling workspace directory that holds a workspace is refused with `ALREADY_INITIALIZED`, naming the sibling | comment 195 on ub-lp9.14 |
| ruling 17 | the accepted defaults that `init --force` re-scaffolds an existing `_unblock` in place and that an empty `_unblock` receives the scaffold | comment 195 on ub-lp9.14 |
| ruling 18 | Windows PowerShell 5.1 reading a script without a byte-order mark in the ANSI code page is a documented code-execution residue, and the claim that PowerShell reads the exact path is scoped | comment 196 on ub-lp9.14; comment 198 cites it by number |
| ruling 19 | an `_unblock` target beside an empty `.unblock` directory is refused with `ALREADY_INITIALIZED`, in a new message saying discovery binds that directory first | comment 196 on ub-lp9.14 |
| ruling 20 | both sibling checks skip a sibling that resolves to the target itself, so `init --force` re-scaffolds as on `main` | comment 196 on ub-lp9.14 |
| ruling 21 | the Windows sign-off rests on the source-checked analysis, the real-host probe is `ub-nu0`, and the sync-export default-path bug is `ub-s82` | comment 196 on ub-lp9.14 |
| ruling 22 | for an `_unblock` target the binds-first check runs before the clobber guard, with or without `--force`, and its message carries no retry text | comment 198 on ub-lp9.14 |
| ruling 23 | a bare `init --agents` in layout L gets the same binds-first message naming the `.unblock` directory, with no retry text | comment 198 on ub-lp9.14 |
| ruling 24 | a target symlinked into another parent is a documented residue, filed as `ub-0p4` | comment 198 on ub-lp9.14 |
| ruling 25 | the PowerShell decode residue also covers captured output and piped stdin, in PowerShell 7 too; it stays documented, with its probe legs on `ub-nu0` | comment 198 on ub-lp9.14 |
| ruling 26 | the retry text writes U+2028 and U+2029 as the text `\u{2028}` and `\u{2029}`, and spine §5b names the reader that re-splits output and the from-the-end extraction rule | comment 204 on ub-lp9.14 |
| ruling 27 | the two init-report snapshot cells redact paths host-neutrally, so the snapshots match on every host | comment 204 on ub-lp9.14 |
| CS-1 to CS-16, CS-14b, CS-15a to CS-15c | the `ub-q3k` csv cells | `crates/unblock-render/src/backend/csv_fmt.rs` tests; `crates/unblock-render/tests/` (snapshots, contract, sanitize_fuzz_seed); `crates/unblock-cli/tests/` (exit_codes, migrate_doctor, init_agents); CS-16 in `crates/unblock-cli/src/output.rs` |
| CS-14b | `doctor_csv_on_a_readable_integrity_dirty_db_reports_and_exits_2`, over the `orphan_a_page` fixture | `crates/unblock-cli/tests/migrate_doctor.rs:585` |
| CP-1 to CP-3 | the cells for the shared name check and child probe | `crates/unblock-config/src/discovery.rs` unit tests |
| TR-3 to TR-21, TR-8b to TR-8d | the `init` target, binds-first and sibling-guard cells; TR-1 and TR-2 were retired when the probe moved to `unblock-config` | `crates/unblock-cli/tests/init_agents.rs`, plus TR-9 and TR-16 in `init.rs` and TR-20 in `exit.rs` unit tests |
| TR-6 | `init_dir_naming_a_root_with_an_underscore_workspace_is_refused` | `crates/unblock-cli/tests/init_agents.rs:617` |
| TR-7 | `bare_init_in_a_root_with_an_underscore_workspace_is_refused` | `crates/unblock-cli/tests/init_agents.rs:641` |
| TR-8 | `init_force_on_an_underscore_root_rescaffolds_it_in_place` | `crates/unblock-cli/tests/init_agents.rs:730` |
| TR-8b | `init_dir_naming_a_root_with_an_empty_underscore_scaffolds_inside_it` | `crates/unblock-cli/tests/init_agents.rs:756` |
| TR-8c | `bare_init_force_in_a_root_with_an_underscore_workspace_rescaffolds_it_in_place`, the cwd twin of TR-8 | `crates/unblock-cli/tests/init_agents.rs:677` |
| TR-8d | `bare_init_in_a_root_with_an_empty_underscore_scaffolds_inside_it`, the cwd twin of TR-8b | `crates/unblock-cli/tests/init_agents.rs:707` |
| TR-10 | `init_beside_an_initialized_underscore_sibling_is_refused` | `crates/unblock-cli/tests/init_agents.rs:808` |
| TR-13 | `bare_init_on_an_empty_dot_unblock_beside_an_initialized_underscore_is_refused` | `crates/unblock-cli/tests/init_agents.rs:873` |
| TR-21 | `an_initialized_underscore_beside_an_empty_dot_unblock_is_refused_naming_the_dot_unblock` | `crates/unblock-cli/tests/init_agents.rs:1108` |
| AR-1 | `init_rejects_an_invalid_actor_after_writing_config_and_force_recovers` | `crates/unblock-cli/tests/init_agents.rs:166` |
| NP-1 to NP-3 | the cells for the non-panicking note writer | NP-1 in `crates/unblock-cli/src/output.rs`; NP-2 and NP-3 in `crates/unblock-cli/tests/init_agents.rs` |
| UA-1 to UA-19 | the `ub-lp9.14` unit cells | unit tests in `crates/unblock-cli/src/` (`cli.rs`, `output.rs`, `commands/init.rs`, `exit.rs`, `commands/agents.rs`) |
| UA-4 | `init_report_maps_to_info_kind`, a cell that only has to compile | `crates/unblock-cli/src/output.rs:337` |
| UA-19 | `retry_escapes_line_and_paragraph_separators_in_both_families` | `crates/unblock-cli/src/commands/init.rs:523` |
| IA-1 to IA-17, IA-8b, IA-9b, IA-9c, IA-11b to IA-11e, IA-14b | the `ub-lp9.14` integration cells | `crates/unblock-cli/tests/init_agents.rs` |
| IA-8b | `init_agents_quiet_still_prints_the_wrote_note`, which pins the `-q` gap `ub-lp9.39` will flip | `crates/unblock-cli/tests/init_agents.rs:1446` |
| IA-9c | `bare_init_hint_runs_verbatim_in_sh` | `crates/unblock-cli/tests/init_agents.rs:1519` |
| IA-11b | `refused_init_agents_on_an_underscore_root_names_its_canonical_dir` | `crates/unblock-cli/tests/init_agents.rs:1613` |
| IA-11e | `refused_init_agents_at_a_root_whose_empty_dot_unblock_binds_first_carries_no_retry_text` | `crates/unblock-cli/tests/init_agents.rs:1703` |
| IA-13 | `init_agents_write_failure_keeps_the_scaffold_and_exits_8` | `crates/unblock-cli/tests/init_agents.rs:1770` |
| IA-16 | `init_report_default_is_snapshot_pinned` and `init_report_with_agents_is_snapshot_pinned` | `crates/unblock-cli/tests/init_agents.rs:1911` and `:1916` |
| EX-1 to EX-4 | existing cells. EX-1 is `init_scaffolds_only_config_and_db`, EX-2 the `help_init` snapshot re-bless, EX-3 the set that must stay unmodified and green, and EX-4 `agents_io_failure_keeps_its_message` | EX-1 at `crates/unblock-cli/tests/init_agents.rs:25`, EX-2 at `crates/unblock-cli/tests/snapshots/help_snapshots__help_init.snap`, EX-4 at `crates/unblock-cli/tests/init_agents.rs:431` |
| MT-1 | the hint line, `hint: to write the AGENTS.md block for this workspace, run <retry text>` | `crates/unblock-cli/src/commands/init.rs:290` |
| MT-2 | the exit-8 `InitAgentsWrite` message, naming the workspace, the `AGENTS.md` path and the OS error, then `; to finish, run <retry text>` | `crates/unblock-cli/src/exit.rs:123` |
| MT-3 | the already-initialized refusal under `--agents`, which is MT-3b plus `; to write its AGENTS.md block, run <retry text>` | `crates/unblock-cli/src/exit.rs:94` and `:179` |
| MT-3b | the bare already-initialized refusal, `workspace already initialized at <dir>`, byte-identical to `main` | `crates/unblock-cli/src/exit.rs:94` |
| MT-4 | the shared note `wrote <path>`, unchanged | `crates/unblock-cli/src/commands/agents.rs:63` |
| MT-5 | the `agents` I/O failure message `file operation failed: <os error>`, unchanged | `crates/unblock-cli/src/exit.rs:139` |
| MT-6 | the rule that builds the retry text, sanitizing the path, escaping U+2028 and U+2029, then quoting it per shell family | `crates/unblock-cli/src/commands/init.rs:253-285` |
| MT-7 | the exact csv payload CS-2 asserts for escaped and sanitized cells | the test `diagnostics_cells_are_sanitized_then_escaped` in `crates/unblock-render/src/backend/csv_fmt.rs` |
| MT-8 | the `SiblingBindsFirst` message, `<bound> already exists, and discovery binds it before <hidden>` | `crates/unblock-cli/src/exit.rs:109` |
| FU-1 | spine §5b says PRD D3 named only four lifecycle commands, while the PRD lists seven | filed as ub-lp9.43 |
| FU-2 | the spine's `UpdateArgs` disagrees with the shipped `dry_run` field | filed as ub-lp9.43 |
| FU-3 | README names a `--db` flag the CLI lacks | filed as ub-lp9.42 |
| FU-4 | roadmap §1 omits `init` and `agents` from the crates it touches | filed as ub-lp9.43 |
| FU-5 | the config crate plan and discovery docs name `init` as a caller of APIs it never calls | filed as ub-lp9.44 |
| FU-6 | `output::diag` ignores `-q` for the `wrote` note and the `update` notes | filed as ub-lp9.39 |
| FU-7 | the `AGENTS.md` write is not atomic and follows symlinks | filed as ub-lp9.32 |
| FU-8 | the managed-block merge pairs the first BEGIN marker with the first END marker | filed as ub-lp9.33 |
| FU-9 | `InitReport.config_path` is the one non-canonical path in the report | filed as ub-lp9.40 |
| FU-10 | ci-cd §2.1(a) overstates what doc-lint class (a) checks | filed as ub-lp9.48 |
| FU-11 | the spine, the impl-plan and the PRD name a CLI-local `DoctorReport` the code lacks | filed as ub-lp9.43 |
| FU-12 | blocking `std::fs` calls on `init`'s async path | filed as ub-lp9.41 |
| FU-13 | the render plan names two fuzz targets that do not exist | filed as ub-lp9.49 |
| FU-14 | `unblock-mcp` declares `unblock-render` and uses nothing from it | filed as ub-lp9.50 |
| FU-15 | the `wrote` note prints the `AGENTS.md` path unescaped | filed as ub-lp9.34 |
| FU-16 | the `--dir` help text and README describe a `.unblock/`-only workspace directory | filed as ub-lp9.42 |
| FU-17 | a second workspace directory at one root | closed by this pull request through 507b859 and 30b04bf; the symlink remainder is ub-0p4 |
| FU-18 | whether Claude Code reads `AGENTS.md` when a `CLAUDE.md` exists | filed as ub-lp9.51 |
| FU-19 | an end-to-end readable but integrity-dirty doctor fixture | closed by this pull request through c0c0c7c, recorded by 3e1f4c4 |
| FU-20 | the cli plan's `Update` field name | closed by the spec commit 21cd118 |
| FU-21 | the spine, the PRD and the cli plan omit the `.write.lock` the first open creates | filed as ub-lp9.43 |
| FU-22 | whether `sanitize_inline` escapes bidi, zero-width and line-separator characters | filed as ub-lp9.36 |
| FU-23 | error messages print paths raw, so a newline in a path forges a second error line | filed as ub-lp9.35 |
| FU-24 | the MCP `sync export` default path in an `_unblock` workspace | filed as ub-s82 during design round 4 |
| FU-25 | the real-host Windows probe of the retry text | filed as ub-nu0 during design round 4 |
| FU-26 | a target symlinked into another parent, extended at Track with the symlinked workspace-dir write redirect | filed as ub-0p4 during design round 5 |
| V-retry-field | a Verify follow-up key for exposing the retry command as structured data | filed as ub-lp9.37 |
| V-ksh-probe | a Verify follow-up key for probing interactive line-editor rewrites, the ksh93u+ multibyte corruption and bash bracketed paste | filed as ub-lp9.38 |
| V-force-help | a Verify follow-up key for the `--force` help and two doc lines that assume a `.unblock` target | folded into ub-lp9.42 |
| V-concurrent-runs | a Verify follow-up key for the concurrent-runs residue missing from spine §5b | filed as ub-lp9.45 |
| V-first-delimiter-cells | a Verify follow-up key for IA-11b and IA-13 splitting the message at its first delimiter | filed as ub-lp9.46 |
| V-no-default-features | a Verify follow-up key for the two cells that fail without default features | filed as ub-lp9.47 |
| V-module-cycle | a Verify follow-up key for `exit.rs` and `commands::init` importing each other | filed as ub-lp9.52 |
| wf_26c6543d-cd7 | the Spec/Plan workflow run | the orchestrator's local workflow journal, outside the repository; comment 188 on ub-lp9.14 |
| wf_79e2f7c9-065 | the design Review round-1 workflow run | the local workflow journal; comment 191 on ub-lp9.14 and comment 192 on ub-q3k |
| wf_857d6c08-ae1 | the round-2 workflow run, which wrote the spec commit and ran design Review round 2; its first worktree hosts this branch | the local workflow journal; comment 193 on ub-lp9.14 and comment 194 on ub-q3k |
| wf_0a4ee0fa-86e | the design Review round-3 workflow run | the local workflow journal; comment 195 on ub-lp9.14 |
| wf_4bce49ba-4dc | the design Review round-4 workflow run | the local workflow journal; comment 196 on ub-lp9.14 |
| wf_efd871ef-a44 | the design Review round-5 workflow run | the local workflow journal; comment 198 on ub-lp9.14 |
| wf_125a553f-c61 | the design Review round-6 workflow run | the local workflow journal; comment 199 on ub-lp9.14 |
| wf_baf689eb-28f | the Implement workflow run | the local workflow journal; comment 202 on ub-lp9.14 |
| wf_88205dba-602 | the Verify round-1 workflow run | the local workflow journal; comment 204 on ub-lp9.14 and comment 205 on ub-q3k |
| wf_a178ead8-3f9 | the Implement loop-back workflow run | the local workflow journal; comment 206 on ub-lp9.14 |
| wf_9f3cfe13-d21 | the Verify round-2 workflow run | the local workflow journal; comment 207 on ub-lp9.14 and comment 208 on ub-q3k |
| wf_ecfefc6c-213 | the Track workflow run that consolidated and filed the follow-ups | the local workflow journal; comment 211 on ub-lp9.14 |

## Links

- `ub-lp9.14` — this task; `init` left a workspace without its managed `AGENTS.md` block until a
  second command ran.
- `ub-q3k` — the csv renderer refused the lifecycle reports D27 promises in all five formats.
- `ub-lp9` — the v1.1 epic both tasks and every follow-up sit under.
- `ub-lp9.32` to `ub-lp9.52` — the 21 follow-ups filed at Track; the glossary maps each key.
- `ub-nu0`, `ub-s82` and `ub-0p4` — the Windows probe, the sync-export default path in an `_unblock`
  workspace, and the symlinked-target residue, filed during the design rounds.
- `ub-c5o` and `ub-amw` — related and not blocking, per the Understand map. The first is the latent
  stdout-owner bug in `emit_report`, and the second is the `AGENTS.md` block that describes only
  unblock.
- Pull request — none open at the time of writing.
- Key files touched —
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-cli/src/commands/init.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-cli/src/commands/agents.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-cli/src/exit.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-cli/src/output.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-cli/tests/init_agents.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-config/src/discovery.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-render/src/backend/csv_fmt.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/docs/PRD.md` (the D27 amendment and FR-14),
  `/Users/ramosmig/Public/WS-Labs/unblock/docs/plans/01-design-spine.md` (§4 and §5b).
- Memories — `project-dogfood-unblock-is-the-tracker`, curated in this commit, and
  `feedback-macos-probe-masks-linux-ci-path-confinement`, the trap behind Verify round 1's
  canonicalization finding.
- Prior related run-reports — `runs/2026-08-07-mcp-stdout-framing-channel.md`, the D48 run that made
  the stdout-channel list measured, and `runs/2026-09-23-d51-two-product-modes.md`, the run whose
  merge this branch starts from.
