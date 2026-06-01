// Integration tests for the org service's six private RPCs against the
// real Encore-managed Postgres cluster (bead unblock-tv8.40 — review
// cleanup of unblock-tv8.8). The pre-existing org_test.go header
// promised "Integration tests for the six private RPCs against the
// real Encore-managed Postgres cluster", but every Test* in that file
// was helper/validation/Authorize-matrix only — no test exercised a
// real RPC body end-to-end against the DB. This file honours that
// header by adding happy-path coverage for CreateOrganization,
// CreateProject, GetOrganization, GetProject, AddMember, and Authorize.
//
// External `package org_test` shape (mirrors authorize_ordering_test.go):
// blank-importing encore.app/db fires the migration-owner service's
// init(), which calls org.BindDB(DB) (and every other consumer's
// BindDB hook), populating org.db before any subtest runs. Without it
// the test binary would load org in isolation, leave org.db == nil,
// and every RPC body would panic on a nil *sqldb.Database inside
// encore.dev/storage/sqldb.
//
// Reads (GetOrganization, GetProject) and Authorize require a caller
// auth.Identity in the request context. et.OverrideAuthInfo sets the
// Encore auth info for the current test request so callerIdentity(ctx)
// resolves to the seeded identity — the same payload shape
// (*auth.AuthData) the production authhandler returns.
//
// None of these tests call t.Parallel(): BindDB is not goroutine-safe
// (bead unblock-tv8.34) and et.OverrideAuthInfo mutates per-request
// auth info; every integration test in this service is single-threaded
// by convention.
//
// MUST run under `encore test ./apps/api/org/...` (Docker-backed
// cluster) — plain `go test` leaves org.db == nil and the RPC bodies
// panic before any assertion fires.

package org_test

import (
	"context"
	"strings"
	"testing"

	"encore.app/auth"
	// Importing encore.app/db triggers its init() which calls
	// org.BindDB(DB) and every other consumer's BindDB hook. The handle
	// (db.DB) is also used directly for the auth.users seed AddMember's
	// FK requires.
	encoredb "encore.app/db"
	"encore.app/org"
	"encore.app/shared/ulid"
	encoreauth "encore.dev/beta/auth"
	"encore.dev/beta/errs"
	"encore.dev/et"
)

// seedUser inserts a real auth.users row so AddMember's
// members_user_id_fkey FK is satisfiable. Returns the new user id and
// registers a best-effort cleanup. ULID-suffixed provider id / email
// avoid colliding with prior runs on the local Encore cluster.
func seedUser(t *testing.T, ctx context.Context) string {
	t.Helper()
	userID, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid (user): %v", err)
	}
	suffix := strings.ToLower(userID[len(userID)-12:])
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO auth.users (id, primary_provider, primary_provider_id, email, display_name)
		 VALUES ($1, 'github', $2, $3, $4)`,
		userID, "orgtest-"+suffix, suffix+"@orgtest.local", "orgtest user",
	); err != nil {
		t.Fatalf("insert auth.users: %v", err)
	}
	t.Cleanup(func() {
		if _, err := encoredb.DB.Exec(ctx, `DELETE FROM auth.users WHERE id = $1`, userID); err != nil {
			t.Logf("cleanup: delete auth.users %s: %v", userID, err)
		}
	})
	return userID
}

// seedOrg creates a real organization via the production
// CreateOrganization RPC and registers cascade-cleanup (ON DELETE
// CASCADE removes dependent org.projects / org.members rows).
func seedOrg(t *testing.T, ctx context.Context, label string) *org.Organization {
	t.Helper()
	suffix, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid (org suffix): %v", err)
	}
	o, err := org.CreateOrganization(ctx, &org.CreateOrganizationRequest{
		Name: "tv8-40 " + label,
		Slug: strings.ToLower("tv8-40-" + label + "-" + suffix[len(suffix)-12:]),
	})
	if err != nil {
		t.Fatalf("CreateOrganization(%s) failed: %v", label, err)
	}
	t.Cleanup(func() {
		if _, err := encoredb.DB.Exec(ctx, `DELETE FROM org.organizations WHERE id = $1`, o.ID); err != nil {
			t.Logf("cleanup: delete org %s: %v", o.ID, err)
		}
	})
	return o
}

// asUser sets the Encore auth context for the current test request so
// callerIdentity(ctx) resolves to an org-scoped human identity.
func asUser(userID, orgID string) {
	et.OverrideAuthInfo(encoreauth.UID(userID), &auth.AuthData{
		Identity: auth.Identity{
			UserID: userID,
			OrgID:  orgID,
			Role:   "owner",
		},
	})
}

// TestCreateOrganizationHappyPath exercises the CreateOrganization RPC
// body end-to-end and reads the row back via GetOrganization (the org
// owner reads its own organization).
func TestCreateOrganizationHappyPath(t *testing.T) {
	ctx := context.Background()
	userID := seedUser(t, ctx)
	o := seedOrg(t, ctx, "create")

	if o.ID == "" {
		t.Fatalf("CreateOrganization returned empty ID")
	}
	if o.Name != "tv8-40 create" {
		t.Fatalf("Name = %q, want %q", o.Name, "tv8-40 create")
	}

	// Read it back through the production GetOrganization path, scoped
	// to the owner's own org (GetOrganization denies any id != caller
	// OrgID with NotFound).
	asUser(userID, o.ID)
	got, err := org.GetOrganization(ctx, o.ID)
	if err != nil {
		t.Fatalf("GetOrganization failed: %v", err)
	}
	if got.ID != o.ID || got.Slug != o.Slug {
		t.Fatalf("GetOrganization round-trip mismatch: got %+v, want id=%s slug=%s", got, o.ID, o.Slug)
	}
}

// TestCreateAndGetProjectHappyPath exercises CreateProject and reads it
// back via GetProject (rbac-scoped to the caller's org).
func TestCreateAndGetProjectHappyPath(t *testing.T) {
	ctx := context.Background()
	userID := seedUser(t, ctx)
	o := seedOrg(t, ctx, "project")

	suffix, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid (project slug): %v", err)
	}
	p, err := org.CreateProject(ctx, &org.CreateProjectRequest{
		OrgID: o.ID,
		Name:  "tv8-40 project",
		Slug:  strings.ToLower("p-" + suffix[len(suffix)-12:]),
	})
	if err != nil {
		t.Fatalf("CreateProject failed: %v", err)
	}
	if p.ID == "" || p.OrgID != o.ID {
		t.Fatalf("CreateProject returned %+v, want non-empty id and org_id=%s", p, o.ID)
	}

	asUser(userID, o.ID)
	got, err := org.GetProject(ctx, p.ID)
	if err != nil {
		t.Fatalf("GetProject failed: %v", err)
	}
	if got.ID != p.ID || got.OrgID != o.ID {
		t.Fatalf("GetProject round-trip mismatch: got %+v, want id=%s org_id=%s", got, p.ID, o.ID)
	}
}

// TestAddMemberHappyPath exercises AddMember against a real org + real
// auth.users row (the FK members_user_id_fkey must resolve), then
// asserts a duplicate insert is rejected AlreadyExists via the
// typed-pgconn isUniqueViolation path on members_org_user_uniq.
func TestAddMemberHappyPath(t *testing.T) {
	ctx := context.Background()
	o := seedOrg(t, ctx, "member")
	userID := seedUser(t, ctx)

	if err := org.AddMember(ctx, &org.AddMemberRequest{
		OrgID:  o.ID,
		UserID: userID,
		Role:   "admin",
	}); err != nil {
		t.Fatalf("AddMember (first insert) failed: %v", err)
	}

	// Second insert of the same (org_id, user_id) must hit
	// members_org_user_uniq → AlreadyExists. This also exercises the
	// rewritten isUniqueViolation typed-pgconn classification.
	err := org.AddMember(ctx, &org.AddMemberRequest{
		OrgID:  o.ID,
		UserID: userID,
		Role:   "member",
	})
	if err == nil {
		t.Fatalf("AddMember (duplicate) succeeded, want AlreadyExists")
	}
	if errs.Code(err) != errs.AlreadyExists {
		t.Fatalf("duplicate AddMember code = %v, want AlreadyExists (err=%v)", errs.Code(err), err)
	}
}

// TestAddMemberUnknownUserIsNotFound exercises the FK-violation
// classification: a non-existent user_id must surface NotFound via the
// rewritten isForeignKeyViolation typed-pgconn (SQLSTATE 23503) path.
func TestAddMemberUnknownUserIsNotFound(t *testing.T) {
	ctx := context.Background()
	o := seedOrg(t, ctx, "fkuser")

	ghostUser, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid (ghost user): %v", err)
	}
	err = org.AddMember(ctx, &org.AddMemberRequest{
		OrgID:  o.ID,
		UserID: ghostUser, // no matching auth.users row
		Role:   "member",
	})
	if err == nil {
		t.Fatalf("AddMember with unknown user_id succeeded, want NotFound")
	}
	if errs.Code(err) != errs.NotFound {
		t.Fatalf("unknown-user AddMember code = %v, want NotFound (err=%v)", errs.Code(err), err)
	}
}

// TestAuthorizeHappyPathSameOrgPermit exercises the Authorize RPC body
// on a permit path: an owner reading a workitems resource in its own
// org. Authorize's step-5 effective-role derivation reads a real
// org.members row, so the caller MUST first be enrolled as a member
// (a fabricated Identity with no members row is denied with reason
// "caller is not a member of the target org"). This test seeds a real
// user, enrols them as owner via AddMember, then asserts Authorize
// permits (returns nil) per the role-action matrix.
func TestAuthorizeHappyPathSameOrgPermit(t *testing.T) {
	ctx := context.Background()
	o := seedOrg(t, ctx, "authz")
	userID := seedUser(t, ctx)

	if err := org.AddMember(ctx, &org.AddMemberRequest{
		OrgID:  o.ID,
		UserID: userID,
		Role:   "owner",
	}); err != nil {
		t.Fatalf("AddMember (owner) failed: %v", err)
	}

	err := org.Authorize(ctx, &org.AuthorizeRequest{
		Identity: auth.Identity{
			UserID: userID,
			OrgID:  o.ID,
			Role:   "owner",
		},
		Resource: "workitems.items",
		Action:   "read",
		OrgID:    o.ID,
	})
	if err != nil {
		t.Fatalf("Authorize (same-org owner read) denied, want permit: %v", err)
	}
}
