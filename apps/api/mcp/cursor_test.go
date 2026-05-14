// cursor_test.go covers the §6.2.0 cursor keyset pagination
// helpers (mcp/cursor.go).
//
// Pure unit tests — no DB, no Encore runtime. Exercises the
// encode/verify round-trip, the VALIDATION envelope shape on every
// failure path (malformed token, decode error, HMAC mismatch, wrong
// tuple shape, version discriminator mismatch), and the
// cross-tool isolation guarantee (a Tool 2 cursor MUST NOT decode
// as a Tool 8 cursor).
//
// These tests run under plain `go test ./mcp/...` because cursor.go
// has no sqldb / pubsub init-time dependencies — only an Encore
// secrets struct, which the runtime initialises to its zero value
// when `encore test` is not in the loop. The zero-secret path is
// fine for HMAC round-tripping (encoder and decoder use the same
// empty key) and is what the unit tests exercise.

package mcp

import (
	"encoding/base64"
	"errors"
	"strings"
	"testing"

	"encore.dev/beta/errs"
)

func TestCursor_ReadyRoundTrip(t *testing.T) {
	original := readyCursor{
		V:               cursorVersionReady,
		Priority:        "P2",
		CreatedAtUnixUS: 1715000000000000,
		ID:              "01HZZZ1234567890ABCDEFGHJK",
	}
	token, err := encodeCursor(original)
	if err != nil {
		t.Fatalf("encodeCursor: %v", err)
	}
	if token == "" {
		t.Fatalf("encodeCursor returned empty token")
	}
	if !strings.Contains(token, ".") {
		t.Fatalf("token missing payload.tag separator: %q", token)
	}

	var decoded readyCursor
	if err := decodeCursor(token, cursorVersionReady, &decoded); err != nil {
		t.Fatalf("decodeCursor: %v", err)
	}
	if decoded != original {
		t.Fatalf("decoded != original:\n got  %+v\n want %+v", decoded, original)
	}
}

func TestCursor_ListRoundTrip(t *testing.T) {
	original := listCursor{V: cursorVersionList, ID: "01HZZZ1234567890ABCDEFGHJK"}
	token, err := encodeCursor(original)
	if err != nil {
		t.Fatalf("encodeCursor: %v", err)
	}
	var decoded listCursor
	if err := decodeCursor(token, cursorVersionList, &decoded); err != nil {
		t.Fatalf("decodeCursor: %v", err)
	}
	if decoded != original {
		t.Fatalf("decoded != original:\n got  %+v\n want %+v", decoded, original)
	}
}

func TestCursor_SearchRoundTrip(t *testing.T) {
	original := searchCursor{
		V:         cursorVersionSearch,
		Rank:      0.875,
		ItemID:    "01HZZZ1234567890ABCDEFGHJK",
		CommentID: "01HZZZ9876543210ZYXWVUTSRQ",
	}
	token, err := encodeCursor(original)
	if err != nil {
		t.Fatalf("encodeCursor: %v", err)
	}
	var decoded searchCursor
	if err := decodeCursor(token, cursorVersionSearch, &decoded); err != nil {
		t.Fatalf("decodeCursor: %v", err)
	}
	if decoded != original {
		t.Fatalf("decoded != original:\n got  %+v\n want %+v", decoded, original)
	}
}

// TestCursor_EmptyTokenIsFirstPage: passing in_cursor="" must NOT
// return an error — that is the "first page" signal. Callers rely
// on this so they can shovel an unconditional cursor argument
// straight into decodeCursor without an explicit nil check.
func TestCursor_EmptyTokenIsFirstPage(t *testing.T) {
	var dst readyCursor
	if err := decodeCursor("", cursorVersionReady, &dst); err != nil {
		t.Fatalf("empty token decode should return nil: %v", err)
	}
	if dst != (readyCursor{}) {
		t.Fatalf("empty token decode mutated dst: %+v", dst)
	}
}

// TestCursor_MalformedToken: every shape error MUST collapse to
// errs.InvalidArgument + Meta.field="cursor" so the §7 envelope
// surfaces as VALIDATION (data.field = "cursor").
func TestCursor_MalformedToken(t *testing.T) {
	cases := map[string]string{
		"no separator":      "abcdef",
		"only separator":    ".",
		"bad b64 payload":   "@@@.AAAA",
		"bad b64 tag":       base64.RawURLEncoding.EncodeToString([]byte(`{"v":"r1"}`)) + ".@@@",
		"non-json payload":  base64.RawURLEncoding.EncodeToString([]byte("not-json")) + "." + base64.RawURLEncoding.EncodeToString([]byte("tag")),
		"three sections":    "a.b.c", // SplitN(_, 2) → ("a", "b.c") — decode of "b.c" as b64 fails
		"single dot suffix": "abc.",
	}
	for name, tok := range cases {
		t.Run(name, func(t *testing.T) {
			var dst readyCursor
			err := decodeCursor(tok, cursorVersionReady, &dst)
			if err == nil {
				t.Fatalf("expected error on %q; got nil", tok)
			}
			assertCursorValidationErr(t, err)
		})
	}
}

// TestCursor_HMACMismatch: a token signed with one secret cannot
// be verified with a different secret. We simulate this by
// minting a token, then flipping a tag byte before decode.
func TestCursor_HMACMismatch(t *testing.T) {
	original := readyCursor{V: cursorVersionReady, ID: "01HZZZ1234567890ABCDEFGHJK"}
	token, err := encodeCursor(original)
	if err != nil {
		t.Fatalf("encodeCursor: %v", err)
	}
	// Flip the last byte of the tag (the segment after the dot).
	dot := strings.LastIndex(token, ".")
	if dot < 0 {
		t.Fatalf("token missing separator: %q", token)
	}
	tampered := token[:dot+1] + flipLastByte(token[dot+1:])
	var dst readyCursor
	err = decodeCursor(tampered, cursorVersionReady, &dst)
	if err == nil {
		t.Fatalf("expected HMAC mismatch error; got nil (token=%q tampered=%q)", token, tampered)
	}
	assertCursorValidationErr(t, err)
}

// TestCursor_VersionMismatch: a Tool 2 cursor presented to Tool 8
// MUST fail with VALIDATION — cursors are NOT cross-tool portable.
func TestCursor_VersionMismatch(t *testing.T) {
	tok, err := encodeCursor(readyCursor{V: cursorVersionReady, ID: "01HZZZ1234567890ABCDEFGHJK"})
	if err != nil {
		t.Fatalf("encodeCursor: %v", err)
	}
	var dst listCursor
	err = decodeCursor(tok, cursorVersionList, &dst)
	if err == nil {
		t.Fatalf("expected version mismatch error; got nil")
	}
	assertCursorValidationErr(t, err)
}

// TestCursor_WrongTuple: a payload signed correctly but missing
// the expected discriminator surfaces as VALIDATION. We exercise
// this by minting a listCursor (V="l1") and attempting to decode
// it as a readyCursor (expects V="r1") — Unmarshal succeeds (V is
// left zero), then the version check fails.
func TestCursor_WrongTuple(t *testing.T) {
	tok, err := encodeCursor(listCursor{V: cursorVersionList, ID: "01HZZZ1234567890ABCDEFGHJK"})
	if err != nil {
		t.Fatalf("encodeCursor: %v", err)
	}
	var dst readyCursor
	err = decodeCursor(tok, cursorVersionReady, &dst)
	if err == nil {
		t.Fatalf("expected wrong-tuple error; got nil")
	}
	assertCursorValidationErr(t, err)
}

// assertCursorValidationErr verifies the §6.2.0 contract: any
// decoder error MUST be errs.InvalidArgument + Meta.field="cursor"
// so errmap.classifyEnvelopeError maps it to §7 VALIDATION with
// data.field = "cursor".
func assertCursorValidationErr(t *testing.T, err error) {
	t.Helper()
	var e *errs.Error
	if !errors.As(err, &e) {
		t.Fatalf("expected *errs.Error, got %T: %v", err, err)
	}
	if e.Code != errs.InvalidArgument {
		t.Fatalf("expected errs.InvalidArgument, got %s", e.Code)
	}
	if v, _ := e.Meta["field"].(string); v != "cursor" {
		t.Fatalf("expected Meta.field=\"cursor\", got %q", v)
	}
}

func flipLastByte(s string) string {
	if s == "" {
		return s
	}
	b := []byte(s)
	// Flip case on the last byte (cheap perturbation that keeps it
	// in the base64url alphabet but changes the decoded value).
	last := b[len(b)-1]
	switch {
	case last >= 'A' && last <= 'Z':
		b[len(b)-1] = last - 'A' + 'a'
	case last >= 'a' && last <= 'z':
		b[len(b)-1] = last - 'a' + 'A'
	case last >= '0' && last <= '8':
		b[len(b)-1] = last + 1
	case last == '9':
		b[len(b)-1] = '0'
	default:
		b[len(b)-1] = 'A'
	}
	return string(b)
}
