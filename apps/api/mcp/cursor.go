// cursor.go owns the §6.2.0 cursor keyset pagination contract for
// MCP Tools 2 (`ready`), 8 (`list`), and 9 (`search`).
//
// Encoding: each cursor is `<base64url(payload_json)>.<base64url(hmac)>`
// where `payload_json` is the canonical JSON-marshal of one of
// {readyCursor, listCursor, searchCursor} and `hmac` is the raw
// HMAC-SHA256(secrets.APIKeyHMACSecret, payload_json) tag. The
// envelope is the same shape across tools so the decoder can
// peek the discriminator (`v`) without exposing internal tuples.
//
// Verification: decode panics → VALIDATION; HMAC mismatch (constant-
// time compare via crypto/subtle.ConstantTimeCompare) → VALIDATION;
// wrong tuple shape (`v` field discriminator does not match the
// expected tool) → VALIDATION. All three collapse to the same wire
// error: §7 envelope `data.field = "cursor"`. The caller wraps the
// errs.InvalidArgument we return via mapError.
//
// Secret material: the MCP service declares its own Encore secrets
// struct holding APIKeyHMACSecret (per round-7 §6.2.0: "re-used; no
// new secret is introduced in P01"). Encore allows multiple services
// to declare the same logical secret; the production value is
// provisioned once via `encore secret set` and exposed to each
// declaring service. Rotating the secret invalidates every
// outstanding cursor — operationally identical to the Bearer-key
// rotation tradeoff documented at §4.3.2.
//
// SPEC: docs/specs/01-spec-backend-mvp.md §6.2.0 (Cursor keyset
// pagination); §6.2 Tools 2/8/9 contracts; §7 (VALIDATION envelope).

package mcp

import (
	"crypto/hmac"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/base64"
	"encoding/json"
	"errors"
	"strings"

	"encore.dev/beta/errs"
)

// secrets is the MCP service's view of the deployment secrets
// manifest. We only consume APIKeyHMACSecret here (re-used per
// §6.2.0 — "no new secret is introduced in P01"); Encore allows
// multiple services to declare the same logical secret name. The
// production value is provisioned once via `encore secret set` and
// exposed to every declaring service. Local emulator reads from
// `apps/api/.secrets.local.cue` (shared with auth).
//
//nolint:unused // referenced by encodeCursor/decodeCursor.
var secrets struct {
	APIKeyHMACSecret string
}

// cursorVersion discriminators — embedded in the payload JSON so a
// caller cannot replay a cursor from one tool against a different
// tool. Mismatch is treated as VALIDATION (per §6.2.0).
const (
	cursorVersionReady  = "r1"
	cursorVersionList   = "l1"
	cursorVersionSearch = "s1"
)

// readyCursor is the §6.2.0 keyset anchor for Tool 2 (`ready`).
// Tuple = (priority, created_at_unix_us, id). Strict ordering on
// (priority ASC, created_at ASC, id ASC) — see SPEC §6.2 Tool 2.
type readyCursor struct {
	V               string `json:"v"`
	Priority        string `json:"p"`
	CreatedAtUnixUS int64  `json:"c"`
	ID              string `json:"i"`
}

// listCursor is the §6.2.0 keyset anchor for Tool 8 (`list`).
// Tuple = (id). List orders by `id ASC` only (workitems.List).
type listCursor struct {
	V  string `json:"v"`
	ID string `json:"i"`
}

// searchCursor is the §6.2.0 keyset anchor for Tool 9 (`search`).
// Tuple = (rank, item_id, comment_id). FTS rows order by
// `rank DESC, item_id ASC, comment_id ASC`. `comment_id` is the
// empty string for source="item" rows.
type searchCursor struct {
	V         string  `json:"v"`
	Rank      float64 `json:"r"`
	ItemID    string  `json:"i"`
	CommentID string  `json:"c"`
}

// encodeCursor signs and base64url-encodes a typed cursor payload.
// Returns the wire string. Used by the three read tools when
// emitting next_cursor.
func encodeCursor(payload any) (string, error) {
	raw, err := json.Marshal(payload)
	if err != nil {
		return "", err
	}
	payloadB64 := base64.RawURLEncoding.EncodeToString(raw)

	mac := hmac.New(sha256.New, []byte(secrets.APIKeyHMACSecret))
	mac.Write(raw)
	tagB64 := base64.RawURLEncoding.EncodeToString(mac.Sum(nil))

	return payloadB64 + "." + tagB64, nil
}

// decodeCursor verifies the HMAC tag and unmarshals the payload
// JSON into dst. The `version` argument is the expected
// discriminator (cursorVersionReady / cursorVersionList /
// cursorVersionSearch); a mismatch is reported as VALIDATION just
// like a decode/hmac failure so the caller cannot use the error
// shape to fingerprint which tool minted the cursor.
//
// Returns errs.InvalidArgument with Meta.field="cursor" on any
// failure path — the MCP errmap translates that to §7 VALIDATION
// (data.field = "cursor") per the round-7 contract.
func decodeCursor(token, version string, dst any) error {
	if token == "" {
		return nil
	}
	parts := strings.SplitN(token, ".", 2)
	if len(parts) != 2 {
		return cursorValidationErr("malformed cursor")
	}
	rawPayload, err := base64.RawURLEncoding.DecodeString(parts[0])
	if err != nil {
		return cursorValidationErr("cursor payload decode failed")
	}
	rawTag, err := base64.RawURLEncoding.DecodeString(parts[1])
	if err != nil {
		return cursorValidationErr("cursor tag decode failed")
	}

	mac := hmac.New(sha256.New, []byte(secrets.APIKeyHMACSecret))
	mac.Write(rawPayload)
	expected := mac.Sum(nil)
	// ConstantTimeCompare returns 0 on length-mismatch as well as on
	// content-mismatch; subtle is the right primitive even though we
	// already know the tag length is fixed (32 bytes for SHA-256).
	if subtle.ConstantTimeCompare(rawTag, expected) != 1 {
		return cursorValidationErr("cursor signature invalid")
	}

	if err := json.Unmarshal(rawPayload, dst); err != nil {
		return cursorValidationErr("cursor payload shape invalid")
	}

	// Version discriminator — the unmarshalled value is reached via
	// type assertion so we can read the V field without reflection.
	var got string
	switch t := dst.(type) {
	case *readyCursor:
		got = t.V
	case *listCursor:
		got = t.V
	case *searchCursor:
		got = t.V
	default:
		return cursorValidationErr("cursor payload type unknown")
	}
	if got != version {
		return cursorValidationErr("cursor version mismatch")
	}
	return nil
}

// cursorValidationErr is the shared shape returned by decodeCursor on
// every failure path. errs.InvalidArgument + Meta.field="cursor" so
// errmap.classifyEnvelopeError produces §7 VALIDATION with
// data.field = "cursor".
func cursorValidationErr(reason string) error {
	return &errs.Error{
		Code:    errs.InvalidArgument,
		Message: reason,
		Meta:    errs.Metadata{"field": "cursor", "reason": reason},
	}
}

// errCursorEmpty is returned by the decoder when the caller passes
// an empty token — not an error, just the "first page" signal. We
// keep the const here as a named sentinel for readability; the
// decoder returns nil instead.
//
// returns nil rather than this value to keep the call site simple.
//
//nolint:unused // documents the empty-token semantics; the decoder
var errCursorEmpty = errors.New("cursor: empty (first page)")
