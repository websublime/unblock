// identity.go bridges the MCP transport's Bearer-resolved Identity
// into the Encore auth context so downstream private RPCs
// (workitems.*, deps.*, org.*) can read it via encoreauth.UserID()
// and encoreauth.Data().
//
// Why this seam exists (Sherlock RISK 1, 2026-05-14): the MCP path
// is a raw //encore:api endpoint and intentionally bypasses
// //encore:authhandler so the 405 method-not-allowed branch and the
// pre-auth UNAUTHENTICATED envelope can return BEFORE auth runs
// (SPEC §4.3.1 + §7). Once mcp.go's serveMCP resolves Identity via
// auth.Validate and binds the resolved fields on tracectx, no Encore
// auth context exists for downstream RPCs to read — they would see
// encoreauth.UserID() = "" and reject with Unauthenticated.
//
// withIdentity reads the resolved Identity from tracectx and wraps
// ctx with encoreauth.WithContext so workitems.List, workitems.Claim,
// deps.RecentCascadeEvents, etc. see the same Identity Encore would
// have produced via //encore:authhandler. The bridge is the SINGLE
// trust handoff between the raw-endpoint world (manual auth) and the
// private-RPC world (handler-resolved auth).
//
// SPEC anchors:
//   - §4.3.1 (raw endpoint bypasses //encore:authhandler)
//   - §4.3.2 (Bearer hot path → Identity)
//   - §4.3.3 (//encore:authhandler contract that this bridge mimics
//     for downstream RPCs)
//   - §10.1 (write-side authorisation gated at the MCP boundary)

package mcp

import (
	"context"
	"errors"

	"encore.app/auth"
	"encore.app/shared/tracectx"
	encoreauth "encore.dev/beta/auth"
	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

// agentRole is the synthetic runtime role minted for API-key callers
// per SPEC §4.3.2 step 8. Matches org.go's `roleAgent` literal — kept
// as an unexported package-private const here so the bridge does not
// have to import the org package (which would also be a layering
// inversion: org is downstream of mcp).
const agentRole = "agent"

// errMissingIdentity is returned when no per-request state is
// registered under the request's trace_id. Reaching this branch
// means the handler ran outside serveMCP (no registration occurred)
// — a programmer error in any production path; expected only on
// unit-test paths that bypass the transport.
var errMissingIdentity = errors.New("mcp: identity bridge invoked outside serveMCP")

// withIdentityFromReq returns ctx wrapped with the Encore auth
// context populated from the per-request state registered by
// serveMCP. Tool handlers MUST call this before invoking any
// private RPC that reads auth.UserID() / auth.Data() — without it
// the RPC will see no caller identity and reject with
// Unauthenticated.
//
// The synthesised AuthData carries Role="agent" because every MCP
// caller is an API-key holder per the §4.3 Identity model. Human
// session callers travel through the BFF auth handler and never
// reach this bridge.
//
// withIdentityFromReq ALSO re-binds tracectx on the returned ctx
// so downstream rlog calls and the §7 envelope writer see the
// per-request trace_id (the SDK's stateful session model means the
// handler ctx otherwise carries the initialize-time trace_id —
// recordtoolcall.go::requestStateRegistry doc-comment explains the
// SDK behaviour).
func withIdentityFromReq(ctx context.Context, req *sdkmcp.CallToolRequest) (context.Context, error) {
	state := stateFromReq(req)
	if state == nil || state.Identity.OrgID == "" || state.Identity.UserID == "" {
		return ctx, errMissingIdentity
	}
	ctx = tracectx.With(ctx, tracectx.Fields{
		TraceID:   state.TraceID,
		Service:   "mcp",
		OrgID:     state.Identity.OrgID,
		UserID:    state.Identity.UserID,
		AgentKind: state.Identity.AgentKind,
	})
	ctx = encoreauth.WithContext(ctx, encoreauth.UID(state.Identity.UserID), &auth.AuthData{
		Identity: auth.Identity{
			UserID:    state.Identity.UserID,
			OrgID:     state.Identity.OrgID,
			Role:      agentRole,
			AgentKind: state.Identity.AgentKind,
		},
	})
	return ctx, nil
}

// stateFromReq returns the *requestState bound to req by serveMCP.
// Returns nil on test paths that invoke the handler without the
// transport (no header → no registry hit).
func stateFromReq(req *sdkmcp.CallToolRequest) *requestState {
	if req == nil || req.Extra == nil {
		return nil
	}
	return requestStateFromHeaders(req.Extra.Header)
}

// identityFromReq returns the narrow Identity tuple registered by
// serveMCP for the current request. The bool reports whether a
// binding was found — false means the handler ran outside
// serveMCP (test path) and the caller should surface
// UNAUTHENTICATED via mapError.
func identityFromReq(req *sdkmcp.CallToolRequest) (identityFields, bool) {
	state := stateFromReq(req)
	if state == nil || state.Identity.OrgID == "" || state.Identity.UserID == "" {
		return identityFields{}, false
	}
	return identityFields{
		UserID:    state.Identity.UserID,
		OrgID:     state.Identity.OrgID,
		AgentKind: state.Identity.AgentKind,
	}, true
}
