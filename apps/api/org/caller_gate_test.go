// Cross-tenant caller-gate tests for org.AddMember and org.CreateProject
// (bead unblock-tv8.86 — org-provisioning write-surface tenant gates,
// SPEC §4.2 / §10.1.1).
//
// Both RPCs gained an off-wire CallerUserID channel pinned from the
// resolved caller identity (the future key-management / web-admin BFF's
// session→user→org resolution, §4.3.2), NEVER from the wire. When
// CallerUserID is non-empty:
//
//   - AddMember requires the caller to hold an admin/owner org.members
//     row in OrgID AND caps the granted Role at the caller's effective
//     role (a CRITICAL cross-tenant privilege-escalation gate).
//   - CreateProject requires the caller to be a write-capable member of
//     OrgID (a WARNING-class cross-tenant write IDOR gate, replacing the
//     FK→NotFound that only caught a non-existent org).
//
// A foreign / non-member / non-admin caller → NotFound (existence not
// leaked); an AddMember over-grant → PermissionDenied. Nothing is
// inserted in either rejection path.
//
// Empty CallerUserID is the DORMANT no-op (the trusted §11.1.1 seed +
// org / rbactest / exitcriteriontest / perftest callers pass no caller
// identity) — exercised by every pre-existing org RPC test, which still
// passes. These tests assert the ACTIVE (non-empty-CallerUserID) path.
//
// Shares the seedUser / seedOrg / asUser helpers and the package shape
// of rpc_integration_test.go. MUST run under `encore test
// ./apps/api/org/...` (Docker-backed cluster) — plain `go test` leaves
// org.db == nil and the RPC bodies panic. None call t.Parallel() (BindDB
// is not goroutine-safe; same single-threaded convention as the sibling
// integration tests).

package org_test

import (
	"context"
	"strings"
	"testing"

	encoredb "encore.app/db"
	"encore.app/org"
	"encore.app/shared/ulid"
	"encore.dev/beta/errs"
)

// enrol adds userID to orgID with the given role via the DORMANT-gate
// path (no CallerUserID), the same way the §11.1.1 seed enrols members.
// It deliberately bypasses the active caller-gate via the empty-caller
// no-op to establish the membership fixture — the gate under test would
// otherwise reject the very rows these assertions need in place. Used to
// establish the caller's own membership before the active-gate assertions.
func enrol(t *testing.T, ctx context.Context, orgID, userID, role string) {
	t.Helper()
	if err := org.AddMember(ctx, &org.AddMemberRequest{
		OrgID:  orgID,
		UserID: userID,
		Role:   role,
		// CallerUserID empty → dormant no-op (seed-style enrolment).
	}); err != nil {
		t.Fatalf("enrol %s as %s in %s: %v", userID, role, orgID, err)
	}
}

// memberCount returns the number of org.members rows for (orgID, userID).
// Used to assert that a rejected write inserted nothing.
func memberCount(t *testing.T, ctx context.Context, orgID, userID string) int {
	t.Helper()
	var n int
	if err := encoredb.DB.QueryRow(ctx,
		`SELECT count(*) FROM org.members WHERE org_id = $1 AND user_id = $2`,
		orgID, userID,
	).Scan(&n); err != nil {
		t.Fatalf("memberCount(%s,%s): %v", orgID, userID, err)
	}
	return n
}

// projectCountInOrg returns the number of org.projects rows under orgID.
// Used to assert that a rejected CreateProject inserted nothing.
func projectCountInOrg(t *testing.T, ctx context.Context, orgID string) int {
	t.Helper()
	var n int
	if err := encoredb.DB.QueryRow(ctx,
		`SELECT count(*) FROM org.projects WHERE org_id = $1`,
		orgID,
	).Scan(&n); err != nil {
		t.Fatalf("projectCountInOrg(%s): %v", orgID, err)
	}
	return n
}

func newSlug(t *testing.T, prefix string) string {
	t.Helper()
	suffix, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid (%s slug): %v", prefix, err)
	}
	return strings.ToLower(prefix + "-" + suffix[len(suffix)-12:])
}

// TestAddMemberForeignOrgRejected (AC5a) — a caller who is an admin of
// org A cannot add a member to a FOREIGN org B in which they hold no
// row. The pinned CallerUserID gate rejects with NotFound (existence not
// leaked) and inserts nothing.
func TestAddMemberForeignOrgRejected(t *testing.T) {
	ctx := context.Background()

	orgA := seedOrg(t, ctx, "am-foreign-a")
	orgB := seedOrg(t, ctx, "am-foreign-b")

	caller := seedUser(t, ctx) // admin of A only
	enrol(t, ctx, orgA.ID, caller, "admin")

	victim := seedUser(t, ctx) // the user the attacker tries to plant in B

	err := org.AddMember(ctx, &org.AddMemberRequest{
		OrgID:        orgB.ID, // FOREIGN org — caller holds no row here
		UserID:       victim,
		Role:         "member",
		CallerUserID: caller, // pinned → ACTIVE gate
	})
	if err == nil {
		t.Fatalf("AddMember into foreign org succeeded, want NotFound")
	}
	if errs.Code(err) != errs.NotFound {
		t.Fatalf("foreign-org AddMember code = %v, want NotFound (err=%v)", errs.Code(err), err)
	}
	if n := memberCount(t, ctx, orgB.ID, victim); n != 0 {
		t.Fatalf("foreign-org AddMember inserted %d rows, want 0", n)
	}
}

// TestAddMemberOverGrantRejected (AC5b) — an admin of an org cannot grant
// a role ABOVE their own (here: admin attempting to mint an owner). The
// role cap rejects with PermissionDenied and inserts nothing.
func TestAddMemberOverGrantRejected(t *testing.T) {
	ctx := context.Background()

	o := seedOrg(t, ctx, "am-overgrant")
	caller := seedUser(t, ctx) // admin (strength 3)
	enrol(t, ctx, o.ID, caller, "admin")

	victim := seedUser(t, ctx)

	err := org.AddMember(ctx, &org.AddMemberRequest{
		OrgID:        o.ID,
		UserID:       victim,
		Role:         "owner", // strength 4 > caller's 3 → over-grant
		CallerUserID: caller,
	})
	if err == nil {
		t.Fatalf("AddMember over-grant succeeded, want PermissionDenied")
	}
	if errs.Code(err) != errs.PermissionDenied {
		t.Fatalf("over-grant AddMember code = %v, want PermissionDenied (err=%v)", errs.Code(err), err)
	}
	if n := memberCount(t, ctx, o.ID, victim); n != 0 {
		t.Fatalf("over-grant AddMember inserted %d rows, want 0", n)
	}
}

// TestAddMemberNonAdminCallerRejected — a same-org MEMBER (not admin) is
// not authorised to add members. The gate rejects with NotFound
// (existence not leaked) and inserts nothing.
func TestAddMemberNonAdminCallerRejected(t *testing.T) {
	ctx := context.Background()

	o := seedOrg(t, ctx, "am-nonadmin")
	caller := seedUser(t, ctx) // plain member (no admin authority)
	enrol(t, ctx, o.ID, caller, "member")

	victim := seedUser(t, ctx)

	err := org.AddMember(ctx, &org.AddMemberRequest{
		OrgID:        o.ID,
		UserID:       victim,
		Role:         "member",
		CallerUserID: caller,
	})
	if err == nil {
		t.Fatalf("AddMember by non-admin member succeeded, want NotFound")
	}
	if errs.Code(err) != errs.NotFound {
		t.Fatalf("non-admin AddMember code = %v, want NotFound (err=%v)", errs.Code(err), err)
	}
	if n := memberCount(t, ctx, o.ID, victim); n != 0 {
		t.Fatalf("non-admin AddMember inserted %d rows, want 0", n)
	}
}

// TestAddMemberSameOrgAdminPositive — same-org positive control: an admin
// adding a member at-or-below their own role succeeds via the ACTIVE
// gate, and the row is written.
func TestAddMemberSameOrgAdminPositive(t *testing.T) {
	ctx := context.Background()

	o := seedOrg(t, ctx, "am-positive")
	caller := seedUser(t, ctx) // admin
	enrol(t, ctx, o.ID, caller, "admin")

	newMember := seedUser(t, ctx)

	if err := org.AddMember(ctx, &org.AddMemberRequest{
		OrgID:        o.ID,
		UserID:       newMember,
		Role:         "member", // at-or-below admin → within cap
		CallerUserID: caller,
	}); err != nil {
		t.Fatalf("same-org admin AddMember (within cap) failed: %v", err)
	}
	if n := memberCount(t, ctx, o.ID, newMember); n != 1 {
		t.Fatalf("same-org admin AddMember wrote %d rows, want 1", n)
	}

	// invited_by must record the pinned CallerUserID (the audit trail
	// follows the off-wire caller identity when present).
	var invitedBy *string
	if err := encoredb.DB.QueryRow(ctx,
		`SELECT invited_by FROM org.members WHERE org_id = $1 AND user_id = $2`,
		o.ID, newMember,
	).Scan(&invitedBy); err != nil {
		t.Fatalf("read invited_by: %v", err)
	}
	if invitedBy == nil || *invitedBy != caller {
		t.Fatalf("invited_by = %v, want %q (the pinned CallerUserID)", invitedBy, caller)
	}
}

// TestAddMemberOwnerCanGrantOwnerPositive — role-cap boundary: an owner
// (the top role) MAY grant owner. Confirms the cap is "above", not
// "at-or-above".
func TestAddMemberOwnerCanGrantOwnerPositive(t *testing.T) {
	ctx := context.Background()

	o := seedOrg(t, ctx, "am-owner-grant")
	caller := seedUser(t, ctx) // owner
	enrol(t, ctx, o.ID, caller, "owner")

	coOwner := seedUser(t, ctx)

	if err := org.AddMember(ctx, &org.AddMemberRequest{
		OrgID:        o.ID,
		UserID:       coOwner,
		Role:         "owner", // equal to caller's role → within cap
		CallerUserID: caller,
	}); err != nil {
		t.Fatalf("owner granting owner failed, want permit: %v", err)
	}
	if n := memberCount(t, ctx, o.ID, coOwner); n != 1 {
		t.Fatalf("owner-grant AddMember wrote %d rows, want 1", n)
	}
}

// TestCreateProjectForeignOrgRejected (AC5c) — a caller who is a member
// of org A cannot create a project under a FOREIGN org B. The pinned
// CallerUserID gate rejects with NotFound (replacing the FK→NotFound that
// only caught a non-existent org) and inserts nothing under B.
func TestCreateProjectForeignOrgRejected(t *testing.T) {
	ctx := context.Background()

	orgA := seedOrg(t, ctx, "cp-foreign-a")
	orgB := seedOrg(t, ctx, "cp-foreign-b")

	caller := seedUser(t, ctx) // member of A only
	enrol(t, ctx, orgA.ID, caller, "member")

	before := projectCountInOrg(t, ctx, orgB.ID)

	_, err := org.CreateProject(ctx, &org.CreateProjectRequest{
		OrgID:        orgB.ID, // FOREIGN org — caller holds no row here
		Name:         "intruder project",
		Slug:         newSlug(t, "intruder"),
		CallerUserID: caller, // pinned → ACTIVE gate
	})
	if err == nil {
		t.Fatalf("CreateProject under foreign org succeeded, want NotFound")
	}
	if errs.Code(err) != errs.NotFound {
		t.Fatalf("foreign-org CreateProject code = %v, want NotFound (err=%v)", errs.Code(err), err)
	}
	if after := projectCountInOrg(t, ctx, orgB.ID); after != before {
		t.Fatalf("foreign-org CreateProject changed project count under B: %d → %d, want unchanged", before, after)
	}
}

// TestCreateProjectViewerCallerRejected — a same-org VIEWER (read-only)
// is not write-capable and cannot create a project. The gate rejects
// with NotFound and inserts nothing.
func TestCreateProjectViewerCallerRejected(t *testing.T) {
	ctx := context.Background()

	o := seedOrg(t, ctx, "cp-viewer")
	caller := seedUser(t, ctx) // viewer (read-only)
	enrol(t, ctx, o.ID, caller, "viewer")

	before := projectCountInOrg(t, ctx, o.ID)

	_, err := org.CreateProject(ctx, &org.CreateProjectRequest{
		OrgID:        o.ID,
		Name:         "viewer project",
		Slug:         newSlug(t, "viewer"),
		CallerUserID: caller,
	})
	if err == nil {
		t.Fatalf("CreateProject by viewer succeeded, want NotFound")
	}
	if errs.Code(err) != errs.NotFound {
		t.Fatalf("viewer CreateProject code = %v, want NotFound (err=%v)", errs.Code(err), err)
	}
	if after := projectCountInOrg(t, ctx, o.ID); after != before {
		t.Fatalf("viewer CreateProject changed project count: %d → %d, want unchanged", before, after)
	}
}

// TestCreateProjectSameOrgMemberPositive — same-org positive control: a
// write-capable member creates a project under their own org via the
// ACTIVE gate, and the row is written.
func TestCreateProjectSameOrgMemberPositive(t *testing.T) {
	ctx := context.Background()

	o := seedOrg(t, ctx, "cp-positive")
	caller := seedUser(t, ctx) // member (write-capable)
	enrol(t, ctx, o.ID, caller, "member")

	p, err := org.CreateProject(ctx, &org.CreateProjectRequest{
		OrgID:        o.ID,
		Name:         "member project",
		Slug:         newSlug(t, "member"),
		CallerUserID: caller,
	})
	if err != nil {
		t.Fatalf("same-org member CreateProject failed: %v", err)
	}
	if p.ID == "" || p.OrgID != o.ID {
		t.Fatalf("CreateProject returned %+v, want non-empty id and org_id=%s", p, o.ID)
	}
}
