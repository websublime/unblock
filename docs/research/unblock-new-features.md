# Unblock — New Feature Ideas

**Opportunities identified during architecture and product design sessions.**

| | |
|---|---|
| **Date** | March 2026 |
| **Status** | Exploration — none of these are committed |

---

## 1. `://impact` — priority by graph weight

Today priority is P0-P4, assigned manually by humans. The graph knows more — the real impact of closing any issue.

`what_if_close` already computes direct unblocking. Extend to recursive cascade depth: closing #42 unblocks #50, which unblocks #55 and #56, which unblock #60. Impact score of #42 = 4 (entire chain), not 1 (direct).

A tool `://impact` that shows the ready set ordered by cascade depth gives agents and tech leads an answer no PM can compute mentally: "this P2 has more real impact than that P0."

**Implementation:** Graph traversal from each ready node, counting total reachable blocked descendants. `petgraph` BFS/DFS. Purely computational — no API calls needed beyond cache.

**Value:** Agents pick higher-impact work first. Tech leads allocate developers to maximum-unblocking issues.

---

## 2. `://bottleneck` — where is the constraint

The inverse of impact. Which issue blocks the most progress in the project? The node with the highest weighted in-degree (direct + indirect dependents).

If #42 blocks 8 issues (directly and transitively) and #38 blocks 1, #42 is the bottleneck. The graph engine computes this trivially.

For a tech lead with 30 open issues and 3 developers, knowing "resolve #42 first and half the board unblocks" is the most valuable information possible.

**Implementation:** Reverse BFS from each open issue, count reachable blocked nodes. Sort by count descending. Could be a mode of `://stats` or standalone tool.

**Value:** Focus allocation. Stop spreading effort — concentrate on the constraint.

---

## 3. `://velocity` — learn from history

Each issue has `created_at`, `claimed_at` (Projects V2 field), and closed timestamp (GitHub native). The difference between claimed and closed = implementation time.

The MCP server could compute averages by supervisor, by priority, by story points. After 20 issues, the system knows: "P1 tasks with rust-supervisor take 45 minutes on average."

**Use cases:**
- Fernando suggests story points based on historical issues with similar scope
- Orchestrator predicts when an in-progress issue should be done (and alerts if overdue)
- Tech lead sees velocity trends: "this week was 40% faster than last week"
- Sprint planning with data-backed estimates instead of gut feeling

**Implementation:** Scan closed issues from GitHub API, compute statistics. No persistent storage needed — computed on demand. Could cache results with TTL.

**Value:** Data-driven estimation. Early warning for stuck work. Velocity tracking without external tools.

---

## 4. `://health` — project pulse check

Goes beyond `://stats` (which shows current snapshot). Health is trend analysis:

```
://health

pace:
  this week: 12 issues closed (↑ from 8 last week)
  avg cycle time: 52min (↓ from 1h10 last week)

warnings:
  3 issues blocked > 5 days with no progress (stale blockers)
  needs-rework ratio: 40% — agents failing acceptance criteria
    → specs may need more detail

agents:
  rust-supervisor: 8 issues, 38min avg, 10% rework rate
  node-supervisor: 4 issues, 1h12 avg, 50% rework rate
    → node-supervisor may need refinement

graph:
  2 cycles detected (see ://dep_cycles)
  longest chain: 6 issues deep (#42 → #50 → #55 → #56 → #60 → #65)
```

Not a dashboard — it's a text report consumable by agents and humans. Agents can act on it: "rework ratio is high, I should write more detailed acceptance criteria."

**Implementation:** GitHub API for timestamp data, graph engine for chain/cycle analysis. Pure computation, no storage.

**Value:** Project management intelligence without a PM tool. Self-diagnosing system.

---

## 5. `://commit-context` — enriched commit messages

Today: `git commit -m "implement rate limiter (#42)"`

With graph context:

```
implement rate limiter (#42)

refs: blocked by #38 (closed), blocks #50
decisions: token bucket over sliding window (see DECISION comment)
supervisor: rust-supervisor
story points: 3
```

The agent doesn't invent prose — the graph injects real context into git history. Months later, `git log` shows not just what changed, but why, and what it unblocked.

**Implementation:** Tool that reads issue data + comments and generates a structured commit message. The supervisor calls it before committing.

**Value:** Rich git history. Traceability from code back to decisions and dependencies. Useful for audits and onboarding.

---

## 6. `://plan-check` — spec drift detection

Automatic comparison of the branch diff against acceptance criteria. Runs in the self-check loop of `/start-task`, before push.

Detects:
- "acceptance criterion 3 has no corresponding file change in the diff" → missing implementation
- "file X was modified but is not covered by any criterion" → possible scope creep
- "acceptance criterion says 'implement retry logic' but no retry-related code found" → gap

**Implementation:** The agent already has LLM capability. The tool provides the structured input (diff + criteria list), the agent's own reasoning does the comparison. Could also be a dedicated sub-agent in the self-check loop.

**Value:** Reduces review findings. Catches scope drift before review, not during. Faster review cycles.

---

## 7. `://suggest-deps` — dependency suggestions

Agent creates #55 "implement caching layer". The MCP server analyzes the title and body against existing open issues and suggests: "#42 rate limiter touches the same middleware stack — should #55 depend on #42?"

Not auto-creation — suggestions. The agent or developer confirms.

**Implementation:** This requires semantic similarity between issue titles/bodies. Two approaches:
- **Lightweight:** keyword overlap + file path intersection (issues touching same files are likely related)
- **Full:** embedding similarity via LLM. Heavy for MCP server, but the agent can do this — `://suggest-deps #55` returns candidate issues, the agent's LLM decides

**Value:** Prevents orphaned dependencies. Catches missing blocking relationships before they cause conflicts.

---

## 8. GitHub Discussions as knowledge base

Issue comments are work log. But architectural decisions that affect multiple issues ("we chose token bucket over sliding window for ALL rate limiters") don't belong to a specific issue. They belong to the project.

GitHub Discussions is the correct primitive. A tool `://discuss` that creates or searches discussions by topic. DECISION comments in issues link to the discussion: "DECISION: token bucket — see discussion #7".

This creates a searchable knowledge base without leaving GitHub.

**Implementation:** GitHub Discussions API (GraphQL). Create, search, link. The `comment` tool could auto-detect architectural decisions and suggest creating a discussion.

**Value:** Institutional knowledge that outlives individual issues. Searchable architectural decision records (ADRs) as GitHub Discussions.

---

## 9. `://changelog` — automated release notes

When tagging a release, the MCP server reads all issues closed since the last tag, groups by milestone/epic, and generates changelog:

```markdown
## v1.1.0

### rate limiting (#42)
- implemented token bucket middleware
- 429 response with retry-after header

### error handling (#45, #46)
- snafu error types for domain + infrastructure
- integration tests for all error paths

### bug fixes
- #51 fix race condition in cache invalidation
```

Extracted from COMPLETED comments and titles. Not the agent inventing prose — the graph aggregating what was done.

**Implementation:** `git tag` list → find previous tag → list issues closed between tags (GitHub API `since` parameter or search `closed:>date`) → read COMPLETED comments → group by milestone → format.

**Value:** Release notes that write themselves. Accurate, traceable, consistent. No more "what did we ship?"

---

## 10. Cross-repo awareness

Listed in PRD as future, but the most interesting angle isn't "multi-repo dashboard" — it's **cross-repo blocking**.

GitHub natively supports blocking relationships between repos. If `websublime/unblock-core#42` blocks `websublime/unblock-mcp#15`, the graph should reflect this.

Today the MCP server operates on one repo. Expanding to N repos means:
- Multiple `fetch_graph_data()` calls (one per repo)
- Merge graphs into a unified `DependencyGraph`
- Compute ready set across all repos
- `://ready` returns issues from any repo
- `://impact` and `://bottleneck` operate on the global graph

**Implementation:** Configuration accepts multiple repos or an org-wide scope. GraphQL queries fan out. Graph merge is additive — same `petgraph` structure, nodes get repo prefix (`core#42`, `mcp#15`).

**Value:** For orgs with microservices in separate repos, this is the killer feature. A cross-repo dependency graph that no tool provides today. The tech lead sees "service A's deploy is blocked by service B's auth module" — across repos, computed from real blocking relationships.

---

## Priority Assessment

| # | Feature | Effort | Graph engine needed | API calls | When |
|---|---|---|---|---|---|
| 1 | `://impact` | Low | Yes (BFS traversal) | 0 (cache) | Could ship in v1 |
| 2 | `://bottleneck` | Low | Yes (reverse BFS) | 0 (cache) | Could ship in v1 |
| 3 | `://velocity` | Medium | No | Scan closed issues | Post v1 |
| 4 | `://health` | Medium | Partial | Scan + compute | Post v1 |
| 5 | `://commit-context` | Low | Read-only | 1 (issue read) | Plugin feature |
| 6 | `://plan-check` | Medium | No | Agent LLM reasoning | Plugin feature |
| 7 | `://suggest-deps` | Medium | Partial | Keyword/file analysis | Post v1 |
| 8 | `://discuss` | Medium | No | Discussions API | Post v1 |
| 9 | `://changelog` | Low | No | Git tags + issues | Post v1 |
| 10 | Cross-repo | High | Yes (multi-graph merge) | N × fetch | Enterprise |

Features 1 and 2 are the most aligned with the core value proposition ("dependency-aware") and require the least effort — the graph engine already has the data, they just expose new traversals. They could be tools 18 and 19, shipping alongside the initial 17.
