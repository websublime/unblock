// helpers.go gathers tiny shared utilities for the deps service.
// Kept distinct from deps.go so the RPC bodies file stays focused on
// the locked SPEC §4.5 surface.

package deps

import (
	"fmt"
	"strings"

	"encore.app/shared/ulid"
)

// newULID wraps ulid.New with the package's standard error shape so
// callers can `if err != nil { return ... }` without re-wrapping at
// every site. The underlying crypto/rand failure is extremely rare.
func newULID() (string, error) {
	id, err := ulid.New()
	if err != nil {
		return "", fmt.Errorf("deps: ulid: %w", err)
	}
	return id, nil
}

// isUniqueViolation returns true when err is a Postgres UNIQUE
// violation on the named constraint. Matched by substring (pgx error
// wrapping varies by Encore version). Mirrors workitems.isUniqueViolation
// and org.isUniqueViolation — kept package-local to avoid a shared
// helper package for a 3-line predicate.
func isUniqueViolation(err error, constraint string) bool {
	if err == nil {
		return false
	}
	msg := err.Error()
	return strings.Contains(msg, "duplicate key") && strings.Contains(msg, constraint)
}
