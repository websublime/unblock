---
name: 2026-09-22-shared-state-remote-mode-spike
description: Resolving where a shared primary lives and which mechanism carries it before the v1.3 lock (tracker ub-w3a) — Miguel replaced the spike's two-axis framing with two product modes, self-hosted sqld fell to dated vendor evidence, a desk conclusion that every route to remote mode was blocked by a frozen crate's five advisories was falsified by a fourth crate Miguel found in the vendor's own quickstart, the hands-on phase then measured the same three probes against a Docker libsql-server, a tursodb sync server and a real Turso Cloud database, and Miguel closed the run by deciding two modes with no offline and no cache.
type: run
date: 2026-09-22
branch: ub-w3a-shared-state-research
pr: -
issues: [ub-w3a]
---

# Run — shared-state spike, from two axes to two product modes

## Context

Issue ub-w3a is the research spike that had to answer where a shared primary database lives and which
sync mechanism carries it, before the v1.3 shared-state release could be locked. It also doubled as the
fresh vendor-maturity check that the v1.3 lock requires. The run went from 2026-09-21 to 2026-09-22 off
main at 1ff74f2, on branch `ub-w3a-shared-state-research`. Its pull request carries this report and the
tracker re-export, and nothing else.

The work ran in three stages. A desk-research workflow of five lenses plus a coordinator went first, a
second workflow of two lenses plus a coordinator followed it, and then the orchestrator ran a hands-on
Docker and Cloud phase alone. Miguel steered at every fork and took the product decision at the end.

## What & why

The repository already recorded two open questions about shared state and had answered neither. The
roadmap's shared-state section, `docs/plans/00-roadmap.md` §4, described a design in which writes
serialize at the primary, reads stay local through embedded replicas, and offline means read-only. That
description was written in 2026-07 against vendor facts that nobody had rechecked since. The spike
existed to recheck them with evidence and to say whether the recorded direction still held.

The spike read the roadmap's shared-state section as the clause under test, the storage crate plan
(`docs/plans/crates/unblock-storage.md`) for the backend seam and its non-default feature, and the
process guide's decision rules for how any resulting change would have to land. It restates none of
them as rules. The write path was read directly, at `crates/unblock-engine/src/session/write.rs` and
`crates/unblock-storage/src/libsql/mod.rs`, to check a claimed data-loss exposure against our own
locking.

Two constraints shaped the plan up front. No normative document would change during the spike, so the
output is a recommendation plus this report. Miguel started the Docker daemon himself rather than having
the session do it.

## Outcome

No normative document changed in this run. The product decision is Miguel's and it is recorded in the
issue thread; the documents change in a separate cascade, tracked as ub-b07.

### Miguel replaced the framing, and the new one is better

The research opened on two axes, where the primary lives and which mechanism syncs it, and treated the
Docker container as a way to de-risk self-hosting as a deployment. Miguel rejected that framing. It
mixed product modes with implementation mechanisms. He replaced it with two modes. In local mode the
workspace is a SQLite file on one machine, which is what unblock does today. In remote mode the
workspace is a database on a server, and every developer connected to it sees the same data.

The reframing paid immediately. The Docker container became a test fixture for exercising remote mode
without an account, so the self-hosted server's missing authentication and missing transport encryption
stopped being a blocker for the spike and became a deployment concern. The hosting axis dissolved into a
configuration choice.

### Self-hosted sqld was ruled out on dated evidence

Turso's founder and chief technology officer wrote on 2026-07-12 that the libSQL server is in deep
maintenance and that Turso no longer runs it in production. The last tagged server release is from
2025-02-14, seven commits touched the server directory in twelve months, and a corruption-after-power-loss
report from 2026-07 was independently reproduced in 2026-08 and remains open. The client-facing port
carries no transport encryption, and authentication fails open when it is not configured. For a
data-integrity tool holding company data, that combination disqualified it as the production primary.

### A first conclusion was wrong, and the error shape is worth keeping

The desk research concluded that every route to remote mode ran through the frozen libsql crate and
therefore inherited five unfixable advisories. The orchestrator had verified the advisory counts
directly, twice, so the measurements were sound. The conclusion still did not hold.

Miguel pointed at the vendor's own Rust quickstart, which documents a remote-only mode built on a fourth
crate that nobody in the spike had examined. The research had looked at two crates and generalised from
those two to every route. Generalising from two libraries to all libraries is the error, and the
measurements being correct is exactly what made it convincing.

That fourth crate, `turso_serverless` 0.1.3, audits clean. Zero vulnerabilities and zero warnings across
183 transitive crates, measured on 2026-09-22 against the current advisory database. It speaks SQL over
HTTP, carries no SQL engine at all, and its only constructor opens a remote connection, so it cannot
open a local file even in principle. Its maturity is the standing risk. It was first published in
2026-07, has four releases, and has very few downloads.

### The hands-on phase measured instead of arguing

Three probe shapes ran against three targets. Each probe used the real unblock write shape rather than a
toy query.

| probe | what it asserts |
|---|---|
| remote write, independent read | client A writes, a separately built client B reads the row back with no sync call, no local database file appears |
| identifier allocation under concurrency | eight concurrent writers, twelve allocations each, reading the high-water mark then inserting high-water plus one |
| row and audit event together | one issue row and its audit event commit together, or a constraint violation rolls both back |

The targets were a libsql-server container on the loopback interface, a `tursodb --sync-server` process,
and a real Turso Cloud database that Miguel created and supplied a token for. The allocation probe
returned 96 committed, 96 distinct, exactly 1 through 96, with no gap and no duplicate, on every target
that the client could address. The atomic-batch probe committed both rows on the happy path and rolled
the issue row back when the event step violated a constraint, on every target.

One real difference showed up between the two server generations. The older libsql-server serves both
the version 2 and version 3 pipeline endpoints and issues batons, so interactive transactions survive
across HTTP requests. The newer `tursodb --sync-server` serves only the version 2 endpoint, returns a
null baton on every response, and knows two request types. Its documentation shows it is generated from
a prompt and tagged 0.0.1, so it is a reference implementation and not a server product. The clean client
hardcodes the version 3 path with no configuration knob, so the two cannot be paired at all.

That difference turned out not to bite. Identifier allocation rewritten as one statement, and the
mutation rewritten as one batch request with explicit begin, commit and rollback steps, both work on
either server generation without a session. Turso Cloud serves both endpoints and honours interactive
transactions, and a database created there now runs the newer engine.

### Two objections dissolved rather than being solved

The research had raised last-push-wins conflict resolution as a contradiction of the roadmap clause that
forbids multi-master semantics, and had raised multi-process access to a synced file as a possible
outright killer. Both were properties of keeping a local copy. Remote mode has no local copy, so neither
question arises. The roadmap's serialize-at-the-primary clause is satisfied by construction.

### Miguel's decision

unblock has two modes and the product must say so. Local mode is one SQLite file with no network, no
server and no account. Remote mode is one shared database with two supported server options that the
application code does not distinguish between, since only the address and the token differ. A private
sqld and Turso Cloud are presented as peers in usage, with a dated status note on the sqld option
naming the deep-maintenance status, the missing transport encryption and the fail-open authentication.
Miguel chose that presentation over presenting them unqualified and over making Cloud the default with
sqld as an advanced section.

He then decided offline, superseding his own earlier position in the same thread. He had asked whether a
local cache could serve offline reads, and three ways to hold one were put to him. He closed all three.
unblock supports local mode or online mode and nothing in between. In remote mode a lost network stops
reads too, which for an agent-first tool means an agent loop stops rather than degrades. He weighed that
against one source of truth with no reconciliation, no staleness semantics and no second engine in the
binary, and chose the simplicity.

That decision changed an instruction already written into the follow-up cascade. ub-b07 had been written
to mark the offline clause pending; it now lands the clause as decided, and a comment on ub-b07 says so.

### Follow-ups filed

Three findings were split out of the spike rather than carried in it, and one cascade task was created
to land the decision. They are listed under Links.

## Gotchas

- The code graph returned a misaligned `name` column during the Understand phase, so a symbol-name search
  answered with a confident single hit in an unrelated crate; queries by qualified-name prefix stayed
  correct, and that workaround carried the whole spike (filed as ub-7m4).
- One lens in the first research workflow died on a structured-output size cap, and its entire report was
  discarded rather than truncated, so a whole perspective silently vanished from the round.
- A macOS Python interpreter failed certificate verification against the public endpoint during the Cloud
  probe, which is a local trust-store gap and not a server fault.
- A concurrency assertion that passes on the first run proves nothing until a control run fails; swapping
  the deferred transaction behaviour in for the immediate one produced 12 commits and 84 conflicts, which
  is what made the clean run believable.
- Auditing a scratch crate is not the same as running the repository's required jobs, because the licence
  allowlist is derived from today's lockfile, so a large dependency addition can turn the dependency-ban
  job red while the advisory job stays green.
- Measurements can be correct and the conclusion drawn from them still wrong; the spike audited two
  crates accurately and then generalised to every route to remote mode, which a fourth crate falsified.
- A credential supplied for a probe passes through the session transcript, so it needs rotating once the
  probe is done.
- The probe containers and processes were left running on two loopback ports and need explicit teardown,
  since nothing in the run removes them.
- The knowledge-layer bash guard fails closed when an interpreter command merely mentions a
  `.knowledge` path, so a close-reason string naming this report's own location blocked the call; the
  fix is to put the text in a file and pass the path.
- The unblock MCP server timed out at session start because the workspace launches it through `cargo run`
  and the debug binary had to compile first; pre-building it fixed the startup, and the issue comments
  were written meanwhile through a line-delimited JSON-RPC stdio harness.

## Glossary

No session-local ids were used in this run.

## Links

- ub-w3a — the spike itself, resolving where a shared primary lives and which mechanism carries it before
  the v1.3 shared-state lock. Its comment thread is the full narrative this report summarises.
- ub-b07 — the follow-up spec-first cascade that lands Miguel's two-mode decision in the product
  requirements document, the roadmap and the spine, including the decision-id range bump.
- ub-jv2 — the storage crate's cargo feature is named after the wrong libsql constructor, so the obvious
  forwarding would resolve to a pathless network-per-query client.
- ub-9m0 — a write-ahead-log reset race in the bundled SQLite core, which our own locking cannot trigger
  but a foreign process checkpointing the workspace file can.
- ub-7m4 — the code graph index returns a `name` field that does not belong to the node the rest of the
  row describes, so every symbol-name lookup fails misleadingly rather than emptily.

Repository files read during the run.

- `/Users/ramosmig/Public/WS-Labs/unblock/docs/plans/00-roadmap.md` — the shared-state section under test.
- `/Users/ramosmig/Public/WS-Labs/unblock/docs/plans/crates/unblock-storage.md` — the backend seam and its
  non-default feature.
- `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-engine/src/session/write.rs` — the write permit
  and the cross-process advisory lock.
- `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-storage/src/libsql/mod.rs` — the immediate
  transaction wrapper and the passive checkpoint.
- `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-storage/src/libsql/migrations.rs` — the migrate
  path's truncating checkpoint.

The only repository files this run changed are this report and the tracker's git record,
`.unblock/issues.jsonl`.
