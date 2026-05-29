// //encore:authhandler entry point. Reads `Authorization: Bearer …`
// and dispatches into Validate per the SPEC §4.3.3 contract:
//
//	X-Unblock-BFF-Origin == ""  → MCP path: raw API key
//	X-Unblock-BFF-Origin != ""  → BFF path: session id (P01 returns Unimplemented)
//
// DRIFT-B closure (bead investigation): SPEC §4.3.3 originally documented
// the simple-token form `func AuthHandler(ctx, token string)`. That form
// cannot read the X-Unblock-BFF-Origin header which the same section's
// dispatch rule requires. Encore's structured-params form (ENCORE.md
// lines 388-398) is the only handler shape that exposes incoming headers
// via `header:"…"` struct tags, so this handler uses that form.
//
// Identity is propagated to downstream handlers via Encore's auth
// context (encore.dev/beta/auth.UserID() returns the auth.UID we
// return; auth.Data() returns the *AuthData with the full Identity).

package auth

import (
	"context"

	"encore.app/shared/httpauth"
	"encore.dev/beta/auth"
	"encore.dev/beta/errs"
)

// AuthParams is the structured input to //encore:authhandler. Field
// tags map to incoming HTTP request headers per Encore's auth-handler
// contract (ENCORE.md lines 388-398).
type AuthParams struct {
	// Authorization is the raw value of the `Authorization` header.
	// Expected form: `Bearer <token>` where `<token>` is either a raw
	// API key (`unblock_pat_…`) on the MCP path or a session id (ULID)
	// on the BFF path.
	Authorization string `header:"Authorization"`

	// BFFOrigin is the value of `X-Unblock-BFF-Origin`. Presence (any
	// non-empty value) signals the request originates from the Astro
	// BFF and the token must be interpreted as a session id; absence
	// signals a direct MCP client and the token is interpreted as an
	// API key. SPEC §4.3.3.
	BFFOrigin string `header:"X-Unblock-BFF-Origin"`
}

// AuthData is what downstream handlers receive via auth.Data(). Wraps
// the resolved Identity per SPEC §4.3.3.
type AuthData struct {
	Identity Identity `json:"identity"`
}

// AuthHandler is the //encore:authhandler invoked by Encore on every
// request to an //encore:api with `auth: true`. Returns
// errs.Unauthenticated for any token failure; downstream services
// observe the resolved Identity via auth.UserID() and auth.Data().
//
// Hot path: this function is on every MCP request. It parses the
// Bearer token cheaply and delegates the DB lookup to Validate so the
// hot path stays at <5 ms p99 (SPEC §4.3.2 line 451).
//
//encore:authhandler
func AuthHandler(ctx context.Context, p *AuthParams) (auth.UID, *AuthData, error) {
	if p == nil || p.Authorization == "" {
		return "", nil, &errs.Error{
			Code:    errs.Unauthenticated,
			Message: "missing Authorization header",
		}
	}

	token, ok := httpauth.ParseBearer(p.Authorization)
	if !ok {
		return "", nil, &errs.Error{
			Code:    errs.Unauthenticated,
			Message: "Authorization header must be \"Bearer <token>\"",
		}
	}

	// Dispatch on BFF-origin header presence. SPEC §4.3.3.
	kind := tokenKindAPIKey
	if p.BFFOrigin != "" {
		kind = tokenKindSession
	}

	resp, err := Validate(ctx, &ValidateRequest{Token: token, TokenKind: kind})
	if err != nil {
		// Validate already returns properly classified errs.* values
		// (Unauthenticated for revoked / expired / bad HMAC / bad
		// prefix; Unimplemented for the deferred session path).
		// Propagate verbatim so the HTTP transport returns the right
		// status code and the MCP envelope mapper at D-1 sees the
		// canonical kind.
		return "", nil, err
	}

	// auth.UID is the canonical user identifier surface of the Encore
	// auth context. We use Identity.UserID — a ULID for the
	// session-token path; for API-key callers it is the
	// `mcp.api_keys.issued_to_user` value (nullable in the schema; an
	// empty string is acceptable for org-level service keys).
	return auth.UID(resp.Identity.UserID), &AuthData{Identity: resp.Identity}, nil
}
