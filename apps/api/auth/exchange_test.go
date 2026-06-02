// Integration-adjacent tests for ExchangeOAuthCode input validation.
//
// Scope (bead unblock-tv8.38 W2): the BFF-readiness IP-address guard. A
// non-empty but unparseable IPAddress must be rejected with
// errs.InvalidArgument *before* any provider exchange or DB work, rather
// than fall through to the session INSERT's NULLIF($6, '')::inet cast and
// fail the whole pgx transaction with an opaque Postgres error surfaced as
// errs.Internal (a generic 500 / DoS surface once the BFF forwards an
// upstream-controlled X-Forwarded-For).
//
// Runner note (bead unblock-tv8.57): the auth root package panics at init()
// on empty auth secrets (boot fail-fast in secrets.go). Run these under
// `encore test ./auth/...`, which populates secrets from
// apps/api/.secrets.local.cue and brings up the Docker cluster. The guard
// under test returns before reaching sqldb, so the assertion does not
// depend on the DB — but the package still cannot load under plain
// `go test`. See apps/api/auth/db.go for the full rationale.
//
// We read err.Code by type assertion via the shared errCode helper
// (authhandler_test.go) rather than errs.Code, which panics as a runtime
// stub outside the Encore CLI. Every input-error branch of
// ExchangeOAuthCode returns a concrete *errs.Error, so the assertion is
// safe regardless of runner.

package auth

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"

	"encore.dev/beta/errs"
)

// TestExchangeOAuthCodeIPGuard verifies the W2 net.ParseIP guard: a
// malformed ip_address yields InvalidArgument (not a generic Internal /
// 500) and the validation happens before the GitHub exchange so no
// network or DB interaction is required.
func TestExchangeOAuthCodeIPGuard(t *testing.T) {
	tests := []struct {
		name      string
		ipAddress string
		wantCode  errs.ErrCode
	}{
		{
			name:      "malformed ip rejected with InvalidArgument",
			ipAddress: "not-an-ip",
			wantCode:  errs.InvalidArgument,
		},
		{
			name:      "garbage-with-dots rejected with InvalidArgument",
			ipAddress: "999.999.999.999",
			wantCode:  errs.InvalidArgument,
		},
		{
			name:      "trailing-junk on a valid prefix rejected",
			ipAddress: "10.0.0.1 OR 1=1",
			wantCode:  errs.InvalidArgument,
		},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			// A complete request that passes every prior validation
			// (provider/code/pkce_verifier) so the IP guard is the first
			// — and only — branch exercised. The guard returns before the
			// GitHub exchange, so the absence of an httptest stub is
			// intentional: reaching the network would mean the guard
			// failed to fire.
			req := &ExchangeOAuthCodeRequest{
				Provider:     "github",
				Code:         "abc123",
				PKCEVerifier: "the-quick-brown-fox-jumps-over-the-lazy-dog",
				IPAddress:    tc.ipAddress,
			}
			_, err := ExchangeOAuthCode(context.Background(), req)
			if err == nil {
				t.Fatalf("ExchangeOAuthCode(ip=%q) = nil error, want %v", tc.ipAddress, tc.wantCode)
			}
			if code := errCode(err); code != tc.wantCode {
				t.Fatalf("ExchangeOAuthCode(ip=%q) code = %v, want %v", tc.ipAddress, code, tc.wantCode)
			}
		})
	}
}

// TestExchangeOAuthCodeAcceptsValidAndEmptyIP confirms the guard does NOT
// reject well-formed IPv4/IPv6 or an empty string (empty maps to NULL via
// NULLIF in the session INSERT and must remain allowed). These inputs pass
// the IP guard and proceed to the GitHub exchange, which fails without a
// stub — so we assert the error code is NOT InvalidArgument (the guard did
// not fire), rather than asserting success.
func TestExchangeOAuthCodeAcceptsValidAndEmptyIP(t *testing.T) {
	// Stub the GitHub token endpoint so a valid/empty IP that passes the
	// guard fails deterministically at the exchange (returning an OAuth
	// error → errs.Unauthenticated) instead of making a live outbound
	// call to github.com. We assert only that the failure is NOT
	// InvalidArgument — i.e. the IP guard did not fire.
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"error":"bad_verification_code","error_description":"stub"}`))
	}))
	defer srv.Close()

	oldEndpoint, oldClient := githubTokenEndpoint, oauthHTTPClient
	githubTokenEndpoint = srv.URL
	oauthHTTPClient = srv.Client()
	t.Cleanup(func() {
		githubTokenEndpoint = oldEndpoint
		oauthHTTPClient = oldClient
	})

	tests := []struct {
		name      string
		ipAddress string
	}{
		{"empty ip allowed (maps to NULL)", ""},
		{"valid IPv4 allowed", "203.0.113.7"},
		{"valid IPv6 allowed", "2001:db8::1"},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			req := &ExchangeOAuthCodeRequest{
				Provider:     "github",
				Code:         "abc123",
				PKCEVerifier: "the-quick-brown-fox-jumps-over-the-lazy-dog",
				IPAddress:    tc.ipAddress,
			}
			_, err := ExchangeOAuthCode(context.Background(), req)
			// The stubbed exchange fails (bad_verification_code →
			// Unauthenticated), so we expect a non-nil error — but it
			// must NOT be InvalidArgument, which would mean the IP guard
			// wrongly rejected a valid/empty value.
			if err == nil {
				t.Fatalf("ExchangeOAuthCode(ip=%q) = nil error, want a non-InvalidArgument error", tc.ipAddress)
			}
			if code := errCode(err); code == errs.InvalidArgument {
				t.Fatalf("ExchangeOAuthCode(ip=%q) wrongly rejected with InvalidArgument", tc.ipAddress)
			}
		})
	}
}
