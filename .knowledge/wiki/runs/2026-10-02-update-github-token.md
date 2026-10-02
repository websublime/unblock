---
name: 2026-10-02-update-github-token
description: Making unblock update send the documented GitHub token (tracker ub-jh5) — the v1.0.1 live smoke hit a 403 rate limit because axoupdater reads no token env and unblock never called set_github_token; the fix reads AXOUPDATER_GITHUB_TOKEN, a hermetic token-gated mock proves it fails before and passes after, and a live measurement shows authenticated queries no longer spend the per-IP anonymous quota.
type: run
date: 2026-10-02
branch: ub-jh5-update-github-token
pr: '455'
issues: [ub-jh5]
---

# Run — `unblock update` sends the GitHub token it documented

## Context

`ub-jh5` was found during the `ub-lp9.26` acceptance run. The aarch64-apple-darwin leg of
`update-smoke.yml` (run 37020158075) failed twice at `unblock update --dry-run` with
`403 rate limit exceeded`, even though the workflow sets `AXOUPDATER_GITHUB_TOKEN`. That leg then
passed by hand on the maintainer's machine. Solo session; the branch is `ub-jh5-update-github-token`
off `main` 97ebcda (`release: v1.0.1`).

## What & why

These were read rather than restated here: ci-cd §4, `crates/unblock-cli/src/commands/update.rs`,
`crates/unblock-cli/tests/update_verify.rs`, and axoupdater 0.10.0 `src/release/mod.rs` and
`src/release/github.rs`.

ci-cd §4 already said the variable 'feeds `set_github_token`'. The code never did that, and the
axoupdater library reads no token env of its own; its only env reads are the app name, the receipt
paths and the base-URL overrides. So every release query went out unauthenticated, at 60 per hour
per IP, and shared egress such as CI runners or NAT exhausts that. Miguel ruled to fix it rather
than withdraw the claim. No new decision was needed, because the spec already described the
intended behaviour.

## Outcome

### What landed

- `update.rs` reads `AXOUPDATER_GITHUB_TOKEN` after the receipt loads. A non-empty value goes to
  `set_github_token`, which axoupdater sends as a bearer header on every GitHub API request: latest,
  list and tag. Unset, empty or whitespace-only means no token, so an exported-but-blank variable
  cannot send an empty bearer and get a 401.
- One new cell, `update_dry_run_authenticates_with_the_github_token_env`, uses a mock that answers
  only `Authorization: Bearer <token>` and gives 403 to anything else. The right token passes. Unset
  and blank are refused. A wrong token is refused, and it never appears on stdout or stderr. A small
  unit test pins the blank-means-none rule.
- The false claim is corrected in ci-cd §4, the `update-smoke.yml` env comment, and both smoke-script
  headers. All of them now say the variable only helps when `from_tag` already reads it, and that
  v1.0.0 and v1.0.1 ignore it.
- The error mapping is unchanged: a 403 or 401 is still `INTERNAL_ERROR`, exit 1, `retryable:false`.
  Moving it to `RATE_LIMITED` (exit 2) would change the stable CLI exit surface, so that question is
  left open on the issue.

### Measured

- With `update.rs` stashed back to `main`, the new cell FAILED ("with the token set, the gated query
  must succeed"). With the fix, it and the other 5 `update_verify` cells passed.
- Live against api.github.com, a binary rebuilt with the fix and a fabricated receipt at 1.0.0:
  three `--dry-run` with a real token left the anonymous core quota unchanged (used 15 → 15). Three
  without a token spent three (15 → 18). All six printed `update available: 1.0.1`. A bogus token
  gave `401 Unauthorized` (exit 1) and was not rendered.
- `cargo fmt --check`, clippy with `-D warnings` on `unblock-cli`, `cargo xtask no-network`, the
  `--no-default-features` build, `doc-lint`, `verify-pins`, `knowledge-lint`, every
  `scripts/checks/*.sh` and the knowledge-layer invariants all passed.

## Gotchas

- The first live measurement showed authenticated runs still spending anonymous quota. The binary
  was stale: the fails-before check (`git stash` of `update.rs` plus `cargo test`) had rebuilt
  `target/debug/unblock` without the fix, and `stash pop` does not rebuild. Rebuild before any live
  measurement that follows a stash-based negative control.
- A wrong token now fails with 401 instead of falling back to an anonymous query. That is the
  explicit-credential behaviour, but a stale token in a user's shell will surface as an update error.
- The CI smoke cannot benefit from the token until `from_tag` is a release carrying this read,
  because the querying binary is the installed `from_tag`. Until then, a rate-limited macOS leg must
  be re-run by hand (RELEASING.md section 5).

## Glossary

No session-local ids were used in this run.

## Links

- `ub-jh5` — `unblock update` never sent a GitHub token, so every query was unauthenticated and
  rate-limited.
- `ub-lp9.26` — the live update smoke whose acceptance run found it (comment 265).
- Pull request — 455 (`websublime/unblock`), opened 2026-10-02; merging is the human gate.
- Key files touched —
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-cli/src/commands/update.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-cli/tests/update_verify.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/docs/plans/ci-cd-and-distribution.md` (§4).
- Prior related run-report — `runs/2026-10-02-update-live-smoke.md`.
