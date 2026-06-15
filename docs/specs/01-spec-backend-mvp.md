# SPEC: P01 — Backend MVP Implementation Contract

**Status:** APPROVED (org-provisioning write-surface tenant gates applied 2026-06-15 — §10.1.1 gate-model + §4.2 extended to the two `org` RPCs that write tenant-scoped rows from the wire (`org.AddMember` caller-admin `org.members` predicate + role cap — closing a CRITICAL cross-tenant privilege escalation; `org.CreateProject` caller-membership predicate replacing the FK→`NotFound` that only caught a non-existent org — closing a WARNING write IDOR), the sibling org-provisioning RPCs the `.85` admin/BFF sweep set aside — DORMANT (empty-caller no-op) until a future key-management BFF pins the caller identity (bead `unblock-tv8.86`); auth/BFF admin write-surface tenant gates applied 2026-06-15 — §10.1.1 gate-model extended to the two `auth` RPCs that write `mcp.api_keys` (`IssueAPIKey` `org.Authorize`-on-`CallerUserID` ownership + `issued_to_user ∈ org.members`; `RevokeAPIKey` `CallerOrgID` row predicate on the UPDATE), closing TWO LATENT cross-tenant write IDORs on the admin/BFF surface the MCP-wire sweep set aside — DORMANT (empty-caller no-op) until a future key-management BFF pins the caller identity (bead `unblock-tv8.85`); `Update.milestone_id` write-scope tenant gate applied 2026-06-15 — §10.1.1 gate-model extended to `workitems.Update`'s wire-supplied `milestone_id` selector (org-XOR-project milestone predicate, `AssignItem`/`Create` precedent), closing the CRITICAL cross-tenant write IDOR that `.83`'s AC4 wrongly assumed gated (bead `unblock-tv8.84`); `create_milestone.project_id` INSERT-scope tenant gate applied 2026-06-15 — §10.1.1 gate-model extended to `workitems.CreateMilestone`'s project-scoped `project_id` selector (guarded INSERT…SELECT, `CreateLabel` precedent), closing the last ungated cross-reference write-IDOR in the family (bead `unblock-tv8.83`); create-path cross-reference tenant validation applied 2026-06-12 — §10.1.1 gate-model extended to `workitems.Create`'s wire references, keyed on the existing `req.OrgID` per Miguel's 2026-06-12 DECISION, + §4.4 `CreateRequest`/`DependencyEdge` drift reconciliation; round-16 write-surface tenant-hardening lockstep applied 2026-06-11 — §10.1.1 row-level write gate + per-RPC `CallerOrgID` channel + deps fold-in + `CreateLabel` empty-`CallerOrgID` guard; round-16 label-tools drift-closure applied 2026-06-11 — labels `updated_at` migration `0130` + Tools 20–23 auth wording + Tools 16/19 wire-sample/prose alignment; round-16 P01 tool-surface scope amendment applied 2026-06-04; §3.5 fifth-secret addition applied 2026-06-02; round-15 NFR-1 harness test-isolation + mcpaudittest hardening applied 2026-05-29; round-14 NFR-1 latency-harness scope applied 2026-05-29; round-6 cascade-symmetry applied 2026-05-12; round-5 tracing applied 2026-05-12; round-4 auth applied 2026-05-11; DRIFT-1/-2 applied 2026-05-08; round-2 applied 2026-05-08; round-3 research applied 2026-05-08; original APPROVED 2026-05-07)
**Changelog:**
- 2026-06-15 — org-provisioning write-surface tenant gates (bead `unblock-tv8.86`, discovered-from the 2026-06-15 admin/BFF surface IDOR sweep — the SIBLING org-provisioning RPCs the `.85` round, on the same admin/BFF surface, deliberately set aside; status remains APPROVED — this is an additive §10.1.1 gate-model + §4.2 extension closing a CRITICAL cross-tenant privilege escalation + a write IDOR, NOT a re-architecting). Two `private` Encore RPCs in `apps/api/org/org.go` write tenant-scoped rows from the wire with NO caller-ownership check: **(1) `org.AddMember` (org.go:429-475) — CRITICAL cross-tenant privilege escalation** — takes `OrgID`/`UserID`/`Role` straight from the wire and INSERTs an `org.members` row with ZERO caller-ownership check (`callerIdentity` feeds only the `invited_by` audit column, NEVER authorization), and `Role` has no cap → a caller can mint itself (or anyone) as `owner` of ANY existing org; **(2) `org.CreateProject` (org.go:280-311) — WARNING** — takes `OrgID` from the wire and INSERTs an `org.projects` row under it; the only guard is the FK→`NotFound`, which catches a NON-EXISTENT org but NOT a FOREIGN existing one. NEITHER is reachable from the MCP agent wire today (no MCP tool maps to them; no non-test in-repo caller; `apps/web` is a README placeholder) — both are LATENTLY exploitable cross-tenant once a future key-management / web-admin BFF is wired. **Locked contract (decided by Miguel — fix now via spec-first, mirroring `.85`):** these RPCs carry NO caller identity today; the fix ADDS an off-wire `CallerUserID` channel pinned from the resolved session identity (the future BFF's session→user→org resolution, §4.3.2), NEVER from the wire — exactly the §10.1.1 / `.85` internal-channel convention, NOT a wire argument. **AddMember:** new `CallerUserID` field; when non-empty, require the caller to hold an **admin/owner** `org.members` row in `OrgID` (`SELECT role FROM org.members WHERE org_id=$1 AND user_id=$2`, §4.2 / `org.go:520`) BEFORE the INSERT, AND **cap the grantable `Role` at the caller's effective role** (a caller cannot grant above their own). A foreign / non-member `OrgID`, an unauthorised caller, or an over-grant → `NOT_FOUND` / appropriate error, nothing inserted, existence not leaked. **CreateProject:** new `CallerUserID` field; when non-empty, require the caller to be a **write-capable member** of `OrgID` before the INSERT; a foreign / non-member `OrgID` → `NOT_FOUND`, **replacing the FK→`NotFound`** that only caught a non-existent org. **Empty-caller NO-OP (dormant gate):** when `CallerUserID` is empty (the trusted §11.1.1 seed + `org` / `rbactest` / `exitcriteriontest` / `perftest` callers, which pass no caller identity) the gate is skipped — DORMANT until the future BFF pins the caller; that future bead MUST pin it (else the no-op leaves the priv-esc / IDOR open). Same empty-caller no-op precedent as `.85` / the §10.1.1 item/milestone write-RPC pattern. Bootstrapping is correctly OUT of scope: `org.CreateOrganization` (caller becomes owner), `auth.ExchangeOAuthCode` / `auth.Validate` (identity establishment). Infra CONFIRMED present: `org.members` (migration `0030`), `org.Authorize` (`org.go:520`), the org service owns the `org` schema (direct read OK). Proactively noted: `org.project_members` has no write RPC yet (seed-only) — a future `AddProjectMember`-style RPC will need the IDENTICAL gate. Patched in lockstep: §4.2 (new `CreateProject` / `AddMember` tenant-gate doc-comments + `CreateProjectRequest` / `AddMemberRequest` struct stubs carrying the new `CallerUserID` channel; the `Authorize` doc-comment reconciled — see below; the future-`AddProjectMember` note); §10.1.1 "Auth / BFF admin write surface" subsection (two new rows — `org.AddMember`, `org.CreateProject` — + the org-provisioning intro paragraph + widened closing no-op prose). **Spec self-overclaim reconciled:** the §4.2 `Authorize` doc-comment said `Authorize` is "called by every other service before reading or writing a resource", which could be read to imply `org`'s OWN writes route through it — they do NOT; reworded to "the canonical CROSS-SERVICE RBAC predicate … called by every OTHER service", with an explicit note that `CreateProject` / `AddMember` self-gate via the new `CallerUserID` `org.members` predicate (dormant) while `Authorize` remains the cross-service primitive OTHER services call. **NOT touched (separate ownership): `apps/api/org/org.go` (the gates + the new request fields) + `apps/api/workitems/workitems.go:109-117` (the FALSE "org writes are gated" auth-model doc-comment, which is the CODE's to correct) + tests — the implementation bead `unblock-tv8.86` (Greta) owns those on its branch.** **No DDL / migration / public-API change** — the gates are membership predicates + an `org.Authorize` call + (for `AddMember`) a role cap; the `org` schema (incl. `org.members`, `org.projects`) is UNCHANGED. **Root `docs/SPEC.md` is untouched** — its §5.6 RBAC prose (row-level filtering "applied uniformly to every read and write path") is STRENGTHENED, not contradicted; it carries NO claim that `org`'s own provisioning self-writes are already gated, so no contradiction exists there (verified). Spec status remains APPROVED.
- 2026-06-15 — auth/BFF admin write-surface tenant gates (bead `unblock-tv8.85`, discovered-from the 2026-06-15 cross-tenant write-IDOR audit extended to the ADMIN/BFF surface the MCP-wire sweep — `.75`/`.77`/`.78`/`.80`/`.83`/`.84` — deliberately set aside; status remains APPROVED — this is an additive §10.1.1 gate-model extension closing TWO LATENT auth-surface IDORs, NOT a re-architecting). Two `private` Encore RPCs in `apps/api/auth/auth.go` write/modify `mcp.api_keys` rows scoped by `org_id` with NO caller-org ownership check: **(1) `auth.RevokeAPIKey`** — `UPDATE mcp.api_keys SET revoked_at=COALESCE(revoked_at,now()) WHERE id=$1` with no caller predicate (any tenant's key revocable by id); **(2) `auth.IssueAPIKey`** — the INSERT stamps `org_id` + `issued_to_user` straight from the wire with no check the caller owns `org_id` nor that `issued_to_user` is a member of `org_id`. NEITHER is reachable from the MCP agent wire today (no MCP tool maps to them; only test/seed callers, and the §11.1.1 E2E seed writes `mcp.api_keys` via direct INSERT) — they are LATENTLY exploitable cross-tenant once a future key-management BFF / web-admin surface is wired. **Locked contract (decided by Miguel — fix now via spec-first):** these RPCs carry NO caller identity today; the fix ADDS a caller-identity channel pinned from the resolved caller identity (the future BFF's session→user→org resolution, §4.3.2 / `auth.Validate` `TokenKind="session"`), NEVER from the wire — exactly the §10.1.1 internal-channel convention, NOT a wire argument. **RevokeAPIKey:** new `CallerOrgID` field; the UPDATE gains `AND ($caller='' OR org_id=$caller)` so a cross-tenant `key_id` affects zero rows → `NOT_FOUND` (existence not leaked); the `COALESCE` idempotency is PRESERVED. **IssueAPIKey:** new `CallerUserID` field (needed because the ownership check uses `org.Authorize`, which keys on the caller's user id); gate on BOTH (a) caller owns `org_id` via `org.Authorize(CallerUserID, <write action>, OrgID)` — `SELECT role FROM org.members WHERE org_id=$1 AND user_id=$2` (`apps/api/org/org.go:520`) — and (b) `issued_to_user ∈ org.members(OrgID)`; a foreign `org_id` or a non-member `issued_to_user` → rejected (`NOT_FOUND` / appropriate error), nothing inserted. **Empty-caller NO-OP (dormant gate):** when the caller-identity field is empty (the trusted §11.1.1 E2E seed + integration / mcpaudit / perf tests, which pass no caller identity or seed `mcp.api_keys` via direct INSERT) the gate is skipped — DORMANT until the future key-management BFF / admin surface pins the caller identity; that future bead MUST pin it (else the no-op leaves it open). This mirrors the §10.1.1 empty-`CallerOrgID` no-op precedent (the MilestoneTree / item-milestone write-RPC pattern), adapted to the auth/BFF surface. Infra CONFIRMED present: `org.members` (migration `0030_org.up.sql:19`, cols org_id/user_id/role), `org.Authorize` (`org.go:520`, membership+role check), `org.AddMember` (`org.go:428`). Patched in lockstep: §4.1 `IssueAPIKey` doc-comment + `IssueAPIKeyRequest` (new `CallerUserID` field) and `RevokeAPIKey` doc-comment + `RevokeAPIKeyRequest` (new `CallerOrgID` field) — each annotated "pinned from the resolved caller identity; NEVER from the wire"; §4.3.2 (new note distinguishing the BFF session-resolution caller identity from the Bearer API-key Validate hot path); §10.1.1 (new "auth/BFF admin write surface" subsection + predicate table — `RevokeAPIKey` caller-org UPDATE predicate, `IssueAPIKey` `org.Authorize` ownership + `issued_to_user` membership — noting (a) NOT MCP-wire-reachable, caller identity pinned by the FUTURE BFF not the Bearer handler; (b) DORMANT empty-caller no-op until that surface is wired, the future bead MUST pin it; (c) cross-tenant probes → `NOT_FOUND`). **NOT touched (separate ownership): `apps/api/auth/auth.go` (the gates + the new request fields) + tests — the implementation bead `unblock-tv8.85` (Greta) owns those on its branch.** **No DDL / migration / public-API change** — the gates are query predicates + an `org.Authorize` call + a membership predicate; the `mcp.api_keys` schema is UNCHANGED. **Root `docs/SPEC.md` is untouched** — its §5.6 RBAC prose (row-level filtering "applied uniformly to every read and write path", `docs/SPEC.md:732`) is STRENGTHENED not contradicted, and its `mcp.api_keys` note (`docs/SPEC.md:1855`) already records `auth.IssueAPIKey` is exercised via direct-INSERT test seeds in P01 — consistent with the dormant-gate framing; no contradicting claim about these RPCs exists there (verified). Spec status remains APPROVED.
- 2026-06-15 — `Update.milestone_id` write-scope tenant gate (bead `unblock-tv8.84`, discovered-from an adversarial completeness sweep during review of `.83` — which PROVED `.83`'s AC4 ("`create_milestone.project_id` is the LAST ungated selector") INCOMPLETE: `workitems.Update`'s wire-supplied `milestone_id` write was the residual ungated selector — proven by code read 2026-06-15; status remains APPROVED — this is an additive §10.1.1 gate-model extension closing the CRITICAL `Update.milestone_id` cross-tenant write IDOR, NOT a re-architecting). `workitems.Update` wrote the wire-supplied `milestone_id` into `workitems.items` with NO check that the milestone belongs to the caller's org — the `UPDATE`'s only tenant predicate gates the TARGET ITEM's org (`org_id = $caller`, bead `.77`), NOT the new `milestone_id`. Same IDOR class already closed for `.75` (`CreateLabel.project_id`), `.77` (`CreateMilestone.parent` + Update target-item seam), `.78` (`workitems.Create.*`), `.80` (`AppendComment.parent_id`), and `.83` (`create_milestone.project_id`). **Locked contract:** when the request sets a **non-empty** `milestone_id`, the `UPDATE` additionally requires `milestone_id IN (SELECT id FROM workitems.milestones WHERE org_id = $caller OR project_id IN (SELECT id FROM org.projects WHERE org_id = $caller))` — the org-XOR-project milestone predicate, mirroring the EXISTING sibling gates on the two other paths that write the same `items.milestone_id` column, `workitems.Create` and `AssignItem` (both already org-XOR-project gated): a foreign-but-existing `milestone_id` yields zero affected rows → `NOT_FOUND`, the item UNCHANGED, indistinguishable from a missing milestone. The **clear-to-null** path (`milestone_id = ""`) and the **nil = unchanged** path both satisfy the empty-`milestone_id` disjunct and carry NO milestone predicate — PRESERVED; the existing target-item `org_id = $caller` gate (`.77`) and the empty-`CallerOrgID` no-op are PRESERVED. Patched in lockstep: §10.1.1 write-surface predicate table (new `workitems.Update` `milestone_id` write-scope row, distinct from and additional to the existing target-item row); §4.4 `Update` RPC doc-comment (Tenant-gate block extended with the milestone-write paragraph — no struct/field change, `UpdateRequest` already carries `CallerOrgID` and `MilestoneID`); §6.2 Tool 5 `update` (gating prose extended to cover the `milestone_id` selector alongside the existing item gate — no wire change). **NOT touched (separate ownership): `apps/api/workitems/workitems.go` (the `Update` `WHERE`-clause milestone gate + the now-accurate doc-comment) + `apps/api/exitcriteriontest/write_surface_cross_tenant_test.go` (the missing owned-item + foreign-milestone subtest) — the implementation bead `unblock-tv8.84` (Greta) owns those on its branch.** **No DDL / migration / public-API / struct change** — the gate is a query predicate; the existence-only FK is untouched. **Root `docs/SPEC.md` is untouched** — no DDL change, and its §5.6 RBAC prose (row-level filtering applies "uniformly to every read and write path") is strengthened, not contradicted. Spec status remains APPROVED.
- 2026-06-15 — `create_milestone.project_id` INSERT-scope tenant gate (bead `unblock-tv8.83`, discovered-from the live MCP sweep / `unblock-tv8.78`, proven live 2026-06-12; status remains APPROVED — this is an additive §10.1.1 gate-model extension closing the LAST ungated cross-reference write-IDOR in the family, `create_milestone.project_id`, NOT a re-architecting). `workitems.CreateMilestone` inserted the project-scoped milestone's wire-supplied `project_id` with NO check that it belongs to the caller's org — the same IDOR class already closed for `.75` (`CreateLabel.project_id`), `.77` (`CreateMilestone.parent_milestone_id`), `.78` (`workitems.Create.*`), and `.80` (`AppendComment.parent_id`). `create_milestone.project_id` was the one remaining selector. **Locked contract:** on the **project-scoped branch** (non-empty `project_id`), the milestone INSERT becomes a guarded `INSERT … SELECT` requiring `project_id IN (SELECT id FROM org.projects WHERE org_id = $caller)` — mirroring the EXISTING `CreateLabel` INSERT…SELECT precedent: a foreign-but-existing `project_id` yields zero source rows → `NOT_FOUND`, nothing inserted, indistinguishable from a missing project. The **org-scoped branch** (`org_id` set, `project_id` empty) carries no project predicate, and the **empty-`CallerOrgID` no-op IS preserved** (a DELIBERATE divergence from `CreateLabel`'s hard-reject — `CreateMilestone` has trusted internal / E2E-seed callers per §10.1.1). The already-gated `parent_milestone_id` parent-read seam (bead `unblock-tv8.77`) is preserved unchanged. Patched in lockstep: §10.1.1 write-surface predicate table (new `workitems.CreateMilestone` `project_id` INSERT-scope row); §6.2 Tool 16 `create_milestone` (gating prose extended to cover the `project_id` selector alongside the existing parent-read seam — no wire change); §4.4.1 `CreateMilestone` doc-comment (Tenant-gate block extended with the project-scoped INSERT…SELECT paragraph — no struct/field change, `CreateMilestoneRequest` already carries `CallerOrgID`). **NOT touched (separate ownership): `apps/api/workitems/workitems.go` (the guarded INSERT…SELECT) + tests — the implementation bead `unblock-tv8.83` (Greta) owns those on its branch.** **No DDL / migration / public-API / struct change** — the gate is a query predicate; the existence-only FK is untouched. **Root `docs/SPEC.md` is untouched** — no DDL change, and its §5.6 RBAC prose already states row-level filtering applies "uniformly to every read and write path" (strengthened, not contradicted). Spec status remains APPROVED.
- 2026-06-12 — uniform §7 argument-validation contract WRITTEN at the MCP boundary (contract LOCKED by Miguel 2026-06-12 BEFORE implementation; bead `unblock-tv8.82`, discovered-from the live MCP sweep B2+B3; status remains APPROVED — this WRITES a uniform argument-validation contract + ENRICHES the advertised tool schema, an additive boundary-contract extension, NOT a re-architecting). **The live `tools/list` schema is the go-sdk-reflected (`jsonschema.ForType`, v1.6.0) registered schema — type + required + `additionalProperties:false` only — NOT `catalogue.json` (an off-wire P01 authoring artifact for the P02 `meta_catalogue` tool); the catalogue's advertised bounds/enums were never on the wire.** Three locked decisions: **(1) Rich schema advertised (NET-NEW)** — `tools/list` now advertises the FULL input-argument contract: `enum` values, `minimum`/`maximum` bounds, and `required[]` (today only type+required are reflected). **(2) Uniform §7 VALIDATION** — EVERY argument-shape violation (missing required, invalid enum, wrong type, AND out-of-range numeric bound) returns the §7 `VALIDATION` envelope (`kind=VALIDATION`, `trace_id`, `data.field` naming the offending argument; `data.reason`/`data.bound` on a range violation), uniformly across the 23-tool surface — no bare `isError` text frames for argument violations. **(3) Bounds ENFORCED (out-of-range REJECTS — behavior change)** — `prime.ready_limit` (1..50), `ready.limit` (1..200), `list.limit` (1..200), `search.limit` (1..100): a value below the minimum (incl. `limit<=0`) OR above the maximum REJECTS with `VALIDATION`, NOT silently coerced/clamped. This re-locks the round-7 coerce-to-default / clamp-to-max limit semantics to the REJECT semantics wherever handler doc-comments or in-code comments disagree (an OMITTED optional limit still takes the per-tool default — the bound check applies only to a supplied value). **Mechanism (documented at contract altitude):** the REGISTERED (SDK-validated) schema is RELAXED on exactly the keywords the shared boundary-validation layer owns (so `applySchema` → `resolved.Validate` does not pre-reject with a bare frame), while `tools/list` advertises the full rich schema; a shared `validateArgs` pass at the handler boundary validates required/enum/type/range and mints §7 `VALIDATION` via the existing `apps/api/mcp/errmap.go` `mapError` path (`errs.InvalidArgument` → `VALIDATION` with `data.field`) — the precedent `apps/api/mcp/handler_update.go` already ships (relaxed `InputSchema` + handler-top raw-arg validation + `mapError`). Patched in lockstep: §7 `VALIDATION` table row (argument-shape violations enumerated; range `data.reason`/`data.bound` sample added) + new §7.3 (uniform argument validation; §7.3.1 bounds-enforced/reject re-lock; §7.3.2 relaxed-registered-schema + enriched-`tools/list` + shared `validateArgs` mechanism); new §6.2.0a (rich schema advertised NET-NEW + paginated `limit` bounds table + enum/type/required + round-7 re-lock; the `deps.RecentCascadeEvents` internal "capped at 50" private-RPC clamp is explicitly distinguished from the WIRE `ready_limit` bound and left untouched); §10.3 (catalogue.json = off-wire authoring artifact vs. the now-enriched live reflected `tools/list` schema reconciliation). NOT touched (separate ownership): `apps/api/mcp/` handlers + the shared validation layer + `catalogue.json` + tests (the implementation bead `unblock-tv8.82`, Greta, owns those on its branch). Root `docs/SPEC.md` is **untouched** — its §7.5 BLOCK-condition schema governs PIPELINE state-transition validation (Layer 1), not argument-shape validation, and carries NO claim that `catalogue.json` `input_schema` bounds are live-on-the-wire nor any clamp/coerce limit semantics; no contradiction exists (verified). Spec status remains APPROVED.
- 2026-06-12 — §6.2 Tool 13 I-4 prose/SQL pseudocode RECONCILED toward the §11.1.2 exit criterion (bug `unblock-tv8.81`, discovered-from the live MCP sweep; status remains APPROVED — this reconciles an internal SELF-CONTRADICTION toward the spec's own exit criterion, NOT a re-architecting / behavioral-contract change). **The contradiction (all three verbatim from the file):** the §6.2 Tool 13 I-4 PROSE row keyed the reject on `req.review_state` (so the one-call rework `set_state(impl=pending, review=needs_rework)` SUCCEEDS); the I-4 SQL PSEUDOCODE keyed the reject on the COALESCED `new_review IN ('approved','needs_rework')` (so the SAME call REJECTS with `review_change_requires_impl_done` — what the buggy code at `workitems.go:1707` implements); and the §11.1.2 EXIT CRITERION (~lines 3914-3917) REQUIRES that rework call to SUCCEED ("The same call when `review_state='needs_rework'` succeeds"). The exit criterion is authoritative, so the ONLY self-consistent reading is: **I-4 is the FORWARD review gate — `review_state → approved` requires `impl_state=done`; the `review_state → needs_rework` REVERSE trigger is EXEMPT** (it is precisely the I-5 rework path that legitimately reverts `impl_state` `done → pending`). The prose + exit criterion win; the SQL pseudocode was WRONG. **Fixed in lockstep (docs only):** (1) §6.2 Tool 13 I-4 SQL pseudocode — `violates_i4` predicate narrowed from `new_review IN ('approved','needs_rework') AND new_impl <> 'done'` to `new_review = 'approved' AND new_impl <> 'done'` (`needs_rework` dropped from the reject condition); (2) §6.2 Tool 13 I-4 prose row — rewritten unambiguously: I-4 requires `impl_state=done` only for a transition to `approved`; `needs_rework` is the rework trigger governed by I-5 (which permits the concurrent `impl done→pending`), explicitly noting the one-call `set_state(impl_state=pending, review_state=needs_rework)` on a claimed `impl=done` item SUCCEEDS (impl→pending, review→needs_rework, qa auto-reset→pending per I-1) per the §11.1.2 exit criterion. **The forward gate is PRESERVED explicitly** — `review_state → approved` on an `impl=pending` item still REJECTS (the carve-out is NOT widened to `approved`; widening would let unfinished work be approved and corrupt the §5.7.1 `pipeline_stage` derivation). §11.1.2 (~3914-3917) was VERIFIED already consistent (it states the rework call succeeds) and is NOT weakened. (The bead's cited "line 3884" is a MISLABEL — line 3884 is the `edge_added` cascade-event assertion, not the I-4 criterion; the binding criterion is §11.1.2 lines 3914-3917.) Root `docs/SPEC.md` is **untouched** — its §5.7 explicitly defers the precondition map ("The exact precondition map and error shapes land in the P02 spec") and carries NO I-4 forward/reverse gate claim; its §5.7.1 derivation table (`review_state = needs_rework → Implementation`; `review_state = approved AND qa_state = pending → Quality`) is fully consistent with the reconciled reading. **No DDL / migration / behavioral-contract change** beyond making the rework path reachable as the exit criterion always required. NOT touched (separate ownership): `apps/api/workitems/workitems.go:1707` (the `violates_i4` predicate fix) + the now-un-skipped I-5 property test — the implementation bead `unblock-tv8.81` (Greta) owns those on its branch. Spec status remains APPROVED.
- 2026-06-12 — comment `parent_id` same-item threading-scope contract WRITTEN (contract LOCKED by Miguel 2026-06-12 BEFORE implementation; bead `unblock-tv8.80`, discovered-from the live MCP sweep and proven live via the MCP endpoint; status remains APPROVED — this WRITES a missing threading-scope contract and closes an IDOR, an additive §10.1.1 gate-model extension, NOT a re-architecting). **This contract did not previously exist anywhere in the spec — a genuine GAP, not a drift.** `AppendComment` inserted the wire-supplied `parent_id` verbatim with NO scoping predicate (gating only the target `item_id` by org per the `unblock-tv8.77` round), and the `comments` FK is existence-only/unnamed, so a caller could thread a comment under a foreign-org OR cross-item parent — a live-proven cross-tenant IDOR, same class as `unblock-tv8.77` / `unblock-tv8.78`. (The bead's `§6.5` citation is a MISLABEL: spec §6.5 is "Cycle detection at write time" and PRD §6.5 is the comment kind×status axes — neither defines `parent_id` scope; the contract is written where the write-gate model lives, §6.2 Tool 10 + §10.1.1.) **Locked contract:** a comment's `parent_id`, when non-empty, MUST resolve to an EXISTING comment ON THE SAME item as the new comment (`item_id` = the target item); a foreign-org OR cross-item `parent_id` yields `NOT_FOUND`, indistinguishable from a missing parent. Same-item transitively guarantees same-org (the target item is already `CallerOrgID`-gated), so this is the stricter, correct predicate that closes the IDOR — no separate parent-org branch is needed. The self-parent prohibition (`comments_no_self_parent_chk`) and the empty-`parent_id` top-level path are preserved. Enforced by the `AppendComment` `INSERT … SELECT` predicate (the §10.1.1 write-gate mechanism), NOT by the existence-only FK. Patched in lockstep: §6.2 Tool 10 (`comment`) — new "Threading scope" paragraph; §10.1.1 write-surface predicate table — new `workitems.AppendComment` `parent_id` row (`parent_id IN (SELECT id FROM workitems.comments WHERE item_id = $target_item)`, foreign/cross-item → `NOT_FOUND`); §4.4 `AppendComment` RPC doc-comment — same-item `parent_id` scoping prose. NOT touched (separate ownership): `apps/api/workitems/workitems.go` (`AppendComment` predicate + the now-false code comment that claims no parent predicate) + tests — the implementation bead `unblock-tv8.80` (Greta) owns those on its branch. **No DDL / migration change is needed** — the gate is a query predicate, not DDL (the existence-only FK is untouched). Root `docs/SPEC.md` is **untouched**: no DDL/migration change and no comment-threading or `AppendComment` auth-model claim there contradicts the same-item rule (its §5.6 RBAC prose — row-level filtering "applied uniformly to every read and write path" — is strengthened, not contradicted; §9.4 `workitems.comments` DDL is unaffected). Spec status remains APPROVED.
- 2026-06-12 — corrective forward migration `0140` for the `cascade_events_kind_chk` in-place-edit drift (discovered-from live MCP testing on bead `unblock-tv8.79`; status remains APPROVED — additive migration-table lockstep, not a re-architecting). Round-6 widened `deps.cascade_events_kind_chk` from 2 kinds to 4 (`'close','edge_added','edge_removed','state_change'`) by editing `0050_deps.up.sql` IN PLACE (commit `3e0d00d`); because golang-migrate keys by version number, any DB that had already applied `0050` silently retained the stale 2-kind constraint, so `state_change`/`edge_added` cascade audit inserts fail with SQLSTATE 23514. Fix: a NEW up-only forward migration `0140_deps_cascade_events_kind_chk_fix.up.sql` that DROPs (IF EXISTS) and re-ADDs the constraint with the full 4 kinds — idempotent (no-op-equivalent on fresh DBs, corrective on stale ones). Roll-forward discipline is preserved: `0050` is NOT re-edited. No audit-kind coverage gap exists — §11.1.2 already enumerates and asserts all four kinds, and §3.2:242 (`0050` row) + the §9.4.4-mirroring "`cascade_events.kind` enum (round-6)" block in this file already list the 4 kinds in lockstep with the source (confirmed against migration source + spec; both at 4 kinds). Patched in lockstep: §3.2 migration table (new `0140` row after `0130`). NOT touched (already correct / separate ownership): §3.2:242 `0050` row + the §9.4.4 canonical-DDL mirror block (both already 4-kind), root `docs/SPEC.md` §9.4.4 (no DDL semantics change — the source and spec are already at 4 kinds), and `apps/api/db/migrations/0140_deps_cascade_events_kind_chk_fix.up.sql` itself (the implementation bead `unblock-tv8.79`, Greta, owns the migration file). Spec status remains APPROVED.
- 2026-06-12 — create-path cross-reference tenant validation (contract LOCKED by Miguel 2026-06-12 BEFORE implementation; bead `unblock-tv8.78`, discovered-from the `unblock-tv8.77` write-surface review and proven live via the MCP endpoint 2026-06-12; status remains APPROVED — drift-closure + §10.1.1 gate-model extension to the create path, not a re-architecting). The .77 round hardened the item/milestone write-**by-id** RPCs; this round closes the symmetric seam on the **create** path. `workitems.Create` (Tool 4) stamps `org_id` from the caller identity but, pre-this-round, accepted `project_id` / `parent_id` / `discovered_from_id` / `milestone_id` / `labels[]` from the wire and validated them for FK EXISTENCE in ANY org only — letting a caller create an item whose `org_id ≠` a referenced row's org. **(1) §10.1.1 per-RPC predicate table gains a `workitems.Create` row:** each wire reference is validated against the caller org before/at the INSERT inside the existing single create transaction (the bead-`unblock-tv8.17` atomicity contract). Per-reference predicates — `project_id` → `IN (SELECT id FROM org.projects WHERE org_id = $caller)` (CreateLabel precedent); `parent_id` / `discovered_from_id` → a caller-org item (`IN (SELECT id FROM workitems.items WHERE org_id = $caller)`); `milestone_id` → org-XOR-project (`org_id = $caller OR project_id IN (SELECT id FROM org.projects WHERE org_id = $caller)`, AssignItem precedent); `labels[]` → every `label_id` org-scoped to `$caller` OR project-scoped to a project in `$caller` (a foreign label attaches nothing). A foreign-but-existing reference yields the SAME `NOT_FOUND` as a missing id (never a "belongs to another org" message). The `dependencies[]` path is unchanged — already gated by `deps.AddEdgeInTx`'s `CallerOrgID` endpoint check. **(2) Gate-key framing (Miguel's DECISION, recorded in §10.1.1):** Create reuses its existing `req.OrgID` (already pinned from `identity.OrgID`, already validated non-empty) as the gate key — it does NOT introduce a separate `CallerOrgID` field and does NOT use the empty-`OrgID` no-op branch the .77 update/delete-by-id RPCs use, because Create's internal callers all pass a real same-org `OrgID`. Deliberate divergence from the .77 convention; identical coverage. **(3) §4.4 Create RPC doc-comment + §6.2 Tool 4** state the create-path reference validation and the foreign-reference → `NOT_FOUND` contract. **(4) §4.4 `CreateRequest` DRIFT-2 reconciliation:** the spec declared `Dependencies []Edge` plus a separate `DependencyEdge {BlockerItemID, Kind}` element type, but the shipped RPC field is `Dependencies []deps.Edge` (`deps.Edge` = `{ID, FromItem, ToItem, Kind, CreatedAt, CreatedBy}`, §4.5; only `FromItem` + `Kind` read on the create path, `ToItem` = the new item) and `DependencyEdge {BlockerItemID, Kind}` is actually the JSON WIRE shape living in the MCP handler (`apps/api/mcp/handler_create.go::createDependencyIn`), mapped to a `deps.Edge` before the RPC call. Spec reconciled to the real RPC/wire split (accuracy fix, no redesign). Patched in lockstep: §4.4 Create doc-comment + `CreateRequest` + `DependencyEdge` (drift reconciliation); §6.2 Tool 4 tenant-scoping note; §10.1.1 predicate table (`workitems.Create` row) + the create-path gate-key DECISION block. NOT touched (separate ownership): `apps/api/workitems/workitems.go` create-path gates + auth-model doc-comment (the implementation bead `unblock-tv8.78` owns that — Greta's lockstep). Root `docs/SPEC.md` is **untouched**: no DDL/migration change (no new column), and its §5.6 RBAC prose already states row-level filtering is "applied uniformly to every read and write path" — the create-path gate strengthens that claim, it does not contradict it; §9.4.x `workitems.items` DDL is unaffected. Spec status remains APPROVED.
- 2026-06-11 — round-16 write-surface tenant-hardening lockstep (contract LOCKED by Miguel 2026-06-11 BEFORE implementation; discovered-from the `unblock-tv8.75` review via the `unblock-tv8.77` investigation; status remains APPROVED — IDOR-seam closure, not re-architecting). The read path already self-gated via `rbac.For` (§10.1); this round hardens the WRITE path symmetrically. **(1) Every item/milestone write-by-id RPC self-gates via a row-level tenant predicate keyed on an internal `CallerOrgID` channel** (populated by the MCP handler from `identity.OrgID`, NEVER from the wire — the established label/milestone internal-channel pattern). Affected `workitems` RPCs: `Update` (Tool 5), `SetStateColumns` (13), `Close` (6), `Claim` (3), `Promote` (15), `AppendComment` (10), `UpdateMilestone` (17), `AssignItem` (18), plus `CreateMilestone`'s parent-read seam (16). Predicate forms: items — `org_id = $caller`; milestones — `org_id = $caller OR project_id IN (SELECT id FROM org.projects WHERE org_id = $caller)` (org-XOR-project; project-scoped milestones carry `NULL org_id`); `AppendComment` (an INSERT) — gated via INSERT…SELECT on the parent item's org. A foreign id yields `NOT_FOUND` (or zero rows inserted), never a cross-tenant mutation. **(2) The item/milestone RPCs take the empty-`CallerOrgID` NO-OP form** `($caller = '' OR <predicate>)` (the `MilestoneTree` precedent, NOT the label-write hard-reject): trusted internal no-auth callers (the §11.1.1 exit-criterion seed + integration tests) call them directly with no org context; MCP handlers ALWAYS pin `CallerOrgID`, so the no-op branch is unreachable from the agent surface. Ratified explicitly in §10.1.1. **(3) `deps.AddEdge` / `deps.RemoveEdge` harden too** (folded in by Miguel) — both are MCP-reachable (Tools 11/12) and resolve endpoint orgs from the DB; both now receive `CallerOrgID` and reject with `NOT_FOUND` when a resolved endpoint org ≠ caller's org. **(4) `CreateLabel` gains the explicit empty-`CallerOrgID` `InvalidArgument` guard** (closing the deferred-epic RISK) — consistency with `UpdateLabel` / `DeleteLabel`; hard guard is correct there (MCP-only callers). Patched in lockstep: new §10.1.1 (write-gate model + predicate table + no-op-vs-hard-guard ratification); §4.4 `Update`/`AppendComment`/`SetStateColumns`/`Close`/`Claim` doc-comments + request structs (new `CallerOrgID` field); §4.4.1 `CreateMilestone`/`UpdateMilestone`/`AssignItem`/`MilestoneTree` doc-comments + request structs; §4.4 `CreateLabelRequest` (new `CallerOrgID` field + `InvalidArgument` guard prose) + label-RPC header comment + §6.2 closing note; §4.5 `AddEdge`/`RemoveEdge` doc-comments + request structs; §6.2 Tools 3/5/6/10/13/15 tenant-gate notes + Tools 16/17/18/19/20 auth wording (the "does not self-gate" phrasing on Tools 16/20 reworded to the row-level predicate) + Tools 11/12 tenant-gate notes. NOT touched (separate ownership / no drift): `apps/api/workitems/workitems.go` auth-model doc-comment + the catalogue + migrations (Greta's implementation lockstep on the bead branch); root `docs/SPEC.md` (its §5.6 RBAC prose already states row-level filtering is "applied uniformly to every read and write path" — already aligned with this hardening, no contradicting "write RPCs do not self-gate" claim exists there). **Addendum (pre-QA cleanup, commits `8cfaff1`/`ea7b8ca` on branch `chore/unblock-tv8-77`):** `AssignItem`'s tenant gate extends beyond the target item — the assign-branch milestone read is now ALSO `CallerOrgID`-gated with the org-XOR-project milestone predicate, so a foreign `milestone_id` yields `NOT_FOUND` before the M-INV-7 check instead of disclosing existence via `PRECONDITION_NOT_MET`. Patched in lockstep: §10.1.1 per-RPC predicate table (new `AssignItem` assign-branch milestone-read row) + §6.2 Tool 18 (`assign_item`) prose. No DDL / migration-file / public-API / `go.mod` change. Spec status remains APPROVED.
- 2026-06-11 — round-16 lockstep-completion drift-closure (spec-drift surfaced by `/investigate` on bead `unblock-tv8.75`, the label-registry MCP Tools 20–23; status remains APPROVED): three drift items closed, one with a NEW migration, two mechanical-prose. (1) **DRIFT-1 — labels gain `updated_at`, RESOLVED by Miguel's decision (2026-06-11).** §4.4 `Label` always declared `UpdatedAt time.Time`, but the `workitems.labels` DDL omitted the column and the §6.2 closing note said "no new migration is required" — a genuine contradiction. Resolution (locked): ADD the column via a NEW up-only migration `0130_workitems_labels_updated_at.up.sql` (`updated_at timestamptz NOT NULL DEFAULT now()`, next free slot after the committed `0120`), rather than dropping the struct field — the registry is mutable via Tool 22 `update_label` and `items` / `milestones` / `comments` all carry `updated_at`. Patched in lockstep: §3.2 migration table (new `0130` row), the §6.2 "no new migration" closing note (rewritten to name `0130`), §4.4 label-RPC header comment + the `UpdateLabel` doc-comment (bumps `updated_at` on every write), §12 D-8 task row (migration `0130` reference), and root `docs/SPEC.md` §9.4.3 canonical `workitems.labels` DDL (column added with a dated provenance comment, mirroring the §9.4.6 precedent) + its §11 P01 row + its changelog. (2) **DRIFT-2a — Tools 20–23 authorization wording** aligned with the established Bearer-Identity org-scoping pattern (CONFIRMED against code: zero `org.Authorize` calls in `apps/api/mcp/`). The write tools (Tool 20 `create_label` / 22 `update_label` / 23 `delete_label`) had `org.Authorize`-on-`workitems.labels` / "RBAC-gated (action …)" phrasing and the closing note said the RPCs "route through `org.Authorize` exactly like the item RPCs"; reworded to the truth — write tools resolve the caller via `withIdentityFromReq` and pass `identity.OrgID` into a backing RPC that does NOT self-gate; the read tool (Tool 21 `list_labels`) self-gates via `rbac.For` SQL tenant-predicate injection (auth-model doc-comment `apps/api/workitems/workitems.go:28-66`). Mirrors the Tools 16–19 wording corrected in commit `ae30927`. (3) **DRIFT-2b — Tools 16/19 residual wire hygiene** (post-QA addendum from `.74`, CONFIRMED against `catalogue.json`). Tool 16 `create_milestone` and Tool 19 `milestone_tree` JSON wire samples still listed `org_id` as a client argument — the shipped tools take NO wire `org_id` (org pinned to `identity.OrgID`; the catalogue input-schemas omit it). Removed `org_id` from both samples (Tools 17/18 samples verified already clean). Tool 19's gating prose also claimed the backing RPC "self-gates via `rbac.For`" — corrected to the shipped mechanism: an EXPLICIT tenant predicate in the rooted-CTE anchor (`apps/api/workitems/workitems.go` ~2691, gated by `req.OrgID = identity.OrgID`, with the empty-`OrgID` internal-caller no-op). No catalogue / code / migration-file change — the implementation bead `unblock-tv8.75` owns the migration `0130` file + handler + RPC code. (4) **DRIFT-2c — label tools pin org to identity, NO wire `org_id` (DECIDED by Miguel 2026-06-11).** The remaining open question flagged during this drift-closure is now LOCKED: the label tools (Tools 20–23) pin org to the Bearer-resolved identity exactly like the milestone tools (Tools 16–19) — there is NO wire `org_id`. The optional `project_id` argument is the XOR selector: absent → org-scoped label (org from `identity.OrgID`); present → project-scoped (the handler/RPC validates the project belongs to the caller's org). Patched in lockstep: §4.4 `CreateLabelRequest` + `ListLabelsRequest` — `OrgID` re-annotated as populated from `identity.OrgID` and NEVER from the wire (mirrors `CreateMilestoneRequest` / the §6.2 Tool 19 read prose; `ProjectID` is the sole org/project wire selector); §6.2 Tool 20 `create_label` and Tool 21 `list_labels` JSON wire samples — `org_id` removed (Tools 22/23 samples verified already clean, scoping via `label_id`). `UpdateLabelRequest` / `DeleteLabelRequest` carry no `OrgID` (they scope via `LabelID`), so no struct change there. **(5) DRIFT-3 — two justified code-review divergences on `unblock-tv8.75` RATIFIED at spec level (DECIDED by Miguel 2026-06-11), in lockstep with the code rework on branch `feat/unblock-tv8-75-label-registry-mcp-tools`.** (5a) **`ListLabels` gates via an EXPLICIT raw-SQL tenant predicate, NOT `rbac.For`.** The shipped `ListLabels` RPC cannot use `rbac.For` because the PRD §6.4 project-wins-on-identical-name resolution is a `UNION ALL` that `rbac.For` cannot express; it instead injects an explicit `org_id = identity.OrgID` predicate into its raw SQL — the SAME justified-deviation precedent as Tool 19 `milestone_tree`, whose prose was corrected earlier in this changelog. The earlier "self-gates via `rbac.For`" wording for `ListLabels` was therefore wrong; corrected at every site — §6.2 Tool 21 `list_labels` gating prose + its wire-sample comment (mirrors the corrected Tool 19 style), the §6.2 Tools 20–23 "Label private RPCs" closing note, and the §4.4 label-RPC header comment + `ListLabelsRequest.OrgID` field comment. (5b) **`UpdateLabel` / `DeleteLabel` add a row-level tenant predicate.** The rework makes the targeted row's tenancy a precondition (label `org_id = identity.OrgID` OR its `project_id` belongs to a project in the caller's org), so a foreign `label_id` yields `NOT_FOUND` instead of acting cross-tenant — closing the same IDOR seam Tool 19 closed for reads. Reworded the §6.2 Tool 22 / Tool 23 auth sentences (formerly "the backing RPC does not self-gate") and the §4.4 `UpdateLabel` / `DeleteLabel` doc-comments to state the row-level constraint. The equivalent item / milestone write-RPC wording is deliberately NOT touched here — hardening those RPCs is a separate new bead. No catalogue / migration / public-API / DDL change (no canonical DDL is affected, so root `docs/SPEC.md` is untouched); the implementation bead `unblock-tv8.75` owns the handler + RPC code on its branch. Spec status remains APPROVED — drift closure, not a re-architecting.
- 2026-06-11 — round-16 lockstep-completion drift-closure (spec-drift surfaced by `/investigate` on bead `unblock-tv8.74`; status remains APPROVED): two mechanical prose corrections, no design change. (1) **§4.4.1 milestone P02-deferral stragglers corrected** — the 2026-06-04 round-16 changelog claimed §4.4.1 "milestone-tools-now wording" was patched, but three prose lines escaped and still said the milestone MCP tools defer to P02 (the §4.4.1 intro paragraph, the `UpdateMilestone` reparenting doc-comment cross-reference, and the `MilestoneTree` "Used by" note); all three now state the tools ship NOW in P01 as MCP Tools 16–19 (§6.2), in lockstep with the round-16 §1/§6 override. The genuinely-deferred reparenting feature and the 4 P02 memory tools are untouched. (2) **§6.2 Tools 16–19 authorization wording aligned with the established Bearer-Identity org-scoping pattern** — the round-16 Tool 16/18 contracts mentioned a literal `org.Authorize` call, but NO shipped MCP handler (all 15) calls `org.Authorize` directly; the established pattern is org scoping via the Bearer-resolved org-scoped `Identity` (`withIdentityFromReq` + `identity.OrgID` on the write path; `rbac.For` SQL-injected tenant predicate on the read path), with the backing `workitems` write RPCs not self-gating (auth-model doc-comment at `apps/api/workitems/workitems.go:28-66`). Tools 16/17/18 (write) and 19 (read) reworded accordingly. Scoped to milestone tools only; the label Tools 20–23 (sibling bead `unblock-tv8.75`) carry the same `org.Authorize` phrasing and are deliberately left for .75's flow. No DDL / migration / public-API / `go.mod` change — the implementation bead `unblock-tv8.74` owns the catalogue + handler code. Spec status remains APPROVED.
- 2026-06-11 — round-16 drift-closure (spec-drift closure from `/investigate` on bead `unblock-tv8.73`; status remains APPROVED): four mechanical lockstep corrections to the 2026-06-04 round-16 amendment, no design change. (1) **Migration renumbered `0110` → `0120`** — the round-16 text named the new up-only migration `0110_mcp_issued_to_user_notnull.up.sql`, but slot `0110` is already taken by the committed `0110_mcp_warning_codes.up.sql` (bead `unblock-tv8.63`); renamed to `0120_mcp_issued_to_user_notnull.up.sql` at every reference (round-16 changelog bullet, §3.2 table row + sequence note, §4.3.2 step 8, §11.1.2, §12 A-3 row). (2) **Root `docs/SPEC.md` §9.4.6 brought into lockstep** — the `mcp.api_keys.issued_to_user` DDL there still declared the column nullable with `ON DELETE SET NULL`; corrected to NOT NULL / `ON DELETE CASCADE` with the audit-survival note, and the §11 P01 row + changelog migration name renumbered to `0120`. (3) **§3.2 prose column fix** — the audit-survival FK was mis-named `mcp.tool_calls.issued_to_user` (no such column); corrected to `mcp.tool_calls.api_key_id` (already `ON DELETE SET NULL`). (4) **§4.3.2 step 3 SELECT aligned with code** — the documented `validateAPIKey` SELECT omitted `issued_to_user`; the column is added to match `apps/api/auth/auth.go`. No DDL semantics change, no public-API change, no `go.mod` change — the implementation bead `unblock-tv8.73` owns the migration file + code. Spec status remains APPROVED.
- 2026-06-04 — round-16 (P01 MCP tool-surface scope amendment; surfaced by the 2026-06-03 local-MCP demo + 2026-06-04 review; covers epic `unblock-tv8` beads .71/.72/.73/.74/.75/.76): the P01 agent-facing tool inventory grows from **14 to 23** and the v1.0 inventory from **18 to 27** (still + the 4 memory tools at P02). Five contract changes land in lockstep. (1) **`promote` keystone (.71)** — a new Tool 15 `promote` transitions an item Backlog→Ready, precondition `status='Backlog' AND is_ready=true`; the not-ready rejection reuses the EXISTING §7 `PRECONDITION_NOT_MET` kind, extended ADDITIVELY with a `{status, required}` pair alongside the existing `{missing}`/`{rejection_reason}` shapes. .71 also closes round-12 DRIFT-2 ("the state-machine does not allow the canonical fixture's Done/Ready end-states via RPC") by pinning the FULL Backlog/Ready/InProgress/Blocked/Done transition map (§6.6) incl. the Ready→Blocked demotion that fires when a `blocks` edge is later added to a Ready item. .71 further pins the **`is_ready`-on-create rule**: a freshly-created item with no incoming `blocks` edges MUST get `is_ready=true` INLINE at create time inside `workitems.Create`'s own transaction (today `recomputeReady` never fires on the create path, so such items are stranded non-ready); `workitems.Create` joins the §6.3.0 Regime A `is_ready` single-writer allow-list and the §11.3 `no_direct_is_ready_write` allow-list, and the misleading create doc-comment is corrected. (2) **`issued_to_user` REQUIRED (.73)** — the column becomes NOT NULL on every MCP API key; all "nullable / org-level service key" wording is removed; a new up-only migration `0120_mcp_issued_to_user_notnull.up.sql` runs `ALTER … SET NOT NULL` and swaps the FK from `ON DELETE SET NULL` to `ON DELETE CASCADE` (deleting a user deletes their keys; `mcp.tool_calls` audit rows survive via their own `ON DELETE SET NULL`); `IssueAPIKey` rejects empty `IssuedToUser` with `InvalidArgument`; the §4.3.2 validate path never constructs an empty-UID identity. (3) **Milestone MCP tools (.74)** — OVERRIDES the §6.2 round-2 D1 deferral note + the §1 / §4.4.1 P02-deferral wording: the four milestone tools `create_milestone` / `update_milestone` / `assign_item` (incl. unassign via empty `milestone_id`) / `milestone_tree` are exposed NOW (Tools 16–19), thin MCP facades over the already-shipping `workitems.CreateMilestone`/`UpdateMilestone`/`AssignItem`/`MilestoneTree` private RPCs (§4.4.1). (4) **Label-registry MCP tools (.75)** — four NEW org-scoped, RBAC-gated tools `create_label` / `list_labels` / `update_label` (rename + recolor) / `delete_label` (Tools 20–23) over the existing `workitems.labels` table; four new `workitems` private RPCs back them. (5) **`show` resolves references (.76)** — Tool 7 `show` resolves the parent and the direct in/out dependency targets to `{id, title, status}` objects (one level deep, payload bounded) instead of bare IDs. The §7 `PRECONDITION_NOT_MET {status, required}` extension defined by .71 is the SAME taxonomy reused by .72 (the `claim`-on-not-Ready error) — .72 carries no separate spec text; its error is satisfied by the .71 §7 design. Patches in lockstep: §1 overview (tool-count bullets + milestone P02-defer wording) … §3.2 (new migration `0120`) … §4.1 (`IssueAPIKeyRequest.IssuedToUser` REQUIRED + doc-comment) … §4.3.2 step 8 (no empty-UID identity) … §4.4.1 (milestone-tools-now wording) … §5.2 (NOT-exposed list trimmed) … §6 header ("The 23 P01 MCP Tools") … §6.2 deferral note (rewritten: milestone + label + promote tools now in P01) … §6.2 Tool 7 `show` (reference resolution) … new §6.2 Tools 15–23 … new §6.6 (status transition map) … §6.3.0 Regime A allow-list (+`workitems.Create`) … §7 (`PRECONDITION_NOT_MET {status, required}` extension) … §10.3 (catalogue tool count 14→23) … §11.1.2 (promote assertion) … §11.3 `is_ready` allow-list (+`workitems.Create`) … §12 task table (D-2 promote, D-8 milestone+label tools, A-3 migration 0120) … §14 approval checklist (14→23). Cross-doc reconciliation (CONFIRMED by Miguel, no doc left stale): docs/PRD.md FR-8 + priority row, docs/SPEC.md §5.2.2 inventory + the "18 tools" mentions, and docs/plans/01-plan-backend-mvp.md tool table are all updated to the identical P01=23 / v1.0=27 counts, each carrying a P01-round-16 provenance note. Spec status remains APPROVED — this is a scope amendment, not a re-architecting.
- 2026-05-08 — DRIFT-1 (naming): clarified §3.5 that the four logical secret names are spec-level identifiers; added logical-name ↔ Go-field mapping table for the Encore Go secrets manifest.
- 2026-05-08 — DRIFT-2 (format): corrected the local-secrets file path/format from `.encore/local-secrets.toml` (TOML) to `apps/api/.secrets.local.cue` (CUE) per Encore official docs (https://encore.dev/docs/go/primitives/secrets); updated syntax examples and gitignore guidance.
- 2026-05-11 — round-4 (auth drift fixes from Sherlock's investigation on bead unblock-tv8.7): §4.3.2 step 2 aligned with locked key-format note (DRIFT-A); §4.3.3 AuthHandler signature corrected to Encore structured-params form for header dispatch (DRIFT-B); §12 task table cell for B-1 corrected from §6.4 to §4.3.3 (DRIFT-C); session-path P01 contract pinned (returns errs.Unimplemented; multi-org disambiguation deferred to BFF phase).
- 2026-05-12 — round-5 (tracing contract from Sherlock's investigation on bead `unblock-tv8.5`): §10.2 picks Option B — ULID minted at MCP entry, propagated via `context.Context` only; removed the spurious `X-Unblock-Trace-Id` outgoing-RPC header (Encore's generated client carries `ctx` across private RPCs for free, and Pub/Sub embeds the id in the payload). §7, §8.1, §8.2, §4.5, and §6.3.1 reworded to reference Option B. Encore's runtime `req.Trace.TraceID` is observability-only and not persisted. DDL frozen — no schema change.
- 2026-05-14 — round-7 (cursor keyset pagination, P01): Tools 2 (`ready`), 8 (`list`), and 9 (`search`) now share a cursor keyset pagination contract pinned in the new §6.2.0. Tool 2's "No pagination at v1.0" paragraph is removed; the §6.2 contracts for Tools 2/9 gain `cursor` argument + `next_cursor` result (Tool 8 already carried both). Cursors are opaque base64url-encoded JSON tuples HMAC-signed with `API_KEY_HMAC_SECRET`; the per-tool tuples are `{priority, created_at_unix_us, id}` (Tool 2), `{id}` (Tool 8), `{rank, item_id, comment_id}` (Tool 9). Invalid cursors → §7 `VALIDATION` envelope with `data.field = "cursor"`. No schema change — migration `0100` already covers the Tool 2 ORDER BY.
- 2026-05-12 — round-6 (cascade-symmetry): the cascade subsystem is split into two regimes (new §6.3.0). `is_ready` is maintained inline by the writer that mutated the row/edge (single-hop); `pipeline_stage` is maintained exclusively by the cascade subscriber (multi-hop). `deps.cascade_events.kind` CHECK is extended from 2 values (`'close' | 'edge_removed'`) to 4 values (`'close' | 'edge_added' | 'edge_removed' | 'state_change'`). Tools 6 (close), 11 (add_dependency), 12 (remove_dependency), 13 (set_state — narrow rule for §5.7.1-affecting writes), and `workitems.Claim` (only on the I-3 reset path) post-commit publish `CascadeRequested` with the matching `Reason`. Tool 12 reuses the inline audit row's `event_id` on its post-commit publish; the subscriber's `ON CONFLICT (event_id, triggered_by_item_id) DO NOTHING` collapses the second insert to no-op. The §11.3 single-writer invariant is fractured: (a) `pipeline_stage` single-writer = cascade subscriber; (b) `is_ready` single-writer = the mutating call site (Tool 6 close, §6.5 add_edge, Tool 12 remove_edge, internal helper `deps.recomputeReady`); the linter rule scope tightens to `pipeline_stage` only with an explicit allowlist for `is_ready`. DDL migration `0050_deps.up.sql` updated in lockstep (CHECK list + doc comments).
- 2026-05-15 — Round 8 — D-4 drift cleanup (targeted clarifications surfaced during `/investigate` on bead unblock-tv8.19; status remains APPROVED): (1) §4.4 `SearchRequest` / `SearchResponse` gain typed cursor fields (`CursorRank`, `CursorItemID`, `CursorCommentID` + matching `NextCursor*`) and a doc-note pinning the keyset tuple `(rank desc, item_id asc, comment_id asc)` and `LIMIT+1` over-fetch semantics — mirrors the Ready RPC pattern in `apps/api/workitems/workitems.go` and aligns §4.4 with the §6.2.0 / §6.2 Tool 9 cursor contract. `SearchHit` and §3.4 FTS DDL unchanged. (2) §6.2 Tool 10 (`comment`) gains a one-line note documenting the body-length enforcement boundary: handler enforces 1..16384 chars at the MCP boundary; `workitems.AppendComment` enforces the non-empty floor. (3) §2 C3 row and §4.3.1 SDK-pin paragraph updated from `github.com/modelcontextprotocol/go-sdk v0.5.0` to `v1.6.0` with a D-1 rationale ("v1.6.0 is the latest stable as of phase-01 implementation; v0.5.0 was the original pin during planning"); aligns the spec with `apps/api/mcp/transport.go:18-22` and `go.mod`. Clarification patch only — no re-approval round.
- 2026-05-19 — round-9 (rbac-coverage): §10.1 — added deps.cascade_events to E-3 RBAC suite scope (closes coverage gap on the AF2 read path via Tool 1 prime). C-6 scope unchanged.
- 2026-05-19 — round-10 (rbac-coverage closure for E-3 dual-shape): §10.1 — `deps.cascade_events` joins `org.resourceAllowed` and `org.agentReadWriteResources` so the Authorize-gate `KindAuthorizeOnly` axis can be exercised alongside the existing `KindOrgScoped` row-leak axis. Agents read the table through Tool 1 `prime`'s AF2 path (read-side); writes remain closed-loop (only the cascade subscriber emits rows server-side). Without this allow-list extension, an `KindAuthorizeOnly` tuple short-circuits to `InvalidArgument` instead of asserting the intended `PermissionDenied` contract. CI separate-report wiring (E-3 bead AC #3) is reassigned to A-6 (`unblock-tv8.6`, infra-supervisor) and gated by an explicit `tv8.25 → tv8.6` dependency edge; E-3 ships the suite discoverable as `encore test ./apps/api/shared/rbactest/...`, A-6 wires the separate gate.
- 2026-05-19 — round-11 (`-race` removed from §11.2 NFR-10 gate set, encore upstream bug closure): §11.2 NFR-10 — `go test ./... -race` dropped from the gate set and replaced with a split between Encore service packages (use `encore test ./...`, no `-race`) and leaf packages without `encore.dev` imports (use `go test -race`, native Go toolchain). Reason: [encoredev/encore#1943](https://github.com/encoredev/encore/issues/1943) — `encore test ... -race` reproducibly SIGSEGVs inside the encore-go runtime's `lazyTraceInit.initStream` goroutine spawn (cross-platform: confirmed on macOS arm64 and Linux amd64 ubuntu-24.04 GHA runner). Bug filed 2025-05-27, no maintainer triage, no fix PR. Toolchain footprint: encore v1.57.0, encore-go go1.26.2-encore (verified no race-related fixes in v1.57.1..v1.57.5). The rbactest suite is single-threaded by design (`rbac.Bind` not goroutine-safe; no `t.Parallel`), so dropping `-race` removes no real coverage on the encore-side gate. Leaf-package race coverage remains via native `go test -race` on `apps/api/shared/ulid/`, `apps/api/shared/rbac/`, `apps/api/shared/lint/` (packages without `encore.dev` imports). E-3 bead AC #1 (tv8.25) and A-6 bead AC (tv8.6) are patched in lockstep with this round.
- 2026-05-25 — round-13 (cascade-subscriber test invocation; spec-drift closure from `/investigate` on bead `unblock-tv8.26`): §11.1.2 and §11.3 require row-level assertions on `deps.cascade_events` for kinds `'close'`, `'edge_added'`, and `'state_change'`, but those rows are only written by the cascade subscriber (`apps/api/deps/cascade_subscriber.go::handleCascadeRequested`) and two facts make the assertions unreachable in-test by construction: (1) Encore Pub/Sub subscriptions DO NOT fire under `encore test` (the test harness simulates publishes but does not consume them — `et.Topic(...).PublishedMessages()` is publish-side only), and (2) `handleCascadeRequested` is package-private to `deps` with no exported test hook. Resolution: a thin exported wrapper `deps.HandleCascadeRequestedForTest(ctx context.Context, msg *deps.CascadeRequested) error` is added (file location is implementor's call — either `apps/api/deps/cascade_subscriber.go` or a new `apps/api/deps/export_test_handler.go`), pass-through to `handleCascadeRequested` with no behavioural divergence from production. The exit-criterion harness in `apps/api/exitcriteriontest/` (and any future Encore test needing cascade row materialisation) publishes via the producing RPC, captures published messages via `et.Topic(deps.CascadeRequestedTopic).PublishedMessages()`, then invokes `deps.HandleCascadeRequestedForTest` once per captured publish to drive the subscriber. This mirrors the established `mcp.ServeMCPForTest` precedent (`apps/api/mcp/export_test_writer.go:49-65`) — a thin exported wrapper around a package-private handler whose `ForTest` suffix is the audit trail. The wrapper is exported on the production import path BUT does not appear on Encore's API surface (plain Go function, not an `//encore:api`), so the public RPC catalogue is unaffected. §11.1.1 (round-12) gains a "Cascade subscriber test invocation" paragraph codifying the contract; §11.3 gains a one-line cross-reference under the single-writer invariant block. No DDL change, no migration, no public API change, no `go.mod` addition. Spec status remains APPROVED — this is a test-harness contract clarification, not a re-architecting.
- 2026-05-29 — round-15 (NFR-1 harness test-isolation + mcpaudittest hardening; CI-failure closure on bead `unblock-tv8.24` rework, CI run 26633703926): the round-14 gate-semantics assumption — that the harness could live in the default `encore test ./...` suite with `UNBLOCK_PERF_GATE` merely controlling assertion fatality — was proven incomplete by CI. Under the default full-suite run, the perftest package co-schedules with ~15 other test packages against ONE shared local Postgres: warm-cache `Validate`/`Claim`/`Ready` calls ballooned from a local ~87 ms p99 to 5–16 s; one measurement-loop response returned an empty body and tripped a hard `t.Fatalf` ("no SSE data" at `harness_test.go:220`) that the gate does NOT guard (it guards only the p99/goroutine assertions, not transport errors); and the harness's ~630 concurrent `mcp.tool_calls` audit-row writes broke `mcpaudittest`'s `TestD1_POSTNoAuthReturnsUnauthenticated` global-count assertion (`tool_calls rows = 1, want 0`). §11.2 NFR-1 gains two paragraphs: (1) **Test isolation** — the harness MUST be excluded from the default suite (Gate 5) and run ONLY in a dedicated isolated CI step under `UNBLOCK_PERF_GATE=1`; with the gate unset the package MUST contribute zero DB load and zero `mcp.tool_calls` rows (no seed, no loops — not merely log-and-pass); mechanism (build tag `//go:build perf` vs `UNBLOCK_PERF_GATE`-gated `t.Skip`/`TestMain` short-circuit) is the implementer's choice, validated by `encore check` + both suite runs; dedicated CI step owned by Olive. (2) **mcpaudittest hardening** — `selectToolCalls` (global `mcp.tool_calls` query) MUST be scoped to the test's own org/session so the D1 audit-row assertions are robust to any concurrent writer; a pre-existing latent test-isolation defect that the perftest load made deterministic, fixed in the same rework. Additionally the W3 paragraph's assertion wording is corrected from "asserts 401 / errs.Unauthenticated" to the actual MCP transport signal (HTTP 200 + JSON-RPC envelope `code -32000`, `data.kind=UNAUTHENTICATED`), ratifying the DEVIATION logged on the bead and confirmed at code review. No DDL / migration / public-API / `go.mod` change. Spec status remains APPROVED — test-harness isolation + sibling-test correctness, not re-architecting. Bead `unblock-tv8.24` reopened to rework in lockstep.
- 2026-05-29 — round-14 (NFR-1 latency-harness scope codification; spec-drift closure from `/investigate` on bead `unblock-tv8.24`): §11.2 NFR-1 expanded from a single latency line into the full E-2 harness contract, folding in the two cross-linked WARNINGs from the closed B-1 review (`unblock-tv8.7`) that were recorded on the bead but absent from the spec. (DRIFT-A) seeding doctrine pinned — harness owns its fixture via direct `sqldb.Exec` per the §11.1.1 round-12 doctrine, shortULID-salted slug, in-test key issuance via direct `INSERT INTO mcp.api_keys`, seed `N = 2 × iterations` ready rows. (DRIFT-B) W3 negative-auth-path coverage promoted from cross-link note to acceptance scope — the harness package ships a sibling test covering the §4.3.2 negative paths (revoked / expired / unknown-prefix / bad-HMAC / missing-prefix), each asserting `401` / `errs.Unauthenticated`, closing the inspection-only gap from B-1. (DRIFT-C) W4 goroutine-leak detection given a concrete contract — three `runtime.NumGoroutine` samples (`baseline` / `peak` / `drained` after a 2 s post-loop sleep) with assertion `drained - baseline ≤ 20` (drain-window check, not a per-iteration ratio). Gate semantics pinned — harness always logs samples + p99 + goroutine deltas as JSON-Lines via `t.Logf`; hard-fail (`t.Fatalf`) gated by `UNBLOCK_PERF_GATE=1` to keep CI advisory on slow runners, release-blocking wiring deferred to P02 (Olive). The cold-start exclusion is sharpened to "M ≥ 10 warm-up iterations discarded". No DDL change, no migration, no public API change, no `go.mod` addition, no production-code-path change — the harness is a new test-only Encore package (`apps/api/perftest/`) mirroring `apps/api/exitcriteriontest/`. Spec status remains APPROVED — this is a test-harness contract codification, not a re-architecting. Bead `unblock-tv8.24` AC patched in lockstep.
- 2026-05-22 — round-12 (seeder CLI deleted from P01 scope; spec-drift closure from `/investigate` on bead `unblock-tv8.23`): the one-shot `apps/api/cmd/unblock-seed/` Go CLI is removed from P01 entirely. Four blockers triggered the deletion — (DRIFT-1) no `auth.users` private RPC exists for the seeder to call; (DRIFT-2) the state-machine does not allow the canonical fixture's `Done`/`Ready` end-states via RPC; (DRIFT-3) `--issue-key` lacks operand flags; (DRIFT-4) Encore parser invariant E1388 forbids `package main` under `cmd/` from calling private RPCs, making "tiny CLI that calls private RPCs" architecturally impossible without scope inflation (new public RPCs or promoting the binary to an Encore service — both compromise design). Seeding responsibility moves to the E2E test package (`apps/api/exitcriteriontest/`) which owns a `TestMain` + direct-SQL seed mirroring `apps/api/shared/rbactest/seed.go:46-53` ("All rows go through direct `sqldb.Exec`, NOT through the auth/org RPCs"); fixture data lives as Go constants/structs in `apps/api/exitcriteriontest/fixture.go` (no YAML, no `gopkg.in/yaml.v3` dependency). The canonical 5-item graph topology (former §9.2: `itm_a`..`itm_e` with chain `a→b`, `b→c`, `b→d`, `d→e` plus the cycle-attempt edge closing the loop) is relocated verbatim into §11.1 as the authoritative exit-criterion fixture description. `--issue-key` is deferred entirely from P01 — no operator-key CLI in scope; in-test key issuance happens via direct `INSERT INTO mcp.api_keys` (computing `key_hash` with `secrets.APIKeyHMACSecret` per `apps/api/auth/apikey.go:103-111`); dev exploration outside tests uses `psql`. No new migrations, no DDL change, no `go.mod` additions. Patches in lockstep: §1 overview seeder bullet removed; §4.1 `IssueAPIKey` doc-comment updated; §4.4 / §4.4.1 / §6.2 Tool 4 seeder consumer notes rewritten; §9 deleted (tombstone retained to preserve §10..§14 numbering and the 30 cross-references that depend on it); §11.1 exit-criterion fixture relocation + E2E seed ownership note added; §11.4 docs and §14 approval checklist seeder mentions removed; §12 task-table E-1 row deleted (bead `unblock-tv8.23` cancelled in lockstep, post-spec, by the orchestrator); plan §2.1/§2.4/§4.5/§6 Q3 and root SPEC §9.4.6 / research AF4 / `apps/api/auth/auth.go` / `apps/api/db/migrations/0070_mcp.up.sql` updated. Spec status remains APPROVED — this is a scope reduction, not a re-architecting.
- 2026-06-02 — fifth-secret addition (§3.5; spec-drift closure from `/investigate` on bead `unblock-tv8.38` W1): §3.5 expands the locked secret set from four to **five**, adding `GITHUB_OAUTH_REDIRECT_URI` / `GitHubOAuthRedirectURI` — the OAuth2+PKCE registered callback URL sent as the `redirect_uri` parameter in the GitHub token-exchange POST body, preventing `redirect_uri_mismatch` once the BFF wires a real GitHub OAuth app. Consumer is `auth.ExchangeOAuthCode` (same call site as the other two GitHub OAuth secrets). Patched in lockstep: the DRIFT-1 naming note (four→five), the logical-name ↔ Go-field mapping table, the Go manifest struct, the `.secrets.local.cue` example, and the purpose table. This is a lockstep additive contract change — the auth service's boot fail-fast init MUST include the new secret, and every env type + `.secrets.local.cue` MUST provision it (per Olive). Spec status remains APPROVED — this is an additive contract clarification, not a re-architecting.

**Author:** Ada (architect)
**Date:** 2026-05-08
**Source PRD:** [docs/PRD.md](../PRD.md) (APPROVED, 2026-05-07)
**Source SPEC:** [docs/SPEC.md](../SPEC.md) (APPROVED 2026-05-07, round-3 research applied 2026-05-08; cascade_events.kind column added 2026-05-08)
**Source Plan:** [docs/plans/01-plan-backend-mvp.md](../plans/01-plan-backend-mvp.md) (APPROVED 2026-05-07; resolutions applied 2026-05-08)
**Source Research:** [docs/research/01-research-backend-mvp.md](../research/01-research-backend-mvp.md) (closed 2026-05-08; 6× CONTRADICTED, 3× PARTIAL, 1× CONFIRMED)
**Companion:** [docs/MANIFESTO.md](../MANIFESTO.md) (APPROVED, 2026-05-07)

**Round-2 review iterations (2026-05-08).** Architecturally-significant
findings closed in this round:
- D1 — Milestone CRUD private RPCs added in P01 (§4.4); MCP tools deferred
  to P02 (option (c) — preserves FR-8 "18 tools at v1.0").
- D2 — PRD §6.2 five structural state-machine invariants enforced at MCP
  layer in P01 (§6.2 Tool 13, §4.4 SetStateColumns + Claim).
- L7-W2 — `MCPHandler` raw endpoint pinned to a single `//encore:api`
  annotation with the elided-method form (raw-endpoint default per
  ENCORE.md §raw_endpoints; functionally equivalent to a conceptual
  `method=*` wildcard, which Encore v1.52.1 rejects with E1371);
  HTTP-method dispatch lives inside the handler.
- `deps.cascade_events.kind` column added (CHECK enum extended to 4
  values per §6.3.0); used by every `CascadeRequested` consumer.
  Reflected in SPEC §9.4.4 + §3.2 + §6.3.

> Stage 2 deliverable. This document is the **JSON-locked, RPC-locked,
> migration-locked implementation contract** for P01. Every field type,
> every signature, every error envelope, every migration filename is pinned
> here. Phase 02 may extend the surfaces named here, but P01 implementation
> may not deviate from them — deviations are flagged via the `DEVIATION`
> comment trail per Manifesto Law 8.
>
> **Research alignment.** This spec is grounded in the seven contradictions
> closed by Smith's research (C1, C2, C3, C5, C6, C7, plus AF1/AF5). Each
> design choice below references the research finding it honours; assumptions
> the research left as PARTIAL (R-P01-2, R-P01-4, R-P01-7) are pinned with
> explicit values here so implementation has no remaining ambiguity.

---

## 1. Overview

P01 ships the agent-facing core of `://unblock`:

- Five live Encore Go services (`auth`, `org`, `workitems`, `deps`, `mcp`).
- Three schema-only services (`providers`, `boards`, `memory` — DDL ships,
  service code ships in P02 / P05).
- Single Postgres database with **all eight schemas** migrating from a
  single migration-owner directory at `apps/api/db/migrations/`, owned
  by a dedicated zero-API `db` service.
- Streamable HTTP MCP transport (per MCP spec 2025-06-18) at `POST /mcp` +
  `GET /mcp` exposing **23 tools** with Bearer API key auth (round-16:
  the 14 core tools + the new `promote` tool + the four milestone tools
  + the four label-registry tools — see §6.2). The v1.0 inventory is
  **27** (these 23 P01 tools + the 4 memory tools added in P02).
- Cascade subsystem on Encore Pub/Sub maintaining `is_ready` and
  `pipeline_stage` materialised columns.
- Atomic claim transaction (`SELECT FOR UPDATE`).
- Cycle detection at write time using a depth-counter recursive CTE
  guarded by a per-project advisory lock.
- **Exit-criterion fixture seeding (round-12).** Owned by the E2E test
  package `apps/api/exitcriteriontest/` itself — `TestMain` runs a
  direct-SQL seed mirroring the `apps/api/shared/rbactest/seed.go`
  pattern; fixture data lives as Go constants in
  `apps/api/exitcriteriontest/fixture.go`. No standalone CLI binary
  (former §9 deleted; see §11.1 for the canonical fixture topology
  and round-12 changelog for the rationale).
- **Milestones (round-2 D1; round-16 amendment).** Recursive milestones
  (PRD §6.3 + SPEC §9.4.3) ship in P01 as **private RPCs**
  (`workitems.CreateMilestone`, `UpdateMilestone`, `AssignItem`,
  `MilestoneTree` — §4.4); the four M-INV-2 / M-INV-3 / M-INV-6 / M-INV-7
  invariants are enforced in app code per the SPEC §9.4.3 DDL note.
  **Round-16 (bead `unblock-tv8.74`) OVERRIDES the original D1 deferral:**
  the four milestone MCP tools (`create_milestone`, `update_milestone`,
  `assign_item`, `milestone_tree` — Tools 16–19, §6.2) ARE exposed in P01,
  as thin facades over the private RPCs above. P01 agents see them now.

P01 explicitly **defers** Layer-1 BLOCK conditions (P02), the four memory
tools (P02), GitHub webhook ingestion (P02), the Astro frontend (P05),
the plugin renderer (P04), and `unblock-code` (P03) — see Plan §3.

> **Round-16 amendment (2026-06-04).** The original round-2 D1 deferral —
> "the four milestone MCP tools defer to P02 to preserve FR-8 '18 tools at
> v1.0'" — is **OVERRIDDEN** by bead `unblock-tv8.74`. The milestone MCP
> tools (`create_milestone`, `update_milestone`, `assign_item`,
> `milestone_tree`) ship in P01 as thin facades over the milestone private
> RPCs (§4.4.1) that already exist. In the same round, `promote`
> (`unblock-tv8.71`) and four label-registry tools (`unblock-tv8.75`) are
> added. P01 now exposes **23** agent-facing tools (was 14); v1.0 is **27**
> (was 18). The FR-8 "18 tools" figure is reconciled to 27 across PRD §5.1
> FR-8, SPEC §5.2.2, this spec, and Plan §2.2 in lockstep (see the
> round-16 changelog).

**P01 Exit criterion (PRD §8 verbatim):** an agent authenticates via
Bearer API key and completes `prime → ready → claim → close` against a
manually-seeded graph; cascade fires; cycle detection rejects offending
edges.

---

## 2. Research Findings Resolution

This spec embodies the closure of all ten R-P01-* items and the five
additional findings (AF1–AF5) surfaced by Smith. Every contradiction has a
binding design decision below.

| Research finding | Status in research | How this spec resolves it |
|---|---|---|
| **C1 — Pub/Sub envelope `delivery_id`** | CONTRADICTED | §6.4 — publisher generates `event_id` (ULID) at emit; subscriber reads it from typed payload; idempotency key `(event_id, triggered_by_item_id)` enforced by DDL UNIQUE on `deps.cascade_events`. |
| **C2 — Encore DB ownership / multi-schema** | CONTRADICTED | §3.1, §5.1 — a dedicated `apps/api/db/` service is the sole migration-owner AND single binding authority; all eight schemas' migrations live under `apps/api/db/migrations/`. Every domain service receives its `*sqldb.Database` handle via the canonical BindDB late-bind pattern (each service declares `var db *sqldb.Database` + `func BindDB(d *sqldb.Database) { db = d }`; `apps/api/db/db.go::init` calls `<service>.BindDB(DB)` for every consumer). Domain services MUST NOT declare `sqldb.Named("unblock")` at package init — it panics outside the Encore runtime and breaks plain `go test` for leaf packages. |
| **C3 — "rmcp Go bindings" misnomer** | CONTRADICTED | §6.1 — pinned dependency: `github.com/modelcontextprotocol/go-sdk` v1.6.0 (D-1 decision: v1.6.0 is the latest stable as of phase-01 implementation; v0.5.0 was the original pin during planning). Pinned by Greta in `go.mod` under task D-1. |
| **C4 — Encore Cloud edge-proxy timeout** | PARTIAL | §11.2 — NFR-1 measurement methodology declares "warm cache, local emulator only"; cloud SSE behaviour is a P02 ops item owned by Olive. P01 spec does not target Cloud. |
| **C5 — Recursive CTE `LIMIT 256` semantics** | CONTRADICTED | §6.5 — cycle CTE uses an explicit `depth` counter with `WHERE depth < 256`. The exact CTE is reproduced verbatim from SPEC §9.4.9. |
| **C6 — `GET /mcp/sse` is deprecated transport** | CONTRADICTED | §5 — Streamable HTTP per MCP 2025-06-18: single endpoint at `/mcp` supporting both `POST` (client → server, may stream) and `GET` (server-initiated SSE). No legacy SSE+POST fallback. |
| **C7 — argon2id over 32-byte API key** | CONTRADICTED | §4.3 — API key hash is `HMAC-SHA256(server_secret, raw_key)` stored as `bytea` (32 bytes raw). Lookup by `key_prefix` (UNIQUE), then constant-time HMAC compare. |
| **R-P01-2 — Encore migration runner** | PARTIAL | §3.1 — single-owner pattern (C2 resolution); migrations sequential per Encore convention; bootstrap migration declares both extensions. |
| **R-P01-4 — Free-tier ceilings vs NFR-1** | PARTIAL | §11.2 — NFR-1 measured on local emulator; warm-cache definition pinned (pool established + identity validated, no cold-start). |
| **R-P01-7 — Multi-table FTS** | PARTIAL | §3.4 — `tsvector` `GENERATED` columns on both `workitems.items` and `workitems.comments`; per-table GIN indexes; `search` RPC issues `UNION ALL` over both. |
| **R-P01-9 — GitHub OAuth scopes** | CONFIRMED | §4.1 — `read:user` (and `user:email` if needed); no `repo` scope at v1.0; PKCE S256 mandatory. |
| **AF1 — `workitems` FTS DDL** | NEW | §3.4 — same as R-P01-7. DDL is already in SPEC §9.4.3 (research-applied). |
| **AF2 — `prime`'s "recent cascade events" cap** | NEW | §6.2 / Tool 1 — last 50 rows scoped to org/project; uses existing `cascade_events_org_triggered_idx`. |
| **AF3 — Close precondition without DDL CHECK** | NEW | §6.2 / Tool 6 — MCP-layer precondition `claimed_by_id IS NOT NULL` enforced in handler; structured error envelope on violation. |
| **AF4 — API key lifecycle for v1.0** | NEW | §4.3 — keys default `expires_at = NULL`; rotation = manual "issue new + revoke old"; no auto-rotate; revocation flips `revoked_at`. |
| **AF5 — Cycle-detection write race** | NEW | §6.5 — `pg_advisory_xact_lock(hashtext('deps.add_dependency:' || $project_id))` acquired at transaction start; serialises racing inserts within a project. |
| **OQ1 — Copilot transport coverage** | OPEN | §11.4 — P01 acceptance harness uses Claude Code only; Copilot coverage is P04 plugin renderer scope. |
| **OQ2 — `MEMORY_DEK` provisioning** | OPEN | §3.5 — `MEMORY_DEK` is provisioned in P01 by Olive via Encore secret manager (the bootstrap migration would fail without it because `auth.oauth_tokens.*_enc` columns are exercised by integration tests). Local emulator uses dev DEK from `apps/api/.secrets.local.cue` (CUE format, per Encore docs). |
| **OQ3 — pgcrypto / pg_trgm availability** | OPEN | §3.1 — bootstrap migration `CREATE EXTENSION IF NOT EXISTS` for both; smoke-tested against the local Encore emulator's bundled Postgres in CI before any other migration runs. |
| **OQ4 — `key_hash` column type** | OPEN | §4.3 — `bytea NOT NULL` (32 bytes raw HMAC output). No hex/base32 encoding ambiguity. |

---

## 3. Database Migrations (canonical filenames)

### 3.1 Migration owner and ordering

Per SPEC §5.2 / research C2: **a dedicated `apps/api/db/` service is the
sole migration-owner AND the sole binding authority for every domain
service's database handle**. It is a zero-API Encore service whose only
responsibilities are:

- declare `sqldb.NewDatabase("unblock", ...)` exactly once across the
  workspace,
- ship the canonical migration set under `apps/api/db/migrations/`,
- bind every consumer's nil `*sqldb.Database` handle from its single
  central `init()`.

It owns no business logic, no RPCs, and no domain schema. The directory
`apps/api/db/migrations/` holds the migration set for the entire
`unblock` database.

Every domain service (`auth`, `org`, `workitems`, `deps`, `mcp`,
`providers`, `boards`, `memory`) follows the **canonical BindDB
late-bind consumer pattern** described below; no domain service writes
migration files, and no domain service declares
`sqldb.Named("unblock")` at package init.

Rationale (decoupling): this decouples `auth` from owning DDL for
schemas it does not consume. Every domain service is an equal consumer
of the database, and the migration-owner role lives in a single piece
of infrastructure wiring rather than being grafted onto a domain
service. The historical auth-as-owner pattern (lifted from the C2
closure at spec approval time) accidentally coupled the auth service's
surface to the lifecycle of org / workitems / deps / etc. DDL — bead
`unblock-bne` corrected that. The `var db = sqldb.NewDatabase(...)`
call still happens in exactly one place across the workspace; only its
location changed.

Rationale (consumer pattern, BindDB late-bind): empirical check
against `encore.dev/storage/sqldb` v1.52.1 disproved the assumption
that `sqldb.Named("unblock")` is a benign runtime lookup — its
implementation calls `doPanic` at package-load time exactly like
`sqldb.NewDatabase` (`pkgfn.go:182-192`). Every domain service that
declared `var db = sqldb.Named("unblock")` at package init therefore
panicked any plain `go test ./apps/api/<service>/...` invocation with
"encore apps must be run using the encore command", breaking the
unblock-xuk goal (plain-`go test`-loads-without-panic) for that
service. The BindDB late-bind shape — a nil `*sqldb.Database` pointer
populated by `apps/api/db/db.go`'s `init` — is the only shape that
preserves the xuk goal across every domain service. Bead
`unblock-bne`'s pre-review scope expansion converted the org service
from the eager `sqldb.Named` shape to BindDB and made BindDB the
canonical pattern for every current and future domain service.

Canonical consumer pattern (mandatory for every domain service that
touches the unblock database):

```go
package <service>

import "encore.dev/storage/sqldb"

// db is populated exactly once at process start by
// apps/api/db/db.go's init via <service>.BindDB(DB).
var db *sqldb.Database

// BindDB installs the unblock-database handle. The companion
// apps/api/db package owns the sqldb.NewDatabase call and invokes
// BindDB from its package init.
func BindDB(d *sqldb.Database) { db = d }
```

Then add a `<service>.BindDB(DB)` line to `apps/api/db/db.go`'s
`init()` so the dedicated db service binds the new handle at process
bootstrap. There is no per-service `initbind.go`; the central bind in
`apps/api/db/` is the sole binding authority.

### 3.2 Migration files (locked filenames)

Filename convention: `NNNN_<descr>.up.sql` with `NNNN` strictly increasing
in steps of 10. Step numbering matches §9.4.0 ordering:

| File | Content |
|---|---|
| `0010_bootstrap.up.sql` | `CREATE EXTENSION IF NOT EXISTS pgcrypto;` and `CREATE EXTENSION IF NOT EXISTS pg_trgm;` |
| `0020_auth.up.sql` | Schema `auth` per SPEC §9.4.1 (tables `users`, `oauth_tokens`, `sessions` + indexes) |
| `0030_org.up.sql` | Schema `org` per SPEC §9.4.2 (tables `organizations`, `members`, `projects`, `project_members` + indexes) |
| `0040_workitems.up.sql` | Schema `workitems` per SPEC §9.4.3 (tables `milestones` (recursive, self-referential `parent_milestone_id`, scope-XOR + date-range CHECK constraints; M-INV-2/3/5/6/7 enforced in app code per SPEC §9.4.3 note), `items`, `labels`, `item_labels`, `comments` + all indexes including FTS GIN per AF1) |
| `0050_deps.up.sql` | Schema `deps` per SPEC §9.4.4 (tables `dependencies`, `cycles`, `cascade_events` + indexes; `cascade_events_event_trigger_uniq` for AR-11 idempotency; `cascade_events.kind` column with CHECK `IN ('close','edge_added','edge_removed','state_change')` — see §6.3.0 for the symmetric writer model that introduced the 4-value enum) |
| `0060_providers.up.sql` | Schema `providers` per SPEC §9.4.5 (tables `installations`, `events`, `mappings` + indexes). **Schema-only in P01** — no service code consumes it until P02. |
| `0070_mcp.up.sql` | Schema `mcp` per SPEC §9.4.6 (tables `api_keys`, `tool_calls` + indexes; `key_hash bytea`, `key_prefix UNIQUE` per C7) |
| `0080_boards.up.sql` | Schema `boards` per SPEC §9.4.7 (tables `boards`, `columns` + indexes). **Schema-only in P01** — no service code until P05. |
| `0090_memory.up.sql` | Schema `memory` per SPEC §9.4.8 (tables `entries`, `entry_refs` + indexes). **Schema-only in P01** — no service code until P02. |
| `0120_mcp_issued_to_user_notnull.up.sql` (round-16, bead `unblock-tv8.73`) | Tighten `mcp.api_keys.issued_to_user`: (1) `ALTER TABLE mcp.api_keys ALTER COLUMN issued_to_user SET NOT NULL;` (2) drop the existing FK and re-add it as `FOREIGN KEY (issued_to_user) REFERENCES auth.users(id) ON DELETE CASCADE` (was `ON DELETE SET NULL`). Every MCP API key is now owned by exactly one user; deleting that user deletes their keys. `mcp.tool_calls.api_key_id` (the audit FK) is unaffected and KEEPS its `ON DELETE SET NULL` so audit rows survive a user deletion. Up-only, pre-prod — no rows exist yet, so the `SET NOT NULL` cannot fail on existing data; the migration is additive to the §3.2 sequence and does not renumber 0010..0090. Slot `0110` is already taken by the committed `0110_mcp_warning_codes.up.sql` (bead `unblock-tv8.63`), so this migration takes the next free slot `0120`. |
| `0130_workitems_labels_updated_at.up.sql` (round-16, bead `unblock-tv8.75`) | Add the `updated_at` column to `workitems.labels`: `ALTER TABLE workitems.labels ADD COLUMN updated_at timestamptz NOT NULL DEFAULT now();`. Closes the contradiction between the §4.4 `Label.UpdatedAt` field (which has always declared the column) and the original `0040_workitems.up.sql` DDL (which omitted it). Resolution DECIDED by Miguel 2026-06-11: the label registry is mutable via Tool 22 (`update_label` rename/recolor) and every other long-lived `workitems` row (`items`, `milestones`, `comments`) already carries `updated_at`, so the column is added rather than dropping the struct field. The backing `workitems.UpdateLabel` RPC bumps `updated_at` on every write (§4.4). Up-only, pre-prod — no rows exist yet, so the `NOT NULL DEFAULT now()` cannot fail on existing data; the migration is additive to the §3.2 sequence and does not renumber 0010..0120. Next free slot after the committed `0120` → `0130`. |
| `0140_deps_cascade_events_kind_chk_fix.up.sql` (round-16, bead `unblock-tv8.79`) | Re-assert `deps.cascade_events_kind_chk` with the full 4-kind set, correcting environments that applied `0050_deps.up.sql` BEFORE the round-6 in-place widening (commit `3e0d00d`). The round-6 edit widened the CHECK from 2 kinds to 4 (`'close','edge_added','edge_removed','state_change'`) by editing `0050` in place; golang-migrate keys by version number, so any DB that had already applied `0050` silently retained the stale 2-kind constraint, and `state_change`/`edge_added` cascade audit inserts then fail with SQLSTATE 23514. This NEW up-only forward migration is corrective: `ALTER TABLE deps.cascade_events DROP CONSTRAINT IF EXISTS cascade_events_kind_chk;` then `ALTER TABLE deps.cascade_events ADD CONSTRAINT cascade_events_kind_chk CHECK (kind IN ('close','edge_added','edge_removed','state_change'));`. Idempotent — a no-op-equivalent on fresh DBs (which already carry the 4-kind constraint from the post-edit `0050`) and corrective on stale ones. Roll-forward discipline: `0050` is NOT re-edited. Up-only, pre-prod; additive to the §3.2 sequence and does not renumber 0010..0130. Next free slot after `0130` → `0140`. |

> **Migration `0100`** is the Tool-2 covering index pinned in round-7
> (§6.2.0 / §6.2 Tool 2). It is referenced but its row is documented at
> its point of use; the round-16 `0120` migration follows it in the
> sequence (slot `0110` is held by the committed `0110_mcp_warning_codes`
> migration).

**No `down.sql` files in P01.** Pre-prod (no users, no migration tax per
`feedback_pre_production`). Down migrations re-introduce risk without
benefit at this stage.

### 3.3 Migration content rules

- Files contain DDL only. No data migrations in P01.
- Each file is self-contained: a successful run leaves the schema in the
  exact state declared by the matching SPEC §9.4.X subsection.
- `IF NOT EXISTS` on `CREATE SCHEMA` statements; everything else assumes a
  fresh schema (the migration runner refuses to re-run a file).
- Every `CHECK` and `UNIQUE` constraint receives the named identifier
  declared in SPEC §9.4 (e.g. `comments_kind_chk`, `api_keys_prefix_uniq`).
  These names are part of the contract — error messages reference them.

### 3.4 FTS DDL (AF1 closure)

Both FTS additions ship in `0040_workitems.up.sql`:

```sql
ALTER TABLE workitems.items ADD COLUMN fts tsvector
    GENERATED ALWAYS AS (
        setweight(to_tsvector('english', coalesce(title, '')), 'A') ||
        setweight(to_tsvector('english', coalesce(body,  '')), 'B')
    ) STORED;
CREATE INDEX items_fts_idx ON workitems.items USING GIN (fts);

ALTER TABLE workitems.comments ADD COLUMN fts tsvector
    GENERATED ALWAYS AS (to_tsvector('english', coalesce(body, ''))) STORED;
CREATE INDEX comments_fts_idx ON workitems.comments USING GIN (fts);
```

The `search` MCP tool issues a `UNION ALL` over `items_fts_idx` and
`comments_fts_idx` (PG GIN indexes are per-table per research D10).

### 3.5 Encore secrets required at P01

Owned by Olive, provisioned via the Encore secret manager. Local emulator
reads from a **CUE** file at `apps/api/.secrets.local.cue` (Encore app
root, next to `encore.app`), per Encore official docs
(https://encore.dev/docs/go/primitives/secrets).

> **Coherence (bead `unblock-f6z`).** The non-production placeholder
> values for these secrets live in a single committed source of truth,
> `apps/api/secrets.nonprod.cue`, with the Go field names below as keys.
> CI (`apps-api-ci.yml`) copies it to `.secrets.local.cue` and a
> drift-check gate fails the build if any `secrets.go` field is missing
> from it; `apps/api/scripts/secrets-provision.sh` pushes it to the
> platform `local`+`pr` env types. Real `prod`/`dev` values stay human-set
> on the Encore Platform and are NEVER committed. The split GitHub `CI_*`
> repo-secret registry is retired. Full runbook + the Encore↔GitHub name
> mapping: `apps/api/SECRETS.md`.

> **DRIFT-1 — naming.** The five secret identifiers below
> (`MEMORY_DEK`, `API_KEY_HMAC_SECRET`, `GITHUB_OAUTH_CLIENT_ID`,
> `GITHUB_OAUTH_CLIENT_SECRET`, `GITHUB_OAUTH_REDIRECT_URI`) are **spec-level logical names**, not
> literal manifest field names. The Encore Go secrets manifest declares
> them as Go struct fields in PascalCase, and Encore secret-manager keys
> + CUE-file keys must use those Go field names verbatim.

**Logical-name ↔ Go-field mapping** (binding for both the secret-manager
key and the `.secrets.local.cue` field name):

| Spec-level logical name | Go struct field (manifest key + CUE key) |
|---|---|
| `MEMORY_DEK` | `MemoryDEK` |
| `API_KEY_HMAC_SECRET` | `APIKeyHMACSecret` |
| `GITHUB_OAUTH_CLIENT_ID` | `GitHubOAuthClientID` |
| `GITHUB_OAUTH_CLIENT_SECRET` | `GitHubOAuthClientSecret` |
| `GITHUB_OAUTH_REDIRECT_URI` | `GitHubOAuthRedirectURI` |

The Encore Go secrets manifest (declared once in the `auth` service):

```go
var secrets struct {
    MemoryDEK               string
    APIKeyHMACSecret        string
    GitHubOAuthClientID     string
    GitHubOAuthClientSecret string
    GitHubOAuthRedirectURI  string
}
```

Local override via `apps/api/.secrets.local.cue` uses CUE syntax with the
Go field names verbatim:

```cue
MemoryDEK:               "dev-dek-32-bytes-base64..."
APIKeyHMACSecret:        "dev-hmac-secret..."
GitHubOAuthClientID:     "dev-client-id"
GitHubOAuthClientSecret: "dev-client-secret"
GitHubOAuthRedirectURI:  "http://localhost:4321/auth/callback"
```

| Secret (logical) | Purpose | Used by |
|---|---|---|
| `MEMORY_DEK` | pgcrypto symmetric DEK for `*_enc` columns | `auth` (oauth_tokens encryption tests in P01); fully exercised P02 |
| `API_KEY_HMAC_SECRET` | server-side secret for `HMAC-SHA256(secret, raw_key)` per C7 (Bearer auth) AND `HMAC-SHA256(secret, cursor_payload)` per §6.2.0 (paginated cursor signing — re-uses the same key, no new secret) | `auth` (Bearer auth check on every MCP call; API key issuance); `mcp` (paginated cursor encode/decode per §6.2.0) |
| `GITHUB_OAUTH_CLIENT_ID` | OAuth2+PKCE client id (test app at v1.0) | `auth.ExchangeOAuthCode` |
| `GITHUB_OAUTH_CLIENT_SECRET` | OAuth2+PKCE client secret | `auth.ExchangeOAuthCode` |
| `GITHUB_OAUTH_REDIRECT_URI` | OAuth2+PKCE registered callback URL included in the token-exchange POST body (prevents `redirect_uri_mismatch` once the BFF wires real GitHub) | `auth.ExchangeOAuthCode` |

**Gitignore status (verified 2026-05-08).** The current
`apps/api/.gitignore` ignores `/.encore` and the generated `encore.gen.*`
artefacts but **does not** cover `apps/api/.secrets.local.cue`. Olive
must add an explicit entry (`/.secrets.local.cue`) to `apps/api/.gitignore`
as part of A-2 so the local-override file is never committed. The edit to
`.gitignore` itself is owned by the implementing supervisor (Greta/Olive)
under bead A-2 — this spec only records the requirement.

**P01 exit criterion does not exercise OAuth interactively** — the E2E
test seed (`apps/api/exitcriteriontest/`, round-12) inserts `auth.users`
rows via direct `sqldb.Exec`. The OAuth secrets exist so unit tests that
exercise `auth.ExchangeOAuthCode` against a stubbed provider
have a place to read fixtures from.

### 3.6 JSON wire convention (snake_case lock)

Every exported field of every Go struct in `apps/api/` that may transit
JSON — Encore `//encore:api` request/response types, Pub/Sub payloads,
MCP tool I/O DTOs, error envelopes, and internal helper structs that
may be passed to `encoding/json` — MUST declare an explicit
`json:"snake_case_name"` struct tag. Go's default field-name
serialisation (PascalCase on the wire) is forbidden — explicit tags
are the only sanctioned form.

Rationale: §6.2 (MCP tool wire) and §7 (error envelope) already lock
snake_case for the public agent-facing surface, and
`apps/api/mcp/**` already implements it verbatim. Extending the same
convention to the private Encore RPC surface and Pub/Sub payloads
collapses the project to a single rule, aligns the JSON wire with
Postgres column names (zero impedance between DB rows and the JSON
shape an agent observes), and matches `://unblock`'s agent-native
MCP-first focus. The TypeScript-client cost is absorbed at the
Astro BFF (Zod schemas at the Astro Actions boundary handle the
snake_case ↔ camelCase rename if the frontend wants idiomatic JS
property names).

Exception (third-party HTTP unmarshal): structs that decode a
third-party HTTP response MAY mirror the third-party wire format —
the project does not control that shape. See
`apps/api/auth/oauth.go`'s `githubUserResponse` and
`githubAccessTokenResponse` (coincidentally already snake_case;
unmodified).

JSON-RPC protocol fields (`jsonrpc`, `protocolVersion`,
`structuredContent`, `isError`) follow the MCP 2025-06-18 transport
specification verbatim — they are not under this project's wire
convention.

Query-string serialisation (Encore default snake_case per ENCORE.md)
remains unchanged; the rule above applies to JSON bodies, not URL
query parameters.

Success-side warnings (§7.1, added unblock-tv8.63) are in scope: the
`Warning`/`WithWarnings` Out-struct fields carry explicit snake_case
tags (`json:"code"`, `json:"message"`, `json:"warnings,omitempty"`,
`json:"details,omitempty"`) and any `details` map keys are snake_case
(e.g. `intent_comment_kind`). Routing warnings through a declared
`structuredContent` Out-struct field — rather than the rejected `_meta`
route — is what keeps the `grep -rnE 'json:"[A-Z]' apps/api/` gate
(NFR-10) meaningful: the warning channel is a real tagged Go field the
gate inspects, whereas an untyped `_meta` map would have escaped the
struct-tag gate entirely. The additive audit column
`mcp.tool_calls.warning_codes` (§8.1.1) is snake_case, consistent with
the existing column naming.

Quality gate (NFR-10): `grep -rnE 'json:"[A-Z]' apps/api/` MUST
return zero matches. Cross-references:
[§6.2 (MCP tool wire — snake_case already locked)](#62-tool-by-tool-contracts) /
[§7 (error envelope — snake_case already locked)](#7-error-envelope-locked).

---

## 4. Service Surfaces

### 4.1 `auth` service

Owns: schema `auth` only. The canonical migrations directory lives at
`apps/api/db/migrations/` and is owned by the dedicated `db` service
(§3.1). The auth service consumes the database via the canonical
BindDB late-bind pattern (§3.1) in `apps/api/auth/db.go`: a nil
`*sqldb.Database` pointer plus an exported `BindDB(d *sqldb.Database)`
hook populated by `apps/api/db/db.go`'s `init`.

Public APIs: **none** (the OAuth callback lives on the Astro origin per
PRD FR-12; in P01 it is exercised only by integration tests).

Private RPCs (locked signatures):

```go
package auth

// Identity is the resolved caller record carried inside the Encore mesh.
type Identity struct {
    UserID    string // ULID
    OrgID     string // ULID — primary org binding for this auth event
    Role      string // "owner" | "admin" | "member" | "viewer"
    AgentKind string // empty for human sessions; AgentKind value for API-key callers
}

// Validate accepts an opaque token (session id OR raw API key) and resolves
// it to an Identity. Returns ErrUnauthenticated on miss / revoked / expired.
//
//encore:api private method=POST path=/auth.Validate
func Validate(ctx context.Context, req ValidateRequest) (*ValidateResponse, error)

type ValidateRequest struct {
    Token     string // either auth.sessions.id (browser BFF) or raw API key
    TokenKind string // "session" | "api_key"
}
type ValidateResponse struct {
    Identity Identity
}

// ExchangeOAuthCode is called by the Astro Action /auth/[provider]/callback
// (P05) and by P01 integration tests. Verifies PKCE, exchanges the code for
// a provider access token, upserts auth.users + auth.oauth_tokens, and
// issues a new auth.sessions row. Returns the opaque session id.
//
//encore:api private method=POST path=/auth.ExchangeOAuthCode
func ExchangeOAuthCode(ctx context.Context, req ExchangeOAuthCodeRequest) (*ExchangeOAuthCodeResponse, error)

type ExchangeOAuthCodeRequest struct {
    Provider     string // "github" | "gitlab"
    Code         string
    PKCEVerifier string
    UserAgent    string
    IPAddress    string
}
type ExchangeOAuthCodeResponse struct {
    SessionID string // ULID; opaque; used as Bearer for private RPCs
    UserID    string // ULID
    ExpiresAt time.Time
}

// IssueAPIKey creates a new mcp.api_keys row. In P01 it is called from
// test seeds via direct INSERT (round-12: see §11.1 — the E2E test
// `apps/api/exitcriteriontest/` issues its own key by writing the row
// straight to `mcp.api_keys` with `key_hash` computed via
// `secrets.APIKeyHMACSecret` per `apps/api/auth/apikey.go:103-111`).
// Operator-facing surfaces (CLI or web admin) are deferred to a future
// phase. Returns the raw key ONCE — the caller stores it; subsequent
// reads return only the prefix and metadata.
//
// **Round-16 (bead `unblock-tv8.73`): IssuedToUser is REQUIRED.** Every
// MCP API key is owned by exactly one user — there is no org-level
// "service key" without an owning user. IssueAPIKey rejects an empty
// IssuedToUser with `errs.InvalidArgument` (kind=VALIDATION at the MCP
// boundary, data.field="issued_to_user") BEFORE any INSERT runs. The
// underlying `mcp.api_keys.issued_to_user` column is NOT NULL with an
// `ON DELETE CASCADE` FK to `auth.users(id)` per migration `0120`.
//
// **Tenant gate (bead `unblock-tv8.85`).** This RPC writes a
// `mcp.api_keys` row scoped by `OrgID` + `IssuedToUser` straight from
// its arguments. It is NOT reachable from the MCP agent wire (no MCP
// tool maps to it; only test/seed callers exist today) — but it is a
// LATENT cross-tenant write IDOR once a future key-management BFF /
// web-admin surface is wired: nothing today checks the caller owns
// `OrgID`, nor that `IssuedToUser` is a member of `OrgID`. The gate
// adds a `CallerUserID` channel pinned from the resolved caller identity
// (the future BFF's session→user→org resolution, §4.3.2), NEVER from the
// wire — exactly the §10.1.1 internal-channel convention, NOT a wire
// argument. When `CallerUserID` is non-empty the RPC enforces BOTH:
// (a) the caller owns `OrgID` — via `org.Authorize` keyed on the
// caller's user id (`org.Authorize` does `SELECT role FROM org.members
// WHERE org_id=$1 AND user_id=$2`, §4.2 / `apps/api/org/org.go:520`),
// rejecting a write to a foreign org; and (b) `IssuedToUser` is a member
// of `OrgID` — an `org.members` membership predicate. A foreign `OrgID`
// or a non-member `IssuedToUser` is rejected (`NOT_FOUND` / appropriate
// error), nothing is inserted, existence is not leaked. **Empty
// `CallerUserID` is a NO-OP (dormant gate)** — the trusted §11.1.1 E2E
// seed + integration / mcpaudit / perf tests pass no caller identity (or
// seed `mcp.api_keys` via direct INSERT), so the gate is skipped. The
// gate is therefore DORMANT until the future key-management BFF / admin
// surface pins `CallerUserID`; that future bead MUST pin it (else the
// no-op leaves it open). This mirrors the §10.1.1 empty-`CallerOrgID`
// no-op precedent (the item/milestone write-RPC pattern), adapted to the
// auth/BFF admin surface. NO `mcp.api_keys` schema change is required —
// the gate is an `org.Authorize` call + a membership predicate.
//
//encore:api private method=POST path=/auth.IssueAPIKey
func IssueAPIKey(ctx context.Context, req IssueAPIKeyRequest) (*IssueAPIKeyResponse, error)

type IssueAPIKeyRequest struct {
    OrgID         string // ULID
    IssuedToUser  string // ULID; REQUIRED (round-16 / tv8.73) — empty is rejected with InvalidArgument; there is no nullable org-level service key
    Label         string // human-readable, e.g. "claude-code-laptop"
    AgentKind     string // AgentKind value
    Scopes        []string
    ExpiresAt     *time.Time // nullable; default: never
    CallerUserID  string // ULID; pinned from the resolved caller identity (future BFF session→user→org resolution, §4.3.2); NEVER from the wire. Gate key for org.Authorize ownership (bead unblock-tv8.85). Empty → dormant no-op (trusted §11.1.1 seed / integration / mcpaudit / perf callers).
}
type IssueAPIKeyResponse struct {
    KeyID     string // ULID (mcp.api_keys.id)
    KeyPrefix string // first 8 chars of the raw key
    RawKey    string // FULL raw key — returned ONCE; never persisted in clear
}

// RevokeAPIKey flips revoked_at; idempotent.
//
// **Tenant gate (bead `unblock-tv8.85`).** Pre-this-round the UPDATE was
// `UPDATE mcp.api_keys SET revoked_at=COALESCE(revoked_at,now()) WHERE
// id=$1` with NO caller-org predicate. It is NOT reachable from the MCP
// agent wire (no MCP tool maps to it; only test/seed callers exist
// today) — but it is a LATENT cross-tenant write IDOR once a future
// key-management BFF / web-admin surface is wired: a caller could revoke
// any tenant's key by id. The gate adds a `CallerOrgID` channel pinned
// from the resolved caller identity (the future BFF's session→user→org
// resolution, §4.3.2), NEVER from the wire — exactly the §10.1.1
// internal-channel convention, NOT a wire argument. The UPDATE becomes
// `... WHERE id=$1 AND ($caller='' OR org_id=$caller)`: a cross-tenant
// `KeyID` affects zero rows → `NOT_FOUND` (existence is NOT leaked). The
// `COALESCE` idempotency is preserved (a same-org re-revoke is still a
// no-op success). **Empty `CallerOrgID` is a NO-OP (dormant gate)** —
// the trusted §11.1.1 E2E seed + integration / mcpaudit / perf tests pass
// no caller identity, so the `$caller=''` disjunct skips the predicate.
// The gate is therefore DORMANT until the future key-management BFF /
// admin surface pins `CallerOrgID`; that future bead MUST pin it (else
// the no-op leaves it open). This mirrors the §10.1.1 empty-`CallerOrgID`
// no-op precedent (the item/milestone write-RPC pattern), adapted to the
// auth/BFF admin surface. NO `mcp.api_keys` schema change is required —
// the gate is a query predicate on the existing UPDATE.
//
//encore:api private method=POST path=/auth.RevokeAPIKey
func RevokeAPIKey(ctx context.Context, req RevokeAPIKeyRequest) error

type RevokeAPIKeyRequest struct {
    KeyID       string // ULID
    CallerOrgID string // ULID; pinned from the resolved caller identity (future BFF session→user→org resolution, §4.3.2); NEVER from the wire. Row-level tenant predicate on the UPDATE (bead unblock-tv8.85). Empty → dormant no-op (trusted §11.1.1 seed / integration / mcpaudit / perf callers).
}
```

### 4.2 `org` service

Owns: schema `org` only. Consumes the unblock database via the
canonical BindDB late-bind pattern (§3.1) in `apps/api/org/db.go`:
a nil `*sqldb.Database` pointer plus an exported
`BindDB(d *sqldb.Database)` hook populated by `apps/api/db/db.go`'s
`init`. No per-service `initbind.go`.

Public APIs: **none**.

Private RPCs:

```go
package org

//encore:api private method=POST path=/org.CreateOrganization
func CreateOrganization(ctx context.Context, req CreateOrganizationRequest) (*Organization, error)

// **Tenant gate (bead `unblock-tv8.86`).** This RPC INSERTs a `org.projects`
// row under the wire-supplied `OrgID`. Pre-this-round the only guard was the
// `org_id` FK → `NotFound`, which catches a NON-EXISTENT org but NOT a
// FOREIGN EXISTING one — so a caller could create a project under any other
// tenant's org (a WARNING-class cross-tenant write IDOR). It is NOT reachable
// from the MCP agent wire today (no MCP tool maps to it; only test/seed
// callers exist) — LATENTLY exploitable once a future key-management /
// web-admin BFF is wired. The gate adds a `CallerUserID` channel pinned from
// the resolved caller identity (the future BFF's session→user→org resolution,
// §4.3.2), NEVER from the wire — exactly the §10.1.1 internal-channel
// convention, NOT a wire argument. When `CallerUserID` is non-empty the RPC
// requires the caller to be a write-capable member of `OrgID`
// (`SELECT role FROM org.members WHERE org_id=$1 AND user_id=$2`, §4.2 /
// `apps/api/org/org.go:520`) BEFORE the INSERT; a foreign / non-member
// `OrgID` → `NOT_FOUND` (replacing the FK→`NotFound`, which only catches a
// non-existent org), nothing inserted, existence not leaked. **Empty
// `CallerUserID` is a NO-OP (dormant gate)** — the trusted §11.1.1 seed +
// `org` / `rbactest` / `exitcriteriontest` / `perftest` callers pass no
// caller identity, so the gate is skipped; DORMANT until the future BFF pins
// `CallerUserID`; that future bead MUST pin it (else the no-op leaves it
// open). Same empty-caller no-op precedent as `unblock-tv8.85` / the §10.1.1
// item/milestone write-RPC pattern. NO `org` schema change is required — the
// gate is a membership predicate.
//
//encore:api private method=POST path=/org.CreateProject
func CreateProject(ctx context.Context, req CreateProjectRequest) (*Project, error)

type CreateProjectRequest struct {
    OrgID         string // ULID; the org the project is created under
    Name          string
    Slug          string
    CallerUserID  string // ULID; pinned from the resolved caller identity (future BFF session→user→org resolution, §4.3.2); NEVER from the wire. Gate key for the org.members caller-membership predicate (bead unblock-tv8.86). Empty → dormant no-op (trusted §11.1.1 seed / org / rbactest / exitcriteriontest / perftest callers).
}

//encore:api private method=GET path=/org.GetOrganization/:id
func GetOrganization(ctx context.Context, id string) (*Organization, error)

//encore:api private method=GET path=/org.GetProject/:id
func GetProject(ctx context.Context, id string) (*Project, error)

// **Tenant gate (bead `unblock-tv8.86`) — CRITICAL privilege escalation.**
// This RPC INSERTs an `org.members` row from the wire-supplied `OrgID`,
// `UserID`, and `Role` with ZERO caller-ownership check — `callerIdentity`
// feeds only the `invited_by` audit column, NEVER authorization — and `Role`
// has no cap. So a caller could mint themselves (or anyone) as `owner` of ANY
// existing org: a CRITICAL cross-tenant privilege escalation. It is NOT
// reachable from the MCP agent wire today (no MCP tool maps to it; only
// test/seed callers exist) — LATENTLY exploitable once a future
// key-management / web-admin BFF is wired. The gate adds a `CallerUserID`
// channel pinned from the resolved caller identity (the future BFF's
// session→user→org resolution, §4.3.2), NEVER from the wire — exactly the
// §10.1.1 internal-channel convention, NOT a wire argument. When
// `CallerUserID` is non-empty the RPC enforces BOTH: (a) the caller holds an
// admin/owner `org.members` row in `OrgID`
// (`SELECT role FROM org.members WHERE org_id=$1 AND user_id=$2`, §4.2 /
// `apps/api/org/org.go:520`) BEFORE the INSERT; and (b) the granted `Role` is
// CAPPED at the caller's effective role — a caller cannot grant a role above
// their own. A foreign / non-member `OrgID`, an unauthorised (non-admin)
// caller, or an over-grant → `NOT_FOUND` / appropriate error, nothing
// inserted, existence not leaked. **Empty `CallerUserID` is a NO-OP (dormant
// gate)** — the trusted §11.1.1 seed + `org` / `rbactest` /
// `exitcriteriontest` / `perftest` callers pass no caller identity, so the
// gate is skipped; DORMANT until the future BFF pins `CallerUserID`; that
// future bead MUST pin it (else the no-op leaves the priv-esc open). Same
// empty-caller no-op precedent as `unblock-tv8.85` / the §10.1.1
// item/milestone write-RPC pattern. NO `org` schema change is required — the
// gate is a membership predicate + a role cap.
//
//encore:api private method=POST path=/org.AddMember
func AddMember(ctx context.Context, req AddMemberRequest) error

type AddMemberRequest struct {
    OrgID         string // ULID; the org the member is added to
    UserID        string // ULID; the user being added
    Role          string // role granted — CAPPED at the caller's effective role when CallerUserID is non-empty (bead unblock-tv8.86)
    CallerUserID  string // ULID; pinned from the resolved caller identity (future BFF session→user→org resolution, §4.3.2); NEVER from the wire. Gate key for the org.members caller-admin predicate + role cap (bead unblock-tv8.86); also feeds the invited_by audit column. Empty → dormant no-op (trusted §11.1.1 seed / org / rbactest / exitcriteriontest / perftest callers).
}

// Authorize is the canonical CROSS-SERVICE RBAC predicate. Called by every
// OTHER service before reading or writing a resource it owns. Returns nil on
// permit; ErrForbidden on deny. The org_id of the resource is matched against
// the identity's org_id; cross-tenant calls are rejected here.
//
// NOTE (bead `unblock-tv8.86`): the `org` service's OWN tenant-scoped write
// RPCs (`CreateProject`, `AddMember`) do NOT route through `Authorize` — they
// self-gate via the new `CallerUserID` `org.members` membership predicate
// documented on each RPC above (dormant until the future BFF pins the caller).
// `Authorize` remains the cross-service primitive that OTHER services call;
// it is NOT the gate for `org`'s own provisioning writes. (Bootstrapping
// writes — `CreateOrganization`, where the caller BECOMES the owner — carry
// no membership gate by design and are correctly out of scope.)
//
//encore:api private method=POST path=/org.Authorize
func Authorize(ctx context.Context, req AuthorizeRequest) error

type AuthorizeRequest struct {
    Identity   auth.Identity
    Resource   string // "workitems.items" | "deps.dependencies" | etc.
    Action     string // "read" | "write" | "delete"
    OrgID      string
    ProjectID  string // optional
}
```

> **Future RPC note (bead `unblock-tv8.86`):** `org.project_members` has no
> write RPC yet (seed-only). A future `AddProjectMember`-style RPC will need
> the IDENTICAL caller-membership gate (`CallerUserID` `org.members` /
> `org.project_members` predicate, dormant empty-caller no-op) before it
> INSERTs a project-membership row from the wire — else it reopens the same
> IDOR class on the project-membership surface.

### 4.3 `mcp` service

Owns: schema `mcp` (writes only), the public Streamable HTTP endpoint, the
23 P01 tool handlers (round-16). Consumes the unblock database via the canonical
BindDB late-bind pattern (§3.1) when DB-touching code lands (P01 D-1+):
a nil `*sqldb.Database` pointer + exported `BindDB` hook in
`apps/api/mcp/db.go`, registered in `apps/api/db/db.go`'s `init`. No
per-service `initbind.go`.

#### 4.3.1 Public endpoint (Streamable HTTP per MCP 2025-06-18 spec)

```go
package mcp

// MCPHandler is the single MCP entry point. Both POST and GET hit the same
// handler; HTTP-method dispatch happens inside the function body. Encore's
// raw-endpoint convention is one //encore:api annotation per function
// (https://encore.dev/docs/go/primitives/raw-endpoints): paired
// POST+GET annotations on a single function are NOT supported by the
// Encore parser. P01 uses the elided-method form (no `method=` token)
// so the same handler receives every HTTP method on `/mcp`; this is
// the documented raw-endpoint default (ENCORE.md §raw_endpoints) and
// is functionally equivalent to the conceptual `method=*` wildcard
// (Encore v1.52.1 rejects the literal `method=*` with E1371
// "Invalid endpoint method"). The handler rejects methods other than
// POST/GET with a 405 reply produced via the Go MCP SDK's transport
// adapter.
//
//encore:api public raw path=/mcp
func MCPHandler(w http.ResponseWriter, r *http.Request) {
    switch r.Method {
    case http.MethodPost:
        // delegate to Go MCP SDK Streamable HTTP POST handler
    case http.MethodGet:
        // delegate to Go MCP SDK Streamable HTTP GET handler (server-initiated SSE)
    default:
        w.Header().Set("Allow", "POST, GET")
        http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
    }
}
```

**L7-W2 closure (round-2).** Earlier drafts of this section showed two
`//encore:api public raw` annotations stacked on a single function (one
per HTTP method). Per Encore's documented raw-endpoint syntax that form
is unsupported — the parser binds at most one annotation per function.
The elided-method form (no `method=` token) is the raw-endpoint
default and delegates routing to the function body — functionally
identical to the conceptual `method=*` wildcard but compatible with
the Encore v1.52.1 parser (the literal `method=*` is rejected with
E1371). This matches the MCP 2025-06-18 spec, which intentionally
puts both methods on the same path. (Alternative considered: split into `MCPPostHandler`
+ `MCPGetHandler` with two separate `//encore:api` declarations and let
Encore's path multiplexer route. Rejected because the Go MCP SDK is
designed around a single transport adapter that owns both methods —
splitting forces a session-store seam between the two handlers that
adds nothing.)

Implementation pinned to `github.com/modelcontextprotocol/go-sdk` v1.6.0
(D-1 decision: v1.6.0 is the latest stable as of phase-01 implementation;
v0.5.0 was the original pin during planning. Pinned in `go.mod`; documented
in the PR description and in `apps/api/mcp/transport.go:18-22`). Per research
C3 the dependency is the canonical Go MCP SDK; **`rmcp` (Rust SDK) is not
used in the Go backend.**

Auth: every `POST /mcp` and `GET /mcp` request must carry
`Authorization: Bearer <api-key>`. `Mcp-Session-Id` header is set on the
`initialize` response and echoed by the client on subsequent requests.

#### 4.3.2 API key Bearer auth hot path (C7 closure)

On every MCP request:

1. Parse `Authorization: Bearer <raw_key>`.
2. Extract `key_prefix = base32_portion[:8]` (i.e. `raw_key[12:20]` for inputs of the form `unblock_pat_<base32>`). *key_prefix is the first 8 chars of the random base32 portion, **after** stripping the literal `unblock_pat_` (12 chars) — see the locked key-format note below.*
3. `SELECT id, org_id, issued_to_user, key_hash, agent_kind, revoked_at, expires_at FROM mcp.api_keys WHERE key_prefix = $1` (uses `api_keys_prefix_uniq` UNIQUE index — O(1) lookup). `issued_to_user` is selected here so step 8 can populate `Identity.UserID` without a second round-trip (matches `apps/api/auth/auth.go` `validateAPIKey`).
4. Reject if `revoked_at IS NOT NULL` or `expires_at IS NOT NULL AND expires_at < now()`.
5. Compute `expected = HMAC-SHA256(API_KEY_HMAC_SECRET, raw_key)`.
6. `if !subtle.ConstantTimeCompare(stored_key_hash, expected) { reject }`.
7. `UPDATE mcp.api_keys SET last_used_at = now() WHERE id = $1` (fire-and-forget).
8. Construct `auth.Identity{UserID: issued_to_user, OrgID: org_id, Role: "agent", AgentKind: agent_kind}` and inject via Encore's auth handler. **Round-16 (bead `unblock-tv8.73`):** `issued_to_user` is NOT NULL on every `mcp.api_keys` row (migration `0120`), so this path NEVER constructs an empty-UID identity. If a row with an empty `issued_to_user` were somehow observed (it cannot be, given the NOT NULL constraint), the request is rejected as `UNAUTHENTICATED` rather than yielding an unscoped identity — defence in depth against a malformed row.

Total budget for this path: <5 ms p99 on warm cache (no Argon2 cost per C7).

**Caller identity for the BFF admin write surface (bead `unblock-tv8.85`).**
The hot path above resolves the caller for the **MCP agent wire** from a
Bearer **API key** (`auth.IssueAPIKey` / `auth.RevokeAPIKey` are NOT on this wire —
no MCP tool maps to them). The auth/BFF admin RPCs that write `mcp.api_keys`
(`auth.IssueAPIKey`'s `CallerUserID`, `auth.RevokeAPIKey`'s `CallerOrgID`,
§4.1) take their caller identity from a DISTINCT channel: the FUTURE
key-management BFF / web-admin surface resolves a browser **session**
(`auth.Validate` with `TokenKind="session"` → `Identity{UserID, OrgID}`,
§4.1) and pins `CallerUserID` / `CallerOrgID` from that resolved identity —
NEVER from the wire. This is the same internal-channel convention as §10.1.1
(`CallerOrgID` pinned from `identity.OrgID`), adapted to the BFF session
path. Until that surface exists the pinned fields are empty and the §4.1 /
§10.1.1 tenant gates are DORMANT (empty-caller no-op).

**Raw key format (locked):** `unblock_pat_<base32-32-byte>` — 12-char fixed
prefix + 32 bytes of crypto/rand encoded as base32 (no padding, lowercase),
total 64 characters. The first 8 chars of the encoded portion populate
`key_prefix` (the literal `unblock_pat_` prefix is stripped before
prefixing — `key_prefix` is over the random portion only). This keeps the
prefix UNIQUE across the entire key space without colliding on the literal
brand prefix.

#### 4.3.3 Auth handler

```go
// AuthParams is the structured auth-handler input. The simple-token form
// (`func AuthHandler(ctx, token string)`) cannot read request headers, and
// this handler MUST inspect `X-Unblock-BFF-Origin` to dispatch between the
// MCP API-key path and the BFF session path. Per Encore's documented
// auth-handler contract (ENCORE.md lines 388-398) the structured-params
// form is the only variant that exposes incoming headers via `header:"…"`
// struct tags, so it is required here.
type AuthParams struct {
    Authorization string `header:"Authorization"`
    BFFOrigin     string `header:"X-Unblock-BFF-Origin"`
}

//encore:authhandler
func AuthHandler(ctx context.Context, p *AuthParams) (auth.UID, *AuthData, error) {
    // Parse Bearer from p.Authorization, dispatch on p.BFFOrigin presence:
    //   - p.BFFOrigin == ""  → MCP path: raw API key (handled by §4.3.2 above)
    //   - p.BFFOrigin != ""  → BFF path: session_id (handled by auth.Validate(TokenKind="session"))
}

type AuthData struct {
    Identity auth.Identity
}
```

> **P01 contract (session-path deferral).** `Validate(TokenKind="session", token=...)` returns `errs.Unimplemented` in P01. The session token path is exercised only by the future BFF (Astro Actions) and the multi-org disambiguation rule (because `auth.sessions` has no `org_id` column and `Identity.OrgID` would require a lookup in `org.members` that is undefined when a user belongs to multiple orgs) will be defined as part of the BFF phase. The E2E test seed bypasses OAuth (§3.5, round-12 — direct `sqldb.Exec` writes to `auth.users` mirror the rbactest pattern) and the MCP transport (D-1) authenticates via API key only, so this deferral does not affect any P01 acceptance criterion.

### 4.4 `workitems` service

Owns: schema `workitems` only. Consumes the unblock database via the
canonical BindDB late-bind pattern (§3.1) in `apps/api/workitems/db.go`:
a nil `*sqldb.Database` pointer + exported `BindDB` hook, registered
in `apps/api/db/db.go`'s `init`. The skeleton hook is pre-wired in
P01 A-1; RPC bodies start reading `db` in beads B-1+ when DB-touching
code lands (bodies, FTS, milestones, claim transaction, state-machine
invariants). No per-service `initbind.go`.

Private RPCs (called by MCP tool handlers; never directly by clients):

```go
package workitems

//encore:api private method=POST path=/workitems.Create
// Creates a work item in status=Backlog.
//
// **is_ready-on-create (round-16, bead unblock-tv8.71).** Create sets
// `is_ready` INLINE inside its own transaction (it is a Regime A is_ready
// writer per §6.3.0 / §11.3). A new item with no incoming `blocks` edges
// gets `is_ready=true`. **Do NOT** rely on the cascade subscriber to
// materialise readiness for a newly-created item: the subscriber maintains
// `pipeline_stage` only (round-6 §6.3.0) and `recomputeReady` never fires
// for a create with no edge mutation — without the inline write the item
// would be stranded non-ready. (This corrects the pre-round-16 doc-comment
// that implied readiness was subscriber-materialised on create.) `status`
// stays `Backlog`; an unblocked item is then `promote`-able via Tool 15.
//
// **Create-path cross-reference tenant validation (round-16, bead
// unblock-tv8.78).** Create stamps `org_id` from `req.OrgID` (itself pinned
// from `identity.OrgID` by the MCP handler) but ALSO validates every
// wire-supplied cross-reference against that caller org BEFORE / at the
// INSERT, inside the existing single transaction (the bead-`unblock-tv8.17`
// atomicity contract). Without this, the INSERT's FK constraints only check
// reference EXISTENCE (in ANY org), so a caller could name another org's
// `project_id` / `parent_id` / `milestone_id` / `discovered_from_id` /
// label and produce an item whose `org_id` differs from the referenced
// row's org — the create-path analogue of the §10.1.1 write-by-id IDOR
// seam. Per-reference predicates (a foreign-but-existing id yields the SAME
// `NOT_FOUND` envelope as a missing id, never a "belongs to another org"
// message):
//   - project_id        → project_id IN (SELECT id FROM org.projects WHERE org_id = $caller)
//   - parent_id         → parent_id IN (SELECT id FROM workitems.items WHERE org_id = $caller)
//   - discovered_from_id→ same as parent_id (a caller-org item)
//   - milestone_id      → org_id = $caller OR project_id IN (SELECT id FROM org.projects WHERE org_id = $caller) (org-XOR-project, AssignItem precedent)
//   - labels[]          → every label_id is org-scoped to $caller OR project-scoped to a project in $caller; a foreign label_id attaches nothing
// **Gate-key framing (DECISION, Miguel 2026-06-12):** Create reuses its
// existing `req.OrgID` as the gate key — it does NOT introduce a separate
// `CallerOrgID` field and does NOT take the empty-`OrgID` no-op branch the
// §10.1.1 update/delete-by-id RPCs use, because Create's internal callers
// all pass a real same-org `OrgID` (already validated non-empty). This is a
// deliberate divergence from the .77 separate-`CallerOrgID` convention;
// coverage is identical (the create path keys on the already-trusted
// `req.OrgID`). The `dependencies[]` path is unchanged — it remains gated
// by `deps.AddEdgeInTx`'s own `CallerOrgID` endpoint check (§10.1.1).
func Create(ctx context.Context, req CreateRequest) (*Item, error)

// CreateRequest is the workitems.Create RPC input. The `dependencies[]`
// element type is `deps.Edge` (NOT a local `workitems.Edge`); the
// skeleton-time local struct was removed in C-1 (bead unblock-tv8.10).
// `deps.Edge` carries `{ID, FromItem, ToItem, Kind, CreatedAt, CreatedBy}`
// (§4.5) — the create path populates only `FromItem` (the blocker) and
// `Kind`; `ToItem` is the newly-created item, stamped by Create. The MCP
// wire shape is DIFFERENT (see `DependencyEdge` below): the Tool 4 handler
// (`apps/api/mcp/handler_create.go`) maps each JSON `{blocker_item_id,
// kind}` entry to a `deps.Edge{FromItem: blocker_item_id, Kind: kind}`.
type CreateRequest struct {
    OrgID            string   // pinned from identity.OrgID by the MCP handler;
                              // ALSO the create-path gate key for every
                              // cross-reference (round-16, bead unblock-tv8.78)
    ProjectID        string
    ParentID         string   // optional epic id; required for type=finding
    DiscoveredFromID string   // required for type=finding
    Type             string   // "epic" | "task" | "finding"
    Title            string   // 1..200 chars
    Body             string   // optional, default ""
    Priority         string   // "P0".."P4"; default "P3"
    MilestoneID      string   // optional
    Labels           []string // label IDs to attach
    Dependencies    []deps.Edge // optional; cycle-checked atomically with the create.
                                // Element type is deps.Edge (§4.5) — only FromItem +
                                // Kind are read here; ToItem is the new item (review L15-W2).
    Severity        string   // required when Type="finding"
    KindOfFinding   string   // "review" | "qa"; required when Type="finding"
}

// DependencyEdge is the JSON WIRE shape of one `dependencies[]` entry on the
// Tool 4 (`create`) MCP surface (§6.2). It lives in the MCP handler
// (`apps/api/mcp/handler_create.go` as `createDependencyIn`), NOT inside
// `workitems.CreateRequest`: the handler maps it to a `deps.Edge` before
// calling `workitems.Create`. It is documented here alongside CreateRequest
// purely for wire-contract reference.
type DependencyEdge struct {
    BlockerItemID string // wire `blocker_item_id` → deps.Edge.FromItem: must complete first
    Kind          string // "blocks" | "related"; default "blocks"
}

type Item struct {
    ID                  string
    OrgID               string
    ProjectID           string
    MilestoneID         string
    ParentID            string
    DiscoveredFromID    string
    Type                string
    Title               string
    Body                string
    Status              string // §6.1
    Priority            string // §6.1
    PipelineStage       string // §6.1; subscriber-maintained per SPEC §5.7.1
    AgentKind           string
    ImplState           string // "pending" | "done"
    ReviewState         string // "pending" | "approved" | "needs_rework"
    QAState             string // "pending" | "passed" | "failed"
    PipelineState       string // "running" | "needs_human" | "paused" | "no_investigation"
    Severity            string
    KindOfFinding       string
    ClaimedByID         string
    ClaimedByAgent      string
    ClaimedAt           *time.Time
    IsReady             bool
    MilestoneAssignedAt *time.Time
    MilestoneAssignedBy string
    Labels              []string  // label IDs from workitems.labels (review L3-W6)
    CreatedAt           time.Time
    UpdatedAt           time.Time
    ClosedAt            *time.Time
}

//encore:api private method=POST path=/workitems.Update
// Tenant gate (round-16 / bead unblock-tv8.77): self-gates the UPDATE on
// org_id = CallerOrgID (CallerOrgID pinned from identity.OrgID by the MCP
// handler, NEVER from the wire). A foreign ItemID yields NOT_FOUND, never
// a cross-tenant mutation. Empty CallerOrgID is the §10.1 no-op
// ($caller = '' OR org_id = $caller) for trusted internal callers; the
// MCP handler always pins it, so the no-op is unreachable from agents.
//
// Milestone-write gate (bead unblock-tv8.84): the wire-supplied MilestoneID,
// when non-empty, is ALSO gated — it must belong to the caller's org via the
// org-XOR-project milestone predicate (org_id = CallerOrgID OR project_id IN
// the caller's projects), mirroring AssignItem / Create. A foreign-but-existing
// MilestoneID yields NOT_FOUND with the item UNCHANGED (zero affected rows),
// indistinguishable from a missing milestone. The clear-to-null path
// (MilestoneID = "") and the nil = unchanged path carry no milestone predicate
// and are preserved. This closes the residual cross-tenant write IDOR on the
// milestone_id selector that bead unblock-tv8.83's AC4 wrongly assumed gated.
func Update(ctx context.Context, req UpdateRequest) (*Item, error)

type UpdateRequest struct {
    ItemID      string
    CallerOrgID string // pinned from identity.OrgID; NEVER from the wire (§10.1)
    Title       *string
    Body        *string
    Priority    *string
    MilestoneID *string
    Labels      *[]string // nil = no change; pointer to slice = full replace
}

//encore:api private method=GET path=/workitems.Get/:id
func Get(ctx context.Context, id string) (*Item, error)

//encore:api private method=POST path=/workitems.GetTrail
func GetTrail(ctx context.Context, req GetTrailRequest) (*Trail, error)

type GetTrailRequest struct {
    ItemID string
}
type Trail struct {
    Item              *Item
    Parent            *ResolvedRef     // round-16 / tv8.76: resolved parent {id,title,status}; nil when no parent
    Comments          []Comment        // ordered by created_at asc
    DependenciesIn    []ResolvedRef    // round-16 / tv8.76: edges where to_item == Item.ID, target resolved to {id,title,status,kind}
    DependenciesOut   []ResolvedRef    // round-16 / tv8.76: edges where from_item == Item.ID, target resolved to {id,title,status,kind}
    Findings          []Item           // children with type=finding
}

// ResolvedRef is a one-level-deep resolution of a related item (parent or
// dependency target) to its identity + display fields. Bounded by design:
// no body, no comments, no nested neighbours (round-16 / tv8.76).
type ResolvedRef struct {
    ID     string // target item ULID
    Title  string // target item title
    Status string // target item Status enum (§6.1)
    Kind   string // edge kind ("blocks" | "related"); empty for the parent ref
}

//encore:api private method=POST path=/workitems.AppendComment
// Tenant gate (round-16 / bead unblock-tv8.77): being an INSERT, gates via
// INSERT … SELECT predicated on the target item's tenancy — the comment row
// is inserted only when ItemID resolves to an item with
// org_id = CallerOrgID (pinned from identity.OrgID, NEVER from the wire). A
// foreign ItemID inserts zero rows → NOT_FOUND, never a cross-tenant
// comment. Empty CallerOrgID is the §10.1 no-op for trusted internal
// callers; the MCP handler always pins it.
//
// Threading scope (bead unblock-tv8.80, LOCKED by Miguel 2026-06-12): when
// ParentID is non-empty, the same INSERT … SELECT additionally requires the
// parent comment to live on the SAME item — parent_id IN (SELECT id FROM
// workitems.comments WHERE item_id = $target_item). A foreign-org or
// cross-item ParentID inserts zero rows → NOT_FOUND, indistinguishable from
// a missing parent. The target item is already CallerOrgID-gated, so
// same-item transitively implies same-org (no separate parent-org branch).
// Empty ParentID is the top-level-comment path; the self-parent prohibition
// (comments_no_self_parent_chk) is preserved. This closes the parent_id
// cross-tenant/cross-item IDOR (§10.1.1, §6.2 Tool 10).
func AppendComment(ctx context.Context, req AppendCommentRequest) (*Comment, error)

type AppendCommentRequest struct {
    ItemID       string
    CallerOrgID  string // pinned from identity.OrgID; NEVER from the wire (§10.1)
    AuthorID     string // user id; nullable if AuthorAgent set
    AuthorAgent  string // AgentKind value; nullable if AuthorID set
    ParentID     string // optional; thread parent
    Kind         string // PRD §6.5 / SPEC §9.4.3 comments_kind_chk:
                        // investigation | decision | deviation | completed |
                        // review | qa | deferred | pr | needs-human |
                        // override | general
    Status       string // "error" | "warning" | "info" | "success"
    Body         string
}

type Comment struct {
    ID          string
    ItemID      string
    ParentID    string
    AuthorID    string
    AuthorAgent string
    Kind        string
    Status      string
    Body        string
    CreatedAt   time.Time
    UpdatedAt   time.Time
}

//encore:api private method=POST path=/workitems.SetStateColumns
// Writes one or more of (impl_state, review_state, qa_state, pipeline_state)
// + recomputes pipeline_stage via the cascade subscriber path.
//
// **Tenant gate (round-16 / bead unblock-tv8.77):** self-gates the
// SELECT … FOR UPDATE row lock on org_id = CallerOrgID (pinned from
// identity.OrgID, NEVER from the wire). A foreign ItemID yields NOT_FOUND
// BEFORE any invariant check runs, never a cross-tenant state mutation.
// Empty CallerOrgID is the §10.1 no-op for trusted internal callers; the
// MCP handler always pins it.
//
// **P01 enforces:**
//  - structural invariants (e.g. impl_state=done requires claimed_by_id IS NOT NULL);
//  - the **five PRD §6.2 state-machine invariants** (round-2 D2 — see
//    §6.2 Tool 13 for the canonical table). These are pure column-value
//    rules with no comment-trail dependency, so they ship in P01.
//
// Layer-1 BLOCK conditions (comment-trail-driven preconditions) are P02
// (Plan §3.4); they layer on top of the five invariants below.
//
// All five invariants are enforced inside ONE Postgres transaction. The
// implementation uses a CTE / SELECT ... FOR UPDATE / UPDATE chain in a
// single SQL round-trip (preferred over PL/pgSQL for readability — the
// invariants are independent column-value checks, not iterative). The
// CTE shape is documented in §6.2 Tool 13.
func SetStateColumns(ctx context.Context, req SetStateRequest) (*Item, error)

type SetStateRequest struct {
    ItemID        string
    CallerOrgID   string // pinned from identity.OrgID; NEVER from the wire (§10.1)
    ImplState     *string
    ReviewState   *string
    QAState       *string
    PipelineState *string
    // The MCP layer attaches a (kind, status, body) comment trail entry
    // when the agent calls set_state with an intent_comment field.
    // workitems.SetStateColumns DOES NOT write comments — that is
    // AppendComment's job. The MCP tool handler composes the two RPCs
    // sequentially, NOT in one transaction: SetStateColumns commits
    // first, then AppendComment runs best-effort (orchestrator DECISION
    // 2026-05-18 on bead unblock-tv8.21 — cross-RPC Postgres
    // transactions are out of architectural scope for P01). An
    // AppendComment failure does NOT roll back the committed state
    // mutation; it surfaces as a non-fatal warning on the SUCCESS
    // result (code=intent_comment_dropped) per §6.2 Tool 13 and the §7
    // success-side warnings contract.
}

//encore:api private method=POST path=/workitems.Close
// MCP-layer precondition (AF3): rejects if claimed_by_id IS NULL.
// Sets status=Done, closed_at=now(), emits deps.cascade.requested.
// Tenant gate (round-16 / bead unblock-tv8.77): self-gates the status-flip
// UPDATE on org_id = CallerOrgID (pinned from identity.OrgID, NEVER from
// the wire), checked BEFORE the AF3 claimed_by_id precondition. A foreign
// ItemID yields NOT_FOUND, never a cross-tenant close. Empty CallerOrgID is
// the §10.1 no-op for trusted internal callers; the MCP handler always
// pins it.
func Close(ctx context.Context, req CloseRequest) (*Item, error)

type CloseRequest struct {
    ItemID      string
    CallerOrgID string // pinned from identity.OrgID; NEVER from the wire (§10.1)
    Reason      string // optional free-text recorded as a kind=completed comment
}

//encore:api private method=POST path=/workitems.Claim
// Atomic claim per SPEC §5.5. Runs the SELECT FOR UPDATE transaction.
// Returns the loser-side ErrAlreadyClaimed with claimed_by_id and
// claimed_at populated.
//
// **Tenant gate (round-16 / bead unblock-tv8.77):** self-gates the
// SELECT … FOR UPDATE row lock on org_id = CallerOrgID (pinned from
// identity.OrgID, NEVER from the wire). A foreign ItemID yields NOT_FOUND
// (not ALREADY_CLAIMED), never a cross-tenant claim. Empty CallerOrgID is
// the §10.1 no-op for trusted internal callers; the MCP handler always
// pins it.
//
// **PRD §6.2 invariant #3 (round-2 D2).** When the item being claimed
// has `qa_state='failed'` at the moment the row is locked, this RPC
// resets `review_state='pending'` AND `qa_state='pending'` atomically
// inside the same transaction (callers MUST NOT expect the failed
// states to persist across a re-claim). The reset is the structural
// implementation of the "next supervisor claim after qa_state=failed"
// rule. AR-18 (round-2) discusses the concurrency interaction with
// SetStateColumns racing the same item.
func Claim(ctx context.Context, req ClaimRequest) (*Item, error)

type ClaimRequest struct {
    ItemID         string
    CallerOrgID    string // pinned from identity.OrgID; NEVER from the wire (§10.1)
    ClaimerUserID  string
    ClaimerAgent   string // AgentKind value
}

//encore:api private method=POST path=/workitems.List
func List(ctx context.Context, req ListRequest) (*ListResponse, error)

type ListRequest struct {
    OrgID        string
    ProjectID    string
    MilestoneID  string
    Status       []string // any of "Backlog","Ready","InProgress","Blocked","Done"
    PipelineStage []string
    ClaimedBy    string
    Labels       []string
    Limit        int    // 1..200; default 50
    Cursor       string // opaque pagination cursor
}
type ListResponse struct {
    Items      []Item
    NextCursor string
}

//encore:api private method=POST path=/workitems.Search
// Multi-table FTS per AF1: UNION ALL over items_fts_idx and comments_fts_idx.
func Search(ctx context.Context, req SearchRequest) (*SearchResponse, error)

type SearchRequest struct {
    OrgID     string
    ProjectID string
    Query     string // websearch_to_tsquery format
    Limit     int    // 1..100; default 25
    // Cursor anchor for keyset pagination (§6.2.0). All three Cursor*
    // fields are populated together; the typed tuple mirrors the Ready
    // RPC pattern (separate typed cursor fields rather than an opaque
    // blob — Encore's wire format carries them transparently). When
    // CursorItemID is "" the server returns the first page.
    CursorRank      float64
    CursorItemID    string
    CursorCommentID string
}
type SearchResponse struct {
    Hits []SearchHit
    // NextCursor* carries the keyset anchor of the row that would
    // START the next page on the canonical FTS sort tuple
    // (rank desc, item_id asc, comment_id asc). All three NextCursor*
    // fields are populated together when more rows exist; all three
    // are zero values when this is the final page. The handler
    // over-fetches LIMIT+1 to detect end-of-stream — same pattern as
    // the Ready RPC.
    NextCursorRank      float64
    NextCursorItemID    string
    NextCursorCommentID string
}
type SearchHit struct {
    ItemID    string
    Source    string // "item" | "comment"
    CommentID string // populated when Source="comment"
    Rank      float64
    Snippet   string // ts_headline output, ≤ 200 chars
}

// --- Label-registry RPCs (round-16, bead unblock-tv8.75) ---
// Back the label MCP tools (§6.2 Tools 20–23) over the EXISTING
// workitems.labels / workitems.item_labels tables (SPEC §9.4.3). One new
// up-only migration 0130_workitems_labels_updated_at.up.sql (§3.2) adds
// the updated_at column declared by the Label struct below (the original
// 0040_workitems.up.sql DDL omitted it; drift DECIDED by Miguel
// 2026-06-11 — ADD the column). UpdateLabel bumps updated_at on every
// write. Org scoping follows the Bearer-Identity pattern (§6.2 closing
// note): the write RPCs (CreateLabel / UpdateLabel / DeleteLabel) trust
// the org-scoped Identity pinned by the MCP handler (identity.OrgID,
// passed RPC-side as CallerOrgID — internal channel, never wire) and
// do NOT call org.Authorize. CreateLabel self-gates the project-scoped
// insert on CallerOrgID and hard-rejects an empty CallerOrgID with
// InvalidArgument (round-16 / bead unblock-tv8.77 — MCP-only callers, so
// the no-op branch is wrong here; consistent with UpdateLabel /
// DeleteLabel). UpdateLabel / DeleteLabel further apply a row-level
// tenant predicate so a foreign LabelID is NOT_FOUND, never a
// cross-tenant mutation. The read RPC (ListLabels) does NOT use rbac.For
// (the project-wins-on-identical-name UNION ALL is not expressible via
// rbac.For); it gates via an EXPLICIT tenant predicate in raw SQL
// (org_id = identity.OrgID) — same justified deviation as MilestoneTree
// (§4.4.1 / §6.2 Tool 19). See the auth-model doc-comment at
// apps/api/workitems/workitems.go:28-66.

//encore:api private method=POST path=/workitems.CreateLabel
func CreateLabel(ctx context.Context, req CreateLabelRequest) (*Label, error)

type CreateLabelRequest struct {
    // OrgID is NOT a wire argument. The MCP handler pins it from the
    // Bearer-resolved org-scoped Identity (identity.OrgID) and passes it
    // RPC-side — exactly like CreateMilestoneRequest (§4.4.1). ProjectID
    // is the XOR selector: empty → org-scoped (to identity.OrgID);
    // non-empty → project-scoped (the handler/RPC validates the project
    // belongs to identity.OrgID). DB CHECK labels_scope_xor_chk is the
    // last line of defence.
    //
    // CallerOrgID is ALSO pinned from identity.OrgID (never wire). On the
    // project-scoped branch CreateLabel gates the insert on
    // project_id IN (SELECT id FROM org.projects WHERE org_id = CallerOrgID)
    // so a Bearer for org A cannot create a label inside org B's project.
    // An empty CallerOrgID is HARD-REJECTED with InvalidArgument (round-16
    // / bead unblock-tv8.77 — MCP-only callers, so the §10.1 no-op branch
    // is wrong here; consistent with UpdateLabel / DeleteLabel).
    OrgID       string // populated from identity.OrgID; NEVER from the wire; XOR ProjectID
    ProjectID   string // ULID; optional wire arg; XOR OrgID
    CallerOrgID string // populated from identity.OrgID; NEVER from the wire; empty → InvalidArgument
    Name        string // 1..64 chars; unique within scope
    Color       string // "#RRGGBB"
    Description  string // optional
}

type Label struct {
    ID          string
    OrgID       string // empty when project-scoped
    ProjectID   string // empty when org-scoped
    Name        string
    Color       string
    Description  string
    CreatedAt   time.Time
    UpdatedAt   time.Time
}

//encore:api private method=POST path=/workitems.ListLabels
// Returns labels in scope; project-scope calls also return inherited org
// labels with PRD §6.4 "project wins on identical name" applied.
func ListLabels(ctx context.Context, req ListLabelsRequest) (*ListLabelsResponse, error)

type ListLabelsRequest struct {
    // OrgID is NOT a wire argument. The read RPC gates via an EXPLICIT
    // tenant predicate in raw SQL (org_id = identity.OrgID) — NOT rbac.For,
    // since the project-wins-on-identical-name UNION ALL is not expressible
    // via rbac.For (same justified deviation as MilestoneTree). The MCP
    // handler resolves the caller from the Bearer-resolved Identity. Org
    // scope is therefore always the caller's org — never wire-supplied
    // (mirrors the milestone read RPC prose, §4.4.1 / §6.2 Tool 19).
    OrgID     string // populated from identity.OrgID; NEVER from the wire
    ProjectID string // optional wire arg; when set, returns project + inherited org labels
}
type ListLabelsResponse struct {
    Labels []Label
}

//encore:api private method=POST path=/workitems.UpdateLabel
// Renames and/or recolors. Scope (OrgID/ProjectID) is immutable. Applies
// a row-level tenant predicate: the targeted label's org_id =
// identity.OrgID OR its project_id belongs to a project in the caller's
// org — a foreign LabelID yields NOT_FOUND, never a cross-tenant write.
// Bumps workitems.labels.updated_at (the column added by migration 0130,
// §3.2) to now() on every successful write; the returned Label carries
// the new UpdatedAt.
func UpdateLabel(ctx context.Context, req UpdateLabelRequest) (*Label, error)

type UpdateLabelRequest struct {
    LabelID     string
    Name        *string // rename
    Color       *string // recolor "#RRGGBB"
    Description  *string
}

//encore:api private method=POST path=/workitems.DeleteLabel
// Deletes the label; the workitems.item_labels junction rows cascade
// (ON DELETE CASCADE per SPEC §9.4.3). Items are not deleted. Applies a
// row-level tenant predicate: the targeted label's org_id =
// identity.OrgID OR its project_id belongs to a project in the caller's
// org — a foreign LabelID yields NOT_FOUND, never a cross-tenant delete.
func DeleteLabel(ctx context.Context, req DeleteLabelRequest) (*DeleteLabelResponse, error)

type DeleteLabelRequest struct {
    LabelID string
}
type DeleteLabelResponse struct {
    Deleted           bool
    LabelID           string
    DetachedItemCount int // number of item_labels rows removed by the cascade
}
```

#### 4.4.1 Milestone RPCs (round-2 D1)

Milestones (PRD §6.3 + SPEC §9.4.3) ship in P01. **Round-16 (bead
`unblock-tv8.74`) OVERRIDES the original round-2 D1 deferral:** these
private RPCs are now also exposed agent-facing as MCP **Tools 16–19**
(`create_milestone` / `update_milestone` / `assign_item` /
`milestone_tree`, §6.2) — thin MCP facades over the RPCs below. Only the
4 memory tools remain deferred to P02 (see §1 overview / round-16
amendment). P01 consumers of these RPCs: the MCP tool handlers (Tools
16–19, §6.2) AND the E2E exit-criterion test
(`apps/api/exitcriteriontest/` — see §11.1, round-12) drives them from
its `TestMain` through Encore's private mesh to assert the
milestone-tree shape; the future Astro client (P05) calls them too.

```go
package workitems

//encore:api private method=POST path=/workitems.CreateMilestone
// Creates a milestone scoped to org_id XOR project_id. Enforces:
//  - M-INV-1 (no self-loop)         — DB CHECK milestones_no_self_loop_chk
//  - M-INV-2 (no parent-chain cycle) — recursive CTE walks ancestors of
//    parent_milestone_id; rejects with kind=PRECONDITION_NOT_MET if the
//    new id appears in the ancestor set
//  - M-INV-3 (child date range ⊆ parent date range) — when
//    parent_milestone_id is non-null, fetch parent (start_date, end_date)
//    and reject if (start_date < parent.start_date OR end_date > parent.end_date)
//  - M-INV-5 (child scope matches parent scope) — when parent_milestone_id
//    is non-null, the new row's (org_id, project_id) must match the parent's
//  - M-INV-6 (max depth = 4) — same recursive CTE depth-counts ancestors
//    and rejects when depth would exceed 4
//  - DB CHECKs milestones_scope_xor_chk and milestones_date_range_chk
//    fire as the last line of defence
// M-INV-7 is enforced lazily on AssignItem (see below).
//
// Tenant gate (round-16 / bead unblock-tv8.77): the parent-read seam
// self-gates on CallerOrgID (pinned from identity.OrgID, NEVER from the
// wire). When ParentMilestoneID is supplied, the parent row used for the
// M-INV-2/3/5/6 ancestor + date + scope checks must satisfy
// org_id = CallerOrgID OR project_id IN (SELECT id FROM org.projects
// WHERE org_id = CallerOrgID); a foreign parent ULID yields NOT_FOUND,
// never a cross-tenant read leak. Empty CallerOrgID is the §10.1 no-op
// for trusted internal callers (the §11.1.1 E2E seed); the MCP handler
// always pins it.
//
// Tenant gate (bead unblock-tv8.83): the project-scoped branch's
// ProjectID is gated via a guarded INSERT … SELECT — when project-scoped
// (ProjectID non-empty), the milestone INSERT requires
// project_id IN (SELECT id FROM org.projects WHERE org_id = CallerOrgID),
// mirroring the CreateLabel INSERT…SELECT precedent; a foreign-but-existing
// ProjectID yields zero source rows → NOT_FOUND, nothing inserted. The
// org-scoped branch (OrgID set, ProjectID empty) carries no project
// predicate, and the empty-CallerOrgID no-op is preserved here (unlike
// CreateLabel's hard-reject — CreateMilestone has trusted internal callers).
// This closes the last ungated cross-reference write-IDOR on the milestone
// create path; the parent_milestone_id parent-read seam above is unchanged.
func CreateMilestone(ctx context.Context, req CreateMilestoneRequest) (*Milestone, error)

type CreateMilestoneRequest struct {
    OrgID             string  // ULID; XOR with ProjectID
    ProjectID         string  // ULID; XOR with OrgID
    CallerOrgID       string  // pinned from identity.OrgID; NEVER from the wire (§10.1)
    ParentMilestoneID string  // optional ULID
    Name              string  // 1..200 chars
    Description       string  // optional, default ""
    StartDate         string  // ISO date (YYYY-MM-DD)
    EndDate           string  // ISO date (YYYY-MM-DD); end_date >= start_date
}

type Milestone struct {
    ID                string
    ParentMilestoneID string     // empty when root
    OrgID             string     // empty when project-scoped
    ProjectID         string     // empty when org-scoped
    Name              string
    Description       string
    StartDate         string     // ISO date
    EndDate           string     // ISO date
    CancelledAt       *time.Time
    CancelledReason   string
    CreatedAt         time.Time
    UpdatedAt         time.Time
}

//encore:api private method=POST path=/workitems.UpdateMilestone
// Updates name, description, start_date, end_date, cancelled_at,
// cancelled_reason. Re-validates M-INV-3 against the (possibly changed)
// parent range AND against any existing children (a date-range narrowing
// that violates a child's range is rejected). Reparenting is NOT
// supported in P01 — change parent_milestone_id is rejected with
// kind=VALIDATION (reparenting itself is deferred to P02; the milestone
// MCP tools ship NOW in P01 as Tools 16–19, §6.2).
//
// Tenant gate (round-16 / bead unblock-tv8.77): self-gates on a row-level
// tenant predicate — the targeted milestone's org_id = CallerOrgID OR its
// project_id IN (SELECT id FROM org.projects WHERE org_id = CallerOrgID)
// (CallerOrgID pinned from identity.OrgID, NEVER from the wire). A foreign
// MilestoneID yields NOT_FOUND, never a cross-tenant mutation. Empty
// CallerOrgID is the §10.1 no-op for trusted internal callers; the MCP
// handler always pins it.
func UpdateMilestone(ctx context.Context, req UpdateMilestoneRequest) (*Milestone, error)

type UpdateMilestoneRequest struct {
    MilestoneID     string
    CallerOrgID     string  // pinned from identity.OrgID; NEVER from the wire (§10.1)
    Name            *string
    Description     *string
    StartDate       *string  // ISO date; pointer = optional
    EndDate         *string  // ISO date
    CancelledAt     *time.Time
    CancelledReason *string
}

//encore:api private method=POST path=/workitems.AssignItem
// Sets workitems.items.milestone_id + milestone_assigned_at +
// milestone_assigned_by atomically. Pass MilestoneID="" to UNASSIGN
// (clears all three columns).
//
// M-INV-7 enforcement (item's milestone scope reachable in item's project):
// the target milestone's scope must satisfy
//   (milestone.project_id = item.project_id)
//   OR (milestone.org_id IS NOT NULL AND milestone.org_id = item.org_id)
// Rejects with kind=PRECONDITION_NOT_MET, data.invariant="M-INV-7" otherwise.
//
// Tenant gate (round-16 / bead unblock-tv8.77): self-gates on the TARGET
// item's tenancy — the item's org_id = CallerOrgID (CallerOrgID pinned
// from identity.OrgID, NEVER from the wire). A foreign ItemID yields
// NOT_FOUND, never a cross-tenant milestone assignment. Empty CallerOrgID
// is the §10.1 no-op for trusted internal callers (the §11.1.1 E2E seed);
// the MCP handler always pins it.
func AssignItem(ctx context.Context, req AssignItemRequest) error

type AssignItemRequest struct {
    ItemID         string
    CallerOrgID    string  // pinned from identity.OrgID; NEVER from the wire (§10.1)
    MilestoneID    string  // ULID; empty string = unassign
    AssignedByUser string  // ULID; the actor performing the assignment
}

//encore:api private method=POST path=/workitems.MilestoneTree
// Returns the recursive milestone tree rooted at RootMilestoneID, OR all
// roots within (OrgID, ProjectID) when RootMilestoneID is empty. Depth
// is capped at M-INV-6 (4) — the recursive CTE walks at most 4 levels
// (matches SPEC §9.4.9 milestone-walk pattern, which is bounded by
// M-INV-6 and is the same source-of-truth CTE used by CreateMilestone /
// UpdateMilestone for ancestor / depth checks).
//
// Used by:
//  - MCP Tool 19 `milestone_tree` (§6.2, round-16) — the agent-facing
//    facade delegates to this RPC;
//  - the E2E exit-criterion test (`apps/api/exitcriteriontest/`,
//    round-12) to verify post-seed milestone-tree shape;
//  - P05 Astro roadmap view (delegates to this RPC).
//
// Read-side tenant gate (round-16 / beads unblock-tv8.75 + .77): the
// recursive-CTE anchor is predicated on org_id = CallerOrgID OR
// project_id IN (SELECT id FROM org.projects WHERE org_id = CallerOrgID)
// (CallerOrgID pinned from identity.OrgID, NEVER from the wire) — NOT
// rbac.For (the rooted-CTE shape is not expressible via rbac.For; same
// justified deviation as ListLabels). A foreign RootMilestoneID produces
// an empty anchor → no rows, closing the IDOR read seam. Empty
// CallerOrgID is the §10.1 no-op for trusted internal callers (the
// §11.1.1 E2E seed, the P05 roadmap RPC); the MCP handler always pins it.
func MilestoneTree(ctx context.Context, req MilestoneTreeRequest) (*MilestoneTree, error)

type MilestoneTreeRequest struct {
    OrgID             string  // required when RootMilestoneID is empty (XOR ProjectID)
    ProjectID         string  // required when RootMilestoneID is empty (XOR OrgID)
    CallerOrgID       string  // pinned from identity.OrgID; NEVER from the wire (§10.1)
    RootMilestoneID   string  // optional; when set, OrgID/ProjectID derived from it
    IncludeCancelled  bool    // default false; when true, cancelled milestones appear
}

type MilestoneTree struct {
    Roots []MilestoneNode
}

type MilestoneNode struct {
    Milestone Milestone
    Depth     int             // 0 for roots, ≤ 3 for leaves (M-INV-6)
    Children  []MilestoneNode // recursive; empty when Depth = 3 (no further descent)
}
```

**AR-17 (new — round-2).** Milestone tree CTE depth bound. The recursive
CTE in `CreateMilestone` / `UpdateMilestone` / `MilestoneTree` is
structurally bounded by M-INV-6 (max depth 4). Unlike the dependency
cycle CTE (AR-8, depth ≤ 256), milestone walks are cheap by construction.
The CTE uses the same depth-counter pattern (`WHERE depth < 4` inside
the recursive term) — `LIMIT` in the recursive term remains undocumented
PG behaviour per research C5. CreateMilestone rejects with
`kind=PRECONDITION_NOT_MET, data.invariant="M-INV-6"` when the chain
would exceed 4 levels.

### 4.5 `deps` service

Owns: schema `deps` only. Consumes the unblock database via the
canonical BindDB late-bind pattern (§3.1) in `apps/api/deps/db.go`:
a nil `*sqldb.Database` pointer + exported `BindDB` hook, registered
in `apps/api/db/db.go`'s `init`. The skeleton hook is pre-wired in
P01 A-1; RPC bodies start reading `db` in beads C-1+ when DB-touching
code lands (cycle CTE, advisory locks, cascade Pub/Sub publisher).
No per-service `initbind.go`.

Private RPCs:

```go
package deps

//encore:api private method=POST path=/deps.AddEdge
// Acquires per-project advisory lock (AF5), runs the depth-counter
// reachability CTE (C5), inserts the edge, emits deps.cascade.requested
// if the to_item's readiness flips.
//
// Tenant gate (round-16 / bead unblock-tv8.77): AddEdge is MCP-reachable
// (add_dependency, Tool 11) and resolves both endpoint orgs from the DB.
// It self-gates on CallerOrgID (pinned from identity.OrgID, NEVER from the
// wire): both FromItem and ToItem must resolve to org_id = CallerOrgID. If
// either endpoint's resolved org differs from the caller's org, the RPC
// rejects with NOT_FOUND (the endpoints are not visible cross-tenant) —
// never a cross-tenant edge. Empty CallerOrgID is the §10.1 no-op for
// trusted internal callers; the MCP handler always pins it.
func AddEdge(ctx context.Context, req AddEdgeRequest) (*Edge, error)

type AddEdgeRequest struct {
    OrgID       string
    ProjectID   string
    CallerOrgID string // pinned from identity.OrgID; NEVER from the wire (§10.1)
    FromItem    string
    ToItem      string
    Kind        string // "blocks" | "related"; default "blocks"
}

type Edge struct {
    ID        string
    FromItem  string
    ToItem    string
    Kind      string
    CreatedAt time.Time
    CreatedBy string
}

//encore:api private method=POST path=/deps.RemoveEdge
// Removes edge; sync-inline recomputes is_ready for the direct to_item
// via the shared deps.recomputeReady helper; writes a cascade_events
// audit row (kind='edge_removed') in the same transaction. Then, after
// the transaction commits, publishes CascadeRequested{Reason:"edge_removed"}
// reusing the SAME event_id as the inline audit row (round-6 §6.3.0
// tension #1). The subscriber's INSERT ... ON CONFLICT
// (event_id, triggered_by_item_id) DO NOTHING collapses to no-op for
// the audit row, but the subscriber still walks the forward closure to
// recompute pipeline_stage on transitively affected items.
// `ToItemNowReady` in the response is documented as the SINGLE-HOP view —
// the direct to_item's new is_ready value. Transitive pipeline_stage
// updates downstream of that to_item are eventually consistent (driven
// by the post-commit publish, not by this RPC's return value). See §6.2
// Tool 12 and §6.3.0.
//
// Tenant gate (round-16 / bead unblock-tv8.77): RemoveEdge is
// MCP-reachable (remove_dependency, Tool 12) and resolves the edge's
// endpoint orgs from the DB. It self-gates on CallerOrgID (pinned from
// identity.OrgID, NEVER from the wire): the resolved edge endpoints must
// belong to org_id = CallerOrgID. If the targeted edge's resolved
// endpoint org differs from the caller's org, the RPC rejects with
// NOT_FOUND (the edge is not visible cross-tenant) — never a cross-tenant
// edge removal. Empty CallerOrgID is the §10.1 no-op for trusted internal
// callers; the MCP handler always pins it.
func RemoveEdge(ctx context.Context, req RemoveEdgeRequest) (*RemoveEdgeResponse, error)

type RemoveEdgeRequest struct {
    EdgeID      string  // EdgeID OR (FromItem + ToItem + Kind), exactly one path
    CallerOrgID string  // pinned from identity.OrgID; NEVER from the wire (§10.1)
    FromItem    string  // composite: paired with ToItem + Kind
    ToItem      string  // composite: paired with FromItem + Kind
    Kind        string  // composite: paired with FromItem + ToItem
}

type RemoveEdgeResponse struct {
    Removed         bool
    ToItemNowReady  bool  // computed inline in same transaction
    ToItemID        string  // resolved from EdgeID if composite path not used
}

//encore:api private method=POST path=/deps.IsReady
// Read-side helper: returns the current is_ready value (read from
// workitems.items, NOT recomputed). Used by smoke tests; production
// readers query workitems.items directly.
func IsReady(ctx context.Context, itemID string) (bool, error)

//encore:api private method=POST path=/deps.Closure
// Returns the transitive 'blocks' closure (incoming) for an item.
func Closure(ctx context.Context, req ClosureRequest) (*ClosureResponse, error)

type ClosureRequest struct {
    ItemID    string
    Direction string // "incoming" | "outgoing"
    MaxDepth  int    // 1..256; default 256
}
type ClosureResponse struct {
    ItemIDs []string
}

//encore:api private method=POST path=/deps.RecentCascadeEvents
// AF2: returns the last 50 deps.cascade_events rows for the org/project,
// ordered by triggered_at DESC. Used by the prime tool.
func RecentCascadeEvents(ctx context.Context, req RecentCascadeEventsRequest) (*RecentCascadeEventsResponse, error)

type RecentCascadeEventsRequest struct {
    OrgID     string
    ProjectID string // optional
    Limit     int    // capped at 50; default 50
}
type RecentCascadeEventsResponse struct {
    Events []CascadeEventRow
}
type CascadeEventRow struct {
    ID                  string
    EventID             string
    TriggeredByItemID   string
    AffectedItemIDs     []string
    CascadedCount       int
    TriggeredAt         time.Time
    TraceID             string // ULID minted by the mcp raw endpoint that triggered the cascade; mirrors deps.cascade_events.trace_id (§10.2 Option B).
}
```

### 4.6 `providers`, `boards`, `memory` (schema-only in P01)

These services have **no Go package code in P01**. Their schemas migrate
in P01 (per Plan §2.1 + Q2 resolution) but no `//encore:api` declarations
exist in their directories until P02 (`providers`, `memory`) and P05
(`boards`).

To prevent the canonical BindDB consumer pattern (§3.1) from referencing
non-existent services, P01 leaves these directories empty (no `.go` files
under `apps/api/providers/`, `apps/api/boards/`, `apps/api/memory/`).
Encore treats absent service directories as non-services; the schemas
exist purely as DB-side artifacts maintained by the dedicated
`apps/api/db/` service's migration runner. When P02 lands `providers`
and `memory` and P05 lands `boards`, each will add its own
`BindDB` hook per §3.1 and a corresponding bind line in
`apps/api/db/db.go`'s `init`.

---

## 5. Public Surface (single Streamable HTTP endpoint)

Per FR-12, P01 exposes **one logical public endpoint**: `POST /mcp` +
`GET /mcp` (Streamable HTTP per MCP spec 2025-06-18).

### 5.1 Transport contract

| Aspect | Value |
|---|---|
| Protocol | HTTP/1.1 + HTTP/2 (Encore-default) |
| Methods | `POST /mcp` (client → server JSON-RPC; may return single `application/json` body OR `text/event-stream` for incremental responses); `GET /mcp` (server → client SSE for resumable sessions) |
| `Accept` (client) | `application/json, text/event-stream` |
| `Authorization` | `Bearer <api-key>` — required on **every** request |
| `Mcp-Session-Id` | Returned by server on `initialize`; echoed by client on subsequent requests |
| Heartbeat | Server emits an MCP-protocol-native JSON-RPC `ping` request over the open session every 15s on long-lived `GET /mcp` streams. On SSE streams the ping surfaces as an `event: message\ndata: {…ping…}` frame, which is what the modelcontextprotocol/go-sdk (v1.6.0, pinned by D-1) emits when its `ServerOptions.KeepAlive` is set per MCP spec 2025-06-18. The earlier `:keepalive\n\n` SSE-comment literal was a pre-SDK placeholder; the JSON-RPC `ping` is the protocol-canonical form and produces the same anti-idle effect at the wire level (mitigates Encore Cloud edge-proxy idle close per RP01-4). |
| Error envelope | JSON-RPC 2.0 error object (see §7) |

### 5.2 What is NOT exposed in P01

- `POST /webhooks/github` — P02 (Plan §3.1).
- `POST /webhooks/gitlab` — v1.1.
- OAuth callback — Astro origin (P05); P01 exercises `auth.ExchangeOAuthCode` via private RPC in tests only.
- `mcp.meta_catalogue` MCP tool — P02 (Plan §3.4 / Q4 resolution).
- `verify_can_transition` — P02.

---

## 6. The 23 P01 MCP Tools (JSON-locked)

Tool names match SPEC §5.2.2. Every tool returns either a typed result
object or a JSON-RPC error object per §7.

> **Round-16 inventory (2026-06-04).** P01 exposes **23** agent-facing
> tools (was 14). Tools 1–14 are the original core set; round-16 adds
> Tool 15 `promote` (bead `unblock-tv8.71`), Tools 16–19 milestone tools
> (`create_milestone` / `update_milestone` / `assign_item` /
> `milestone_tree`, bead `unblock-tv8.74`), and Tools 20–23 label-registry
> tools (`create_label` / `list_labels` / `update_label` / `delete_label`,
> bead `unblock-tv8.75`). The v1.0 total is **27** (these 23 + the 4 memory
> tools at P02). See §6.6 for the status transition map `promote` closes.

The arguments and result schemas below are **canonical** for P01. Phase 02
may add fields (additive only); existing fields are immutable.

> **Wire convention** (cross-ref §3.6): every JSON key in this section is
> snake_case. §3.6 generalises the same convention to the private Encore
> RPC surface and Pub/Sub payloads; the rules quoted in this section are
> the original lock and remain authoritative for MCP.

### 6.1 MCP framing

JSON-RPC 2.0 over Streamable HTTP. Each tool is dispatched via the
standard MCP `tools/call` method:

```jsonc
{
  "jsonrpc": "2.0",
  "id": "<client-supplied id>",
  "method": "tools/call",
  "params": {
    "name": "<tool-name>",
    "arguments": { /* tool-specific */ }
  }
}
```

Tool-call results follow MCP convention:

```jsonc
{
  "jsonrpc": "2.0",
  "id": "<echo>",
  "result": {
    "content": [ { "type": "text", "text": "<JSON-encoded payload>" } ],
    "isError": false,
    "structuredContent": { /* tool-specific typed payload */ }
  }
}
```

P01 uses `structuredContent` for typed payload (introduced in MCP
2025-06-18 spec) and replicates the JSON in `content[0].text` for clients
that have not adopted `structuredContent` parsing.

### 6.2.0 Cursor keyset pagination

Tools 2 (`ready`), 8 (`list`), and 9 (`search`) expose cursor keyset
pagination over their respective canonical sort tuples. The contract is
identical across all three read surfaces:

- **Argument.** `cursor` is an OPTIONAL opaque string. When absent the
  server returns the first page. When present, the server decodes +
  verifies it and returns rows strictly after the encoded anchor.
- **Result.** `next_cursor` is a string OR `null`. When `null` the
  caller has reached the end of the stream — the response is the final
  page. When a string, passing it back as the next request's `cursor`
  yields the next page with zero duplicates and zero skips.
- **Encoding.** Cursors are `base64url`-encoded JSON tuples followed by
  an HMAC-SHA256 tag computed with the deployment's
  `API_KEY_HMAC_SECRET` (re-used; no new secret is introduced in P01).
  Tuple shape per tool:
  - Tool 2 `ready`: `{priority, created_at_unix_us, id}`.
  - Tool 8 `list`: `{id}` (List orders by `id ASC` only — see §4.4).
  - Tool 9 `search`: `{rank, item_id, comment_id}`.
- **Validation errors.** Any of (decode failure, HMAC mismatch, wrong
  tuple shape, tuple field type mismatch) MUST return the §7
  `VALIDATION` envelope with `data.field = "cursor"`. Cursors are NOT
  cross-tool portable — a Tool 2 cursor presented to Tool 8 is a
  shape mismatch and is rejected.
- **Lifetime.** Cursors are NOT persisted server-side. They survive
  process restarts (signed by the secret) but a secret rotation
  invalidates every outstanding cursor — a pre-prod-acceptable
  operational tradeoff identical to the API-key contract (§4.3.2).

Rationale: after migration `0100` (Tool 2's covering index) the keyset
ORDER BY for the three read tools is served from a pure index scan.
Filters (`priority_min`, `project_id`, `claimed_by`, `labels`,
FTS predicate) compose with the cursor predicate as additional WHERE
clauses — they narrow the result set but do NOT substitute for
pagination when an agent legitimately needs to consume the full set
in chunks.

### 6.2.0a Advertised input-argument schema + bounds enforcement (locked — bead `unblock-tv8.82`)

> **(locked, additive)** This subsection pins what the live `tools/list`
> schema advertises and how the per-tool argument bounds are enforced. It is
> the §6.2-level companion to the §7.3 uniform-validation contract. Decisions
> LOCKED by Miguel (2026-06-12). No DDL / migration / success-payload change.

**Rich schema advertised (NET-NEW).** The live `tools/list` input schema for
every tool MUST advertise the **FULL** input-argument contract for agent
discovery: the `enum` value set for every closed-enum argument
(`priority_min`, `status[]`, `pipeline_stage[]`, `comment.kind`,
`comment.status`, `source`, and any other §6.1/§6.5-enumerated argument), the
`minimum`/`maximum` bounds for every numeric argument with a declared range,
and the `required[]` list. This is **net-new**: prior to this bead the live
schema (reflected by the go-sdk `jsonschema.ForType` from each Go input
struct, §10.3) carried only `type` + `required` + `additionalProperties:false`
and NEVER `enum` or `minimum`/`maximum`, so the per-tool bounds quoted in the
§6.2 argument comments (`// 1..200`, `"P0".."P4"`, etc.) and in
`catalogue.json` were never on the wire. They are now first-class, advertised
schema.

**Paginated `limit` bounds (ENFORCED — out-of-range REJECTS).** The paginated
read tools advertise an inclusive `[minimum, maximum]` on their limit
argument and **enforce** it via §7.3:

| Tool | Argument | Range (`min..max`) | Default (when omitted) |
|---|---|---|---|
| Tool 1 `prime` | `ready_limit` | `1..50` | `10` |
| Tool 2 `ready` | `limit` | `1..200` | `10` |
| Tool 8 `list` | `limit` | `1..200` | `50` |
| Tool 9 `search` | `limit` | `1..100` | `25` |

A supplied value outside its range — including `limit <= 0` and a
`prime.ready_limit > 50` — is **REJECTED** with the §7 `VALIDATION` envelope
(`data.field` = the limit argument, `data.bound` = the range), NOT silently
coerced to the default and NOT clamped to the maximum. This **re-locks** the
round-7 pagination semantics: the §6.2 argument comments below that read
`// 1..N; default D` denote an ENFORCED inclusive bound with a default that
applies ONLY on omission — any prior coerce-to-default / clamp-to-max reading
(in handler doc-comments or in-code comments) is superseded by the reject
contract per §7.3.1. (`deps.RecentCascadeEvents.Limit`'s "capped at 50"
internal-RPC note in §4.4 is an internal private-RPC clamp on a server-set
value — NOT a wire argument — and is untouched: `prime`'s WIRE `ready_limit`
is the §7.3 enforced bound; the recent-events fan-out cap is an internal
implementation detail of the `prime` handler.)

**Enum + type + required.** Per §7.3, an invalid enum value, a wrong type, or
a missing required argument on ANY of the 23 tools likewise returns the §7
`VALIDATION` envelope with `data.field`. The `cursor` argument's existing
`VALIDATION` contract (§6.2.0: decode/HMAC/shape/type failures →
`data.field = "cursor"`) is a member of this same uniform family and is
unchanged.

### 6.2 Tool-by-tool contracts

> **Round-16 milestone-tools note (OVERRIDES the round-2 D1 deferral).**
> The original round-2 D1 deferral note read: "Milestone CRUD MCP tools
> are NOT in the P01 14-tool inventory; they ship in P02 alongside the
> memory tools (option (c) preserves PRD FR-8 '18 tools at v1.0')." Bead
> `unblock-tv8.74` **reverses** that deferral. The four milestone MCP
> tools `create_milestone` / `update_milestone` / `assign_item` (incl.
> unassign) / `milestone_tree` ship in P01 as **Tools 16–19** (§6.2),
> thin MCP facades over the `workitems.CreateMilestone`,
> `workitems.UpdateMilestone`, `workitems.AssignItem`, and
> `workitems.MilestoneTree` private RPCs (§4.4.1) that already exist. The
> FR-8 figure is reconciled from "18 tools at v1.0" to "27 tools at v1.0"
> (P01=23, +4 memory at P02) across PRD / SPEC / plan in lockstep (round-16
> changelog). Tool 4 (`create`) and Tool 5 (`update`) still accept a
> `milestone_id` field that references an existing milestone — they do not
> create or modify milestone rows; Tool 8 (`list`) still accepts
> `milestone_id` as a filter.

#### Tool 1 — `prime`

Returns the dashboard for a fresh agent session.

```jsonc
// arguments
{
  "project_id": "<ULID; optional — defaults to caller's primary project>",
  "ready_limit": 10  // 1..50; default 10
}

// structuredContent
{
  "ready_summary": {
    "count_total": 42,
    "items": [ /* up to ready_limit Item objects */ ]
  },
  "claimed_by_me": [ /* Item objects where claimed_by_id = caller's user_id */ ],
  "recent_cascade_events": [ /* last 50 CascadeEventRow per AF2 */ ],
  "memory_hints": []  // empty in P01; populated in P02 once memory ships
}
```

#### Tool 2 — `ready`

```jsonc
// arguments
{
  "project_id": "<ULID; optional>",
  "limit": 10,         // 1..200; default 10
  "priority_min": "P3", // optional; "P0".."P4"
  "cursor": "<opaque>"  // optional; first page when absent
}

// structuredContent
{
  "items": [ /* Item objects ordered by (priority asc, created_at asc, id asc) */ ],
  "total_ready": 0,        // total count for the org/project, may exceed `limit`
  "next_cursor": "<opaque|null>"  // null when this is the last page
}
```

Read implementation: filtered scan of `workitems.items` using
`items_ready_partial_idx` (`WHERE is_ready = true AND status = 'Ready' AND
closed_at IS NULL`). Deterministic ordering is guaranteed by the
`(priority, created_at, id)` composite sort; `id` is a ULID so it serves
as a stable tiebreaker. After migration `0100` the index covers
`(org_id, project_id, priority, created_at, id)` so the ORDER BY +
keyset pagination is served from a pure index scan.

**Cursor keyset pagination.** `cursor` is an opaque, server-signed token
that encodes the last-row position on the canonical sort tuple
`(priority, created_at, id)`. On request, the server decodes + verifies
the token and emits the page strictly after that anchor; on response,
`next_cursor` is the token for the row that would start the next page
(or `null` when no more rows exist). Token shape, signing, and error
contract are pinned in §6.2.0 (Cursor keyset pagination) below. Agents
MUST NOT manufacture or mutate cursor values — invalid cursors are
rejected with the §7 `VALIDATION` envelope (`data.field = "cursor"`).

#### Tool 3 — `claim`

```jsonc
// arguments
{
  "item_id": "<ULID>"
}

// structuredContent (success)
{
  "claimed": true,
  "item": { /* Item with claimed_by_id, claimed_at populated */ }
}
```

Loser receives the structured error envelope (§7) with code
`ALREADY_CLAIMED` and `data.winner_user_id`, `data.winner_agent`,
`data.claimed_at`.

**Tenant gate (round-16 / bead `unblock-tv8.77`).** The handler passes
the Bearer-resolved `identity.OrgID` into `workitems.Claim` as
`CallerOrgID` (internal channel, never wire). `Claim` self-gates the
locked-row `SELECT … FOR UPDATE` on `org_id = $caller`, so a foreign
`item_id` yields `NOT_FOUND` (not `ALREADY_CLAIMED`), never a
cross-tenant claim. See §10.1 for the row-level write-gate model and the
empty-`CallerOrgID` no-op ratification.

#### Tool 4 — `create`

```jsonc
// arguments — mirrors workitems.CreateRequest
{
  "project_id": "<ULID>",
  "parent_id": "<ULID; optional>",
  "discovered_from_id": "<ULID; optional, required for finding>",
  "type": "task",                    // "epic" | "task" | "finding"
  "title": "Implement /ready handler",
  "body": "...",
  "priority": "P2",
  "milestone_id": "<ULID; optional>",
  "labels": ["<label-ULID>", ...],
  "dependencies": [
    { "blocker_item_id": "<ULID>", "kind": "blocks" }
  ],
  "severity": "...",                 // required when type=finding
  "kind_of_finding": "review"        // required when type=finding
}

// structuredContent
{
  "item": { /* Item */ }
}
```

Cycle check (C5/AF5) runs inline for any `dependencies[]` entries on the
new item; if any would create a cycle, the entire `create` is rejected.

**Tenant scoping (round-16, bead `unblock-tv8.78`).** The new item's
`org_id` is stamped from the Bearer-resolved org (`identity.OrgID`, NEVER
the wire), and EVERY wire-supplied reference is validated against that org
inside the create transaction: `project_id`, `parent_id`,
`discovered_from_id`, `milestone_id`, and each entry of `labels[]` must
belong to the caller's org (milestones and labels: org-scoped to the caller
OR project-scoped to a project in the caller's org; `parent_id` /
`discovered_from_id`: a caller-org item). A foreign-but-existing reference
yields the SAME `NOT_FOUND` envelope as a missing id — existence in another
org is never disclosed. The `dependencies[].blocker_item_id` endpoint is
gated identically by `deps.AddEdgeInTx` (its `CallerOrgID` check). Unlike
the §10.1.1 update/delete-by-id RPCs, `create` keys this gate on its
already-trusted `OrgID` field rather than a separate `CallerOrgID` channel
(DECISION, Miguel 2026-06-12 — §10.1.1).

#### Tool 5 — `update`

```jsonc
// arguments
{
  "item_id": "<ULID>",
  "title": "<string; optional>",
  "body": "<string; optional>",
  "priority": "<P0..P4; optional>",
  "milestone_id": "<ULID|null; optional>",
  "labels": ["<label-ULID>", ...]    // optional; full replace when present
}
```

Does NOT touch state dimensions — use `set_state` for those.

**Tenant gate (round-16 / bead `unblock-tv8.77`).** The handler passes
the Bearer-resolved `identity.OrgID` into `workitems.Update` as
`CallerOrgID` (internal channel, never wire). `Update` self-gates its
`UPDATE` on `org_id = $caller`, so a foreign `item_id` yields
`NOT_FOUND`, never a cross-tenant mutation. The backing `workitems.Update`
ALSO gates the wire-supplied `milestone_id` argument on `CallerOrgID`
(bead `unblock-tv8.84`): when non-empty, the `milestone_id` must belong to
the caller's org (org-XOR-project predicate) or the `UPDATE` affects zero
rows → `NOT_FOUND`, nothing changed. The clear-to-null (`milestone_id`:
`null`) and omitted paths are unaffected. See §10.1.1.

#### Tool 6 — `close`

```jsonc
// arguments
{
  "item_id": "<ULID>",
  "reason": "<string; optional>"  // recorded as a kind=completed comment if present
}

// structuredContent
{
  "item": { /* Item with status=Done, closed_at populated */ }
}
```

**P01-only precondition (AF3, plan §3.4):** rejects with
`PRECONDITION_NOT_MET` and `data.missing = "claimed_by_id"` if
`claimed_by_id IS NULL`. The full Layer-1 BLOCK conditions
(`qa_state=passed` etc.) ship in P02.

**Tenant gate (round-16 / bead `unblock-tv8.77`).** The handler passes
the Bearer-resolved `identity.OrgID` into `workitems.Close` as
`CallerOrgID` (internal channel, never wire). `Close` self-gates the
status-flip `UPDATE` on `org_id = $caller`, so a foreign `item_id` yields
`NOT_FOUND`, never a cross-tenant close. The gate is checked BEFORE the
AF3 `claimed_by_id` precondition. See §10.1.

Side-effects (round-6 §6.3.0 symmetric writer model):

(a) **Inline (Regime A — `is_ready`).** In the same transaction as the
status flip to Done, the handler recomputes `is_ready` for the closed
item's direct `blocks` neighbours via the shared
`deps.recomputeReady(ctx, tx, neighbour_id)` helper. This covers the
single-hop view — the immediate dependents may now flip ready.

(b) **Post-commit (Regime B — `pipeline_stage`).** After the
transaction commits, the handler publishes
`CascadeRequested{Reason:"close", TriggeredByItemID: item_id, …}` on
`deps.cascade.requested`. The subscriber walks the forward `blocks`
closure and recomputes `pipeline_stage` per §5.7.1 on every
transitively affected item; it writes one `deps.cascade_events` row
with `kind='close'`.

Cross-reference: §6.3.0 (propagation regimes), §6.3.2 (subscriber
dispatch).

#### Tool 7 — `show`

```jsonc
// arguments
{
  "item_id": "<ULID>",
  "include_comments": true,         // default true
  "include_dependencies": true,     // default true
  "include_findings": true          // default true
}

// structuredContent
{
  "item": { /* Item */ },
  "parent": { "id": "<ULID>", "title": "...", "status": "..." },  // null when item has no parent (round-16 / tv8.76)
  "comments": [ /* Comment[] ordered by created_at asc */ ],
  "dependencies_in":  [ /* ResolvedRef[] — see below; one per Edge where to_item = item_id */ ],
  "dependencies_out": [ /* ResolvedRef[] — see below; one per Edge where from_item = item_id */ ],
  "findings":         [ /* Item[] of children with type=finding */ ]
}

// ResolvedRef shape (round-16 / tv8.76)
{
  "id":     "<ULID>",   // the dependency edge's target item id
  "title":  "...",      // the target item's title
  "status": "...",      // the target item's Status enum (§6.1)
  "kind":   "blocks"    // the edge kind ("blocks" | "related") — carried so the agent keeps edge semantics
}
```

**Reference resolution (round-16, bead `unblock-tv8.76`).** `show` now
resolves the parent and the direct in/out dependency targets to
`{id, title, status}` objects instead of returning bare IDs, so an agent
can render the immediate neighbourhood without N follow-up `show`/`get`
calls. Resolution is **bounded to exactly one level** — the resolved
neighbours' OWN parents and dependencies are NOT walked; `dependencies_in`
/ `dependencies_out` carry one `ResolvedRef` per direct edge and nothing
transitive. `parent` is `null` when the item has no parent epic.
Payload is bounded: the direct in/out edge sets are already capped by the
per-project graph degree at v1 scale, and each `ResolvedRef` carries only
the four scalar fields above (no body, no comment trail, no nested
findings) — the full neighbour record is reachable via a follow-up
`show(<neighbour-id>)`. Resolution joins `workitems.items` once per
direction inside `workitems.GetTrail` (the backing RPC); all resolved
rows are RBAC-scoped identically to the root item, so a cross-tenant or
unauthorised neighbour is omitted rather than leaked. The §4.4 `Trail`
struct's `DependenciesIn` / `DependenciesOut` are widened from `[]Edge`
to a resolved form in lockstep (the `Edge.ToItem`/`Edge.FromItem` ids are
joined to the target item's `title` + `status` at query time).

#### Tool 8 — `list`

```jsonc
// arguments
{
  "project_id": "<ULID; optional>",
  "milestone_id": "<ULID; optional>",
  "status": ["Ready", "InProgress"],  // optional []
  "pipeline_stage": ["Implementation"], // optional []
  "claimed_by": "<user-ULID; optional>",
  "labels": ["<label-ULID>"],         // optional []
  "limit": 50,                        // 1..200; default 50
  "cursor": "<opaque>"
}

// structuredContent
{
  "items": [ /* Item[] */ ],
  "next_cursor": "<opaque|null>"
}
```

#### Tool 9 — `search`

```jsonc
// arguments
{
  "project_id": "<ULID; optional>",
  "query": "ready handler",   // websearch_to_tsquery format
  "limit": 25,                 // 1..100; default 25
  "cursor": "<opaque>"         // optional; first page when absent
}

// structuredContent
{
  "hits": [
    {
      "item_id": "<ULID>",
      "source": "item",            // "item" | "comment"
      "comment_id": "<ULID|null>",
      "rank": 0.87,
      "snippet": "<ts_headline output, ≤ 200 chars>"
    }
  ],
  "next_cursor": "<opaque|null>"  // null when this is the last page
}
```

Query plan: `UNION ALL` over `items_fts_idx` and `comments_fts_idx`
(per AF1 / R-P01-7), filtered by `org_id` (and `project_id` if supplied)
via the RBAC helper, ranked by `ts_rank_cd` desc, limited to N. Keyset
pagination uses the canonical FTS sort tuple `(rank desc, item_id asc,
comment_id asc)` — see §6.2.0 for the cursor contract; `comment_id` is
the empty string for `source="item"` rows so the tiebreaker is total.

#### Tool 10 — `comment`

```jsonc
// arguments
{
  "item_id": "<ULID>",
  "parent_id": "<ULID; optional thread parent>",
  "kind": "investigation",         // §6.5 kinds
  "status": "info",                // "error" | "warning" | "info" | "success"
  "body": "..."                    // 1..16384 chars
}

// structuredContent
{
  "comment": { /* Comment */ }
}
```

Append-only by construction (no update/delete tool ships in P01).

**Body length enforcement boundary.** Handler enforces 1..16384 chars at
the MCP boundary; `workitems.AppendComment` enforces the non-empty floor.

**Tenant gate (round-16 / bead `unblock-tv8.77`).** The handler passes
the Bearer-resolved `identity.OrgID` into `workitems.AppendComment` as
`CallerOrgID` (internal channel, never wire). Because `AppendComment` is
an `INSERT`, it gates via an `INSERT … SELECT` whose `SELECT` is
predicated on the PARENT item's tenancy — the row is inserted only when
the `item_id` resolves to an item with `org_id = $caller`. A foreign
`item_id` inserts zero rows and the RPC returns `NOT_FOUND`, never a
cross-tenant comment. See §10.1.

**Threading scope — `parent_id` is SAME-ITEM scoped (bead
`unblock-tv8.80`, contract LOCKED by Miguel 2026-06-12).** `parent_id` is
optional: when empty/absent the comment is a top-level (root) comment on
`item_id`; when non-empty it MUST resolve to an EXISTING comment **on the
same item** (`item_id` of the parent comment = the target `item_id`). A
`parent_id` that references a comment on a DIFFERENT item — including a
comment under a FOREIGN-org item — yields `NOT_FOUND`, indistinguishable
from a missing parent (no existence disclosure). This is enforced by the
`AppendComment` `INSERT … SELECT` predicate (the §10.1.1 write-gate
mechanism), NOT merely by the existence-only/unnamed `comments` FK: the
same `INSERT … SELECT` that gates `item_id` on `org_id = $caller`
additionally requires the supplied `parent_id` to belong to a comment
whose `item_id` is the target item. Because the target item is already
`CallerOrgID`-gated, same-item transitively guarantees same-org — this is
the stricter, correct predicate that closes the cross-tenant / cross-item
`parent_id` IDOR (live-proven in the MCP sweep; same class as
`unblock-tv8.77` / `unblock-tv8.78`). The self-parent prohibition
(`comments_no_self_parent_chk`) and the empty-`parent_id` top-level path
are preserved.

#### Tool 11 — `add_dependency`

```jsonc
// arguments
{
  "from_item_id": "<ULID>",   // blocker
  "to_item_id":   "<ULID>",   // blocked
  "kind": "blocks"            // "blocks" | "related"; default "blocks"
}

// structuredContent
{
  "edge": { /* Edge */ }
}
```

Cycle check: per-project advisory lock + depth-counter CTE per §6.5 below.
On rejection, error code `CYCLE_DETECTED` with `data.cycle_path = ["<id>", ...]`.

**Tenant gate (round-16 / bead `unblock-tv8.77`).** The handler passes
the Bearer-resolved `identity.OrgID` into `deps.AddEdge` as `CallerOrgID`
(internal channel, never wire). `AddEdge` resolves BOTH endpoints'
(`from_item_id`, `to_item_id`) orgs from `workitems.items` and rejects
with `NOT_FOUND` if either resolved endpoint org differs from
`CallerOrgID` — so an agent cannot wire a `blocks`/`related` edge to or
from another tenant's item. The gate is checked alongside the existing
same-project requirement (cross-project edges are already `VALIDATION`).
See §10.1.

**`project_id` derivation (review L6-W8).** The advisory lock key is the
`to_item_id`'s `project_id`, looked up in `workitems.items` at the start
of the transaction. **P01 rejects cross-project edges** with
`code: VALIDATION, kind: VALIDATION, data.field = "to_item_id"` if
`workitems.items[from_item_id].project_id != workitems.items[to_item_id].project_id`.
Cross-project dependencies are explicitly out-of-scope at v1.0 (the
single-project advisory lock is the simplest correct concurrency model;
cross-project locking would need org-level coordination that adds
complexity without v1.0 value).

**Required field for type=finding** (review L6-W1, cross-cutting):
when `from_item_id` references a `type='finding'` work item, the edge
is allowed (findings can block other items) but the
`items_finding_required_fields_chk` constraint on the `from_item_id`
row must already be satisfied — the spec relies on the DDL CHECK
having been enforced at finding creation time.

**Side-effects (round-6 §6.3.0 symmetric writer model).** After the
transaction in §6.5 commits, the handler publishes
`CascadeRequested{Reason:"edge_added", TriggeredByItemID: to_item_id,
EventID: ulid.New(), TraceID: tracectx.From(ctx), EmittedAt: time.Now()}`
on `deps.cascade.requested`. The inline §6.5 UPDATE covers the
single-hop `is_ready` recompute for `to_item`; the publish drives the
multi-hop `pipeline_stage` recompute on the forward closure (Regime B).
The subscriber writes one `deps.cascade_events` row with
`kind='edge_added'`.

#### Tool 12 — `remove_dependency`

```jsonc
// arguments
{
  "edge_id": "<ULID>"        // OR (from_item_id + to_item_id + kind)
}

// structuredContent
{
  "removed": true,
  "to_item_now_ready": true   // computed inline; sync within the same transaction
}
```

**Tenant gate (round-16 / bead `unblock-tv8.77`).** The handler passes
the Bearer-resolved `identity.OrgID` into `deps.RemoveEdge` as
`CallerOrgID` (internal channel, never wire). `RemoveEdge` resolves the
targeted edge's endpoint orgs from the DB and rejects with `NOT_FOUND` if
the resolved endpoint org differs from `CallerOrgID` — so an agent cannot
delete another tenant's dependency edge by guessing an `edge_id` (or a
composite `from/to/kind` triple). The gate fires BEFORE the `DELETE`. See
§10.1.

**Implementation (round-6 §6.3.0 symmetric writer model).** The
single-hop `is_ready` recompute on the direct `to_item` runs inline in
the same Postgres transaction as the `DELETE` (Regime A); the
multi-hop `pipeline_stage` recompute is driven by a post-commit
`CascadeRequested` publish (Regime B). The inline audit row's
`event_id` is **reused** as the publish envelope's `EventID` so the
subscriber's later insert collapses to no-op via the `ON CONFLICT
(event_id, triggered_by_item_id) DO NOTHING` clause (tension #1
ruling — exactly one `deps.cascade_events` row per logical edge
remove). The whole inline flow:

```
event_id := ulid.New()  -- captured before BEGIN so it can be reused
                           -- on the post-commit publish below
BEGIN;
  DELETE FROM deps.dependencies WHERE id = $edge_id (or composite);
  -- Regime A: shared helper deps.recomputeReady(ctx, tx, item_id)
  --   recomputes is_ready for the direct to_item via the closure CTE
  --   (§6.5) and writes UPDATE workitems.items SET is_ready = $new
  to_item_now_ready := deps.recomputeReady(ctx, tx, $to_item_id);
  -- Audit row written inline; event_id is the ULID captured above.
  -- `kind='edge_removed'` is the discriminant (see §9.4.4 + §3.2).
  INSERT INTO deps.cascade_events (id, event_id, kind,
    triggered_by_item_id, affected_item_ids, cascaded_count, ...)
    VALUES (ulid(), event_id, 'edge_removed', $to_item_id,
            ARRAY[$to_item_id], 1, ...);
COMMIT;
-- Regime B: post-commit publish, REUSING event_id.
deps.CascadeRequestedTopic.Publish(ctx, &deps.CascadeRequested{
    EventID:           event_id,                -- reused (tension #1)
    OrgID:             $org_id,
    ProjectID:         $project_id,
    TriggeredByItemID: $to_item_id,
    Reason:            "edge_removed",
    TraceID:           tracectx.From(ctx),
    EmittedAt:         time.Now(),
})
return { removed: true, to_item_now_ready };
```

**`to_item_now_ready` is the single-hop view.** The boolean returned
in the RPC response reflects ONLY the direct `to_item`'s
`is_ready` value after the inline recompute. Transitive
`pipeline_stage` updates on items downstream of `to_item` are
**eventually consistent** — they are applied by the cascade
subscriber after the publish above is delivered. Callers that need
the transitively-consistent view should poll `get_state` or read
`workitems.items.pipeline_stage` after observing the corresponding
`deps.cascade_events` row land.

**Exactly one audit row per logical remove (tension #1).** The inline
INSERT lands first (during the transaction) and the subscriber's
attempted INSERT collapses via the UNIQUE
`(event_id, triggered_by_item_id)` constraint and the
`ON CONFLICT … DO NOTHING` clause in §6.3.2 step 5. The subscriber
still performs its `pipeline_stage` recompute pass; the no-op only
applies to the audit-row insert.

**Why a publish at all (round-6 rationale).** Pre-round-6, this tool
ran fully sync inline and did not publish. The round-6 cascade-symmetry
review observed that removing an edge can flip the `pipeline_stage`
of items transitively downstream of `to_item` per §5.7.1 (the
upstream chain's readiness changes). The single-hop inline UPDATE
covers `is_ready` for `to_item` but cannot cover transitive
`pipeline_stage` changes without doing the subscriber's work twice.
Symmetric writer model (§6.3.0): single-hop inline, multi-hop via
publish — uniform across the four cascade kinds.

#### Tool 13 — `set_state`

```jsonc
// arguments
{
  "item_id": "<ULID>",
  "impl_state":     "<pending|done; optional>",
  "review_state":   "<pending|approved|needs_rework; optional>",
  "qa_state":       "<pending|passed|failed; optional>",
  "pipeline_state": "<running|needs_human|paused|no_investigation; optional>",
  "intent_comment": {                  // optional but recommended
    "kind": "completed",
    "status": "success",
    "body": "Implementation complete; all tests pass"
  }
}

// structuredContent
{
  "item": { /* Item with the new state columns + recomputed pipeline_stage */ },
  "warnings": [                          // optional; omitempty when empty (§7 success-side contract)
    {
      "code": "intent_comment_dropped",
      "message": "state mutation committed; intent_comment append failed and was dropped",
      "details": { "intent_comment_kind": "completed", "intent_comment_status": "success" }
    }
  ]
}
```

**Tenant gate (round-16 / bead `unblock-tv8.77`).** The handler passes
the Bearer-resolved `identity.OrgID` into `workitems.SetStateColumns` as
`CallerOrgID` (internal channel, never wire). `SetStateColumns`
self-gates the `SELECT … FOR UPDATE` row lock on `org_id = $caller`, so a
foreign `item_id` yields `NOT_FOUND` BEFORE any invariant check runs,
never a cross-tenant state mutation. See §10.1.

**P01 enforcement (round-2 D2 — five PRD §6.2 invariants + structural
checks):**

Writes are gated by:

(a) **Structural invariants:**

- `impl_state=done` requires `claimed_by_id IS NOT NULL` (DB-level CHECK
  via `items_claim_status_chk` once `status` is updated; also enforced
  defensively at the MCP layer with a clearer error).
- The `(impl_state, review_state, qa_state, pipeline_state)` CHECK
  constraints from `0040_workitems.up.sql` reject malformed enum
  combinations (e.g. unknown values).

(b) **The five PRD §6.2 state-machine invariants (round-2 D2).** Each is
enforced inside the same transaction as the column write; on violation,
the RPC returns `kind=PRECONDITION_NOT_MET` with `data.invariant`
populated for machine-readability:

| # | Invariant (PRD §6.2 verbatim) | Enforcement | `data.invariant` |
|---|---|---|---|
| I-1 | Writing `review_state=needs_rework` resets `qa_state=pending` in the same transaction | Atomic UPDATE: when `req.review_state='needs_rework'`, the SQL writes both columns (no error case — invariant is auto-applied) | n/a (no rejection; auto-reset applied) |
| I-2 | Writing `qa_state=failed` requires `review_state=approved` | Pre-check inside the FOR UPDATE: reject if `req.qa_state='failed'` AND current `review_state <> 'approved'` (after applying any concurrent `req.review_state` change in the same call) | `qa_failed_requires_review_approved` |
| I-3 | After `qa_state=failed`, the next supervisor `claim` resets `review_state=pending` + `qa_state=pending` atomically | Enforced in `workitems.Claim`, NOT here. Documented for cross-reference. | n/a (lives in Claim) |
| I-4 | `impl_state=done` is required before `review_state` can be set to `approved` (the FORWARD review gate). A `review_state → needs_rework` transition is the REWORK trigger, governed by I-5 (it legitimately reverts `impl_state` `done → pending`), and is NOT blocked by I-4. | Pre-check inside the FOR UPDATE: reject if `new_review = 'approved'` AND `new_impl <> 'done'` (where `new_*` is the COALESCED value after applying any concurrent `req.impl_state` change in the same call). `needs_rework` is EXEMPT — it is not in the I-4 reject condition. Consequence: the one-call `set_state(impl_state=pending, review_state=needs_rework)` on a claimed `impl=done` item SUCCEEDS (impl→pending, review→needs_rework, qa auto-reset→pending per I-1), satisfying the §11.1.2 exit criterion; whereas `set_state(review_state=approved)` on an `impl=pending` item is REJECTED (the forward gate stands — unfinished work can never be approved, preserving the §5.7.1 `pipeline_stage` derivation). | `review_change_requires_impl_done` |
| I-5 | Transitioning `impl_state=done → pending` is allowed only via the rework path (Review NEEDS-REWORK or QA FAIL) | Pre-check: reject if `req.impl_state='pending'` AND current `impl_state='done'` AND NOT (`req.review_state='needs_rework'` OR `req.qa_state='failed'` OR (current `qa_state='failed'` AND `req.qa_state IS NULL`)) | `impl_done_to_pending_requires_rework_path` |

The implementation builds these checks as a CTE chain inside one SQL
round-trip; pseudo-shape:

```sql
WITH locked AS (
  SELECT impl_state, review_state, qa_state, pipeline_state
    FROM workitems.items
   WHERE id = $item_id
   FOR UPDATE
),
new_values AS (
  SELECT COALESCE($req_impl_state,     locked.impl_state)     AS new_impl,
         COALESCE($req_review_state,   locked.review_state)   AS new_review,
         CASE WHEN $req_review_state = 'needs_rework' THEN 'pending'
              ELSE COALESCE($req_qa_state, locked.qa_state)
         END                                                   AS new_qa, -- I-1
         COALESCE($req_pipeline_state, locked.pipeline_state) AS new_pipe
    FROM locked
),
validated AS (
  SELECT *,
         -- I-2
         (new_qa = 'failed' AND new_review <> 'approved')                               AS violates_i2,
         -- I-4 (FORWARD review gate: approved-only; needs_rework is the
         --      rework trigger, governed by I-5, and is EXEMPT here)
         (new_review = 'approved' AND new_impl <> 'done')                                AS violates_i4,
         -- I-5
         (new_impl = 'pending' AND locked.impl_state = 'done'
          AND NOT (new_review = 'needs_rework' OR new_qa = 'failed'))                   AS violates_i5
    FROM new_values, locked
)
-- the application layer reads `validated`, returns PRECONDITION_NOT_MET
-- with the matching data.invariant if any violation flag is true,
-- otherwise issues the UPDATE with the validated columns.
```

**Layer-1 BLOCK conditions (comment-trail-driven preconditions, e.g.
`qa_state → passed` requires a `(kind=qa, status=success)` comment) ship
in P02** per Plan §3.4 and PRD §8 P02 exit criterion. P01 implementation
of `set_state` writes the `intent_comment` (if present) on a best-effort
basis AFTER the state mutation commits, and does NOT verify any
comment-trail-based precondition.

**`intent_comment` partial-failure behaviour (best-effort, non-atomic —
DECISION 2026-05-18 bead unblock-tv8.21, contract activated by
unblock-tv8.63).** The handler is deliberately two-phase and
NON-transactional across the two RPCs (cross-RPC Postgres transactions
are out of P01 architectural scope):

1. `workitems.SetStateColumns` runs and **commits** the state mutation
   (within its own single-RPC transaction — the five I-1..I-5
   invariants above are still enforced atomically *inside* that RPC).
2. If `intent_comment` was supplied, the handler then calls
   `workitems.AppendComment` **best-effort**. This call is NOT part of
   the SetStateColumns transaction and CANNOT roll it back.

On AppendComment failure the tool **returns SUCCESS** — the state was
genuinely mutated and `structuredContent.item` carries the new state
columns + recomputed `pipeline_stage`. The dropped comment is surfaced
two ways, never as an error:

- **Caller-visible:** `structuredContent.warnings[]` carries exactly one
  entry `{ code: "intent_comment_dropped", message, details }` per the
  §7 success-side warnings contract. `details` echoes the
  `intent_comment_kind` and `intent_comment_status` (snake_case, §3.6) so
  the agent can decide whether to re-issue a standalone `comment` call;
  the comment **body is never echoed** into `details` (it may be large /
  sensitive — only its length + sha256 land in diagnostics, below).
- **Operator-visible:** the existing `rlog.Error` diagnostic on the
  failure path records `item_id`, `intent_comment_kind`,
  `intent_comment_status`, `intent_comment_body_sha256`, and
  `intent_comment_body_len` (NOT the body text — keeps the comment
  payload out of the observability surface) and the additive
  `mcp.tool_calls.warning_codes` audit column records
  `["intent_comment_dropped"]` per §8.1. `result_kind` STAYS `ok` on
  this path — the call succeeded; the audit widening is the warning
  column, NOT a new `result_kind` value.

This is the ONLY warning producer wired in P01/P02 today; the §7 code
registry is extensible for future cases without a result-shape change.

**Side-effects (round-6 §6.3.0 symmetric writer model — tension #3
narrow rule).** After the validating UPDATE commits, the handler
publishes `CascadeRequested{Reason:"state_change", TriggeredByItemID:
item_id, …}` ONLY when the write changes `(impl_state, review_state,
qa_state)` in a way that materially affects §5.7.1 `pipeline_stage`
derivation. Pure `pipe_state` mutations (with no change to the other
three columns) do NOT publish — §5.7.1 derives `pipeline_stage` from
the upstream chain's readiness/closure, not from a downstream item's
own `pipe_state`. The publish drives the multi-hop `pipeline_stage`
recompute on the forward `blocks` closure (Regime B). Note: this tool
writes no `is_ready`-affecting state directly; Regime A is not invoked
here.

**AR-18 (new — round-2).** State-invariant interaction with concurrent
`Claim`. Invariant I-3 is enforced in `workitems.Claim` (not in
`SetStateColumns`); a racing `SetStateColumns(qa_state=failed)` and
`Claim` on the same item could in principle observe an inconsistent
intermediate (Claim sees `qa_state=passed`, then SetStateColumns flips
it to `failed` after Claim's transaction commits — the next Claim then
sees `failed` and applies I-3). Both RPCs use `SELECT FOR UPDATE` on
the row, so the second transaction always observes the first's commit;
the architecture is correct by serialisation, not by avoidance. The
exit-criterion harness adds a property test: N=100 concurrent
`Claim`/`SetStateColumns(qa_state=failed)` interleavings on the same
item; assert that every Claim winner observes either the pre-failure
or post-failure state, never a torn read, and that whenever a Claim
observes `qa_state='failed'`, it resets both review and qa to
`pending` atomically. This invariant test is part of §11.1 acceptance
(I-1..I-5 below).

#### Tool 14 — `get_state`

```jsonc
// arguments
{
  "item_id": "<ULID>"
}

// structuredContent
{
  "project_id":     "<ULID>",   // project owning this work item (for audit correlation and client-side scoping)
  "impl_state":     "...",
  "review_state":   "...",
  "qa_state":       "...",
  "pipeline_state": "...",
  "pipeline_stage": "...",   // materialised
  "is_ready":       true,
  "claimed_by_id":  "<ULID|null>",
  "claimed_at":     "<ts|null>",
  "recent_kinds": [
    { "kind": "investigation", "status": "info",    "comment_id": "...", "created_at": "..." },
    { "kind": "decision",      "status": "info",    "comment_id": "...", "created_at": "..." },
    { "kind": "completed",     "status": "success", "comment_id": "...", "created_at": "..." }
  ]
}
```

`recent_kinds[]` returns the most recent `(kind, status)` per `kind`
(grouped, ordered by `created_at desc`, one row per kind).

#### Tool 15 — `promote` (round-16, bead `unblock-tv8.71`)

Transitions a Backlog item to Ready. This is the canonical Ready writer
that round-12 DRIFT-2 observed was missing ("the state-machine does not
allow the canonical fixture's Done/Ready end-states via RPC"). `promote`
plus the full §6.6 transition map close that gap.

```jsonc
// arguments
{
  "item_id": "<ULID>"
}

// structuredContent (success)
{
  "item": { /* Item with status=Ready */ }
}
```

**Precondition.** `status = 'Backlog' AND is_ready = true`. The item must
already be in Backlog AND have no unresolved incoming `blocks` edges
(i.e. `is_ready` is `true` — every blocker is `Done`). Recall `is_ready`
is a single-writer materialised column (§6.3.0 Regime A); `promote` READS
it and does NOT recompute it.

**Rejections (§7 error envelope).**

- Not in Backlog OR not ready → `PRECONDITION_NOT_MET` with the round-16
  `{status, required}` extension (defined once in §7): `data.status`
  carries the item's CURRENT `status` and `data.required = "Ready"`. When
  the block is specifically "still has open blockers", the handler also
  sets `data.missing = "is_ready"` so the agent can disambiguate "wrong
  status" from "blocked". Example:
  `{ "kind": "PRECONDITION_NOT_MET", "details": { "status": "Backlog", "required": "Ready", "missing": "is_ready" } }`.
- Item not found / not visible → `NOT_FOUND`.

**Tenant gate (round-16 / bead `unblock-tv8.77`).** The handler passes
the Bearer-resolved `identity.OrgID` into `workitems.Promote` as
`CallerOrgID` (internal channel, never wire). `Promote` self-gates the
`SELECT … FOR UPDATE` row lock on `org_id = $caller`, so a foreign
`item_id` yields `NOT_FOUND` (the "not visible" case above) BEFORE the
Backlog/`is_ready` precondition is evaluated, never a cross-tenant
promotion. See §10.1.

**Transition performed.** Inside a single `SELECT … FOR UPDATE`
transaction: re-check the precondition against the locked row, then
`UPDATE workitems.items SET status = 'Ready' WHERE id = $item_id`.
`promote` writes NO state-dimension columns (`impl_state` etc.) and
does NOT touch `is_ready` or `claimed_by_*`.

**Side-effects.** None on the cascade subsystem: moving an item
Backlog→Ready does not change any OTHER item's `is_ready` (it has no
effect on items that depend on it — `is_ready` of a dependent flips only
when ITS blocker becomes `Done`, not merely Ready) and does not change
§5.7.1 `pipeline_stage` derivation inputs. `promote` therefore publishes
no `CascadeRequested` and is not a Regime A `is_ready` writer.

#### Tool 16 — `create_milestone` (round-16, bead `unblock-tv8.74`)

Thin MCP facade over `workitems.CreateMilestone` (§4.4.1). Org- or
project-scoped (XOR). Org scoping is enforced by the Bearer-resolved
org-scoped `Identity` — the handler resolves the caller via
`withIdentityFromReq` and passes `identity.OrgID` into the backing RPC
as `CallerOrgID` (the internal-channel field, NEVER a wire argument),
matching every other write tool on the surface. The backing
`workitems.CreateMilestone` self-gates its parent-read seam on
`CallerOrgID` (round-16 / bead `unblock-tv8.77`): when
`parent_milestone_id` is supplied the parent row must satisfy
`org_id = $caller OR project_id IN (SELECT id FROM org.projects WHERE
org_id = $caller)`, so a foreign parent ULID yields `NOT_FOUND`, never a
cross-tenant M-INV-3/5 read leak (see the auth-model doc-comment at
`apps/api/workitems/workitems.go`). The backing `workitems.CreateMilestone`
ALSO gates the `project_id` scope selector on `CallerOrgID` (bead
`unblock-tv8.83`): on the project-scoped branch a project-scoped milestone's
`project_id` must belong to the caller's org — the guarded `INSERT … SELECT`
requires `project_id IN (SELECT id FROM org.projects WHERE org_id = $caller)`,
so a foreign-but-existing `project_id` yields `NOT_FOUND` with nothing
inserted (the org-scoped branch carries no project predicate). Enforces
M-INV-1/2/3/5/6 per the backing RPC.

```jsonc
// arguments — mirrors workitems.CreateMilestoneRequest (NO wire org_id:
// the org is pinned from the Bearer-resolved identity.OrgID; pass
// project_id to project-scope, omit it to org-scope)
{
  "project_id": "<ULID; optional — omit to org-scope to the caller's org>",
  "parent_milestone_id": "<ULID; optional>",
  "name": "Q1",
  "description": "...",
  "start_date": "2026-01-01",   // ISO date
  "end_date":   "2026-03-31"    // ISO date; >= start_date
}

// structuredContent
{ "milestone": { /* Milestone (§4.4.1) */ } }
```

Invariant violations surface as `PRECONDITION_NOT_MET` with
`data.invariant` (`"M-INV-2"` … `"M-INV-6"`) per §4.4.1; scope/date CHECK
failures surface as `VALIDATION`.

#### Tool 17 — `update_milestone` (round-16, bead `unblock-tv8.74`)

Thin MCP facade over `workitems.UpdateMilestone` (§4.4.1). Updates name,
description, start/end dates, and cancellation. **Reparenting is rejected**
in P01 (`VALIDATION`) exactly as the backing RPC specifies. Org scoping
is enforced by the Bearer-resolved org-scoped `Identity` (`identity.OrgID`
via `withIdentityFromReq`), passed to the backing RPC as `CallerOrgID`
(internal channel, never wire). `UpdateMilestone` applies a row-level
tenant predicate (round-16 / bead `unblock-tv8.77`): the targeted
milestone's `org_id = $caller OR project_id IN (SELECT id FROM
org.projects WHERE org_id = $caller)`, so a foreign `milestone_id` yields
`NOT_FOUND`, never a cross-tenant mutation.

```jsonc
// arguments — mirrors workitems.UpdateMilestoneRequest
{
  "milestone_id": "<ULID>",
  "name": "<string; optional>",
  "description": "<string; optional>",
  "start_date": "<ISO date; optional>",
  "end_date": "<ISO date; optional>",
  "cancelled_at": "<ts; optional>",
  "cancelled_reason": "<string; optional>"
}

// structuredContent
{ "milestone": { /* Milestone */ } }
```

#### Tool 18 — `assign_item` (round-16, bead `unblock-tv8.74`)

Thin MCP facade over `workitems.AssignItem` (§4.4.1). Assigns a work item
to a milestone, or **unassigns** when `milestone_id` is the empty string
(clears `milestone_id` + `milestone_assigned_at` + `milestone_assigned_by`).
Enforces M-INV-7. Org scoping is enforced by the Bearer-resolved
org-scoped `Identity` (`identity.OrgID` via `withIdentityFromReq`),
passed to the backing RPC as `CallerOrgID` (internal channel, never
wire). `AssignItem` applies a row-level tenant predicate on the target
item (round-16 / bead `unblock-tv8.77`): the item's `org_id = $caller`,
so a foreign `item_id` yields `NOT_FOUND`, never a cross-tenant
milestone assignment. The assign-branch milestone read is ALSO
`CallerOrgID`-gated (round-16 / bead `unblock-tv8.77`) with the
org-XOR-project milestone predicate (`org_id = $caller OR project_id IN
(SELECT id FROM org.projects WHERE org_id = $caller)`), so a foreign
`milestone_id` yields `NOT_FOUND` before the M-INV-7 check — never
disclosing the milestone's existence via `PRECONDITION_NOT_MET`.

```jsonc
// arguments — mirrors workitems.AssignItemRequest
{
  "item_id": "<ULID>",
  "milestone_id": "<ULID; empty string = unassign>"
}

// structuredContent
{ "assigned": true, "item_id": "<ULID>", "milestone_id": "<ULID|null>" }
```

`assigned_by_user` is taken from the caller's resolved `Identity`
(not a client-supplied argument). M-INV-7 violation →
`PRECONDITION_NOT_MET` with `data.invariant = "M-INV-7"`.

#### Tool 19 — `milestone_tree` (round-16, bead `unblock-tv8.74`)

Thin MCP facade over `workitems.MilestoneTree` (§4.4.1). Returns the
recursive milestone tree (depth bounded at M-INV-6 = 4). Read-side org
scoping is enforced by the Bearer-resolved org-scoped `Identity` — the
handler ALWAYS passes `identity.OrgID` into the backing RPC as
`CallerOrgID` (internal channel, never wire). Unlike the
`rbac.For` read RPCs (Get / GetTrail / List / Search), `MilestoneTree`
gates via an EXPLICIT tenant predicate in the recursive-CTE anchor
(`org_id = $caller OR project_id IN (SELECT id FROM org.projects
WHERE org_id = $caller)`, `apps/api/workitems/workitems.go` ~2691),
so a foreign `root_milestone_id` yields an empty anchor (no rows) —
closing the cross-tenant / IDOR read seam. The empty-`CallerOrgID` no-op
branch (the predicate degrades to `($caller = '' OR <predicate>)`) is
reserved for trusted internal callers (the §11.1.1 E2E seed, the P05
roadmap RPC) and is never reachable from the MCP boundary — see the
no-op-vs-hard-guard ratification under §10.1.

```jsonc
// arguments — mirrors workitems.MilestoneTreeRequest (NO wire org_id:
// the org is pinned from the Bearer-resolved identity.OrgID; pass
// project_id to scope to a project, or root_milestone_id for a subtree)
{
  "project_id": "<ULID; optional — scopes roots to a project>",
  "root_milestone_id": "<ULID; optional — returns the subtree rooted here>",
  "include_cancelled": false
}

// structuredContent
{ "roots": [ /* MilestoneNode[] (§4.4.1) */ ] }
```

#### Tool 20 — `create_label` (round-16, bead `unblock-tv8.75`)

Label-registry management over the existing `workitems.labels` table
(PRD §6.4). Org- or project-scoped (XOR, enforced by the existing
`labels_scope_xor_chk` CHECK). Org scoping is enforced by the
Bearer-resolved org-scoped `Identity` — the handler resolves the caller
via `withIdentityFromReq` and passes `identity.OrgID` into the backing
RPC as `CallerOrgID` (internal channel, never wire), matching every
other write tool on the surface. `CreateLabel` self-gates the
project-scoped insert on `CallerOrgID`
(`project_id IN (SELECT id FROM org.projects WHERE org_id = $caller)`,
so a Bearer for org A cannot create a label inside org B's project) and
hard-rejects an empty `CallerOrgID` with `InvalidArgument` (round-16 /
bead `unblock-tv8.77` — the label RPCs are MCP-only callers, so the
no-op branch is incorrect here; this closes the deferred-epic RISK and
makes `CreateLabel` consistent with `UpdateLabel` / `DeleteLabel`). See
the auth-model doc-comment at `apps/api/workitems/workitems.go:28-66`.
Backed by the private RPC `workitems.CreateLabel`.

```jsonc
// arguments — NO wire org_id: the org is pinned from the Bearer-resolved
// identity.OrgID; pass project_id to project-scope, omit it to org-scope
// to the caller's org
{
  "project_id": "<ULID; optional — omit to org-scope to the caller's org>",
  "name": "bug",            // 1..64 chars; unique within scope
  "color": "#d73a4a",       // hex color; validated #RRGGBB
  "description": "<string; optional>"
}

// structuredContent
{ "label": { "id": "<ULID>", "org_id": "<ULID|null>", "project_id": "<ULID|null>", "name": "bug", "color": "#d73a4a", "description": "..." } }
```

A duplicate `name` within the same scope → `CONFLICT` with
`data.constraint` naming the UNIQUE index; malformed color/name →
`VALIDATION`.

#### Tool 21 — `list_labels` (round-16, bead `unblock-tv8.75`)

Lists labels visible to the caller within a scope. Project labels and the
org labels reachable from that project are both returned; the PRD §6.4
"project wins on identical name" resolution is applied at query time.
Read-side org scoping is enforced by the Bearer-resolved org-scoped
`Identity` — the handler ALWAYS passes `identity.OrgID` into the backing
RPC. Unlike the `rbac.For` read RPCs (Get / GetTrail / List / Search),
`ListLabels` gates via an EXPLICIT tenant predicate in raw SQL
(`org_id = $caller_org`, plus the project-wins-on-identical-name
`UNION ALL` resolution that `rbac.For` cannot express) — the
project-wins UNION ALL is why this RPC deviates from the `rbac.For`
read-side convention, the same justified-deviation precedent as Tool 19
(`milestone_tree`), so a foreign `project_id` yields only the caller's
own labels (no cross-tenant rows). Backed by `workitems.ListLabels`.

```jsonc
// arguments — NO wire org_id: the read RPC gates to the caller's org via
// an explicit tenant predicate (org_id = identity.OrgID); pass project_id
// to scope within that org to a project
{
  "project_id": "<ULID; optional — when set, returns project labels + inherited org labels>"
}

// structuredContent
{ "labels": [ /* Label objects (same shape as create_label result) */ ] }
```

#### Tool 22 — `update_label` (round-16, bead `unblock-tv8.75`)

Renames and/or recolors an existing label. Cannot change a label's scope
(`org_id` / `project_id` are immutable — a scope change is a
delete-then-create). Org scoping is enforced by the Bearer-resolved
org-scoped `Identity` (`identity.OrgID` via `withIdentityFromReq`),
matching the rest of the write-tool surface; the backing RPC also applies
a row-level tenant predicate — the targeted label's `org_id =
identity.OrgID` OR its `project_id` belongs to a project in the caller's
org — so a foreign `label_id` yields `NOT_FOUND` rather than acting
cross-tenant. A successful write bumps `workitems.labels.updated_at` (the
column added by migration `0130`, §3.2). Backed by
`workitems.UpdateLabel`.

```jsonc
// arguments
{
  "label_id": "<ULID>",
  "name": "<string; optional>",       // rename
  "color": "<#RRGGBB; optional>",     // recolor
  "description": "<string; optional>"
}

// structuredContent
{ "label": { /* updated Label */ } }
```

A rename that collides with an existing label in the same scope →
`CONFLICT`.

#### Tool 23 — `delete_label` (round-16, bead `unblock-tv8.75`)

Deletes a label from the registry. The many-to-many
`workitems.item_labels` rows referencing it are removed in the same
transaction (the existing junction-table FK is `ON DELETE CASCADE` per
SPEC §9.4.3) — deleting a label detaches it from every item; it does NOT
delete the items. Org scoping is enforced by the Bearer-resolved
org-scoped `Identity` (`identity.OrgID` via `withIdentityFromReq`),
matching the rest of the write-tool surface; the backing RPC also applies
a row-level tenant predicate — the targeted label's `org_id =
identity.OrgID` OR its `project_id` belongs to a project in the caller's
org — so a foreign `label_id` yields `NOT_FOUND` rather than acting
cross-tenant. Backed by `workitems.DeleteLabel`.

```jsonc
// arguments
{ "label_id": "<ULID>" }

// structuredContent
{ "deleted": true, "label_id": "<ULID>", "detached_item_count": 0 }
```

Label not found / not visible → `NOT_FOUND`.

> **Label private RPCs (round-16).** Tools 20–23 are backed by four new
> `workitems` private RPCs — `CreateLabel`, `ListLabels`, `UpdateLabel`,
> `DeleteLabel` — added to §4.4 in lockstep. They operate over the
> existing `workitems.labels` / `workitems.item_labels` DDL (SPEC §9.4.3).
> **One new up-only migration `0130_workitems_labels_updated_at.up.sql`**
> (§3.2, bead `unblock-tv8.75`) adds the `updated_at timestamptz NOT NULL
> DEFAULT now()` column to `workitems.labels` — the §4.4 `Label.UpdatedAt`
> field has always declared it and `UpdateLabel` bumps it on every write,
> but the original `0040_workitems.up.sql` DDL omitted it (drift DECIDED
> by Miguel 2026-06-11: ADD the column — the registry is mutable via Tool
> 22 and the other long-lived `workitems` rows all carry `updated_at`). No
> other DDL change is required for labels (the `labels` / `item_labels`
> tables already exist). Org scoping follows the established
> Bearer-Identity pattern, NOT a direct `org.Authorize` call: each
> backing RPC is dispatched with the caller's org pinned to
> `identity.OrgID` (passed RPC-side as `CallerOrgID` — internal channel,
> never wire) — write RPCs (`CreateLabel` / `UpdateLabel` /
> `DeleteLabel`) trust the org-scoped `Identity` resolved by the MCP
> handler via `withIdentityFromReq`. `CreateLabel` self-gates the
> project-scoped insert on `CallerOrgID` and hard-rejects an empty
> `CallerOrgID` with `InvalidArgument` (round-16 / bead
> `unblock-tv8.77` — MCP-only callers, so the §10.1 no-op branch is wrong
> here; consistent with `UpdateLabel` / `DeleteLabel`, closes the
> deferred-epic RISK). `UpdateLabel` / `DeleteLabel`
> additionally apply a row-level tenant predicate so a foreign
> `label_id` is `NOT_FOUND` rather than a cross-tenant mutation. The read
> RPC (`ListLabels`) does NOT use `rbac.For` — the
> project-wins-on-identical-name `UNION ALL` is not expressible via
> `rbac.For` — and instead gates via an EXPLICIT tenant predicate in raw
> SQL (`org_id = identity.OrgID`), the same justified deviation as
> `MilestoneTree` (§4.4.1 / §6.2 Tool 19) (see the auth-model doc-comment
> at `apps/api/workitems/workitems.go:28-66`). No MCP handler in P01
> calls `org.Authorize` directly.

### 6.3 Cascade subsystem (Manifesto Law 1)

#### 6.3.0 Propagation regimes (round-6 cascade-symmetry)

The cascade subsystem maintains two materialised columns on
`workitems.items`: `is_ready` (single-hop derivation; depends only on
the direct incoming `blocks` edges) and `pipeline_stage` (multi-hop
derivation; depends on the upstream chain's readiness/closure per
§5.7.1). Round-6 splits the writer responsibility along that natural
boundary.

**Regime A — `is_ready` (single-hop, writer-inline).** Every call site
that mutates a row or edge in a way that can flip `is_ready` for the
**directly** affected item recomputes `is_ready` synchronously inside
the same SQL transaction as the mutation, via the shared helper
`deps.recomputeReady(ctx, tx, item_id)`. The cascade subscriber never
writes `is_ready`. Allowed writers:

- `workitems.Create` (Tool 4) — **round-16, bead `unblock-tv8.71`:** sets
  `is_ready` INLINE at row insert. A freshly-created item with no incoming
  `blocks` edges is `is_ready=true` (no blocker can be open); a create that
  ever inlines an incoming blocker recomputes via the same `NOT EXISTS`
  predicate. Before round-16 nothing set `is_ready` on the create path and
  such items were stranded non-ready — see §6.6 for the rule and the
  corrected create doc-comment. `status` remains `Backlog` at create;
  `is_ready=true` makes the item immediately `promote`-able (Tool 15).
- `workitems.Close` (Tool 6) — recomputes `is_ready` for the closed
  item's direct `blocks` neighbours inline.
- `deps.AddEdge` (Tool 11 / §6.5 cycle-detect block) — recomputes
  `is_ready` for `to_item` inline (the new edge may now block it).
- `deps.RemoveEdge` (Tool 12) — recomputes `is_ready` for the direct
  `to_item` inline.
- `deps.recomputeReady` — the shared helper itself (internal).

**Regime B — `pipeline_stage` (multi-hop, subscriber-only).** The
cascade subscriber is the **sole writer** of `pipeline_stage`. Every
call site that materially mutates §5.7.1 derivation inputs publishes
`CascadeRequested{Reason:<kind>, TriggeredByItemID:<id>, …}` after its
transaction commits. The subscriber walks the forward `blocks` closure
from `TriggeredByItemID` and recomputes `pipeline_stage` (only) for the
affected items.

**Who emits which Reason, what the subscriber does:**

| `Reason` | Emitted by | Trigger | Subscriber behaviour |
|---|---|---|---|
| `"close"` | `workitems.Close` (Tool 6) post-commit | Status flipped to Done. | Walk forward `blocks` closure from the closed item; recompute `pipeline_stage` on every reachable item per §5.7.1. |
| `"edge_added"` | `deps.AddEdge` (Tool 11 / §6.5) post-commit | New `blocks` edge committed. | Walk forward `blocks` closure from `to_item`; recompute `pipeline_stage` (the new upstream blocker may push downstream stages backward). |
| `"edge_removed"` | `deps.RemoveEdge` (Tool 12) post-commit, with `event_id` REUSED from the inline audit row | Edge deleted. | Walk forward `blocks` closure from `to_item`; recompute `pipeline_stage` on transitively reachable items. The audit-row insert collapses to no-op via `ON CONFLICT (event_id, triggered_by_item_id) DO NOTHING` (the inline path already wrote it). |
| `"state_change"` | `workitems.SetStateColumns` (Tool 13) post-commit when the write changes `(impl_state, review_state, qa_state)` materially per §5.7.1; AND `workitems.Claim` post-commit ONLY on the I-3 reset path (current `qa_state='failed'` triggers the in-transaction reset of `review_state` and `qa_state` to `pending`). | State-column mutation that affects §5.7.1 derivation. | Walk forward `blocks` closure from the mutated item; recompute `pipeline_stage` (downstream items may transition between Implementation / Review / QA stages). |

**Explicit non-publishers (round-6 tensions, resolved).**

- `workitems.SetStateColumns` writes that affect ONLY `pipe_state`
  (with no change to `impl_state`/`review_state`/`qa_state`) do NOT
  publish — §5.7.1 derives `pipeline_stage` from the upstream chain's
  readiness/closure, not from a downstream item's own `pipe_state`
  (tension #3 ruling).
- `workitems.Claim` in the normal Ready→InProgress path (no I-3 reset
  fires) does NOT publish — the claimed item was non-Done before and
  remains non-Done; no §5.7.1 downstream re-derivation is needed
  (tension #2 ruling). Only the I-3 reset path publishes.

**Audit-row kind reuse.** `deps.cascade_events.kind` carries the same
discriminant value as the `Reason` field of the originating
`CascadeRequested`. The CHECK constraint enumerates all four kinds
(see §9.4.4 + the `0050_deps.up.sql` migration). The
`(event_id, triggered_by_item_id)` UNIQUE constraint remains the
AR-11 idempotency mechanism across both regimes.

#### 6.3.1 Pub/Sub topics

```go
package deps

import "encore.dev/pubsub"

type CascadeRequested struct {
    EventID            string // ULID, generated by publisher (C1 closure)
    OrgID              string
    ProjectID          string
    TriggeredByItemID  string
    Reason             string // "close" | "edge_added" | "edge_removed" | "state_change"
    TraceID            string // ULID minted by the mcp raw endpoint, copied from ctx into the payload at publish time (Encore Pub/Sub does not propagate context across the topic boundary). Persisted on deps.cascade_events.trace_id by the subscriber. See §10.2 Option B.
    EmittedAt          time.Time
}

type CascadeCompleted struct {
    EventID             string
    TriggeredByItemID   string
    AffectedItemIDs     []string
    CascadedCount       int
    CompletedAt         time.Time
}

var CascadeRequestedTopic = pubsub.NewTopic[*CascadeRequested]("deps.cascade.requested",
    pubsub.TopicConfig{DeliveryGuarantee: pubsub.AtLeastOnce})

var CascadeCompletedTopic = pubsub.NewTopic[*CascadeCompleted]("deps.cascade.completed",
    pubsub.TopicConfig{DeliveryGuarantee: pubsub.AtLeastOnce})
```

#### 6.3.2 Subscriber (idempotent per AR-11)

```go
var _ = pubsub.NewSubscription(CascadeRequestedTopic, "deps-cascade-subscriber",
    pubsub.SubscriptionConfig[*CascadeRequested]{
        Handler: handleCascadeRequested,
        // No retry override: Encore default backoff applies.
    })

func handleCascadeRequested(ctx context.Context, msg *CascadeRequested) error {
    // Round-6 §6.3.0: the subscriber maintains pipeline_stage ONLY
    // (Regime B). is_ready is writer-inline and never touched here.
    // Dispatch over the four documented Reason kinds — direction is
    // forward (along outgoing 'blocks' edges) in all four branches;
    // only the semantic justification differs per kind.
    switch msg.Reason {
    case "close":
        // Triggered by workitems.Close (Tool 6) post-commit. The closed
        // item's neighbours have already had is_ready flipped inline;
        // walk the forward closure to propagate pipeline_stage per
        // §5.7.1 (downstream items may now move forward stages).
    case "edge_added":
        // Triggered by deps.AddEdge (Tool 11 / §6.5) post-commit. The
        // direct to_item's is_ready has already been recomputed inline;
        // walk the forward closure from to_item — a new upstream
        // blocker can push downstream stages backward per §5.7.1.
    case "edge_removed":
        // Triggered by deps.RemoveEdge (Tool 12) post-commit, with the
        // event_id REUSED from the inline audit row (tension #1). The
        // ON CONFLICT clause on the audit INSERT below collapses the
        // second insert to no-op; the pipeline_stage recompute pass
        // still runs. is_ready was recomputed inline for the direct
        // to_item; walk the forward closure to propagate pipeline_stage.
    case "state_change":
        // Triggered by workitems.SetStateColumns (Tool 13) post-commit
        // when (impl_state, review_state, qa_state) changed materially
        // per §5.7.1, OR by workitems.Claim post-commit ONLY on the
        // I-3 reset path (tension #2 narrow rule). Walk the forward
        // closure; pipeline_stage may transition on downstream items.
    default:
        // Unknown Reason — log + drop (defensive; the publisher set
        // is closed, but a malformed redelivery should not crash the
        // subscriber).
        return nil
    }
    // Shared body across all four Reasons:
    // 1. BFS from msg.TriggeredByItemID forward along 'blocks' edges,
    //    collecting items where pipeline_stage might change. Max depth
    //    256 per AR-8.
    // 2. For each affected item, recompute pipeline_stage per §5.7.1
    //    derivation; UPDATE workitems.items SET pipeline_stage = $new
    //    WHERE id = $id AND pipeline_stage <> $new (idempotent).
    //    The subscriber MUST NOT write is_ready (Regime A invariant
    //    — see §11.3 linter rule).
    // 3. INSERT INTO deps.cascade_events (id, event_id, kind, org_id,
    //    project_id, triggered_by_item_id, affected_item_ids,
    //    cascaded_count, trace_id, ...)
    //    VALUES (..., msg.Reason, ..., msg.TraceID, ...)
    //    ON CONFLICT (event_id, triggered_by_item_id) DO NOTHING.
    //    `kind = msg.Reason` (one of 'close','edge_added',
    //    'edge_removed','state_change' — see §9.4.4 CHECK). The
    //    ON CONFLICT clause is the AR-11 idempotency mechanism (C1)
    //    AND the tension #1 mechanism for edge_removed (the inline
    //    audit row already exists with the same event_id).
    // 4. Publish CascadeCompleted with the affected set (best-effort;
    //    the subscriber's commit is the source of truth).
    return nil
}
```

`affected_item_ids` cardinality bound: in P01 the cascade walks the
forward 'blocks' closure of the triggered item (max depth 256 per AR-8).
The `(event_id, triggered_by_item_id)` UNIQUE constraint guarantees a
duplicate delivery is a no-op insert.

**`cascade_events.kind` enum (round-6).** SPEC §9.4.4 declares the
column with `CHECK (kind IN ('close','edge_added','edge_removed','state_change'))`:

- `'close'` — Tool 6 publish; subscriber writes the audit row during
  its `pipeline_stage` recompute pass.
- `'edge_added'` — Tool 11 / §6.5 publish; subscriber writes the audit
  row during its `pipeline_stage` recompute pass.
- `'edge_removed'` — Tool 12 writes the audit row INLINE in the same
  transaction as `DELETE FROM deps.dependencies`; Tool 12 also
  publishes post-commit with the SAME `event_id`, and the subscriber's
  re-insert collapses to no-op via the ON CONFLICT clause (tension #1).
  The subscriber's `pipeline_stage` recompute pass still runs.
- `'state_change'` — Tool 13 publish on §5.7.1-affecting writes; AND
  `workitems.Claim` publish on the I-3 reset path only (tensions #2
  and #3 narrow rules). Subscriber writes the audit row during its
  `pipeline_stage` recompute pass.

P01 ships all four kinds (round-6 cascade-symmetry). Subsequent phases
that introduce new cascade kinds extend the enum in their own phase
migration — adding a value is an additive CHECK rewrite.

### 6.4 Atomic claim transaction (Manifesto Law 5)

Verbatim from SPEC §5.5:

```sql
BEGIN;
  SELECT id FROM workitems.items
   WHERE id = $1 AND status = 'Ready' AND claimed_by_id IS NULL
   FOR UPDATE;
  -- if zero rows: rollback + return ErrAlreadyClaimed with winner info
  UPDATE workitems.items
     SET claimed_by_id   = $2,
         claimed_by_agent = $3,
         claimed_at      = now(),
         status          = 'InProgress'
   WHERE id = $1;
COMMIT;
```

On the loser path (zero rows from `SELECT FOR UPDATE`):

```sql
SELECT claimed_by_id, claimed_by_agent, claimed_at
  FROM workitems.items
 WHERE id = $1;
```

…and return error `ALREADY_CLAIMED` with `data.winner_user_id`,
`data.winner_agent`, `data.claimed_at`.

Pool-mode safety (R-P01-6 closure): the entire critical section lives in
one transaction, so PgBouncer transaction-mode and session-mode both
preserve the lock. The spec **does not pin** Encore Cloud's pool mode —
both modes work.

**Side-effects on I-3 path (round-6 §6.3.0 — tension #2 narrow rule).**
Normal `Claim` (Ready → InProgress with no I-3 reset) does NOT publish
any `CascadeRequested` — the claimed item was non-Done before the
claim and remains non-Done; downstream `pipeline_stage` is unaffected
per §5.7.1, and a publish would burn one cascade pass against the
NFR-1 budget for no observable effect.

`Claim` publishes `CascadeRequested{Reason:"state_change",
TriggeredByItemID: item_id, …}` **only when the I-3 reset path fires**
— that is, the locked row carried `qa_state='failed'` at the start of
the transaction, and the transaction therefore writes
`(review_state, qa_state) = ('pending', 'pending')` atomically with
the claim. In that case the state-column write materially affects
§5.7.1 derivation and the multi-hop `pipeline_stage` recompute is
required (Regime B). The publish happens after the transaction
commits; the subscriber writes one `deps.cascade_events` row with
`kind='state_change'`.

### 6.5 Cycle detection at write time (NFR-5)

Verbatim from SPEC §9.4.9, applied to every `add_dependency` call and to
every `dependencies[]` entry inside `create`:

```sql
BEGIN;
  -- AF5: per-project advisory lock serialises concurrent edge writes
  SELECT pg_advisory_xact_lock(hashtext('deps.add_dependency:' || $project_id));

  -- C5: depth-counter recursive CTE (LIMIT inside recursive term is
  -- undocumented PG behaviour; depth counter is the standard pattern)
  WITH RECURSIVE reachable(id, depth) AS (
      SELECT $2::text, 0
      UNION ALL
      SELECT d.to_item, r.depth + 1
        FROM deps.dependencies d
        JOIN reachable r ON d.from_item = r.id
       WHERE d.kind = 'blocks'
         AND r.depth < 256
  )
  SELECT 1 FROM reachable WHERE id = $1 LIMIT 1;
  -- If a row is returned: cycle would be created; rollback + reject with
  -- CYCLE_DETECTED. Optionally INSERT INTO deps.cycles for forensics.

  INSERT INTO deps.dependencies (id, from_item, to_item, kind, ...)
  VALUES ($edge_id, $1, $2, $kind, ...);

  -- Re-evaluate readiness of the newly-blocked to_item (it may now be
  -- non-ready if the from_item is not Done):
  UPDATE workitems.items
     SET is_ready = (
       NOT EXISTS (
         SELECT 1 FROM deps.dependencies d2
           JOIN workitems.items i ON i.id = d2.from_item
          WHERE d2.to_item = $2 AND d2.kind = 'blocks' AND i.status <> 'Done'
       )
     )
   WHERE id = $2;

COMMIT;

-- Round-6 §6.3.0: post-commit publish drives the multi-hop
-- pipeline_stage recompute on the forward closure. The inline UPDATE
-- above is single-hop (is_ready on to_item only — Regime A).
deps.CascadeRequestedTopic.Publish(ctx, &deps.CascadeRequested{
    EventID:           ulid.New(),
    OrgID:             $org_id,
    ProjectID:         $project_id,
    TriggeredByItemID: $to_item_id,
    Reason:            "edge_added",
    TraceID:           tracectx.From(ctx),
    EmittedAt:         time.Now(),
})
```

The 256 cap is a v1.0 product constraint (RP01-3 risk in plan §7); error
envelope on overflow includes the offending chain prefix.

### 6.6 Status transition map (round-16, bead `unblock-tv8.71`)

`workitems.items.Status` (§6.1 enum: `Backlog`, `Ready`, `InProgress`,
`Blocked`, `Done`) is governed by the transition map below. Round-12
DRIFT-2 recorded that P01 had **no Ready writer** — nothing moved an item
into `Ready` via RPC, so the canonical fixture's Ready end-states were
unreachable through the API. `promote` (Tool 15) is that writer; this map
makes the full lifecycle explicit and closes DRIFT-2.

| From | To | Trigger (writer) | Precondition | Notes |
|---|---|---|---|---|
| (create) | `Backlog` | `workitems.Create` (Tool 4) | — | Default landing status for a new item. `is_ready` is set inline at create per the rule below. |
| `Backlog` | `Ready` | `promote` (Tool 15) | `status='Backlog' AND is_ready=true` | The new Ready writer. Rejects with §7 `PRECONDITION_NOT_MET {status, required:'Ready'}` otherwise. |
| `Ready` | `InProgress` | `claim` (Tool 3) / `workitems.Claim` | `status='Ready' AND claimed_by_id IS NULL` | Atomic `SELECT FOR UPDATE` per §6.4. Loser → `ALREADY_CLAIMED`. |
| `Ready` | `Blocked` | `add_dependency` (Tool 11) / `deps.AddEdge` | a new incoming `blocks` edge whose `from_item` is not `Done` is committed against a `Ready` item | **Demotion (`Ready`-only).** The §6.5 inline `is_ready` recompute flips `is_ready=false`. The demotion writer sets `status='Blocked'` ONLY when the affected item was `Ready` (unclaimed); for an `InProgress` (claimed) item it writes `is_ready=false` and LEAVES `status='InProgress'` untouched (see the note below the table). This is the inverse of `promote`: a dependency added to a Ready item demotes it. |
| `Blocked` | `Ready` | cascade subscriber → inline `is_ready` recompute on the blocker's `close`/`remove_dependency` | every incoming `blocks` blocker is `Done` (`is_ready` flips back to `true`) | When the last blocker closes (Tool 6) or its edge is removed (Tool 12), the Regime A inline recompute (§6.3.0) flips `is_ready=true`; an item that was `Blocked` and is not yet claimed returns to `Ready` in the same write. A claimed item is never `Blocked` (see the note below), so there is no claim to preserve here; an already-`InProgress`/`Done` item is NOT re-promoted. |
| `InProgress` | `Done` | `close` (Tool 6) / `workitems.Close` | P01: `claimed_by_id IS NOT NULL` (AF3). P02 tightens to `qa_state='passed'`. | Fires the cascade (§6.3.0 Regime A inline + Regime B publish). |

**`InProgress` is never demoted (round-16, DECISION by Miguel).** Demotion
to `Blocked` applies ONLY to `Ready` (unclaimed) items. When an
`InProgress` (claimed) item gains a new unmet incoming `blocks` edge, the
§6.5 inline recompute flips `is_ready=false` but the item keeps
`status='InProgress'` — `status` is NOT changed and the claimant stays on
the item so they can resolve the blocker. There is no `InProgress→Blocked`
transition. When the blocker later resolves, the Regime A inline recompute
flips `is_ready=true` and the item is simply still `InProgress` (no
transition needed). Consequently a claimed item is NEVER `Blocked`, so no
"Blocked-with-claim" state exists and the `Blocked→Ready` recovery row
above never has a claim to retain.

**`is_ready`-on-create rule (round-16, bead `unblock-tv8.71`).** Today
`deps.recomputeReady` is invoked only by `close` / `add_dependency` /
`remove_dependency` (§6.3.0 Regime A allow-list), and the cascade
subscriber maintains `pipeline_stage` only — so a freshly-created item
with NO incoming `blocks` edges never has its `is_ready` set by any path
and is stranded at the column default. P01's create doc-comment implying
"readiness is materialised by the cascade subscriber" is **misleading for
the create case** and is corrected.

Pinned rule: `workitems.Create` (Tool 4) MUST set `is_ready` INLINE,
inside its own transaction, at the moment the item row is inserted:

- If the new item is created with NO incoming `blocks` dependencies (the
  `dependencies[]` argument contains no edge where the new item is the
  `to_item`, which in P01 it never does — `create`'s inline edges make the
  NEW item the `from_item`/blocker side per §4.4 `DependencyEdge`), then
  `is_ready = true` is written inline.
- If a future create path ever inlines an incoming `blocks` edge, the same
  inline recompute (`NOT EXISTS (open blocker)`, identical to the §6.5
  UPDATE predicate) decides `is_ready`.

This makes `workitems.Create` a **Regime A `is_ready` writer**:
§6.3.0's Regime A allow-list and §11.3's `no_direct_is_ready_write`
allow-list are both extended to include `workitems.Create` (in lockstep —
see those sections). The status at create remains `Backlog`; `is_ready`
may be `true` (no blockers) while `status='Backlog'`, which is exactly the
`promote` precondition — a created item with no blockers is immediately
`promote`-able. `create` does NOT itself set `status='Ready'`; promotion
is an explicit agent action (Tool 15) so the agent controls when a Backlog
item enters the ready queue.

**Relationship to `set_state` / `pipeline_state`.** This map governs the
`Status` enum (the ready-queue dimension). It is orthogonal to the three
PRD §6.2 state dimensions (`impl_state` / `review_state` / `qa_state`) and
the `pipeline_state` exception column, which are governed by §6.2 Tool 13
(`set_state`) and its I-1..I-5 invariants. `promote` writes ONLY `status`;
`set_state` never writes `status`.

---

## 7. Error Envelope (locked)

> **Wire convention** (cross-ref §3.6): every JSON key in this section is
> snake_case. §3.6 generalises the same convention to the private Encore
> RPC surface and Pub/Sub payloads; the envelope quoted here remains the
> authoritative lock for MCP tool errors.

All MCP tool errors return a JSON-RPC 2.0 error object:

```jsonc
{
  "jsonrpc": "2.0",
  "id": "<echo>",
  "error": {
    "code": -32000,             // JSON-RPC reserved range; we always use -32000 for "tool error"
    "message": "<one-line human-readable>",
    "data": {
      "kind": "<MACHINE_CODE>",   // see table below
      "tool": "claim",
      "trace_id": "<ULID>",
      "details": { /* kind-specific */ }
    }
  }
}
```

| `kind` | Meaning | `details` shape |
|---|---|---|
| `UNAUTHENTICATED` | Bearer missing / invalid / revoked / expired | `{}` |
| `FORBIDDEN` | Authenticated, but `org.Authorize` denies | `{ "resource": "...", "action": "..." }` |
| `NOT_FOUND` | Subject id does not exist or not visible to caller | `{ "kind": "item", "id": "..." }` |
| `VALIDATION` | Argument shape / type / range violation — missing required argument, invalid enum value, wrong type, OR out-of-range numeric bound (§7.3) | `{ "field": "title", "reason": "must be 1..200 chars" }` or (range) `{ "field": "limit", "reason": "out of range", "bound": "1..200" }` |
| `ALREADY_CLAIMED` | `claim` loser path | `{ "winner_user_id": "...", "winner_agent": "...", "claimed_at": "..." }` |
| `CYCLE_DETECTED` | `add_dependency` / `create` cycle reject | `{ "from": "...", "to": "...", "cycle_path": ["...", "..."] }` |
| `PRECONDITION_NOT_MET` | Structural precondition violated (P01) or BLOCK condition (P02+) | `{ "missing": "claimed_by_id" }` or `{ "rejection_reason": "..." }` or (round-16) `{ "status": "Backlog", "required": "Ready" }` — see the §7.2 status-precondition extension |
| `CONFLICT` | Optimistic concurrency or unique constraint violation | `{ "constraint": "<name>" }` |
| `INTERNAL` | Unhandled server error (logged with full trace_id) | `{}` |

`trace_id` is the ULID minted by the `mcp` raw endpoint at request
entry (§10.2 Option B). It is stored verbatim in
`mcp.tool_calls.trace_id`, embedded in the Pub/Sub payload
`CascadeRequested.TraceID` (Encore Pub/Sub does not carry
`context.Context` across the topic boundary — the publisher copies the
id into the message explicitly), and re-emitted as the
`trace_id` structured field on every JSON-Lines log line for
correlation. Encore's runtime trace id is observability-only and is
not surfaced here.

### 7.1 Success-side warnings (locked — added unblock-tv8.63)

> **(locked)** This subsection is an intentional, rationale-backed
> *addition* to the §7 contract; it does not relax any existing error
> rule. Origin: bead unblock-tv8.63 (REVIEW SUGGESTION[semantic] §2 on
> unblock-tv8.21). It exists because a tool can fully **succeed** in its
> primary mutation yet leave a non-fatal residue (the dropped
> `intent_comment` on `set_state`) that the caller deserves to observe
> without it being modelled as an error. Modelling it as a §7 error
> would be semantically wrong (the call succeeded) and would force the
> agent down the failure path; widening `result_kind` would be a
> breaking audit-schema change for the same wrong reason. A typed,
> optional `warnings` array on the **success** result is the correct
> home. Labelled a **P02-activated contract addition** even though it
> patches the P01 spec (per unblock-tv8.63 AC#1) — the contract lives
> where §6.2/§7 live, and the one wired producer is `set_state`.

A successful MCP tool result MAY carry an optional `warnings` array
**inside `structuredContent`** (NOT in `_meta`, NOT as a top-level
`CallToolResult` sibling — go-sdk v1.6.0's `CallToolResult` does not
expose a custom top-level slot, and jsonschema-go infers
`additionalProperties: false`, so a warning channel MUST be a declared
field of the tool's typed Out struct). When no warnings are present the
field is **omitted entirely** (`omitempty`):

```jsonc
// structuredContent (success)  — example: set_state with a dropped intent_comment
{
  "item": { /* tool-specific success payload */ },
  "warnings": [                         // optional; omitempty when empty
    {
      "code": "intent_comment_dropped", // see registry below
      "message": "state mutation committed; intent_comment append failed and was dropped",
      "details": {                      // optional; snake_case (§3.6); shape is per-code
        "intent_comment_kind": "completed",
        "intent_comment_status": "success"
      }
    }
  ]
}
```

**Warning object** (every key snake_case per §3.6):

| field | type | required | meaning |
|---|---|---|---|
| `code` | string (enum, registry below) | yes | machine-stable warning identifier |
| `message` | string | yes | one-line human-readable summary |
| `details` | object (snake_case keys) | optional (`omitempty`) | code-specific structured context; shape defined per code; never carries large/sensitive payloads (e.g. comment bodies) |

**Warning `code` registry** — extensible; a code is listed ONLY once a
producer exists (no speculative codes):

| `code` | Emitted by | Condition | `details` shape |
|---|---|---|---|
| `intent_comment_dropped` | Tool 13 `set_state` (§6.2) | State mutation committed but the best-effort `intent_comment` AppendComment failed (DECISION 2026-05-18, unblock-tv8.21) | `{ "intent_comment_kind": "<kind>", "intent_comment_status": "<status>" }` — kind/status only; the comment body is excluded (length + sha256 go to rlog diagnostics, never the wire) |

Future P02 cases (e.g. a `cascade_delayed` warning) attach here by
adding a registry row plus an Out-struct producer; no result-shape
change is required.

**Implementation shape (pinned decision — shared embedded struct, one
wired producer).** Two alternatives were weighed:

- *(A) Per-tool `Warnings` field re-declared on each Out struct.* Rejected:
  duplicates the Warning object definition N times and invites drift in
  the json tags / omitempty behaviour across tools.
- *(B, PINNED) A single shared `WithWarnings` struct embedded into the
  Out structs that can emit warnings.* One canonical
  `type Warning struct { Code string \`json:"code"\`; Message string \`json:"message"\`; Details map[string]any \`json:"details,omitempty"\` }`
  and `type WithWarnings struct { Warnings []Warning \`json:"warnings,omitempty"\` }`,
  embedded into `setStateOut`. jsonschema-go promotes the embedded
  field into the tool's output schema as a sibling of `item`, preserving
  the single-object shape and `additionalProperties:false`. Only
  `setStateOut` embeds it in P01/P02 (one wired producer); the type is
  reusable for the next producer with zero re-definition. Cross-ref §3.6
  (snake_case) and §8.1 (audit column).

### 7.2 `PRECONDITION_NOT_MET` status extension (locked — round-16, owned by `unblock-tv8.71`)

> **(locked, additive)** This subsection ADDS an optional `data` shape to
> the existing `PRECONDITION_NOT_MET` kind (§7 table). It does not change
> any existing shape — `{ "missing": … }` and `{ "rejection_reason": … }`
> remain valid verbatim. The extension is defined ONCE here and reused by
> every status-precondition rejection.

When a tool rejects because the subject item is in the WRONG `Status`
(§6.1 enum) for the requested operation, the `data.details` object carries
two additional optional fields. They live INSIDE `data.details` (the
locked §7 base-table lists `{ "status": "Backlog", "required": "Ready" }`
in the `details` shape column), alongside the existing `missing` /
`rejection_reason` keys — NOT as siblings of `kind` / `tool` / `trace_id`:

```jsonc
{
  "kind": "PRECONDITION_NOT_MET",
  "tool": "promote",
  "trace_id": "<ULID>",
  "details": {
    "status":   "Backlog",   // the item's CURRENT Status enum value
    "required": "Ready",     // the Status the operation requires
    "missing":  "is_ready"   // OPTIONAL; present when the blocker is specifically an unmet readiness/structural precondition
  }
}
```

- **`status`** — the item's current `Status` at the moment of rejection.
- **`required`** — the `Status` (or readiness condition) the operation
  demanded. For `promote`, `required = "Ready"`.
- **`missing`** — OPTIONAL, additive to the existing `{missing}` shape;
  when present it names the specific unmet structural precondition
  (e.g. `"is_ready"` when a Backlog item still has open blockers, or
  `"claimed_by_id"` for the §6.2 Tool 6 close precondition).

`data.details.status` and `data.details.required` are present together or
not at all. A rejection that is purely structural (e.g. close's
`claimed_by_id IS NULL`) MAY omit `status`/`required` and carry only
`missing`, exactly as today.

**Owned by `promote` (Tool 15 / `unblock-tv8.71`); reused by:**

- **`claim` (Tool 3) — bead `unblock-tv8.72`.** The
  `claim`-on-not-Ready rejection (when an item is targeted by `claim`
  but its `Status <> 'Ready'`, distinct from the `ALREADY_CLAIMED`
  loser path which still uses its own kind) MUST emit
  `PRECONDITION_NOT_MET` with `{ status: <current>, required: 'Ready' }`.
  `unblock-tv8.72` carries no separate spec text — its error contract IS
  this extension. (The `ALREADY_CLAIMED` kind continues to cover the
  concurrent-claim loser path unchanged; the status extension covers the
  "item never was Ready" path.)
- **Milestone tools (Tools 16–19 / `unblock-tv8.74`)** — invariant
  rejections continue to use `data.invariant` (§4.4.1); any status-shaped
  precondition reuses this same `{status, required}` shape.
- **`show` (Tool 7 / `unblock-tv8.76`)** — no new rejection kind; resolved
  references that are not visible are omitted (RBAC), not error-flagged.

This keeps the taxonomy consistent across `promote` and `claim`: a
machine reading `data.required = "Ready"` handles both identically.

### 7.3 Uniform argument validation at the MCP boundary (locked — bead `unblock-tv8.82`)

> **(locked, additive)** This subsection pins a UNIFORM argument-validation
> contract across the entire 23-tool MCP surface. It is an additive
> boundary-contract extension — it does not re-architect any tool, change any
> DDL, or alter any success-path payload. Origin: bead `unblock-tv8.82`,
> discovered-from the live MCP sweep (B2+B3). Decisions LOCKED by Miguel
> (2026-06-12).

**The contract.** EVERY argument-shape violation at the MCP boundary —

1. a **missing required** argument,
2. an **invalid enum** value (a string outside a closed value set, e.g.
   `priority_min` outside `"P0".."P4"`, `status[]` / `pipeline_stage[]`
   outside their §6.1 enums, `kind` / `status` on `comment` outside §6.5),
3. a **wrong type** (e.g. a string where a number is required, an object
   where an array is required), AND
4. an **out-of-range numeric bound** (a paginated `limit` / `ready_limit`
   below the minimum or above the maximum — §7.3.1)

— MUST surface as the §7 `VALIDATION` envelope, uniformly, **before** the
handler's domain logic runs. The envelope carries `kind = "VALIDATION"`, the
`trace_id` (§10.2), and `data.field` naming the offending argument; for a
range violation it additionally carries `data.reason` and `data.bound` (the
advertised `min..max`, e.g. `"1..200"`). No argument violation may surface as
a bare `isError` text frame (`CallToolResult{isError:true, content:[text]}`)
with no §7 envelope.

**§7.3.1 Bounds are ENFORCED — out-of-range REJECTS (behavior change).** The
paginated tools advertise an inclusive `[minimum, maximum]` on their `limit`
argument (§6.2.0a): `prime.ready_limit` ∈ `1..50`, `ready.limit` ∈ `1..200`,
`list.limit` ∈ `1..200`, `search.limit` ∈ `1..100`. A value below the minimum
(including `0`, negative, or absent-as-zero when supplied explicitly) OR above
the maximum is **REJECTED** with `VALIDATION` (`data.field` = the limit
argument, `data.bound` = the advertised range). The server does **NOT**
silently clamp-to-max or coerce-to-default an out-of-range value. This is a
deliberate **behavior change** from the round-7 pagination semantics, where
`limit <= 0` coerced to the per-tool default and `prime.ready_limit > 50`
clamped to 50: under this contract **both reject**. Any round-7 handler
doc-comment, prose, or in-code comment that describes coerce-to-default or
clamp-to-max limit semantics is **RE-LOCKED to the reject semantics** by this
subsection — the reject contract is now authoritative wherever the two
disagree. (An OMITTED optional `limit`/`ready_limit` still takes the per-tool
default — the bound check applies only to a value the caller actually
supplies; absence is not a zero.)

**§7.3.2 Mechanism (stated at contract altitude, not line-by-line).** The
live `tools/list` schema is REFLECTED by the go-sdk (v1.6.0,
`jsonschema.ForType`) from each handler's Go input struct; by default it
carries only `type`, `required` (absence of `,omitempty`), and
`additionalProperties:false` — it has NEVER carried `enum` or
`minimum`/`maximum`, so the advertised bounds/enums were never on the wire
(see §10.3 / §6.2.0a). The go-sdk validates the REGISTERED schema PRE-handler
(`applySchema` → `resolved.Validate` before the typed handler runs); a
PRE-handler failure returns a bare `isError` text frame with **no** §7
envelope — and §7 envelopes can only be minted from INSIDE the handler (via
the existing `apps/api/mcp/errmap.go` `mapError` path, which maps
`errs.InvalidArgument` → `VALIDATION` with `data.field`). Therefore, to make
every argument violation §7-shaped:

- the **registered** (SDK-validated) input schema is **RELAXED** on exactly
  the keywords the shared boundary-validation layer owns (so `applySchema`
  does not pre-reject any of the four violation classes with a bare frame),
  while
- the advertised **`tools/list`** schema is **ENRICHED** to the full rich
  contract — `enum`, `minimum`/`maximum`, and `required[]` — for agent
  discovery (§6.2.0a), and
- a **shared `validateArgs` pass at the handler boundary** validates
  required / enum / type / range against the contract and mints the §7
  `VALIDATION` envelope via the existing `mapError` path.

This is the precedent already shipped by `apps/api/mcp/handler_update.go`
(register a relaxed `InputSchema` so `applySchema` does not pre-reject, then
validate the raw args at handler-top and mint §7 via `mapError`). The
mechanism is documented here at CONTRACT altitude; the per-tool wiring,
the shared-layer implementation, and tests are owned by Greta (the
implementation bead).

---

## 8. Observability and Audit

### 8.1 `mcp.tool_calls` (ships in `0070_mcp.up.sql`)

Every MCP tool dispatch writes one row at request end:

```go
func recordToolCall(ctx context.Context, call ToolCall) {
    db.Exec(ctx, `
      INSERT INTO mcp.tool_calls
        (id, api_key_id, org_id, project_id, item_id, tool_name,
         arguments, result_kind, rejection_reason, error_code,
         warning_codes, duration_ms, trace_id, called_at)
      VALUES (...)`, ...)   // warning_codes: jsonb array, default [] (§8.1.1)
}
```

`result_kind` ∈ `{ok, rejected, error}`; `rejection_reason` populated on
PRECONDITION_NOT_MET (canonically named for analysis). `trace_id` is the
ULID minted by `MCPHandler` at request entry (§10.2 Option B), pulled
from `ctx` by `recordToolCall`. The `mcp.tool_calls.trace_id` DDL column
is `text` (frozen in `0070_mcp.up.sql`) and accepts the ULID string
verbatim — no schema change required.

#### 8.1.1 `warning_codes` audit column (additive — added unblock-tv8.63)

The success-side warnings of §7.1 are audited via a new **additive**
column on `mcp.tool_calls`:

| column | type | nullable | default | stores |
|---|---|---|---|---|
| `warning_codes` | `jsonb` | NOT NULL | `'[]'::jsonb` | JSON array of the `code` strings present on the tool's success-result `warnings[]` (empty array when none) |

**Type — `jsonb`, NOT `text` (pinned).** Rationale: it is a true list
(0..N codes), it is queryable for the FR-9 rejection/quality analytics
(`WHERE warning_codes @> '["intent_comment_dropped"]'`, GIN-indexable —
matching the existing `arguments` jsonb + `tool_calls_arguments_gin_idx`
precedent), and `jsonb` normalises/validates the array on write. A `text`
column would force string-matching and lose array semantics. A separate
`mcp.tool_call_warnings` child table was considered and rejected as
over-engineered for a bounded enum that is read in aggregate (the parent
row already has the org/tool/trace correlation keys).

**What it stores on the partial-failure path.** On `set_state` with a
dropped `intent_comment`, `result_kind` STAYS `'ok'` (the call
succeeded — NEVER widen the `result_kind` enum / CHECK) and
`warning_codes` is `["intent_comment_dropped"]`. On every other
call (no warnings) it is the default `[]`.

**How `recordToolCall` populates it.** The handler sets a
`WarningCodes []string` field on the `ToolCall` record (alongside the
existing `ResultKind`, `RejectionReason`, `ErrorCode`); `set_state`
appends `"intent_comment_dropped"` to it on the AppendComment-failure
branch (the same branch that already emits the rlog diagnostic and the
§7.1 `warnings[]`). `recordToolCall` marshals the slice to jsonb in the
INSERT (mirroring the `arguments` jsonb handling); a nil/empty slice
serialises to `'[]'`. The INSERT column list in §8.1 gains
`warning_codes`.

**Migration — NEW sequential migration `0110_mcp_warning_codes.up.sql`
(pinned), NOT an amend of `0070`.** Even though pre-production permits
breaking changes (no users, no migration tax), `apps/api/db/migrations/`
is append-only sequential and `0070` already shipped and ran in CI /
local clusters; editing an applied migration in place desyncs any
environment that already ran it and violates the single-migration-owner
append discipline (`apps/api/db/` owns all migrations). The new migration
takes the next sequential 10-step number ABOVE the current highest applied
migration (0100 at implementation time → 0110), because golang-migrate only
applies versions strictly greater than the current max and a lower number
(e.g. `0071`) would silently never run. The additive
column is a clean forward migration:

```sql
-- 0110_mcp_warning_codes.up.sql  (owner: apps/api/db/)
ALTER TABLE mcp.tool_calls
    ADD COLUMN warning_codes jsonb NOT NULL DEFAULT '[]'::jsonb;
CREATE INDEX tool_calls_warning_codes_gin_idx
    ON mcp.tool_calls USING gin (warning_codes);
```

(Up-only — no `0110_mcp_warning_codes.down.sql`, per the §3.3 "No
`down.sql` files in P01" convention.) The `result_kind` CHECK constraint
(`tool_calls_result_chk`) is **untouched** — confirming §7.1 / §8.1.1's
invariant that the partial-success path remains `result_kind='ok'`.

### 8.2 Logging (NFR-12)

`encore.dev/rlog` emits JSON Lines on STDERR. STDOUT is reserved for MCP
JSON-RPC payloads only (per Manifesto / NFR-12). Mixing is a quality-gate
failure.

Required structured fields per log line:
- `trace_id` — ULID minted by the `mcp` raw endpoint at request
  entry and bound on `context.Context` via `rlog.With` (§10.2
  Option B). This is the audit/business correlation id and is the
  same value persisted on `mcp.tool_calls.trace_id`,
  `deps.cascade_events.trace_id`, and emitted in the §7 error
  envelope.
- `org_id`, `project_id`, `user_id`, `agent_kind` — when known
- `tool` — tool name on MCP-path logs
- `service` — Encore service name

Encore's runtime trace id (`req.Trace.TraceID`) is **not** emitted
on application log lines; Encore Cloud's observability stack records
it separately at the infrastructure layer.

### 8.3 `deps.cascade_events` (ships in `0050_deps.up.sql`)

Every successful cascade subscriber pass writes one row (idempotent on
`(event_id, triggered_by_item_id)` per AR-11). Drives PRD M-5
(cascade-events-per-day metric) without touching observability stack
retention windows.

**Round-6 (cascade-symmetry).** The `kind` column now carries all four
values — `'close'`, `'edge_added'`, `'edge_removed'`, `'state_change'`
— per §6.3.0. `'edge_removed'` rows are written INLINE by Tool 12 in
the same transaction as the `DELETE` (the subscriber's later attempted
INSERT collapses to no-op via the `ON CONFLICT (event_id,
triggered_by_item_id) DO NOTHING` clause, since the publish reuses the
inline event_id — tension #1). The other three kinds are written by
the subscriber during its `pipeline_stage` recompute pass.

---

## 9. [Removed — round-12]

The one-shot Go CLI seeder previously specified here
(`apps/api/cmd/unblock-seed/`) was deleted from P01 scope on 2026-05-22
after `/investigate` on bead `unblock-tv8.23` surfaced four blockers
(DRIFT-1..4 — see round-12 changelog at the top of this spec).

Replacement: the E2E exit-criterion test (`apps/api/exitcriteriontest/`)
owns its own seed via `TestMain` + direct `encore.dev/storage/sqldb`
writes, mirroring the `apps/api/shared/rbactest/seed.go` pattern. The
canonical 5-item graph fixture topology that used to live here is
relocated to §11.1 as the authoritative exit-criterion fixture
description. Fixture data lives as Go constants/structs in
`apps/api/exitcriteriontest/fixture.go` — no YAML, no
`gopkg.in/yaml.v3` dependency.

Section number 9 is retained as a tombstone to preserve the §10..§14
numbering used by 30+ cross-references throughout this spec.

---

## 10. Cross-cutting Machinery

### 10.1 RBAC (`pkg/rbac`, NFR-2)

Located at `apps/api/shared/rbac/` (called `pkg/rbac` colloquially per
plan; the actual import path is `encore.app/shared/rbac`). Exposes:

```go
package rbac

// ScopedQuery is a typed query builder that refuses to compile a query
// against an org/project-scoped table without an explicit scope filter.
type ScopedQuery[T any] struct{ /* internal */ }

func For[T any](identity auth.Identity, table string) *ScopedQuery[T]

// Where appends a WHERE clause. The scope filter is automatically added
// to the WHERE chain — it is not optional, not bypassable.
func (q *ScopedQuery[T]) Where(clause string, args ...any) *ScopedQuery[T]

// Run executes; returns an error if the executing service does not own
// the schema (compile-time check via Encore's per-service DB binding;
// runtime check via the typed query builder enforcing the scope filter).
func (q *ScopedQuery[T]) Run(ctx context.Context) ([]T, error)
```

**Mechanism — typed query builder, not Encore middleware (review
L11-W5).** Plan §2.3 mentions "per-service `//encore:middleware` for
tenant filtering" as a candidate. P01 ships **the typed query builder
above** instead, for three reasons:
- Compile-time safety: an attempt to query an org-scoped table without
  going through `rbac.For[T]` is a code-review smell that the linter at
  §11.3 explicitly catches (no direct `db.Query("SELECT ... FROM
  workitems.items")` anywhere outside `pkg/rbac`).
- Encore middleware can intercept request lifecycle but cannot rewrite
  SQL — middleware would need to push a context-bound filter that the
  data layer reads, which adds an indirect layer that the typed builder
  collapses.
- Single canonical helper means the RBAC regression suite has one
  surface to fuzz, not two (middleware AND query path).

Encore middleware (`//encore:middleware`) IS used elsewhere — auth
handler context propagation, request/response logging, panic recovery —
but tenant filtering specifically uses the typed-query-builder
mechanism. Plan §2.3's wording is non-normative on this; spec §10.1 is
the authoritative pin.

The exhaustive RBAC regression suite under `apps/api/shared/rbactest/`
(NFR-2) fires one test per (caller-org, target-org, caller-role, table,
action) combination across the schemas P01 exposes. The role axis is
first-class — `org.Authorize` branches on `Identity.Role` ({owner,
admin, member, viewer, agent}; the synthetic `agent` is the API-key
runtime role per §4.3.2 step 8) before the role-action matrix, so a
matrix that omits role fails to exercise the actual policy. CI gates
release on zero cross-tenant leaks.

**Suite scope is split across three phase tasks, not delivered in one
bead:**

- **B-3 (this bead, `unblock-tv8.9`)** — auth + org schemas only.
  Concretely: `auth.users`, `auth.oauth_tokens`, `auth.sessions`,
  `org.organizations`, `org.projects`, `org.members`,
  `org.project_members`. Lays down the seed/cleanup scaffolding,
  the matrix-axis vocabulary, and the `t.Run`-per-tuple driver that
  the two follow-up tasks extend. The scope here is the surface the
  two live P01 services (auth, org) actually touch; everything beyond
  is reserved for the tasks below so each bead has independent
  acceptance criteria.
- **C-6 (`unblock-tv8.15`)** — extends the matrix to `workitems.items`,
  `workitems.comments`, `workitems.trail`, `deps.dependencies`. Lands
  after the C-group RPCs that introduce those tables.
- **E-3 (`unblock-tv8.25`)** — final P01 gate. Closes the remaining
  `deps.cascade_events`, `mcp.tool_calls`, `mcp.api_keys`,
  `memory.entries`, `boards.boards` surfaces and is the
  release-blocking exhaustive sweep across every P01-exposed schema.
  `deps.cascade_events` lands here (not in C-6) because it is read by
  `deps.RecentCascadeEvents` (AF2 / Tool 1 `prime`) and carries
  `org_id`, so it requires both row-leak (`rbac.For`-style) and
  Authorize-gate coverage; deferring to E-3 keeps C-6 scope minimal.
  The E-3 worker MUST extend `org.resourceAllowed` to include
  `deps.cascade_events` (resource constant `resourceDepsCascadeEvents
  = "deps.cascade_events"`) and `org.agentReadWriteResources` to
  include the same — agents read the table through Tool 1 `prime`'s
  AF2 path, while writes remain closed-loop (only the cascade
  subscriber emits rows server-side, server-identity). Without these
  two additions, the `KindAuthorizeOnly` axis in the matrix would
  short-circuit to `InvalidArgument` instead of asserting the
  intended `PermissionDenied` contract (round-10 closure). The
  release-blocking CI separate-report wiring referenced under §11.2
  NFR-2 is reassigned to A-6 (`unblock-tv8.6`, infra-supervisor) —
  E-3 ships the suite discoverable as `encore test
  ./apps/api/shared/rbactest/...` and A-6 wires the gate
  (`tv8.25 → tv8.6` dependency edge, round-10).

**Mechanism within each task.** Tables with an `org_id` column are
driven through `rbac.For[T]` (the read-side scope predicate is what's
under test). Tables without an `org_id` column —
`auth.users`, `auth.oauth_tokens`, `auth.sessions`,
`org.organizations`, `org.project_members` — are driven through
`org.Authorize` directly (their cross-tenant gate is the Authorize
predicate, not a `WHERE … org_id = $1` filter). The suite seeds two
orgs with a full role complement per side, runs without `t.Parallel`
(rbac.Bind is not goroutine-safe; bead `unblock-tv8.34` tracks the
hardening), and is discoverable as `encore test
./apps/api/shared/rbactest/...` so the A-6 CI bead (`unblock-tv8.6`)
can wire the gate without touching the suite.

#### 10.1.1 Write-surface row-level tenant gate (round-16, bead `unblock-tv8.77`)

The read path self-gates in SQL via `rbac.For[T]` (§10.1 above). The
**write path** is hardened symmetrically: every item / milestone
write-by-id RPC and the two MCP-reachable `deps` edge RPCs self-gate via
a **row-level tenant predicate** keyed on an internal `CallerOrgID`
channel. `CallerOrgID` is populated by the MCP handler from the
Bearer-resolved `identity.OrgID` and is **NEVER accepted from the wire** —
it travels as a private RPC struct field, exactly like the `OrgID`
internal channel established for the label / milestone RPCs (§4.4 /
§4.4.1). A foreign target id therefore yields `NOT_FOUND` (or zero
affected rows on an INSERT), never a cross-tenant mutation. This closes
the IDOR write seams that the §10.1 read gate already closed on the read
side — discovered-from the `unblock-tv8.75` review and decided by Miguel
(2026-06-11).

**Gated RPCs and predicate forms:**

| RPC (MCP tool) | Predicate form |
|---|---|
| `workitems.Update` (5 `update`), `Close` (6), `Claim` (3), `Promote` (15), `SetStateColumns` (13), `AssignItem` (18 — on the target item) | `org_id = $caller` on the `SELECT … FOR UPDATE` / `UPDATE` row |
| `workitems.AssignItem` (18 — on the assign-branch milestone read) | same milestone predicate `org_id = $caller OR project_id IN (SELECT id FROM org.projects WHERE org_id = $caller)` — org-XOR-project scoped; a foreign `milestone_id` yields `NOT_FOUND` before the M-INV-7 check, never disclosing existence via `PRECONDITION_NOT_MET` |
| `workitems.AppendComment` (10) | INSERT … SELECT predicated on the target item's `org_id = $caller` (zero rows inserted → `NOT_FOUND`) |
| `workitems.AppendComment` (10 — `parent_id` threading scope, bead `unblock-tv8.80`) | when `parent_id` is non-empty, the same `INSERT … SELECT` additionally requires `parent_id IN (SELECT id FROM workitems.comments WHERE item_id = $target_item)` — the parent comment must live on the SAME target item; a foreign-org OR cross-item `parent_id` inserts zero rows → `NOT_FOUND`, indistinguishable from a missing parent. The target item is already `CallerOrgID`-gated (row above), so same-item transitively implies same-org — no separate parent-org branch is needed. Empty `parent_id` is the top-level-comment path (no predicate); the self-parent prohibition (`comments_no_self_parent_chk`) is preserved. |
| `workitems.UpdateMilestone` (17), `CreateMilestone` parent-read seam (16) | `org_id = $caller OR project_id IN (SELECT id FROM org.projects WHERE org_id = $caller)` — org-XOR-project scoped (project-scoped milestones carry `NULL org_id`) |
| `workitems.CreateMilestone` (16 — `project_id` INSERT-scope selector, bead `unblock-tv8.83`) | on the **project-scoped branch** (non-empty `project_id`), the milestone INSERT is a guarded `INSERT … SELECT` requiring `project_id IN (SELECT id FROM org.projects WHERE org_id = $caller)` — mirroring the `CreateLabel` INSERT…SELECT precedent: a foreign-but-existing `project_id` yields zero source rows → zero inserted → `NOT_FOUND`, indistinguishable from a missing project. The **org-scoped branch** (`org_id` set, `project_id` empty) carries NO project predicate. The empty-`CallerOrgID` no-op IS preserved here (unlike `CreateLabel`'s hard-reject) — `CreateMilestone` has trusted internal / E2E-seed callers per §10.1.1, so an empty `$caller` skips the project predicate rather than rejecting. The already-gated `parent_milestone_id` parent-read seam (row above, bead `unblock-tv8.77`) is preserved unchanged. This closes the LAST ungated cross-reference write-IDOR in the create/write family. |
| `workitems.Update` (5 `update` — `milestone_id` write-scope selector, bead `unblock-tv8.84`) | DISTINCT from and ADDITIONAL to the target-item `org_id = $caller` gate above (bead `unblock-tv8.77`). When the request sets a **non-empty** `milestone_id`, the `UPDATE` additionally requires `milestone_id IN (SELECT id FROM workitems.milestones WHERE org_id = $caller OR project_id IN (SELECT id FROM org.projects WHERE org_id = $caller))` — the org-XOR-project milestone predicate, mirroring the `AssignItem` (row above) / `Create` (row below) precedent. A foreign-but-existing `milestone_id` yields zero affected rows → `NOT_FOUND`, the item UNCHANGED, indistinguishable from a missing milestone. The **clear-to-null** path (`milestone_id = ""`) and the **nil = unchanged** path carry NO milestone predicate (the `$6 = ''` disjunct) and are preserved; the empty-`CallerOrgID` no-op and the existing target-item gate are preserved. Closes the residual `Update.milestone_id` cross-tenant write IDOR missed by `.83`'s AC4. |
| `workitems.MilestoneTree` (19, read) | same milestone predicate, in the recursive-CTE anchor |
| `deps.AddEdge` (11), `deps.RemoveEdge` (12) | resolve each endpoint's org from the DB; reject with `NOT_FOUND` if any resolved endpoint org ≠ `$caller` |
| `workitems.Create` (4 `create`) — each wire-supplied cross-reference, validated against the caller org before/at the INSERT inside the single create transaction (the bead-`unblock-tv8.17` atomicity contract). See the **create-path block** below for the gate-key divergence. | per reference: `project_id` → `project_id IN (SELECT id FROM org.projects WHERE org_id = $caller)`; `parent_id` → `parent_id IN (SELECT id FROM workitems.items WHERE org_id = $caller)`; `discovered_from_id` → same as `parent_id`; `milestone_id` → `org_id = $caller OR project_id IN (SELECT id FROM org.projects WHERE org_id = $caller)` (org-XOR-project); `labels[]` → every `label_id` is org-scoped to `$caller` OR project-scoped to a project in `$caller`. A foreign-but-existing reference yields the SAME `NOT_FOUND` as a missing id (never a "belongs to another org" message); a foreign `label_id` attaches nothing. The `dependencies[].blocker_item_id` endpoint is gated separately by `deps.AddEdgeInTx`'s `CallerOrgID` check (rows 11/12 above, in-tx form) — unchanged. |

**Create-path gate-key framing (DECIDED by Miguel 2026-06-12, bead
`unblock-tv8.78`).** The `workitems.Create` references above are gated, but
`Create` is a **deliberate divergence from the .77 separate-`CallerOrgID`
convention**: it reuses its **existing `req.OrgID`** as the gate key — the
same field the INSERT stamps `org_id` from — rather than introducing a new
`CallerOrgID` field, and it does **NOT** take the empty-`OrgID` no-op branch
the item/milestone write-by-id RPCs use below. Rationale: `req.OrgID` is
already pinned from `identity.OrgID` by the MCP handler and already
validated non-empty, and Create's internal callers (the §11.1.1
exit-criterion seed + integration tests) all pass a real, same-org `OrgID`,
so there is no trusted-no-auth-caller path that needs an empty-org no-op.
Coverage is identical to the `CallerOrgID`-channel RPCs; only the key
differs. This closes the create-path cross-reference IDOR seam discovered
from the `unblock-tv8.77` write-surface review (proven live via the MCP
endpoint 2026-06-12): before this round, `workitems.Create` validated wire
references for FK existence in ANY org only, letting a caller produce an
item whose `org_id ≠` a referenced project/parent/milestone/label's org.

**No-op-vs-hard-guard ratification (DECIDED by Miguel 2026-06-11).** The
item / milestone write RPCs above take the **empty-`CallerOrgID` NO-OP
form** — the predicate is `($caller = '' OR <predicate>)`, the same
precedent as `MilestoneTree` / `ListLabels`, NOT the hard-reject the
label *write* RPCs use. This is a deliberate divergence: trusted internal
no-auth callers — the §11.1.1 exit-criterion seed and integration tests
that drive these RPCs directly through Encore's private mesh with no org
context — pass an empty `CallerOrgID`, and the no-op branch lets them
operate unscoped. The branch is reachable ONLY from those trusted
callers: every MCP handler ALWAYS pins `CallerOrgID` from
`identity.OrgID` before dispatch, so the no-op is **unreachable from the
agent surface**. By contrast `CreateLabel` / `UpdateLabel` / `DeleteLabel`
**hard-reject** an empty `CallerOrgID` with `InvalidArgument` — those RPCs
have no trusted-internal-caller path (they are MCP-only), so an empty
`CallerOrgID` there is always a programming error. `CreateLabel`'s hard
guard is added in this round for consistency with `UpdateLabel` /
`DeleteLabel`, closing the deferred-epic RISK.

**Auth / BFF admin write surface (round-16, bead `unblock-tv8.85`).** The
two `auth` private RPCs that write `mcp.api_keys` rows (§4.1) extend this
write-gate model to a surface DISTINCT from the MCP agent wire. Unlike every
row above — whose `CallerOrgID` is pinned by the MCP handler from the
Bearer-resolved `identity.OrgID` — these RPCs' caller identity is pinned by
the **FUTURE key-management BFF / web-admin surface** from a resolved browser
**session** (`auth.Validate` with `TokenKind="session"`, §4.1 / §4.3.2), NOT
from the Bearer API-key handler. They are **NOT MCP-wire-reachable** today (no
MCP tool maps to them; only test/seed callers exist, and the §11.1.1 E2E seed
writes `mcp.api_keys` via direct INSERT), so the IDORs they close are
**LATENT** — exploitable cross-tenant only once that admin surface is wired.
This is the same IDOR class as `.75` / `.77` / `.78` / `.80` / `.83` / `.84`,
on the ADMIN/BFF surface instead of the MCP wire.

**Org-provisioning write surface (bead `unblock-tv8.86`).** The 2026-06-15
admin/BFF surface IDOR sweep that produced `.85` set aside the SIBLING
org-provisioning RPCs on this same admin surface; this round adds them. Two
`private` `org` RPCs (`apps/api/org/org.go`, §4.2) write tenant-scoped rows
from the wire with NO caller-ownership check: **(1) `org.AddMember`** —
INSERTs an `org.members` row from the wire-supplied `OrgID`/`UserID`/`Role`
with ZERO caller check (`callerIdentity` feeds only the `invited_by` audit
column, never authorization) and NO `Role` cap → a CRITICAL cross-tenant
privilege escalation (a caller could mint itself `owner` of ANY org);
**(2) `org.CreateProject`** — INSERTs an `org.projects` row under the
wire-supplied `OrgID` guarded only by the FK→`NotFound`, which catches a
NON-EXISTENT org but NOT a FOREIGN existing one (a WARNING-class write IDOR).
Like the `auth` rows, NEITHER is MCP-wire-reachable today (no MCP tool maps to
them; only test/seed callers), so both IDORs are **LATENT** — exploitable
cross-tenant only once a future key-management / web-admin BFF is wired. Same
IDOR class + dormant-gate pattern as `.85`, on the org-provisioning surface.
**Bootstrapping writes are correctly OUT of scope:** `org.CreateOrganization`
(the caller BECOMES the owner) and `auth.ExchangeOAuthCode` / `auth.Validate`
(identity establishment) carry no membership gate by design.

| RPC (auth/BFF admin) | Predicate form |
|---|---|
| `auth.RevokeAPIKey` (§4.1) | the UPDATE gains `id=$1 AND ($caller='' OR org_id=$caller)`, with `$caller` = the new `CallerOrgID` channel (pinned from the resolved session identity, §4.3.2, NEVER from the wire). A cross-tenant `KeyID` affects zero rows → `NOT_FOUND` (existence not leaked); the `COALESCE` idempotency is preserved. |
| `auth.IssueAPIKey` (§4.1) | gate on BOTH (a) caller-owns-org — `org.Authorize` keyed on the new `CallerUserID` channel (`SELECT role FROM org.members WHERE org_id=$1 AND user_id=$2`, §4.2 / `apps/api/org/org.go:520`) — and (b) `IssuedToUser ∈ org.members(OrgID)` (membership predicate; infra: `org.members` per migration `0030`, `org.AddMember` §4.2). A foreign `OrgID` or a non-member `IssuedToUser` → rejected (`NOT_FOUND` / appropriate error), nothing inserted, existence not leaked. |
| `org.AddMember` (§4.2) — **CRITICAL priv-esc** (bead `unblock-tv8.86`) | the INSERT of an `org.members` row from the wire-supplied `OrgID`/`UserID`/`Role` gains the new `CallerUserID` channel (pinned from the resolved session identity, §4.3.2, NEVER from the wire — `callerIdentity` previously fed only the `invited_by` audit column, never authorization). When `$caller` is non-empty the RPC requires BOTH (a) the caller holds an **admin/owner** `org.members` row in `OrgID` (`SELECT role FROM org.members WHERE org_id=$1 AND user_id=$2`, §4.2 / `apps/api/org/org.go:520`) AND (b) the granted `Role` is **capped at the caller's effective role** (no granting above one's own). A foreign / non-member `OrgID`, an unauthorised (non-admin) caller, or an over-grant → `NOT_FOUND` / appropriate error, nothing inserted, existence not leaked. Closes a CRITICAL cross-tenant privilege-escalation (a caller could mint itself `owner` of ANY org). |
| `org.CreateProject` (§4.2) (bead `unblock-tv8.86`) | the INSERT of an `org.projects` row under the wire-supplied `OrgID` gains the new `CallerUserID` channel (pinned from the resolved session identity, §4.3.2, NEVER from the wire). When `$caller` is non-empty the RPC requires the caller to be a **write-capable member** of `OrgID` (`SELECT role FROM org.members WHERE org_id=$1 AND user_id=$2`, §4.2 / `apps/api/org/org.go:520`) before the INSERT. A foreign / non-member `OrgID` → `NOT_FOUND`, **replacing the bare FK→`NotFound`** (which only caught a non-existent org, not a foreign existing one), nothing inserted, existence not leaked. |

All four take the **empty-caller NO-OP form** — `RevokeAPIKey`'s `$caller=''`
disjunct, and the empty-`CallerUserID` skip on `IssueAPIKey` / `org.AddMember`
/ `org.CreateProject` — exactly the empty-`CallerOrgID` no-op precedent
ratified above (the item/milestone write-RPC pattern), NOT the `CreateLabel`
hard-reject: the trusted §11.1.1 E2E seed + integration / mcpaudit / perf
tests pass no caller identity (or seed `mcp.api_keys` via direct INSERT), and
the trusted §11.1.1 seed + `org` / `rbactest` / `exitcriteriontest` /
`perftest` callers drive the `org` RPCs with no caller identity. The gate is
therefore **DORMANT until the future key-management BFF / admin surface is
wired**, and that future bead MUST pin the caller identity
(`CallerOrgID` / `CallerUserID`) — else the no-op leaves the IDOR (and, for
`org.AddMember`, the priv-esc) open. Cross-tenant probes return `NOT_FOUND`,
never leaking existence. **NO `mcp.api_keys` NOR `org` schema / DDL / migration
change** — the gates are query predicates + an `org.Authorize` call +
membership predicates + (for `org.AddMember`) a role cap; the `mcp.api_keys`
and `org` schemas are unchanged. The infra all CONFIRMED present:
`org.members` (migration `0030`), `org.Authorize` (`org.go:520`, the
`SELECT role FROM org.members WHERE org_id=$1 AND user_id=$2` membership/role
check), and the org service owns the `org` schema (direct read OK). The
load-bearing code lives in `apps/api/auth/auth.go` (`IssueAPIKey` /
`RevokeAPIKey` + the new request fields) and `apps/api/org/org.go`
(`AddMember` / `CreateProject` gates + the new `CallerUserID` request fields),
updated in lockstep on the implementation bead's branch. (`org.project_members`
has no write RPC yet — seed-only; a future `AddProjectMember`-style RPC will
need the IDENTICAL caller-membership gate, noted in §4.2.)

The load-bearing description of this write-gate model lives in the
`apps/api/workitems/workitems.go` auth-model doc-comment (§10.1 read-side
/ write-side split); the `deps` endpoint-resolution gate lives in
`apps/api/deps/`. Those code doc-comments are updated in lockstep on the
implementation bead's branch.

### 10.2 Tracing (NFR-12)

**Decision (round-5, 2026-05-12 — closes contradiction blocking
`unblock-tv8.5`): Option B — single ULID minted at MCP entry, propagated
via `context.Context` only.**

Rationale: keeps a single id format across the public surface
(matches the §7 error-envelope `trace_id: "<ULID>"` contract,
`mcp.tool_calls.trace_id`, and `CascadeRequested.TraceID`), preserves
ULID consistency with workitem identifiers, avoids leaking
Encore-runtime-format strings into stored audit data, and requires
zero custom inter-service header plumbing because Encore's generated
client carries `context.Context` across private RPCs automatically.
Encore's own `req.Trace.TraceID` (per `encore.CurrentRequest()`)
remains available for runtime observability only and is **not**
persisted.

Contract:

- The `mcp` raw endpoint (`MCPHandler`, §5.3) mints a ULID
  `trace_id` at request entry, before dispatching any tool. This is
  the canonical audit/business correlation id.
- The id is attached to the request `context.Context` (`rlog.With`
  binds it as the `trace_id` structured field on every log line for
  the remainder of the request).
- Cross-service propagation rides the standard Encore generated
  client: handlers call e.g. `workitems.Claim(ctx, params)` and
  the framework carries `ctx` (including the bound `trace_id`)
  into the callee. **No `X-Unblock-Trace-Id` header is set or
  required** — that mechanism is not part of the supported Encore
  surface and is explicitly removed from this spec.
- The id is written to `mcp.tool_calls.trace_id` at request end
  (§8.1; column type is `text`, accepts the ULID string verbatim).
- The id is copied into `CascadeRequested.TraceID` (§4.5 / §6.3.1)
  at publish time so Pub/Sub subscribers can persist it on
  `deps.cascade_events.trace_id` (§8.3) — Encore Pub/Sub does not
  surface context across the topic boundary, so the publisher
  explicitly embeds it in the payload (the same pattern used for
  `EventID` per C1 closure).
- The id is echoed in the JSON-RPC error envelope `data.trace_id`
  (§7).

Encore's runtime trace id (`req.Trace.TraceID`) is **not** stored in
any persisted column or pushed onto the public surface; it is left
to the Encore Cloud observability dashboard as the
infrastructure-level correlation id and is independent of
`trace_id` above.

### 10.3 Catalogue authoring (Plan §2.3 / Q4)

`apps/api/mcp/catalogue.json` is **created in P01** with the **23** P01
tools' tool definitions (round-16: the 14 core tools + `promote` + the
four milestone tools + the four label tools — see §6.2), but the
`block_conditions[]` arrays are **empty placeholders** for every
transition:

```jsonc
{
  "schema_version": "v0.1",
  "tools": [
    {
      "name": "prime",
      "description": "...",
      "input_schema": { /* JSON Schema */ },
      "output_schema": { /* JSON Schema */ }
    },
    /* ... 22 more (round-16: 23 tools total) ... */
  ],
  "transitions": []  // empty; populated in P02
}
```

`go generate` is wired and emits `apps/api/mcp/catalogue.gen.go`
containing the embedded JSON as a `[]byte` constant + helper getters. CI
fails if `go generate` produces a diff against the committed file.

The catalogue-drift CI workflow (`.github/workflows/catalogue-drift.yml`)
is **scaffolded** in P01 but is a no-op (the `unblock-plugin` consumer
does not exist until P04). It activates as load-bearing in P04. (Path
correction per D-7 bead unblock-tv8.22 — pre-D-7 spec referenced
`infra/github/workflows/`, which is not the GitHub Actions workflow
discovery path.)

`mcp.meta_catalogue` MCP tool itself is **not** exposed in P01 — it ships
in P02 once the BLOCK conditions are authored.

**Catalogue vs. live `tools/list` schema (reconciliation — bead
`unblock-tv8.82`).** `apps/api/mcp/catalogue.json` is an **off-wire
authoring artifact** consumed at build time (P02 `mcp.meta_catalogue` +
the P04 `unblock-plugin` renderer); its `input_schema` bounds/enums are
**NOT** the schema the live MCP endpoint advertises and were never on the
wire. The schema served live by `tools/list` is **REFLECTED** by the go-sdk
(v1.6.0) via `jsonschema.ForType` from each handler's Go input struct (it
does not read `catalogue.json` at runtime). Pre-bead-`unblock-tv8.82` that
reflected schema carried only `type` + `required` + `additionalProperties:
false`; bead `unblock-tv8.82` ENRICHES the live `tools/list` schema to also
advertise `enum` + `minimum`/`maximum` + `required[]` (§6.2.0a) and enforces
them via the §7.3 uniform-validation contract. The catalogue's bounds/enums
and the now-enriched live schema therefore SHOULD agree by construction, but
the live `tools/list` reflected schema — not `catalogue.json` — is
authoritative for what an agent receives on the wire.

### 10.4 Security boundary / threat model (D-1 transport addendum)

The public `POST /mcp` + `GET /mcp` surface (§5) is exposed only on
Encore Cloud's public hostname (`api.unblock.websublime.com` per
`apps/api/README.md` and the project CLAUDE.md). The Bearer API-key hot
path (§4.3.2) is the canonical authentication gate; every successful
request resolves to a tenant-scoped `Identity` before any tool body
runs, and every method other than `POST` / `GET` returns `405 Allow:
POST, GET` *before* auth fires (§5.1, AC #3).

The modelcontextprotocol/go-sdk Streamable HTTP handler exposes a
defense-in-depth setting `StreamableHTTPOptions.DisableLocalhostProtection`
that, when left at its default (`false`), refuses requests whose
`Host` header is not loopback while the listener is bound to
loopback — a DNS-rebinding mitigation aimed at single-binary
desktop / dev deployments. P01 sets it to `true` (apps/api/mcp/transport.go).
Rationale:

- Encore Cloud's public hostname is the same as the listening interface
  in production; the SDK's loopback check would refuse every legitimate
  request and is therefore inapplicable to the deployment target.
- The Bearer hot path (§4.3.2) + Encore Cloud's TLS-terminating edge
  proxy are the real security boundary; an unauthenticated cross-origin
  caller cannot reach a tool body regardless of `Host` header value.
- The acceptance test suite (`apps/api/shared/mcpaudittest/`) runs
  against `httptest.NewServer`, which emits `Host: 127.0.0.1:<port>`
  in some cases and other host strings in others; the SDK's localhost
  guard would refuse the very tests that prove the Bearer gate works.
- The trade-off is explicit: P01 does **not** support running the
  Encore binary on a developer laptop as a localhost-only MCP server
  for un-trusted local web pages to call. That deployment shape is out
  of scope; the `unblock-code` Rust binary serves that local-dev role
  via its own stdio MCP transport (Plan §1.2).

If a future phase introduces a localhost-binding deployment shape
(e.g. an on-prem appliance), the setting must be re-evaluated and the
threat model patched accordingly.

---

## 11. Acceptance Criteria

### 11.1 Functional acceptance (PRD §8 P01 exit criterion)

#### 11.1.0 Exit-criterion fixture (relocated from former §9.2, round-12)

The fixture is the canonical 5-item dependency graph the E2E test
materialises and asserts against. Topology preserved verbatim from
former §9.2:

| Item | Project | Type | Title | Status | impl_state | review_state | qa_state | is_ready | closed_at |
|---|---|---|---|---|---|---|---|---|---|
| `itm_a` | `prj_exit` | task | Bootstrap (already done) | `Done` | `done` | `approved` | `passed` | — | `now` |
| `itm_b` | `prj_exit` | task | Implement core (ready)   | `Ready` | (default) | (default) | (default) | `true` | — |
| `itm_c` | `prj_exit` | task | Depends on B             | (default) | (default) | (default) | (default) | (default) | — |
| `itm_d` | `prj_exit` | task | Depends on B             | (default) | (default) | (default) | (default) | (default) | — |
| `itm_e` | `prj_exit` | task | Cycle attempt target     | `Ready` | (default) | (default) | (default) | `true` | — |

Dependency edges (all `kind = 'blocks'`):

- `itm_a → itm_b`
- `itm_b → itm_c`
- `itm_b → itm_d`
- `itm_d → itm_e` (added so `itm_e → itm_a` closes the chain — review L10-C1: `itm_a → itm_b → itm_d → itm_e → itm_a`)

Org / project / user / API-key scaffolding the fixture seeds alongside
the graph:

- One `org.organizations` row: id=`org_exit_criterion`, slug=`exit-criterion`, name=`P01 Exit Criterion`.
- One `org.projects` row: id=`prj_exit`, org=`org_exit_criterion`, slug=`default`, name=`Default`.
- One `auth.users` row: id=`usr_alice`, primary_provider=`github`, primary_provider_id=`"1"`, email=`alice@example.com`, display_name=`Alice`.
- One `mcp.api_keys` row: issued_to_user=`usr_alice`, org=`org_exit_criterion`, label=`alice-claude-code`, agent_kind=`claude-code`.

The displayed ids above are illustrative labels — the actual seed mints
ULIDs at runtime via `apps/api/shared/ulid` (same constraint as
`apps/api/shared/rbactest/seed.go`: hard-coded ids would clash on UNIQUE
constraints across long-lived dev clusters).

#### 11.1.1 Seed ownership (round-12)

The E2E test in `apps/api/exitcriteriontest/` owns its own seed via
`TestMain` + direct `encore.dev/storage/sqldb` writes, mirroring
`apps/api/shared/rbactest/seed.go:46-53` ("All rows go through direct
`sqldb.Exec`, NOT through the auth/org RPCs. The RPC surfaces require
an Encore auth context the test cannot easily fabricate"). Fixture
data lives as Go constants/structs in
`apps/api/exitcriteriontest/fixture.go` — no YAML file, no
`gopkg.in/yaml.v3` dependency in `go.mod`.

The `mcp.api_keys` row is inserted via direct SQL with `key_hash`
computed using `secrets.APIKeyHMACSecret` per the production hashing
in `apps/api/auth/apikey.go:103-111` (HMAC-SHA256 over the raw key,
32-byte digest stored as `bytea`); the raw key value is held in
memory by the test goroutine and used as the `Bearer` token in the
RPC assertions below. The test never calls `auth.IssueAPIKey` —
direct INSERT is the seed contract (DRIFT-4 round-12: Encore parser
invariant E1388 forbids `package main` under `cmd/` from calling
private RPCs).

Milestone rows seeded for the round-2 D1 milestone assertions (see
below) ARE created through Encore's private mesh by calling
`workitems.CreateMilestone` / `workitems.AssignItem` directly from
the test goroutine — milestone RPCs work from a test-internal Encore
context (the test is part of an Encore service, not `package main`
under `cmd/`).

**Cascade subscriber test invocation (round-13).** The cascade
subscriber (`apps/api/deps/cascade_subscriber.go::handleCascadeRequested`)
is the SOLE writer of `deps.cascade_events` rows for kinds `'close'`,
`'edge_added'`, `'edge_removed'`, and `'state_change'` (per the round-6
§6.3.0 cascade-symmetry split — `pipeline_stage` single-writer is the
subscriber, `is_ready` single-writer is the inline mutating call site).
Encore Pub/Sub subscriptions DO NOT fire under `encore test`: the test
harness records published messages but does not consume them, so a
publish from `workitems.Close` / `deps.AddEdge` / `deps.RemoveEdge` /
`workitems.SetStateColumns` / `workitems.Claim` (I-3 reset path) never
reaches `handleCascadeRequested` and no row materialises. To make the
§11.1.2 and §11.3 row-level assertions reachable, the subscriber is
invoked directly via the exported wrapper
`deps.HandleCascadeRequestedForTest(ctx context.Context, msg *deps.CascadeRequested) error`
— a thin pass-through to `handleCascadeRequested` with no behavioural
divergence from production. The wrapper mirrors the established
`mcp.ServeMCPForTest` precedent (`apps/api/mcp/export_test_writer.go:49-65`):
exported on the production import path, `ForTest` suffix is the audit
trail, production callers MUST NOT invoke it. The wrapper is a plain
Go function, NOT an `//encore:api` — Encore's public API surface is
unaffected.

Test ordering contract for the exit-criterion harness and any future
Encore test needing cascade row materialisation:

1. Invoke the producing RPC through the normal MCP / private-mesh path
   (`workitems.Close`, `deps.AddEdge`, `deps.RemoveEdge`,
   `workitems.SetStateColumns`, or `workitems.Claim` on the I-3 reset
   path) — this performs the inline `is_ready` recompute (Regime A)
   and publishes `CascadeRequested` for the multi-hop work.
2. Capture the published messages via
   `et.Topic(deps.CascadeRequestedTopic).PublishedMessages()` (or the
   Encore-generated equivalent on the typed topic handle).
3. For each captured message, invoke
   `deps.HandleCascadeRequestedForTest(ctx, &msg)` exactly once to
   materialise the `deps.cascade_events` row(s) and apply the
   `pipeline_stage` updates (Regime B).
4. Assert the row(s) per §11.1.2.

Idempotency assertions (§11.3 — re-delivery property test) re-invoke
`deps.HandleCascadeRequestedForTest` with the same `event_id` and
assert the `ON CONFLICT (event_id, triggered_by_item_id) DO NOTHING`
clause collapses the second insert to no-op, yielding byte-identical
post-state and exactly one row per `(event_id, triggered_by_item_id)`.
The Tool 12 (`remove_dependency`) inline-INSERT + post-commit
subscriber re-INSERT collapse via reused `event_id` (round-6 changelog,
§6.3.0 tension #1) is exercised through the same wrapper.

#### 11.1.2 Functional assertions (PRD §8 P01 exit criterion)

The end-to-end harness in `apps/api/exitcriteriontest/` runs against the
seeded fixture and asserts:

- [ ] `auth_handler` accepts a `Bearer <api-key>` derived from the `mcp.api_keys` row inserted by the test seed (§11.1.1) and resolves to the correct `Identity`.
- [ ] `prime` returns a non-empty `ready_summary` (the test seed placed `itm_b` and `itm_e` in ready state per §11.1.0) and an empty `claimed_by_me`.
- [ ] `ready --limit 1` returns one item, deterministically.
- [ ] `claim` on the returned item succeeds; a second concurrent `claim` from a different agent receives `{ "kind": "ALREADY_CLAIMED", ... }`.
- [ ] `set_state(impl_state=done)` on the claimed item is accepted (structural invariant only — `claimed_by_id` is set).
- [ ] `close` on the same item succeeds (P01 relaxation: `claimed_by_id IS NOT NULL` is the only precondition); cascade subscriber fires.
- [ ] After cascade, `prime` reflects newly unblocked dependents (`itm_c`, `itm_d` flip `is_ready=true`); they remain `status='Backlog'` until promoted.
- [ ] **`promote` (round-16, bead `unblock-tv8.71`).** `promote(item_id=itm_c)` — where `itm_c` is `status='Backlog' AND is_ready=true` after the cascade above — succeeds and returns the item with `status='Ready'`; a subsequent `ready` lists `itm_c`. `promote` on an item that is still blocked (e.g. an item with an open `blocks` parent, `is_ready=false`) is rejected with `{ "kind": "PRECONDITION_NOT_MET", "details": { "status": "Backlog", "required": "Ready", "missing": "is_ready" } }` per §7.2; `promote` on an already-`Ready` item is rejected with `{ "kind": "PRECONDITION_NOT_MET", "details": { "status": "Ready", "required": "Ready" } }`.
- [ ] **`is_ready`-on-create (round-16, bead `unblock-tv8.71`).** A `create` of a fresh `type=task` item with no inline dependencies returns an item whose `is_ready=true` and `status='Backlog'` (asserts the inline create-path write, not subscriber materialisation); the item is immediately `promote`-able.
- [ ] **`claim`-on-not-Ready (round-16, bead `unblock-tv8.72`).** `claim` on an item whose `Status <> 'Ready'` (e.g. a Backlog item that was never promoted) is rejected with `{ "kind": "PRECONDITION_NOT_MET", "details": { "status": "Backlog", "required": "Ready" } }` per §7.2 — the SAME extension `promote` defines (distinct from the `ALREADY_CLAIMED` concurrent-loser path).
- [ ] **`issued_to_user` REQUIRED (round-16, bead `unblock-tv8.73`).** `auth.IssueAPIKey` with an empty `issued_to_user` is rejected with `InvalidArgument`; a successfully-issued key resolves (§4.3.2) to an `Identity` whose `UserID` is non-empty. (DDL-level: the `0120` migration makes `mcp.api_keys.issued_to_user` NOT NULL.)
- [ ] **`show` reference resolution (round-16, bead `unblock-tv8.76`).** `show(itm_b)` returns `parent` (or `null`), and `dependencies_in` / `dependencies_out` as `ResolvedRef[]` each carrying `{id, title, status, kind}` for the DIRECT neighbours only — assert the neighbour's `title` + `status` are populated and that the resolution is one level deep (the neighbour's own dependencies are NOT present in the payload).
- [ ] **Milestone + label MCP tools reachable (round-16, beads `unblock-tv8.74` / `unblock-tv8.75`).** The harness drives `create_milestone` → `assign_item` → `milestone_tree` through the MCP boundary (not just the private RPCs) and asserts the tree shape; and drives `create_label` → `list_labels` → `update_label` → `delete_label`, asserting the registry round-trips and `delete_label` detaches the label from any items it was applied to.
- [ ] `add_dependency(from=itm_e, to=itm_a)` is rejected with `CYCLE_DETECTED` (would form `itm_a → itm_b → … → itm_e → itm_a`; the §11.1.0 fixture includes the `itm_d → itm_e` edge that closes the chain when the cycle-attempt edge is added).
- [ ] `deps.cascade_events` has one row per fired cascade with a populated `event_id` and the affected set; `kind='close'` for the cascade triggered by Tool 6 above.
- [ ] **Cascade-symmetry kinds (round-6 §6.3.0).** The exit-criterion
  harness exercises each of the four `kind` values and asserts exactly
  one `deps.cascade_events` row materialises per logical trigger:
  - `'close'` — from the Tool 6 close above.
  - `'edge_added'` — issue `add_dependency(from=itm_c, to=itm_d)` after
    setup; assert a row with `kind='edge_added'` and
    `triggered_by_item_id=itm_d`.
  - `'edge_removed'` — issue `remove_dependency` on the edge above and
    assert a single row with `kind='edge_removed'` (tension #1: the
    inline INSERT and the post-commit subscriber re-insert collapse
    via the reused `event_id` + `ON CONFLICT` clause).
  - `'state_change'` — issue `set_state(qa_state=failed)` on an item
    with `review_state='approved'`, then `claim` it (different agent);
    the Claim fires the I-3 reset path and publishes
    `state_change`. Assert a row with `kind='state_change'` and
    `triggered_by_item_id` = the claimed item id.
- [ ] **Milestones (round-2 D1).** The E2E test (`apps/api/exitcriteriontest/`, round-12)
  calls `workitems.CreateMilestone` twice via Encore's private mesh — once
  for a parent (depth=1) and once for a child whose `parent_milestone_id`
  references the parent (depth=2) — then calls
  `workitems.AssignItem(itm_b, child_milestone_id)`; `MilestoneTree` returns
  the parent with the child nested, and `workitems.Get(itm_b)` returns the
  expected `MilestoneID`. M-INV-7 is exercised: assigning an item to a
  milestone whose `project_id` differs from the item's `project_id` is
  rejected with `kind=PRECONDITION_NOT_MET, data.invariant="M-INV-7"`.
- [ ] **State-machine invariants (round-2 D2 — five property tests).**
  - I-1: `set_state(review_state=needs_rework)` on an item with
    `qa_state='passed'` flips `qa_state='pending'` in the same write.
  - I-2: `set_state(qa_state=failed)` on an item with `review_state <>
    'approved'` is rejected with `data.invariant="qa_failed_requires_review_approved"`.
  - I-3: After `set_state(qa_state=failed)`, the next `claim` resets both
    `review_state='pending'` and `qa_state='pending'` atomically (verified
    via `get_state` immediately post-claim).
  - I-4: `set_state(review_state=approved)` on an item with
    `impl_state='pending'` is rejected with
    `data.invariant="review_change_requires_impl_done"`.
  - I-5: `set_state(impl_state=pending)` on an item with `impl_state='done'`
    AND no rework path active is rejected with
    `data.invariant="impl_done_to_pending_requires_rework_path"`. The
    same call when `review_state='needs_rework'` succeeds.

### 11.2 Non-functional acceptance

- [ ] **NFR-1 — Latency.** `prime → ready → claim` p99 < 2 s on the
  warm-cache harness (`apps/api/perftest/`). **Measurement methodology
  (C4 closure):** harness runs against the **local Encore emulator**;
  warm cache means (a) Postgres connection pool established, (b) API key
  validated once before the timer starts, (c) no first-request cold-start
  outliers (M ≥ 10 warm-up iterations discarded before measurement
  begins). Cloud measurement is a P02 ops item.

  **Seeding doctrine.** The harness owns its fixture via direct
  `sqldb.Exec` per the §11.1.1 round-12 doctrine (no `auth`/`org` RPCs in
  the seed path); the org/project slug is salted with a shortULID to avoid
  dev-cluster collision. In-test key issuance uses direct
  `INSERT INTO mcp.api_keys` with `key_hash` computed against
  `secrets.APIKeyHMACSecret`. Precedent: `apps/api/exitcriteriontest/seed.go`.
  Seed `N = 2 × iterations` ready items so each measured `claim` consumes a
  fresh row.

  **W3 closure (negative auth paths).** The harness MUST include sibling
  coverage of the §4.3.2 negative paths against the same `httptest` server:
  revoked key, expired key, unknown prefix, bad HMAC, and missing
  `unblock_pat_` prefix. Each path asserts the MCP transport's
  auth-rejection wire signal — **HTTP 200 + a JSON-RPC error envelope
  (`error.code == -32000`, `error.data.kind == "UNAUTHENTICATED"`)**, which
  is the faithful realisation of `errs.Unauthenticated` at the Streamable
  HTTP edge (the transport never emits a bare HTTP 401; precedent
  `apps/api/shared/mcpaudittest/d1_transport_test.go`). This closes the W3
  gap carried forward from the closed B-1 review (cross-linked on bead
  `unblock-tv8.24`): the four DB-bound auth RPC bodies were verified only by
  inspection in B-1.

  **W4 closure (goroutine drain check).** The Bearer hot path fires
  `go touchLastUsedAt(id)` per request with a 1 s context cap
  (`apps/api/auth/auth.go:217`); under load this can pile up. The harness
  samples `runtime.NumGoroutine` three times — `baseline` (before warm-up),
  `peak` (immediately after the measurement loop), and `drained` (after a
  `2 * time.Second` post-loop sleep, giving the 1 s cap two cycles to
  expire). Assertion: `drained - baseline ≤ 20` (absolute margin for
  runtime/SDK overhead). The harness is the leak alarm; the RS01-4 LRU-cache
  mitigation remains the fix and is tracked separately.

  **Gate semantics.** When the harness runs, it ALWAYS logs per-call latency
  samples, the computed p99, and the goroutine deltas as JSON-Lines via
  `t.Logf` (informative on every run — aligns with the bead AC verb
  "reports"). A hard-fail (`t.Fatalf` on `p99 ≥ 2 s` OR
  `drained - baseline > 20`) is gated by the `UNBLOCK_PERF_GATE=1`
  environment variable. Release-blocking pipeline wiring is a P02 ops item
  owned by Olive.

  **Test isolation (round-15 — CI-failure closure).** The harness MUST be
  **excluded from the default `apps/api` full test suite (§11.2 NFR-10
  Gate 5, `encore test ./...`)** and MUST run ONLY in a dedicated, isolated
  CI step under `UNBLOCK_PERF_GATE=1`. Rationale (empirical, CI run
  [26633703926](https://github.com/websublime/unblock/actions/runs/26633703926)):
  a latency harness cannot co-schedule with the full functional suite on a
  single shared local Postgres and produce either meaningful measurements or
  a reliable verdict. Under concurrent load the warm-cache `Validate` /
  `Claim` / `Ready` calls ballooned from a local ~87 ms p99 to 5–16 s per
  call; one measurement-loop response returned an empty body, tripping a
  hard `t.Fatalf` ("no SSE data") that the gate does NOT guard (the gate
  guards only the p99/goroutine assertions, not transport errors); and the
  harness's ~630 concurrent `mcp.tool_calls` audit-row writes broke a
  sibling package's global-count assertion (see mcpaudittest hardening
  below). **Default-suite contract:** with `UNBLOCK_PERF_GATE` unset the
  harness package MUST contribute **zero database load and zero
  `mcp.tool_calls` rows** — it must not merely log-and-pass, it must not
  execute its seed or its measurement/negative-auth loops at all. **The
  exclusion mechanism is the implementer's choice** — a `//go:build perf`
  build tag (compile-time exclusion) or an `UNBLOCK_PERF_GATE`-gated
  `t.Skip` / `TestMain` short-circuit (run-time exclusion that also skips
  the seed) are both acceptable — but it MUST be validated against the
  Encore parser with `encore check` and against the default suite with
  `encore test ./...` (perftest contributes nothing) AND the gated step
  (`UNBLOCK_PERF_GATE=1`, perftest runs in isolation). The dedicated CI
  step is owned by Olive.

  **mcpaudittest hardening (round-15 — coupled correctness fix).** The
  `apps/api/shared/mcpaudittest` audit-row assertions
  (`d1_transport_test.go`'s `selectToolCalls`) query
  `mcp.tool_calls` **globally** (filtered only by `tool_name NOT LIKE
  'rbactest%'`), so the "0 rows on auth-failure" contract
  (`TestD1_POSTNoAuthReturnsUnauthenticated`) is fragile to ANY concurrent
  writer of non-rbactest audit rows. `selectToolCalls` MUST be scoped to the
  test's own org (and/or session) so the D1 audit-row assertions are robust
  to concurrent writers regardless of the perftest isolation above. This is
  a pre-existing latent test-isolation defect that the perftest load made
  deterministic; it is fixed in the same rework.
- [ ] **NFR-2 — RBAC.** `apps/api/shared/rbactest/` green; zero
  cross-tenant leaks across every P01 read and write surface.
- [ ] **NFR-5 — Cycle integrity.** Cycle creation is rejected at write
  time (write-time enforcement, not read-time detection). Property test
  N=100 random graph mutations: zero cycles ever materialise in the DB.
- [ ] **NFR-9 — Decoupled deliverables.** No Rust code under `crates/`
  ships with P01. `crates/` directory remains as in stage-1 (empty or
  placeholder Cargo.toml).
- [ ] **NFR-10 — Quality gates.** Greta gate set green:
  - `cd apps/api && go fmt ./...` produces zero diffs
  - `go vet ./...` clean
  - `golangci-lint run --max-warnings 0`
  - `encore test ./...` (Encore service packages — the encore-test wrapper brings up Postgres + Pub/Sub + Cron emulators that plain `go test` cannot supply). **No `-race` flag** — see round-11 changelog: `encore test ... -race` reproducibly SIGSEGVs inside encore-go's `lazyTraceInit.initStream` goroutine spawn ([encoredev/encore#1943](https://github.com/encoredev/encore/issues/1943), open ~1 year, cross-platform). The rbactest suite is single-threaded by design (`rbac.Bind` is not goroutine-safe; no `t.Parallel`), so dropping `-race` removes no real coverage on the encore-side gate.
  - `go test ./shared/ulid/... ./shared/rbac/... ./shared/lint/... -race` (leaf packages without `encore.dev` imports — these run under the native Go race detector and DO get race coverage; this is where the gate's race signal lives).
  - `encore check` clean
  - Encore-generated TypeScript client diff: zero (regenerate, compare to committed in `apps/web/src/lib/encore.gen.ts` if present in P05; in P01 the generated file ships at `apps/api/encore.gen.ts` as a build artifact).
- [ ] **NFR-12 — Logging (HTTP transport reframe).** P01 ships MCP over
  Streamable HTTP, not stdio — so the original NFR-12 phrasing ("STDOUT
  carries only MCP envelopes") is reframed for the HTTP context: (a) all
  service logs go to STDERR via `encore.dev/rlog` as JSON Lines; (b) MCP
  JSON-RPC envelopes travel exclusively via `http.ResponseWriter`, never
  via STDOUT; (c) acceptance check: harness asserts that no log line
  appears in any HTTP response body, and that STDERR is exclusively
  JSON-Lines (one log object per line, parseable). The "no mixing"
  invariant degenerates to "logs and protocol payloads use disjoint
  channels (STDERR for logs, ResponseWriter for envelopes; STDOUT
  unused)". Verified via integration test that captures both streams
  during the exit-criterion harness run.

### 11.3 Architectural invariants

- [ ] All eight Postgres schemas exist with the canonical SPEC §9.4 DDL
  after running migrations 0010..0090.
- [ ] **Single-writer invariants (round-6 §6.3.0 — fractured).** The
  materialised columns on `workitems.items` have distinct, asymmetric
  writers:
  - (a) **`pipeline_stage` single-writer = the cascade subscriber.**
    Integration test asserts no other code path UPDATEs
    `workitems.items.pipeline_stage`. The
    `apps/api/shared/lint/no_direct_is_ready_write.go` linter rule
    enforces this statically — its scope tightens (round-6) to
    `pipeline_stage` writes only; any UPDATE targeting
    `workitems.items.pipeline_stage` outside
    `apps/api/deps/cascade_subscriber.go` is rejected.
  - (b) **`is_ready` single-writer = the mutating call site
    (Regime A).** `is_ready` is recomputed inline by the call site
    that mutated the row/edge — the cascade subscriber MUST NOT write
    `is_ready`. Explicit allowlist of permitted writers:
    `workitems.Create` (Tool 4 — **round-16, bead `unblock-tv8.71`:**
    inline `is_ready` set at row insert per §6.6),
    `workitems.Close` (Tool 6 — inline recompute on direct `blocks`
    neighbours), `deps.AddEdge` (Tool 11 / §6.5 — inline recompute on
    `to_item`), `deps.RemoveEdge` (Tool 12 — inline recompute on
    direct `to_item`), and the internal shared helper
    `deps.recomputeReady` (called by the above sites). Integration
    test asserts the cascade subscriber never UPDATEs `is_ready`; an
    additional static check in the linter rule asserts the allowlist
    above is exhaustive. The `apps/api/shared/lint/no_direct_is_ready_write.go`
    allowlist gains the `workitems.Create` insert site in lockstep with
    this round.
- [ ] `rbac.For` and `rbac.ScopedQuery.Where` are never called with
  runtime-constructed string arguments — every table identifier (For
  arg 2) AND every clause string (Where arg 1) MUST be a Go string
  literal or untyped string constant. Runtime values destined for
  Where flow through `args...` exclusively; the table identifier of
  For has no runtime channel (SQL identifiers cannot be bound).
  (Static analysis: `golangci-lint` custom linter rule under
  `apps/api/shared/lint/no_rbac_dynamic_clause.go` rejects any
  non-literal call site across the unblock backend for BOTH call
  shapes. Locked SPEC §10.1 surface is unchanged; the analyzer is a
  meta-guard. unblock-tv8.33, unblock-tv8.35.)
- [ ] `deps.cascade_events` insert is idempotent on re-delivery (property
  test: re-deliver every `CascadeRequested` event twice; assert post-state
  is byte-identical and exactly one row exists per `(event_id,
  triggered_by_item_id)`). Re-delivery under `encore test` is driven by
  invoking `deps.HandleCascadeRequestedForTest` twice with the same
  message (Encore Pub/Sub subscriptions do not fire under `encore test`
  — see §11.1.1 "Cascade subscriber test invocation" for the wrapper
  contract and the `mcp.ServeMCPForTest` precedent).
- [ ] Atomic claim is a single transaction with `SELECT FOR UPDATE`
  (property test: N=100 concurrent claim attempts on the same item;
  assert exactly one winner and N-1 `ALREADY_CLAIMED` errors).
- [ ] Cycle detection runs inside a transaction holding
  `pg_advisory_xact_lock(hashtext('deps.add_dependency:' || project_id))`
  (integration test: simulate two concurrent `add_dependency` calls that
  would form a cycle from different vantage points; assert at most one
  succeeds).
- [ ] Manifesto Laws covered in P01 (L1 cascade, L2 one graph, L3
  Postgres-truth, L5 atomic claim, L7 < 2s) are structurally present —
  each invariant is backed by at least one regression test.

### 11.4 Documentation

- [ ] `docs/specs/01-spec-backend-mvp.md` is **APPROVED** before
  implementation starts (this document).
- [ ] README.md updated with P01 user surface (MCP Bearer auth, the 23
  tools' one-liners — round-16). The former `unblock-seed` invocation deliverable
  is dropped (round-12 — seeder CLI deleted from P01 scope; the
  exit-criterion fixture is now seeded by the E2E test itself per
  §11.1.1).
- [ ] `apps/api/README.md` documents service decomposition and migration
  ownership (the dedicated `db`-service migration-owner pattern is
  non-obvious — every domain service is an equal consumer).

### 11.5 Open Question carry-overs

- **OQ1 (Copilot transport):** P01 acceptance harness uses Claude Code as
  the reference MCP client. Copilot transport coverage is P04 plugin
  renderer scope; if a P01 reviewer wants Copilot manual-tested, they may
  run it against the same `Bearer + Streamable HTTP` endpoint and report
  findings as a P02 input — it does not block P01 close.

---

## 12. Implementation Tasks (mapped to Plan §4)

This spec is the contract. The plan §4 task breakdown remains
authoritative for sequencing. Below maps each plan task to the spec
section that locks its contract:

| Plan task | Owner | Spec section(s) |
|---|---|---|
| A-1 (Encore app init) | Greta | §3.1 (migration owner), §4 (service skeletons) |
| A-2 (Bootstrap migration) | Greta | §3.2 (`0010_bootstrap.up.sql`), §3.5 (secrets) |
| A-3 (Migrations §9.4.1–§9.4.8 + round-16 `0120`) | Greta | §3.2 (migrations 0020..0090; round-16 `0120_mcp_issued_to_user_notnull` per bead `unblock-tv8.73`), §3.4 (FTS DDL) |
| A-4 (`pkg/rbac`) | Greta | §10.1 |
| A-5 (Tracing scaffold) | Greta | §10.2 |
| A-6 (CI gates) | Olive | §11.2 (NFR-10 commands) |
| B-1 (`auth` service; round-16 `issued_to_user` REQUIRED) | Greta | §4.1 (`IssueAPIKey` rejects empty `IssuedToUser`), §4.3.2 step 8 (no empty-UID identity), §4.3.3 — bead `unblock-tv8.73` |
| B-2 (`org` service) | Greta | §4.2 |
| B-3 (RBAC suite) | Greta | §10.1, §11.2 (NFR-2) |
| C-1 (`workitems` service; round-16 label RPCs) | Greta | §4.4 (incl. round-16 `CreateLabel`/`ListLabels`/`UpdateLabel`/`DeleteLabel` private RPCs per bead `unblock-tv8.75`; `is_ready`-on-create per §6.6), §4.4.1 (milestone RPCs — round-2 D1) |
| C-2 (`deps` service + cycle CTE) | Greta | §4.5, §6.5, §6.3.0 (post-commit publishes for `edge_added` and `edge_removed`) |
| C-3 (Cascade subsystem) | Greta | §6.3, §6.3.0 (four-Reason dispatch; `pipeline_stage`-only writer) |
| C-4 (Atomic claim) | Greta | §6.4 (incl. I-3-reset `state_change` publish per §6.3.0) |
| C-5 (`pipeline_stage` derivation tests) | Greta | §6.3.2, §6.3.0, SPEC §5.7.1 |
| C-6 (RBAC suite extensions) | Greta | §10.1 |
| D-1 (MCP transport skeleton) | Greta | §5, §4.3.1, §4.3.2 |
| D-2 (Tools 1–4 + round-16 `promote` + `is_ready`-on-create) | Greta | §6.2 (tools 1–4 + Tool 15 `promote`), §6.6 (status transition map + `is_ready`-on-create rule, beads `unblock-tv8.71`/`unblock-tv8.72`), §7.2 (`{status,required}` extension) |
| D-3 (Tools 5–8; round-16 `show` reference resolution) | Greta | §6.2 (tools 5–8, incl. Tool 7 `show` `{id,title,status}` resolution per bead `unblock-tv8.76`), §4.4 (`Trail` / `ResolvedRef` widening) |
| D-4 (Tools 9–10) | Greta | §6.2 (tools 9–10), §3.4 (FTS), §6.2 #9 |
| D-5 (Tools 11–12) | Greta | §6.2 (tools 11–12), §6.5 |
| D-6 (Tools 13–14) | Greta | §6.2 (tools 13–14) |
| D-7 (Catalogue v0; round-16 23 tools) | Greta | §10.3 (catalogue carries all 23 P01 tool definitions) |
| D-8 (round-16 milestone + label MCP tools — Tools 16–23) | Greta | §6.2 (Tools 16–19 milestone facades per bead `unblock-tv8.74`; Tools 20–23 label tools per bead `unblock-tv8.75`), §4.4 (label RPCs incl. migration `0130_workitems_labels_updated_at` per bead `unblock-tv8.75`), §4.4.1 (milestone RPCs), §3.2 (migration `0130`) |
| E-2 (NFR-1 latency harness) | Greta | §11.2 (warm-cache definition) |
| E-3 (NFR-2 RBAC suite) | Greta | §10.1, §11.2 |
| E-4 (Exit-criterion E2E test) | Greta | §11.1 (incl. §11.1.0 fixture topology + §11.1.1 seed ownership), §6.3 (cascade), §6.5 (cycle) |

> **Round-12 note.** The former E-1 row (Seeder CLI) is removed — bead
> `unblock-tv8.23` is cancelled in lockstep, post-spec, by the
> orchestrator. The exit-criterion fixture is now owned by E-4 itself
> per §11.1.1 (the E2E test runs its own `TestMain` seed via direct
> `sqldb.Exec`, mirroring `apps/api/shared/rbactest/seed.go`).

---

## 13. Risks (P01 spec-level)

Plan §7 tracks phase risks; spec-level risks below reflect what could
break a contract pinned in this document.

| # | Risk | Mitigation |
|---|---|---|
| RS01-1 | Go MCP SDK API changes during P01 (the SDK is "in collaboration with Google" but new) | Pin exact version in `go.mod`; vendor-allowlist in CI (no auto-bump on `go.mod`); update path in P02 if SDK breaks. |
| RS01-2 | Encore adds new constraints on multi-schema use that break the dedicated-`db`-service migration-owner pattern | Smoke test in CI: `encore check` on every PR; if Encore's behaviour ever splits the migration runner, escalate to a `rebase` plan. |
| RS01-3 | The `Mcp-Session-Id` header semantics change between MCP spec revisions before P01 ships | Pin to MCP spec 2025-06-18 (current canonical); migration to a future spec is a P02+ task. |
| RS01-4 | The `subtle.ConstantTimeCompare` cost on the API-key hot path (§4.3.2) is unexpectedly high under Encore's request handler | Benchmark inline as part of E-2 latency harness; if budget pressure emerges, introduce a 1-minute in-process LRU cache keyed by `key_prefix` + `revoked_at,expires_at` snapshot (eviction on revoke event). |
| RS01-5 | Cascade subscriber re-delivery storm on a degenerate fixture | The `(event_id, triggered_by_item_id)` UNIQUE constraint absorbs duplicates structurally; a load test with N=10k re-deliveries asserts zero duplicate inserts, zero `is_ready` flips beyond the first one. |
| RS01-6 | `pg_advisory_xact_lock(hashtext(...))` collisions across unrelated projects | `hashtext` is a 32-bit hash; collision probability at v1 scale (≤ 100 projects per org) is negligible (~10⁻⁸). Documented as acceptable; revisit at v1.1 if scale breaks the assumption. Alternative if needed: switch to `pg_advisory_xact_lock(int4, int4)` two-key form using `(org_seq_id, project_seq_id)`. |
| RS01-7 (round-2 D1) | Milestone tree CTE depth violation under racing inserts — two concurrent `CreateMilestone` calls each see their own parent at depth=3, both attempt to insert at depth=4 (legal), but a third call could push one into depth=5. | The recursive ancestor walk runs inside the same transaction as the insert; the parent row is `SELECT ... FOR UPDATE` to serialise concurrent children. M-INV-6 enforcement is therefore strictly serial per parent. Cross-parent concurrency is fine because milestone trees are scope-bounded and the ancestor walk is parent-id-driven. AR-17 documents the cap. |
| RS01-8 (round-2 D2) | State-machine invariants implemented in app code (CTE) rather than DB CHECK constraints — drift risk if a future migration adds new state values without updating the invariant table. | `(impl_state, review_state, qa_state, pipeline_state)` enum CHECK constraints in `0040_workitems.up.sql` are declared `IMMUTABLE` (Postgres-natural) and named (`items_impl_state_chk`, …); any phase that adds a new value MUST update both the CHECK and the invariant CTE in lockstep, asserted by a documentation cross-link in the migration's commit message. The five property tests in §11.1 form the regression net. AR-18 covers the concurrency dimension. |

---

## 14. Approval Checklist

Before this spec moves from DRAFT to APPROVED, the user (orchestrator)
confirms:

- [ ] All seven research contradictions (C1, C2, C3, C5, C6, C7 + AF1/AF5)
  are honoured in the design above.
- [ ] The 23 MCP tool contracts (round-16: 14 core + `promote` + 4
  milestone + 4 label) are signature-locked and no field is ambiguous
  (every "optional" / "default" stated explicitly). v1.0 total is 27
  (+4 memory at P02).
- [ ] Migration filenames and numbering are agreed.
- [ ] Error envelope kinds and `data` shapes cover every failure mode the
  exit-criterion harness exercises.
- [ ] No simplification has been smuggled in — every plan §2 / §6 / §3
  resolution is preserved.

Post-approval, `/tasks` (Fernando) decomposes this spec into bd beads
under epic P01. Each bead's description references the spec section that
locks its contract; no bead is a self-sufficient document
(`feedback_bead_description_not_spec`).
