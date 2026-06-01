// This test deliberately lives in the `types` package and imports
// nothing from encore.dev/* or the auth root package. It is the
// executable lock on the leaf-cleanness contract documented in
// types.go: `apps/api/auth/types` MUST stay loadable under plain
// `go test ./auth/types/...` with no Docker and no Encore runtime.
//
// If a future edit drags an encore.dev import (directly or
// transitively) into this package, this test file stops compiling
// under plain `go test` and the regression is caught immediately —
// rather than surfacing far away as a package-init panic in every
// consumer that imports auth.Identity (org, rbac, …). See bead
// unblock-tv8.30 for the original extraction rationale and
// unblock-tv8.37 for this guard.
package types

import "testing"

// TestIdentityZeroValueIsConstructible pins two things at once:
//  1. the smoke fact that Identity's zero value is a valid, buildable
//     value (no required-init invariant sneaks in), and
//  2. the import-cleanness contract — this file builds and runs with no
//     Encore/Docker dependency, proving the package is still a leaf.
func TestIdentityZeroValueIsConstructible(t *testing.T) {
	var id Identity

	if id.UserID != "" || id.OrgID != "" || id.Role != "" || id.AgentKind != "" {
		t.Fatalf("zero-value Identity should have empty fields, got %+v", id)
	}

	// A fully-populated literal must also compile against the locked
	// field set (guards accidental field renames in the leaf type that
	// auth.Identity = types.Identity re-exports).
	_ = Identity{
		UserID:    "user-ulid",
		OrgID:     "org-ulid",
		Role:      "owner",
		AgentKind: "",
	}
}
