---
name: 2026-07-29-duplicate-key-execution-flip
description: Closing the duplicate-JSON-key execution flip — a frame that read as one action executed another; three design-gate rounds, an owned scanning transport, and a fix proven against the original exploit.
type: run
date: 2026-07-29
branch: lp921-duplicate-key-reject
pr: -
issues: [ub-lp9.21]
---

# Run — the duplicate-key execution flip (decision D43)

## Context

Task ub-lp9.21 (an unblock task id; unblock is this repo's MCP-based issue tracker), taken as the
top-priority ready item after the knowledge-layer epic closed. It is a P0 data-integrity and security
defect, pre-existing since general availability: a DUPLICATE JSON key inside a tool call's parameters
was collapsed last-wins while the transport decoded the frame, so a frame whose text READ as one action
EXECUTED a different one. Branch `lp921-duplicate-key-reject` off main, worked in an isolated worktree.
Full lifecycle: understand, decide, spec, a design gate that took three rounds, implement, and a Verify
gate that failed once and looped back. The fix ships as decision D43 in the v1.0.1 patch slot.

## What & why

**The defect, reproduced twice independently before any work started.** A frame reading as a create,
carrying a second action key set to delete, returned a success result and tombstoned the target issue.
The blast radius turned out to be total rather than partial: every key inside the parameters object, at
any depth, was substitutable on all eight tools; 178 ordered shown-arm to executed-arm flips were
constructible, and 100 of those were "schema-clean", meaning the payload's other fields were also valid
for the arm a reader sees, so nothing in the frame looked out of place. Executed proofs included a
read-only frame performing a write, a dry-run delete flipped to permanent row removal, and a dry-run
import flipped to a real write of attacker-authored data.

**Why the existing defence was blind.** The repo already rejects unknown arguments strictly. That
defence operates on the parsed object, and the duplicate is collapsed several frames earlier, inside the
transport's byte decode. So the existing guard could not see it — and, tellingly, neither could any
existing test: the test harness serialises an already-deduplicated object, and the argument-boundary
guard deserialises from text, which DOES catch duplicates and therefore does not model production. The
input was not merely uncovered; it was inexpressible.

**The mechanism.** Detection had to move down to the raw bytes, and two whole families of proposals were
ruled out as structurally impossible: a fix at the typed-parse seam or inside tool bodies (the key is
already gone), and detection inside a transport decorator (that layer receives an already-parsed
message). Detection and rejection therefore live in different layers. The shipped design is an owned
transport that replaces the framework's, doing read-line, then duplicate-scan, then parse, then stamp a
three-state verdict — all in one place, which removes an entire class of correlation bugs by
construction. The scanner is a deserializer visitor rather than a hand-rolled tokenizer, so it reuses the
parser's own string decoding (escape-equivalent keys compare equal) and inherits its recursion bound. The
verdict rides a carrier that cannot be forged from the wire. A single gate rejects in-band at the
tool-call boundary, ahead of the existing quota; an absent or indeterminate verdict rejects too, so an
unscanned frame cannot slip through. The file-import path carries the same property.

**Decisions the maintainer took.** Rejection is IN-BAND rather than an out-of-band protocol error, which
preserves the normative promise even though the cheaper option had in-tree precedent. The scan root was
WIDENED from the arguments object to the whole parameters subtree, including the reserved metadata, so a
future consumer of that metadata cannot silently reopen the gap. The scope INCLUDES the second instance
of the same root cause in the file importer. It ships in the open patch slot, and rides a new decision id
rather than an amendment, because the product doc already recorded that this sits outside the earlier
decision's seam.

**A constraint that shaped the whole design:** minting a new error code would have moved the frozen
contract hash and become a breaking change, so the fix reuses the existing validation-failure code and
carries the duplicate-key kind, the offending key and a pointer to its location in the error's free-form
context. The contract, the quota options and the in-band-channel test are byte-unchanged on the branch.

## Outcome

The defect is closed, demonstrated against the original exploit rather than asserted. The same byte
sequence that tombstoned an issue now returns an in-band structured validation failure with the target
left open. A nested duplicate inside an array element, a duplicate inside the reserved metadata, and a
delete-mode flip from dry-run to permanent are all rejected; a clean frame still succeeds and the
pre-existing unknown-field rejection is unchanged. The adversarial security review could not break it
across escape and decode equivalence in both directions, depths up to the parser's own limit, all eight
tools, multi-megabyte and over-quota frames, byte-order marks, pipelining, envelope attacks, invalid and
overlong encodings, and two hundred thousand keys. The interface specification, which was asserting a
property the product did not have, now tells the truth, and an executable check keeps it that way.

## Gotchas

- **A green suite can prove nothing when the test harness cannot express the input.** That was the
  recurring theme of every gate here. It is worth checking, for any security property, whether the
  existing harness is even capable of constructing the hostile input — and if it is not, that is the
  first thing to build.
- **A counter placed one line too early makes a complexity guard vacuous.** The first cut incremented per
  key DECODED, before the membership probe, so the count equalled the key count under any algorithm and a
  quadratic regression stayed green while running fifty times slower. The repaired guard counts inside the
  comparison itself, and now has three orders of magnitude of margin.
- **Re-implementing a dependency's framing means re-implementing its quirks.** The forked compatibility
  filter is only reached AFTER the typed parse fails; because every corpus frame parsed, the filter had
  zero effective coverage and three mutants of it survived a green suite. Coverage of a fallback path
  needs an input that actually fails the primary path.
- **The fuzz target disproved one of the implementer's own invariants inside sixty seconds** — a claim
  that a duplicate inside the scanned subtree implies a duplicate of the whole document. False, because
  the two roots do not read the same bytes and one skips string validation. Harmless in production, but
  wrong in a direction nobody had checked; the failing input is now a committed corpus seed.
- **Parallel review agents mutation-testing in one shared worktree contaminate each other's builds.** A
  reviewer here found the prebuilt binary was a mutant, discarded its own first round of results, and
  rebuilt from a pristine archive. Check a build's timestamp before trusting a behavioural verdict from it.
- **Design review is not free, and it earned its cost here.** Round one found the maintainer's late scope
  decision had not propagated to twelve sites, including an executable test asserting the OPPOSITE of the
  approved design. Round two found three of round one's own remedies homed where they could not compile —
  two of them on fail-closed security arms that would otherwise have shipped with zero executable coverage.

## Glossary

No session-local id codes (short mutation-testing or must-fix labels — an uppercase letter immediately
followed by a number) appear in this report or in this run's issue comments; the comment thread was
written code-free on purpose. The table records the durable references the report leans on.

| id | what it is (in words) | where it lives |
|----|-----------------------|----------------|
| ub-lp9.21 | this task — the duplicate-key execution flip | the unblock tracker; git record `.unblock/issues.jsonl` |
| ub-cnv | the sibling defect this run split out: a duplicated envelope id makes a request decode as a notification, so no response is sent | the unblock tracker |
| D43 | the decision this fix rides — reject a duplicate key in a request's parameters, in-band | `docs/PRD.md` §4 |
| D42 | the earlier argument-boundary decision this does NOT fall under (strict rejection of unknown arguments) | `docs/PRD.md` §4 |
| D35 | the general-availability stability decision that makes a contract-hash move a breaking change | `docs/PRD.md` §4 |
| the argument-boundary contract | the normative interface section that was asserting the false property | `docs/plans/01-design-spine.md` §5.6 |

## Links

- ub-lp9.21 — the task; its comment thread carries the per-phase narrative, including the three design
  rounds and the Verify gate's findings.
- ub-cnv — the duplicated-envelope-id defect, deliberately scoped out of this fix because it has no
  in-band channel to answer on.
- `docs/PRD.md` §4 — the D43 decision row, with reciprocal cross-references to the decision it does not
  fall under.
- `docs/plans/01-design-spine.md` §5.6 — the argument-boundary contract, now reconciled.
- `scripts/checks/d43-argument-boundary-claims.sh` — the executable check that keeps the corrected claims
  from rotting back; mutation-tested in both directions.
- Prior run-reports: `.knowledge/wiki/runs/2026-07-24-knowledge-gardener-sweep.md`.
