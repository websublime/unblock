# Releasing unblock

A release of unblock is **a pushed version tag**. Pushing `vX.Y.Z` (or a pre-release `vX.Y.Z-rc.N`)
fires the `dist`-generated `.github/workflows/release.yml`, which builds and publishes the artifacts.
`dist` **requires the tag to equal** the root `Cargo.toml` `[workspace.package]` `version` — a
mismatched or malformed tag is the one manual step that can break or prematurely publish a release.

The `cargo xtask release` helper automates the tag step behind a strict, human-operated safety model
so you never have to hand-craft the version bump, commit, tag, and push.

> **Authoritative detail:** [`docs/plans/ci-cd-and-distribution.md`](docs/plans/ci-cd-and-distribution.md)
> §3 (release/dist) and §4 (self-update). This runbook is the operator's how-to; §3/§4 are the spec —
> do not let them drift.

## 1. Prerequisites

Before cutting a release:

- You are on **`main`** with a **clean working tree** (`git status --porcelain` is empty).
- Local `main` is **in sync with `origin/main`** (neither ahead nor behind).
- The repo has a **`WS_GH_TOKEN`** secret with `contents: write` scope. The dist-generated
  `release.yml` uses `${{ secrets.WS_GH_TOKEN }}` for its publish steps because the org restricts the
  default `GITHUB_TOKEN`. A secret cannot be verified from the client, so `cargo xtask release` only
  prints a reminder — confirm it is set before you push. (The `actions/attest` provenance step is
  unchanged; it uses the workflow OIDC identity, not the PAT.)

The pre-flight below enforces the first three; the token is your responsibility to have configured.

## 2. `cargo xtask release`

```sh
cargo xtask release            # interactive, guarded, cuts the tag
cargo xtask release --dry-run  # preview only — no edit, commit, tag, or push
```

This helper is **developer tooling, not a CI job**: it never runs unattended and pushes nothing on its
own.

### Flow

1. **Pre-flight** (aborts before any mutation): HEAD is `main`; the working tree is clean;
   `git fetch origin` then local `main` == `origin/main` (refuses if ahead or behind). It also prints
   the `WS_GH_TOKEN` reminder.
2. **Prompt — release type:** pre-release (`rc`) or final.
3. **Prompt — version bump:** none / patch / minor / major.
4. **Compute the new version** (see below), **guard the tag** (the computed tag must not already exist
   locally **or** on the remote — abort if it does; this too runs before any mutation), and **show the
   plan** (current → new version, tag, pre-release?, files changed = `Cargo.toml` + `Cargo.lock`), with
   an IRREVERSIBLE-push notice.
5. **(real run) Apply.** The first typed-tag confirmation (see
   [Typed double-confirmation](#typed-double-confirmation)) gates this whole sequence — a mismatch
   aborts with nothing touched. Then, in order:
   - set the `[workspace.package]` `version` key in `Cargo.toml` (the **only** place the version
     changes — never the `cargo-dist-version` pin or any dependency pin);
   - `cargo update --workspace` to refresh `Cargo.lock`;
   - commit, staging **only** the two release files (`git add -- Cargo.toml Cargo.lock`), so a stray
     path can never enter the public release commit;
   - create an annotated tag;
   - a single `git push --atomic origin main <tag>` — both refs advance or **neither** does, so
     `origin/main` can never be published without its tag, and a non-fast-forward race aborts both.

### Version compute

Strip any existing pre-release, apply the core bump (`none` keeps the current core; `patch`/`minor`/
`major` do the usual semver increment, zeroing lower parts). For a **pre-release**, attach `rc.N` where
`N` is one past the highest existing `v<core>-rc.<N>` tag (else `1`). A final release carries no
pre-release. Examples from `1.0.0`: final/none → `v1.0.0`; final/minor → `v1.1.0`; pre-release/none
(no prior rc) → `v1.0.0-rc.1`.

### Typed double-confirmation

A real run demands the operator **type the tag exactly**, twice:

- **Once before any change** — a mismatch aborts with nothing touched.
- **Once more before the push** — a mismatch stops with the local commit + tag intact but nothing
  pushed. The push is called out as **IRREVERSIBLE** (it triggers the public dist release).

### Partial-failure recovery

The mutation path is ordered, and each step carries a remediation hint on failure, so an interrupted
release never leaves you with a raw backend error and no way back:

| Where it failed | State on disk | Recovery |
|---|---|---|
| Before the commit exists (partial version bump / lock refresh) | `Cargo.toml` / `Cargo.lock` changed, no release commit | `git checkout -- Cargo.toml Cargo.lock` |
| At the annotated tag (the release commit exists, no tag) | commit exists | `git reset --hard HEAD~1` |
| At the push gate, or during the atomic push (commit + tag exist, nothing pushed) | commit + tag exist | `git tag -d <tag>` then `git reset --hard HEAD~1` |

Because the push is a single `--atomic` publish of both refs, there is no "main pushed but tag missing"
half-release to recover from.

### `--dry-run`

Runs every read-only step (pre-flight, prompts, compute, tag-existence guard, plan) and stops before
any edit/commit/tag/push, printing the `[dry-run] would: …` plan; the working tree is left untouched.
Use it to preview the exact version and tag a real run would produce.

## 3. What `dist` produces per release

When the tag lands, `release.yml` (generated from `dist-workspace.toml [dist]`, dist `0.32.0`) builds
and publishes:

- Cross-platform **archives** for all five target triples (self-contained binary, no runtime system
  deps).
- **shell** (`curl … | sh`) and **powershell** (`irm … | iex`) installers.
- **SHA256 checksums** plus a machine-readable **`dist-manifest.json`** — the sole client-side
  verify-before-swap gate consulted by `unblock update`.
- **GitHub artifact attestations** (provenance) on every artifact, emitted by `actions/attest`
  (pinned to `v4.1.1` in `release.yml`), verifiable out-of-band via `gh attestation verify`
  (publish-side; not consulted on the auto-update path).
- A **GitHub Release** with notes and the **`axoupdater`** updater artifact that backs
  `unblock update`.

> The `verify-pins` CI gate (`cargo xtask verify-pins`) backstops the NFR-9 SHA-pins that `dist`
> regenerates into `release.yml` — re-apply and re-verify the 40-char SHA pins after any `dist`
> upgrade or regen.

## 4. The first GA (v1.0.0) cut

The **v1.0.0 tag-push and the one-time GitHub secrets/permissions setup are the maintainer's act — they
are human-gated and never automated.** GA graduation is decision D35 (semver stability applies from
GA: the MCP contract, CLI surface, and 0–8 exit codes are stable; a breaking change bumps to 2.0.0).

To cut GA once the release PR is merged and `main` is clean and synced:

1. Ensure the `WS_GH_TOKEN` secret (`contents: write`) and the workflow permissions are configured
   (one-time).
2. Run `cargo xtask release --dry-run` and confirm the plan targets `v1.0.0`.
3. Run `cargo xtask release`, choose **final** / **none** (release `1.0.0` as-is), and type `v1.0.0`
   at both confirmations.
4. Watch `release.yml` complete; verify the GitHub Release, the five archives, both installers,
   `dist-manifest.json`, and the attestations. The `releases/latest/download/` installer links in the
   README resolve once this release is published.

## 5. Reference

- Release / distribution pipeline (authoritative): [`docs/plans/ci-cd-and-distribution.md`](docs/plans/ci-cd-and-distribution.md) §3
- Self-update via axoupdater: [`docs/plans/ci-cd-and-distribution.md`](docs/plans/ci-cd-and-distribution.md) §4
- The helper source: [`xtask/src/release.rs`](xtask/src/release.rs)
- dist config: [`dist-workspace.toml`](dist-workspace.toml)
