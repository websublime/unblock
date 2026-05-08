// Package auth owns the auth schema and is the sole migration-owner service
// for the unblock database. The canonical migration directory at
// apps/api/auth/migrations/ holds the migration set for every schema
// (auth, org, workitems, deps, providers, mcp, boards, memory). Other
// services consume the database via sqldb.Named("unblock"); see SPEC §3.1.
//
// In P01 task A-1 this package only declares the //encore:api skeletons
// for the four private RPCs (Validate, ExchangeOAuthCode, IssueAPIKey,
// RevokeAPIKey) so Encore recognises auth as a service. Bodies return
// errNotImplemented; the bootstrap migration and sqldb.NewDatabase call
// land in task A-2 (unblock-tv8.2) — see SPEC §3.2.
package auth

import (
	"context"
	"errors"
	"time"
)

// errNotImplemented is the sentinel returned by every P01 A-1 skeleton
// body. Real implementations land in subsequent beads (B-1..D-3).
var errNotImplemented = errors.New("auth: not implemented in P01 A-1 skeleton")

// Identity is the resolved caller record carried inside the Encore mesh.
// Locked shape per SPEC §4.1.
type Identity struct {
	UserID    string // ULID
	OrgID     string // ULID — primary org binding for this auth event
	Role      string // "owner" | "admin" | "member" | "viewer"
	AgentKind string // empty for human sessions; AgentKind value for API-key callers
}

// ValidateRequest is the input to Validate. SPEC §4.1.
type ValidateRequest struct {
	Token     string // either auth.sessions.id (browser BFF) or raw API key
	TokenKind string // "session" | "api_key"
}

// ValidateResponse is the output of Validate. SPEC §4.1.
type ValidateResponse struct {
	Identity Identity
}

// Validate accepts an opaque token (session id OR raw API key) and resolves
// it to an Identity. Returns ErrUnauthenticated on miss / revoked / expired.
//
//encore:api private method=POST path=/auth.Validate
func Validate(ctx context.Context, req *ValidateRequest) (*ValidateResponse, error) {
	return nil, errNotImplemented
}

// ExchangeOAuthCodeRequest is the input to ExchangeOAuthCode. SPEC §4.1.
type ExchangeOAuthCodeRequest struct {
	Provider     string // "github" | "gitlab"
	Code         string
	PKCEVerifier string
	UserAgent    string
	IPAddress    string
}

// ExchangeOAuthCodeResponse is the output of ExchangeOAuthCode. SPEC §4.1.
type ExchangeOAuthCodeResponse struct {
	SessionID string // ULID; opaque; used as Bearer for private RPCs
	UserID    string // ULID
	ExpiresAt time.Time
}

// ExchangeOAuthCode is called by the Astro Action /auth/[provider]/callback
// (P05) and by P01 integration tests. Verifies PKCE, exchanges the code for
// a provider access token, upserts auth.users + auth.oauth_tokens, and
// issues a new auth.sessions row. Returns the opaque session id.
//
//encore:api private method=POST path=/auth.ExchangeOAuthCode
func ExchangeOAuthCode(ctx context.Context, req *ExchangeOAuthCodeRequest) (*ExchangeOAuthCodeResponse, error) {
	return nil, errNotImplemented
}

// IssueAPIKeyRequest is the input to IssueAPIKey. SPEC §4.1.
type IssueAPIKeyRequest struct {
	OrgID        string // ULID
	IssuedToUser string // ULID; nullable (org-level service key)
	Label        string // human-readable, e.g. "claude-code-laptop"
	AgentKind    string // AgentKind value
	Scopes       []string
	ExpiresAt    *time.Time // nullable; default: never
}

// IssueAPIKeyResponse is the output of IssueAPIKey. SPEC §4.1.
type IssueAPIKeyResponse struct {
	KeyID     string // ULID (mcp.api_keys.id)
	KeyPrefix string // first 8 chars of the raw key
	RawKey    string // FULL raw key — returned ONCE; never persisted in clear
}

// IssueAPIKey creates a new mcp.api_keys row. Called by the seeder CLI
// (P01) and by future operator surfaces. Returns the raw key ONCE — the
// caller stores it; subsequent reads return only the prefix and metadata.
//
//encore:api private method=POST path=/auth.IssueAPIKey
func IssueAPIKey(ctx context.Context, req *IssueAPIKeyRequest) (*IssueAPIKeyResponse, error) {
	return nil, errNotImplemented
}

// RevokeAPIKeyRequest is the input to RevokeAPIKey. SPEC §4.1.
type RevokeAPIKeyRequest struct {
	KeyID string // ULID
}

// RevokeAPIKey flips revoked_at; idempotent.
//
//encore:api private method=POST path=/auth.RevokeAPIKey
func RevokeAPIKey(ctx context.Context, req *RevokeAPIKeyRequest) error {
	return errNotImplemented
}
