// Unit tests for the API key format helpers.
//
// Scope (B-1, bead unblock-tv8.7):
//   - generateRawKey produces the locked 64-char `unblock_pat_<base32-32-byte>` shape.
//   - prefixOf strips the literal `unblock_pat_` prefix per DRIFT-A
//     (raw_key[12:20], NOT raw_key[:8]).
//   - hashRawKey is deterministic (HMAC-SHA256 with the secrets-manifest secret).
//
// These tests do NOT require Docker / encore CLI — `go test ./auth/...`
// runs them directly because none of the helpers touch sqldb at unit
// time (the package-level `db` var registers an Encore resource but no
// connection is opened until a Database method is invoked).

package auth

import (
	"bytes"
	"strings"
	"testing"
)

func TestGenerateRawKey(t *testing.T) {
	t.Run("produces a well-formed 64-char unblock_pat_ key", func(t *testing.T) {
		got, err := generateRawKey()
		if err != nil {
			t.Fatalf("generateRawKey: %v", err)
		}
		if len(got) != rawKeyTotalLen {
			t.Fatalf("len(rawKey) = %d, want %d", len(got), rawKeyTotalLen)
		}
		if !strings.HasPrefix(got, rawKeyPrefix) {
			t.Fatalf("rawKey %q missing prefix %q", got, rawKeyPrefix)
		}
		// Body must be 52 chars of lowercase Crockford-like base32
		// (no padding). We verify the alphabet defensively — drift in
		// the encoder would silently change the wire format.
		body := got[len(rawKeyPrefix):]
		if len(body) != rawKeyEncodedLen {
			t.Fatalf("len(body) = %d, want %d", len(body), rawKeyEncodedLen)
		}
		const allowed = "abcdefghijklmnopqrstuvwxyz234567"
		for i, c := range body {
			if !strings.ContainsRune(allowed, c) {
				t.Fatalf("body[%d] = %q not in allowed alphabet", i, c)
			}
		}
	})

	t.Run("two calls produce distinct keys", func(t *testing.T) {
		// crypto/rand collisions on 32 bytes are mathematically
		// infeasible; the assertion guards against a future
		// regression where the entropy source is accidentally
		// substituted with a deterministic generator.
		a, err := generateRawKey()
		if err != nil {
			t.Fatalf("generateRawKey #1: %v", err)
		}
		b, err := generateRawKey()
		if err != nil {
			t.Fatalf("generateRawKey #2: %v", err)
		}
		if a == b {
			t.Fatalf("two generateRawKey calls collided: %q", a)
		}
	})
}

func TestPrefixOf(t *testing.T) {
	tests := []struct {
		name    string
		input   string
		want    string
		wantErr bool
	}{
		{
			name:  "well-formed key returns first 8 chars after the brand prefix",
			input: "unblock_pat_abcdefghijklmnopqrstuvwxyz234567abcdefghijklmnop",
			want:  "abcdefgh",
		},
		{
			name:    "wrong total length is rejected",
			input:   "unblock_pat_short",
			wantErr: true,
		},
		{
			name:    "missing brand prefix is rejected even when length matches",
			input:   strings.Repeat("a", rawKeyTotalLen),
			wantErr: true,
		},
		{
			name:    "empty input is rejected",
			input:   "",
			wantErr: true,
		},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			got, err := prefixOf(tc.input)
			if tc.wantErr {
				if err == nil {
					t.Fatalf("prefixOf(%q) = %q, want error", tc.input, got)
				}
				return
			}
			if err != nil {
				t.Fatalf("prefixOf(%q): %v", tc.input, err)
			}
			if got != tc.want {
				t.Fatalf("prefixOf(%q) = %q, want %q", tc.input, got, tc.want)
			}
			if len(got) != keyPrefixLen {
				t.Fatalf("prefix len = %d, want %d", len(got), keyPrefixLen)
			}
		})
	}
}

func TestPrefixOfRoundTripFromGenerate(t *testing.T) {
	// End-to-end smoke: a freshly generated key always parses, and
	// the extracted prefix is a substring of the raw key starting
	// at byte 12 (DRIFT-A invariant).
	raw, err := generateRawKey()
	if err != nil {
		t.Fatalf("generateRawKey: %v", err)
	}
	prefix, err := prefixOf(raw)
	if err != nil {
		t.Fatalf("prefixOf: %v", err)
	}
	if raw[12:20] != prefix {
		t.Fatalf("raw[12:20] = %q, prefix = %q", raw[12:20], prefix)
	}
}

func TestHashRawKey(t *testing.T) {
	t.Run("is deterministic for the same secret + key", func(t *testing.T) {
		secret := "test-hmac-secret"
		key := "unblock_pat_" + strings.Repeat("a", rawKeyEncodedLen)
		a := hashRawKey(secret, key)
		b := hashRawKey(secret, key)
		if !bytes.Equal(a, b) {
			t.Fatalf("hashRawKey not deterministic: %x vs %x", a, b)
		}
	})

	t.Run("different secrets produce different digests", func(t *testing.T) {
		key := "unblock_pat_" + strings.Repeat("a", rawKeyEncodedLen)
		a := hashRawKey("secret-one", key)
		b := hashRawKey("secret-two", key)
		if bytes.Equal(a, b) {
			t.Fatalf("different secrets produced the same digest %x", a)
		}
	})

	t.Run("different keys produce different digests", func(t *testing.T) {
		secret := "shared-secret"
		a := hashRawKey(secret, "unblock_pat_"+strings.Repeat("a", rawKeyEncodedLen))
		b := hashRawKey(secret, "unblock_pat_"+strings.Repeat("b", rawKeyEncodedLen))
		if bytes.Equal(a, b) {
			t.Fatalf("different keys produced the same digest %x", a)
		}
	})

	t.Run("digest length is 32 bytes (HMAC-SHA256)", func(t *testing.T) {
		got := hashRawKey("s", "k")
		if len(got) != 32 {
			t.Fatalf("digest len = %d, want 32", len(got))
		}
	})
}
