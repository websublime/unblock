// matrix.go declares the test-matrix vocabulary: the role axis, the
// action axis, and the per-table classification (org_id-scoped vs
// Authorize-only) that selects the assertion shape. The exhaustive
// matrix sweep in rbactest_test.go iterates these.
//
// The constants here are intentionally re-declared instead of re-using
// the org package's unexported `resource*` / `role*` / `action*`
// values. SPEC §10.1's matrix is a contract — the suite asserts the
// contract, so the contract values live here in plain sight. If a
// drift ever emerges between the org service's closed-set constants
// and this file, the divergence is a finding the suite is meant to
// surface, not a sync target.

package rbactest

// Role axis (SPEC §10.1, §4.2 role-action matrix; org.Authorize
// branches on Identity.Role).
//
// owner/admin/member/viewer are seeded as org.members rows so the
// Authorize effective-role derivation (max(org_role, project_role))
// reads the persisted policy state.
//
// agent is the synthetic API-key runtime identity per SPEC §4.3.2
// step 8: never an org.members row, constructed in-memory only.
// Authorize's agent branch (apps/api/org/org.go step 4) is the
// production code path that must answer same-org permit / cross-org
// deny / delete-everywhere deny / non-agent-resource deny.
const (
	RoleOwner  = "owner"
	RoleAdmin  = "admin"
	RoleMember = "member"
	RoleViewer = "viewer"
	RoleAgent  = "agent"
)

// AllRoles is the canonical iteration order for the role axis. Order
// is policy-strength descending plus agent last, which keeps the
// subtest tree readable in --verbose output.
var AllRoles = []string{
	RoleOwner,
	RoleAdmin,
	RoleMember,
	RoleViewer,
	RoleAgent,
}

// Action axis (SPEC §4.2 RBAC matrix).
const (
	ActionRead   = "read"
	ActionWrite  = "write"
	ActionDelete = "delete"
)

// AllActions is the canonical iteration order for the action axis.
var AllActions = []string{
	ActionRead,
	ActionWrite,
	ActionDelete,
}

// Org/User axis labels. The suite seeds exactly two orgs. The names
// are short to keep the generated subtest names parse-friendly.
const (
	OrgA = "A"
	OrgB = "B"
)

// AllOrgs is the canonical iteration order for the caller-org / target-org
// axes. The full sweep iterates the cross-product so every
// (caller, target) pairing — including same-org permits — is asserted.
var AllOrgs = []string{OrgA, OrgB}

// TableKind classifies a table by how cross-tenant isolation is
// enforced. Each kind selects a different assertion shape inside the
// matrix sweep — see doc.go for the rationale.
type TableKind int

const (
	// KindOrgScoped tables carry an `org_id` column AND are reached
	// through rbac.For[T] in production code. The assertion shape is
	// row-level: the suite seeds a row in both orgs and asserts a
	// caller-org reader sees zero rows whose org_id != caller-org.
	// rbac.For is read-only, so the action axis collapses to
	// {ActionRead} for this kind; the write/delete halves of the policy
	// matrix for the same table go through org.Authorize and are
	// covered by KindAuthorizeOnly assertions on that table identifier.
	KindOrgScoped TableKind = iota + 1

	// KindAuthorizeOnly tables do NOT carry an `org_id` column. Their
	// cross-tenant gate is the org.Authorize predicate, not a SQL scope
	// filter. The assertion shape is permit-level: the suite calls
	// Authorize for every (caller-role × action) and asserts:
	//   - cross-org deny on every tuple
	//   - same-org permit/deny per the policy matrix
	// The action axis covers all of {read, write, delete} for this
	// kind because Authorize answers all three.
	KindAuthorizeOnly
)

// TableSpec is a single row of the table-classification matrix. The
// rbac builder always reads tables with their fully-qualified
// `<schema>.<table>` identifier; the Resource string used by
// org.Authorize matches that identifier verbatim by convention
// (apps/api/org/org.go constants). This struct keeps the two
// spellings together so the matrix sweep never has to guess which
// surface a given table belongs to.
type TableSpec struct {
	// Name is the fully-qualified `<schema>.<table>` identifier. Used
	// both as the rbac.For[T] table argument (KindOrgScoped) and the
	// Authorize Resource value (KindAuthorizeOnly).
	Name string

	// Kind selects the assertion shape — see TableKind.
	Kind TableKind
}

// AuthOrgTables is the closed set of P01-exposed schema tables the
// release-gate matrix asserts isolation against. B-3 (unblock-tv8.9)
// laid down the auth + org coverage; C-6 (unblock-tv8.15) extended to
// workitems + deps.dependencies; E-3 (unblock-tv8.25) closes the
// remaining deps.cascade_events + mcp.* + memory.* + boards.* surfaces.
// This is the final P01 release-gate matrix — every P01-exposed table
// with cross-tenant semantics is represented below.
//
// KindAuthorizeOnly additions are pure append-only on this slice;
// KindOrgScoped additions also require a paired typed row struct
// plus a new case in rbactest_test.go's selectScopedOrgIDs switch
// mirroring the table's column shape.
//
// Classification rationale (verified against migrations
// apps/api/db/migrations/0020_auth.up.sql lines 11-66,
// 0030_org.up.sql lines 7-62, 0040_workitems.up.sql lines 46-225,
// 0050_deps.up.sql lines 13-130, 0070_mcp.up.sql lines 9-74,
// 0080_boards.up.sql lines 8-27, 0090_memory.up.sql lines 13-45):
//
//   - auth.users / auth.oauth_tokens / auth.sessions — no org_id column;
//     rbac.For would emit `<table>.org_id = $1` which Postgres rejects
//     with "column does not exist". Authorize gates these via step 1's
//     cross-tenant short-circuit on Identity.OrgID vs req.OrgID.
//   - org.organizations — no org_id column; the row's id IS the org_id
//     (apps/api/org/org.go GetOrganization at line ~322 enforces
//     id==identity.OrgID directly).
//   - org.project_members — no org_id column (scoped via the project's
//     org_id). Same Authorize-gate treatment.
//   - org.projects — has org_id column; rbac.For-scoped.
//   - org.members — has org_id column; rbac.For-scoped.
//   - workitems.items — has org_id column AND is reached through both
//     surfaces: rbac.For for row-leak reads (workitems.go:870, 926,
//     996, 1569, 1753) AND org.Authorize for write/delete gating
//     (deps.AddEdge, workitems.SetStateColumns). The matrix carries
//     TWO TableSpec entries on the same Name: one KindOrgScoped (read
//     row-leak only) and one KindAuthorizeOnly (read/write/delete
//     policy decision). SPEC §10.1 line 2356-2361 names exactly this
//     dual treatment.
//   - workitems.comments — no org_id column (scoped via parent
//     items.org_id; workitems.go:1010-1020). Authorize-only gate.
//   - workitems.trail — virtual resource (no SQL table; resource
//     identifier at apps/api/org/org.go:111 gating workitems.GetTrail).
//     Authorize-only.
//   - deps.dependencies — no org_id column (gated at the MCP layer via
//     org.Authorize; deps.go:18-25 header doc). Authorize-only.
//   - deps.cascade_events — has org_id NOT NULL (0050_deps.up.sql:98);
//     read by deps.RecentCascadeEvents (AF2 / Tool 1 `prime`) via
//     direct db.Query with explicit org-scope WHERE clause. Round-10
//     adds resourceDepsCascadeEvents to org.resourceAllowed AND
//     agentReadWriteResources (apps/api/org/org.go); the matrix
//     carries BOTH TableSpec shapes — KindOrgScoped (row-leak via
//     rbac.For[T]) AND KindAuthorizeOnly (policy-gate assertion). The
//     dual coverage mirrors workitems.items.
//   - mcp.tool_calls — has org_id NOT NULL (0070_mcp.up.sql:55).
//     Written by mcp/recordtoolcall.go via direct db.Exec; never read
//     through rbac.For in production. Carried as BOTH shapes so the
//     row-leak axis covers any future read-side rbac.For wiring (e.g.
//     audit-tab UI) and the Authorize axis asserts the policy
//     contract (agents permitted, no delete).
//   - mcp.api_keys — has org_id NOT NULL (0070_mcp.up.sql:11). Read
//     by auth.Validate via direct db.Query (key_prefix lookup);
//     written by auth.IssueAPIKey / IssueOrgKey via direct db.Exec.
//     Agents are NOT in the agent permit set — key issuance is an
//     org-admin operation. Carried as BOTH shapes for defensive
//     coverage.
//   - memory.entries — has org_id NULLABLE (0090_memory.up.sql:16) as
//     scope discriminator. SCHEMA-ONLY in P01. The seed uses
//     scope='org' rows so rbac.For's `org_id = $1` predicate hits
//     non-NULL rows; project/user-scoped entries are invisible to
//     rbac.For by design (when memory service ships in P02 it must
//     route project/user reads through a different predicate).
//     Carried as BOTH shapes.
//   - boards.boards — has org_id NOT NULL (0080_boards.up.sql:10).
//     SCHEMA-ONLY in P01. Agents are NOT in the agent permit set —
//     board management is a user-driven UI operation. Carried as
//     BOTH shapes.
var AuthOrgTables = []TableSpec{
	{Name: "auth.users", Kind: KindAuthorizeOnly},
	{Name: "auth.oauth_tokens", Kind: KindAuthorizeOnly},
	{Name: "auth.sessions", Kind: KindAuthorizeOnly},
	{Name: "org.organizations", Kind: KindAuthorizeOnly},
	{Name: "org.project_members", Kind: KindAuthorizeOnly},
	{Name: "org.projects", Kind: KindOrgScoped},
	{Name: "org.members", Kind: KindOrgScoped},
	// C-6 (unblock-tv8.15): workitems + deps surfaces.
	{Name: "workitems.items", Kind: KindOrgScoped},
	{Name: "workitems.items", Kind: KindAuthorizeOnly},
	{Name: "workitems.comments", Kind: KindAuthorizeOnly},
	{Name: "workitems.trail", Kind: KindAuthorizeOnly},
	{Name: "deps.dependencies", Kind: KindAuthorizeOnly},
	// E-3 (unblock-tv8.25): deps.cascade_events + mcp.* + memory.* +
	// boards.* dual-shape coverage. Every table here is in
	// org.resourceAllowed (post-round-10 for deps.cascade_events) so
	// the KindAuthorizeOnly axis asserts PermissionDenied, not the
	// InvalidArgument fail-closed default.
	{Name: "deps.cascade_events", Kind: KindOrgScoped},
	{Name: "deps.cascade_events", Kind: KindAuthorizeOnly},
	{Name: "mcp.tool_calls", Kind: KindOrgScoped},
	{Name: "mcp.tool_calls", Kind: KindAuthorizeOnly},
	{Name: "mcp.api_keys", Kind: KindOrgScoped},
	{Name: "mcp.api_keys", Kind: KindAuthorizeOnly},
	{Name: "memory.entries", Kind: KindOrgScoped},
	{Name: "memory.entries", Kind: KindAuthorizeOnly},
	{Name: "boards.boards", Kind: KindOrgScoped},
	{Name: "boards.boards", Kind: KindAuthorizeOnly},
}

// agentPermittedResources mirrors the org.agentReadWriteResources map
// (apps/api/org/org.go ~line 193). The suite uses this to predict
// Authorize's policy decision for agent callers — same-org permit on
// this set (read/write only, never delete); deny on every other
// resource. Re-declared here, not imported, so the suite asserts the
// policy contract rather than the policy implementation: a drift
// between this set and the org-package set is exactly the kind of
// silent regression the suite must surface.
var agentPermittedResources = map[string]struct{}{
	"workitems.items":     {},
	"workitems.comments":  {},
	"workitems.trail":     {},
	"deps.dependencies":   {},
	"deps.cascade_events": {}, // E-3 round-10: AF2 / Tool 1 prime read path.
	"mcp.tool_calls":      {},
	"memory.entries":      {},
}

// rolePermitsAction encodes the SPEC §4.2 role-action matrix:
//
//	owner:  read, write, delete
//	admin:  read, write, delete
//	member: read, write
//	viewer: read
//
// agent is excluded from this matrix — agent callers go through the
// agent branch (agentPermittedResources above) before this function is
// consulted. The suite uses this to predict Authorize's same-org
// policy decision for non-agent callers. Re-declared, not imported,
// for the same drift-detection reason as agentPermittedResources.
func rolePermitsAction(role, action string) bool {
	switch role {
	case RoleOwner, RoleAdmin:
		return action == ActionRead || action == ActionWrite || action == ActionDelete
	case RoleMember:
		return action == ActionRead || action == ActionWrite
	case RoleViewer:
		return action == ActionRead
	default:
		return false
	}
}
