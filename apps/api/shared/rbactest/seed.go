// seed.go provisions the fixture two-org cross-membership graph the
// matrix sweep asserts isolation against. The seed runs once from
// TestMain and is torn down once at exit; per-subtest cleanup is
// avoided because the matrix subtests are read-only on the fixture.
//
// Fixture shape (B-3 scope — auth + org schemas):
//
//   - Two org.organizations rows (Org A, Org B).
//   - Per side: four auth.users rows, one per non-agent role
//     (owner/admin/member/viewer). The agent identity is constructed
//     in-memory only — never an auth.users or org.members row, per
//     SPEC §4.3.2 step 8.
//   - Per side: one org.members row per non-agent user, binding the
//     user to its home org with the matching role. Authorize's
//     effective-role derivation reads these rows.
//   - Per side: one org.projects row (the canonical project under
//     each org). Used as KindOrgScoped row-leak bait for org.projects
//     and as the parent of project_members rows below.
//   - Per side: one org.project_members row binding the project's
//     home-org owner user to the project. project_members has no
//     org_id column — Authorize gates it via the parent project's
//     org_id (apps/api/org/org.go step 3 containment check).
//   - Per side: leak-bait rows in the org_id-free auth schema
//     (auth.users with a side-specific email; auth.oauth_tokens
//     bound to that user; auth.sessions bound to that user). These
//     never have an org_id of their own — the suite asserts
//     Authorize denies cross-org reads on the resource identifier,
//     not on the row itself.
//   - Per side: one mcp.api_keys row. Foreshadows the E-3
//     (unblock-tv8.25) matrix extension; the B-3 sweep does NOT
//     assert on this row but seeding it now means the FK chain is
//     exercised end-to-end and the C-6/E-3 follow-up bead adds
//     assertions without re-touching seed.go.
//
// Constraints:
//
//   - Every id is a freshly-minted ULID via apps/api/shared/ulid.
//     Hard-coded ids would clash on the schema's UNIQUE constraints
//     across encore-test invocations that share a long-lived dev
//     cluster. A clean teardown is still done at exit (best-effort);
//     the ULID prefix is the safety net.
//   - The org_id-free auth schema is seeded with leak-bait rows
//     whose `primary_provider_id` carries the side label so the
//     suite can identify which side a given auth.users row belongs
//     to without inventing a synthetic org_id column.
//   - All rows go through direct sqldb.Exec, NOT through the
//     auth/org RPCs. The RPC surfaces require an Encore auth context
//     the test cannot easily fabricate (the authhandler reads a
//     real `Authorization: Bearer …` header). Direct INSERT lets
//     the seed install the rows without dancing through the auth
//     mesh, and the suite still drives Authorize / rbac.For through
//     the production code paths — only the fixture provisioning
//     bypasses the RPC layer.

package rbactest

import (
	"context"
	"fmt"
	"strings"
	"testing"
	"time"

	"encore.app/shared/ulid"
	"encore.dev/rlog"
	"encore.dev/storage/sqldb"
)

// Fixture is the materialised seed graph. Held in memory between
// TestMain seed-in and the matrix sweep so individual subtests can
// reference role-tagged ids without re-querying the DB.
type Fixture struct {
	// Orgs maps the OrgA / OrgB label to the persisted ULID.
	Orgs map[string]string

	// Users maps (org-label, role) -> auth.users.id. Populated for
	// the four non-agent roles only; the agent role is constructed
	// in-memory by Identity helpers (see matrix_test.go).
	Users map[userKey]string

	// Projects maps the OrgA / OrgB label to the persisted ULID for
	// the canonical project row under each org.
	Projects map[string]string

	// APIKeyRows maps the OrgA / OrgB label to the persisted ULID for
	// the mcp.api_keys row seeded under each org. B-3 does not
	// assert on these rows; the seed exists so the FK chain is
	// validated end-to-end and the C-6/E-3 extensions inherit a
	// non-empty mcp.api_keys table.
	APIKeyRows map[string]string

	// Items maps the OrgA / OrgB label to the persisted ULID for the
	// canonical workitems.items row seeded under each org. Used by
	// C-6 (unblock-tv8.15) as KindOrgScoped row-leak bait for
	// workitems.items (rbac.For path) and as the parent of the
	// workitems.comments row below.
	Items map[string]string

	// Comments maps the OrgA / OrgB label to the persisted ULID for
	// the canonical workitems.comments row seeded under each org.
	// The comments table has no org_id column; its cross-tenant gate
	// is org.Authorize on the parent item's org. Seeded so the FK
	// chain (comments.item_id → items.id → items.org_id) is
	// exercised end-to-end.
	Comments map[string]string

	// CascadeEvents maps the OrgA / OrgB label to the persisted ULID
	// for the canonical deps.cascade_events row seeded under each
	// org (E-3 / unblock-tv8.25). Carries org_id NOT NULL; serves as
	// KindOrgScoped row-leak bait for the rbac.For path and as a
	// concrete row so the FK chain (cascade_events.org_id →
	// org.organizations.id, cascade_events.triggered_by_item_id →
	// workitems.items.id) is exercised end-to-end.
	CascadeEvents map[string]string

	// ToolCalls maps the OrgA / OrgB label to the persisted ULID for
	// the canonical mcp.tool_calls row seeded under each org (E-3).
	// Production code writes via mcp/recordtoolcall.go; the suite
	// seeds directly so the KindOrgScoped row-leak path has a row
	// to surface.
	ToolCalls map[string]string

	// MemoryEntries maps the OrgA / OrgB label to the persisted ULID
	// for the canonical memory.entries row seeded under each org
	// (E-3). scope='org' so org_id is non-NULL and rbac.For's
	// `org_id = $1` predicate hits the row. The schema is
	// service-less in P01 (no apps/api/memory/*.go); the row exists
	// purely as KindOrgScoped row-leak bait.
	MemoryEntries map[string]string

	// Boards maps the OrgA / OrgB label to the persisted ULID for
	// the canonical boards.boards row seeded under each org (E-3).
	// Service-less in P01 (no apps/api/boards/*.go); the row exists
	// purely as KindOrgScoped row-leak bait.
	Boards map[string]string
}

// userKey is the composite key for Fixture.Users. Declared as a
// struct (not a string) so a typo on either dimension is caught at
// compile time.
type userKey struct {
	OrgLabel string // OrgA | OrgB
	Role     string // RoleOwner | RoleAdmin | RoleMember | RoleViewer
}

// SeedFixture installs the two-org cross-membership graph described
// in the file header. Returns the materialised Fixture on success.
// Any DB error is fatal to the test process — the matrix sweep is
// meaningless without a healthy seed.
//
// The seed is idempotent only in the sense that it ALWAYS mints new
// ULIDs; running it twice in the same process produces two disjoint
// fixtures. TestMain calls it exactly once. The matching Teardown
// function below removes the rows the seed installed.
func SeedFixture(ctx context.Context, db *sqldb.Database) (*Fixture, error) {
	if db == nil {
		return nil, fmt.Errorf("rbactest: SeedFixture called with nil *sqldb.Database — the dedicated apps/api/db/ service must have bound the handle before TestMain ran; check that encore test ./... is being used (NOT plain go test)")
	}

	fx := &Fixture{
		Orgs:          make(map[string]string, 2),
		Users:         make(map[userKey]string, 8),
		Projects:      make(map[string]string, 2),
		APIKeyRows:    make(map[string]string, 2),
		Items:         make(map[string]string, 2),
		Comments:      make(map[string]string, 2),
		CascadeEvents: make(map[string]string, 2),
		ToolCalls:     make(map[string]string, 2),
		MemoryEntries: make(map[string]string, 2),
		Boards:        make(map[string]string, 2),
	}

	// Per-side seeding. Both sides get the identical row complement;
	// the side label is folded into slugs / emails / labels so the
	// rows are distinguishable when something leaks.
	for _, orgLabel := range AllOrgs {
		if err := seedSide(ctx, db, fx, orgLabel); err != nil {
			return nil, fmt.Errorf("seed side %q: %w", orgLabel, err)
		}
	}

	return fx, nil
}

// seedSide installs the rows for a single side (OrgA or OrgB).
// Order matters because of the FK chain: users before tokens/sessions,
// orgs before members/projects, projects before project_members and
// api_keys.
func seedSide(ctx context.Context, db *sqldb.Database, fx *Fixture, orgLabel string) error {
	// 1. The org row itself.
	orgID, err := ulid.New()
	if err != nil {
		return fmt.Errorf("org ulid: %w", err)
	}
	orgSlug := strings.ToLower(fmt.Sprintf("rbactest-%s-%s", orgLabel, shortULID(orgID)))
	if _, err := db.Exec(ctx,
		`INSERT INTO org.organizations (id, slug, name) VALUES ($1, $2, $3)`,
		orgID, orgSlug, fmt.Sprintf("rbactest org %s", orgLabel),
	); err != nil {
		return fmt.Errorf("insert org.organizations: %w", err)
	}
	fx.Orgs[orgLabel] = orgID

	// 2. Per-role user rows. Each role gets its own auth.users row
	//    so the role-action matrix sweep can resolve a stable user
	//    per (orgLabel, role) tuple.
	for _, role := range []string{RoleOwner, RoleAdmin, RoleMember, RoleViewer} {
		userID, err := ulid.New()
		if err != nil {
			return fmt.Errorf("user ulid (%s/%s): %w", orgLabel, role, err)
		}
		// primary_provider_id carries the side+role label so the
		// auth.users row is identifiable without an org_id column.
		// The (primary_provider, primary_provider_id) UNIQUE
		// constraint expects a stable identifier; the ULID suffix
		// keeps it unique across repeated runs.
		providerID := fmt.Sprintf("rbactest-%s-%s-%s", orgLabel, role, shortULID(userID))
		email := fmt.Sprintf("%s.%s+%s@rbactest.local", strings.ToLower(orgLabel), role, shortULID(userID))
		display := fmt.Sprintf("rbactest %s %s", orgLabel, role)

		if _, err := db.Exec(ctx,
			`INSERT INTO auth.users
			   (id, primary_provider, primary_provider_id, email, display_name)
			 VALUES ($1, 'github', $2, $3, $4)`,
			userID, providerID, email, display,
		); err != nil {
			return fmt.Errorf("insert auth.users (%s/%s): %w", orgLabel, role, err)
		}
		fx.Users[userKey{OrgLabel: orgLabel, Role: role}] = userID

		// 2a. auth.oauth_tokens row for this user. The *_enc columns
		//     are bytea — direct INSERT of a literal byte string is
		//     fine; the suite never reads these back.
		tokID, err := ulid.New()
		if err != nil {
			return fmt.Errorf("oauth ulid (%s/%s): %w", orgLabel, role, err)
		}
		if _, err := db.Exec(ctx,
			`INSERT INTO auth.oauth_tokens
			   (id, user_id, provider, access_token_enc, refresh_token_enc, scopes)
			 VALUES ($1, $2, 'github', $3, $4, $5)`,
			tokID, userID,
			[]byte("rbactest-access"), []byte("rbactest-refresh"),
			[]string{"repo"},
		); err != nil {
			return fmt.Errorf("insert auth.oauth_tokens (%s/%s): %w", orgLabel, role, err)
		}

		// 2b. auth.sessions row for this user. expires_at must be
		//     strictly greater than issued_at per
		//     sessions_expiry_chk.
		sessID, err := ulid.New()
		if err != nil {
			return fmt.Errorf("session ulid (%s/%s): %w", orgLabel, role, err)
		}
		issuedAt := time.Now().UTC()
		expiresAt := issuedAt.Add(24 * time.Hour)
		if _, err := db.Exec(ctx,
			`INSERT INTO auth.sessions
			   (id, user_id, issued_at, last_seen_at, expires_at, user_agent)
			 VALUES ($1, $2, $3, $3, $4, $5)`,
			sessID, userID, issuedAt, expiresAt, "rbactest",
		); err != nil {
			return fmt.Errorf("insert auth.sessions (%s/%s): %w", orgLabel, role, err)
		}

		// 2c. org.members row binding the user to its home org with
		//     the matching role. Authorize's effective-role
		//     derivation (max(org_role, project_role)) reads this.
		memberID, err := ulid.New()
		if err != nil {
			return fmt.Errorf("member ulid (%s/%s): %w", orgLabel, role, err)
		}
		if _, err := db.Exec(ctx,
			`INSERT INTO org.members (id, org_id, user_id, role)
			 VALUES ($1, $2, $3, $4)`,
			memberID, orgID, userID, role,
		); err != nil {
			return fmt.Errorf("insert org.members (%s/%s): %w", orgLabel, role, err)
		}
	}

	// 3. The canonical org.projects row under this org. Used both
	//    as KindOrgScoped row-leak bait and as the parent for the
	//    project_members row below.
	projectID, err := ulid.New()
	if err != nil {
		return fmt.Errorf("project ulid: %w", err)
	}
	projectSlug := strings.ToLower(fmt.Sprintf("default-%s", shortULID(projectID)))
	if _, err := db.Exec(ctx,
		`INSERT INTO org.projects (id, org_id, slug, name)
		 VALUES ($1, $2, $3, $4)`,
		projectID, orgID, projectSlug, fmt.Sprintf("rbactest project %s", orgLabel),
	); err != nil {
		return fmt.Errorf("insert org.projects: %w", err)
	}
	fx.Projects[orgLabel] = projectID

	// 4. One org.project_members row binding the owner user to the
	//    project. project_members has no org_id column — the
	//    Authorize gate for this resource is the cross-tenant
	//    containment check (apps/api/org/org.go step 3), not a
	//    rbac.For scope predicate.
	ownerID := fx.Users[userKey{OrgLabel: orgLabel, Role: RoleOwner}]
	pmID, err := ulid.New()
	if err != nil {
		return fmt.Errorf("project_member ulid: %w", err)
	}
	if _, err := db.Exec(ctx,
		`INSERT INTO org.project_members (id, project_id, user_id, role)
		 VALUES ($1, $2, $3, $4)`,
		pmID, projectID, ownerID, RoleOwner,
	); err != nil {
		return fmt.Errorf("insert org.project_members: %w", err)
	}

	// 4a. workitems.items row (C-6 / unblock-tv8.15). Seeds the
	//     KindOrgScoped row-leak bait for workitems.items reads
	//     through rbac.For AND the parent row for the
	//     workitems.comments seed below. Constraints:
	//       - type='task' keeps items_finding_required_fields_chk
	//         vacuous (severity/kind_of_finding/discovered_from_id/
	//         parent_id all NULL is legal for non-findings).
	//       - status='Backlog' + claimed_by_id NULL + claimed_at NULL
	//         satisfies items_claim_status_chk's first leg.
	//       - is_ready=false is the schema DEFAULT; written explicit
	//         here so a future ALTER never silently flips the seed.
	itemID, err := ulid.New()
	if err != nil {
		return fmt.Errorf("item ulid: %w", err)
	}
	if _, err := db.Exec(ctx,
		`INSERT INTO workitems.items
		   (id, org_id, project_id, type, title, status, is_ready)
		 VALUES ($1, $2, $3, 'task', $4, 'Backlog', false)`,
		itemID, orgID, projectID, fmt.Sprintf("rbactest item %s", orgLabel),
	); err != nil {
		return fmt.Errorf("insert workitems.items: %w", err)
	}
	fx.Items[orgLabel] = itemID

	// 4b. workitems.comments row (C-6 / unblock-tv8.15). The
	//     comments table has no org_id column — its cross-tenant
	//     gate is org.Authorize on the parent item's org. Seeded
	//     primarily so the FK chain
	//     (comments.item_id → items.id → items.org_id) is exercised
	//     end-to-end and future C-6 KindOrgScoped extensions (if
	//     workitems.comments ever grows an org_id column) inherit a
	//     non-empty fixture. Constraints:
	//       - kind='general' is in the comments_kind_chk allow-list.
	//       - author_id NOT NULL satisfies comments_author_chk.
	commentID, err := ulid.New()
	if err != nil {
		return fmt.Errorf("comment ulid: %w", err)
	}
	if _, err := db.Exec(ctx,
		`INSERT INTO workitems.comments
		   (id, item_id, author_id, kind, body)
		 VALUES ($1, $2, $3, 'general', $4)`,
		commentID, itemID, ownerID,
		fmt.Sprintf("rbactest comment %s", orgLabel),
	); err != nil {
		return fmt.Errorf("insert workitems.comments: %w", err)
	}
	fx.Comments[orgLabel] = commentID

	// 5. mcp.api_keys row foreshadowing the C-6 / E-3 extension. Not
	//    asserted on in B-3; seed only so the FK chain is exercised
	//    and the follow-up beads inherit a non-empty table.
	apiKeyID, err := ulid.New()
	if err != nil {
		return fmt.Errorf("api_key ulid: %w", err)
	}
	// Last 8 chars of the ULID give a high-entropy unique prefix; the
	// random tail dominates so two seeded keys never collide on the
	// UNIQUE index even if both ULIDs were minted in the same
	// millisecond (the timestamp-based prefix would).
	keyPrefix := apiKeyID[len(apiKeyID)-8:]
	// issued_to_user is NOT NULL (bead unblock-tv8.73): every key is
	// owned by exactly one user. Bind to the org owner already seeded
	// above (also the comment author_id) so the FK chain is satisfied.
	if _, err := db.Exec(ctx,
		`INSERT INTO mcp.api_keys
		   (id, org_id, issued_to_user, label, agent_kind, key_hash, key_prefix, scopes)
		 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)`,
		apiKeyID, orgID, ownerID,
		fmt.Sprintf("rbactest-%s-key", orgLabel),
		"claude-code",
		[]byte("rbactest-key-hash-placeholder"),
		keyPrefix,
		[]string{},
	); err != nil {
		return fmt.Errorf("insert mcp.api_keys: %w", err)
	}
	fx.APIKeyRows[orgLabel] = apiKeyID

	// 6. deps.cascade_events row (E-3 / unblock-tv8.25). KindOrgScoped
	//    row-leak bait for the rbac.For path. Constraints:
	//      - event_id is a fresh ULID so the
	//        cascade_events_event_trigger_uniq UNIQUE (event_id,
	//        triggered_by_item_id) holds across repeated seeds.
	//      - kind='close' is in the cascade_events_kind_chk allow-list
	//        (round-6 widened the set to 4 values; 'close' is the
	//        historical baseline).
	//      - triggered_by_item_id references the workitems.items row
	//        seeded above (FK ON DELETE SET NULL — non-NULL satisfies
	//        the suite's row-leak intent).
	//      - cascaded_count=0 satisfies cascade_events_count_chk
	//        (>=0).
	cascadeID, err := ulid.New()
	if err != nil {
		return fmt.Errorf("cascade_event ulid: %w", err)
	}
	cascadeEventID, err := ulid.New()
	if err != nil {
		return fmt.Errorf("cascade_event event_id ulid: %w", err)
	}
	if _, err := db.Exec(ctx,
		`INSERT INTO deps.cascade_events
		   (id, event_id, kind, org_id, project_id, triggered_by_item_id,
		    affected_item_ids, cascaded_count)
		 VALUES ($1, $2, 'close', $3, $4, $5, '{}', 0)`,
		cascadeID, cascadeEventID, orgID, projectID, itemID,
	); err != nil {
		return fmt.Errorf("insert deps.cascade_events: %w", err)
	}
	fx.CascadeEvents[orgLabel] = cascadeID

	// 7. mcp.tool_calls row (E-3 / unblock-tv8.25). KindOrgScoped
	//    row-leak bait. Constraints:
	//      - api_key_id references the mcp.api_keys row seeded above
	//        (FK ON DELETE SET NULL — non-NULL keeps the audit row
	//        well-formed).
	//      - tool_name='prime' is the canonical AF2 read path; any
	//        string is legal at the schema level.
	//      - result_kind='ok' is in the tool_calls_result_chk
	//        allow-list ('ok' | 'rejected' | 'error').
	//      - duration_ms=0 is legal (no NOT-NULL constraint on
	//        non-negativity; the column is NOT NULL only on
	//        presence).
	toolCallID, err := ulid.New()
	if err != nil {
		return fmt.Errorf("tool_call ulid: %w", err)
	}
	if _, err := db.Exec(ctx,
		`INSERT INTO mcp.tool_calls
		   (id, api_key_id, org_id, project_id, tool_name,
		    arguments, result_kind, duration_ms)
		 VALUES ($1, $2, $3, $4, $5, '{}'::jsonb, 'ok', 0)`,
		toolCallID, apiKeyID, orgID, projectID,
		fmt.Sprintf("rbactest-tool-%s", orgLabel),
	); err != nil {
		return fmt.Errorf("insert mcp.tool_calls: %w", err)
	}
	fx.ToolCalls[orgLabel] = toolCallID

	// 8. memory.entries row (E-3 / unblock-tv8.25). KindOrgScoped
	//    row-leak bait. Constraints:
	//      - scope='org' satisfies entries_scope_chk AND the
	//        entries_scope_target_chk first leg (org_id NOT NULL,
	//        project_id NULL, user_id NULL).
	//      - key is side-tagged to satisfy the entries_org_key_uniq
	//        partial UNIQUE index (org_id, key) WHERE scope='org'
	//        across repeated seed runs in the same dev cluster.
	//      - author_id references the seeded owner user (satisfies
	//        entries_author_chk's NOT NULL alternation).
	//      - value_enc is a placeholder bytea; value_size=8 matches
	//        the byte length and stays within the 1..8192 bound of
	//        entries_size_chk.
	//      - ts_doc is computed via to_tsvector('english', $N) in
	//        the INSERT so pgx never needs a tsvector codec on the
	//        bind side. The SELECT-side codec gap is avoided entirely:
	//        the memory.entries read passes .Columns(memoryEntriesColumnList)
	//        which EXCLUDES ts_doc, so it is never projected/scanned
	//        (unblock-8xb.8 round-18; see rbactest_test.go).
	memEntryID, err := ulid.New()
	if err != nil {
		return fmt.Errorf("memory_entry ulid: %w", err)
	}
	memEntryKey := fmt.Sprintf("rbactest-%s-%s", orgLabel, shortULID(memEntryID))
	memValue := []byte("rbactest")
	if _, err := db.Exec(ctx,
		`INSERT INTO memory.entries
		   (id, scope, org_id, author_id, key, value_enc, value_size,
		    ts_doc)
		 VALUES ($1, 'org', $2, $3, $4, $5, $6,
		         to_tsvector('english', $7))`,
		memEntryID, orgID, ownerID, memEntryKey,
		memValue, len(memValue), "rbactest",
	); err != nil {
		return fmt.Errorf("insert memory.entries: %w", err)
	}
	fx.MemoryEntries[orgLabel] = memEntryID

	// 9. boards.boards row (E-3 / unblock-tv8.25). KindOrgScoped
	//    row-leak bait. Constraints:
	//      - org_id NOT NULL; user_id NOT NULL FK to auth.users.
	//        Owner user is reused (already seeded above).
	//      - layout='kanban' is in the boards_layout_chk allow-list
	//        ({'kanban', 'list', 'graph', 'roadmap'}).
	//      - is_default=false sidesteps the
	//        boards_default_per_user_project_uniq partial UNIQUE
	//        index (user_id, COALESCE(project_id, '')) WHERE
	//        is_default=true — the seed only installs one board per
	//        side, so the partial UNIQUE is satisfied trivially with
	//        is_default=false.
	boardID, err := ulid.New()
	if err != nil {
		return fmt.Errorf("board ulid: %w", err)
	}
	if _, err := db.Exec(ctx,
		`INSERT INTO boards.boards
		   (id, org_id, project_id, user_id, name, layout, is_default)
		 VALUES ($1, $2, $3, $4, $5, 'kanban', false)`,
		boardID, orgID, projectID, ownerID,
		fmt.Sprintf("rbactest board %s", orgLabel),
	); err != nil {
		return fmt.Errorf("insert boards.boards: %w", err)
	}
	fx.Boards[orgLabel] = boardID

	return nil
}

// Teardown removes every row this Fixture installed. Called from
// TestMain on the way out. The org.organizations rows cascade-delete
// everything reachable (members, projects, project_members,
// api_keys, tool_calls, workitems.items via items.org_id +
// items.project_id, workitems.comments via comments.item_id,
// deps.cascade_events via cascade_events.org_id ON DELETE CASCADE,
// memory.entries via entries.org_id ON DELETE CASCADE [scope='org'
// rows only — project/user-scoped entries fall through other FK
// branches], boards.boards via boards.org_id ON DELETE CASCADE)
// per the schema's ON DELETE CASCADE chain. auth.users /
// auth.oauth_tokens / auth.sessions are NOT reachable via the
// org_id cascade — they are deleted separately, keyed by the ids
// the fixture installed.
//
// Teardown is best-effort: failures are reported but do not abort.
// The unique-by-ULID safety net in SeedFixture ensures a partial
// teardown does not poison subsequent test runs.
func (f *Fixture) Teardown(ctx context.Context, db *sqldb.Database) {
	if db == nil || f == nil {
		return
	}

	// 1. Org cascade — kills org.members, org.projects,
	//    org.project_members, mcp.api_keys (and downstream tool_calls
	//    on FK SET NULL).
	for _, orgID := range f.Orgs {
		if _, err := db.Exec(ctx, `DELETE FROM org.organizations WHERE id = $1`, orgID); err != nil {
			// Log and continue — teardown is best-effort.
			rlog.Error("rbactest teardown: delete org failed", "org_id", orgID, "err", err)
		}
	}

	// 2. auth.users rows installed by the seed. ON DELETE CASCADE on
	//    auth.oauth_tokens.user_id and auth.sessions.user_id
	//    handles the dependent rows.
	for _, userID := range f.Users {
		if _, err := db.Exec(ctx, `DELETE FROM auth.users WHERE id = $1`, userID); err != nil {
			rlog.Error("rbactest teardown: delete user failed", "user_id", userID, "err", err)
		}
	}
}

// shortULID returns the first 8 chars of a ULID. Used as a uniqueness
// salt on slugs / emails / provider ids so repeated SeedFixture calls
// in the same dev cluster do not collide on UNIQUE constraints. Not
// security-sensitive — the truncation is cosmetic.
func shortULID(s string) string {
	if len(s) < 8 {
		return s
	}
	return s[:8]
}

// fatalIf is a tiny helper for the seed path inside TestMain. The
// caller passes t == nil (TestMain has no *testing.T), in which case
// the function panics; otherwise it calls t.Fatal. Centralised so
// callers don't sprinkle the nil-check.
func fatalIf(t *testing.T, err error, msg string) {
	if err == nil {
		return
	}
	if t == nil {
		panic(fmt.Sprintf("%s: %v", msg, err))
	}
	t.Fatalf("%s: %v", msg, err)
}
