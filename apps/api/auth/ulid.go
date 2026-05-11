// ULID generation for the auth service.
//
// History (bead unblock-tv8.8 / B-2): the inline implementation
// previously lived here. It was lifted to `apps/api/shared/ulid` so
// org/, workitems/, deps/ (and future services) can mint ULIDs without
// re-declaring the algorithm. The package-private `newULID` alias is
// preserved so existing call sites in auth.go keep their original
// spelling and SPEC anchors stay intact (SPEC §4.1, §4.3.2).
//
// The shared package is leaf — Encore-free, importable from plain
// `go test` consumers — so taking the dependency here adds no runtime
// surface beyond what was already linked.

package auth

import (
	"encore.app/shared/ulid"
)

// newULID is the package-private alias retained for the existing call
// sites (`mcp.api_keys.id`, `auth.users.id`, `auth.oauth_tokens.id`,
// `auth.sessions.id`). It delegates to the shared canonical generator
// at `encore.app/shared/ulid`.
func newULID() (string, error) { return ulid.New() }

// crockfordAlphabet is retained as a package-private constant so the
// existing `ulid_test.go` (which validates generated values against
// the alphabet) compiles unchanged. Sourced from the shared package
// to guarantee the two stay in lockstep.
var crockfordAlphabet = ulid.Alphabet()
