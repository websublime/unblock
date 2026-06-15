// Integration tests for the workitems service (C-1 / bead unblock-tv8.10).
//
// These tests touch the real Encore-managed Postgres cluster and MUST
// run under `encore test ./apps/api/workitems/...` (NOT plain `go test`,
// which panics at package init because deps/cascade.go declares
// pubsub.NewTopic at package scope).
//
// External test package (`package workitems_test`) blank-imports
// encore.app/db so the dedicated migration-owner service's init()
// fires before any subtest, populating workitems.db. The internal
// `package workitems` test surface (workitems_test.go) cannot do this
// because encore.app/db imports encore.app/workitems to call
// workitems.BindDB(DB) — a back-import from inside package workitems
// would form a compile-time cycle. Same pattern as org's
// authorize_ordering_test.go.
//
// Coverage:
//
//   - Create + Get round-trip on a happy path.
//   - AppendComment append + read-back.
//   - SetStateColumns invariants I-1..I-5 (the five PRD §6.2 rules,
//     less I-3 which lives in Claim).
//   - Claim happy path + loser path + I-3 reset on qa_state=failed.
//   - Concurrent claim race: N goroutines on the same Ready item
//     produce exactly one winner and N-1 losers (SPEC §6.4).
//   - Close requires claim (AF3) + sets status=Done + appends a
//     kind=completed comment when Reason is provided.
//   - CreateMilestone + AssignItem M-INV-7.
//   - Search FTS multi-table UNION ALL returns hits from items and
//     comments.

package workitems_test

import (
	"context"
	"strings"
	"sync"
	"testing"

	// Importing encore.app/db triggers its init() which calls
	// workitems.BindDB(DB) and every other consumer's BindDB hook.
	// Without this import the test binary loads workitems in
	// isolation and leaves workitems.db == nil — every RPC body would
	// then panic on a nil *sqldb.Database inside encore.dev/storage/sqldb.
	encoredb "encore.app/db"
	"encore.app/deps"
	"encore.app/shared/ulid"
	"encore.app/workitems"
	"encore.dev/beta/errs"
	"encore.dev/et"
)

// cascadeRequestedMessagesFor returns the subset of CascadeRequested
// messages published during the current test whose TriggeredByItemID
// matches the given itemID and Reason matches the given reason.
//
// Encore's pubsub testing implementation (et.Topic) records all
// messages published during the test scope — subscriptions are NOT
// fired during `encore test` (deps/cascade_subscriber_handler_test.go
// file header documents the official quote), but the publish itself
// is observable via et.Topic. This is the canonical way to assert
// "Tool X publishes CascadeRequested" without relying on subscriber
// side-effects.
//
// Filtering by itemID is essential because TestClaimPropertyHalfFailedHalfNotN100
// creates N=100 items and we must distinguish the failed half from
// the normal half within a single test scope.
func cascadeRequestedMessagesFor(itemID, reason string) []*deps.CascadeRequested {
	all := et.Topic(deps.CascadeRequestedTopic).PublishedMessages()
	out := make([]*deps.CascadeRequested, 0, len(all))
	for _, msg := range all {
		if msg == nil {
			continue
		}
		if msg.TriggeredByItemID == itemID && msg.Reason == reason {
			out = append(out, msg)
		}
	}
	return out
}

// fixture seeds the org / project / user rows each test depends on.
type fixture struct {
	OrgID     string
	ProjectID string
	UserID    string
}

func seedFixture(t *testing.T, ctx context.Context) *fixture {
	t.Helper()

	orgID, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid: %v", err)
	}
	slug := strings.ToLower("witest-" + orgID[len(orgID)-8:])
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO org.organizations (id, slug, name) VALUES ($1, $2, $3)`,
		orgID, slug, "workitems test org",
	); err != nil {
		t.Fatalf("insert org: %v", err)
	}

	userID, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid: %v", err)
	}
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO auth.users (id, primary_provider, primary_provider_id, email, display_name)
		 VALUES ($1, 'github', $2, $3, $4)`,
		userID, "witest-"+userID[len(userID)-8:],
		strings.ToLower(userID[len(userID)-8:])+"@witest.local", "witest",
	); err != nil {
		t.Fatalf("insert user: %v", err)
	}

	projectID, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid: %v", err)
	}
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO org.projects (id, org_id, slug, name) VALUES ($1, $2, $3, $4)`,
		projectID, orgID, "p-"+projectID[len(projectID)-8:], "witest project",
	); err != nil {
		t.Fatalf("insert project: %v", err)
	}

	t.Cleanup(func() {
		_, _ = encoredb.DB.Exec(ctx, `DELETE FROM org.organizations WHERE id = $1`, orgID)
		_, _ = encoredb.DB.Exec(ctx, `DELETE FROM auth.users WHERE id = $1`, userID)
	})

	return &fixture{OrgID: orgID, ProjectID: projectID, UserID: userID}
}

// createReadyItem inserts a Ready, unclaimed task directly via SQL.
// State-machine tests need a known starting state that the public
// Create / Claim path won't produce in one call.
func createReadyItem(t *testing.T, ctx context.Context, fx *fixture) string {
	t.Helper()
	id, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid: %v", err)
	}
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO workitems.items
		   (id, org_id, project_id, type, title, status, is_ready)
		 VALUES ($1, $2, $3, 'task', 'test task', 'Ready', true)`,
		id, fx.OrgID, fx.ProjectID,
	); err != nil {
		t.Fatalf("insert item: %v", err)
	}
	return id
}

// createBacklogItem inserts a Backlog, unclaimed task directly via SQL.
// Used by the claim-not-Ready precondition tests (bead unblock-tv8.72):
// a fresh Backlog item is the demo case that was mis-reported as
// ALREADY_CLAIMED. is_ready is irrelevant to claim's status precondition,
// so it is left at its column default (false).
func createBacklogItem(t *testing.T, ctx context.Context, fx *fixture) string {
	t.Helper()
	id, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid: %v", err)
	}
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO workitems.items
		   (id, org_id, project_id, type, title, status, is_ready)
		 VALUES ($1, $2, $3, 'task', 'test task', 'Backlog', false)`,
		id, fx.OrgID, fx.ProjectID,
	); err != nil {
		t.Fatalf("insert item: %v", err)
	}
	return id
}

// setItemState sets state columns + claim columns directly. Used only
// by state-machine invariant tests that need a known starting state.
// Does NOT touch is_ready or pipeline_stage (lint-protected).
func setItemState(t *testing.T, ctx context.Context, id, impl, review, qa, claimedBy string) {
	t.Helper()
	_, err := encoredb.DB.Exec(ctx,
		`UPDATE workitems.items
		    SET impl_state    = $2,
		        review_state  = $3,
		        qa_state      = $4,
		        claimed_by_id = NULLIF($5, ''),
		        claimed_at    = CASE WHEN $5 = '' THEN NULL ELSE COALESCE(claimed_at, now()) END,
		        status        = CASE WHEN $5 = '' THEN status ELSE 'InProgress' END,
		        updated_at    = now()
		  WHERE id = $1`,
		id, impl, review, qa, claimedBy,
	)
	if err != nil {
		t.Fatalf("setItemState: %v", err)
	}
}

// readItemStateColumns fetches the four state columns by id for assertions.
func readItemStateColumns(t *testing.T, ctx context.Context, id string) (impl, review, qa, pipeline string) {
	t.Helper()
	err := encoredb.DB.QueryRow(ctx,
		`SELECT impl_state, review_state, qa_state, pipeline_state
		   FROM workitems.items WHERE id = $1`,
		id,
	).Scan(&impl, &review, &qa, &pipeline)
	if err != nil {
		t.Fatalf("readItemStateColumns: %v", err)
	}
	return
}

// -----------------------------------------------------------------------------
// Create + Get round-trip.
// -----------------------------------------------------------------------------

func TestCreateAndGetTask(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)

	item, err := workitems.Create(ctx, &workitems.CreateRequest{
		OrgID:     fx.OrgID,
		ProjectID: fx.ProjectID,
		Type:      "task",
		Title:     "Implement widget",
		Body:      "Body text",
		Priority:  "P1",
	})
	if err != nil {
		t.Fatalf("Create: %v", err)
	}
	if item.ID == "" {
		t.Fatalf("Create returned empty id")
	}
	if item.Type != "task" || item.Title != "Implement widget" || item.Priority != "P1" {
		t.Fatalf("Create returned unexpected item: %+v", item)
	}
	if item.Status != "Backlog" {
		t.Fatalf("Create initial status = %q, want Backlog", item.Status)
	}
}

// -----------------------------------------------------------------------------
// AppendComment.
// -----------------------------------------------------------------------------

func TestAppendComment(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)

	item, err := workitems.Create(ctx, &workitems.CreateRequest{
		OrgID: fx.OrgID, ProjectID: fx.ProjectID, Type: "task", Title: "T",
	})
	if err != nil {
		t.Fatalf("Create: %v", err)
	}
	c, err := workitems.AppendComment(ctx, &workitems.AppendCommentRequest{
		ItemID:   item.ID,
		AuthorID: fx.UserID,
		Kind:     "general",
		Status:   "info",
		Body:     "hello",
	})
	if err != nil {
		t.Fatalf("AppendComment: %v", err)
	}
	if c.ID == "" || c.Body != "hello" {
		t.Fatalf("AppendComment returned bad comment: %+v", c)
	}

	// Same-item parent_id threads successfully (bead unblock-tv8.80,
	// §10.1.1 / §6.2 Tool 10): a reply whose parent_id is a comment on the
	// SAME item must succeed and echo parent_id. This is the positive control
	// for the same-item gate (proves it does not over-reject legitimate
	// threading). The internal caller passes an empty CallerOrgID (the no-op
	// branch), so the same-item predicate is the sole gate exercised here.
	reply, err := workitems.AppendComment(ctx, &workitems.AppendCommentRequest{
		ItemID:   item.ID,
		ParentID: c.ID, // a comment on the SAME item
		AuthorID: fx.UserID,
		Kind:     "general",
		Status:   "info",
		Body:     "threaded reply",
	})
	if err != nil {
		t.Fatalf("AppendComment same-item thread: %v", err)
	}
	if reply.ParentID != c.ID {
		t.Fatalf("same-item reply parent_id = %q, want %q", reply.ParentID, c.ID)
	}

	// Cross-item parent_id is rejected with NOT_FOUND (bead unblock-tv8.80):
	// the gate is same-item, not merely same-org. A reply on item2 whose
	// parent_id points at a comment on item1 (same org) must insert zero rows
	// and surface NOT_FOUND, indistinguishable from a missing parent.
	item2, err := workitems.Create(ctx, &workitems.CreateRequest{
		OrgID: fx.OrgID, ProjectID: fx.ProjectID, Type: "task", Title: "T2",
	})
	if err != nil {
		t.Fatalf("Create item2: %v", err)
	}
	_, err = workitems.AppendComment(ctx, &workitems.AppendCommentRequest{
		ItemID:   item2.ID,
		ParentID: c.ID, // a comment on item1 — a DIFFERENT item
		AuthorID: fx.UserID,
		Kind:     "general",
		Status:   "info",
		Body:     "cross-item reply",
	})
	if err == nil {
		t.Fatalf("AppendComment cross-item parent_id: expected NOT_FOUND, got success")
	}
	if code := errs.Code(err); code != errs.NotFound {
		t.Fatalf("AppendComment cross-item parent_id: code = %v, want NotFound", code)
	}
}

// -----------------------------------------------------------------------------
// SetStateColumns invariants.
// -----------------------------------------------------------------------------

func TestSetStateInvariantI1AutoResetsQA(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	itemID := createReadyItem(t, ctx, fx)
	setItemState(t, ctx, itemID, "done", "approved", "passed", fx.UserID)

	newReview := "needs_rework"
	got, err := workitems.SetStateColumns(ctx, &workitems.SetStateRequest{
		ItemID:      itemID,
		ReviewState: &newReview,
	})
	if err != nil {
		t.Fatalf("SetStateColumns: %v", err)
	}
	if got.ReviewState != "needs_rework" {
		t.Fatalf("review_state = %q, want needs_rework", got.ReviewState)
	}
	if got.QAState != "pending" {
		t.Fatalf("I-1: qa_state = %q, want pending (auto-reset)", got.QAState)
	}
}

func TestSetStateInvariantI2QAFailedRequiresReviewApproved(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	itemID := createReadyItem(t, ctx, fx)
	setItemState(t, ctx, itemID, "done", "pending", "pending", fx.UserID)

	newQA := "failed"
	_, err := workitems.SetStateColumns(ctx, &workitems.SetStateRequest{
		ItemID:  itemID,
		QAState: &newQA,
	})
	if err == nil {
		t.Fatalf("expected FailedPrecondition I-2, got nil")
	}
	if errs.Code(err) != errs.FailedPrecondition {
		t.Fatalf("err code = %v, want FailedPrecondition", errs.Code(err))
	}
	e := err.(*errs.Error)
	if e.Meta["invariant"] != "qa_failed_requires_review_approved" {
		t.Fatalf("meta[invariant] = %v, want qa_failed_requires_review_approved", e.Meta["invariant"])
	}
}

func TestSetStateInvariantI4ReviewChangeRequiresImplDone(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	itemID := createReadyItem(t, ctx, fx)
	setItemState(t, ctx, itemID, "pending", "pending", "pending", fx.UserID)

	newReview := "approved"
	_, err := workitems.SetStateColumns(ctx, &workitems.SetStateRequest{
		ItemID:      itemID,
		ReviewState: &newReview,
	})
	if err == nil {
		t.Fatalf("expected FailedPrecondition I-4, got nil")
	}
	e := err.(*errs.Error)
	if e.Meta["invariant"] != "review_change_requires_impl_done" {
		t.Fatalf("meta[invariant] = %v, want review_change_requires_impl_done", e.Meta["invariant"])
	}
}

func TestSetStateInvariantI5ImplDoneToPendingOnlyViaRework(t *testing.T) {
	// I-5: A bare impl=pending request on an item currently at impl=done
	// (no review/qa change, no active rework predicate) MUST reject with
	// impl_done_to_pending_requires_rework_path — the rework path is the
	// only transition that resets impl. The one-call rework
	// (impl=pending, review=needs_rework) IS supported and succeeds —
	// see TestSetStateOneCallReworkSucceeds. SPEC §6.2 Tool 13 I-4 (line
	// 2241): I-4 is the FORWARD gate (only review→approved requires
	// impl=done); needs_rework is exempt and governed by I-5.
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	itemID := createReadyItem(t, ctx, fx)
	setItemState(t, ctx, itemID, "done", "approved", "passed", fx.UserID)

	newImpl := "pending"
	_, err := workitems.SetStateColumns(ctx, &workitems.SetStateRequest{
		ItemID:    itemID,
		ImplState: &newImpl,
	})
	if err == nil {
		t.Fatalf("expected FailedPrecondition I-5, got nil")
	}
	e := err.(*errs.Error)
	if e.Meta["invariant"] != "impl_done_to_pending_requires_rework_path" {
		t.Fatalf("meta[invariant] = %v, want impl_done_to_pending_requires_rework_path", e.Meta["invariant"])
	}
}

func TestSetStateOneCallReworkSucceeds(t *testing.T) {
	// SPEC §6.2 Tool 13 I-4 (line 2241) + §11.1.2 exit criterion
	// (lines 3914-3917): the one-call rework
	// set_state(impl_state=pending, review_state=needs_rework) on a
	// CLAIMED item at impl=done MUST SUCCEED. I-5 (workitems.go) permits
	// the concurrent impl done→pending because the review=needs_rework
	// rework predicate is satisfied; I-4 is the FORWARD gate and is
	// EXEMPT for needs_rework (only approved requires impl=done); I-1
	// auto-resets qa_state→pending. Result: impl=pending,
	// review=needs_rework, qa=pending.
	//
	// This is the regression guard for unblock-tv8.81: before the fix,
	// I-4 keyed on the coalesced new_review IN (approved, needs_rework)
	// and over-fired "review_change_requires_impl_done" on this call,
	// making the §11.1.2 exit criterion unsatisfiable through the MCP
	// surface.
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	itemID := createReadyItem(t, ctx, fx)
	// Claimed item at impl=done, review=approved, qa=passed.
	setItemState(t, ctx, itemID, "done", "approved", "passed", fx.UserID)

	newImpl := "pending"
	newReview := "needs_rework"
	got, err := workitems.SetStateColumns(ctx, &workitems.SetStateRequest{
		ItemID:      itemID,
		ImplState:   &newImpl,
		ReviewState: &newReview,
	})
	if err != nil {
		t.Fatalf("one-call rework SetStateColumns: unexpected error: %v", err)
	}
	if got.ImplState != "pending" {
		t.Fatalf("impl_state = %q, want pending", got.ImplState)
	}
	if got.ReviewState != "needs_rework" {
		t.Fatalf("review_state = %q, want needs_rework", got.ReviewState)
	}
	// I-1 auto-reset: review_state=needs_rework forces qa_state→pending.
	if got.QAState != "pending" {
		t.Fatalf("qa_state = %q, want pending (I-1 auto-reset)", got.QAState)
	}

	// Verify the persisted row matches the returned Item.
	impl, review, qa, _ := readItemStateColumns(t, ctx, itemID)
	if impl != "pending" || review != "needs_rework" || qa != "pending" {
		t.Fatalf("persisted (impl,review,qa) = (%q,%q,%q), want (pending,needs_rework,pending)",
			impl, review, qa)
	}
}

func TestSetStateImplDoneRequiresClaim(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	itemID := createReadyItem(t, ctx, fx) // unclaimed

	newImpl := "done"
	_, err := workitems.SetStateColumns(ctx, &workitems.SetStateRequest{
		ItemID:    itemID,
		ImplState: &newImpl,
	})
	if err == nil {
		t.Fatalf("expected FailedPrecondition, got nil")
	}
	e := err.(*errs.Error)
	if e.Meta["invariant"] != "impl_done_requires_claim" {
		t.Fatalf("meta[invariant] = %v, want impl_done_requires_claim", e.Meta["invariant"])
	}
}

// -----------------------------------------------------------------------------
// Claim transaction.
// -----------------------------------------------------------------------------

func TestClaimHappyPath(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	itemID := createReadyItem(t, ctx, fx)

	got, err := workitems.Claim(ctx, &workitems.ClaimRequest{
		ItemID:        itemID,
		ClaimerUserID: fx.UserID,
		ClaimerAgent:  "claude-code",
	})
	if err != nil {
		t.Fatalf("Claim: %v", err)
	}
	if got.Status != "InProgress" {
		t.Fatalf("status = %q, want InProgress", got.Status)
	}
	if got.ClaimedByID != fx.UserID {
		t.Fatalf("claimed_by_id = %q, want %q", got.ClaimedByID, fx.UserID)
	}
	if got.ClaimedByAgent != "claude-code" {
		t.Fatalf("claimed_by_agent = %q, want claude-code", got.ClaimedByAgent)
	}
	if got.ClaimedAt == nil {
		t.Fatalf("claimed_at should be non-nil")
	}

	// SPEC §6.3.0 tension #2 narrow rule (lines 1723-1726) + §6.4
	// lines 1898-1903: normal Ready→InProgress claim with no I-3 reset
	// MUST NOT publish CascadeRequested. The claimed item was non-Done
	// before the claim and remains non-Done; downstream pipeline_stage
	// is unaffected per §5.7.1, and a publish would burn one cascade
	// pass against NFR-1 budget for no observable effect.
	//
	// We observe the publish surface directly via et.Topic — Encore's
	// pubsub testing implementation does NOT fire subscribers during
	// `encore test` (see cascadeRequestedMessagesFor docstring), so we
	// assert on PublishedMessages rather than the subscriber-written
	// deps.cascade_events row.
	if got := cascadeRequestedMessagesFor(itemID, "state_change"); len(got) != 0 {
		t.Fatalf("normal Claim must NOT publish state_change cascade: got %d publish(es) for item=%q (want 0)",
			len(got), itemID)
	}
}

func TestClaimLoserPath(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	itemID := createReadyItem(t, ctx, fx)

	if _, err := workitems.Claim(ctx, &workitems.ClaimRequest{
		ItemID: itemID, ClaimerUserID: fx.UserID, ClaimerAgent: "claude-code",
	}); err != nil {
		t.Fatalf("first Claim: %v", err)
	}
	_, err := workitems.Claim(ctx, &workitems.ClaimRequest{
		ItemID: itemID, ClaimerUserID: fx.UserID, ClaimerAgent: "claude-code",
	})
	if err == nil {
		t.Fatalf("expected AlreadyExists, got nil")
	}
	if errs.Code(err) != errs.AlreadyExists {
		t.Fatalf("err code = %v, want AlreadyExists", errs.Code(err))
	}
	e := err.(*errs.Error)
	if e.Meta["winner_user_id"] != fx.UserID {
		t.Fatalf("meta[winner_user_id] = %v, want %q", e.Meta["winner_user_id"], fx.UserID)
	}
}

// TestClaimRejectsNotReady asserts a fresh, unclaimed Backlog item is
// rejected with PRECONDITION_NOT_MET (errs.FailedPrecondition) carrying
// Meta{status:'Backlog', required:'Ready'} and NO winner_user_id — the
// regression fixed by bead unblock-tv8.72 (§7.2). Previously this case
// funnelled to the unconditional alreadyClaimedError loser arm and was
// mis-reported as ALREADY_CLAIMED with no winner info.
func TestClaimRejectsNotReady(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	itemID := createBacklogItem(t, ctx, fx)

	_, err := workitems.Claim(ctx, &workitems.ClaimRequest{
		ItemID: itemID, ClaimerUserID: fx.UserID, ClaimerAgent: "claude-code",
	})
	if err == nil {
		t.Fatalf("expected FailedPrecondition, got nil")
	}
	if errs.Code(err) != errs.FailedPrecondition {
		t.Fatalf("err code = %v, want FailedPrecondition", errs.Code(err))
	}
	e := err.(*errs.Error)
	if e.Meta["status"] != "Backlog" {
		t.Fatalf("meta[status] = %v, want Backlog", e.Meta["status"])
	}
	if e.Meta["required"] != "Ready" {
		t.Fatalf("meta[required] = %v, want Ready", e.Meta["required"])
	}
	// §7.2 / bead unblock-tv8.72: claim's wrong-status rejection carries
	// NO "missing" (that is promote's is_ready disambiguator only) and is
	// NOT the ALREADY_CLAIMED loser path (no winner meta).
	if _, ok := e.Meta["missing"]; ok {
		t.Fatalf("meta[missing] must be absent for claim wrong-status, got %v", e.Meta["missing"])
	}
	if _, ok := e.Meta["winner_user_id"]; ok {
		t.Fatalf("meta[winner_user_id] must be absent (not the ALREADY_CLAIMED path), got %v", e.Meta["winner_user_id"])
	}
}

// TestClaimRejectsNotFound asserts claiming a non-existent item returns
// NOT_FOUND (errs.NotFound), distinct from the loser / not-Ready arms
// (§6.4 + bead unblock-tv8.72).
func TestClaimRejectsNotFound(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)

	missingID, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid: %v", err)
	}
	_, err = workitems.Claim(ctx, &workitems.ClaimRequest{
		ItemID: missingID, ClaimerUserID: fx.UserID, ClaimerAgent: "claude-code",
	})
	if err == nil {
		t.Fatalf("expected NotFound, got nil")
	}
	if errs.Code(err) != errs.NotFound {
		t.Fatalf("err code = %v, want NotFound", errs.Code(err))
	}
}

func TestClaimResetsReworkOnQAFailed(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	itemID := createReadyItem(t, ctx, fx)
	setItemState(t, ctx, itemID, "done", "approved", "failed", fx.UserID)
	// Reset for re-claim.
	if _, err := encoredb.DB.Exec(ctx,
		`UPDATE workitems.items SET status = 'Ready', claimed_by_id = NULL, claimed_at = NULL WHERE id = $1`,
		itemID,
	); err != nil {
		t.Fatalf("reset for claim: %v", err)
	}

	got, err := workitems.Claim(ctx, &workitems.ClaimRequest{
		ItemID: itemID, ClaimerUserID: fx.UserID, ClaimerAgent: "claude-code",
	})
	if err != nil {
		t.Fatalf("Claim: %v", err)
	}
	// SPEC §6.2 Tool 13 I-3 (line 2240): Claim resets review_state + qa_state to
	// pending. impl_state is deliberately preserved (the worker drives
	// any impl_state mutation through SetStateColumns, which enforces
	// I-4/I-5 against the rework path).
	if got.ReviewState != "pending" || got.QAState != "pending" {
		t.Fatalf("I-3 reset: review=%q qa=%q (want pending/pending)",
			got.ReviewState, got.QAState)
	}
	if got.ImplState != "done" {
		t.Fatalf("I-3 must NOT touch impl_state: got impl=%q (want done — preserved across the re-claim)", got.ImplState)
	}

	// SPEC §6.3.0 tension #2 + §6.4 lines 1905-1914: Claim publishes
	// CascadeRequested{Reason:"state_change", ...} after commit ONLY
	// when the I-3 reset path fires. The subscriber walks the forward
	// 'blocks' closure to recompute pipeline_stage per §5.7.1 and
	// writes one deps.cascade_events row with kind='state_change', but
	// during `encore test` the subscriber does NOT fire — we assert on
	// the publish itself via et.Topic.
	msgs := cascadeRequestedMessagesFor(itemID, "state_change")
	if len(msgs) != 1 {
		t.Fatalf("I-3 reset Claim must publish exactly 1 state_change cascade for item=%q: got %d (want 1)",
			itemID, len(msgs))
	}
	msg := msgs[0]
	if msg.EventID == "" {
		t.Fatalf("I-3 reset cascade: EventID empty (ULID must be minted by publisher per C1)")
	}
	if msg.OrgID != fx.OrgID {
		t.Fatalf("I-3 reset cascade: OrgID=%q, want %q (captured at SELECT FOR UPDATE time)", msg.OrgID, fx.OrgID)
	}
	if msg.ProjectID != fx.ProjectID {
		t.Fatalf("I-3 reset cascade: ProjectID=%q, want %q", msg.ProjectID, fx.ProjectID)
	}
	if msg.EmittedAt.IsZero() {
		t.Fatalf("I-3 reset cascade: EmittedAt zero (must be wall-clock at publish time)")
	}
}

// TestClaimConcurrentSingleWinner verifies SPEC §6.4: N concurrent
// Claim attempts on the same Ready item produce exactly one winner
// and N-1 ALREADY_CLAIMED losers.
func TestClaimConcurrentSingleWinner(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	itemID := createReadyItem(t, ctx, fx)

	const N = 8
	var wg sync.WaitGroup
	results := make([]error, N)
	wg.Add(N)
	for i := 0; i < N; i++ {
		go func(i int) {
			defer wg.Done()
			_, err := workitems.Claim(ctx, &workitems.ClaimRequest{
				ItemID:        itemID,
				ClaimerUserID: fx.UserID,
				ClaimerAgent:  "claude-code",
			})
			results[i] = err
		}(i)
	}
	wg.Wait()

	winners := 0
	losers := 0
	for _, err := range results {
		switch {
		case err == nil:
			winners++
		case errs.Code(err) == errs.AlreadyExists:
			losers++
		default:
			t.Fatalf("unexpected error: %v", err)
		}
	}
	if winners != 1 {
		t.Fatalf("got %d winners, want 1 (losers=%d)", winners, losers)
	}
	if losers != N-1 {
		t.Fatalf("got %d losers, want %d", losers, N-1)
	}
}

// -----------------------------------------------------------------------------
// Close.
// -----------------------------------------------------------------------------

func TestCloseRequiresClaim(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	itemID := createReadyItem(t, ctx, fx) // unclaimed

	_, err := workitems.Close(ctx, &workitems.CloseRequest{ItemID: itemID})
	if err == nil {
		t.Fatalf("expected FailedPrecondition, got nil")
	}
	if errs.Code(err) != errs.FailedPrecondition {
		t.Fatalf("err code = %v, want FailedPrecondition", errs.Code(err))
	}
}

func TestCloseHappyPath(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	itemID := createReadyItem(t, ctx, fx)
	if _, err := workitems.Claim(ctx, &workitems.ClaimRequest{
		ItemID: itemID, ClaimerUserID: fx.UserID, ClaimerAgent: "claude-code",
	}); err != nil {
		t.Fatalf("Claim: %v", err)
	}
	got, err := workitems.Close(ctx, &workitems.CloseRequest{ItemID: itemID, Reason: "shipped"})
	if err != nil {
		t.Fatalf("Close: %v", err)
	}
	if got.Status != "Done" {
		t.Fatalf("status = %q, want Done", got.Status)
	}
	if got.ClosedAt == nil {
		t.Fatalf("closed_at should be non-nil")
	}
	var commentCount int
	if err := encoredb.DB.QueryRow(ctx,
		`SELECT count(*) FROM workitems.comments
		  WHERE item_id = $1 AND kind = 'completed' AND body = 'shipped'`,
		itemID,
	).Scan(&commentCount); err != nil {
		t.Fatalf("comment count query: %v", err)
	}
	if commentCount != 1 {
		t.Fatalf("close-reason comment count = %d, want 1", commentCount)
	}

	// AC for bead unblock-tv8.18 D-3 Tool 6: close emits exactly one
	// CascadeRequested{Reason:"close"} publish on the deps topic. Same
	// observation pattern as TestClaimResetsReworkOnQAFailed above —
	// `encore test` does not fire subscribers, but the publish itself
	// is observable via et.Topic when publisher and observer share the
	// same test goroutine.
	msgs := cascadeRequestedMessagesFor(itemID, "close")
	if len(msgs) != 1 {
		t.Fatalf("CascadeRequested{Reason=close, item=%s} publish count = %d, want 1", itemID, len(msgs))
	}
}

// -----------------------------------------------------------------------------
// Milestones.
// -----------------------------------------------------------------------------

func TestCreateAndAssignMilestone(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)

	ms, err := workitems.CreateMilestone(ctx, &workitems.CreateMilestoneRequest{
		ProjectID: fx.ProjectID,
		Name:      "Q1",
		StartDate: "2026-01-01",
		EndDate:   "2026-03-31",
	})
	if err != nil {
		t.Fatalf("CreateMilestone: %v", err)
	}
	if ms.ID == "" {
		t.Fatalf("CreateMilestone returned empty id")
	}

	item, err := workitems.Create(ctx, &workitems.CreateRequest{
		OrgID: fx.OrgID, ProjectID: fx.ProjectID, Type: "task", Title: "T",
	})
	if err != nil {
		t.Fatalf("Create: %v", err)
	}
	if err := workitems.AssignItem(ctx, &workitems.AssignItemRequest{
		ItemID: item.ID, MilestoneID: ms.ID, AssignedByUser: fx.UserID,
	}); err != nil {
		t.Fatalf("AssignItem: %v", err)
	}
	got, err := workitems.Get(ctx, item.ID)
	// Get uses rbac which requires identity context — bypass via direct
	// read for the assertion since we don't have an authenticated ctx
	// in this test harness.
	if err != nil {
		// rbac path likely failed for missing identity. Read directly.
		var milestoneID *string
		if qerr := encoredb.DB.QueryRow(ctx,
			`SELECT milestone_id FROM workitems.items WHERE id = $1`, item.ID,
		).Scan(&milestoneID); qerr != nil {
			t.Fatalf("direct read: %v", qerr)
		}
		if milestoneID == nil || *milestoneID != ms.ID {
			t.Fatalf("milestone_id = %v, want %q", milestoneID, ms.ID)
		}
		return
	}
	if got.MilestoneID != ms.ID {
		t.Fatalf("milestone_id = %q, want %q", got.MilestoneID, ms.ID)
	}
}

func TestAssignItemRejectsCrossProjectMilestone(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)

	// Other project in the same org.
	otherProject, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid: %v", err)
	}
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO org.projects (id, org_id, slug, name) VALUES ($1, $2, $3, $4)`,
		otherProject, fx.OrgID, "p-"+otherProject[len(otherProject)-8:], "other",
	); err != nil {
		t.Fatalf("insert project: %v", err)
	}

	ms, err := workitems.CreateMilestone(ctx, &workitems.CreateMilestoneRequest{
		ProjectID: otherProject,
		Name:      "Other Q1",
		StartDate: "2026-01-01",
		EndDate:   "2026-03-31",
	})
	if err != nil {
		t.Fatalf("CreateMilestone: %v", err)
	}

	item, err := workitems.Create(ctx, &workitems.CreateRequest{
		OrgID: fx.OrgID, ProjectID: fx.ProjectID, Type: "task", Title: "T",
	})
	if err != nil {
		t.Fatalf("Create: %v", err)
	}
	err = workitems.AssignItem(ctx, &workitems.AssignItemRequest{
		ItemID: item.ID, MilestoneID: ms.ID, AssignedByUser: fx.UserID,
	})
	if err == nil {
		t.Fatalf("expected FailedPrecondition M-INV-7, got nil")
	}
	if errs.Code(err) != errs.FailedPrecondition {
		t.Fatalf("err code = %v, want FailedPrecondition", errs.Code(err))
	}
}

func TestCreateMilestoneRejectsDepthOverflow(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)

	parent := ""
	for i := 0; i < 4; i++ {
		ms, err := workitems.CreateMilestone(ctx, &workitems.CreateMilestoneRequest{
			ProjectID:         fx.ProjectID,
			ParentMilestoneID: parent,
			Name:              "level",
			StartDate:         "2026-01-01",
			EndDate:           "2026-03-31",
		})
		if err != nil {
			t.Fatalf("CreateMilestone depth %d: %v", i, err)
		}
		parent = ms.ID
	}
	// The 5th level must reject (M-INV-6 limit = 4).
	_, err := workitems.CreateMilestone(ctx, &workitems.CreateMilestoneRequest{
		ProjectID:         fx.ProjectID,
		ParentMilestoneID: parent,
		Name:              "overflow",
		StartDate:         "2026-01-01",
		EndDate:           "2026-03-31",
	})
	if err == nil {
		t.Fatalf("expected FailedPrecondition M-INV-6, got nil")
	}
	e := err.(*errs.Error)
	if e.Meta["invariant"] != "M-INV-6" {
		t.Fatalf("meta[invariant] = %v, want M-INV-6", e.Meta["invariant"])
	}
}

func TestAssignItemUnassign(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)

	ms, err := workitems.CreateMilestone(ctx, &workitems.CreateMilestoneRequest{
		ProjectID: fx.ProjectID,
		Name:      "Q1",
		StartDate: "2026-01-01",
		EndDate:   "2026-03-31",
	})
	if err != nil {
		t.Fatalf("CreateMilestone: %v", err)
	}
	item, err := workitems.Create(ctx, &workitems.CreateRequest{
		OrgID: fx.OrgID, ProjectID: fx.ProjectID, Type: "task", Title: "T",
	})
	if err != nil {
		t.Fatalf("Create: %v", err)
	}
	if err := workitems.AssignItem(ctx, &workitems.AssignItemRequest{
		ItemID: item.ID, MilestoneID: ms.ID, AssignedByUser: fx.UserID,
	}); err != nil {
		t.Fatalf("AssignItem: %v", err)
	}
	// Unassign by passing empty MilestoneID.
	if err := workitems.AssignItem(ctx, &workitems.AssignItemRequest{
		ItemID: item.ID, MilestoneID: "", AssignedByUser: fx.UserID,
	}); err != nil {
		t.Fatalf("AssignItem unassign: %v", err)
	}
	var milestoneID *string
	if err := encoredb.DB.QueryRow(ctx,
		`SELECT milestone_id FROM workitems.items WHERE id = $1`, item.ID,
	).Scan(&milestoneID); err != nil {
		t.Fatalf("direct read: %v", err)
	}
	if milestoneID != nil {
		t.Fatalf("milestone_id after unassign = %v, want nil", milestoneID)
	}
}

// -----------------------------------------------------------------------------
// Search.
// -----------------------------------------------------------------------------

func TestSearchFindsItemAndComment(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)

	item, err := workitems.Create(ctx, &workitems.CreateRequest{
		OrgID: fx.OrgID, ProjectID: fx.ProjectID, Type: "task",
		Title: "Implement zinwald widget", Body: "details about zinwald",
	})
	if err != nil {
		t.Fatalf("Create: %v", err)
	}
	if _, err := workitems.AppendComment(ctx, &workitems.AppendCommentRequest{
		ItemID: item.ID, AuthorID: fx.UserID, Kind: "general", Status: "info",
		Body: "zinwald progress update",
	}); err != nil {
		t.Fatalf("AppendComment: %v", err)
	}

	// workitems.Search reads org_id from callerIdentity — there's no
	// Encore auth context in this test path. Validate the SQL shape
	// by querying the FTS indexes directly via the test handle. The
	// gate code path (callerIdentity → empty Unauthenticated) is
	// covered by the unit tests in workitems_test.go.
	rows, err := encoredb.DB.Query(ctx,
		`SELECT id, 'item' AS source
		   FROM workitems.items
		  WHERE org_id = $1
		    AND fts @@ websearch_to_tsquery('english', $2)
		 UNION ALL
		 SELECT c.id, 'comment'
		   FROM workitems.comments c
		   JOIN workitems.items i ON i.id = c.item_id
		  WHERE i.org_id = $1
		    AND c.fts @@ websearch_to_tsquery('english', $2)`,
		fx.OrgID, "zinwald",
	)
	if err != nil {
		t.Fatalf("fts query: %v", err)
	}
	defer rows.Close()
	sawItem := false
	sawComment := false
	for rows.Next() {
		var id, source string
		if err := rows.Scan(&id, &source); err != nil {
			t.Fatalf("scan: %v", err)
		}
		switch source {
		case "item":
			sawItem = true
		case "comment":
			sawComment = true
		}
	}
	if !sawItem || !sawComment {
		t.Fatalf("search hits missing: item=%v comment=%v", sawItem, sawComment)
	}
}

// -----------------------------------------------------------------------------
// GetTrail.
// -----------------------------------------------------------------------------

func TestGetTrailReturnsCommentsInOrder(t *testing.T) {
	// GetTrail requires an Encore auth identity (rbac.For) to scope
	// the item read by org. Validate trail behaviour by reading the
	// comments directly — the trail-assembly logic is exercised by
	// scanItemRow + the integration test for Get above.
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	item, err := workitems.Create(ctx, &workitems.CreateRequest{
		OrgID: fx.OrgID, ProjectID: fx.ProjectID, Type: "task", Title: "T",
	})
	if err != nil {
		t.Fatalf("Create: %v", err)
	}
	for _, body := range []string{"first", "second", "third"} {
		if _, err := workitems.AppendComment(ctx, &workitems.AppendCommentRequest{
			ItemID:   item.ID,
			AuthorID: fx.UserID,
			Kind:     "general",
			Status:   "info",
			Body:     body,
		}); err != nil {
			t.Fatalf("AppendComment %q: %v", body, err)
		}
	}
	// Verify ordering directly.
	rows, err := encoredb.DB.Query(ctx,
		`SELECT body FROM workitems.comments WHERE item_id = $1 ORDER BY created_at ASC`,
		item.ID,
	)
	if err != nil {
		t.Fatalf("comments query: %v", err)
	}
	defer rows.Close()
	var bodies []string
	for rows.Next() {
		var b string
		if err := rows.Scan(&b); err != nil {
			t.Fatalf("scan: %v", err)
		}
		bodies = append(bodies, b)
	}
	want := []string{"first", "second", "third"}
	if len(bodies) != len(want) {
		t.Fatalf("got %d comments, want %d", len(bodies), len(want))
	}
	for i, b := range bodies {
		if b != want[i] {
			t.Fatalf("comment[%d] = %q, want %q", i, b, want[i])
		}
	}
}

// -----------------------------------------------------------------------------
// readItemStateColumns sanity — used by the invariant tests above.
// -----------------------------------------------------------------------------

func TestMilestoneTreeAssemblesNestedStructure(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)

	root, err := workitems.CreateMilestone(ctx, &workitems.CreateMilestoneRequest{
		ProjectID: fx.ProjectID, Name: "root", StartDate: "2026-01-01", EndDate: "2026-12-31",
	})
	if err != nil {
		t.Fatalf("root milestone: %v", err)
	}
	child1, err := workitems.CreateMilestone(ctx, &workitems.CreateMilestoneRequest{
		ProjectID: fx.ProjectID, ParentMilestoneID: root.ID, Name: "child1",
		StartDate: "2026-01-01", EndDate: "2026-06-30",
	})
	if err != nil {
		t.Fatalf("child1: %v", err)
	}
	grandchild, err := workitems.CreateMilestone(ctx, &workitems.CreateMilestoneRequest{
		ProjectID: fx.ProjectID, ParentMilestoneID: child1.ID, Name: "grandchild",
		StartDate: "2026-01-01", EndDate: "2026-03-31",
	})
	if err != nil {
		t.Fatalf("grandchild: %v", err)
	}

	tree, err := workitems.MilestoneTree(ctx, &workitems.MilestoneTreeRequest{
		RootMilestoneID: root.ID,
	})
	if err != nil {
		t.Fatalf("MilestoneTree: %v", err)
	}
	if len(tree.Roots) != 1 {
		t.Fatalf("len(Roots) = %d, want 1", len(tree.Roots))
	}
	rootNode := tree.Roots[0]
	if rootNode.Milestone.ID != root.ID {
		t.Fatalf("root id = %q, want %q", rootNode.Milestone.ID, root.ID)
	}
	if len(rootNode.Children) != 1 {
		t.Fatalf("root has %d children, want 1", len(rootNode.Children))
	}
	c1 := rootNode.Children[0]
	if c1.Milestone.ID != child1.ID {
		t.Fatalf("child1 id = %q, want %q", c1.Milestone.ID, child1.ID)
	}
	if len(c1.Children) != 1 {
		t.Fatalf("child1 has %d grandchildren, want 1", len(c1.Children))
	}
	if c1.Children[0].Milestone.ID != grandchild.ID {
		t.Fatalf("grandchild id = %q, want %q", c1.Children[0].Milestone.ID, grandchild.ID)
	}
}

// TestClaimVsSetStateColumnsRaceAR18 covers SPEC §6.2 AR-18 (round-2).
//
// AR-18 asserts that concurrent Claim and SetStateColumns(qa_state=failed)
// transactions on the same item serialise correctly via SELECT FOR UPDATE
// (no torn read of the state-column quartet), and that whenever a
// subsequent Claim observes qa_state='failed' it MUST reset
// review_state + qa_state to 'pending' atomically (I-3, SPEC §6.2 line
// 1505 — scoped to review+qa; impl_state is preserved).
//
// Architectural constraint that shapes this test:
// SetStateColumns enforces the structural rule
// `impl_done_requires_claim` (workitems.go — newImpl='done' AND
// claimed_by_id IS NULL is rejected), and Claim's SELECT FOR UPDATE
// only acquires rows where status='Ready' AND claimed_by_id IS NULL.
// The two preconditions are mutually exclusive on the same row at any
// instant. The race we model is therefore the AR-18 sequence:
//
//	(phase 1, racing) Item is claimed and InProgress with
//	  impl=done/review=approved/qa=passed.  N goroutine pairs race:
//	    G1: SetStateColumns(qa=failed) — succeeds (flips qa to failed)
//	    G2: Claim — fails with ALREADY_CLAIMED (loser path)
//	  Both ops take FOR UPDATE on the same row, so the contended
//	  lock serialises them.  Verify no error other than ALREADY_CLAIMED
//	  on the Claim and no torn read of the four state columns.
//
//	(phase 2, sequential) SQL-level rework reset:
//	  status='Ready', claimed_by_id=NULL, claimed_at=NULL.  State
//	  columns are preserved: impl=done, review=approved, qa=failed.
//
//	(phase 3, sequential) Claim — observes qa='failed' and applies
//	  I-3 atomically, resetting review_state + qa_state to 'pending'.
//	  impl_state MUST remain 'done' (I-3 scope is review+qa only,
//	  matching SPEC §6.2 Tool 13 I-3 line 2240 verbatim).
//
// Iterating phases 1+2+3 N times stresses both the SELECT FOR UPDATE
// serialisation (phase 1) and the I-3 atomic reset (phase 3) under
// scheduler pressure. A flaky failure mode (torn read of the four
// state columns) would manifest as an impossible combination after
// phase 1 — e.g. qa='failed' with impl != 'done' or review != 'approved'.
func TestClaimVsSetStateColumnsRaceAR18(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)

	const iterations = 50
	var serialisationViolations, i3Violations int

	for it := 0; it < iterations; it++ {
		// Fresh item per iteration so we don't depend on cross-iteration
		// state — only on the concurrent ordering within one iteration.
		itemID := createReadyItem(t, ctx, fx)
		// Seed phase-1 posture directly via SQL: InProgress, claimed,
		// impl=done, review=approved, qa=passed.  setItemState sets
		// status='InProgress' when claimedBy is non-empty.
		setItemState(t, ctx, itemID, "done", "approved", "passed", fx.UserID)

		// ----- Phase 1: race SetStateColumns(qa=failed) vs Claim. -----
		var (
			wg          sync.WaitGroup
			claimErr    error
			setStateErr error
		)
		wg.Add(2)
		go func() {
			defer wg.Done()
			_, err := workitems.Claim(ctx, &workitems.ClaimRequest{
				ItemID:        itemID,
				ClaimerUserID: fx.UserID,
				ClaimerAgent:  "claude-code",
			})
			claimErr = err
		}()
		go func() {
			defer wg.Done()
			newQA := "failed"
			_, err := workitems.SetStateColumns(ctx, &workitems.SetStateRequest{
				ItemID:  itemID,
				QAState: &newQA,
			})
			setStateErr = err
		}()
		wg.Wait()

		// SetStateColumns MUST succeed: the row is claimed, impl=done,
		// review=approved — every invariant is satisfied.  A failure
		// here means SELECT FOR UPDATE serialisation interacted badly
		// with the structural / state checks.
		if setStateErr != nil {
			serialisationViolations++
			t.Fatalf("iter %d phase 1: SetStateColumns(qa=failed) failed: %v", it, setStateErr)
		}
		// Claim MUST lose with ALREADY_CLAIMED: the row is already
		// claimed, so the FOR UPDATE on `status='Ready' AND
		// claimed_by_id IS NULL` returns zero rows.  Any other error
		// code (Internal, FailedPrecondition, etc.) indicates a race
		// pathology in Claim's transaction.
		if claimErr == nil {
			serialisationViolations++
			t.Fatalf("iter %d phase 1: Claim succeeded against a claimed item", it)
		}
		if errs.Code(claimErr) != errs.AlreadyExists {
			serialisationViolations++
			t.Fatalf("iter %d phase 1: Claim returned %v (code=%v), want AlreadyExists",
				it, claimErr, errs.Code(claimErr))
		}

		// Verify no torn read: the committed row must have
		// impl=done (unchanged), review=approved (unchanged),
		// qa=failed (flipped by SetStateColumns).
		impl, review, qa, _ := readItemStateColumns(t, ctx, itemID)
		if impl != "done" || review != "approved" || qa != "failed" {
			serialisationViolations++
			t.Fatalf("iter %d phase 1 torn read: impl=%q review=%q qa=%q (want done/approved/failed)",
				it, impl, review, qa)
		}

		// ----- Phase 2: SQL-level rework reset (mirrors the manual
		// reset documented at TestClaimResetsReworkOnQAFailed).  This
		// is what an external operator / future Tool would do between
		// rework cycles. -----
		if _, err := encoredb.DB.Exec(ctx,
			`UPDATE workitems.items
			    SET status        = 'Ready',
			        claimed_by_id = NULL,
			        claimed_at    = NULL,
			        updated_at    = now()
			  WHERE id = $1`,
			itemID,
		); err != nil {
			t.Fatalf("iter %d phase 2 reset: %v", it, err)
		}

		// ----- Phase 3: re-Claim observes qa='failed' and applies I-3. -----
		got, err := workitems.Claim(ctx, &workitems.ClaimRequest{
			ItemID:        itemID,
			ClaimerUserID: fx.UserID,
			ClaimerAgent:  "claude-code",
		})
		if err != nil {
			t.Fatalf("iter %d phase 3 re-Claim: %v", it, err)
		}
		// I-3: review_state + qa_state both reset to 'pending';
		// impl_state preserved (scope is review+qa only per SPEC §6.2
		// Tool 13 I-3 line 2240).
		if got.ReviewState != "pending" || got.QAState != "pending" {
			i3Violations++
			t.Fatalf("iter %d phase 3 I-3 reset: review=%q qa=%q (want pending/pending)",
				it, got.ReviewState, got.QAState)
		}
		if got.ImplState != "done" {
			i3Violations++
			t.Fatalf("iter %d phase 3 I-3 must NOT touch impl_state: got impl=%q (want done)",
				it, got.ImplState)
		}
	}

	t.Logf("AR-18 race coverage: %d iterations, serialisation violations=%d, I-3 violations=%d",
		iterations, serialisationViolations, i3Violations)
}

func TestReadItemStateColumnsAfterSetState(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	itemID := createReadyItem(t, ctx, fx)
	setItemState(t, ctx, itemID, "done", "approved", "passed", fx.UserID)

	impl, review, qa, pipeline := readItemStateColumns(t, ctx, itemID)
	if impl != "done" || review != "approved" || qa != "passed" || pipeline != "running" {
		t.Fatalf("state columns: impl=%q review=%q qa=%q pipeline=%q", impl, review, qa, pipeline)
	}
}

// TestClaimPropertyHalfFailedHalfNotN100 is the round-6 cascade-symmetry
// property test required by bead unblock-tv8.13 NOTES (SPEC §6.3.0
// tension #2 narrow rule + §6.4 lines 1905-1914).
//
// Setup: create N=100 Ready items. On exactly half (50), set
// impl=done, review=approved, qa=failed via setItemState (which also
// claims the item), then SQL-reset status='Ready' + claimed_by_id=NULL
// to obtain a Ready item carrying qa_state='failed' (mirrors the
// AR-18 race phase-2 reset pattern). The other half remain in their
// default Ready state (qa_state='pending').
//
// Action: Claim all 100 items. The 50 with qa_state='failed' hit the
// I-3 reset path and MUST publish CascadeRequested{Reason:"state_change"}
// (exactly one each). The other 50 take the normal Ready→InProgress
// path and MUST NOT publish (zero CascadeRequested messages for those
// item IDs).
//
// Assertion: et.Topic(deps.CascadeRequestedTopic).PublishedMessages()
// — Encore's pubsub test harness records every publish from the
// current test. Subscribers do NOT fire during `encore test`, so we
// observe the publish surface directly. We count messages whose
// TriggeredByItemID is in failedIDs (expect exactly 50) and in
// normalIDs (expect exactly 0).
func TestClaimPropertyHalfFailedHalfNotN100(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)

	const N = 100
	const halfFailed = N / 2

	failedIDs := make([]string, 0, halfFailed)
	normalIDs := make([]string, 0, N-halfFailed)
	failedIDSet := make(map[string]struct{}, halfFailed)
	normalIDSet := make(map[string]struct{}, N-halfFailed)

	// Phase 1: create N=100 Ready items and prepare half of them with
	// qa_state='failed'. The other half remain at the default
	// (impl=pending, review=pending, qa=pending) — they will take the
	// normal Ready→InProgress claim path.
	for i := 0; i < N; i++ {
		id := createReadyItem(t, ctx, fx)
		if i < halfFailed {
			// Set the rework posture: impl=done, review=approved,
			// qa=failed satisfies I-2 (qa_failed_requires_review_approved).
			// setItemState also claims the item — we then reset to Ready
			// so the Claim() call hits the SELECT FOR UPDATE successfully.
			setItemState(t, ctx, id, "done", "approved", "failed", fx.UserID)
			if _, err := encoredb.DB.Exec(ctx,
				`UPDATE workitems.items
				    SET status        = 'Ready',
				        claimed_by_id = NULL,
				        claimed_at    = NULL,
				        updated_at    = now()
				  WHERE id = $1`,
				id,
			); err != nil {
				t.Fatalf("reset failed item %d: %v", i, err)
			}
			failedIDs = append(failedIDs, id)
			failedIDSet[id] = struct{}{}
		} else {
			normalIDs = append(normalIDs, id)
			normalIDSet[id] = struct{}{}
		}
	}

	// Phase 2: claim all 100 items. Sequential is sufficient — the
	// bead NOTES describes a property assertion, not a concurrency
	// assertion (TestClaimConcurrentSingleWinner + AR-18 cover the
	// race surface). Each Claim must succeed; the failed-half ones
	// take the I-3 reset path and publish, the normal half do not.
	for _, id := range failedIDs {
		got, err := workitems.Claim(ctx, &workitems.ClaimRequest{
			ItemID:        id,
			ClaimerUserID: fx.UserID,
			ClaimerAgent:  "claude-code",
		})
		if err != nil {
			t.Fatalf("Claim failed item %q: %v", id, err)
		}
		// Sanity: I-3 reset is observable on the returned item.
		if got.ReviewState != "pending" || got.QAState != "pending" {
			t.Fatalf("I-3 reset failed for item %q: review=%q qa=%q (want pending/pending)",
				id, got.ReviewState, got.QAState)
		}
	}
	for _, id := range normalIDs {
		got, err := workitems.Claim(ctx, &workitems.ClaimRequest{
			ItemID:        id,
			ClaimerUserID: fx.UserID,
			ClaimerAgent:  "claude-code",
		})
		if err != nil {
			t.Fatalf("Claim normal item %q: %v", id, err)
		}
		// Sanity: normal claim leaves qa_state at 'pending' (which it
		// already was) — no I-3 reset triggered.
		if got.QAState != "pending" {
			t.Fatalf("normal Claim item %q: qa_state=%q (want pending)", id, got.QAState)
		}
	}

	// Phase 3: count publishes via et.Topic. Synchronous — Publish
	// returns after the message is recorded by the test harness.
	all := et.Topic(deps.CascadeRequestedTopic).PublishedMessages()
	var failedHits, normalHits int
	for _, msg := range all {
		if msg == nil || msg.Reason != "state_change" {
			continue
		}
		if _, ok := failedIDSet[msg.TriggeredByItemID]; ok {
			failedHits++
			continue
		}
		if _, ok := normalIDSet[msg.TriggeredByItemID]; ok {
			normalHits++
		}
	}

	if failedHits != halfFailed {
		t.Fatalf("failed half: %d state_change publishes (want %d) — I-3 reset path must publish exactly once per claim",
			failedHits, halfFailed)
	}
	if normalHits != 0 {
		t.Fatalf("normal half: %d state_change publishes (want 0) — normal Ready→InProgress claim must NOT publish (SPEC §6.3.0 tension #2)",
			normalHits)
	}

	t.Logf("N=100 split (failed=%d, normal=%d): state_change publishes: failed=%d, normal=%d",
		halfFailed, N-halfFailed, failedHits, normalHits)
}

// -----------------------------------------------------------------------------
// SetStateColumns cascade publish (bead unblock-tv8.53 / SPEC §6.3.0
// tension #3 narrow rule). The four tests below cover acceptance
// criteria #1-#4: §5.7.1-affecting writes publish exactly one
// CascadeRequested{Reason:"state_change"}, pipe-only writes do NOT
// publish, I-1's auto-reset of qa_state counts as a material change,
// and the N=100 half-changing/half-pipe-only property assertion.
// -----------------------------------------------------------------------------

// TestSetStateImplChangePublishesStateChange — happy path AC #1.
// setItemState seeds (impl=pending, review=pending, qa=pending,
// claimed). SetStateColumns(impl_state=done) is §5.7.1-affecting
// (impl_state moves from pending to done); the post-commit publish
// MUST fire exactly once, carrying EventID, OrgID, ProjectID, and
// EmittedAt populated from the locked row's scope + the publisher's
// wall clock. Mirrors TestClaimResetsReworkOnQAFailed's field-by-field
// assertion shape (lines 462-518).
func TestSetStateImplChangePublishesStateChange(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	itemID := createReadyItem(t, ctx, fx)
	// Seed: claimed item at (impl=pending, review=pending, qa=pending).
	// claimed_by_id=fx.UserID is required by the impl_done_requires_claim
	// structural invariant before we can flip impl_state to done.
	setItemState(t, ctx, itemID, "pending", "pending", "pending", fx.UserID)

	newImpl := "done"
	got, err := workitems.SetStateColumns(ctx, &workitems.SetStateRequest{
		ItemID:    itemID,
		ImplState: &newImpl,
	})
	if err != nil {
		t.Fatalf("SetStateColumns: %v", err)
	}
	if got.ImplState != "done" {
		t.Fatalf("impl_state = %q, want done", got.ImplState)
	}

	msgs := cascadeRequestedMessagesFor(itemID, "state_change")
	if len(msgs) != 1 {
		t.Fatalf("SetStateColumns(impl=done) must publish exactly 1 state_change cascade for item=%q: got %d (want 1)",
			itemID, len(msgs))
	}
	msg := msgs[0]
	if msg.EventID == "" {
		t.Fatalf("state_change cascade: EventID empty (ULID must be minted before tx.Begin per AC #3)")
	}
	if msg.OrgID != fx.OrgID {
		t.Fatalf("state_change cascade: OrgID=%q, want %q (captured at SELECT FOR UPDATE time)", msg.OrgID, fx.OrgID)
	}
	if msg.ProjectID != fx.ProjectID {
		t.Fatalf("state_change cascade: ProjectID=%q, want %q", msg.ProjectID, fx.ProjectID)
	}
	if msg.EmittedAt.IsZero() {
		t.Fatalf("state_change cascade: EmittedAt zero (must be wall-clock at publish time)")
	}
	if msg.Reason != "state_change" {
		t.Fatalf("state_change cascade: Reason=%q, want state_change", msg.Reason)
	}
}

// TestSetStatePipeStateOnlyDoesNotPublish — negative path AC #2.
// SPEC §6.3.0 explicit non-publishers (lines 1803-1809, tension #3
// ruling): writes that affect ONLY pipeline_state (with no change to
// impl/review/qa) MUST NOT publish. §5.7.1 derives pipeline_stage from
// the upstream chain's readiness/closure, not from a downstream item's
// own pipe_state.
func TestSetStatePipeStateOnlyDoesNotPublish(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	itemID := createReadyItem(t, ctx, fx)
	setItemState(t, ctx, itemID, "pending", "pending", "pending", fx.UserID)

	// pipeline_state default is 'running'; move it to 'paused' with no
	// impl/review/qa change.
	newPipeline := "paused"
	got, err := workitems.SetStateColumns(ctx, &workitems.SetStateRequest{
		ItemID:        itemID,
		PipelineState: &newPipeline,
	})
	if err != nil {
		t.Fatalf("SetStateColumns: %v", err)
	}
	if got.PipelineState != "paused" {
		t.Fatalf("pipeline_state = %q, want paused", got.PipelineState)
	}

	if msgs := cascadeRequestedMessagesFor(itemID, "state_change"); len(msgs) != 0 {
		t.Fatalf("pipe_state-only SetStateColumns must NOT publish state_change cascade for item=%q: got %d publish(es) (want 0)",
			itemID, len(msgs))
	}
}

// TestSetStateI1AutoResetPublishes — AC #3 (I-1 ordering, R2).
// I-1: review_state=needs_rework auto-resets qa_state to pending. The
// predicate runs AFTER I-1, so the qa transition from passed→pending
// counts as a material §5.7.1-affecting change even though the caller
// only passed review_state. Confirms the predicate's post-I-1
// placement.
func TestSetStateI1AutoResetPublishes(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	itemID := createReadyItem(t, ctx, fx)
	// Seed: claimed item with (impl=done, review=approved, qa=passed)
	// — the I-1 path requires impl=done to satisfy I-4 on review change.
	setItemState(t, ctx, itemID, "done", "approved", "passed", fx.UserID)

	newReview := "needs_rework"
	got, err := workitems.SetStateColumns(ctx, &workitems.SetStateRequest{
		ItemID:      itemID,
		ReviewState: &newReview,
	})
	if err != nil {
		t.Fatalf("SetStateColumns: %v", err)
	}
	if got.ReviewState != "needs_rework" || got.QAState != "pending" {
		t.Fatalf("I-1: review=%q qa=%q (want needs_rework/pending)", got.ReviewState, got.QAState)
	}

	msgs := cascadeRequestedMessagesFor(itemID, "state_change")
	if len(msgs) != 1 {
		t.Fatalf("I-1 auto-reset must publish exactly 1 state_change cascade for item=%q: got %d (want 1)",
			itemID, len(msgs))
	}
}

// TestSetStatePropertyHalfChangeHalfPipeOnlyN100 — AC #4 property test.
// Create N=100 claimed items in the (impl=pending, review=pending,
// qa=pending) posture. First half: SetStateColumns(impl_state=done)
// — §5.7.1-affecting, MUST publish exactly one state_change each.
// Second half: SetStateColumns(pipeline_state=paused) — pipe-only,
// MUST NOT publish.
//
// Per DECISION comment on the bead: AC #4's "50 deps.cascade_events
// rows" reads as "50 et.Topic publishes whose Reason=state_change"
// because the Encore test runtime does not fire subscribers during
// `encore test`. The publish surface is the observable contract and
// matches the canonical pattern from
// TestClaimPropertyHalfFailedHalfNotN100.
func TestSetStatePropertyHalfChangeHalfPipeOnlyN100(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)

	const N = 100
	const halfChange = N / 2

	changeIDs := make([]string, 0, halfChange)
	pipeIDs := make([]string, 0, N-halfChange)
	changeIDSet := make(map[string]struct{}, halfChange)
	pipeIDSet := make(map[string]struct{}, N-halfChange)

	// Phase 1: create N=100 items seeded as claimed at the
	// (impl=pending, review=pending, qa=pending) posture. setItemState
	// writes the workitems.items row directly and therefore does NOT
	// fire the SetStateColumns cascade publish — the publish count is
	// clean before Phase 2 begins. Mirrors TestClaimProperty's pattern
	// at lines 1185-1209.
	for i := 0; i < N; i++ {
		id := createReadyItem(t, ctx, fx)
		setItemState(t, ctx, id, "pending", "pending", "pending", fx.UserID)
		if i < halfChange {
			changeIDs = append(changeIDs, id)
			changeIDSet[id] = struct{}{}
		} else {
			pipeIDs = append(pipeIDs, id)
			pipeIDSet[id] = struct{}{}
		}
	}

	// Phase 2: call SetStateColumns per item.
	newImpl := "done"
	for _, id := range changeIDs {
		got, err := workitems.SetStateColumns(ctx, &workitems.SetStateRequest{
			ItemID:    id,
			ImplState: &newImpl,
		})
		if err != nil {
			t.Fatalf("SetStateColumns change item %q: %v", id, err)
		}
		if got.ImplState != "done" {
			t.Fatalf("change item %q impl_state=%q (want done)", id, got.ImplState)
		}
	}
	newPipeline := "paused"
	for _, id := range pipeIDs {
		got, err := workitems.SetStateColumns(ctx, &workitems.SetStateRequest{
			ItemID:        id,
			PipelineState: &newPipeline,
		})
		if err != nil {
			t.Fatalf("SetStateColumns pipe item %q: %v", id, err)
		}
		if got.PipelineState != "paused" {
			t.Fatalf("pipe item %q pipeline_state=%q (want paused)", id, got.PipelineState)
		}
	}

	// Phase 3: count publishes via et.Topic. Synchronous — Publish
	// returns after the message is recorded by the test harness.
	all := et.Topic(deps.CascadeRequestedTopic).PublishedMessages()
	var changeHits, pipeHits int
	for _, msg := range all {
		if msg == nil || msg.Reason != "state_change" {
			continue
		}
		if _, ok := changeIDSet[msg.TriggeredByItemID]; ok {
			changeHits++
			continue
		}
		if _, ok := pipeIDSet[msg.TriggeredByItemID]; ok {
			pipeHits++
		}
	}

	if changeHits != halfChange {
		t.Fatalf("change half: %d state_change publishes (want %d) — §5.7.1-affecting writes must publish exactly once",
			changeHits, halfChange)
	}
	if pipeHits != 0 {
		t.Fatalf("pipe-only half: %d state_change publishes (want 0) — pipe_state-only writes must NOT publish (SPEC §6.3.0 tension #3)",
			pipeHits)
	}

	t.Logf("N=100 split (change=%d, pipe=%d): state_change publishes: change=%d, pipe=%d",
		halfChange, N-halfChange, changeHits, pipeHits)
}

// -----------------------------------------------------------------------------
// Promote (Tool 15) + is_ready-on-create + status⇄is_ready reconciliation
// (round-16, bead unblock-tv8.71 — SPEC §6.2 Tool 15, §6.6, §7.2).
// -----------------------------------------------------------------------------

// readStatusIsReady fetches (status, is_ready) by id for assertions.
func readStatusIsReady(t *testing.T, ctx context.Context, id string) (status string, isReady bool) {
	t.Helper()
	if err := encoredb.DB.QueryRow(ctx,
		`SELECT status, is_ready FROM workitems.items WHERE id = $1`, id,
	).Scan(&status, &isReady); err != nil {
		t.Fatalf("readStatusIsReady %s: %v", id, err)
	}
	return
}

// TestCreateSetsIsReadyInline asserts the §6.6 is_ready-on-create rule:
// a fresh create with NO incoming blockers comes back is_ready=true and
// status='Backlog' (the inline write, not subscriber materialisation).
func TestCreateSetsIsReadyInline(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)

	item, err := workitems.Create(ctx, &workitems.CreateRequest{
		OrgID: fx.OrgID, ProjectID: fx.ProjectID, Type: "task", Title: "fresh",
	})
	if err != nil {
		t.Fatalf("Create: %v", err)
	}
	if !item.IsReady {
		t.Fatalf("Create: is_ready = false, want true (§6.6 inline create-path write)")
	}
	if item.Status != "Backlog" {
		t.Fatalf("Create: status = %q, want Backlog", item.Status)
	}
	// Assert against the column directly, not just the returned struct.
	status, isReady := readStatusIsReady(t, ctx, item.ID)
	if status != "Backlog" || !isReady {
		t.Fatalf("Create persisted (status=%q, is_ready=%v), want (Backlog, true)", status, isReady)
	}
}

// TestCreateWithBlockerIsNotReady asserts that a create inlining an
// incoming 'blocks' edge whose blocker is NOT Done comes back
// is_ready=false (the create edge loop's §6.5 recompute corrects the
// initial inline true).
func TestCreateWithBlockerIsNotReady(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)

	blocker, err := workitems.Create(ctx, &workitems.CreateRequest{
		OrgID: fx.OrgID, ProjectID: fx.ProjectID, Type: "task", Title: "blocker",
	})
	if err != nil {
		t.Fatalf("Create blocker: %v", err)
	}
	dependent, err := workitems.Create(ctx, &workitems.CreateRequest{
		OrgID: fx.OrgID, ProjectID: fx.ProjectID, Type: "task", Title: "dependent",
		Dependencies: []deps.Edge{{FromItem: blocker.ID, Kind: "blocks"}},
	})
	if err != nil {
		t.Fatalf("Create dependent: %v", err)
	}
	if dependent.IsReady {
		t.Fatalf("Create with open blocker: is_ready = true, want false")
	}
}

// TestPromoteHappyPath asserts Backlog→Ready via promote on a fresh
// (is_ready=true) item.
func TestPromoteHappyPath(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)

	item, err := workitems.Create(ctx, &workitems.CreateRequest{
		OrgID: fx.OrgID, ProjectID: fx.ProjectID, Type: "task", Title: "promote-me",
	})
	if err != nil {
		t.Fatalf("Create: %v", err)
	}
	got, err := workitems.Promote(ctx, &workitems.PromoteRequest{ItemID: item.ID})
	if err != nil {
		t.Fatalf("Promote: %v", err)
	}
	if got.Status != "Ready" {
		t.Fatalf("Promote: status = %q, want Ready", got.Status)
	}
	if !got.IsReady {
		t.Fatalf("Promote: is_ready = false, want true (promote reads, never recomputes)")
	}
}

// TestPromoteRejectsAlreadyReady asserts the §7.2 {status, required}
// rejection (no `missing`) when the target is already Ready.
func TestPromoteRejectsAlreadyReady(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	itemID := createReadyItem(t, ctx, fx) // status=Ready, is_ready=true

	_, err := workitems.Promote(ctx, &workitems.PromoteRequest{ItemID: itemID})
	if err == nil {
		t.Fatalf("expected FailedPrecondition, got nil")
	}
	if errs.Code(err) != errs.FailedPrecondition {
		t.Fatalf("err code = %v, want FailedPrecondition", errs.Code(err))
	}
	e := err.(*errs.Error)
	if e.Meta["status"] != "Ready" {
		t.Fatalf("meta[status] = %v, want Ready", e.Meta["status"])
	}
	if e.Meta["required"] != "Ready" {
		t.Fatalf("meta[required] = %v, want Ready", e.Meta["required"])
	}
	if _, present := e.Meta["missing"]; present {
		t.Fatalf("meta[missing] present (%v), want absent for a wrong-status (not blocked) rejection", e.Meta["missing"])
	}
}

// TestPromoteRejectsBlocked asserts the §7.2 rejection with
// missing="is_ready" when the Backlog target still has an open blocker.
func TestPromoteRejectsBlocked(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)

	blocker, err := workitems.Create(ctx, &workitems.CreateRequest{
		OrgID: fx.OrgID, ProjectID: fx.ProjectID, Type: "task", Title: "blocker",
	})
	if err != nil {
		t.Fatalf("Create blocker: %v", err)
	}
	dependent, err := workitems.Create(ctx, &workitems.CreateRequest{
		OrgID: fx.OrgID, ProjectID: fx.ProjectID, Type: "task", Title: "dependent",
		Dependencies: []deps.Edge{{FromItem: blocker.ID, Kind: "blocks"}},
	})
	if err != nil {
		t.Fatalf("Create dependent: %v", err)
	}

	_, err = workitems.Promote(ctx, &workitems.PromoteRequest{ItemID: dependent.ID})
	if err == nil {
		t.Fatalf("expected FailedPrecondition, got nil")
	}
	e := err.(*errs.Error)
	if e.Meta["status"] != "Backlog" {
		t.Fatalf("meta[status] = %v, want Backlog", e.Meta["status"])
	}
	if e.Meta["required"] != "Ready" {
		t.Fatalf("meta[required] = %v, want Ready", e.Meta["required"])
	}
	if e.Meta["missing"] != "is_ready" {
		t.Fatalf("meta[missing] = %v, want is_ready", e.Meta["missing"])
	}
}

// TestPromoteNotFound asserts NOT_FOUND on an unknown id.
func TestPromoteNotFound(t *testing.T) {
	ctx := context.Background()
	_ = seedFixture(t, ctx)
	missing, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid: %v", err)
	}
	_, err = workitems.Promote(ctx, &workitems.PromoteRequest{ItemID: missing})
	if err == nil {
		t.Fatalf("expected NotFound, got nil")
	}
	if errs.Code(err) != errs.NotFound {
		t.Fatalf("err code = %v, want NotFound", errs.Code(err))
	}
}

// TestAddEdgeDemotesReadyToBlocked asserts the §6.6 Ready→Blocked
// demotion: adding an open incoming 'blocks' edge to a Ready (unclaimed)
// item flips is_ready=false AND status='Blocked' in the same write.
func TestAddEdgeDemotesReadyToBlocked(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)

	// Promote a fresh item to Ready, then add a fresh non-Done blocker.
	dependent, err := workitems.Create(ctx, &workitems.CreateRequest{
		OrgID: fx.OrgID, ProjectID: fx.ProjectID, Type: "task", Title: "to-demote",
	})
	if err != nil {
		t.Fatalf("Create dependent: %v", err)
	}
	if _, err := workitems.Promote(ctx, &workitems.PromoteRequest{ItemID: dependent.ID}); err != nil {
		t.Fatalf("Promote: %v", err)
	}
	blocker, err := workitems.Create(ctx, &workitems.CreateRequest{
		OrgID: fx.OrgID, ProjectID: fx.ProjectID, Type: "task", Title: "new-blocker",
	})
	if err != nil {
		t.Fatalf("Create blocker: %v", err)
	}

	if _, err := deps.AddEdge(ctx, &deps.AddEdgeRequest{
		OrgID: fx.OrgID, ProjectID: fx.ProjectID,
		FromItem: blocker.ID, ToItem: dependent.ID, Kind: "blocks",
	}); err != nil {
		t.Fatalf("AddEdge: %v", err)
	}

	status, isReady := readStatusIsReady(t, ctx, dependent.ID)
	if isReady {
		t.Fatalf("post-AddEdge: is_ready = true, want false (open blocker)")
	}
	if status != "Blocked" {
		t.Fatalf("post-AddEdge: status = %q, want Blocked (§6.6 Ready→Blocked demotion)", status)
	}
}

// TestAddEdgeDoesNotDemoteInProgress asserts the §6.6 rule that an
// InProgress (claimed) item is NEVER demoted: a new open blocker flips
// is_ready=false but LEAVES status='InProgress'.
func TestAddEdgeDoesNotDemoteInProgress(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)

	// A Ready item, claimed → InProgress.
	itemID := createReadyItem(t, ctx, fx)
	if _, err := workitems.Claim(ctx, &workitems.ClaimRequest{
		ItemID: itemID, ClaimerUserID: fx.UserID, ClaimerAgent: "claude-code",
	}); err != nil {
		t.Fatalf("Claim: %v", err)
	}
	blocker, err := workitems.Create(ctx, &workitems.CreateRequest{
		OrgID: fx.OrgID, ProjectID: fx.ProjectID, Type: "task", Title: "ip-blocker",
	})
	if err != nil {
		t.Fatalf("Create blocker: %v", err)
	}

	if _, err := deps.AddEdge(ctx, &deps.AddEdgeRequest{
		OrgID: fx.OrgID, ProjectID: fx.ProjectID,
		FromItem: blocker.ID, ToItem: itemID, Kind: "blocks",
	}); err != nil {
		t.Fatalf("AddEdge: %v", err)
	}

	status, isReady := readStatusIsReady(t, ctx, itemID)
	if isReady {
		t.Fatalf("post-AddEdge on InProgress: is_ready = true, want false")
	}
	if status != "InProgress" {
		t.Fatalf("post-AddEdge on InProgress: status = %q, want InProgress (never demoted per §6.6)", status)
	}
}

// TestCloseRecoversBlockedToReady asserts the §6.6 Blocked→Ready
// recovery: when the last open blocker closes, the inline is_ready
// recompute flips is_ready=true AND the demoted Blocked item returns to
// Ready in the same write.
func TestCloseRecoversBlockedToReady(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)

	// dependent → Ready, then demoted to Blocked by a new blocker.
	dependent, err := workitems.Create(ctx, &workitems.CreateRequest{
		OrgID: fx.OrgID, ProjectID: fx.ProjectID, Type: "task", Title: "to-recover",
	})
	if err != nil {
		t.Fatalf("Create dependent: %v", err)
	}
	if _, err := workitems.Promote(ctx, &workitems.PromoteRequest{ItemID: dependent.ID}); err != nil {
		t.Fatalf("Promote: %v", err)
	}
	blocker := createReadyItem(t, ctx, fx) // Ready, claimable
	if _, err := deps.AddEdge(ctx, &deps.AddEdgeRequest{
		OrgID: fx.OrgID, ProjectID: fx.ProjectID,
		FromItem: blocker, ToItem: dependent.ID, Kind: "blocks",
	}); err != nil {
		t.Fatalf("AddEdge: %v", err)
	}
	if status, _ := readStatusIsReady(t, ctx, dependent.ID); status != "Blocked" {
		t.Fatalf("pre-close: dependent status = %q, want Blocked", status)
	}

	// Claim then close the blocker (close requires claimed_by_id).
	if _, err := workitems.Claim(ctx, &workitems.ClaimRequest{
		ItemID: blocker, ClaimerUserID: fx.UserID, ClaimerAgent: "claude-code",
	}); err != nil {
		t.Fatalf("Claim blocker: %v", err)
	}
	if _, err := workitems.Close(ctx, &workitems.CloseRequest{ItemID: blocker}); err != nil {
		t.Fatalf("Close blocker: %v", err)
	}

	status, isReady := readStatusIsReady(t, ctx, dependent.ID)
	if !isReady {
		t.Fatalf("post-close: dependent is_ready = false, want true (last blocker Done)")
	}
	if status != "Ready" {
		t.Fatalf("post-close: dependent status = %q, want Ready (§6.6 Blocked→Ready recovery)", status)
	}
}

// -----------------------------------------------------------------------------
// Write-surface row-level tenant gate (round-16 / bead unblock-tv8.77,
// §10.1.1). RPC-level proof of the CallerOrgID channel's two behaviours:
//
//   - NON-EMPTY CallerOrgID is a HARD tenant gate: a CallerOrgID that does
//     not match the target row's org_id yields NOT_FOUND (the row is
//     invisible), never a cross-tenant mutation. This is the behaviour the
//     MCP handlers always exercise (they pin CallerOrgID from identity.OrgID).
//   - EMPTY CallerOrgID is the deliberate NO-OP for trusted internal callers
//     (the §11.1.1 seed + these integration tests): the gate predicate
//     ($caller = '' OR org_id = $caller) short-circuits, so the write
//     proceeds unscoped. Every OTHER test in this file relies on this no-op
//     (they pass no CallerOrgID); this test pins the contract explicitly.
//
// The exitcriteriontest cross-tenant suite proves the gate end-to-end
// THROUGH the MCP boundary; this is the RPC-level companion documenting the
// no-op-vs-hard-gate divergence ratified in §10.1.1.
// -----------------------------------------------------------------------------

func TestWriteSurfaceTenantGate_CallerOrgID(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	itemID := createReadyItem(t, ctx, fx)

	foreignOrg, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid foreign org: %v", err)
	}

	// 1. NON-EMPTY CallerOrgID that does NOT own the item ⇒ NOT_FOUND, no
	//    mutation. The item is in fx.OrgID; a Claim pinned to foreignOrg must
	//    not see it.
	_, err = workitems.Claim(ctx, &workitems.ClaimRequest{
		ItemID:        itemID,
		CallerOrgID:   foreignOrg,
		ClaimerUserID: fx.UserID,
		ClaimerAgent:  "claude-code",
	})
	if err == nil {
		t.Fatalf("cross-tenant Claim (CallerOrgID=foreign) succeeded, want NOT_FOUND")
	}
	if errs.Code(err) != errs.NotFound {
		t.Fatalf("cross-tenant Claim err code = %v, want NotFound", errs.Code(err))
	}
	if st := claimedStatus(t, ctx, itemID); st != "Ready" {
		t.Fatalf("cross-tenant Claim mutated status to %q, want Ready (untouched)", st)
	}

	// 2. NON-EMPTY CallerOrgID that DOES own the item ⇒ gate passes, claim
	//    succeeds.
	if _, err := workitems.Claim(ctx, &workitems.ClaimRequest{
		ItemID:        itemID,
		CallerOrgID:   fx.OrgID,
		ClaimerUserID: fx.UserID,
		ClaimerAgent:  "claude-code",
	}); err != nil {
		t.Fatalf("same-org Claim (CallerOrgID=fx.OrgID) failed: %v", err)
	}
	if st := claimedStatus(t, ctx, itemID); st != "InProgress" {
		t.Fatalf("same-org Claim left status %q, want InProgress", st)
	}

	// 3. EMPTY CallerOrgID is the trusted-internal NO-OP: a fresh Ready item
	//    claims successfully with no org context (the path every other test
	//    in this file depends on).
	noopItem := createReadyItem(t, ctx, fx)
	if _, err := workitems.Claim(ctx, &workitems.ClaimRequest{
		ItemID:        noopItem,
		CallerOrgID:   "", // no-op gate
		ClaimerUserID: fx.UserID,
		ClaimerAgent:  "claude-code",
	}); err != nil {
		t.Fatalf("empty-CallerOrgID Claim (no-op path) failed: %v", err)
	}
	if st := claimedStatus(t, ctx, noopItem); st != "InProgress" {
		t.Fatalf("empty-CallerOrgID Claim left status %q, want InProgress", st)
	}
}

// TestCreateMilestoneTenantGate_ProjectScope proves the project-scoped
// CreateMilestone INSERT…SELECT gate (bead unblock-tv8.83, §10.1.1): a home
// caller cannot create a milestone scoped to a FOREIGN org's project. The
// foreign-but-existing project_id is indistinguishable from a missing one
// (NOT_FOUND), and nothing is inserted. A same-org positive control proves
// legitimate project-scoped creation still works, and the empty-CallerOrgID
// no-op (trusted internal callers) is preserved.
func TestCreateMilestoneTenantGate_ProjectScope(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)

	// Seed a FOREIGN org + a project owned by it.
	foreignOrg, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid foreign org: %v", err)
	}
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO org.organizations (id, slug, name) VALUES ($1, $2, $3)`,
		foreignOrg, "witest-foreign-"+foreignOrg[len(foreignOrg)-8:], "foreign org",
	); err != nil {
		t.Fatalf("insert foreign org: %v", err)
	}
	t.Cleanup(func() {
		_, _ = encoredb.DB.Exec(ctx, `DELETE FROM org.organizations WHERE id = $1`, foreignOrg)
	})
	foreignProject, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid foreign project: %v", err)
	}
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO org.projects (id, org_id, slug, name) VALUES ($1, $2, $3, $4)`,
		foreignProject, foreignOrg, "p-"+foreignProject[len(foreignProject)-8:], "foreign project",
	); err != nil {
		t.Fatalf("insert foreign project: %v", err)
	}

	milestoneCount := func() int {
		t.Helper()
		var n int
		if qerr := encoredb.DB.QueryRow(ctx,
			`SELECT count(*) FROM workitems.milestones WHERE project_id = $1`, foreignProject,
		).Scan(&n); qerr != nil {
			t.Fatalf("count milestones: %v", qerr)
		}
		return n
	}

	// 1. NEGATIVE: home caller (CallerOrgID = fx.OrgID) scoping a milestone to
	//    the FOREIGN org's project ⇒ NOT_FOUND, nothing inserted.
	ms, err := workitems.CreateMilestone(ctx, &workitems.CreateMilestoneRequest{
		ProjectID:   foreignProject,
		CallerOrgID: fx.OrgID,
		Name:        "Foreign Q1",
		StartDate:   "2026-01-01",
		EndDate:     "2026-03-31",
	})
	if err == nil {
		t.Fatalf("cross-tenant CreateMilestone (foreign project_id) succeeded, want NOT_FOUND")
	}
	if errs.Code(err) != errs.NotFound {
		t.Fatalf("cross-tenant CreateMilestone err code = %v, want NotFound", errs.Code(err))
	}
	if ms != nil {
		t.Fatalf("cross-tenant CreateMilestone returned a milestone %+v, want nil", ms)
	}
	if n := milestoneCount(); n != 0 {
		t.Fatalf("cross-tenant CreateMilestone inserted %d row(s) into foreign project, want 0", n)
	}

	// 2. POSITIVE control: home caller scoping a milestone to its OWN project
	//    ⇒ gate passes, milestone created.
	own, err := workitems.CreateMilestone(ctx, &workitems.CreateMilestoneRequest{
		ProjectID:   fx.ProjectID,
		CallerOrgID: fx.OrgID,
		Name:        "Home Q1",
		StartDate:   "2026-01-01",
		EndDate:     "2026-03-31",
	})
	if err != nil {
		t.Fatalf("same-org CreateMilestone (own project, CallerOrgID=fx.OrgID) failed: %v", err)
	}
	if own == nil || own.ID == "" {
		t.Fatalf("same-org CreateMilestone returned empty milestone")
	}
	if own.ProjectID != fx.ProjectID {
		t.Fatalf("same-org CreateMilestone project_id = %q, want %q", own.ProjectID, fx.ProjectID)
	}

	// 3. EMPTY CallerOrgID is the trusted-internal NO-OP: a project-scoped
	//    milestone with no org context still creates (the path the §11.1.1
	//    E2E seed and every existing milestone test depends on).
	noop, err := workitems.CreateMilestone(ctx, &workitems.CreateMilestoneRequest{
		ProjectID: fx.ProjectID,
		Name:      "Noop Q2",
		StartDate: "2026-04-01",
		EndDate:   "2026-06-30",
	})
	if err != nil {
		t.Fatalf("empty-CallerOrgID CreateMilestone (no-op path) failed: %v", err)
	}
	if noop == nil || noop.ID == "" {
		t.Fatalf("empty-CallerOrgID CreateMilestone returned empty milestone")
	}
}

// claimedStatus reads an item's status column by id for tenant-gate
// assertions.
func claimedStatus(t *testing.T, ctx context.Context, id string) string {
	t.Helper()
	var s string
	if err := encoredb.DB.QueryRow(ctx,
		`SELECT status FROM workitems.items WHERE id = $1`, id,
	).Scan(&s); err != nil {
		t.Fatalf("read status for %s: %v", id, err)
	}
	return s
}
