---
name: project-aarch64-windows-no-axoupdater-bit-rc1-release
description: v1.0.0-rc.1 release FAILED (aarch64-pc-windows-msvc): dist install-updater=true is global-only + axoupdater ships NO standalone updater for Windows ARM64 in ANY version → the 6th triple can't be built with the updater; other 5 pass
type: reference
---

The first real release run (tag `v1.0.0-rc.1`, GH Actions run 29486619269, 2026-07-16) failed on **one** of the 6 build-local-artifacts jobs: **`aarch64-pc-windows-msvc`**. The crate compiled fine (`Finished dist profile in 3m39s`); the failure is in dist's post-build updater step:

```
× A build was requested for aarch64-pc-windows-msvc, but the standalone updater isn't available for it.
  help: set `install-updater = false` in your config.
```

Confirmed facts:
- **axoupdater ships NO standalone updater binary for `aarch64-pc-windows-msvc` in ANY version** (checked latest 0.10.0 release assets: the `axoupdater-cli-*` set covers 8 targets — aarch64/x86_64 apple-darwin, aarch64/x86_64 linux-gnu, aarch64/x86_64 linux-musl, x86_64 windows-gnu, x86_64 windows-msvc — **Windows ARM64 is absent**). So a dist/axoupdater **version bump does NOT fix it**.
- `install-updater` in dist is **global-only** (config ref: "since 0.12.0 global-only") — it **cannot** be disabled per-target. So "keep 6 triples + updater on the other 5" is impossible via config.
- unblock's real self-update UX = the **embedded** `unblock update` command (`AxoUpdater::new_for("unblock-cli")` in [crates/unblock-cli/src/commands/update.rs](crates/unblock-cli/src/commands/update.rs), library + install receipt + `self_replace`), NOT the standalone `<app>-update` helper that `install-updater=true` bundles. The receipt is a base dist-installer artifact (cargo-dist ≥0.10.0); `install-updater` (0.12.0) only adds the redundant standalone helper.

Spec conflict this exposed (a **spine/ci-cd bug**): `docs/plans/ci-cd-and-distribution.md` §Locked + `dist-workspace.toml` declare **6 triples INCLUDING aarch64-pc-windows-msvc** AND `install-updater = true` — mutually impossible. Also stated in CLAUDE.md ("6 target triples"), PRD/roadmap, NFR-11.

**Update-to-latest does NOT help** (2026-07-16): dist 0.32.0 IS latest (repo already pinned there), axoupdater 0.10.0 IS latest; axoupdater's OWN dist-workspace.toml build matrix excludes aarch64-pc-windows-msvc (structural upstream gap, no tracking issue) — dist can only ship the updater targets axoupdater publishes.

**Option B (`install-updater=false`) is DISPROVEN — do NOT use it.** Verified empirically: generated the dist installers with install-updater=false (dist 0.30.2 local) AND read the pinned **0.32.0** `installer.sh.j2` template on GitHub — in BOTH shell + powershell, the install-receipt write is **gated inside `if INSTALL_UPDATER = 1`** (`echo "$RECEIPT" > "$RECEIPT_HOME/$APP_NAME-receipt.json"`). So install-updater=false writes NO receipt → `AxoUpdater::new_for("unblock-cli")` (the embedded `unblock update`) breaks on **ALL 6 platforms**, not just Windows. install-updater=true is therefore MANDATORY (it's what writes the receipt our embedded updater needs). The initial "receipts predate install-updater so independent" reasoning was WRONG — verify installer templates, don't assume.

**Conclusion: Option A is the ONLY correct fix** — drop `aarch64-pc-windows-msvc` (6→5 triples), keep `install-updater=true`; self-update stays intact on all 5 shipped targets. Win-ARM64 users run the x86_64-windows build under Win11 x64 emulation; native Win-ARM64 can return in v1.1+ IF axoupdater adds the target upstream. This is exactly cargo-dist's own recommended path and what axodotdev themselves ship.

Fix = spec-first (6→5 triples across ci-cd §Locked/§NFR-11/§Open-Items, CLAUDE.md, PRD/roadmap, README) → T3.6 branch → gates → PR → re-cut the release (delete/re-push tag or cut rc.2; human tag-push). See [[project-t3-6-release-pipeline-scope]].

**DONE (2026-07-16): PR [#419](https://github.com/websublime/unblock/pull/419) MERGED into main at `aec6765`** (branch `t3.6-drop-aarch64-windows`, commit 67f8f3a → rebase-merged; local main synced, worktree/branch cleaned). CI note: `bench-gate` flaked once on `cmp_ready_sort/250000` (589>500ms) — re-run cleared it (docs-only change can't affect perf); see [[reference-bench-gate-cmp-ready-sort-250k-flaky]]. Executed as: design Review gate (3 lenses, mint-D36 unanimous) → single worktree-isolated implementer (22-site spec cascade) → Verify gate (3 lenses, PASS_WITH_CHANGES; 1 prose nit fixed). D17 KEPT verbatim + D36 amendment note (freeze-the-decision convention, mirrors D14→D31), NOT rewritten. `release.yml` untouched (dynamic dist-plan matrix → verify-pins green). 10 files, all probes green (fmt/doc-lint/check-layering/`cargo test --workspace`). The release was later re-cut and GA v1.0.0 shipped (2026-07-20).
