# Unblock — Jira Integration Research

**How Unblock coexists with Jira in enterprise environments.**

| | |
|---|---|
| **Version** | 0.1.0-draft |
| **Author** | Miguel Ramos |
| **Org** | websublime |
| **Date** | March 2026 |
| **Status** | Research |
| **Priority** | Future — no code changes required |

---

## 1. Problem

Enterprise teams use Jira for sprint planning, ceremonies, stakeholder reporting, and velocity tracking. AI agents need Unblock for dependency-aware task execution. Both systems must coexist without replacing each other.

**Constraint:** Bidirectional sync (Jira ↔ GitHub) is a known hard problem. Tools like Exalate and Unito exist for this and are still fragile — field mapping conflicts, edge cases, eventual consistency failures. We avoid this entirely.

---

## 2. Principle

**Jira plans, GitHub executes, agents compute.**

| Layer | Tool | Owns |
|---|---|---|
| Planning | Jira | Epics, stories, sprint ceremonies, stakeholder reporting, velocity |
| Execution | GitHub Issues + Unblock | Tasks, dependencies, ready queue, agent workflow |
| Bridge | GitHub Actions + conventions | Handoff, status sync, traceability |

No bidirectional sync. Unidirectional event-driven notifications at two points: handoff (Jira → GitHub) and completion (GitHub → Jira).

---

## 3. Integration Points

### 3.1 Handoff: Jira → GitHub Issues

Sprint planning produces a set of stories. The tech lead (or an agent) creates corresponding GitHub Issues with Jira references.

**Convention — Jira reference in issue body:**

```markdown
## Description
Implement OAuth2 flow for third-party integrations.

## Jira
PROJ-1234
```

**Convention — Jira reference as label:**

```
jira:PROJ-1234
```

Both provide traceability. The body section is richer (clickable in GitHub if Jira URL is included), the label is queryable via `list --label jira:PROJ-1234`.

**Manual handoff:** Tech lead reads sprint board, creates GitHub Issues via Unblock `create` with appropriate deps, priorities, and Jira references. This is the simplest path and works today with zero tooling changes.

**Agent-assisted handoff:** The agent has access to both Jira MCP and Unblock MCP. The lead says "create tasks for sprint 42". The agent reads the sprint from Jira, interprets stories, and calls Unblock `create` N times with deps. This is the "create from plan" workflow pattern — the plan is the Jira sprint.

**Automated handoff (Jira Automation):** Jira Automation rule triggers on "story moved to sprint" → sends webhook → GitHub Action creates issue via REST API. More complex to set up, less flexible than agent-assisted.

### 3.2 Progress: GitHub → Jira

When an agent closes a GitHub Issue, the linked Jira ticket should reflect completion.

**GitHub Action — event-driven, unidirectional:**

```yaml
# .github/workflows/jira-sync.yml
name: Sync to Jira
on:
  issues:
    types: [closed, reopened]

jobs:
  sync:
    runs-on: ubuntu-latest
    if: contains(github.event.issue.body, '## Jira')
    steps:
      - name: Extract Jira key
        id: jira
        run: |
          KEY=$(echo "${{ github.event.issue.body }}" | grep -oP '[A-Z]+-\d+' | head -1)
          echo "key=$KEY" >> $GITHUB_OUTPUT

      - name: Transition Jira ticket
        if: steps.jira.outputs.key != ''
        uses: atlassian/gajira-transition@v3
        with:
          issue: ${{ steps.jira.outputs.key }}
          transition: ${{ github.event.action == 'closed' && 'Done' || 'To Do' }}
        env:
          JIRA_BASE_URL: ${{ secrets.JIRA_URL }}
          JIRA_USER_EMAIL: ${{ secrets.JIRA_EMAIL }}
          JIRA_API_TOKEN: ${{ secrets.JIRA_TOKEN }}

      - name: Add comment to Jira
        if: steps.jira.outputs.key != '' && github.event.action == 'closed'
        uses: atlassian/gajira-comment@v3
        with:
          issue: ${{ steps.jira.outputs.key }}
          comment: |
            Completed via GitHub Issue #${{ github.event.issue.number }}.
            ${{ github.event.issue.html_url }}
        env:
          JIRA_BASE_URL: ${{ secrets.JIRA_URL }}
          JIRA_USER_EMAIL: ${{ secrets.JIRA_EMAIL }}
          JIRA_API_TOKEN: ${{ secrets.JIRA_TOKEN }}
```

**What this covers:**
- GitHub Issue closed → Jira ticket transitions to "Done"
- GitHub Issue reopened → Jira ticket transitions back to "To Do"
- Closing comment with link to GitHub Issue for audit trail

**What this does NOT cover:**
- In-progress status sync (agent claims issue → Jira moves to "In Progress"). Possible with `issues.labeled` event on `status:in_progress` label, but adds complexity. Evaluate if stakeholders actually need real-time status.

### 3.3 Context: Jira → Agent (read-only via MCP)

The agent may need business context that lives in Jira — acceptance criteria, design decisions, stakeholder notes — that wasn't copied to the GitHub Issue body.

**Solution:** Jira MCP server alongside Unblock MCP. The agent reads from both.

```json
{
  "mcpServers": {
    "unblock": {
      "command": "unblock-mcp",
      "env": {
        "GITHUB_TOKEN": "${GITHUB_TOKEN}"
      }
    },
    "jira": {
      "command": "mcp-atlassian",
      "env": {
        "JIRA_URL": "https://company.atlassian.net",
        "JIRA_TOKEN": "${JIRA_TOKEN}"
      }
    }
  }
}
```

Agent workflow:

1. `ready` → sees "Issue #42 — Implement OAuth2 (jira:PROJ-1234)"
2. `claim #42`
3. Reads PROJ-1234 via Jira MCP for acceptance criteria
4. Works on the task with full business context
5. `close #42` → GitHub Action syncs to Jira

No data duplication. Each system holds what it's good at.

---

## 4. Data Flow Diagram

```
┌─────────────────────────────────────────────────────────┐
│                    JIRA (Planning)                        │
│                                                          │
│  Sprint Board → Stories selected → Acceptance criteria   │
│                                                          │
└──────────┬───────────────────────────────────▲───────────┘
           │                                   │
           │ Handoff                           │ Status sync
           │ (manual / agent / automation)     │ (GitHub Action)
           │                                   │
           ▼                                   │
┌──────────────────────────────────────────────┴───────────┐
│                 GITHUB ISSUES (Execution)                  │
│                                                           │
│  Issues with deps + Jira refs                             │
│  Projects V2 fields (status, priority, agent)             │
│                                                           │
└──────────┬───────────────────────────────────▲───────────┘
           │                                   │
           │ MCP (stdio)                       │ Writes
           │                                   │
           ▼                                   │
┌──────────────────────────────────────────────┴───────────┐
│              UNBLOCK MCP SERVER (Compute)                  │
│                                                           │
│  Graph engine → ready queue → claim → close → cascade     │
│                                                           │
└──────────┬───────────────────────────────────▲───────────┘
           │                                   │
           │ MCP (stdio)                       │ Tool calls
           │                                   │
           ▼                                   │
┌──────────────────────────────────────────────────────────┐
│                    AI AGENT                                │
│                                                           │
│  Claude Code / Copilot / Codex / Aider                   │
│  + Jira MCP for context reads                             │
│                                                           │
└──────────────────────────────────────────────────────────┘
```

---

## 5. Impact on Unblock

**Zero code changes required.** The integration is entirely external:

| Component | Change needed |
|---|---|
| `unblock-core` | None |
| `unblock-github` | None |
| `unblock-mcp` | None |
| `unblock-app` | None |
| GitHub Actions | New workflow `jira-sync.yml` (per-repo, optional) |
| MCP config | Add `mcp-atlassian` alongside `unblock` (per-team) |
| Conventions | Document Jira reference pattern in body/labels |

---

## 6. Secrets Required (per-repo, optional)

| Secret | Purpose |
|---|---|
| `JIRA_URL` | Jira instance base URL (e.g. `https://company.atlassian.net`) |
| `JIRA_EMAIL` | Jira user email for API auth |
| `JIRA_TOKEN` | Jira API token |

---

## 7. Future Considerations

| Idea | Complexity | Value | Notes |
|---|---|---|---|
| In-progress sync (claim → Jira transition) | Low | Medium | `issues.labeled` event on status change. Only if stakeholders need real-time |
| Agent-assisted handoff as documented workflow | None | High | Already possible with dual MCP (Jira + Unblock). Document as integration guide |
| `import_sprint` tool in Unblock | Medium | Low | Agent can already do this by composing Jira MCP reads + Unblock creates. Dedicated tool adds maintenance without real value |
| Jira custom field with GitHub Issue link | Low | Medium | Jira Automation or the sync Action adds the link back. Bidirectional traceability |
| Sprint velocity from GitHub close events | Medium | Medium | GitHub Action aggregates closed issues per sprint and posts summary to Jira. Nice for reporting |

---

## 8. Recommendation

Start with the minimal integration:

1. **Convention:** `## Jira\nPROJ-1234` in issue body for all handoff issues
2. **`jira-sync.yml`:** Close → Jira transition + comment. Copy-paste the workflow above
3. **Dual MCP:** Add `mcp-atlassian` to agent config for context reads
4. **Documentation:** Integration guide in README or separate doc

Evaluate in-progress sync and sprint velocity reporting only after the basic flow is validated with a team.
