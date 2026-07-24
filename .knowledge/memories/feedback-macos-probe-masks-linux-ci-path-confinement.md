---
name: feedback-macos-probe-masks-linux-ci-path-confinement
description: macOS local/worktree cargo probe can pass while Linux CI fails for path-confinement tests using hardcoded /tmp paths (the /tmp→/private/tmp symlink); make such tests deterministic via explicit ..-traversal
type: gotcha
---

Implementer/Verify agents probe in **worktrees on macOS**; CI runs on **Linux**. For `unblock-sync` path-confinement tests this diverges: macOS `/tmp` is a symlink to `/private/tmp`, so a test whose session hardcodes a raw `/tmp/...` workspace (e.g. MCP `common::session()` `workspace_dir = "/tmp/unblock-mcp-test-ws"`, non-existent, `open_in_memory`) gets a **spurious PATH_TRAVERSAL** on macOS (confine_root canonicalizes to `/private/tmp/...` but the target stays `/tmp/...` → `starts_with` fails), while on **Linux `/tmp` is real** so the same export is confinable and **succeeds** — flipping a `assert!(is_error)` from pass→FAIL. This is exactly what reddened a PR's CI (`sync_export_surfaces_clean_in_band_error_when_path_unconfinable`) after every macOS probe (Implement + Verify + follow-up) reported green.

**Why:** `validate_sync_path` step-4 containment depends on filesystem canonicalization, which is platform-sensitive; hardcoded `/tmp` test paths bake in the macOS symlink quirk. Production is NOT affected (config discovery canonicalizes the workspace, so `unblock_dir` is always canonical) — it is a **test-only** artifact.

**How to apply:** (1) For a "path is unconfinable → PATH_TRAVERSAL" test, pass an explicit **`../`-escaping path** (e.g. `"../../../../../../etc/x.jsonl"`) — `validate_sync_path` rejects it **lexically** at the `had_parent_dir` guard (path.rs:68-90) BEFORE any FS access/canonicalize, so PATH_TRAVERSAL is deterministic on all platforms. NEVER rely on the default `/tmp` workspace path being "unconfinable". (2) Treat a Linux-CI-only failure after an all-green macOS probe as an **environment-dependent path/FS assumption** first. (3) The 11-job CI (Linux) is the real gate — a macOS worktree probe is necessary but not sufficient for FS/path-confinement code. See [[feedback-implementer-probe-must-include-cargo-fmt]].
