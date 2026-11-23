---
name: 2026-11-21-ub-b9z-v1-0-x-sync-path-colon-device
description: The release/v1.0.x backport of the sync-path name guard (tracker ub-b9z) — validate_sync_path now refuses, before any filesystem probe and even under allow_external, a file name that holds a colon (an NTFS alternate data stream on Windows) or is a Windows reserved device name, with two new PathTraversal reasons, three public lexical predicates and the v1.0.4 NFR-7 and FR-8 texts; two design-Review iterations and an escalation, a mutant sweep at Verify, and a behaviour change for such names on Linux and macOS inside a patch release.
type: run
date: 2026-11-21
branch: ub-b9z-v1-0-4-record
pr: pending
issues: [ub-b9z]
---

# Run — v1.0.4 backport: refuse a colon or a Windows device name in a sync path name (ub-b9z)

## Context

Issue `ub-b9z` (bug, P2, discovered from `ub-c15`): on Windows a sync path such as
`.unblock\unblock.db:x.jsonl` passes the allowlist of `validate_sync_path` and names an NTFS alternate
data stream of the live database, and a leaf such as `NUL.jsonl` names a device. This report covers the
1.0-line pull request; `main` has its own pull request (#483) and its own run-report. The run went from
2026-11-20 to 2026-11-21 on branch `ub-b9z-v1-0-x-sync-path-colon-device`, cut from
`origin/release/v1.0.x` at `b21c73bd` (`release: v1.0.3`, version 1.0.3), in the isolated worktree
`../unblock-b9z-v1.0.x`. PR: #484 (draft, base `release/v1.0.x`). Phases and gates, each recorded as a
tracker comment on `ub-b9z`: Understand (three read-only lenses, comment 490); Decide (Miguel's rulings,
comment 491); Spec/Plan v1 (comments 493–494) and its sub-fork rulings (comment 492); design Review
iteration 1 FAIL (comment 495) and the gate rulings (comment 496); plan v2 (comments 497–499) and the
slot ruling (comment 500); design Review iteration 2 FAIL, escalated to Miguel (comment 501), and his
escalation ruling (comment 502); plan v2.1, the locked plan (comments 503–505), locked in comment 506;
Implement as one work-in-progress commit per line (comment 507); Verify PASS (comment 508); then this
Track step. Team: a coordinator, three Understand lenses, three Spec/Plan writers, four design-Review
lenses per iteration (architect, Rust engineer, research analyst, QA/project manager), two implementers
(one per line), four Verify lenses (code, security, Rust, QA) and one Track agent per line.

## What & why

Nothing in `validate_sync_path` looked at `:` or at device names, so on Windows an MCP `sync` export to
`unblock.db:x.jsonl` would allocate its temp as a named stream on the live database and write the export
into it, and a device leaf would be probed, opened or written as a device (the hazard list in Outcome).
Miguel's Decide rulings (comment 491) chose a hybrid scope: the leaf containing `:` is refused on every
platform, every normal component containing `:` is refused on Windows, a drive or UNC prefix is never
checked; new `PathReject::ColonInName` and `PathReject::ReservedDeviceName`, both mapped to the existing
`PATH_TRAVERSAL` (exit 6) with no MCP contract bump (`unblock.mcp.v1.13`); Windows reserved device names
folded in; no CI change; the backport as a separate pull request on `release/v1.0.x`, folded into this
issue; the decision recorded inline in NFR-7, FR-8, the `unblock-sync` crate plan and the module doc, with
no new D-id. The configured names (`jsonl_export_filename`, `db_filename`, `--db`) are split to `ub-gp0`.
The design-Review rulings (comment 496) split the old `.git` check into its lexical half and its
ancestor-canonicalization half and put the new step between them, so no filesystem probe of the path
precedes the refusal, and made a path-level predicate public next to the two name predicates.

The resulting order of the calls in `validate_sync_path` (1.0 `crates/unblock-sync/src/path.rs`): step
(1) `reject_git_component(path)?` (the raw-path `.git` test, `:312`); step (1a)
`if let Some(reason) = unportable_name_reject(path)` (`:318`), on the raw path, under every
`allow_external`; then `reject_git_ancestors(path)?` (`:326`), and steps (2)–(7) unchanged. The public
predicates are `has_stream_colon` (`:474`), `leaf_name_reject` (`:483`) and `unportable_name_reject`
(`:499`), re-exported from `lib.rs`. A device name is a leaf whose stem (the bytes before the first `.`,
trailing ASCII spaces removed, compared ASCII-case-insensitively) is `CON`, `PRN`, `AUX`, `NUL`,
`CONIN$`, `CONOUT$`, or `COM`/`LPT` plus one digit 0–9 or a superscript one, two or three; `CLOCK$` and
two-digit ports are not in the set.

**Leaf refinement of ruling 491.1.** The ruling says `Path::file_name()`; the leaf actually checked is
the last normal component that the lexical `..` normalization leaves (`lexical_leaf`, `:559`), so
`root/ab:c.jsonl/x/..` is refused too. Pinned by `colon_leaf_reached_through_dotdot_rejected` and the
`mut-raw-leaf` mutant (Implement comment 507).

**Behaviour change on Linux and macOS inside a patch release.** v1.0.4 newly refuses (exit 6) a sync
path whose file name contains `:` or whose stem is a device name on every platform, so a unix export that
works on v1.0.3 to `.unblock/ab:c.jsonl`, `aux.jsonl`, `con.jsonl`, `com1.jsonl` or `NUL .jsonl` fails
after the upgrade. The reason is portability: such a `.unblock/` tree cannot be checked out or used on
Windows (git for Windows refuses those names). The 1.0 NFR-7 text states the change and its reason; no
such name exists in either repo line.

Authoritative text on the branch: PRD NFR-7 (the v1.0.4 `ub-b9z` sentence on the NFR-7 line) and FR-8
(the `[ub-b9z]` item in the path-confinement list); the `unblock-sync` crate plan (`src/lib.rs`,
`src/error.rs` and `src/path.rs` rows, the API sketch); the module doc of `path.rs`.

## Outcome

Implemented as one work-in-progress commit, `6237fd1b`; Track re-cut it into the four atomic commits of
plan v2.1 §5.3, whose final tree equals `6237fd1b` outside `.knowledge/` and `.unblock/`, except
`Cargo.lock`, which comes from the ub-c0t base (url 2.5.9):
- `cd6b34e3` docs(spec): refuse a colon or a Windows device name in a sync path name (NFR-7, FR-8) (ub-b9z) — `docs/PRD.md`, `docs/plans/crates/unblock-sync.md`
- `980a7731` fix(sync): refuse a colon or a Windows device name in a sync path before any filesystem probe (ub-b9z) — `path.rs` (module doc, variants, step (1a), the split, the predicates), the `error.rs` doc, the `lib.rs` re-export
- `e5afe6e0` test(sync): colon, device-name, order and re-export cells for validate_sync_path (ub-b9z) — `path.rs` and `error.rs` test modules, `tests/session_files.rs`
- `chore(tracker): land the ub-b9z run-report and re-export the record on the 1.0 line (ub-b9z)` — this report, its `index.md` line and the full tracker re-export in one commit, the shape of plan v2.1 §5.3 (the `4820d3b5` precedent); the rebased base export already carries `ub-b9z` (from `ub-c0t`'s re-export), so the shape is kept as planned, not forced by knowledge-lint k4; the 44 `ub-kpe` glossary rows landed with `ub-c0t`'s 1.0 pull request #486, so this commit no longer touches that report (see Gotchas)

Rebased on 2026-11-23 onto `03ddbc23` (ub-c0t, url 2.5.9; Miguel's merge-order ruling, ub-c0t comment 513
and ub-b9z comment 514); code and docs unchanged.

**No slot texts on this line.** The v1.0.4 PLANNED slot (roadmap, `CLAUDE.md`, `docs/roadmap.html`,
implementation-plan T3.19) opens on `main` only, in #483 (rulings 500 and 502.3); the branch carries only
what its code needs.

**Cells on this line.** 19 new `path::` cells (one of them, `colon_in_directory_rejected_on_windows`,
`#[cfg(windows)]`): `colon_in_leaf_rejected`, `colon_in_leaf_rejected_even_under_allow_external`,
`colon_in_sidecar_leaf_rejected` (literal sidecar names, since this line has no sidecar-suffix constant),
`colon_in_temp_leaf_rejected`, `git_component_precedes_name_checks`,
`name_checks_precede_symlinked_git_ancestor` (unix), `colon_precedes_parent_escape_and_extension`,
`colon_precedes_containment_and_existence`, `colon_leaf_reached_through_dotdot_rejected`,
`colon_in_directory_accepted_on_unix` (unix), `non_utf8_leaf_with_colon_rejected` (unix),
`colon_component_cases`, `reserved_device_name_rejected`, `device_like_names_accepted`,
`leaf_name_reject_cases`, `colon_checked_before_device_name`, `issues_jsonl_still_accepted`,
`lexical_leaf_cases`; the error cell `name_rejects_map_to_path_traversal_exit_6`; and the end-to-end cell
`colon_and_device_name_targets_are_refused_for_every_action` in `tests/session_files.rs`, which runs the
three 1.0 actions (no `dry_run` row and no external-override rows on this line's harness) and pins the
three public re-exports. CI on this line: `test (workspace)` runs every cell on Linux;
`exclusivity-macos` runs the engine and CLI lock-retention tests, `unblock-health --lib` and, from
`unblock-sync`, only `--test session_files` (1.0 `.github/workflows/ci.yml:158-167`), so the end-to-end
cell is the only `ub-b9z` cell CI runs on macOS (ruling 492.12).

**Hazard before this fix** (every Windows runtime step is `[INFERENCE: not run on Windows]`, with the
sources of plan v2.1 §3.1; 1.0 anchors at `b21c73bd`):
- **Every device leaf** (`NUL`, `COM*`, `LPT*`, `AUX`, `PRN`, `CON`, `CONIN$`, `CONOUT$`; e.g.
  `.unblock\NUL.jsonl`, which passes the allowlist, `path.rs:518-528`) passes steps (1)–(7) on Windows 10
  and earlier as a new file.
  - Step (1)'s ancestor loop opens `NUL`/`COM*`/`LPT*` with a zero-access `canonicalize`, whose
    `GetFinalPathNameByHandleW` then fails (Stack Overflow 46385457); the loop ignores the error and
    moves on (`path.rs:391-401`).
  - Step (6)'s `normalized.exists()` (`path.rs:334`, step (6) `:334-348`) is `fs::metadata(…).is_ok()`
    (Rust 1.96.0 `library/std/src/path.rs:3517-3519`): a zero-access open plus
    `GetFileInformationByHandle` (`library/std/src/sys/fs/windows.rs:1490-1497`, `:523-526`), which fails
    on character devices (eryksun, bpo-37074 msg343781); `CON` already fails the zero-access open
    (rust-lang/rust#83751). So `exists()` is false and `symlink_metadata` never runs.
  - **Import** (`dry_run` and `import_bd` included): `open_guarded`'s `std::fs::metadata`
    (`conflict.rs:78-82`) fails the same way, so the import ends as `SyncError::Io` ("reading metadata
    for") before any data is read; no device is read and nothing hangs.
  - **Export**: `allocate_temp_file`'s `create_new` (`atomic.rs:242-246`) opens `<leaf>.<pid>.tmp`,
    which has the same device stem, for write. Whether a device accepts `CREATE_NEW` depends on its driver
    (Microsoft, CreateFileW: "Communications Resources" requires `OPEN_EXISTING`, "Consoles" advises it).
    Where the device accepts it, as `NUL` is expected to and `CON` as console output, the export lines are
    written into the device, and the export then fails at the rename.
- **Windows 11.** Forms with an extension are plain files, which other tools mishandle (Super User
  1929654; .NET, File path formats, "Handle legacy devices"; dotnet/runtime#52282 reports the relaxation
  on some late Windows 10 builds, so the Windows 10/11 boundary stays `[INFERENCE]`); a bare `NUL` is
  still the device, but a bare name fails the allowlist (`DisallowedName`) anyway.
- **Streams** (`unblock.db:x.jsonl`): the temp `create_new` creates a named stream on the live database
  and the export is written into it (Understand map, comment 490).

After this fix every one of these is refused at step (1a), lexically, before step (1)'s ancestor loop, so
no stream and no device is opened for a **refused** name. Caveat: a **folder** named like a device
(`.unblock\NUL.jsonl\..\x.jsonl`, or `.unblock\aux\x.jsonl`), which the leaf-only device rule accepts
(ruling 492.2), is still resolved by the `.git` ancestor loop on Windows 10 and earlier, and, for a
folder named `NUL` (also `nul.` and `nul `), on Windows 11 too `[INFERENCE]`; that class is `ub-iem`.
Side benefit: on Windows the every-component check refuses `.git::$INDEX_ALLOCATION\x.jsonl` lexically
(the CVE-2019-1352 path shape).

**Reason changes for paths that were already refused** (all still `PATH_TRAVERSAL`, exit 6; rulings
492.7 and 496(a)):
- `/../ab:c.jsonl`: `ParentEscape` → `ColonInName`.
- `con.txt`: `DisallowedExtension` → `ReservedDeviceName`.
- `NUL`: `DisallowedName` → `ReservedDeviceName`.
- On unix, a confine root named `aux` or `ab:c` targeted as `root/x/..`: `NonRegularFile` → the name
  reason.
- A bad name outside the confine root (`other_tmp/ab:c.jsonl`, `elsewhere/sub/NUL.jsonl`):
  `OutsideConfineRoot` → the name reason.
- A bad name under an ancestor symlink that escapes the root (`.unblock/./esc/ab:c.jsonl`):
  `SymlinkEscape` → the name reason.
- New with the split: a target with a bad name whose `.git` is found only by canonicalizing an existing
  ancestor (a symlink, or on a case-insensitive filesystem an alias such as `.GIT`; on Windows also
  `GIT~1` `[INFERENCE]`): `GitComponent` → `ColonInName`/`ReservedDeviceName`. The symlink case is pinned
  by `name_checks_precede_symlinked_git_ancestor`; the `.GIT` case was measured on macOS APFS by the
  Rust-engineer lens of design Review iteration 2 and is pinned by no cell. A raw-path `.git` still wins
  (`git_component_precedes_name_checks`).

This line's own consequences: it has no health caller of `validate_sync_path` (the callers are
`atomic.rs:242,333-334`, `export.rs:67` and `import.rs:172`) and no `recover` (1.0
`crates/unblock-engine/src/session/lifecycle.rs:341-343` returns `FeatureNotWired`), so the `recover`
consequence that `main`'s run-report records does not exist here, and no code outside `unblock-sync`
matches a `PathReject` on this line.

**Verify (comment 508): PASS.** Four lenses (code, security, Rust, QA), each PASS with no must-fix and no
nit on either line; the coordinator's one nit (the local §4.4 runs, `cargo doc` in particular) was closed
by the QA lens before Track.
- Code lens: every touched file rebuilt from its base plus the plan's before/after texts matches the
  work-in-progress head byte for byte; this line touches exactly the six planned files; no added line is
  over 100 columns.
- Security lens: no filesystem probe precedes step (1a) on any MCP `sync` entry point, and step (1a) does
  not depend on `allow_external`; the findings it hit were already filed as `ub-wer`, `ub-iem`, `ub-1as`
  and `ub-gp0`.
- Rust lens: the Windows-target clippy of plan v2.1 §4.4 command 17 (`x86_64-pc-windows-msvc`,
  `-D warnings`) exits 0 on this line; a type-error probe in `colon_in_directory_rejected_on_windows`
  turns it red (`error[E0308]` at `path.rs:969:26`); real-crate clippy `-p unblock-sync --all-targets`
  exits 0.
- QA lens: 34 of 34 mutants match their planned red sets on this line — the nine plan v2.1 requires
  (`mut-drop-step`, `mut-drop-device`, `mut-relaxed-by-external`, `mut-drop-reexport`,
  `mut-step-after-git-ancestors`, the `NUL.jsonl`-row variant of `mut-step-before-git`,
  `mut-pop-resets`, `mut-port-digit-any-c2`, `mut-trim-tab`) with commands 1 and 3, and 25 more with
  command 1. Under `mut-relaxed-by-external` the end-to-end cell stays green as planned (no override rows
  here; three lib cells kill it). `mut-windows-branch-disconnected` and `mut-prefix-checked` survive off
  Windows, as planned (ruling 491.6; no Windows test runner exists).
- CI on #484 at `6237fd1b` (run 44905078336, release plan 44905078259): 21 jobs pass, including
  `test (workspace)`, `exclusivity`, `windows-check`, clippy, fmt and doc-lint; `run-report-gate` failed
  as expected until this report landed.
- Local runs at the work-in-progress head: `path::` 31 passed, `error::` 6, `session_files` 9,
  `atomic_failure` (fault-injection) 9; fmt, clippy with fault-injection and `check -p unblock-cli`
  clean; `cargo doc -p unblock-sync --no-deps` shows 3 warnings, all pre-existing private-link warnings
  (`import_bd` → `map_bd_record` and `apply_records`, `export_jsonl` → `corpus_closed_under_blockers`),
  none naming a new item; doc-lint, knowledge-lint and check-layering OK; 12 of 12 claims scripts green.

**Track.** At every commit of the re-cut: all 12 `scripts/checks/*.sh`, `cargo xtask doc-lint` and
`cargo xtask knowledge-lint` green; `cargo check -p unblock-sync` (and `--all-targets`) green with no
warning at the fix commit; `cargo check -p unblock-sync --all-targets` green at the test commit. At the
re-cut code tree (`e5afe6e0`, identical to the work-in-progress code): `path::` 31 passed, `error::` 6,
`session_files` 9; `cargo doc -p unblock-sync --no-deps --locked` exits 0 with the same 3 pre-existing
warnings and none naming a new item. After the re-export at the original base, knowledge-lint reported no
finding on this report and 115 k6 findings (44 distinct tokens) on the `ub-kpe` run-report; those 44 rows
landed with `ub-c0t`'s 1.0 pull request #486, so after the rebase this commit no longer touches that
report (see Gotchas).

**Residual hazards, out of scope, each owned.** Configured names (`db_filename`, `--db`,
`jsonl_export_filename`) — `ub-gp0`; on the 1.0 line configured names stay unrefused, because `ub-gp0`
is `main`/v1.1 only (1.0 `crates/unblock-config/src/merge.rs:110-112` keeps `jsonl_export_filename`
configurable, and the doctor scans it without the validator, `crates/unblock-health/src/file_state.rs:427`).
The rest of git's Win32-invalid set and device names in folders — `ub-iem`, which will also move
`NUL\t.jsonl` out of the device-like accepted list when it lands. The `.git` NTFS aliases — `ub-1as`.
`\\.\…` and `\\?\GLOBALROOT\…` prefixes are outside step (1a) and rely on containment alone, which
`allow_external` relaxes; v1 never sets it (1.0 `crates/unblock-engine/src/session/interchange.rs:37,79`)
but `ExportOptions.allow_external` is public (`crates/unblock-sync/src/export.rs:31`) — `ub-wer`. A
trailing dot or space on the whole leaf and 8.3 short names stay blocked by the allowlist.

**Pending (outside this PR):** Miguel's merge of #483 and then #484; his confirmation of the v1.0.4 cut
when it is due; rc.1, the final v1.0.4 and the `v1.0.3 → v1.0.4` update smoke; the record pull request on
`main` that copies this report and closes `ub-b9z`. If Miguel declines the cut, a docs pull request on
`main` (`ub-b9z-v1-0-4-not-cut`) rewrites the slot and closes `ub-b9z` (rulings 496(d), 500 and 502.2).

## Gotchas

- **The 1.0 tracker export lacked `ub-b9z` at the original base.** At `b21c73bd` a report-only commit would
  fail knowledge-lint k4, so the report, its index line and the re-export land in one commit (plan v2.1
  §5.3; the `4820d3b5` precedent). After the rebase the base export already carries `ub-b9z` (from
  `ub-c0t`'s re-export), so the one-commit shape is kept as planned, not forced by k4.
- **A fresh export widens k6 on the `ub-kpe` run-report.** Its `issues:` list names `ub-c15`, and the
  re-export brings `ub-c15`'s tracker comments 477–484, dated 2026-11-19 (the report's date), onto this
  line, so knowledge-lint k6 asked for 44 glossary rows there (measured at the original base). They are
  the 44 rows `main` already appended to its copy in `d2179915`; they landed on this line unchanged with
  `ub-c0t`'s 1.0 pull request #486, so after the rebase this commit no longer touches that report. No
  existing row, its `issues:` list or its date changed, and the two copies differ only in their `branch`
  and `pr` fields, as before.
- **A single letter before `:` is a drive on Windows.** `Path::join` of `a:b.jsonl` replaces the whole
  path there, so every test input puts at least two characters before each `:` (plan v2.1, "Terms").
- **Checking the normalized path would miss the colon.** `normalize_lexically` rebuilds with
  `PathBuf::push`, which on Windows turns `d:s.jsonl` into a drive prefix; step (1a) runs on the raw path
  and finds the leaf with `lexical_leaf` instead.
- **This line's harness is smaller.** No `dry_run` action, no external override and no fd witness in
  `tests/session_files.rs`, and no sidecar-suffix constant in `path.rs`, so the end-to-end cell has three
  actions and `colon_in_sidecar_leaf_rejected` uses literal names.
- **No CI job runs `cargo doc`.** Command 11 had no CI stand-in; the QA lens and Track ran it locally.
- **`flock` exists only as a shell builtin on this host.** A `#!/usr/bin/env bash` script run as a child
  process found no `flock`; the per-commit checks were re-run sourced into the shell that has it.

## Glossary

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-----------------------------------------------|
| step (1a) | The new lexical name check in `validate_sync_path`, between the raw-path `.git` test and the `.git` ancestor resolution | `crates/unblock-sync/src/path.rs:318` (1.0) |
| leaf | The last normal component that the lexical `..` normalization leaves: the name actually opened | `lexical_leaf`, `crates/unblock-sync/src/path.rs:559` (1.0) |
| stem | The leaf's bytes before its first `.`, trailing ASCII spaces removed; compared against the device list | `is_reserved_device_name`, `crates/unblock-sync/src/path.rs:527` (1.0) |
| ruling 491.N, 492.N, 496(x), 500, 502.N | Miguel's rulings on this issue: Decide, plan v1 sub-forks, design-Review iteration 1 sub-forks, the slot placement, the iteration-2 escalation | ub-b9z comments 491, 492, 496, 500, 502 |
| plan v1, v2, v2.1 | The Spec/Plan document across its review iterations; v2.1 is the locked plan (session-local, not in the repo) | ub-b9z comments 493–494, 497–499, 503–505, 506 |
| plan v2.1 §N | A section of the locked plan (e.g. §3.1 the device-name rule and hazard, §4.4 the local commands, §5.3 the 1.0 pull request, §6 the risks) | ub-b9z comments 503–505 |
| command N | The numbered local verification commands 1–17 of plan v2.1 §4.4 (1 `path::` tests, 2 `error::` tests, 3 `session_files`, 11 `cargo doc`, 16 `run-report-gate.sh`, 17 the Windows-target clippy) | plan v2.1 §4.4; ub-b9z comment 504 |
| `mut-…` | A named non-vacuity mutant: a deliberate one-site break that Verify applies to prove a cell is not vacuous (e.g. `mut-drop-step` deletes step (1a), `mut-raw-leaf` checks `Path::file_name()` instead of the lexical leaf) | plan v2.1 §4.3; ub-b9z comment 508 |
| iteration 1, iteration 2 | The two design-Review gate rounds, both FAIL; the second escalated to Miguel | ub-b9z comments 495, 501 |
| lens | One reviewer role in a gate (architect, Rust engineer, research analyst, QA, code, security) | ub-b9z comments 490, 495, 501, 508 |
| Q311 | The d58 claims row pinning main's NFR-7 line (main only; the 1.0 line has no d58 script) | main `scripts/checks/d58-health-full-claims.sh:493` |
| k4, k6 | knowledge-lint checks: k4 every frontmatter issue is in the export; k6 the mandatory sections and glossary token coverage | `xtask/src/knowledge_lint.rs:189-198`, `:1186-1410` |
| work-in-progress commit | The single Implement commit per line that Track re-cut (`6237fd1b` here) | ub-b9z comment 507 |

## Links

- `ub-b9z` — refuse a colon (an NTFS stream) or a Windows device name in a sync path name (this run);
  `ub-c15` — the v1.1 session-file guard it was discovered from; `ub-gp0` — refuse the same names in the
  configured file names and `--db` (`main`/v1.1); `ub-iem` — the other Windows-non-portable names and
  device names in folders; `ub-1as` — the `.git` NTFS aliases; `ub-wer` — device-namespace prefixes
  stopped only by containment while `ExportOptions.allow_external` is public.
- PRs: https://github.com/websublime/unblock/pull/484 (this line, draft) and
  https://github.com/websublime/unblock/pull/483 (`main`, draft).
- Key files: `docs/PRD.md` (NFR-7, FR-8), `docs/plans/crates/unblock-sync.md`,
  `crates/unblock-sync/src/path.rs`, `crates/unblock-sync/src/error.rs`, `crates/unblock-sync/src/lib.rs`,
  `crates/unblock-sync/tests/session_files.rs`.
- Prior related run-reports: `2026-11-19-ub-kpe-v1-0-x-doctor-probe-fix` (the v1.0.3 maintenance patch
  and the `4820d3b5` one-commit precedent); on `main`, `2026-11-20-ub-c15-session-file-jsonl-guard`.
