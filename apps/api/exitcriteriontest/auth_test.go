// auth_test.go covers the §11.1.2 first bullet:
//
//   - auth_handler accepts a `Bearer <api-key>` derived from the
//     mcp.api_keys row inserted by the test seed (§11.1.1) and
//     resolves to the correct Identity.
//
// "Correct Identity" is asserted indirectly: the initialize
// handshake's 200 + Mcp-Session-Id only succeeds if the Bearer
// hot path (auth.validateAPIKey at apps/api/auth/auth.go:158-237)
// returns a non-error ValidateResponse, AND the MCP layer's
// requestStateRegistry registers the resolved Identity for
// downstream tools/call dispatch. The follow-up `prime` tool call
// returns a successful structured response only if the Identity
// resolves to the same OrgID the seed installed (rbac.For gates
// every read on identity.OrgID — a wrong Identity would produce a
// "no rows" empty `ready_summary` or an Unauthenticated error).
//
// The negative-path assertions (invalid Bearer → UNAUTHENTICATED,
// missing Bearer → UNAUTHENTICATED) are covered by the D-1 transport
// suite at apps/api/shared/mcpaudittest/d1_transport_test.go; we do
// not duplicate them here.

package exitcriteriontest_test

import (
	"encoding/json"
	"testing"
)

// TestExitCriterion_AuthBearerResolvesIdentity asserts the
// §11.1.2 bullet 1: the Bearer token derived from the seed's
// mcp.api_keys row resolves to a working Identity.
//
// Verification path:
//
//  1. initializeSession with Fixture.RawKey → MUST return a
//     non-empty Mcp-Session-Id (the SDK only mints one after the
//     full Bearer hot path succeeds).
//  2. Drive the `prime` tool against that session with no
//     project_id filter (org-wide read). MUST return success and
//     ready_summary.count_total >= 2 (itm_b and itm_e are both
//     is_ready=true per the seed).
//
// Implicitly verified: identity.OrgID, identity.UserID, and
// identity.AgentKind are populated correctly because:
//
//   - rbac.For (apps/api/shared/rbac/builder.go) reads
//     auth.Data().OrgID and would return zero rows on a mismatched
//     OrgID — the prime ready_summary would carry count_total=0
//     instead of >=2.
//   - The MCP transport's tracectx population (apps/api/mcp/mcp.go:222-228)
//     would fail downstream rbac assertions if Identity.UserID was
//     empty — the SDK would return an UNAUTHENTICATED error envelope
//     from any tool that calls withIdentityFromReq
//     (apps/api/mcp/identity.go:73-94 explicitly returns
//     errMissingIdentity when UserID is empty).
func TestExitCriterion_AuthBearerResolvesIdentity(t *testing.T) {
	f := fx(t)

	sessionID := initializeSession(t, f.RawKey)
	if sessionID == "" {
		t.Fatal("initializeSession returned empty Mcp-Session-Id — auth Bearer hot path did not produce an Identity")
	}

	// Drive `prime` against the resolved identity. The seed placed
	// itm_b and itm_e at is_ready=true, so an org-wide prime should
	// surface count_total=2 minimum.
	env := callTool(t, f.RawKey, sessionID, "prime", map[string]any{
		"project_id":  f.ProjectID,
		"ready_limit": 10,
	})
	raw := expectSuccess(t, env)

	var structured struct {
		ReadySummary struct {
			CountTotal int `json:"count_total"`
			Items      []struct {
				ID       string `json:"id"`
				IsReady  bool   `json:"is_ready"`
				Status   string `json:"status"`
				Priority string `json:"priority"`
			} `json:"items"`
		} `json:"ready_summary"`
		ClaimedByMe []struct {
			ID string `json:"id"`
		} `json:"claimed_by_me"`
	}
	if err := json.Unmarshal(raw, &structured); err != nil {
		t.Fatalf("unmarshal prime structured content: %v; raw=%s", err, string(raw))
	}

	// §11.1.2 bullet 2 anchor (we re-assert in
	// prime_ready_claim_close_test.go too — kept here so the auth
	// path's positive Identity proof is self-contained).
	if structured.ReadySummary.CountTotal < 2 {
		t.Fatalf("ready_summary.count_total = %d, want >= 2 (itm_b + itm_e); identity OrgID likely mismatched", structured.ReadySummary.CountTotal)
	}
	if len(structured.ClaimedByMe) != 0 {
		t.Fatalf("claimed_by_me len = %d, want 0 (no items claimed by Alice in seed)", len(structured.ClaimedByMe))
	}

	// Cross-check: the returned ids contain both itm_b and itm_e.
	wantItemB := f.ItemID("itm_b")
	wantItemE := f.ItemID("itm_e")
	sawB, sawE := false, false
	for _, it := range structured.ReadySummary.Items {
		if it.ID == wantItemB {
			sawB = true
		}
		if it.ID == wantItemE {
			sawE = true
		}
	}
	if !sawB {
		t.Errorf("prime ready_summary did not surface itm_b (%s); items=%+v", wantItemB, structured.ReadySummary.Items)
	}
	if !sawE {
		t.Errorf("prime ready_summary did not surface itm_e (%s); items=%+v", wantItemE, structured.ReadySummary.Items)
	}
}
