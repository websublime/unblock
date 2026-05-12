// Unit tests for the //encore:authhandler input parsing.
//
// Scope: parseBearer + AuthHandler dispatch (input validation only).
// Validate's DB path is exercised at integration time (encore test).
//
// Test-runtime constraint (bead unblock-xuk follow-on): we read
// err.Code by direct type assertion on *errs.Error rather than calling
// errs.Code(err). errs.Code (like every other top-level helper in
// encore.dev/beta/errs) is a runtime-stub function that panics
// unconditionally with "encore apps must be run using the encore
// command" when invoked outside the Encore CLI's process bootstrap.
// AuthHandler returns a concrete *errs.Error in every input-error
// branch (see authhandler.go), so the assertion is safe — and it lets
// these tests run under plain `go test ./auth/...` without Docker,
// which is the whole point of the package-load fix unblock-xuk
// landed.

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

func TestParseBearer(t *testing.T) {
	tests := []struct {
		name    string
		input   string
		wantTok string
		wantOK  bool
	}{
		{name: "canonical form", input: "Bearer abc", wantTok: "abc", wantOK: true},
		{name: "case-insensitive scheme", input: "bearer xyz", wantTok: "xyz", wantOK: true},
		{name: "empty", input: "", wantOK: false},
		{name: "scheme only", input: "Bearer ", wantOK: false},
		{name: "no scheme", input: "abc", wantOK: false},
		{name: "trailing space", input: "Bearer abc ", wantOK: false},
		{name: "leading space (after scheme)", input: "Bearer  abc", wantOK: false},
		{name: "wrong scheme", input: "Basic abc", wantOK: false},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			got, ok := parseBearer(tc.input)
			if ok != tc.wantOK {
				t.Fatalf("parseBearer(%q) ok=%v, want %v", tc.input, ok, tc.wantOK)
			}
			if got != tc.wantTok {
				t.Fatalf("parseBearer(%q) tok=%q, want %q", tc.input, got, tc.wantTok)
			}
		})
	}
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
