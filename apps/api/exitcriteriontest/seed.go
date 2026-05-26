// seed.go provisions the §11.1.0 fixture (the canonical 5-item
// dependency graph + scaffolding org/project/user/api-key rows) via
// direct `encore.dev/storage/sqldb` writes. Mirrors the
// `apps/api/shared/rbactest/seed.go` pattern verbatim (FK ordering,
// ULID minting, direct sqldb.Exec, no RPC calls).
//
// Why direct SQL (SPEC §11.1.1, round-12).
//
// The auth/org RPC surfaces require an Encore auth context the test
// cannot easily fabricate (the authhandler reads a real
// `Authorization: Bearer …` header). Direct INSERT lets the seed
// install the rows without dancing through the auth mesh; the
// production code paths are still exercised by the test bodies that
// drive the MCP transport.
//
// The mcp.api_keys row is computed in-test: a fresh raw key is
// minted via crypto/rand + base32 mirroring
// `apps/api/auth/apikey.go::generateRawKey`, the HMAC digest is
// computed via the local hashRawKey helper (identical to the
// production `apps/api/auth/apikey.go::hashRawKey`), and both the
// digest and the prefix are inserted into mcp.api_keys. The raw key
// is held in memory on the Fixture struct for use as the Bearer
// token; it is never persisted to disk.
//
// FK ordering (matches rbactest/seed.go:188-519 with the §11.1.0
// scope):
//
//  1. org.organizations (id, slug, name)
//  2. auth.users (id, primary_provider, primary_provider_id, email, display_name)
//  3. org.projects (id, org_id, slug, name)
//  4. org.members (org_id, user_id, role) — needed so any RBAC path
//     exercised by the MCP tool surface resolves a real role row for
//     usr_alice in org_exit_criterion. SPEC §11.1.0 does not name
//     this row explicitly, but every production tool body calls
//     org.Authorize which reads from org.members.
//  5. org.project_members (project_id, user_id, role) — same
//     rationale; project-scoped tool calls need a project_member row.
//  6. mcp.api_keys (id, org_id, issued_to_user, label, agent_kind,
//     key_hash, key_prefix, scopes)
//  7. workitems.items × 5 (per §11.1.0 per-row state)
//  8. deps.dependencies × 4 (per §11.1.0 edge set; all kind='blocks')
//
// is_ready in the seed (SPEC §11.3 (b) + lint allowlist).
//
// The seed writes `is_ready=true` on itm_b and itm_e via direct
// INSERT (NOT UPDATE). The `no_direct_is_ready_write` linter
// (apps/api/shared/lint/no_direct_is_ready_write.go) matches `UPDATE
// workitems.items` statements only — the regex is anchored on the
// UPDATE keyword (lines 102-160 of the analyzer source) — so a
// fresh-row INSERT with is_ready in the column list does not trip
// the analyzer. Verified by reading the regex: `\bupdate(?:\s|\\.)+`
// requires the literal UPDATE token; INSERT statements skip the
// matcher entirely.
//
// SPEC anchors: §11.1.0, §11.1.1, §11.3 (b).

package exitcriteriontest

import (
	"context"
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base32"
	"fmt"
	"strings"
	"time"

	"encore.app/shared/ulid"
	"encore.dev/rlog"
	"encore.dev/storage/sqldb"
)

// rawKeyPrefix mirrors auth.rawKeyPrefix verbatim. We re-declare it
// here because the auth constant is unexported. Any change to the
// production constant requires a lockstep change here; the linked
// SPEC §4.3.2 line range (453-460) is the canonical contract.
const rawKeyPrefix = "unblock_pat_"

// rawKeyRandomBytes mirrors auth.rawKeyRandomBytes (32 bytes,
// 256-bit entropy per the locked format).
const rawKeyRandomBytes = 32

// keyPrefixLen mirrors auth.keyPrefixLen — the first 8 chars of the
// random base32 portion populate `mcp.api_keys.key_prefix`.
const keyPrefixLen = 8

// rawKeyEncoder mirrors auth.rawKeyEncoder — lowercase base32 with
// padding disabled (SPEC §4.3.2 line 455).
var rawKeyEncoder = base32.NewEncoding("abcdefghijklmnopqrstuvwxyz234567").WithPadding(base32.NoPadding)

// generateRawKey produces a fresh raw key in the production format
// (`unblock_pat_` + 52-char lowercase base32 over 32 crypto/rand
// bytes). Mirrors auth.generateRawKey verbatim; the keying material
// is hashed with secrets.APIKeyHMACSecret in computeKeyHash below
// and the digest matches what auth.validateAPIKey will compute when
// the test posts `Bearer <rawKey>` to the MCP endpoint.
func generateRawKey() (string, error) {
	buf := make([]byte, rawKeyRandomBytes)
	if _, err := rand.Read(buf); err != nil {
		return "", fmt.Errorf("exitcriteriontest: read crypto/rand: %w", err)
	}
	return rawKeyPrefix + rawKeyEncoder.EncodeToString(buf), nil
}

// prefixOf extracts the `key_prefix` value from a raw key — the
// first 8 chars of the random base32 portion (rawKey[12:20]).
// Mirrors auth.prefixOf for the test's INSERT path; the production
// hot path reads back the prefix from the wire and looks up
// mcp.api_keys.key_prefix via the api_keys_prefix_uniq UNIQUE index.
func prefixOf(rawKey string) string {
	// The seed has full control over rawKey shape (we minted it
	// above) so we skip the defensive length / brand-prefix checks
	// that auth.prefixOf performs on untrusted input.
	return rawKey[len(rawKeyPrefix) : len(rawKeyPrefix)+keyPrefixLen]
}

// computeKeyHash mirrors auth.hashRawKey verbatim — HMAC-SHA256 of
// rawKey under secrets.APIKeyHMACSecret. The returned 32-byte digest
// is stored as bytea in mcp.api_keys.key_hash; the production hot
// path compares it via subtle.ConstantTimeCompare on every Bearer
// auth check (SPEC §4.3.2 steps 5-6 + apps/api/auth/auth.go:206-210).
func computeKeyHash(secret, rawKey string) []byte {
	mac := hmac.New(sha256.New, []byte(secret))
	mac.Write([]byte(rawKey))
	return mac.Sum(nil)
}

// SeedFixture installs the §11.1.0 fixture. Returns the materialised
// Fixture on success; any DB error is fatal to the test process.
//
// The seed is idempotent only in the sense that it ALWAYS mints
// fresh ULIDs; running it twice in the same process produces two
// disjoint fixtures. TestMain calls it exactly once.
func SeedFixture(ctx context.Context, db *sqldb.Database) (*Fixture, error) {
	if db == nil {
		return nil, fmt.Errorf("exitcriteriontest: SeedFixture called with nil *sqldb.Database — the dedicated apps/api/db/ service must have bound the handle before TestMain ran; check that encore test is being used (NOT plain go test)")
	}
	if secrets.APIKeyHMACSecret == "" {
		return nil, fmt.Errorf("exitcriteriontest: APIKeyHMACSecret is empty — apps/api/.secrets.local.cue must declare APIKeyHMACSecret, or run under `encore test` (not plain go test) so Encore populates the secret")
	}

	fx := &Fixture{
		Items: make(map[string]string, len(allItemLabels)),
	}

	// 1. org.organizations
	orgID, err := ulid.New()
	if err != nil {
		return nil, fmt.Errorf("org ulid: %w", err)
	}
	orgSlug := strings.ToLower(fmt.Sprintf("exit-criterion-%s", shortULID(orgID)))
	if _, err := db.Exec(ctx,
		`INSERT INTO org.organizations (id, slug, name) VALUES ($1, $2, $3)`,
		orgID, orgSlug, "P01 Exit Criterion",
	); err != nil {
		return nil, fmt.Errorf("insert org.organizations: %w", err)
	}
	fx.OrgID = orgID

	// 2. auth.users — usr_alice. SPEC §11.1.0 names
	// primary_provider_id="1" verbatim; the auth.users
	// users_primary_provider_provider_id_uniq UNIQUE constraint
	// would collide on repeated encore-test runs that share a
	// long-lived dev cluster, so we suffix with a ULID-derived
	// salt (same shape as rbactest/seed.go:212-218 — the "1" in
	// SPEC §11.1.0 is illustrative just like the ids).
	userID, err := ulid.New()
	if err != nil {
		return nil, fmt.Errorf("user ulid: %w", err)
	}
	providerID := fmt.Sprintf("1-%s", shortULID(userID))
	if _, err := db.Exec(ctx,
		`INSERT INTO auth.users
		   (id, primary_provider, primary_provider_id, email, display_name)
		 VALUES ($1, 'github', $2, $3, $4)`,
		userID, providerID, "alice@example.com", "Alice",
	); err != nil {
		return nil, fmt.Errorf("insert auth.users: %w", err)
	}
	fx.UserID = userID

	// 3. org.projects — prj_exit.
	projectID, err := ulid.New()
	if err != nil {
		return nil, fmt.Errorf("project ulid: %w", err)
	}
	if _, err := db.Exec(ctx,
		`INSERT INTO org.projects (id, org_id, slug, name)
		 VALUES ($1, $2, $3, $4)`,
		projectID, orgID, "default", "Default",
	); err != nil {
		return nil, fmt.Errorf("insert org.projects: %w", err)
	}
	fx.ProjectID = projectID

	// 4. org.members — bind usr_alice to org_exit_criterion as
	// 'owner'. The role choice is informational for this fixture
	// (every MCP tool call carries the agent role on the resolved
	// Identity per apps/api/mcp/identity.go::agentRole — the
	// org.Authorize path branches on AgentKind, not on the human
	// role from org.members). 'owner' is the safest default; any
	// hypothetical fallback path that consults the members table
	// resolves to maximum effective role.
	memberID, err := ulid.New()
	if err != nil {
		return nil, fmt.Errorf("member ulid: %w", err)
	}
	if _, err := db.Exec(ctx,
		`INSERT INTO org.members (id, org_id, user_id, role)
		 VALUES ($1, $2, $3, 'owner')`,
		memberID, orgID, userID,
	); err != nil {
		return nil, fmt.Errorf("insert org.members: %w", err)
	}

	// 5. org.project_members — same rationale. Owner of the
	// project so any project-scoped tool call resolves to maximum
	// effective role.
	pmID, err := ulid.New()
	if err != nil {
		return nil, fmt.Errorf("project_member ulid: %w", err)
	}
	if _, err := db.Exec(ctx,
		`INSERT INTO org.project_members (id, project_id, user_id, role)
		 VALUES ($1, $2, $3, 'owner')`,
		pmID, projectID, userID,
	); err != nil {
		return nil, fmt.Errorf("insert org.project_members: %w", err)
	}

	// 6. mcp.api_keys — alice-claude-code. The raw key is minted
	// in-test and held on the Fixture struct; key_hash is computed
	// with secrets.APIKeyHMACSecret per SPEC §11.1.1.
	apiKeyID, err := ulid.New()
	if err != nil {
		return nil, fmt.Errorf("api_key ulid: %w", err)
	}
	rawKey, err := generateRawKey()
	if err != nil {
		return nil, fmt.Errorf("generate raw api key: %w", err)
	}
	keyHash := computeKeyHash(secrets.APIKeyHMACSecret, rawKey)
	keyPrefix := prefixOf(rawKey)
	if _, err := db.Exec(ctx,
		`INSERT INTO mcp.api_keys
		   (id, org_id, issued_to_user, label, agent_kind, key_hash, key_prefix, scopes)
		 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)`,
		apiKeyID, orgID, userID,
		"alice-claude-code",
		"claude-code",
		keyHash,
		keyPrefix,
		[]string{},
	); err != nil {
		return nil, fmt.Errorf("insert mcp.api_keys: %w", err)
	}
	fx.APIKeyID = apiKeyID
	fx.RawKey = rawKey

	// 7. workitems.items × 5. itm_a..itm_e per §11.1.0; per-row
	// state pulled from itemSpec(). The closed_at column is
	// populated with NOW() for itm_a (status=Done end-state) and
	// NULL for every other row.
	//
	// Linter note (no_direct_is_ready_write): the analyzer matches
	// UPDATE statements only — INSERTs that list is_ready in the
	// column list do NOT trip the regex. Verified by reading
	// apps/api/shared/lint/no_direct_is_ready_write.go:138-160.
	now := time.Now().UTC()
	for _, label := range allItemLabels {
		itemID, err := ulid.New()
		if err != nil {
			return nil, fmt.Errorf("item ulid (%s): %w", label, err)
		}
		status, impl, review, qa, isReady, closedNow := itemSpec(label)

		var closedAt *time.Time
		if closedNow {
			ts := now
			closedAt = &ts
		}

		if _, err := db.Exec(ctx,
			`INSERT INTO workitems.items
			   (id, org_id, project_id, type, title, status,
			    impl_state, review_state, qa_state,
			    is_ready, closed_at)
			 VALUES ($1, $2, $3, 'task', $4, $5,
			         $6, $7, $8,
			         $9, $10)`,
			itemID, orgID, projectID,
			titleFor(label),
			status,
			impl, review, qa,
			isReady, closedAt,
		); err != nil {
			return nil, fmt.Errorf("insert workitems.items (%s): %w", label, err)
		}
		fx.Items[label] = itemID
	}

	// 8. deps.dependencies × 4. All kind='blocks' per §11.1.0.
	// created_by is usr_alice — the audit trail names the test's
	// fixture owner as the canonical edge creator (no fallback path
	// reads created_by during the §11.1.2 assertions; the value
	// satisfies the FK only).
	for _, e := range edgeSpecs {
		edgeID, err := ulid.New()
		if err != nil {
			return nil, fmt.Errorf("edge ulid (%s→%s): %w", e.From, e.To, err)
		}
		if _, err := db.Exec(ctx,
			`INSERT INTO deps.dependencies
			   (id, from_item, to_item, kind, created_by)
			 VALUES ($1, $2, $3, 'blocks', $4)`,
			edgeID, fx.ItemID(e.From), fx.ItemID(e.To), userID,
		); err != nil {
			return nil, fmt.Errorf("insert deps.dependencies (%s→%s): %w", e.From, e.To, err)
		}
	}

	return fx, nil
}

// titleFor returns the §11.1.0 verbatim Title column value for the
// given label. Kept as a helper rather than inlined in the INSERT
// loop so the §11.1.0 wording is self-documenting and easy to grep.
func titleFor(label string) string {
	switch label {
	case LabelItemA:
		return "Bootstrap (already done)"
	case LabelItemB:
		return "Implement core (ready)"
	case LabelItemC, LabelItemD:
		return "Depends on B"
	case LabelItemE:
		return "Cycle attempt target"
	default:
		panic("exitcriteriontest: titleFor: unknown label: " + label)
	}
}

// Teardown removes every row this Fixture installed. Called from
// TestMain on the way out. The org.organizations row cascade-deletes
// everything reachable (org.members, org.projects,
// org.project_members, mcp.api_keys, workitems.items via
// items.org_id, deps.dependencies via dependencies.from/to_item ON
// DELETE CASCADE on workitems.items, etc.). auth.users is NOT
// reachable via the org_id cascade — deleted separately.
//
// Teardown is best-effort: failures are surfaced via rlog.Error
// (consistent with the rest of the backend's logging convention) but
// never abort the test process (TestMain logs and continues). The
// unique-by-ULID safety net in SeedFixture ensures a partial
// teardown does not poison subsequent runs.
func (f *Fixture) Teardown(ctx context.Context, db *sqldb.Database) {
	if db == nil || f == nil {
		return
	}

	if f.OrgID != "" {
		if _, err := db.Exec(ctx, `DELETE FROM org.organizations WHERE id = $1`, f.OrgID); err != nil {
			rlog.Error("exitcriteriontest: teardown delete org failed", "err", err, "org_id", f.OrgID)
		}
	}
	if f.UserID != "" {
		if _, err := db.Exec(ctx, `DELETE FROM auth.users WHERE id = $1`, f.UserID); err != nil {
			rlog.Error("exitcriteriontest: teardown delete user failed", "err", err, "user_id", f.UserID)
		}
	}
}

// shortULID returns the first 8 chars of a ULID. Used as a
// uniqueness salt on slugs / provider ids so repeated SeedFixture
// calls in the same dev cluster do not collide on UNIQUE
// constraints. Mirrors rbactest/seed.go::shortULID verbatim.
func shortULID(s string) string {
	if len(s) < 8 {
		return s
	}
	return s[:8]
}
