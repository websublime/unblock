---
name: reference-mcp-tool-description-bytes-duplicated-twice
description: An unblock MCP tool's description is contract bytes TWICE — the const fed into capabilities()/CONTRACT_HASH and a duplicate literal in the #[tool(description)] wire attribute — pin them with a (name, description) assert; GA-frozen.
type: gotcha
---

Every unblock MCP tool's description string exists in **two** places that must stay byte-identical:
1. A const fed into `capabilities()` + `schema_bundle()` → **digested into `CONTRACT_HASH`**.
2. A **duplicate literal** in the `#[tool(description = "…")]` attribute — this is what `tools/list` actually
   emits on the wire, and **rmcp requires a literal there** (no const interpolation).

**The trap:** nothing cross-checks the two. `live_list_tools_equals_the_builder_eight`
(`contract_suite.rs`) historically compared tool **names only**, and `contract_hash_matches_the_pinned_gate`
only sees copy #1. So mutating the wire literal (copy #2) leaves the whole `contract_suite` GREEN — the wire
description can silently diverge from the hashed one, and it **freezes at GA under semver**.

**Proven real, not hypothetical (T3.9, 2026-07-17):** the `claim` tool shipped this divergence since rc.1 — wire
= `"…for an assignee; the loser of a race is reported."`, hashed `capabilities()` = truncated
`"…for an assignee."`. Surfaced only when T3.9 added a `(name, description)` pair assert. The resolution
adopted the WIRE bytes as canonical (clients rc.1–rc.3 already receive them; the truncated descriptor was the
lying copy; the extra text is info agents consume). Fix pattern: hoist a shared `pub(crate) const
X_TOOL_DESCRIPTION`, use it in `capabilities()`, and copy it byte-identically into the attribute literal.

**How to apply:** when adding/editing any MCP tool, (a) single-source the description as a const and copy it
verbatim into the attribute; (b) ensure `live_list_tools_equals_the_builder_eight` maps to **(name, description)
pairs**, not names — that one ~3-line assert closes it for all tools at once; (c) treat the description as
**GA-frozen contract bytes** (inside `CONTRACT_HASH`). See [[project-comments-pull-forward-v1]] and the CD-1
schemars(extend) root-type:object trap in [[project-mcp-conformance-confrontation-d1-d5]].
