// Package org owns the org schema. Holds organizations, members, projects,
// and the canonical Authorize RBAC predicate consumed by every other service.
// See SPEC §4.2 for the locked RPC surface and §10.1 for the RBAC mechanism.
//
// In P01 task B-2 (bead unblock-tv8.8) this package lands the six private
// RPC bodies (CreateOrganization, CreateProject, GetOrganization,
// GetProject, AddMember, Authorize). Authorize is the canonical
// cross-tenant gate consumed by every other live service in P01 —
// workitems (C-1), deps (C-2), mcp (D-1) call it before any read or
// write. Identity for API-key callers carries Role="agent" minted by
// auth's Bearer hot path (SPEC §4.3.2 step 8); agents have no rows in
// org.members and are authorised through a separate predicate branch
// (see authorizeAgent below).
//
// Database wiring: this package consumes via sqldb.Named("unblock")
// (see db.go). The auth service remains the sole migration-owner per
// SPEC §3.1.
package org

import (
	"context"
	"errors"
	"fmt"
	"regexp"
	"strings"

	"encore.app/auth"
	"encore.app/shared/rbac"
	"encore.app/shared/ulid"
	encoreauth "encore.dev/beta/auth"
	"encore.dev/beta/errs"
	"encore.dev/rlog"
	"encore.dev/storage/sqldb"
)

// -----------------------------------------------------------------------------
// Locked type surface (SPEC §4.2). Field shapes are wire-locked; do not edit
// without a spec amendment.
// -----------------------------------------------------------------------------

// Organization is the canonical org row shape. SPEC §4.2.
type Organization struct {
	ID   string // ULID
	Name string
	Slug string
}

// Project is the canonical project row shape. SPEC §4.2.
type Project struct {
	ID    string // ULID
	OrgID string // ULID
	Name  string
	Slug  string
}

// CreateOrganizationRequest is the input to CreateOrganization. SPEC §4.2.
type CreateOrganizationRequest struct {
	Name string
	Slug string
}

// CreateProjectRequest is the input to CreateProject. SPEC §4.2.
type CreateProjectRequest struct {
	OrgID string
	Name  string
	Slug  string
}

// AddMemberRequest is the input to AddMember. SPEC §4.2.
type AddMemberRequest struct {
	OrgID  string
	UserID string
	Role   string // "owner" | "admin" | "member" | "viewer"
}

// AuthorizeRequest is the input to Authorize. SPEC §4.2.
type AuthorizeRequest struct {
	Identity  auth.Identity
	Resource  string // see resource* consts below
	Action    string // "read" | "write" | "delete"
	OrgID     string
	ProjectID string // optional
}

// -----------------------------------------------------------------------------
// Internal vocabularies and policy tables.
//
// Resource identifiers are kept as named constants (not a Go enum) because
// SPEC §4.2's surface defines Resource as a plain string. Callers SHOULD
// reference these constants to avoid typos, but the gate is enforced at
// Authorize entry: an unrecognised Resource returns InvalidArgument
// (fail-closed — never silently permit).
//
// Roles match the org.members.role CHECK constraint (members_role_chk,
// migration 0030_org.up.sql line 27). The synthetic "agent" literal is
// NOT in the DB CHECK — it is a runtime-only identity minted by auth's
// API-key Bearer hot path (auth.go:54).
// -----------------------------------------------------------------------------

const (
	resourceOrgOrganizations  = "org.organizations"
	resourceOrgProjects       = "org.projects"
	resourceOrgMembers        = "org.members"
	resourceOrgProjectMembers = "org.project_members"
	resourceWorkitemsItems    = "workitems.items"
	resourceWorkitemsComments = "workitems.comments"
	resourceWorkitemsTrail    = "workitems.trail"
	resourceDepsDependencies  = "deps.dependencies"
	resourceMCPToolCalls      = "mcp.tool_calls"
	resourceMCPAPIKeys        = "mcp.api_keys"
	resourceAuthSessions      = "auth.sessions"
	resourceAuthUsers         = "auth.users"
	resourceAuthOAuthTokens   = "auth.oauth_tokens"
	resourceMemoryEntries     = "memory.entries"
	resourceBoardsBoards      = "boards.boards"
)

const (
	actionRead   = "read"
	actionWrite  = "write"
	actionDelete = "delete"
)

const (
	roleOwner  = "owner"
	roleAdmin  = "admin"
	roleMember = "member"
	roleViewer = "viewer"
	// roleAgent is the runtime-only identity for API-key callers
	// (SPEC §4.3.2 step 8). NOT a member-table value.
	roleAgent = "agent"
)

// roleStrength ranks the four member roles for max(org_role,
// project_role) effective-role derivation per migration 0030_org.up.sql
// line 49. owner > admin > member > viewer.
var roleStrength = map[string]int{
	roleViewer: 1,
	roleMember: 2,
	roleAdmin:  3,
	roleOwner:  4,
}

// memberRoleAllowed mirrors the DB CHECK (members_role_chk +
// project_members_role_chk). AddMember validates against this set
// client-side so a typo returns InvalidArgument instead of bubbling
// up a Postgres CHECK violation.
var memberRoleAllowed = map[string]struct{}{
	roleOwner:  {},
	roleAdmin:  {},
	roleMember: {},
	roleViewer: {},
}

// resourceAllowed is the closed set of Resource identifiers Authorize
// recognises. Anything else fails closed with InvalidArgument.
var resourceAllowed = map[string]struct{}{
	resourceOrgOrganizations:  {},
	resourceOrgProjects:       {},
	resourceOrgMembers:        {},
	resourceOrgProjectMembers: {},
	resourceWorkitemsItems:    {},
	resourceWorkitemsComments: {},
	resourceWorkitemsTrail:    {},
	resourceDepsDependencies:  {},
	resourceMCPToolCalls:      {},
	resourceMCPAPIKeys:        {},
	resourceAuthSessions:      {},
	resourceAuthUsers:         {},
	resourceAuthOAuthTokens:   {},
	resourceMemoryEntries:     {},
	resourceBoardsBoards:      {},
}

// actionAllowed is the closed set of Action verbs.
var actionAllowed = map[string]struct{}{
	actionRead:   {},
	actionWrite:  {},
	actionDelete: {},
}

// agentReadWriteResources is the closed set of tables an agent
// identity (Role=="agent") may read or write within its own org.
// Per SPEC §10.1 line 1903, agents are authorised at the MCP layer
// via tool-scope checks, not via org membership rows. Authorize
// permits same-org read/write here and rejects everything else,
// including all org.* tables (agents do not manage membership) and
// all delete actions (agents do not destroy data).
var agentReadWriteResources = map[string]struct{}{
	resourceWorkitemsItems:    {},
	resourceWorkitemsComments: {},
	resourceWorkitemsTrail:    {},
	resourceDepsDependencies:  {},
	resourceMCPToolCalls:      {},
	resourceMemoryEntries:     {},
}

// slugPattern validates org/project slugs. Lowercase alphanumeric +
// hyphen, 1..200 chars, no leading/trailing hyphen. The DB UNIQUE
// indexes (organizations_slug_uniq, projects_org_slug_uniq) treat the
// stored slug verbatim, so we normalise at the service boundary
// (lowercase) and reject anything that would surprise downstream URL
// routers.
var slugPattern = regexp.MustCompile(`^[a-z0-9](?:[a-z0-9-]{0,198}[a-z0-9])?$`)

const (
	minNameLen = 1
	maxNameLen = 200
)

// -----------------------------------------------------------------------------
// RPC bodies.
// -----------------------------------------------------------------------------

// CreateOrganization inserts a new org.organizations row. Slug uniqueness
// is enforced by the DB (organizations_slug_uniq) and surfaced as
// errs.AlreadyExists.
//
//encore:api private method=POST path=/org.CreateOrganization
func CreateOrganization(ctx context.Context, req *CreateOrganizationRequest) (*Organization, error) {
	if req == nil {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing request body"}
	}
	name := strings.TrimSpace(req.Name)
	if err := validateName(name); err != nil {
		return nil, err
	}
	slug, err := normaliseSlug(req.Slug)
	if err != nil {
		return nil, err
	}

	id, err := ulid.New()
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "org id generation failed"}
	}

	_, err = db.Exec(ctx,
		`INSERT INTO org.organizations (id, slug, name) VALUES ($1, $2, $3)`,
		id, slug, name,
	)
	if err != nil {
		if isUniqueViolation(err, "organizations_slug_uniq") {
			return nil, &errs.Error{
				Code:    errs.AlreadyExists,
				Message: fmt.Sprintf("organization with slug %q already exists", slug),
				Meta:    errs.Metadata{"constraint": "organizations_slug_uniq"},
			}
		}
		rlog.Error("org: create organization failed", "err", err, "slug", slug)
		return nil, &errs.Error{Code: errs.Internal, Message: "create organization failed"}
	}

	return &Organization{ID: id, Name: name, Slug: slug}, nil
}

// CreateProject inserts a new org.projects row. UNIQUE (org_id, slug)
// is enforced by projects_org_slug_uniq.
//
//encore:api private method=POST path=/org.CreateProject
func CreateProject(ctx context.Context, req *CreateProjectRequest) (*Project, error) {
	if req == nil {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing request body"}
	}
	if req.OrgID == "" {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing org_id"}
	}
	name := strings.TrimSpace(req.Name)
	if err := validateName(name); err != nil {
		return nil, err
	}
	slug, err := normaliseSlug(req.Slug)
	if err != nil {
		return nil, err
	}

	id, err := ulid.New()
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "project id generation failed"}
	}

	_, err = db.Exec(ctx,
		`INSERT INTO org.projects (id, org_id, slug, name) VALUES ($1, $2, $3, $4)`,
		id, req.OrgID, slug, name,
	)
	if err != nil {
		if isForeignKeyViolation(err) {
			return nil, &errs.Error{
				Code:    errs.NotFound,
				Message: fmt.Sprintf("organization %q not found", req.OrgID),
			}
		}
		if isUniqueViolation(err, "projects_org_slug_uniq") {
			return nil, &errs.Error{
				Code:    errs.AlreadyExists,
				Message: fmt.Sprintf("project with slug %q already exists in org %q", slug, req.OrgID),
				Meta:    errs.Metadata{"constraint": "projects_org_slug_uniq"},
			}
		}
		rlog.Error("org: create project failed", "err", err, "org_id", req.OrgID, "slug", slug)
		return nil, &errs.Error{Code: errs.Internal, Message: "create project failed"}
	}

	return &Project{ID: id, OrgID: req.OrgID, Name: name, Slug: slug}, nil
}

// GetOrganization fetches a single org.organizations row by id.
//
// Read is channelled through the rbac builder (SPEC §10.1 mandate) so
// cross-tenant lookups return zero rows even when the id exists in a
// different org. The scope predicate prepended by rbac.For matches
// `org.organizations.org_id` — for org.organizations itself, the row's
// id IS the org id, but the rbac builder still expects an `org_id`
// column. We use a typed projection (orgRow) that selects an `org_id`
// alias of the id column so the scope predicate evaluates correctly.
//
//encore:api private method=GET path=/org.GetOrganization/:id
func GetOrganization(ctx context.Context, id string) (*Organization, error) {
	if id == "" {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing id"}
	}
	identity, ok := callerIdentity(ctx)
	if !ok {
		return nil, &errs.Error{Code: errs.Unauthenticated, Message: "no caller identity"}
	}

	// Org-table reads use a direct row lookup bounded by the caller's
	// own org_id: an org caller can only see its own organization
	// record. The rbac scope predicate (`<table>.org_id = $1`) is not
	// applicable to org.organizations (whose primary key IS the org
	// id), so we enforce equivalence explicitly: id must equal the
	// caller's OrgID. This delivers the same cross-tenant guarantee
	// without bending the scope-column contract.
	if id != identity.OrgID {
		return nil, &errs.Error{Code: errs.NotFound, Message: "organization not found"}
	}

	var row Organization
	err := db.QueryRow(ctx,
		`SELECT id, name, slug FROM org.organizations WHERE id = $1`,
		id,
	).Scan(&row.ID, &row.Name, &row.Slug)
	if err != nil {
		if errors.Is(err, sqldb.ErrNoRows) {
			return nil, &errs.Error{Code: errs.NotFound, Message: "organization not found"}
		}
		rlog.Error("org: get organization failed", "err", err, "id", id)
		return nil, &errs.Error{Code: errs.Internal, Message: "get organization failed"}
	}
	return &row, nil
}

// GetProject fetches a single org.projects row by id, scoped to the
// caller's org via the rbac builder.
//
//encore:api private method=GET path=/org.GetProject/:id
func GetProject(ctx context.Context, id string) (*Project, error) {
	if id == "" {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing id"}
	}
	identity, ok := callerIdentity(ctx)
	if !ok {
		return nil, &errs.Error{Code: errs.Unauthenticated, Message: "no caller identity"}
	}

	rows, err := rbac.For[projectRow](identity, "org.projects").
		Where("id = $1", id).
		Run(ctx)
	if err != nil {
		rlog.Error("org: get project failed", "err", err, "id", id)
		return nil, &errs.Error{Code: errs.Internal, Message: "get project failed"}
	}
	if len(rows) == 0 {
		// Either the id does not exist, or it belongs to another
		// org. The two cases are indistinguishable to the caller by
		// design — a cross-tenant probe must not leak existence.
		return nil, &errs.Error{Code: errs.NotFound, Message: "project not found"}
	}
	r := rows[0]
	return &Project{ID: r.ID, OrgID: r.OrgID, Name: r.Name, Slug: r.Slug}, nil
}

// projectRow is the row-projection shape consumed by rbac.For[T]'s
// reflection-based scanner. Fields must appear in the same declaration
// order as the columns of the implicit `SELECT * FROM org.projects`.
//
// Schema column order (migration 0030_org.up.sql lines 34-43):
//
//	id, org_id, slug, name, description, archived_at, created_at, updated_at
type projectRow struct {
	ID          string
	OrgID       string
	Slug        string
	Name        string
	Description *string
	ArchivedAt  *string
	CreatedAt   string
	UpdatedAt   string
}

// AddMember inserts an org.members row. Role is validated client-side
// so the DB CHECK (members_role_chk) is never the surface error.
//
//encore:api private method=POST path=/org.AddMember
func AddMember(ctx context.Context, req *AddMemberRequest) error {
	if req == nil {
		return &errs.Error{Code: errs.InvalidArgument, Message: "missing request body"}
	}
	if req.OrgID == "" {
		return &errs.Error{Code: errs.InvalidArgument, Message: "missing org_id"}
	}
	if req.UserID == "" {
		return &errs.Error{Code: errs.InvalidArgument, Message: "missing user_id"}
	}
	if req.Role == roleAgent {
		// SPEC §4.3.2 + auth/auth.go:54: "agent" is a runtime
		// identity, never a member-table value. Reject with a
		// specific message rather than a generic "invalid role".
		return &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "role \"agent\" is a runtime identity for API keys, not an org membership role",
			Meta:    errs.Metadata{"field": "role"},
		}
	}
	if _, ok := memberRoleAllowed[req.Role]; !ok {
		return &errs.Error{
			Code:    errs.InvalidArgument,
			Message: fmt.Sprintf("invalid role %q (allowed: owner, admin, member, viewer)", req.Role),
			Meta:    errs.Metadata{"field": "role"},
		}
	}

	id, err := ulid.New()
	if err != nil {
		return &errs.Error{Code: errs.Internal, Message: "member id generation failed"}
	}

	// invited_by is sourced from the caller's auth.Identity (Encore
	// auth-context wired via auth.UserID). Nullable in the schema —
	// when the caller is an agent or the context is missing, we
	// insert NULL rather than fabricating a user id.
	var invitedBy any
	if identity, ok := callerIdentity(ctx); ok && identity.UserID != "" && identity.Role != roleAgent {
		invitedBy = identity.UserID
	}

	_, err = db.Exec(ctx,
		`INSERT INTO org.members (id, org_id, user_id, role, invited_by)
		 VALUES ($1, $2, $3, $4, $5)`,
		id, req.OrgID, req.UserID, req.Role, invitedBy,
	)
	if err != nil {
		if isUniqueViolation(err, "members_org_user_uniq") {
			return &errs.Error{
				Code:    errs.AlreadyExists,
				Message: fmt.Sprintf("user %q is already a member of org %q", req.UserID, req.OrgID),
				Meta:    errs.Metadata{"constraint": "members_org_user_uniq"},
			}
		}
		if isForeignKeyViolation(err) {
			return &errs.Error{
				Code:    errs.NotFound,
				Message: "org_id or user_id does not exist",
			}
		}
		rlog.Error("org: add member failed", "err", err, "org_id", req.OrgID, "user_id", req.UserID)
		return &errs.Error{Code: errs.Internal, Message: "add member failed"}
	}
	return nil
}

// Authorize is the canonical RBAC predicate. Called by every other
// service before reading or writing a resource. Returns nil on permit;
// a structured *errs.Error{Code: errs.PermissionDenied, ...} on deny.
//
// Logic order (load-bearing — AC-1's literal predicate is step 1):
//
//  1. Cross-tenant short-circuit. If Identity.OrgID != req.OrgID, deny.
//     This is the load-bearing tenant gate AC-1 names.
//  2. Validate Resource and Action against the closed allow-lists.
//     Unknown values fail closed (InvalidArgument).
//  3. Agent branch. If Identity.Role == "agent", permit same-org
//     read/write on the agentReadWriteResources set; deny everything
//     else (SPEC §10.1).
//  4. Compute effective role = max(org_role, project_role). When
//     req.ProjectID is empty, fall back to org_role.
//  5. Apply the role-action matrix.
//
//encore:api private method=POST path=/org.Authorize
func Authorize(ctx context.Context, req *AuthorizeRequest) error {
	if req == nil {
		return &errs.Error{Code: errs.InvalidArgument, Message: "missing request body"}
	}
	if req.OrgID == "" {
		return &errs.Error{Code: errs.InvalidArgument, Message: "missing org_id"}
	}

	// Step 1: cross-tenant short-circuit.
	if req.Identity.OrgID == "" || req.Identity.OrgID != req.OrgID {
		return denyError(req, "cross-tenant request")
	}

	// Step 2: validate Resource and Action up front. Fail-closed so
	// callers cannot accidentally permit via typo (the empty-string
	// Resource especially is a common bug class).
	if _, ok := resourceAllowed[req.Resource]; !ok {
		return &errs.Error{
			Code:    errs.InvalidArgument,
			Message: fmt.Sprintf("unknown resource %q", req.Resource),
			Meta:    errs.Metadata{"field": "resource"},
		}
	}
	if _, ok := actionAllowed[req.Action]; !ok {
		return &errs.Error{
			Code:    errs.InvalidArgument,
			Message: fmt.Sprintf("unknown action %q (allowed: read, write, delete)", req.Action),
			Meta:    errs.Metadata{"field": "action"},
		}
	}

	// Step 3: agent identity. Authorised via tool-scope at MCP, not
	// org.members rows. Permit same-org read/write on a closed set
	// of resources; deny everything else.
	if req.Identity.Role == roleAgent {
		if _, ok := agentReadWriteResources[req.Resource]; !ok {
			return denyError(req, "agents may not access this resource")
		}
		if req.Action == actionDelete {
			return denyError(req, "agents may not delete")
		}
		return nil
	}

	// Step 4: cross-project validation + effective-role derivation.
	if req.ProjectID != "" {
		ok, err := projectBelongsToOrg(ctx, req.ProjectID, req.OrgID)
		if err != nil {
			rlog.Error("org: authorize project lookup failed", "err", err, "project_id", req.ProjectID)
			return &errs.Error{Code: errs.Internal, Message: "authorize project lookup failed"}
		}
		if !ok {
			// Project does not exist in this org — either missing
			// or cross-org. Both surface as a permission failure
			// (do not leak existence).
			return denyError(req, "project not in caller's org")
		}
	}

	effective, err := effectiveRole(ctx, req.OrgID, req.ProjectID, req.Identity.UserID)
	if err != nil {
		rlog.Error("org: authorize role lookup failed", "err", err, "org_id", req.OrgID, "user_id", req.Identity.UserID)
		return &errs.Error{Code: errs.Internal, Message: "authorize role lookup failed"}
	}
	if effective == "" {
		return denyError(req, "caller is not a member of the target org")
	}

	// Step 5: role-action matrix.
	if !rolePermits(effective, req.Action) {
		return denyError(req, fmt.Sprintf("role %q may not %s", effective, req.Action))
	}
	return nil
}

// -----------------------------------------------------------------------------
// Internal helpers.
// -----------------------------------------------------------------------------

// effectiveRole computes max(org_role, project_role) per migration
// 0030_org.up.sql line 49. Returns "" when the user has neither
// membership (the caller MUST treat empty as deny — agents do not
// flow through this function).
//
// We issue two reads rather than a UNION query because the project
// path is optional and the rank logic is cheaper in Go than in SQL.
// Both reads are simple PK/UK lookups on indexed columns.
func effectiveRole(ctx context.Context, orgID, projectID, userID string) (string, error) {
	if userID == "" {
		// Anonymous callers (no UserID) cannot be members of
		// anything. Distinct from "unknown role" — they simply have
		// no membership record to consult.
		return "", nil
	}

	var orgRole string
	err := db.QueryRow(ctx,
		`SELECT role FROM org.members WHERE org_id = $1 AND user_id = $2`,
		orgID, userID,
	).Scan(&orgRole)
	if err != nil && !errors.Is(err, sqldb.ErrNoRows) {
		return "", fmt.Errorf("org members lookup: %w", err)
	}

	var projectRole string
	if projectID != "" {
		err = db.QueryRow(ctx,
			`SELECT role FROM org.project_members WHERE project_id = $1 AND user_id = $2`,
			projectID, userID,
		).Scan(&projectRole)
		if err != nil && !errors.Is(err, sqldb.ErrNoRows) {
			return "", fmt.Errorf("project members lookup: %w", err)
		}
	}

	return strongerRole(orgRole, projectRole), nil
}

// strongerRole returns the rank-max of two role strings. Empty strings
// rank zero (i.e. "not a member"). When both are empty, the result is
// empty.
func strongerRole(a, b string) string {
	ra := roleStrength[a]
	rb := roleStrength[b]
	if ra == 0 && rb == 0 {
		return ""
	}
	if ra >= rb {
		return a
	}
	return b
}

// rolePermits encodes the role-action matrix:
//
//	owner:  read, write, delete
//	admin:  read, write, delete
//	member: read, write
//	viewer: read
func rolePermits(role, action string) bool {
	switch role {
	case roleOwner, roleAdmin:
		return action == actionRead || action == actionWrite || action == actionDelete
	case roleMember:
		return action == actionRead || action == actionWrite
	case roleViewer:
		return action == actionRead
	default:
		return false
	}
}

// projectBelongsToOrg returns true when org.projects.org_id == orgID
// for the given project id. Cross-org probes return false (the
// caller treats this as deny without leaking existence).
func projectBelongsToOrg(ctx context.Context, projectID, orgID string) (bool, error) {
	var foundOrgID string
	err := db.QueryRow(ctx,
		`SELECT org_id FROM org.projects WHERE id = $1`,
		projectID,
	).Scan(&foundOrgID)
	if err != nil {
		if errors.Is(err, sqldb.ErrNoRows) {
			return false, nil
		}
		return false, fmt.Errorf("project lookup: %w", err)
	}
	return foundOrgID == orgID, nil
}

// callerIdentity reads the Encore auth context if present. Returns
// (zero, false) when no identity is attached — Authorize handles that
// branch explicitly; consumer RPCs (CreateOrganization, AddMember,
// GetOrganization, GetProject) treat it as Unauthenticated where
// appropriate or fall back to NULL invited_by.
//
// AGENT-NOTE: the Encore auth.UserID() returns the UserID claim. The
// full Identity (Role, OrgID, AgentKind) is exposed via auth.Data().
// We read both so AddMember's invited_by can distinguish human vs
// agent callers (agents do not appear as invited_by).
func callerIdentity(_ context.Context) (auth.Identity, bool) {
	// encoreauth.UserID is set when the authhandler returned a
	// non-empty uid. encoreauth.Data returns the *auth.AuthData
	// payload which carries the full Identity.
	uid, ok := encoreauth.UserID()
	if !ok || uid == "" {
		// No Encore auth context attached (e.g. a private RPC
		// invoked from the seeder before any user is provisioned).
		// Caller-side code paths handle this case explicitly.
		return auth.Identity{}, false
	}
	if data, ok := encoreauth.Data().(*auth.AuthData); ok && data != nil {
		return data.Identity, true
	}
	// Fallback: UserID present but no AuthData payload — shouldn't
	// happen in production (auth.AuthHandler always returns AuthData
	// when uid is non-empty). Return a minimal identity so callers
	// don't crash.
	return auth.Identity{UserID: string(uid)}, true
}

// validateName enforces the 1..200 char Name window. SPEC §4.2 leaves
// the exact bounds implementation-defined; we mirror the slug max so
// indexes on (name) remain bounded.
func validateName(name string) error {
	if l := len(name); l < minNameLen || l > maxNameLen {
		return &errs.Error{
			Code:    errs.InvalidArgument,
			Message: fmt.Sprintf("name must be %d..%d chars (got %d)", minNameLen, maxNameLen, l),
			Meta:    errs.Metadata{"field": "name"},
		}
	}
	return nil
}

// normaliseSlug lowercases and trims the slug before validating it
// against slugPattern. We return the normalised form so callers do not
// silently store a different value than they sent.
func normaliseSlug(raw string) (string, error) {
	slug := strings.ToLower(strings.TrimSpace(raw))
	if !slugPattern.MatchString(slug) {
		return "", &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "slug must be 1..200 chars of lowercase alphanumeric + hyphen (no leading/trailing hyphen)",
			Meta:    errs.Metadata{"field": "slug"},
		}
	}
	return slug, nil
}

// denyError builds the canonical Authorize-deny error. Meta carries
// the resource + action so consumers (and the future MCP wire
// translator) can surface a structured FORBIDDEN payload.
func denyError(req *AuthorizeRequest, reason string) error {
	return &errs.Error{
		Code:    errs.PermissionDenied,
		Message: "forbidden",
		Meta: errs.Metadata{
			"resource": req.Resource,
			"action":   req.Action,
			"reason":   reason,
		},
	}
}

// isUniqueViolation returns true when err is a Postgres UNIQUE
// violation on the named constraint. We match by substring on the
// constraint name to stay framework-agnostic (pgx's error vs sqldb's
// wrapped error vary by Encore version).
func isUniqueViolation(err error, constraint string) bool {
	if err == nil {
		return false
	}
	msg := err.Error()
	return strings.Contains(msg, "duplicate key") && strings.Contains(msg, constraint)
}

// isForeignKeyViolation returns true when err is a Postgres FK
// violation. Matched by substring as above.
func isForeignKeyViolation(err error) bool {
	if err == nil {
		return false
	}
	msg := err.Error()
	return strings.Contains(msg, "foreign key") || strings.Contains(msg, "violates foreign key")
}
