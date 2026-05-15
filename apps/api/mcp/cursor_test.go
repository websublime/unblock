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
	"crypto/rand"
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
// be verified with a tampered tag. We simulate this by minting a
// token, then flipping a *raw* byte of the HMAC tag (decode →
// XOR 0x01 → re-encode) before passing it to decodeCursor.
//
// Why raw-byte flip and not base64-char flip: the HMAC-SHA256 tag
// is 32 bytes = 256 bits, which RawURLEncoding emits as 43 base64
// chars. The 43rd char carries only 4 meaningful bits + 2
// don't-care padding bits, and Go's *Encoding decodes non-Strict
// by default — so a single-char flip can land on a char whose
// 4 MSB match the original and decode to identical bytes,
// silently passing HMAC verify (~25% probability per random
// secret). See unblock-tv8.58 for the CI symptom.
//
// To guard against regressions across secret values, we sweep
// 100 cryptographically-random 32-byte secrets, signing under
// each and asserting tamper detection every time.
func TestCursor_HMACMismatch(t *testing.T) {
	const iterations = 100
	// Save and restore the package-level secret so we do not leak
	// state into adjacent tests in the same binary.
	orig := secrets.APIKeyHMACSecret
	t.Cleanup(func() { secrets.APIKeyHMACSecret = orig })

	for i := 0; i < iterations; i++ {
		// 32 random bytes — full entropy across the SHA-256 keyspace.
		var key [32]byte
		if _, err := rand.Read(key[:]); err != nil {
			t.Fatalf("rand.Read: %v", err)
		}
		secrets.APIKeyHMACSecret = string(key[:])

		original := readyCursor{V: cursorVersionReady, ID: "01HZZZ1234567890ABCDEFGHJK"}
		token, err := encodeCursor(original)
		if err != nil {
			t.Fatalf("iter=%d encodeCursor: %v", i, err)
		}
		tampered, err := tamperTag(token)
		if err != nil {
			t.Fatalf("iter=%d tamperTag: %v", i, err)
		}
		if tampered == token {
			t.Fatalf("iter=%d tamperTag returned unchanged token: %q", i, token)
		}
		var dst readyCursor
		err = decodeCursor(tampered, cursorVersionReady, &dst)
		if err == nil {
			t.Fatalf("iter=%d expected HMAC mismatch error; got nil (secret=%x token=%q tampered=%q)",
				i, key, token, tampered)
		}
		assertCursorValidationErr(t, err)
	}
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

// tamperTag returns a copy of `token` (payload.tag) with the HMAC
// tag bytes mutated by XOR-ing 0x01 into the first byte. The
// mutation is applied in raw-byte space (decode → mutate → re-
// encode), which guarantees the re-encoded tag differs from the
// original by at least one byte regardless of secret value — in
// contrast to a base64-char flip, which can be a no-op under
// non-Strict decoding when the flipped char shares its 4 MSB
// with the original (see unblock-tv8.58).
func tamperTag(token string) (string, error) {
	dot := strings.LastIndex(token, ".")
	if dot < 0 {
		return "", errors.New("token missing separator")
	}
	tagB64 := token[dot+1:]
	rawTag, err := base64.RawURLEncoding.DecodeString(tagB64)
	if err != nil {
		return "", err
	}
	if len(rawTag) == 0 {
		return "", errors.New("empty tag")
	}
	rawTag[0] ^= 0x01
	return token[:dot+1] + base64.RawURLEncoding.EncodeToString(rawTag), nil
}
