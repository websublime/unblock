package auth

// secrets is the Encore Go secrets manifest for the `unblock` backend. The
// auth service is the canonical consumer per SPEC §3.5 — every secret below
// is read here on the auth call path (Bearer-key validation, OAuth code
// exchange, oauth_tokens encryption).
//
// Field names are the wire identifiers used by both the Encore secret
// manager (`encore secret set --type {dev,prod} <FieldName>`) and the
// local-dev override file `apps/api/.secrets.local.cue` (CUE format, per
// Encore docs at https://encore.dev/docs/go/primitives/secrets). The
// SPEC §3.5 logical-name ↔ Go-field mapping is:
//
//	MEMORY_DEK              ↔ MemoryDEK
//	API_KEY_HMAC_SECRET     ↔ APIKeyHMACSecret
//	GITHUB_OAUTH_CLIENT_ID  ↔ GitHubOAuthClientID
//	GITHUB_OAUTH_CLIENT_SECRET ↔ GitHubOAuthClientSecret
//
// Provisioning policy across Encore Cloud env types (per tv8.56):
//
//	--type prod   — real production values (Olive seeds before prod deploy).
//	                MemoryDEK + APIKeyHMACSecret carry real long-lived
//	                values; rotating either is a destructive operation
//	                (invalidates issued API keys, paginated cursors, or
//	                encrypted oauth_tokens rows). GitHubOAuth* must be
//	                rotated to the real GitHub OAuth app credentials
//	                before the prod OAuth flow is first exercised.
//	--type dev    — staging values (Olive seeds before staging deploy).
//	--type pr     — CI placeholder values. Required so Encore Cloud's
//	                pre-deploy `encore test` step on PR / preview-env
//	                builds boots without tripping the mcp/transport.go
//	                fail-fast on APIKeyHMACSecret. Internal mapping:
//	                ephemeral → preview (encoredev/encore set.go:179).
//	--type local  — CI placeholder values. Provisioned defensively
//	                because Encore Cloud does not document which env type
//	                its pre-deploy CI test runner queries; covering all
//	                four documented types eliminates the variable.
//
// Local emulator (`encore run`) reads the four fields from
// `apps/api/.secrets.local.cue` (gitignored), which overlays on top of
// whatever the platform returns for `--type dev`.
//
// All four secrets MUST be set across all four types before deploy. Missing
// values surface as: APIKeyHMACSecret → boot panic (mcp/transport.go:114);
// MemoryDEK, GitHubOAuth* → runtime panic when their code path fires.
//
//nolint:unused // referenced by RPC bodies starting in beads B-1..D-3.
var secrets struct {
	// MemoryDEK is the pgcrypto symmetric data-encryption key used to
	// encrypt `auth.oauth_tokens.*_enc` columns (and, in P02, every
	// `memory.entries.body_enc`). 32 bytes; base64- or hex-encoded at rest.
	// Logical name: MEMORY_DEK.
	MemoryDEK string

	// APIKeyHMACSecret is the server-side secret used by
	// HMAC-SHA256(secret, raw_key) when hashing MCP Bearer keys per
	// research C7 (no argon2id; constant-time HMAC compare on lookup by
	// `mcp.api_keys.key_prefix`). Rotating this secret invalidates every
	// outstanding API key — operationally a one-time provisioning value.
	// Logical name: API_KEY_HMAC_SECRET.
	APIKeyHMACSecret string

	// GitHubOAuthClientID is the OAuth2+PKCE client identifier for the
	// `://unblock` GitHub OAuth app. Read by auth.ExchangeOAuthCode during
	// the code → token exchange. Logical name: GITHUB_OAUTH_CLIENT_ID.
	GitHubOAuthClientID string

	// GitHubOAuthClientSecret is the OAuth2+PKCE client secret paired with
	// GitHubOAuthClientID. Read by auth.ExchangeOAuthCode. Logical name:
	// GITHUB_OAUTH_CLIENT_SECRET.
	GitHubOAuthClientSecret string
}
