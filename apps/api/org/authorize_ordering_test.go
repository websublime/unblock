// Tests that exercise org.Authorize through the production code path
// from outside the org package. The external `org_test` package
// shape (rather than `package org`) lets this file blank-import
// encore.app/db so the dedicated migration-owner service's init()
// fires before TestMain returns, populating every domain service's
// nil *sqldb.Database handle (auth, org, workitems, deps) and the
// shared rbac builder. Without that blank import, `encore test
// ./apps/api/org/...` would compile a test binary that loads org
// in isolation and leave org.db == nil — the same panic shape every
// other DB-touching integration suite avoids by importing db.
//
// The internal `package org` test surface (apps/api/org/org_test.go)
// cannot do this because `encore.app/db` imports `encore.app/org` to
// call `org.BindDB(DB)`; adding a back-import from inside `package
// org` itself would create a Go compile-time import cycle. Go's
// external test package (`package <pkg>_test`) is the standard
// escape hatch — the external test package is a separate
// compilation unit so the cycle does not form.
//
// Bead unblock-tv8.41 (parent epic unblock-tv8 / WARNING finding
// from review of unblock-tv8.8): lock the agent-ordering invariant
// in org.Authorize — step 3 (cross-project containment via
// projectBelongsToOrg) MUST run before step 4 (agent allow-list).
// Before this test, the invariant was provable by inspection only
// (apps/api/org/org.go lines 537-566); a future refactor could swap
// steps 3 and 4 and no test would fail.
//
// MUST run under `encore test` (Docker-backed cluster). Step 3
// executes db.QueryRow; under plain `go test` the package-level db
// pointer is nil and the query would panic before any assertion
// fires.

package org_test

import (
	"context"
	"errors"
	"strings"
	"testing"

	"encore.app/auth"
	// Importing encore.app/db triggers its init() which calls
	// org.BindDB(DB) and every other consumer's BindDB hook. Without
	// this import the org test binary loads org in isolation and
	// leaves org.db == nil — CreateOrganization / CreateProject would
	// then panic on a nil *sqldb.Database inside
	// encore.dev/storage/sqldb.(*Database).Exec.
	//
	// The handle is also used for cleanup below (db.DB is exported
	// for exactly this kind of out-of-service test surface — see
	// apps/api/db/db.go's package doc-comment).
	encoredb "encore.app/db"
	"encore.app/org"
	"encore.app/shared/ulid"
	"encore.dev/beta/errs"
)

// TestAuthorizeAgentRejectsCrossOrgProjectID locks the agent-ordering
// invariant: step 3 (cross-project containment) MUST run before step
// 4 (agent allow-list) in org.Authorize. A future refactor that
// swapped these steps would let an agent caller pass a ProjectID
// owned by a different org and have step 4 permit on the agent
// allow-list before step 3's containment check rejected the request.
// The data layer would still deny via rbac.For's org_id scope, but
// Authorize's own predicate must be consistent across human and
// agent callers — agents must not get a different deny shape than
// org members for the same input.
//
// The test name mirrors the bead title (unblock-tv8.41). The
// production symbol is the single org.Authorize function; this test
// exercises the agent sub-branch within that function.
//
// Scenario shape (only this shape proves step 3 fires before step 4):
//
//   - Two real org rows seeded via org.CreateOrganization (org_a,
//     org_b). ULID-suffixed slugs avoid colliding with prior runs on
//     the local Encore cluster (organizations_slug_uniq /
//     projects_org_slug_uniq).
//   - One project seeded under org_b via org.CreateProject — the
//     cross-org ProjectID the test feeds into Authorize.
//   - AuthorizeRequest: Identity.OrgID == org_a (same-org so step 1
//     passes), Role == "agent" (so step 4 WOULD permit if it fired
//     first), Resource == "workitems.items" (a member of org's
//     agentReadWriteResources allow-list, so step 4 WOULD permit if
//     it fired first), Action is read or write (delete would be
//     denied by step 4 unconditionally and would defeat the ordering
//     proof), OrgID == org_a, ProjectID == cross-org project id.
//   - Assertion: err is *errs.Error with Code == PermissionDenied
//     AND Meta["reason"] == "project not in caller's org". Without
//     the reason check the test only proves SOME deny — not
//     specifically the step-3 deny.
//
// The test does NOT call t.Parallel(): BindDB is not goroutine-safe
// (bead unblock-tv8.34) and every other integration test in this
// service follows the same single-threaded convention.
func TestAuthorizeAgentRejectsCrossOrgProjectID(t *testing.T) {
	ctx := context.Background()

	// Seed two real orgs. Unique ULID-suffixed slugs avoid colliding
	// on organizations_slug_uniq across repeated runs of the local
	// Encore cluster (same pattern apps/api/shared/rbactest/seed.go
	// uses for its fixture).
	suffixA, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid (org_a suffix): %v", err)
	}
	suffixB, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid (org_b suffix): %v", err)
	}
	suffixP, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid (project suffix): %v", err)
	}

	orgA, err := org.CreateOrganization(ctx, &org.CreateOrganizationRequest{
		Name: "tv8-41 org a",
		Slug: strings.ToLower("tv8-41-a-" + suffixA[len(suffixA)-12:]),
	})
	if err != nil {
		t.Fatalf("CreateOrganization(org_a) failed: %v", err)
	}
	orgB, err := org.CreateOrganization(ctx, &org.CreateOrganizationRequest{
		Name: "tv8-41 org b",
		Slug: strings.ToLower("tv8-41-b-" + suffixB[len(suffixB)-12:]),
	})
	if err != nil {
		t.Fatalf("CreateOrganization(org_b) failed: %v", err)
	}

	// Seed one project under org_b — this is the cross-org ProjectID
	// the test feeds into Authorize.
	crossOrgProject, err := org.CreateProject(ctx, &org.CreateProjectRequest{
		OrgID: orgB.ID,
		Name:  "tv8-41 cross-org project",
		Slug:  strings.ToLower("xorg-" + suffixP[len(suffixP)-12:]),
	})
	if err != nil {
		t.Fatalf("CreateProject(org_b) failed: %v", err)
	}

	// Best-effort teardown. ON DELETE CASCADE from org.organizations
	// removes the dependent org.projects row. Failures only log: the
	// ULID-suffixed slugs already guarantee non-collision on reruns,
	// so a leaked row blocks nothing.
	t.Cleanup(func() {
		if _, err := encoredb.DB.Exec(ctx,
			`DELETE FROM org.organizations WHERE id IN ($1, $2)`,
			orgA.ID, orgB.ID,
		); err != nil {
			t.Logf("cleanup: delete organizations: %v", err)
		}
	})

	// Both read and write are on the agent's permitted action set —
	// step 4 WOULD permit if it fired first. Delete is deliberately
	// excluded: step 4 denies delete unconditionally, which would
	// mask the ordering proof (the deny would fire even with
	// reordered steps).
	for _, action := range []string{"read", "write"} {
		t.Run(action, func(t *testing.T) {
			req := &org.AuthorizeRequest{
				Identity: auth.Identity{
					UserID:    "u_agent_tv8_41",
					OrgID:     orgA.ID,
					Role:      "agent",
					AgentKind: "claude-code",
				},
				// Resource on the agent allow-list — step 4 would
				// permit if it ran before step 3.
				Resource:  "workitems.items",
				Action:    action,
				OrgID:     orgA.ID,            // same org as identity — step 1 passes
				ProjectID: crossOrgProject.ID, // belongs to org_b — step 3 must deny
			}
			err := org.Authorize(ctx, req)
			if err == nil {
				t.Fatalf("expected PermissionDenied (cross-org project), got nil")
			}
			if errs.Code(err) != errs.PermissionDenied {
				t.Fatalf("err code = %v, want PermissionDenied (err=%v)", errs.Code(err), err)
			}

			// Lock the exact step. Without this assertion the test
			// passes for ANY deny — including a hypothetical step-4
			// deny if step 3 were ever reordered out. The literal
			// reason string is set at apps/api/org/org.go line 551
			// inside Authorize's step-3 branch.
			var ee *errs.Error
			if !errors.As(err, &ee) {
				t.Fatalf("err is not *errs.Error (got %T: %v)", err, err)
			}
			gotReason, _ := ee.Meta["reason"].(string)
			if gotReason != "project not in caller's org" {
				t.Fatalf("Meta[reason] = %q, want %q — step 3 (cross-project containment) did not fire; ordering invariant broken",
					gotReason, "project not in caller's org")
			}
		})
	}
}
