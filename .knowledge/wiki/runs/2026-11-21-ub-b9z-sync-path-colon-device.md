---
name: 2026-11-21-ub-b9z-sync-path-colon-device
description: The sync-path colon and device-name guard on main for v1.1 (tracker ub-b9z) — validate_sync_path refuses, lexically and before any filesystem probe, a file name containing a colon (on Windows also any other normal component) as ColonInName and a Windows reserved device name as ReservedDeviceName, both PATH_TRAVERSAL exit 6 and never relaxed by allow_external; the .git check is split around the new step, three public predicates serve the configured-name follow-up, two design-Review rounds ended in Miguel's escalation ruling, and the v1.0.4 maintenance slot opens for the 1.0 backport.
type: run
date: 2026-11-21
branch: ub-b9z-sync-path-colon-device
pr: '483'
issues: [ub-b9z]
---

# Run — the sync-path colon and device-name guard on main (ub-b9z)

## Context

Issue `ub-b9z` (bug, P2, discovered from `ub-c15`, the v1.1 fix that made every JSONL operation
refuse the session's own database): on Windows an MCP `sync` path such as
`.unblock\unblock.db:x.jsonl` passed `validate_sync_path`'s allowlist and named an NTFS alternate
data stream of the live database, and a leaf such as `NUL.jsonl` named a device. The run went from
2026-11-20 to 2026-11-21 on branch `ub-b9z-sync-path-colon-device` in the isolated worktree
`../unblock-b9z`, base main `b3f63b95`, draft PR #483; the 1.0 backport ran in parallel on
`ub-b9z-v1-0-x-sync-path-colon-device` (`../unblock-b9z-v1.0.x`, base `release/v1.0.x` `b21c73bd`,
draft PR #484) and has its own run-report. Phases and gates (tracker comments on `ub-b9z`):
Understand map (490); Decide, Miguel's rulings 491.1–491.8 (491); plan v1 (493–494) with the
sub-fork rulings 492.1–492.16 (492); design Review iteration 1 FAIL (495: 10 must-fixes, 14 nits,
4 sub-forks) and Miguel's gate rulings 496(a)–(d) (496); plan v2 (497–499) and ruling 500 on its one
sub-fork (500); iteration 2 FAIL, escalated (501: the architect lens failed with 2 must-fixes, all
text, plus 21 nits) and Miguel's escalation ruling 502.1–502.3 (502); plan v2.1 (503–505) LOCKED
(506); Implement as one WIP commit `dd1e6f08` (507); Verify gate PASS (508); this Track re-cut.
Team: one coordinator; three Understand lenses (architect, Rust engineer, research analyst); three
Spec/Plan section writers (spec text, code, tests and delivery); four design-Review lenses per
iteration (architect, Rust engineer, research analyst, QA/project manager); two implementers, one
per worktree; four Verify lenses (code, security, Rust, QA); two Track agents, one per line. All
subagents ran on Opus from ruling 496 on.

## What & why

The hazard (Understand map, comment 490; plan v2.1 §3.1; every Windows runtime step
`[INFERENCE: not run on Windows]`; anchors at `b3f63b95`, before this fix):
- **Every device leaf** (`NUL`, `COM*`, `LPT*`, `AUX`, `PRN`, `CON`, `CONIN$`, `CONOUT$`; e.g.
  `.unblock\NUL.jsonl`, which passes the allowlist, `path.rs:578-588`) passed steps (1)–(7) on
  Windows 10 and earlier as a new file.
  - Step (1)'s ancestor loop opened `NUL`/`COM*`/`LPT*` with a zero-access `canonicalize`, whose
    `GetFinalPathNameByHandleW` then fails (Stack Overflow 46385457); the loop ignores the error
    and moves on (`path.rs:451-461`).
  - Step (6)'s `normalized.exists()` (`path.rs:394`) is `fs::metadata(…).is_ok()` (Rust 1.96.0
    `library/std/src/path.rs:3517-3519`): a zero-access open plus `GetFileInformationByHandle`
    (`library/std/src/sys/fs/windows.rs:1490-1497`, `:523-526`), which fails on character devices
    (eryksun, bpo-37074 msg343781); `CON` already fails the zero-access open
    (rust-lang/rust#83751). So `exists()` is false and `symlink_metadata` never runs.
  - **Import** (incl. `dry_run` and `import_bd`): `open_guarded`'s `std::fs::metadata`
    (`conflict.rs:78-82`) fails the same way, so the import ends as `SyncError::Io` ("reading
    metadata for") before any data is read; no device is read and nothing hangs.
  - **Export**: `allocate_temp_file`'s `create_new` (`atomic.rs:243-247`) opens
    `<leaf>.<pid>.tmp`, which has the same device stem, for write. Whether a device accepts
    `CREATE_NEW` depends on its driver (Microsoft, CreateFileW: "Communications Resources" requires
    `OPEN_EXISTING`, "Consoles" advises it). Where the device accepts it, as `NUL` is expected to
    and `CON` as console output, the export lines are written into the device, and the export then
    fails at the rename.
- **Windows 11.** Forms with an extension are plain files, which other tools mishandle (Super User
  1929654; .NET, File path formats, "Handle legacy devices"; dotnet/runtime#52282 reports the
  relaxation on some late Windows 10 builds, so the Windows 10/11 boundary stays `[INFERENCE]`); a
  bare `NUL` is still the device, but a bare name fails the allowlist (`DisallowedName`) anyway.
- **Streams** (`unblock.db:x.jsonl`): the temp `create_new` created a named stream on the live
  database and the export was written into it.

After this fix every one of these is refused at step (1a), lexically, before step (1)'s ancestor
loop, so no stream and no device is opened for a **refused** name. Caveat: a **folder** named like
a device (`.unblock\NUL.jsonl\..\x.jsonl`, or `.unblock\aux\x.jsonl`), which the leaf-only device
rule accepts (ruling 492.2), is still resolved by the `.git` ancestor loop on Windows 10 and
earlier, and, for a folder named `NUL` (also `nul.` and `nul `), on Windows 11 too
`[INFERENCE]`; that class belongs to the non-portable-names follow-up (Links).

The design (rulings 491, 492, 496, 500, 502; authoritative text in PRD NFR-7 and FR-8, the
`unblock-sync` crate plan and the `path.rs` module doc; no new D-id, no MCP contract bump,
`unblock.mcp.v1.13` stays): `reject_git_path` splits into `reject_git_component` (the lexical
`.git` test on the raw path) and `reject_git_ancestors` (the `dunce::canonicalize` loop), and the
new step (1a) runs between them, on the raw path, under every `allow_external`. It refuses a leaf
containing `:` on every platform as `PathReject::ColonInName` (an NTFS stream on Windows, a name a
committed `.unblock/` tree cannot carry to Windows elsewhere), on Windows also every
`Component::Normal` containing `:` (prefix components such as `C:` and `\\?\C:` never checked; the
branch is chosen by passing `cfg!(windows)` as an argument, so the folder loop is unit-tested on
unix), and a leaf whose stem (the bytes before the first `.`, trailing spaces trimmed, any ASCII
case) is `CON`, `PRN`, `AUX`, `NUL`, `COM`/`LPT` plus one digit 0–9 or superscript one, two or three,
`CONIN$` or `CONOUT$`, as `PathReject::ReservedDeviceName`, with or without an extension. Both map
to `PATH_TRAVERSAL`, exit 6. `has_stream_colon`, `leaf_name_reject` and `unportable_name_reject`
are public and re-exported from `lib.rs`, so the configured-name follow-up reuses one rule.

**Leaf refinement of ruling 491.1.** Ruling 491.1 says the leaf is `Path::file_name()`. The code
checks the `..`-aware lexical leaf instead (the last name the lexical `..` normalization leaves,
found by the allocation-free `lexical_leaf`), because the raw `file_name()` of
`root/ab:c.jsonl/x/..` is `None` while the opened name is `ab:c.jsonl`; pinned by
`colon_leaf_reached_through_dotdot_rejected` and the `mut-raw-leaf` mutant (Implement comment 507).
The check runs on the raw path, not the normalized one: on Windows `PathBuf::push` of a
`Component::Normal("d:s.jsonl")` re-parses it as a drive prefix and replaces the buffer.

**Behaviour change on Linux and macOS.** A working unix export named `ab:c.jsonl`, `aux.jsonl`,
`con.jsonl`, `com1.jsonl` or `NUL .jsonl` is newly refused (exit 6); the NFR-7 sentence states it
and its reason (portability: git for Windows refuses these names at checkout under
`core.protectNTFS`). No such name exists in either repo line.

**Reason changes for already-refused paths** (all still `PATH_TRAVERSAL`, exit 6; rulings 492.7
and 496(a)):
- `/../ab:c.jsonl`: `ParentEscape` → `ColonInName`.
- `con.txt`: `DisallowedExtension` → `ReservedDeviceName`.
- `NUL`: `DisallowedName` → `ReservedDeviceName`.
- On unix, a confine root named `aux` or `ab:c` targeted as `root/x/..`: `NonRegularFile` → the
  name reason.
- A bad name outside the confine root (`other_tmp/ab:c.jsonl`, `elsewhere/sub/NUL.jsonl`):
  `OutsideConfineRoot` → the name reason.
- A bad name under an ancestor symlink that escapes the root (`.unblock/./esc/ab:c.jsonl`):
  `SymlinkEscape` → the name reason.
- New with the split: a target with a bad name whose `.git` is found only by canonicalizing an
  existing ancestor (a symlink, or on a case-insensitive filesystem an alias such as `.GIT`; on
  Windows also `GIT~1` `[INFERENCE]`): `GitComponent` → `ColonInName`/`ReservedDeviceName`. The
  symlink case is pinned by `name_checks_precede_symlinked_git_ancestor`; the `.GIT` case was
  measured on macOS APFS by the Rust-engineer lens of design Review iteration 2 and is pinned by no
  cell. A raw-path `.git` still wins (`git_component_precedes_name_checks`).

The outside-root, symlink-escape and ancestor-`.git` classes are the three that the Rust-engineer
lens's differential run (49,392 path/flag pairs, old versus patched validator) added in iteration
2. Only health's `SessionDatabase` arm matches a reason, and it is unaffected.

**The `recover` consequence (main only).** A configured `jsonl_export_filename` that step (1a)
refuses (`aux.jsonl`, `ab:c.jsonl`), with conflict markers in that export, makes doctor plan
`RegenerateExport` (`crates/unblock-health/src/anomaly.rs:228-229`; its scan never calls the
validator) that `recover` can never perform: the evidence copy (`RunDir::copy_export`) fails as
`HealthError::RecoveryEvidenceWrite`, `IO_ERROR`, exit 8, at `online.fail`
(`crates/unblock-engine/src/session/lifecycle.rs:330`) before `storage.repair` (`:341`), skipping
any other online repair planned in that run, and the regeneration itself (`:356-360`) would be
refused as `PATH_TRAVERSAL`. Doctor's export scan still opens a configured `nul.jsonl` on Windows
`[INFERENCE]`. Both belong to the configured-name follow-up (its criterion 2; Links).

**Side benefit.** On Windows the every-component loop refuses `.git::$INDEX_ALLOCATION\x.jsonl`
lexically (the CVE-2019-1352 path shape: `::$INDEX_ALLOCATION` aliases a directory).

## Outcome

Design gate: iteration 1 FAIL (10 must-fixes, 14 nits, 4 sub-forks, all text, test inputs,
coverage, tracker text or delivery; the design held); iteration 2 FAIL, escalated (the architect
lens's 2 must-fixes, both text, and 21 nits); Miguel's ruling 502 took lock path (A): plan v2.1
applied every finding, re-measured out of the repo every change touching code or tests (rustfmt,
the touched cells, the three new mutants `mut-pop-resets`, `mut-port-digit-any-c2` and
`mut-trim-tab`, the Windows-target clippy run, the claims scripts on both lines), and was locked
after the architect lens confirmed PASS-WITH-NITS and the coordinator checked both must-fix texts
word for word (506). The re-measurement agreed with every Rust-engineer prescription on behaviour:
every new row green, every new mutant red on exactly its listed cells and surviving without the new
rows. The one difference was formatting: rustfmt splits one prescribed test line (the `two` row of
`colon_leaf_reached_through_dotdot_rejected`, a six-call `join` chain) over several lines, and the
plan carries rustfmt's form; that is not a re-convening trigger, so the Rust-engineer lens was not
re-convened.

Implement (507): one WIP commit per line, two implementers on disjoint worktrees, every cargo call
serialized through one `flock` lock, 4 cores, targeted. Main: `path::` 36 passed, `error::` 6,
`session_files` 10, `atomic_failure` 9, health `recovery_layout` 26; rustfmt, clippy pedantic
`-D warnings` (sync, sync with fault-injection, health), `cargo check -p unblock-cli`, doc-lint,
knowledge-lint, check-layering and 16 of 16 claims scripts green (the NFR-7 pin, claims row Q311,
included).

Verify (508): PASS. Four lenses PASS with no must-fix and no nit on either line:
- code: every touched file rebuilt from its base plus the plan's before/after texts equals the WIP
  byte for byte; exactly the 10 planned files; the six v1.0.4 slot texts once each, main only;
  no added line over 100 columns; NFR-7 on one physical line (Q311 gives 1 and 1);
  `reject_git_path` gone;
- security: no filesystem probe before step (1a) on any MCP `sync` entry point (export, import with
  `dry_run` true and false, `import_bd`), step (1a) independent of `allow_external`; fullwidth and
  lookalike colons, 8.3 short names, an interior NUL, `..namedfork/rsrc` and lone surrogates over
  MCP refuted or out of reach; every finding hit was already filed as a follow-up;
- Rust: the Windows-target clippy of the real `path.rs` in the plan's scratch harness
  (`cargo +1.96.0 clippy --all-targets --target x86_64-pc-windows-msvc -D warnings`) exit 0, a
  type-error probe in `colon_in_directory_rejected_on_windows` red (`error[E0308]`), restored
  green; real-crate clippy exit 0; no heap allocation on the accept path, no `unwrap`, `expect` or
  `unsafe`;
- QA: every mutant of plan §4.3 plus the `NUL.jsonl`-row variant of `mut-step-before-git`, 34 of 34,
  turned exactly its planned cells red; `mut-windows-branch-disconnected` and `mut-prefix-checked`
  survive off Windows as planned (ruling 491.6); `mut-drop-reexport` fails the end-to-end cell to
  compile (`error[E0432]`, all three names).
CI run 44905074604 on `dd1e6f08`: 21 jobs green (test, exclusivity on macOS, windows-check, clippy
pedantic, fmt, doc-lint, layering, toolchain, audit, deny, and the rest), only `run-report-gate` red,
as expected before this report. The coordinator's one nit (plan §4.4 command 11, `cargo doc`, not
run at the WIP heads) was closed by the QA lens's addendum: `cargo doc -p unblock-sync --no-deps`
gives the 4 pre-existing private-link warnings and none naming a new item; commands 2, 4–6, 8–10
and 12–15 also passed locally.

Acceptance criteria (tracker, plan §1.4 and §4.7): (1) colon refusal, `PATH_TRAVERSAL` exit 6, never
relaxed, placed between the two halves of the `.git` check — covered; "no stream or device opened
before the refusal" covered by the ordering proxy (the Windows runtime effect is `[INFERENCE]`); the
Windows every-component wiring is honestly uncovered (no Windows test job, ruling 491.6). (2) the
device-name set, with or without an extension, bare `NUL` — covered. (3) temp path, `.git`
precedence, split order, negative controls, end-to-end re-export pins — covered. (4) NFR-7, FR-8,
crate plan, module doc; no D-id, no contract bump, no CI change — covered. (5) the backport pull
request (#484), the v1.0.4 cut after both merge and confirmed with Miguel, and the close in the
record pull request — pending; at this Track step the decline path was appended to criterion (5)
(a declined cut rewrites the v1.0.4 slot in a docs pull request on `main` that closes `ub-b9z`,
rulings 496(d), 500 and 502.2).

Track re-cut the WIP into the commits of plan v2.1 §5.2; the tree outside `.knowledge/` and
`.unblock/` equals the WIP tree, except `Cargo.lock`, which comes from the ub-c0t base (url 2.5.9):
- `82a191b2` docs(spec): refuse a colon or a Windows device name in a sync path name (NFR-7, FR-8) (ub-b9z)
- `2a5ac3d9` docs(roadmap): open the v1.0.4 maintenance slot (ub-b9z)
- `e0956ff3` fix(sync): refuse a colon or a Windows device name in a sync path before any filesystem probe (ub-b9z)
- `4805d298` test(sync): colon, device-name, order and re-export cells for validate_sync_path (ub-b9z)
- then this report, and the tracker re-export.

Rebased on 2026-11-23 onto `36029f88` (ub-c0t, url 2.5.9; Miguel's merge-order ruling, ub-c0t
comment 513 and ub-b9z comment 514); code and docs unchanged.

At every SHA, all 16 `scripts/checks/*.sh`, `cargo xtask doc-lint` and `cargo xtask
knowledge-lint` passed, and at the two code commits a 4-core `cargo check -p unblock-sync
--all-targets` (the fix commit also without `--all-targets`). Track's tracker edits: the criterion
(5) sentence above; a comment on the `.git`-alias follow-up naming the split of `reject_git_path`
(comment 509 there); the configured-name follow-up re-read, no comment needed (no public name or
signature changed in Implement).

## Gotchas

- A test input with one ASCII letter before `:` (`a:b.jsonl`) parses on Windows as a drive prefix,
  and `Path::join` then replaces the whole path; every test input puts at least two characters
  before every `:`, except the two deliberate drive-prefix rows of `colon_component_cases`.
- Checking the normalized path would miss the stream on Windows: `normalize_lexically` rebuilds the
  path with `PathBuf::push`, which re-parses `d:s.jsonl` as a prefix. Check the raw path, and find
  the leaf lexically.
- `Path::ancestors()` yields the path itself first, so the old `.git` ancestor loop
  `canonicalize`d the target (a zero-access open on Windows) before any later step; that is why the
  `.git` check had to be split, not merely followed.
- The NFR-7 sentence must stay on the same physical line as the existing `ub-c15` text: claims row
  Q311 anchors the `- **NFR-7 [security]**` line and fails any anchored line without its
  requirement (negative control measured in plan v2.1 §2).
- Knowledge-lint k6 scrubs inline code spans, so cell and mutant names in backticks never need a
  glossary row; the plan and every tracker comment of this run were kept free of bare
  session-local codes (0 hits).
- `docs/roadmap.html` is always substantive for the run-report gate (fail-closed for `*.html`), so
  the v1.0.4 slot commit alone already requires this report on the branch.
- Verify's lenses relied on CI for several plan §4.4 commands and none ran `cargo doc`; no CI job
  runs it. The gap surfaced only in the coordinator's obligation table, and the QA lens closed it
  with local runs.

## Glossary

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-----------------------------------------------|
| step (1a) | The new lexical name check of `validate_sync_path`, between the lexical `.git` test and the `.git` ancestor resolution | `crates/unblock-sync/src/path.rs` module doc; crate plan `src/path.rs` row |
| leaf | The last normal component the lexical `..` normalization leaves: the name actually opened | plan v2.1 terms; `lexical_leaf` in `path.rs` |
| stem | The leaf's bytes before its first `.`, trailing ASCII spaces removed, compared ignoring ASCII case | plan v2.1 §3.1 |
| device name | A Windows reserved DOS device name (`CON`, `PRN`, `AUX`, `NUL`, `COM`/`LPT` plus one port digit, `CONIN$`, `CONOUT$`) | plan v2.1 §3.1; ruling 492.3 |
| ruling 491.N | Miguel's Decide rulings 1–8 | comment 491 |
| ruling 492.N | Miguel's rulings on the plan v1 sub-forks 1–16 | comment 492 |
| ruling 496(a)–(d) | Miguel's rulings on the iteration-1 gate sub-forks (split, path-level predicate, follow-ups, close timing) | comment 496 |
| ruling 500 | The v1.0.4 slot opens in the main pull request's second commit | comment 500 |
| ruling 502.N | Miguel's escalation ruling: lock path (A), close of a declined cut, slot in commit 2 | comment 502 |
| must-fix N / nit N | Numbered findings of the design-Review verdicts (iteration 1: comment 495; "iteration-2" ones: comment 501) | comments 495, 501 |
| claims row Q311 | The d58 claims row anchoring the NFR-7 line | `scripts/checks/d58-health-full-claims.sh` |
| claims row Q369 | The d58 claims row pinning the windows-check compile line, untouched | `scripts/checks/d58-health-full-claims.sh` |
| windows-check | The CI job compiling `unblock-cli` for `x86_64-pc-windows-msvc` (library code only, no tests) | `.github/workflows/ci.yml` |
| plan §4.4 command N | The numbered local verification commands of the locked plan (16 = the run-report gate) | plan v2.1 §4.4 (comments 503–505) |
| `mut-*` | The non-vacuity mutants of plan v2.1 §4.3, by name | plan v2.1 §4.3; comment 508 |
| T3.19 | The implementation-plan task for the v1.0.4 slot | `docs/plans/implementation-plan.md` |

## Links

- `ub-b9z` — this run. Follow-ups, each `discovered-from` `ub-b9z`, not implemented here (ruling
  496(c)): `ub-gp0` — the configured-name follow-up (`jsonl_export_filename`, `db_filename`,
  `--db`), reusing `leaf_name_reject` / `unportable_name_reject`, and owning the `recover`
  consequence and doctor's export scan; `ub-iem` — the non-portable-names follow-up (`<>"|?*`,
  control bytes, a trailing dot or space, device names in folders); `ub-1as` — the `.git` NTFS
  aliases (`.GIT`, `.git.`, `GIT~1`); `ub-wer` — `\\.\` and `\\?\GLOBALROOT\` device-namespace
  paths stopped only by containment while `ExportOptions.allow_external` is public. Prior: `ub-c15`
  (the session-file JSONL guard that discovered this issue).
- PR #483 (main), PR #484 (the `release/v1.0.x` backport). Key files:
  `crates/unblock-sync/src/path.rs`, `crates/unblock-sync/src/error.rs`,
  `crates/unblock-sync/src/lib.rs`, `crates/unblock-sync/tests/session_files.rs`, `docs/PRD.md`
  (NFR-7, FR-8), `docs/plans/crates/unblock-sync.md`, `docs/plans/00-roadmap.md` (v1.0.4).
- Sources: Microsoft, Naming Files, Paths, and Namespaces; Microsoft, File Streams; Microsoft,
  CreateFileW; .NET, File path formats on Windows; Chris Denton, Special DOS Device Names; J.
  Forshaw, Win32 to NT path conversion; git `compat/mingw.c` `is_valid_win32_path`.
- Prior: [2026-11-20-ub-c15-session-file-jsonl-guard](2026-11-20-ub-c15-session-file-jsonl-guard.md),
  [2026-11-19-ub-kpe-v1-0-x-doctor-probe-fix](2026-11-19-ub-kpe-v1-0-x-doctor-probe-fix.md).
