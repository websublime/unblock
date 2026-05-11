// Unit tests for the OAuth helpers.
//
// Network-bound paths (exchangeGitHubCode, fetchGitHubUser) use the
// package-level oauthHTTPClient + endpoint vars so an httptest.Server
// can stand in for GitHub.

package auth

import (
	"context"
	"crypto/sha256"
	"encoding/base64"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func TestPKCEMatches(t *testing.T) {
	verifier := "the-quick-brown-fox-jumps-over-the-lazy-dog"
	sum := sha256.Sum256([]byte(verifier))
	challenge := base64.RawURLEncoding.EncodeToString(sum[:])

	t.Run("correct verifier matches", func(t *testing.T) {
		if !pkceMatches(verifier, challenge) {
			t.Fatalf("expected pkceMatches to return true for valid pair")
		}
	})

	t.Run("wrong verifier rejected", func(t *testing.T) {
		if pkceMatches("not-the-verifier", challenge) {
			t.Fatalf("expected pkceMatches to return false for mismatched verifier")
		}
	})

	t.Run("empty verifier rejected", func(t *testing.T) {
		if pkceMatches("", challenge) {
			t.Fatalf("expected pkceMatches to return false for empty verifier")
		}
	})

	t.Run("empty challenge rejected", func(t *testing.T) {
		if pkceMatches(verifier, "") {
			t.Fatalf("expected pkceMatches to return false for empty challenge")
		}
	})
}

func TestSplitScopes(t *testing.T) {
	tests := []struct {
		name string
		in   string
		want []string
	}{
		{"empty input yields empty slice", "", []string{}},
		{"whitespace input yields empty slice", "   ", []string{}},
		{"space-separated", "repo user:email", []string{"repo", "user:email"}},
		{"comma-separated", "repo,user:email", []string{"repo", "user:email"}},
		{"mixed delimiters", "repo, user:email  read:org", []string{"repo", "user:email", "read:org"}},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			got := splitScopes(tc.in)
			if len(got) != len(tc.want) {
				t.Fatalf("got %v, want %v", got, tc.want)
			}
			for i := range got {
				if got[i] != tc.want[i] {
					t.Fatalf("got[%d]=%q, want %q", i, got[i], tc.want[i])
				}
			}
		})
	}
}

// TestExchangeGitHubCode wires an httptest server in place of GitHub
// and verifies the form-encoded POST shape and JSON parsing.
func TestExchangeGitHubCode(t *testing.T) {
	t.Run("happy path returns access token", func(t *testing.T) {
		srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			if r.Method != http.MethodPost {
				t.Errorf("method = %q, want POST", r.Method)
			}
			if got := r.Header.Get("Accept"); got != "application/json" {
				t.Errorf("Accept = %q, want application/json", got)
			}
			if err := r.ParseForm(); err != nil {
				t.Fatalf("ParseForm: %v", err)
			}
			if got := r.Form.Get("client_id"); got != "test-id" {
				t.Errorf("client_id = %q, want test-id", got)
			}
			if got := r.Form.Get("client_secret"); got != "test-secret" {
				t.Errorf("client_secret = %q, want test-secret", got)
			}
			if got := r.Form.Get("code"); got != "abc123" {
				t.Errorf("code = %q, want abc123", got)
			}
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`{"access_token":"gho_xxx","token_type":"bearer","scope":"repo user:email"}`))
		}))
		defer srv.Close()

		oldEndpoint, oldClient := githubTokenEndpoint, oauthHTTPClient
		githubTokenEndpoint = srv.URL
		oauthHTTPClient = srv.Client()
		t.Cleanup(func() {
			githubTokenEndpoint = oldEndpoint
			oauthHTTPClient = oldClient
		})

		got, err := exchangeGitHubCode(context.Background(), "abc123", "test-id", "test-secret")
		if err != nil {
			t.Fatalf("exchangeGitHubCode: %v", err)
		}
		if got.AccessToken != "gho_xxx" {
			t.Errorf("access_token = %q, want gho_xxx", got.AccessToken)
		}
		if got.Scope != "repo user:email" {
			t.Errorf("scope = %q, want %q", got.Scope, "repo user:email")
		}
	})

	t.Run("provider error response surfaces as error", func(t *testing.T) {
		srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`{"error":"bad_verification_code","error_description":"The code passed is incorrect or expired."}`))
		}))
		defer srv.Close()

		oldEndpoint, oldClient := githubTokenEndpoint, oauthHTTPClient
		githubTokenEndpoint = srv.URL
		oauthHTTPClient = srv.Client()
		t.Cleanup(func() {
			githubTokenEndpoint = oldEndpoint
			oauthHTTPClient = oldClient
		})

		_, err := exchangeGitHubCode(context.Background(), "expired", "id", "secret")
		if err == nil {
			t.Fatalf("expected error for github error response")
		}
		if !strings.Contains(err.Error(), "bad_verification_code") {
			t.Errorf("err = %v, want substring %q", err, "bad_verification_code")
		}
	})

	t.Run("non-200 status surfaces as error", func(t *testing.T) {
		srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
			w.WriteHeader(http.StatusInternalServerError)
		}))
		defer srv.Close()

		oldEndpoint, oldClient := githubTokenEndpoint, oauthHTTPClient
		githubTokenEndpoint = srv.URL
		oauthHTTPClient = srv.Client()
		t.Cleanup(func() {
			githubTokenEndpoint = oldEndpoint
			oauthHTTPClient = oldClient
		})

		_, err := exchangeGitHubCode(context.Background(), "code", "id", "secret")
		if err == nil {
			t.Fatalf("expected error for 500 response")
		}
	})
}

func TestFetchGitHubUser(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if got := r.Header.Get("Authorization"); got != "Bearer gho_xxx" {
			t.Errorf("Authorization = %q, want Bearer gho_xxx", got)
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"id":42,"login":"octocat","name":"The Octocat","email":"octo@github.com","avatar_url":"https://example.com/a.png"}`))
	}))
	defer srv.Close()

	oldEndpoint, oldClient := githubUserEndpoint, oauthHTTPClient
	githubUserEndpoint = srv.URL
	oauthHTTPClient = srv.Client()
	t.Cleanup(func() {
		githubUserEndpoint = oldEndpoint
		oauthHTTPClient = oldClient
	})

	got, err := fetchGitHubUser(context.Background(), "gho_xxx")
	if err != nil {
		t.Fatalf("fetchGitHubUser: %v", err)
	}
	if got.ID != 42 {
		t.Errorf("ID = %d, want 42", got.ID)
	}
	if got.Login != "octocat" {
		t.Errorf("Login = %q, want octocat", got.Login)
	}
	if got.Email != "octo@github.com" {
		t.Errorf("Email = %q, want octo@github.com", got.Email)
	}
}
