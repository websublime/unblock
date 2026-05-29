// negauth.go provides the seed helpers the W3 negative-auth matrix
// (auth_negative_test.go) needs. Each helper mints a raw key in
// production format and installs a corresponding mcp.api_keys row in a
// specific bad state, so the Bearer hot path
// (apps/api/auth/auth.go::validateAPIKey) rejects it at the documented
// §4.3.2 step:
//
//   - SeedRevokedKey   — row with revoked_at set (auth.go step 4,
//     line 199).
//   - SeedExpiredKey   — row with expires_at < now() (auth.go step 4,
//     line 202).
//   - SeedBadHMACKey   — row whose key_hash is a deliberately wrong
//     digest, so the constant-time compare fails (auth.go step 6,
//     line 208). The key_prefix DOES resolve, so the lookup succeeds
//     and the rejection happens at the HMAC compare — distinct from
//     the unknown-prefix path.
//
// The "unknown prefix" and "missing prefix" negative paths need no
// seed row: the harness mints a raw key whose prefix is not in the DB
// (unknown), or a token without the `unblock_pat_` brand prefix
// (missing). Those helpers (UnknownPrefixRawKey, MissingPrefixRawKey)
// are pure string mints — no DB write.
//
// All helpers reuse the rawKey minting + HMAC primitives from seed.go.
// The negative rows are scoped to the same OrgID / UserID the main
// fixture installed, so the main Fixture.Teardown (cascade on OrgID)
// reaps them — no separate teardown needed.
//
// SPEC anchors: §4.3.2 (Bearer hot path), §11.2 W3 closure.

package perftest

import (
	"context"
	"crypto/rand"
	"fmt"
	"time"

	"encore.app/shared/ulid"
	"encore.dev/storage/sqldb"
)

// SeedRevokedKey installs a fully-valid mcp.api_keys row scoped to the
// fixture's org/user, then sets revoked_at = now(). Returns the raw
// key. The Bearer hot path resolves the prefix and HMAC successfully
// but rejects at the revocation check (auth.go:199).
//
// We set revoked_at via a second UPDATE rather than inline so the row
// first passes the same INSERT shape the main seed uses (proving the
// rejection is the revocation check, not a malformed row).
func (f *Fixture) SeedRevokedKey(ctx context.Context, db *sqldb.Database) (string, error) {
	rawKey, id, err := f.insertScopedKey(ctx, db, "perftest-revoked", nil, nil)
	if err != nil {
		return "", err
	}
	if _, err := db.Exec(ctx,
		`UPDATE mcp.api_keys SET revoked_at = now() WHERE id = $1`, id,
	); err != nil {
		return "", fmt.Errorf("perftest: revoke key %s: %w", id, err)
	}
	return rawKey, nil
}

// SeedExpiredKey installs a valid-shape mcp.api_keys row whose
// expires_at is one hour in the past. The Bearer hot path resolves the
// prefix and HMAC but rejects at the expiry check (auth.go:202).
func (f *Fixture) SeedExpiredKey(ctx context.Context, db *sqldb.Database) (string, error) {
	past := time.Now().Add(-time.Hour)
	rawKey, _, err := f.insertScopedKey(ctx, db, "perftest-expired", &past, nil)
	if err != nil {
		return "", err
	}
	return rawKey, nil
}

// SeedBadHMACKey installs a row whose key_prefix matches a freshly
// minted raw key (so the §4.3.2 prefix lookup SUCCEEDS) but whose
// key_hash is a deliberately wrong 32-byte digest (HMAC of a DIFFERENT
// raw key). The Bearer hot path resolves the prefix, then the
// constant-time compare fails (auth.go:208). Returns the raw key whose
// prefix is stored — the body the caller presents hashes to a digest
// that will not match the stored (wrong) hash.
func (f *Fixture) SeedBadHMACKey(ctx context.Context, db *sqldb.Database) (string, error) {
	presentedKey, err := generateRawKey()
	if err != nil {
		return "", fmt.Errorf("perftest: bad-hmac presented key: %w", err)
	}
	// A different raw key whose HMAC becomes the stored (wrong) digest.
	otherKey, err := generateRawKey()
	if err != nil {
		return "", fmt.Errorf("perftest: bad-hmac other key: %w", err)
	}
	wrongHash := computeKeyHash(secrets.APIKeyHMACSecret, otherKey)

	apiKeyID, err := ulid.New()
	if err != nil {
		return "", fmt.Errorf("perftest: bad-hmac ulid: %w", err)
	}
	if _, err := db.Exec(ctx,
		`INSERT INTO mcp.api_keys
		   (id, org_id, issued_to_user, label, agent_kind, key_hash, key_prefix, scopes)
		 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)`,
		apiKeyID, f.OrgID, f.UserID,
		"perftest-bad-hmac",
		"claude-code",
		wrongHash,
		prefixOf(presentedKey),
		[]string{},
	); err != nil {
		return "", fmt.Errorf("perftest: insert bad-hmac key: %w", err)
	}
	return presentedKey, nil
}

// UnknownPrefixRawKey mints a fresh raw key in production format whose
// prefix is (with overwhelming probability) not present in the DB. The
// Bearer hot path's prefix lookup returns ErrNoRows and rejects
// (auth.go:189). No DB write.
func UnknownPrefixRawKey() (string, error) {
	return generateRawKey()
}

// MissingPrefixRawKey returns a raw token WITHOUT the `unblock_pat_`
// brand prefix. The Bearer hot path's prefixOf parse fails at step 2
// (auth.go:159-165) before any DB lookup. No DB write.
func MissingPrefixRawKey() (string, error) {
	buf := make([]byte, rawKeyRandomBytes)
	if _, err := rand.Read(buf); err != nil {
		return "", fmt.Errorf("perftest: missing-prefix key: %w", err)
	}
	// Deliberately omit the rawKeyPrefix so prefixOf rejects it.
	return "nopat_" + rawKeyEncoder.EncodeToString(buf), nil
}

// insertScopedKey mints a fresh raw key and inserts a mcp.api_keys row
// scoped to the fixture's org/user with the given label, optional
// expires_at, and optional revoked_at. Returns the raw key and the row
// id. The key_hash is the correct HMAC of the minted raw key, so the
// row passes prefix lookup + HMAC compare — any rejection comes from
// the revoked/expired columns the caller sets.
func (f *Fixture) insertScopedKey(
	ctx context.Context,
	db *sqldb.Database,
	label string,
	expiresAt *time.Time,
	revokedAt *time.Time,
) (rawKey, id string, err error) {
	rawKey, err = generateRawKey()
	if err != nil {
		return "", "", fmt.Errorf("perftest: scoped key (%s): %w", label, err)
	}
	apiKeyID, err := ulid.New()
	if err != nil {
		return "", "", fmt.Errorf("perftest: scoped key ulid (%s): %w", label, err)
	}
	keyHash := computeKeyHash(secrets.APIKeyHMACSecret, rawKey)
	if _, err := db.Exec(ctx,
		`INSERT INTO mcp.api_keys
		   (id, org_id, issued_to_user, label, agent_kind, key_hash, key_prefix, scopes, expires_at, revoked_at)
		 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)`,
		apiKeyID, f.OrgID, f.UserID,
		label,
		"claude-code",
		keyHash,
		prefixOf(rawKey),
		[]string{},
		expiresAt,
		revokedAt,
	); err != nil {
		return "", "", fmt.Errorf("perftest: insert scoped key (%s): %w", label, err)
	}
	return rawKey, apiKeyID, nil
}
