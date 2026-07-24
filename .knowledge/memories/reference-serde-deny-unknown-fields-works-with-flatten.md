---
name: reference-serde-deny-unknown-fields-works-with-flatten
description: serde's documented "deny_unknown_fields is incompatible with flatten" restriction is stale — on serde 1.0.228 it compiles AND rejects at runtime.
type: reference
---

`#[serde(deny_unknown_fields)]` **does** work alongside `#[serde(flatten)]` on serde 1.0.228. `serde_derive`
emits a post-flatten leftover scan. Verified 2026-07-21 with a scratch crate pinned to the workspace serde,
replicating a real outer container with a flattened attribution struct:

```
GOOD -> Ok("ok")
TYPO -> Err(Error("unknown field `descriptionn`", line: 1, column: 32))
```

This killed an estimate of 22 hand-written `JsonObject` deserializers, collapsing it to ~10 one-line attributes.

**Three placement rules are load-bearing** (proven, not assumed):
1. `deny` on a flatten **target** is a **silent no-op** — put it on outer containers only.
2. `deny` is **not recursive** — every nested non-flattened struct needs its own.
3. `deny` **alone** does not fix silent data loss: it converts a silently-discarded unknown field into an
   out-of-band JSON-RPC `-32602`. It must co-land with in-band structured-error mapping.

**Why:** rule 1 and rule 2 both fail *silently with a green test suite* — a missed nested attribute leaves the
data-loss bug live and nothing goes red. Per-container mutation expectations are the only guard.

**How to apply:** don't trust the serde docs' incompatibility note; compile it against the project's pinned
serde. And when adding `deny`, enumerate every input type yourself rather than trusting a plan's container list.

Related: [[reference-mcp-tool-schema-flattening-is-client-side]]
