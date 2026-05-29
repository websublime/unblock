// Package httpauth is the canonical parser for the HTTP `Authorization:
// Bearer <token>` header shared across the unblock backend. It is a
// zero-Encore leaf package (no sqldb / pubsub / rlog imports) so it
// loads cleanly under plain `go test` and can be consumed by any
// service.
//
// History (bead unblock-tv8.55 / D-1 review cleanup): the same six-line
// parser was inlined twice — as the package-private `parseBearer` in
// both `apps/api/auth/authhandler.go` and `apps/api/mcp/mcp.go` — with
// byte-identical bodies and constants. Lifting it here removes the
// duplication and gives both call sites a single tested implementation.
// The behaviour is preserved exactly: case-insensitive `Bearer` scheme
// (RFC 6750 §2.1) and rejection of any leading/trailing whitespace
// inside the token itself.
package httpauth

import "strings"

// BearerPrefix is the scheme prefix the `Authorization` header must
// begin with. RFC 6750 says Bearer is case-insensitive, but the GitHub
// Copilot, Claude Code, Cursor, Codex, and Aider clients all emit
// `Bearer` literally. We compare with EqualFold for resilience.
const BearerPrefix = "Bearer "

// ParseBearer extracts the token portion of an `Authorization:
// Bearer <token>` header. Returns ("", false) on any deviation from
// the expected shape. Accepts case-insensitive `Bearer` for
// resilience (RFC 6750 §2.1) but rejects leading/trailing whitespace
// inside the token itself (callers that emit `Bearer  <token>` have a
// bug).
func ParseBearer(authzHeader string) (string, bool) {
	if len(authzHeader) <= len(BearerPrefix) {
		return "", false
	}
	if !strings.EqualFold(authzHeader[:len(BearerPrefix)], BearerPrefix) {
		return "", false
	}
	tok := authzHeader[len(BearerPrefix):]
	if tok == "" || tok != strings.TrimSpace(tok) {
		return "", false
	}
	return tok, true
}
