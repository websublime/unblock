# **`://`**`unblock`

**dependency-aware task tracking for ai agents, powered by github.**

[![license: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-A78BFA.svg)](LICENSE-MIT)
[![rust](https://img.shields.io/badge/rust-edition%202024-A78BFA.svg)](https://www.rust-lang.org)

---

an mcp server that turns github issues into a dependency graph.
agents ask `://ready` and get the answer to the only question that matters:
**"what can i work on right now?"**

```
agent: ://ready
unblock: #42 implement rate limiter [P1, task, ready]
         #45 add error types       [P2, task, ready]
         #46 write integration tests [P2, task, ready]

agent: ://claim #42
unblock: claimed. status → in_progress, agent → claude-alpha

agent: ://close #42
unblock: closed. cascade: #50 unblocked → ready
```

---

## the problem

ai agents can write code. they can't manage work.

without a dependency graph, an agent will happily start coding a task
that's blocked by three others. without a ready queue, it picks work
at random. without claim semantics, two agents work on the same issue.
without cascade, closing a task doesn't unblock anything.

`://unblock` fixes all of this. github stores the data,
rust computes the graph, agents get the answers.

---

## how it works

```
┌──────────────┐   mcp/stdio   ┌──────────────┐   https   ┌──────────────┐
│  claude code │◄─────────────►│  ://unblock   │◄─────────►│  github api  │
│  copilot     │               │  (rust binary)│           │  graphql+rest│
│  cursor      │               │               │           │              │
│  any mcp     │               │  petgraph     │           │  issues      │
│  client      │               │  graph engine │           │  projects v2 │
└──────────────┘               └──────────────┘           └──────────────┘
```

- **github** stores issues, projects v2 fields, blocking relationships, comments
- **://unblock** builds a dependency graph in memory, computes the ready set, exposes 17 mcp tools
- **agents** interact via mcp protocol (stdio). humans see the same data in github ui
- **zero custom storage** — the mcp server computes, github stores

---

## install

```bash
# homebrew (macos / linux)
brew install websublime/tap/unblock-mcp

# shell installer
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/websublime/unblock/releases/latest/download/unblock-mcp-installer.sh | sh

# npm
npx @unblock/cli

# cargo (requires rust toolchain)
cargo install unblock-mcp
```

### configure

the only required config is a github token:

```bash
export GITHUB_TOKEN=ghp_...
```

add to your mcp client config:

```json
{
  "mcpServers": {
    "unblock": {
      "command": "unblock-mcp",
      "env": {
        "GITHUB_TOKEN": "${GITHUB_TOKEN}"
      }
    }
  }
}
```

repo is auto-detected from git remote. project is auto-detected from linked projects v2.
zero config for the common case.

### github enterprise

```json
{
  "mcpServers": {
    "unblock": {
      "command": "unblock-mcp",
      "env": {
        "GITHUB_TOKEN": "${GITHUB_TOKEN}",
        "GITHUB_API_URL": "https://ghe.corp.com/api/v3"
      }
    }
  }
}
```

### first run

```bash
# bootstrap projects v2 fields (run once per repo)
# creates: Status, Priority, Agent, Claimed At, Ready State, Story Points, Defer Until
://setup
```

---

## tools

17 tools. each operates on the current repo.

### core workflow

| tool | purpose |
|---|---|
| `://ready` | find issues with no active blockers — the ready queue |
| `://claim` | take ownership — sets agent, status, timestamp atomically |
| `://close` | complete + cascade — unblocks dependents automatically |
| `://create` | new issue with deps, priority, labels, parent |
| `://prime` | session context — inject repo state into agent context |

### dependency graph

| tool | purpose |
|---|---|
| `://depends` | declare a blocking relationship |
| `://dep_remove` | remove a blocking relationship |
| `://dep_cycles` | detect circular dependencies |
| `://show` | full issue detail with deps, comments, parsed body |

### management

| tool | purpose |
|---|---|
| `://update` | edit fields — priority, status, labels, body sections, assignees |
| `://comment` | add structured comment to issue |
| `://list` | filtered issue list with sorting and pagination |
| `://search` | full-text search via github search api |
| `://reopen` | reopen a closed issue, recompute blocking status |
| `://stats` | repo stats — by status, priority, blocked/ready counts, agents |
| `://setup` | create projects v2 fields, migrate existing issues |
| `://doctor` | health check — fields, project, graph consistency, rate limits |

---

## the pipeline

`://unblock` powers a structured development pipeline via the **unblock plugin** for claude code.

```
://product-requirements → ://architect-solution → ://create-tasks
        │
        ▼
://start-task #42          local: investigate → implement → self-check → push
        │
        │ label: needs-review
        ▼
[github action]            ci: code review in clean session (zero implementation context)
        │
        ├── approve → [github action] ci: qa validation in clean session
        │                  ├── pass → developer merges
        │                  └── fail → ://rework-task
        │
        ├── needs-refactoring → fix + re-review (same session)
        └── needs-rework → ://rework-task (local) → loop
```

review and qa run in **isolated ci sessions** via github actions. the reviewer has zero memory of the implementation. context comes exclusively from the comment trail on the issue.

---

## comment trail

every issue accumulates structured comments. any agent or human can reconstruct full context.

```
INVESTIGATION:  what was found in the codebase before implementation
DECISION:       non-trivial implementation choices and reasoning
DEVIATION:      where implementation differs from spec and why
COMPLETED:      summary, files changed, test results
REVIEW:         findings with severities, verdict
REFACTORING:    what was fixed, skipped, deferred
QA:             spec conformity, test/build/lint, verdict
```

---

## environment variables

| variable | required | default | notes |
|---|---|---|---|
| `GITHUB_TOKEN` | yes | — | pat or github app token |
| `GITHUB_API_URL` | no | `https://api.github.com` | ghe: `https://<host>/api/v3` |
| `UNBLOCK_REPO` | no | auto-detect | `owner/repo` format |
| `UNBLOCK_PROJECT` | no | auto-detect | project number |
| `UNBLOCK_AGENT` | no | `agent` | default agent name |
| `UNBLOCK_CACHE_TTL` | no | `30` | cache ttl seconds |
| `UNBLOCK_LOG_LEVEL` | no | `info` | log level |

---

## architecture

```
websublime/unblock/
├── crates/
│   ├── unblock-core/       types, graph engine, cache (zero network)
│   ├── unblock-github/     github api client (shared)
│   └── unblock-mcp/        mcp server binary
```

- **unblock-core** — pure rust. petgraph dependency graph, ready set computation, cascade, cycle detection. fully testable without network
- **unblock-github** — reqwest + graphql/rest. github api client shared across products
- **unblock-mcp** — rmcp server, 17 tool handlers, stdio transport

---

## desktop

the human's window into what agents see.

force-directed dependency graph. nodes coloured by status. ready queue panel.
agent activity cards. full crud. keyboard-driven.

coming soon.

---

## docs

| document | purpose |
|---|---|
| [`unblock-prd-github.md`](docs/unblock-prd-github.md) | product requirements — mcp server |
| [`unblock-architecture-github.md`](docs/unblock-architecture-github.md) | architecture specification — mcp server |
| [`unblock-project-plan.md`](docs/unblock-project-plan.md) | project plan — mcp server |
| [`unblock-cicd-architecture.md`](docs/unblock-cicd-architecture.md) | ci/cd — builds, releases, distribution |
| [`unblock-prd-plugin.md`](docs/unblock-prd-plugin.md) | product requirements — plugin (cc + copilot) |
| [`unblock-jira-research.md`](docs/research/unblock-jira-research.md) | jira integration research |
| [`beads-vs-unblock-comparison.md`](docs/research/beads-vs-unblock-comparison.md) | feature comparison with beads cli |
| [`unblock-brand-guide.md`](branding/unblock-brand-guide.md) | brand guide — identity, colours, typography |
| [`DEV-ROADMAP.md`](DEV-ROADMAP.md) | development roadmap |

---

## license

licensed under either of

- [MIT license](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

at your option.

### contribution

unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.

---

<p align="center">
  <code><b>://</b>unblock</code> — a <a href="https://github.com/websublime">websublime</a> product
</p>
