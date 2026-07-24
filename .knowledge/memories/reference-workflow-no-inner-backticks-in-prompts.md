---
name: reference-workflow-no-inner-backticks-in-prompts
description: Workflow scripts are plain JS — a backtick code-span inside an agent-prompt template literal closes the string early and throws a parse error; use single quotes for code identifiers in prompts
type: gotcha
---

When authoring a `Workflow` script, the `agent(...)` prompt strings are usually **template literals** (delimited by
backticks). Writing an inline code span with backticks INSIDE that prompt (e.g. `` `current_user_version` `` or
`` `some-wrapped-command ...` ``) **terminates the template literal early** → `Script parse error: Unexpected token`.
This bit me twice in the T3.1 session (design-Review + re-Verify workflows), each costing a failed launch + resend.

**How to apply:** in Workflow agent-prompt template literals, refer to code identifiers with SINGLE QUOTES or plain
text (`'schema_version'`, not `` `schema_version` ``). If you truly need a backtick, escape it (`` \` ``) — that
worked fine in the spec-writer prompt where it was written as `` \`cargo xtask doc-lint\` ``. Same rule for `${...}`: only use
it for real interpolation. Relates to [[project-workflow-flat-schema-for-coordinator]] (the other recurring Workflow
authoring gotcha — flat schemas + agents write files).
