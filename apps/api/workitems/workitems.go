// Package workitems owns the workitems schema (items, comments, labels,
// milestones) and exposes the private RPCs called by MCP tool handlers.
// See SPEC §4.4 for the full RPC surface.
//
// In P01 task C-1 (bead unblock-tv8.10) this package lands the bodies of
// every //encore:api skeleton declared in A-1: Create, Update, Get,
// GetTrail, AppendComment, SetStateColumns, Close, Claim, List, Search,
// CreateMilestone, UpdateMilestone, AssignItem, MilestoneTree.
//
// SetStateColumns enforces the five PRD §6.2 state-machine invariants
// I-1..I-5 inside one SQL round-trip per SPEC §6.2 Tool 13. Claim
// enforces invariant I-3 (qa_state=failed → review_state and qa_state
// reset to 'pending' atomically) inside the SELECT FOR UPDATE
// transaction defined in SPEC §6.4. Close publishes
// deps.CascadeRequestedTopic with Reason="close" + TraceID from
// tracectx.From(ctx) per SPEC §6.3.1 / §10.2 Option B.
//
// Database wiring: this package uses the canonical BindDB late-bind
// hook (see db.go) — a nil *sqldb.Database pointer populated by the
// dedicated apps/api/db/ service's init via workitems.BindDB(DB). RPC
// bodies read `db` directly after process bootstrap.
//
// Direct UPDATE on workitems.items.is_ready or pipeline_stage is
// FORBIDDEN here (those columns are maintained by the cascade
// subscriber per SPEC §5.7.1 — encore.app/deps owns the write path,
// enforced by apps/api/shared/lint/no_direct_is_ready_write.go).
//
// # Authorisation model — symmetric row-level tenant gate (read + write)
//
// The workitems service self-gates BOTH the read path and the write
// path in SQL. The tenant predicate is enforced by the SQL itself, not
// by a callable check, so a misbehaving or compromised caller can never
// act across orgs through these RPCs.
//
//   - Read-side RPCs (Get, GetTrail, List, Search) self-gate via
//     rbac.For[T](identity, table). The rbac builder injects the
//     tenant predicate (org_id = $caller_org) directly into every
//     emitted SQL clause. ListLabels / MilestoneTree use an EXPLICIT
//     raw-SQL tenant predicate instead of rbac.For (their UNION-ALL /
//     rooted-CTE shapes are not expressible via the builder) — the same
//     justified deviation documented on those RPCs.
//
//   - Write-side RPCs self-gate via a ROW-LEVEL tenant predicate keyed
//     on an internal CallerOrgID channel (round-16 / bead
//     unblock-tv8.77, SPEC §10.1.1). CallerOrgID is populated by the MCP
//     tool handler from the Bearer-resolved identity.OrgID and is NEVER
//     accepted from the wire — it travels as a private RPC struct field.
//     Each item write RPC (Update, AppendComment, SetStateColumns,
//     Close, Claim, Promote, AssignItem) injects
//     (CallerOrgID = ” OR org_id = CallerOrgID) into its targeting SQL
//     (the SELECT … FOR UPDATE row lock, the mutating UPDATE/DELETE, or
//     — for AppendComment's INSERT — the INSERT … SELECT on the parent
//     item's org). Each milestone write RPC (CreateMilestone's
//     parent-read seam, UpdateMilestone) and MilestoneTree use the
//     org-XOR-project form
//     (CallerOrgID = ” OR org_id = CallerOrgID OR project_id IN
//     (SELECT id FROM org.projects WHERE org_id = CallerOrgID)),
//     because project-scoped milestones carry NULL org_id. A foreign
//     target id therefore matches zero rows → NOT_FOUND (or zero rows
//     inserted on an INSERT), never a cross-tenant mutation.
//
//   - The CREATE path (Create, Tool 4) self-gates its wire-supplied
//     cross-references symmetrically (round-16 / bead unblock-tv8.78,
//     SPEC §10.1.1 / §4.4 Create). The item INSERT is a guarded
//     INSERT … SELECT whose WHERE validates project_id (IN caller-org
//     projects), parent_id / discovered_from_id (IN caller-org items),
//     and milestone_id (org-XOR-project) against the caller org; a
//     foreign reference yields ZERO inserted rows → NOT_FOUND, the same
//     envelope a missing id yields. Labels are gated identically by
//     attachLabelsTx (org-XOR-project label-ownership form). The
//     dependencies[] endpoints stay gated by deps.AddEdgeInTx's own
//     CallerOrgID check.
//
//     DECISION (Miguel 2026-06-12, SPEC §10.1.1): the create-path gate
//     keys on the EXISTING req.OrgID — the same value the INSERT stamps
//     org_id from, already pinned from identity.OrgID by the MCP handler
//     and validated non-empty — NOT a separate CallerOrgID channel, and
//     with NO empty-OrgID no-op branch. Deliberate divergence from the
//     .77 update/delete-by-id convention below: Create's internal callers
//     (the §11.1.1 exit-criterion seed + integration tests) all pass a
//     real, same-org OrgID referencing same-org rows, so the non-empty
//     req.OrgID gate passes them with no no-op branch. Coverage is
//     identical to the CallerOrgID-channel RPCs; only the key differs.
//     (attachLabelsTx itself takes the empty-callerOrg no-op form because
//     it is SHARED with the Update label-replace path, which keys on the
//     .77 CallerOrgID channel that trusted internal callers leave empty;
//     Create always passes a non-empty req.OrgID, so the gate is active
//     on the create path regardless.)
//
// # Empty-CallerOrgID no-op (item/milestone) vs hard guard (labels)
//
// The item / milestone write RPCs take the empty-CallerOrgID NO-OP form:
// (CallerOrgID = ” OR <predicate>) — empty CallerOrgID is a no-op gate.
// This is deliberate (SPEC §10.1.1, ratified by Miguel 2026-06-11):
// trusted internal no-auth callers — the §11.1.1 exit-criterion seed and
// the integration tests that drive these RPCs directly through Encore's
// private mesh with no org context — pass an empty CallerOrgID, and the
// no-op branch lets them operate unscoped. The branch is reachable ONLY
// from those trusted callers: every MCP handler ALWAYS pins CallerOrgID
// from identity.OrgID before dispatch, so the no-op is unreachable from
// the agent surface. A hard CallerOrgID=="" reject on these RPCs would
// break the entire exit-criterion + integration suite.
//
// By contrast the label write RPCs (CreateLabel, UpdateLabel,
// DeleteLabel) HARD-REJECT an empty CallerOrgID with InvalidArgument —
// they have no trusted-internal-caller path (they are MCP-only), so an
// empty CallerOrgID there is always a programming error.
//
// The org service's own tenant-scoped private writes (org.CreateProject,
// org.AddMember) self-gate via a CallerUserID org.members membership
// predicate of their own (bead unblock-tv8.86, SPEC §4.2 / §10.1.1) —
// NOT via this CallerOrgID channel and NOT via org.Authorize (which is
// the cross-SERVICE primitive other services call, never the gate for
// org's own provisioning writes). That org-write gate is DORMANT today
// (an empty-CallerUserID no-op for the trusted §11.1.1 seed + org /
// rbactest / exitcriteriontest / perftest callers) and goes live once a
// future key-management / web-admin BFF pins CallerUserID from the
// resolved session identity. (Bootstrapping writes — org.CreateOrganization,
// where the caller BECOMES the owner — carry no membership gate by
// design and are correctly out of scope.) This matches SPEC §10.1 /
// §10.1.1's gate model: each service's write tenant gate lives in its own
// RPC, keyed on a caller-identity channel pinned off-wire from the
// resolved session — the workitems write gate keys on the CallerOrgID
// channel above; the org provisioning writes key on their own
// CallerUserID membership predicate.
package workitems

import (
	"context"
	"errors"
	"fmt"
	"regexp"
	"strings"
	"time"
	"unicode/utf8"

	"encore.app/auth"
	"encore.app/deps"
	"encore.app/shared/rbac"
	"encore.app/shared/tracectx"
	"encore.app/shared/ulid"
	encoreauth "encore.dev/beta/auth"
	"encore.dev/beta/errs"
	"encore.dev/rlog"
	"encore.dev/storage/sqldb"
)

// -----------------------------------------------------------------------------
// Locked type surface (SPEC §4.4). Field shapes are wire-locked; do not edit
// without a spec amendment.
//
// CreateRequest.Dependencies uses deps.Edge directly per SPEC §4.4 lines
// 591-592 — the skeleton-time local workitems.Edge struct was removed in
// C-1 (bead unblock-tv8.10), closing findings unblock-tv8.27 and
// unblock-tv8.28. Trail.DependenciesIn/Out were subsequently WIDENED from
// []deps.Edge to []ResolvedRef in round-16 (bead unblock-tv8.76): `show`
// resolves the FAR dependency target to {id,title,status,kind} rather than
// returning the bare edge row.
// -----------------------------------------------------------------------------

// Item is the canonical work-item row shape. SPEC §4.4.
type Item struct {
	ID                  string     `json:"id"`
	OrgID               string     `json:"org_id"`
	ProjectID           string     `json:"project_id"`
	MilestoneID         string     `json:"milestone_id"`
	ParentID            string     `json:"parent_id"`
	DiscoveredFromID    string     `json:"discovered_from_id"`
	Type                string     `json:"type"`
	Title               string     `json:"title"`
	Body                string     `json:"body"`
	Status              string     `json:"status"`
	Priority            string     `json:"priority"`
	PipelineStage       string     `json:"pipeline_stage"`
	AgentKind           string     `json:"agent_kind"`
	ImplState           string     `json:"impl_state"`
	ReviewState         string     `json:"review_state"`
	QAState             string     `json:"qa_state"`
	PipelineState       string     `json:"pipeline_state"`
	Severity            string     `json:"severity"`
	KindOfFinding       string     `json:"kind_of_finding"`
	ClaimedByID         string     `json:"claimed_by_id"`
	ClaimedByAgent      string     `json:"claimed_by_agent"`
	ClaimedAt           *time.Time `json:"claimed_at"`
	IsReady             bool       `json:"is_ready"`
	MilestoneAssignedAt *time.Time `json:"milestone_assigned_at"`
	MilestoneAssignedBy string     `json:"milestone_assigned_by"`
	Labels              []string   `json:"labels"`
	CreatedAt           time.Time  `json:"created_at"`
	UpdatedAt           time.Time  `json:"updated_at"`
	ClosedAt            *time.Time `json:"closed_at"`
}

// CreateRequest is the input to Create. SPEC §4.4.
type CreateRequest struct {
	OrgID            string      `json:"org_id"`
	ProjectID        string      `json:"project_id"`
	ParentID         string      `json:"parent_id"`
	DiscoveredFromID string      `json:"discovered_from_id"`
	Type             string      `json:"type"`
	Title            string      `json:"title"`
	Body             string      `json:"body"`
	Priority         string      `json:"priority"`
	MilestoneID      string      `json:"milestone_id"`
	Labels           []string    `json:"labels"`
	Dependencies     []deps.Edge `json:"dependencies"`
	Severity         string      `json:"severity"`
	KindOfFinding    string      `json:"kind_of_finding"`
}

// Comment is the canonical comment row shape. SPEC §4.4.
type Comment struct {
	ID          string    `json:"id"`
	ItemID      string    `json:"item_id"`
	ParentID    string    `json:"parent_id"`
	AuthorID    string    `json:"author_id"`
	AuthorAgent string    `json:"author_agent"`
	Kind        string    `json:"kind"`
	Status      string    `json:"status"`
	Body        string    `json:"body"`
	CreatedAt   time.Time `json:"created_at"`
	UpdatedAt   time.Time `json:"updated_at"`
}

// UpdateRequest is the input to Update. SPEC §4.4.
//
// CallerOrgID is NOT a wire argument — the MCP handler pins it from the
// Bearer-resolved org-scoped Identity (identity.OrgID) and passes it
// RPC-side so Update self-gates its UPDATE on org_id = CallerOrgID. A
// foreign ItemID matches zero rows → NOT_FOUND, never a cross-tenant
// mutation. An empty CallerOrgID is the §10.1.1 no-op for trusted internal
// callers (the §11.1.1 E2E seed + integration tests); the MCP handler
// always pins it, so the no-op is unreachable from the agent surface
// (round-16 / bead unblock-tv8.77).
type UpdateRequest struct {
	ItemID      string    `json:"item_id"`
	CallerOrgID string    `json:"caller_org_id"`
	Title       *string   `json:"title"`
	Body        *string   `json:"body"`
	Priority    *string   `json:"priority"`
	MilestoneID *string   `json:"milestone_id"`
	Labels      *[]string `json:"labels"`
}

// GetTrailRequest is the input to GetTrail. SPEC §4.4.
type GetTrailRequest struct {
	ItemID string `json:"item_id"`
}

// Trail is the comment + resolved-neighbours + findings bundle returned
// by GetTrail. SPEC §4.4.
//
// Round-16 / bead unblock-tv8.76: Parent + DependenciesIn/Out resolve the
// item's immediate neighbourhood to {id,title,status,kind} ResolvedRefs so
// an agent can render it without N follow-up show/get calls. Resolution is
// bounded to exactly ONE level — the resolved neighbours' own parents and
// dependencies are NOT walked. All resolved rows are RBAC-scoped
// identically to the root item, so a cross-tenant neighbour is omitted
// rather than leaked.
type Trail struct {
	Item            *Item         `json:"item"`
	Parent          *ResolvedRef  `json:"parent"` // nil when the item has no parent epic
	Comments        []Comment     `json:"comments"`
	DependenciesIn  []ResolvedRef `json:"dependencies_in"`  // edges where to_item == Item.ID, target resolved
	DependenciesOut []ResolvedRef `json:"dependencies_out"` // edges where from_item == Item.ID, target resolved
	Findings        []Item        `json:"findings"`
}

// ResolvedRef is a one-level-deep resolution of a related item (parent or
// dependency target) to its identity + display fields. Bounded by design:
// no body, no comments, no nested neighbours (round-16 / bead
// unblock-tv8.76, SPEC §4.4).
type ResolvedRef struct {
	ID     string `json:"id"`     // target item ULID
	Title  string `json:"title"`  // target item title
	Status string `json:"status"` // target item Status enum (§6.1)
	Kind   string `json:"kind"`   // edge kind ("blocks" | "related"); empty for the parent ref
}

// AppendCommentRequest is the input to AppendComment. SPEC §4.4.
//
// CallerOrgID is NOT a wire argument — the MCP handler pins it from the
// Bearer-resolved org-scoped Identity (identity.OrgID). AppendComment is an
// INSERT, so it gates via INSERT … SELECT predicated on the PARENT item's
// org_id = CallerOrgID: a foreign ItemID inserts zero rows → NOT_FOUND,
// never a cross-tenant comment. An empty CallerOrgID is the §10.1.1 no-op
// for trusted internal callers; the MCP handler always pins it (round-16 /
// bead unblock-tv8.77).
type AppendCommentRequest struct {
	ItemID      string `json:"item_id"`
	CallerOrgID string `json:"caller_org_id"`
	AuthorID    string `json:"author_id"`
	AuthorAgent string `json:"author_agent"`
	ParentID    string `json:"parent_id"`
	Kind        string `json:"kind"`
	Status      string `json:"status"`
	Body        string `json:"body"`
}

// SetStateRequest is the input to SetStateColumns. SPEC §4.4.
//
// CallerOrgID is NOT a wire argument — the MCP handler pins it from the
// Bearer-resolved org-scoped Identity (identity.OrgID) so SetStateColumns
// self-gates the SELECT … FOR UPDATE row lock on org_id = CallerOrgID. A
// foreign ItemID yields NOT_FOUND BEFORE any invariant check runs, never a
// cross-tenant state mutation. An empty CallerOrgID is the §10.1.1 no-op
// for trusted internal callers; the MCP handler always pins it (round-16 /
// bead unblock-tv8.77).
type SetStateRequest struct {
	ItemID        string  `json:"item_id"`
	CallerOrgID   string  `json:"caller_org_id"`
	ImplState     *string `json:"impl_state"`
	ReviewState   *string `json:"review_state"`
	QAState       *string `json:"qa_state"`
	PipelineState *string `json:"pipeline_state"`
}

// GetStateRequest is the input to GetState. SPEC §6.2 Tool 14 line
// 1730-1733.
type GetStateRequest struct {
	ItemID string `json:"item_id"`
}

// RecentKindRow is a single (kind, status, comment_id, created_at) tuple
// returned by GetState's `recent_kinds` aggregate. SPEC §6.2 Tool 14
// lines 1745-1750: one row per distinct comment kind on the item,
// carrying the most recent (status, comment_id, created_at) for that
// kind. Ordered by `kind` ASC for deterministic wire output.
type RecentKindRow struct {
	Kind      string    `json:"kind"`
	Status    string    `json:"status"`
	CommentID string    `json:"comment_id"`
	CreatedAt time.Time `json:"created_at"`
}

// GetStateResponse is the output of GetState. SPEC §6.2 Tool 14 lines
// 1736-1751: every state dimension on the item plus the per-kind
// `recent_kinds` aggregate from workitems.comments. The four state
// columns surface as plain strings (never pointers) because every item
// row has them populated via column defaults — the empty string here
// would indicate a corrupted row.
//
// ProjectID is the item's project scope, sourced from the same row
// already loaded by the rbac.For org gate. It is exposed so the MCP
// handler can stamp `state.Call.ProjectID` on the audit row (SPEC §8.1
// requires per-call project_id for dashboard filtering) and surface a
// top-level `project_id` field on the §6.2 Tool 14 wire envelope (the
// state surface is contextually scoped to a project).
type GetStateResponse struct {
	ProjectID     string          `json:"project_id"`
	ImplState     string          `json:"impl_state"`
	ReviewState   string          `json:"review_state"`
	QAState       string          `json:"qa_state"`
	PipelineState string          `json:"pipeline_state"`
	PipelineStage string          `json:"pipeline_stage"`
	IsReady       bool            `json:"is_ready"`
	ClaimedByID   string          `json:"claimed_by_id"`
	ClaimedAt     *time.Time      `json:"claimed_at"`
	RecentKinds   []RecentKindRow `json:"recent_kinds"`
}

// CloseRequest is the input to Close. SPEC §4.4.
//
// CallerOrgID is NOT a wire argument — the MCP handler pins it from the
// Bearer-resolved org-scoped Identity (identity.OrgID) so Close self-gates
// the SELECT … FOR UPDATE row lock on org_id = CallerOrgID, checked BEFORE
// the AF3 claimed_by_id precondition. A foreign ItemID yields NOT_FOUND,
// never a cross-tenant close. An empty CallerOrgID is the §10.1.1 no-op for
// trusted internal callers; the MCP handler always pins it (round-16 /
// bead unblock-tv8.77).
type CloseRequest struct {
	ItemID      string `json:"item_id"`
	CallerOrgID string `json:"caller_org_id"`
	Reason      string `json:"reason"`
}

// ClaimRequest is the input to Claim. SPEC §4.4.
//
// CallerOrgID is NOT a wire argument — the MCP handler pins it from the
// Bearer-resolved org-scoped Identity (identity.OrgID) so Claim self-gates
// the SELECT … FOR UPDATE row lock on org_id = CallerOrgID. A foreign
// ItemID yields NOT_FOUND (not ALREADY_CLAIMED), never a cross-tenant
// claim. An empty CallerOrgID is the §10.1.1 no-op for trusted internal
// callers; the MCP handler always pins it (round-16 / bead unblock-tv8.77).
type ClaimRequest struct {
	ItemID        string `json:"item_id"`
	CallerOrgID   string `json:"caller_org_id"`
	ClaimerUserID string `json:"claimer_user_id"`
	ClaimerAgent  string `json:"claimer_agent"`
}

// ListRequest is the input to List. SPEC §4.4.
type ListRequest struct {
	OrgID         string   `json:"org_id"`
	ProjectID     string   `json:"project_id"`
	MilestoneID   string   `json:"milestone_id"`
	Status        []string `json:"status"`
	PipelineStage []string `json:"pipeline_stage"`
	ClaimedBy     string   `json:"claimed_by"`
	Labels        []string `json:"labels"`
	Limit         int      `json:"limit"`
	Cursor        string   `json:"cursor"`
}

// ListResponse is the output of List. SPEC §4.4.
type ListResponse struct {
	Items      []Item `json:"items"`
	NextCursor string `json:"next_cursor"`
}

// ReadyRequest is the input to Ready. SPEC §6.2 Tool 2 (lines 1177-1206)
// + §6.2.0 (cursor keyset pagination).
//
// Empty ProjectID means org-wide scope (caller has no "primary project"
// concept in P01 — see SPEC §6.2 line 1161 "defaults to caller's
// primary project"; until a primary-project column ships, the MCP
// layer collapses missing project_id to org-wide). PriorityMin is the
// lowest priority included in results, "P0".."P4" lexicographic
// (P0 highest, P4 lowest); empty string = no filter.
//
// Cursor fields (CursorPriority, CursorCreatedAt, CursorID) carry the
// anchor of the previous page, encoded by the MCP layer and decoded
// before this RPC is called. The RPC itself is cursor-token-agnostic
// — the MCP layer owns the §6.2.0 HMAC/opacity contract; this RPC
// only sees the resolved keyset tuple. All three fields are present
// or all three are zero (the MCP boundary enforces this); a partially
// populated triple is an invalid call and Ready will return no
// next-cursor predicate.
type ReadyRequest struct {
	// OrgID is intentionally absent — Ready pins scope to
	// identity.OrgID via rbac.For per the §10.1 canonical pattern.
	// The rework S1 removed the field so confused-deputy callers
	// cannot pass a mismatched org_id; the org gate is the
	// authenticated identity, period.
	ProjectID   string `json:"project_id"`
	Limit       int    `json:"limit"`
	PriorityMin string `json:"priority_min"`

	// Cursor anchor (§6.2.0 keyset tuple for Tool 2). When CursorID
	// is non-empty, Ready emits rows STRICTLY AFTER
	// (CursorPriority, CursorCreatedAt, CursorID) on the canonical
	// (priority ASC, created_at ASC, id ASC) order.
	CursorPriority  string    `json:"cursor_priority"`
	CursorCreatedAt time.Time `json:"cursor_created_at"`
	CursorID        string    `json:"cursor_id"`
}

// ReadyResponse is the output of Ready. SPEC §6.2 Tool 2 + §6.2.0.
//
// TotalReady is the count of ready items across the same scope so the
// caller can decide whether more exist behind the Limit cap.
// NextCursor* carries the keyset anchor of the row that would START
// the next page. All three NextCursor* fields are populated together;
// they are zero when this is the final page. The MCP layer encodes
// the triple into the opaque §6.2.0 cursor token (or null) before
// surfacing on the wire.
type ReadyResponse struct {
	Items      []Item `json:"items"`
	TotalReady int    `json:"total_ready"`

	NextCursorPriority  string    `json:"next_cursor_priority"`
	NextCursorCreatedAt time.Time `json:"next_cursor_created_at"`
	NextCursorID        string    `json:"next_cursor_id"`
}

// SearchRequest is the input to Search. SPEC §4.4 (round-8: typed
// keyset cursor fields added so the §6.2.0 / §6.2 Tool 9 contract is
// satisfied without an opaque blob in the RPC layer — the MCP layer
// owns the opaque envelope; here we carry the decoded tuple).
type SearchRequest struct {
	OrgID     string `json:"org_id"`
	ProjectID string `json:"project_id"`
	Query     string `json:"query"`
	Limit     int    `json:"limit"`

	// CursorRank / CursorItemID / CursorCommentID are populated together
	// by the MCP cursor decoder when paginating; all three zero values
	// signal "first page". The keyset predicate uses the canonical FTS
	// sort tuple `(rank desc, item_id asc, comment_id asc)` — see SPEC
	// §6.2 Tool 9 lines 1449-1452. Mirrors the Ready RPC pattern at the
	// top of this file; the spec §4.4 SearchRequest type was extended
	// in round-8 to align with the §6.2.0 cursor contract.
	CursorRank      float64 `json:"cursor_rank"`
	CursorItemID    string  `json:"cursor_item_id"`
	CursorCommentID string  `json:"cursor_comment_id"`
}

// SearchHit is one row of a Search response. SPEC §4.4.
type SearchHit struct {
	ItemID    string  `json:"item_id"`
	Source    string  `json:"source"`
	CommentID string  `json:"comment_id"`
	Rank      float64 `json:"rank"`
	Snippet   string  `json:"snippet"`
}

// SearchResponse is the output of Search. SPEC §4.4 (round-8: typed
// next-cursor fields). The MCP layer encodes the triple into the
// opaque §6.2.0 cursor token (or null) before surfacing on the wire.
type SearchResponse struct {
	Hits []SearchHit `json:"hits"`

	// NextCursorRank / NextCursorItemID / NextCursorCommentID carry the
	// keyset anchor of the row that would START the next page on the
	// canonical FTS sort tuple. All three populated together when more
	// rows exist; all three zero on end-of-stream. Search over-fetches
	// LIMIT+1 to detect end-of-stream — same pattern as Ready.
	NextCursorRank      float64 `json:"next_cursor_rank"`
	NextCursorItemID    string  `json:"next_cursor_item_id"`
	NextCursorCommentID string  `json:"next_cursor_comment_id"`
}

// Milestone is the canonical milestone row shape. SPEC §4.4.1.
type Milestone struct {
	ID                string     `json:"id"`
	ParentMilestoneID string     `json:"parent_milestone_id"`
	OrgID             string     `json:"org_id"`
	ProjectID         string     `json:"project_id"`
	Name              string     `json:"name"`
	Description       string     `json:"description"`
	StartDate         string     `json:"start_date"`
	EndDate           string     `json:"end_date"`
	CancelledAt       *time.Time `json:"cancelled_at"`
	CancelledReason   string     `json:"cancelled_reason"`
	CreatedAt         time.Time  `json:"created_at"`
	UpdatedAt         time.Time  `json:"updated_at"`
}

// CreateMilestoneRequest is the input to CreateMilestone. SPEC §4.4.1.
//
// CallerOrgID is NOT a wire argument — the MCP handler pins it from the
// Bearer-resolved org-scoped Identity (identity.OrgID). When
// ParentMilestoneID is supplied, the parent-read seam self-gates on the
// milestone tenant predicate (org_id = CallerOrgID OR project_id IN the
// caller's org's projects) so a foreign parent ULID yields NOT_FOUND,
// never a cross-tenant read leak. An empty CallerOrgID is the §10.1.1
// no-op for trusted internal callers (the §11.1.1 E2E seed); the MCP
// handler always pins it (round-16 / bead unblock-tv8.77).
type CreateMilestoneRequest struct {
	OrgID             string `json:"org_id"`
	ProjectID         string `json:"project_id"`
	CallerOrgID       string `json:"caller_org_id"`
	ParentMilestoneID string `json:"parent_milestone_id"`
	Name              string `json:"name"`
	Description       string `json:"description"`
	StartDate         string `json:"start_date"`
	EndDate           string `json:"end_date"`
}

// UpdateMilestoneRequest is the input to UpdateMilestone. SPEC §4.4.1.
//
// CallerOrgID is NOT a wire argument — the MCP handler pins it from the
// Bearer-resolved org-scoped Identity (identity.OrgID) so UpdateMilestone
// self-gates on the milestone tenant predicate (the targeted milestone's
// org_id = CallerOrgID OR its project_id IN the caller's org's projects).
// A foreign MilestoneID yields NOT_FOUND, never a cross-tenant mutation.
// An empty CallerOrgID is the §10.1.1 no-op for trusted internal callers;
// the MCP handler always pins it (round-16 / bead unblock-tv8.77).
type UpdateMilestoneRequest struct {
	MilestoneID     string     `json:"milestone_id"`
	CallerOrgID     string     `json:"caller_org_id"`
	Name            *string    `json:"name"`
	Description     *string    `json:"description"`
	StartDate       *string    `json:"start_date"`
	EndDate         *string    `json:"end_date"`
	CancelledAt     *time.Time `json:"cancelled_at"`
	CancelledReason *string    `json:"cancelled_reason"`
}

// AssignItemRequest is the input to AssignItem. SPEC §4.4.1.
//
// CallerOrgID is NOT a wire argument — the MCP handler pins it from the
// Bearer-resolved org-scoped Identity (identity.OrgID) so AssignItem
// self-gates on the TARGET item's tenancy (the item's org_id =
// CallerOrgID). A foreign ItemID yields NOT_FOUND, never a cross-tenant
// milestone assignment. An empty CallerOrgID is the §10.1.1 no-op for
// trusted internal callers (the §11.1.1 E2E seed); the MCP handler always
// pins it (round-16 / bead unblock-tv8.77).
type AssignItemRequest struct {
	ItemID         string `json:"item_id"`
	CallerOrgID    string `json:"caller_org_id"`
	MilestoneID    string `json:"milestone_id"`
	AssignedByUser string `json:"assigned_by_user"`
}

// MilestoneTreeRequest is the input to MilestoneTree. SPEC §4.4.1.
//
// CallerOrgID is NOT a wire argument — the MCP handler pins it from the
// Bearer-resolved org-scoped Identity (identity.OrgID). The recursive-CTE
// anchor self-gates on the milestone tenant predicate (org_id =
// CallerOrgID OR project_id IN the caller's org's projects). A foreign
// RootMilestoneID produces an empty anchor → no rows, closing the IDOR
// read seam. An empty CallerOrgID is the §10.1.1 no-op for trusted internal
// callers (the §11.1.1 E2E seed, the P05 roadmap RPC); the MCP handler
// always pins it (round-16 / beads unblock-tv8.75 + .77). OrgID/ProjectID
// remain the wire-supplied scope selectors for the roots walk.
type MilestoneTreeRequest struct {
	OrgID            string `json:"org_id"`
	ProjectID        string `json:"project_id"`
	CallerOrgID      string `json:"caller_org_id"`
	RootMilestoneID  string `json:"root_milestone_id"`
	IncludeCancelled bool   `json:"include_cancelled"`
}

// MilestoneNode is one node in the recursive milestone tree response.
// SPEC §4.4.1.
type MilestoneNode struct {
	Milestone Milestone       `json:"milestone"`
	Depth     int             `json:"depth"`
	Children  []MilestoneNode `json:"children"`
}

// MilestoneTreeResponse is the output of MilestoneTree. SPEC §4.4.1.
//
// (The spec names the type `MilestoneTree` but Go disallows a function
// and a top-level type sharing one name in the same package. The RPC
// route, signature shape, and JSON-on-the-wire encoding are unchanged —
// Encore serialises by field, not by type name. See DECISION trail on
// bead unblock-tv8.1.)
type MilestoneTreeResponse struct {
	Roots []MilestoneNode `json:"roots"`
}

// --- Label-registry types (round-16, bead unblock-tv8.75) ---
// Back the label MCP tools (§6.2 Tools 20–23) over the EXISTING
// workitems.labels / workitems.item_labels tables (SPEC §9.4.3). Migration
// 0130_workitems_labels_updated_at.up.sql (§3.2) adds the updated_at column
// declared by Label below. Org scoping follows the Bearer-Identity pattern
// (§6.2 closing note): the write RPCs (CreateLabel / UpdateLabel /
// DeleteLabel) trust the org-scoped Identity pinned by the MCP handler
// (identity.OrgID) and do NOT call org.Authorize; the read RPC (ListLabels)
// self-gates via an explicit tenant predicate scoped to the caller's org.

// Label is one workitems.labels row. OrgID is empty when project-scoped;
// ProjectID is empty when org-scoped (the labels_scope_xor_chk CHECK
// enforces exactly-one). SPEC §4.4.
type Label struct {
	ID          string    `json:"id"`
	OrgID       string    `json:"org_id"`
	ProjectID   string    `json:"project_id"`
	Name        string    `json:"name"`
	Color       string    `json:"color"`
	Description string    `json:"description"`
	CreatedAt   time.Time `json:"created_at"`
	UpdatedAt   time.Time `json:"updated_at"`
}

// CreateLabelRequest is the input to CreateLabel. SPEC §4.4.
//
// OrgID is NOT a wire argument — the MCP handler pins it from the
// Bearer-resolved org-scoped Identity (identity.OrgID) and passes it
// RPC-side (mirrors CreateMilestoneRequest). ProjectID is the XOR
// selector: empty → org-scoped to OrgID; non-empty → project-scoped.
//
// CallerOrgID is also pinned from identity.OrgID (never wire). On the
// project-scoped branch (ProjectID set, OrgID empty) it is the org the
// project MUST belong to: CreateLabel gates the insert on
// project_id IN (SELECT id FROM org.projects WHERE org_id = CallerOrgID),
// so a Bearer for org A cannot create a label inside org B's project by
// passing a foreign project ULID (DRIFT-2c locked decision). On the
// org-scoped branch CallerOrgID equals OrgID and the gate is a no-op.
type CreateLabelRequest struct {
	OrgID       string `json:"org_id"`
	ProjectID   string `json:"project_id"`
	CallerOrgID string `json:"caller_org_id"`
	Name        string `json:"name"`
	Color       string `json:"color"`
	Description string `json:"description"`
}

// ListLabelsRequest is the input to ListLabels. SPEC §4.4.
//
// OrgID is NOT a wire argument — the read RPC scopes to the caller's org
// via an explicit tenant predicate (org from identity.OrgID, pinned by the
// MCP handler). When ProjectID is set the RPC returns the project's labels
// PLUS the inherited org labels, applying PRD §6.4 "project wins on
// identical name".
type ListLabelsRequest struct {
	OrgID     string `json:"org_id"`
	ProjectID string `json:"project_id"`
}

// ListLabelsResponse is the output of ListLabels. SPEC §4.4.
type ListLabelsResponse struct {
	Labels []Label `json:"labels"`
}

// UpdateLabelRequest is the input to UpdateLabel. Renames and/or recolors;
// the label's scope (OrgID / ProjectID) is immutable. SPEC §4.4.
//
// CallerOrgID is NOT a wire argument — the MCP handler pins it from the
// Bearer-resolved org-scoped Identity (identity.OrgID) and passes it
// RPC-side so UpdateLabel can apply its row-level tenant predicate (the
// targeted label's org_id = CallerOrgID OR its project_id belongs to a
// project in the caller's org). A foreign LabelID yields NOT_FOUND, never
// a cross-tenant write. SPEC §4.4 (DRIFT-3b).
type UpdateLabelRequest struct {
	LabelID     string  `json:"label_id"`
	CallerOrgID string  `json:"caller_org_id"`
	Name        *string `json:"name"`
	Color       *string `json:"color"`
	Description *string `json:"description"`
}

// DeleteLabelRequest is the input to DeleteLabel. SPEC §4.4.
//
// CallerOrgID is NOT a wire argument — the MCP handler pins it from the
// Bearer-resolved org-scoped Identity (identity.OrgID) and passes it
// RPC-side so DeleteLabel can apply its row-level tenant predicate (the
// targeted label's org_id = CallerOrgID OR its project_id belongs to a
// project in the caller's org). A foreign LabelID yields NOT_FOUND, never
// a cross-tenant delete. SPEC §4.4 (DRIFT-3b).
type DeleteLabelRequest struct {
	LabelID     string `json:"label_id"`
	CallerOrgID string `json:"caller_org_id"`
}

// DeleteLabelResponse is the output of DeleteLabel. DetachedItemCount is
// the number of workitems.item_labels rows removed by the FK cascade.
// SPEC §4.4.
type DeleteLabelResponse struct {
	Deleted           bool   `json:"deleted"`
	LabelID           string `json:"label_id"`
	DetachedItemCount int    `json:"detached_item_count"`
}

// -----------------------------------------------------------------------------
// Internal vocabularies.
// -----------------------------------------------------------------------------

const (
	typeEpic    = "epic"
	typeTask    = "task"
	typeFinding = "finding"
)

const (
	statusBacklog    = "Backlog"
	statusReady      = "Ready"
	statusInProgress = "InProgress"
	statusBlocked    = "Blocked"
	statusDone       = "Done"
)

const (
	implPending = "pending"
	implDone    = "done"
)

const (
	reviewPending     = "pending"
	reviewApproved    = "approved"
	reviewNeedsRework = "needs_rework"
)

const (
	qaPending = "pending"
	qaPassed  = "passed"
	qaFailed  = "failed"
)

const (
	pipelineStateRunning         = "running"
	pipelineStateNeedsHuman      = "needs_human"
	pipelineStatePaused          = "paused"
	pipelineStateNoInvestigation = "no_investigation"
)

const (
	commentKindGeneral   = "general"
	commentKindCompleted = "completed"

	commentStatusInfo    = "info"
	commentStatusWarning = "warning"
	commentStatusError   = "error"
	commentStatusSuccess = "success"
)

var (
	typeAllowed = map[string]struct{}{
		typeEpic:    {},
		typeTask:    {},
		typeFinding: {},
	}

	priorityAllowed = map[string]struct{}{
		"P0": {}, "P1": {}, "P2": {}, "P3": {}, "P4": {},
	}

	statusAllowed = map[string]struct{}{
		statusBacklog:    {},
		statusReady:      {},
		statusInProgress: {},
		statusBlocked:    {},
		statusDone:       {},
	}

	pipelineStageAllowed = map[string]struct{}{
		"Investigation":  {},
		"Implementation": {},
		"Review":         {},
		"Quality":        {},
		"Deferred":       {},
		"Done":           {},
	}

	implStateAllowed = map[string]struct{}{
		implPending: {},
		implDone:    {},
	}

	reviewStateAllowed = map[string]struct{}{
		reviewPending:     {},
		reviewApproved:    {},
		reviewNeedsRework: {},
	}

	qaStateAllowed = map[string]struct{}{
		qaPending: {},
		qaPassed:  {},
		qaFailed:  {},
	}

	pipelineStateAllowed = map[string]struct{}{
		pipelineStateRunning:         {},
		pipelineStateNeedsHuman:      {},
		pipelineStatePaused:          {},
		pipelineStateNoInvestigation: {},
	}

	severityAllowed = map[string]struct{}{
		"critical":  {},
		"major":     {},
		"minor":     {},
		"risk":      {},
		"extra":     {},
		"deviation": {},
	}

	kindOfFindingAllowed = map[string]struct{}{
		"review": {},
		"qa":     {},
	}

	commentKindAllowed = map[string]struct{}{
		"investigation": {},
		"decision":      {},
		"deviation":     {},
		"completed":     {},
		"review":        {},
		"qa":            {},
		"deferred":      {},
		"pr":            {},
		"needs-human":   {},
		"override":      {},
		"general":       {},
	}

	commentStatusAllowed = map[string]struct{}{
		commentStatusError:   {},
		commentStatusWarning: {},
		commentStatusInfo:    {},
		commentStatusSuccess: {},
	}

	agentKindAllowed = map[string]struct{}{
		"claude-code": {},
		"copilot":     {},
		"cursor":      {},
		"codex":       {},
		"aider":       {},
		"custom":      {},
	}
)

const (
	titleMinLen = 1
	titleMaxLen = 200

	listDefaultLimit = 50
	listMaxLimit     = 200

	searchDefaultLimit = 25
	searchMaxLimit     = 100

	milestoneMaxDepth = 4 // M-INV-6

	// Label name window (SPEC §6.2 Tool 20: "1..64 chars").
	labelNameMinLen = 1
	labelNameMaxLen = 64
)

// labelColorPattern mirrors the labels_color_chk DB CHECK (#RRGGBB).
// Validating in Go first surfaces a clean §7 VALIDATION error before the
// INSERT/UPDATE reaches the DB CHECK (which is the last line of defence).
var labelColorPattern = regexp.MustCompile(`^#[0-9a-fA-F]{6}$`)

// -----------------------------------------------------------------------------
// Core RPC bodies.
// -----------------------------------------------------------------------------

// Create inserts a new workitems.items row, optionally attaches labels,
// and emits cycle-checked dependency edges via deps.AddEdge. SPEC §4.4.
//
//encore:api private method=POST path=/workitems.Create
func Create(ctx context.Context, req *CreateRequest) (*Item, error) {
	if req == nil {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing request body"}
	}
	if req.OrgID == "" {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing org_id", Meta: errs.Metadata{"field": "org_id"}}
	}

	itemType := req.Type
	if itemType == "" {
		itemType = typeTask
	}
	if _, ok := typeAllowed[itemType]; !ok {
		return nil, &errs.Error{
			Code:    errs.InvalidArgument,
			Message: fmt.Sprintf("invalid type %q (allowed: epic, task, finding)", itemType),
			Meta:    errs.Metadata{"field": "type"},
		}
	}

	title := strings.TrimSpace(req.Title)
	if err := validateTitle(title); err != nil {
		return nil, err
	}

	priority := req.Priority
	if priority == "" {
		priority = "P3"
	}
	if _, ok := priorityAllowed[priority]; !ok {
		return nil, &errs.Error{
			Code:    errs.InvalidArgument,
			Message: fmt.Sprintf("invalid priority %q (allowed: P0..P4)", priority),
			Meta:    errs.Metadata{"field": "priority"},
		}
	}

	// Finding-specific validation (mirrors items_finding_required_fields_chk
	// migration line 114-121 — early reject for a clearer error than the
	// downstream Postgres CHECK violation).
	if itemType == typeFinding {
		if req.ParentID == "" {
			return nil, &errs.Error{Code: errs.InvalidArgument, Message: "finding requires parent_id (parent epic)", Meta: errs.Metadata{"field": "parent_id"}}
		}
		if req.DiscoveredFromID == "" {
			return nil, &errs.Error{Code: errs.InvalidArgument, Message: "finding requires discovered_from_id", Meta: errs.Metadata{"field": "discovered_from_id"}}
		}
		if req.Severity == "" {
			return nil, &errs.Error{Code: errs.InvalidArgument, Message: "finding requires severity", Meta: errs.Metadata{"field": "severity"}}
		}
		if _, ok := severityAllowed[req.Severity]; !ok {
			return nil, &errs.Error{Code: errs.InvalidArgument, Message: fmt.Sprintf("invalid severity %q", req.Severity), Meta: errs.Metadata{"field": "severity"}}
		}
		if req.KindOfFinding == "" {
			return nil, &errs.Error{Code: errs.InvalidArgument, Message: "finding requires kind_of_finding", Meta: errs.Metadata{"field": "kind_of_finding"}}
		}
		if _, ok := kindOfFindingAllowed[req.KindOfFinding]; !ok {
			return nil, &errs.Error{Code: errs.InvalidArgument, Message: fmt.Sprintf("invalid kind_of_finding %q (allowed: review, qa)", req.KindOfFinding), Meta: errs.Metadata{"field": "kind_of_finding"}}
		}
	}

	id, err := ulid.New()
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "item id generation failed"}
	}

	// Atomicity contract (orchestrator DECISION on bead unblock-tv8.17,
	// 2026-05-14, decision #1): the item INSERT, label attaches, AND any
	// dependencies[] edge inserts run inside a SINGLE transaction so a
	// failure on any later edge rolls back the item row (SPEC § 6.2
	// Tool 4 line 1255: "the entire create is rejected"). Pre-D-2 the
	// Create path committed the item before the edges loop and could
	// leave phantom rows on FK / validation / network failure. The
	// mathematical note on the DECISION recognises that a Tool 4 create
	// only adds INCOMING edges to a brand-new node so cycle is
	// impossible by construction — the cycle-check remains as defensive
	// code matching SPEC § 6.2 line 1254 ("Cycle check (C5/AF5) runs
	// inline") but the real value of this refactor is atomicity against
	// FK / validation / advisory-lock contention failures.
	tx, err := db.Begin(ctx)
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "db begin failed"}
	}
	defer func() { _ = tx.Rollback() }()

	// Initial status is 'Backlog'; pipeline_stage / pipeline_state are
	// schema defaults. is_ready is set INLINE here (round-16, bead
	// unblock-tv8.71, §6.6 is_ready-on-create rule + §6.3.0 Regime A).
	//
	// A freshly-created item has no incoming 'blocks' edges yet, so it is
	// ready by construction — is_ready=true. (The column DEFAULT is false;
	// before round-16 nothing on the create path set it, and an item with
	// no incoming blockers never triggered deps.recomputeReady — the sole
	// is_ready writer runs only from AddEdge/RemoveEdge/Close — so such
	// items were stranded non-ready and thus never promote-able.) If the
	// dependencies[] argument inlines an incoming 'blocks' edge (the new
	// item as to_item), the AddEdgeInTx loop below recomputes is_ready for
	// the new item via the §6.5 NOT EXISTS predicate inside this same
	// transaction, correcting the initial true to the proper value.
	//
	// status stays 'Backlog' — is_ready=true makes the item immediately
	// promote-able (Tool 15 / §6.6), but promotion is an explicit agent
	// action, not an implicit side-effect of create.
	// Create-path cross-reference tenant gate (round-16, bead unblock-tv8.78,
	// SPEC §10.1.1 / §4.4 Create). The INSERT's FK constraints only check
	// reference EXISTENCE in ANY org — a foreign-but-existing project_id /
	// parent_id / discovered_from_id / milestone_id would otherwise be stored,
	// producing an item whose org_id differs from the referenced row's org (the
	// create-path analogue of the §10.1.1 write-by-id IDOR seam). We close that
	// seam with a guarded INSERT … SELECT: each wire reference is validated
	// against the caller org in the SELECT's WHERE before the row materialises.
	//
	// Gate-key DECISION (Miguel 2026-06-12, §10.1.1): the gate keys on the
	// existing req.OrgID — the SAME value the INSERT stamps org_id from, already
	// pinned from identity.OrgID by the MCP handler and validated non-empty at
	// :874 — NOT a separate CallerOrgID channel, and with NO empty-OrgID no-op
	// branch. This is a deliberate divergence from the .77 update/delete-by-id
	// convention: Create's internal callers (the §11.1.1 exit-criterion seed +
	// integration tests) all pass a real same-org OrgID referencing same-org
	// rows, so the non-empty req.OrgID gate passes them without a no-op branch.
	// Coverage is identical to the CallerOrgID-channel RPCs.
	//
	// Per-reference predicates (each guarded by `$n = '' OR …` so an UNSET
	// optional reference skips the gate):
	//   - project_id        → IN (SELECT id FROM org.projects WHERE org_id = $caller)
	//   - parent_id         → IN (SELECT id FROM workitems.items WHERE org_id = $caller)
	//   - discovered_from_id→ same as parent_id (a caller-org item)
	//   - milestone_id      → org_id = $caller OR project_id IN caller-org projects
	//                         (org-XOR-project; project-scoped milestones carry NULL org_id)
	//
	// A foreign reference yields ZERO inserted rows → NOT_FOUND below, the SAME
	// envelope a genuinely-missing id yields (existence in another org is never
	// disclosed). All inside the existing single tx (bead-unblock-tv8.17
	// atomicity) so a reject rolls the whole create back. The dependencies[]
	// endpoints are gated separately by deps.AddEdgeInTx below — unchanged.
	tag, err := tx.Exec(ctx,
		`INSERT INTO workitems.items
		   (id, org_id, project_id, milestone_id, parent_id, discovered_from_id,
		    type, title, body, priority,
		    severity, kind_of_finding, is_ready)
		 SELECT $1, $2, NULLIF($3, ''), NULLIF($4, ''), NULLIF($5, ''), NULLIF($6, ''),
		        $7, $8, $9, $10,
		        NULLIF($11, ''), NULLIF($12, ''), true
		  WHERE ($3 = '' OR $3 IN (SELECT id FROM org.projects WHERE org_id = $2))
		    AND ($5 = '' OR $5 IN (SELECT id FROM workitems.items WHERE org_id = $2))
		    AND ($6 = '' OR $6 IN (SELECT id FROM workitems.items WHERE org_id = $2))
		    AND ($4 = ''
		         OR $4 IN (SELECT id FROM workitems.milestones
		                    WHERE org_id = $2
		                       OR project_id IN (SELECT id FROM org.projects WHERE org_id = $2)))`,
		id, req.OrgID, req.ProjectID, req.MilestoneID, req.ParentID, req.DiscoveredFromID,
		itemType, title, req.Body, priority,
		req.Severity, req.KindOfFinding,
	)
	if err != nil {
		if isForeignKeyViolation(err) {
			return nil, &errs.Error{Code: errs.NotFound, Message: "referenced org/project/milestone/parent does not exist"}
		}
		if isCheckViolation(err, "items_finding_required_fields_chk") {
			return nil, &errs.Error{Code: errs.InvalidArgument, Message: "finding required fields not satisfied"}
		}
		rlog.Error("workitems: create insert failed", "err", err, "org_id", req.OrgID)
		return nil, &errs.Error{Code: errs.Internal, Message: "create insert failed"}
	}
	// Zero inserted rows means a cross-reference tenant gate rejected the
	// INSERT: one of project_id / parent_id / discovered_from_id / milestone_id
	// does not belong to the caller's org (or does not exist). Surface
	// NOT_FOUND — the same shape a genuinely-missing reference yields — so a
	// foreign-but-existing id is indistinguishable from a missing one and never
	// leaks existence across the tenant boundary (§10.1.1, CreateLabel
	// zero-rows precedent).
	if tag.RowsAffected() == 0 {
		return nil, &errs.Error{Code: errs.NotFound, Message: "referenced org/project/milestone/parent does not exist"}
	}

	// Attach labels in the same transaction. attachLabelsTx gates each
	// wire-supplied label_id against the caller org (req.OrgID, the same
	// create-path gate key as the cross-references above, §10.1.1): a foreign
	// label_id attaches nothing and yields NOT_FOUND — never a cross-tenant
	// attach (round-16, bead unblock-tv8.78).
	if len(req.Labels) > 0 {
		if err := attachLabelsTx(ctx, tx, id, req.OrgID, req.Labels); err != nil {
			return nil, err
		}
	}

	// Cycle-checked edges via deps.AddEdgeInTx (the package-internal
	// helper introduced under D-2 / unblock-tv8.17 — see
	// deps/deps.go::AddEdgeInTx doc-comment). Each edge runs inside the
	// CURRENT tx with the §6.5 per-project advisory lock + depth-counter
	// CTE; any error rolls the entire create back (item row included).
	postCommits := make([]deps.AddEdgeInTxPostCommit, 0, len(req.Dependencies))
	for _, edge := range req.Dependencies {
		kind := edge.Kind
		if kind == "" {
			kind = "blocks"
		}
		// CallerOrgID is the just-created item's org (req.OrgID is itself
		// pinned from identity.OrgID by the MCP handler). deps.AddEdgeInTx
		// gates both endpoints on CallerOrgID, so a Create that names a
		// foreign FromItem is rejected NOT_FOUND. The empty-CallerOrgID no-op
		// covers the trusted internal create path (round-16 / bead
		// unblock-tv8.77, §10.1.1).
		_, postCommit, err := deps.AddEdgeInTx(ctx, tx, &deps.AddEdgeRequest{
			OrgID:       req.OrgID,
			ProjectID:   req.ProjectID,
			CallerOrgID: req.OrgID,
			FromItem:    edge.FromItem,
			ToItem:      id,
			Kind:        kind,
		})
		if err != nil {
			return nil, err
		}
		postCommits = append(postCommits, postCommit)
	}

	if err := tx.Commit(); err != nil {
		rlog.Error("workitems: create commit failed", "err", err)
		return nil, &errs.Error{Code: errs.Internal, Message: "create commit failed"}
	}

	// Regime B post-commit publishes for each newly created edge. Same
	// best-effort semantics as deps.AddEdge's standalone path: a publish
	// failure does NOT roll back the edge (already committed).
	for _, postCommit := range postCommits {
		postCommit(ctx)
	}

	return readItem(ctx, id)
}

// Update mutates editable item columns (title, body, priority,
// milestone_id, labels). nil pointer fields preserve the current value;
// Labels=*[]string follows SPEC §4.4 line 643 — pointer present is
// full-replace (empty slice clears all labels).
//
//encore:api private method=POST path=/workitems.Update
func Update(ctx context.Context, req *UpdateRequest) (*Item, error) {
	if req == nil || req.ItemID == "" {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing item_id"}
	}
	if req.Title != nil {
		t := strings.TrimSpace(*req.Title)
		if err := validateTitle(t); err != nil {
			return nil, err
		}
		req.Title = &t
	}
	if req.Priority != nil {
		if _, ok := priorityAllowed[*req.Priority]; !ok {
			return nil, &errs.Error{Code: errs.InvalidArgument, Message: fmt.Sprintf("invalid priority %q", *req.Priority), Meta: errs.Metadata{"field": "priority"}}
		}
	}

	tx, err := db.Begin(ctx)
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "db begin failed"}
	}
	defer func() { _ = tx.Rollback() }()

	// Build a COALESCE-based UPDATE so unspecified fields preserve the
	// existing column value. milestone_id uses a sentinel — when the
	// pointer is set to "" it clears, otherwise it sets the new value.
	// Title/Body/Priority follow the same nil = unchanged contract.
	//
	// Row-level tenant gate (round-16 / bead unblock-tv8.77, §10.1.1): the
	// WHERE clause's ($7 = '' OR org_id = $7) predicate is the TARGET-ITEM
	// IDOR gate. A foreign ItemID (org_id != CallerOrgID) matches zero rows →
	// NOT_FOUND below, never a cross-tenant mutation. The empty-CallerOrgID
	// no-op keeps trusted internal callers (the §11.1.1 E2E seed, integration
	// tests) operating unscoped; the MCP handler always pins CallerOrgID from
	// identity.OrgID, so the no-op is unreachable from the agent surface.
	//
	// Milestone-write tenant gate (bead unblock-tv8.84, §10.1.1): DISTINCT
	// from and ADDITIONAL to the target-item gate above. When the request
	// sets a non-empty milestone_id ($6 != ''), the UPDATE additionally
	// requires that milestone to belong to the caller's org via the
	// org-XOR-project predicate ($6 IN milestones WHERE org_id = $7 OR
	// project_id IN caller-org projects) — mirroring the EXACT sibling gates
	// on the two other paths that write items.milestone_id, workitems.Create
	// (~1044) and AssignItem (~3137). A foreign-but-existing milestone_id
	// matches zero rows → NOT_FOUND, the item UNCHANGED, indistinguishable
	// from a missing milestone (the existence-only FK at 0040:50 would
	// otherwise pass). The clear-to-null path (milestone_id = "") and the
	// nil = unchanged path both set $6 = '' and satisfy the empty disjunct,
	// carrying NO milestone predicate — PRESERVED. The empty-CallerOrgID
	// no-op ($7 = '') is PRESERVED for those empty-milestone ($6 = '')
	// paths. It is NARROWED for the set-milestone case: when CallerOrgID is
	// empty AND a non-empty milestone_id is supplied, the milestone subquery
	// (org_id = '' OR project_id IN projects of org_id = '') matches nothing →
	// zero rows → NOT_FOUND, rather than an unscoped set. This is unreachable
	// in practice — the MCP handler always pins CallerOrgID from identity.OrgID
	// and no trusted internal caller sets a milestone via Update with an empty
	// CallerOrgID — and mirrors the Create (~1044) / AssignItem (~3137)
	// precedent exactly.
	tag, err := tx.Exec(ctx,
		`UPDATE workitems.items
		    SET title       = COALESCE($2, title),
		        body        = COALESCE($3, body),
		        priority    = COALESCE($4, priority),
		        milestone_id = CASE
		                         WHEN $5::boolean THEN NULLIF($6, '')
		                         ELSE milestone_id
		                       END,
		        updated_at  = now()
		  WHERE id = $1
		    AND ($7 = '' OR org_id = $7)
		    AND ($6 = ''
		         OR $6 IN (SELECT id FROM workitems.milestones
		                    WHERE org_id = $7
		                       OR project_id IN (SELECT id FROM org.projects WHERE org_id = $7)))`,
		req.ItemID, req.Title, req.Body, req.Priority,
		req.MilestoneID != nil, derefString(req.MilestoneID),
		req.CallerOrgID,
	)
	if err != nil {
		if isForeignKeyViolation(err) {
			return nil, &errs.Error{Code: errs.NotFound, Message: "referenced milestone does not exist"}
		}
		rlog.Error("workitems: update failed", "err", err, "item_id", req.ItemID)
		return nil, &errs.Error{Code: errs.Internal, Message: "update failed"}
	}
	// Zero affected rows means one of: the item does not exist, it belongs to
	// another tenant (org_id != CallerOrgID), OR a non-empty milestone_id
	// belongs to another tenant (the unblock-tv8.84 milestone-write gate).
	// Surface NOT_FOUND for all — a cross-tenant ItemID or milestone_id is
	// indistinguishable from a missing one and never leaks existence across
	// the tenant boundary.
	if tag.RowsAffected() == 0 {
		return nil, &errs.Error{Code: errs.NotFound, Message: "item not found"}
	}

	// Labels: full-replace when the pointer is set (empty slice = clear).
	if req.Labels != nil {
		if _, err := tx.Exec(ctx, `DELETE FROM workitems.item_labels WHERE item_id = $1`, req.ItemID); err != nil {
			return nil, &errs.Error{Code: errs.Internal, Message: "label clear failed"}
		}
		if len(*req.Labels) > 0 {
			// Gate replacement labels on the .77 CallerOrgID channel (empty for
			// trusted internal callers → no-op; MCP handlers always pin it). A
			// foreign label_id attaches nothing → NOT_FOUND, matching the
			// create-path label gate (round-16, bead unblock-tv8.78, §10.1.1).
			if err := attachLabelsTx(ctx, tx, req.ItemID, req.CallerOrgID, *req.Labels); err != nil {
				return nil, err
			}
		}
	}

	if err := tx.Commit(); err != nil {
		rlog.Error("workitems: update commit failed", "err", err)
		return nil, &errs.Error{Code: errs.Internal, Message: "update commit failed"}
	}

	return readItem(ctx, req.ItemID)
}

// Get returns a single workitems.items row scoped to the caller's org
// via rbac.For. SPEC §4.4.
//
//encore:api private method=GET path=/workitems.Get/:id
func Get(ctx context.Context, id string) (*Item, error) {
	if id == "" {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing id"}
	}
	identity, ok := callerIdentity(ctx)
	if !ok {
		return nil, &errs.Error{Code: errs.Unauthenticated, Message: "no caller identity"}
	}
	rows, err := rbac.For[itemRow](identity, "workitems.items").
		Where("id = $1", id).
		Run(ctx)
	if err != nil {
		rlog.Error("workitems: get failed", "err", err, "id", id)
		return nil, &errs.Error{Code: errs.Internal, Message: "get failed"}
	}
	if len(rows) == 0 {
		return nil, &errs.Error{Code: errs.NotFound, Message: "item not found"}
	}
	item, err := itemFromRow(ctx, rows[0])
	if err != nil {
		return nil, err
	}
	return item, nil
}

// GetState returns the four state dimensions + materialised
// pipeline_stage + is_ready + claim columns + the per-kind
// `recent_kinds` aggregate from workitems.comments. SPEC §6.2 Tool 14
// (lines 1727-1754).
//
// Read-side org gate: the item lookup uses rbac.For (same pattern as
// Get), so a cross-org item_id surfaces as NotFound to the caller. The
// recent_kinds query is scoped to the resolved item_id — no separate
// org predicate is required because the item lookup above already
// validated the caller's org owns the row, and workitems.comments.item_id
// is a FK to workitems.items.id (cross-org comments are impossible by
// construction).
//
// `recent_kinds` SQL shape: `SELECT DISTINCT ON (kind) kind, status,
// id, created_at FROM workitems.comments WHERE item_id = $1 ORDER BY
// kind ASC, created_at DESC`. Postgres' DISTINCT ON returns the FIRST
// row per `kind` partition under the supplied ORDER BY, so the
// secondary `created_at DESC` term picks the most recent comment per
// kind. The outer ordering also drives wire stability (kind ASC) so
// downstream consumers see a deterministic sequence.
//
// No covering index on (kind, created_at DESC) ships in P01 — the
// query plan against a small comment table per item is acceptable.
// Index addition is deferred to the NFR-1 latency harness (bead
// unblock-tv8.24) per the D-6 INVESTIGATION risk R6.
//
//encore:api private method=POST path=/workitems.GetState
func GetState(ctx context.Context, req *GetStateRequest) (*GetStateResponse, error) {
	if req == nil || req.ItemID == "" {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing item_id"}
	}
	identity, ok := callerIdentity(ctx)
	if !ok {
		return nil, &errs.Error{Code: errs.Unauthenticated, Message: "no caller identity"}
	}

	// Item read via rbac.For pins the org gate at the SQL layer; an
	// item_id from another org surfaces as NotFound here, never as a
	// stale row leak to the MCP wire.
	itemRows, err := rbac.For[itemRow](identity, "workitems.items").
		Where("id = $1", req.ItemID).
		Run(ctx)
	if err != nil {
		rlog.Error("workitems: get_state item fetch failed", "err", err, "item_id", req.ItemID)
		return nil, &errs.Error{Code: errs.Internal, Message: "get_state fetch failed"}
	}
	if len(itemRows) == 0 {
		return nil, &errs.Error{Code: errs.NotFound, Message: "item not found"}
	}
	item, err := itemFromRow(ctx, itemRows[0])
	if err != nil {
		return nil, err
	}

	// recent_kinds: one row per distinct comment kind on this item, with
	// the most recent (status, comment_id, created_at) per kind. Ordered
	// by kind ASC for deterministic wire output.
	kindRows, err := db.Query(ctx,
		`SELECT DISTINCT ON (kind) kind, status, id, created_at
		   FROM workitems.comments
		  WHERE item_id = $1
		  ORDER BY kind ASC, created_at DESC`,
		req.ItemID,
	)
	if err != nil {
		rlog.Error("workitems: get_state recent_kinds query failed", "err", err, "item_id", req.ItemID)
		return nil, &errs.Error{Code: errs.Internal, Message: "get_state recent_kinds fetch failed"}
	}
	defer kindRows.Close()
	var recent []RecentKindRow
	for kindRows.Next() {
		var r RecentKindRow
		if err := kindRows.Scan(&r.Kind, &r.Status, &r.CommentID, &r.CreatedAt); err != nil {
			return nil, &errs.Error{Code: errs.Internal, Message: "get_state recent_kinds scan failed"}
		}
		recent = append(recent, r)
	}
	if err := kindRows.Err(); err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "get_state recent_kinds iter failed"}
	}

	return &GetStateResponse{
		ProjectID:     item.ProjectID,
		ImplState:     item.ImplState,
		ReviewState:   item.ReviewState,
		QAState:       item.QAState,
		PipelineState: item.PipelineState,
		PipelineStage: item.PipelineStage,
		IsReady:       item.IsReady,
		ClaimedByID:   item.ClaimedByID,
		ClaimedAt:     item.ClaimedAt,
		RecentKinds:   recent,
	}, nil
}

// GetTrail returns the item plus its parent and direct in/out dependency
// targets resolved to {id,title,status,kind} (one level, org-scoped), its
// comments, and findings. SPEC §4.4.
//
//encore:api private method=POST path=/workitems.GetTrail
func GetTrail(ctx context.Context, req *GetTrailRequest) (*Trail, error) {
	if req == nil || req.ItemID == "" {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing item_id"}
	}
	identity, ok := callerIdentity(ctx)
	if !ok {
		return nil, &errs.Error{Code: errs.Unauthenticated, Message: "no caller identity"}
	}

	// Fetch the item with org-scope predicate.
	rows, err := rbac.For[itemRow](identity, "workitems.items").
		Where("id = $1", req.ItemID).
		Run(ctx)
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "trail item fetch failed"}
	}
	if len(rows) == 0 {
		return nil, &errs.Error{Code: errs.NotFound, Message: "item not found"}
	}
	item, err := itemFromRow(ctx, rows[0])
	if err != nil {
		return nil, err
	}

	// Comments — ordered by created_at asc, scoped to the same item.
	// Comments rows do not carry org_id directly; the parent item's
	// org_id is the scope gate (already validated above).
	commentRows, err := db.Query(ctx,
		`SELECT id, item_id, COALESCE(parent_id, ''), COALESCE(author_id, ''),
		        COALESCE(author_agent, ''), kind, status, body,
		        created_at, updated_at
		   FROM workitems.comments
		  WHERE item_id = $1
		  ORDER BY created_at ASC`,
		req.ItemID,
	)
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "trail comments fetch failed"}
	}
	defer commentRows.Close()
	var comments []Comment
	for commentRows.Next() {
		var c Comment
		if err := commentRows.Scan(
			&c.ID, &c.ItemID, &c.ParentID, &c.AuthorID,
			&c.AuthorAgent, &c.Kind, &c.Status, &c.Body,
			&c.CreatedAt, &c.UpdatedAt,
		); err != nil {
			return nil, &errs.Error{Code: errs.Internal, Message: "trail comment scan failed"}
		}
		comments = append(comments, c)
	}
	if err := commentRows.Err(); err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "trail comments iter failed"}
	}

	// Parent: resolved to {id,title,status} (Kind empty), nil when the
	// item has no parent or the parent is cross-tenant (omitted, not
	// leaked) — round-16 / bead unblock-tv8.76, SPEC §4.4 + §6.2 Tool 7.
	parent, err := readResolvedParent(ctx, item.ParentID, identity.OrgID)
	if err != nil {
		return nil, err
	}

	// DependenciesIn (edges where to_item = item.id) and
	// DependenciesOut (edges where from_item = item.id), each with the
	// FAR endpoint resolved to {id,title,status,kind} via a single
	// org-scoped JOIN — one round-trip per direction (SPEC §6.2 Tool 7).
	in, err := readResolvedEdges(ctx, "in", req.ItemID, identity.OrgID)
	if err != nil {
		return nil, err
	}
	out, err := readResolvedEdges(ctx, "out", req.ItemID, identity.OrgID)
	if err != nil {
		return nil, err
	}

	// Findings: child items with type='finding'.
	findingRows, err := db.Query(ctx,
		`SELECT `+itemColumnList+`
		   FROM workitems.items
		  WHERE parent_id = $1 AND type = 'finding' AND org_id = $2
		  ORDER BY created_at ASC`,
		req.ItemID, identity.OrgID,
	)
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "trail findings fetch failed"}
	}
	defer findingRows.Close()
	var findings []Item
	for findingRows.Next() {
		var r itemRow
		if err := scanItemRow(findingRows, &r); err != nil {
			return nil, &errs.Error{Code: errs.Internal, Message: "trail finding scan failed"}
		}
		f, err := itemFromRow(ctx, r)
		if err != nil {
			return nil, err
		}
		findings = append(findings, *f)
	}
	if err := findingRows.Err(); err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "trail findings iter failed"}
	}

	return &Trail{
		Item:            item,
		Parent:          parent,
		Comments:        comments,
		DependenciesIn:  in,
		DependenciesOut: out,
		Findings:        findings,
	}, nil
}

// AppendComment inserts a workitems.comments row. SPEC §4.4. The
// (kind, status) pair is validated against the DB CHECK enums before
// the INSERT so callers get a clearer error than a Postgres CHECK
// violation.
//
// Tenant + threading gate (§10.1.1, §6.2 Tool 10): the INSERT … SELECT
// gates the target item on org_id = CallerOrgID (empty CallerOrgID is the
// trusted-internal no-op), and — when ParentID is non-empty — additionally
// requires the parent comment to live on the SAME target item (parent_id IN
// (SELECT id FROM workitems.comments WHERE item_id = $target_item), bead
// unblock-tv8.80). Same-item transitively implies same-org, so no separate
// parent-org branch is needed; a foreign-org OR cross-item parent_id inserts
// zero rows → NOT_FOUND, indistinguishable from a missing parent. The
// empty-ParentID top-level-comment path and the self-parent prohibition
// (comments_no_self_parent_chk) are preserved.
//
//encore:api private method=POST path=/workitems.AppendComment
func AppendComment(ctx context.Context, req *AppendCommentRequest) (*Comment, error) {
	if req == nil || req.ItemID == "" {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing item_id"}
	}
	if req.AuthorID == "" && req.AuthorAgent == "" {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "comment requires author_id OR author_agent"}
	}
	kind := req.Kind
	if kind == "" {
		kind = commentKindGeneral
	}
	if _, ok := commentKindAllowed[kind]; !ok {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: fmt.Sprintf("invalid comment kind %q", kind), Meta: errs.Metadata{"field": "kind"}}
	}
	status := req.Status
	if status == "" {
		status = commentStatusInfo
	}
	if _, ok := commentStatusAllowed[status]; !ok {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: fmt.Sprintf("invalid comment status %q", status), Meta: errs.Metadata{"field": "status"}}
	}
	if req.AuthorAgent != "" {
		if _, ok := agentKindAllowed[req.AuthorAgent]; !ok {
			return nil, &errs.Error{Code: errs.InvalidArgument, Message: fmt.Sprintf("invalid author_agent %q", req.AuthorAgent), Meta: errs.Metadata{"field": "author_agent"}}
		}
	}
	if strings.TrimSpace(req.Body) == "" {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "comment body must be non-empty", Meta: errs.Metadata{"field": "body"}}
	}

	id, err := ulid.New()
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "comment id generation failed"}
	}

	// Row-level tenant gate (round-16 / bead unblock-tv8.77, §10.1.1):
	// AppendComment is an INSERT, so it gates via INSERT … SELECT predicated
	// on the PARENT item's org_id = CallerOrgID. The SELECT yields a row
	// ONLY when the target item exists AND ($9 = '' OR its org_id = $9) — a
	// foreign ItemID (org_id != CallerOrgID) yields zero source rows → zero
	// inserted rows → NOT_FOUND below, never a cross-tenant comment. The
	// empty-CallerOrgID no-op keeps trusted internal callers operating
	// unscoped; the MCP handler always pins CallerOrgID from identity.OrgID,
	// so the no-op is unreachable from the agent surface.
	//
	// parent_id same-item threading scope (bead unblock-tv8.80, §10.1.1,
	// §6.2 Tool 10, contract LOCKED by Miguel 2026-06-12): when ParentID is
	// non-empty it MUST resolve to an existing comment ON THE SAME target
	// item ($2) — the AND ($3 = '' OR $3 IN (SELECT id FROM
	// workitems.comments WHERE item_id = $2)) arm below. The target item is
	// already CallerOrgID-gated by the i.org_id predicate, so same-item
	// transitively guarantees same-org; no separate parent-org branch is
	// needed. A foreign-org OR cross-item parent_id matches zero comments →
	// zero source rows → zero inserted rows → NOT_FOUND, indistinguishable
	// from a missing parent (closes the live-proven parent_id IDOR). The
	// empty-ParentID arm preserves the top-level-comment path. The
	// self-parent prohibition is still enforced by comments_no_self_parent_chk
	// (a comment cannot be its own parent even on the same item). The
	// existence-only inline FK to workitems.comments(id) (migration 0040,
	// ON DELETE SET NULL, unnamed — there is NO constraint named
	// comments_parent_fk) is retained as defense-in-depth; the INSERT … SELECT
	// predicate is the primary, tenant- and item-scoped gate.
	tag, err := db.Exec(ctx,
		`INSERT INTO workitems.comments
		   (id, item_id, parent_id, author_id, author_agent, kind, status, body)
		 SELECT $1, i.id, NULLIF($3, ''), NULLIF($4, ''), NULLIF($5, ''), $6, $7, $8
		   FROM workitems.items i
		  WHERE i.id = $2
		    AND ($9 = '' OR i.org_id = $9)
		    AND ($3 = ''
		         OR $3 IN (SELECT id FROM workitems.comments WHERE item_id = $2))`,
		id, req.ItemID, req.ParentID, req.AuthorID, req.AuthorAgent,
		kind, status, req.Body, req.CallerOrgID,
	)
	if err != nil {
		if isForeignKeyViolation(err) {
			return nil, &errs.Error{Code: errs.NotFound, Message: "item or parent comment does not exist"}
		}
		rlog.Error("workitems: append comment failed", "err", err, "item_id", req.ItemID)
		return nil, &errs.Error{Code: errs.Internal, Message: "append comment failed"}
	}
	// Zero inserted rows means the target item does not exist OR belongs to
	// another tenant (org_id != CallerOrgID) OR a non-empty parent_id does
	// not resolve to a comment on the SAME target item (foreign-org or
	// cross-item parent). Surface NOT_FOUND for all so a cross-tenant ItemID
	// or a foreign/cross-item ParentID is indistinguishable from a missing one.
	if tag.RowsAffected() == 0 {
		return nil, &errs.Error{Code: errs.NotFound, Message: "item not found"}
	}

	// Read back so updated_at default is reflected.
	var c Comment
	err = db.QueryRow(ctx,
		`SELECT id, item_id, COALESCE(parent_id, ''), COALESCE(author_id, ''),
		        COALESCE(author_agent, ''), kind, status, body,
		        created_at, updated_at
		   FROM workitems.comments
		  WHERE id = $1`,
		id,
	).Scan(&c.ID, &c.ItemID, &c.ParentID, &c.AuthorID,
		&c.AuthorAgent, &c.Kind, &c.Status, &c.Body,
		&c.CreatedAt, &c.UpdatedAt)
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "comment read-back failed"}
	}
	return &c, nil
}

// SetStateColumns writes one or more of (impl_state, review_state,
// qa_state, pipeline_state) inside a single transaction that enforces
// the five PRD §6.2 state-machine invariants I-1..I-5 (I-3 lives in
// Claim). On violation, returns errs.FailedPrecondition with
// Meta["invariant"] populated per SPEC §6.2 Tool 13.
//
// Side-effects (round-6 §6.3.0 symmetric writer model — tension #3
// narrow rule, SPEC lines 1700-1711 + 1801): after the validating
// UPDATE commits, publishes CascadeRequested{Reason:"state_change",
// TriggeredByItemID:item_id, …} when the write changes one or more
// of (impl_state, review_state, qa_state) — including I-1's
// auto-reset of qa_state. Pure pipe_state mutations (no change to
// the other three) do NOT publish (SPEC §6.3.0 explicit
// non-publishers, tension #3 ruling). The publish drives the
// multi-hop pipeline_stage recompute on the forward 'blocks' closure
// (Regime B; the cascade subscriber is the sole writer of
// pipeline_stage).
//
//encore:api private method=POST path=/workitems.SetStateColumns
func SetStateColumns(ctx context.Context, req *SetStateRequest) (*Item, error) {
	if req == nil || req.ItemID == "" {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing item_id"}
	}
	if err := validateStateEnums(req); err != nil {
		return nil, err
	}

	// Mint the cascade event id BEFORE tx.Begin per the round-6
	// retry-safe dedup pattern (deps.RemoveEdge:468-477). On
	// ulid.New() failure we proceed with the state mutation and skip
	// the publish — the AR-11 UNIQUE constraint on
	// (event_id, triggered_by_item_id) makes the audit row's absence
	// preferable to failing a committed state change. Mirrors Close's
	// best-effort handling at workitems.go:1380-1385.
	eventID, eventIDErr := ulid.New()
	if eventIDErr != nil {
		rlog.Warn("workitems: set_state cascade event id generation failed", "err", eventIDErr, "item_id", req.ItemID)
	}

	tx, err := db.Begin(ctx)
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "db begin failed"}
	}
	defer func() { _ = tx.Rollback() }()

	// Lock the row and read current state columns + scope (org_id,
	// project_id). Scope is projected unconditionally so the
	// predicate-evaluation branch stays uniform; only the publishing
	// branch consumes it. COALESCE(project_id,'') mirrors the Close
	// pattern at workitems.go:1290-1296.
	// Row-level tenant gate (round-16 / bead unblock-tv8.77, §10.1.1): the
	// FOR UPDATE lock is predicated on ($2 = '' OR org_id = $2). A foreign
	// ItemID (org_id != CallerOrgID) matches no row → ErrNoRows → NOT_FOUND
	// BEFORE any invariant check runs, never a cross-tenant state mutation.
	// The empty-CallerOrgID no-op keeps trusted internal callers operating
	// unscoped; the MCP handler always pins CallerOrgID from identity.OrgID.
	var cur stateRow
	err = tx.QueryRow(ctx,
		`SELECT impl_state, review_state, qa_state, pipeline_state, claimed_by_id,
		        org_id, COALESCE(project_id, '')
		   FROM workitems.items
		  WHERE id = $1
		    AND ($2 = '' OR org_id = $2)
		  FOR UPDATE`,
		req.ItemID, req.CallerOrgID,
	).Scan(&cur.Impl, &cur.Review, &cur.QA, &cur.Pipeline, &cur.ClaimedBy,
		&cur.OrgID, &cur.ProjectID)
	if err != nil {
		if errors.Is(err, sqldb.ErrNoRows) {
			return nil, &errs.Error{Code: errs.NotFound, Message: "item not found"}
		}
		return nil, &errs.Error{Code: errs.Internal, Message: "state read failed"}
	}

	newImpl := coalesceState(req.ImplState, cur.Impl)
	newReview := coalesceState(req.ReviewState, cur.Review)
	newQA := coalesceState(req.QAState, cur.QA)
	newPipeline := coalesceState(req.PipelineState, cur.Pipeline)

	// I-1: review_state=needs_rework auto-resets qa_state to pending.
	if req.ReviewState != nil && *req.ReviewState == reviewNeedsRework {
		newQA = qaPending
	}

	// Structural invariant: impl_state=done requires claimed_by_id IS NOT NULL.
	if newImpl == implDone && (cur.ClaimedBy == nil || *cur.ClaimedBy == "") {
		return nil, preconditionError("impl_done_requires_claim", "impl_state=done requires claimed_by_id IS NOT NULL")
	}

	// I-2: qa_state=failed requires review_state=approved.
	if newQA == qaFailed && newReview != reviewApproved {
		return nil, preconditionError("qa_failed_requires_review_approved", "qa_state=failed requires review_state=approved")
	}

	// I-5: impl_state=done → pending is only allowed via the rework path.
	// Check BEFORE I-4 because an impl→pending transition without a
	// matching review_state=needs_rework would also trip I-4 (review
	// is unchanged but now impl != done) — the spec's intent is to
	// flag the rework-path violation specifically, so we surface it
	// before the more general I-4 rule.
	if newImpl == implPending && cur.Impl == implDone {
		reqReviewIsNeedsRework := req.ReviewState != nil && *req.ReviewState == reviewNeedsRework
		reqQAIsFailed := req.QAState != nil && *req.QAState == qaFailed
		currentQAFailedAndUnchanged := cur.QA == qaFailed && req.QAState == nil
		if !reqReviewIsNeedsRework && !reqQAIsFailed && !currentQAFailedAndUnchanged {
			return nil, preconditionError("impl_done_to_pending_requires_rework_path",
				"impl_state=done → pending requires review_state=needs_rework or qa_state=failed")
		}
	}

	// I-4 (FORWARD review gate, SPEC §6.2 Tool 13 line 2241 + SQL
	// pseudocode line 2269): review_state → approved requires
	// impl_state=done. The review_state → needs_rework transition is the
	// REWORK trigger, governed by I-5 above (which legitimately permits
	// the concurrent impl done → pending), so it is EXEMPT from I-4.
	// Keying on needs_rework here would make the one-call
	// set_state(impl_state=pending, review_state=needs_rework) rework
	// unreachable and violate the §11.1.2 exit criterion.
	if newReview == reviewApproved && newImpl != implDone {
		return nil, preconditionError("review_change_requires_impl_done", "review_state change requires impl_state=done")
	}

	// Compute the §6.3.0 tension #3 "materially changed" predicate
	// BEFORE the UPDATE so the publish gate captures the state
	// transition post-I-1-auto-reset (R2 of the bead investigation).
	// Pure pipe_state writes leave newImpl/newReview/newQA equal to
	// cur and evaluate to false (AC #2). Over-publishing is acceptable
	// (subscriber's idempotent UPDATE guard absorbs it); under-publishing
	// would be a correctness bug — hence the simple any-of-three form.
	publishStateChange := (newImpl != cur.Impl) ||
		(newReview != cur.Review) ||
		(newQA != cur.QA)

	// All invariants validated. Apply the update.
	_, err = tx.Exec(ctx,
		`UPDATE workitems.items
		    SET impl_state    = $2,
		        review_state  = $3,
		        qa_state      = $4,
		        pipeline_state = $5,
		        updated_at    = now()
		  WHERE id = $1`,
		req.ItemID, newImpl, newReview, newQA, newPipeline,
	)
	if err != nil {
		rlog.Error("workitems: set_state update failed", "err", err, "item_id", req.ItemID)
		return nil, &errs.Error{Code: errs.Internal, Message: "set_state update failed"}
	}

	if err := tx.Commit(); err != nil {
		rlog.Error("workitems: set_state commit failed", "err", err)
		return nil, &errs.Error{Code: errs.Internal, Message: "set_state commit failed"}
	}

	// Round-6 §6.3.0 tension #3 narrow rule (SPEC §6.2 Tool 13 lines
	// 1700-1711 + §6.3.0 line 1801): publish CascadeRequested with
	// Reason="state_change" ONLY when the write changed at least one
	// of (impl_state, review_state, qa_state) — including I-1's
	// auto-reset of qa_state. Pipe-only writes do NOT publish. Encore
	// Pub/Sub does not carry ctx across the topic boundary; TraceID is
	// copied from tracectx into the payload explicitly (mirrors Close
	// at workitems.go:1377-1397 + Claim at 1492-1526). Best-effort:
	// log.Warn on publish failure, do not return error — the state
	// mutation is already committed.
	if publishStateChange && eventIDErr == nil {
		if _, err := deps.CascadeRequestedTopic.Publish(ctx, &deps.CascadeRequested{
			EventID:           eventID,
			OrgID:             cur.OrgID,
			ProjectID:         cur.ProjectID,
			TriggeredByItemID: req.ItemID,
			Reason:            "state_change",
			TraceID:           tracectx.TraceID(ctx),
			EmittedAt:         time.Now().UTC(),
		}); err != nil {
			rlog.Warn("workitems: set_state cascade publish failed (set_state already committed)",
				"err", err, "item_id", req.ItemID)
		}
	}

	return readItem(ctx, req.ItemID)
}

// Close sets status=Done, closed_at=now(), then publishes
// deps.CascadeRequestedTopic with Reason="close" and TraceID copied
// from tracectx.From(ctx). SPEC §4.4 + §6.3.1 + §10.2.
//
// The AF3 precondition (claimed_by_id IS NOT NULL) is the MCP-layer's
// job per SPEC §6.2 Tool 6. We enforce it defensively here too for a
// clearer error than the downstream CHECK violation.
//
//encore:api private method=POST path=/workitems.Close
func Close(ctx context.Context, req *CloseRequest) (*Item, error) {
	if req == nil || req.ItemID == "" {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing item_id"}
	}

	tx, err := db.Begin(ctx)
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "db begin failed"}
	}
	defer func() { _ = tx.Rollback() }()

	// Read org_id, project_id, claimed_by_id to validate AF3 and to
	// scope the cascade event.
	//
	// Row-level tenant gate (round-16 / bead unblock-tv8.77, §10.1.1): the
	// FOR UPDATE lock is predicated on ($2 = '' OR org_id = $2), checked
	// BEFORE the AF3 claimed_by_id precondition. A foreign ItemID (org_id !=
	// CallerOrgID) matches no row → ErrNoRows → NOT_FOUND, never a
	// cross-tenant close. The empty-CallerOrgID no-op keeps trusted internal
	// callers operating unscoped; the MCP handler always pins CallerOrgID.
	var orgID, projectID string
	var claimedBy *string
	var currentStatus string
	err = tx.QueryRow(ctx,
		`SELECT org_id, COALESCE(project_id, ''), claimed_by_id, status
		   FROM workitems.items
		  WHERE id = $1
		    AND ($2 = '' OR org_id = $2)
		  FOR UPDATE`,
		req.ItemID, req.CallerOrgID,
	).Scan(&orgID, &projectID, &claimedBy, &currentStatus)
	if err != nil {
		if errors.Is(err, sqldb.ErrNoRows) {
			return nil, &errs.Error{Code: errs.NotFound, Message: "item not found"}
		}
		return nil, &errs.Error{Code: errs.Internal, Message: "close read failed"}
	}
	if claimedBy == nil || *claimedBy == "" {
		// AF3 defensive check. SPEC §6.2 Tool 6 line 1334 mandates the §7
		// envelope carry `data.missing = "claimed_by_id"` on this path;
		// preconditionErrorMissing sets BOTH Meta["invariant"] (for
		// rejection_reason on the audit row) AND Meta["missing"] (for the
		// envelope's data.missing) so the MCP layer never has to retrofit
		// the field at the wire boundary.
		return nil, preconditionErrorMissing(
			"claimed_by_id_required", "claimed_by_id",
			"close requires claimed_by_id IS NOT NULL",
		)
	}
	if currentStatus == statusDone {
		// Idempotent close — read back and return.
		return readItem(ctx, req.ItemID)
	}

	if _, err := tx.Exec(ctx,
		`UPDATE workitems.items
		    SET status     = 'Done',
		        closed_at  = COALESCE(closed_at, now()),
		        updated_at = now()
		  WHERE id = $1`,
		req.ItemID,
	); err != nil {
		rlog.Error("workitems: close update failed", "err", err, "item_id", req.ItemID)
		return nil, &errs.Error{Code: errs.Internal, Message: "close update failed"}
	}

	// Append an optional kind=completed comment when Reason is provided.
	if strings.TrimSpace(req.Reason) != "" {
		commentID, err := ulid.New()
		if err == nil {
			// Author is the claimer (claimed_by_id). The MCP layer can
			// override author by calling AppendComment directly; here
			// we record the close reason as the canonical actor.
			_, _ = tx.Exec(ctx,
				`INSERT INTO workitems.comments
				   (id, item_id, author_id, kind, status, body)
				 VALUES ($1, $2, $3, 'completed', 'success', $4)`,
				commentID, req.ItemID, *claimedBy, req.Reason,
			)
		}
	}

	// Regime A (SPEC §6.3.0 lines 1691-1692): recompute is_ready inline
	// for the closed item's DIRECT 'blocks' downstream neighbours, in
	// the SAME transaction as the status='Done' write. The cascade
	// subscriber maintains pipeline_stage multi-hop (Regime B) but the
	// single-hop is_ready flip is the writer's responsibility — Postgres
	// holds the transaction's view consistent with the readiness flip,
	// so downstream readers never observe Done-but-unblocker-not-ready
	// state. The helper is the SOLE exported Regime A write path; the
	// is_ready UPDATE itself lives in encore.app/deps (gated by the
	// no_direct_is_ready_write lint analyzer to that single package).
	flipped, err := deps.RecomputeReadyForBlocksDownstream(ctx, tx, req.ItemID)
	if err != nil {
		rlog.Error("workitems: close inline is_ready recompute failed",
			"err", err, "item_id", req.ItemID)
		return nil, &errs.Error{Code: errs.Internal, Message: "is_ready recompute failed"}
	}

	if err := tx.Commit(); err != nil {
		rlog.Error("workitems: close commit failed", "err", err)
		return nil, &errs.Error{Code: errs.Internal, Message: "close commit failed"}
	}

	// Log the neighbours that became ready as a result of this close.
	// An empty list is normal (no direct blocks downstream, or other
	// blockers remain on every neighbour) — never fail on it.
	if len(flipped) > 0 {
		rlog.Info("workitems: close flipped is_ready on direct blocks downstream",
			"item_id", req.ItemID, "flipped", flipped, "count", len(flipped))
	}

	// Publish cascade event. Encore Pub/Sub does not carry ctx across
	// the topic boundary (cascade.go file header lines 25-33); we copy
	// TraceID from tracectx into the payload explicitly.
	eventID, err := ulid.New()
	if err != nil {
		// Don't fail the close — the cascade is best-effort; the audit
		// row would lack an event id but the item is already closed.
		rlog.Warn("workitems: cascade event id generation failed", "err", err)
	} else {
		if _, err := deps.CascadeRequestedTopic.Publish(ctx, &deps.CascadeRequested{
			EventID:           eventID,
			OrgID:             orgID,
			ProjectID:         projectID,
			TriggeredByItemID: req.ItemID,
			Reason:            "close",
			TraceID:           tracectx.TraceID(ctx),
			EmittedAt:         time.Now().UTC(),
		}); err != nil {
			rlog.Warn("workitems: cascade publish failed (close already committed)", "err", err, "item_id", req.ItemID)
		}
	}

	return readItem(ctx, req.ItemID)
}

// Claim performs the SPEC §6.4 atomic claim transaction. The row is
// locked with SELECT … FOR UPDATE and the precondition
// (status='Ready' AND claimed_by_id IS NULL) is evaluated in Go on the
// locked row, so the three zero-row causes are reported distinctly
// (§7.2, bead unblock-tv8.72):
//   - item absent                      → errs.NotFound (NOT_FOUND)
//   - claimed_by_id IS NOT NULL        → errs.AlreadyExists (ALREADY_CLAIMED)
//     with Meta carrying winner info
//   - status <> 'Ready' (unclaimed)    → errs.FailedPrecondition
//     (PRECONDITION_NOT_MET) with
//     Meta{status:<current>, required:'Ready'}
//
// Enforces invariant I-3 (qa_state=failed → review_state and qa_state
// both reset to 'pending' in the same transaction).
//
//encore:api private method=POST path=/workitems.Claim
func Claim(ctx context.Context, req *ClaimRequest) (*Item, error) {
	if req == nil || req.ItemID == "" {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing item_id"}
	}
	if req.ClaimerUserID == "" {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing claimer_user_id"}
	}
	if req.ClaimerAgent != "" {
		if _, ok := agentKindAllowed[req.ClaimerAgent]; !ok {
			return nil, &errs.Error{Code: errs.InvalidArgument, Message: fmt.Sprintf("invalid claimer_agent %q", req.ClaimerAgent), Meta: errs.Metadata{"field": "claimer_agent"}}
		}
	}

	tx, err := db.Begin(ctx)
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "db begin failed"}
	}
	defer func() { _ = tx.Rollback() }()

	// SELECT FOR UPDATE on the BARE row (no status/claimed predicate),
	// mirroring Promote's read-then-branch shape (§6.4 + §7.2, bead
	// unblock-tv8.72). The §6.4 SQL filters the lock by
	// (status='Ready' AND claimed_by_id IS NULL); doing the SAME filter
	// in SQL collapses three DISTINCT zero-row causes into one ErrNoRows
	// arm and mis-reports a never-Ready item as ALREADY_CLAIMED. Instead
	// we lock the row unconditionally and discriminate the three cases in
	// Go AFTER the lock (TOCTOU-safe — the FOR UPDATE serialises claim
	// against a concurrent promote/add_dependency/claim):
	//   - ErrNoRows                          → NOT_FOUND
	//   - claimed_by_id IS NOT NULL          → ALREADY_CLAIMED (winner meta)
	//   - status <> 'Ready' (and unclaimed)  → PRECONDITION_NOT_MET
	//                                          {status:<current>, required:'Ready'}
	// Only (status='Ready' AND claimed_by_id IS NULL) proceeds to the
	// I-3/UPDATE path below.
	//
	// We also project org_id and project_id here so the I-3-path cascade
	// publish (post-commit, below) has the scope fields it needs without a
	// second read.
	// Row-level tenant gate (round-16 / bead unblock-tv8.77, §10.1.1): the
	// FOR UPDATE lock is predicated on ($2 = '' OR org_id = $2). A foreign
	// ItemID (org_id != CallerOrgID) matches no row → ErrNoRows → NOT_FOUND
	// (NOT ALREADY_CLAIMED), never a cross-tenant claim. The gate runs
	// before the §6.4 / §7.2 loser/precondition discrimination so a foreign
	// id can never reach the ALREADY_CLAIMED or PRECONDITION_NOT_MET arms.
	// The empty-CallerOrgID no-op keeps trusted internal callers operating
	// unscoped; the MCP handler always pins CallerOrgID from identity.OrgID.
	var lockedID, orgID, projectID, currentStatus, qaState string
	var claimedByID *string
	err = tx.QueryRow(ctx,
		`SELECT id, org_id, COALESCE(project_id, ''), status, claimed_by_id, qa_state
		   FROM workitems.items
		  WHERE id = $1
		    AND ($2 = '' OR org_id = $2)
		  FOR UPDATE`,
		req.ItemID, req.CallerOrgID,
	).Scan(&lockedID, &orgID, &projectID, &currentStatus, &claimedByID, &qaState)
	if err != nil {
		if errors.Is(err, sqldb.ErrNoRows) {
			return nil, &errs.Error{Code: errs.NotFound, Message: "item not found"}
		}
		return nil, &errs.Error{Code: errs.Internal, Message: "claim lock failed"}
	}

	// Discriminate on the LOCKED row (§6.4 loser path + §7.2, bead
	// unblock-tv8.72). This MUST run BEFORE the I-3 reset / UPDATE block
	// below — a not-Ready or already-claimed item must never reach the
	// claim UPDATE.
	if claimedByID != nil {
		// Genuine concurrent-loser path — fetch winner info and return
		// ALREADY_CLAIMED unchanged (§6.4).
		//
		// Concurrency hazard: we MUST release the SELECT FOR UPDATE
		// transaction (and its underlying pgxpool conn) BEFORE calling
		// alreadyClaimedError, which opens a fresh pool conn via
		// db.QueryRow. The deferred tx.Rollback above runs only at
		// function-return, which is too late: at function entry under
		// high concurrent claim load (N > pgxpool MaxConns), every
		// goroutine already holds one conn for its own SELECT FOR
		// UPDATE tx and the pool has zero free conns. Each loser would
		// then queue forever for the second conn alreadyClaimedError
		// needs, deadlocking N-way on the pool (no losers can release
		// their first conn until they've returned from Claim, but they
		// can't return until alreadyClaimedError completes, which can't
		// run without a second conn). Explicit early-rollback here frees
		// the conn back to the pool BEFORE the second acquisition is
		// attempted, so the loser path scales linearly with N under any
		// pool size. The deferred rollback is harmless after an explicit
		// one — pgx treats double-rollback as a no-op (ErrTxClosed).
		_ = tx.Rollback()
		return nil, alreadyClaimedError(ctx, req.ItemID)
	}
	if currentStatus != statusReady {
		// Item exists and is unclaimed but is not in Ready (e.g. a fresh
		// Backlog item that was never promoted). §7.2 status-precondition
		// extension: emit PRECONDITION_NOT_MET carrying the item's CURRENT
		// status + the required status. NO "missing" — claim's block is a
		// wrong-status, not an unmet structural readiness gate ("missing"
		// is promote's Backlog-but-is_ready=false disambiguator only).
		// errmap surfaces Meta[status]/[required] inside data.details.
		// This arm opens no second pool conn, so the deferred rollback
		// suffices (no early-rollback needed).
		return nil, &errs.Error{
			Code:    errs.FailedPrecondition,
			Message: "claim requires status='Ready'",
			Meta: errs.Metadata{
				"invariant": "claim_requires_ready_item",
				"status":    currentStatus,
				"required":  statusReady,
			},
		}
	}

	// I-3: when claimed item has qa_state='failed', reset
	// review_state and qa_state to pending in the same transaction.
	// Scope is exactly review_state + qa_state per SPEC §6.2 line 1505 —
	// impl_state is deliberately NOT touched here (a re-claimed item
	// still carries its prior impl_state and the worker drives any
	// subsequent impl_state mutation through SetStateColumns, which
	// enforces I-4 and I-5 against the rework path).
	resetRework := qaState == qaFailed

	if resetRework {
		_, err = tx.Exec(ctx,
			`UPDATE workitems.items
			    SET claimed_by_id    = $2,
			        claimed_by_agent = NULLIF($3, ''),
			        claimed_at       = now(),
			        status           = 'InProgress',
			        review_state     = 'pending',
			        qa_state         = 'pending',
			        updated_at       = now()
			  WHERE id = $1`,
			req.ItemID, req.ClaimerUserID, req.ClaimerAgent,
		)
	} else {
		_, err = tx.Exec(ctx,
			`UPDATE workitems.items
			    SET claimed_by_id    = $2,
			        claimed_by_agent = NULLIF($3, ''),
			        claimed_at       = now(),
			        status           = 'InProgress',
			        updated_at       = now()
			  WHERE id = $1`,
			req.ItemID, req.ClaimerUserID, req.ClaimerAgent,
		)
	}
	if err != nil {
		if isForeignKeyViolation(err) {
			return nil, &errs.Error{Code: errs.NotFound, Message: "claimer user does not exist"}
		}
		rlog.Error("workitems: claim update failed", "err", err, "item_id", req.ItemID)
		return nil, &errs.Error{Code: errs.Internal, Message: "claim update failed"}
	}

	if err := tx.Commit(); err != nil {
		rlog.Error("workitems: claim commit failed", "err", err)
		return nil, &errs.Error{Code: errs.Internal, Message: "claim commit failed"}
	}

	// Round-6 §6.3.0 tension #2 narrow rule (SPEC §6.4 lines 1898-1914):
	// Claim publishes CascadeRequested{Reason:"state_change", ...} ONLY
	// on the I-3 reset path — that is, when the locked row carried
	// qa_state='failed' at the start of the transaction and the
	// transaction therefore wrote (review_state, qa_state) =
	// ('pending', 'pending') atomically with the claim. Normal
	// Ready→InProgress claim does NOT publish: the claimed item was
	// non-Done before the claim and remains non-Done, so §5.7.1
	// downstream pipeline_stage is unaffected. Encore Pub/Sub does
	// not carry ctx across the topic boundary; we copy TraceID from
	// tracectx into the payload explicitly (mirrors Close above).
	if resetRework {
		eventID, err := ulid.New()
		if err != nil {
			// Best-effort publish — the claim is already committed.
			// A missing audit row is preferred to failing a committed
			// claim. Matches Close's error handling above.
			rlog.Warn("workitems: claim cascade event id generation failed",
				"err", err, "item_id", req.ItemID)
		} else {
			if _, err := deps.CascadeRequestedTopic.Publish(ctx, &deps.CascadeRequested{
				EventID:           eventID,
				OrgID:             orgID,
				ProjectID:         projectID,
				TriggeredByItemID: req.ItemID,
				Reason:            "state_change",
				TraceID:           tracectx.TraceID(ctx),
				EmittedAt:         time.Now().UTC(),
			}); err != nil {
				rlog.Warn("workitems: claim cascade publish failed (claim already committed)",
					"err", err, "item_id", req.ItemID)
			}
		}
	}

	return readItem(ctx, req.ItemID)
}

// PromoteRequest is the input to Promote. SPEC §6.2 Tool 15.
//
// CallerOrgID is NOT a wire argument — the MCP handler pins it from the
// Bearer-resolved org-scoped Identity (identity.OrgID) so Promote
// self-gates the SELECT … FOR UPDATE row lock on org_id = CallerOrgID. A
// foreign ItemID yields NOT_FOUND, never a cross-tenant promotion. An empty
// CallerOrgID is the §10.1.1 no-op for trusted internal callers; the MCP
// handler always pins it (round-16 / bead unblock-tv8.77). Promote has no
// §4.4 block — its gate is contract-defined in §6.2 Tool 15 prose only.
type PromoteRequest struct {
	ItemID      string `json:"item_id"`
	CallerOrgID string `json:"caller_org_id"`
}

// Promote transitions a Backlog item to Ready (SPEC §6.2 Tool 15 / §6.6
// status transition map / round-16, bead unblock-tv8.71). This is the
// canonical Ready writer that round-12 DRIFT-2 observed was missing —
// before promote nothing moved an item into Ready via RPC, so the ready
// queue and claim (which require status='Ready') were inert for any item
// created through the create tool.
//
// Precondition: status='Backlog' AND is_ready=true. The item must already
// be in Backlog (only a Backlog item is promotable; an already-Ready,
// InProgress, Blocked, or Done item is rejected) AND have no unresolved
// incoming 'blocks' edges (is_ready=true — every blocker is Done).
// is_ready is a single-writer materialised column (§6.3.0 Regime A);
// promote READS it and does NOT recompute it.
//
// Rejections (§7 error envelope):
//   - Not in Backlog OR not ready → PRECONDITION_NOT_MET with the §7.2
//     {status, required} extension: data.status carries the item's CURRENT
//     status and data.required="Ready". When the block is specifically
//     "still has open blockers" (status='Backlog' but is_ready=false) the
//     handler also sets data.missing="is_ready" so the agent can
//     disambiguate "wrong status" from "blocked".
//   - Item not found / not visible → NOT_FOUND.
//
// Side-effects: NONE on the cascade subsystem. Moving an item
// Backlog→Ready does not change any OTHER item's is_ready (a dependent's
// is_ready flips only when ITS blocker becomes Done, not merely Ready) and
// does not change §5.7.1 pipeline_stage derivation inputs. promote
// therefore publishes no CascadeRequested and is NOT a Regime A is_ready
// writer (it writes only status). It writes NO state-dimension columns
// (impl_state etc.) and does NOT touch is_ready or claimed_by_*.
//
//encore:api private method=POST path=/workitems.Promote
func Promote(ctx context.Context, req *PromoteRequest) (*Item, error) {
	if req == nil || req.ItemID == "" {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing item_id"}
	}

	tx, err := db.Begin(ctx)
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "db begin failed"}
	}
	defer func() { _ = tx.Rollback() }()

	// Lock the row and read its current status + is_ready. We re-check the
	// precondition against the LOCKED row (not a prior read) so a
	// concurrent add_dependency that flips is_ready false cannot race the
	// promotion (TOCTOU): the FOR UPDATE serialises promote against the
	// §6.5 inline is_ready recompute.
	// Row-level tenant gate (round-16 / bead unblock-tv8.77, §10.1.1): the
	// FOR UPDATE lock is predicated on ($2 = '' OR org_id = $2). A foreign
	// ItemID (org_id != CallerOrgID) matches no row → ErrNoRows → NOT_FOUND,
	// checked before the Backlog/is_ready precondition, never a cross-tenant
	// promotion. The empty-CallerOrgID no-op keeps trusted internal callers
	// operating unscoped; the MCP handler always pins CallerOrgID.
	var currentStatus string
	var isReady bool
	err = tx.QueryRow(ctx,
		`SELECT status, is_ready
		   FROM workitems.items
		  WHERE id = $1
		    AND ($2 = '' OR org_id = $2)
		  FOR UPDATE`,
		req.ItemID, req.CallerOrgID,
	).Scan(&currentStatus, &isReady)
	if err != nil {
		if errors.Is(err, sqldb.ErrNoRows) {
			return nil, &errs.Error{Code: errs.NotFound, Message: "item not found"}
		}
		return nil, &errs.Error{Code: errs.Internal, Message: "promote lock failed"}
	}

	// Precondition: status='Backlog' AND is_ready=true. On failure emit
	// PRECONDITION_NOT_MET with the §7.2 {status, required} extension
	// (errmap surfaces these inside data.details). data.missing="is_ready"
	// is added only when the item IS in Backlog but still blocked, so the
	// agent distinguishes "wrong status" from "blocked".
	if currentStatus != statusBacklog || !isReady {
		meta := errs.Metadata{
			"invariant": "promote_requires_ready_backlog",
			"status":    currentStatus,
			"required":  statusReady,
		}
		if currentStatus == statusBacklog && !isReady {
			meta["missing"] = "is_ready"
		}
		return nil, &errs.Error{
			Code:    errs.FailedPrecondition,
			Message: "promote requires status='Backlog' AND is_ready=true",
			Meta:    meta,
		}
	}

	if _, err := tx.Exec(ctx,
		`UPDATE workitems.items
		    SET status     = 'Ready',
		        updated_at = now()
		  WHERE id = $1`,
		req.ItemID,
	); err != nil {
		rlog.Error("workitems: promote update failed", "err", err, "item_id", req.ItemID)
		return nil, &errs.Error{Code: errs.Internal, Message: "promote update failed"}
	}

	if err := tx.Commit(); err != nil {
		rlog.Error("workitems: promote commit failed", "err", err)
		return nil, &errs.Error{Code: errs.Internal, Message: "promote commit failed"}
	}

	return readItem(ctx, req.ItemID)
}

// List returns a paginated slice of items filtered by the request's
// scalar/array filters. SPEC §4.4. Read path goes through rbac.For for
// the org_id scope predicate.
//
//encore:api private method=POST path=/workitems.List
func List(ctx context.Context, req *ListRequest) (*ListResponse, error) {
	if req == nil {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing request body"}
	}
	identity, ok := callerIdentity(ctx)
	if !ok {
		return nil, &errs.Error{Code: errs.Unauthenticated, Message: "no caller identity"}
	}

	limit := req.Limit
	if limit <= 0 {
		limit = listDefaultLimit
	}
	if limit > listMaxLimit {
		limit = listMaxLimit
	}

	// Validate enum filters early so the SQL never sees bogus values.
	for _, s := range req.Status {
		if _, ok := statusAllowed[s]; !ok {
			return nil, &errs.Error{Code: errs.InvalidArgument, Message: fmt.Sprintf("invalid status %q", s), Meta: errs.Metadata{"field": "status"}}
		}
	}
	for _, s := range req.PipelineStage {
		if _, ok := pipelineStageAllowed[s]; !ok {
			return nil, &errs.Error{Code: errs.InvalidArgument, Message: fmt.Sprintf("invalid pipeline_stage %q", s), Meta: errs.Metadata{"field": "pipeline_stage"}}
		}
	}

	// Pagination: opaque cursor is the lexicographic anchor on
	// (created_at, id). We pass cursor as a string "ULID|RFC3339"; an
	// empty Cursor means first page. For P01 we use a simpler
	// id-only cursor (ULIDs are time-sortable, so id alone suffices).
	q := rbac.For[itemRow](identity, "workitems.items")
	if req.ProjectID != "" {
		q = q.Where("project_id = $1", req.ProjectID)
	}
	if req.MilestoneID != "" {
		q = q.Where("milestone_id = $1", req.MilestoneID)
	}
	if req.ClaimedBy != "" {
		q = q.Where("claimed_by_id = $1", req.ClaimedBy)
	}
	if len(req.Status) > 0 {
		q = q.Where("status = ANY($1)", req.Status)
	}
	if len(req.PipelineStage) > 0 {
		q = q.Where("pipeline_stage = ANY($1)", req.PipelineStage)
	}
	if req.Cursor != "" {
		q = q.Where("id > $1", req.Cursor)
	}
	// Order + limit live in a fixed-string clause — they are part of
	// the assembled SQL but carry no runtime values.
	q = q.Where("1 = 1 ORDER BY id ASC LIMIT $1", limit+1)

	rows, err := q.Run(ctx)
	if err != nil {
		rlog.Error("workitems: list failed", "err", err)
		return nil, &errs.Error{Code: errs.Internal, Message: "list failed"}
	}

	// Labels filter is applied post-fetch because labels live in a
	// junction table. To avoid an N+1 round-trip (one SELECT per row
	// for the limit+1 window), we batch-load every item's labels for
	// the entire window in a single query keyed by item_id = ANY($1),
	// then filter in Go.
	itemLabels := map[string]map[string]struct{}{}
	if len(req.Labels) > 0 && len(rows) > 0 {
		ids := make([]string, 0, len(rows))
		for _, r := range rows {
			ids = append(ids, r.ID)
		}
		lrows, err := db.Query(ctx,
			`SELECT item_id, label_id
			   FROM workitems.item_labels
			  WHERE item_id = ANY($1)`,
			ids,
		)
		if err != nil {
			rlog.Error("workitems: list label batch fetch failed", "err", err)
			return nil, &errs.Error{Code: errs.Internal, Message: "list label check failed"}
		}
		for lrows.Next() {
			var itemID, labelID string
			if err := lrows.Scan(&itemID, &labelID); err != nil {
				lrows.Close()
				return nil, &errs.Error{Code: errs.Internal, Message: "list label scan failed"}
			}
			set, ok := itemLabels[itemID]
			if !ok {
				set = make(map[string]struct{})
				itemLabels[itemID] = set
			}
			set[labelID] = struct{}{}
		}
		lrows.Close()
	}
	hasAllLabels := func(itemID string, want []string) bool {
		if len(want) == 0 {
			return true
		}
		got := itemLabels[itemID]
		for _, w := range want {
			if _, ok := got[w]; !ok {
				return false
			}
		}
		return true
	}

	var items []Item
	var nextCursor string
	for i, r := range rows {
		if i >= limit {
			// Sentinel row (rows[limit]) — we fetched limit+1 just to
			// detect "more pages exist". The cursor anchor is the LAST
			// row of the CURRENT page (rows[limit-1]); the strict-
			// greater-than predicate (`id > $1`) on the next request
			// resumes at the sentinel without duplicating the anchor
			// AND without skipping the sentinel. This mirrors Ready's
			// cursor model (workitems.Ready :: nextCursorID =
			// rows[limit-1].ID) — both tools share the §6.2.0
			// "zero duplicates, zero skips" invariant. Setting
			// nextCursor to the sentinel itself (rows[limit].ID) would
			// drop one row per page under `id > $1`.
			nextCursor = rows[limit-1].ID
			break
		}
		if len(req.Labels) > 0 && !hasAllLabels(r.ID, req.Labels) {
			continue
		}
		item, err := itemFromRow(ctx, r)
		if err != nil {
			return nil, err
		}
		items = append(items, *item)
	}

	return &ListResponse{Items: items, NextCursor: nextCursor}, nil
}

// readyDefaultLimit / readyMaxLimit cap the §6.2 Tool 2 page size.
// Spec §6.2 Tool 2 line 1183: limit 1..200; default 10. (Round-7
// rework: prior implementation truncated downstream at 50 — a contract
// violation per Linus REVIEW S2; corrected here to honour the full
// 1..200 range so the spec-mandated ceiling is reachable.)
const (
	readyDefaultLimit = 10
	readyMaxLimit     = 200
)

// Ready returns the ready set for the §6.2 Tool 2 MCP `ready` tool.
// Priority comparison is lexicographic on the literal "P0".."P4"
// strings — P0 is highest, P4 lowest — so priority_min = "P3"
// means "include P0..P3" (priority <= 'P3' on the SQL side).
//
// Authorisation: scope is pinned to identity.OrgID via rbac.For per
// the §10.1 canonical pattern. req.OrgID is ignored — the MCP layer
// already trusts identity; carrying an org_id field on the request
// would only invite confused-deputy seams.
//
// Filters: project_id (optional — empty = org-wide scope),
// priority_min (optional — "P0".."P4" lexicographic). Ordering is
// deterministic on (priority asc, created_at asc, id asc) and covered
// by items_ready_partial_idx (migration 0040 + 0100). After migration
// 0100 the index columns are (org_id, project_id, priority,
// created_at, id) so the ORDER BY + keyset pagination serve entirely
// from a pure index scan.
//
// total_ready counts every ready item in the same scope so the caller
// has a denominator even when paginating; the cursor anchor on the
// response carries the keyset for the next page (or zero when this is
// the last page).
//
//encore:api private method=POST path=/workitems.Ready
func Ready(ctx context.Context, req *ReadyRequest) (*ReadyResponse, error) {
	if req == nil {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing request body"}
	}
	identity, ok := callerIdentity(ctx)
	if !ok {
		return nil, &errs.Error{Code: errs.Unauthenticated, Message: "no caller identity"}
	}

	limit := req.Limit
	if limit <= 0 {
		limit = readyDefaultLimit
	}
	if limit > readyMaxLimit {
		limit = readyMaxLimit
	}

	priorityMin := req.PriorityMin
	if priorityMin != "" {
		if _, ok := priorityAllowed[priorityMin]; !ok {
			return nil, &errs.Error{
				Code:    errs.InvalidArgument,
				Message: fmt.Sprintf("invalid priority_min %q (allowed: P0..P4)", priorityMin),
				Meta:    errs.Metadata{"field": "priority_min"},
			}
		}
	}

	// Hot-path read against items_ready_partial_idx, gated by rbac.For
	// so the org_id predicate is the rbac builder's canonical scope
	// clause (style-only refactor; rework S1). The partial-index
	// predicate (is_ready = true AND status = 'Ready' AND closed_at
	// IS NULL) MUST match the index definition verbatim — drift here
	// would force the planner off the index. Empty project_id skips
	// the project filter for org-wide scope per the §6.2 Tool 1/2
	// "primary project" P01 contract.
	//
	// The ORDER BY + LIMIT clause is smuggled through Where("1 = 1
	// ORDER BY ...") because rbac.For's SELECT * shape pins the
	// builder to filter predicates only; this mirrors the same trick
	// used by workitems.List for its own ORDER BY.
	q := rbac.For[itemRow](identity, "workitems.items").
		Where("is_ready = true").
		Where("status = 'Ready'").
		Where("closed_at IS NULL")
	if req.ProjectID != "" {
		q = q.Where("project_id = $1", req.ProjectID)
	}
	if priorityMin != "" {
		q = q.Where("priority <= $1", priorityMin)
	}
	// §6.2.0 keyset pagination. The anchor tuple is (priority,
	// created_at, id) — all three fields populated together by the
	// MCP cursor decoder, all three zero on a first-page request.
	// We compare against the lexicographic tuple via the canonical
	// `(a, b, c) > (x, y, z)` row-constructor form so the partial
	// index serves the predicate as an index range scan.
	if req.CursorID != "" {
		q = q.Where(
			"(priority, created_at, id) > ($1, $2, $3)",
			req.CursorPriority, req.CursorCreatedAt, req.CursorID,
		)
	}
	// Keyset "fetch limit+1 to peek next page" pattern.
	//
	// We request `limit+1` rows from the partial index so we can detect
	// whether a next page exists WITHOUT issuing a second COUNT query.
	// The extra (limit+1) row is treated as a sentinel: its mere
	// presence proves `len(rows) > limit`, but the row itself is
	// DISCARDED — it never enters `out`, it never reaches the caller,
	// and the loop body below breaks BEFORE the materialisation step
	// (`itemFromRow`) ever sees it. Cursor anchor = `rows[limit-1]`
	// (the last row of the current page); the next request emits rows
	// STRICTLY AFTER that anchor via the `(priority, created_at, id) >
	// (cursor)` row-constructor predicate, so the sentinel re-appears
	// as the FIRST row of the next page (correctly, no skip).
	q = q.Where("1 = 1 ORDER BY priority ASC, created_at ASC, id ASC LIMIT $1", limit+1)
	rows, err := q.Run(ctx)
	if err != nil {
		rlog.Error("workitems: ready query failed", "err", err, "org_id", identity.OrgID)
		return nil, &errs.Error{Code: errs.Internal, Message: "ready query failed"}
	}

	out := make([]Item, 0, limit)
	var nextCursorPriority, nextCursorID string
	var nextCursorCreatedAt time.Time
	for i, r := range rows {
		if i >= limit {
			// Sentinel row (rows[limit], i.e. the (limit+1)th). We never
			// materialise it. The cursor anchor is rows[limit-1] — the
			// last row of the CURRENT page — so the next request resumes
			// at the sentinel position via the strict-greater-than
			// predicate. break immediately so itemFromRow is never
			// called on the sentinel.
			last := rows[limit-1]
			nextCursorPriority = last.Priority
			nextCursorCreatedAt = last.CreatedAt
			nextCursorID = last.ID
			break
		}
		item, err := itemFromRow(ctx, r)
		if err != nil {
			return nil, err
		}
		out = append(out, *item)
	}

	// Second query for the total count across the same predicate.
	// The partial index serves this as an index-only scan with no
	// rows returned. We intentionally do NOT inline this as a window
	// function on the first query — that would force the planner off
	// the partial index for the LIMIT path. Identity is the scope
	// gate (org_id = identity.OrgID), matching the rbac.For query
	// above; cursor and limit are NOT applied here (total_ready is
	// the unpaginated count).
	var totalReady int
	if err := db.QueryRow(ctx,
		`SELECT COUNT(*)
		   FROM workitems.items
		  WHERE org_id = $1
		    AND is_ready = true
		    AND status = 'Ready'
		    AND closed_at IS NULL
		    AND ($2 = '' OR project_id = $2)
		    AND ($3 = '' OR priority <= $3)`,
		identity.OrgID, req.ProjectID, priorityMin,
	).Scan(&totalReady); err != nil {
		rlog.Error("workitems: ready count failed", "err", err, "org_id", identity.OrgID)
		return nil, &errs.Error{Code: errs.Internal, Message: "ready count failed"}
	}

	return &ReadyResponse{
		Items:               out,
		TotalReady:          totalReady,
		NextCursorPriority:  nextCursorPriority,
		NextCursorCreatedAt: nextCursorCreatedAt,
		NextCursorID:        nextCursorID,
	}, nil
}

// Search performs multi-table FTS (UNION ALL over items_fts_idx and
// comments_fts_idx) per SPEC §4.4 + AF1. Query uses websearch_to_tsquery.
//
// Pagination: keyset over `(rank desc, item_id asc, comment_id asc)`
// per SPEC §6.2.0 + §6.2 Tool 9. Over-fetch is LIMIT+1; the sentinel
// row supplies the next-cursor anchor without a second COUNT query —
// mirrors the Ready RPC pattern earlier in this file. `comment_id` is
// the empty string for source="item" rows, which keeps the 3-tuple
// total per SPEC §6.2 Tool 9 line 1451.
//
//encore:api private method=POST path=/workitems.Search
func Search(ctx context.Context, req *SearchRequest) (*SearchResponse, error) {
	if req == nil {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing request body"}
	}
	identity, ok := callerIdentity(ctx)
	if !ok {
		return nil, &errs.Error{Code: errs.Unauthenticated, Message: "no caller identity"}
	}
	if strings.TrimSpace(req.Query) == "" {
		return &SearchResponse{}, nil
	}

	limit := req.Limit
	if limit <= 0 {
		limit = searchDefaultLimit
	}
	if limit > searchMaxLimit {
		return nil, &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "limit out of range (1..100)",
			Meta:    errs.Metadata{"field": "limit"},
		}
	}

	// Project filter is enforced inside the SQL when set; org_id is
	// always the scope gate. We use direct SQL here (rather than
	// rbac.For) because the query shape is a UNION ALL across two
	// tables — the rbac builder is single-table-only.
	//
	// Param plan:
	//   $1 = identity.OrgID
	//   $2 = req.Query (websearch_to_tsquery input)
	//   $3 = limit + 1 (over-fetch sentinel)
	//   $4..$6 = cursor anchor (rank, item_id, comment_id)
	//   $7    = req.ProjectID (only when projectFilter is non-empty)
	//
	// Cursor anchor params are ALWAYS bound (zero values on first
	// page); the predicate evaluates to TRUE when the anchor is zero
	// because no FTS row has rank > any-real-float that is greater
	// than the all-zero anchor — see the boolean guard below ($4=0 AND
	// $5='' AND $6='' short-circuits the keyset predicate).
	args := []any{
		identity.OrgID,
		req.Query,
		limit + 1,
		req.CursorRank,
		req.CursorItemID,
		req.CursorCommentID,
	}
	projectFilter := ""
	if req.ProjectID != "" {
		projectFilter = ` AND project_id = $7`
		args = append(args, req.ProjectID)
	}

	// Canonical sort tuple: (rank desc, item_id asc, comment_id asc).
	// Keyset predicate in row-constructor form, inverted on rank
	// (descending) and ascending on (item_id, comment_id). The
	// first-page short-circuit lives in the `OR (... = '' ...)`
	// branch so the planner can fold the predicate to TRUE when no
	// cursor was supplied.
	//
	// SPEC §10.1: no user-controlled string concatenation. The only
	// dynamic SQL fragment is the static `projectFilter` ON/OFF; both
	// `req.ProjectID` and the cursor anchors flow through parameter
	// placeholders.
	const sqlTpl = `WITH ranked AS (
	    SELECT id            AS item_id,
	           'item'        AS source,
	           ''            AS comment_id,
	           ts_rank_cd(fts, websearch_to_tsquery('english', $2))::float8 AS rank,
	           ts_headline('english', body, websearch_to_tsquery('english', $2),
	                       'MaxFragments=1,MaxWords=20,MinWords=5')  AS snippet
	      FROM workitems.items
	     WHERE org_id = $1{{PROJECT_ITEMS}}
	       AND fts @@ websearch_to_tsquery('english', $2)
	    UNION ALL
	    SELECT i.id            AS item_id,
	           'comment'       AS source,
	           c.id            AS comment_id,
	           ts_rank_cd(c.fts, websearch_to_tsquery('english', $2))::float8 AS rank,
	           ts_headline('english', c.body, websearch_to_tsquery('english', $2),
	                       'MaxFragments=1,MaxWords=20,MinWords=5') AS snippet
	      FROM workitems.comments c
	      JOIN workitems.items i ON i.id = c.item_id
	     WHERE i.org_id = $1{{PROJECT_COMMENTS}}
	       AND c.fts @@ websearch_to_tsquery('english', $2)
	)
	SELECT item_id, source, comment_id, rank, snippet
	  FROM ranked
	 WHERE ($5 = '' AND $6 = '')
	    OR rank < $4
	    OR (rank = $4 AND item_id > $5)
	    OR (rank = $4 AND item_id = $5 AND comment_id > $6)
	 ORDER BY rank DESC, item_id ASC, comment_id ASC
	 LIMIT $3`

	projectItems := ""
	projectComments := ""
	if projectFilter != "" {
		projectItems = projectFilter
		projectComments = strings.ReplaceAll(projectFilter, "project_id", "i.project_id")
	}
	sql := strings.ReplaceAll(sqlTpl, "{{PROJECT_ITEMS}}", projectItems)
	sql = strings.ReplaceAll(sql, "{{PROJECT_COMMENTS}}", projectComments)

	rows, err := db.Query(ctx, sql, args...)
	if err != nil {
		rlog.Error("workitems: search failed", "err", err)
		return nil, &errs.Error{Code: errs.Internal, Message: "search failed"}
	}
	defer rows.Close()

	// Materialise up to `limit` hits plus the sentinel (limit+1). The
	// sentinel — if present — provides the next-cursor anchor; we
	// DISCARD it from the response. Anchor is rows[limit-1] (the last
	// row of the CURRENT page), matching the Ready RPC's pattern at the
	// top of this file. A future request emits rows STRICTLY AFTER that
	// anchor via the keyset predicate above.
	all := make([]SearchHit, 0, limit+1)
	for rows.Next() {
		var h SearchHit
		if err := rows.Scan(&h.ItemID, &h.Source, &h.CommentID, &h.Rank, &h.Snippet); err != nil {
			return nil, &errs.Error{Code: errs.Internal, Message: "search scan failed"}
		}
		// Trim snippet to 200 chars (SPEC §4.4 cap). Rune-aware: a
		// naive byte-slice (Snippet[:200]) can sever a multi-byte
		// UTF-8 sequence — or a ts_headline <b>…</b> markup tag —
		// mid-codepoint, yielding invalid UTF-8 on the wire. Count
		// runes and slice on a rune boundary instead.
		if utf8.RuneCountInString(h.Snippet) > 200 {
			runes := []rune(h.Snippet)
			h.Snippet = string(runes[:200])
		}
		all = append(all, h)
	}
	if err := rows.Err(); err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "search iter failed"}
	}

	resp := &SearchResponse{}
	if len(all) > limit {
		// Sentinel detected — page is full and there are more rows.
		last := all[limit-1]
		resp.NextCursorRank = last.Rank
		resp.NextCursorItemID = last.ItemID
		resp.NextCursorCommentID = last.CommentID
		resp.Hits = all[:limit]
	} else {
		resp.Hits = all
	}
	return resp, nil
}

// -----------------------------------------------------------------------------
// Milestone RPC bodies (round-2 D1; SPEC §4.4.1).
// -----------------------------------------------------------------------------

// CreateMilestone inserts a new workitems.milestones row enforcing
// M-INV-1..M-INV-3 / M-INV-5 / M-INV-6 inside the same transaction.
// M-INV-7 is enforced lazily on AssignItem.
//
//encore:api private method=POST path=/workitems.CreateMilestone
func CreateMilestone(ctx context.Context, req *CreateMilestoneRequest) (*Milestone, error) {
	if req == nil {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing request body"}
	}
	// M-INV (scope XOR): exactly one of OrgID or ProjectID must be set.
	if (req.OrgID == "" && req.ProjectID == "") || (req.OrgID != "" && req.ProjectID != "") {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "milestone scope must be exactly one of org_id or project_id"}
	}
	name := strings.TrimSpace(req.Name)
	if l := len(name); l < 1 || l > 200 {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "milestone name must be 1..200 chars", Meta: errs.Metadata{"field": "name"}}
	}
	startDate, err := time.Parse("2006-01-02", req.StartDate)
	if err != nil {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "start_date must be YYYY-MM-DD", Meta: errs.Metadata{"field": "start_date"}}
	}
	endDate, err := time.Parse("2006-01-02", req.EndDate)
	if err != nil {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "end_date must be YYYY-MM-DD", Meta: errs.Metadata{"field": "end_date"}}
	}
	if endDate.Before(startDate) {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "end_date must be >= start_date"}
	}

	id, err := ulid.New()
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "milestone id generation failed"}
	}

	tx, err := db.Begin(ctx)
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "db begin failed"}
	}
	defer func() { _ = tx.Rollback() }()

	// Parent validation: M-INV-3 (date range), M-INV-5 (scope match),
	// M-INV-6 (max depth). Lock parent FOR UPDATE so a concurrent
	// reparent / update cannot invalidate the check between read and
	// our insert.
	if req.ParentMilestoneID != "" {
		// Row-level tenant gate (round-16 / bead unblock-tv8.77, §10.1.1):
		// the parent-read seam is predicated on the milestone tenant
		// predicate — ($5 = '' OR org_id = $5 OR project_id IN the caller's
		// org's projects). A foreign parent ULID matches no row → ErrNoRows
		// → NOT_FOUND, never a cross-tenant read leak of a foreign parent's
		// scope/dates. The empty-CallerOrgID no-op keeps trusted internal
		// callers operating unscoped; the MCP handler always pins CallerOrgID.
		var parentOrg, parentProject *string
		var parentStart, parentEnd time.Time
		err := tx.QueryRow(ctx,
			`SELECT org_id, project_id, start_date, end_date
			   FROM workitems.milestones
			  WHERE id = $1
			    AND ($2 = ''
			         OR org_id = $2
			         OR project_id IN (SELECT id FROM org.projects WHERE org_id = $2))
			  FOR UPDATE`,
			req.ParentMilestoneID, req.CallerOrgID,
		).Scan(&parentOrg, &parentProject, &parentStart, &parentEnd)
		if err != nil {
			if errors.Is(err, sqldb.ErrNoRows) {
				return nil, &errs.Error{Code: errs.NotFound, Message: "parent milestone not found"}
			}
			return nil, &errs.Error{Code: errs.Internal, Message: "parent milestone read failed"}
		}
		// M-INV-5: scope match.
		pOrg := nilString(parentOrg)
		pProj := nilString(parentProject)
		if pOrg != req.OrgID || pProj != req.ProjectID {
			return nil, preconditionError("M-INV-5", "child milestone scope must match parent")
		}
		// M-INV-3: date range containment.
		if startDate.Before(parentStart) || endDate.After(parentEnd) {
			return nil, preconditionError("M-INV-3", "child date range must be ⊆ parent date range")
		}
		// M-INV-6: max depth = 4. Ancestor walk counts levels.
		var depth int
		err = tx.QueryRow(ctx,
			`WITH RECURSIVE ancestors(id, parent_id, depth) AS (
			       SELECT id, parent_milestone_id, 1
			         FROM workitems.milestones
			        WHERE id = $1
			       UNION ALL
			       SELECT m.id, m.parent_milestone_id, a.depth + 1
			         FROM workitems.milestones m
			         JOIN ancestors a ON a.parent_id = m.id
			        WHERE a.depth < 10
			     )
			     SELECT COALESCE(MAX(depth), 0) FROM ancestors`,
			req.ParentMilestoneID,
		).Scan(&depth)
		if err != nil {
			return nil, &errs.Error{Code: errs.Internal, Message: "ancestor walk failed"}
		}
		// depth = number of ancestors (including parent itself = 1).
		// The child being inserted would sit at depth+1.
		if depth+1 > milestoneMaxDepth {
			return nil, preconditionError("M-INV-6", fmt.Sprintf("milestone tree depth would exceed %d", milestoneMaxDepth))
		}
		// M-INV-2: cycle prevention. The ancestor walk already
		// terminates when it can no longer find a parent; if any of
		// the ancestors equals the proposed new id, that's a cycle.
		// Since we're inserting a fresh ULID, this is structurally
		// impossible — but documented here so future re-parent code
		// (deferred to P02) inherits the gate.
	}

	// Row-level tenant gate for the project-scoped branch (bead
	// unblock-tv8.83, §10.1.1): the project-scoped milestone's ProjectID is
	// stamped from the wire, so the INSERT only proceeds when the target
	// project belongs to the caller's org. We express this as a guarded
	// INSERT … SELECT whose WHERE is satisfiable ONLY when (a) the
	// empty-CallerOrgID no-op fires (trusted internal / E2E-seed callers —
	// a DELIBERATE divergence from CreateLabel's hard-reject), (b) this is
	// the org-scoped branch (project_id empty), or (c) the project_id is in
	// the caller's org's projects. A foreign project ULID yields ZERO
	// inserted rows → NOT_FOUND below, never a cross-tenant write. The
	// already-gated parent_milestone_id parent-read seam above is unchanged;
	// CallerOrgID is pinned from identity.OrgID by the MCP handler, never the
	// wire.
	tag, err := tx.Exec(ctx,
		`INSERT INTO workitems.milestones
		   (id, parent_milestone_id, org_id, project_id, name, description, start_date, end_date)
		 SELECT $1, NULLIF($2, ''), NULLIF($3, ''), NULLIF($4, ''), $5, $6, $7, $8
		  WHERE $9 = ''
		     OR $4 = ''
		     OR $4 IN (SELECT id FROM org.projects WHERE org_id = $9)`,
		id, req.ParentMilestoneID, req.OrgID, req.ProjectID, name, req.Description, req.StartDate, req.EndDate, req.CallerOrgID,
	)
	if err != nil {
		if isCheckViolation(err, "milestones_scope_xor_chk") {
			return nil, &errs.Error{Code: errs.InvalidArgument, Message: "milestone scope XOR violation"}
		}
		if isCheckViolation(err, "milestones_date_range_chk") {
			return nil, &errs.Error{Code: errs.InvalidArgument, Message: "milestone date range invalid"}
		}
		if isCheckViolation(err, "milestones_no_self_loop_chk") {
			return nil, preconditionError("M-INV-1", "milestone cannot be its own parent")
		}
		if isForeignKeyViolation(err) {
			return nil, &errs.Error{Code: errs.NotFound, Message: "milestone parent/org/project does not exist"}
		}
		rlog.Error("workitems: milestone insert failed", "err", err)
		return nil, &errs.Error{Code: errs.Internal, Message: "milestone insert failed"}
	}
	// Zero inserted rows means the project-scoped guard rejected the project:
	// the project_id does not belong to the caller's org (or does not exist).
	// Surface NOT_FOUND — the same shape a non-existent project would yield —
	// so a cross-tenant project ULID is indistinguishable from a missing one
	// and never leaks existence across the tenant boundary.
	if tag.RowsAffected() == 0 {
		return nil, &errs.Error{Code: errs.NotFound, Message: "milestone project does not exist"}
	}

	if err := tx.Commit(); err != nil {
		rlog.Error("workitems: milestone create commit failed", "err", err)
		return nil, &errs.Error{Code: errs.Internal, Message: "milestone commit failed"}
	}

	return readMilestone(ctx, id)
}

// UpdateMilestone updates name / description / dates / cancellation.
// Re-parenting is rejected (deferred to P02 per SPEC §4.4.1 line 856).
//
//encore:api private method=POST path=/workitems.UpdateMilestone
func UpdateMilestone(ctx context.Context, req *UpdateMilestoneRequest) (*Milestone, error) {
	if req == nil || req.MilestoneID == "" {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing milestone_id"}
	}
	if req.Name != nil {
		n := strings.TrimSpace(*req.Name)
		if l := len(n); l < 1 || l > 200 {
			return nil, &errs.Error{Code: errs.InvalidArgument, Message: "milestone name must be 1..200 chars"}
		}
		req.Name = &n
	}
	var newStart, newEnd *string
	if req.StartDate != nil {
		if _, err := time.Parse("2006-01-02", *req.StartDate); err != nil {
			return nil, &errs.Error{Code: errs.InvalidArgument, Message: "start_date must be YYYY-MM-DD"}
		}
		newStart = req.StartDate
	}
	if req.EndDate != nil {
		if _, err := time.Parse("2006-01-02", *req.EndDate); err != nil {
			return nil, &errs.Error{Code: errs.InvalidArgument, Message: "end_date must be YYYY-MM-DD"}
		}
		newEnd = req.EndDate
	}

	tx, err := db.Begin(ctx)
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "db begin failed"}
	}
	defer func() { _ = tx.Rollback() }()

	// Lock the row and read current values to validate M-INV-3 against
	// the parent (if any) and against any existing children.
	// Row-level tenant gate (round-16 / bead unblock-tv8.77, §10.1.1): the
	// FOR UPDATE lock is predicated on the milestone tenant predicate —
	// ($2 = '' OR org_id = $2 OR project_id IN the caller's org's projects).
	// A foreign MilestoneID matches no row → ErrNoRows → NOT_FOUND, never a
	// cross-tenant mutation. The empty-CallerOrgID no-op keeps trusted
	// internal callers operating unscoped; the MCP handler always pins
	// CallerOrgID from identity.OrgID.
	var parentID *string
	var curStart, curEnd time.Time
	err = tx.QueryRow(ctx,
		`SELECT parent_milestone_id, start_date, end_date
		   FROM workitems.milestones
		  WHERE id = $1
		    AND ($2 = ''
		         OR org_id = $2
		         OR project_id IN (SELECT id FROM org.projects WHERE org_id = $2))
		  FOR UPDATE`,
		req.MilestoneID, req.CallerOrgID,
	).Scan(&parentID, &curStart, &curEnd)
	if err != nil {
		if errors.Is(err, sqldb.ErrNoRows) {
			return nil, &errs.Error{Code: errs.NotFound, Message: "milestone not found"}
		}
		return nil, &errs.Error{Code: errs.Internal, Message: "milestone read failed"}
	}

	effectiveStart := curStart
	effectiveEnd := curEnd
	if newStart != nil {
		effectiveStart, _ = time.Parse("2006-01-02", *newStart)
	}
	if newEnd != nil {
		effectiveEnd, _ = time.Parse("2006-01-02", *newEnd)
	}
	if effectiveEnd.Before(effectiveStart) {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "end_date must be >= start_date"}
	}

	// M-INV-3 against parent (if any).
	if parentID != nil && *parentID != "" {
		var pStart, pEnd time.Time
		err := tx.QueryRow(ctx,
			`SELECT start_date, end_date FROM workitems.milestones WHERE id = $1`,
			*parentID,
		).Scan(&pStart, &pEnd)
		if err != nil {
			return nil, &errs.Error{Code: errs.Internal, Message: "parent milestone read failed"}
		}
		if effectiveStart.Before(pStart) || effectiveEnd.After(pEnd) {
			return nil, preconditionError("M-INV-3", "child date range must be ⊆ parent date range")
		}
	}

	// M-INV-3 against children — a narrowing update must not orphan
	// any child's range.
	var bad int
	err = tx.QueryRow(ctx,
		`SELECT COUNT(*) FROM workitems.milestones
		  WHERE parent_milestone_id = $1
		    AND (start_date < $2 OR end_date > $3)`,
		req.MilestoneID, effectiveStart.Format("2006-01-02"), effectiveEnd.Format("2006-01-02"),
	).Scan(&bad)
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "children range check failed"}
	}
	if bad > 0 {
		return nil, preconditionError("M-INV-3", "milestone date narrowing would orphan child date range")
	}

	// Apply the update.
	_, err = tx.Exec(ctx,
		`UPDATE workitems.milestones
		    SET name             = COALESCE($2, name),
		        description      = COALESCE($3, description),
		        start_date       = COALESCE($4::date, start_date),
		        end_date         = COALESCE($5::date, end_date),
		        cancelled_at     = CASE WHEN $6::boolean THEN $7 ELSE cancelled_at END,
		        cancelled_reason = COALESCE($8, cancelled_reason),
		        updated_at       = now()
		  WHERE id = $1`,
		req.MilestoneID, req.Name, req.Description, newStart, newEnd,
		req.CancelledAt != nil, req.CancelledAt, req.CancelledReason,
	)
	if err != nil {
		rlog.Error("workitems: milestone update failed", "err", err, "milestone_id", req.MilestoneID)
		return nil, &errs.Error{Code: errs.Internal, Message: "milestone update failed"}
	}

	if err := tx.Commit(); err != nil {
		rlog.Error("workitems: milestone update commit failed", "err", err)
		return nil, &errs.Error{Code: errs.Internal, Message: "milestone update commit failed"}
	}
	return readMilestone(ctx, req.MilestoneID)
}

// AssignItem sets / clears the item's milestone_id atomically. M-INV-7
// enforcement: the target milestone's scope must be reachable from the
// item's project (same project OR org-wide milestone in the same org).
//
// Row-level tenant gate (§10.1.1): BOTH the target-item read/unassign UPDATE
// AND the assign-branch milestone read + final UPDATE are predicated on the
// CallerOrgID channel ($caller = ” OR <tenant predicate>). A foreign item_id
// OR a foreign milestone_id therefore yields NOT_FOUND — the foreign milestone
// is invisible BEFORE the M-INV-7 reachability check, so a cross-tenant
// milestone never leaks its existence via PRECONDITION_NOT_MET. The empty-
// CallerOrgID no-op keeps trusted internal (non-MCP) callers operating
// unscoped; the MCP handler always pins CallerOrgID from identity.OrgID.
//
//encore:api private method=POST path=/workitems.AssignItem
func AssignItem(ctx context.Context, req *AssignItemRequest) error {
	if req == nil || req.ItemID == "" {
		return &errs.Error{Code: errs.InvalidArgument, Message: "missing item_id"}
	}

	tx, err := db.Begin(ctx)
	if err != nil {
		return &errs.Error{Code: errs.Internal, Message: "db begin failed"}
	}
	defer func() { _ = tx.Rollback() }()

	if req.MilestoneID == "" {
		// Unassign: clear all three columns.
		//
		// Row-level tenant gate (round-16 / bead unblock-tv8.77, §10.1.1):
		// the UPDATE is predicated on ($2 = '' OR org_id = $2). A foreign
		// ItemID matches zero rows → NOT_FOUND below, never a cross-tenant
		// unassign. The empty-CallerOrgID no-op keeps trusted internal
		// callers operating unscoped; the MCP handler always pins CallerOrgID.
		res, err := tx.Exec(ctx,
			`UPDATE workitems.items
			    SET milestone_id          = NULL,
			        milestone_assigned_at = NULL,
			        milestone_assigned_by = NULL,
			        updated_at            = now()
			  WHERE id = $1
			    AND ($2 = '' OR org_id = $2)`,
			req.ItemID, req.CallerOrgID,
		)
		if err != nil {
			return &errs.Error{Code: errs.Internal, Message: "milestone unassign failed"}
		}
		// A WHERE id = $1 UPDATE that matches no row silently succeeds.
		// Surface a non-existent OR cross-tenant item as NotFound instead of
		// a bogus 200 — the assign branch returns NotFound for the same
		// condition via its gated SELECT, so mirror that contract here.
		if res.RowsAffected() == 0 {
			return &errs.Error{Code: errs.NotFound, Message: "item not found"}
		}
	} else {
		// M-INV-7: scope reachability check.
		//
		// Row-level tenant gate (round-16 / bead unblock-tv8.77, §10.1.1):
		// the target-item read is predicated on ($2 = '' OR org_id = $2). A
		// foreign ItemID matches no row → ErrNoRows → NOT_FOUND, never a
		// cross-tenant milestone assignment. The empty-CallerOrgID no-op
		// keeps trusted internal callers operating unscoped; the MCP handler
		// always pins CallerOrgID from identity.OrgID.
		var itemOrg, itemProject *string
		err = tx.QueryRow(ctx,
			`SELECT org_id, project_id FROM workitems.items
			  WHERE id = $1
			    AND ($2 = '' OR org_id = $2)`,
			req.ItemID, req.CallerOrgID,
		).Scan(&itemOrg, &itemProject)
		if err != nil {
			if errors.Is(err, sqldb.ErrNoRows) {
				return &errs.Error{Code: errs.NotFound, Message: "item not found"}
			}
			return &errs.Error{Code: errs.Internal, Message: "item read failed"}
		}
		// Row-level tenant gate (bead unblock-tv8.77 pre-QA cleanup,
		// §10.1.1): the milestone read is predicated on
		// ($2 = '' OR org_id = $2 OR project_id IN caller-org projects) —
		// the milestone org-XOR-project form. A foreign MilestoneID matches
		// no row → ErrNoRows → NOT_FOUND, so a cross-tenant milestone never
		// reaches the M-INV-7 reachability check (which would otherwise
		// disclose its existence via PRECONDITION_NOT_MET). The empty-
		// CallerOrgID no-op keeps trusted internal callers operating
		// unscoped; the MCP handler always pins CallerOrgID from
		// identity.OrgID.
		var msOrg, msProject *string
		err = tx.QueryRow(ctx,
			`SELECT org_id, project_id FROM workitems.milestones
			  WHERE id = $1
			    AND ($2 = ''
			         OR org_id = $2
			         OR project_id IN (SELECT id FROM org.projects WHERE org_id = $2))`,
			req.MilestoneID, req.CallerOrgID,
		).Scan(&msOrg, &msProject)
		if err != nil {
			if errors.Is(err, sqldb.ErrNoRows) {
				return &errs.Error{Code: errs.NotFound, Message: "milestone not found"}
			}
			return &errs.Error{Code: errs.Internal, Message: "milestone read failed"}
		}
		// Reachable iff:
		//   (milestone.project_id = item.project_id)
		//   OR (milestone.org_id IS NOT NULL AND milestone.org_id = item.org_id)
		itemProj := nilString(itemProject)
		itemOrgID := nilString(itemOrg)
		msProj := nilString(msProject)
		msOrgID := nilString(msOrg)
		reachable := (msProj != "" && msProj == itemProj) || (msOrgID != "" && msOrgID == itemOrgID)
		if !reachable {
			return preconditionError("M-INV-7", "milestone scope is not reachable in item's project")
		}

		// Row-level tenant gate (bead unblock-tv8.77 pre-QA cleanup,
		// §10.1.1): the final assign UPDATE carries the same
		// ($4 = '' OR org_id = $4) predicate as the unassign branch and the
		// gated read above. The preceding read is a plain QueryRow (not FOR
		// UPDATE), so — unlike Close/Claim/SetState — there is no row lock
		// bridging read→write; gating the UPDATE statement itself makes the
		// mutating write self-contained and defense-in-depth symmetric with
		// the unassign branch.
		_, err = tx.Exec(ctx,
			`UPDATE workitems.items
			    SET milestone_id          = $2,
			        milestone_assigned_at = now(),
			        milestone_assigned_by = NULLIF($3, ''),
			        updated_at            = now()
			  WHERE id = $1
			    AND ($4 = '' OR org_id = $4)`,
			req.ItemID, req.MilestoneID, req.AssignedByUser, req.CallerOrgID,
		)
		if err != nil {
			if isForeignKeyViolation(err) {
				return &errs.Error{Code: errs.NotFound, Message: "milestone or assignee does not exist"}
			}
			return &errs.Error{Code: errs.Internal, Message: "milestone assign failed"}
		}
	}

	if err := tx.Commit(); err != nil {
		rlog.Error("workitems: milestone assign commit failed", "err", err)
		return &errs.Error{Code: errs.Internal, Message: "milestone assign commit failed"}
	}
	return nil
}

// MilestoneTree returns the recursive tree of milestones rooted at
// RootMilestoneID OR all roots within (OrgID, ProjectID). SPEC §4.4.1 +
// §9.4.9 (depth-bounded by M-INV-6).
//
// Tenant scoping (round-16 / beads unblock-tv8.75 + .77, §10.1.1): the
// CallerOrgID channel gates BOTH walks — the roots walk selects only roots
// reachable from CallerOrgID, and the rooted walk (RootMilestoneID set)
// requires the root milestone itself to be reachable from CallerOrgID
// (directly org-scoped, or in a project owned by CallerOrgID). A foreign
// root therefore yields an empty result rather than leaking a cross-tenant
// subtree. OrgID / ProjectID remain the wire-supplied scope SELECTORS for
// the roots walk (which roots to enumerate), distinct from the CallerOrgID
// tenant GATE (which tenant the caller may see). When CallerOrgID is empty
// (trusted internal callers — the §11.1.1 E2E seed, the P05 roadmap RPC)
// the gate is a no-op. The agent-facing milestone_tree MCP tool always
// pins CallerOrgID from identity.OrgID on both paths, so the no-op is
// unreachable from the agent surface.
//
//encore:api private method=POST path=/workitems.MilestoneTree
func MilestoneTree(ctx context.Context, req *MilestoneTreeRequest) (*MilestoneTreeResponse, error) {
	if req == nil {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing request body"}
	}
	if req.RootMilestoneID == "" && req.OrgID == "" && req.ProjectID == "" {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "MilestoneTree requires root_milestone_id OR (org_id and/or project_id)"}
	}

	var rows *sqldb.Rows
	var err error
	switch {
	case req.RootMilestoneID != "":
		// Rooted walk. The recursive-anchor WHERE clause gates the root
		// milestone by tenant reachability from req.CallerOrgID: the root
		// must be directly org-scoped to $4 OR project-scoped to a project
		// owned by $4. When $4 is the empty string (trusted internal callers
		// — the E2E exit-criterion test, the P05 roadmap RPC) the predicate
		// is a no-op and the walk is unscoped. The agent-facing milestone_tree
		// MCP tool ALWAYS pins CallerOrgID from identity.OrgID, so a foreign
		// root_milestone_id yields an empty anchor (no rows) — closing the
		// cross-tenant / IDOR read seam on the rooted path (SPEC §10.1.1 /
		// §4.4.1; §6.2 Tool 19 tenant-predicate-injected read).
		rows, err = db.Query(ctx,
			`WITH RECURSIVE tree(id, parent_milestone_id, org_id, project_id, name, description,
			                     start_date, end_date, cancelled_at, cancelled_reason,
			                     created_at, updated_at, depth) AS (
			       SELECT id, parent_milestone_id, org_id, project_id, name, description,
			              start_date, end_date, cancelled_at, cancelled_reason,
			              created_at, updated_at, 0
			         FROM workitems.milestones
			        WHERE id = $1
			          AND ($4 = '' OR org_id = $4 OR project_id IN (SELECT id FROM org.projects WHERE org_id = $4))
			       UNION ALL
			       SELECT m.id, m.parent_milestone_id, m.org_id, m.project_id, m.name, m.description,
			              m.start_date, m.end_date, m.cancelled_at, m.cancelled_reason,
			              m.created_at, m.updated_at, t.depth + 1
			         FROM workitems.milestones m
			         JOIN tree t ON m.parent_milestone_id = t.id
			        WHERE t.depth < $2
			     )
			     SELECT id, COALESCE(parent_milestone_id, ''), COALESCE(org_id, ''), COALESCE(project_id, ''),
			            name, COALESCE(description, ''),
			            start_date, end_date, cancelled_at, COALESCE(cancelled_reason, ''),
			            created_at, updated_at, depth
			       FROM tree
			      WHERE ($3::boolean OR cancelled_at IS NULL)
			      ORDER BY depth, start_date, id`,
			req.RootMilestoneID, milestoneMaxDepth-1, req.IncludeCancelled, req.CallerOrgID,
		)
	default:
		// Walk from all roots in the scope. Roots are milestones whose
		// parent_milestone_id IS NULL within (org_id, project_id).
		//
		// Two distinct predicates apply to the anchor (round-16 / bead
		// unblock-tv8.77, §10.1.1):
		//   - $1 (OrgID) / $2 (ProjectID) — the wire-supplied SCOPE SELECTOR
		//     (which roots to enumerate). Unchanged from the existing
		//     contract.
		//   - $5 (CallerOrgID) — the tenant GATE. A row is visible only when
		//     its org_id = CallerOrgID OR its project_id is in the caller's
		//     org's projects. The empty-CallerOrgID no-op keeps trusted
		//     internal callers unscoped; the MCP handler always pins it.
		// Separating the two means a caller cannot enumerate another org's
		// roots even by supplying that org's id as the OrgID selector — the
		// CallerOrgID gate still rejects the foreign rows.
		rows, err = db.Query(ctx,
			`WITH RECURSIVE tree(id, parent_milestone_id, org_id, project_id, name, description,
			                     start_date, end_date, cancelled_at, cancelled_reason,
			                     created_at, updated_at, depth) AS (
			       SELECT id, parent_milestone_id, org_id, project_id, name, description,
			              start_date, end_date, cancelled_at, cancelled_reason,
			              created_at, updated_at, 0
			         FROM workitems.milestones
			        WHERE parent_milestone_id IS NULL
			          AND ($1 = '' OR org_id = $1 OR project_id IN (SELECT id FROM org.projects WHERE org_id = $1))
			          AND ($2 = '' OR project_id = $2)
			          AND ($5 = '' OR org_id = $5 OR project_id IN (SELECT id FROM org.projects WHERE org_id = $5))
			       UNION ALL
			       SELECT m.id, m.parent_milestone_id, m.org_id, m.project_id, m.name, m.description,
			              m.start_date, m.end_date, m.cancelled_at, m.cancelled_reason,
			              m.created_at, m.updated_at, t.depth + 1
			         FROM workitems.milestones m
			         JOIN tree t ON m.parent_milestone_id = t.id
			        WHERE t.depth < $3
			     )
			     SELECT id, COALESCE(parent_milestone_id, ''), COALESCE(org_id, ''), COALESCE(project_id, ''),
			            name, COALESCE(description, ''),
			            start_date, end_date, cancelled_at, COALESCE(cancelled_reason, ''),
			            created_at, updated_at, depth
			       FROM tree
			      WHERE ($4::boolean OR cancelled_at IS NULL)
			      ORDER BY depth, start_date, id`,
			req.OrgID, req.ProjectID, milestoneMaxDepth-1, req.IncludeCancelled, req.CallerOrgID,
		)
	}
	if err != nil {
		rlog.Error("workitems: milestone_tree failed", "err", err)
		return nil, &errs.Error{Code: errs.Internal, Message: "milestone tree failed"}
	}
	defer rows.Close()

	type rowFlat struct {
		M     Milestone
		Depth int
	}
	var flat []rowFlat
	for rows.Next() {
		var r rowFlat
		var startDate, endDate time.Time
		if err := rows.Scan(
			&r.M.ID, &r.M.ParentMilestoneID, &r.M.OrgID, &r.M.ProjectID,
			&r.M.Name, &r.M.Description,
			&startDate, &endDate, &r.M.CancelledAt, &r.M.CancelledReason,
			&r.M.CreatedAt, &r.M.UpdatedAt, &r.Depth,
		); err != nil {
			return nil, &errs.Error{Code: errs.Internal, Message: "milestone tree scan failed"}
		}
		r.M.StartDate = startDate.Format("2006-01-02")
		r.M.EndDate = endDate.Format("2006-01-02")
		flat = append(flat, r)
	}
	if err := rows.Err(); err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "milestone tree iter failed"}
	}

	// Assemble nested structure. Iterate in REVERSE depth order
	// (deepest first) so when we copy a node into its parent's
	// Children slice, the node's own Children are already attached.
	// SQL's ORDER BY depth, start_date, id places parents before
	// children — reverse-iteration flips that.
	byID := make(map[string]*MilestoneNode, len(flat))
	for i := range flat {
		byID[flat[i].M.ID] = &MilestoneNode{Milestone: flat[i].M, Depth: flat[i].Depth}
	}
	for i := len(flat) - 1; i >= 0; i-- {
		parentID := flat[i].M.ParentMilestoneID
		if parentID == "" {
			continue
		}
		parent, ok := byID[parentID]
		if !ok {
			// Parent not in result set (subtree query rooted at a
			// non-root milestone). This row is effectively a root.
			continue
		}
		// Prepend rather than append so the in-order traversal
		// preserves the SQL's start_date / id ordering after the
		// reverse iteration.
		parent.Children = append([]MilestoneNode{*byID[flat[i].M.ID]}, parent.Children...)
	}
	// Materialise root values. A row is a root iff its parent is
	// empty OR its parent is not in the result set.
	var roots []MilestoneNode
	for i := range flat {
		parentID := flat[i].M.ParentMilestoneID
		if parentID == "" || byID[parentID] == nil {
			roots = append(roots, *byID[flat[i].M.ID])
		}
	}

	return &MilestoneTreeResponse{Roots: roots}, nil
}

// -----------------------------------------------------------------------------
// Label-registry RPCs (round-16, beads unblock-tv8.75 + .77). Back §6.2
// Tools 20–23 over the EXISTING workitems.labels / workitems.item_labels
// DDL (SPEC §9.4.3) + migration 0130 (updated_at). Org scoping follows the
// row-level tenant gate (see the package auth-model doc-comment): the write
// RPCs (CreateLabel / UpdateLabel / DeleteLabel) self-gate on the
// CallerOrgID internal channel pinned by the MCP handler from
// identity.OrgID and HARD-REJECT an empty CallerOrgID with InvalidArgument
// (MCP-only callers, no trusted-internal no-op — round-16 / bead
// unblock-tv8.77); a foreign LabelID / project ULID yields NOT_FOUND, never
// a cross-tenant mutation. ListLabels scopes via an explicit tenant
// predicate. SPEC §4.4 / §10.1.1.
// -----------------------------------------------------------------------------

// CreateLabel inserts a label into the registry, org- XOR project-scoped.
// OrgID is pinned by the MCP handler from identity.OrgID (never wire). A
// duplicate name within the same scope (case-insensitive per the
// lower(name) UNIQUE indexes) → AlreadyExists with Meta["constraint"]
// naming the violated index, which errmap projects into the §7 CONFLICT
// envelope's data.constraint. SPEC §4.4 / §6.2 Tool 20.
//
//encore:api private method=POST path=/workitems.CreateLabel
func CreateLabel(ctx context.Context, req *CreateLabelRequest) (*Label, error) {
	if req == nil {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing request body"}
	}
	if req.CallerOrgID == "" {
		// The MCP handler always pins CallerOrgID from identity.OrgID; an
		// empty value here is a programmer error (a hard tenant gate, never
		// an unscoped write). CreateLabel has no trusted-internal no-auth
		// caller path — it is MCP-only — so the §10.1.1 empty-CallerOrgID
		// no-op is WRONG here. Hard-reject for consistency with UpdateLabel /
		// DeleteLabel (round-16 / bead unblock-tv8.77 — closes the
		// deferred-epic RISK).
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "CreateLabel requires caller org scope"}
	}
	// Scope XOR: exactly one of OrgID or ProjectID must be set. The DB
	// labels_scope_xor_chk is the last line of defence; reject early here
	// so the error is a clean VALIDATION rather than a CHECK violation.
	if (req.OrgID == "" && req.ProjectID == "") || (req.OrgID != "" && req.ProjectID != "") {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "label scope must be exactly one of org_id or project_id"}
	}
	name := strings.TrimSpace(req.Name)
	if l := len(name); l < labelNameMinLen || l > labelNameMaxLen {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: fmt.Sprintf("label name must be %d..%d chars", labelNameMinLen, labelNameMaxLen), Meta: errs.Metadata{"field": "name"}}
	}
	if !labelColorPattern.MatchString(req.Color) {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "color must be #RRGGBB", Meta: errs.Metadata{"field": "color"}}
	}

	id, err := ulid.New()
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "label id generation failed"}
	}

	// Row-level tenant gate for the project-scoped branch (DRIFT-2c locked
	// decision): the INSERT only proceeds when the target project belongs to
	// the caller's org. We express this as a guarded INSERT … SELECT whose
	// WHERE is satisfiable ONLY when either (a) this is the org-scoped branch
	// (project_id empty) or (b) the project_id is in the caller's org's
	// projects (cross-schema read of org.projects, the same precedent
	// ListLabels uses). A foreign project ULID yields ZERO inserted rows →
	// NOT_FOUND below, never a cross-tenant write. CallerOrgID is pinned from
	// identity.OrgID by the MCP handler, never the wire.
	tag, err := db.Exec(ctx,
		`INSERT INTO workitems.labels (id, org_id, project_id, name, color, description)
		 SELECT $1, NULLIF($2, ''), NULLIF($3, ''), $4, $5, NULLIF($6, '')
		  WHERE $3 = ''
		     OR $3 IN (SELECT id FROM org.projects WHERE org_id = $7)`,
		id, req.OrgID, req.ProjectID, name, req.Color, req.Description, req.CallerOrgID,
	)
	if err != nil {
		// Case-insensitive uniqueness is enforced by the lower(name) UNIQUE
		// indexes — a "Bug" vs "bug" duplicate trips the same index. Surface
		// the violated index name in Meta["constraint"] for §7 CONFLICT.
		if isUniqueViolation(err, "labels_org_name_uniq") {
			return nil, &errs.Error{Code: errs.AlreadyExists, Message: "label name already exists in this org", Meta: errs.Metadata{"constraint": "labels_org_name_uniq"}}
		}
		if isUniqueViolation(err, "labels_project_name_uniq") {
			return nil, &errs.Error{Code: errs.AlreadyExists, Message: "label name already exists in this project", Meta: errs.Metadata{"constraint": "labels_project_name_uniq"}}
		}
		if isCheckViolation(err, "labels_color_chk") {
			return nil, &errs.Error{Code: errs.InvalidArgument, Message: "color must be #RRGGBB", Meta: errs.Metadata{"field": "color"}}
		}
		if isCheckViolation(err, "labels_scope_xor_chk") {
			return nil, &errs.Error{Code: errs.InvalidArgument, Message: "label scope XOR violation"}
		}
		if isForeignKeyViolation(err) {
			return nil, &errs.Error{Code: errs.NotFound, Message: "label org/project does not exist"}
		}
		rlog.Error("workitems: label insert failed", "err", err)
		return nil, &errs.Error{Code: errs.Internal, Message: "label insert failed"}
	}
	// Zero inserted rows means the project-scoped guard rejected the project:
	// the project_id does not belong to the caller's org (or does not exist).
	// Surface NOT_FOUND — the same shape a non-existent project would yield —
	// so a cross-tenant project ULID is indistinguishable from a missing one
	// and never leaks existence across the tenant boundary.
	if tag.RowsAffected() == 0 {
		return nil, &errs.Error{Code: errs.NotFound, Message: "label project does not exist"}
	}

	return readLabel(ctx, id)
}

// ListLabels returns the labels visible within the caller's scope. When
// ProjectID is empty the result is the org's own labels (org_id = OrgID).
// When ProjectID is set the result is the project's labels PLUS the
// inherited org labels, with PRD §6.4 "project wins on identical name"
// applied at query time: an org label is suppressed when the project
// defines a label with the same lower(name). Org scope is always the
// caller's OrgID (pinned by the MCP handler); the predicate is a
// hard tenant gate. SPEC §4.4 / §6.2 Tool 21.
//
//encore:api private method=POST path=/workitems.ListLabels
func ListLabels(ctx context.Context, req *ListLabelsRequest) (*ListLabelsResponse, error) {
	if req == nil {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing request body"}
	}
	if req.OrgID == "" {
		// The MCP handler always pins OrgID from identity.OrgID; an empty
		// OrgID here is a programmer error (no trusted-internal no-op is
		// defined for labels, unlike MilestoneTree).
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "ListLabels requires org scope"}
	}

	var rows *sqldb.Rows
	var err error
	if req.ProjectID == "" {
		// Org-scoped: the caller's own org labels only.
		rows, err = db.Query(ctx,
			`SELECT id, COALESCE(org_id, ''), COALESCE(project_id, ''),
			        name, color, COALESCE(description, ''), created_at, updated_at
			   FROM workitems.labels
			  WHERE org_id = $1
			  ORDER BY lower(name), id`,
			req.OrgID,
		)
	} else {
		// Project-scoped: the project's labels UNION the inherited org
		// labels, the org label suppressed when the project shadows it on
		// lower(name) (PRD §6.4 "project wins on identical name"). The
		// project must belong to the caller's org — a project that is not
		// owned by $1 yields no project rows AND no org-inheritance leak
		// because the org branch is still gated on org_id = $1.
		// The UNION ALL is wrapped in a subquery so the outer ORDER BY can
		// reference lower(name): Postgres rejects an expression ORDER BY
		// applied directly to a set operation (SQLSTATE 0A000 "invalid
		// UNION/INTERSECT/EXCEPT ORDER BY clause").
		rows, err = db.Query(ctx,
			`SELECT id, org_id, project_id, name, color, description, created_at, updated_at
			   FROM (
			       SELECT id, COALESCE(org_id, '') AS org_id, COALESCE(project_id, '') AS project_id,
			              name, color, COALESCE(description, '') AS description, created_at, updated_at
			         FROM workitems.labels
			        WHERE project_id = $2
			          AND project_id IN (SELECT id FROM org.projects WHERE org_id = $1)
			       UNION ALL
			       SELECT id, COALESCE(o.org_id, '') AS org_id, COALESCE(o.project_id, '') AS project_id,
			              o.name, o.color, COALESCE(o.description, '') AS description, o.created_at, o.updated_at
			         FROM workitems.labels o
			        WHERE o.org_id = $1
			          AND NOT EXISTS (
			              SELECT 1 FROM workitems.labels p
			               WHERE p.project_id = $2
			                 AND lower(p.name) = lower(o.name)
			          )
			   ) merged
			  ORDER BY lower(name), id`,
			req.OrgID, req.ProjectID,
		)
	}
	if err != nil {
		rlog.Error("workitems: list labels failed", "err", err)
		return nil, &errs.Error{Code: errs.Internal, Message: "list labels failed"}
	}
	defer rows.Close()

	labels := make([]Label, 0)
	for rows.Next() {
		var l Label
		if err := rows.Scan(&l.ID, &l.OrgID, &l.ProjectID, &l.Name, &l.Color, &l.Description, &l.CreatedAt, &l.UpdatedAt); err != nil {
			return nil, &errs.Error{Code: errs.Internal, Message: "label scan failed"}
		}
		labels = append(labels, l)
	}
	if err := rows.Err(); err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "label iter failed"}
	}
	return &ListLabelsResponse{Labels: labels}, nil
}

// UpdateLabel renames and/or recolors an existing label and bumps
// updated_at on every successful write. Scope (org_id / project_id) is
// immutable — a scope change is a delete-then-create. A rename that
// collides with an existing label in the same scope (case-insensitive) →
// AlreadyExists with Meta["constraint"] → §7 CONFLICT. SPEC §4.4 / §6.2
// Tool 22.
//
// Applies a row-level tenant predicate (DRIFT-3b): the targeted label's
// org_id = CallerOrgID OR its project_id belongs to a project in the
// caller's org. A foreign LabelID matches zero rows → NOT_FOUND, never a
// cross-tenant write. CallerOrgID is pinned from identity.OrgID by the MCP
// handler, never the wire.
//
//encore:api private method=POST path=/workitems.UpdateLabel
func UpdateLabel(ctx context.Context, req *UpdateLabelRequest) (*Label, error) {
	if req == nil || req.LabelID == "" {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing label_id"}
	}
	if req.CallerOrgID == "" {
		// The MCP handler always pins CallerOrgID from identity.OrgID; an
		// empty value here is a programmer error (a hard tenant gate, never
		// an unscoped write).
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "UpdateLabel requires caller org scope"}
	}
	if req.Name != nil {
		n := strings.TrimSpace(*req.Name)
		if l := len(n); l < labelNameMinLen || l > labelNameMaxLen {
			return nil, &errs.Error{Code: errs.InvalidArgument, Message: fmt.Sprintf("label name must be %d..%d chars", labelNameMinLen, labelNameMaxLen), Meta: errs.Metadata{"field": "name"}}
		}
		req.Name = &n
	}
	if req.Color != nil && !labelColorPattern.MatchString(*req.Color) {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "color must be #RRGGBB", Meta: errs.Metadata{"field": "color"}}
	}

	// The WHERE clause's row-level tenant predicate is the IDOR gate: a
	// label whose org_id is not the caller's AND whose project_id is not in
	// the caller's org's projects matches zero rows → NOT_FOUND below. This
	// closes the same cross-tenant seam Tool 19 closed for reads.
	tag, err := db.Exec(ctx,
		`UPDATE workitems.labels
		    SET name        = COALESCE($2, name),
		        color       = COALESCE($3, color),
		        description  = CASE WHEN $4::boolean THEN NULLIF($5, '') ELSE description END,
		        updated_at   = now()
		  WHERE id = $1
		    AND (org_id = $6
		         OR project_id IN (SELECT id FROM org.projects WHERE org_id = $6))`,
		req.LabelID, req.Name, req.Color, req.Description != nil, ptrToString(req.Description), req.CallerOrgID,
	)
	if err != nil {
		if isUniqueViolation(err, "labels_org_name_uniq") {
			return nil, &errs.Error{Code: errs.AlreadyExists, Message: "label name already exists in this org", Meta: errs.Metadata{"constraint": "labels_org_name_uniq"}}
		}
		if isUniqueViolation(err, "labels_project_name_uniq") {
			return nil, &errs.Error{Code: errs.AlreadyExists, Message: "label name already exists in this project", Meta: errs.Metadata{"constraint": "labels_project_name_uniq"}}
		}
		if isCheckViolation(err, "labels_color_chk") {
			return nil, &errs.Error{Code: errs.InvalidArgument, Message: "color must be #RRGGBB", Meta: errs.Metadata{"field": "color"}}
		}
		rlog.Error("workitems: label update failed", "err", err, "label_id", req.LabelID)
		return nil, &errs.Error{Code: errs.Internal, Message: "label update failed"}
	}
	if tag.RowsAffected() == 0 {
		return nil, &errs.Error{Code: errs.NotFound, Message: "label not found"}
	}

	return readLabel(ctx, req.LabelID)
}

// DeleteLabel removes a label from the registry. The workitems.item_labels
// junction rows referencing it cascade away in the SAME transaction (the
// FK is ON DELETE CASCADE per SPEC §9.4.3) — deleting a label detaches it
// from every item without deleting the items. DetachedItemCount is the
// number of junction rows removed. SPEC §4.4 / §6.2 Tool 23.
//
// Applies a row-level tenant predicate (DRIFT-3b): the targeted label's
// org_id = CallerOrgID OR its project_id belongs to a project in the
// caller's org. A foreign LabelID matches zero rows → NOT_FOUND, never a
// cross-tenant delete. CallerOrgID is pinned from identity.OrgID by the MCP
// handler, never the wire.
//
//encore:api private method=POST path=/workitems.DeleteLabel
func DeleteLabel(ctx context.Context, req *DeleteLabelRequest) (*DeleteLabelResponse, error) {
	if req == nil || req.LabelID == "" {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing label_id"}
	}
	if req.CallerOrgID == "" {
		// The MCP handler always pins CallerOrgID from identity.OrgID; an
		// empty value here is a programmer error (a hard tenant gate, never
		// an unscoped delete).
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "DeleteLabel requires caller org scope"}
	}

	tx, err := db.Begin(ctx)
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "db begin failed"}
	}
	defer func() { _ = tx.Rollback() }()

	// Count the junction rows BEFORE the delete so DetachedItemCount
	// reflects exactly what the FK cascade will remove. The count is gated
	// by the SAME row-level tenant predicate as the DELETE so a foreign
	// label_id yields a 0 count AND the DELETE matches zero rows → NOT_FOUND
	// (never a cross-tenant existence/count leak). Done inside the
	// transaction so a concurrent attach/detach cannot skew the count
	// relative to the delete.
	var detached int
	if err := tx.QueryRow(ctx,
		`SELECT COUNT(*)
		   FROM workitems.item_labels il
		   JOIN workitems.labels l ON l.id = il.label_id
		  WHERE il.label_id = $1
		    AND (l.org_id = $2
		         OR l.project_id IN (SELECT id FROM org.projects WHERE org_id = $2))`,
		req.LabelID, req.CallerOrgID,
	).Scan(&detached); err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "label detach count failed"}
	}

	tag, err := tx.Exec(ctx,
		`DELETE FROM workitems.labels
		  WHERE id = $1
		    AND (org_id = $2
		         OR project_id IN (SELECT id FROM org.projects WHERE org_id = $2))`,
		req.LabelID, req.CallerOrgID,
	)
	if err != nil {
		rlog.Error("workitems: label delete failed", "err", err, "label_id", req.LabelID)
		return nil, &errs.Error{Code: errs.Internal, Message: "label delete failed"}
	}
	if tag.RowsAffected() == 0 {
		return nil, &errs.Error{Code: errs.NotFound, Message: "label not found"}
	}

	if err := tx.Commit(); err != nil {
		rlog.Error("workitems: label delete commit failed", "err", err)
		return nil, &errs.Error{Code: errs.Internal, Message: "label delete commit failed"}
	}

	return &DeleteLabelResponse{Deleted: true, LabelID: req.LabelID, DetachedItemCount: detached}, nil
}

// -----------------------------------------------------------------------------
// Internal helpers — see helpers.go for row scanners and error mappers.
// -----------------------------------------------------------------------------

// validateTitle enforces the 1..200 char Title window per SPEC §4.4 line 586.
func validateTitle(title string) error {
	if l := len(title); l < titleMinLen || l > titleMaxLen {
		return &errs.Error{
			Code:    errs.InvalidArgument,
			Message: fmt.Sprintf("title must be %d..%d chars (got %d)", titleMinLen, titleMaxLen, l),
			Meta:    errs.Metadata{"field": "title"},
		}
	}
	return nil
}

// validateStateEnums rejects unknown state-column values before the
// transaction begins. Returns nil when every non-nil pointer is in
// its respective allow-list.
func validateStateEnums(req *SetStateRequest) error {
	if req.ImplState != nil {
		if _, ok := implStateAllowed[*req.ImplState]; !ok {
			return &errs.Error{Code: errs.InvalidArgument, Message: fmt.Sprintf("invalid impl_state %q", *req.ImplState), Meta: errs.Metadata{"field": "impl_state"}}
		}
	}
	if req.ReviewState != nil {
		if _, ok := reviewStateAllowed[*req.ReviewState]; !ok {
			return &errs.Error{Code: errs.InvalidArgument, Message: fmt.Sprintf("invalid review_state %q", *req.ReviewState), Meta: errs.Metadata{"field": "review_state"}}
		}
	}
	if req.QAState != nil {
		if _, ok := qaStateAllowed[*req.QAState]; !ok {
			return &errs.Error{Code: errs.InvalidArgument, Message: fmt.Sprintf("invalid qa_state %q", *req.QAState), Meta: errs.Metadata{"field": "qa_state"}}
		}
	}
	if req.PipelineState != nil {
		if _, ok := pipelineStateAllowed[*req.PipelineState]; !ok {
			return &errs.Error{Code: errs.InvalidArgument, Message: fmt.Sprintf("invalid pipeline_state %q", *req.PipelineState), Meta: errs.Metadata{"field": "pipeline_state"}}
		}
	}
	return nil
}

// coalesceState returns the requested new value when set, else the
// current column value. Used by SetStateColumns to compute the
// post-update tuple before the invariant checks fire.
func coalesceState(req *string, current string) string {
	if req != nil {
		return *req
	}
	return current
}

// preconditionError builds the canonical FailedPrecondition error
// carrying the named invariant in Meta. SPEC §6.2 Tool 13 line 1499.
func preconditionError(invariant, message string) error {
	return &errs.Error{
		Code:    errs.FailedPrecondition,
		Message: message,
		Meta:    errs.Metadata{"invariant": invariant},
	}
}

// preconditionErrorMissing builds a FailedPrecondition error that
// additionally carries a `missing` field naming the column / argument
// whose absence triggered the precondition. The MCP errmap projects
// Meta["missing"] into the §7 PRECONDITION_NOT_MET envelope's
// `data.missing` per SPEC §7 (line 2061) and §6.2 Tool 6 (line 1334:
// "PRECONDITION_NOT_MET and data.missing = \"claimed_by_id\"").
//
// Use this builder when the precondition is a "<column> IS NOT NULL"
// gate; for invariants that have no single missing-input scalar (e.g.
// the I-3 / I-4 / I-5 transitions in SetStateColumns) keep
// preconditionError above — Meta["invariant"] alone surfaces as
// `data.rejection_reason` per errmap's classifyEnvelopeError.
func preconditionErrorMissing(invariant, missing, message string) error {
	return &errs.Error{
		Code:    errs.FailedPrecondition,
		Message: message,
		Meta: errs.Metadata{
			"invariant": invariant,
			"missing":   missing,
		},
	}
}

// alreadyClaimedError reads the winner row info and packages it as the
// loser-side error. Used by Claim's loser branch per SPEC §6.4.
func alreadyClaimedError(ctx context.Context, itemID string) error {
	var winnerID, winnerAgent *string
	var claimedAt *time.Time
	err := db.QueryRow(ctx,
		`SELECT claimed_by_id, claimed_by_agent, claimed_at
		   FROM workitems.items
		  WHERE id = $1`,
		itemID,
	).Scan(&winnerID, &winnerAgent, &claimedAt)
	if err != nil {
		if errors.Is(err, sqldb.ErrNoRows) {
			return &errs.Error{Code: errs.NotFound, Message: "item not found"}
		}
		return &errs.Error{Code: errs.Internal, Message: "claim winner lookup failed"}
	}
	meta := errs.Metadata{"reason": "already_claimed"}
	if winnerID != nil {
		meta["winner_user_id"] = *winnerID
	}
	if winnerAgent != nil {
		meta["winner_agent"] = *winnerAgent
	}
	if claimedAt != nil {
		meta["claimed_at"] = claimedAt.Format(time.RFC3339Nano)
	}
	return &errs.Error{
		Code:    errs.AlreadyExists,
		Message: "item already claimed",
		Meta:    meta,
	}
}

// callerIdentity reads the Encore auth context. Mirrors org.callerIdentity.
func callerIdentity(_ context.Context) (auth.Identity, bool) {
	uid, ok := encoreauth.UserID()
	if !ok || uid == "" {
		return auth.Identity{}, false
	}
	if data, ok := encoreauth.Data().(*auth.AuthData); ok && data != nil {
		return data.Identity, true
	}
	return auth.Identity{UserID: string(uid)}, true
}

// derefString returns *s when s != nil, else "".
func derefString(s *string) string {
	if s == nil {
		return ""
	}
	return *s
}

// nilString returns *s when s != nil, else "".
func nilString(s *string) string {
	if s == nil {
		return ""
	}
	return *s
}
