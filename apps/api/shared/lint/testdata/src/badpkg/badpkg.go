// Fixture for analysistest: a package that MUST be flagged for any
// UPDATE on workitems.items.is_ready or pipeline_stage. Inline
// annotations on each positive literal declare the expected
// diagnostic positions.
//
// Layout: positive cases (must fire) carry an annotation on the
// literal's source line; negative cases (must NOT fire) carry an
// inline comment explaining what false-positive class they pin down
// for the regex matcher.
package badpkg

const (
	// Positive: a direct UPDATE on is_ready outside the allowed package.
	flagged1 = `UPDATE workitems.items SET is_ready = true WHERE id = $1` // want `direct UPDATE on workitems.items.is_ready or pipeline_stage outside encore.app/deps`

	// Positive: multi-line UPDATE expressed as a regular string literal
	// so the diagnostic position lands on a single source line that the
	// `want` annotation can match. Pins the (?s) dotall flag — without
	// it the .* between UPDATE and SET would not span the embedded
	// newline.
	flagged2 = "UPDATE workitems.items\n   SET pipeline_stage = 'Done'\n WHERE id = $1" // want `direct UPDATE on workitems.items.is_ready or pipeline_stage outside encore.app/deps`

	// Positive: regular Go string with \n escape between UPDATE and
	// the items table. Pins the (?:\s|\\.) gap accepting Go-source
	// escape bytes (the literal two-character sequence \n) in addition
	// to real whitespace.
	flagged3 = "UPDATE\nworkitems.items SET is_ready = true WHERE id = $1" // want `direct UPDATE on workitems.items.is_ready or pipeline_stage outside encore.app/deps`

	// Positive: bare `items` table (no schema qualifier). The regex
	// accepts both `workitems.items` and the unqualified form because
	// some queries operate inside a SET search_path = workitems
	// connection scope.
	flagged4 = `UPDATE items SET is_ready = true WHERE id = $1` // want `direct UPDATE on workitems.items.is_ready or pipeline_stage outside encore.app/deps`

	// Negative: a clean UPDATE on a different column must NOT fire.
	cleanColumn = `UPDATE workitems.items SET status = 'Done' WHERE id = $1`

	// Negative: a SELECT clause that incidentally returns is_ready
	// must NOT fire. Pins the requirement that the regex anchors on
	// UPDATE, not just any SQL containing "is_ready".
	cleanSelect = `SELECT id, is_ready, pipeline_stage FROM workitems.items WHERE org_id = $1`

	// Negative: a comment string that mentions an UPDATE on
	// items.is_ready in prose must NOT fire. Pins the requirement that
	// the regex demands a real SET keyword between UPDATE and the
	// column — a documentation fragment that names the column without
	// a SET clause is harmless.
	cleanComment = `-- historical note: previous code did update items is_ready directly; now routed via cascade`

	// Negative: an INSERT that lists is_ready in the column list must
	// NOT fire. Pins the requirement that the regex anchors on the
	// UPDATE keyword, not on the column name appearing anywhere.
	cleanInsert = `INSERT INTO workitems.items (id, org_id, is_ready, pipeline_stage) VALUES ($1, $2, false, 'Investigation')`

	// Negative: an identifier that contains "update" as a substring
	// (here as a column name) must NOT fire. Pins the \bupdate\s+
	// word boundary on the UPDATE keyword.
	cleanIdentifier = `SELECT update_items_at, is_ready FROM workitems.items WHERE id = $1`

	// Negative: a DELETE statement on workitems.items must NOT fire,
	// even when an adjacent literal in the same const block mentions
	// is_ready. Pins the requirement that the regex requires UPDATE,
	// not any DML keyword.
	cleanDelete = `DELETE FROM workitems.items WHERE id = $1 AND is_ready = false`

	// Negative: a SELECT on a table whose name ends in "items" but is
	// NOT items itself (e.g. workitems.dependency_items) must NOT
	// fire. Pins the \bitems\b word boundary on the table name.
	cleanSiblingTable = `UPDATE workitems.dependency_items SET created_at = now() WHERE parent_id = $1`
)

// Reference the consts so go vet does not complain about unused
// package-level identifiers in the analysistest run.
var _ = flagged1 + flagged2 + flagged3 + flagged4 +
	cleanColumn + cleanSelect + cleanComment + cleanInsert +
	cleanIdentifier + cleanDelete + cleanSiblingTable
