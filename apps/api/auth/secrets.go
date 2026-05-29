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
// values now surface uniformly as boot-time panics (bead unblock-tv8.57):
// APIKeyHMACSecret → boot panic in mcp/transport.go's init; MemoryDEK,
// GitHubOAuthClientID, GitHubOAuthClientSecret → boot panic in this file's
// init (below). The prior asymmetry — where the latter three resolved to ""
// and failed late on the OAuth/encrypt call path (auth.go:326,459) — is
// eliminated.
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

// init enforces boot-time fail-fast on the three auth secrets that were
// previously only validated at runtime (bead unblock-tv8.57). It mirrors
// the unconditional fail-fast the mcp service applies to its own secret at
// mcp/transport.go (the APIKeyHMACSecret cursor-signing guard): if Encore's
// synchronous secret resolution leaves any required value empty at process
// bootstrap, the process crashes immediately with an actionable message
// rather than limping into traffic and failing deep on a hot call path.
//
// Before this guard, the asymmetry documented in secrets.go was:
//   - APIKeyHMACSecret  → boot panic (mcp/transport.go) — fail-fast.
//   - MemoryDEK         → runtime panic at the first pgcrypto encrypt of an
//     oauth_tokens row (auth.go:459) — fails late, on the
//     OAuth callback path.
//   - GitHubOAuthClientID / GitHubOAuthClientSecret → silently resolve to ""
//     and produce a malformed/unauthorized GitHub code
//     exchange (auth.go:326) — fails late, with a remote
//     400/401 that masks the real cause (empty secret).
//
// Exercising the OAuth → token-exchange → encrypt path (e.g. the unblock-tv8
// exit-criterion E2E) would have surfaced those as confusing downstream
// failures. Panicking here makes every empty-secret misconfiguration surface
// uniformly at deploy time, visible in the service logs as a startup crash —
// the same operator experience the mcp guard already provides.
//
// Trade-off accepted (bead unblock-tv8.57): this guard supersedes the
// unblock-xuk invariant that plain `go test ./apps/api/auth/...` loaded the
// auth root package without Docker. With these secrets empty under plain
// `go test`, auth.init() now panics by design. The canonical gate for the
// auth root package is `encore test` (which populates secrets from
// apps/api/.secrets.local.cue and brings up the Docker cluster), so deploy-
// time fail-fast is the correct trade against go-test-without-Docker
// ergonomics. The leaf sub-package apps/api/auth/types/ does NOT import the
// auth root and so remains plain-`go test`-clean. See apps/api/auth/db.go
// for the full invariant-supersession note.
//
// Encore secret resolution is synchronous at process bootstrap, so by the
// time this init runs each value is either populated or definitively empty.
func init() {
	if secrets.MemoryDEK == "" {
		panic("auth: MemoryDEK is empty — provision via `encore secret set` (or apps/api/.secrets.local.cue) before boot; required for pgcrypto encryption of auth.oauth_tokens.*_enc (auth.go:459)")
	}
	if secrets.GitHubOAuthClientID == "" {
		panic("auth: GitHubOAuthClientID is empty — provision via `encore secret set` (or apps/api/.secrets.local.cue) before boot; required for the OAuth2+PKCE code exchange (auth.go:326)")
	}
	if secrets.GitHubOAuthClientSecret == "" {
		panic("auth: GitHubOAuthClientSecret is empty — provision via `encore secret set` (or apps/api/.secrets.local.cue) before boot; required for the OAuth2+PKCE code exchange (auth.go:326)")
	}
}
