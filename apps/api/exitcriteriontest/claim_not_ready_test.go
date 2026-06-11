// claim_not_ready_test.go covers the §11.1.2 round-16 (bead
// unblock-tv8.72) MCP-boundary assertion for the claim-on-not-Ready
// precondition:
//
//   - claim on an item whose Status <> 'Ready' (a fresh Backlog item
//     that was never promoted) is rejected with PRECONDITION_NOT_MET
//     carrying data.details {status:'Backlog', required:'Ready'} per §7.2
//     — the SAME status-extension promote defines — and NO `missing`
//     (claim's block is the wrong status, not an unmet readiness gate).
//     This is DISTINCT from the ALREADY_CLAIMED concurrent-loser path.
//
// Before bead unblock-tv8.72 the SELECT … FOR UPDATE filtered the lock
// by (status='Ready' AND claimed_by_id IS NULL), so a never-Ready,
// unclaimed Backlog item produced zero rows and funnelled to the
// unconditional ALREADY_CLAIMED loser arm — mis-reporting it as already
// claimed (with no winner info), so the agent could not tell "never
// Ready" from "lost the race".
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 6.4 (atomic claim) +
// § 7.2 ({status, required} extension, reused by claim Tool 3) +
// § 11.1.2 L3378 (the claim-on-not-Ready functional assertion).

package exitcriteriontest_test

import (
	"encoding/json"
	"testing"
)

// TestExitCriterion_ClaimNotReady drives create → (do NOT promote) →
// claim through the MCP boundary and asserts the §7.2 status-precondition
// envelope. Single Mcp-Session-Id across the sequence.
func TestExitCriterion_ClaimNotReady(t *testing.T) {
	f := fx(t)
	ctx := t.Context()
	sessionID := initializeSession(t, f.RawKey)

	// Create a fresh item. With no inline dependencies it is is_ready=true
	// but status='Backlog' (§6.6 is_ready-on-create). We deliberately do
	// NOT promote it, so its Status stays 'Backlog'.
	//
	// NOTE: this MCP create path yields is_ready=true, whereas the
	// workitems-level createBacklogItem helper seeds is_ready=false. Both
	// are intentional — is_ready is irrelevant to claim's status gate, which
	// keys only on Status<>'Ready', so either is_ready value drives the same
	// PRECONDITION_NOT_MET rejection asserted below.
	createEnv := callTool(t, f.RawKey, sessionID, "create", map[string]any{
		"project_id": f.ProjectID,
		"type":       "task",
		"title":      "claim-not-ready-target",
	})
	var created createOutItem
	if err := json.Unmarshal(expectSuccess(t, createEnv), &created); err != nil {
		t.Fatalf("unmarshal create result: %v", err)
	}
	if created.Item.ID == "" {
		t.Fatalf("create returned empty item id")
	}
	if created.Item.Status != "Backlog" {
		t.Fatalf("create: status = %q, want Backlog", created.Item.Status)
	}
	// Belt-and-suspenders: confirm the persisted row is Backlog before the
	// claim, so the rejection we assert is genuinely the not-Ready path.
	assertDBStatus(t, ctx, created.Item.ID, "Backlog")

	// Claim the never-promoted Backlog item. It must reject with the §7.2
	// status extension, NOT ALREADY_CLAIMED.
	claimEnv := callTool(t, f.RawKey, sessionID, "claim", map[string]any{
		"item_id": created.Item.ID,
	})
	data := expectError(t, claimEnv)
	if data.Kind != "PRECONDITION_NOT_MET" {
		t.Fatalf("claim-not-Ready: kind = %q, want PRECONDITION_NOT_MET (not ALREADY_CLAIMED)", data.Kind)
	}
	if got, _ := data.Details["status"].(string); got != "Backlog" {
		t.Fatalf("claim-not-Ready: details.status = %q, want Backlog; details=%+v", got, data.Details)
	}
	if got, _ := data.Details["required"].(string); got != "Ready" {
		t.Fatalf("claim-not-Ready: details.required = %q, want Ready; details=%+v", got, data.Details)
	}
	// claim's wrong-status rejection carries NO `missing` (that is
	// promote's is_ready disambiguator only — claim has no readiness gate,
	// just a wrong-status block).
	if _, present := data.Details["missing"]; present {
		t.Fatalf("claim-not-Ready: details.missing present (%v), want absent (the block is wrong status, not unmet readiness)", data.Details["missing"])
	}
	// And it is NOT the ALREADY_CLAIMED loser path — no winner info.
	if _, present := data.Details["winner_user_id"]; present {
		t.Fatalf("claim-not-Ready: details.winner_user_id present (%v), want absent (not the ALREADY_CLAIMED path)", data.Details["winner_user_id"])
	}
}
