# DEV-ROADMAP.md

**how ://unblock gets built — from zero to product.**

---

## overview

the project has four products built in sequence. each depends on the previous being validated before starting the next. the monorepo (`websublime/unblock`) holds everything in a single cargo workspace.

```
phase 1-3     mcp server (v1.0.0)              ← you are here
v1.1.0        post-launch enhancements (homebrew, stale, jira, etc.)
phase 4       plugin (claude code + copilot)
phase 5-6     desktop app (v2.0.0)
phase 7       monetisation
```

---

## phase 1 — foundation (mcp server)

**goal:** an agent can find, claim, edit, complete work and see cascade unblocking.

**crates:** `unblock-core`, `unblock-github`, `unblock-mcp`

| what | detail |
|---|---|
| workspace setup | cargo workspace, edition 2024, workspace deps, clippy pedantic, `deny(unsafe_code)` |
| domain types | `Issue`, `BlockingEdge`, `Status`, `Priority`, `BodySections` (3 sections), `ReadyState` |
| graph engine | petgraph `DiGraph`, `build()`, `compute_ready_set()`, `unblock_cascade()`, `detect_cycles()`, `would_create_cycle()` |
| cache | `GraphCache` with ttl, invalidation, freshness check |
| config | env vars only (`GITHUB_TOKEN`, `GITHUB_API_URL`, `UNBLOCK_REPO`, `UNBLOCK_PROJECT`, etc.) |
| github client (`unblock-github`) | `reqwest` + graphql reads + rest mutations. `GitHubClient` with `api_base_url` for ghe support. shared crate reusable by mcp and desktop |
| github graphql | `fetch_graph_data()` — single paginated query returns all issues + blocking edges + projects v2 fields |
| github mutations | `create_issue()`, `close_issue()`, `reopen_issue()`, `update_issue()`, `add_comment()`, `add_blocked_by()`, `remove_blocked_by()` |
| github projects | `resolve_project()`, `setup_fields()`, `batch_update_fields()` |
| mcp server | rmcp stdio transport, `ServerState`, tool registration |
| core tools | `ready`, `claim`, `close`, `create`, `depends`, `dep_remove`, `show`, `prime` |
| errors | `snafu` domain errors + infrastructure errors with mcp error conversion |

**quality gate:** >80% coverage, clippy clean, fmt clean, `cargo doc` clean.

**output:** `v0.1.0` — agent can find ready work, claim, implement, close, see cascade.

---

## phase 2 — complete (mcp server)

**goal:** full 17-tool suite + claude code plugin.

| what | detail |
|---|---|
| remaining tools | `update`, `comment`, `list`, `search`, `stats`, `reopen`, `dep_cycles`, `setup`, `doctor` |
| body section parsing | `BodySections::from_markdown()` / `to_markdown()` for 3 sections |
| field validation at boot | verify 7 projects v2 fields exist with correct types/options |
| setup --migrate | add existing open issues to project (idempotent) |
| claude code plugin | `.mcp.json`, slash commands, workflow skill |

**output:** `v0.2.0` — complete tool suite, plugin installable.

---

## phase 3 — production (mcp server)

**goal:** hardened for real-world use. distributed.

| what | detail |
|---|---|
| circuit breaker | `CircuitBreaker` — 5 failures → open for 10s → half-open probe |
| retry | exponential backoff + jitter, max 3, only on 429/503 |
| observability | opentelemetry optional (`UNBLOCK_OTEL_ENDPOINT`), structured tracing json |
| distribution | cargo-dist: 5 targets (linux x86_64/arm64 musl, macos x86_64/arm64, windows x86_64). shell + powershell installers |
| npm wrapper | `@unblock/cli` — downloads platform binary on postinstall |
| v1.0.0 gap features | batch ops (`update`/`close`/`reopen`/`show`), `dep_tree` tool, date range filters, label OR filter |
| release flow | `cargo-release` → tag `unblock-mcp-v1.0.0` → cargo-dist auto-builds |

**quality gate:** 100% public api coverage, property tests (proptest), fuzz testing for parser.

**output:** `v1.0.0` — production-ready mcp server.

---

## v1.1.0 — post-launch enhancements

**goal:** features deferred from v1.0.0 that strengthen the product before the plugin phase.

| what | detail |
|---|---|
| homebrew tap | `websublime/homebrew-tap` formula, auto-updated by cargo-dist |
| `stale` tool | find issues with no updates for N days |
| `create_from_plan` tool | parse markdown document → N issues with dependencies |
| `merge` tool | consolidate duplicate issues |
| label list | list all labels in the repo |
| issue templates | parametrized templates for common issue patterns |
| jira awareness | `setup --jira` generates `jira-sync.yml` workflow. `create --jira-key` populates body convention. `doctor` verifies jira secrets when configured |

---

## phase 4 — plugin

**goal:** structured development pipeline for agents. review and qa in isolated ci sessions.

**depends on:** mcp server v1.0.0 shipped and validated.

| what | detail |
|---|---|
| plugin location | `plugin/` directory in monorepo, independent versioning (v0.x) |
| core agents (8) | grace (pm), ada (architect), fernando (po), sherlock (research), daphne (discovery), linus (reviewer), quinn (qa), martin (refactorer). all agents are `.md` configuration files (role, instructions, tool permissions), not compiled code |
| planning skills | `/product-requirements`, `/architect-solution`, `/create-tasks`, `/create-issue` |
| execution skills | `/start-task` (6 phases + self-check loop), `/rework-task`, `/review-task`, `/qa-task` |
| setup skills | `/setup-project` (fields + "review findings" milestone + supervisors + github actions + editor configs), `/add-supervisor`, `/update-plugin` |
| hooks | `SessionStart` (prime context + needs-rework alert), `PreToolUse` (discipline injection for supervisors), `PreCompact` (progress preservation) |
| github actions | `unblock-review.yml` (label trigger: needs-review), `unblock-qa.yml` (label trigger: approved) |
| session isolation | review and qa run in fresh ci sessions. zero implementation context. context from comment trail only |
| findings tracking | non-positive items from review/qa → issues under "review findings" milestone via fernando. dedup against existing issues |
| comment trail | INVESTIGATION, DECISION, DEVIATION, COMPLETED, REVIEW, REFACTORING, QA |
| copilot support | `.github/copilot-instructions.md` generated by setup. mcp tools identical. github actions identical |
| multi-editor | `.vscode/mcp.json`, `.cursor/rules/unblock.mdc`, `.windsurfrules` — all generated by setup |
| discipline | rule 0 (follow instructions exactly), rule 0.1 (read the issue first), self-check loop, log decisions/deviations, never close issues |

**output:** plugin installable via claude code marketplace. copilot/cursor/windsurf via mcp + custom instructions.

---

## phase 5 — desktop foundation

**goal:** basic desktop app with graph view + ready queue.

**depends on:** mcp server v1.0.0 validated. user feedback confirming demand.

| what | detail |
|---|---|
| shared crate | `unblock-github` already exists as shared crate (created in phase 1). desktop imports directly |
| gpui app bootstrap | window creation, custom theme, force-directed graph renderer |
| core views | graph view, ready queue panel, agent panel, toolbar, detail panel |

**output:** `v2.0-alpha`

---

## phase 6 — desktop complete

**goal:** full desktop app with crud, animations, polish, distribution.

| what | detail |
|---|---|
| remaining views | list view, create dialog |
| write operations | claim, close, create, update, depends |
| distribution | `.dmg` (macos), appimage (linux), homebrew cask |

**output:** `v2.0.0`

---

## phase 7 — monetisation

**goal:** sustainable business. details in internal docs (not public).

**depends on:** 12+ months community adoption.

**output:** revenue.

---

## how development happens

the project uses `://unblock` itself for task tracking (dogfooding) and the mister-anderson-derived plugin for the development pipeline.

```
1. docs define the work    → prd, architecture, project plan
2. issues track the work   → github issues with deps, milestones, projects v2 fields
3. agents execute the work → /start-task → implement → self-check → push
4. ci validates the work   → /review-task (clean session) → /qa-task (clean session)
5. human merges the work   → pr review → merge → close issue → cascade
```

### workspace commands

```bash
# dev
cargo fmt --check --all                    # format check
cargo clippy --workspace -- -D warnings    # lint
cargo test --workspace                     # test all
cargo test -p unblock-core                 # test core only
cargo build -p unblock-mcp                 # build mcp server

# release
cargo release -p unblock-mcp --execute 1.0.0   # mcp release → triggers cargo-dist

# run
GITHUB_TOKEN=ghp_... cargo run -p unblock-mcp   # run mcp server locally
```

### document index

```
docs/
├── unblock-prd-github.md               ← what to build (mcp)
├── unblock-architecture-github.md       ← how to build it (mcp)
├── unblock-project-plan.md              ← when to build it (mcp)
├── unblock-cicd-architecture.md         ← how to ship it (all products)
├── unblock-prd-plugin.md                ← what to build (plugin)
├── unblock-architecture-plugin.md       ← how to build it (plugin)
└── research/
    ├── beads-vs-unblock-comparison.md   ← competitive analysis
    └── unblock-jira-research.md         ← enterprise integration
```

desktop, business model, and monetisation docs are internal and not in the public repo.

---

## principles

these don't change.

| principle | meaning |
|---|---|
| github stores, rust computes | zero custom storage. github is the source of truth |
| every write invalidates and recomputes | consistency after mutations. cache is ephemeral |
| the agent is always one command away from productive work | `prime` → `ready` → `claim` in under 2 seconds |
| correct github primitive | each data type lives where it belongs. comments for work log, auto-links for references, fields for typed data, body for prose |
| session isolation for review/qa | reviewers must not remember implementation. clean sessions only |
| the pipeline is the product | tools without process are chainsaws without safety guards |
