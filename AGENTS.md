<!-- BEGIN unblock -->
## unblock (MCP)

This workspace is tracked by **unblock**. Issue-data operations are exposed over MCP — the
`unblock` CLI is lifecycle/ops only.

- Start the server: `unblock mcp` (MCP over stdio).
- Contract: `unblock.mcp.v1.12`.
- Machine-readable discovery: read `unblock://capabilities` (the source of these tables) and
`unblock://schema` (the full JsonSchema bundle for every tool I/O).

### Tools

Descriptors (from `unblock://capabilities`):

| Tool | Description |
|---|---|
| `issue` | Create, show, update, close, reopen, delete, restore, relabel, or close_eligible issues, or get, set, unset and list runtime configuration with config. Where an action accepts the optional agent_name, harness and model fields, unblock currently discards them. |
| `claim` | Atomically claim an issue for an assignee; the loser of a race is reported. Where an action accepts the optional agent_name, harness and model fields, unblock currently discards them. |
| `defer` | Defer an issue until a future timestamp, or undefer it. Where an action accepts the optional agent_name, harness and model fields, unblock currently discards them. |
| `query` | Query issues: list, ready, blocked, search, count, stale, labels, or epics. |
| `dep` | Manage and query dependencies: add, remove, list, tree, cycles, or graph. Where an action accepts the optional agent_name, harness and model fields, unblock currently discards them. |
| `sync` | Export/import the issue store as JSONL, or one-shot import a bd export. |
| `diagnostics` | Diagnostics: stats, info, where, version, lint, changelog, orphans, or dangling. |
| `comment` | Comment on issues: add, list, update, or delete (soft-redact). Where an action accepts the optional agent_name, harness and model fields, unblock currently discards them. |

Actions (structural — derived from the `unblock://schema` `oneOf` discriminants; the
Discriminant cell is the exact `key: value` pair to send, and each row lists the FULL
parameter surface, required AND optional):

Discriminant keys: `action` — `issue`, `defer`, `dep`, `sync`, `comment`; `kind` — `query`, `diagnostics`; none (flat) — `claim`.

| Tool | Discriminant | Required params | Optional params |
|---|---|---|---|
| `issue` | `action: create` | `title` | `acceptance_criteria`, `agent_context`, `agent_name`, `assignee`, `defer_until`, `deps`, `description`, `design`, `due_at`, `ephemeral`, `estimated_minutes`, `harness`, `issue_type`, `labels`, `model`, `parent`, `priority`, `quick`, `slug` |
| `issue` | `action: create_bulk` | `markdown` | — |
| `issue` | `action: show` | `id` | — |
| `issue` | `action: update` | `ids` | `acceptance_criteria`, `agent_name`, `assignee`, `close_reason`, `description`, `design`, `due_at`, `estimated_minutes`, `external_ref`, `harness`, `issue_type`, `labels_add`, `labels_remove`, `labels_set`, `model`, `notes`, `owner`, `parent`, `priority`, `status`, `title` |
| `issue` | `action: close` | `id` | `agent_name`, `harness`, `model`, `reason`, `suggest_next` |
| `issue` | `action: reopen` | `id` | `agent_name`, `harness`, `model` |
| `issue` | `action: delete` | `ids` | `agent_name`, `harness`, `mode`, `model` |
| `issue` | `action: restore` | `id` | `agent_name`, `harness`, `model` |
| `issue` | `action: relabel` | `new_name`, `old_name` | `agent_name`, `harness`, `model` |
| `issue` | `action: close_eligible` | — | `agent_name`, `dry_run`, `harness`, `model` |
| `issue` | `action: config` | `op` | `agent_name`, `harness`, `key`, `model`, `value` |
| `claim` | — | `assignee`, `id` | `agent_name`, `harness`, `model` |
| `defer` | `action: defer` | `id`, `until` | `agent_name`, `harness`, `model` |
| `defer` | `action: undefer` | `id` | `agent_name`, `harness`, `model` |
| `query` | `kind: list` | — | `assignee`, `include_closed`, `include_deferred`, `issue_type`, `labels_all`, `labels_any`, `limit`, `offset`, `priority_max`, `priority_min`, `status`, `text_contains` |
| `query` | `kind: ready` | — | `assignee`, `include_closed`, `include_deferred`, `issue_type`, `labels_all`, `labels_any`, `limit`, `offset`, `priority_max`, `priority_min`, `status`, `text_contains` |
| `query` | `kind: blocked` | — | `assignee`, `include_closed`, `include_deferred`, `issue_type`, `labels_all`, `labels_any`, `limit`, `offset`, `priority_max`, `priority_min`, `status`, `text_contains` |
| `query` | `kind: search` | `query` | `assignee`, `include_closed`, `include_deferred`, `issue_type`, `labels_all`, `labels_any`, `limit`, `offset`, `priority_max`, `priority_min`, `status`, `text_contains` |
| `query` | `kind: count` | — | `assignee`, `group_by`, `include_closed`, `include_deferred`, `issue_type`, `labels_all`, `labels_any`, `limit`, `offset`, `priority_max`, `priority_min`, `status`, `text_contains` |
| `query` | `kind: stale` | `older_than` | `assignee`, `include_closed`, `include_deferred`, `issue_type`, `labels_all`, `labels_any`, `limit`, `offset`, `priority_max`, `priority_min`, `status`, `text_contains` |
| `query` | `kind: labels` | — | — |
| `query` | `kind: epics` | — | `eligible_only`, `id` |
| `dep` | `action: add` | `dep_type`, `depends_on_id`, `issue_id` | `agent_name`, `harness`, `metadata`, `model` |
| `dep` | `action: remove` | `dep_type`, `depends_on_id`, `issue_id` | `agent_name`, `harness`, `model` |
| `dep` | `action: list` | `id` | — |
| `dep` | `action: tree` | `id` | — |
| `dep` | `action: cycles` | — | `blocking_only` |
| `dep` | `action: graph` | — | `roots` |
| `sync` | `action: export` | — | `path` |
| `sync` | `action: import` | `path` | `dry_run` |
| `sync` | `action: import_bd` | `path` | — |
| `diagnostics` | `kind: stats` | — | — |
| `diagnostics` | `kind: info` | — | — |
| `diagnostics` | `kind: where` | — | — |
| `diagnostics` | `kind: version` | — | — |
| `diagnostics` | `kind: lint` | — | — |
| `diagnostics` | `kind: changelog` | — | `since` |
| `diagnostics` | `kind: orphans` | — | — |
| `diagnostics` | `kind: dangling` | — | — |
| `comment` | `action: add` | `body`, `issue_id` | `agent_name`, `harness`, `model` |
| `comment` | `action: list` | `issue_id` | — |
| `comment` | `action: update` | `body`, `comment_id` | `agent_name`, `harness`, `model` |
| `comment` | `action: delete` | `comment_id` | `agent_name`, `harness`, `model` |

### Resources

| Resource | Description |
|---|---|
| `unblock://issues/{id}` | A single issue by id. |
| `unblock://issues/ready` | The default-complete ready set (agent entrypoint). |
| `unblock://issues/blocked` | The blocked set. |
| `unblock://capabilities` | This discovery document. |
| `unblock://schema` | The JsonSchema bundle for every tool I/O. |

### Prompts

| Prompt | Description |
|---|---|
| `triage` | A guided triage workflow over blocked/unassigned/deferred work. |
| `plan_next_work` | Drive the ready -> claim selection (FR-20). |
| `close_with_suggestions` | Close an issue and surface the newly-unblocked set (FR-11). |

### Error codes

| Code | Exit | Retryable |
|---|---|---|
| `DATABASE_NOT_FOUND` | 2 | no |
| `DATABASE_LOCKED` | 2 | yes |
| `SCHEMA_MISMATCH` | 2 | no |
| `DATABASE_ERROR` | 2 | no |
| `NOT_INITIALIZED` | 2 | no |
| `ALREADY_INITIALIZED` | 2 | no |
| `RATE_LIMITED` | 2 | yes |
| `ISSUE_NOT_FOUND` | 3 | no |
| `AMBIGUOUS_ID` | 3 | yes |
| `ID_COLLISION` | 3 | no |
| `INVALID_ID` | 3 | no |
| `NOTHING_TO_DO` | 3 | no |
| `ALREADY_CLAIMED` | 3 | yes |
| `VALIDATION_FAILED` | 4 | yes |
| `INVALID_STATUS` | 4 | yes |
| `INVALID_TYPE` | 4 | yes |
| `INVALID_PRIORITY` | 4 | yes |
| `REQUIRED_FIELD` | 4 | yes |
| `POLICY_VIOLATION` | 4 | no |
| `CYCLE_DETECTED` | 5 | no |
| `DEPENDENCY_NOT_FOUND` | 5 | no |
| `HAS_DEPENDENTS` | 5 | no |
| `SELF_DEPENDENCY` | 5 | no |
| `DUPLICATE_DEPENDENCY` | 5 | no |
| `JSONL_PARSE_ERROR` | 6 | no |
| `PREFIX_MISMATCH` | 6 | no |
| `IMPORT_COLLISION` | 6 | no |
| `SYNC_CONFLICT` | 6 | no |
| `CONFLICT_MARKERS` | 6 | no |
| `PATH_TRAVERSAL` | 6 | no |
| `CONFIG_ERROR` | 7 | no |
| `CONFIG_NOT_FOUND` | 7 | no |
| `CONFIG_PARSE_ERROR` | 7 | no |
| `IO_ERROR` | 8 | no |
| `JSON_ERROR` | 8 | no |
| `INTERNAL_ERROR` | 1 | no |
<!-- END unblock -->
