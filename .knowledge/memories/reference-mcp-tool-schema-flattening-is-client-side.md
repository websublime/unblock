---
name: reference-mcp-tool-schema-flattening-is-client-side
description: The flattened `required: []` MCP tool schema you see in your own tool surface is the CLIENT renderer, not what the server publishes — verify on the wire before calling it a server defect.
type: gotcha
---

When an MCP tool's schema appears in your own tool definitions as a flat object with `required: []` and the
real constraint demoted to a prose sentence like *"Input constraint: Provide parameters for exactly one of:
(action, issue_id, body) or ..."*, that is **the MCP client's renderer synthesizing it** — not the server
serving a degraded schema.

Verified 2026-07-21 on unblock: over real JSON-RPC, `tools/list[i].inputSchema` is **byte-identical** to the
`unblock://schema` bundle's `[tool].input` for all 8 tools — full `oneOf` per action, 66 `required` entries,
`comment.add` correctly declaring `required: ["action","issue_id","body"]`. The "Input constraint" prose has
**zero** hits in the repo and **zero** in `rmcp-1.7.0/src`.

**Why this matters:** an earlier claim asserted the server was publishing a degraded `tools/list` and that
this was the proximate cause of the ub-lp9.12 dogfood bug. Both were wrong. The hypothesis was falsified by
a review lens and no artifact may assert that causal chain.

**How to apply:** before claiming a schema-fidelity defect, stand up the server against a throwaway workspace
and diff the raw `tools/list` response against the schema resource. Your own tool-surface rendering is not
evidence about the server. Genuine server-side gaps found the same way *were* real: `outputSchema` absent on
all 8 tools (rmcp 1.7 supports it — not blocked upstream), and `additionalProperties` never emitted.

Related: [[reference-serde-deny-unknown-fields-works-with-flatten]]
