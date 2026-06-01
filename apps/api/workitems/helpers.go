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

	"encore.app/deps"
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
// transaction handle. Errors map FK violations to NotFound and unique
// violations to AlreadyExists.
func attachLabelsTx(ctx context.Context, tx *sqldb.Tx, itemID string, labels []string) error {
	for _, labelID := range labels {
		if labelID == "" {
			continue
		}
		_, err := tx.Exec(ctx,
			`INSERT INTO workitems.item_labels (item_id, label_id) VALUES ($1, $2)
			 ON CONFLICT (item_id, label_id) DO NOTHING`,
			itemID, labelID,
		)
		if err != nil {
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
	}
	return nil
}

// readEdges fetches deps.dependencies rows where the named column
// equals itemID. col MUST be "from_item" or "to_item" (compile-time
// string constants — runtime values are NEVER accepted here per
// SPEC §10.1). The returned slice may be empty.
func readEdges(ctx context.Context, col, itemID string) ([]deps.Edge, error) {
	var sql string
	switch col {
	case "from_item":
		sql = `SELECT id, from_item, to_item, kind, created_at, COALESCE(created_by, '')
		         FROM deps.dependencies
		        WHERE from_item = $1
		        ORDER BY created_at`
	case "to_item":
		sql = `SELECT id, from_item, to_item, kind, created_at, COALESCE(created_by, '')
		         FROM deps.dependencies
		        WHERE to_item = $1
		        ORDER BY created_at`
	default:
		// Programmer error — caller passed something other than the
		// two whitelisted column names. Surface a structured error
		// rather than concatenating the bad value into SQL.
		return nil, &errs.Error{Code: errs.Internal, Message: "readEdges: invalid column"}
	}
	rows, err := db.Query(ctx, sql, itemID)
	if err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "edges read failed"}
	}
	defer rows.Close()
	var out []deps.Edge
	for rows.Next() {
		var e deps.Edge
		if err := rows.Scan(&e.ID, &e.FromItem, &e.ToItem, &e.Kind, &e.CreatedAt, &e.CreatedBy); err != nil {
			return nil, &errs.Error{Code: errs.Internal, Message: "edge scan failed"}
		}
		out = append(out, e)
	}
	if err := rows.Err(); err != nil {
		return nil, &errs.Error{Code: errs.Internal, Message: "edges iter failed"}
	}
	return out, nil
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
//
//nolint:unused
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
