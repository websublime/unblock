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
// # Authorisation model — layered gate (read-side vs write-side)
//
// The workitems service uses a deliberately asymmetric authorisation
// pattern that callers MUST respect:
//
//   - Read-side RPCs (Get, GetTrail, List, Search) self-gate via
//     rbac.For[T](identity, table). The rbac builder injects the
//     tenant predicate (org_id = $caller_org) directly into every
//     emitted SQL clause, so a misbehaving or compromised caller can
//     never read across orgs through these RPCs — the tenant gate is
//     enforced by the SQL itself, not by a callable check.
//
//   - Write-side RPCs (Create, Update, AppendComment, SetStateColumns,
//     Close, Claim, CreateMilestone, UpdateMilestone, AssignItem) do
//     NOT call org.Authorize internally. They trust that the MCP tool
//     handler invoked org.Authorize against the caller's session +
//     the request's org_id BEFORE dispatching to the private RPC. The
//     MCP layer is the single authoritative gate for the write path
//     because it owns the session→identity resolution and the role
//     resolution that org.Authorize requires. Layering Authorize
//     inside every private write RPC would duplicate that
//     resolution, double-bill the auth schema, and split the audit
//     trail between two layers.
//
// This is consistent with the org service's own private writes
// (org.CreateProject etc.) and matches SPEC §10.1's gate model: read
// gates live in the SQL, write gates live at the MCP boundary. If a
// new caller outside the MCP layer (e.g. another internal service)
// needs to invoke a write RPC here, the caller is responsible for
// calling org.Authorize first; the RPC's pre-validation (field
// validation, enum allow-listing, FK checks) is NOT a substitute for
// the org gate.
//
// Belt-and-suspenders defence-in-depth (a second org.Authorize call
// inside each write RPC) was considered and rejected during round-2
// review of bead unblock-tv8.10 — the duplicate gate would obscure
// the layered model without adding a meaningful security property,
// since the only callers in P01 are the MCP layer and the test
// harness (which uses the same rbac.For gate on the read side).
package workitems

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"time"

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
// Edge (CreateRequest.Dependencies and Trail.DependenciesIn/Out) uses
// deps.Edge directly per SPEC §4.4 lines 591-592 — the skeleton-time local
// workitems.Edge struct was removed in C-1 (bead unblock-tv8.10), closing
// findings unblock-tv8.27 and unblock-tv8.28.
// -----------------------------------------------------------------------------

// Item is the canonical work-item row shape. SPEC §4.4.
type Item struct {
	ID                  string
	OrgID               string
	ProjectID           string
	MilestoneID         string
	ParentID            string
	DiscoveredFromID    string
	Type                string
	Title               string
	Body                string
	Status              string
	Priority            string
	PipelineStage       string
	AgentKind           string
	ImplState           string
	ReviewState         string
	QAState             string
	PipelineState       string
	Severity            string
	KindOfFinding       string
	ClaimedByID         string
	ClaimedByAgent      string
	ClaimedAt           *time.Time
	IsReady             bool
	MilestoneAssignedAt *time.Time
	MilestoneAssignedBy string
	Labels              []string
	CreatedAt           time.Time
	UpdatedAt           time.Time
	ClosedAt            *time.Time
}

// CreateRequest is the input to Create. SPEC §4.4.
type CreateRequest struct {
	OrgID            string
	ProjectID        string
	ParentID         string
	DiscoveredFromID string
	Type             string
	Title            string
	Body             string
	Priority         string
	MilestoneID      string
	Labels           []string
	Dependencies     []deps.Edge
	Severity         string
	KindOfFinding    string
}

// Comment is the canonical comment row shape. SPEC §4.4.
type Comment struct {
	ID          string
	ItemID      string
	ParentID    string
	AuthorID    string
	AuthorAgent string
	Kind        string
	Status      string
	Body        string
	CreatedAt   time.Time
	UpdatedAt   time.Time
}

// UpdateRequest is the input to Update. SPEC §4.4.
type UpdateRequest struct {
	ItemID      string
	Title       *string
	Body        *string
	Priority    *string
	MilestoneID *string
	Labels      *[]string
}

// GetTrailRequest is the input to GetTrail. SPEC §4.4.
type GetTrailRequest struct {
	ItemID string
}

// Trail is the comment + edges + findings bundle returned by GetTrail.
// SPEC §4.4. DependenciesIn / DependenciesOut use deps.Edge (post C-1
// reconciliation, bead unblock-tv8.10).
type Trail struct {
	Item            *Item
	Comments        []Comment
	DependenciesIn  []deps.Edge
	DependenciesOut []deps.Edge
	Findings        []Item
}

// AppendCommentRequest is the input to AppendComment. SPEC §4.4.
type AppendCommentRequest struct {
	ItemID      string
	AuthorID    string
	AuthorAgent string
	ParentID    string
	Kind        string
	Status      string
	Body        string
}

// SetStateRequest is the input to SetStateColumns. SPEC §4.4.
type SetStateRequest struct {
	ItemID        string
	ImplState     *string
	ReviewState   *string
	QAState       *string
	PipelineState *string
}

// CloseRequest is the input to Close. SPEC §4.4.
type CloseRequest struct {
	ItemID string
	Reason string
}

// ClaimRequest is the input to Claim. SPEC §4.4.
type ClaimRequest struct {
	ItemID        string
	ClaimerUserID string
	ClaimerAgent  string
}

// ListRequest is the input to List. SPEC §4.4.
type ListRequest struct {
	OrgID         string
	ProjectID     string
	MilestoneID   string
	Status        []string
	PipelineStage []string
	ClaimedBy     string
	Labels        []string
	Limit         int
	Cursor        string
}

// ListResponse is the output of List. SPEC §4.4.
type ListResponse struct {
	Items      []Item
	NextCursor string
}

// ReadyRequest is the input to Ready. SPEC §6.2 Tool 2 (lines 1177-1206).
//
// Empty ProjectID means org-wide scope (caller has no "primary project"
// concept in P01 — see SPEC §6.2 line 1161 "defaults to caller's
// primary project"; until a primary-project column ships, the MCP
// layer collapses missing project_id to org-wide). PriorityMin is the
// lowest priority included in results, "P0".."P4" lexicographic
// (P0 highest, P4 lowest); empty string = no filter.
type ReadyRequest struct {
	OrgID       string
	ProjectID   string
	Limit       int
	PriorityMin string
}

// ReadyResponse is the output of Ready. SPEC §6.2 Tool 2.
//
// TotalReady is the count of ready items across the same scope (no
// pagination at v1.0 — review L6-W7) so the caller can decide whether
// more exist behind the Limit cap.
type ReadyResponse struct {
	Items      []Item
	TotalReady int
}

// SearchRequest is the input to Search. SPEC §4.4.
type SearchRequest struct {
	OrgID     string
	ProjectID string
	Query     string
	Limit     int
}

// SearchHit is one row of a Search response. SPEC §4.4.
type SearchHit struct {
	ItemID    string
	Source    string
	CommentID string
	Rank      float64
	Snippet   string
}

// SearchResponse is the output of Search. SPEC §4.4.
type SearchResponse struct {
	Hits []SearchHit
}

// Milestone is the canonical milestone row shape. SPEC §4.4.1.
type Milestone struct {
	ID                string
	ParentMilestoneID string
	OrgID             string
	ProjectID         string
	Name              string
	Description       string
	StartDate         string
	EndDate           string
	CancelledAt       *time.Time
	CancelledReason   string
	CreatedAt         time.Time
	UpdatedAt         time.Time
}

// CreateMilestoneRequest is the input to CreateMilestone. SPEC §4.4.1.
type CreateMilestoneRequest struct {
	OrgID             string
	ProjectID         string
	ParentMilestoneID string
	Name              string
	Description       string
	StartDate         string
	EndDate           string
}

// UpdateMilestoneRequest is the input to UpdateMilestone. SPEC §4.4.1.
type UpdateMilestoneRequest struct {
	MilestoneID     string
	Name            *string
	Description     *string
	StartDate       *string
	EndDate         *string
	CancelledAt     *time.Time
	CancelledReason *string
}

// AssignItemRequest is the input to AssignItem. SPEC §4.4.1.
type AssignItemRequest struct {
	ItemID         string
	MilestoneID    string
	AssignedByUser string
}

// MilestoneTreeRequest is the input to MilestoneTree. SPEC §4.4.1.
type MilestoneTreeRequest struct {
	OrgID            string
	ProjectID        string
	RootMilestoneID  string
	IncludeCancelled bool
}

// MilestoneNode is one node in the recursive milestone tree response.
// SPEC §4.4.1.
type MilestoneNode struct {
	Milestone Milestone
	Depth     int
	Children  []MilestoneNode
}

// MilestoneTreeResponse is the output of MilestoneTree. SPEC §4.4.1.
//
// (The spec names the type `MilestoneTree` but Go disallows a function
// and a top-level type sharing one name in the same package. The RPC
// route, signature shape, and JSON-on-the-wire encoding are unchanged —
// Encore serialises by field, not by type name. See DECISION trail on
// bead unblock-tv8.1.)
type MilestoneTreeResponse struct {
	Roots []MilestoneNode
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
)

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

	// Initial status / pipeline_stage are defaults. is_ready starts false
	// (cascade subscriber recomputes; for items with no incoming
	// 'blocks' edges deps.AddEdge / subscriber will flip to true).
	// pipeline_state defaults to 'running'.
	_, err = tx.Exec(ctx,
		`INSERT INTO workitems.items
		   (id, org_id, project_id, milestone_id, parent_id, discovered_from_id,
		    type, title, body, priority,
		    severity, kind_of_finding)
		 VALUES ($1, $2, NULLIF($3, ''), NULLIF($4, ''), NULLIF($5, ''), NULLIF($6, ''),
		         $7, $8, $9, $10,
		         NULLIF($11, ''), NULLIF($12, ''))`,
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

	// Attach labels in the same transaction.
	if len(req.Labels) > 0 {
		if err := attachLabelsTx(ctx, tx, id, req.Labels); err != nil {
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
		_, postCommit, err := deps.AddEdgeInTx(ctx, tx, &deps.AddEdgeRequest{
			OrgID:     req.OrgID,
			ProjectID: req.ProjectID,
			FromItem:  edge.FromItem,
			ToItem:    id,
			Kind:      kind,
		})
		if err != nil {
			return nil, err
		}
		postCommits = append(postCommits, postCommit)
	}

	if err := tx.Commit(); err != nil {
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
	_, err = tx.Exec(ctx,
		`UPDATE workitems.items
		    SET title       = COALESCE($2, title),
		        body        = COALESCE($3, body),
		        priority    = COALESCE($4, priority),
		        milestone_id = CASE
		                         WHEN $5::boolean THEN NULLIF($6, '')
		                         ELSE milestone_id
		                       END,
		        updated_at  = now()
		  WHERE id = $1`,
		req.ItemID, req.Title, req.Body, req.Priority,
		req.MilestoneID != nil, derefString(req.MilestoneID),
	)
	if err != nil {
		if isForeignKeyViolation(err) {
			return nil, &errs.Error{Code: errs.NotFound, Message: "referenced milestone does not exist"}
		}
		rlog.Error("workitems: update failed", "err", err, "item_id", req.ItemID)
		return nil, &errs.Error{Code: errs.Internal, Message: "update failed"}
	}

	// Labels: full-replace when the pointer is set (empty slice = clear).
	if req.Labels != nil {
		if _, err := tx.Exec(ctx, `DELETE FROM workitems.item_labels WHERE item_id = $1`, req.ItemID); err != nil {
			return nil, &errs.Error{Code: errs.Internal, Message: "label clear failed"}
		}
		if len(*req.Labels) > 0 {
			if err := attachLabelsTx(ctx, tx, req.ItemID, *req.Labels); err != nil {
				return nil, err
			}
		}
	}

	if err := tx.Commit(); err != nil {
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

// GetTrail returns the item + its comments + incoming/outgoing edges +
// findings. SPEC §4.4.
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

	// DependenciesIn (edges where to_item = item.id) and
	// DependenciesOut (edges where from_item = item.id).
	in, err := readEdges(ctx, "to_item", req.ItemID)
	if err != nil {
		return nil, err
	}
	out, err := readEdges(ctx, "from_item", req.ItemID)
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

	_, err = db.Exec(ctx,
		`INSERT INTO workitems.comments
		   (id, item_id, parent_id, author_id, author_agent, kind, status, body)
		 VALUES ($1, $2, NULLIF($3, ''), NULLIF($4, ''), NULLIF($5, ''), $6, $7, $8)`,
		id, req.ItemID, req.ParentID, req.AuthorID, req.AuthorAgent,
		kind, status, req.Body,
	)
	if err != nil {
		if isForeignKeyViolation(err) {
			return nil, &errs.Error{Code: errs.NotFound, Message: "item or parent comment does not exist"}
		}
		rlog.Error("workitems: append comment failed", "err", err, "item_id", req.ItemID)
		return nil, &errs.Error{Code: errs.Internal, Message: "append comment failed"}
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
//encore:api private method=POST path=/workitems.SetStateColumns
func SetStateColumns(ctx context.Context, req *SetStateRequest) (*Item, error) {
	if req == nil || req.ItemID == "" {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing item_id"}
	}
	if err := validateStateEnums(req); err != nil {
		return nil, err
	}

	tx, err := db.Begin(ctx)
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "db begin failed"}
	}
	defer func() { _ = tx.Rollback() }()

	// Lock the row and read current state columns.
	var cur stateRow
	err = tx.QueryRow(ctx,
		`SELECT impl_state, review_state, qa_state, pipeline_state, claimed_by_id
		   FROM workitems.items
		  WHERE id = $1
		  FOR UPDATE`,
		req.ItemID,
	).Scan(&cur.Impl, &cur.Review, &cur.QA, &cur.Pipeline, &cur.ClaimedBy)
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
		if !(reqReviewIsNeedsRework || reqQAIsFailed || currentQAFailedAndUnchanged) {
			return nil, preconditionError("impl_done_to_pending_requires_rework_path",
				"impl_state=done → pending requires review_state=needs_rework or qa_state=failed")
		}
	}

	// I-4: review_state ∈ (approved, needs_rework) requires impl_state=done.
	if (newReview == reviewApproved || newReview == reviewNeedsRework) && newImpl != implDone {
		return nil, preconditionError("review_change_requires_impl_done", "review_state change requires impl_state=done")
	}

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
		return nil, &errs.Error{Code: errs.Internal, Message: "set_state commit failed"}
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
	var orgID, projectID string
	var claimedBy *string
	var currentStatus string
	err = tx.QueryRow(ctx,
		`SELECT org_id, COALESCE(project_id, ''), claimed_by_id, status
		   FROM workitems.items
		  WHERE id = $1
		  FOR UPDATE`,
		req.ItemID,
	).Scan(&orgID, &projectID, &claimedBy, &currentStatus)
	if err != nil {
		if errors.Is(err, sqldb.ErrNoRows) {
			return nil, &errs.Error{Code: errs.NotFound, Message: "item not found"}
		}
		return nil, &errs.Error{Code: errs.Internal, Message: "close read failed"}
	}
	if claimedBy == nil || *claimedBy == "" {
		// AF3 defensive check.
		return nil, preconditionError("claimed_by_id_required", "close requires claimed_by_id IS NOT NULL")
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

// Claim performs the SPEC §6.4 atomic claim transaction. On loser
// path returns errs.AlreadyExists with Meta carrying winner info.
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

	// SELECT FOR UPDATE: only succeeds if status='Ready' AND not yet
	// claimed. Zero rows means the loser path. We also project org_id
	// and project_id here so the I-3-path cascade publish (post-commit,
	// below) has the scope fields it needs without a second read.
	var lockedID, orgID, projectID, qaState string
	err = tx.QueryRow(ctx,
		`SELECT id, org_id, COALESCE(project_id, ''), qa_state
		   FROM workitems.items
		  WHERE id = $1 AND status = 'Ready' AND claimed_by_id IS NULL
		  FOR UPDATE`,
		req.ItemID,
	).Scan(&lockedID, &orgID, &projectID, &qaState)
	if err != nil {
		if errors.Is(err, sqldb.ErrNoRows) {
			// Loser path — fetch winner info.
			return nil, alreadyClaimedError(ctx, req.ItemID)
		}
		return nil, &errs.Error{Code: errs.Internal, Message: "claim lock failed"}
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
			// We fetched limit+1 to know if a next page exists. The
			// extra row's id becomes the next cursor.
			nextCursor = r.ID
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
// Spec: limit 1..50; default 10 (lines 1183-1184).
const (
	readyDefaultLimit = 10
	readyMaxLimit     = 50
)

// Ready returns the ready set for the §6.2 Tool 2 MCP `ready` tool.
// Priority comparison is lexicographic on the literal "P0".."P4"
// strings — P0 is highest, P4 lowest — so priority_min = "P3"
// means "include P0..P3" (priority <= 'P3' on the SQL side).
//
// Filters: org_id (required), project_id (optional — empty = org-wide
// scope), priority_min (optional — "P0".."P4" lexicographic). Ordering
// is deterministic on (priority asc, created_at asc, id asc) and
// covered by items_ready_partial_idx (migration 0040 + 0100 — see the
// 0100 file header for the index extension that lets the planner serve
// the ORDER BY from a pure index scan).
//
// total_ready counts every ready item in the same scope so the caller
// can detect overflow past `limit` without paginating (v1.0 has NO
// pagination on this endpoint — review L6-W7).
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

	// MCP layer is the authoritative caller of this RPC. We do NOT
	// trust req.OrgID at face value — pin it to identity.OrgID so a
	// confused upstream cannot leak across orgs. (Same posture as
	// workitems.List which uses rbac.For for the same guarantee.)
	if req.OrgID == "" {
		req.OrgID = identity.OrgID
	}
	if req.OrgID != identity.OrgID {
		return nil, &errs.Error{
			Code:    errs.PermissionDenied,
			Message: "cross-org ready read is not allowed",
			Meta:    errs.Metadata{"field": "org_id"},
		}
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

	// Hot-path read against items_ready_partial_idx. The partial
	// predicate (is_ready = true AND status = 'Ready' AND closed_at
	// IS NULL) MUST match the index definition verbatim — drift here
	// would force the planner off the index. After migration 0100 the
	// index columns are (org_id, project_id, priority, created_at, id)
	// so the ORDER BY below serves entirely from the index. Empty
	// project_id ($2 = '') skips the project filter for org-wide
	// scope per the §6.2 Tool 1/2 "primary project" P01 contract.
	rows, err := db.Query(ctx,
		`SELECT `+itemColumnList+`
		   FROM workitems.items
		  WHERE org_id = $1
		    AND is_ready = true
		    AND status = 'Ready'
		    AND closed_at IS NULL
		    AND ($2 = '' OR project_id = $2)
		    AND ($3 = '' OR priority <= $3)
		  ORDER BY priority ASC, created_at ASC, id ASC
		  LIMIT $4`,
		req.OrgID, req.ProjectID, priorityMin, limit,
	)
	if err != nil {
		rlog.Error("workitems: ready query failed", "err", err, "org_id", req.OrgID)
		return nil, &errs.Error{Code: errs.Internal, Message: "ready query failed"}
	}
	defer rows.Close()

	out := make([]Item, 0, limit)
	for rows.Next() {
		var r itemRow
		if err := scanItemRow(rows, &r); err != nil {
			return nil, &errs.Error{Code: errs.Internal, Message: "ready scan failed"}
		}
		item, err := itemFromRow(ctx, r)
		if err != nil {
			return nil, err
		}
		out = append(out, *item)
	}
	if err := rows.Err(); err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "ready iter failed"}
	}

	// Second query for the total count across the same predicate. The
	// partial index serves this as an index-only scan with no rows
	// returned. We intentionally do NOT inline this as a window
	// function on the first query — that would force the planner off
	// the partial index for the LIMIT path.
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
		req.OrgID, req.ProjectID, priorityMin,
	).Scan(&totalReady); err != nil {
		rlog.Error("workitems: ready count failed", "err", err, "org_id", req.OrgID)
		return nil, &errs.Error{Code: errs.Internal, Message: "ready count failed"}
	}

	return &ReadyResponse{Items: out, TotalReady: totalReady}, nil
}

// Search performs multi-table FTS (UNION ALL over items_fts_idx and
// comments_fts_idx) per SPEC §4.4 + AF1. Query uses websearch_to_tsquery.
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
		limit = searchMaxLimit
	}

	// Project filter is enforced inside the SQL when set; org_id is
	// always the scope gate. We use direct SQL here (rather than
	// rbac.For) because the query shape is a UNION ALL across two
	// tables — the rbac builder is single-table-only.
	args := []any{identity.OrgID, req.Query, limit}
	projectFilter := ""
	if req.ProjectID != "" {
		projectFilter = ` AND project_id = $4`
		args = append(args, req.ProjectID)
	}

	// SPEC §10.1 forbids runtime-constructed SQL clauses where a
	// tenant gate is involved. The org_id predicate IS the gate and
	// is parameterised at $1 — the only string concatenation below
	// is the static projectFilter ON/OFF, which carries no
	// user-controlled value (req.ProjectID flows through $4).
	sql := `SELECT id            AS item_id,
	               'item'        AS source,
	               ''            AS comment_id,
	               ts_rank_cd(fts, websearch_to_tsquery('english', $2))::float8 AS rank,
	               ts_headline('english', body, websearch_to_tsquery('english', $2),
	                           'MaxFragments=1,MaxWords=20,MinWords=5')  AS snippet
	          FROM workitems.items
	         WHERE org_id = $1` + projectFilter + `
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
	         WHERE i.org_id = $1` + strings.ReplaceAll(projectFilter, "project_id", "i.project_id") + `
	           AND c.fts @@ websearch_to_tsquery('english', $2)
	         ORDER BY rank DESC
	         LIMIT $3`

	rows, err := db.Query(ctx, sql, args...)
	if err != nil {
		rlog.Error("workitems: search failed", "err", err)
		return nil, &errs.Error{Code: errs.Internal, Message: "search failed"}
	}
	defer rows.Close()

	var hits []SearchHit
	for rows.Next() {
		var h SearchHit
		if err := rows.Scan(&h.ItemID, &h.Source, &h.CommentID, &h.Rank, &h.Snippet); err != nil {
			return nil, &errs.Error{Code: errs.Internal, Message: "search scan failed"}
		}
		// Trim snippet to 200 chars (SPEC §4.4 line 793 cap).
		if len(h.Snippet) > 200 {
			h.Snippet = h.Snippet[:200]
		}
		hits = append(hits, h)
	}
	if err := rows.Err(); err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "search iter failed"}
	}
	return &SearchResponse{Hits: hits}, nil
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
		var parentOrg, parentProject *string
		var parentStart, parentEnd time.Time
		err := tx.QueryRow(ctx,
			`SELECT org_id, project_id, start_date, end_date
			   FROM workitems.milestones
			  WHERE id = $1
			  FOR UPDATE`,
			req.ParentMilestoneID,
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

	_, err = tx.Exec(ctx,
		`INSERT INTO workitems.milestones
		   (id, parent_milestone_id, org_id, project_id, name, description, start_date, end_date)
		 VALUES ($1, NULLIF($2, ''), NULLIF($3, ''), NULLIF($4, ''), $5, $6, $7, $8)`,
		id, req.ParentMilestoneID, req.OrgID, req.ProjectID, name, req.Description, req.StartDate, req.EndDate,
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

	if err := tx.Commit(); err != nil {
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
	var parentID *string
	var curStart, curEnd time.Time
	err = tx.QueryRow(ctx,
		`SELECT parent_milestone_id, start_date, end_date
		   FROM workitems.milestones
		  WHERE id = $1
		  FOR UPDATE`,
		req.MilestoneID,
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
		return nil, &errs.Error{Code: errs.Internal, Message: "milestone update commit failed"}
	}
	return readMilestone(ctx, req.MilestoneID)
}

// AssignItem sets / clears the item's milestone_id atomically. M-INV-7
// enforcement: the target milestone's scope must be reachable from the
// item's project (same project OR org-wide milestone in the same org).
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
		_, err := tx.Exec(ctx,
			`UPDATE workitems.items
			    SET milestone_id          = NULL,
			        milestone_assigned_at = NULL,
			        milestone_assigned_by = NULL,
			        updated_at            = now()
			  WHERE id = $1`,
			req.ItemID,
		)
		if err != nil {
			return &errs.Error{Code: errs.Internal, Message: "milestone unassign failed"}
		}
	} else {
		// M-INV-7: scope reachability check.
		var itemOrg, itemProject *string
		err = tx.QueryRow(ctx,
			`SELECT org_id, project_id FROM workitems.items WHERE id = $1`,
			req.ItemID,
		).Scan(&itemOrg, &itemProject)
		if err != nil {
			if errors.Is(err, sqldb.ErrNoRows) {
				return &errs.Error{Code: errs.NotFound, Message: "item not found"}
			}
			return &errs.Error{Code: errs.Internal, Message: "item read failed"}
		}
		var msOrg, msProject *string
		err = tx.QueryRow(ctx,
			`SELECT org_id, project_id FROM workitems.milestones WHERE id = $1`,
			req.MilestoneID,
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

		_, err = tx.Exec(ctx,
			`UPDATE workitems.items
			    SET milestone_id          = $2,
			        milestone_assigned_at = now(),
			        milestone_assigned_by = NULLIF($3, ''),
			        updated_at            = now()
			  WHERE id = $1`,
			req.ItemID, req.MilestoneID, req.AssignedByUser,
		)
		if err != nil {
			if isForeignKeyViolation(err) {
				return &errs.Error{Code: errs.NotFound, Message: "milestone or assignee does not exist"}
			}
			return &errs.Error{Code: errs.Internal, Message: "milestone assign failed"}
		}
	}

	if err := tx.Commit(); err != nil {
		return &errs.Error{Code: errs.Internal, Message: "milestone assign commit failed"}
	}
	return nil
}

// MilestoneTree returns the recursive tree of milestones rooted at
// RootMilestoneID OR all roots within (OrgID, ProjectID). SPEC §4.4.1 +
// §9.4.9 (depth-bounded by M-INV-6).
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
		rows, err = db.Query(ctx,
			`WITH RECURSIVE tree(id, parent_milestone_id, org_id, project_id, name, description,
			                     start_date, end_date, cancelled_at, cancelled_reason,
			                     created_at, updated_at, depth) AS (
			       SELECT id, parent_milestone_id, org_id, project_id, name, description,
			              start_date, end_date, cancelled_at, cancelled_reason,
			              created_at, updated_at, 0
			         FROM workitems.milestones
			        WHERE id = $1
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
			req.RootMilestoneID, milestoneMaxDepth-1, req.IncludeCancelled,
		)
	default:
		// Walk from all roots in the scope. Roots are milestones whose
		// parent_milestone_id IS NULL within (org_id, project_id).
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
			req.OrgID, req.ProjectID, milestoneMaxDepth-1, req.IncludeCancelled,
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
