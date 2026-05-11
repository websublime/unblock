// Unit tests for the inline ULID generator.

package auth

import (
	"strings"
	"testing"
)

func TestNewULID(t *testing.T) {
	t.Run("produces 26 chars from the Crockford alphabet", func(t *testing.T) {
		got, err := newULID()
		if err != nil {
			t.Fatalf("newULID: %v", err)
		}
		if len(got) != 26 {
			t.Fatalf("len(ulid) = %d, want 26", len(got))
		}
		for i, c := range got {
			if !strings.ContainsRune(crockfordAlphabet, c) {
				t.Fatalf("ulid[%d] = %q not in Crockford alphabet", i, c)
			}
		}
	})

	t.Run("two consecutive ulids differ", func(t *testing.T) {
		// 80-bit random tail per ms — collision is mathematically
		// negligible, so a hit signals an entropy regression.
		a, err := newULID()
		if err != nil {
			t.Fatalf("newULID #1: %v", err)
		}
		b, err := newULID()
		if err != nil {
			t.Fatalf("newULID #2: %v", err)
		}
		if a == b {
			t.Fatalf("two newULID calls collided: %q", a)
		}
	})
}
