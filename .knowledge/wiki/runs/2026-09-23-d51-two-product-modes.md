---
name: 2026-09-23-d51-two-product-modes
description: Landing the two-product-modes decision as a spec-first documentation cascade (decision D51, tracker ub-b07) — 44 files and no code behaviour changed, every mechanical check green at every round while every finding either gate raised was prose truth, the design gate repaired nineteen defects inside the specification rather than sending it back, the Verify gate then failed twice and closed on a Miguel-ruled escalation, two of its second round's three defects sat in text a gate had itself ratified and an implementer had applied as instructed, a false and git-checkable attribution nearly shipped inside the decision row with the specification sharpening it rather than doubting it, and the check scripts were proven non-vacuous by mutation twice on two different scripts.
type: run
date: 2026-09-23
branch: ub-b07-two-product-modes
pr: '446'
issues: [ub-b07]
---

# Run — the two-product-modes cascade (D51)

## Context

Task `ub-b07` is the spec-first cascade that lands Miguel's product decision of 2026-09-22, which came
out of the `ub-w3a` research spike. The decision mints D51 in `docs/PRD.md` §4 and retires the
embedded-replica framing the roadmap had carried since 2026-07. Miguel chose to land it now rather than
at the v1.3 lock, so the decision would not age inside an issue comment while the documents said
something else.

The run went from 2026-09-22 to 2026-09-23, off `main` at 085e58c, on branch
`ub-b07-two-product-modes`. It is one commit, `d723a81`, amended twice — once after each failed Verify
round. No pull request is open at the time of writing.

Every phase ran as a Workflow in an isolated worktree. Understand ran three lenses plus a coordinator
that re-read the disputed sites rather than trusting the lens reports. Spec ran three drafters plus a
coordinator that reconciled them into one implement order. The design Review gate ran four adversarial
lenses plus a coordinator. Implement ran five sequential agents in one worktree. Verify ran three lenses
in three throwaway worktrees plus a coordinator, twice. The round-2 repair ran as a single
`rust-engineer`. Track is this report and the tracker re-export.

## What & why

The decision itself is recorded in `docs/PRD.md` §4 row D51, and this report points at it rather than
restating it. In short, unblock has two product modes. Local mode is one SQLite file on one machine,
which is what every shipped binary does today. Remote mode is one shared database reached over SQL over
HTTP through the `turso_serverless` crate, served by either a private sqld or Turso Cloud as peers, with
a dated status note on the sqld option. Remote mode lands at v1.3 (PROPOSED) and is in no shipped
binary. There is no offline in remote mode, reads included, and the accepted consequence is stated once.
Embedded replicas are excluded because their only Rust client fails this repository's own required
advisory job.

The cascade was written against these sections, read rather than paraphrased here.

- `docs/PRD.md` §4, the rows D51 supersedes — D1, D5, D13, D14, D15, D28, D31 and D41 — plus the Overview,
  the domain-model table, §8.2 and the NFR-3, NFR-10, NFR-17, NFR-18 and NFR-19 requirements.
- `docs/plans/00-roadmap.md` §4, the shared-state section, which was the clause under test.
- `docs/plans/01-design-spine.md` §3.4 and §6.7, where the second backend and the network-surface
  statement live.
- `docs/plans/crates/unblock-storage.md`, `unblock-health.md`, `unblock-sync.md`, `unblock-model.md`,
  `unblock-error.md` and `unblock-config.md`.
- `docs/PROCESS.md` §3 for the D-id and D-range rules, and §5 for escalation after two failed rounds.

Understand found the task description citing the wrong requirement. It pointed the cascade at NFR-6 for
the no-network clause, but NFR-6 is about git and the network clause is NFR-17. Cascading against NFR-6
would have missed NFR-17 and its transitive-surface twin NFR-10, which are the two sentences this
decision most falsifies. The same miscitation was live in `CLAUDE.md` and is wrong independently of this
decision.

Understand also settled that the acceptance sweep cannot be a bare zero-return gate. The new decision row
has to name embedded replicas and libsql's `replication` feature in order to record why they are
excluded, so the retired term survives on purpose. Only the forbidden-assertion half reaches zero.

## Outcome

### What landed

One commit, `d723a81`, 44 files, 568 insertions, 340 deletions. No crate gains code and no behaviour
changes. `CONTRACT_VERSION`, `CONTRACT_HASH`, the 0-8 exit table and the CLI surface are byte-unchanged,
so this is not a semver event under D35, and v1.3 stays PROPOSED. The storage `remote` cargo feature
keeps its name and stays an empty marker, which closes `ub-jv2`. The live decision range moves to
D1..D51 at every site `docs/PROCESS.md` §3 names, which is three prose statements plus thirteen knob
lines across seven check scripts.

No gate script was minted. The convention in this repository is that the range knobs ride the spec
commit while a script and its prose ride the implementation commit, and this decision has no
implementation commit. The precedent holds — the two earlier documentation-only decisions have no script
either. One consequence is recorded in the D51 row rather than left to be discovered. The newest
script's own live-range knob and its four prose lines are now pinned by nothing, and stay unpinned until
the next mint.

### The gate record

Every mechanical check was green at every round, and every finding either gate raised was prose truth.
No lint reaches any of them.

- Design Review (2026-09-23) returned PASS, and it passed in one round only because the gate repaired
  the specification instead of returning it. Three of the four lenses returned FAIL and each was right.
  Nineteen findings were applied inside the specification and eleven were dismissed with reasons. The
  three that decided it were the decision row turning three required CI steps red in the same commit
  that mints it, a false and git-checkable fact about to ship at four sites, and a sweep acceptance
  criterion that would have deleted the record the next decision needs.
- Verify round 1 (2026-09-23, on `29e8fe6`) returned FAIL with nine must-fixes, MF-A through MF-I. None
  was behaviour. The decisive one is MF-A, where the interface spine claimed the default build links no
  network symbol while the same commit said the opposite twice, in higher-authority documents that both
  cite the spine as the place that already says it precisely. The root cause is direct —
  `crates/unblock-cli/Cargo.toml:86` sets `default = ["self-update"]`, which pulls reqwest in.
- Verify round 2 (2026-09-23, on `d61e77a`) returned FAIL with three must-fixes, MF-1 through MF-3. All
  nine round-1 repairs had landed verbatim with no overshoot, and the gate dismissed eleven further lens
  findings, including one from each lens's own must-fix list.
- Verify closed under a Miguel-ruled escalation rather than a third round, which is the
  `docs/PROCESS.md` §5 route after two iterations. The three round-2 edits were applied by one
  `rust-engineer`, followed by a targeted re-check rather than a full adversarial pass.

### The gates wrote two of the defects they later found

Two of the three round-2 must-fixes sit in text a gate had already ratified, applied by an implementer
who did nothing wrong.

MF-3 is round-1's own MF-A replacement bytes, applied verbatim as instructed. They say network and TLS
already link into the default build and reach it by exactly two routes, then name route two as the
non-default, opt-in `remote` feature. A non-default feature is not in the default build, so the sentence
has to be read against itself. It sits on the exact line round 1 failed the commit over, in the
interface reference. The round-2 coordinator weighed dismissing it, because a careful reader sees the
incoherence rather than believing it, and carried it anyway for that reason.

MF-2 is one generation further back. The sentence came from the ratified specification's own replacement
block for the storage crate's module doc, and the design gate then scoped two sibling rustdoc sites to
v1.3 while leaving that one unscoped. Round 1 looked at the neighbouring manifest comment and dismissed
it because it self-corrects three lines later, without reaching the front page itself. Two round-2
lenses found it independently.

### The cascade broke its own pointers in three different shapes

A decision-row edit shifts line numbers, and this cascade proved that three separate ways.

1. Implement added six lines above the PRD decision table, which shifted every row down by six and left
   four frozen line-number self-pointers inside the D49 and D50 rows aimed at D42 and D43. The
   implementer caught this itself at verification and re-measured all four.
2. Verify round 1 found the class the implementer had stopped short of. The sweep had covered pointers
   internal to the PRD and not pointers from the PRD into the other files the same commit edited. Three
   went stale because those files gained lines above the cited ones —
   `docs/plans/ci-cd-and-distribution.md:65` cited four times, `Cargo.toml:130`, and a cited source
   range in `xtask/src/no_network.rs` that ended before one of the three symbols its own sentence names.
3. The round-1 repair repeated the shift. Miguel's Overview clause sits above the decision table, moved
   every row down by one and broke five row self-pointers, which the same implementer caught and
   repaired in the same commit. It also re-measured the cross-references for the open question it
   rewrote and found four citations by number rather than the three the instruction predicted.

Verify round 2 then found the same class surviving as text rather than as line numbers. Three v1.3
section headings in crate plans quoted a roadmap sentence the same commit had deleted, and one
attributed it to the roadmap section by name, so the attribution did not resolve. Each heading also made
a retired type name its section's trigger while the bullet directly beneath it, rewritten by the same
commit, said the name is retired. No lint class reaches a heading.

### A false, git-checkable fact nearly shipped inside the decision row

Four sites were about to state that commit `0e3568a` cleared the `rustls-webpki 0.102.x` family from
this repository, one of them the D51 row itself. The tracker record refutes it. That commit fixed a
rustls advisory and its own note records resolving to rustls 0.23.45 and rustls-webpki 0.103.15,
entirely inside the 0.103 line. A Review lens then walked every lockfile revision and found
`rustls-webpki 0.102.8` entering at `6e36cc7` and leaving at `1e73d28`, three months earlier in an
unrelated commit.

What makes it worth recording is the direction of travel. The specification was sharpening a hedged date
into a precise one, and its own provenance note quoted the refuting record as if it were support. The
gate replaced the attribution with the checkable half — the excluded route's advisories sit in the
0.102.x family and this tree's committed lockfile pins 0.103.15 — and ruled that no commit hash be
reinstated there.

### The check scripts were proven non-vacuous, twice, on two different scripts

A passing exit status does not prove a check script checks anything. Both Verify rounds measured that
directly, each on a different script, and this is the single most valuable measurement of the run.

In round 1 the measurement lens replaced `d46-schema-migration-claims.sh`'s escaped `RANGE_ALT_RE` knob
with a bare alternation. That script then exited 0 vacuously, because the bare parenthesis becomes a
capture group and the bare pipes become real alternation, which matches almost anything. Its sibling
`d50-pre-handshake-gate-claims.sh` caught it red on row Q29. Three further mutations reddened six, five
and four scripts.

Round 2 ran a fresh mutation on a different script. Baring `d47-envelope-id-claims.sh`'s knob made it
exit 0 while printing "parentheses not balanced" twice, and three siblings went red, with d50 naming the
exact anchored line on row Q31. That is the newest-pins-the-older design working as documented, and the
property survived two rounds of repairs. In both rounds the coordinator also read all thirteen knob
bytes directly rather than trusting an exit status.

### Miguel's rulings

| when | what he decided | why |
|---|---|---|
| Understand, 2026-09-22 | Keep every shipped name. The second `Storage` implementation goes in a new module inside `unblock-storage`, and the cargo feature keeps the name `remote`. | The decision makes "remote mode" the product's own word, so the name becomes correct rather than wrong. `ub-jv2` closes by stating the forwarding target positively, not by renaming. |
| Understand, 2026-09-22 | This spec commit mints no durable gate script. | The knobs ride the spec commit and the script rides the implementation commit; this decision has no implementation commit, and the two earlier documentation-only decisions have no script either. |
| Understand, 2026-09-22 | Rescope the always-on no-network hard rule in `CLAUDE.md` now rather than later. | Leaving it would put a knowing contradiction between a decision row and the always-on contract file, which is the ageing this cascade was landed now to avoid. |
| Spec, 2026-09-23 | Name the second backend after the protocol — `src/sql_over_http/`, `SqlOverHttpStorage`, `open_remote(url, token)`. | The vendor documents the wire protocol under that name; its lineage name, Hrana, was rejected as opaque at the call site. |
| Spec, 2026-09-23 | The two shipped `crates/unblock-config/src/schema.rs` error strings stay frozen. | They are user-visible text whose change needs a snapshot re-bless, so they ride the v1.3 implementation. The accepted consequence is one file shipping with a comment and a string that disagree until then. |
| design Review, 2026-09-23 | The product's headline sentence is ratified as the gate wrote it, byte-identical at the PRD and the README, each keeping its own wrapper (F16). | The two sites are byte-identical today and two drafts were about to split them, with nothing in the corpus to catch the divergence. |
| design Review, 2026-09-23 | The word "offline" in the v1.4 terminal-interface framing is rescoped, not dropped. All three drafts had deleted it. | The claim survives with the scope clause that makes it true, which is the same treatment D13 and the no-network hard rule get elsewhere in this cascade, so the corpus applies one rule to the word rather than two. |
| design Review, 2026-09-23 | The roadmap's server row keeps the full restatement of the three sqld facts rather than trimming to a pointer, and carries the per-fact sourcing. | A reader scanning that table is choosing a server and must meet the warning there. The sanctioned-copies list therefore reads four rather than three. |
| Verify round 1, 2026-09-23 | The PRD Overview gains the clause that remote mode lands at v1.3 (PROPOSED) and is in no shipped binary today. | The README already carried the scope and the decision row said so; the Overview was the one place the fact was missing, in the product's most-read section. The gate had left it out because no rule forces it. |
| Verify round 2, 2026-09-23 | Close by applying the three must-fixes plus a targeted re-check, then go to Track. No third adversarial round. | Each edit is one clause, fully specified, and none shifts a line number or touches a decision row or a range knob. A round-2 gate already held a high bar and would keep finding items of the same calibre until the stopping criterion became fatigue. |
| Verify round 2, 2026-09-23 | Restore "offline-capable" in the README's first line, scoped to local mode. | The commit had deleted the word rather than scoping it, which is the opposite of his own earlier ruling. That ruling had named three specific sites, so the gate raised the product sentence rather than extending it on its own authority. |

### Follow-ups filed

Six items were routed to their own tracker issues rather than made conditions on this change. They are
listed under Links.

## Gotchas

- A gate that dictates exact replacement bytes must respect the target file's wrap. MF-2's literal paste
  would have produced a 114-character rustdoc line in `crates/unblock-storage/src/lib.rs`, nine over the
  file's existing maximum, on the page `cargo doc` publishes. The implementer reflowed the paragraph and
  proved by assertion that the resulting word sequence equals the original with only the ratified swap.
- Adding any line above the PRD decision table shifts every row beneath it and breaks the frozen
  line-number self-pointers inside the older decision rows. It happened twice in this run, at six lines
  and then at one. No check script pins a line number in that file, so nothing catches it.
- A pointer sweep that covers only the file being edited is half a sweep. The same commit's edits to
  `ci-cd-and-distribution.md`, the root `Cargo.toml` and `xtask/src/no_network.rs` invalidated pointers
  aimed at them from the PRD, and the second Verify round found the same class again as quoted text in
  section headings rather than as line numbers.
- Verify round 1's stated premise for MF-I — that the roadmap was the only live site paraphrasing the
  five-advisory fact — was itself imprecise. There were two paraphrases, and round 2 recorded that for
  the record while grading the second one below its bar.
- A specification that sharpens a claim is not the same as one that checks it. The draft turned a hedged
  date into a precise commit attribution and cited, as support, the very tracker record that refutes it.
  Anything naming a commit hash as having cleared an advisory is cheap to check against the lockfile
  history and should be checked before it is sharpened.
- `git grep` on this machine (git 2.54.0, Apple Git-157) does not honour a word boundary in extended
  regular expressions. The probe that counts how many script lines name the id after the live range
  returned zero under two lenses and thirteen under a third; the basic and Perl forms both return
  thirteen and the extended form silently returns zero. A zero-hit sweep proves nothing until the regex
  flavour is pinned, and this repository's zero-live-hits standard runs on exactly such sweeps.
- The acceptance sweep for this cascade cannot reach zero and must never be wired as a bare zero-return
  gate, because the decision row itself has to name the retired mechanism to record why it is excluded.
  Only the forbidden-assertion half reaches zero; the surviving hits are negations, past tense and
  exclusion records, and every one has to be read.
- Intermediate commits were impossible here. Any commit carrying a file that cites the new decision id
  without the row itself is red on the required doc-lint, which resolves every id against the ids parsed
  from the PRD's §4 rows. The apply stages made four checkpoint commits so nothing could be lost
  mid-run, and the final stage collapsed them into one.
- Two writers each believed the other owned one specification entry, so `CLAUDE.md` still said libsql is
  the source of truth after both stages had run. The implement order had warned about exactly that
  ownerless-entry trap and it landed anyway; only the final verification pass caught it.
- The knowledge layer was left untouched by ruling, including the memories that still record the
  embedded-replica plan. It is descriptive and never normative, so it cannot make a document wrong, and
  the semantic consolidation sweep owns it.

## Glossary

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-----------------------------------------------|
| MF-A | Verify round 1's must-fix on the spine's false claim that the default build links no network symbol | the round-1 verdict, `temp/ub-b07-spec/VERIFY-ROUND-1.md` (gitignored scratch, not committed); the repaired site is `docs/plans/01-design-spine.md` §6.7 |
| MF-B | round 1's must-fix rewriting the roadmap open question that the same commit had already answered with a new risk-register row | same verdict file; the repaired site is `docs/plans/00-roadmap.md` §4, open question 5 |
| MF-C | round 1's must-fix narrowing the PRD §8.2 cross-machine `claim` guarantee from a measured result to a v1.3 obligation resting on a measured premise | same verdict file; `docs/PRD.md` §8.2 |
| MF-D | round 1's must-fix on four stale `ci-cd-and-distribution.md` line citations the commit itself created | same verdict file; `docs/PRD.md`, the D49, D50 and D51 rows |
| MF-E | round 1's must-fix on the stale root-manifest line citation inside the frozen D49 row | same verdict file; `docs/PRD.md`, the D49 row |
| MF-F | round 1's must-fix on a cited source range that ended before one of the three symbols its own sentence names | same verdict file; `docs/plans/ci-cd-and-distribution.md` §2 |
| MF-G | round 1's must-fix on the last unscoped source-of-truth claim, which escaped every sweep because it spells the phrase with a comma rather than a verb | same verdict file; `docs/PRD.md`, the domain-model table |
| MF-H | round 1's must-fix on a falsified rationale left standing in the audience row while the decision row claimed every falsified row was covered | same verdict file; `docs/PRD.md`, the D28 row |
| MF-I | round 1's must-fix canonicalising the spelling of the five-advisory fact at the one site that paraphrased it | same verdict file; `docs/plans/00-roadmap.md` §4 |
| MF-1 | Verify round 2's must-fix on three crate-plan v1.3 headings quoting a roadmap sentence the same commit deleted | the round-2 verdict, `temp/ub-b07-spec/VERIFY-ROUND-2.md` (gitignored scratch); the repaired sites are `docs/plans/crates/unblock-error.md:33`, `unblock-model.md:31` and `unblock-sync.md:76` |
| MF-2 | round 2's must-fix on the storage crate's published front page asserting a second implementation that does not exist | same verdict file; `crates/unblock-storage/src/lib.rs:10-14` |
| MF-3 | round 2's must-fix on round 1's own ratified MF-A replacement text, which reads against itself | same verdict file; `docs/plans/01-design-spine.md` §6.7 |
| F5 | the ratified specification's canonical-fact row fixing one spelling for the excluded route's five advisories, and dropping the false commit attribution | `temp/ub-b07-spec/SPEC.md` §1 and §3.6 (gitignored scratch); the landed form is in `docs/PRD.md` §4 row D51 clause (5) |
| F16 | the ratified specification's canonical-fact row holding the product's value-proposition sentence, byte-identical at two sites | `temp/ub-b07-spec/SPEC.md` §1 and §3.14 (gitignored scratch); the landed sites are `docs/PRD.md:14-17` and `README.md:24-27` |
| Q29 | the D50 gate-script row asserting that the D46 sibling's alternation knob still holds the escaped literal | `scripts/checks/d50-pre-handshake-gate-claims.sh:256` |
| Q31 | the D50 gate-script row asserting the same for the D47 sibling, which is the row that caught round 2's mutation | `scripts/checks/d50-pre-handshake-gate-claims.sh:258` |

## Links

- `ub-b07` — this task; the spec-first cascade that mints D51 and retires the embedded-replica framing.
  Its comment thread carries the Understand map, the Spec report, both gate verdicts and the Implement
  summary that this report condenses.
- `ub-w3a` — the shared-state spike that produced the decision. Its run-report is the direct predecessor
  of this one.
- `ub-jv2` — the storage crate's `remote` cargo feature was named after the wrong libsql constructor.
  CLOSED by D51, because remote mode uses none of those libsql shapes.
- `ub-n69` — nothing in CI pins the byte-identity of the product headline across the two files that must
  carry it, and they wrap at different columns.
- `ub-rjq` — `PRAGMA integrity_check` has no remote-mode reading anywhere in the corpus, which is the
  same shape as the schema-migration question already routed to the v1.3 lock.
- `ub-054` — three sites say a missing no-network whitelist entry makes a required job fail, when the
  measured behaviour is a silent pass.
- `ub-eo8` — three pre-existing stale line citations, all already stale on `main` at 085e58c.
- `ub-d2m` — the two frozen `unblock-config` error strings have no live record of why they stay frozen.
- `ub-4x2` — three phrasings left deliberately below the Verify round-2 bar.
- Pull request — none open at the time of writing.
- Key files touched —
  `/Users/ramosmig/Public/WS-Labs/unblock/docs/PRD.md` (the D51 row and its eight reciprocal notes),
  `/Users/ramosmig/Public/WS-Labs/unblock/docs/plans/00-roadmap.md` (the rewritten shared-state section),
  `/Users/ramosmig/Public/WS-Labs/unblock/docs/plans/01-design-spine.md`,
  `/Users/ramosmig/Public/WS-Labs/unblock/docs/plans/crates/unblock-storage.md`,
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-storage/Cargo.toml`,
  `/Users/ramosmig/Public/WS-Labs/unblock/xtask/src/doc_lint.rs`.
- Prior related run-reports — `runs/2026-09-22-shared-state-remote-mode-spike.md`, the spike this
  decision came out of, and `runs/2026-09-21-pre-handshake-frame-gate.md`, the closest structural
  analogue, a decision cascade whose design and Verify gates each failed twice and both closed on a
  Miguel-ruled escalation.
