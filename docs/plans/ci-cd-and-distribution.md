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

| Job | Gate | NFR |
|---|---|---|
| `fmt` | `cargo fmt --check` | — |
| `clippy` | `cargo clippy --workspace --all-targets -- -D warnings` (pedantic; `forbid(unsafe_code)`) | NFR-9, NFR-15 |
| `test` | `cargo test --workspace` incl. the `Storage` **contract suite** + proptest | NFR-16 |
| `snapshots` | `cargo insta test --check` (stable output shapes) | NFR-14, NFR-16 |
| `layering` | acyclic-crate-graph assertion (no back-edges; storage = model+error only) | NFR-15 |
| `audit` | `cargo audit` (advisories; catches e.g. an archived retry crate) | NFR-3, NFR-9 |
| `deny` | `cargo deny check` (licenses, bans, sources; **no-git ban**: no git crate in tree; transitive budget) | NFR-6, NFR-9, NFR-10 |
| `fuzz-smoke` | short `cargo fuzz` run on ingestion targets | NFR-16 |
| `bench-gate` | `criterion` baselines with a 10% regression gate (perf-sensitive paths) | NFR-1 |
| `scale` | 250k-issue corpus under the single-serve topology (D14) | NFR-2 |
| `contention` | **the contention lab** — assert no 100% CPU hot-spin (libsql WAL + busy_timeout) | NFR-3 |
| `toolchain` | pin `rust-toolchain.toml` to **stable `1.96.0`** and build the workspace with `--locked`; a green stable build (no nightly-only features) is the gate. Fails if any crate requires nightly. | NFR-12 |
| `no-network` | **workspace-wide no-network symbol-scan** — assert no networking symbols (`reqwest`, `hyper`, `std::net`, raw TLS) link into any crate except the `self-update`-feature-gated `axoupdater` surface in `unblock-cli`; spot-checks the default-feature binary too. | NFR-17 |
| `rate-limit` | rate-limit / stress assertions for the single-serve gate (NFR-18 rate-limit half) and the long-lived interleaved-write stress harness (NFR-5); owned here in CI (harness file cross-ref impl-plan T3.5). | NFR-5, NFR-18 |
| `doc-lint` | **doc-corpus consistency lint** (see below) — catches the D-id / FR-tier / command-token / stamp / cross-ref / doc-count drift classes. | — |

- **Action pinning (NFR-9):** every `uses:` is pinned to a 40-char commit SHA, with an action-pins inventory and a network-free local verifier. This applies to **both** the hand-authored CI and the dist-generated release workflow (post-process / pin the generated `uses:` lines; re-pin on `dist` upgrades).
- **`Cargo.lock` committed** (NFR-9).

### 2.1 `doc-lint` — doc-corpus consistency lint

A mechanical lint over the `docs/` corpus (PRD.md, the `docs/plans/*` set, the 12 crate plans + fuzz) that fails CI on the drift classes seen in the consolidated review. It checks:

- **(a) D-id coherence** — each `D-id` (D1..D17) appears with the **same** version tag, packaging, and verification scheme everywhere it is referenced (would have caught D12-vs-D17 self-update drift).
- **(b) FR/NFR tier coherence** — each `FR-id`/`NFR-id` resolves to a PRD definition and carries a **consistent tier** (v1/v1.1/v1.2/v1.3) across all docs (would have caught the FR-25 version drift).
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
| NFR-5/18 | `rate-limit` job: single-serve rate-limit assertions (NFR-18) + long-lived interleaved-write stress harness (NFR-5). |

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
