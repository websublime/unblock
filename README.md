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

## CI live tests

The `test-mcp-live` job in `.github/workflows/ci.yml` exercises the live GitHub API path of `unblock-mcp` (the `#[ignore]` integration tests under `crates/unblock-mcp/tests/`). It runs only on first-party events — pushes to `main`, scheduled nightly builds, manual `workflow_dispatch`, and PRs whose head branch lives in this repository — so forked PRs never see the secret.

The job has a preflight step that fails fast with an actionable error if any of the three required repository settings is missing. To configure them on a fresh clone, run (with `gh` authenticated against `websublime/unblock`):

```bash
# Repository secret — fine-grained PAT or classic PAT with `repo` + `project` scopes
gh secret set UNBLOCK_TEST_TOKEN --body '<github-pat>'

# Repository variable — the owner/repo string the live tests should hit
gh variable set UNBLOCK_TEST_REPO --body 'websublime/unblock'

# Repository variable — the GitHub Project (Projects V2) number used by the e2e_workflow test
gh variable set UNBLOCK_TEST_PROJECT --body '<project-number>'
```

The `coverage` job (mock-only) runs on every PR and the `test-mcp-live` job covers both live execution and live coverage in a single tarpaulin pass on first-party events. Codecov merges the two reports under the `mock` and `live` flags; the >80% target in spec §13.2 applies to combined coverage.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.
