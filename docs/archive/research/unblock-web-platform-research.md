# Unblock Web Platform — Research

**Exploring the SaaS evolution: from local MCP server to real-time web platform.**

| | |
|---|---|
| **Date** | March 2026 |
| **Status** | Exploration — nothing committed |
| **Context** | These ideas emerged from exploring GitHub Apps, webhook relay, and the limitations of a desktop-only approach |

---

## 1. Platform Concept

A GitHub App + hosted backend + web frontend that provides real-time dependency intelligence on top of GitHub Issues. The MCP server stays local and free. The web platform is the Pro/Enterprise product.

### 1.1 Architecture

```
GitHub (webhooks — real-time events)
     │
     ▼
GitHub App (receives: issue created/closed/labeled, PR events, comments)
     │
     ▼
Backend (Rust — runs unblock-core graph engine)
     │
     ├── Computes graph in real-time on each webhook
     ├── Persists state (in-memory or Redis for scale)
     ├── Serves REST/GraphQL API for the frontend
     │
     └── WebSocket fan-out
          │
          ▼
     Web App (browser)
          │
          ├── Force-directed graph (d3.js / canvas)
          ├── Ready queue
          ├── Team activity (real-time updates)
          ├── Pipeline board
          ├── Impact / bottleneck visualization
          └── Notifications (push, Slack, email)
```

### 1.2 Why Web Instead of Desktop

| Aspect | Desktop (GPUI) | Web |
|---|---|---|
| Install | Binary download, unsigned macOS | Open browser |
| Platforms | macOS, Linux (no Windows) | Everything with a browser |
| Updates | Manual download or Homebrew | Instant, server-side |
| Real-time | Poll-based (TTL cache) | WebSocket push from webhooks |
| Team | Single user, local cache | Multi-user, shared state |
| Audience | Developers only | Developers + tech leads + PMs + stakeholders |
| Auth | GitHub PAT | GitHub OAuth (login with GitHub) |
| Billing | License key validation | Subscription via Stripe |

### 1.3 Core Differentiator

Not "another project management tool." The dependency graph is the product, not an add-on. Linear has deps but they're flat links — no ready set computation, no cascade, no bottleneck detection. Jira deps are a third-party plugin. In Unblock Web, the graph IS the interface.

**Amplifies GitHub, doesn't replace it.** Issues stay in GitHub. PRs stay in GitHub. Code review stays in GitHub. Unblock Web is the intelligence layer on top. If Unblock Web disappears, the developer loses visualization but not data. Zero lock-in.

---

## 2. Innovative Features

### 2.1 Live Graph Replay

The graph has history. Every issue created, every dep added, every close + cascade — all have timestamps. A temporal slider that replays project evolution.

**Interface:** Timeline slider at the bottom of the graph view. Drag to a date — the graph morphs to that point in time. Press play — watch 2 weeks of progress in 30 seconds. Issues appear, deps form, nodes turn green when closed, cascades propagate visually.

**Use cases:**
- Sprint review: "this is what we accomplished this sprint" — stakeholders SEE progress happening
- Post-mortem: "when did the bottleneck form? When did we notice?" — scrub to the moment
- Onboarding: new team member watches the project's history to understand how it evolved

**Implementation:** GitHub issue events have timestamps. Git tags mark releases. The backend stores snapshots or computes from event log. The frontend interpolates between states for smooth animation.

**Who does this:** Nobody. No PM tool has temporal graph replay.

---

### 2.2 AI Planning from Natural Language

The tech lead types in the web app chat: "we need OAuth2 authentication with refresh tokens, support for Google and GitHub providers, and a settings page for users to manage connections."

The app uses LLM (via API) to: decompose into issues, suggest deps between them, estimate story points from velocity history, assign supervisors from detected tech stack, and show the resulting graph BEFORE creating anything.

**Interface:** Chat panel on the side. Type the idea. The graph materializes in preview mode (ghost nodes, dashed edges). The tech lead adjusts visually — drag deps, change priorities, remove issues, split or merge. Click "create" — issues materialize in GitHub.

**What makes it different from ChatGPT + manual creation:** The graph is the feedback loop. The tech lead sees the dependency structure before committing. "Wait, these two tasks don't actually depend on each other — remove that edge." Visual planning, not text planning.

**Implementation:** LLM call with structured output (JSON: issues, deps, priorities). Graph preview rendered from JSON. On confirm, batch `create` + `depends` via Unblock MCP or GitHub API directly.

---

### 2.3 Conflict Prediction

The system knows which files each agent is touching (from branch diffs via GitHub API). It knows which issues are being worked in parallel. It can predict merge conflicts before they happen.

**Scenario:** Issues #42 and #45 are both `in_progress`. Agent A's branch for #42 modifies `src/middleware/auth.rs`. Agent B's branch for #45 also modifies `src/middleware/auth.rs`. The system alerts: "conflict likely — both branches touch auth.rs."

**Goes further:** Analyzes historical file conflict frequency. Files that historically generate merge conflicts are flagged preemptively when two agents are assigned to issues that touch them — before anyone starts working, not after.

**Interface:** In the graph view, edges between in-progress nodes glow red when conflict is predicted. Click the edge to see which files overlap. The notification says: "consider sequentializing #42 and #45, or splitting auth.rs changes."

**Implementation:** GitHub API: compare branch diffs (`/repos/{owner}/{repo}/compare/{base}...{head}`). File path intersection. Historical conflict data from merged PRs with conflict markers.

---

### 2.4 Agent Performance Analytics

Granular analytics per agent and supervisor, not generic velocity.

**Metrics:**
- **Rework rate by supervisor** — if `node-supervisor` has 50% rework rate, it needs refinement
- **Cycle time distribution** — histogram, not average (averages lie). P50, P90, P99
- **Review findings heatmap** — files that accumulate the most findings = concentrated tech debt
- **Cascade efficiency** — what percentage of closes generate cascades. Low = graph poorly structured
- **First-pass success rate** — issues that pass review without rework on first attempt
- **Story point accuracy** — estimated vs actual time. Are estimates improving over time?

**Interface:** Dashboard with charts. Filter by supervisor, time range, milestone. Compare supervisors side by side. Drill down into specific issues that contributed to outlier metrics.

**Use cases:**
- "node-supervisor has 50% rework — let's refine its system prompt"
- "P1 issues take 2x longer than estimated — we're underestimating complexity"
- "auth.rs appears in 60% of review findings — this module needs refactoring"

**Implementation:** All data exists in GitHub (issue timestamps, comments with structured types, labels). Backend aggregates and computes statistics on demand or cached.

---

### 2.5 Dependency Health Score

The graph can be healthy or diseased. Metrics that nobody computes today:

| Metric | Healthy | Unhealthy | What it means |
|---|---|---|---|
| Average chain depth | 2-3 | 6+ | Deep chains = project is too sequential |
| Blocked/ready ratio | <30% blocked | >50% blocked | Systemic bottleneck |
| Orphan rate | <10% | >40% | Issues without deps — isolated by design or forgotten? |
| Fan-out ratio | Balanced | 1 issue blocks 10+ | Risk concentration — single point of failure |
| Cycle count | 0 | 2+ | Graph integrity issue |
| Stale blocker age | <3 days | >7 days | Blockers not being addressed |

**Score:** 0-100 composite with automated recommendations:

```
dependency health: 62/100

⚠ chain depth 7 — consider parallelizing by splitting sequential deps
⚠ #42 blocks 8 issues — high fan-out concentration risk
✓ zero cycles
✓ blocked/ready ratio 28% — healthy
⚠ 3 stale blockers >5 days — address #38, #40, #41
```

**Implementation:** Pure graph analysis. All computable from `unblock-core` graph engine. No API calls beyond initial data fetch.

---

### 2.6 Smart Notifications

Not dumb event notifications ("issue #42 was closed"). Dependency-aware, personalized, actionable.

**Examples:**

| Event | Dumb notification | Smart notification |
|---|---|---|
| Issue closed | "#42 was closed" | "#42 was closed by dev A. YOUR issue #50 is now unblocked — you can start working. Impact: closing #50 unblocks 2 more." |
| Bottleneck forming | (none) | "#42 has been in_progress for 4h (3x average). It blocks 5 issues. Consider helping dev A or reassigning." |
| Sprint progress | (none) | "7 of 12 sprint issues closed. Current pace: on track. Remaining bottleneck: #42 (blocks 3)." |
| Cascade | "#50 status changed" | "Dev B closed #38 → cascade: #50, #51, #52 are now ready. Highest impact: #50 (P1, unblocks 3). Recommendation: pick #50 next." |

**Channels:** Browser push, Slack bot, email digest (daily/weekly). Configurable per user.

**Implementation:** Webhook events → compute impact → route to configured channels. The intelligence is in the graph analysis, not the notification infrastructure.

---

### 2.7 What-If Simulator

Interactive in the browser. "If I close #42 and #45, what happens to the project?"

**Interface:** Drag issues to a "simulate close" zone. The graph recalculates in real-time. Ready count changes, nodes pulse green, impact score updates. The tech lead uses this for sprint planning: "if we prioritize these 3 issues, we unblock 12 in the next sprint."

**Extend to dependency editing:** Drag an edge between two nodes — "if I add a dep from #50 to #42, does it create a cycle?" Instant validation. Visual graph design.

**Extend to "what if we add a developer":** Simulate increasing parallel capacity from 2 to 3 agents. Based on velocity data, show estimated sprint completion difference.

**Implementation:** Client-side graph copy. All simulation runs in the browser using the same graph algorithms as `unblock-core` (compiled to WASM, or reimplemented in JS). Zero API calls during simulation.

---

### 2.8 Cross-Team Dependency Map

An org with 5 teams, each with their own repo. The web dashboard aggregates into ONE graph with clusters per team.

**Interface:** Zoom levels. Zoomed out: team clusters connected by cross-repo deps. Click a cluster to zoom into that team's graph. Cross-repo edges are highlighted differently (dashed, colored by source team).

**The CTO view:** "Auth team is blocking 3 downstream teams. Platform team has zero external blockers — they're self-contained. Mobile team is waiting on API team for 4 issues."

**Implementation:** GitHub Apps can be installed org-wide. The backend fetches data from all repos in the org. Graph merge: nodes get repo prefix (`core#42`, `mcp#15`). Cross-repo blocking edges come from GitHub's native cross-repo blocking API.

**Nobody does this with real data.** Every org-level dependency view today is a manually maintained spreadsheet or a Miro board. Unblock has the actual blocking relationships from the actual code work.

---

### 2.9 Spec-to-Graph AI

Upload a document — PRD, RFC, design doc, meeting notes, even a screenshot of a whiteboard. The app analyzes with LLM and suggests a complete issue graph.

**Interface:** Upload zone (drag file or paste text). Processing animation. Graph appears in preview mode. Same adjust-then-create flow as 2.2.

**Different from 2.2 (natural language planning):** This accepts entire documents, not conversational input. A 10-page PRD goes in, a 30-issue graph comes out. The LLM extracts requirements, identifies implicit dependencies ("auth must exist before user management"), estimates complexity from requirement density, and suggests milestone groupings.

**Goes further:** If the project has existing issues, the LLM cross-references: "requirement 5 in this PRD is already covered by issue #42 — linking instead of creating duplicate."

**Implementation:** LLM call with document context. Structured output: issues with titles, descriptions, acceptance criteria, deps, priorities, milestones. Preview graph. On confirm, batch create via GitHub API.

---

### 2.10 Anomaly Detection

The system learns patterns from historical data and flags deviations proactively.

**Pattern: time anomalies**

"Issues of this type with this supervisor take 45 min on average." When an issue is `in_progress` for 3 hours: "This issue is 4x above average. Possible causes: scope creep, ambiguous spec, hidden deps."

**Pattern: rework trends**

"The last 5 issues from node-supervisor had rework." The system doesn't just flag — it suggests: "Consider refining the node-supervisor prompt, or review recent specs for this module — something changed."

**Pattern: estimation drift**

"Story point estimates for P1 issues are consistently 40% under actual time." Suggests: "Increase P1 estimates by 1.4x or split P1 issues into smaller units."

**Pattern: dependency structure anomalies**

"Issue #55 was created with zero deps but touches 3 files that are also touched by #42 and #45." Suggests: "Missing dependency? Consider whether #55 should be blocked by #42."

**Implementation:** Statistical analysis over issue history. Moving averages, standard deviation thresholds, file-overlap analysis. No ML needed — simple statistics on structured data.

---

### 2.11 Supervisor Marketplace

The community creates and shares supervisors. A curated library of supervisor agents for specific tech stacks.

**Examples:**
- `rust-axum-supervisor` — specialized for Axum web framework patterns
- `react-nextjs-supervisor` — Next.js App Router conventions
- `terraform-supervisor` — infrastructure-as-code with plan/apply workflow
- `flutter-supervisor` — cross-platform mobile with platform-specific testing
- `solidity-supervisor` — smart contract development with security patterns

**Interface:** Browse supervisors by technology, rating, velocity metrics from the community. "This supervisor has been used on 200 projects, average rework rate 12%, average cycle time 35 min." One-click install to project.

**Monetization angle:** Free community supervisors + premium enterprise supervisors (SAP, Salesforce, Oracle integrations). Supervisor authors can publish and optionally charge.

**Implementation:** Supervisor files are markdown — they're just agent system prompts. The marketplace is a registry with metadata (tech stack, metrics, ratings). Install = copy file to `.claude/agents/` or equivalent. Metrics aggregated from projects that opt in to telemetry.

---

### 2.12 Dependency-Aware Code Ownership

Cross-reference the dependency graph with git blame. Not "who committed most" — "who knows most about this area of the project."

**Knowledge score per developer per module:**
- Commits to files in this module (git blame)
- Issues worked that touch this module (from COMPLETED comments listing files)
- INVESTIGATION comments analyzing this module
- DECISION comments accepted by reviewer for this module
- Review findings resolved in this module

**Use cases:**
- Developer leaves the team → system shows which modules lose coverage: "auth module drops from 3 knowledgeable developers to 1. Risk: single point of knowledge."
- New developer joins → system suggests onboarding path: "start with module X (most documentation, lowest complexity), then module Y (builds on X)."
- Code review assignment → "developer C has the highest knowledge score for this module — suggest as reviewer."

**Interface:** Heatmap overlay on the graph view. Colour intensity = team knowledge coverage. Click a node → see which developers have context. Red zones = single-developer knowledge. Green = well-distributed.

**Implementation:** Git blame API + issue/comment cross-reference. The data exists in GitHub — nobody aggregates it this way.

---

## 3. Priority Assessment

| # | Feature | Complexity | Uniqueness | Value |
|---|---|---|---|---|
| 2.1 | Live graph replay | Medium | Very high — nobody does this | Sprint reviews, onboarding, post-mortems |
| 2.2 | AI planning from NL | Medium | High — visual graph feedback unique | Fastest path from idea to structured work |
| 2.3 | Conflict prediction | Medium | High | Prevents wasted work in parallel teams |
| 2.4 | Agent analytics | Low-Medium | Medium — analytics exist, agent-specific don't | Supervisor refinement, team performance |
| 2.5 | Dep health score | Low | High — nobody computes graph health | Project coaching, structural improvement |
| 2.6 | Smart notifications | Medium | High — dep-aware notifications are new | Right info to right person at right time |
| 2.7 | What-if simulator | Medium | Very high — interactive graph simulation | Sprint planning, dep design |
| 2.8 | Cross-team dep map | High | Very high — nobody does this with real data | Org-level bottleneck detection |
| 2.9 | Spec-to-graph AI | Medium | High — document → graph is novel | Zero-friction project setup |
| 2.10 | Anomaly detection | Medium | Medium-High | Proactive problem identification |
| 2.11 | Supervisor marketplace | High (ecosystem) | Very high — no precedent | Community, monetization, adoption |
| 2.12 | Code ownership | Medium | High — aggregation is unique | Knowledge management, risk mitigation |

---

## 4. What This Means for the Desktop

If the web platform materializes, the desktop app (GPUI) becomes redundant for most users. The web provides:
- Same graph view (d3.js instead of GPUI — different renderer, same visualization)
- Real-time instead of poll-based (WebSocket vs TTL cache)
- Multi-user instead of single-user
- Zero install instead of binary download
- Features that require a backend (analytics, anomaly detection, cross-team) that a local binary can't provide

The desktop could survive as an "offline mode" or "local-first" option for developers who want zero cloud dependency. But the primary paid product shifts from desktop to web.

This decision doesn't need to be made now. The MCP server is the priority. The web platform is a future direction that could replace or complement the desktop plan.

---

## 5. Open Questions

- Hosting: where does the backend run? Cloudflare Workers, Fly.io, Railway, self-hosted?
- Auth: GitHub OAuth is the obvious choice. How to handle org-level permissions?
- Data: does the backend persist issue data or always read from GitHub? Caching strategy?
- Pricing: does this change the per-seat model? Usage-based (per webhook) vs flat rate?
- Graph rendering: d3.js (SVG, CPU) vs WebGL (GPU, handles large graphs) vs WASM (port unblock-core)?
- Open source: is the web backend open source (self-hostable) or closed source (SaaS only)?
- Competition: Linear is investing heavily in AI. GitHub Projects is improving. Window of opportunity?
