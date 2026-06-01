// Package auth owns ONLY the auth schema and is one of eight equal
// consumer services on the canonical `unblock` Postgres database.
// Migrations for all eight schemas (auth, org, workitems, deps,
// providers, mcp, boards, memory) live in apps/api/db/migrations/ and
// are owned by the dedicated apps/api/db/ service per SPEC §3.1; this
// package — like every other domain service — consumes the database
// via the canonical BindDB late-bind hook in db.go (a nil
// *sqldb.Database pointer populated by apps/api/db/db.go's init).
// Direct `sqldb.Named("unblock")` at package init is forbidden — the
// v1.52.1 runtime panics outside the encore CLI on every call to
// either sqldb.NewDatabase or sqldb.Named, breaking plain
// `go test ./apps/api/<service>/...`.
//
// In P01 task B-1 (bead unblock-tv8.7) this package lands the four
// private RPC bodies (Validate, ExchangeOAuthCode, IssueAPIKey,
// RevokeAPIKey) and the //encore:authhandler. Wiring of the shared
// rbac builder happens centrally in the dedicated apps/api/db/
// service's init (which calls auth.BindDB to populate this package's
// db var and rbac.Bind to install the shared rbac handle); the auth
// package itself owns no init() function.
//
// SPEC anchors: §4.1 (locked signatures), §4.3.2 (Bearer hot path),
// §4.3.3 (auth handler), §3.5 (secrets manifest), §11.1 P01 contract.
package auth

import (
	"context"
	"crypto/subtle"
	"errors"
	"fmt"
	"strings"
	"time"

	"encore.app/auth/types"
	"encore.dev/beta/errs"
	"encore.dev/rlog"
	"encore.dev/storage/sqldb"
)

// TokenKind enum literals accepted by Validate. SPEC §4.1.
const (
	tokenKindAPIKey  = "api_key"
	tokenKindSession = "session"
)

// agentKindAllowed mirrors the `mcp.api_keys.api_keys_agent_chk` SQL
// CHECK constraint. IssueAPIKey validates against this set client-side
// so the call returns a clean errs.InvalidArgument instead of bubbling
// up a Postgres CHECK violation. Order does not matter.
var agentKindAllowed = map[string]struct{}{
	"claude-code": {},
	"copilot":     {},
	"cursor":      {},
	"codex":       {},
	"aider":       {},
	"custom":      {},
}

// roleAgent is the literal role assigned to API-key callers. SPEC
// §4.3.2 step 8: `Role: "agent"`. Distinct from the org/project
// member roles ({owner, admin, member, viewer}) because API keys are
// machine identities — RBAC for agents is enforced via tool-scope
// checks at the MCP service, not via org membership rows.
const roleAgent = "agent"

// defaultAPIKeySessionTTL is the expiry applied to OAuth-issued
// sessions when the caller does not supply an override. 30 days
// matches typical Astro BFF cookie lifetime; renewal is expressed as
// a new sessions row + revocation of the old one (no in-place
// extension). Honoured by `auth.sessions.expires_at`.
const defaultAPIKeySessionTTL = 30 * 24 * time.Hour

// Identity is the resolved caller record carried inside the Encore mesh.
// Locked shape per SPEC §4.1.
//
// Re-exported as a type alias from the leaf `auth/types` sub-package
// (bead unblock-tv8.30). The alias preserves SPEC §10.1's literal
// spelling `auth.Identity` at every consumer call site (org, rbac,
// future B-1..D-3 / E-*) while letting Encore-free test paths import
// the pure-value definition directly via `encore.app/auth/types`.
//
// Do NOT redeclare Identity as a non-alias type here — that breaks
// the structural identity guarantee that lets `auth.Identity{...}`
// literals interoperate with `types.Identity{...}` in code that
// crosses both spellings.
type Identity = types.Identity

// ValidateRequest is the input to Validate. SPEC §4.1.
type ValidateRequest struct {
	Token     string `json:"token"`      // either auth.sessions.id (browser BFF) or raw API key
	TokenKind string `json:"token_kind"` // "session" | "api_key"
}

// ValidateResponse is the output of Validate. SPEC §4.1.
//
// APIKeyID is an additive field (bead unblock-tv8.16 / D-1 DECISION 2)
// over SPEC §4.1's locked struct. It carries the mcp.api_keys.id ULID
// when TokenKind="api_key" succeeds; it is empty for any other path
// (session-token path returns Unimplemented in P01; failure paths
// return an error and no ValidateResponse). The MCP transport requires
// the id to populate mcp.tool_calls.api_key_id (SPEC §8.1, FK + NOT
// NULL surface) without a second DB round-trip — auth.go:165 already
// SELECTs the column, so surfacing it on the response is the
// zero-cost path that preserves the <5 ms p99 hot-path budget
// (SPEC §4.3.2). Pre-prod stance allows additive struct fields
// per CLAUDE.md.
type ValidateResponse struct {
	Identity Identity `json:"identity"`
	APIKeyID string   `json:"api_key_id"`
}

// Validate accepts an opaque token (session id OR raw API key) and resolves
// it to an Identity. Returns errs.Unauthenticated on miss / revoked /
// expired and errs.Unimplemented on the deferred session path.
//
// SPEC §4.3.2 (8-step API-key hot path) and §4.3.3 (P01 session-path
// deferral).
//
//encore:api private method=POST path=/auth.Validate
func Validate(ctx context.Context, req *ValidateRequest) (*ValidateResponse, error) {
	if req == nil || req.Token == "" {
		return nil, &errs.Error{Code: errs.Unauthenticated, Message: "empty token"}
	}

	switch req.TokenKind {
	case tokenKindAPIKey:
		return validateAPIKey(ctx, req.Token)
	case tokenKindSession:
		// SPEC §4.3.3 P01 contract (round-4): session path returns
		// errs.Unimplemented in P01. The BFF (Astro Actions) is the
		// only consumer of this branch; P01 exit criterion uses API
		// keys exclusively (PRD §3.5 lines 245-248). The org_id
		// disambiguation rule (auth.sessions has no org_id column;
		// users may belong to multiple orgs) is left for the BFF
		// phase to define. See DECISION on the bead.
		return nil, &errs.Error{
			Code:    errs.Unimplemented,
			Message: "session-token validation deferred to the BFF phase (SPEC §4.3.3 P01 contract)",
		}
	default:
		return nil, &errs.Error{
			Code:    errs.InvalidArgument,
			Message: fmt.Sprintf("unknown token_kind %q (expected %q or %q)", req.TokenKind, tokenKindAPIKey, tokenKindSession),
		}
	}
}

// validateAPIKey implements SPEC §4.3.2 verbatim (8 steps).
//
//  1. Parse `Authorization: Bearer <raw_key>` (caller-side; we receive raw_key).
//  2. Extract key_prefix = raw_key[12:20] per the locked key-format note.
//  3. SELECT … FROM mcp.api_keys WHERE key_prefix = $1 (UNIQUE index).
//  4. Reject if revoked or expired.
//  5. Compute expected = HMAC-SHA256(secret, raw_key).
//  6. subtle.ConstantTimeCompare(stored, expected); reject on mismatch.
//  7. UPDATE mcp.api_keys SET last_used_at = now() (fire-and-forget).
//  8. Construct Identity{Role:"agent", AgentKind:row.agent_kind, …}.
func validateAPIKey(ctx context.Context, rawKey string) (*ValidateResponse, error) {
	prefix, err := prefixOf(rawKey)
	if err != nil {
		// Step 2 failure: malformed input. Bearer auth contract
		// returns Unauthenticated for any input that does not parse
		// (no information leak about which check failed).
		return nil, &errs.Error{Code: errs.Unauthenticated, Message: "invalid api key"}
	}

	// Step 3: O(1) lookup via api_keys_prefix_uniq.
	var (
		id           string
		orgID        string
		issuedToUser *string
		keyHash      []byte
		agentKind    string
		revokedAt    *time.Time
		expiresAt    *time.Time
	)
	err = db.QueryRow(ctx,
		`SELECT id, org_id, issued_to_user, key_hash, agent_kind, revoked_at, expires_at
		 FROM mcp.api_keys
		 WHERE key_prefix = $1`,
		prefix,
	).Scan(&id, &orgID, &issuedToUser, &keyHash, &agentKind, &revokedAt, &expiresAt)
	if err != nil {
		// We do not differentiate "no rows" from other DB errors in
		// the response — the Bearer auth contract returns
		// Unauthenticated for both. The DB error is logged via rlog
		// for ops visibility (SPEC §11.2 NFR-12: STDERR-only
		// JSON-Lines).
		if !errors.Is(err, sqldb.ErrNoRows) {
			rlog.Error("auth: api_key lookup failed", "err", err, "prefix", prefix)
		}
		return nil, &errs.Error{Code: errs.Unauthenticated, Message: "invalid api key"}
	}

	// Step 4: revocation + expiry checks. now() in Go (not SQL) so a
	// long-running connection-pool wait does not lengthen the
	// effective expiry window.
	now := time.Now()
	if revokedAt != nil {
		return nil, &errs.Error{Code: errs.Unauthenticated, Message: "api key revoked"}
	}
	if expiresAt != nil && expiresAt.Before(now) {
		return nil, &errs.Error{Code: errs.Unauthenticated, Message: "api key expired"}
	}

	// Steps 5-6: HMAC + constant-time compare.
	expected := hashRawKey(secrets.APIKeyHMACSecret, rawKey)
	if subtle.ConstantTimeCompare(keyHash, expected) != 1 {
		return nil, &errs.Error{Code: errs.Unauthenticated, Message: "invalid api key"}
	}

	// Step 7: fire-and-forget last_used_at update. The Bearer hot
	// path's <5 ms p99 budget (SPEC §4.3.2 line 451) does not allow
	// a synchronous UPDATE on every request, so we detach it onto a
	// background context. We swallow errors deliberately — a failed
	// last_used_at write is a UI hint loss, not an auth failure.
	go touchLastUsedAt(id)

	// Step 8: construct Identity.
	uid := ""
	if issuedToUser != nil {
		uid = *issuedToUser
	}
	return &ValidateResponse{
		Identity: Identity{
			UserID:    uid,
			OrgID:     orgID,
			Role:      roleAgent,
			AgentKind: agentKind,
		},
		// APIKeyID populated for the api_key path so the MCP
		// transport's deferred audit row can write
		// mcp.tool_calls.api_key_id without a second DB lookup
		// (DECISION 2 on bead unblock-tv8.16).
		APIKeyID: id,
	}, nil
}

// touchLastUsedAt issues a fire-and-forget UPDATE on
// mcp.api_keys.last_used_at. Detached from the request context so a
// client that disconnects between auth and downstream-RPC end does not
// abort the write; bounded to 1 second so we cannot leak an unbounded
// goroutine on a permanently-stalled DB.
//
// Tracked risk (bead): at high QPS this single-row UPDATE becomes a
// hot spot — RS01-4 / E-2 will revisit with an LRU cache if the
// latency harness shows pressure. Acceptable for v1 throughput.
func touchLastUsedAt(keyID string) {
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	if _, err := db.Exec(ctx, `UPDATE mcp.api_keys SET last_used_at = now() WHERE id = $1`, keyID); err != nil {
		// Best-effort; debug-level so a saturated DB does not
		// flood error logs.
		rlog.Debug("auth: touch last_used_at failed", "err", err, "key_id", keyID)
	}
}

// ExchangeOAuthCodeRequest is the input to ExchangeOAuthCode. SPEC §4.1.
type ExchangeOAuthCodeRequest struct {
	Provider     string `json:"provider"` // "github" | "gitlab"
	Code         string `json:"code"`
	PKCEVerifier string `json:"pkce_verifier"`
	UserAgent    string `json:"user_agent"`
	IPAddress    string `json:"ip_address"`

	// PKCEChallenge is the S256 challenge the client originally sent
	// to /authorize. SPEC §4.1's locked struct does not include it —
	// in the BFF design the server stores it server-side keyed by
	// `code` and looks it up here. P01 has no /authorize storage
	// surface (the seeder bypasses OAuth entirely per SPEC §3.5
	// line 246-248), so for the integration-test path we accept it
	// inline. This is an additive field; existing call sites that do
	// not set it fall through to the legacy (verifier-only)
	// validation pattern documented at the field. See DECISION on
	// the bead.
	PKCEChallenge string `json:"pkce_challenge"`
}

// ExchangeOAuthCodeResponse is the output of ExchangeOAuthCode. SPEC §4.1.
type ExchangeOAuthCodeResponse struct {
	SessionID string    `json:"session_id"` // ULID; opaque; used as Bearer for private RPCs
	UserID    string    `json:"user_id"`    // ULID
	ExpiresAt time.Time `json:"expires_at"`
}

// ExchangeOAuthCode is called by the Astro Action /auth/[provider]/callback
// (P05) and by P01 integration tests. Verifies PKCE, exchanges the code for
// a provider access token, upserts auth.users + auth.oauth_tokens, and
// issues a new auth.sessions row. Returns the opaque session id.
//
// P01 scope (per Plan §3.6 + bead investigation): only the GitHub
// provider is wired; GitLab is documented in the schema but
// implementation lands in P02.
//
//encore:api private method=POST path=/auth.ExchangeOAuthCode
func ExchangeOAuthCode(ctx context.Context, req *ExchangeOAuthCodeRequest) (*ExchangeOAuthCodeResponse, error) {
	if req == nil {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing request body"}
	}
	if req.Provider != "github" {
		// gitlab: SPEC §4.1 documents both, but Plan §3.6 defers
		// GitLab to P02. Returning Unimplemented (not InvalidArgument)
		// signals "valid input, not yet wired".
		return nil, &errs.Error{
			Code:    errs.Unimplemented,
			Message: fmt.Sprintf("provider %q not implemented in P01 (only \"github\" is wired)", req.Provider),
		}
	}
	if req.Code == "" {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing code"}
	}
	if req.PKCEVerifier == "" {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing pkce_verifier"}
	}

	// PKCE verification. The BFF supplies `PKCEChallenge` from its
	// per-request store (or, for the integration-test surface, inline
	// on this request). When unset we skip the check (legacy path
	// for callers that have not migrated yet). A future bead will
	// promote this to mandatory once the BFF storage lands.
	if req.PKCEChallenge != "" && !pkceMatches(req.PKCEVerifier, req.PKCEChallenge) {
		return nil, &errs.Error{Code: errs.Unauthenticated, Message: "pkce verification failed"}
	}

	// Exchange the code for a GitHub access token.
	tok, err := exchangeGitHubCode(ctx, req.Code, secrets.GitHubOAuthClientID, secrets.GitHubOAuthClientSecret)
	if err != nil {
		rlog.Error("auth: github code exchange failed", "err", err)
		return nil, &errs.Error{Code: errs.Unauthenticated, Message: "oauth exchange failed"}
	}

	// Fetch the GitHub user profile so we can populate auth.users.
	ghUser, err := fetchGitHubUser(ctx, tok.AccessToken)
	if err != nil {
		rlog.Error("auth: github user fetch failed", "err", err)
		return nil, &errs.Error{Code: errs.Internal, Message: "github user fetch failed"}
	}
	if ghUser.Email == "" {
		// GitHub returns an empty email when the user marked it
		// private. P02 will add a fallback to /user/emails; in P01
		// we reject the exchange so we never insert a row violating
		// the auth.users.email NOT NULL invariant.
		return nil, &errs.Error{Code: errs.FailedPrecondition, Message: "github user has no public email"}
	}

	tx, err := db.Begin(ctx)
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "db begin failed"}
	}
	// Roll back on any early return. The `committed` flag suppresses the
	// rollback once Commit succeeds: pgx would swallow the post-Commit
	// ErrTxClosed as a no-op anyway, but gating it keeps the intent
	// explicit and avoids the cosmetically-loose double-finalize.
	committed := false
	defer func() {
		if !committed {
			_ = tx.Rollback()
		}
	}()

	// Upsert auth.users on (primary_provider, primary_provider_id).
	// We mint a fresh ULID only when no existing row matches.
	userID, err := upsertGitHubUser(ctx, tx, ghUser)
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: fmt.Sprintf("upsert users: %v", err)}
	}

	// Upsert auth.oauth_tokens. The schema's UNIQUE (user_id,
	// provider) index enforces one row per (user, provider) pair —
	// rotation overwrites in place. The pgcrypto encryption uses the
	// MEMORY_DEK secret per SPEC §3.5 / §9.4.1.
	if err := upsertGitHubOAuthToken(ctx, tx, userID, tok, ghUser); err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: fmt.Sprintf("upsert oauth_tokens: %v", err)}
	}

	// Mint a fresh session.
	sessionID, err := newULID()
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "session id generation failed"}
	}
	now := time.Now().UTC()
	expiresAt := now.Add(defaultAPIKeySessionTTL)
	_, err = tx.Exec(ctx,
		`INSERT INTO auth.sessions (id, user_id, issued_at, last_seen_at, expires_at, user_agent, ip_inet)
		 VALUES ($1, $2, $3, $3, $4, $5, NULLIF($6, '')::inet)`,
		sessionID, userID, now, expiresAt, req.UserAgent, req.IPAddress,
	)
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: fmt.Sprintf("insert session: %v", err)}
	}

	if err := tx.Commit(); err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: fmt.Sprintf("commit: %v", err)}
	}
	committed = true

	return &ExchangeOAuthCodeResponse{
		SessionID: sessionID,
		UserID:    userID,
		ExpiresAt: expiresAt,
	}, nil
}

// upsertGitHubUser inserts a fresh row or refreshes the existing one
// keyed by (primary_provider, primary_provider_id). Returns the
// (existing or newly-minted) ULID.
func upsertGitHubUser(ctx context.Context, tx *sqldb.Tx, gh *githubUserResponse) (string, error) {
	// We use a CTE so we get the row id back regardless of whether
	// INSERT or UPDATE fired. The ON CONFLICT clause keys on the
	// schema's existing UNIQUE constraint
	// (users_primary_provider_unique).
	//
	// COALESCE on display_name / email / avatar_url so a follow-up
	// login that returns a richer profile refreshes the row but a
	// missing field does not blank an existing one.
	displayName := gh.Name
	if displayName == "" {
		displayName = gh.Login
	}

	// newID is always minted, but only consumed on the INSERT path: the
	// $1 binding becomes the row id when no conflict fires. On the
	// ON CONFLICT DO UPDATE path Postgres keeps the existing row's id and
	// RETURNING yields that, so this freshly-minted ULID is discarded. A
	// wasted ULID per repeat login is cheaper than a pre-flight SELECT to
	// decide whether to mint one.
	newID, err := newULID()
	if err != nil {
		return "", err
	}
	providerID := fmt.Sprintf("%d", gh.ID)

	row := tx.QueryRow(ctx,
		`INSERT INTO auth.users (id, primary_provider, primary_provider_id, email, display_name, avatar_url)
		 VALUES ($1, 'github', $2, $3, $4, NULLIF($5, ''))
		 ON CONFLICT (primary_provider, primary_provider_id) DO UPDATE
		   SET email        = EXCLUDED.email,
		       display_name = EXCLUDED.display_name,
		       avatar_url   = COALESCE(EXCLUDED.avatar_url, auth.users.avatar_url),
		       updated_at   = now()
		 RETURNING id`,
		newID, providerID, gh.Email, displayName, gh.AvatarURL,
	)
	var id string
	if err := row.Scan(&id); err != nil {
		return "", err
	}
	return id, nil
}

// upsertGitHubOAuthToken writes (or refreshes) the oauth_tokens row.
// The *_enc columns use pgp_sym_encrypt with the MEMORY_DEK secret
// per SPEC §3.5; the key value is passed inline as the second
// argument of the SQL function.
func upsertGitHubOAuthToken(ctx context.Context, tx *sqldb.Tx, userID string, tok *githubAccessTokenResponse, _ *githubUserResponse) error {
	rowID, err := newULID()
	if err != nil {
		return err
	}
	scopes := splitScopes(tok.Scope)
	_, err = tx.Exec(ctx,
		`INSERT INTO auth.oauth_tokens
		   (id, user_id, provider, access_token_enc, refresh_token_enc, scopes, expires_at, rotated_at)
		 VALUES (
		   $1, $2, 'github',
		   pgp_sym_encrypt($3, $5),
		   CASE WHEN $4 = '' THEN NULL ELSE pgp_sym_encrypt($4, $5) END,
		   $6, NULL, now()
		 )
		 ON CONFLICT (user_id, provider) DO UPDATE
		   SET access_token_enc  = EXCLUDED.access_token_enc,
		       refresh_token_enc = EXCLUDED.refresh_token_enc,
		       scopes            = EXCLUDED.scopes,
		       rotated_at        = now()`,
		rowID, userID, tok.AccessToken, tok.RefreshToken, secrets.MemoryDEK, scopes,
	)
	return err
}

// splitScopes splits the comma-or-space delimited GitHub scope string
// into the text[] form auth.oauth_tokens.scopes expects. Empty input
// yields an empty slice (the schema default is '{}').
func splitScopes(raw string) []string {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return []string{}
	}
	// GitHub returns space-separated; older docs said comma. Accept
	// both for defensive parsing.
	fields := strings.FieldsFunc(raw, func(r rune) bool { return r == ' ' || r == ',' })
	out := make([]string, 0, len(fields))
	for _, f := range fields {
		f = strings.TrimSpace(f)
		if f != "" {
			out = append(out, f)
		}
	}
	return out
}

// IssueAPIKeyRequest is the input to IssueAPIKey. SPEC §4.1.
type IssueAPIKeyRequest struct {
	OrgID        string     `json:"org_id"`         // ULID
	IssuedToUser string     `json:"issued_to_user"` // ULID; nullable (org-level service key)
	Label        string     `json:"label"`          // human-readable, e.g. "claude-code-laptop"
	AgentKind    string     `json:"agent_kind"`     // AgentKind value
	Scopes       []string   `json:"scopes"`
	ExpiresAt    *time.Time `json:"expires_at"` // nullable; default: never
}

// IssueAPIKeyResponse is the output of IssueAPIKey. SPEC §4.1.
type IssueAPIKeyResponse struct {
	KeyID     string `json:"key_id"`     // ULID (mcp.api_keys.id)
	KeyPrefix string `json:"key_prefix"` // first 8 chars of the random base32 portion
	RawKey    string `json:"raw_key"`    // FULL raw key — returned ONCE; never persisted in clear
}

// IssueAPIKey creates a new mcp.api_keys row. In P01 it is invoked from
// test seeds via direct INSERT (the E2E test under
// apps/api/exitcriteriontest/ writes the row straight to mcp.api_keys
// with key_hash computed via secrets.APIKeyHMACSecret per
// apps/api/auth/apikey.go:103-111 — see spec §11.1.1, round-12).
// Operator-facing surfaces (CLI or web admin) are deferred to a future
// phase. Returns the raw key ONCE — the caller stores it; subsequent
// reads return only the prefix and metadata.
//
//encore:api private method=POST path=/auth.IssueAPIKey
func IssueAPIKey(ctx context.Context, req *IssueAPIKeyRequest) (*IssueAPIKeyResponse, error) {
	if req == nil {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing request body"}
	}
	if req.OrgID == "" {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing org_id"}
	}
	if req.Label == "" {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing label"}
	}
	if _, ok := agentKindAllowed[req.AgentKind]; !ok {
		// Catch the CHECK constraint client-side so the caller gets
		// a clean 400 instead of a Postgres-flavoured 500.
		return nil, &errs.Error{
			Code:    errs.InvalidArgument,
			Message: fmt.Sprintf("invalid agent_kind %q (allowed: claude-code, copilot, cursor, codex, aider, custom)", req.AgentKind),
		}
	}

	keyID, err := newULID()
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "key id generation failed"}
	}
	rawKey, err := generateRawKey()
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "raw key generation failed"}
	}
	prefix, err := prefixOf(rawKey)
	if err != nil {
		// Should be impossible — generateRawKey produces well-formed
		// keys by construction. Defensive guard against future drift.
		return nil, &errs.Error{Code: errs.Internal, Message: "key prefix derivation failed"}
	}
	hash := hashRawKey(secrets.APIKeyHMACSecret, rawKey)

	scopes := req.Scopes
	if scopes == nil {
		scopes = []string{}
	}
	var issuedToUser any
	if req.IssuedToUser != "" {
		issuedToUser = req.IssuedToUser
	}

	_, err = db.Exec(ctx,
		`INSERT INTO mcp.api_keys
		   (id, org_id, issued_to_user, label, agent_kind, key_hash, key_prefix, scopes, expires_at)
		 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)`,
		keyID, req.OrgID, issuedToUser, req.Label, req.AgentKind, hash, prefix, scopes, req.ExpiresAt,
	)
	if err != nil {
		// Do NOT include rawKey in the log — it has not been
		// returned to the caller yet and is the only copy in
		// memory; an error log here would be the only persistent
		// trace of the secret.
		rlog.Error("auth: api_key insert failed", "err", err, "org_id", req.OrgID, "agent_kind", req.AgentKind)
		return nil, &errs.Error{Code: errs.Internal, Message: "issue api key failed"}
	}

	return &IssueAPIKeyResponse{
		KeyID:     keyID,
		KeyPrefix: prefix,
		RawKey:    rawKey,
	}, nil
}

// RevokeAPIKeyRequest is the input to RevokeAPIKey. SPEC §4.1.
type RevokeAPIKeyRequest struct {
	KeyID string `json:"key_id"` // ULID
}

// RevokeAPIKey flips revoked_at; idempotent.
//
// Idempotency contract: a second Revoke call on the same key is a
// no-op (revoked_at is preserved at the first-revoke timestamp via
// COALESCE). Revoking a non-existent key is also accepted silently —
// the caller cannot distinguish a key it never had from one already
// revoked, and revoking what you don't own is harmless. A future
// bead may add an authorization check (the caller must own org_id).
//
//encore:api private method=POST path=/auth.RevokeAPIKey
func RevokeAPIKey(ctx context.Context, req *RevokeAPIKeyRequest) error {
	if req == nil || req.KeyID == "" {
		return &errs.Error{Code: errs.InvalidArgument, Message: "missing key_id"}
	}
	_, err := db.Exec(ctx,
		`UPDATE mcp.api_keys
		   SET revoked_at = COALESCE(revoked_at, now())
		 WHERE id = $1`,
		req.KeyID,
	)
	if err != nil {
		rlog.Error("auth: api_key revoke failed", "err", err, "key_id", req.KeyID)
		return &errs.Error{Code: errs.Internal, Message: "revoke api key failed"}
	}
	return nil
}
