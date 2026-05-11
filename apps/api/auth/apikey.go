// API key format helpers for the MCP Bearer auth hot path.
//
// Locked format (SPEC §4.3.2 lines 453-460): `unblock_pat_<base32-32-byte>`.
//
//	rawKey   = "unblock_pat_" || base32(crypto/rand 32 bytes, lowercase, no padding)
//	totalLen = 12 + 52 = 64
//
// `key_prefix` (the UNIQUE-indexed lookup hint stored in `mcp.api_keys`) is
// the first 8 chars of the random base32 portion — i.e. `rawKey[12:20]`.
// The literal `unblock_pat_` prefix is stripped before slicing so the
// prefix space never collides on the brand prefix (DRIFT-A in the bead
// investigation: §4.3.2 step 2's loose `raw_key[:8]` is superseded by the
// locked key-format note at lines 453-460).
//
// Hash: HMAC-SHA256(secrets.APIKeyHMACSecret, rawKey) — 32 bytes raw,
// stored bytea in `mcp.api_keys.key_hash`. Constant-time compare via
// crypto/subtle.ConstantTimeCompare on every Bearer auth check
// (SPEC §4.3.2 step 6).

package auth

import (
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base32"
	"errors"
	"fmt"
)

// rawKeyPrefix is the literal brand prefix prepended to every raw key.
// Length 12 (the underscore is part of the prefix). SPEC §4.3.2.
const rawKeyPrefix = "unblock_pat_"

// rawKeyRandomBytes is the count of crypto/rand bytes that feed the
// base32 portion of every raw key. 32 bytes = 256 bits of entropy.
const rawKeyRandomBytes = 32

// rawKeyTotalLen is the expected total length of a well-formed raw key:
// `len(rawKeyPrefix)` + `len(base32-no-padding(32 bytes))` = 12 + 52 = 64.
const rawKeyTotalLen = len(rawKeyPrefix) + rawKeyEncodedLen

// rawKeyEncodedLen is the base32-no-padding encoded length of
// rawKeyRandomBytes. Computed at package-init via the encoding's
// formula: ceil(n/5) * 8 chars, then trimmed of '=' padding. For
// n=32 this is 52.
const rawKeyEncodedLen = 52

// keyPrefixLen is the number of leading chars of the random base32
// portion that populate `mcp.api_keys.key_prefix`. SPEC §4.3.2 line 456.
const keyPrefixLen = 8

// rawKeyEncoder is the lowercase variant of standard base32 with the
// padding character disabled. SPEC §4.3.2 line 455 ("base32 (no
// padding, lowercase)").
//
// Encore's stdlib does not ship a lowercase base32 encoding, so we
// derive one by remapping the standard alphabet at package init.
var rawKeyEncoder = base32.NewEncoding("abcdefghijklmnopqrstuvwxyz234567").WithPadding(base32.NoPadding)

// ErrInvalidRawKey is returned when an input key cannot be parsed for
// prefix extraction (wrong length or missing brand prefix). The Bearer
// auth hot path translates this into errs.Unauthenticated; never echo
// the raw input value back to the caller.
var ErrInvalidRawKey = errors.New("auth: invalid raw API key format")

// generateRawKey produces a fresh raw key with crypto/rand entropy.
// Returns the full key string. The caller is responsible for
// (a) storing the HMAC hash in `mcp.api_keys.key_hash`, (b) returning
// the raw key to its consumer EXACTLY ONCE, and (c) never logging it.
func generateRawKey() (string, error) {
	buf := make([]byte, rawKeyRandomBytes)
	if _, err := rand.Read(buf); err != nil {
		return "", fmt.Errorf("auth: read crypto/rand: %w", err)
	}
	encoded := rawKeyEncoder.EncodeToString(buf)
	if len(encoded) != rawKeyEncodedLen {
		// Should be impossible — base32(32 bytes, no padding) is
		// always 52 chars. Defensive guard against future format
		// drift (e.g. someone retunes rawKeyRandomBytes without
		// updating the constants).
		return "", fmt.Errorf("auth: encoded key length %d != %d", len(encoded), rawKeyEncodedLen)
	}
	return rawKeyPrefix + encoded, nil
}

// prefixOf extracts the `key_prefix` value (first 8 chars of the
// random base32 portion) from a raw key. The literal `unblock_pat_`
// prefix is stripped before slicing — see DRIFT-A in the bead
// investigation. Returns ErrInvalidRawKey on any length mismatch or
// missing brand prefix; callers in the hot path translate that into
// errs.Unauthenticated.
func prefixOf(rawKey string) (string, error) {
	if len(rawKey) != rawKeyTotalLen {
		return "", ErrInvalidRawKey
	}
	if rawKey[:len(rawKeyPrefix)] != rawKeyPrefix {
		return "", ErrInvalidRawKey
	}
	return rawKey[len(rawKeyPrefix) : len(rawKeyPrefix)+keyPrefixLen], nil
}

// hashRawKey computes HMAC-SHA256(secret, rawKey) and returns the
// 32-byte raw digest. The output is stored in `mcp.api_keys.key_hash`
// as bytea and compared with crypto/subtle.ConstantTimeCompare on every
// Bearer auth check (SPEC §4.3.2 steps 5-6).
func hashRawKey(secret, rawKey string) []byte {
	mac := hmac.New(sha256.New, []byte(secret))
	mac.Write([]byte(rawKey))
	return mac.Sum(nil)
}
