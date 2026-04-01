# Unblock — Git Primitives Research

**Purpose:** Evaluate which Git client-side primitives can be leveraged by the Unblock plugin to cover gaps left by the Beads comparison, substitute the Beads daemon model, and enrich the agent workflow without introducing custom storage or violating the "GitHub stores, Rust computes" architectural principle.

| | |
|---|---|
| **Version** | 1.0.0 |
| **Author** | Miguel Ramos |
| **Org** | websublime |
| **Date** | April 2026 |
| **Status** | Research |
| **Relates to** | `unblock-architecture-plugin.md`, `beads-vs-unblock-comparison.md` |

---

## Table of Contents

1. [Context and Motivation](#1-context-and-motivation)
2. [Architectural Boundary](#2-architectural-boundary)
3. [Git Hooks — Daemon Replacement](#3-git-hooks--daemon-replacement)
4. [Git Trailers — Structured Commit Metadata](#4-git-trailers--structured-commit-metadata)
5. [Git Worktrees — Agent Isolation](#5-git-worktrees--agent-isolation)
6. [Git Diff — Plan-Check Input](#6-git-diff--plan-check-input)
7. [Git Notes and Custom Refs — Verdict](#7-git-notes-and-custom-refs--verdict)
8. [Summary Table](#8-summary-table)
9. [Implementation Roadmap](#9-implementation-roadmap)

---

## 1. Context and Motivation

The Beads CLI comparison identified a set of gaps between Beads and Unblock. Several of those gaps are not product gaps — they are architecture gaps that arise because Beads uses a local SQLite daemon while Unblock uses GitHub as the source of truth. Beads requires a daemon, import/export, sync, compaction, and sandbox mode precisely because its storage can diverge from reality. Unblock has none of these problems.

However, one class of gaps is genuine: **local event handling and session isolation**. Beads' daemon reacts to local events (checkout, commit, push) in real time. Unblock has no equivalent because it is a stateless MCP server — it only runs when an agent calls a tool.

Git provides a set of client-side primitives that fill this gap natively, without any daemon:

- **Git hooks** — fire scripts on specific Git lifecycle events
- **Git trailers** — structured key-value pairs embedded in commit messages
- **Git worktrees** — multiple independent checkouts of the same repository
- **Git diff** — structured comparison of branches

Additionally, this document evaluates and formally dismisses two primitives that were considered:

- **Git notes** — annotate commits with separate metadata
- **Custom refs → blobs** — use Git's object store directly for arbitrary key-value data

---

## 2. Architectural Boundary

The guiding constraint is the Unblock architectural principle:

> **"GitHub stores, Rust computes."** GitHub is the single source of truth. Zero custom storage. Any data that needs to be visible to CI sessions, to the review agent, to the QA agent, or to the developer in the GitHub browser must live in GitHub.

This creates a clear rule for evaluating any Git primitive:

| Use case | Where data must live | Git primitive allowed? |
|---|---|---|
| Task state (status, agent, priority) | GitHub Projects V2 | No — GitHub only |
| Work log, decisions, deviations | GitHub Issue comments | No — GitHub only |
| Session context for review/QA in CI | GitHub Issue body + comments | No — GitHub only |
| Local trigger: claim on branch create | Local only (session signal) | Yes — git hooks |
| Commit attribution (audit trail) | Commit message (travels with push) | Yes — git trailers |
| Agent isolation (no file conflicts) | Local only (workspace isolation) | Yes — git worktrees |
| Spec vs diff comparison (pre-push) | Local only (ephemeral computation) | Yes — git diff |

The boundary is: **ephemeral local signals and computation → Git. Persistent shared state → GitHub.**

---

## 3. Git Hooks — Daemon Replacement

### What

Git hooks are scripts that Git executes automatically at specific points in the repository lifecycle. They live in `.git/hooks/` and are triggered by Git commands. They are not committed to the repository — they are local to each clone.

The most relevant hooks for Unblock:

| Hook | Trigger | When it fires |
|---|---|---|
| `post-checkout` | `git checkout`, `git switch`, `git worktree add` | After any branch or worktree switch |
| `commit-msg` | `git commit` | After the commit message is written, before the commit is finalised |
| `post-commit` | `git commit` | After a commit is finalised |
| `pre-push` | `git push` | Before pushing to the remote |

### Why

The Beads daemon exists to react to Git events without the developer having to remember to call `bd` commands manually. For example, when a developer checks out a branch named `worker1-task-42`, Beads can automatically mark the corresponding issue as in-progress.

Unblock has the same problem: agents work on branches named `issue-42-auth-flow`, but the claim happens only if the agent explicitly calls the `claim` MCP tool. If the agent forgets, the issue stays unclaimed.

Git hooks eliminate this dependency. The hook fires unconditionally — the agent cannot forget to trigger it.

### How

The `/setup-project` skill installs hooks as part of project bootstrapping. The hook scripts call the `unblock-mcp` binary directly (it is already on PATH after installation).

**`post-checkout` — auto-claim on branch switch:**

```bash
#!/usr/bin/env bash
# .git/hooks/post-checkout
# $3 = 1 if branch checkout (not file checkout)
[ "$3" = "1" ] || exit 0

BRANCH=$(git symbolic-ref --short HEAD 2>/dev/null)
ISSUE=$(echo "$BRANCH" | grep -oP '(?<=issue-)\d+')

[ -n "$ISSUE" ] || exit 0

# Only claim if issue is open and unblocked — the MCP tool validates this
unblock-mcp claim "$ISSUE" --agent "$(git config user.name)" --silent 2>/dev/null || true
```

The `--silent` flag (to be added to the `claim` tool) suppresses output on success and exits 0 on AlreadyClaimed, so the hook does not disrupt the checkout flow.

**`commit-msg` — inject trailers automatically:**

```bash
#!/usr/bin/env bash
# .git/hooks/commit-msg
COMMIT_MSG_FILE="$1"

BRANCH=$(git symbolic-ref --short HEAD 2>/dev/null)
ISSUE=$(echo "$BRANCH" | grep -oP '(?<=issue-)\d+')

[ -n "$ISSUE" ] || exit 0

# Inject issue trailer if not already present
grep -qP "^Issue: #" "$COMMIT_MSG_FILE" || \
  git interpret-trailers --in-place --trailer "Issue: #$ISSUE" "$COMMIT_MSG_FILE"
```

**`post-commit` — log commit to GitHub Issue:**

```bash
#!/usr/bin/env bash
# .git/hooks/post-commit
BRANCH=$(git symbolic-ref --short HEAD 2>/dev/null)
ISSUE=$(echo "$BRANCH" | grep -oP '(?<=issue-)\d+')

[ -n "$ISSUE" ] || exit 0

SHA=$(git rev-parse --short HEAD)
MSG=$(git log -1 --pretty=%s)

# Append to issue work log
unblock-mcp comment "$ISSUE" "COMMIT $SHA: $MSG" --silent 2>/dev/null || true
```

**`pre-push` — self-check gate:**

```bash
#!/usr/bin/env bash
# .git/hooks/pre-push
BRANCH=$(git symbolic-ref --short HEAD 2>/dev/null)
ISSUE=$(echo "$BRANCH" | grep -oP '(?<=issue-)\d+')

[ -n "$ISSUE" ] || exit 0

# Block push if issue is not in in_progress state
STATUS=$(unblock-mcp show "$ISSUE" --field status --quiet 2>/dev/null)
if [ "$STATUS" != "in_progress" ]; then
  echo "unblock: issue #$ISSUE is not claimed. Run 'claim $ISSUE' before pushing." >&2
  exit 1
fi
```

### Constraints

- Hooks are not committed — they must be installed by `/setup-project` on each clone.
- The `unblock-mcp` binary must be on PATH at hook execution time.
- Hooks must be fast and non-blocking. Use `--silent` and `|| true` to prevent hook failures from blocking Git operations.
- Hooks in team environments are per-developer — they are a convenience, not a guarantee. State integrity always lives in GitHub, not in hook execution.

---

## 4. Git Trailers — Structured Commit Metadata

### What

Git trailers are structured key-value pairs in the footer of commit messages, separated from the subject and body by a blank line. They follow the format `Key: Value` and are part of the Git specification since Git 2.33 via the `git interpret-trailers` command.

```
implement rate limiter (#42)

Token bucket algorithm chosen over sliding window.
See DECISION comment on #42 for rationale.

Issue: #42
Supervisor: rust-supervisor
Blocked-by: #38 (closed)
Unblocks: #50
Story-points: 3
```

Trailers are not an annotation layer — they are part of the commit message itself. They travel with `git push`, they are visible in `git log`, and GitHub renders them in pull request diff views.

### Why

The Unblock feature research doc describes `://commit-context` — a tool that enriches commit messages with graph context (what this commit unblocks, what it was blocked by, the supervisor that worked on it, story points). Without a structured format, this information is prose that cannot be queried later.

Git trailers make commit history machine-readable. After closing an issue and merging the branch, `git log --grep="Issue: #42"` reliably returns every commit that touched that issue. `git log --grep="Supervisor: rust-supervisor"` returns every commit by that supervisor. This is an audit trail with zero infrastructure.

Additionally, trailers resolve the `discovered-from` semantic dependency type (Beads gap G9). Beads distinguishes `discovered-from` from `blocks` — a soft link that says "I found this issue while working on that one, but it does not block me." Unblock currently implements this as `blocked_by + comment`. A `Discovered-from:` trailer in the commit that created the new issue captures the relationship in the code history directly.

### How

**`git interpret-trailers` — the standard tool:**

```bash
# Add trailers to a commit message file
git interpret-trailers --in-place \
  --trailer "Issue: #42" \
  --trailer "Supervisor: rust-supervisor" \
  .git/COMMIT_EDITMSG

# Verify trailers are well-formed
git log -1 --format="%(trailers)" HEAD

# Query by trailer key
git log --all --format="%(trailers:key=Issue)" | sort | uniq -c
```

**`commit_context` MCP tool — proposed:**

A new MCP tool that agents call before committing. It reads the issue data and the dependency graph, and returns a formatted commit message with the correct trailers:

```rust
pub struct CommitContextParams {
    pub issue: u64,
    pub summary: String,          // "implement rate limiter"
    pub body: Option<String>,     // optional extended prose
}

pub struct CommitContextResult {
    pub message: String,          // full commit message with trailers
    pub trailers: Vec<(String, String)>,
}

// Example output:
// implement rate limiter (#42)
//
// Token bucket algorithm. See DECISION comment on #42.
//
// Issue: #42
// Supervisor: rust-supervisor
// Blocked-by: #38 (closed)
// Unblocks: #50
// Story-points: 3
```

The agent receives this message and uses it with `git commit -F <(unblock-mcp commit-context 42 "implement rate limiter")` or equivalent.

**Trailer schema for Unblock:**

| Trailer | Value format | Required | Notes |
|---|---|---|---|
| `Issue` | `#N` | Yes | Always present on issue branches |
| `Supervisor` | `string` | Yes | The supervisor that implemented |
| `Blocked-by` | `#N (closed)` | When applicable | Shows what was unblocked by this commit |
| `Unblocks` | `#N, #N` | When applicable | Issues newly unblocked |
| `Discovered-from` | `#N` | When applicable | Soft dep — issue found while working on N |
| `Story-points` | integer | Optional | Set if the issue has story points |

**Querying trailers — `trace` MCP tool (proposed):**

```bash
# All commits for issue #42
git log --all --format="%H %s" --grep="Issue: #42"

# All issues a supervisor worked on
git log --all --format="%(trailers:key=Issue)" --grep="Supervisor: rust-supervisor" \
  | grep -oP '#\d+' | sort | uniq

# All issues discovered while working on #41
git log --all --grep="Discovered-from: #41" --format="%s"
```

The `trace` MCP tool wraps these queries and returns structured data. It uses `git2` (libgit2 Rust bindings) for programmatic access without shelling out.

### Constraints

- Requires Git 2.33+ for `git interpret-trailers`. All modern systems satisfy this.
- GitHub renders trailers in PR descriptions but not in the issue timeline. Trailers are for `git log` and the `trace` tool, not for GitHub UI consumption.
- Trailers cannot be added retroactively without rewriting history. The `commit-msg` hook ensures they are injected at commit time.

---

## 5. Git Worktrees — Agent Isolation

### What

`git worktree` allows multiple independent working trees to be attached to a single Git repository. Each worktree has its own branch, index, and working directory, but shares the same `.git` object store.

```bash
# Create a worktree for issue #42
git worktree add ../unblock-issue-42 -b issue-42-auth-flow

# List active worktrees
git worktree list

# Remove when done
git worktree remove ../unblock-issue-42
git branch -D issue-42-auth-flow
```

The worktree at `../unblock-issue-42` is a fully functional checkout. Changes there do not affect the main working tree. Two worktrees can be on different branches simultaneously.

### Why

The Beads molecular chemistry system (wisp, bond, burn) exists to create ephemeral working contexts: a `wisp` is a temporary issue with an isolated working environment, a `bond` pairs two wisps for related parallel work, a `burn` discards the wisp cleanly. The underlying need is agent isolation — preventing an agent working on issue #42 from interfering with an agent working on issue #50.

With Unblock, git worktrees provide exactly this isolation natively, without any custom issue types or daemon coordination. Each agent gets a worktree. Worktrees cannot conflict because they are separate directory trees. When the agent finishes (push + label `needs-review`), the worktree is removed. This is the burn.

Additionally, the Unblock plugin architecture document already notes worktree support as a future capability for parallel implementation:

> "Claude Code provides: Worktree support (future: parallel implementation)"

Git worktrees are the mechanism that makes this concrete.

**Beads concept mapping:**

| Beads concept | Git worktree equivalent |
|---|---|
| `bd mol wisp` | `git worktree add ../issue-N -b issue-N-slug` |
| `bd mol bond A B` | Two worktrees, no dep between them, same parent issue |
| `bd mol burn` | `git worktree remove --force ../issue-N && git branch -D issue-N-slug` |
| Wisp isolation | Each worktree is a separate directory — no cross-contamination |
| Wisp lifecycle tracking | `git worktree list` — the MCP server reads this |

### How

**Integration with `/start-task`:**

The `/start-task` skill currently creates a branch and works in the main working tree. With worktrees, each task gets an isolated directory:

```bash
# Phase: Branch + Implementation (modified)
BASE_DIR=$(git rev-parse --show-toplevel)
PARENT_DIR=$(dirname "$BASE_DIR")
WORKTREE_PATH="$PARENT_DIR/$(basename $BASE_DIR)-issue-$ISSUE"

git worktree add "$WORKTREE_PATH" -b "issue-$ISSUE-$SLUG"
cd "$WORKTREE_PATH"

# ... agent implements here ...

git push origin "issue-$ISSUE-$SLUG"
cd "$BASE_DIR"
git worktree remove "$WORKTREE_PATH"
```

**`worktree` MCP tool — proposed:**

```rust
pub enum WorktreeAction {
    /// Create a worktree for an issue (wisp)
    Create {
        issue: u64,
        base: Option<String>,  // default: "main"
    },
    /// List active worktrees with their associated issues
    List,
    /// Remove a worktree for an issue (burn)
    Remove {
        issue: u64,
        force: bool,
    },
    /// Show which worktrees are active (for prime context)
    Status,
}

pub struct WorktreeInfo {
    pub issue: u64,
    pub path: PathBuf,
    pub branch: String,
    pub commit: String,
    pub dirty: bool,  // uncommitted changes
}
```

The `prime` tool already shows agent context. Adding worktree status to `prime` output makes it possible to see at a glance which issues have active worktrees:

```
## Active worktrees
- #42: issue-42-auth-flow — 3 commits ahead of main, 2 uncommitted files
- #50: issue-50-rate-limiter — clean, ready to push
```

**Parallel agent orchestration:**

With worktrees, the orchestrator can dispatch multiple implementation supervisors in parallel, each to their own worktree. The dependency graph ensures that only unblocked issues are dispatched. The `ready` tool already computes this — the orchestrator calls `ready`, picks the top N issues, creates N worktrees, and dispatches N agents simultaneously.

```
ready → [#42 (P0), #50 (P1), #55 (P2)]

dispatch rust-supervisor → worktree for #42
dispatch node-supervisor → worktree for #50

# #55 depends on #50 — not dispatched until #50 is done
```

### Constraints

- Worktrees require the repository to not be in a bare state.
- The `git worktree add` command requires Git 2.5+.
- Worktrees are local — they do not sync to the remote. They are session-scoped.
- The `/start-task` skill's `NEVER use isolation: "worktree"` ban in the current CLAUDE.md template refers to Claude Code's `Task()` dispatch isolation, not to Git worktrees. These are orthogonal. Git worktrees are created explicitly by the skill, not by the Claude Code runtime.
- If the agent session ends before the worktree is removed (crash, timeout), the worktree remains on disk. A `worktree prune` command in the `/setup-project` setup and in the `SessionStart` hook cleans stale entries.

---

## 6. Git Diff — Plan-Check Input

### What

`git diff` computes the difference between two tree-ish objects — branches, commits, tags, or the working tree. The most relevant form for Unblock:

```bash
# All changes on the issue branch vs main
git diff main...issue-42-auth-flow

# File list only
git diff --name-only main...issue-42-auth-flow

# Statistics
git diff --stat main...issue-42-auth-flow

# Structured output (machine-readable)
git diff --numstat main...issue-42-auth-flow
```

### Why

The Unblock new feature research describes `://plan-check` — automatic comparison of the branch diff against acceptance criteria before push. The self-check loop in `/start-task` currently relies on the agent visually reading its own diff and acceptance criteria. This is error-prone: agents miss criteria, implement scope creep, or mark items as complete without corresponding code changes.

Git diff provides the structured input that makes plan-check mechanical:

- The diff shows exactly which files changed and how.
- The acceptance criteria from the issue body provide the expected changes.
- The agent (or a dedicated sub-agent) compares the two and identifies gaps.

This is not a new tool — it is a new use of existing data. The diff is already available. The issue's acceptance criteria are already parsed via `BodySections`. The `diff_summary` MCP tool simply makes the combination explicit and structured.

### How

**`diff_summary` MCP tool — proposed:**

```rust
pub struct DiffSummaryParams {
    pub issue: u64,
    pub base: Option<String>,  // default: "main"
}

pub struct DiffSummary {
    pub files_changed: Vec<FileChange>,
    pub insertions: u32,
    pub deletions: u32,
    pub acceptance_criteria: String,  // raw markdown from issue body
    pub branch: String,
    pub commits_ahead: u32,
}

pub struct FileChange {
    pub path: String,
    pub insertions: u32,
    pub deletions: u32,
    pub status: FileStatus,  // Added, Modified, Deleted, Renamed
}
```

The agent receives this and uses its own reasoning to identify mismatches:

```
diff_summary #42

Files changed (4):
  M  src/rate_limiter.rs    (+87 -12)
  M  src/main.rs            (+3  -1)
  A  tests/rate_limiter.rs  (+145 -0)
  M  Cargo.toml             (+1  -0)

Acceptance criteria:
  1. Implement token bucket algorithm
  2. Add integration test with 100rps load
  3. Add metrics endpoint /metrics/rate_limiter
  4. Document algorithm choice in Design Notes

→ Criterion 3 (/metrics/rate_limiter) has no corresponding file change.
→ src/metrics.rs was not modified. Possible gap.
```

The last two lines are produced by the agent's LLM reasoning over the structured output — not by the tool itself. The tool provides data; the agent provides analysis.

**Integration in `/start-task` self-check loop:**

```
Self-check loop (Phase 5):
  1. Run tests → if fail → fix → re-run
  2. Run build → if fail → fix → re-run
  3. Run lint → if fail → fix → re-run
  4. diff_summary #ISSUE → agent compares against acceptance criteria
     → if criterion missing → implement → re-check
  5. All pass → exit loop
```

Step 4 is new. It replaces the current "Diff review: read own diff against acceptance criteria" instruction (which relied entirely on the agent's memory of the criteria) with a structured tool call that surfaces the data directly.

### Constraints

- Requires the repository to be accessible from the MCP server. Since the MCP server runs locally (launched by Claude Code), it has access to the local git repository via `git2`.
- The base branch (`main`) must be up to date. If main is ahead of the branch's fork point, the diff may include unrelated changes. Use three-dot diff (`main...branch`) to compare only the branch's own changes.
- The plan-check analysis is probabilistic — it depends on the agent's reasoning. It does not guarantee coverage. It is a best-effort gate, not a formal proof.

---

## 7. Git Notes and Custom Refs — Verdict

This section formally evaluates and dismisses two approaches that were explored during the research process.

### 7.1 Git Notes

**What:** `git notes` annotates commits with separate metadata blobs stored in `refs/notes/*`, without altering the commit SHA.

**Why it was considered:** Attaching execution context (cycle time, unblocked issues, supervisor) to commits post-facto, without modifying commit messages. The `://trace` feature idea — reconstructing what happened during a session from git history — seemed to map naturally to notes.

**Why it is dismissed:**

The fundamental problems are structural and cannot be worked around:

**SHA-coupling.** A note is indexed by the commit SHA it annotates. Any operation that rewrites that SHA destroys the note:

```bash
git commit --amend          # new SHA → note orphaned on old SHA
git rebase main             # all SHAs rewritten → all notes lost
git cherry-pick             # new SHA → note not carried over
```

The `/start-task` self-check loop makes incremental commits during implementation. It is routine for agents to `--amend` the last commit to fix a lint error or adjust a message. Every such amend silently destroys the note attached to that commit.

**Not pushed by default.** Notes require explicit push configuration:

```bash
git push origin refs/notes/*
```

Without this in `.gitconfig` or in the push refspec, notes never leave the local machine. CI sessions (review, QA) check out a clean clone — they never see notes, even if they were pushed, because `git fetch` does not pull notes by default either.

**GitHub does not render notes.** The GitHub pull request UI, issue timeline, and code review interface have zero awareness of git notes. For a product whose value proposition includes visibility in GitHub, invisible metadata is not metadata.

**Merge conflicts.** When multiple agents push notes to the same ref namespace, Git treats it as a note tree merge. Conflicts are possible and are not automatically resolved.

**Verdict: dismissed.** Trailers cover the use case that notes were meant to address. Trailers travel with the commit, require no extra push configuration, and GitHub renders them.

### 7.2 Custom Refs → Blobs (Git Object Store Direct)

**What:** Writing arbitrary data directly to the Git object store via `git hash-object -w` and pointing named refs at the resulting blobs via `git update-ref`.

```bash
# Write a blob to the object store
SHA=$(echo '{"issue": 42, "claimed": true}' | git hash-object -w --stdin)

# Create a named ref pointing to it
git update-ref refs/unblock/claimed/42 $SHA

# Read it back
git cat-file -p refs/unblock/claimed/42
```

**Why it was considered:** Unlike `git notes`, custom refs are not coupled to commit SHAs. `refs/unblock/claimed/42` is a logical name — it does not break when commits are rewritten.

**Why it is partially retained as a local signal:**

The SHA-coupling problem is solved. The ref persists across rebase, amend, and cherry-pick. For purely local, session-scoped signals — "does this MCP server instance know that issue #42 has an active worktree?" — custom refs are a lightweight and idiomatic mechanism.

**Why it is dismissed for persistent or shared state:**

The push problem remains identical to git notes. The GitHub rendering problem remains identical. Any data that must survive a `git clone`, be visible in CI, or be queryable by a developer in the browser must live in GitHub.

**Verdict: narrow retained use.** Custom refs are acceptable as a local cache signal — the MCP server writes `refs/unblock/worktree/42` when a worktree is created and deletes it when the worktree is removed. This lets the server answer "which issues have active worktrees?" by reading `git for-each-ref refs/unblock/worktree/` without an API call. The data is ephemeral by design and does not need to be shared.

---

## 8. Summary Table

| Git Primitive | Beads Gap Covered | Use Case | Verdict | Where Data Lives |
|---|---|---|---|---|
| `git hooks` (post-checkout) | Daemon — auto-claim | Auto-claim on branch checkout | **Adopt** | Signal only — outcome in GitHub |
| `git hooks` (commit-msg) | Daemon — auto-inject metadata | Inject trailers at commit time | **Adopt** | Commit message (travels with push) |
| `git hooks` (post-commit) | Daemon — auto-log | Append commit to issue work log | **Adopt** | GitHub Issue comment |
| `git hooks` (pre-push) | Daemon — validation | Block push if issue not claimed | **Adopt** | Signal only |
| `git trailers` | G9 discovered-from, `://commit-context` | Structured commit metadata, audit trail | **Adopt** | Commit message (travels with push) |
| `git worktrees` | Wisp / Bond / Burn, parallel agents | Agent isolation, parallel implementation | **Adopt** | Local only (session-scoped) |
| `git diff` | `://plan-check` | Structured input for spec drift check | **Adopt** | Local only (ephemeral computation) |
| `git notes` | `://trace` | Post-commit metadata annotation | **Dismissed** | SHA-coupled, not pushed, GitHub-invisible |
| Custom refs → blobs | Local cache | Active worktree signal for MCP server | **Narrow use** | Local only, ephemeral |

---

## 9. Implementation Roadmap

All items below are additions to the existing project plan. They slot into Phase 1 (hooks + trailers, low effort) and Phase 3 gap features (worktrees, diff_summary).

### Phase 1 — v0.1.x (Low effort, high value)

**`setup --hooks`** — new sub-command of the `setup` MCP tool that installs the four hook scripts into `.git/hooks/`. Idempotent. Should be called by `/setup-project` automatically.

Tasks:
- Add `SetupHooks` variant to `SetupAction` enum in `unblock-mcp`
- Write the four hook scripts as embedded strings in the binary (no external files)
- Install to `.git/hooks/` with `chmod +x`
- Add `setup --hooks --remove` to uninstall

**`commit_context` tool** — reads issue + graph state, returns a formatted commit message with trailers.

Tasks:
- New MCP tool `commit_context` in `unblock-mcp`
- Reads issue body, deps, supervisor from graph
- Returns `CommitContextResult { message: String, trailers: Vec<(String, String)> }`
- Supervisor calls it before `git commit` in the self-check completion step

### Phase 3 — v1.1.x (Medium effort)

**`worktree` tool** — create, list, remove, status.

Tasks:
- New MCP tool `worktree` with `WorktreeAction` enum
- Uses `git2` crate for worktree operations
- Integrate `worktree status` into `prime` output
- Update `/start-task` skill to create worktree in Phase 5

**`diff_summary` tool** — structured branch diff + acceptance criteria.

Tasks:
- New MCP tool `diff_summary` in `unblock-mcp`
- Uses `git2` crate for diff computation
- Returns `DiffSummary { files_changed, insertions, deletions, acceptance_criteria, branch }`
- Integrate as Step 4 of the self-check loop in `/start-task`

**`trace` tool** — query git log by trailer keys.

Tasks:
- New MCP tool `trace` in `unblock-mcp`
- Uses `git2` crate to walk commits and parse trailers
- Supports queries: by issue number, by supervisor, by discovered-from
- Returns `Vec<CommitTrace> { sha, message, trailers, timestamp }`

### Effort estimate

| Feature | Effort | Phase |
|---|---|---|
| `setup --hooks` (4 hooks) | 1 day | Phase 1 |
| `commit_context` tool | 1 day | Phase 1 |
| `worktree` tool | 2 days | Phase 3 |
| `diff_summary` tool | 1 day | Phase 3 |
| `trace` tool | 1.5 days | Phase 3 |
| **Total** | **~6.5 days** | |

### Dependencies

- `git2` crate (libgit2 Rust bindings) — required for `worktree`, `diff_summary`, `trace`. Risk: `git2` requires libgit2 as a native dependency, which can complicate static musl builds. Evaluated in Risk Register (Phase 3 spike task 3.2.0 in Project Plan). Alternative: shell out to `git` binary for complex operations and use `git2` for simple reads only.
- No new GitHub API surface — all features are client-side Git.
- No new external storage — all persistent data continues to flow to GitHub via existing tools.

---

## 10. Design Decisions (post-review, 2026-04-01)

| # | Primitive | Decision | Rationale |
|---|---|---|---|
| GP1 | `commit_context` tool | **Adopted** — MCP tool in Phase 2 | Agent asks "what commit message?" and gets structured response with trailers. Low effort, high audit value |
| GP2 | Git hooks (3) | **Adopted** — plugin scope via `/setup-project` | `post-checkout` (auto-claim), `commit-msg` (trailer injection), `pre-push` (claim warning). Replaces daemon model |
| GP3 | Custom refs | **Adopted** — already in Epic 3.2 | `refs/unblock/*` for persistent cache. Validated with `git2` spike (2026-04-01) |
| GP4 | `post-commit` hook | **Rejected** | Comment spam — 20+ commits per session would pollute the issue timeline. Comments should be semantic (DECISION/COMPLETED/DEVIATION), not activity logs |
| GP5 | `pre-push` as blocker | **Rejected** | Too restrictive. Agents push branches not associated with issues (docs, refactors). Changed to non-blocking warning |
| GP6 | `worktree` MCP tool | **Rejected** | IDE/plugin manages worktrees, not the MCP server. Claude Code already has built-in worktree support. Duplicating in MCP is scope creep |
| GP7 | `diff_summary` MCP tool | **Rejected** | Agent has direct access to `git diff`. MCP server knows issues and graphs, not code. Mixing concerns |
| GP8 | `trace` MCP tool | **Rejected** | Walking git log by trailers is a CLI utility, not an MCP tool. The MCP server talks to GitHub, not local git history |
| GP9 | Git notes | **Rejected** | SHA-coupled (breaks on amend/rebase), not pushed by default, GitHub-invisible. Trailers cover the same use case |
