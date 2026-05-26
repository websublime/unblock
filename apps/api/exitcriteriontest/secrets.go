// secrets.go declares this service's view of the deployment-secrets
// manifest. We only consume APIKeyHMACSecret here — the seed in
// seed.go computes `key_hash = HMAC-SHA256(APIKeyHMACSecret, rawKey)`
// via the same algorithm as `apps/api/auth/apikey.go::hashRawKey`
// (lines 107-111), and the production Bearer hot path
// (`apps/api/auth/auth.go::validateAPIKey`) compares the stored
// `key_hash` against `HMAC-SHA256(APIKeyHMACSecret, rawKey)` on every
// request. The two computations MUST use the same secret value or
// the §11.1.2 auth assertion fails before any other check runs.
//
// Why declare the secret here instead of importing auth.secrets.
//
// `auth.secrets` is package-private to the auth package
// (`apps/api/auth/secrets.go:48-72`). The Go visibility rules forbid
// cross-package access. Encore allows multiple services to declare
// the same logical secret name; the production value is provisioned
// once via `encore secret set` and exposed to every declaring
// service. Local emulator reads from `apps/api/.secrets.local.cue`,
// shared with auth and mcp.
//
// Precedent: `apps/api/mcp/cursor.go:44-55` declares the identical
// `var secrets struct { APIKeyHMACSecret string }` for the §6.2.0
// cursor signing path. This file mirrors that pattern verbatim.
//
// SPEC anchors: §11.1.1 (round-12) — seed contract says the test
// computes `key_hash` using `secrets.APIKeyHMACSecret per the
// production hashing in apps/api/auth/apikey.go:103-111`; the seed's
// HMAC computation is exercised at boot in seed.go.

package exitcriteriontest

// secrets is the exitcriteriontest package's view of the deployment
// secrets manifest. Only APIKeyHMACSecret is consumed; the rest of
// the production secrets manifest is not relevant to the seed or the
// MCP transport assertions.
//
// Encore reads the value from the configured secret store on process
// bootstrap (local emulator: apps/api/.secrets.local.cue). Under
// plain `go test` the value resolves to the empty string and the
// HMAC seed produces a digest the production hot path will never
// match — that is the symptom of running the suite without
// `encore test`. doc.go's "Encore-runtime requirement" section calls
// this out explicitly.
//
//nolint:unused // referenced by seed.go (computeKeyHash) and the audit trail in doc.go.
var secrets struct {
	APIKeyHMACSecret string
}
