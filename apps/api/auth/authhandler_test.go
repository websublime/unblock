// Unit tests for the //encore:authhandler input parsing.
//
// Scope: AuthHandler dispatch (input validation only). Bearer-header
// parsing now lives in apps/api/shared/httpauth and is covered by
// httpauth_test.go. Validate's DB path is exercised at integration
// time (encore test).
//
// Test-runtime constraint (bead unblock-xuk follow-on): we read
// err.Code by direct type assertion on *errs.Error rather than calling
// errs.Code(err). errs.Code (like every other top-level helper in
// encore.dev/beta/errs) is a runtime-stub function that panics
// unconditionally with "encore apps must be run using the encore
// command" when invoked outside the Encore CLI's process bootstrap.
// AuthHandler returns a concrete *errs.Error in every input-error
// branch (see authhandler.go), so the assertion is safe (it avoids the
// errs.Code runtime-stub panic regardless of runner).
//
// Runner note (bead unblock-tv8.57): the auth root package now panics at
// init() on empty auth secrets (boot fail-fast in secrets.go, mirroring
// mcp), superseding the unblock-xuk plain-`go test`-loads invariant for
// this package. Run these under `encore test ./auth/...`, which populates
// secrets from apps/api/.secrets.local.cue. See apps/api/auth/db.go for the
// full rationale.

package auth

import (
	"context"
	"errors"
	"testing"

	"encore.dev/beta/errs"
)

// errCode extracts the errs.ErrCode from err by type assertion on
// *errs.Error. Returns errs.OK for nil, errs.Unknown for any non-nil
// error that is not (or does not wrap) *errs.Error. Mirrors the
// documented behaviour of the runtime errs.Code function (per the
// doc-comment on encore.dev/beta/errs.Code) without invoking the
// runtime stub that panics outside the Encore CLI.
func errCode(err error) errs.ErrCode {
	if err == nil {
		return errs.OK
	}
	var e *errs.Error
	if errors.As(err, &e) {
		return e.Code
	}
	return errs.Unknown
}

// TestAuthHandlerInputErrors verifies the input-validation paths of
// AuthHandler that do NOT require a working sqldb. The DB-bound paths
// (Validate calls into mcp.api_keys) are exercised under
// `encore test ./auth/...` once the Encore CLI is available.
func TestAuthHandlerInputErrors(t *testing.T) {
	t.Run("nil params returns Unauthenticated", func(t *testing.T) {
		_, _, err := AuthHandler(context.Background(), nil)
		if code := errCode(err); code != errs.Unauthenticated {
			t.Fatalf("err code = %v, want Unauthenticated", code)
		}
	})

	t.Run("empty Authorization returns Unauthenticated", func(t *testing.T) {
		_, _, err := AuthHandler(context.Background(), &AuthParams{})
		if code := errCode(err); code != errs.Unauthenticated {
			t.Fatalf("err code = %v, want Unauthenticated", code)
		}
	})

	t.Run("malformed Authorization returns Unauthenticated", func(t *testing.T) {
		_, _, err := AuthHandler(context.Background(), &AuthParams{Authorization: "Token abc"})
		if code := errCode(err); code != errs.Unauthenticated {
			t.Fatalf("err code = %v, want Unauthenticated", code)
		}
	})

	t.Run("BFF origin set: session path returns Unimplemented (P01 contract)", func(t *testing.T) {
		// SPEC §4.3.3 P01 contract: session path is deferred. The
		// dispatch happens inside AuthHandler before any DB call,
		// so this assertion is sound without a live database.
		_, _, err := AuthHandler(context.Background(), &AuthParams{
			Authorization: "Bearer some-session-id",
			BFFOrigin:     "https://unblock.websublime.com",
		})
		if code := errCode(err); code != errs.Unimplemented {
			t.Fatalf("err code = %v, want Unimplemented (P01 session-path deferral)", code)
		}
	})
}
