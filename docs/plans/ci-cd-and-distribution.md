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
| `doc-lint` | `cargo xtask doc-lint` — **doc-corpus consistency lint** (see §2.1) over the fixed 19-file corpus; catches the D-id / FR-tier / command-token / stamp / cross-ref / doc-count drift classes. Plus the knowledge-layer steps (§2.3): `cargo xtask knowledge-lint` (k1..k6, separate corpus) + `scripts/knowledge/tests/run-report-gate-selftest.sh` (the gate predicate's executable proof). | — | **M0 (T0.9)** |
| `fuzz-smoke` | short `cargo fuzz` run on the 9 ingestion targets, on a **scheduled** (nightly) workflow (`fuzz-smoke.yml`): nightly-`2026-04-01` (= rustc 1.96.0-nightly) + libFuzzer for the targets, plus a separate stable-1.96 step that runs the two `#[ignore]`d contention-lab controls (forced-spin, WAL-negative) to keep the M0 gate proven non-vacuous. Both controls are **core-independent** so they are non-flaky on the 4-vCPU runner: the forced-spin control asserts a busy-retry + CPU-burn (`cpu/wall`) hot-spin signature (not just `R > ceiling`), and the WAL-negative control drives a fixed write total — see `unblock-storage.md` and `STATUS.md` T0.8. Failure routing at M0 = just go red; `workflow_dispatch` allows a manual re-run; no issue-opening. **Repair note (post-T1.3):** this leg was effectively **DOA since T0.7/T0.9** — the former `nightly-2024-10-31` pin (cargo 1.84) predated edition 2024 (>= 1.85) + let-chains (>= 1.88) so the `unblock-*` tree could not parse, and the nested `fuzz/Cargo.toml` lacked an empty `[workspace]` table so `cargo fuzz` could not build it directly. Re-pinned to `nightly-2026-04-01` (>= the stable 1.96 target) + the `[workspace]` table added; the unwatched cron is now repaired. | NFR-16 | **M0 (T0.9)** — nightly schedule |
| `bench-gate` | HYBRID `criterion` gate (D34): a hard per-PR **generous absolute-ms** budget on a pinned ≥2-vCPU runner (`benches/storage.rs`, `benches/engine.rs` + the existing policy/render `criterion` benches wired into the SAME gate, F-7 — `cargo xtask bench-gate`) plus the **advisory/nightly 10% relative-regression report** (`cargo xtask bench-compare` vs the committed `xtask/bench-baseline.json`, homed in the `fuzz-smoke` `perf-advisory` leg, **report-only — never fails a PR/nightly**) | NFR-1 | **landed at T3.5 (P1)**; the read ceilings were **re-tightened + the advisory relative-10% leg landed at T3.5.1** (once batch hydration fixed the `collect_hydrated` N+1) |
| `scale` | 250k-issue corpus (storage-direct, validated but non-minted — D34) under the child-per-client topology (D14+D31); a **timed integration test** (`crates/unblock-storage/tests/scale.rs` + `crates/unblock-engine/tests/scale.rs`), NOT a `criterion` bench — per-PR with an explicit timeout + an `#[ignore]`-gated soak variant | NFR-2 | **owned by / lands at T3.5 (P1)** (the 250k corpus harness/`seed_corpus` is built at T3.5) |
| `no-network` | **workspace-wide no-network source-scan** — assert no networking symbols (`reqwest`, `hyper`, `std::net`, raw TLS, un-gated `axoupdater`) link into any crate except the whitelisted `self-update`-feature-gated `axoupdater` TLS path in `unblock-cli`; spot-checks the default-feature binary too. | NFR-17 | **LANDS at T3.6** (workspace-wide source-scan; whitelists the `self-update` axoupdater TLS path). T3.1 already landed the crate-scoped static tripwire `tests/no_git_gate.rs` (no `Command::new("git")`/git crate/network symbol on the default build; the only network dep is behind `self-update`); T3.6 adds the workspace-wide CI `no-network` xtask job (scans all `crates/*/src` + `xtask/src`) + the default-feature binary spot-check. |
| `stress-integrity` | long-lived single-workspace stress (`crates/unblock-engine/tests/stress_longlived.rs` — a MODEST 10^3–10^4-op mixed run, NOT the 250k perf corpus, which is T3.5) + interleaved concurrent command-family integrity (`crates/unblock-engine/tests/interleaved_families.rs` — create/update/close vs export/import vs read) — INTEGRITY-only (no corruption / no partial write / linearizable; NO latency/throughput budget), with a default-CI-sized run + an `#[ignore]`-gated longer soak (NFR-5 stress/interleaving half). | NFR-5 | **DEFERRED → T3.4** (ships with the engine reliability gates + the `unblock-sync/src/atomic.rs` fault-injection seam) |
| `rate-limit` | single-MCP-server rate-limit assertions (NFR-18 rate-limit half — the `Arc<Semaphore>` chokepoint + minted `RateLimited`, D34); a cheap, **independent** `-p unblock-mcp` leg (`crates/unblock-mcp/tests/rate_limit.rs`) | NFR-18 | **LANDED at T3.5 (P2)** |
| `feature-matrix` | `cargo build -p unblock-cli --no-default-features --locked` (proves `axoupdater`/`reqwest`/`hyper` are unreachable when the `self-update` feature is off — the AUTHORITATIVE confinement proof) **plus** the default-on leg; pinned checkout/toolchain(`1.96.0`)/rust-cache | NFR-10, NFR-17 | **lands at T3.6 (P2)** |
| `verify-pins` | `cargo xtask verify-pins` — fails if any `uses:` in `.github/workflows/release.yml` (or `ci.yml`) is not pinned to a 40-char commit SHA. `dist` CLOBBERS the generated pins on every regen, so this backstops the standing NFR-9 re-pin duty; it MUST cover `actions/attest@v4` (a floating major in dist's template). | NFR-9 | **lands at T3.6 (P1)** |
| `run-report-gate` | `scripts/knowledge/run-report-gate.sh "origin/${GITHUB_BASE_REF}"` — the structural substantive-PR predicate + run-report requirement (§2.3.3); PR-only, toolchain-free; dependabot exempt by PR-author identity; required check on `main` (flip sequenced at §2.3.3) | — | knowledge-layer landing PR (post-GA) |

> **The standalone `contention` NFR-3 job is folded into `storage-testkit` at M0** — the contention lab is the M0 exit gate (T0.8) and runs as `--test contention_lab` under `storage-testkit`; the separately-owned long-lived stress half is the deferred `stress-integrity` job (T3.4).

> **DEFERRED ledger (M0 scope of T0.9).** The 11 jobs marked **M0 (T0.9)** are authored now in `.github/workflows/ci.yml` (+ the nightly `fuzz-smoke.yml`). The five genuinely-deferred jobs — **`bench-gate` (→ T3.5)**, **`scale` (→ T3.5)**, **`no-network` (→ T3.1/T3.6)**, **`stress-integrity` (→ T3.4)**, **`rate-limit` (→ T3.5)** — depend on artefacts (a `benches/` suite, the 250k corpus harness, the `unblock-cli` binary + `axoupdater`/dist surface, the fault-injection/stress-integrity harness for T3.4, the rate-limit harness for T3.5) that do not exist until their gating task lands. They are listed at the top of `ci.yml` as a comment so the gap is visible, not silent. **Landing status (as of T3.6):** `stress-integrity` landed at T3.4; `bench-gate` + `scale` landed at T3.5 (P1); `rate-limit` landed at T3.5 (P2); `no-network` + the two NEW `feature-matrix` and `verify-pins` jobs land at **T3.6** — so with T3.6 **all deferred CI jobs have landed** (the deferred ledger is empty). The per-job rows above carry the live status.

- **Action pinning (NFR-9):** every `uses:` is pinned to a 40-char commit SHA, with a trailing `# vX.Y.Z` comment. This applies to **both** the hand-authored CI and the dist-generated release workflow (post-process / pin the generated `uses:` lines; re-pin on `dist` upgrades).
- **`Cargo.lock` committed** (NFR-9); all M0 build/test jobs run `--locked`.

### 2.2 Targeted features vs `--all-features` (D15/NFR-10 — the M0 gate must not link TLS)

`cargo tree -e features --all-features` resolves the libsql **`remote`** feature, which pulls `reqwest`/`hyper`/`rustls`/`hyper-rustls` into the build. Activating `--all-features` in the M0 quality gate would therefore compile the network/TLS surface that D15 keeps **off the default path** (NFR-10/NFR-17). So the testkit clippy/test steps use **targeted features** — `-p unblock-storage --features testkit` — which is verified TLS- and network-free (`testkit` pulls no deps). **Never `--all-features` in CI** until/unless the remote path is itself a tested target (v1.3).

### 2.1 `doc-lint` — doc-corpus consistency lint

`cargo xtask doc-lint` — a mechanical, offline, sub-second lint over a **fixed 19-file corpus** (`docs/PRD.md`; the six `docs/plans/*.md` — `00-roadmap`, `01-design-spine`, `README`, `STATUS`, `ci-cd-and-distribution`, `implementation-plan`; the 12 `docs/plans/crates/unblock-*.md` incl. fuzz). `docs/PROCESS.md` and `docs/plans/templates/*` are **out of corpus**. An existence-guard FAILs on any missing **or** unexpected corpus file (a smaller-than-expected corpus is a vacuous pass). Global guards: a `CommonMark` block-fence mask (all classes skip fenced lines), an inline-code-span index (class (c) fires only in-code), a never-finding glyph set (`● ◐ — ☑ ⊘ ☐`), and an approximate-number guard (`≈`/`~`). Findings are sorted `(file, line, class)` and emitted as `path:line: [x] msg` on stderr; clean ⇒ `doc-lint OK: 19 docs, 6 classes clean` on stdout, exit 0. It checks:

- **(a) D-id coherence** — each `D-id` (D1..D46) appears with the **same** version tag, packaging, and verification scheme everywhere it is referenced (would have caught D12-vs-D17 self-update drift). *(The D-set is PRD-§4-data-driven: the lint parses the defined ids from the `| **Dx** |` rows and resolves membership against that set — the range is documentation, not a hard-coded regex; adding D46 to PRD §4 extends the set with no lint code change — as D41, D42, D43, D44, D45 and D46 each did.)* **This string is one of the PROSE D-range bump sites** (the others are `CLAUDE.md`'s document map and the `xtask/src/doc_lint.rs` tokenizer comment, whose regex alternation moves with its prose) — **and the prose sites are only part of the cascade, which `docs/PROCESS.md` §3 states normatively as a LIST THAT CARRIES NO COUNT, all in the SAME commit** *(a derived count rotted here five times, the last of them into a number matching neither reading of its own enumeration; the enumeration IS the rule, and `scripts/checks/d46-schema-migration-claims.sh` pins every file the list names as carrying the live range, so the list cannot be off-by-one against itself)*: the prose sites PLUS the live-range knob of each shipped required check script (`scripts/checks/d44-create-deps-claims.sh` `RANGE_RE`; `scripts/checks/ub-lp9.25-dangling-blocker-claims.sh` `RANGE_RE` **and** `RANGE_ALT_RE`; and, since the D46 implementation commit, `scripts/checks/d46-schema-migration-claims.sh` `RANGE_RE` **and** `RANGE_ALT_RE`). **The prose sites are PINNED by EVERY one of those scripts**, whose knobs track the LIVE range and never a frozen historical one — so a bump that stops at the prose turns those required `doc-lint` steps red for reasons unrelated to their own decisions, and a bump that moves only one script's knob turns the others red. The commit that MINTS the id carries the edit at every file in that list which ALREADY EXISTS — the **SPEC** commit, not the implementation one, because a range bump is itself normative text; a script minted later carries the live range from its own first commit, as the D46 one did. **THIS FILE'S row is ROW-ANCHORED on the class-(a) statement above, not file-level, and the reason is a defect D45 actually hit:** a file-level token check on a document that also mentions the range in explanatory prose passes even when the NORMATIVE statement still carries the retired literal — the prose satisfies the check. So the row anchors on the class-(a) line, and this document quotes the LIVE range in exactly ONE place, that normative statement; every other reference to it here is by NAME ("the live range", "the `RANGE_RE` knob"), deliberately, so no prose occurrence can ever satisfy a pin on the normative one.
- **(b) FR/NFR tier coherence** — each `FR-id`/`NFR-id` resolves to a PRD definition and carries a **consistent tier** (v1/v1.1/v1.2/v1.3/v1.4/v1.5) across all docs (would have caught the FR-25 version drift).
- **(c) command-token spelling** — every user-facing `unblock <cmd>` token is spelled identically across PRD, roadmap, README, this doc, and the cli plan (the canonical self-update token is **`unblock update`**; the `self-update` Cargo feature name is deliberately distinct, per CF-K).
- **(d) source-of-truth stamp** — the `PRD APPROVED vX.Y` stamp in every doc matches the PRD header revision (currently **v1.1**).
- **(e) cross-ref resolution** — every `§N.M` cross-reference resolves to an existing anchor.
- **(f) doc-count & RESOLVED claims** — the doc-count and "RESOLVED" claims in the README consistency report match the actual file set.

**Named sub-check (NOT a 7th class — the six above are pinned by the `doc-lint OK: 19 docs, 6 classes clean` success line):** `scripts/checks/d43-argument-boundary-claims.sh` runs as a step of the SAME required `doc-lint` job. It is the D43 cascade's **executable zero-live-hits condition**, sweeping ALL TRACKED files with `git grep` (a sweep over `*.rs` misses dotfiles and config — that is how the last live hit survives a rename). Two forbidden claim families, both case-insensitive: an **unqualified** "strictly deserialized" boundary claim with no duplicate-key qualifier on the same line, and **stale residual framing** that still says the duplicate-key class is open. Every hit must be fixed or carry an explicit allow-list entry with a reason, and the check **self-tests that every allow-list entry still matches something** — a rotted exemption silently widens the blind spot, so it fails too. Exit 0 pass / 1 block / 2 cannot-evaluate, mirroring `scripts/knowledge/run-report-gate.sh`. **There is deliberately no `expect N hits` anywhere in it:** a derived count written into prose is exactly what this repo has watched rot silently, twice.

**Named sub-check (its D44 sibling, same required `doc-lint` job, the step immediately after):** `scripts/checks/d44-create-deps-claims.sh` is the D44 cascade's **executable zero-live-hits condition** (PRD §4 D44 — the one-transaction `issue create` with implicit edge ownership). It sweeps ALL TRACKED files with `git grep`, case-insensitively, for the framing D44 retires, and it does one thing its D43 sibling does not: it also asserts the **required landings**, because a removal is only proven once its replacement is pinned. Two rule kinds, both named per row so a failure says which claim survived. **Forbidden framing** comes in three modes: `plain` (every hit blocks — these are the specific retired sentences, and they get no escape precisely because a decision row is ONE physical line, so a single `D44` on it must not excuse them); `escape` (a hit is clean when the SAME line also names `D44`, which is how a reciprocal `SUPERSEDED by D44` sentence stays legible without re-opening the claim — the escape match is case-SENSITIVE, since D-ids are uppercase); and `subject` (a generic phrase counts only on a line that is also ABOUT the create-with-deps path, so unrelated prose does not trip it). The `F15` family escapes on ANY `D4x` rather than on `D44` alone, deliberately: it targets release-scope sentences claiming no v1.0.1 work touches `unblock-engine`, and forcing each such sentence to name the DECISION it scopes is the whole repair. **The superseded D22 clause is split across two families by grammar:** `F12` keys on the verb (`paths STAY/ARE unchanged`) and therefore needs a subject regex, since `path is unchanged` on its own is a true and unrelated sentence elsewhere in the tree; `F16` keys on verbless spellings — a section header, the restatement "the bulk path is additive", and a Rust test-fn IDENTIFIER named for the retired claim, under which the suite goes green while asserting the opposite of what shipped. **Required landings** are named presence predicates over named files — the decision row, both reciprocal cross-refs, the two strengthened acceptance criteria, the retired-in-place carve-out, the spine (including the `DepInput` shape it never defined), the task DAG, both roadmaps — the markdown one twice over, once by NAME anywhere in the file (which is all that predicate proves; it does NOT pin the v1.0.1 slot) and once at the LOCATED roadmap §9 crate-impact table row marking `unblock-engine` worked in that release — and `docs/roadmap.html`, which is OUTSIDE the 19-file doc-lint corpus, so nothing else in CI can catch it — the three crate plans, the three D-range sites, the `Storage` trait doc, the contract-version literals, and this specification plus the workflow wiring (so the gate cannot be wired-but-unspecified or specified-but-unwired). **The table deliberately reaches PAST documentation, and states why rather than leaving it to inference.** (i) *The fix itself must be reached.* Those landings are almost all prose, and a prose-only table goes fully green on a tree where only prose was rewritten — the cascade would certify itself — so the table also names the **code-side teeth**: the source-less `NewDep` create-edge carrier in `crates/unblock-engine/src/session/write.rs`, and in `crates/unblock-storage/src/libsql/crud.rs` both the RESTORED create-specific duplicate guard (by code) and its `D44` marker. Each of those three had ZERO matches before the change, so none can pass vacuously. (ii) *Every removal must pin its replacement* — the same rule the forbidden families carry, applied to `crates/**`. Two files lose text, and DELETING either would clear all of its forbidden hits at once, so each has affirmative pins: `crates/unblock-mcp/tests/dep_metadata.rs` (whose deletion would remove D44's ONLY end-to-end JSON-RPC coverage) must still assert the rejection CODE **and** the created issue's hydrated edge set — the half that proves the edge landed on the MINTED id; and `crates/unblock-sync/tests/contract.rs`, whose hits are all escapable, must carry the affirmative import-leg fact naming `create_issues` as the actual entry point, so a bare `D44` token cannot clear it with the sentence still false. (iii) *One landing is keyed to a FILE, not to a spelling.* `crates/unblock-engine/tests/create_bulk.rs` carries the superseded D22 clause several times over, including one occurrence split across a line break, so the table pins that FILE affirmatively: it must name `D44`. It had ZERO matches before the change. Finally the table asserts that **`ub-lp9.25` exists in the committed tracker record `.unblock/issues.jsonl`**: PRD, spine and roadmap all cite it as a v1.0.1 **co-requisite** (Miguel's 2026-07-30 ruling that it ships in the SAME 1.0.1 cut, precisely because D44 widens the exposure it closes), and a cited-but-nonexistent id is exactly how a co-ship commitment evaporates. **This paragraph is the gate's SPECIFICATION, so it is normative over the script:** every rule kind and every landing the script enforces is named here, and a landing that exists in one and not the other is a defect to fix in the same change — a maintainer reconciling the script against an out-of-date enumeration here would legitimately delete real teeth. **Two mechanical behaviours are part of that specification and must NOT be deleted as unspecified:** the script EXCLUDES ITS OWN PATH from the sweep (its regex literals are the rules themselves, not claims about the product — without the exclusion it flags every family it defines), and the `escape` mode carries a STATED limitation rather than a hidden one: `docs/PRD.md` §4 is ONE PHYSICAL LINE per decision row, so a `D44` anywhere in a row satisfies every escapable family on that line. That is intended — a reciprocal cross-ref row IS a historical record. The contract version lives in exactly ONE knob at the top of the script. It self-tests both directions: every allow-list entry must still match a real line (a rotted exemption silently widens the blind spot) and the family table must not have shrunk (an emptied table would make every scan a vacuous pass). The only allow-listed path is the generated tracker export `.unblock/issues.jsonl`, path-keyed because `sync export` rewrites it wholesale and it necessarily quotes this very defect's own issue text. Exit 0 pass / 1 block / 2 cannot-evaluate. **Two portability rules it must keep:** every `$( … )` substitutes a FUNCTION CALL (macOS `/bin/sh` mis-parses a `case` arm's `)` inside a command substitution and still exits 0 — a vacuous pass on a developer machine), and every variable expansion uses `printf '%s\n'`, never `echo` (POSIX-mode `echo` turns a `\b` in a regex literal into a backspace, silently unmatchable). Neither is a style choice.

**Named sub-check (its D45 sibling, same required `doc-lint` job, the step immediately after the D44 one):** `scripts/checks/ub-lp9.25-dangling-blocker-claims.sh` is the D45 cascade's **executable zero-live-hits condition** (PRD §4 D45 — the dangling dependency-TARGET guard on every edge-writing path, tracked as `ub-lp9.25`). **SEQUENCING, stated first because it is the easiest thing to get wrong:** the D45 cascade lands in TWO commits — a SPEC commit carrying only normative text, then an IMPLEMENTATION commit carrying the code. The script and its workflow step belong to the SECOND, because its contract-version and action-name landings assert against the `CONTRACT_VERSION` constant and the re-blessed golden snapshots; wiring it earlier would turn a required job red for a change that has not happened yet. This paragraph is written in the FIRST commit anyway, because the specification is normative over the script and the script is reconciled against it, never the reverse. It sweeps ALL TRACKED files with `git grep`, case-insensitively, and carries FOUR rule kinds, each row named so a failure says which claim survived. **Forbidden framing** (`N` rows) uses the same three modes as its two siblings — `plain`, `escape` (clean when the SAME line also names `D45`, case-SENSITIVE, so a reciprocal `SUPERSEDED by D45` sentence stays legible) and `subject` (a generic phrase counts only on a line about this defect). The framings it retires are: the UNDERCOUNT of the edge-writing paths (D45 closes FIVE, and the two the earlier framing never named are the `issue update {parent}` reparent and `issue create_bulk`); the claim that this repair mints no decision id (it mints D45, because Miguel's 2026-07-31 ruling puts the guard in the SHARED per-record insert body and that REVERSES a shipped D44 clause, which `docs/PROCESS.md` section 3 makes ride a new id); the retracted premise that this is a class the SCHEMA should close (the column keeps no foreign key deliberately, because an external target is legitimate, so the repair is application-level and no schema change is authorised — the retired adjective is not repeated here, since that family is `plain` mode and every hit blocks, this specification included); the D44 shared-body hazard clause D45 reverses (each such family escapes on `D45`, so the historical record survives); and the two spine forward-references that declare the class OPEN. **A NEGATIVE sweep can never prove a count is gone** — a line wrapped between the numeral and the noun is unfindable in principle by any line-based regex — so every negative family is PAIRED with a positive landing, and the strongest positives are deliberately SPELLING-INDEPENDENT: they key on DURABLE IDENTIFIERS (`ub-lp9.25`, `D45`, `create_bulk`, `reparent`, `insert_issue_in_tx`, a table-row anchor) rather than on prose, because an identifier survives rewording and rewrapping while a sentence does not. **Required landings** (`P` rows, named presence predicates over named files) are: the PRD decision row and the reciprocal pointer on the D44 row; the three D-range sites at the LIVE range (named, never quoted here — quoting it in prose is exactly what made the sibling gate's own row on THIS file pass vacuously; see the class-(a) paragraph above), each pinned ROW-ANCHORED on its normative line rather than file-level, and at `xtask/src/doc_lint.rs` BOTH halves of that one line — the prose AND the tokenizer alternation — since pinning only the prose is exactly how that site rots into an undefined-D45 finding; the spine; the task DAG; the markdown roadmap by name AND at the LOCATED §9 crate-impact row marking `unblock-sync` worked in v1.0.1 (its cell is blank before D45, and the exporter repair makes that crate gain CODE); `docs/roadmap.html`, which is OUTSIDE the 19-file doc-lint corpus, so nothing else in CI can catch it; the six touched crate plans, two of them at a LOCATED row (`unblock-storage` must name `insert_issue_in_tx` on a `D45` line; `unblock-sync` must state the EDGE consequence, not merely the id); the spine's case-INSENSITIVE `external:` clause, which nothing in the tree defines before D45; the tracker record; and this specification plus the workflow wiring, so the gate cannot ship wired-but-unspecified or specified-but-unwired. **One landing is a CI STEP rather than a document, and it exists because the deliverable would otherwise be green by NON-EXECUTION:** the `dangling` findings are composed in the ENGINE, while every testkit TEST step shipped today is storage-only (`cargo test -p unblock-storage --features testkit --test contract` and `--test contention_lab`; the engine testkit lines in the workflow are a clippy step and the `scale` job). A new engine testkit cell therefore executes in NO job. This specification is normative over the wiring: the cell is hosted as `crates/unblock-engine/tests/dangling.rs` behind the engine's EXISTING `testkit` feature (which already forwards to `unblock-storage/testkit`, so no new feature surface), and the required `storage-testkit` job gains the step `cargo test -p unblock-engine --features testkit --locked --test dangling`. The workflow edit rides the IMPLEMENTATION commit — the test does not exist yet, and wiring a step for a missing target turns a required job red for a change that has not happened — and the D45 gate carries the matching landing over `.github/workflows/ci.yml` so the step cannot be quietly dropped. **Two roadmap landings are deliberately CO-OCCURRENCE predicates rather than token checks:** BEFORE this cascade, `create_bulk` and `reparent` each already appeared in that file on rows unrelated to D45 (the counts are deliberately NOT written down — a derived count in prose is exactly what this repository has watched rot, twice), so each is required to appear on a line that ALSO names this cascade — a bare token check would have passed vacuously. **Row-anchored landings** (`Q` rows — EVERY line matching the anchor must also match the requirement) are the spelling-independent core: every line in the PRD, the spine and the markdown roadmap that names `ub-lp9.25` must also name `D45`. That single shape closes both spine forward-references and every roadmap citation at once, cannot be dodged by rewrapping, and — unlike a negative family — cannot be satisfied by deleting the sentence, since a deleted anchor is no longer a claim. **Contract-version landings** (`RC` rows, one shared knob at `unblock.mcp.v1.8`) use the same three selectors as the D44 sibling — PRESENCE where a file legitimately records the whole bump chain, `EXCLUSIVE` where a file makes present-tense claims and keeps no history (`README.md`, and the GENERATED `AGENTS.md`, which publishes the id to every agent in the workspace), ROW-ANCHORED for the owning crate plan's declaring row. **One `RC` row points at another gate and is a HARD BLOCKER:** the shipped D44 sub-check hard-codes its own contract knob, and its contract table demands that literal be present in four files — so the instant D45 bumps them, THAT required job turns red for a reason having nothing to do with D44 unless its one knob moves in the SAME commit. **The D-range couples the same way but lands EARLIER, and through a live-range knob in EVERY shipped required check script, not one:** the D44 sub-check pins the three prose range sites through its `RANGE_RE` knob, and THIS script pins them through its own `RANGE_RE` **and** `RANGE_ALT_RE` — which, with the prose sites, is the cascade `docs/PROCESS.md` §3 ENUMERATES — that list is the rule and deliberately carries NO count, because a count here rotted five times, the last time into a number matching neither reading of its own enumeration. EVERY one of those knobs tracks the LIVE range, so the **SPEC** commit — the one that bumps the range to the newly-minted id — moves all of them with it, while the contract knobs wait for the implementation commit. Both are one-line edits to a sibling gate, both are enumerated here so neither is rediscovered as a mystery failure, and they ride DIFFERENT commits precisely because one guards prose and the other guards a code constant. **The table reaches PAST documentation, for the same reason its D44 sibling does:** a prose-only table goes fully green on a tree where only prose was rewritten — the cascade would certify itself — so the gate also carries **code-side teeth**, one per guarded entry point plus the exporter, the shared `external:` predicate and the new `dangling` action (whose description is contract bytes TWICE, the `#[tool(description)]` wire literal and the `capabilities()` copy, which must agree). **Two further teeth were added on 2026-08-02, when the `dangling` view's composition was amended into ONE SQL read** (PRD §4 D45 clause (6) as amended; the two-read version measured 10.72 s at 250k rows and took `doctor()` over the `scale` job's 15 s boundedness guard): they pin the new read's **join clause** and its **`ORDER BY`**, in `crates/unblock-storage/src/libsql/diagnostics.rs`. **They exist as their OWN rows rather than as a reworded rationale on the engine row, and the reason is the general one this table is built on:** the engine row keys on the composition's ONE HOME (`fn dangling_findings`), which the amendment did NOT move — only the work behind it moved — so that row would go green on a tree where the home still exists and still does the slow thing. The two new rows cannot: both matched NOTHING before the amendment, since that file carried no join over `dependencies` at all. The join row names the mutant it kills — **appending a status term to the `ON` clause**, which reports every CLOSED and TOMBSTONED blocker as dangling and is the retired fully-inclusive-filters trap returning through a new door — and the behaviour behind it is pinned by two independent cells (the engine `dangling` cell and the NFR-16 contract case), both measured RED under that mutant. The `ORDER BY` row exists because the engine-side re-sort was DELETED with the amendment (a redundant sort would mask a broken `ORDER BY`), so the snapshot-stable finding order now rests on that SQL clause alone. Those rows are **commented-out STUBS in the spec commit and are uncommented in the implementation commit** — a gate that asserts conditions over unwritten code either fails for the wrong reason or gets quietly weakened until it passes — and each stub names the MUTANT it kills, because a coverage claim is worth nothing until a mutant proves it. **Every removal must pin its replacement, applied to `crates/**` too:** the one wire cell written to anticipate this change branches on `is_error` and would therefore PASS on its other branch once the refusal lands, silently ceasing to pin anything while its docstring became false prose in a green suite — so that FILE carries an affirmative pin requiring it to name `D45`. **The allow-list has exactly TWO entries and both are path-only: the wiki run-report archive `.knowledge/wiki/runs/`, and the generated tracker export `.unblock/issues.jsonl` — the latter for the NEGATIVE families only.** Per `docs/PROCESS.md` section 8 the wiki archive is DESCRIPTIVE and never normative — a run-report records how a PAST run went, so the D44 run-report legitimately still states that run's own scope, and a later correction never rewrites it. **The tracker export is allow-listed for the same reason its D44 sibling already allow-lists it, and the earlier draft of this paragraph asserted the opposite in bold — corrected here, because a negative family on that path could never go green.** `.unblock/issues.jsonl` is ONE JSON object per LINE, so an entire issue record — description, every comment, the close reason — is a SINGLE physical line; `docs/PROCESS.md` section 6 makes that comment thread the durable per-task narrative, and the thread for `ub-lp9.25` legitimately QUOTES the framings this cascade retires, including the record's own CORRECTIVE comment ("it is NOT three edge-writing paths: there are five"), which a negative family would itself flag. The file is also regenerated WHOLESALE by `sync export` and is never hand-edited, so it is a RECORD of how the task was reasoned about, not a live claim about the product. Demanding the retired sentences vanish from it would demand deleting the history the process exists to keep.

**The teeth on that path are therefore POSITIVE and spelling-independent, not negative — and this is where the enforcement actually lives.** Three `P` rows over `.unblock/issues.jsonl` require the record to name (i) the DECISION ID `D45`; (ii) all FIVE edge-writing paths, i.e. the corrected count rather than the retired three; and (iii) the two paths the retired count never named, by their DURABLE IDENTIFIERS — `create_bulk` and `reparent`. **The wrapping hazard that forces spelling-independence everywhere else does not exist on this path, and that is why a token predicate is sound here:** a JSONL record is one physical line by construction, so no requirement can be split across a line break and become unfindable in principle. A positive row also cannot be satisfied by DELETING the sentence, which is the failure mode a negative family has; and because the file is regenerated from the live issue, the way to satisfy these rows is to update the issue over the `issue` tool and re-export in the same commit — never to hand-edit the generated file. **This paragraph is the gate's SPECIFICATION, so it is normative over the script:** every rule kind and every landing the script enforces is named here, and a landing that exists in one and not the other is a defect to fix in the same change. **The mechanical behaviours the D44 sibling specifies are part of this specification too and must NOT be deleted as unspecified:** the script excludes its own path from the sweep, the `escape` limitation on one-physical-line PRD rows is stated rather than hidden, and it self-tests both directions — every allow-list entry must still match a real line, and no rule table may have shrunk below the counts it shipped with. Exit 0 pass / 1 block / 2 cannot-evaluate, and the same two portability rules apply verbatim (every `$( … )` substitutes a FUNCTION CALL; every variable expansion uses `printf`, never `echo`).

**AMENDMENT (same v1.0.1 cut, written when the script landed — five points where the enumeration above and the shipped script had to be brought into step, recorded here rather than left as silent drift, since a maintainer reconciling the script against an out-of-date enumeration would legitimately delete real teeth).** **(i) The code-side teeth carry their OWN row prefix `PC` and their own count floor,** rather than sharing the `P` numbering: they are the only rows that make this gate unable to go green on a tree where ONLY PROSE was rewritten, and a shared floor would let a newly-added prose row mask the deletion of a code-side one. Every table is therefore floored SEPARATELY, and the shipped counts are normative: **N = 5, P = 25, PC = 12, Q = 7, RC = 6.** **(ii) The three D-range sites are counted in the `Q` table, not the `P` table** — the paragraph above already requires them row-anchored, and a row-anchored rule belongs in the row-anchored table; `xtask/src/doc_lint.rs` contributes TWO `Q` rows, one per half of its single line (the prose range and the tokenizer alternation). **(iii) The tracker record carries FOUR `P` rows, not three:** requirement (iii) above names TWO durable identifiers (`create_bulk` and `reparent`), and they are split one per row so a failure says WHICH identifier vanished — a single conjunctive row would report only that "something" was lost. **(iv) Two `PC` rows sit slightly outside a literal reading of "one per guarded path plus the exporter, the shared predicate and the new action", and both are warranted by text already in this specification:** one pins the BULK RESOLVER's call to the shared predicate (`crates/unblock-engine/src/session/bulk.rs` — the site of the retired case-SENSITIVE dialect, so "the shared predicate landed" is not proven by its definition alone), and one pins that `crates/unblock-engine/tests/dangling.rs` EXISTS, because the CI-step landing above names that exact path and a step whose target is missing turns the required job red for the wrong reason. **(v) The one contract knob tolerates an OPTIONAL BACKSLASH between the id's segments** (`unblock\\?\.mcp\\?\.v1\\?\.8`). It is still ONE knob; the tolerance is forced by the `RC` row that pins the sibling D44 gate's own knob LINE, which spells the id as a REGEX literal with real backslash bytes — a pattern demanding a literal `.` there could never match it, and that row, whose whole purpose is to stop the sibling job going red in this very commit, would fail permanently for a reason unrelated to the id being current.

**Named sub-check (its D46 sibling — and the one that is DELIBERATELY SMALL, because most of D46's teeth are not a shell gate at all):** D46 (PRD §4 — the comments forward migration, tracked as `ub-lp9.13`) enforces its own class guard in RUST, not in a script: a const-evaluated digest over the embedded DDL + `CURRENT_SCHEMA_VERSION` + the migration ladder, asserted against a hand-blessed literal, plus a ladder-contiguity assertion. Both are `const` assertions, so editing the DDL without a version bump or a forward step is a **compile error** that fires under every job that builds — `fmt` aside, that is `clippy`, `test` and the testkit steps alike — with no annotation, `cfg` or `#[ignore]` able to silence it and no ordering dependency on any lint. **What a script must still carry is the one thing a compile-time assertion cannot: its own survival.** The negative question ("did anyone edit the DDL?") is unwritable, and the positive one is trivial — so the D46 row in the `scripts/checks/` family — **`scripts/checks/d46-schema-migration-claims.sh`, named here now that it exists (it landed with the implementation, per this paragraph's own sequencing rule, after the 2026-08-03 Verify gate found it specified-but-unwritten)** — is a REQUIRED-LANDING check asserting that the two `const` assertions still exist **each in the file it actually lives in: the LADDER-CONTIGUITY one in `crates/unblock-storage/src/libsql/migrations.rs` and the CONTENT-DIGEST one beside the DDL it hashes, in `crates/unblock-storage/src/libsql/schema.rs`** (this sentence used to say "the storage migrations module" for both, which named the wrong file for one of them), **plus the THIRD `const` assertion the same Verify gate added — the one binding the sentinel's witnessed column set to the ladder's NEWEST step, which is what makes the `Storage::migrate` contract's "the newest step's own columns" true as fact rather than as prose a step 3 would falsify** — alongside the usual positive landings (the decision id in `.unblock/issues.jsonl`, the bumped version constant, the NEW `BASELINE_SCHEMA_VERSION` constant that the frozen-baseline discipline introduces, and the step naming both post-baseline columns — the step's naming of them is the POSITIVE form of "the DDL no longer carries them", since the absence itself is unwritable as a check, and each column is pinned as the TUPLE LITERAL the ladder declares (a bare `updated_at` token would be satisfied by the file's own prose about a step that had been deleted, which is the vacuous-pass shape this repo has watched rot twice) — **and, for D46 clause (10), the identifier `schema_version_before_migrate` present in `crates/unblock-config/src/context.rs` AND in `crates/unblock-cli/src/commands/migrate.rs`, a DURABLE-IDENTIFIER landing that survives rewording**: the negative form ("nobody re-sourced `schema_from` from `MigrateOutcome`") is the unwritable kind, and the two-file co-occurrence is what proves the value is both captured and consumed rather than captured and dropped). **AND — since the tracking commit that retired the bump-site COUNT from `docs/PROCESS.md` §3 — the ROW-ANCHORED half of this script pins the LIVE D-range at EVERY file that §3 list enumerates:** the prose sites (`CLAUDE.md`'s document map, this file's class-(a) statement, and the `xtask/src/doc_lint.rs` tokenizer comment in BOTH its halves — the prose range and the regex alternation) **and the `RANGE_RE` / `RANGE_ALT_RE` knob line of each shipped required check script, this one included**, each anchored on the knob line itself so the file's own prose about the knob cannot satisfy the pin. That is what makes the §3 enumeration self-checking: a bump that misses a file, or a list that omits one, turns this required step red instead of rotting silently — which a numeral has now done five times. **SEQUENCING, same discipline the D45 sibling above states and for the same reason:** the D-range knobs move with the SPEC commit (a range bump is normative text), while the script and its workflow step belong to the IMPLEMENTATION commit, because their landings assert against code that does not exist until then. **The same split governs D46's two CODE-COMMENT supersession sites** (`crates/unblock-storage/src/libsql/schema.rs`'s DDL comment and `crates/unblock-storage/src/libsql/migrations.rs`'s module doc, named in PRD §4 D46): they ride the IMPLEMENTATION commit, because the comments they correct sit on the very constants and DDL that commit rewrites — so a spec-only tree missing them is complete, not an unfinished cascade. A spec commit that shipped the script would turn the required `doc-lint` job red against its own tree. **THE CONTRACT KNOBS ARE THE OTHER HALF OF THAT SEQUENCING, and D46 hits them even though it is a storage decision — enumerated here so it is never rediscovered as a mystery failure.** D46 attaches a self-correction hint to the stale-schema failure, which moves `ErrorCode::SchemaMismatch`'s published `hint_shape` off `none` (spine §2.2) — a byte inside `capabilities().error_codes` — so `CONTRACT_HASH` is re-pinned and `CONTRACT_VERSION` bumps `unblock.mcp.v1.8` → `unblock.mcp.v1.9` (spine §5.4 ledger; additive, hence non-breaking under D35, and the fourth such bump in this same v1.0.1 slot). **TWO shipped required check scripts each hard-code the LIVE contract id through their own `CONTRACT_RE` knob** — the D44 sub-check and the D45 sub-check named above — and both include `EXCLUSIVE`/ROW-ANCHORED rows that fail on a STALE literal, not merely on a missing one. So the instant the publishing sites move (the code constant, its independent test pin, `README.md`, the generated `AGENTS.md` contract line and the owning crate plan's declaring row), BOTH knobs must move in that SAME commit or two required jobs go red for reasons having nothing to do with D44 or D45. Those knobs ride the **IMPLEMENTATION** commit, never this spec one, because the id in code is still `unblock.mcp.v1.8` until then — the exact inverse of the `RANGE_RE` coupling one clause above, and the reason the two are stated separately rather than as one rule.

### 2.3 Knowledge layer — format contract, knowledge-lint, run-report gate & hooks

The repo-public knowledge layer `.knowledge/` (memories + wiki run-reports/topics — descriptive, never
normative; process rules in `docs/PROCESS.md` section 8) is machine-enforced from day 1: failures BLOCK,
never warn — no manual bypass, no discretionary label. Three layers: (i) `cargo xtask knowledge-lint`
(§2.3.2), a step in the `doc-lint` job; (ii) the `run-report-gate` required CI job (§2.3.3) — every PR is
classified by a structural substantive-PR predicate, and a substantive PR must carry its wiki
run-report in the same commit/PR as the work; (iii) PreToolUse hooks (§2.3.4), which run the SAME predicate script before
`gh pr create`. CI is the unbypassable server-side floor; hooks are the early in-session net; no rule
exists only in a hook. §2.3.5 records the accepted residuals by name.

#### 2.3.1 Format contract — scaffold, slugs, frontmatter schemas, index grammars, consts

**Tree (exact; no other files or subdirectories are valid — enforced by the §2.3.2 structure guard and
k2):**

```
.knowledge/
├── memories/
│   ├── index.md                      # curated one-liner index of every memory (data, not prose)
│   └── <slug>.md                     # one atomic fact per file (frontmatter + body)
└── wiki/
    ├── index.md                      # categorized index: ## Runs + ## Topics (### <category>)
    ├── runs/
    │   └── <YYYY-MM-DD>-<slug>.md    # one run-report per significant session / team run
    └── topics/
        └── <slug>.md                 # operational runbooks (descriptive, never normative)
```

Layout is **flat** (no subdirectories inside `memories/`, `runs/`, `topics/`). Bootstrap minimum: both
`index.md` files exist. Skeleton entry lines in the grammar blocks below are ILLUSTRATIVE; seed indexes
ship with EMPTY entry lists (plus exactly the first run-report's entry under `## Runs`). Templates do
NOT live inside `.knowledge/` — they are `docs/plans/templates/run-report.md` / `topic-page.md`
(copy-don't-edit), keeping every lint corpus template-free.

**Slug & filename rules:** slug grammar `[a-z0-9][a-z0-9-]*` (kebab-case, ASCII); filename =
`<slug>.md`; run-report filename = `<YYYY-MM-DD>-<slug>.md` with frontmatter `name` = the full
date-prefixed stem. Frontmatter `name` MUST equal the filename stem (k5). Slugs are **immutable once
merged** — a rename is a retire-and-recreate (the slug is a future DB primary key).

**Index formats (index-as-data):** indexes are curated by hand but machine-checked — every non-entry
line is skeleton, every entry line obeys one exact grammar, link-text = target stem, and the one-liner
MUST equal the page's frontmatter `description` verbatim (trim-equal): the index is a projection of
page data, so drift is a lint failure (k1). `memories/index.md` is a flat list with an inline
backticked type token — one grammar per line, one parser, the type already data on the line:

```
# Memory index

One line per memory; the line's one-liner equals the memory's frontmatter description.

- [<slug>](<slug>.md) `<type>` — <description>
```

`wiki/index.md` has **exactly two H2 sections** (`## Runs`, `## Topics` — any other H2 is a k4
finding) with `### <category>` subheadings under `## Topics`, one per category **in use**, drawn from
the 7-value category enum; every topic is listed under the heading matching its frontmatter `category`
(k4). `## Runs` is newest-first by convention (ordering is NOT lint-checked):

```
# Wiki index

Descriptive only — never normative (PRD > spine > crate plans is unchanged).

## Runs

- [<YYYY-MM-DD>-<slug>](runs/<YYYY-MM-DD>-<slug>.md) — <description>

## Topics

### <category>

- [<slug>](topics/<slug>.md) — <description>
```

**Frontmatter schemas (per kind; deny-unknown per kind):** a `---`-fenced block starting at byte 0;
**flat scalar `key: value` lines only** (plus the one inline list below); nested/indented YAML = k3.
Unknown keys (per kind) = k3 — strict now, so the DB migration never meets surprise columns. No YAML
dependency: this shape is parsed with `regex` (cargo-deny transitive budget untouched). Memory pages
carry exactly `name` (== stem), `description` (== the index one-liner, verbatim), `type`. Run-reports
add `date` (== the filename date prefix, k3), `branch` (or `-`), `pr` (number or `-` — `-` at commit
time is valid: the PR number does not exist yet when the same-commit rule lands the report; backfill is
optional), and `issues` (inline flat list of `ub-*` ids; `[]` invalid — a run with no issue is not a
significant run; each id must resolve, k4). Topics add `category`.

**Canonical value consts** (owned by `xtask/src/knowledge_lint.rs`; extending an enum is a normal,
PR-visible, reviewed change to the lint const AND this section — never an ad-hoc new value; the
`CANONICAL_VERBS` precedent from doc-lint class (c)):

- memory `type`: `gotcha` (a trap/failure mode) \| `recipe` (a sequence that worked) \| `reference` (a
  stable tool/API/format fact) \| `environment` (a local/CI/runner fact). The 4-value set is
  deliberately descriptive-only — no decision/constraint kinds, which would invite normative content.
- wiki page `type` per dir: `run` (`wiki/runs/*`) \| `topic` (`wiki/topics/*`).
- topic `category` (7): `orchestration` \| `git-and-worktrees` \| `ci-and-quality-gates` \|
  `testing-and-benches` \| `release-and-distribution` \| `mcp-and-agents` \| `environment-and-tooling`.
- glossary empty-body sentinel (the template and the lint agree on ONE literal): the exact sentence
  `No session-local ids were used in this run.`

**Format contract (migration-friendliness; binding on any future change here):** markdown + flat
frontmatter only (frontmatter keys are future DB columns; `type` is the discriminator); slugs are
stable primary keys; indexes are data (a future importer parses them or regenerates them from
frontmatter with zero information loss); no content in `.knowledge/` may become load-bearing for any
normative process — the docs-in-DB migration must be able to lift it wholesale without touching
PRD/spine/plans. The referenced end-state is the roadmap §7 docs-in-DB row (process-knowledge storage —
distinct from the product "memory screen" DISCARDED at roadmap §5).

#### 2.3.2 Layer (i): cargo xtask knowledge-lint (checks k1..k6)

**Fit + separation guarantee.** The knowledge lint mirrors the doc-lint shape in a **sibling module**
`xtask/src/knowledge_lint.rs` (`knowledge_lint()` entry, testable `lint_at(root)` core, findings
sorted `(file, line, check)`) — it does NOT extend the 19-file corpus. Three separation invariants:
(1) the doc-lint corpus never contains a `.knowledge/**` path (pinned by a corpus-test assertion);
(2) `knowledge_lint.rs` reads only `.knowledge/**` plus the out-of-tree point-reads specified below;
neither module calls the other's class functions; (3) doc-lint classes a..f must NOT run over
`.knowledge` — a run-report legitimately quotes superseded ids, old tiers, dead command spellings, and
foreign session-local codes; descriptive history is not drift. The k-prefix keeps a knowledge finding
from ever reading as a doc-lint a–f finding. Helper reuse, not duplication: the fence mask and
code-span index are shared `pub(crate)` helpers, so a fenced example of an index entry or frontmatter
block can never create a phantom entry/page. Report lines (exact): green
`knowledge-lint OK: <N> pages, 6 checks clean`; red: each finding to stderr as
`path:line: [kN] message`, then `knowledge-lint: <N> findings (k1:<a> k2:<b> k3:<c> k4:<d> k5:<e> k6:<f>)`,
exit FAILURE.

**Corpus model + structure guard.** The knowledge corpus is **dynamic** (discovered by walking
`.knowledge/`), but the skeleton is fixed and guarded: `lint_at` returns `Err` (hard FAIL, before any
check runs) unless `memories/index.md`, `wiki/index.md`, `wiki/runs/` and `wiki/topics/` all exist —
message `knowledge structure incomplete — missing: <comma-list> (an absent skeleton is a vacuous pass;
FAIL)`. Unreadable files are also guard `Err`s. Empty entry lists are valid; a missing tree is not.
Content pages = `memories/*.md` (excluding `index.md`) ∪ `wiki/runs/*.md` ∪ `wiki/topics/*.md`.

**Out-of-tree point-reads (fully specified; a literal implementation must NOT fail open):** every read
resolves against `lint_at`'s root (no cwd dependence). An absent or unreadable `.unblock/issues.jsonl`
or `CLAUDE.md` — or any absent/unreadable repo-local member of the `@`-import closure — is a
structure-guard `Err` (message `knowledge structure incomplete — missing: <path> (out-of-tree read; a
vacuous k4 pass is not a pass; FAIL)`): a k4 that cannot read its inputs must block, never skip. Each
non-empty export line is parsed as a JSON object with `serde_json` (already a direct xtask dependency);
the record id is the **top-level `id` field only** — the real export nests `comments[].id` (numeric)
and `comments[].issue_id`, and ids also appear in prose, so a whole-line substring/regex grep is
FORBIDDEN. A line that does not parse as a JSON object with a string `id` ⇒ guard `Err` (corrupt
export, fail-closed); a `comments` field that is present but malformed — not an array, or a member
lacking a string `text` — is likewise a guard `Err` (an absent `comments` key = legitimately zero
comments). k4 issues-resolve = set-membership of each cited `issues:` id in the collected
top-level-id set — exact string equality, no prefix/substring matching. The `@`-import closure starts
at `CLAUDE.md`, collects `@<path>` references outside fence masks and code spans, resolves repo-local
targets against the root, and recurses with a visited set, max depth 5 (Claude Code's own import-hop
cap); home-dir and root-external targets are outside repo jurisdiction and are skipped. Any closure
member whose text `@`-imports a target whose ROOT-RESOLVED path lies under `.knowledge` (relative,
`./`-relative, and root-internal absolute spellings alike) ⇒ the k4 no-import finding below.

**Export retention invariant (a D5 export-contract rider):** *An id, once exported to
`.unblock/issues.jsonl`, persists in the export forever — PRD D12 reserves compaction fields in the
model, and any future export compaction MUST preserve every id record (tombstoned at most, never
dropped), or it is a breaking change to this gate.* Without it, k4 issues-resolve would create a
referential edge from immutable descriptive history into the live export with no stated guarantee.

**The six checks — conditions and exact messages.** All checks skip fenced lines and code spans.
Attribution: k1/k4 index-side findings carry the index file + entry/heading line; k3/k5/k6 findings
carry the page + offending line (line 1 for whole-file conditions); k2 findings carry the offending
file, line 1.

**k1 — index→file resolution** (fires in the index files):

| Condition | Exact message |
|---|---|
| entry link target does not exist | `index entry '<target>' does not resolve to a file` |
| two entries resolve to one target | `duplicate index entry for '<target>'` |
| target escapes the owning content dir (`..`, absolute, wrong dir) | `index entry '<target>' escapes its content dir` |
| link-shaped line fails the memories grammar | ``malformed index entry (expected '- [slug](slug.md) `type` — one-liner')`` |
| link-shaped line fails the wiki grammar | `malformed index entry (expected '- [name](path.md) — one-liner')` |
| link-text ≠ target stem | `index entry link-text '<text>' != target stem '<stem>'` |
| one-liner ≠ page `description` (trim-equal) | `index one-liner differs from the page's frontmatter description` |

**k2 — file→index (orphans) + structural strays:**

| Condition | Exact message |
|---|---|
| content page not listed exactly once in its index | `page not listed in <index>` |
| file under `.knowledge/` outside the skeleton + content dirs | `stray file '<path>' outside the content dirs` |
| non-`.md` file in a content dir | `unindexable non-markdown file '<path>'` |
| subdirectory in a content dir | `subdirectory '<path>' not allowed (flat layout)` |

(One tree-preservation exception, flagged at landing: a file named exactly `.gitkeep` directly inside a
content dir is skipped — git cannot represent an empty directory, and the seed `wiki/topics/` ships
empty; everything else non-md still fails k2.)

**k3 — frontmatter validity** (content pages; index files are exempt from frontmatter):

| Condition | Exact message |
|---|---|
| no frontmatter block at byte 0 | `missing frontmatter block ('---' ... '---')` |
| unterminated frontmatter | `frontmatter not closed with '---'` |
| required key absent (per kind, §2.3.1) | `frontmatter missing required key '<key>'` |
| empty value | `frontmatter key '<key>' has an empty value` |
| unknown key (per kind) | `frontmatter unknown key '<key>' (allowed for <kind>: <list>)` |
| non-flat / non-`key: value` line | `malformed frontmatter line (expected flat 'key: value')` |
| run `issues:` not an inline non-empty `[..]` list | `'issues' must be a non-empty inline list of ub-* ids` |
| run `date` ≠ filename date prefix | `frontmatter date '<d>' != filename date prefix '<p>'` |

**k4 — enum, category & value agreement:**

| Condition | Exact message |
|---|---|
| memory `type` not canonical | `type '<t>' is not a canonical memory type (gotcha\|recipe\|reference\|environment)` |
| wiki page `type` ≠ its dir's kind | `page in <dir> must have type '<expected>', found '<t>'` |
| topic `category` not canonical | `category '<c>' is not a canonical topic category` |
| `wiki/index.md` H2 set ≠ exactly `Runs`,`Topics` | `wiki index section '## <h>' is not canonical (expected exactly: Runs, Topics)` |
| `### <h>` under `## Topics` not canonical | `wiki index category heading '### <h>' is not canonical` |
| topic listed under H3 ≠ its frontmatter `category` | `'<target>' indexed under '### <heading>' but frontmatter says category '<c>'` |
| run/topic listed under the wrong H2 | `'<target>' indexed under '## <section>' but lives in <dir>` |
| memories-index inline type token ≠ page `type` | `'<target>' indexed with type '<tok>' but frontmatter says '<t>'` |
| run `issues:` id absent from `.unblock/issues.jsonl` | `run cites issue '<id>' not present in .unblock/issues.jsonl` |
| any file in the `CLAUDE.md` `@`-import closure `@`-imports `.knowledge` content | `<file> must not @-import .knowledge content (decision 10 — @-import closure)` |

The issues-resolve check is the cheap, offline anti-stub tie to the tracker (a report citing an
unregistered task fails until the task is registered and exported in the same PR). The no-import check
scans the **closure**, not `CLAUDE.md` alone: `@`-imports nest, so an `@.knowledge` line added to
`docs/PROCESS.md` (already imported by `CLAUDE.md`) would achieve the forbidden always-on import — and
that 1-line M-only edit is docs-class trivial, so no gate would see it; only this lint catches it,
permanently.

**k5 — slug/filename:**

| Condition | Exact message |
|---|---|
| filename stem fails the slug grammar | `filename '<file>' is not a valid slug ([a-z0-9][a-z0-9-]*)` |
| runs filename lacks the date prefix | `run filename must be YYYY-MM-DD-<slug>.md` |
| frontmatter `name` ≠ filename stem | `frontmatter name '<n>' != filename stem '<stem>'` |

**k6 — run-report mandatory sections** (decision 3):

| Condition | Exact message |
|---|---|
| any of the six H2s missing (`## Context`, `## What & why`, `## Outcome`, `## Gotchas`, `## Glossary`, `## Links`) | `run-report missing mandatory section '## <name>'` |
| a mandatory section has an empty body | `section '## <name>' is empty` |
| Glossary body lacks both ≥1 DATA row and the exact sentinel | `'## Glossary' must contain >=1 glossary DATA row or exactly: "No session-local ids were used in this run."` |

**"DATA row" defined (normative for this text AND the unit tests):** within the Glossary's contiguous
`\|`-table block, a DATA row is a `\|`-delimited line that is (a) **not the block's first row** (the
header), (b) **not a separator row** (every cell matches `^\s*:?-+:?\s*$`), and (c) **not an
all-placeholder row** (every cell empty or matching `^\s*<[^>]*>\s*$`). Under this definition the
template's own header, separator, and placeholder lines count as ZERO rows — a template-copied glossary
fails k6 instead of degrading the mandatory glossary to "the heading exists".

**k6 token-coverage rules (decision 3's "or in the run's issue comments" — hard, temporal; both rules
block, never warn):** two rules share the session-local-id pattern const `SESSION_LOCAL_ID_RE`,
normatively defined at §2.3.3 rule 1a (single-sourced; the Rust const and the gate script's sh const
must equal that literal — the §2.3.3 selftest pins the script side and this doc, a unit test pins the
Rust side):

1. *Report-body coverage:* every body token matching the pattern (fence mask + code spans applied)
   must have a Glossary DATA row whose id cell equals it — message
   `session-local id '<tok>' has no glossary row`.
2. *Comment coverage — temporally scoped:* for each id in the run's `issues:` list, collect that
   record's `comments[].text` bodies from `.unblock/issues.jsonl` (same parse as above) and apply the
   same token rule — message `session-local id '<tok>' (from issue '<id>' comments) has no glossary row`.
   **Temporal scope (normative):** the scan considers ONLY comments whose `created_at` is ≤ the
   report's date. The report's date source is its frontmatter `date:` field (== the filename date
   prefix, enforced by k3); the boundary is INCLUSIVE at end-of-day UTC — a comment stamped anywhere
   on the report's own date, including a timestamp exactly equal to the boundary, is IN scope. Later
   comments are OUT of scope by construction, so a frozen report never goes retroactively red when
   later phases post gate-verdict comments on the same issue; codes coined by a later comment are owed
   by THAT comment's own PR — rule 1a (§2.3.3) makes it substantive, so it brings its own report +
   glossary. Amending a report later (the gate's deliberate `A\|M` path) does not widen its scope:
   `date` stays pinned to the filename (k3/k5). A scanned comment member lacking a string
   `created_at`, or one whose value does not parse as a date/datetime, ⇒ structure-guard `Err`
   (fail-closed — never a silent skip).

Known tuning constraint (recorded — not a defeater): durable repo tokens (`M0`–`M3` milestones,
roadmap `R` rows) collide with the pattern; the remedy is tuning the ONE declared const (a reviewed,
self-gated change), never per-file discretion.

**CI wiring + budget:** offline, deterministic, sub-second, single pass per file (same budget class as
doc-lint) — **one added step** in the existing `doc-lint` job, directly after `cargo xtask doc-lint`
(a step in the existing job: identical always-on blocking property, one toolchain spin-up saved; the
gate of §2.3.3 IS its own job because it is PR-only and toolchain-free).

**Tests (the doc-lint proof pattern):** every planted fixture root ships the two out-of-tree stubs (a
minimal `CLAUDE.md` importing a stub `docs/PROCESS.md`, and a synthetic one-line export mirroring the
real record shape — nested numeric `comments[].id` + `issue_id` + the `created_at` timestamp the
rule-2 temporal scope reads). Unit tests: ≥1 planted violation per check k1..k6 + no-false-positive
guards (fenced example frontmatter/index entries are skipped; `index.md` needs no frontmatter; a valid
sentinel-glossary run passes); the DATA-row fixtures (header+separator only → finding;
all-placeholder row → finding; one real DATA row → pass; exact sentinel → pass); the issues-resolve
fixtures (a resolving id; a ghost id → finding; an id appearing ONLY in export prose → finding —
proves field-anchored resolution); the closure fixtures (a planted `@.knowledge` import in the fixture
`docs/PROCESS.md` → finding attributed there; the same text fenced → no finding); the guard fixtures
(absent export / absent `CLAUDE.md` / a non-JSON export line / an absent closure member → `Err`); the
token-coverage fixtures (a body token with no matching DATA row → finding; the same with the row →
pass; an in-scope comment coining an unglossaried token → finding; the SAME comment re-stamped AFTER
the report's date → NO finding — the temporal-boundary pin, equal-timestamp variant IN scope; a
scanned comment lacking `created_at` → `Err`); and the const-equality test (the Rust
`SESSION_LOCAL_ID_RE` const equals the gate script's literal). Integration
(`xtask/tests/knowledge_lint_corpus.rs`): `real_knowledge_is_green` (zero findings at the real repo
root, exercising the real point-reads) and `missing_skeleton_fails_the_structure_guard` (non-vacuity).
The corpus-separation pin lives in `xtask/tests/doc_lint_corpus.rs`.

#### 2.3.3 Layer (ii): the run-report-gate CI job — the substantive-PR predicate

The predicate is POSIX sh + git, deliberately not an xtask: the gate job runs with no toolchain, and a
PreToolUse hook cannot afford a cargo build. **ONE predicate file,
`scripts/knowledge/run-report-gate.sh` (0755), is the single source of truth executed verbatim by BOTH
callers** (this CI job and the pr-create hook — zero drift). Exit codes: `0` pass · `1` BLOCK · `2`
cannot evaluate (fail-closed); both callers treat non-zero as a block. Every knowledge script is
kebab-case, and the CI job + predicate file share the exact token `run-report-gate`.

**The predicate (normative).** Evaluated over the diff listing
`git diff --name-only --diff-filter=ACDM --no-renames <merge-base(base_ref, HEAD)>..HEAD` (+
`--numstat` for sizes). **Classification always runs on a `--no-renames` listing:** with rename
detection on, only the NEW name of a rename is listed, so a `git mv` of a repo doc into `.knowledge/`
would ride the neutral strip — under `--no-renames` a rename decomposes to D+A, each classified
fail-closed. Rename detection is kept ONLY where sizing/shape needs it: the rule-7 run-report numstat,
where it is the HARDER choice (a renamed old report surfaces as `R`, excluded from `A\|M`). **First
matching rule wins per file; anything unclassified is SUBSTANTIVE (fail-closed). No labels, no human
discretion, no bypass input of any kind.**

1. **Neutral strip** (a path class never makes a diff substantive; rule 1a below can):
   `.knowledge/**`, `.unblock/issues.jsonl`. Empty set after stripping AND rule 1a clean → **pass**
   (wiki-gardening, memory-curation, code-free tracker-export PRs). The strip is deliberately narrow —
   any other committed `.unblock` file is unexpected and fails closed; there is no `.gitignore`
   exemption either (unclassified → substantive).

   **1a. Comment-coining export trigger** (closes the export-only escape): the sessions that coin the
   MOST session-local codes (failed gate rounds, decide-only sessions) commit ONLY the export — so
   before the strip may pass a diff, the export's CONTENT diff is scanned: if an ADDED line contains a
   `"comments"`-bearing record carrying a token that matches `SESSION_LOCAL_ID_RE` and does NOT appear
   in the paired REMOVED line for the same record id (new-code detection; an added record with no
   removed pair counts all its matches as new), the diff is **SUBSTANTIVE** — it coins durable
   session-local codes, so the run-report (whose glossary duty covers the run's comments) is due in
   the same PR. **`SESSION_LOCAL_ID_RE` — the session-local-id pattern const, normatively defined
   HERE** (one value, three citation sites that must stay equal: this text / the sh const in
   `run-report-gate.sh`, pinned by the selftest below / the k6 token-coverage Rust const, pinned by a
   unit test): `(^|[^A-Za-z0-9-])(MF|CF|M|R|F|A)-?[0-9]+([^0-9]|$)` — the prefix set, optional
   hyphen, digits, non-word context on both sides. Pairing note: on a raw export line the JSON-syntax
   substring `"id":"<val>"` can only be a real string-valued key literally named `id`, so the first
   such match is the record id, used ONLY as the new-vs-old pairing key; a mis-pairing errs toward
   firing (fail-closed). Known friction (fail-closed by design): durable tokens colliding with the
   pattern in a NEW comment make the PR substantive; tunable ONLY at the const (`scripts/**` →
   self-gated; residual R-F5, §2.3.5). Codes already present in the record's previous version never
   re-trigger.
2. **Pure dependency-bump shape:** every remaining path ∈ `{**/Cargo.toml, Cargo.lock}` → **pass**
   (covers human and dependabot cargo bumps structurally — no label).
3. **Always-substantive classes** (any one file → SUBSTANTIVE): `crates/**`, `xtask/**`, `fuzz/**`,
   `scripts/**`, `migrations/**`, `.github/**`, `.claude/**`, `.mcp.json`, `*.rs`, `*.sql`, `*.sh`,
   `*.py`, `rust-toolchain{,.toml}`, any `*.toml` that is not a Cargo manifest (so `deny.toml`,
   `dist-workspace.toml` count), a Cargo manifest **mixed** with non-manifest changes, and any binary
   file. Deliberate stance: every code change already runs the two ≥3-agent gates, so the report
   content exists by process — a 1-line code fix ships a short report, and that is intended.
   **Self-gating:** `scripts/**` is in this class, so weakening the gate is itself gated; so are the
   hooks (`.claude/**`) and CI (`.github/**`). There is deliberately NO workflow-pin trivial class —
   repointing a `uses:` SHA is a supply-chain edit; `.github/**`, `.claude/**` and `.mcp.json` stay
   always-substantive for non-bot actors (a hook-defanging PR must face its own gate). Note the arm
   ORDER: `crates/**` precedes the docs class, so a crate README edit rides the code class.
4. **Docs class** (`*.md` anywhere not already matched above, `LICENSE*`) — trivial **iff all three**
   hold: (a) no added/removed line matches a PRD-definition pattern — `\|\s*\*\*D[0-9]+\*\*\s*\|` (a
   PRD §4 D-row) or `^\s*-\s*\*\*(FR|NFR)-[0-9]+` (a PRD §5/§6 def line) — a contract-definition
   change is substantive at ANY size; (b) every file's status is `M` (an Added/Deleted/Renamed doc is
   a new/removed/moved artifact); (c) total added+deleted lines across the class (numstat; binary `-`
   counts as 1000, fail-closed) **< 20**. Otherwise → SUBSTANTIVE. The `< 20` floor is the typo-fix
   door; the const sits at the top of the script, and editing the script is `scripts/**` →
   substantive → self-gated.
5. **Fail-closed fallthrough:** any path not classified above → SUBSTANTIVE.
6. **Dependabot:** exempted **structurally by machine identity at the CI job level**, keyed on the
   **PR author login** — `github.event.pull_request.user.login != 'dependabot[bot]'` — never on
   `github.actor` (the EVENT actor: a human re-triggering CI on a dependabot PR would flip the actor,
   run the gate against `.github/**` with no run-report possible, and deadlock the required check; the
   author login is immutable PR metadata). A bot identity is metadata, not a discretionary label;
   GitHub counts a skipped required check as passing. This covers dependabot's GHA pin bumps, which
   would otherwise hit rule 3 via `.github/**`.
7. **Requirement when SUBSTANTIVE:** the diff must contain ≥1 file matching
   `.knowledge/wiki/runs/*.md` — **top-level only** (the `:(glob)` pathspec magic is required: git's
   default pathspec `*` DOES cross `/`, so a stray nested file would otherwise qualify; k2
   independently rejects the subdir) — with status `A` or `M` **and ≥ 10 added lines** (numstat, with
   rename detection deliberately kept: a renamed old report is `R`, excluded; a binary report's `-`
   numstat is excluded too). Absent → BLOCK. `M` legitimately lets a follow-up PR amend its run's
   report; the ≥ 10-added-lines floor stops a one-char touch. Report *validity* (frontmatter,
   glossary, index entry) is knowledge-lint's business (§2.3.2); report-content QUALITY is a named
   residual (§2.3.5, R-B4) — not overclaimed here.

**The exact CI job** (in `.github/workflows/ci.yml`; pure git + POSIX sh — no toolchain, no cache; the
job costs seconds; PR-only, since the predicate needs a base branch and pushes to `main` already carry
a merged, gated PR):

```yaml
  run-report-gate:
    name: run-report-gate (.knowledge run-report on substantive PRs)
    # Decision 7(ii): a substantive PR must carry its wiki run-report in the SAME diff. The predicate is
    # STRUCTURAL (scripts/knowledge/run-report-gate.sh — the same file the pr-create hook runs): no label,
    # no manual bypass. Dependabot is exempt by machine identity keyed on the PR AUTHOR login (immutable
    # PR metadata — github.actor would flip to whoever re-triggers CI on a bot PR and deadlock the
    # required check); its skipped required check counts as success. Spec: ci-cd-and-distribution.md §2.3.
    if: github.event_name == 'pull_request' && github.event.pull_request.user.login != 'dependabot[bot]'
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@08c6903cd8c0fde910a37f88322edcfb5dd907a8 # v5.0.0
        with:
          fetch-depth: 0   # full history so merge-base(origin/<base>, HEAD) resolves
      - name: run-report gate (shared predicate)
        run: scripts/knowledge/run-report-gate.sh "origin/${GITHUB_BASE_REF}"
```

**Branch-protection sequencing (the flip is server-side state; it cannot ride the PR):** owner
**Miguel** (repo admin); timing **immediately AFTER the landing PR merges — never before** (flipped
before the merge, every other open PR shows a permanently "Expected" check and cannot merge — only
`if:`-skipped runs of an EXISTING job count as success; never flipped, a red gate does not block the
merge button and the hard floor is silently a warn). Action: add `run-report-gate` to the `main`
branch-protection required status checks and enable **enforcement for administrators**. Verification
(run, don't assume): `gh api repos/{owner}/{repo}/branches/main/protection --jq
'.required_status_checks.contexts'` must list `run-report-gate`, and `--jq '.enforce_admins.enabled'`
must print `true`. Interim-window residual (stated, not softened): between the landing merge and the
flip, the job RUNS and reds on violating PRs but does not yet block the merge button; the window is
minutes long, owned by Miguel, verified by the `gh api` read, and recorded in the landing run-report.

**Gate self-test harness — `scripts/knowledge/tests/run-report-gate-selftest.sh` (0755).** The gate
script is the single most load-bearing decision-7 artifact, so it ships with its own executable
proof: a **fixture-repo harness**, pure POSIX sh + git, offline and deterministic — for each case,
build a throwaway repo under `mktemp -d`, apply the case's diff on a branch, run the gate, and assert
**both** the exit code and a distinguishing stderr rationale substring. It also greps the script's
`SESSION_LOCAL_ID_RE=` line against the rule-1a literal above AND greps this doc for the identical
literal (the single-sourcing pin). Mandatory case matrix — every arm of the script (each of the 14
rule-3 case-globs, the `*.toml`/manifest-mixed/docs/fallthrough arms, rules 1/1a/2, 4a–4c including
4c's binary fail-closed arm, and rule 7 including its binary numstat exclusion) and all three exit
codes; the `.md`-inside-dir cases double as case-arm ORDER pins — a `.md` under an always-substantive
dir must ride rule 3, never the docs class (the dependabot exemption is job-level `if:` metadata,
outside the script):

| # | Case | Expect |
|---|------|--------|
| 1 | empty diff | 0 |
| 2 | `.knowledge/` page + export touch, no new comment codes | 0 (`only neutral paths`) |
| 3 | export-only diff whose added comment coins a new `MF-9`-style token | 1 (rule 1a, no report) |
| 4 | case 3 + a qualifying run-report | 0 |
| 5 | export record rewrite where the token already existed in the removed line | 0 (no re-trigger) |
| 6 | `Cargo.toml` + `Cargo.lock` only | 0 (`pure manifest`) |
| 7–20 | one case per rule-3 case-glob (all 14): `crates/` README `.md` (order pin: `crates/**` beats the docs class) · `xtask/` `.md` (order pin) · `fuzz/` `.md` (order pin) · `scripts/` file (self-gating) · `migrations/` `.md` (order pin) · `.github/` `.md` (e.g. `PULL_REQUEST_TEMPLATE.md` — order pin) · `.claude/settings.json` · `.mcp.json` · root `*.rs` · root `*.sql` · root `*.sh` · root `*.py` · bare `rust-toolchain` · `rust-toolchain.toml` | 1 each (stderr names the `always-substantive path`) |
| 21 | `deny.toml` (the non-manifest `*.toml` arm) | 1 |
| 22 | `Cargo.toml` mixed with a doc | 1 (manifest-mixed) |
| 23 | unclassified `foo.xyz` | 1 (fail-closed fallthrough) |
| 24 | doc M-only adding a `\| **D9** \|` row | 1 (4a) |
| 25 | doc M-only adding a `- **FR-3**` line | 1 (4a) |
| 26 | new doc (A) | 1 (4b) |
| 27 | deleted doc (D) | 1 (4b) |
| 28 | `git mv` doc → doc (rename decomposition) | 1 (4b) |
| 29 | `git mv` repo-doc → `.knowledge/wiki/topics/x.md` (the D side is classified) | 1 (4b) |
| 30 | 25-line M-only doc edit | 1 (4c) |
| 31 | 5-line M-only doc edit | 0 (trivial) |
| 32 | binary-content `.md` file, M-only (numstat `-` → counted as 1000) | 1 (4c binary arm, fail-closed) |
| 33 | substantive + report with only 3 added lines | 1 (10-line floor) |
| 34 | substantive + `git mv` old report → new name, zero added lines | 1 (renames stay excluded) |
| 35 | substantive + only a NESTED `.knowledge/wiki/runs/sub/x.md` report | 1 (`:(glob)` pin) |
| 36 | substantive + only a BINARY-content run-report (numstat `-`) | 1 (rule-7 binary exclusion) |
| 37 | nonexistent base-ref | 2 (`cannot compute merge-base`) |
| 38 | substantive + qualifying report (≥10 added lines, top-level) | 0 |

CI wiring (always-on, blocking): one step appended to the existing `doc-lint` job, directly after the
knowledge-lint step of §2.3.2 (it needs only git + sh; the job's toolchain is irrelevant to it).

#### 2.3.4 Layer (iii): PreToolUse hooks + the sanctioned retire flow

Hooks are **hard**: every deny is exit code 2 (stderr goes back to the model), and every script
**fails closed** — an unparsable payload or a git error is a deny with a diagnostic, never a
warn-and-pass. **Environmental scope, stated precisely:** that fail-closed property holds once a
script RUNS. Claude Code blocks only on exit 2 — a missing script file, an absent `python3`, or an
unset `$CLAUDE_PROJECT_DIR` surfaces as a NON-blocking hook error (environmental fail-open), and
non-Claude-Code clients (SDK/stdio harnesses) execute no hooks at all. That residual is named at
§2.3.5 R-B9; its compensations are the CI authority (every hook rule has a server-side backstop — no
rule exists only in a hook) and the landing-PR hook smoke tests (one deliberate denied canary per
hook, recorded in the first run-report). Hooks live in the checked-in project settings
(`.claude/settings.json`), so worktree sessions inherit them. Miguel's own terminal is outside hook
jurisdiction by construction — that, plus the sanctioned-flow scripts, is the "sanctioned flow".
`$CLAUDE_PROJECT_DIR` is the documented read-only project-root input (CLAUDE.md conventions) — these
hooks are a sanctioned consumer. Python3 is used for robust JSON parsing (`Bash(python3:*)` is already
in the repo allowlist; no jq dependency).

**Hooks JSON** (the top-level `"hooks"` key in `.claude/settings.json`, sibling of `"permissions"`;
each script reads the hook payload JSON on stdin — `tool_name`, `tool_input`, `cwd`; exit 0 allows,
exit 2 BLOCKS and surfaces stderr to the model):

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Write|Edit|MultiEdit",
        "hooks": [
          {
            "type": "command",
            "command": "\"$CLAUDE_PROJECT_DIR\"/scripts/hooks/knowledge-memories-write-guard.py",
            "timeout": 15
          }
        ]
      },
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "\"$CLAUDE_PROJECT_DIR\"/scripts/hooks/knowledge-memories-bash-guard.py",
            "timeout": 15
          },
          {
            "type": "command",
            "command": "\"$CLAUDE_PROJECT_DIR\"/scripts/hooks/pr-create-run-report-gate.py",
            "timeout": 60
          }
        ]
      }
    ]
  }
}
```

Hook matchers are full-match regexes naming this repo's write-capable tools (`Write`, `Edit`,
`MultiEdit`) — a schema-valid wholesale overwrite of a curated memory would otherwise pass every other
layer (lint green — the new content is valid; gate neutral — `.knowledge/**` is stripped). For memories
the hook is the ONLY pre-commit layer.

**`scripts/hooks/knowledge-memories-write-guard.py`** — Write/Edit protection of `memories/**`. Paths
are normalized cwd-aware BEFORE the marker check (a relative path or a `..`-hop must not slip the
substring marker). Rule table:

| Operation on `.knowledge/memories/**` | Verdict |
|---|---|
| `Write` to a NEW file (memory creation) | allow |
| `Edit`/`MultiEdit` on any memory file or `index.md` (surgical curation) | allow |
| `Write` over an EXISTING memory file (wholesale overwrite) | **deny** — "use Edit" |
| `Write` over `index.md` (regenerating the curated index) | allow (derived data; k1/k2/k4 re-validate it) |
| any Write/Edit to a NESTED path under `memories/` | **deny** (flat layout; k2 backstops) |
| a matched tool call with NO recognizable path field, or a relative path with no `cwd` | **deny** (fail closed) |

**`scripts/hooks/knowledge-memories-bash-guard.py`** — destructive-Bash protection. Deny model,
ordered so the allowlist is reachable: (1) **SANCTIONED first** — exactly one un-chained
`memory-retire.sh <slug>` invocation → allow (the arg pattern is the slug grammar; the script itself
re-validates and rejects `index`). (2) **Trigger B — pathless destructive shapes**, denied even with
NO `.knowledge` mention (uncommitted memories have NO other layer: CI and the lint see committed state
only): any `git clean` (it removes untracked files), and any **recursive `rm`** whose cwd-resolved
target is the repo root, an ancestor of it, `/`, `~`, or `.knowledge`/anything under it. (3) **Trigger
A — `.knowledge` prefix scan:** a command that mentions `.knowledge` (ANY part of the tree — covering
the parent-dir shape and the split-context `cd .knowledge && rm …` shape) AND carries **mutation
capability** is denied; pure reads pass. Mutation capability = hard verbs (`rm unlink rmdir mv cp dd
tee shred truncate install ln touch chmod chown rsync xargs eval`); conditional (`sed`/`perl` only
with `-i`; `find` only with `-delete`/`-exec`; `git` only with destructive verbs); interpreters with
the path in scope; any `>`/`>>` redirection targeting the tree.

**`scripts/knowledge/memory-retire.sh`** — the sanctioned destructive flow. The sanctioned flow is ONE
atomic script (file + index consistent, exact hook allowlist, stages-never-commits) rather than a
two-step de-index-first flow. It (a) rejects the slug `index` explicitly (slug-grammar-valid, so an
exclusion is required); (b) re-validates the slug grammar itself (the hook allowlist is NOT the
enforcement point — the script also runs outside hook jurisdiction); (c) is ordered so NO partial
destructive state survives a mid-script failure: validation + the new index content (pure computation,
written off to the side) come first; mutations run last with the destructive file removal FINAL. Exits:
3 usage · 4 no-such-memory / broken tree · 5 invalid slug. A pre-existing missing index entry produces
a stderr diagnostic (an NFR-14-class note about an already-k2-visible orphan state), not a failure —
the blocking layer (k1/k2) re-validates the post-retire tree on the next lint run.

**`scripts/hooks/pr-create-run-report-gate.py`** — gates `gh pr create` on the run-report by
delegating verdict AND requirement to `scripts/knowledge/run-report-gate.sh` — **the same file CI
runs** (zero drift). `gh api` is allowlisted in the repo settings, so the API pulls-POST shape must be
intercepted too. It evaluates against the local `origin/main` (or the parsed `--base`) without
fetching — no network in hooks; CI recomputes authoritatively. Known residuals: hooks guard the agent
flow, not a human terminal; a `gh pr create -R <other-repo>` still runs this repo's predicate
(harmless); CI remains the authoritative backstop; Write/Edit-tool protection of `wiki/**` is
consciously out of scope (memories-only; shell-side destructive commands naming `.knowledge` ARE
denied by trigger A; a runs deletion still surfaces via k1/k2 + the PR diff) — revisiting it is a
recorded open follow-up of the epic.

#### 2.3.5 Accepted residuals & bypass closures

False positives (cost = friction, never data loss):

- **R-F1** A docs-only sweep ≥ 20 lines demands a run-report. Closure: that scale of sweep IS a
  session worth 10 report lines; the budget const is edited only via a `scripts/**` change — itself
  substantive, so the knob is self-gated. No discretion path added.
- **R-F2** Bash read-pipelines mixing a mutating verb with the memories path are denied. Closure: the
  deny message instructs splitting the command; fail-closed is the approved posture; a zero-risk
  workaround always exists.
- **R-F3** Fenced examples of frontmatter/index entries inside pages could trip k-checks. Closure: the
  reused fence mask skips fenced lines by construction; pinned by a no-false-positive unit test.
- **R-F4** `gh pr create` against a non-main base mis-computes the diff. Closure: the hook parses
  `--base`; the house rule is base=main anyway; CI recomputes authoritatively from the base ref.
- **R-F5** Durable tokens colliding with `SESSION_LOCAL_ID_RE` (`M0`–`M3` milestones, roadmap `R`
  rows) in a NEW issue comment make an export-only PR substantive → a run-report is demanded for a
  session that coined nothing. Closure: fail-closed friction by design (a report for such a session is
  cheap and the glossary sentinel is one line); the pattern is ONE declared const, tunable only via a
  `scripts/**` change — itself substantive, so the knob is self-gated. The same tuning constraint
  applies to the k6 token-coverage const.

Bypass surfaces (each with its closing layer):

- **R-B1** An agent edits `.claude/settings.json` or `scripts/**` to defang hooks. Closure: hooks are
  the early net, **CI is the authority** — `run-report-gate` + `knowledge-lint` run server-side, and
  any such edit is itself SUBSTANTIVE (rule 3), demanding a run-report that documents the change in
  front of the human merger. System-prompt rules already forbid agent permission/config changes.
- **R-B2** A manifest-only dependency ADDITION passes as trivial (rule 2). Closure: an unused
  dependency is inert; using it requires `*.rs` changes → substantive then; `cargo-audit`/`cargo-deny`
  still gate the PR. Accepted residual.
- **R-B3** A crafted < 20-line spec change dodges rule 4c. Closure: rule 4a fires on any D-row /
  FR/NFR definition line regardless of size; the design-Review gate (process layer) owns residual
  semantics — the predicate is a structural backstop, not the only reviewer. Same stance for a
  < 20-line M-only stance edit to `CLAUDE.md`/`docs/PROCESS.md`/`AGENTS.md` (docs class). Accepted
  residual.
- **R-B4** The ONE ownership site for report-content quality: touching an old run-report to satisfy
  the gate, or shipping a frontmatter-valid report with placeholder bodies — both survive the machine.
  Closure: the ≥ 10-added-lines floor stops the one-char touch; a junk paste passes the floor but sits
  in the PR diff under human merge — and knowledge-lint keeps the page schema-valid. Accepted
  residual, NARROWED by the k6 token-coverage rules (§2.3.2): token-bearing junk now needs matching
  glossary DATA rows; token-FREE placeholder padding remains the residual. Optional hardening recorded
  and NOT adopted day-1: counting the 10-line floor over non-heading, non-frontmatter added lines —
  adopt only if dogfooding shows padding (a `scripts/**` change, self-gated).
- **R-B5** Obfuscated destructive Bash (variable indirection; `git filter-repo`). Closure: the guard
  catches the plain parent-dir, pathless, and split-context shapes; the surviving residual is
  variable-indirection obfuscation, which is not a shape agents emit innocently. The durable invariant
  is CI-side — a landed deletion breaks k1/k2 unless the index was updated too, and either way the PR
  diff shows it to the merger. Hooks are defense-in-depth by design.
- **R-B6** PR created via raw `curl` to the GitHub API. Closure: `curl` is not in the repo allowlist →
  a permission prompt (a human decision); the `gh api` pulls-POST shape IS intercepted; the CI gate
  still fails the PR itself.
- **R-B7** Worktree paths evade path-anchored patterns. Closure: all matchers are
  substring/suffix-based, so worktree absolute paths match; worktrees inherit the checked-in hooks and
  scripts.
- **R-B8** Pushing more commits after `gh pr create` (report deleted post-creation). Closure: CI
  re-runs the gate on every push to the PR; the hook is only the early check.
- **R-B9** Hooks fail OPEN environmentally: Claude Code blocks only on exit 2, so a missing script
  file, an absent `python3`, or an unset `$CLAUDE_PROJECT_DIR` is a NON-blocking hook error, and
  non-Claude-Code clients (SDK/stdio harnesses) run no hooks at all. Closure: no rule exists only in a
  hook — CI (`knowledge-lint` + `run-report-gate` + the selftest) is the server-side authority for
  every hook-guarded rule EXCEPT pre-commit protection of uncommitted memories, whose practical
  exposure is in-session (where the landing-PR smoke canaries prove the hooks execute) — plus normal
  traffic exercises the hooks every session, so silent environmental rot surfaces immediately.
  Accepted residual, named — not softened (no warn path is added anywhere).
- **R-B10** The road not taken on glossary depth, recorded with its true cost: under a
  presence-only k6 (shape checks alone), nothing machine-checks that comment-coined codes have
  glossary rows, NOR that report-body-used codes have glossary rows — the shape check ties no row to
  any token, so a report could assert codes directly above a factually-false sentinel and stay green.
  The selected depth (the k6 token-coverage rules of §2.3.2, hard + temporal) closes that; bare facet
  letters (e.g. "arm A") are an inherent bound of ANY digit-anchored pattern and remain duty-only
  (the duty lives in `docs/PROCESS.md` section 8).
- **R-B11** Rule 1a's comment-coining scan keys on `"comments"`-bearing added export lines — so a
  brand-new record exported WITHOUT a `"comments"` key whose `description` prose coins session-local
  codes escapes rule 1a (a record WITH the key is scanned whole-line, fail-closed). Closure: the
  process flow makes every outcome an issue comment, so a description-only coining record is not a
  shape the sanctioned flow emits; such a PR still faces every other predicate rule; and the k6
  comment scan re-covers the codes the moment any in-scope comment cites them. Accepted residual,
  named — no warn path added.

Consciously out of scope (per the approved design): Write/Edit-tool protection of `wiki/**`
(memories-only; shell-side destructive commands on the whole `.knowledge` tree ARE denied; a runs
deletion still surfaces via k1/k2 + the PR diff), and any private-memory migration content (a separate
epic task).

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
  and colored success/abort lines. Color is decided ONCE per stream, honouring `NO_COLOR`, `CLICOLOR`,
  `CLICOLOR_FORCE` (via a pure, unit-tested helper) and auto-disabling on a non-TTY or when `NO_COLOR`
  is set: the structured `stdout` output keys on `stdout`'s TTY-ness, while the `stderr` spinner + error
  lines key on **`stderr`'s OWN** TTY-ness — so a redirected `stderr` (`2>log`) stays plain even when
  `stdout` is a TTY. Styling is a pure presentation layer over the same output sinks, so the safety model
  is UNCHANGED (numbered menus stay numeric; the typed-tag double-confirmation stays free-text;
  `--dry-run` mutates nothing; the pre-flight order and the atomic push are as above). The two slow steps
  (`git fetch`, `cargo update`) show a TTY-gated spinner that degrades to a single static line off a TTY
  and reports the step OUTCOME (a FAILED step renders a failure line, never a success glyph); the spinner
  and all diagnostics go to **stderr**, structured output to **stdout** (NFR-14).

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
