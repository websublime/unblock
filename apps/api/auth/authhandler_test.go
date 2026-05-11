// Unit tests for the //encore:authhandler input parsing.
//
// Scope: parseBearer + AuthHandler dispatch (input validation only).
// Validate's DB path is exercised at integration time (encore test).

package auth

import (
	"context"
	"testing"

	"encore.dev/beta/errs"
)

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
		if code := errs.Code(err); code != errs.Unauthenticated {
			t.Fatalf("err code = %v, want Unauthenticated", code)
		}
	})

	t.Run("empty Authorization returns Unauthenticated", func(t *testing.T) {
		_, _, err := AuthHandler(context.Background(), &AuthParams{})
		if code := errs.Code(err); code != errs.Unauthenticated {
			t.Fatalf("err code = %v, want Unauthenticated", code)
		}
	})

	t.Run("malformed Authorization returns Unauthenticated", func(t *testing.T) {
		_, _, err := AuthHandler(context.Background(), &AuthParams{Authorization: "Token abc"})
		if code := errs.Code(err); code != errs.Unauthenticated {
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
		if code := errs.Code(err); code != errs.Unimplemented {
			t.Fatalf("err code = %v, want Unimplemented (P01 session-path deferral)", code)
		}
	})
}
