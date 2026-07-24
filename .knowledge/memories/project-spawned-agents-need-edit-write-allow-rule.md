---
name: project-spawned-agents-need-edit-write-allow-rule
description: Background subagents can be silently denied Write/Edit inside worktrees unless the project's permission settings grant path-scoped Edit/Write rules covering the repo tree
type: environment
---

Spawned **background subagents** (Implement/spec agents via the Agent tool with `isolation: "worktree"`) can be silently **denied every `Write`/`Edit`** in their worktrees — not for a content reason, but at the permission/sandbox layer. They can Read + run Bash/cargo, but not write files, so they finish a full analysis and then report "blocked" with nothing landed.

**Root cause:** a project's `.claude/settings.json` `permissions.allow` list can name only a narrower tool alias (e.g. an MCP-bridge's own `Write`/`Edit` tool names) and omit the **plain** `Write` / `Edit` tools that ordinary subagents actually use. An interactive orchestrator session can write because its plain Write/Edit gets approved interactively; background agents can't prompt → denied. Under the sandbox, the writable path set is **derived from the `Edit(...)` allow permission rules** (`sandbox.filesystem.allowWrite` is "merged with paths from Edit(...) allow permission rules"), so with no `Edit(<path>)` rule, no path is writable.

**Fix:** add path-scoped rules to `permissions.allow`:
`"Edit(<repo-root>/**)"` and `"Write(<repo-root>/**)"` — a repo-root glob covers the crates, docs, and any agent worktrees nested under `.claude/worktrees/`. A `bypassPermissions` spawn mode alone does **not** help (the path still isn't in the sandbox writable set); the path-allow rule is what fixes it.

**Operational takeaway:** if a spawned implementer comes back "write blocked", check `.claude/settings.json`'s `permissions.allow` for plain `Edit(...)`/`Write(...)` path rules before anything else — and spawn implementers with a cheap write-self-check first instruction so a residual block is reported fast, not after a long analysis. Relates to the orchestrator→implementer model in [[project-unblock-rust-rewrite]].
