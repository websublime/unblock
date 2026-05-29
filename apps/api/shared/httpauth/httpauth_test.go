// Unit tests for the shared Bearer-header parser.
//
// These cases were lifted verbatim from the former parseBearer subtests
// in apps/api/auth/authhandler_test.go (bead unblock-tv8.55) so coverage
// of the parsing logic travels with the implementation. The package has
// zero Encore imports and runs under plain `go test`.
package httpauth

import "testing"

func TestParseBearer(t *testing.T) {
	tests := []struct {
		name    string
		input   string
		wantTok string
		wantOK  bool
	}{
		{name: "canonical form", input: "Bearer abc", wantTok: "abc", wantOK: true},
		{name: "case-insensitive scheme", input: "bearer xyz", wantTok: "xyz", wantOK: true},
		{name: "empty", input: "", wantOK: false},
		{name: "scheme only", input: "Bearer ", wantOK: false},
		{name: "no scheme", input: "abc", wantOK: false},
		{name: "trailing space", input: "Bearer abc ", wantOK: false},
		{name: "leading space (after scheme)", input: "Bearer  abc", wantOK: false},
		{name: "wrong scheme", input: "Basic abc", wantOK: false},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			got, ok := ParseBearer(tc.input)
			if ok != tc.wantOK {
				t.Fatalf("ParseBearer(%q) ok=%v, want %v", tc.input, ok, tc.wantOK)
			}
			if got != tc.wantTok {
				t.Fatalf("ParseBearer(%q) tok=%q, want %q", tc.input, got, tc.wantTok)
			}
		})
	}
}
