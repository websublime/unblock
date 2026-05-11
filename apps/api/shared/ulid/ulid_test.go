// Unit tests for the shared ULID generator.

package ulid

import (
	"strings"
	"testing"
)

func TestNew(t *testing.T) {
	t.Run("produces 26 chars from the Crockford alphabet", func(t *testing.T) {
		got, err := New()
		if err != nil {
			t.Fatalf("New: %v", err)
		}
		if len(got) != 26 {
			t.Fatalf("len(ulid) = %d, want 26", len(got))
		}
		for i, c := range got {
			if !strings.ContainsRune(Alphabet(), c) {
				t.Fatalf("ulid[%d] = %q not in Crockford alphabet", i, c)
			}
		}
	})

	t.Run("two consecutive ulids differ", func(t *testing.T) {
		// 80-bit random tail per ms — collision is mathematically
		// negligible, so a hit signals an entropy regression.
		a, err := New()
		if err != nil {
			t.Fatalf("New #1: %v", err)
		}
		b, err := New()
		if err != nil {
			t.Fatalf("New #2: %v", err)
		}
		if a == b {
			t.Fatalf("two New calls collided: %q", a)
		}
	})
}
