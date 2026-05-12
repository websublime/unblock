// Package workitems owns the workitems schema (items, comments, labels,
// milestones) and exposes the private RPCs called by MCP tool handlers.
// See SPEC §4.4 for the full RPC surface.
//
// In P01 task A-1 this package only declares the //encore:api skeletons so
// Encore recognises workitems as a service. Bodies return errNotImplemented;
// real wiring lands in B-1, B-2, and following: bodies, FTS, milestones,
// claim transaction, state-machine invariants. The DB handle MUST follow
// the canonical BindDB late-bind pattern (a nil *sqldb.Database pointer +
// exported BindDB hook in db.go, registered in apps/api/db/db.go's init —
// see apps/api/db/db.go's CONSUMER PATTERN). Direct
// `sqldb.Named("unblock")` at package init is forbidden (panics outside
// the encore CLI in encore.dev v1.52.1).
package workitems

import (
	"context"
	"errors"
	"time"
)

// errNotImplemented is the sentinel returned by every P01 A-1 skeleton body.
var errNotImplemented = errors.New("workitems: not implemented in P01 A-1 skeleton")

// Edge mirrors deps.Edge for the create-with-deps path. SPEC §4.4 / §4.5.
// Duplicated locally (rather than imported from deps) to keep the
// workitems → deps direction unidirectional in skeleton form; the real
// Create handler in B-1 calls deps.AddEdge with the deps.Edge type.
type Edge struct {
	BlockerItemID string // from_item: must complete first
	Kind          string // "blocks" | "related"; default "blocks"
}

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
	Dependencies     []Edge
	Severity         string
	KindOfFinding    string
}

//encore:api private method=POST path=/workitems.Create
func Create(ctx context.Context, req *CreateRequest) (*Item, error) {
	return nil, errNotImplemented
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

//encore:api private method=POST path=/workitems.Update
func Update(ctx context.Context, req *UpdateRequest) (*Item, error) {
	return nil, errNotImplemented
}

//encore:api private method=GET path=/workitems.Get/:id
func Get(ctx context.Context, id string) (*Item, error) {
	return nil, errNotImplemented
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

// GetTrailRequest is the input to GetTrail. SPEC §4.4.
type GetTrailRequest struct {
	ItemID string
}

// Trail is the comment + edges + findings bundle returned by GetTrail.
// SPEC §4.4. DependenciesIn / DependenciesOut use the workitems.Edge type
// in skeleton form; B-1 will widen to deps.Edge if richer fields are needed.
type Trail struct {
	Item            *Item
	Comments        []Comment
	DependenciesIn  []Edge
	DependenciesOut []Edge
	Findings        []Item
}

//encore:api private method=POST path=/workitems.GetTrail
func GetTrail(ctx context.Context, req *GetTrailRequest) (*Trail, error) {
	return nil, errNotImplemented
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

//encore:api private method=POST path=/workitems.AppendComment
func AppendComment(ctx context.Context, req *AppendCommentRequest) (*Comment, error) {
	return nil, errNotImplemented
}

// SetStateRequest is the input to SetStateColumns. SPEC §4.4.
type SetStateRequest struct {
	ItemID        string
	ImplState     *string
	ReviewState   *string
	QAState       *string
	PipelineState *string
}

// SetStateColumns writes one or more of (impl_state, review_state,
// qa_state, pipeline_state) and recomputes pipeline_stage. Enforces the
// five PRD §6.2 state-machine invariants in app code (round-2 D2). See
// SPEC §4.4 / §6.2 Tool 13.
//
//encore:api private method=POST path=/workitems.SetStateColumns
func SetStateColumns(ctx context.Context, req *SetStateRequest) (*Item, error) {
	return nil, errNotImplemented
}

// CloseRequest is the input to Close. SPEC §4.4.
type CloseRequest struct {
	ItemID string
	Reason string
}

// Close sets status=Done, closed_at=now(), emits deps.cascade.requested.
// MCP-layer precondition (AF3): rejects if claimed_by_id IS NULL.
//
//encore:api private method=POST path=/workitems.Close
func Close(ctx context.Context, req *CloseRequest) (*Item, error) {
	return nil, errNotImplemented
}

// ClaimRequest is the input to Claim. SPEC §4.4.
type ClaimRequest struct {
	ItemID        string
	ClaimerUserID string
	ClaimerAgent  string
}

// Claim performs the atomic claim transaction (SELECT FOR UPDATE) per
// SPEC §5.5. Resets review_state and qa_state to "pending" when the item
// being claimed has qa_state="failed" at lock time (round-2 D2 / PRD §6.2
// invariant #3). See SPEC §4.4.
//
//encore:api private method=POST path=/workitems.Claim
func Claim(ctx context.Context, req *ClaimRequest) (*Item, error) {
	return nil, errNotImplemented
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

//encore:api private method=POST path=/workitems.List
func List(ctx context.Context, req *ListRequest) (*ListResponse, error) {
	return nil, errNotImplemented
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

// Search performs multi-table FTS per AF1 (UNION ALL over items_fts_idx
// and comments_fts_idx). SPEC §4.4.
//
//encore:api private method=POST path=/workitems.Search
func Search(ctx context.Context, req *SearchRequest) (*SearchResponse, error) {
	return nil, errNotImplemented
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

// CreateMilestone enforces M-INV-1, M-INV-2, M-INV-3, M-INV-5, M-INV-6 in
// app code. SPEC §4.4.1 / round-2 D1.
//
//encore:api private method=POST path=/workitems.CreateMilestone
func CreateMilestone(ctx context.Context, req *CreateMilestoneRequest) (*Milestone, error) {
	return nil, errNotImplemented
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

//encore:api private method=POST path=/workitems.UpdateMilestone
func UpdateMilestone(ctx context.Context, req *UpdateMilestoneRequest) (*Milestone, error) {
	return nil, errNotImplemented
}

// AssignItemRequest is the input to AssignItem. SPEC §4.4.1.
type AssignItemRequest struct {
	ItemID         string
	MilestoneID    string
	AssignedByUser string
}

// AssignItem sets workitems.items.milestone_id atomically. Enforces
// M-INV-7. SPEC §4.4.1.
//
//encore:api private method=POST path=/workitems.AssignItem
func AssignItem(ctx context.Context, req *AssignItemRequest) error {
	return errNotImplemented
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

//encore:api private method=POST path=/workitems.MilestoneTree
func MilestoneTree(ctx context.Context, req *MilestoneTreeRequest) (*MilestoneTreeResponse, error) {
	return nil, errNotImplemented
}
