---
name: project-dogfood-unblock-is-the-tracker
description: Post-GA (2026-07-20) unblock DOGFOODS itself as the issue tracker — STATUS.md retired to a stub; how to drive it + findings
type: reference
---

**⭐ ACTIVE since 2026-07-20 (post-GA).** unblock now **dogfoods itself**: the task system-of-record is
**unblock over MCP**, NOT `docs/plans/STATUS.md` (which is now a retired pointer stub). CLAUDE.md +
docs/PROCESS.md were repointed (landed `a9f84ca`; server-key rename `a8f7e2c`) — they now instruct
agents/sessions to track in unblock. Supersedes the STATUS.md-row workflow in [[project-unblock-rust-rewrite]].

**The record + rhythm:** git-backed record = **`.unblock/issues.jsonl`** (D5 committed JSONL export; the
local `unblock.db` is gitignored — `*.db` + `.unblock/.write.lock`). Rhythm: pick ready via the `query`
tool (`ready`), register/update via `issue`/`comment`, **re-export (`sync export`) + commit `issues.jsonl`
in the SAME commit as the work**, close the issue on merge. D5 model B — manual export/import, no 3-way
merge, reconcile by hand until the v1.2 shared remote.

**Driving it (two ways):**
- **Native (fresh session):** the project MCP config MUST be **`.mcp.json`** (WITH the leading dot — Claude
  Code silently IGNORES a no-dot `mcp.json`; this was the reason tools didn't load, fixed 2026-07-20 by
  renaming `mcp.json`→`.mcp.json`, commit `21d40db`, confirmed via claude-code-guide + code.claude.com/docs/mcp).
  It registers server key **`unblock`** → tools `mcp__unblock__*` (via `cargo run --bin unblock mcp`, which
  compiles the LATEST source). **Fresh-session flow:** startup reads `.mcp.json` → a one-time APPROVAL prompt
  (accept, or `/mcp` → approve) → first tool call COMPILES (~30s) → tools live. `AGENTS.md` (managed block from
  `unblock agents`) carries the wiring + the 8-tool/actions capability table. NB: `.vscode/mcp.json` /
  `.cursor/mcp.json` are those editors' OWN (different) config files — correct as-is, not the Claude Code one.
- **Fallback (session WITHOUT native tools, like the one that set this up):** drive `target/debug/unblock
  mcp` via a **line-delimited JSON-RPC-over-stdio** harness (scratchpad `ub_*.py`): initialize →
  notifications/initialized → tools/call; close stdin → server exits 0. **First rebuild** the binary
  (`cargo build --bin unblock`) — the prebuilt `target/debug/unblock` can be a STALE rc (reported `rc.3`
  when main was `1.0.0`); `cargo run`/a rebuild gives the real version.

**Fresh workspace recipe (what to do — an earlier attempt got it wrong):** `cargo build --bin unblock` (→ v1.0.0)
→ `rm -rf .unblock/` (clean wipe, NOT just the .db) → `unblock init --prefix ub` (config + migrated v1 DB)
→ seed via the mcp server. Just deleting the `.db` + reusing an old workspace can hit stale-schema breakage.

**Tracker mechanics (durable):** child issue ids are `parent.N` and **parenting is id-only** (no `parent_id`
column; the JSONL export is still faithful). **Labels reject `.`** (alphanumeric/-/_/: only) → use `v1-1` not
`v1.1`. For the current epic/task state, query the live tracker (unblock over MCP; the `query`/`issue` tools) —
this memory does not carry a point-in-time snapshot.

**Tool arg gotchas (verify EVERY mutation via list/export — an OK is not proof):** `comment add` field is
**`body`** (NOT `content`; DB col is `text`) + `issue_id`; `issue create` uses `title`/`issue_type`/`parent`/
`labels`/`description`/…; `sync export` = `{action:"export"[,path]}`; `issue close` = `{action:"close",id,reason}`;
`query` = `{kind:...}`; **labels reject `.`** (alphanum/-/_/: only → use `v1-1`).

**Findings surfaced by dogfooding (real):**
1. ⚠️ **migration-edit-drift** — T3.9 changed the schema "NO migration", so a `unblock.db` created BEFORE
   comments breaks on the GA binary (`no such column: updated_at`). Fresh installs fine; NO upgrade path for
   old DBs → v1.0.1/v1.1 candidate. (Cousin of [[feedback-migration-edit-drift]].)
2. 💡 **init/agents two-step DX** — `init` does NOT create AGENTS.md (by design, D27/AF-3: `agents` is a
   SEPARATE command). Whether `init` should hint/offer `--agents` is a v1.1 DX candidate.
3. ⚠️ **silent no-op on malformed `comment add`** — sending `content` instead of the required `body` returned
   **`OK` + empty structuredContent and persisted NOTHING** (list=[]), instead of a VALIDATION_FAILED error.
   Silent data-loss risk for an agent-first tracker → confirm (rmcp arg-leniency vs unblock handling) → v1.0.1/v1.1.
4. ✅ `.write.lock` (D31 runtime lock) is now gitignored.

**Reference:** the doc-lint CORPUS is the FIXED 19-file set — `docs/PRD.md` + the 6 `docs/plans/*.md` files
(`00-roadmap.md`, `01-design-spine.md`, `README.md`, `STATUS.md`, `ci-cd-and-distribution.md`,
`implementation-plan.md`) + all 12 `docs/plans/crates/unblock-*.md` crate plans (NOT CLAUDE.md / PROCESS.md) —
per `xtask/src/doc_lint.rs` `CORPUS`.

Relates to [[project-t3-6-release-pipeline-scope]] (GA).
