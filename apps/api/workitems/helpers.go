// helpers.go contains internal helpers consumed by workitems.go:
// row-scan shapes for the rbac builder, item/comment/milestone row
// readers, label attachment helpers, error matchers, and the canonical
// column list for direct SELECT statements.
//
// Everything in this file is package-private. The public RPC surface
// lives in workitems.go and is locked by SPEC §4.4 / §4.4.1.

package workitems

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"time"

	"encore.dev/beta/errs"
	"encore.dev/rlog"
	"encore.dev/storage/sqldb"
)

// itemColumnList is the canonical column projection for workitems.items
// reads that do not go through rbac.For (e.g. nested fetches in
// GetTrail). The order MUST match itemRow's field declaration order.
// Nullable columns are scanned into *string targets; COALESCE is
// avoided so pgx's nullable-text path is exercised directly.
//
// rbac.For uses `SELECT *` so any column-order drift between the
// migration and itemRow would surface as scan errors in rbac.For's
// reflection path — keep them in sync. FTS is intentionally excluded
// here (direct queries do not need it).
const itemColumnList = `id, org_id, project_id, milestone_id, parent_id, discovered_from_id,
	type, title, body, status, priority, pipeline_stage,
	agent_kind, impl_state, review_state, qa_state, pipeline_state,
	severity, kind_of_finding, claimed_by_id, claimed_by_agent,
	claimed_at, is_ready, milestone_assigned_at, milestone_assigned_by,
	created_at, updated_at, closed_at`

// itemRow mirrors workitems.items column order verbatim (per migration
// 0040_workitems.up.sql lines 46-135 + the post-ALTER fts column).
// rbac.For uses `SELECT * FROM workitems.items` and scans by ordinal,
// so this shape MUST match the schema's column declaration order
// including the trailing `fts` tsvector column added by ALTER TABLE.
type itemRow struct {
	ID                  string
	OrgID               string
	ProjectID           *string
	MilestoneID         *string
	ParentID            *string
	DiscoveredFromID    *string
	Type                string
	Title               string
	Body                string
	Status              string
	Priority            string
	PipelineStage       string
	AgentKind           *string
	ImplState           string
	ReviewState         string
	QAState             string
	PipelineState       string
	Severity            *string
	KindOfFinding       *string
	ClaimedByID         *string
	ClaimedByAgent      *string
	ClaimedAt           *time.Time
	IsReady             bool
	MilestoneAssignedAt *time.Time
	MilestoneAssignedBy *string
	CreatedAt           time.Time
	UpdatedAt           time.Time
	ClosedAt            *time.Time
	// FTS is the generated tsvector column appended by ALTER TABLE in
	// 0040_workitems.up.sql line 151-155. rbac.For uses SELECT * which
	// projects it; pgx v5.7 has no registered tsvector type, so the
	// driver delivers the text representation as a raw byte slice.
	// The field is unused downstream — it exists only to keep the
	// ordinal positions aligned with the migration so rbac.For's
	// reflection-based scanner does not error on column count.
	FTS []byte
}

// stateRow is the lightweight projection used by SetStateColumns to
// pull just the columns required for invariant validation AND the
// scope fields (OrgID, ProjectID) consumed by the post-commit
// state_change cascade publish per SPEC §6.3.0 (Regime B). The scope
// fields are projected unconditionally so the predicate logic stays
// uniform across publishing and non-publishing branches; only the
// publishing branch reads them. Mirrors the FOR UPDATE projection
// pattern used in Close (workitems.go:1287-1296) and Claim
// (workitems.go:1431-1438).
type stateRow struct {
	Impl      string
	Review    string
	QA        string
	Pipeline  string
	ClaimedBy *string
	OrgID     string
	ProjectID string
}

// scanItemRow is the canonical scanner for the itemColumnList projection
// (i.e. direct SELECT statements that do not go through rbac.For).
// Order MUST match itemColumnList exactly. FTS is intentionally not
// projected here — direct queries do not need it, and the column's
// pgx-driver representation (raw byte slice for tsvector) is only
// required by rbac.For's reflection path.
func scanItemRow(rows *sqldb.Rows, r *itemRow) error {
	return rows.Scan(
		&r.ID, &r.OrgID,
		&r.ProjectID,
		&r.MilestoneID,
		&r.ParentID,
		&r.DiscoveredFromID,
		&r.Type, &r.Title, &r.Body, &r.Status, &r.Priority, &r.PipelineStage,
		&r.AgentKind,
		&r.ImplState, &r.ReviewState, &r.QAState, &r.PipelineState,
		&r.Severity,
		&r.KindOfFinding,
		&r.ClaimedByID,
		&r.ClaimedByAgent,
		&r.ClaimedAt,
		&r.IsReady,
		&r.MilestoneAssignedAt,
		&r.MilestoneAssignedBy,
		&r.CreatedAt, &r.UpdatedAt, &r.ClosedAt,
	)
}

// itemFromRow converts an itemRow into a fully-populated Item, including
// loading the labels list. ctx is used for the labels query.
func itemFromRow(ctx context.Context, r itemRow) (*Item, error) {
	labels, err := loadLabels(ctx, r.ID)
	if err != nil {
		return nil, err
	}
	return &Item{
		ID:                  r.ID,
		OrgID:               r.OrgID,
		ProjectID:           ptrToString(r.ProjectID),
		MilestoneID:         ptrToString(r.MilestoneID),
		ParentID:            ptrToString(r.ParentID),
		DiscoveredFromID:    ptrToString(r.DiscoveredFromID),
		Type:                r.Type,
		Title:               r.Title,
		Body:                r.Body,
		Status:              r.Status,
		Priority:            r.Priority,
		PipelineStage:       r.PipelineStage,
		AgentKind:           ptrToString(r.AgentKind),
		ImplState:           r.ImplState,
		ReviewState:         r.ReviewState,
		QAState:             r.QAState,
		PipelineState:       r.PipelineState,
		Severity:            ptrToString(r.Severity),
		KindOfFinding:       ptrToString(r.KindOfFinding),
		ClaimedByID:         ptrToString(r.ClaimedByID),
		ClaimedByAgent:      ptrToString(r.ClaimedByAgent),
		ClaimedAt:           r.ClaimedAt,
		IsReady:             r.IsReady,
		MilestoneAssignedAt: r.MilestoneAssignedAt,
		MilestoneAssignedBy: ptrToString(r.MilestoneAssignedBy),
		Labels:              labels,
		CreatedAt:           r.CreatedAt,
		UpdatedAt:           r.UpdatedAt,
		ClosedAt:            r.ClosedAt,
	}, nil
}

// readItem fetches a single item directly (no rbac scope — used after
// a write succeeded and we already validated org_id). Returns nil + a
// NotFound error if the row is missing.
func readItem(ctx context.Context, id string) (*Item, error) {
	rows, err := db.Query(ctx, `SELECT `+itemColumnList+` FROM workitems.items WHERE id = $1`, id)
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "item read-back failed"}
	}
	defer rows.Close()
	if !rows.Next() {
		return nil, &errs.Error{Code: errs.NotFound, Message: "item not found"}
	}
	var r itemRow
	if err := scanItemRow(rows, &r); err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "item scan failed"}
	}
	if err := rows.Err(); err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "item iter failed"}
	}
	return itemFromRow(ctx, r)
}

// loadLabels returns the label ids attached to the given item.
func loadLabels(ctx context.Context, itemID string) ([]string, error) {
	rows, err := db.Query(ctx,
		`SELECT label_id FROM workitems.item_labels WHERE item_id = $1 ORDER BY applied_at`,
		itemID,
	)
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "label load failed"}
	}
	defer rows.Close()
	var labels []string
	for rows.Next() {
		var l string
		if err := rows.Scan(&l); err != nil {
			return nil, &errs.Error{Code: errs.Internal, Message: "label scan failed"}
		}
		labels = append(labels, l)
	}
	if err := rows.Err(); err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "label iter failed"}
	}
	return labels, nil
}

// attachLabelsTx inserts (item_id, label_id) rows using the caller's
// transaction handle, gating each wire-supplied label_id against the caller's
// org (round-16, bead unblock-tv8.78, SPEC §10.1.1). Errors map FK violations
// and cross-tenant / missing labels to NotFound.
//
// Each INSERT is a guarded INSERT … SELECT whose WHERE admits the label only
// when it is org-scoped to callerOrg OR project-scoped to a project in
// callerOrg (the org-XOR-project label-ownership form, milestone precedent) —
// so a foreign label_id (org- or project-scoped to another org) attaches
// NOTHING and yields NOT_FOUND, the SAME envelope a genuinely-missing label
// yields, never disclosing cross-tenant existence and never planting a
// cross-org item_labels row.
//
// callerOrg keys the gate. The predicate takes the empty-callerOrg NO-OP form
// (`callerOrg = ” OR …`): the create path always passes a real, non-empty
// req.OrgID so the gate is active there; the Update label-replace caller
// passes its CallerOrgID channel, which trusted internal no-auth callers leave
// empty (the .77 §10.1.1 no-op convention) — MCP handlers always pin it, so the
// no-op is unreachable from the agent surface. A non-existent label_id matches
// no row → zero inserted → NOT_FOUND regardless of callerOrg.
func attachLabelsTx(ctx context.Context, tx *sqldb.Tx, itemID, callerOrg string, labels []string) error {
	for _, labelID := range labels {
		if labelID == "" {
			continue
		}
		tag, err := tx.Exec(ctx,
			`INSERT INTO workitems.item_labels (item_id, label_id)
			 SELECT $1, $2
			   FROM workitems.labels
			  WHERE id = $2
			    AND ($3 = ''
			         OR org_id = $3
			         OR project_id IN (SELECT id FROM org.projects WHERE org_id = $3))
			 ON CONFLICT (item_id, label_id) DO NOTHING`,
			itemID, labelID, callerOrg,
		)
		if err != nil {
			// Defensive / effectively unreachable for this guarded INSERT … SELECT:
			// a missing label_id selects zero rows (no FK violation) and flows
			// through the RowsAffected()==0 → EXISTS-recheck → NOT_FOUND path
			// below; the item_id FK cannot fire (the item row was just created in
			// this same tx). Kept so the NOT_FOUND outcome holds even if the query
			// shape ever regresses to an INSERT … VALUES.
			if isForeignKeyViolation(err) {
				return &errs.Error{
					Code:    errs.NotFound,
					Message: fmt.Sprintf("label %q does not exist", labelID),
					Meta:    errs.Metadata{"label_id": labelID},
				}
			}
			rlog.Error("workitems: label attach failed", "err", err, "item_id", itemID, "label_id", labelID)
			return &errs.Error{Code: errs.Internal, Message: "label attach failed"}
		}
		// Zero inserted rows means EITHER the label does not exist OR it belongs
		// to another org (the gate rejected it) — UNLESS the (item_id, label_id)
		// pair already exists, in which case ON CONFLICT DO NOTHING legitimately
		// affects zero rows. Distinguish: re-check whether the (item_id,
		// label_id) row is now present. If present, the label was already
		// attached (idempotent re-attach) — not an error. If absent, the gate or
		// a missing label_id rejected it → NOT_FOUND, the same shape a missing
		// label yields (never disclosing cross-tenant existence).
		if tag.RowsAffected() == 0 {
			var exists bool
			if err := tx.QueryRow(ctx,
				`SELECT EXISTS (SELECT 1 FROM workitems.item_labels WHERE item_id = $1 AND label_id = $2)`,
				itemID, labelID,
			).Scan(&exists); err != nil {
				return &errs.Error{Code: errs.Internal, Message: "label attach verify failed"}
			}
			if !exists {
				return &errs.Error{
					Code:    errs.NotFound,
					Message: fmt.Sprintf("label %q does not exist", labelID),
					Meta:    errs.Metadata{"label_id": labelID},
				}
			}
		}
	}
	return nil
}

// readResolvedEdges fetches deps.dependencies rows touching itemID on the
// NEAR endpoint and resolves the FAR endpoint to a ResolvedRef
// {id,title,status,kind} via a single JOIN onto workitems.items — one
// round-trip per direction (SPEC §6.2 Tool 7, round-16 / bead
// unblock-tv8.76). dir MUST be "in" or "out" (compile-time string
// constants — runtime values are NEVER accepted here per SPEC §10.1):
//
//   - "in":  edges where to_item   = itemID; the FAR (resolved) endpoint
//     is from_item — the item that blocks / relates TO itemID.
//   - "out": edges where from_item = itemID; the FAR (resolved) endpoint
//     is to_item — the item itemID blocks / relates to.
//
// The JOIN carries an org_id = $2 predicate on the target item so a
// cross-tenant neighbour resolves to zero rows and is OMITTED, never
// leaked (SPEC §6.2 lines 1841-1843; the round-16 tenant-seam discipline
// applied to a read join). orgID is the caller's org_id, pinned from the
// caller identity — NEVER from the wire. The returned slice may be empty.
func readResolvedEdges(ctx context.Context, dir, itemID, orgID string) ([]ResolvedRef, error) {
	var sql string
	switch dir {
	case "in":
		// Root is to_item; resolve the FAR from_item.
		sql = `SELECT d.from_item, i.title, i.status, d.kind
		         FROM deps.dependencies d
		         JOIN workitems.items i ON i.id = d.from_item AND i.org_id = $2
		        WHERE d.to_item = $1
		        ORDER BY d.created_at`
	case "out":
		// Root is from_item; resolve the FAR to_item.
		sql = `SELECT d.to_item, i.title, i.status, d.kind
		         FROM deps.dependencies d
		         JOIN workitems.items i ON i.id = d.to_item AND i.org_id = $2
		        WHERE d.from_item = $1
		        ORDER BY d.created_at`
	default:
		// Programmer error — caller passed something other than the
		// two whitelisted direction tokens. Surface a structured error
		// rather than concatenating the bad value into SQL.
		return nil, &errs.Error{Code: errs.Internal, Message: "readResolvedEdges: invalid direction"}
	}
	rows, err := db.Query(ctx, sql, itemID, orgID)
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "edges read failed"}
	}
	defer rows.Close()
	var out []ResolvedRef
	for rows.Next() {
		var r ResolvedRef
		if err := rows.Scan(&r.ID, &r.Title, &r.Status, &r.Kind); err != nil {
			return nil, &errs.Error{Code: errs.Internal, Message: "edge scan failed"}
		}
		out = append(out, r)
	}
	if err := rows.Err(); err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "edges iter failed"}
	}
	return out, nil
}

// readResolvedParent resolves an item's parent to a ResolvedRef
// {id,title,status} — Kind is left empty (the parent is not reached via a
// dependency edge; SPEC §4.4 line 831, round-16 / bead unblock-tv8.76).
// parentID is Item.ParentID ("" when the item has no parent). The lookup
// carries an org_id = $2 predicate so a cross-tenant parent resolves to
// zero rows and yields a nil ref (omitted, never leaked); orgID is pinned
// from the caller identity. Returns (nil, nil) when there is no parent or
// the parent is not visible to the caller.
func readResolvedParent(ctx context.Context, parentID, orgID string) (*ResolvedRef, error) {
	if parentID == "" {
		return nil, nil
	}
	var r ResolvedRef
	err := db.QueryRow(ctx,
		`SELECT id, title, status
		   FROM workitems.items
		  WHERE id = $1 AND org_id = $2`,
		parentID, orgID,
	).Scan(&r.ID, &r.Title, &r.Status)
	if err != nil {
		if errors.Is(err, sqldb.ErrNoRows) {
			// Parent exists by id reference but is not visible to this
			// caller (cross-tenant) — omit rather than leak.
			return nil, nil
		}
		return nil, &errs.Error{Code: errs.Internal, Message: "parent resolve failed"}
	}
	return &r, nil
}

// readMilestone fetches a single milestone row by id and returns it as
// a Milestone value. NotFound when the row is missing.
func readMilestone(ctx context.Context, id string) (*Milestone, error) {
	var (
		m                  Milestone
		parentID, orgID    *string
		projectID          *string
		startDate, endDate time.Time
		description        *string
		cancelledReason    *string
	)
	err := db.QueryRow(ctx,
		`SELECT id, parent_milestone_id, org_id, project_id, name, description,
		        start_date, end_date, cancelled_at, cancelled_reason,
		        created_at, updated_at
		   FROM workitems.milestones
		  WHERE id = $1`,
		id,
	).Scan(&m.ID, &parentID, &orgID, &projectID, &m.Name, &description,
		&startDate, &endDate, &m.CancelledAt, &cancelledReason,
		&m.CreatedAt, &m.UpdatedAt)
	if err != nil {
		if errors.Is(err, sqldb.ErrNoRows) {
			return nil, &errs.Error{Code: errs.NotFound, Message: "milestone not found"}
		}
		return nil, &errs.Error{Code: errs.Internal, Message: "milestone read failed"}
	}
	m.ParentMilestoneID = ptrToString(parentID)
	m.OrgID = ptrToString(orgID)
	m.ProjectID = ptrToString(projectID)
	m.Description = ptrToString(description)
	m.CancelledReason = ptrToString(cancelledReason)
	m.StartDate = startDate.Format("2006-01-02")
	m.EndDate = endDate.Format("2006-01-02")
	return &m, nil
}

// readLabel fetches a single workitems.labels row by id and returns it as
// a Label value. NotFound when the row is missing. Used after a write
// (CreateLabel / UpdateLabel) to read back the canonical persisted shape,
// including the now()-bumped updated_at. SPEC §4.4.
func readLabel(ctx context.Context, id string) (*Label, error) {
	var (
		l                Label
		orgID, projectID *string
		description      *string
	)
	err := db.QueryRow(ctx,
		`SELECT id, org_id, project_id, name, color, description, created_at, updated_at
		   FROM workitems.labels
		  WHERE id = $1`,
		id,
	).Scan(&l.ID, &orgID, &projectID, &l.Name, &l.Color, &description, &l.CreatedAt, &l.UpdatedAt)
	if err != nil {
		if errors.Is(err, sqldb.ErrNoRows) {
			return nil, &errs.Error{Code: errs.NotFound, Message: "label not found"}
		}
		return nil, &errs.Error{Code: errs.Internal, Message: "label read failed"}
	}
	l.OrgID = ptrToString(orgID)
	l.ProjectID = ptrToString(projectID)
	l.Description = ptrToString(description)
	return &l, nil
}

// ptrToString returns *p when p != nil, else "".
func ptrToString(p *string) string {
	if p == nil {
		return ""
	}
	return *p
}

// isUniqueViolation returns true when err is a Postgres UNIQUE violation
// on the named constraint. Matched by substring (pgx error wrapping
// varies by Encore version).
func isUniqueViolation(err error, constraint string) bool {
	if err == nil {
		return false
	}
	msg := err.Error()
	return strings.Contains(msg, "duplicate key") && strings.Contains(msg, constraint)
}

// isForeignKeyViolation returns true when err is a Postgres FK violation.
func isForeignKeyViolation(err error) bool {
	if err == nil {
		return false
	}
	msg := err.Error()
	return strings.Contains(msg, "foreign key") || strings.Contains(msg, "violates foreign key")
}

// isCheckViolation returns true when err is a Postgres CHECK violation
// on the named constraint.
func isCheckViolation(err error, constraint string) bool {
	if err == nil {
		return false
	}
	msg := err.Error()
	return strings.Contains(msg, "check constraint") && strings.Contains(msg, constraint)
}
