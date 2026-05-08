// Package deps owns the deps schema and the cycle-detection +
// cascade-publishing primitives. See SPEC §4.5 for the full RPC surface
// and §6.5 for the cycle-detection CTE.
//
// In P01 task A-1 this package only declares the //encore:api skeletons so
// Encore recognises deps as a service. Bodies return errNotImplemented;
// real wiring (sqldb.Named("unblock"), cycle CTE, advisory locks, cascade
// Pub/Sub publisher) lands in C-1, C-2, C-3.
package deps

import (
	"context"
	"errors"
	"time"
)

// errNotImplemented is the sentinel returned by every P01 A-1 skeleton body.
var errNotImplemented = errors.New("deps: not implemented in P01 A-1 skeleton")

// Edge is the canonical dependency edge row shape. SPEC §4.5.
type Edge struct {
	ID        string
	FromItem  string
	ToItem    string
	Kind      string // "blocks" | "related"
	CreatedAt time.Time
	CreatedBy string
}

// AddEdgeRequest is the input to AddEdge. SPEC §4.5.
type AddEdgeRequest struct {
	OrgID     string
	ProjectID string
	FromItem  string
	ToItem    string
	Kind      string // "blocks" | "related"; default "blocks"
}

// AddEdge acquires the per-project advisory lock (AF5), runs the
// depth-counter reachability CTE (C5), inserts the edge, and emits
// deps.cascade.requested when the to_item's readiness flips. SPEC §4.5.
//
//encore:api private method=POST path=/deps.AddEdge
func AddEdge(ctx context.Context, req *AddEdgeRequest) (*Edge, error) {
	return nil, errNotImplemented
}

// RemoveEdgeRequest is the input to RemoveEdge. SPEC §4.5. Pass either
// EdgeID OR (FromItem + ToItem + Kind), exactly one path.
type RemoveEdgeRequest struct {
	EdgeID   string
	FromItem string
	ToItem   string
	Kind     string
}

// RemoveEdgeResponse is the output of RemoveEdge. SPEC §4.5.
type RemoveEdgeResponse struct {
	Removed        bool
	ToItemNowReady bool
	ToItemID       string
}

// RemoveEdge deletes the edge, sync-inline recomputes is_ready for the
// to_item, and writes a kind='edge_removed' cascade_events audit row in
// the same transaction. Does NOT publish a Pub/Sub event. SPEC §4.5 / §6.2
// Tool 12.
//
//encore:api private method=POST path=/deps.RemoveEdge
func RemoveEdge(ctx context.Context, req *RemoveEdgeRequest) (*RemoveEdgeResponse, error) {
	return nil, errNotImplemented
}

// IsReadyRequest is the input to IsReady. SPEC §4.5 wrote the signature as
// `func IsReady(ctx, itemID string) (bool, error)`, but Encore requires
// API request types to be named structs (E1354). This skeleton wraps
// itemID in a request struct; the wire-shape (single item lookup) is
// unchanged. See DEVIATION trail on bead unblock-tv8.1.
type IsReadyRequest struct {
	ItemID string
}

// IsReadyResponse is the output of IsReady. Encore also requires API
// response types to be named structs (a bare `bool` is rejected).
type IsReadyResponse struct {
	IsReady bool
}

// IsReady is a read-side helper that returns the current is_ready value
// (read directly from workitems.items, not recomputed). SPEC §4.5.
//
//encore:api private method=POST path=/deps.IsReady
func IsReady(ctx context.Context, req *IsReadyRequest) (*IsReadyResponse, error) {
	return nil, errNotImplemented
}

// ClosureRequest is the input to Closure. SPEC §4.5.
type ClosureRequest struct {
	ItemID    string
	Direction string // "incoming" | "outgoing"
	MaxDepth  int    // 1..256; default 256
}

// ClosureResponse is the output of Closure. SPEC §4.5.
type ClosureResponse struct {
	ItemIDs []string
}

// Closure returns the transitive 'blocks' closure for an item. SPEC §4.5.
//
//encore:api private method=POST path=/deps.Closure
func Closure(ctx context.Context, req *ClosureRequest) (*ClosureResponse, error) {
	return nil, errNotImplemented
}

// RecentCascadeEventsRequest is the input to RecentCascadeEvents. SPEC §4.5.
type RecentCascadeEventsRequest struct {
	OrgID     string
	ProjectID string
	Limit     int // capped at 50; default 50
}

// CascadeEventRow is one row of a RecentCascadeEvents response. SPEC §4.5.
type CascadeEventRow struct {
	ID                string
	EventID           string
	TriggeredByItemID string
	AffectedItemIDs   []string
	CascadedCount     int
	TriggeredAt       time.Time
	TraceID           string
}

// RecentCascadeEventsResponse is the output of RecentCascadeEvents. SPEC §4.5.
type RecentCascadeEventsResponse struct {
	Events []CascadeEventRow
}

// RecentCascadeEvents returns the last 50 deps.cascade_events rows for the
// org/project, ordered by triggered_at DESC. AF2 closure — used by the
// prime MCP tool. SPEC §4.5.
//
//encore:api private method=POST path=/deps.RecentCascadeEvents
func RecentCascadeEvents(ctx context.Context, req *RecentCascadeEventsRequest) (*RecentCascadeEventsResponse, error) {
	return nil, errNotImplemented
}
