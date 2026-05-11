// OAuth2 + PKCE (S256) primitives used by ExchangeOAuthCode.
//
// SPEC §4.1 / §3.5: GitHub OAuth client id+secret are read from the
// Encore secrets manifest (`secrets.GitHubOAuthClientID`,
// `secrets.GitHubOAuthClientSecret`). GitLab is documented in the
// schema (auth.users.primary_provider CHECK accepts both) but P02
// owns the GitLab callback wiring per Plan §3.6 — P01 only exercises
// GitHub.
//
// Encryption note (DEFERRED): SPEC §3.5 / oauth_tokens.access_token_enc
// uses pgcrypto pgp_sym_encrypt with the `MEMORY_DEK` secret. The DEK
// encoding (base64 vs hex) was deferred on tv8.2; in P01 the
// integration tests stub the provider exchange so this code path is
// not exercised in production. See DECISION on the bead.

package auth

import (
	"context"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
)

// githubTokenEndpoint is the GitHub OAuth2 token-exchange URL.
// Overridable via oauthEndpointForTest in tests; the default is
// the canonical production endpoint.
var githubTokenEndpoint = "https://github.com/login/oauth/access_token"

// githubUserEndpoint is the GitHub /user lookup invoked after a
// successful token exchange to populate auth.users (display_name,
// email, avatar_url). Overridable for tests.
var githubUserEndpoint = "https://api.github.com/user"

// oauthHTTPClient is the HTTP client used for provider calls. Swapped
// in tests via the same package-level variable.
var oauthHTTPClient = http.DefaultClient

// pkceMatches verifies a PKCE S256 challenge against its verifier
// (RFC 7636 §4.6). The verifier is the original random value the
// client kept; the challenge is `BASE64URL(SHA256(verifier))` (no
// padding). The comparison is done in constant time even though both
// values are caller-supplied — defence in depth, the cost is
// negligible.
//
// `expectedChallenge` is the value the client sent at /authorize and
// the server (i.e. this code) is expected to have associated with the
// authorisation code. In P01 we accept the challenge directly on the
// ExchangeOAuthCode call (a future BFF will store it server-side and
// look it up by code).
func pkceMatches(verifier, expectedChallenge string) bool {
	if verifier == "" || expectedChallenge == "" {
		return false
	}
	sum := sha256.Sum256([]byte(verifier))
	got := base64.RawURLEncoding.EncodeToString(sum[:])
	return constantTimeStringEq(got, expectedChallenge)
}

// constantTimeStringEq compares two strings in time linear in the
// shorter input but independent of where they differ. Returns false
// on length mismatch (the length channel itself is not constant-time
// — acceptable for the PKCE use case where both values have a fixed
// 43-char length).
func constantTimeStringEq(a, b string) bool {
	if len(a) != len(b) {
		return false
	}
	var diff byte
	for i := 0; i < len(a); i++ {
		diff |= a[i] ^ b[i]
	}
	return diff == 0
}

// githubAccessTokenResponse mirrors the JSON body GitHub returns from
// the token-exchange endpoint when `Accept: application/json` is set.
// Only the fields we persist are unmarshalled; unknown keys are
// ignored by encoding/json.
type githubAccessTokenResponse struct {
	AccessToken      string `json:"access_token"`
	TokenType        string `json:"token_type"`
	Scope            string `json:"scope"`
	RefreshToken     string `json:"refresh_token"`
	Error            string `json:"error"`
	ErrorDescription string `json:"error_description"`
}

// githubUserResponse is the minimal /user payload we need for
// auth.users insertion. GitHub's `login` is the username; `id` is the
// stable numeric provider id (we serialise it as the primary key to
// keep auth.users.primary_provider_id text-typed per the schema).
type githubUserResponse struct {
	ID        int64  `json:"id"`
	Login     string `json:"login"`
	Name      string `json:"name"`
	Email     string `json:"email"`
	AvatarURL string `json:"avatar_url"`
}

// exchangeGitHubCode performs the OAuth2 code-for-token swap against
// GitHub. Returns the access-token response payload. Caller is
// responsible for translating non-200 responses into errs.* codes
// (typically Unauthenticated for 401, Internal for 5xx).
//
// The function uses the package-level oauthHTTPClient so tests can
// inject an httptest server without monkey-patching.
func exchangeGitHubCode(ctx context.Context, code, clientID, clientSecret string) (*githubAccessTokenResponse, error) {
	form := url.Values{}
	form.Set("client_id", clientID)
	form.Set("client_secret", clientSecret)
	form.Set("code", code)

	req, err := http.NewRequestWithContext(ctx, http.MethodPost, githubTokenEndpoint, strings.NewReader(form.Encode()))
	if err != nil {
		return nil, fmt.Errorf("auth: build github token request: %w", err)
	}
	req.Header.Set("Accept", "application/json")
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")

	resp, err := oauthHTTPClient.Do(req)
	if err != nil {
		return nil, fmt.Errorf("auth: github token exchange: %w", err)
	}
	defer func() { _ = resp.Body.Close() }()

	body, err := io.ReadAll(io.LimitReader(resp.Body, 1<<16))
	if err != nil {
		return nil, fmt.Errorf("auth: read github token response: %w", err)
	}
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("auth: github token exchange status %d", resp.StatusCode)
	}

	var out githubAccessTokenResponse
	if err := json.Unmarshal(body, &out); err != nil {
		return nil, fmt.Errorf("auth: parse github token response: %w", err)
	}
	if out.Error != "" {
		return nil, fmt.Errorf("auth: github oauth error %q: %s", out.Error, out.ErrorDescription)
	}
	if out.AccessToken == "" {
		return nil, fmt.Errorf("auth: github returned empty access_token")
	}
	return &out, nil
}

// fetchGitHubUser invokes GET /user with the freshly issued access
// token and returns the user payload used to populate auth.users.
func fetchGitHubUser(ctx context.Context, accessToken string) (*githubUserResponse, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, githubUserEndpoint, nil)
	if err != nil {
		return nil, fmt.Errorf("auth: build github user request: %w", err)
	}
	req.Header.Set("Authorization", "Bearer "+accessToken)
	req.Header.Set("Accept", "application/vnd.github+json")

	resp, err := oauthHTTPClient.Do(req)
	if err != nil {
		return nil, fmt.Errorf("auth: github user fetch: %w", err)
	}
	defer func() { _ = resp.Body.Close() }()

	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("auth: github user status %d", resp.StatusCode)
	}
	var out githubUserResponse
	if err := json.NewDecoder(io.LimitReader(resp.Body, 1<<16)).Decode(&out); err != nil {
		return nil, fmt.Errorf("auth: parse github user response: %w", err)
	}
	return &out, nil
}
