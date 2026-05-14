// handler_prime.go owns the §6.2 Tool 1 (`prime`) handler — the
// dashboard-style read returned to a fresh agent session.
//
// Composes three reads:
//
//  1. ready_summary — workitems.Ready (the §6.2 Tool 2 RPC) for
//     count_total + items[<=ready_limit] under the same scope rules.
//  2. claimed_by_me — workitems.List filtered by claimed_by =
//     Identity.UserID.
//  3. recent_cascade_events — deps.RecentCascadeEvents (AF2 cap 50).
//
// memory_hints is empty in P01 per SPEC §6.2 line 1173 ("populated in
// P02 once memory ships").
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 6.2 Tool 1 (lines
// 1154-1175) + § 7 (error envelope).

package mcp

import (
	"context"
	"time"

	"encore.app/deps"
	"encore.app/workitems"
	"encore.dev/beta/errs"
	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

// primeReadyLimitDefault matches SPEC §6.2 Tool 1 line 1163
// (default 10, range 1..50). Same shape as the Tool 2 `ready` limit.
const primeReadyLimitDefault = 10

// primeReadyLimitMax mirrors readyMaxLimit — kept as a separate
// const so a future spec amendment that diverges Tool 1 from Tool 2
// is a single-line edit here.
const primeReadyLimitMax = 50

// primeIn is the §6.2 Tool 1 argument shape. Field tags map to the
// JSON schema the SDK derives from the struct via reflection; spec
// names use snake_case (project_id, ready_limit) so we override the
// Go field names accordingly.
type primeIn struct {
	ProjectID  string `json:"project_id,omitempty"`
	ReadyLimit int    `json:"ready_limit,omitempty"`
}

// primeOut is the §6.2 Tool 1 structured result. ready_summary,
// claimed_by_me, recent_cascade_events, memory_hints (empty in P01).
type primeOut struct {
	ReadySummary        primeReadySummary `json:"ready_summary"`
	ClaimedByMe         []primeItem       `json:"claimed_by_me"`
	RecentCascadeEvents []primeCascadeRow `json:"recent_cascade_events"`
	MemoryHints         []primeMemoryHint `json:"memory_hints"`
}

// primeReadySummary mirrors SPEC §6.2 Tool 1 ready_summary —
// count_total + items[<=ready_limit].
type primeReadySummary struct {
	CountTotal int         `json:"count_total"`
	Items      []primeItem `json:"items"`
}

// primeItem mirrors the §6.2 Item shape (Tool 1/2/4 output). The
// canonical Item carries time.Time pointers; we marshal them as ISO
// 8601 strings on the wire (omitempty when nil).
type primeItem struct {
	ID                  string   `json:"id"`
	OrgID               string   `json:"org_id"`
	ProjectID           string   `json:"project_id,omitempty"`
	MilestoneID         string   `json:"milestone_id,omitempty"`
	ParentID            string   `json:"parent_id,omitempty"`
	DiscoveredFromID    string   `json:"discovered_from_id,omitempty"`
	Type                string   `json:"type"`
	Title               string   `json:"title"`
	Body                string   `json:"body,omitempty"`
	Status              string   `json:"status"`
	Priority            string   `json:"priority"`
	PipelineStage       string   `json:"pipeline_stage,omitempty"`
	AgentKind           string   `json:"agent_kind,omitempty"`
	ImplState           string   `json:"impl_state,omitempty"`
	ReviewState         string   `json:"review_state,omitempty"`
	QAState             string   `json:"qa_state,omitempty"`
	PipelineState       string   `json:"pipeline_state,omitempty"`
	Severity            string   `json:"severity,omitempty"`
	KindOfFinding       string   `json:"kind_of_finding,omitempty"`
	ClaimedByID         string   `json:"claimed_by_id,omitempty"`
	ClaimedByAgent      string   `json:"claimed_by_agent,omitempty"`
	ClaimedAt           string   `json:"claimed_at,omitempty"`
	IsReady             bool     `json:"is_ready"`
	MilestoneAssignedAt string   `json:"milestone_assigned_at,omitempty"`
	MilestoneAssignedBy string   `json:"milestone_assigned_by,omitempty"`
	Labels              []string `json:"labels,omitempty"`
	CreatedAt           string   `json:"created_at"`
	UpdatedAt           string   `json:"updated_at"`
	ClosedAt            string   `json:"closed_at,omitempty"`
}

// primeCascadeRow mirrors deps.CascadeEventRow as JSON.
type primeCascadeRow struct {
	ID                string   `json:"id"`
	EventID           string   `json:"event_id"`
	TriggeredByItemID string   `json:"triggered_by_item_id,omitempty"`
	AffectedItemIDs   []string `json:"affected_item_ids"`
	CascadedCount     int      `json:"cascaded_count"`
	TriggeredAt       string   `json:"triggered_at"`
	TraceID           string   `json:"trace_id,omitempty"`
}

// primeMemoryHint is the (currently empty) memory_hints shape. P02
// will populate this; declared here so the JSON schema stays
// additive-compatible from day one.
type primeMemoryHint struct {
	Source string `json:"source"`
	Body   string `json:"body"`
}

// registerHandlePrime is invoked by transport.go's init AFTER
// sdkServer has been constructed. See transport.go::toolRegistrars
// for the wiring rationale (avoids the alphabetical-file init
// order hazard where handler_*.go inits would otherwise crash on a
// nil sdkServer).
func registerHandlePrime(s *sdkmcp.Server) {
	sdkmcp.AddTool(s, &sdkmcp.Tool{
		Name: "prime",
		Description: "Dashboard read for a fresh agent session: ready " +
			"summary, items the caller already claims, recent cascade " +
			"events, and (P02+) memory hints. SPEC § 6.2 Tool 1.",
	}, handlePrime)
}

// handlePrime executes the §6.2 Tool 1 read fan-out. Identity is
// pulled from the request HTTP headers (populated by serveMCP after
// Bearer auth); the synthetic Encore auth context for downstream
// RPCs is installed via withIdentity so workitems.* / deps.* see
// the right caller.
func handlePrime(ctx context.Context, req *sdkmcp.CallToolRequest, in primeIn) (*sdkmcp.CallToolResult, primeOut, error) {
	tool := "prime"
	state := bindTool(req, tool)

	identity, ok := identityFromReq(req)
	if !ok {
		return nil, primeOut{}, mapError(state, tool, errMissingIdentityErr())
	}

	mcpCtx, err := withIdentityFromReq(ctx, req)
	if err != nil {
		return nil, primeOut{}, mapError(state, tool, err)
	}

	if state != nil && state.Call != nil && in.ProjectID != "" {
		state.Call.ProjectID = in.ProjectID
	}

	readyLimit := in.ReadyLimit
	if readyLimit <= 0 {
		readyLimit = primeReadyLimitDefault
	}
	if readyLimit > primeReadyLimitMax {
		readyLimit = primeReadyLimitMax
	}

	// 1) ready_summary — wraps workitems.Ready under the same scope.
	readyResp, err := workitems.Ready(mcpCtx, &workitems.ReadyRequest{
		OrgID:     identity.OrgID,
		ProjectID: in.ProjectID,
		Limit:     readyLimit,
	})
	if err != nil {
		return nil, primeOut{}, mapError(state, tool, err)
	}

	// 2) claimed_by_me — workitems.List filtered by claimed_by =
	// Identity.UserID. List orders by id ASC which is acceptable
	// here (claimed_by_me is informational; no ordering invariant in
	// the §6.2 contract).
	claimedResp, err := workitems.List(mcpCtx, &workitems.ListRequest{
		ProjectID: in.ProjectID,
		ClaimedBy: identity.UserID,
	})
	if err != nil {
		return nil, primeOut{}, mapError(state, tool, err)
	}

	// 3) recent_cascade_events — AF2 cap of 50 enforced inside
	// deps.RecentCascadeEvents (recentCascadeEventsLimitCap).
	cascadeResp, err := deps.RecentCascadeEvents(mcpCtx, &deps.RecentCascadeEventsRequest{
		OrgID:     identity.OrgID,
		ProjectID: in.ProjectID,
	})
	if err != nil {
		return nil, primeOut{}, mapError(state, tool, err)
	}

	out := primeOut{
		ReadySummary: primeReadySummary{
			CountTotal: readyResp.TotalReady,
			Items:      itemsToPrime(readyResp.Items),
		},
		ClaimedByMe:         itemsToPrime(claimedResp.Items),
		RecentCascadeEvents: cascadeRowsToPrime(cascadeResp.Events),
		MemoryHints:         []primeMemoryHint{},
	}

	if state != nil && state.Call != nil {
		state.Call.ResultKind = ResultOK
	}
	return nil, out, nil
}

// errMissingIdentityErr surfaces an Unauthenticated error when the
// MCP transport invoked the handler before Identity resolution. The
// natural callers (production traffic) always have Identity bound;
// the test path may exercise the handler with a half-bound state
// and expects a clean UNAUTHENTICATED envelope.
func errMissingIdentityErr() error {
	return &errs.Error{Code: errs.Unauthenticated, Message: "no caller identity"}
}

// bindTool retrieves the per-request state registered by serveMCP
// under the request's trace_id header AND sets the audit row's
// tool_name. Returns the state pointer (nil for non-MCP test
// paths) so the caller can keep mutating Call.* fields directly
// without re-looking-up the registry on every set.
//
// See recordtoolcall.go::requestStateRegistry for the per-request
// channel rationale.
func bindTool(req *sdkmcp.CallToolRequest, tool string) *requestState {
	state := stateFromReq(req)
	if state != nil && state.Call != nil {
		state.Call.ToolName = tool
	}
	return state
}

// itemsToPrime converts a workitems.Item slice into the §6.2 wire
// shape (primeItem). Time fields are formatted as RFC 3339 nano so
// the wire form is unambiguous; empty fields are elided.
func itemsToPrime(items []workitems.Item) []primeItem {
	if len(items) == 0 {
		return []primeItem{}
	}
	out := make([]primeItem, 0, len(items))
	for i := range items {
		out = append(out, itemToPrime(items[i]))
	}
	return out
}

func itemToPrime(it workitems.Item) primeItem {
	p := primeItem{
		ID:                  it.ID,
		OrgID:               it.OrgID,
		ProjectID:           it.ProjectID,
		MilestoneID:         it.MilestoneID,
		ParentID:            it.ParentID,
		DiscoveredFromID:    it.DiscoveredFromID,
		Type:                it.Type,
		Title:               it.Title,
		Body:                it.Body,
		Status:              it.Status,
		Priority:            it.Priority,
		PipelineStage:       it.PipelineStage,
		AgentKind:           it.AgentKind,
		ImplState:           it.ImplState,
		ReviewState:         it.ReviewState,
		QAState:             it.QAState,
		PipelineState:       it.PipelineState,
		Severity:            it.Severity,
		KindOfFinding:       it.KindOfFinding,
		ClaimedByID:         it.ClaimedByID,
		ClaimedByAgent:      it.ClaimedByAgent,
		IsReady:             it.IsReady,
		MilestoneAssignedBy: it.MilestoneAssignedBy,
		Labels:              it.Labels,
		CreatedAt:           it.CreatedAt.UTC().Format(time.RFC3339Nano),
		UpdatedAt:           it.UpdatedAt.UTC().Format(time.RFC3339Nano),
	}
	if it.ClaimedAt != nil {
		p.ClaimedAt = it.ClaimedAt.UTC().Format(time.RFC3339Nano)
	}
	if it.MilestoneAssignedAt != nil {
		p.MilestoneAssignedAt = it.MilestoneAssignedAt.UTC().Format(time.RFC3339Nano)
	}
	if it.ClosedAt != nil {
		p.ClosedAt = it.ClosedAt.UTC().Format(time.RFC3339Nano)
	}
	return p
}

func cascadeRowsToPrime(rows []deps.CascadeEventRow) []primeCascadeRow {
	if len(rows) == 0 {
		return []primeCascadeRow{}
	}
	out := make([]primeCascadeRow, 0, len(rows))
	for _, r := range rows {
		out = append(out, primeCascadeRow{
			ID:                r.ID,
			EventID:           r.EventID,
			TriggeredByItemID: r.TriggeredByItemID,
			AffectedItemIDs:   append([]string{}, r.AffectedItemIDs...),
			CascadedCount:     r.CascadedCount,
			TriggeredAt:       r.TriggeredAt.UTC().Format(time.RFC3339Nano),
			TraceID:           r.TraceID,
		})
	}
	return out
}

// identityFields is the narrow tuple the handlers read from the
// per-request state. Mirrors auth.Identity for the fields actually
// used in the tool bodies (Role is not consulted — every MCP
// caller is "agent" by construction).
type identityFields struct {
	UserID    string
	OrgID     string
	AgentKind string
}
