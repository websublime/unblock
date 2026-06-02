// Tests for the trace_id mint-failure sentinel (bead unblock-tv8.45).
//
// These are pure-value tests that run under plain `go test ./mcp/...`
// (the mcp package root is loadable without the Encore runtime via the
// BindDB late-bind shape — see recordtoolcall_test.go header).
//
// The crypto/rand failure path inside ulid.New() has no entropy-injection
// seam, so an end-to-end "force rand failure" test is not possible. The
// sentinel-fallback logic is therefore factored into mintTraceID() and the
// invariants of the sentinel value itself (well-formed ULID shape,
// non-empty, NULL-collapse round-trip) are asserted directly here.

package mcp

import (
	"strings"
	"testing"

	"encore.app/shared/ulid"
)

func TestTraceIDMintFailedSentinel(t *testing.T) {
	t.Run("is non-empty so the audit row and §7 envelope are never blank", func(t *testing.T) {
		if TraceIDMintFailedSentinel == "" {
			t.Fatal("sentinel must be non-empty")
		}
	})

	t.Run("is a well-formed 26-char ULID shape per SPEC §10.2", func(t *testing.T) {
		if got := len(TraceIDMintFailedSentinel); got != 26 {
			t.Fatalf("sentinel length = %d, want 26 (ULID is 26 Crockford-base32 chars)", got)
		}
	})

	t.Run("uses only Crockford-base32 alphabet chars", func(t *testing.T) {
		alphabet := ulid.Alphabet()
		for i, r := range TraceIDMintFailedSentinel {
			if !strings.ContainsRune(alphabet, r) {
				t.Fatalf("sentinel char %q at index %d not in Crockford alphabet %q", r, i, alphabet)
			}
		}
	})

	t.Run("collapses to a NON-NULL value through the audit nullable helper", func(t *testing.T) {
		// recordToolCall writes nullable(trace_id) into
		// mcp.tool_calls.trace_id. A NON-NULL sentinel must NOT collapse
		// to SQL NULL — that is the whole point of the bead.
		got := nullable(TraceIDMintFailedSentinel)
		if got == nil {
			t.Fatal("nullable(sentinel) = nil; mint-failure audit row would be NULL trace_id")
		}
		if *got != TraceIDMintFailedSentinel {
			t.Fatalf("nullable(sentinel) = %q, want %q", *got, TraceIDMintFailedSentinel)
		}
	})
}

func TestMintTraceID(t *testing.T) {
	// On the success path mintTraceID returns a fresh, non-sentinel ULID
	// and reports mintFailed=false. (The failure branch is exercised by
	// the sentinel-value invariants above — ulid.New has no rand seam to
	// force the failure path here.)
	id, mintFailed := mintTraceID()
	if mintFailed {
		t.Fatal("mintTraceID reported failure on a healthy crypto/rand source")
	}
	if id == "" {
		t.Fatal("mintTraceID returned empty trace_id on the success path")
	}
	if id == TraceIDMintFailedSentinel {
		t.Fatalf("mintTraceID returned the sentinel %q on the success path", TraceIDMintFailedSentinel)
	}
	if len(id) != 26 {
		t.Fatalf("mintTraceID returned %d-char id, want 26", len(id))
	}
}
