---
name: 2026-09-30-proptest-custom-generator-flake
description: Fixing a model round-trip property test that flaked a required CI job on an unrelated tracker-only pull request (tracker ub-3yk) — the generator built open-enum Custom values from strings like "BuG" that no parse can produce, the wire folded them back to the known variant, and the fix routes the generated strings through FromStr and commits the CI seed as a regression.
type: run
date: 2026-09-30
branch: ub-3yk-proptest-custom-generator
pr: '450'
issues: [ub-3yk]
---

# Run — open-enum generator flake in the model round-trip proptest

## Context

Issue ub-3yk tracks a flaky property test in `unblock-model`. The required `snapshots (insta --check)` job
failed on pull request 449, which carries only a wiki run-report and the tracker re-export for ub-rjq and
changes no code. The run went off main at f521d63 on branch `ub-3yk-proptest-custom-generator`, stacked on
pull request 449's branch so the wiki index and the tracker re-export do not conflict. Pull request 450
carries it. The orchestrator implemented solo, because the change is test-only in one crate. One reviewer
agent ran the Verify gate and passed it with no findings.

## What & why

CI reported that `issue_json_roundtrip` in `crates/unblock-model/tests/proptest_model_roundtrip.rs` failed
with seed `cc d47eb77f…`. The minimal input carried `IssueType::Custom("BuG")`. It serialises to `"BuG"`, and
deserialisation folds it to `IssueType::Bug`, so the equality check fails.

The open enums fold known names case-insensitively in both `FromStr` and `Deserialize`
(`crates/unblock-model/src/enums/issue_type.rs`, `crates/unblock-model/src/enums/status.rs`). That fold is the
documented contract. The generators built `Custom(s)` directly from any `[a-zA-Z]{1,12}` string, so they
could produce a value that no parse reaches and then assert that it survives a parse. Each CI run draws
fresh seeds, so the flake could land on any pull request.

## Outcome

- The CI seed was committed as `crates/unblock-model/tests/proptest_model_roundtrip.proptest-regressions`, and it failed the unchanged test locally.
- `arb_status` and `arb_issue_type` now map their string arm through `Status::from_str` and `IssueType::from_str`.
- The replayed seed now yields `IssueType::Bug`, so it pins the case-fold path.
- After the fix the test passed, including a run at 20,000 cases, and `cargo fmt --check` and `cargo clippy -D warnings` were clean.
- The Verify reviewer returned PASS with no findings.
- Pull request 449 went green after its failed job was re-run.

Other suites build raw `Custom` values the same way and were left alone. The restore round-trip stays in
memory. The panic-safety suite never compares a generated value after a parse. The policy oracles use the
same variant identity or `as_str` string as the code they check.

## Gotchas

- A required job named for snapshot checks also runs the workspace test suite, so a proptest flake surfaces under a name that suggests a snapshot diff.
- Proptest prints `FileFailurePersistence::SourceParallel set, but failed to find lib.rs or main.rs` for integration tests, then saves the seed next to the test file anyway.

## Glossary

No session-local ids were used in this run.

## Links

- ub-3yk — the flaky generator this run fixed.
- ub-rjq — the tracker-only change whose pull request 449 exposed the flake.
- `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-model/tests/proptest_model_roundtrip.rs` — the fixed generators.
- `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-model/tests/proptest_model_roundtrip.proptest-regressions` — the committed CI seed.
- `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-model/src/enums/issue_type.rs` and `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-model/src/enums/status.rs` — the case-folding parse.
