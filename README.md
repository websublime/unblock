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
├── plans/                ← Per-crate implementation plans (numbered)
│   ├── 01-plan-unblock-core.md
│   └── ...
└── specs/                ← Per-component detailed specs (numbered)
    ├── 01-spec-core.md
    └── ...
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

The `docs-old/` directory contains the original documentation written before this structure was adopted. It is being migrated into the structure above.

| Original | Migration target |
|----------|-----------------|
| `unblock-prd-github.md` | `docs/PRD.md` |
| `unblock-prd-plugin.md` | `docs/PRD.md` (plugin phase) |
| `unblock-architecture-github.md` | `docs/SPEC.md` + `docs/specs/` |
| `unblock-architecture-plugin.md` | `docs/SPEC.md` + `docs/specs/` |
| `unblock-cicd-architecture.md` | `docs/specs/` (CI/CD spec) |
| `unblock-project-plan.md` | `docs/plans/` |
| `unblock-reconciliation-plan.md` | `docs/plans/` |
| `unblock-epic-agent-client-detection.md` | `docs/plans/` |
| `research/` | preserved as-is |
| `specs/` | merged into `docs/specs/` |

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.
