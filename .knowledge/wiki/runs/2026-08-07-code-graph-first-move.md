---
name: 2026-08-07-code-graph-first-move
description: Making the code graph a stated first move for structural questions (tracker ub-gwe) — a session-start instruction ignored for a whole preceding task and unfollowable anyway because nothing was indexed, a rule extended in place rather than duplicated into a second document, three Verify hardenings that each closed a real hole rather than polished prose, and five findings filed rather than absorbed — including that no spawned agent team can reach the tools at all.
type: run
date: 2026-08-07
branch: ub-gwe-code-graph-first
pr: '-'
issues: [ub-gwe, ub-4ne, ub-wvh, ub-lg7, ub-tge, ub-amw]
---

# Run — the code graph as the first move

## Context

Task `ub-gwe` — a SessionStart hook instructs every session to reach for the code-graph MCP tools
first when navigating code, and across the whole preceding task the orchestrator never called them
once and never said so. Taken off `main` at 9adc2cb (the tip left by `ub-og3`, the D48
stdout-channel work). Branch `ub-gwe-code-graph-first`; no pull request open at the time of writing.
The lifecycle ran across two sessions: design Review (three adversarial lenses plus a coordinator),
Implement (a single implementer in an isolated worktree), Verify (three adversarial lenses, each in
its OWN private checkout, plus a read-only coordinator), a hardening pass by a second single
implementer, an orchestrator-run delta re-verification, and Track.

## What & why

THE FAILURE THAT OPENED IT. Through the entire preceding task the structural questions — who calls
this, what does this reach — were answered with grep sweeps. One of those answers, that a particular
function had exactly one caller, turned out load-bearing enough that a gate script now rests on it. A
check afterwards showed the graph server had zero projects indexed, so the instruction could not have
been followed as written even by a session that tried. Two distinct failures, then: the protocol was
ignored, and the fact that it was unfollowable was never surfaced to anyone.

WHAT THE REPO ALREADY HAD, AND WHAT IT DID NOT. The hard rules in `CLAUDE.md` already carried the
NEGATIVE half — that semantic search is discovery and not authority, locating `file:line` and never
replacing a full read of the authoritative spec. Absent was the POSITIVE half: that a structural
question opens at the graph rather than at a sweep, and what a session owes when the repository is
not indexed.

Documents read rather than restated: the `CLAUDE.md` hard-rules list and document map, PROCESS
section 4 (teams per phase and how a team writes), section 6 (the issue-comment duty), section 8 (the
knowledge layer and the same-commit rule), and the substantive-pull-request predicate in
`ci-cd-and-distribution.md` section 2.3.

One orientation fact worth keeping, because it inverts the premise the task was filed on: by the time
this session started the graph WAS indexed and fresh — ready, 7367 nodes and 35275 edges, stamped at
the same `main` tip the branch is based on. The task's own bullet asking for the repository to be
indexed was therefore already satisfied and produced nothing to commit.

## Outcome

LANDED — two edited documents, plus two riders Miguel decided explicitly.

In `CLAUDE.md` the existing hard-rules bullet was REPLACED rather than supplemented. The rewritten
bullet sends structural questions (who calls this, is this reachable, which paths do X, what does
this symbol depend on) to the code-graph tools before any grep sweep, leaves text questions —
zero-live-hits sweeps, doc and config content, a literal string — to grep, forbids silently falling
back when the graph is unavailable, unindexed or indexed at another commit, and preserves the old
authority cap word for word with its subject widened from semantic search alone to the graph and
semantic search together. The list stays at five bullets: a second rule about the same subject in a
second document is how these decay, which is the argument the task itself made.

In `docs/PROCESS.md` section 4 the sentence licensing `Explore` for broad search was narrowed to
broad TEXT search — unamended it was the only other statement in either always-in-context file about
how an Understand team searches, and it licensed exactly the grep-first habit this task exists to
stop. Below the teams-per-phase table a new paragraph, "Understand opens at the code graph", carries
the mechanics: which server, that its index is machine-local and commit-stamped, that it is the MAIN
checkout that gets indexed, and the one-line field the Understand comment now carries. The
enumeration of structural questions lives in exactly one place, the `CLAUDE.md` bullet; the process
guide repeats no list.

RIDERS. The `codebase-memory` server entry in `.mcp.json`, committed exactly as it stands — Miguel
was told the trade-off (the binary lives outside this public repository, so a fresh clone gets a
server that fails to start) and chose to commit it without an installation note. And the
`.unblock/issues.jsonl` re-export.

GATE VERDICTS. Design Review: PASS WITH CHANGES. Its three lenses were architecture (doc hierarchy
and rule placement), evidence (does the claimed tooling exist and say what the task says), and
operability (can a team execute the instruction as written); the consolidated output superseded the
design brief's proposed wording in full and became a byte-exact implementation spec, so the
implementer made no judgement calls. Verify: PASS WITH NOTES, ZERO must-fixes. Its three lenses were
a claim audit (does the new prose assert more than is true), an executable-gates lens (run everything
that could go red), and a coupling-and-mutation lens (break each claimed coupling and observe what
actually turns red).

THE THREE HARDENINGS Verify proposed, all applied on Miguel's call by a second single implementer,
all inside the PROCESS paragraph — `CLAUDE.md` came out byte-identical to the gated version, proven
by an md5 of the diff hunk rather than by eye. Each was a real hole:

- The rule prescribed a remedy it never named. It said to index or re-index the main checkout and to
  fix it or say so, while no command or tool verb appeared anywhere in either file and the server's
  own documentation lives outside this public repository. A rule whose remedy is unnamed collapses
  into its escape hatch, because every session takes "say so" when "fix it" has no referent. The
  paragraph now names `index_status` and `index_repository`.
- The expected steady state during branch work was unstated, and it is the COMMON case rather than
  the exception: the index tracks the main checkout while section 4 of that same file mandates that
  Implement teams write in an isolated worktree, so a structural question asked on a branch
  legitimately answers for main's commit. The paragraph now says that declaring it stale and naming
  what answered instead is the correct outcome there.
- The placeholder for the stale value in the new issue-comment field invited a token that a CI
  predicate matches. The run-report gate's session-local-id pattern
  (`scripts/knowledge/run-report-gate.sh:11`) matches a stale marker carrying a sha hand-written with
  a leading uppercase hexadecimal letter, which would silently flip an otherwise-trivial record
  re-export into a substantive one. The placeholder now asks for a lowercase short sha; all three
  field values were tested against that exact regular expression and do not match, while an
  uppercase-leading sha does.

DELTA RE-VERIFICATION, run by the orchestrator because the gate had said explicitly that its verdict
did not cover the hardenings: six check scripts all exit 0; doc-lint OK across 19 documents;
knowledge-lint OK across 62 pages; layering OK. Plus the mechanical property set — the `CLAUDE.md`
bullet is one physical line of 845 characters, the phrase "discovery, not authority" occurs
contiguously exactly once, the file carries zero middot characters, the live decision-range literal
still occurs exactly once, the hard-rules list is still five bullets, the closing sentence about a
returned chunk being a pointer survives verbatim, no trailing whitespace was introduced, and the
eight added process-guide lines peak at 107 characters.

TWO THINGS THE GATES SAID OUT LOUD rather than absorbed. The design gate disclosed that the rule is
INERT for spawned teams by the tooling's design and not by the text's — every agent definition
declares an explicit tool allow-list and none lists any MCP tool — so a spawned team lands in the
"unavailable" arm, which is what that arm is for. And Verify CORRECTED a design-gate claim: the gate
had said a required check rests on `CLAUDE.md` carrying exactly ONE occurrence of the live
decision-range literal; mutation showed that wording is a rationale comment, not an enforced
assertion, since appending a second occurrence left all five range scripts green while a stale range
did fire red.

FIVE FINDINGS SPLIT OUT rather than absorbed, all filed this run: `ub-4ne`, `ub-wvh`, `ub-lg7`,
`ub-tge`, `ub-amw` — one-liners in Links below.

## Gotchas

- A session ended mid-task with the design Review verdict and the Implement summary never written to
  the issue thread; both were recovered from the previous session's scratchpad files and restated in
  full on `ub-gwe`. A phase outcome not written to the tracker at the moment it is produced can be
  lost with the session.
- Column budgets measured with `awk` or `wc` overcount this repo's prose, because every em-dash is
  three bytes. Measure characters with `python3`.
- One reported contradiction did not survive filing: the claim that `AGENTS.md` contradicts the
  `CLAUDE.md` document map is wrong, because that map row is scoped to the unblock tracker by its own
  heading. The narrower real defect is what `ub-amw` records.
- The knowledge-tree bash guard fails closed when an interpreter and a knowledge-tree path share one
  shell command, and earlier still on any command its shell-quoting pass cannot parse — a heredoc of
  prose containing apostrophes is denied wherever it writes. Use the Read and Write tools rather than
  shell redirection.
- A spawned agent whose type declares no `Write` tool cannot produce a file artifact at all, and the
  shell is not a fallback for it under the guard above. Pick the agent type by the tools the artifact
  needs, not by the subject matter.
- The unblock MCP server did not connect to this session, and the server was healthy — a direct stdio
  handshake answered in milliseconds. The cause is the wiring: `.mcp.json` starts it with `cargo run`,
  whose cold build exceeds the MCP client's startup timeout. Work proceeded through a stdio harness
  against the already-built binary.
- The committed git record was stale at orientation by exactly one field set: the predecessor task was
  still recorded open because its export ran a minute before its close. This is structural rather than
  accidental — exporting happens in the work commit and closing happens on merge, which is always
  later — so the closed state reaches the git record one pull request late.

## Glossary

No session-local ids were used in this run.

## Links

- `ub-gwe` — make the codebase graph a stated first move for structural questions; the task this run
  closes.
- `ub-4ne` — no spawned agent team can reach the code-graph tools: every agent definition declares an
  explicit tool allow-list naming no MCP tool, and the committed server entry is project-scoped and
  therefore inert until approved, so it reads as wired without starting.
- `ub-wvh` — `scripts/knowledge/tests/landing-verify.sh` has been red since it landed, with four
  failures byte-identical on pristine `main`, and is wired into no workflow.
- `ub-lg7` — no executable check anchors the `CLAUDE.md` hard rules: deleting the entire rewritten
  bullet leaves all six check scripts and the doc-lint green, proven non-vacuous by a positive control
  that injected rejectable bytes into a corpus file and got two findings back.
- `ub-tge` — the `CLAUDE.md` decision-range check enforces only presence while its own rationale rests
  on there being exactly one occurrence, so a future cascade adding a second mention defeats it
  silently.
- `ub-amw` — the repo describes no MCP server other than unblock, and `AGENTS.md` is a fully generated
  single-server block with no place to put one.
- Key files: `/Users/ramosmig/Public/WS-Labs/unblock/CLAUDE.md`,
  `/Users/ramosmig/Public/WS-Labs/unblock/docs/PROCESS.md`,
  `/Users/ramosmig/Public/WS-Labs/unblock/.mcp.json`,
  `/Users/ramosmig/Public/WS-Labs/unblock/scripts/knowledge/run-report-gate.sh`.
- Prior related run-report: `runs/2026-08-07-mcp-stdout-framing-channel.md` (the task during which
  this omission happened and from which `ub-gwe` was raised).
