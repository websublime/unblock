---
name: feedback-derived-counts-die-in-prose-plans
description: A long prose plan cannot carry derived counts — four consecutive generations in this repo shipped a wrong done-gate number while the design passed every time. Make the gate a script.
type: gotcha
---

In the ub-lp9.12 planning lineage (2026-07-21), **four consecutive plan generations** shipped a wrong derived
count or sweep token — r1 a width-bounded regex, the Review gate "~28 hits", r2 repeating it, r3 two more
(`L25=13` where the command gives 14; a sweep quoting the count of a *different* command). The design core
passed adversarial verification every round. It was never the engineering; it was always a number.

**Why:** each revision RE-TRANSCRIBES ~30 derived numbers into ~2000 lines of prose. Transcription is where
they die. Worse, a wrong count is invisible to review unless someone re-runs the exact command — and reviewers
read prose, they don't re-run greps.

**How to apply:**
- When a plan's done-gate is "zero live hits" or any count, write it as an **executable script**, not a
  sentence. That rule is a gate wearing prose.
- Never let a count appear as a literal expectation (`expect 14`) — it rots on the next code change. Assert a
  **set is empty**, or that a set **matches a derived set**.
- Encode exclusion rules as real filters in the script, not as a comment beside the number — an exclusion
  stated in prose is where the arithmetic broke twice.
- Every number in a plan carries the command that produced it, and the reviewer's job includes **re-running
  them all**. Add that to the check prompt explicitly; it caught three of the four.
- Ask the falsification pass whether the gate can **pass vacuously** (a too-narrow grep, an allowlist that
  swallows real hits, an unpropagated exit code). "Does it fail on main" is the weaker question.

Related: [[reference-d41-doc-cascade-trap-classes]],
[[feedback-rename-zero-live-hits-needs-git-grep-w-allfiles]]
