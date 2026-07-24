---
name: reference-knowledge-glossary-scan-constrains-tracking-comments
description: The .knowledge arm-B temporal glossary lint scans the linked issues' comments — so every re-export of issues.jsonl on a branch must pair with a run-report glossary reconcile + lint green-check in the SAME commit, and issue comments must stay lean/prose to bound the burden.
type: gotcha
---

Miguel's Q3 choice for the `.knowledge` layer (arm B, hard + temporal) makes the committed run-report glossary responsible for **every session-local code in the linked issues' comments dated ≤ the report's date**. The `knowledge-lint` k6 check reddens the branch if any such code lacks a glossary row. Concrete consequences learned when the Verify gate landed (epic ub-knowledge-layer-e4s, 2026-07-23):

1. **Every `sync export` of `.unblock/issues.jsonl` on a feature branch must be paired, in the SAME commit, with a run-report glossary reconcile and a `cargo xtask knowledge-lint` green-check.** Re-exporting alone (to satisfy the same-commit tracker rule) silently reddens the branch if new comments coined codes. Operational loop: run the lint, add a glossary row for every token it reports as uncovered, repeat to green.

2. **The scan regex is `(^|[^A-Za-z0-9-])(MF|CF|M|R|F|A)-?[0-9]+([^0-9]|$)` — UPPERCASE prefixes only.** So `k1..k6`, `D5`/`D41` (D-ids), `Q1..Q5`, `T3.6` (T-ids) are SAFE (never scanned). But `R8`, `A-2`, `F1`, `M0`/`M3` (milestones), and any `MF-n`/`CF-n` DO match and demand a glossary row. Single-sourced const at `knowledge_lint.rs` / `run-report-gate.sh` / the ci-cd rule-1a text — pin equality.

3. **Keep issue comments LEAN and prose-first (decision 4).** Push code-dense narrative into the glossaried run-report / the Verify-verdict file that rides into it, NOT into issue comments. A code-free tracking comment adds zero glossary burden. This is the clarity rule ([[feedback-prose-to-miguel-expands-session-ids]]) and the k6 economics pointing the same way — write the durable per-task spine in words, park the codes where the glossary sits next to them.

4. **Code collisions are a real hazard:** the same token (`MF-1`) meant the bash-guard fix in the design-Review comments and the .gitkeep fix in the Verify comments — two referents, one token, in one scanned corpus. Avoid reusing a code across phases in tracked comments; prefer plain prose, or a continuous numbering with an explicit glossary disambiguation.

5. **Idea for the gardener / a helper:** a script that reads the committed jsonl, extracts every regex-matching token, and diffs against the run-report glossary rows would turn the manual reconcile into one command. Worth proposing when the gardener task (ub-knowledge-layer-e4s.5) is built.

Cousin lesson from the same gate: an implementer's "this deviation is narrow/safe" self-label is NOT authoritative — the Verify gate re-derived the `.gitkeep` exemption's claimed narrowness against the code and found it false (by-basename across all three dirs, no emptiness guard). Re-verify every safety self-label against the artifact — applies to deviation ledgers too.
