// secrets.go declares this package's view of the deployment-secrets
// manifest. We only consume APIKeyHMACSecret — seed.go computes
// `key_hash = HMAC-SHA256(APIKeyHMACSecret, rawKey)` via the same
// algorithm as `apps/api/auth/apikey.go::hashRawKey`, and the
// production Bearer hot path (`apps/api/auth/auth.go::validateAPIKey`)
// compares the stored `key_hash` against
// `HMAC-SHA256(APIKeyHMACSecret, rawKey)` on every request. The two
// computations MUST use the same secret value or the auth assertion
// fails before any latency measurement runs.
//
// Why declare the secret here instead of importing auth.secrets.
//
// `auth.secrets` is package-private to the auth package. The Go
// visibility rules forbid cross-package access. Encore allows multiple
// services to declare the same logical secret name; the production
// value is provisioned once via `encore secret set` and exposed to
// every declaring service. The local emulator reads from
// `apps/api/.secrets.local.cue`, shared with auth, mcp, and
// exitcriteriontest.
//
// Precedent: `apps/api/exitcriteriontest/secrets.go` declares the
// identical `var secrets struct { APIKeyHMACSecret string }`.

package perftest

// secrets is the perftest package's view of the deployment secrets
// manifest. Only APIKeyHMACSecret is consumed; the rest of the
// production secrets manifest is not relevant to the seed or the MCP
// transport.
//
// Encore reads the value from the configured secret store on process
// bootstrap (local emulator: apps/api/.secrets.local.cue). Under plain
// `go test` the value resolves to the empty string and the HMAC seed
// produces a digest the production hot path will never match — that is
// the symptom of running the suite without `encore test`. doc.go's
// "Encore-runtime requirement" section calls this out explicitly.
//
//nolint:unused // referenced by seed.go (computeKeyHash) and the audit trail in doc.go.
var secrets struct {
	APIKeyHMACSecret string
}
