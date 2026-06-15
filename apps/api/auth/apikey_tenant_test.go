// DB-bound cross-tenant tests for the IssueAPIKey / RevokeAPIKey tenant
// gates (bead unblock-tv8.85, SPEC §4.1 / §10.1.1).
//
// These tests exercise the LATENT cross-tenant write IDORs the gates
// close: with a pinned caller identity (CallerUserID on IssueAPIKey,
// CallerOrgID on RevokeAPIKey) a caller may neither issue a key into a
// foreign org / to a non-member user, nor revoke another tenant's key.
// They also pin the empty-caller NO-OP (the §11.1.1 seed / integration /
// mcpaudit / perf callers pass no caller identity, so the gate is
// dormant) — that path MUST keep working.
//
// Package choice: this is an EXTERNAL test package (auth_test) so it can
// blank-import encore.app/db to trigger that service's init(), which
// calls auth.BindDB(DB) (and every other consumer's BindDB hook),
// populating the auth package's db handle before any subtest runs. An
// internal (package auth) test cannot import encore.app/db without an
// import cycle (db imports auth to call BindDB). IssueAPIKey /
// RevokeAPIKey are exported, so the external package reaches them
// directly; seeding uses the exported db.DB handle.
//
// Runner note: these require the Encore Docker cluster (db is bound by
// apps/api/db's init) and the secrets overlay from
// apps/api/.secrets.local.cue. Run under `encore test ./auth/...`; plain
// `go test ./auth/...` panics at package init by design (see
// apps/api/auth/db.go).

package auth_test

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"testing"
	"time"

	"encore.app/auth"
	// Blank import: triggers encore.app/db's init(), which calls
	// auth.BindDB(DB) so auth's db handle is non-nil. db.DB is also used
	// directly here to seed the two-org fixture.
	encoredb "encore.app/db"
	"encore.app/shared/ulid"
	"encore.dev/beta/errs"
	"encore.dev/storage/sqldb"
)

// errCodeOf extracts the errs.ErrCode from an error by type assertion
// (the errs.Code runtime helper is a stub under encore test).
func errCodeOf(err error) errs.ErrCode {
	if err == nil {
		return errs.OK
	}
	if e, ok := err.(*errs.Error); ok {
		return e.Code
	}
	return errs.Unknown
}

// tenantFixture is a minimal two-org graph. OrgA's owner is UserA; OrgB's
// owner is UserB. KeyB is an existing mcp.api_keys row owned by OrgB.
// OutsiderUser exists but belongs to NO org.
type tenantFixture struct {
	OrgA         string
	OrgB         string
	UserA        string
	UserB        string
	OutsiderUser string
	KeyB         string
}

// seedTenantFixture builds the two-org graph via db.DB. Any failure is
// fatal — a half-seeded fixture would yield misleading assertions.
func seedTenantFixture(ctx context.Context, t *testing.T) tenantFixture {
	t.Helper()

	mustULID := func(what string) string {
		id, err := ulid.New()
		if err != nil {
			t.Fatalf("ulid for %s: %v", what, err)
		}
		return id
	}

	fx := tenantFixture{
		OrgA:         mustULID("orgA"),
		OrgB:         mustULID("orgB"),
		UserA:        mustULID("userA"),
		UserB:        mustULID("userB"),
		OutsiderUser: mustULID("outsider"),
		KeyB:         mustULID("keyB"),
	}

	// Two orgs. Slug is UNIQUE — derive from the (already unique) ULID.
	for _, org := range []struct{ id, name string }{
		{fx.OrgA, "Tenant Gate Org A"},
		{fx.OrgB, "Tenant Gate Org B"},
	} {
		slug := "tg-" + strings.ToLower(org.id)
		if _, err := encoredb.DB.Exec(ctx,
			`INSERT INTO org.organizations (id, slug, name) VALUES ($1, $2, $3)`,
			org.id, slug, org.name,
		); err != nil {
			t.Fatalf("insert org %s: %v", org.name, err)
		}
	}

	// Three users. Both primary_provider_id (UNIQUE per provider) and
	// email (users_email_active_uniq among active users) must be unique
	// across every fixture instance on a shared dev cluster — salt both
	// with the user ULID (same shape as exitcriteriontest/seed.go).
	for _, u := range []struct{ id, label string }{
		{fx.UserA, "a"},
		{fx.UserB, "b"},
		{fx.OutsiderUser, "outsider"},
	} {
		email := fmt.Sprintf("%s-%s@example.com", u.label, strings.ToLower(u.id))
		if _, err := encoredb.DB.Exec(ctx,
			`INSERT INTO auth.users
			   (id, primary_provider, primary_provider_id, email, display_name)
			 VALUES ($1, 'github', $2, $3, $4)`,
			u.id, fmt.Sprintf("p-%s", u.id), email, u.label,
		); err != nil {
			t.Fatalf("insert user %s: %v", email, err)
		}
	}

	// Memberships: UserA owns OrgA, UserB owns OrgB. OutsiderUser is
	// deliberately a member of NOTHING.
	for _, m := range []struct{ org, user string }{
		{fx.OrgA, fx.UserA},
		{fx.OrgB, fx.UserB},
	} {
		if _, err := encoredb.DB.Exec(ctx,
			`INSERT INTO org.members (id, org_id, user_id, role)
			 VALUES ($1, $2, $3, 'owner')`,
			mustULID("member"), m.org, m.user,
		); err != nil {
			t.Fatalf("insert member org=%s user=%s: %v", m.org, m.user, err)
		}
	}

	// An existing API key owned by OrgB / UserB, for the revoke tests.
	// Revoke is by id, so the key_hash need not be a real HMAC — the
	// gate predicate is purely on org_id. key_prefix is UNIQUE; derive
	// it from the ULID.
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO mcp.api_keys
		   (id, org_id, issued_to_user, label, agent_kind, key_hash, key_prefix, scopes)
		 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)`,
		fx.KeyB, fx.OrgB, fx.UserB, "orgB-key", "claude-code",
		[]byte("dummy-hash-32-bytes-padding-xxxx"),
		// key_prefix is UNIQUE. ULID chars [10:26] are the random
		// portion (chars [0:10] are the shared-millisecond timestamp,
		// which collides across fixtures seeded in the same ms), so the
		// tail is reliably unique.
		strings.ToLower(fx.KeyB[len(fx.KeyB)-8:]),
		[]string{},
	); err != nil {
		t.Fatalf("insert mcp.api_keys: %v", err)
	}

	return fx
}

// keyRevokedAt reports whether the named key exists and whether it has a
// non-null revoked_at. revoked_at is a timestamptz — scan into
// *time.Time (a *string scan would error on a non-null timestamp and
// mask a successful revoke). A genuine missing row (ErrNoRows) returns
// (false, false); any other query error is fatal so a Scan bug cannot
// silently masquerade as "not revoked".
func keyRevokedAt(ctx context.Context, t *testing.T, keyID string) (exists, revoked bool) {
	t.Helper()
	var revokedAt *time.Time
	err := encoredb.DB.QueryRow(ctx,
		`SELECT revoked_at FROM mcp.api_keys WHERE id = $1`, keyID,
	).Scan(&revokedAt)
	if errors.Is(err, sqldb.ErrNoRows) {
		return false, false
	}
	if err != nil {
		t.Fatalf("keyRevokedAt(%s): %v", keyID, err)
	}
	return true, revokedAt != nil
}

// keyCountForOrg counts mcp.api_keys rows owned by the given org issued
// to the given user — used to prove a rejected IssueAPIKey inserts
// NOTHING.
func keyCountForOrg(ctx context.Context, t *testing.T, orgID, user string) int {
	t.Helper()
	var n int
	if err := encoredb.DB.QueryRow(ctx,
		`SELECT count(*) FROM mcp.api_keys WHERE org_id = $1 AND issued_to_user = $2`,
		orgID, user,
	).Scan(&n); err != nil {
		t.Fatalf("count api_keys: %v", err)
	}
	return n
}

// TestRevokeAPIKeyTenantGate covers the RevokeAPIKey CallerOrgID gate.
func TestRevokeAPIKeyTenantGate(t *testing.T) {
	ctx := context.Background()
	fx := seedTenantFixture(ctx, t)

	// Cross-tenant: OrgA's caller cannot revoke OrgB's key — NOT_FOUND,
	// and the key MUST remain un-revoked (the IDOR).
	t.Run("foreign-org key_id is NOT_FOUND and not revoked", func(t *testing.T) {
		err := auth.RevokeAPIKey(ctx, &auth.RevokeAPIKeyRequest{
			KeyID:       fx.KeyB,
			CallerOrgID: fx.OrgA, // caller owns OrgA, key belongs to OrgB
		})
		if code := errCodeOf(err); code != errs.NotFound {
			t.Fatalf("err code = %v, want NotFound", code)
		}
		exists, revoked := keyRevokedAt(ctx, t, fx.KeyB)
		if !exists {
			t.Fatalf("KeyB vanished — should be untouched")
		}
		if revoked {
			t.Fatalf("KeyB was revoked cross-tenant — IDOR not closed")
		}
	})

	// Same-org positive control: OrgB's caller revokes OrgB's key, then
	// re-revokes (idempotent).
	t.Run("same-org revoke succeeds and is idempotent", func(t *testing.T) {
		if err := auth.RevokeAPIKey(ctx, &auth.RevokeAPIKeyRequest{
			KeyID:       fx.KeyB,
			CallerOrgID: fx.OrgB,
		}); err != nil {
			t.Fatalf("same-org revoke: %v", err)
		}
		if _, revoked := keyRevokedAt(ctx, t, fx.KeyB); !revoked {
			t.Fatalf("KeyB not revoked after same-org revoke")
		}
		if err := auth.RevokeAPIKey(ctx, &auth.RevokeAPIKeyRequest{
			KeyID:       fx.KeyB,
			CallerOrgID: fx.OrgB,
		}); err != nil {
			t.Fatalf("idempotent re-revoke: %v", err)
		}
	})

	// Empty-caller NO-OP (dormant gate): a no-identity caller (the
	// trusted seed / integration path) may still revoke any existing
	// key — the path §11.1.1 callers travel.
	t.Run("empty CallerOrgID no-op still revokes existing key", func(t *testing.T) {
		fresh := seedTenantFixture(ctx, t)
		if err := auth.RevokeAPIKey(ctx, &auth.RevokeAPIKeyRequest{
			KeyID: fresh.KeyB, // no CallerOrgID
		}); err != nil {
			t.Fatalf("empty-caller revoke: %v", err)
		}
		if _, revoked := keyRevokedAt(ctx, t, fresh.KeyB); !revoked {
			t.Fatalf("empty-caller revoke did not revoke the key")
		}
	})
}

// TestIssueAPIKeyTenantGate covers the IssueAPIKey CallerUserID gate
// (caller-owns-org + issued_to_user membership).
func TestIssueAPIKeyTenantGate(t *testing.T) {
	ctx := context.Background()
	fx := seedTenantFixture(ctx, t)

	// Cross-tenant: UserA (member of OrgA only) tries to issue a key
	// INTO OrgB. The caller does not own OrgB → NOT_FOUND, nothing
	// inserted.
	t.Run("foreign org_id rejected, nothing inserted", func(t *testing.T) {
		before := keyCountForOrg(ctx, t, fx.OrgB, fx.UserB)
		_, err := auth.IssueAPIKey(ctx, &auth.IssueAPIKeyRequest{
			OrgID:        fx.OrgB,  // foreign to the caller
			IssuedToUser: fx.UserB, // a genuine OrgB member
			Label:        "evil",
			AgentKind:    "claude-code",
			CallerUserID: fx.UserA, // caller owns OrgA, not OrgB
		})
		if code := errCodeOf(err); code != errs.NotFound {
			t.Fatalf("err code = %v, want NotFound", code)
		}
		if after := keyCountForOrg(ctx, t, fx.OrgB, fx.UserB); after != before {
			t.Fatalf("a key was inserted into OrgB cross-tenant (before=%d after=%d)", before, after)
		}
	})

	// issued_to_user is not a member of the (owned) org → rejected.
	t.Run("non-member issued_to_user rejected, nothing inserted", func(t *testing.T) {
		before := keyCountForOrg(ctx, t, fx.OrgA, fx.OutsiderUser)
		_, err := auth.IssueAPIKey(ctx, &auth.IssueAPIKeyRequest{
			OrgID:        fx.OrgA,         // caller owns this
			IssuedToUser: fx.OutsiderUser, // member of NO org
			Label:        "to-outsider",
			AgentKind:    "claude-code",
			CallerUserID: fx.UserA,
		})
		if code := errCodeOf(err); code != errs.NotFound {
			t.Fatalf("err code = %v, want NotFound", code)
		}
		if after := keyCountForOrg(ctx, t, fx.OrgA, fx.OutsiderUser); after != before {
			t.Fatalf("a key was issued to a non-member (before=%d after=%d)", before, after)
		}
	})

	// Same-org positive control: UserA issues a key in OrgA to UserA.
	t.Run("same-org issue to member succeeds", func(t *testing.T) {
		before := keyCountForOrg(ctx, t, fx.OrgA, fx.UserA)
		resp, err := auth.IssueAPIKey(ctx, &auth.IssueAPIKeyRequest{
			OrgID:        fx.OrgA,
			IssuedToUser: fx.UserA,
			Label:        "legit",
			AgentKind:    "claude-code",
			CallerUserID: fx.UserA,
		})
		if err != nil {
			t.Fatalf("same-org issue: %v", err)
		}
		if resp == nil || resp.RawKey == "" {
			t.Fatalf("expected a raw key on success, got %+v", resp)
		}
		if after := keyCountForOrg(ctx, t, fx.OrgA, fx.UserA); after != before+1 {
			t.Fatalf("expected exactly one new key (before=%d after=%d)", before, after)
		}
	})

	// Empty-caller NO-OP (dormant gate): a no-identity caller may issue
	// a key with org/user it does not "own" — the §11.1.1 seed path.
	t.Run("empty CallerUserID no-op skips the gate", func(t *testing.T) {
		before := keyCountForOrg(ctx, t, fx.OrgB, fx.UserB)
		resp, err := auth.IssueAPIKey(ctx, &auth.IssueAPIKeyRequest{
			OrgID:        fx.OrgB,
			IssuedToUser: fx.UserB,
			Label:        "seed-style",
			AgentKind:    "claude-code",
			// CallerUserID intentionally empty → dormant no-op.
		})
		if err != nil {
			t.Fatalf("empty-caller issue: %v", err)
		}
		if resp == nil || resp.RawKey == "" {
			t.Fatalf("expected a raw key on success, got %+v", resp)
		}
		if after := keyCountForOrg(ctx, t, fx.OrgB, fx.UserB); after != before+1 {
			t.Fatalf("expected exactly one new key via no-op path (before=%d after=%d)", before, after)
		}
	})
}
