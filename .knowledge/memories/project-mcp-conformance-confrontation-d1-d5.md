---
name: project-mcp-conformance-confrontation-d1-d5
description: MCP Inspector confrontation (2026-07-09) found 5 conformance defects; the BLOCKER = rmcp 1.7 enforces root type:object on OUTPUT schemas but NOT input, so tagged-enum tool inputs ship a root oneOf that strict clients reject
type: reference
---

Confrontation of the unblock MCP surface vs MCP spec 2025-06-18 + spine §5 (Miguel drove it from the
Inspector v0.22.0: tools/list errored, resources empty). 18-agent workflow, adversarially verified.
5 CONFIRMED defects (2 refuted). All round-trips (tools/call, resources/read, prompts/get) WORK — the
breaks are at DISCOVERY/validation, not execution.

- **D1 BLOCKER — tools/list inputSchema root lacks `type:"object"`.** 6/7 tools (issue, defer, dep,
  query, sync, diagnostics) use spine §5.2 `#[serde(tag="action"/"kind")]` enums → schemars 1.2.1
  renders a root `oneOf` with no root `type`. Only `claim` (plain struct) conforms. The MCP TS SDK
  (`ToolSchema.inputSchema = z.object({type: z.literal("object")}).passthrough()` inside
  `z.array(ToolSchema).parse()`) rejects the WHOLE list on one bad element → all 7 tools go dark in
  Inspector / Claude Desktop / claude.ai. **Root dependency bug:** rmcp 1.7.0
  `handler/server/common.rs::schema_for_type()` (input path) serializes schemars verbatim with only an
  is-object guard; the sibling `schema_for_output()` DOES enforce root `type:object`. rmcp knows the
  rule and applies it to output but not input. **Fix (ratified):** `#[schemars(extend("type"="object"))]`
  on the 6 enums — injects root type AFTER the oneOf, keeps the discriminated union (no flattening).
  One attribute fixes BOTH the live tools/list AND the `unblock://schema` bundle (same `T::json_schema`).
- **D2 HIGH — structuredContent is a bare top-level array.** spine §5.3 `#[serde(untagged)]`
  QueryOutput/DepOutput/IssueOutput Vec-arms serialize as arrays; `CallToolResult.structuredContent`
  is object-typed. Fix (ratified W1): object-wrap the 5 array arms → {"issues":[...]}, {"counts":[...]},
  {"deps":[...]}, {"cycles":[...]}, {"issues":[...]}.
- **D3 MEDIUM (impl/PRD drift)** — resources/list is []; 4 static URIs (issues/ready, /blocked,
  capabilities, schema) mis-registered as templates (only issues/{id} is a real template). Fix: implement
  `list_resources` for the 4 concrete URIs (re-aligns to PRD §12.2). No hash impact.
- **D4 MEDIUM (impl bug)** — initialize echoes ANY client protocolVersion (even bogus), no clamp (rmcp default).
- **D5 LOW (impl bug)** — resources/read stamps `mimeType:"text"` (non-IANA), contradicts advertised
  application/json; rmcp `ResourceContents::text` hardcodes it; apply `.with_mime_type("application/json")`.

**NOT bugs (do not over-correct):** exactly 3 prompts + 5-resource COUNT == spine §5.4/§5.5 (only the
list/template classification is wrong); zero-arg prompts; not-found→-32002; stdio hygiene; error taxonomy;
no pagination. The Inspector "Apps" error is just a symptom of D1 (+ unblock implements no MCP-UI
`_meta.ui.resourceUri`).

**Why `cargo test` stayed green (test-adequacy gap):** the contract test uses the rmcp RUST client
(`list_all_tools`) which reads `input_schema` as an opaque `Arc<JsonObject>` (no type check) and asserts
NAMES/COUNT only; there is NO insta snapshot of the tools/list or resources/list WIRE bytes; and
`schema_bundle.snap` blessed the broken shape as baseline; `every_tool_schema_is_an_object` asserts
`is_object()` which a oneOf root passes vacuously. Add T-1 (assert `inputSchema.type=="object"` per tool)
as the mandatory minimum.

**Resolution (Miguel ratified):** D1+D2 = spine↔protocol drift → spec-first (spine §5.2a new normative
clause + `extend` on 6 sketches; §5.3 W1 wrap), landed docs-first BEFORE code; single `CONTRACT_VERSION
v1.2→v1.3` bump for the impl (bundle, not split) with re-pin + golden re-bless. D3/D4/D5 = pure impl
fixes (no spine change). See [[project-t3-4-reliability-gates-scope]]. Full report was in scratchpad
CONFRONTATION_REPORT.md (session-only; git is the archive once landed).

**Landed:** PR #400 (spec-first docs-only spine §5.2a + `extend` sketches + §5.3 W1) — MERGED into main (b481205).
PR #401 (branch `impl-mcp-cd1-cd2-v13-contract`) = the paired CODE, single `unblock.mcp.v1.2→v1.3` bump: `extend`
on the 6 enums + IssueList/CountList/DepList/CycleList wrappers + all construction sites + CONTRACT_HASH re-pin
(`1bd36281…29572`) + goldens re-blessed + tests (strengthened every_tool_schema_is_an_object, T-1 live root-type
gate, CD-2 structuredContent-is-object gates) + tracking flips (crate-plan F-5/F-7, STATUS). Verify gate PASS
(4-lens incl MCP-input security), full CI probe green — OPEN, MERGEABLE, awaiting Miguel's merge.
GOTCHA: the impl workflow's single implementer left its work UNCOMMITTED in the worktree (branch at main SHA,
`main..HEAD` empty) — recover by committing the worktree working-tree state to a real branch BEFORE the harness
reclaims it (uncommitted worktree state is NOT guaranteed to persist; commits are). See
[[project-background-agent-crash-recovery]].
PR #402 = CD-3/CD-4/CD-5 (pure ServerHandler impl, no spine, no CONTRACT_HASH) — Verify gate PASS (4-lens), full CI
green — MERGED into main (cea6da7). ALL FIVE defects (CD-1..CD-5) now on main; the original Inspector errors are
resolved.
CD-4 nuance (Miguel-ratified Option 1): a pure `ServerHandler::initialize` override is EMPIRICALLY INSUFFICIENT in
rmcp 1.7 (the serve-loop re-derives the wire protocolVersion as a LEXICAL min(client,handler) AFTER the handler),
so the fix is a `VersionClampingTransport` decorator clamping unsupported inbound versions to ProtocolVersion::LATEST
before negotiation — correct but coupled to an rmcp internal.
CD-6 (harden CD-4): the assumption-pin PART is in PR #404 (open) — a `serve_duplex_unclamped_for_test` helper
(feature=test-util, doc(hidden), bypasses the sole `VersionClampingTransport` wrap site) + a pin asserting rmcp 1.7
echoes an unsupported below-latest version ("1999-01-01") VERBATIM (non-vacuous: only holds if no clamp ran) + a
`KNOWN_VERSIONS`/`LATEST` set pin. Verify gate PASS (3-lens, all confirm non-vacuous). **Still open:** the CD-6
REMAINDER = an upstream rmcp fix making unsupported-version clamping a first-class ServerHandler contract, then
delete `VersionClampingTransport` (outward-facing — not started). D-id question RESOLVED = none (58c42ab). Open
for Miguel: whether any CD warrants a PRD §4 D-id + a light annotation of D25's "untagged/wire-identical" phrasing.
(Workflow gotcha this session: a missing `.join('\n')` on the impl agent() prompt caused 2 clean no-ops — see
[[reference-workflow-agent-prompt-must-be-joined-string]].)
