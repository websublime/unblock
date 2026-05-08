// Tests for NoDirectIsReadyWriteAnalyzer using analysistest.
//
// Two fixtures live under testdata/src/:
//
//   - badpkg: NOT the cascade-subscriber package; every targeted SQL
//     literal must be flagged.
//   - encore.app/deps: the spec-allow-listed package; every targeted
//     literal must pass without diagnostic.
//
// The analyzer's allow-list compares against pass.Pkg.Path(); the
// fixture path under testdata/src/ becomes the import path during
// analysistest, so encore.app/deps is reachable verbatim.
package lint

import (
	"testing"

	"golang.org/x/tools/go/analysis/analysistest"
)

// TestNoDirectIsReadyWrite_FlagsForbiddenPackage runs the analyzer
// against the badpkg fixture and asserts every `// want` annotation
// is satisfied.
func TestNoDirectIsReadyWrite_FlagsForbiddenPackage(t *testing.T) {
	testdata := analysistest.TestData()
	analysistest.Run(t, testdata, NoDirectIsReadyWriteAnalyzer, "badpkg")
}

// TestNoDirectIsReadyWrite_AllowsCascadeSubscriber runs against the
// encore.app/deps fixture and asserts NO diagnostic fires. The
// fixture mirrors the SQL the cascade subscriber is expected to
// emit in C-3 (unblock-tv8.12).
func TestNoDirectIsReadyWrite_AllowsCascadeSubscriber(t *testing.T) {
	testdata := analysistest.TestData()
	analysistest.Run(t, testdata, NoDirectIsReadyWriteAnalyzer, "encore.app/deps")
}
