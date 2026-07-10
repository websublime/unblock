# unblock — CI/CD & Distribution Plan

- **Status:** APPROVED (genuinely-deferred items remain in §7 Open Items)
- **Source of truth:** PRD APPROVED v1.1
- **Date:** 2026-06-19
- **Relates to:** PRD `docs/PRD.md` (NFR-9 supply-chain, NFR-11 portability, NFR-12 stable toolchain, NFR-17 self-update/signing, FR-25 self-update, D9 stable Rust), implementation plan §7 (cross-cutting CI).
- **Tooling:** [`dist`](https://github.com/axodotdev/cargo-dist) (formerly `cargo-dist`) for the release/distribution pipeline.

> **Locked decisions (this doc):** targets = mac + linux + windows on x86_64 **and** aarch64 (6 triples);
> installers = **shell + powershell**; **self-update via `axoupdater` shipped in v1**; signing = **GitHub
> artifact attestations**. (See PRD D17.)

---

## 1. Two pipelines

unblock has two distinct GitHub Actions pipelines:

1. **CI (quality gate)** — runs on every PR/push. Hand-authored. Enforces the PRD's quality/supply-chain NFRs.
2. **Release/distribution** — runs on a version tag. **Generated and maintained by `dist`** from
   `[workspace.metadata.dist]`. Builds cross-platform artifacts, installers, checksums, attestations, a GitHub
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
| `bench-gate` | `criterion` baselines with a 10% regression gate (perf-sensitive paths) | NFR-1 | **DEFERRED → T3.5** (no `benches/` suite until perf budgets land) |
| `scale` | 250k-issue corpus under the single-serve topology (D14) | NFR-2 | **DEFERRED → T3.5** (the 250k corpus harness is built at T3.5) |
| `no-network` | **workspace-wide no-network symbol-scan** — assert no networking symbols (`reqwest`, `hyper`, `std::net`, raw TLS) link into any crate except the `self-update`-feature-gated `axoupdater` surface in `unblock-cli`; spot-checks the default-feature binary too. | NFR-17 | **LANDABLE from T3.1 (D27/AD-6); full scan → T3.6.** T3.1 builds `unblock-cli` and lands the crate-scoped static gate `tests/no_git_gate.rs` (no `Command::new("git")`/git crate/network symbol on the default build; the only network dep is behind `self-update`); the workspace-wide CI `no-network` job + the default-feature binary + `axoupdater`/dist spot-check land at **T3.6** (needs the full dist surface). |
| `stress-integrity` | long-lived single-workspace stress (`crates/unblock-engine/tests/stress_longlived.rs` — a MODEST 10^3–10^4-op mixed run, NOT the 250k perf corpus, which is T3.5) + interleaved concurrent command-family integrity (`crates/unblock-engine/tests/interleaved_families.rs` — create/update/close vs export/import vs read) — INTEGRITY-only (no corruption / no partial write / linearizable; NO latency/throughput budget), with a default-CI-sized run + an `#[ignore]`-gated longer soak (NFR-5 stress/interleaving half). | NFR-5 | **DEFERRED → T3.4** (ships with the engine reliability gates + the `unblock-sync/src/atomic.rs` fault-injection seam) |
| `rate-limit` | single-serve rate-limit assertions (NFR-18 rate-limit half). | NFR-18 | **DEFERRED → T3.5** (rides the perf/scale corpus) |

> **The standalone `contention` NFR-3 job is folded into `storage-testkit` at M0** — the contention lab is the M0 exit gate (T0.8) and runs as `--test contention_lab` under `storage-testkit`; the separately-owned long-lived stress half is the deferred `stress-integrity` job (T3.4).

> **DEFERRED ledger (M0 scope of T0.9).** The 11 jobs marked **M0 (T0.9)** are authored now in `.github/workflows/ci.yml` (+ the nightly `fuzz-smoke.yml`). The five genuinely-deferred jobs — **`bench-gate` (→ T3.5)**, **`scale` (→ T3.5)**, **`no-network` (→ T3.1/T3.6)**, **`stress-integrity` (→ T3.4)**, **`rate-limit` (→ T3.5)** — depend on artefacts (a `benches/` suite, the 250k corpus harness, the `unblock-cli` binary + `axoupdater`/dist surface, the fault-injection/stress-integrity harness for T3.4, the rate-limit harness for T3.5) that do not exist until their gating task lands. They are listed at the top of `ci.yml` as a comment so the gap is visible, not silent.

- **Action pinning (NFR-9):** every `uses:` is pinned to a 40-char commit SHA, with a trailing `# vX.Y.Z` comment. This applies to **both** the hand-authored CI and the dist-generated release workflow (post-process / pin the generated `uses:` lines; re-pin on `dist` upgrades).
- **`Cargo.lock` committed** (NFR-9); all M0 build/test jobs run `--locked`.

### 2.2 Targeted features vs `--all-features` (D15/NFR-10 — the M0 gate must not link TLS)

`cargo tree -e features --all-features` resolves the libsql **`remote`** feature, which pulls `reqwest`/`hyper`/`rustls`/`hyper-rustls` into the build. Activating `--all-features` in the M0 quality gate would therefore compile the network/TLS surface that D15 keeps **off the default path** (NFR-10/NFR-17). So the testkit clippy/test steps use **targeted features** — `-p unblock-storage --features testkit` — which is verified TLS- and network-free (`testkit` pulls no deps). **Never `--all-features` in CI** until/unless the remote path is itself a tested target (v1.2).

### 2.1 `doc-lint` — doc-corpus consistency lint

`cargo xtask doc-lint` — a mechanical, offline, sub-second lint over a **fixed 19-file corpus** (`docs/PRD.md`; the six `docs/plans/*.md` — `00-roadmap`, `01-design-spine`, `README`, `STATUS`, `ci-cd-and-distribution`, `implementation-plan`; the 12 `docs/plans/crates/unblock-*.md` incl. fuzz). `docs/PROCESS.md` and `docs/plans/templates/*` are **out of corpus**. An existence-guard FAILs on any missing **or** unexpected corpus file (a smaller-than-expected corpus is a vacuous pass). Global guards: a `CommonMark` block-fence mask (all classes skip fenced lines), an inline-code-span index (class (c) fires only in-code), a never-finding glyph set (`● ◐ — ☑ ⊘ ☐`), and an approximate-number guard (`≈`/`~`). Findings are sorted `(file, line, class)` and emitted as `path:line: [x] msg` on stderr; clean ⇒ `doc-lint OK: 19 docs, 6 classes clean` on stdout, exit 0. It checks:

- **(a) D-id coherence** — each `D-id` (D1..D31) appears with the **same** version tag, packaging, and verification scheme everywhere it is referenced (would have caught D12-vs-D17 self-update drift). *(The D-set is PRD-§4-data-driven: the lint parses the defined ids from the `| **Dx** |` rows and resolves membership against that set — the range is documentation, not a hard-coded regex; adding D31 to PRD §4 extends the set with no lint code change.)*
- **(b) FR/NFR tier coherence** — each `FR-id`/`NFR-id` resolves to a PRD definition and carries a **consistent tier** (v1/v1.1/v1.2/v1.3/v1.4/v1.5) across all docs (would have caught the FR-25 version drift).
- **(c) command-token spelling** — every user-facing `unblock <cmd>` token is spelled identically across PRD, roadmap, README, this doc, and the cli plan (the canonical self-update token is **`unblock update`**; the `self-update` Cargo feature name is deliberately distinct, per CF-K).
- **(d) source-of-truth stamp** — the `PRD APPROVED vX.Y` stamp in every doc matches the PRD header revision (currently **v1.1**).
- **(e) cross-ref resolution** — every `§N.M` cross-reference resolves to an existing anchor.
- **(f) doc-count & RESOLVED claims** — the doc-count and "RESOLVED" claims in the README consistency report match the actual file set.

## 3. Release / distribution pipeline (`dist`) — at v1 GA

Driven by a version tag (e.g. `unblock-vX.Y.Z`); `dist` generates `.github/workflows/release.yml`.

### 3.1 dist configuration (`[workspace.metadata.dist]`)

```toml
[workspace.metadata.dist]
# Pin the dist version used in CI (managed by `dist init`/`dist generate`).
cargo-dist-version = "<latest>"
ci = ["github"]
installers = ["shell", "powershell"]
targets = [
  "x86_64-unknown-linux-gnu",  "aarch64-unknown-linux-gnu",
  "x86_64-apple-darwin",       "aarch64-apple-darwin",
  "x86_64-pc-windows-msvc",    "aarch64-pc-windows-msvc",
]
# Self-update surface (axoupdater) — ships in v1 (FR-25)
install-updater = true
# Provenance signing — GitHub artifact attestations (NFR-17)
github-attestations = true
```

> Exact key names track the pinned `dist` version (managed via `dist init`); the shape above is canonical.
> The single distributed binary is `unblock` (from `unblock-cli`).

### 3.2 What it produces per release
- Cross-platform archives for all 6 target triples (NFR-11: self-contained binary, no runtime system deps).
- **shell** (`curl … | sh`) and **powershell** (`irm … | iex`) installers.
- SHA256 checksums + a machine-readable `dist-manifest.json`.
- **GitHub artifact attestations** (provenance) on every artifact (NFR-17).
- A GitHub Release with notes; the `axoupdater` updater artifact.

## 4. Self-update via `axoupdater` (FR-25, v1)

- The hand-rolled `self_update` crate is **dropped** (supersedes the original's reqwest/TLS self-update stack).
- `dist` with `install-updater = true` provides the updater; unblock embeds **`axoupdater`** as a library so the
  lifecycle CLI exposes an **`unblock update`** command (D3: lifecycle/ops surface, not a domain feature). The
  command lands in `unblock-cli` behind the **`self-update`** Cargo feature (feature name ≠ command name, by
  design — CF-K; `--no-default-features` drops the `self-update` feature and with it the `unblock update` command).
- **Verification before execution (NFR-17):** updates are checked against GitHub artifact **attestations**; the
  updater refuses to install unverifiable artifacts. No network on any normal command path — only on explicit
  `unblock update` (offline-first preserved, D13).
- CI/release sets `AXOUPDATER_GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}` to avoid GitHub API rate limits.

## 5. Mapping to PRD NFRs

| NFR | How this plan satisfies it |
|---|---|
| NFR-9 supply-chain | CI: clippy/forbid, `cargo audit` + `cargo deny`, committed `Cargo.lock`, SHA-pinned actions (incl. dist-generated workflow). |
| NFR-10 minimize deps | `cargo deny` transitive budget; dropping the `self_update` reqwest/TLS stack in favour of `axoupdater`. |
| NFR-11 portability | 6 target triples, single self-contained binary; (optional musl for fully-static Linux — see Open Items). |
| NFR-12 stable toolchain | `toolchain` job pins `rust-toolchain.toml` to stable `1.96.0` and builds `--locked`; a green stable build is the gate (no nightly-only features). |
| NFR-17 self-update/signing | GitHub artifact attestations; `axoupdater` verifies before execution; no network on normal paths; `no-network` symbol-scan job enforces it workspace-wide. |
| NFR-1/2/3 | `criterion` gate, 250k scale job, contention lab in CI. |
| NFR-5 | `stress-integrity` job (T3.4): long-lived + interleaved-write integrity harness over the `unblock-sync` fault-injection seam. |
| NFR-18 | `rate-limit` job (T3.5): single-serve rate-limit assertions. |

## 6. Version placement

- **CI quality gate:** exists from **M0** (the contract suite + contention lab are M0 exit gates).
- **Release/distribution pipeline + self-update:** lands at **v1 GA** (end of M3). `dist` config is added once the
  `unblock` binary exists (after M3/T3.1).
- **Roadmap:** FR-25 self-update was moved **v1.1 → v1** (now via `axoupdater`); the `00-roadmap.md`
  feature-to-version matrix already reflects FR-25 in v1.

## 7. Open items
- **Homebrew tap** — add the `homebrew` installer + a `websublime/homebrew-tap` repo (deferred; shell/powershell cover v1).
- **npm installer** — attractive because MCP clients are often Node (`npx unblock`); revisit post-v1 with an `@websublime` scope.
- **macOS notarization** — attestations cover provenance; Gatekeeper notarization (Apple Developer cert) deferred unless a macOS GUI install path is wanted.
- **linux musl** — add `*-unknown-linux-musl` triples if a fully-static Linux binary is required by NFR-11.
- **dist version pin & action re-pinning** — pin `dist`; re-run the SHA-pinning verifier whenever `dist` regenerates the release workflow.
