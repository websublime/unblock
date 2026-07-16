# unblock — CI/CD & Distribution Plan

- **Status:** APPROVED (genuinely-deferred items remain in §7 Open Items)
- **Source of truth:** PRD APPROVED v1.1
- **Date:** 2026-06-19
- **Relates to:** PRD `docs/PRD.md` (NFR-9 supply-chain, NFR-11 portability, NFR-12 stable toolchain, NFR-17 self-update/signing, FR-25 self-update, D9 stable Rust), implementation plan §7 (cross-cutting CI).
- **Tooling:** [`dist`](https://github.com/axodotdev/cargo-dist) (formerly `cargo-dist`) for the release/distribution pipeline.

> **Locked decisions (this doc):** targets = mac + linux on x86_64 **and** aarch64, windows on x86_64 (5 triples);
> installers = **shell + powershell**; **self-update via `axoupdater` shipped in v1**; signing = **GitHub
> artifact attestations**. (See PRD D17.)

---

## 1. Two pipelines

unblock has two distinct GitHub Actions pipelines:

1. **CI (quality gate)** — runs on every PR/push. Hand-authored. Enforces the PRD's quality/supply-chain NFRs.
2. **Release/distribution** — runs on a version tag. **Generated and maintained by `dist`** from
   `dist-workspace.toml` `[dist]`. Builds cross-platform artifacts, installers, checksums, attestations, a GitHub
   Release, and the `axoupdater` self-update surface.

## 2. CI pipeline (quality gate) — from M0

Runs on `pull_request` and pushes to the default branch. Jobs (all on stable `1.96.0`, per D9):

| Job | Gate | NFR | Lands |
|---|---|---|---|
| `fmt` | `cargo fmt --check` | — | **M0 (T0.9)** |
| `clippy` | `cargo clippy --workspace --all-targets -- -D warnings` (pedantic; `forbid(unsafe_code)`) **plus** a targeted `cargo clippy -p unblock-storage --all-targets --features testkit -- -D warnings` step so the feature-gated testkit/contract/contention code is lint-clean | NFR-9, NFR-15 | **M0 (T0.9)** |
| `test` | `cargo test --workspace` (the always-on unit + proptest + `behaviour.rs` set; the `testkit`-gated `contract.rs`/`contention_lab.rs` are `#![cfg(feature = "testkit")]` and compile to 0 tests here — they run in the dedicated `storage-testkit` job below) | NFR-16 | **M0 (T0.9)** |
| `storage-testkit` | `cargo test -p unblock-storage --features testkit --test contract` (the NFR-16 **contract suite**) **and** `--test contention_lab` (the M0 **contention gate**, `contention_lab_no_hot_spin_and_correct`). **Requires ≥ 2 vCPU** — the contention test hard-fails on a single-vCPU runner (writers cannot genuinely contend). **Targeted features, not `--all-features`** (see §2.2). | NFR-3, NFR-16 | **M0 (T0.9)** |
| `snapshots` | `cargo insta test --check` (stable output shapes) | NFR-14, NFR-16 | **M0 (T0.9)** |
| `layering` | `cargo xtask check-layering` — acyclic-crate-graph assertion (no back-edges; storage = model+error only), reading the committed `Cargo.lock` so `cargo metadata --offline` resolves | NFR-15 | **M0 (T0.9)** |
| `audit` | `cargo audit` (advisories; catches e.g. an archived retry crate) | NFR-3, NFR-9 | **M0 (T0.9)** |
| `deny` | `cargo deny check` (licenses, bans, sources, advisories; **no-git ban**: no git crate in tree; transitive budget) | NFR-6, NFR-9, NFR-10 | **M0 (T0.9)** |
| `toolchain` | pin `rust-toolchain.toml` to **stable `1.96.0`** and build the workspace with `--locked`; a green stable build (no nightly-only features) is the gate. Fails if any crate requires nightly. | NFR-12 | **M0 (T0.9)** |
| `doc-lint` | `cargo xtask doc-lint` — **doc-corpus consistency lint** (see §2.1) over the fixed 19-file corpus; catches the D-id / FR-tier / command-token / stamp / cross-ref / doc-count drift classes. | — | **M0 (T0.9)** |
| `fuzz-smoke` | short `cargo fuzz` run on the 8 ingestion targets, on a **scheduled** (nightly) workflow (`fuzz-smoke.yml`): nightly-`2026-04-01` (= rustc 1.96.0-nightly) + libFuzzer for the targets, plus a separate stable-1.96 step that runs the two `#[ignore]`d contention-lab controls (forced-spin, WAL-negative) to keep the M0 gate proven non-vacuous. Both controls are **core-independent** so they are non-flaky on the 4-vCPU runner: the forced-spin control asserts a busy-retry + CPU-burn (`cpu/wall`) hot-spin signature (not just `R > ceiling`), and the WAL-negative control drives a fixed write total — see `unblock-storage.md` and `STATUS.md` T0.8. Failure routing at M0 = just go red; `workflow_dispatch` allows a manual re-run; no issue-opening. **Repair note (post-T1.3):** this leg was effectively **DOA since T0.7/T0.9** — the former `nightly-2024-10-31` pin (cargo 1.84) predated edition 2024 (>= 1.85) + let-chains (>= 1.88) so the `unblock-*` tree could not parse, and the nested `fuzz/Cargo.toml` lacked an empty `[workspace]` table so `cargo fuzz` could not build it directly. Re-pinned to `nightly-2026-04-01` (>= the stable 1.96 target) + the `[workspace]` table added; the unwatched cron is now repaired. | NFR-16 | **M0 (T0.9)** — nightly schedule |
| `bench-gate` | HYBRID `criterion` gate (D34): a hard per-PR **generous absolute-ms** budget on a pinned ≥2-vCPU runner (`benches/storage.rs`, `benches/engine.rs` + the existing policy/render `criterion` benches wired into the SAME gate, F-7 — `cargo xtask bench-gate`) plus the **advisory/nightly 10% relative-regression report** (`cargo xtask bench-compare` vs the committed `xtask/bench-baseline.json`, homed in the `fuzz-smoke` `perf-advisory` leg, **report-only — never fails a PR/nightly**) | NFR-1 | **landed at T3.5 (P1)**; the read ceilings were **re-tightened + the advisory relative-10% leg landed at T3.5.1** (once batch hydration fixed the `collect_hydrated` N+1) |
| `scale` | 250k-issue corpus (storage-direct, validated but non-minted — D34) under the child-per-client topology (D14+D31); a **timed integration test** (`crates/unblock-storage/tests/scale.rs` + `crates/unblock-engine/tests/scale.rs`), NOT a `criterion` bench — per-PR with an explicit timeout + an `#[ignore]`-gated soak variant | NFR-2 | **owned by / lands at T3.5 (P1)** (the 250k corpus harness/`seed_corpus` is built at T3.5) |
| `no-network` | **workspace-wide no-network source-scan** — assert no networking symbols (`reqwest`, `hyper`, `std::net`, raw TLS, un-gated `axoupdater`) link into any crate except the whitelisted `self-update`-feature-gated `axoupdater` TLS path in `unblock-cli`; spot-checks the default-feature binary too. | NFR-17 | **LANDS at T3.6** (workspace-wide source-scan; whitelists the `self-update` axoupdater TLS path). T3.1 already landed the crate-scoped static tripwire `tests/no_git_gate.rs` (no `Command::new("git")`/git crate/network symbol on the default build; the only network dep is behind `self-update`); T3.6 adds the workspace-wide CI `no-network` xtask job (scans all `crates/*/src` + `xtask/src`) + the default-feature binary spot-check. |
| `stress-integrity` | long-lived single-workspace stress (`crates/unblock-engine/tests/stress_longlived.rs` — a MODEST 10^3–10^4-op mixed run, NOT the 250k perf corpus, which is T3.5) + interleaved concurrent command-family integrity (`crates/unblock-engine/tests/interleaved_families.rs` — create/update/close vs export/import vs read) — INTEGRITY-only (no corruption / no partial write / linearizable; NO latency/throughput budget), with a default-CI-sized run + an `#[ignore]`-gated longer soak (NFR-5 stress/interleaving half). | NFR-5 | **DEFERRED → T3.4** (ships with the engine reliability gates + the `unblock-sync/src/atomic.rs` fault-injection seam) |
| `rate-limit` | single-MCP-server rate-limit assertions (NFR-18 rate-limit half — the `Arc<Semaphore>` chokepoint + minted `RateLimited`, D34); a cheap, **independent** `-p unblock-mcp` leg (`crates/unblock-mcp/tests/rate_limit.rs`) | NFR-18 | **LANDED at T3.5 (P2)** |
| `feature-matrix` | `cargo build -p unblock-cli --no-default-features --locked` (proves `axoupdater`/`reqwest`/`hyper` are unreachable when the `self-update` feature is off — the AUTHORITATIVE confinement proof) **plus** the default-on leg; pinned checkout/toolchain(`1.96.0`)/rust-cache | NFR-10, NFR-17 | **lands at T3.6 (P2)** |
| `verify-pins` | `cargo xtask verify-pins` — fails if any `uses:` in `.github/workflows/release.yml` (or `ci.yml`) is not pinned to a 40-char commit SHA. `dist` CLOBBERS the generated pins on every regen, so this backstops the standing NFR-9 re-pin duty; it MUST cover `actions/attest@v4` (a floating major in dist's template). | NFR-9 | **lands at T3.6 (P1)** |

> **The standalone `contention` NFR-3 job is folded into `storage-testkit` at M0** — the contention lab is the M0 exit gate (T0.8) and runs as `--test contention_lab` under `storage-testkit`; the separately-owned long-lived stress half is the deferred `stress-integrity` job (T3.4).

> **DEFERRED ledger (M0 scope of T0.9).** The 11 jobs marked **M0 (T0.9)** are authored now in `.github/workflows/ci.yml` (+ the nightly `fuzz-smoke.yml`). The five genuinely-deferred jobs — **`bench-gate` (→ T3.5)**, **`scale` (→ T3.5)**, **`no-network` (→ T3.1/T3.6)**, **`stress-integrity` (→ T3.4)**, **`rate-limit` (→ T3.5)** — depend on artefacts (a `benches/` suite, the 250k corpus harness, the `unblock-cli` binary + `axoupdater`/dist surface, the fault-injection/stress-integrity harness for T3.4, the rate-limit harness for T3.5) that do not exist until their gating task lands. They are listed at the top of `ci.yml` as a comment so the gap is visible, not silent. **Landing status (as of T3.6):** `stress-integrity` landed at T3.4; `bench-gate` + `scale` landed at T3.5 (P1); `rate-limit` landed at T3.5 (P2); `no-network` + the two NEW `feature-matrix` and `verify-pins` jobs land at **T3.6** — so with T3.6 **all deferred CI jobs have landed** (the deferred ledger is empty). The per-job rows above carry the live status.

- **Action pinning (NFR-9):** every `uses:` is pinned to a 40-char commit SHA, with a trailing `# vX.Y.Z` comment. This applies to **both** the hand-authored CI and the dist-generated release workflow (post-process / pin the generated `uses:` lines; re-pin on `dist` upgrades).
- **`Cargo.lock` committed** (NFR-9); all M0 build/test jobs run `--locked`.

### 2.2 Targeted features vs `--all-features` (D15/NFR-10 — the M0 gate must not link TLS)

`cargo tree -e features --all-features` resolves the libsql **`remote`** feature, which pulls `reqwest`/`hyper`/`rustls`/`hyper-rustls` into the build. Activating `--all-features` in the M0 quality gate would therefore compile the network/TLS surface that D15 keeps **off the default path** (NFR-10/NFR-17). So the testkit clippy/test steps use **targeted features** — `-p unblock-storage --features testkit` — which is verified TLS- and network-free (`testkit` pulls no deps). **Never `--all-features` in CI** until/unless the remote path is itself a tested target (v1.2).

### 2.1 `doc-lint` — doc-corpus consistency lint

`cargo xtask doc-lint` — a mechanical, offline, sub-second lint over a **fixed 19-file corpus** (`docs/PRD.md`; the six `docs/plans/*.md` — `00-roadmap`, `01-design-spine`, `README`, `STATUS`, `ci-cd-and-distribution`, `implementation-plan`; the 12 `docs/plans/crates/unblock-*.md` incl. fuzz). `docs/PROCESS.md` and `docs/plans/templates/*` are **out of corpus**. An existence-guard FAILs on any missing **or** unexpected corpus file (a smaller-than-expected corpus is a vacuous pass). Global guards: a `CommonMark` block-fence mask (all classes skip fenced lines), an inline-code-span index (class (c) fires only in-code), a never-finding glyph set (`● ◐ — ☑ ⊘ ☐`), and an approximate-number guard (`≈`/`~`). Findings are sorted `(file, line, class)` and emitted as `path:line: [x] msg` on stderr; clean ⇒ `doc-lint OK: 19 docs, 6 classes clean` on stdout, exit 0. It checks:

- **(a) D-id coherence** — each `D-id` (D1..D36) appears with the **same** version tag, packaging, and verification scheme everywhere it is referenced (would have caught D12-vs-D17 self-update drift). *(The D-set is PRD-§4-data-driven: the lint parses the defined ids from the `| **Dx** |` rows and resolves membership against that set — the range is documentation, not a hard-coded regex; adding D36 to PRD §4 extends the set with no lint code change.)*
- **(b) FR/NFR tier coherence** — each `FR-id`/`NFR-id` resolves to a PRD definition and carries a **consistent tier** (v1/v1.1/v1.2/v1.3/v1.4/v1.5) across all docs (would have caught the FR-25 version drift).
- **(c) command-token spelling** — every user-facing `unblock <cmd>` token is spelled identically across PRD, roadmap, README, this doc, and the cli plan (the canonical self-update token is **`unblock update`**; the `self-update` Cargo feature name is deliberately distinct, per CF-K).
- **(d) source-of-truth stamp** — the `PRD APPROVED vX.Y` stamp in every doc matches the PRD header revision (currently **v1.1**).
- **(e) cross-ref resolution** — every `§N.M` cross-reference resolves to an existing anchor.
- **(f) doc-count & RESOLVED claims** — the doc-count and "RESOLVED" claims in the README consistency report match the actual file set.

## 3. Release / distribution pipeline (`dist`) — at v1 GA

Driven by a version tag (e.g. `vX.Y.Z`, the `dist` single-App default); `dist` generates `.github/workflows/release.yml`.

### 3.1 dist configuration (`dist-workspace.toml` `[dist]`)

```toml
# dist-workspace.toml [dist] — the canonical dist config location since dist 0.24.0
[workspace]
members = ["cargo:."]

[dist]
# Pin the dist version used in CI (managed by `dist init`/`dist generate`).
cargo-dist-version = "0.32.0"
ci = ["github"]
installers = ["shell", "powershell"]
targets = [
  "x86_64-unknown-linux-gnu",  "aarch64-unknown-linux-gnu",
  "x86_64-apple-darwin",       "aarch64-apple-darwin",
  "x86_64-pc-windows-msvc",
]
# Self-update surface (axoupdater) — ships in v1 (FR-25)
install-updater = true
# Provenance signing — GitHub artifact attestations (NFR-17), emitted by `actions/attest@v4`
github-attestations = true
# Per-artifact SHA256 checksum in dist-manifest.json — the SOLE client-side verify-before-swap gate
# (NFR-17); set explicitly in the P1 dist config so a future `false` cannot silently remove the gate.
checksum = "sha256"
# We SHA-pin every `uses:` in the generated `release.yml` (NFR-9). `allow-dirty = ["ci"]` makes dist
# (a) NOT fail `dist plan`/`build`/`host` on that drift — they run as steps INSIDE `release.yml`, so
# without it the real release would refuse — and (b) refuse to regenerate the CI, protecting the pins
# from being clobbered. `cargo xtask verify-pins` is the replacement freshness guard.
allow-dirty = ["ci"]

# MANDATORY force-include (every workspace crate is publish=false, so dist would otherwise ship ZERO
# apps): mark the shipped `unblock` binary in crates/unblock-cli/Cargo.toml —
#   [package.metadata.dist]
#   dist = true
```

> Exact key names track the pinned `dist` version (`0.32.0`, managed via `dist init`/`dist generate`); the
> config lives in `dist-workspace.toml` `[dist]` (canonical since dist 0.24.0). The single distributed binary
> is `unblock` (from `unblock-cli`), force-included via `[package.metadata.dist] dist = true` on that crate.
> `dist init` also adds a `[profile.dist]` (`inherits = "release"`, `lto = "thin"`) to the root `Cargo.toml`
> — REQUIRED, since the generated `release.yml` builds with `--profile dist`. dist derives the release App
> from the **package** name (`unblock-cli`), so archives/installers/the axoupdater install receipt are named
> `unblock-cli-*` even though the shipped binary is `unblock` (relevant to the P2 self-update receipt env).
>
> **P2 corrective (supersedes design-review MF-4 and the spec/plan §3 D7 `unblock` naming):** because the App-name
> is the package name `unblock-cli`, the install receipt is `unblock-cli-receipt.json`. P2's G1 self-update fix MUST
> call `AxoUpdater::new_for("unblock-cli")` (or `load_receipt_as("unblock-cli")`) and the D7 fixture MUST use
> `unblock-cli-receipt.json` / `AXOUPDATER_APP_NAME=unblock-cli` — the constructor + receipt name MUST equal the
> App-name P1 ships. (Miguel's GA branding ruling, 2026-07-15: accept `unblock-cli-*`, no package rename — dist
> 0.32.0 offers no app-name override.)
>
> **Hand-applied post-generation edits to `release.yml`** (protected by `allow-dirty = ["ci"]`; a `dist generate`
> would clobber them, so re-apply after any regen — `cargo xtask verify-pins` backstops the SHA-pins): (1) every
> third-party `uses:` is SHA-pinned to a 40-char commit (NFR-9); (2) the 5 publish-step `GH_TOKEN` envs use
> `${{ secrets.WS_GH_TOKEN }}` because the org restricts the default `GITHUB_TOKEN` (the `WS_GH_TOKEN` PAT needs
> `contents: write`). The `actions/attest` provenance step is UNCHANGED — it uses the workflow OIDC identity
> (`id-token`/`attestations: write`), not the PAT.

### 3.2 What it produces per release
- Cross-platform archives for all 5 target triples (NFR-11: self-contained binary, no runtime system deps).
- **shell** (`curl … | sh`) and **powershell** (`irm … | iex`) installers.
- SHA256 checksums + a machine-readable `dist-manifest.json`.
- **GitHub artifact attestations** (provenance) on every artifact, emitted by `actions/attest@v4` (NFR-17) — publish-side, verifiable out-of-band via `gh attestation verify`, not consulted on the auto-update path.
- A GitHub Release with notes; the `axoupdater` updater artifact.

### 3.3 `cargo xtask release` — interactive release helper

`dist` fires the §3.1 pipeline on a pushed version tag and REQUIRES that tag to equal the workspace
`[workspace.package]` version (root `Cargo.toml`). Pushing a mismatched or malformed tag is the one
manual step that can break — or prematurely publish — a release. `cargo xtask release` (`xtask/src/release.rs`)
automates that step behind a strict, human-operated safety model.

- **Flow.** pre-flight → prompt (release type: pre-release rc / final) → prompt (bump: none / patch /
  minor / major) → compute the new version → show the plan → (real run) edit the version key + refresh
  `Cargo.lock` (`cargo update --workspace`) → commit (staging ONLY `Cargo.toml` + `Cargo.lock`, via
  `git add -- Cargo.toml Cargo.lock`, so a stray path can never enter the public commit) → annotated
  tag → a single atomic `git push --atomic origin main <tag>` (both refs advance or NEITHER does, so
  `origin/main` can never be published without its tag, and a non-fast-forward race aborts both). The
  version is bumped in exactly ONE place — the `[workspace.package]` `version` key — never the
  `cargo-dist-version` pin or any dependency pin. New version = strip any existing pre-release, apply
  the core bump (none keeps the core), and for a pre-release attach `rc.N` where `N` is one past the
  highest existing `v<core>-rc.<N>` tag (else `1`).
- **Pre-flight (aborts before any mutation).** HEAD is `main`; the working tree is clean; `git fetch
  origin` then local `main` == `origin/main` (refuse if ahead/behind); the computed tag must not exist
  locally NOR on the remote. It also prints a reminder that the publish step needs a `WS_GH_TOKEN`
  secret with `contents: write` (a secret cannot be verified from the client, so this is a warning).
- **Typed confirmation.** A real run demands the operator TYPE THE TAG exactly — once before any change
  (mismatch = abort, nothing touched) and once more before the push (mismatch = stop with the local
  commit + tag intact but nothing pushed). The push is called out as IRREVERSIBLE.
- **Partial-failure recovery.** The mutation path is ordered (bump version → refresh `Cargo.lock` →
  commit → annotated tag → the atomic push) and each step carries a remediation hint on failure, so an
  interrupted release never leaves a raw backend error with no way back. The reachable half-states and
  their remediation: a failure BEFORE the commit exists (partial bump / lock refresh) →
  `git checkout -- Cargo.toml Cargo.lock`; a failure at the tag (the commit already exists) →
  `git reset --hard HEAD~1`; a stop at the push gate OR a failure during the atomic push (commit + tag
  exist, nothing pushed) → `git tag -d <tag>` then `git reset --hard HEAD~1`. Because the push is a
  single `--atomic` publish of both refs, there is no "main pushed but tag missing" half-release to
  recover from.
- **`--dry-run`.** Runs every read-only step (pre-flight, prompts, compute, guard, plan) and stops
  before any edit/commit/tag/push, printing the `[dry-run] would: …` plan; the working tree is left
  unchanged. Use it to preview the exact version + tag a real run would produce.
- **Relation to §2.** This helper is developer-tooling, NOT a CI job; it does not push anything on its
  own and never runs unattended. The `verify-pins` gate (§2) still backstops the SHA-pins `dist`
  regenerates into `release.yml`. NFR-9 (committed `Cargo.lock`) is preserved: the bump commit includes
  the refreshed lock. Semver stability from GA is D35's rule — the helper is the mechanical way to cut
  the tags that honour it.
- **Presentation (T3.8).** The helper's terminal output is styled with `anstyle` (already in the lock
  via clap → no new dependency): a header banner, colored pass/fail pre-flight checks, a boxed release
  plan (with the version diff + tag emphasized), a loud red IRREVERSIBLE banner before the typed gate,
  and colored success/abort lines. Color is decided ONCE from `stdout` (honouring `NO_COLOR`, `CLICOLOR`,
  `CLICOLOR_FORCE`) and auto-disables on a non-TTY or when `NO_COLOR` is set — styling is a pure
  presentation layer over the same output sink, so the safety model is UNCHANGED (numbered menus stay
  numeric; the typed-tag double-confirmation stays free-text; `--dry-run` mutates nothing; the pre-flight
  order and the atomic push are as above). The two slow steps (`git fetch`, `cargo update`) show a
  TTY-gated spinner that degrades to a single static line off a TTY; the spinner and all diagnostics go
  to **stderr**, structured output to **stdout** (NFR-14).

## 4. Self-update via `axoupdater` (FR-25, v1)

- The hand-rolled `self_update` crate is **dropped** (supersedes the original's reqwest/TLS self-update stack).
- `dist` with `install-updater = true` provides the updater; unblock embeds **`axoupdater`** as a library so the
  lifecycle CLI exposes an **`unblock update`** command (D3: lifecycle/ops surface, not a domain feature). The
  command lands in `unblock-cli` behind the **`self-update`** Cargo feature (feature name ≠ command name, by
  design — CF-K; `--no-default-features` drops the `self-update` feature and with it the `unblock update` command).
- **Verify-before-swap (NFR-17):** axoupdater runs the dist installer, which verifies each artifact's **SHA256
  checksum** against `dist-manifest.json` before `self_replace` swaps the binary — a mismatched/tampered download
  is refused and nothing is swapped. GitHub artifact **attestations** are publish-side provenance (`actions/attest@v4`,
  verifiable out-of-band via `gh attestation verify`), NOT consulted on the auto-update path. No network on any
  normal command path — only on explicit `unblock update` (offline-first preserved, D13).
- `AXOUPDATER_GITHUB_TOKEN` is a **client-runtime** env read by axoupdater on the user's machine (feeds
  `set_github_token` to avoid GitHub API rate limits) — it is NOT a release-workflow secret; the dist-generated
  `release.yml` publishes with the standard `${{ secrets.GITHUB_TOKEN }}`.

## 5. Mapping to PRD NFRs

| NFR | How this plan satisfies it |
|---|---|
| NFR-9 supply-chain | CI: clippy/forbid, `cargo audit` + `cargo deny`, committed `Cargo.lock`, SHA-pinned actions (incl. dist-generated workflow). |
| NFR-10 minimize deps | `cargo deny` transitive budget; dropping the `self_update` reqwest/TLS stack in favour of `axoupdater`. |
| NFR-11 portability | 5 target triples, single self-contained binary; (optional musl for fully-static Linux — see Open Items). |
| NFR-12 stable toolchain | `toolchain` job pins `rust-toolchain.toml` to stable `1.96.0` and builds `--locked`; a green stable build is the gate (no nightly-only features). |
| NFR-17 self-update/signing | publish-side attestations (`actions/attest@v4`, `gh attestation verify`); client verify-before-swap = the dist installer's SHA256 checksum (`dist-manifest.json`); no network on normal paths; the workspace-wide `no-network` symbol-scan job enforces confinement (whitelisting only the `self-update` axoupdater path). |
| NFR-1/2/3 | `criterion` gate, 250k scale job, contention lab in CI. |
| NFR-5 | `stress-integrity` job (T3.4): long-lived + interleaved-write integrity harness over the `unblock-sync` fault-injection seam. |
| NFR-18 | `rate-limit` job (T3.5): single-MCP-server rate-limit assertions. |

## 6. Version placement

- **CI quality gate:** exists from **M0** (the contract suite + contention lab are M0 exit gates).
- **Release/distribution pipeline + self-update:** lands at **v1 GA** (end of M3). `dist` config is added once the
  `unblock` binary exists (after M3/T3.1).
- **Roadmap:** FR-25 self-update was moved **v1.1 → v1** (now via `axoupdater`); the `00-roadmap.md`
  feature-to-version matrix already reflects FR-25 in v1.

## 7. Open items

> **Out of v1 GA (D35):** the homebrew tap, npm installer, macOS notarization, and linux-musl below are all
> deferred past v1 — shell + powershell installers across the 5 gnu/darwin/msvc triples cover the 1.0.0 GA.

- **Homebrew tap** — add the `homebrew` installer + a `websublime/homebrew-tap` repo (deferred; shell/powershell cover v1).
- **npm installer** — attractive because MCP clients are often Node (`npx unblock`); revisit post-v1 with an `@websublime` scope.
- **macOS notarization** — attestations cover provenance; Gatekeeper notarization (Apple Developer cert) deferred unless a macOS GUI install path is wanted.
- **linux musl** — add `*-unknown-linux-musl` triples if a fully-static Linux binary is required by NFR-11.
- **dist version pin & action re-pinning** — pin `dist`; re-run the SHA-pinning verifier whenever `dist` regenerates the release workflow.
