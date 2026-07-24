---
name: project-d38b-eof-prehandshake-exit-code
description: D38(b)/proposed-D40 — EOF-before-MCP-handshake exit code ratified to 0 (was 1); pre-GA, implementing now
type: reference
---

**RATIFIED 2026-07-20 (Miguel).** The D38 follow-up (b): when `unblock mcp` runs and the client
closes stdin **before** completing the MCP `initialize` handshake, with **NO signal recorded**, the
process must exit **0** (was exit 1/`InternalError`). Also ratified: **implement now, pre-tag**.

**Why exit 0 (Decide team, 3 lenses unanimous + coordinator re-verified at source):**
- rmcp surfaces this as `ServerInitializeError::ConnectionClosed` (distinct from `Cancelled` = signal,
  from `TransportError` = bind/IO fault, from `ExpectedInitializeRequest` = protocol violation). Today it
  falls through `McpServerError::Transport` → NOT `is_cancellation()` → `InternalError` → exit 1.
- Nothing internal failed — unblock bound stdio, opened+migrated the DB, waited; the peer just never
  spoke. exit 1/`InternalError` is the exact mislabel D38 already rejected ("blaming InternalError on a
  process for obeying"). A **post**-handshake EOF already returns exit 0 (`mcp_lifecycle.rs:~339`); the
  code itself flags this 1-vs-0 as an open question — exit 0 unifies the two.
- Precedent: beads_rust `run_stdio` exits clean on EOF (only non-zero serve exit = `128+signo`). MCP
  spec 2025-06-18: closing the input stream IS the sanctioned clean shutdown; a pre-init disconnect is a
  shutdown, not an enumerated error.
- **Semver (D35): additive/non-breaking.** exit 0 = the `Ok(None)` success path — no `StructuredError`
  built, §2.3 0..=8 table never engaged, no `CONTRACT_HASH`/`CONTRACT_VERSION` bump. FREE pre-tag; after
  the `v1.0.0` tag the 1→0 flip would be semver-relevant — hence decide+land NOW.

**Honest residual risk (does NOT flip verdict):** rmcp collapses receive-side read errors into
`receive()==None`, so `ConnectionClosed` also fires if the inbound pipe breaks mid-handshake or on
`echo garbage | unblock mcp`. exit 0 greenwashes those. Mitigation: still a peer-gone event (unblock's
OWN faults keep exit 1 — `TransportError`/`RunLoop`/`Session::open`); the D39 workspace stderr line + a
`-vv` `tracing::debug!` remain so nothing is swallowed.

**Change sites (from Decide):** `unblock-mcp/src/error.rs` (add additive
`is_pre_handshake_disconnect()` matching only `Transport{ConnectionClosed}` + `__connection_closed_error()`
test-util mirroring `is_cancellation`/`__cancelled_error`, + a narrowness test that a genuine
`__transport_error` is NOT the disconnect class); `unblock-cli/src/commands/mcp.rs` (`resolve_mcp_exit`
unsignalled arm → `Ok(None)`/exit 0 reported as a Debug diagnostic, genuine teardown Err still decides;
`diagnostic_route`→Debug; module docs); `unblock-cli/tests/mcp_lifecycle.rs` (@~328-339 comment + e2e
bare pre-init stdin close → exit 0 + a `resolve_mcp_exit` unit case); spine §5b (~:2189) + §2.3 note
(~:599); PRD §4. **D-id = mint D40** (recommended — D38 is merged/frozen; a new row is more traceable +
doc-lint-clean; D-range `D1..D39`→`D1..D40` at the 3 sites: CLAUDE.md, ci-cd §2.1(a), xtask/src/doc_lint.rs).

**Lifecycle: ✅ MERGED & CLOSED (2026-07-20).** [PR #425](https://github.com/websublime/unblock/pull/425)
rebase-merged into main (`5829044` docs + `178a3d8` code); STATUS T3.2.1 follow-up (b) flipped landing→merged
(`bbad707`, pushed direct to main). Local main synced to origin, worktree + both branches pruned, tree clean.
Understand+Decide → design Review
(PASS_WITH_MUSTFIX, all folded — the top must-fix: the disconnect arm must DELEGATE to teardown, never
blanket `Ok(None)`, so a failing `session.shutdown()` still decides via its 0–8 code) → Implement (single
implementer, worktree, 2 commits `f7bae16` docs + `7cf0aac` code on branch `d40-eof-exit-0` off `e4491c4`)
→ Verify (PASS, 0 must-fix, coordinator re-verified on-disk "No material surviving mutation found"). The
authoritative probe was green (fmt/clippy/test(cli+mcp)/doc-lint 19·6/check-layering); one teardown-absorption
mutant (blanket-Ok(None)) + one predicate-narrowness mutant (`!matches!`) both confirmed genuine test-RED then
reverted. **[PR #425](https://github.com/websublime/unblock/pull/425) OPEN** (Claude opened; human merges). On
merge: flip STATUS T3.2.1 follow-up (b) → resolved. Goldens + `CONTRACT_HASH ddd02b7b`/`unblock.mcp.v1.5`
UNMOVED. 3 non-blocking residual nits deferred (teardown_error() docstring Engine-vs-Io; D38-row inline
annotation; signalled-disconnect combo unit). Relates to [[project-t3-6-release-pipeline-scope]] (GA tag).
