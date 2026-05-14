// Package deps owns the deps schema and the cycle-detection +
// cascade-publishing primitives. See SPEC §4.5 for the full RPC surface
// and §6.5 for the cycle-detection CTE.
//
// In P01 task C-2 (bead unblock-tv8.11) this package lands the bodies
// of the five //encore:api endpoints declared in A-1 (AddEdge,
// RemoveEdge, IsReady, Closure, RecentCascadeEvents) plus the shared
// inline helper recomputeReady. Round-6 cascade-symmetry (SPEC §6.3.0)
// drives the writer split: is_ready is Regime A (single-hop, writer-
// inline) and pipeline_stage is Regime B (multi-hop, subscriber-only).
// Every Regime-B-affecting write here publishes CascadeRequested with
// the matching Reason after the transaction commits.
//
// Database wiring follows the canonical BindDB late-bind pattern (see
// db.go) — a nil *sqldb.Database pointer populated by the dedicated
// apps/api/db/ service's init via deps.BindDB(DB). RPC bodies read
// `db` directly after process bootstrap.
//
// Authorisation: deps RPCs are private and called from the MCP tool
// layer (D-5 / unblock-tv8.20) which gates via org.Authorize at the
// session→org boundary BEFORE dispatching here. The deps RPCs trust
// that gate the same way workitems write-side RPCs do (see
// workitems.go file header for the layered model). RecentCascadeEvents
// scopes by explicit (org_id, project_id) inputs so the SQL filter is
// explicit on every read.
package deps

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"time"

	"encore.app/auth"
	"encore.app/shared/tracectx"
	encoreauth "encore.dev/beta/auth"
	"encore.dev/beta/errs"
	"encore.dev/rlog"
	"encore.dev/storage/sqldb"
)

// allowedEdgeKinds is the closed set of edge kinds accepted by AddEdge.
// The deps.dependencies CHECK constraint enforces the same set; we
// reject early so the caller gets a structured InvalidArgument instead
// of a CHECK violation under errs.Internal.
var allowedEdgeKinds = map[string]struct{}{
	"blocks":  {},
	"related": {},
}

// allowedClosureDirections is the closed set of Closure directions.
var allowedClosureDirections = map[string]struct{}{
	"incoming": {},
	"outgoing": {},
}

// closureMaxDepth caps both the Closure RPC's depth and the cycle CTE
// (AR-8). 256 is the v1.0 product constraint (SPEC §6.5 / RP01-3).
const closureMaxDepth = 256

// recentCascadeEventsLimitCap caps the RecentCascadeEvents response
// size per AF2 / SPEC §4.5 line 1015.
const recentCascadeEventsLimitCap = 50

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
// depth-counter reachability CTE (C5) extended to project the cycle
// path, inserts the edge, recomputes is_ready for to_item inline
// (Regime A — single-hop), and publishes CascadeRequested with
// Reason="edge_added" post-commit (Regime B — multi-hop pipeline_stage
// recompute on the forward closure). SPEC §4.5 + §6.5 + §6.3.0.
//
// Cross-project edges are rejected with InvalidArgument
// Meta{"field":"to_item_id"} per §6.2 Tool 11 (round-6 L6-W8).
//
// Cycle violations return FailedPrecondition Meta{"kind":"CYCLE_DETECTED",
// "from","to","cycle_path"} — the MCP layer (D-5) translates this Meta
// payload into the JSON-RPC error envelope's data field per §7.
//
//encore:api private method=POST path=/deps.AddEdge
func AddEdge(ctx context.Context, req *AddEdgeRequest) (*Edge, error) {
	tx, err := db.Begin(ctx)
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "db begin failed"}
	}
	defer func() { _ = tx.Rollback() }()

	edge, postCommit, err := AddEdgeInTx(ctx, tx, req)
	if err != nil {
		return nil, err
	}

	if err := tx.Commit(); err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "commit failed"}
	}

	postCommit(ctx)
	return edge, nil
}

// AddEdgeInTxPostCommit is the post-commit work the caller of
// AddEdgeInTx MUST invoke AFTER successfully committing the caller's
// transaction. It performs the §6.3.0 Regime-B publish on
// `deps.cascade.requested` so the multi-hop pipeline_stage recompute
// fires on the forward closure from to_item.
//
// The function takes ctx so the publisher can pull trace_id from
// tracectx (Encore Pub/Sub does not carry ctx across the topic
// boundary; the publisher copies TraceID into the message payload
// explicitly). Best-effort: a publish failure does NOT roll back the
// edge — the transaction is already committed by the time this is
// invoked.
type AddEdgeInTxPostCommit func(ctx context.Context)

// AddEdgeInTx runs the §4.5 AddEdge body inside the caller-provided
// transaction. The caller is responsible for tx.Begin / tx.Commit /
// tx.Rollback and for invoking the returned post-commit hook
// (CascadeRequested publish) AFTER a successful commit.
//
// Use this helper when multiple writes must be atomic with the edge
// insert — for example, workitems.Create's combined item+labels+edges
// transaction (orchestrator DECISION on bead unblock-tv8.17 / D-2,
// 2026-05-14, decision #1).
//
// Standalone callers should invoke the AddEdge //encore:api endpoint
// which wraps this helper in its own transaction and is on the public
// RPC surface.
//
// Concurrency: this helper acquires the §6.5 per-project advisory
// lock via pg_advisory_xact_lock. The caller's transaction holds the
// lock for the rest of the tx's lifetime — keep the tx short.
//
// Errors mirror AddEdge's contract verbatim:
//
//   - InvalidArgument for missing/invalid fields, self-loop, bad
//     kind, cross-org or cross-project endpoints.
//   - NotFound when either endpoint is missing.
//   - AlreadyExists on duplicate (from, to, kind) (UNIQUE
//     dependencies_pair_uniq).
//   - FailedPrecondition with Meta.kind="CYCLE_DETECTED" + cycle_path
//     when the edge would close a cycle. The cycle forensic row in
//     deps.cycles is written via a SEPARATE top-level statement BEFORE
//     this helper returns, so it survives the caller's tx.Rollback —
//     same contract as AddEdge's standalone path.
//   - Internal for any unexpected DB error.
//
// SPEC: §4.5 + §6.5.
func AddEdgeInTx(ctx context.Context, tx *sqldb.Tx, req *AddEdgeRequest) (*Edge, AddEdgeInTxPostCommit, error) {
	if req == nil {
		return nil, nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing request"}
	}
	if req.OrgID == "" {
		return nil, nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing org_id", Meta: errs.Metadata{"field": "org_id"}}
	}
	if req.ProjectID == "" {
		return nil, nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing project_id", Meta: errs.Metadata{"field": "project_id"}}
	}
	if req.FromItem == "" {
		return nil, nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing from_item", Meta: errs.Metadata{"field": "from_item_id"}}
	}
	if req.ToItem == "" {
		return nil, nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing to_item", Meta: errs.Metadata{"field": "to_item_id"}}
	}
	if req.FromItem == req.ToItem {
		// dependencies_no_self_loop_chk would also reject this; surface a
		// clearer error than the CHECK violation.
		return nil, nil, &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "self-loop edges are forbidden",
			Meta:    errs.Metadata{"field": "to_item_id"},
		}
	}
	kind := req.Kind
	if kind == "" {
		kind = "blocks"
	}
	if _, ok := allowedEdgeKinds[kind]; !ok {
		return nil, nil, &errs.Error{
			Code:    errs.InvalidArgument,
			Message: fmt.Sprintf("invalid edge kind %q (allowed: blocks, related)", kind),
			Meta:    errs.Metadata{"field": "kind"},
		}
	}

	// Resolve both endpoints' (org_id, project_id) inside the tx — the
	// advisory lock key is the to_item's project_id (review L6-W8), the
	// cross-project rejection per §6.2 Tool 11 line 1417 needs both
	// project_ids, and the post-commit CascadeRequested publish uses the
	// DB-resolved (org_id, project_id) — NOT the request values — so the
	// trust boundary of this private RPC stays narrow even if a caller
	// upstream of the MCP gate ever slipped through (review L6-W1).
	// Two QueryRow calls keep the error mapping unambiguous: missing
	// endpoint becomes a per-field NotFound, not a generic empty
	// result set.
	var (
		fromProject, toProject string
		fromOrg, toOrg         string
	)
	if err := tx.QueryRow(ctx,
		`SELECT org_id, COALESCE(project_id, '') FROM workitems.items WHERE id = $1`,
		req.FromItem,
	).Scan(&fromOrg, &fromProject); err != nil {
		if errors.Is(err, sqldb.ErrNoRows) {
			return nil, nil, &errs.Error{
				Code:    errs.NotFound,
				Message: "from_item not found",
				Meta:    errs.Metadata{"field": "from_item_id", "id": req.FromItem},
			}
		}
		return nil, nil, &errs.Error{Code: errs.Internal, Message: "from_item lookup failed"}
	}
	if err := tx.QueryRow(ctx,
		`SELECT org_id, COALESCE(project_id, '') FROM workitems.items WHERE id = $1`,
		req.ToItem,
	).Scan(&toOrg, &toProject); err != nil {
		if errors.Is(err, sqldb.ErrNoRows) {
			return nil, nil, &errs.Error{
				Code:    errs.NotFound,
				Message: "to_item not found",
				Meta:    errs.Metadata{"field": "to_item_id", "id": req.ToItem},
			}
		}
		return nil, nil, &errs.Error{Code: errs.Internal, Message: "to_item lookup failed"}
	}
	// Cross-org edges are caught by the cross-project guard below in the
	// common case (org_id partitions projects), but check explicitly so
	// a corrupted row never leaks across orgs in the publish.
	if fromOrg != toOrg {
		return nil, nil, &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "cross-org edges are not allowed",
			Meta: errs.Metadata{
				"field":       "to_item_id",
				"from_org_id": fromOrg,
				"to_org_id":   toOrg,
			},
		}
	}
	if fromProject != toProject {
		// Round-6 §6.2 Tool 11: cross-project edges are explicitly
		// out-of-scope at v1.0. Error code is VALIDATION per spec; the
		// internal Encore code is InvalidArgument and the MCP layer
		// maps it to JSON-RPC kind="VALIDATION".
		return nil, nil, &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "cross-project edges are not allowed",
			Meta: errs.Metadata{
				"field":           "to_item_id",
				"from_project_id": fromProject,
				"to_project_id":   toProject,
			},
		}
	}
	if toProject == "" {
		// Defensive: workitems.items.project_id is nullable in the DDL
		// but P01 always populates it. An edge between two
		// project-less items would skip the advisory lock partitioning
		// — reject explicitly so the gap is visible.
		return nil, nil, &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "endpoints have no project_id",
			Meta:    errs.Metadata{"field": "to_item_id"},
		}
	}

	// AF5: per-project advisory lock serialises concurrent edge writes
	// within one project. Verbatim §6.5 line 1924.
	if _, err := tx.Exec(ctx,
		`SELECT pg_advisory_xact_lock(hashtext('deps.add_dependency:' || $1))`,
		toProject,
	); err != nil {
		return nil, nil, &errs.Error{Code: errs.Internal, Message: "advisory lock failed"}
	}

	// Cycle check: depth-counter recursive CTE extended to project the
	// walk path so we can populate data.cycle_path on rejection (AC #1)
	// and write deps.cycles forensics with a real path.
	cyclePath, err := checkCycle(ctx, tx, req.FromItem, req.ToItem)
	if err != nil {
		return nil, nil, &errs.Error{Code: errs.Internal, Message: "cycle check failed"}
	}
	if cyclePath != nil {
		// Caller's transaction will be rolled back by its deferred
		// tx.Rollback when this error returns — no edge is written. The
		// forensic deps.cycles INSERT runs as a separate top-level
		// statement (the db handle, NOT tx) so the audit row survives
		// the rollback. Same shape as AddEdge's standalone path.
		var rejectedBy string
		if id, ok := callerUserID(ctx); ok {
			rejectedBy = id
		}
		if err := recordCycle(ctx, req.FromItem, req.ToItem, cyclePath, rejectedBy); err != nil {
			rlog.Warn("deps: cycles forensic insert failed", "err", err,
				"from", req.FromItem, "to", req.ToItem)
		}
		// errs.Metadata is gob-encoded across the Encore RPC boundary;
		// gob cannot encode a bare []interface{}. We store the cycle
		// path as a comma-joined string and as a typed []string under
		// distinct keys — the MCP layer (D-5) picks the typed slice
		// for the JSON-RPC data.cycle_path array per §7 and falls
		// back to splitting the joined form when the typed slice is
		// not registered (e.g. across a gob round-trip).
		return nil, nil, &errs.Error{
			Code:    errs.FailedPrecondition,
			Message: "edge would create a cycle",
			Meta: errs.Metadata{
				"kind":            "CYCLE_DETECTED",
				"from":            req.FromItem,
				"to":              req.ToItem,
				"cycle_path":      strings.Join(cyclePath, ","),
				"cycle_path_list": cyclePath,
			},
		}
	}

	// Insert the edge.
	edgeID, err := newULID()
	if err != nil {
		return nil, nil, &errs.Error{Code: errs.Internal, Message: "ulid generation failed"}
	}
	var createdBy *string
	if id, ok := callerUserID(ctx); ok {
		c := id
		createdBy = &c
	}
	var (
		retID        string
		retFrom      string
		retTo        string
		retKind      string
		retCreatedAt time.Time
		retCreatedBy *string
	)
	if err := tx.QueryRow(ctx,
		`INSERT INTO deps.dependencies (id, from_item, to_item, kind, created_by)
		 VALUES ($1, $2, $3, $4, $5)
		 RETURNING id, from_item, to_item, kind, created_at, created_by`,
		edgeID, req.FromItem, req.ToItem, kind, createdBy,
	).Scan(&retID, &retFrom, &retTo, &retKind, &retCreatedAt, &retCreatedBy); err != nil {
		if isUniqueViolation(err, "dependencies_pair_uniq") {
			return nil, nil, &errs.Error{
				Code:    errs.AlreadyExists,
				Message: "edge already exists",
				Meta: errs.Metadata{
					"from": req.FromItem,
					"to":   req.ToItem,
					"kind": kind,
				},
			}
		}
		rlog.Warn("deps: dependency insert failed", "err", err,
			"from", req.FromItem, "to", req.ToItem, "kind", kind)
		return nil, nil, &errs.Error{Code: errs.Internal, Message: "dependency insert failed"}
	}

	// Regime A: recompute is_ready for to_item inline. Only 'blocks'
	// edges affect readiness, but recomputeReady is unconditional for
	// safety (the SQL filters by kind='blocks' in the NOT EXISTS
	// subquery, so a 'related' edge's recompute is a no-op).
	if _, err := recomputeReady(ctx, tx, req.ToItem); err != nil {
		rlog.Error("deps: AddEdge recomputeReady failed", "err", err, "to_item", req.ToItem)
		return nil, nil, &errs.Error{Code: errs.Internal, Message: "is_ready recompute failed"}
	}

	createdByOut := ""
	if retCreatedBy != nil {
		createdByOut = *retCreatedBy
	}

	// Capture publish scope under the DB-resolved (toOrg, toProject)
	// — NOT the request values — so the post-commit publish never
	// widens the trust boundary of this helper even when the caller
	// passes request values that drift from the DB row (review L6-W1).
	publishOrg := toOrg
	publishProject := toProject
	publishToItem := req.ToItem
	postCommit := func(publishCtx context.Context) {
		publishCascadeRequested(publishCtx, publishOrg, publishProject, publishToItem, "edge_added", "")
	}

	return &Edge{
		ID:        retID,
		FromItem:  retFrom,
		ToItem:    retTo,
		Kind:      retKind,
		CreatedAt: retCreatedAt,
		CreatedBy: createdByOut,
	}, postCommit, nil
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

// RemoveEdge deletes the edge, recomputes is_ready for the direct
// to_item inline (Regime A — single-hop), writes a cascade_events row
// with kind='edge_removed' INLINE in the same transaction, then
// post-commit publishes CascadeRequested REUSING the same event_id so
// the subscriber's ON CONFLICT (event_id, triggered_by_item_id) DO
// NOTHING collapses to no-op (exactly one cascade_events row per
// logical remove — round-6 tension #1).
//
// Returns to_item_now_ready as the SINGLE-HOP view. Transitive
// pipeline_stage updates downstream of to_item are eventually
// consistent — driven by the post-commit publish.
//
//encore:api private method=POST path=/deps.RemoveEdge
func RemoveEdge(ctx context.Context, req *RemoveEdgeRequest) (*RemoveEdgeResponse, error) {
	if req == nil {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing request"}
	}
	// Exactly-one selection: EdgeID XOR (FromItem+ToItem+Kind).
	haveEdgeID := req.EdgeID != ""
	haveComposite := req.FromItem != "" && req.ToItem != "" && req.Kind != ""
	switch {
	case haveEdgeID && haveComposite:
		return nil, &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "pass either edge_id OR (from_item, to_item, kind), not both",
		}
	case !haveEdgeID && !haveComposite:
		return nil, &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "pass either edge_id OR (from_item, to_item, kind)",
		}
	}
	if haveComposite {
		if _, ok := allowedEdgeKinds[req.Kind]; !ok {
			return nil, &errs.Error{
				Code:    errs.InvalidArgument,
				Message: fmt.Sprintf("invalid edge kind %q (allowed: blocks, related)", req.Kind),
				Meta:    errs.Metadata{"field": "kind"},
			}
		}
	}

	// Round-6 HIGH-risk: event_id MUST be minted BEFORE BEGIN so the
	// inline audit row and the post-commit publish share the SAME
	// ulid. If a serialisation conflict triggers a retry path inside
	// the tx, we keep using THIS event_id — never re-mint, otherwise
	// the subscriber's ON CONFLICT collapse breaks and we write two
	// rows per logical remove.
	eventID, err := newULID()
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "ulid generation failed"}
	}

	tx, err := db.Begin(ctx)
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "db begin failed"}
	}
	defer func() { _ = tx.Rollback() }()

	// Resolve (edge_id, to_item, org_id, project_id) for the audit
	// row and the post-commit publish. Both composite and EdgeID paths
	// converge here.
	var (
		edgeID    string
		toItem    string
		orgID     string
		projectID string
	)
	if haveEdgeID {
		err = tx.QueryRow(ctx,
			`SELECT d.id, d.to_item, i.org_id, COALESCE(i.project_id, '')
			   FROM deps.dependencies d
			   JOIN workitems.items i ON i.id = d.to_item
			  WHERE d.id = $1
			  FOR UPDATE OF d`,
			req.EdgeID,
		).Scan(&edgeID, &toItem, &orgID, &projectID)
	} else {
		err = tx.QueryRow(ctx,
			`SELECT d.id, d.to_item, i.org_id, COALESCE(i.project_id, '')
			   FROM deps.dependencies d
			   JOIN workitems.items i ON i.id = d.to_item
			  WHERE d.from_item = $1 AND d.to_item = $2 AND d.kind = $3
			  FOR UPDATE OF d`,
			req.FromItem, req.ToItem, req.Kind,
		).Scan(&edgeID, &toItem, &orgID, &projectID)
	}
	if err != nil {
		if errors.Is(err, sqldb.ErrNoRows) {
			return nil, &errs.Error{Code: errs.NotFound, Message: "edge not found"}
		}
		return nil, &errs.Error{Code: errs.Internal, Message: "edge lookup failed"}
	}

	// Delete the edge.
	if _, err := tx.Exec(ctx,
		`DELETE FROM deps.dependencies WHERE id = $1`,
		edgeID,
	); err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "edge delete failed"}
	}

	// Regime A: recompute is_ready inline for the direct to_item.
	toNowReady, err := recomputeReady(ctx, tx, toItem)
	if err != nil {
		rlog.Error("deps: RemoveEdge recomputeReady failed", "err", err, "to_item", toItem)
		return nil, &errs.Error{Code: errs.Internal, Message: "is_ready recompute failed"}
	}

	// Inline audit row. event_id was captured BEFORE BEGIN (tension #1).
	auditID, err := newULID()
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "ulid generation failed"}
	}
	var (
		projectColumn *string
		traceColumn   *string
	)
	if projectID != "" {
		p := projectID
		projectColumn = &p
	}
	if tid := tracectx.TraceID(ctx); tid != "" {
		t := tid
		traceColumn = &t
	}
	// ON CONFLICT (event_id, triggered_by_item_id) DO NOTHING mirrors
	// the subscriber's collapse clause (§6.3.2) — defence-in-depth so a
	// future caller-driven retry that reuses the same event_id can
	// never mint a duplicate audit row (review L6-W2).
	if _, err := tx.Exec(ctx,
		`INSERT INTO deps.cascade_events
		   (id, event_id, kind, org_id, project_id,
		    triggered_by_item_id, affected_item_ids, cascaded_count, trace_id)
		 VALUES ($1, $2, 'edge_removed', $3, $4, $5, $6, 1, $7)
		 ON CONFLICT (event_id, triggered_by_item_id) DO NOTHING`,
		auditID, eventID, orgID, projectColumn, toItem, []string{toItem}, traceColumn,
	); err != nil {
		rlog.Error("deps: cascade_events insert failed", "err", err, "event_id", eventID)
		return nil, &errs.Error{Code: errs.Internal, Message: "audit insert failed"}
	}

	if err := tx.Commit(); err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "commit failed"}
	}

	// Regime B: post-commit publish REUSING the same event_id. The
	// subscriber attempts an INSERT and collapses to no-op via the
	// UNIQUE (event_id, triggered_by_item_id) ON CONFLICT clause —
	// the inline row above already exists.
	publishCascadeRequested(ctx, orgID, projectID, toItem, "edge_removed", eventID)

	return &RemoveEdgeResponse{
		Removed:        true,
		ToItemNowReady: toNowReady,
		ToItemID:       toItem,
	}, nil
}

// IsReadyRequest is the input to IsReady. SPEC §4.5 wrote the signature
// as `func IsReady(ctx, itemID string) (bool, error)`; Encore requires
// API request types to be named structs (E1354). The wire-shape is
// unchanged. See DEVIATION trail on bead unblock-tv8.1.
type IsReadyRequest struct {
	ItemID string
}

// IsReadyResponse is the output of IsReady.
type IsReadyResponse struct {
	IsReady bool
}

// IsReady returns the current is_ready value for itemID (read directly
// from workitems.items, NOT recomputed). Smoke-test helper; production
// readers query workitems.items directly. SPEC §4.5.
//
//encore:api private method=POST path=/deps.IsReady
func IsReady(ctx context.Context, req *IsReadyRequest) (*IsReadyResponse, error) {
	if req == nil || req.ItemID == "" {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing item_id"}
	}
	var ready bool
	err := db.QueryRow(ctx,
		`SELECT is_ready FROM workitems.items WHERE id = $1`,
		req.ItemID,
	).Scan(&ready)
	if err != nil {
		if errors.Is(err, sqldb.ErrNoRows) {
			return nil, &errs.Error{Code: errs.NotFound, Message: "item not found"}
		}
		return nil, &errs.Error{Code: errs.Internal, Message: "is_ready read failed"}
	}
	return &IsReadyResponse{IsReady: ready}, nil
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

// Closure returns the transitive 'blocks' closure for an item.
// Direction="outgoing" walks d.from_item=r.id projecting d.to_item
// (items reachable from itemID). Direction="incoming" walks
// d.to_item=r.id projecting d.from_item (items that reach itemID).
// Excludes the seed itself. Depth capped at 256 per AR-8. SPEC §4.5.
//
//encore:api private method=POST path=/deps.Closure
func Closure(ctx context.Context, req *ClosureRequest) (*ClosureResponse, error) {
	if req == nil || req.ItemID == "" {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing item_id"}
	}
	if _, ok := allowedClosureDirections[req.Direction]; !ok {
		return nil, &errs.Error{
			Code:    errs.InvalidArgument,
			Message: fmt.Sprintf("invalid direction %q (allowed: incoming, outgoing)", req.Direction),
			Meta:    errs.Metadata{"field": "direction"},
		}
	}
	depth := req.MaxDepth
	if depth <= 0 {
		depth = closureMaxDepth
	}
	if depth > closureMaxDepth {
		return nil, &errs.Error{
			Code:    errs.InvalidArgument,
			Message: fmt.Sprintf("max_depth must be <= %d", closureMaxDepth),
			Meta:    errs.Metadata{"field": "max_depth"},
		}
	}

	// The two directions only differ in which column joins on r.id and
	// which column is projected. closureSQL(direction) returns one of
	// two constant SQL strings — no runtime string concat into SQL.
	rows, err := db.Query(ctx, closureSQL(req.Direction), req.ItemID, depth)
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "closure query failed"}
	}
	defer rows.Close()
	out := make([]string, 0)
	for rows.Next() {
		var id string
		if err := rows.Scan(&id); err != nil {
			return nil, &errs.Error{Code: errs.Internal, Message: "closure scan failed"}
		}
		out = append(out, id)
	}
	if err := rows.Err(); err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "closure iter failed"}
	}
	return &ClosureResponse{ItemIDs: out}, nil
}

// closureSQL returns the recursive-CTE SQL for the requested Closure
// direction. Two constant strings — no runtime interpolation of the
// direction into the SQL text (review L6-S1). The caller passes a value
// that has already been validated against allowedClosureDirections;
// any other value collapses to the outgoing form by default but the
// caller is responsible for the validation gate.
func closureSQL(direction string) string {
	switch direction {
	case "incoming":
		return `WITH RECURSIVE reachable(id, depth) AS (
		         SELECT $1::text, 0
		         UNION ALL
		         SELECT d.from_item, r.depth + 1
		           FROM deps.dependencies d
		           JOIN reachable r ON d.to_item = r.id
		          WHERE d.kind = 'blocks'
		            AND r.depth < $2
		       )
		       SELECT DISTINCT id FROM reachable WHERE id <> $1
		       ORDER BY id`
	default: // "outgoing"
		return `WITH RECURSIVE reachable(id, depth) AS (
		         SELECT $1::text, 0
		         UNION ALL
		         SELECT d.to_item, r.depth + 1
		           FROM deps.dependencies d
		           JOIN reachable r ON d.from_item = r.id
		          WHERE d.kind = 'blocks'
		            AND r.depth < $2
		       )
		       SELECT DISTINCT id FROM reachable WHERE id <> $1
		       ORDER BY id`
	}
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

// RecentCascadeEvents returns the last N (≤50) deps.cascade_events rows
// for the org/project, ordered by triggered_at DESC. AF2 closure — used
// by the prime MCP tool. SPEC §4.5.
//
// Authorisation: this RPC is private and called from the MCP layer
// which gates via org.Authorize at the session→org boundary BEFORE
// dispatching here. The SQL filter is explicit on (org_id, project_id)
// so the scope is auditable.
//
//encore:api private method=POST path=/deps.RecentCascadeEvents
func RecentCascadeEvents(ctx context.Context, req *RecentCascadeEventsRequest) (*RecentCascadeEventsResponse, error) {
	if req == nil || req.OrgID == "" {
		return nil, &errs.Error{Code: errs.InvalidArgument, Message: "missing org_id", Meta: errs.Metadata{"field": "org_id"}}
	}
	limit := req.Limit
	if limit <= 0 || limit > recentCascadeEventsLimitCap {
		limit = recentCascadeEventsLimitCap
	}

	rows, err := db.Query(ctx,
		`SELECT id, event_id, COALESCE(triggered_by_item_id, ''),
		        affected_item_ids, cascaded_count, triggered_at,
		        COALESCE(trace_id, '')
		   FROM deps.cascade_events
		  WHERE org_id = $1
		    AND ($2 = '' OR project_id = $2)
		  ORDER BY triggered_at DESC
		  LIMIT $3`,
		req.OrgID, req.ProjectID, limit,
	)
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "cascade events query failed"}
	}
	defer rows.Close()
	out := make([]CascadeEventRow, 0, limit)
	for rows.Next() {
		var row CascadeEventRow
		if err := rows.Scan(
			&row.ID, &row.EventID, &row.TriggeredByItemID,
			&row.AffectedItemIDs, &row.CascadedCount, &row.TriggeredAt,
			&row.TraceID,
		); err != nil {
			return nil, &errs.Error{Code: errs.Internal, Message: "cascade events scan failed"}
		}
		out = append(out, row)
	}
	if err := rows.Err(); err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "cascade events iter failed"}
	}
	return &RecentCascadeEventsResponse{Events: out}, nil
}

// publishCascadeRequested emits a CascadeRequested for the §6.3.0
// Regime-B (multi-hop pipeline_stage) recompute pass.
//
// reuseEventID, when non-empty, is used verbatim as the publish
// envelope's EventID (the "tension #1" pattern: deps.RemoveEdge writes
// the inline audit row with this id, then publishes with the same id
// so the subscriber's ON CONFLICT clause collapses to no-op). An empty
// reuseEventID mints a fresh ulid (the AddEdge case, where no inline
// audit row exists and the subscriber writes the kind='edge_added'
// row during its recompute pass).
//
// Publish failure is logged as a warning, not returned: the underlying
// transaction has already committed and the cascade is best-effort
// (idempotency / replay handles the rare delivery loss).
func publishCascadeRequested(ctx context.Context, orgID, projectID, triggeredBy, reason, reuseEventID string) {
	eventID := reuseEventID
	if eventID == "" {
		id, err := newULID()
		if err != nil {
			rlog.Warn("deps: cascade event id generation failed", "err", err, "reason", reason)
			return
		}
		eventID = id
	}
	if _, err := CascadeRequestedTopic.Publish(ctx, &CascadeRequested{
		EventID:           eventID,
		OrgID:             orgID,
		ProjectID:         projectID,
		TriggeredByItemID: triggeredBy,
		Reason:            reason,
		TraceID:           tracectx.TraceID(ctx),
		EmittedAt:         time.Now().UTC(),
	}); err != nil {
		rlog.Warn("deps: cascade publish failed (transaction already committed)",
			"err", err, "reason", reason, "triggered_by", triggeredBy)
	}
}

// callerUserID returns the caller's user id from the Encore auth
// context, when available. Returns ("", false) when no identity is
// bound (e.g. seeder runs, tests without auth setup).
func callerUserID(_ context.Context) (string, bool) {
	uid, ok := encoreauth.UserID()
	if !ok || uid == "" {
		return "", false
	}
	if data, ok := encoreauth.Data().(*auth.AuthData); ok && data != nil && data.Identity.UserID != "" {
		return data.Identity.UserID, true
	}
	return string(uid), true
}
