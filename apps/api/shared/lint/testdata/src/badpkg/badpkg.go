// Fixture for analysistest: a package that MUST be flagged for any
// UPDATE on workitems.items.is_ready or pipeline_stage. The `want`
// comments declare the expected diagnostic positions.
package badpkg

const (
	// A direct UPDATE on is_ready outside the allowed package.
	flagged1 = `UPDATE workitems.items SET is_ready = true WHERE id = $1` // want `direct UPDATE on workitems.items.is_ready or pipeline_stage outside encore.app/deps`

	// Multi-line UPDATE expressed as a regular string literal so the
	// diagnostic position lands on a single source line that the
	// `want` annotation can match.
	flagged2 = "UPDATE workitems.items\n   SET pipeline_stage = 'Done'\n WHERE id = $1" // want `direct UPDATE on workitems.items.is_ready or pipeline_stage outside encore.app/deps`

	// A clean update on a different column must NOT fire.
	clean = `UPDATE workitems.items SET status = 'Done' WHERE id = $1`
)

// Reference the consts so go vet does not complain about unused
// package-level identifiers in the analysistest run.
var _ = flagged1 + flagged2 + clean
