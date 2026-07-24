---
name: reference-cargo-test-failed-is-red-not-compile-error
description: In a Verify mutation-matrix script, cargo's `error: test failed, to rerun pass ...` means a test BUILT+RAN+FAILED (genuine RED) — not a compile error; a classifier grepping `^error:` mislabels RED as compile-error and can hide a vacuous test.
type: gotcha
---

When scripting the project's recurring **mutation-matrix** non-vacuity proof (perturb production → prove
the target test goes RED → `git checkout` revert), `cargo test` prints **`error: test failed, to rerun
pass '<...>'`** whenever a test binary **compiled, ran, and a case FAILED** — i.e. that IS the RED you
want. A classifier that greps a bare `^error:` (or `^error(\[|:)`) mislabels this genuine RED as a
"compile error / bad mutation", which in the worst case masks a truly VACUOUS test as inconclusive.

Classify correctly:
- **True compile error** (bad mutation, inconclusive): `error[E....]`, `could not compile`, `cannot find`, `error: expected`.
- **Genuine RED** (non-vacuous ✓): non-zero exit AND (`test result: FAILED` OR `panicked at`). Capture the
  panic/assertion line as evidence.
- **Vacuous** (bad — the property broke but the test stayed green): exit 0.

Bit me on T3.2 (PR #395): the first mutation run labeled all 5 mutations "COMPILE ERROR"; re-running with
the corrected classifier showed all 5 as genuine RED with the exact assertion messages. Also: capture
clippy exit status directly (`cargo clippy ...; rc=$?`) — piping to `tail` then `&& echo OK` masks a
clippy failure behind `tail`'s exit 0. Related discipline: [[feedback-implementer-probe-must-include-cargo-fmt]].
