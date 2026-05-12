// helpers.go gathers tiny shared utilities for the deps service.
// Kept distinct from deps.go so the RPC bodies file stays focused on
// the locked SPEC §4.5 surface.

package deps

import (
	"errors"
	"fmt"
	"strings"

	"encore.app/shared/ulid"
	"github.com/jackc/pgx/v5/pgconn"
)

// pgUniqueViolationCode is the Postgres SQLSTATE for unique_violation
// (Class 23 — integrity constraint violation). See
// https://www.postgresql.org/docs/current/errcodes-appendix.html.
const pgUniqueViolationCode = "23505"

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
// violation on the named constraint. Prefers typed matching via
// pgconn.PgError (SQLSTATE 23505 + ConstraintName) — pgx/v5 is
// transitively present through Encore's sqldb wrapping. Falls back to
// the legacy substring match when the error has been re-wrapped
// through a path that hides the typed *pgconn.PgError (defence in
// depth — review L6-S2). Mirrors workitems.isUniqueViolation and
// org.isUniqueViolation — kept package-local to avoid a shared helper
// package for a small predicate.
func isUniqueViolation(err error, constraint string) bool {
	if err == nil {
		return false
	}
	var pgErr *pgconn.PgError
	if errors.As(err, &pgErr) && pgErr.Code == pgUniqueViolationCode {
		return pgErr.ConstraintName == constraint
	}
	// Fallback: typed unwrap missed it. Match on SQLSTATE token rather
	// than the English "duplicate key" prose so locale changes can't
	// silently break the predicate.
	msg := err.Error()
	return (strings.Contains(msg, "SQLSTATE "+pgUniqueViolationCode) ||
		strings.Contains(msg, "duplicate key")) &&
		strings.Contains(msg, constraint)
}
