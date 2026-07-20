# PROCESS — how we build unblock

This is the **methodology** a fresh session needs (the *how*); `CLAUDE.md` is the *contract* (the *what*).
Imported into `CLAUDE.md` via `@`, so every session loads it. Keep it lean — it is always in context.

## 0. North star
Optimize for **correctness and completeness over speed** — this is a from-scratch rewrite of a data-integrity
tool. **Never simplify the solution to make progress; if you reach that point, stop and ask Miguel.**

## 1. Doc topology & single source of truth
- `PRD.md` = product truth · `01-design-spine.md` = **interface SSOT** · `crates/*.md` = how each crate is built ·
  **unblock (MCP) = live task state** (git record `.unblock/issues.jsonl`; wiring `AGENTS.md`) · `00-roadmap.md` = when ·
  `implementation-plan.md` = task DAG + AC. (`STATUS.md` is now a retired pointer stub — history in git.)
- **Hierarchy: PRD > spine > crate plans.** The spine wins on any cross-crate interface disagreement; a crate plan
  that diverges is the bug, not the spine.

## 2. Lifecycle of any change
`understand → decide → spec/plan → review → implement → verify → track`.
- **Spec-first on drift:** when reality diverges from the docs, fix the authoritative doc FIRST (spine for
  interfaces, PRD for product), then implement. Never let code and docs drift silently.
- Pick the next **ready** task from **unblock** via the `query` tool (`ready` action; ready = all its deps are
  satisfied); register/update it via the `issue` (+ `comment`) tools and re-export the git record
  `.unblock/issues.jsonl` (the `sync` tool's `export` action) in the **same commit** as the work; a task is done only
  after it meets its PRD acceptance criteria **and** passed review.
- **Lock versions just-in-time.** Review and lock a future version (v1.1, v1.2, …) only when the current version
  **nears completion**, using real learnings — not speculatively up front. Later versions stay PROPOSED *direction*
  whose job is to shape the current version's **seams**, not to be planned in detail. (So **unblock** carries
  decomposed tasks only for the active version.)

## 3. Decisions
- Every real decision gets a **D-id** in `PRD §4` with its rationale.
- **Decision-change checklist** — when a D-id, FR tier, or command/name surface changes, update *in one commit*: the
  decision row + any superseded sibling, PRD (§5/§6/§11/§12/§13/§14), roadmap, README, ci-cd, impl-plan, the owning
  crate plan, and the spine. It is not "done" until a grep for the old framing/version/name returns **zero live
  hits**. The CI doc-lint (`ci-cd-and-distribution.md §2.1`) enforces the recurring classes.
- **Confirm genuine forks with Miguel** before acting; use the question tool for real choices, not for defaults you
  can pick yourself. Don't barrel ahead past a decision that's his to make.

## 4. Multi-agent orchestration (when & how)
- **Role separation — the main session is the *orchestrator*, not an implementer.** Miguel conducts the main
  session; the orchestrator (Claude) makes decisions *with* Miguel, **assigns the per-phase team as a Workflow —
  including the Implement phase** — awaits the outcome, then decides/acts on it. The main session edits files
  **directly only** in conversation or genuinely trivial turns (a one-line fix, an unblock `issue` status update).
  **Scaffolding, crate creation, and any multi-file / multi-crate change are always done by a spawned team — even
  when the spec is exact** (a well-specified change is *not* a trivial one). This keeps the orchestrator's context
  clean and makes every substantive change an auditable Workflow transcript instead of ad-hoc main-session edits.
- **How a team writes (operational).** An Implement team writes in an **isolated git worktree** (`isolation:
  "worktree"`) when writers run in parallel, or via a **single implementer agent** when the artifact must stay
  coherent (e.g. one scaffold); it returns the diff + a short summary. The orchestrator reviews that diff and runs
  the Verify gate — it does **not** hand-write the artifact. **Never run file-mutating agents in the shared working
  tree** (they can switch branches or clobber state); isolate them.
- **Use a Workflow** for substantive, decomposable, or review work (decompose + cover in parallel; independent
  perspectives before committing; scale beyond one context). **Solo** for trivial/mechanical edits or conversation.
- **Proportionality.** The per-phase team lineup (below) is the **default for substantive work**; trivial/mechanical
  tasks run **solo or with 2 agents**. The **≥3-agent gate is mandatory only for substantive changes** — anything
  touching a public interface, a spec/contract, or multiple crates/files. Don't spawn a 4-agent team for a one-line edit.
- **Patterns we use:** N specialists + a coordinator (discovery, review); per-crate fan-out + coordinator (plans);
  lens-based review + coordinator (gap/drift); spine-first → consumers → verify (reconciliation).
- **Avoid the schema-output loop:** for large outputs, have agents WRITE files and RETURN short summaries; cap
  finding lists (~12–15); keep schemas bounded.
- **Adversarially review before locking a phase** (multi-lens or gap/drift), then consolidate + run a verify pass.

### Teams per phase (hand-pick from the repo's agents)
Every session runs the **same lineup** per lifecycle phase, spawned as a Workflow (mates in parallel + the
coordinator synthesizing). Both gates (design **Review** and **Verify**) require **≥3 specialist agents + the
coordinator**, run adversarially.

| Phase | Mates (specialists) | Coordinator |
|---|---|---|
| **Understand** | `Plan` (architect), `rust-engineer`, `research-analyst` | `multi-agent-coordinator` |
| **Decide** | `Plan`, `rust-engineer`, `research-analyst` (+ `project-idea-validator` for product forks) | `multi-agent-coordinator` |
| **Spec/Plan** | `Plan`, `rust-engineer`, `project-manager` | `multi-agent-coordinator` |
| **Review** (design gate, ≥3) | `Plan`, `rust-engineer`, `research-analyst` (± `project-manager` / `project-idea-validator` lenses) | `multi-agent-coordinator` |
| **Implement** | `rust-engineer` + the crate specialist: `mcp-developer` (`unblock-mcp`), `cli-developer` (`unblock-cli`); core crates `rust-engineer`-led (`refactoring-specialist` for refactors). **Runs as a Workflow that writes in an isolated worktree / a single implementer — the orchestrator never hand-writes the change** (see §4 role + operational notes). | `multi-agent-coordinator` |
| **Verify** (quality gate, ≥3) | `code-reviewer`, `qa-expert`, `rust-engineer` (+ `/security-review` for sync/storage/MCP-input changes) | `multi-agent-coordinator` |
| **Track** | `project-manager`, `git-workflow-manager` | `multi-agent-coordinator` |

`fullstack-developer` is not used (no full-stack/UI surface). `Explore` may augment **Understand** for broad search.

## 5. Review & QA discipline
- **No close without review.** Mark a finding/CF **RESOLVED only after the fix LANDS** in the authoritative doc (the
  spine for interfaces) — not when it is merely decided.
- The **design Review** (pre-implement) and the **Verify quality gate** (post-implement) are each **≥3-agent**
  adversarial passes (teams in §4). A phase/change is not locked until its gate's coordinator returns a pass.
- **Gate failure loops back:** a failed gate returns the work to the prior phase (Verify → Implement; design
  Review → Spec/Plan). After **2 iterations** without a pass, **escalate to Miguel** rather than looping further.
- Keep the README consistency report and the **unblock** task state current; re-run the gap/drift sweep after
  significant plan changes.
- **Drift/gap policy:** any drift or gap is **reported and, by default, resolved in the same session** — never
  deferred or left to accumulate (that is exactly how the 24-finding pile formed). Report it with the template
  `docs/plans/templates/drift-gap-report.md` so every coordinator/session uses the same shape; land the fixes in the
  real docs and log the outcome as a **comment** on the task's unblock issue + the commit (git is the archive — don't keep standalone reports).
- **Spine is the reference; resolution is collaborative.** On a plan↔spine interface disagreement the spine is the
  authoritative reference, but the fix is a **review → iterate → adjust loop with Miguel**: usually the plan is
  updated to match the spine, but the drift may reveal a *spine* bug. **Never silently overwrite a plan or the spine.**

## 6. Tracking, commits & PRs
- **unblock (MCP) is the durable, cross-session system of record** for tasks — configured in `mcp.json` + `AGENTS.md`;
  in a fresh session use the native `mcp__unblock__*` tools and read `unblock://capabilities` / `unblock://schema`
  for the surface. Its **git-backed record is `.unblock/issues.jsonl`** (the D5 committed JSONL export; the local
  `unblock.db` is gitignored). Harness Tasks = per-session execution. Update the unblock issue the moment a task
  changes state, and re-export `.unblock/issues.jsonl` (the `sync` tool's `export` action) in the **same commit** as
  the work. **D5 model B** — manual export/import, no 3-way merge or locks; until the v1.2 shared remote, concurrent
  sessions reconcile `.unblock/issues.jsonl` by hand.
- **Every outcome is a comment on the task's issue.** Each phase/analysis result for a task — the Understand map, the
  Decide rationale + Miguel's fork resolutions, each gate verdict (design Review / Verify) with its must-fixes, the
  Implement summary, the findings, and the Track/merge result — is recorded as a **comment** on the task's unblock
  issue (the `comment` tool). The comment thread IS the durable, auditable per-task narrative that the verbose
  `STATUS.md` rows used to carry; `.unblock/issues.jsonl` snapshots it into git (re-export in the **same commit** as
  the work).
- **Branch off `main`** — never commit directly to `main`. Branch name: `t<mid>.<n>-<slug>` (e.g. `t0.6-libsql-impl`).
- A change may be committed only **after both gates pass** (design Review **and** Verify). The Track team
  (`git-workflow-manager`) makes **Conventional Commits** (`feat`/`fix`/`docs`/`refactor`/`test`/`chore`/`ci`…),
  **atomic** — one logical change per commit.
- **Claude opens the PR** (`gh pr create`): summary + the linked **unblock** issue id + gate results. Claude does **not**
  merge — merging to `main` is a human gate (Miguel/an approver) unless Miguel says otherwise.
- **One PR per task (T-id)** by default — atomic Conventional Commits within it; split a large task into sub-PRs
  only when it has genuinely independent, separately-reviewable parts.
- **On PR merge → close the unblock issue** (the `issue` tool's `close` action). "Done" is tied to the merge, after both gates.
- **Bootstrap ends at T0.1.** Foundation docs are edited solo only until the first crate work begins; from **T0.1
  on, spec/plan/doc changes follow the same review → commit → PR discipline as code** (they are artifacts too).

## 7. Language & artifacts
- **Converse in Portuguese; write all artifacts (code, docs, comments) in English.**
- In always-on docs (`CLAUDE.md`, this file): pointers over prose — they cost context every session.
