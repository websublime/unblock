# ://unblock — documentation

> Documentation repository for [://unblock](https://github.com/websublime/unblock).  
> Reference project: [arc-pm](https://github.com/websublime/arc-pm)

---

## Structure

```
docs/
├── MANIFESTO.md          ← Soul of the product: why, beliefs, immutable laws
├── PRD.md                ← Requirements: personas, market, phases, metrics
├── SPEC.md               ← Master technical specification (all systems)
├── plans/                ← Per-phase implementation plans (numbered)
│   ├── 01-plan-mcp-foundation.md
│   ├── 02-plan-mcp-complete.md
│   └── 03-plan-mcp-production.md
└── specs/                ← Per-component detailed specs (numbered)
    ├── 01-spec-graph-engine.md
    ├── 02-spec-github-client.md
    └── 03-spec-mcp-tools.md
```

## Document hierarchy — abstract to concrete

| Level | Document | Answers | Tone |
|-------|----------|---------|------|
| 1 | **MANIFESTO** | *Why do we exist? What do we believe?* | Philosophical, declarative, immutable |
| 2 | **PRD** | *What to build? For whom? In what order?* | Product, strategic — links to plans/specs |
| 3 | **SPEC** | *How does it work technically?* | Engineering, precise, all sections |
| 4 | **plans/** | *How to implement each module?* | Epics, tasks, definition of done |
| 5 | **specs/** | *What are the algorithms and edge cases?* | Validation, invariants, error catalogues |

## Patterns

- **MANIFESTO is the foundation** — everything else is implementation detail
- **Cross-referencing** — PRD header links to all plans and specs
- **Consistent numbering** — plans and specs share the same number (`01-plan-*` ↔ `01-spec-*`)
- **Companion links** — every document links to its related plan/spec
- **Plans have a fixed structure**: Purpose → Rust Idioms → Public API Surface → Epics → Definition of Done

## Source material

The `docs/archive/` directory contains the original documentation written before this structure was adopted. Preserved for reference but superseded by the documents above.

## Token scopes

The configured `GITHUB_TOKEN` (PAT or GitHub App) drives both the live
test job and the production MCP server. The required scopes depend on
which `setup` operations are exercised:

- `repo` — required for issue read/write, REST mutations, GraphQL
  read queries, and Projects V2 item placement.
- `project` — required for Projects V2 field management
  (`createProjectV2Field`, `updateProjectV2Field`, view CRUD).
- `read:org` — required for **reading** org-level GitHub issue types
  during `setup_fields` (always read; idempotent diff against the
  canonical eight `Task`, `Bug`, `Feature`, `Spike`, `Epic`, `Chore`,
  `Refactor`, `Docs`).
- `admin:org` — required ONLY when `setup_fields`'s `IssueType`
  ensure-and-heal step needs to **create** missing org-level issue
  types (`POST /orgs/{org}/issue-types`). Subsequent `setup` runs
  against an org that already carries all eight canonical types only
  need `read:org`. When the token lacks `admin:org` and creation is
  needed, the server surfaces a typed
  `IssueTypeManagementForbidden { org }` error pointing operators at
  the upgrade path. Org admins can also pre-create the eight canonical
  issue types in the org settings UI as a no-token-elevation alternative.

See [docs/specs/01-spec-mcp-foundation.md §5.7][spec-§5.7] for the
ensure-and-heal contract and [§12][spec-§12] for the full token-scope
matrix.

[spec-§5.7]: docs/specs/01-spec-mcp-foundation.md
[spec-§12]: docs/specs/01-spec-mcp-foundation.md

## Create-time defaults

The `create` MCP tool resolves four default-bearing project fields
deterministically before issuing any GitHub mutation. The precedence
table below is normative — see
[docs/specs/01-spec-mcp-foundation.md §8.3][spec-§8.3]:

| Field       | If `params.<x>` is `Some`               | If `params.<x>` is `None`                                                  |
|-------------|------------------------------------------|----------------------------------------------------------------------------|
| `Status`    | (server-managed — no param)              | `Backlog` (sticky default per `unblock-1zj`; only explicit transitions move out) |
| `Priority`  | use the validated value                  | `P2` — Medium                                                              |
| `Agent`     | use the validated non-empty value        | `state.agent_kind_str()` if a known agent kind was detected, else **omit** |
| `IssueType` | use the canonical name (case-insensitive) | `Task`                                                                     |

Note: `config.agent` is NOT consulted by `claim` or `create` — the
`UNBLOCK_AGENT` env var is preserved for legacy/test purposes only
(see [§12][spec-§12]).

The eight canonical `IssueType` variants are sourced from
`unblock_core::types::IssueType::canonical_name`: `Task`, `Bug`,
`Feature`, `Spike`, `Epic`, `Chore`, `Refactor`, `Docs`. The `update`
tool's `issue_type` param accepts the same set with case-insensitive
matching, but per [§8.6][spec-§8.6] follows an
absence-leaves-unmodified rule (no fallback chain — explicit
opt-in only).

[spec-§8.3]: docs/specs/01-spec-mcp-foundation.md
[spec-§8.6]: docs/specs/01-spec-mcp-foundation.md

## CI live tests

The `test-mcp-live` job in `.github/workflows/ci.yml` exercises the live GitHub API path of `unblock-mcp` (the `#[ignore]` integration tests under `crates/unblock-mcp/tests/`). It runs only on first-party events — pushes to `main`, scheduled nightly builds, manual `workflow_dispatch`, and PRs whose head branch lives in this repository — so forked PRs never see the secret.

The job has a preflight step that fails fast with an actionable error if any of the three required repository settings is missing. To configure them on a fresh clone, run (with `gh` authenticated against `websublime/unblock`):

```bash
# Repository secret — fine-grained PAT or classic PAT with `repo` + `project` scopes
# (plus `read:org` for IssueType ensure-and-heal; add `admin:org` ONLY if the test
#  org does not already have the eight canonical types — see Token scopes above)
gh secret set UNBLOCK_TEST_TOKEN --body '<github-pat>'

# Repository variable — the owner/repo string the live tests should hit
gh variable set UNBLOCK_TEST_REPO --body 'websublime/unblock'

# Repository variable — the GitHub Project (Projects V2) number used by the e2e_workflow test
gh variable set UNBLOCK_TEST_PROJECT --body '<project-number>'
```

The `coverage` job (mock-only) runs on every PR and the `test-mcp-live` job covers both live execution and live coverage in a single tarpaulin pass on first-party events. Codecov merges the two reports under the `mock` and `live` flags; the >80% target in spec §13.2 applies to combined coverage.

### Clean-state contract for the test project

The live test job mutates the GitHub Project pointed at by `UNBLOCK_TEST_PROJECT`. Five invariants govern its starting state on every run:

1. **The 6 unblock-managed custom fields must NOT exist on the project.** They are: `Priority`, `PipelineStage`, `Agent`, `ClaimedAt`, `StoryPoints`, `DeferUntil`. The `setup_fields` flow creates them and the test asserts on the post-creation field IDs; a stale, half-configured set from a previous failed run breaks the assertion or trips a `Name has already been taken` collision under parallel test execution.
2. **The built-in `Status` field is left in place.** GitHub Projects V2 forbids deleting the built-in Status field (`deleteProjectV2Field` returns `Only custom fields can be deleted`). `setup_fields` auto-heals the Status options to the spec's canonical set (`Backlog`, `Ready`, `In Progress`, `Blocked`, `Deferred`, `Closed` — TitleCase, board order; sourced from `Status::option_name`, see spec §5.7) on every run by issuing `updateProjectV2Field` against the existing field id, so no manual surgery is required for Status. The auto-heal matcher reuses existing option IDs across the lowercase → TitleCase rename via a normalised name comparison, so item assignments survive the migration.
3. **Fixture issues are wiped before every CI run.** Every live test attaches the canonical `unblock-fixture` label to every issue it creates via the `fixture_labels()` test helper. The CI live job invokes `scripts/setup-test-project.sh --wipe-issues` before tarpaulin starts; the wipe enumerates all OPEN `unblock-fixture` issues in the test repo, closes them, and removes their `ProjectV2Item` cards from the board (close-only would leave the cards visible — `close_issue` is a REST PATCH `state: closed`, not a delete, and Projects V2 keeps closed items as cards until `deleteProjectV2Item` is called). This is the deterministic safety-net for the panic-path Drop guards in the test files, which fire-and-forget cleanup via `tokio::spawn` and silently skip when the runtime is torn down before the spawned task is polled (bead `unblock-ekf` documented best-effort caveat).
4. **Project views are NOT auto-cleaned — they are reused instead.** GitHub Projects V2 exposes no public API to delete a project view (no `deleteProjectV2View` in GraphQL, no `DELETE /views` endpoint in REST; verified against the v2 schema and 2026-03-10 REST OpenAPI). The three view-creation tests (`create_view_board_and_list_views`, `create_view_table_layout`, `create_view_roadmap_layout`) reuse fixed canonical names — `test-board-fixture`, `test-table-fixture`, `test-roadmap-fixture` — and check for pre-existence before calling `create_view`. View count on the test project stays bounded at exactly 3 fixtures across all runs. If a fixture view drifts (wrong layout, accidental rename), an operator must delete it manually via the GitHub Web UI; the next live test run will recreate it from canonical params.
5. **Repo labels are wiped before every CI run too.** The live test surface is pinned to exactly two canonical labels — `unblock-fixture` (the wipe anchor applied to every fixture issue) and `unblock-test-label` (the per-test discriminator used by `e2e_workflow` and the `ensure_labels` integration test). The CI live job invokes `scripts/setup-test-project.sh --wipe-labels` after the issues wipe; it enumerates all repo labels, deletes any whose name matches the orphan test patterns (`e2e-test-*`, `test-label-*`, `unblock-run-*`), and preserves the two canonical labels plus every production label that doesn't match a test pattern. Cycle 1 of bead `unblock-1hz` had introduced a per-run `unblock-run-<millis>` label; cycle 2 dropped it because the per-run label was accumulating in the test repo (~7 occurrences per CI run, no upstream bulk-delete API) — the same accumulation problem that drove the views fixed-name refactor, applied to labels.

To bootstrap a fresh project (or recover from a partial run) — custom-fields wipe:

```bash
scripts/setup-test-project.sh <owner> <project-number>
```

The script is idempotent: it lists the project's fields, deletes any that match the 6-name list, and exits 0 with no changes when the project is already clean. It does not touch the built-in Status field. Requires `gh` authenticated against the target owner with `project` + `repo` scopes, and `jq`.

To wipe accumulated fixture issues + their project board cards (run before or after a manual live test session):

```bash
scripts/setup-test-project.sh --wipe-issues <owner> <project-number> <repo>
```

The third positional `<repo>` is the bare repository name (not `owner/repo`). The mode lists every OPEN issue in `<owner>/<repo>` carrying the canonical `unblock-fixture` label, closes them (via `gh issue close`), and removes their corresponding `ProjectV2Item` cards from `<project-number>` (via the `deleteProjectV2Item` GraphQL mutation). Idempotent: a no-op when no open fixture issues exist. View cleanup is not performed — see invariant 4 above.

To wipe accumulated orphan test labels (run before or after a manual live test session, or as a one-time backlog cleanup):

```bash
scripts/setup-test-project.sh --wipe-labels <owner> <repo>
```

The `--wipe-labels` mode lists every label in `<owner>/<repo>` and deletes those whose name matches one of the orphan test glob patterns (`e2e-test-*`, `test-label-*`, `unblock-run-*`). The two canonical labels (`unblock-fixture`, `unblock-test-label`) are preserved by name even if their name matches a pattern, and every production label that doesn't match a test pattern is preserved as well — the deletion criterion is pattern match AND not-canonical, so a real label called `test` (or any operator-managed label) is left intact. Idempotent.

The three mutating modes are orthogonal — to perform multiple wipes, run the script multiple times. `--wipe-issues` does NOT trigger custom-fields cleanup or label cleanup, the default custom-fields mode does NOT touch issues or labels, and `--wipe-labels` does NOT touch issues or fields. This keeps the existing `--check` semantics on the custom-fields path unchanged and avoids the ambiguity of a combined `--all` flag.

To assert the project is in the canonical clean state without mutating it (useful as a CI preflight or local sanity check), pass `--check`:

```bash
scripts/setup-test-project.sh --check <owner> <project-number>
```

`--check` (or its alias `--dry-run`) lists any stale unblock-managed custom fields and exits **non-zero (4)** when drift is present, exits 0 when the project is already clean. Re-run without `--check` to actually delete them. `--check`, `--wipe-issues` and `--wipe-labels` are pairwise mutually exclusive.

The live test job runs `cargo tarpaulin ... -- --ignored --test-threads=1` to serialise the `#[ignore]` integration tests. Without the serialisation guard, the two test files that both call `setup_fields` against the shared test project (`crates/unblock-github/tests/integration.rs::setup_fields_creates_all_seven_fields` and `crates/unblock-mcp/tests/e2e_workflow.rs::e2e_workflow_all_10_tools`) race and either the second mutation collides on `createProjectV2Field` (`Name has already been taken`) or one test deletes options the other still needs. Local runs of `cargo test --workspace -- --ignored` should pass `--test-threads=1` for the same reason.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.
