// rbactest_test.go drives the exhaustive RBAC regression sweep.
//
// One `t.Run` per (caller-org × target-org × caller-role × table ×
// action) tuple. The subtest name is
// caller=<X>/target=<Y>/role=<R>/table=<T>/action=<A> so a CI failure
// pinpoints the offending tuple without further investigation.
//
// Two assertion shapes inside the sweep, selected by TableKind (see
// matrix.go):
//
//   - KindOrgScoped: drive rbac.For[T] with the caller's Identity and
//     assert zero target-org rows leak into a caller-org reader. The
//     rbac builder is read-only, so the action axis collapses to
//     {ActionRead}; write/delete on the same tables are covered by
//     KindAuthorizeOnly assertions through Authorize.
//   - KindAuthorizeOnly: drive org.Authorize with every (caller-role
//     × action) tuple and assert the deny/permit decision matches
//     the policy contract (cross-org deny everywhere; same-org per
//     the role matrix and the agent-resource set).
//
// Concurrency: no t.Parallel anywhere — rbac.Bind is not goroutine-
// safe; bead unblock-tv8.34 tracks the hardening.

package rbactest

import (
	"context"
	"fmt"
	"os"
	"testing"
	"time"

	"encore.app/auth"
	"encore.app/db"
	"encore.app/org"
	"encore.app/shared/rbac"
	"encore.dev/beta/errs"
)

// fixture is the global, one-per-process fixture installed by
// TestMain. The matrix sweep reads it without locking — it is
// effectively read-only after TestMain returns.
var fixture *Fixture

// TestMain seeds the fixture once, runs the suite, tears down once.
// Per-subtest cleanup is deliberately avoided — the matrix subtests
// never mutate fixture state (they read through rbac.For or call
// Authorize, neither of which writes), so a single seed/teardown is
// both correct and fastest. If a future contributor adds a mutating
// subtest, it MUST `t.Cleanup` its own rollback.
//
// Encore-runtime requirement (re-stated from doc.go for the reader
// landing here first): this package MUST run under
// `encore test ./apps/api/shared/rbactest/...`. Plain `go test`
// leaves every service's *sqldb.Database pointer nil and SeedFixture
// returns a nil-handle error before any subtest fires.
func TestMain(m *testing.M) {
	ctx := context.Background()

	// db.DB is the canonical *sqldb.Database handle. Importing
	// encore.app/db is what makes the package's init() fire (it
	// binds auth.db, org.db, workitems.db, deps.db, and rbac.db).
	// We rely on the encore-test process having executed those
	// inits before TestMain runs — that is the Encore process
	// bootstrap contract apps/api/db/db.go's doc-comment locks in.
	var err error
	fixture, err = SeedFixture(ctx, db.DB)
	if err != nil {
		// fatalIf with nil *testing.T panics with the message,
		// which is the right behaviour from TestMain (no test has
		// started, so we can't t.Fatal).
		fatalIf(nil, err, "rbactest seed failed")
	}

	code := m.Run()

	// Best-effort teardown. Failures are printed but do not change
	// the exit code — the test verdict is what the runner cares
	// about.
	fixture.Teardown(ctx, db.DB)

	os.Exit(code)
}

// identityFor constructs the auth.Identity the matrix sweep passes
// to rbac.For / org.Authorize for a given (orgLabel, role) tuple.
//
//   - Non-agent roles: UserID comes from the seeded auth.users row;
//     OrgID is the seeded org id; Role is the seeded org.members
//     role. Authorize's effective-role derivation reads the
//     persisted org.members row.
//   - Agent role: UserID is empty (org-level service key shape per
//     SPEC §4.3.2 step 8) or a side-tagged placeholder; OrgID is
//     the seeded org id; Role is "agent"; AgentKind is non-empty
//     so the org.AuthHandler-equivalent shape is well-formed.
//     There is NO org.members row for the agent — Authorize's
//     agent branch takes effect before the members table is
//     consulted.
func identityFor(fx *Fixture, orgLabel, role string) auth.Identity {
	orgID := fx.Orgs[orgLabel]
	if role == RoleAgent {
		return auth.Identity{
			UserID:    "", // org-level agent key
			OrgID:     orgID,
			Role:      RoleAgent,
			AgentKind: "claude-code",
		}
	}
	return auth.Identity{
		UserID:    fx.Users[userKey{OrgLabel: orgLabel, Role: role}],
		OrgID:     orgID,
		Role:      role,
		AgentKind: "",
	}
}

// projectIDForAuthorize returns the project id to pass on
// AuthorizeRequest.ProjectID for a given target-org. Authorize's
// step-3 containment check verifies the project belongs to req.OrgID;
// passing the target-org's seeded project id matches that
// invariant. The two follow-up beads (C-6, E-3) may extend this with
// per-test project ids for project_members assertions.
func projectIDForAuthorize(fx *Fixture, targetOrgLabel string, table string) string {
	// project_members is the only table whose Authorize call REQUIRES
	// a ProjectID (the containment check fires when ProjectID is
	// non-empty, and is what makes the cross-tenant deny test
	// meaningful for that resource). For every other table we leave
	// ProjectID empty — the cross-tenant short-circuit on OrgID is
	// the load-bearing gate and ProjectID would only add noise.
	if table == "org.project_members" {
		return fx.Projects[targetOrgLabel]
	}
	return ""
}

// expectAuthorizeOK predicts the Authorize policy decision for a
// given (caller-role × resource × action) tuple, ASSUMING same-org
// (the cross-org branch denies unconditionally so the prediction is
// trivial for that case). Returns true when Authorize should permit.
//
// Predictions mirror apps/api/org/org.go:
//
//   - agent: permit iff resource ∈ agentPermittedResources AND
//     action != delete.
//   - owner/admin: permit on read/write/delete.
//   - member: permit on read/write; deny on delete.
//   - viewer: permit on read; deny on write/delete.
//   - unknown role: deny.
//
// org.organizations gets a deliberate refinement: same-org reads of
// org.organizations go through the policy matrix (agents are NOT in
// agentPermittedResources for this resource), so the role-action
// matrix governs. Same applies to auth.* and other admin tables —
// the agent branch denies them regardless of action.
func expectAuthorizeOK(role, resource, action string) bool {
	if role == RoleAgent {
		if _, ok := agentPermittedResources[resource]; !ok {
			return false
		}
		return action != ActionDelete
	}
	return rolePermitsAction(role, action)
}

// TestRBACMatrix is the exhaustive sweep. The driver iterates the
// full cross-product and dispatches to the per-kind assertion
// shapes. The subtest tree is deliberately deep so a CI failure
// reads as a navigable path.
//
// Tuple count under B-3 (auth + org schemas):
//   - org axes:      2 * 2 = 4 (caller × target)
//   - role axis:     5
//   - tables:        7 (5 KindAuthorizeOnly + 2 KindOrgScoped)
//   - actions:       3 (read/write/delete) for KindAuthorizeOnly,
//     1 (read only) for KindOrgScoped
//
// = 4 * 5 * (5*3 + 2*1) = 4 * 5 * 17 = 340 subtests.
func TestRBACMatrix(t *testing.T) {
	if fixture == nil {
		t.Fatalf("rbactest: fixture is nil; SeedFixture must run from TestMain before this test fires")
	}

	for _, callerOrg := range AllOrgs {
		for _, targetOrg := range AllOrgs {
			for _, role := range AllRoles {
				for _, table := range AuthOrgTables {
					switch table.Kind {
					case KindOrgScoped:
						// rbac.For path — read action only.
						runOrgScopedTuple(t, fixture, callerOrg, targetOrg, role, table.Name)

					case KindAuthorizeOnly:
						for _, action := range AllActions {
							runAuthorizeOnlyTuple(t, fixture, callerOrg, targetOrg, role, table.Name, action)
						}

					default:
						t.Fatalf("rbactest: unknown TableKind %d for table %q", table.Kind, table.Name)
					}
				}
			}
		}
	}
}

// runOrgScopedTuple is the KindOrgScoped assertion shape. The
// caller's Identity is constructed from (callerOrg, role); the
// suite issues a rbac.For read against the table and asserts no row
// with org_id == targetOrg's id leaks back. The target-org check
// is the load-bearing assertion — same-org reads MAY return rows
// (and should!), cross-org reads MUST return zero.
//
// The action axis collapses to "read" for this kind because the rbac
// builder is read-only. Write/delete on the same physical table
// route through org.Authorize and are covered by the
// KindAuthorizeOnly shape on the same table identifier — see C-6
// (workitems/deps) and E-3 (mcp/memory/boards) extensions.
func runOrgScopedTuple(t *testing.T, fx *Fixture, callerOrg, targetOrg, role, table string) {
	t.Helper()
	subtest := fmt.Sprintf("caller=%s/target=%s/role=%s/table=%s/action=%s",
		callerOrg, targetOrg, role, table, ActionRead)

	t.Run(subtest, func(t *testing.T) {
		id := identityFor(fx, callerOrg, role)
		targetOrgID := fx.Orgs[targetOrg]

		// Run a scoped read. T = scopedRow is a minimal projection
		// over (id, org_id) — the scanner reads exported fields in
		// declaration order, which matches `SELECT * FROM <table>`
		// only if T's field count matches the table's column count.
		// We therefore use rbac.For[scopedRow] only when the table's
		// row shape begins with (id, org_id, ...) — true for
		// org.projects and org.members (verified against
		// migrations 0030_org.up.sql lines 19-43).
		//
		// For C-6/E-3 extensions the table shapes diverge and the
		// suite will need per-table row types (or an explicit
		// SELECT). Within B-3 the minimal projection is sufficient
		// because both KindOrgScoped tables share the (id, org_id,
		// ...) prefix and we discard every column after org_id by
		// declaring only those two fields and pairing them with an
		// explicit SELECT via a Where that is a no-op…
		//
		// Reality: rbac.For uses `SELECT *` (apps/api/shared/rbac
		// rbac.go ~line 413). The scanner expects T's field count
		// to equal the column count. Declaring scopedRow as a
		// full-column shape per table is overkill for the row-leak
		// assertion (we only need org_id). The suite instead uses
		// a separate, raw SQL probe that selects just `org_id` for
		// the leak check and exercises rbac.For with a typed-row T
		// matching the actual table layout. See selectScopedOrgIDs
		// below.
		gotOrgIDs, err := selectScopedOrgIDs(context.Background(), id, table)
		if err != nil {
			// Same-org callers may legitimately see rows; cross-org
			// callers MUST see zero. An error here is a suite bug
			// or a DB problem — fail loudly.
			t.Fatalf("scoped read on %q with identity %+v: %v", table, id, err)
		}

		// The load-bearing assertion: no target-org row leaks into
		// the caller's read. Applies regardless of whether
		// callerOrg == targetOrg (same-org has zero target rows
		// because the target IS the caller in that case; cross-org
		// has zero target rows because the scope predicate filters
		// them out).
		//
		// The check is symmetric: we look for any row whose
		// org_id == targetOrgID AND targetOrg != callerOrg. If
		// callerOrg == targetOrg we're asserting on the same id
		// twice — the assertion still holds (the rows belong to
		// callerOrg == targetOrg, which is fine) and reading the
		// tuple keeps the subtest name uniform.
		if callerOrg != targetOrg {
			for _, gotOrgID := range gotOrgIDs {
				if gotOrgID == targetOrgID {
					t.Fatalf("cross-tenant leak on %q: caller=%s (org_id=%s) saw row with org_id=%s",
						table, callerOrg, id.OrgID, gotOrgID)
				}
			}
		}

		// Sanity check: same-org reads should not be empty for the
		// fixture (each side seeded at least one row per table).
		// This guards against a silent over-scoping bug where the
		// builder accidentally filters EVERYTHING out — the
		// "zero rows" assertion above would pass trivially in that
		// case.
		//
		// The agent role is an exception for org.projects /
		// org.members: agents do not appear in org.members and the
		// schema rows are visible regardless of caller-role (rbac
		// scopes by org_id, not by role). So this sanity check
		// fires for every role uniformly.
		if callerOrg == targetOrg && len(gotOrgIDs) == 0 {
			t.Fatalf("same-org read on %q returned zero rows; seed missing or scope over-filters", table)
		}
	})
}

// runAuthorizeOnlyTuple is the KindAuthorizeOnly assertion shape.
// Drives org.Authorize directly and asserts the deny/permit
// decision matches the policy contract.
//
// Cross-org: always deny (Authorize step 1 short-circuit).
// Same-org:  deny or permit per expectAuthorizeOK.
func runAuthorizeOnlyTuple(t *testing.T, fx *Fixture, callerOrg, targetOrg, role, table, action string) {
	t.Helper()
	subtest := fmt.Sprintf("caller=%s/target=%s/role=%s/table=%s/action=%s",
		callerOrg, targetOrg, role, table, action)

	t.Run(subtest, func(t *testing.T) {
		id := identityFor(fx, callerOrg, role)
		targetOrgID := fx.Orgs[targetOrg]
		projectID := projectIDForAuthorize(fx, targetOrg, table)

		req := &org.AuthorizeRequest{
			Identity:  id,
			Resource:  table,
			Action:    action,
			OrgID:     targetOrgID,
			ProjectID: projectID,
		}
		err := org.Authorize(context.Background(), req)

		// Predict the decision. Cross-org is unconditional deny;
		// same-org consults the policy matrix.
		var expectPermit bool
		if callerOrg == targetOrg {
			expectPermit = expectAuthorizeOK(role, table, action)
		}

		if expectPermit {
			if err != nil {
				t.Fatalf("expected permit, got %v (code=%s)", err, errs.Code(err))
			}
			return
		}

		// Expected deny. We accept the canonical PermissionDenied
		// code; any other failure (Internal, InvalidArgument, …)
		// indicates a different bug and we surface it.
		if err == nil {
			t.Fatalf("expected deny, got nil")
		}
		if errs.Code(err) != errs.PermissionDenied {
			t.Fatalf("expected PermissionDenied, got code=%s err=%v", errs.Code(err), err)
		}
	})
}

// scopedRow is the minimal projection used by selectScopedOrgIDs to
// read just `(id, org_id)` from each KindOrgScoped table. The
// rbac.For builder issues `SELECT *` (apps/api/shared/rbac.go
// ~line 413), so the suite cannot directly use rbac.For with a
// two-field T over a many-column table — the scanner would mismatch
// column count vs field count.
//
// Workaround: use a typed row shape that matches each table's full
// column layout. Both KindOrgScoped tables in B-3 (org.projects,
// org.members) share the (id, org_id, ...) prefix but diverge in
// the suffix; we therefore declare per-table row types below.
//
// This is a row-shape helper, not part of the policy contract.
// When C-6 / E-3 add new KindOrgScoped tables, add a matching row
// type and extend the switch in selectScopedOrgIDs.

// orgProjectsRow mirrors migration 0030_org.up.sql lines 34-43
// (8 columns). Fields are in declaration order to match
// rbac.For[T]'s reflection scanner. pgx scans timestamptz natively
// into time.Time, so the time-typed fields below are scan-correct.
// The suite only reads OrgID; the other fields exist so the column
// count matches the rbac builder's implicit `SELECT *`.
type orgProjectsRow struct {
	ID          string
	OrgID       string
	Slug        string
	Name        string
	Description *string
	ArchivedAt  *time.Time
	CreatedAt   time.Time
	UpdatedAt   time.Time
}

// orgMembersRow mirrors migration 0030_org.up.sql lines 19-30
// (6 columns: id, org_id, user_id, role, invited_by, created_at).
type orgMembersRow struct {
	ID        string
	OrgID     string
	UserID    string
	Role      string
	InvitedBy *string
	CreatedAt time.Time
}

// selectScopedOrgIDs issues a rbac.For[T] read against the named
// table and returns the org_id of every row the caller sees. The
// per-table T shape is selected by the switch below; the only
// dimension the assertion uses is OrgID.
func selectScopedOrgIDs(ctx context.Context, id auth.Identity, table string) ([]string, error) {
	switch table {
	case "org.projects":
		rows, err := rbac.For[orgProjectsRow](id, "org.projects").Run(ctx)
		if err != nil {
			return nil, err
		}
		out := make([]string, 0, len(rows))
		for _, r := range rows {
			out = append(out, r.OrgID)
		}
		return out, nil

	case "org.members":
		rows, err := rbac.For[orgMembersRow](id, "org.members").Run(ctx)
		if err != nil {
			return nil, err
		}
		out := make([]string, 0, len(rows))
		for _, r := range rows {
			out = append(out, r.OrgID)
		}
		return out, nil

	default:
		return nil, fmt.Errorf("selectScopedOrgIDs: unknown KindOrgScoped table %q (add a row type and switch case)", table)
	}
}
