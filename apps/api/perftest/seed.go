// seed.go provisions the perftest fixture (org + user + project +
// members + api_key + readyItemCount ready items) via direct
// `encore.dev/storage/sqldb` writes. Mirrors
// `apps/api/exitcriteriontest/seed.go` — FK ordering, ULID minting,
// direct sqldb.Exec, no RPC calls.
//
// Why direct SQL (SPEC §11.2 + §11.1.1 round-12).
//
// The auth/org RPC surfaces require an Encore auth context the test
// cannot easily fabricate (the authhandler reads a real
// `Authorization: Bearer …` header). Direct INSERT lets the seed
// install the rows without dancing through the auth mesh; the
// production code paths (the Bearer hot path + the prime/ready/claim
// tools) are still exercised end-to-end by the measurement loop that
// drives the MCP transport.
//
// The mcp.api_keys row is computed in-test: a fresh raw key is minted
// via crypto/rand + base32 mirroring `apps/api/auth/apikey.go`, the
// HMAC digest is computed via the local computeKeyHash helper
// (identical to production `hashRawKey`), and both the digest and the
// prefix are inserted. The raw key is held in memory on the Fixture
// struct as the Bearer token; it is never persisted to disk.
//
// is_ready in the seed (SPEC §11.3 (b) + lint allowlist).
//
// The seed writes `is_ready=true` on every ready row via direct INSERT
// (NOT UPDATE). The `no_direct_is_ready_write` linter
// (apps/api/shared/lint/no_direct_is_ready_write.go) matches `UPDATE
// workitems.items` statements only — a fresh-row INSERT with is_ready
// in the column list does not trip the analyzer (verified against
// exitcriteriontest/seed.go, which does the same).
//
// SPEC anchors: §11.2 NFR-1 (round-14), §11.1.1 round-12, §11.3 (b).

package perftest

import (
	"context"
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base32"
	"fmt"
	"strings"

	"encore.app/shared/ulid"
	"encore.dev/rlog"
	"encore.dev/storage/sqldb"
)

// rawKeyPrefix mirrors auth.rawKeyPrefix verbatim. Re-declared here
// because the auth constant is unexported. Any change to the
// production constant requires a lockstep change here.
const rawKeyPrefix = "unblock_pat_"

// rawKeyRandomBytes mirrors auth.rawKeyRandomBytes (32 bytes, 256-bit
// entropy per the locked format).
const rawKeyRandomBytes = 32

// keyPrefixLen mirrors auth.keyPrefixLen — the first 8 chars of the
// random base32 portion populate `mcp.api_keys.key_prefix`.
const keyPrefixLen = 8

// rawKeyEncoder mirrors auth.rawKeyEncoder — lowercase base32 with
// padding disabled.
var rawKeyEncoder = base32.NewEncoding("abcdefghijklmnopqrstuvwxyz234567").WithPadding(base32.NoPadding)

// generateRawKey produces a fresh raw key in the production format
// (`unblock_pat_` + 52-char lowercase base32 over 32 crypto/rand
// bytes). Mirrors auth.generateRawKey verbatim.
func generateRawKey() (string, error) {
	buf := make([]byte, rawKeyRandomBytes)
	if _, err := rand.Read(buf); err != nil {
		return "", fmt.Errorf("perftest: read crypto/rand: %w", err)
	}
	return rawKeyPrefix + rawKeyEncoder.EncodeToString(buf), nil
}

// prefixOf extracts the `key_prefix` value from a raw key — the first
// 8 chars of the random base32 portion (rawKey[12:20]). The seed has
// full control over rawKey shape (we minted it above) so we skip the
// defensive checks auth.prefixOf performs on untrusted input.
func prefixOf(rawKey string) string {
	return rawKey[len(rawKeyPrefix) : len(rawKeyPrefix)+keyPrefixLen]
}

// computeKeyHash mirrors auth.hashRawKey verbatim — HMAC-SHA256 of
// rawKey under secrets.APIKeyHMACSecret. The returned 32-byte digest
// is stored as bytea in mcp.api_keys.key_hash; the production hot path
// compares it via subtle.ConstantTimeCompare on every Bearer auth
// check.
func computeKeyHash(secret, rawKey string) []byte {
	mac := hmac.New(sha256.New, []byte(secret))
	mac.Write([]byte(rawKey))
	return mac.Sum(nil)
}

// SeedFixture installs the perftest fixture. Returns the materialised
// Fixture on success; any DB error is fatal to the test process.
//
// FK ordering:
//
//  1. org.organizations
//  2. auth.users
//  3. org.projects
//  4. org.members (owner — so any RBAC path resolves a real role row)
//  5. org.project_members (owner)
//  6. mcp.api_keys (key_hash computed against secrets.APIKeyHMACSecret)
//  7. workitems.items × readyItemCount (all status='Ready',
//     is_ready=true — the ready-claim pool)
//
// No deps.dependencies are seeded: the prime → ready → claim hot path
// traverses no edges, and every seeded item is independently ready.
func SeedFixture(ctx context.Context, db *sqldb.Database) (*Fixture, error) {
	if db == nil {
		return nil, fmt.Errorf("perftest: SeedFixture called with nil *sqldb.Database — the dedicated apps/api/db/ service must have bound the handle before TestMain ran; check that encore test is being used (NOT plain go test)")
	}
	if secrets.APIKeyHMACSecret == "" {
		return nil, fmt.Errorf("perftest: APIKeyHMACSecret is empty — apps/api/.secrets.local.cue must declare APIKeyHMACSecret, or run under `encore test` (not plain go test) so Encore populates the secret")
	}

	fx := &Fixture{
		ReadyItemIDs: make([]string, 0, readyItemCount),
	}

	// 1. org.organizations — slug salted with a shortULID to avoid
	// dev-cluster collision with exitcriteriontest / other suites
	// (SPEC §11.2 seeding doctrine; R6).
	orgID, err := ulid.New()
	if err != nil {
		return nil, fmt.Errorf("org ulid: %w", err)
	}
	orgSlug := strings.ToLower(fmt.Sprintf("perftest-%s", shortULID(orgID)))
	if _, err := db.Exec(ctx,
		`INSERT INTO org.organizations (id, slug, name) VALUES ($1, $2, $3)`,
		orgID, orgSlug, "P01 NFR-1 Latency Harness",
	); err != nil {
		return nil, fmt.Errorf("insert org.organizations: %w", err)
	}
	fx.OrgID = orgID

	// 2. auth.users — provider_id salted to dodge the
	// users_primary_provider_provider_id_uniq constraint across runs.
	userID, err := ulid.New()
	if err != nil {
		return nil, fmt.Errorf("user ulid: %w", err)
	}
	providerID := fmt.Sprintf("perf-%s", shortULID(userID))
	if _, err := db.Exec(ctx,
		`INSERT INTO auth.users
		   (id, primary_provider, primary_provider_id, email, display_name)
		 VALUES ($1, 'github', $2, $3, $4)`,
		userID, providerID, fmt.Sprintf("perf-%s@example.com", shortULID(userID)), "Perf Harness",
	); err != nil {
		return nil, fmt.Errorf("insert auth.users: %w", err)
	}
	fx.UserID = userID

	// 3. org.projects.
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

	// 4. org.members — owner. Same rationale as exitcriteriontest:
	// every production tool body calls org.Authorize which reads
	// org.members; owner resolves to maximum effective role.
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

	// 5. org.project_members — owner.
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

	// 6. mcp.api_keys — raw key minted in-test, key_hash computed with
	// secrets.APIKeyHMACSecret (SPEC §11.2 seeding doctrine).
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
		"perftest-harness",
		"claude-code",
		keyHash,
		keyPrefix,
		[]string{},
	); err != nil {
		return nil, fmt.Errorf("insert mcp.api_keys: %w", err)
	}
	fx.APIKeyID = apiKeyID
	fx.RawKey = rawKey

	// 7. workitems.items × readyItemCount. Each row is status='Ready',
	// is_ready=true, closed_at NULL — the exact predicate the
	// items_ready_partial_idx covers, so the `ready` tool's hot path
	// uses the index (the p99 budget depends on it). Uniform priority
	// keeps the (priority asc, created_at asc, id asc) ordering
	// deterministic.
	//
	// Linter note (no_direct_is_ready_write): the analyzer matches
	// UPDATE statements only; INSERTs that list is_ready in the column
	// list do NOT trip the regex.
	for i := 0; i < readyItemCount; i++ {
		itemID, err := ulid.New()
		if err != nil {
			return nil, fmt.Errorf("item ulid (#%d): %w", i, err)
		}
		if _, err := db.Exec(ctx,
			`INSERT INTO workitems.items
			   (id, org_id, project_id, type, title, status, priority, is_ready)
			 VALUES ($1, $2, $3, $4, $5, $6, $7, true)`,
			itemID, orgID, projectID,
			seedItemType,
			fmt.Sprintf("perf ready item %04d", i),
			seedItemStatus,
			seedItemPriority,
		); err != nil {
			return nil, fmt.Errorf("insert workitems.items (#%d): %w", i, err)
		}
		fx.ReadyItemIDs = append(fx.ReadyItemIDs, itemID)
	}

	return fx, nil
}

// Teardown removes every row this Fixture installed. Called from
// TestMain on the way out. The org.organizations row cascade-deletes
// everything reachable (org.members, org.projects, org.project_members,
// mcp.api_keys, workitems.items via items.org_id). auth.users is NOT
// reachable via the org_id cascade — deleted separately.
//
// Best-effort: failures are surfaced via rlog.Error but never abort
// the test process. The unique-by-ULID safety net in SeedFixture
// ensures a partial teardown does not poison subsequent runs.
func (f *Fixture) Teardown(ctx context.Context, db *sqldb.Database) {
	if db == nil || f == nil {
		return
	}
	if f.OrgID != "" {
		if _, err := db.Exec(ctx, `DELETE FROM org.organizations WHERE id = $1`, f.OrgID); err != nil {
			rlog.Error("perftest: teardown delete org failed", "err", err, "org_id", f.OrgID)
		}
	}
	if f.UserID != "" {
		if _, err := db.Exec(ctx, `DELETE FROM auth.users WHERE id = $1`, f.UserID); err != nil {
			rlog.Error("perftest: teardown delete user failed", "err", err, "user_id", f.UserID)
		}
	}
}

// shortULID returns the first 8 chars of a ULID. Used as a uniqueness
// salt on slugs / provider ids so repeated SeedFixture calls in the
// same dev cluster do not collide on UNIQUE constraints. Mirrors
// exitcriteriontest/seed.go::shortULID verbatim.
func shortULID(s string) string {
	if len(s) < 8 {
		return s
	}
	return s[:8]
}
