// Unit tests for IssueAPIKey input validation (bead unblock-tv8.73).
//
// Scope: the request-validation guards in IssueAPIKey that run BEFORE any
// sqldb access — in particular the round-16 contract that every MCP API key
// MUST be issued to a user (issued_to_user is REQUIRED; there is no userless
// "org-level service key"). An empty IssuedToUser is rejected with
// InvalidArgument before the function reaches db.Exec, so these assertions
// need no live database.
//
// The DB-bound happy path (issue a user-linked key, then resolve it through
// the Bearer auth handler to an Identity whose UserID is the non-empty
// owning user) is exercised end-to-end by the integration suites that seed
// real user-linked keys: apps/api/exitcriteriontest/, apps/api/perftest/,
// and apps/api/shared/mcpaudittest/.
//
// Runner note (bead unblock-tv8.57): the auth root package panics at init()
// on empty auth secrets (boot fail-fast in secrets.go). Plain
// `go test ./auth/...` therefore panics on package load by design — run
// these under `encore test ./auth/...`, which populates secrets from
// apps/api/.secrets.local.cue. See apps/api/auth/db.go for the rationale.

package auth

import (
	"context"
	"errors"
	"testing"

	"encore.dev/beta/errs"
)

// TestIssueAPIKeyInputErrors verifies the pre-DB request-validation guards of
// IssueAPIKey. Each case fails before any sqldb call, so no live database is
// required. errCode (defined in authhandler_test.go) reads the code by type
// assertion to avoid the errs.Code runtime stub.
func TestIssueAPIKeyInputErrors(t *testing.T) {
	const (
		validOrg   = "01J0000000000000000000ORG0"
		validUser  = "01J0000000000000000000USR0"
		validLabel = "claude-code-laptop"
		validKind  = "claude-code"
	)

	t.Run("nil request returns InvalidArgument", func(t *testing.T) {
		_, err := IssueAPIKey(context.Background(), nil)
		if code := errCode(err); code != errs.InvalidArgument {
			t.Fatalf("err code = %v, want InvalidArgument", code)
		}
	})

	t.Run("empty org_id returns InvalidArgument", func(t *testing.T) {
		_, err := IssueAPIKey(context.Background(), &IssueAPIKeyRequest{
			IssuedToUser: validUser,
			Label:        validLabel,
			AgentKind:    validKind,
		})
		if code := errCode(err); code != errs.InvalidArgument {
			t.Fatalf("err code = %v, want InvalidArgument", code)
		}
	})

	t.Run("empty label returns InvalidArgument", func(t *testing.T) {
		_, err := IssueAPIKey(context.Background(), &IssueAPIKeyRequest{
			OrgID:        validOrg,
			IssuedToUser: validUser,
			AgentKind:    validKind,
		})
		if code := errCode(err); code != errs.InvalidArgument {
			t.Fatalf("err code = %v, want InvalidArgument", code)
		}
	})

	// The keystone of bead unblock-tv8.73: a key with no owning user is
	// rejected up front. Without this guard the row would later be NULL on
	// issued_to_user (now forbidden by the NOT NULL constraint) and resolve
	// to an empty-UID Identity that Encore's auth handler rejects opaquely.
	t.Run("empty issued_to_user returns InvalidArgument", func(t *testing.T) {
		_, err := IssueAPIKey(context.Background(), &IssueAPIKeyRequest{
			OrgID:     validOrg,
			Label:     validLabel,
			AgentKind: validKind,
			// IssuedToUser intentionally omitted.
		})
		if code := errCode(err); code != errs.InvalidArgument {
			t.Fatalf("err code = %v, want InvalidArgument (key must be issued to a user)", code)
		}
		var e *errs.Error
		if !errors.As(err, &e) {
			t.Fatalf("err = %v, want *errs.Error", err)
		}
		if e.Message != "api key must be issued to a user" {
			t.Fatalf("err message = %q, want %q", e.Message, "api key must be issued to a user")
		}
	})

	t.Run("invalid agent_kind returns InvalidArgument", func(t *testing.T) {
		_, err := IssueAPIKey(context.Background(), &IssueAPIKeyRequest{
			OrgID:        validOrg,
			IssuedToUser: validUser,
			Label:        validLabel,
			AgentKind:    "not-a-real-agent",
		})
		if code := errCode(err); code != errs.InvalidArgument {
			t.Fatalf("err code = %v, want InvalidArgument", code)
		}
	})
}
