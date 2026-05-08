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
// Production values are seeded by Olive via `encore secret set --type prod`
// before deploy; staging via `--type dev`. Local emulator reads the four
// fields from `apps/api/.secrets.local.cue` (gitignored).
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
