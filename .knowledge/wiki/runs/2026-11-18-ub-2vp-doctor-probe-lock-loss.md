---
name: 2026-11-18-ub-2vp-doctor-probe-lock-loss
description: Measuring the v1.0.x doctor header probe's POSIX lock loss (tracker ub-2vp, D58 criterion (n)) — the probe's raw std::fs close drops the process's SQLite SHARED lock beside live libsql connections, another process's last close then unlinks the -wal and -shm, and a later commit by the orphaned holder is silently lost; confirmed on Linux, macOS and the real v1.0.2 binary, absent on main (guards fd4, fd7, G3), with the v1.0.3 fix split out to ub-kpe, a dist precise-builds fix for testkit reaching shipped binaries, and a torn /proc/locks read found and fixed as a flaky instrument.
type: run
date: 2026-11-18
branch: ub-2vp-doctor-probe-lock-loss
pr: '476'
issues: [ub-2vp, ub-kpe, ub-9m0]
---

# Run — v1.0.x doctor probe: descriptor-close lock loss (D58 criterion (n))

## Context

Issue `ub-2vp`: the shipped v1.0.x `unblock doctor` reads the first 16 bytes of `unblock.db` through
`std::fs` while the same process holds libsql connections, and on Unix that descriptor's close drops
every POSIX lock the process holds on the file. The run went from 2026-11-17 to 2026-11-18 on branch
`ub-2vp-doctor-probe-lock-loss`, cut from main `469d704`, in the isolated worktree `../unblock-ub-2vp`.
PR: #476 (draft, opened for the CI measurement under the PROCESS.md §6 exception). Phases and gates:
Understand (three read-only lenses, comment 429); Decide (rulings (a), (c), (d); (b) deferred, comment
430); Spec/Plan v1 (comment 431, OQ1–OQ8 resolved); design Review gate, iteration 1 PASS with 28
deduplicated findings and rulings MR1–MR3 (comment 432); Implement as one WIP commit (comment 433);
the Measurement and its classification M1 (comment 434); Decide on (b) (comment 435); Verify in four
iterations (comments 436, 438, 439, 440), escalated to Miguel under PROCESS.md §5 after iteration 2
and again at the M5 flake; then this Track step. Team: one coordinator, three Understand lenses, three
Spec/Plan lenses, three design-Review lenses, one implementer, three Verify lenses (code-review, QA,
rust/spec/security) and one Track agent.

## What & why

The probe (`db_header_mismatches_sqlite`, v1.0.x `crates/unblock-health/src/file_state.rs:203-211`,
the same blob `77b62bb1…` in v1.0.0, v1.0.1 and v1.0.2) runs after `Session::doctor`'s first
`integrity_check` has taken SQLite's SHARED lock. SQLite uses classic per-process `fcntl` locks, so
a close the VFS did not make bypasses its `setPendingFd` deferral and drops the lock, while SQLite's
per-inode count still says SHARED and never re-asks the kernel. Without SHARED, another process's last
close can take EXCLUSIVE, checkpoint and unlink the `-wal` and `-shm` the prober still has open. Main
deleted `file_state.rs` at v1.1 (ub-lp9.3), so the question was whether the hazard is real in the
shipped line and whether main's replacements — the pre-open primitives, `gather_preopen`'s
`live > 0` refusal and the repair window's sidecar re-read — are free of it. Authoritative text: PRD
§4 D58 clause (6)(a2) and (6)(c), clause (12) criterion (n) and its ub-2vp amendment; spine §3.1
(`ExclusiveRequest` testkit setters) and §3.3 step (c); the storage, engine and health crate plans;
implementation-plan T4.3; ci-cd §2 (job table), §2.1 (the D58 sub-check paragraph) and §3.1 (the D17
amendment); `scripts/checks/d58-health-full-claims.sh`.

## Outcome

Landed on the branch over `469d704` as five atomic commits (re-cut from the WIP commits `58057c0`,
`317a580`, `41fb71b`, `e8a6e7e`, `c88c5d1`, `e875482`; the final tree is unchanged):
- `docs(d58)` — the criterion (n) amendment, the spine setters, the fd7 citations, the crate-plan and
  ci-cd edits and the dated measured note, with d58 rows Q278–Q282 and Q295–Q303 (the measured note
  shares the spec commit because the measurement ran on the WIP before the re-cut).
- `build(dist)` — `precise-builds = true` and the D17 amendment (ruling V2).
- `test(storage)` — the two testkit-gated window hooks.
- `test(storage)` — holder modes, the `Holder` refactor, `posix_locks.rs`, the fd cells and fdp,
  `nix`, the `ci.yml` comment edits, rows Q283–Q291 and Q294.
- `test(engine)` — G3, G3p and the engine `/proc/locks` copy, rows Q292–Q293. The Q floor is 303.

**Measurement evidence (comment 434, outcome M1: lock loss CONFIRMED).**

Provenance. HEAD cells on WIP `58057c07be38e0a1541e463be343b42c4bec71a6` over `469d704`, draft PR #476,
CI run 44251096720; libsql 0.9.30 with `libsql::version()` = `3.45.1` on every leg. Probe copied
verbatim from tag v1.0.2, blob `77b62bb1328c69545ed87319655243e579d3b4c5`, `file_state.rs:31` and
`:203-211`. v1.0.2 binaries built from a detached worktree at `842cdff3909ac356ac73f116d85516839167afba`
with an empty `git status --porcelain` after both builds and the SQLite 3.45.1 source id found once
in each binary: a debug build (default features) for lock observation and a `--profile dist` build
for timing only (MR3). Instruments: HEAD helper `exclusive_window_holder` sha256 `a8dd62ce4b98…c538`;
a throwaway driver linking only `nix`, sha256 `40f3e623…`, deleted after the run. Toolchain `rustc
1.96.0 (ac68faa20 2026-05-25)`.

| Leg | Environment | fd1 (reproduction) | fd5 (impact) |
|---|---|---|---|
| Local macOS | 27.2.1 (26B131), APFS, aarch64 | SHARED dropped; O `LOCKED` | `-wal`/`-shm` unlinked |
| CI `exclusivity-macos` | `macos-15-arm64`, image 20261104.0514.1 | SHARED dropped (`F_GETLK`); O `LOCKED` | unlinked |
| CI `storage-testkit` | `ubuntu-26.04`, image 20261111.82.6 | SHARED dropped (`F_GETLK` and `/proc/locks` agree); O `LOCKED` | unlinked |
| CI `test (workspace)` | `ubuntu-26.04` | all fd cells pass | pass |
| v1.0.2 real `unblock doctor` | local macOS | Run 1: dropped | Run 2: unlinked |

fd1 trace (CI Linux, holder pid 3344): baseline `F_GETLK` reports Read by pid 3344 at offset
1073741826, length 510, and `/proc/locks` agrees; after `PROBE-V1` there is no lock (`F_UNLCK`, nothing
in `/proc/locks`) while the holder still has 2 live connections; after a further `READ` and a
SQLite-owned reopen there is still none; O then gets `LOCKED`.

v1.0.2 runs. Run 0 (the same-binary control that gates the rest): `unblock mcp` holds Read under its
own pid and O gets `BUSY 5`. Run 1: doctor held after its probe by stdout back-pressure (200k planted
dangling edges, 12,800,184 bytes drained, exit 0) holds no SHARED lock while `lsof` shows `unblock.db`
×2, `-wal` ×2 and `-shm` open, and O gets `LOCKED`. Run 2: with doctor alive, another process's
open-read-close deletes `-wal` and `-shm`.

Controls and guards (green on every leg, so neither M5 nor M6): fd0 (holder holds SHARED, O `BUSY 5`),
fd2 (no-op keeps it), fd3 (SQLite-owned close keeps it), fd5c (no probe, sidecars stay), fd4 (main's
pre-open primitives: `lost=false live=2 write_lock=false`, Read kept), fd7 (P1: the window holds Write
on the SHARED range, `-wal`/`-shm` carry no POSIX lock, O `BUSY 5`; P2: Write kept; outcome
`refused=None checkpoint_truncated=true renamed=1`), G3 (Linux: a refused `gather_preopen` keeps the
session's SHARED lock), fdp and G3p (parsers). So main@`469d704` does not carry the hazard; only the
shipped v1.0.0–v1.0.2 do.

Impact (identical on CI Linux, CI macOS and local macOS). fd6: after the unlink the holder's write
reports `WROTE`; a second process holding the new `-wal`/`-shm` sees `ABSENT` before and after the
first holder quits; the row is LOST and `integrity_check` is clean. fd6e: with a clean exit and nobody
else alive the row survives, because the holder's close checkpoints its unlinked WAL. fd6k: after a
SIGKILL the row is LOST. Silent data loss is therefore reachable with no corruption signal; in v1.0.x
the process that loses its lock is doctor, which issues no SQL write (its close may checkpoint), so the
direct v1.0.x consequence is another process deleting the `-wal`/`-shm` doctor is still reading.

Exposure window (Run T; it corrects comment 429 §4). From the probe to doctor's exit v1.0.2 runs a
second full `integrity_check`, `schema_version`, the dangling read and rendering:

| Corpus | debug | dist (shipped) |
|---|---|---|
| empty | 2.96 ms ± 0.55 ms | 1.82 ms ± 0.37 ms |
| 250k issues | 2.179 s ± 0.24 ms | 0.698 s ± 0.27 ms |

Every timeline: no lock, Read, Read lost at the probe, a short Write at close, exit.

**Classification and rulings.** M1 accepted (comment 435): fd1, fd5 and fd6 are permanent required
assertions (fd6 with row Q294, OQ3 = a); fd6e and fd6k were dropped (WIP only). Ruling (b) = **B1a**:
ship a v1.0.3 maintenance patch as a separately planned unit, tracked as `ub-kpe` (P1, `data-integrity`,
`v1-0-3`, `discovered-from` ub-2vp), which must settle the fix choice, a D29 addendum or new D-id, the
roadmap slot, the release path (`cargo xtask release` is main-only) and a v1.0.x regression cell.
ub-2vp closes when PR #476 merges.

**Verify.** Iteration 1 (diff `469d704..317a580`, CI run 44253746192): code-review and rust/spec/security
PASS-WITH-MUST-FIX, QA PASS; must-fixes: the measured note's clause step is (6)(a2) not (6)(b), stale
`health.rs` line numbers in fd1/fd5/fd6 messages, STYLE.md Colon Rule. Rulings V1 (match the device in
the `/proc/locks` parsers) and V2 (fix in this PR: dist 0.32.0 built `--workspace`, so feature
unification with `unblock-fuzz`'s normal-edge `unblock-storage/testkit` compiled the storage testkit
into the shipped `unblock` v1.0.0–v1.0.2 — unreachable because the hooks are always `None`, but it
falsified "never enters a production build"; fixed by `precise-builds = true` and verified with
`cargo tree -p unblock-cli -e features -i unblock-storage`, `dist plan` and a local `--profile dist`
build). Iteration 2 (`317a580..41fb71b`, CI run 44258293095, all required checks green; `plan` shows
`--package unblock-cli`): QA and rust/spec/security PASS, code-review PASS-WITH-MUST-FIX (one fd7
message); escalated, ruling V1b (match the device only when some line on the inode carries it, else
inode only — the btrfs case). Iteration 3 (`41fb71b..e8a6e7e`): the lenses passed, but CI run
44262638365 job 134659435403 failed fd3 at its holder baseline: `/proc/locks` returned no line while
`F_GETLK` saw Read by pid 2817 — outcome **M5**, root cause **H4** (std's `read_to_string` reads
`/proc/locks` in many small `read()` calls, each restarting the kernel's seq_file walk at a position,
so a lock dropped earlier in the list by a parallel cell skips the holder's line); H1–H3 refuted.
Ruled fix (a)–(d): one 256 KiB `File::read` buffer, dedup, a bounded re-read of both witnesses up to
N=10 that fails safe, and `attempts`/`proc_reads`/`proc_bytes` in the evidence line. Iteration 4
(`e8a6e7e..c88c5d1`): all three lenses PASS; the rust lens confirmed from `fs/seq_file.c` that the
buffer makes reads consistent per page and the re-read makes the check correct across pages.
Stress evidence (CI run 44275707776, job 134703155789, draft-only commit `c88c5d1`):
`UB2VP-STRESS leg=linux/storage-testkit churners=50 reads=100000 proc_locks_bytes_max=7088
old_misses=12682 new_misses=0 new_reads_max=11 new_multi_page_reads=99475 observe_failures=0
observe_retried=0` — the old method missed the stable line in 12.7% of reads, the new one never,
although 99.5% of reads spanned several pages. `e875482` removed every draft-only item and applied
the iteration-4 nits. The ruled twenty sequential `storage testkit` re-runs on `e875482` passed
20/20 with no failed cell: CI run 44324250725, jobs 134871125096 (attempt 1) through 134901450921
(attempt 20) (comment 441).

**macOS leg on `e875482`.** GitHub cancelled the `exclusivity (macOS)` job twice before any runner
picked it up: job 134871125140 in attempt 1, and the re-run job 134902771602 in run 44324250725, after
15 minutes in the queue ("The job was not acquired by Runner of type hosted even after multiple
attempts"; "Due to capacity constraints, jobs targeting macOS arm64 runners may experience longer
queue times"). The macOS leg had passed on CI at `58057c0`, `317a580`, `41fb71b` (run 44258293095),
`e8a6e7e` (run 44262638365) and `c88c5d1` (run 44275707776). Locally on `e875482`, `exclusive_window`
ran 36/36 (the `fd` filter 9/9) and engine `gather_preopen` 3/3. The only changes from `c88c5d1` that
compile on macOS are the shared `posix_locks.rs` reader and doc wording. The Verify PASS is
**conditional**: it becomes final only when the `exclusivity (macOS)` job of the CI run started by the
re-cut history's force-push is green, and the job is re-run until a runner picks it up. The PR stays
draft until then (comment 442).

## Gotchas

- **Apple's `sqlite3` persists WAL.** `/usr/bin/sqlite3` 3.54.0 keeps `-wal`/`-shm` by default, so
  planting the v1.0.2 corpus needed `.filectrl persist_wal 0` and the polluted workspace was discarded
  (comment 434).
- **`/proc/locks` seq_file skips.** A `read_to_string` of `/proc/locks` is not a snapshot; concurrent
  lock churn skips lines (H4, 12.7% under stress). Read with one large buffer and re-read to confirm
  (`crates/unblock-storage/tests/common/posix_locks.rs:181`, comments 439–440).
- **btrfs `st_dev`.** `stat`'s device is the subvolume's anonymous device while `/proc/locks` prints the
  superblock's, so strict device matching false-reds on btrfs; hence V1b's inode-only fallback
  (comment 438).
- **LTO makes a strings check non-discriminating.** The `dist` profile's LTO drops the unreachable
  testkit code, so `strings` on a release binary cannot show whether testkit was compiled in;
  `cargo tree -e features -i unblock-storage` is the discriminating check (V2, comment 436).
- **Claims row Q242.** It pins every `ci.yml` line containing `--test exclusive_window`, comments
  included, so new comments name "the exclusive_window cells" and no new target may be named
  `exclusive_window_*` (`scripts/checks/d58-health-full-claims.sh`, comment 431).
- **`cancel-in-progress` cancels evidence runs.** `ci.yml:46-48` cancels a superseded run on the same
  ref, so a push during a measurement or stress run kills the evidence; pushes waited for runs to end.
- **The stress step took 4 h 43 min** on the Linux runner (CI run 44275707776, job 134703155789,
  comment 440); it was draft-only and removed before Track, and nothing like it stays in `ci.yml`.
- **A GitHub macOS runner shortage cancels jobs.** A job targeting the hosted macOS arm64 runner was
  cancelled after about 15 minutes in the queue when no runner acquired it, twice on `e875482`
  (jobs 134871125140 and 134902771602). That says nothing about the code; re-run until a runner takes
  it (comment 442).
- **One comment overstated the CI state.** Comment 441 first said every other required check was
  green; `exclusivity (macOS)` had been cancelled, not run. The comment was edited to say so.
- **The reviewer agent hit a provider quota error.** The first design-Review launch failed with
  `402 quota_exceeded`; the gate re-ran on a different agent type with identical briefs (comment 432).
- **Tracker export is path-confined.** The unblock `sync export` refuses a path outside the main
  checkout (`PATH_TRAVERSAL`), so the export went to the main checkout's default path, was copied into
  the worktree and the main copy restored.

## Glossary

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-----------------------------------------------|
| A | The holder process: holds libsql connections through `open_local` and runs commands such as the probe (holder mode `lock-probe`) | crates/unblock-storage/tests/bin/exclusive_window_holder.rs:75; plan v2 glossary |
| A2 | A second `lock-probe` holder spawned after the unlink in fd6 | crates/unblock-storage/tests/exclusive_window.rs:1321 |
| B | A one-shot ordinary process that opens, reads and closes (holder mode `read-and-close`); its last close may unlink the sidecars | exclusive_window_holder.rs:77 |
| O | The behavioural observer: a fresh process running `locking_mode=EXCLUSIVE` + `BEGIN EXCLUSIVE` with `busy_timeout=0`, printing `LOCKED` or `BUSY <code>` (mode `observe-exclusive`) | exclusive_window_holder.rs:76 |
| W | The window child running the real exclusive window with the fd7 hooks (mode `window`, testkit only) | exclusive_window_holder.rs:79 |
| SH | The direct observation of who holds SQLite's SHARED range (0x40000002, 510 bytes): `F_GETLK` on both OSes plus `/proc/locks` on Linux | crates/unblock-storage/tests/common/posix_locks.rs:506 (`shared_range`) |
| P1 / P2 | fd7's observation points in the real window: P1 after `lock_exclusively` and before the evidence check, P2 after it | crates/unblock-storage/src/libsql/health.rs:153, :162 |
| f1 / f5 | Existing cells: f1 a WAL reader elsewhere makes `locking_mode=EXCLUSIVE` BUSY (O's detector); f5 `BEGIN EXCLUSIVE` without `locking_mode` misses a reader | exclusive_window.rs:218, :378 |
| fd0 | Baseline: the holder's store holds SHARED and O gets BUSY 5 | exclusive_window.rs:1084 |
| fd1 | The reproduction: the verbatim v1.0.x probe's close drops SHARED | exclusive_window.rs:1101 |
| fd2 / fd3 | Controls: a no-op, and a SQLite-owned close, keep SHARED | exclusive_window.rs:1169, :1186 |
| fd4 | = G1: main's storage pre-open primitives keep SHARED | exclusive_window.rs:1203 |
| fd5 / fd5c | Impact: after the probe another process's last close unlinks the live sidecars; fd5c the control without the probe | exclusive_window.rs:1245, :1272 |
| fd6 | A commit after the unlink is lost while another process holds the new files | exclusive_window.rs:1321 |
| fd6e / fd6k | WIP-only variants of fd6: clean exit (row survives) and SIGKILL (row lost); dropped after M1 | WIP commit 58057c0; comments 434, 435 |
| fd7 | The repair window's sidecar re-read keeps its exclusive lock and puts no lock on `-wal`/`-shm` | exclusive_window.rs:1403 |
| fd8 | The split-brain cell, deliberately not added | comment 431 OQ3 |
| fdp | The storage `/proc/locks` parser canned-lines cell | exclusive_window.rs:1507 |
| G1 / G3 / G3p | Guards for main: G1 = fd4; G3 a refused `gather_preopen` keeps the session's SHARED lock (Linux); G3p the engine parser copy's canned-lines cell | crates/unblock-engine/tests/health.rs:860, :913 |
| M1 | Outcome "loss confirmed": fd1 drops SHARED and O gets LOCKED on every leg, fd5 shows the unlink, controls and guards hold, v1.0.2 Runs 0/1/2/T agree — the outcome of this run | plan v2 §8.2; ub-2vp comments 432, 434, 439 |
| M2 | Outcome "not reproduced": SHARED retained after the probe on every leg | plan v2 §8.2; ub-2vp comments 432, 434, 439 |
| M3 | Outcome "environment split": M3a CI Linux vs CI macOS disagree, M3b local macOS vs CI macos-15 disagree | plan v2 §8.2; ub-2vp comments 432, 434, 439 |
| M4 | Outcome "HEAD and the real v1.0.2 binary disagree" | plan v2 §8.2; ub-2vp comments 432, 434, 439 |
| M5 | Outcome "instrument invalid": a control or detector red, or the witnesses disagree; hit once in Verify iteration 3 (fd3 baseline, root cause H4) | plan v2 §8.2; ub-2vp comments 432, 434, 439 |
| M6 | Outcome "guard red on main": fd4, fd7 or G3 red with valid instruments; stop and escalate | plan v2 §8.2; ub-2vp comments 432, 434, 439 |
| M7 | Outcome "reproduction/impact split": fd1 and fd5 (or v1.0.2 Runs 1 and 2) contradict | plan v2 §8.2; ub-2vp comments 432, 434, 439 |
| OQ1–OQ8 | Spec/Plan open questions (cell location, guarding pre-open, impact cells, repair window, holding v1.0.2 doctor, which binary, driver location, tracker metadata) | comment 431 |
| MR1–MR3 | Design-Review gate-1 rulings, all option A: two fd7 hooks; the measurement branch point and Verify interplay; a dist build for timing only | comment 432 |
| GF-1–GF-28 | The 28 deduplicated gate-1 findings | plan v2 §0; comment 432 |
| Arch / Rust / Research #n | Gate-1 finding n of the architect, rust-engineer and research-analyst lenses | plan v2 header; comment 432 |
| (a)–(d) | Miguel's Decide rulings: (a) reproduce on HEAD and on real v1.0.2; (b) v1.0.3 or not (deferred, then B1a); (c) permanent CI cell; (d) `nix` accepted | comments 430, 435 |
| B1a / B1b | The ruling (b) options: ship v1.0.3 / let v1.1 carry it | plan v2 §8.2; comment 435 |
| (a)–(d) of the M5 fix | One-buffer read, dedup, bounded re-read of both witnesses (N=10), extra evidence fields | comment 439 |
| V1 / V1b / V2 | Verify rulings: V1 match the device in the parsers; V1b fall back to inode-only when no line carries the stat device (btrfs); V2 fix testkit reaching shipped binaries with `precise-builds` | comments 436, 438 |
| H1–H4 | Hypotheses for the M5 flake: H1 merged range, H2 holder timing, H3 inode/device/pid selection defect (all refuted), H4 seq_file restart skips (root cause, confirmed by the stress run) | comment 439 |
| S1–S12 | The wave-1 spec edits of plan v2 §3.1 (PRD criterion (n), ci-cd rows and §2.1, crate plans, spine setters, (6)(c) citations) | plan v2 §3.1 |
| C1, C1b, C2, C2b, C3, C4, Cdist, WIP | The planned commits: spec, hooks, storage cells, engine cells, measured note, Track; Cdist the D17 fix; WIP the measurement commits | plan v2 §7; this report's Outcome |
| Q278–Q303 | d58 claims rows added by ub-2vp: Q278–Q282 spec text, Q283–Q291 storage cells, Q292–Q293 engine cells, Q294 fd6, Q295–Q303 measured note | scripts/checks/d58-health-full-claims.sh |
| Q242 / Q224–Q226 / Q240–Q241 | Existing d58 rows kept intact: the `--test exclusive_window` run line, the f1/f5/f4 cell names, the macOS job key and image | scripts/checks/d58-health-full-claims.sh; plan v2 §2.4 |
| P20 | The ub-lp9.25 row pinning the engine `--test dangling` run line | scripts/checks/ub-lp9.25-dangling-blocker-claims.sh |
| Run 0 / 1 / 2 / T | v1.0.2 procedure steps: Run 0 the `unblock mcp` control; Run 1 the held-doctor observation; Run 2 the impact hold; Run T unpinned timing runs | plan v2 §5; comment 434 |
| `UB2VP-MEASURE` / `UB2VP-STRESS` | Prefixes of the cells' evidence lines and of the draft-only stress line | exclusive_window.rs:996; comment 440 |
| `PROBE-V1` | The holder command that runs the verbatim v1.0.x probe | exclusive_window_holder.rs:244 |
| iteration 1–4 | Verify loop passes, iterations 3–4 authorised by Miguel | comments 436, 438, 439, 440 |
| attempt 1–20 | The twenty sequential re-runs of the `storage testkit` job on `e875482` (attempt 1 = job 134871125096, attempt 20 = job 134901450921) | CI run 44324250725; comment 441 |
| commits 1–7 | Comment 441's numbering of the re-cut history: the five code commits and the two Track commits listed in this report's Outcome | comment 441 |
| plan v2 | The Spec/Plan document after gate 1 (session-local, not in the repo) | comment 431 + 432 |

## Links

- `ub-2vp` — v1.0.x doctor's header probe drops the POSIX SHARED lock (this run); `ub-kpe` — the v1.0.3
  maintenance patch (ruling (b) = B1a); `ub-9m0` — related: foreign-process checkpoint data loss on the
  bundled SQLite 3.45.1.
- PR: https://github.com/websublime/unblock/pull/476
- Key files: `docs/PRD.md` (D17, D58), `docs/plans/01-design-spine.md`,
  `docs/plans/ci-cd-and-distribution.md`, `docs/plans/crates/unblock-storage.md`,
  `docs/plans/crates/unblock-engine.md`, `docs/plans/crates/unblock-health.md`,
  `docs/plans/implementation-plan.md`, `dist-workspace.toml`,
  `crates/unblock-storage/src/libsql/health.rs`, `crates/unblock-storage/tests/exclusive_window.rs`,
  `crates/unblock-storage/tests/bin/exclusive_window_holder.rs`,
  `crates/unblock-storage/tests/common/posix_locks.rs`, `crates/unblock-engine/tests/health.rs`,
  `crates/unblock-engine/tests/common/proc_locks.rs`, `scripts/checks/d58-health-full-claims.sh`,
  `.github/workflows/ci.yml`.
- Prior related run-reports: [2026-11-16-ub-lp9-3-fr16-health-full](2026-11-16-ub-lp9-3-fr16-health-full.md),
  [2026-11-17-ub-9ov-rmcp3-advisories](2026-11-17-ub-9ov-rmcp3-advisories.md).
