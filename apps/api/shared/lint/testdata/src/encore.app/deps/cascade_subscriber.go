// Fixture for analysistest: the spec-allow-listed cascade subscriber
// package. Every targeted UPDATE must pass without diagnostic. No
// `want` comments here — analysistest treats their absence as
// "expect zero diagnostics".
package deps

const (
	_ = `UPDATE workitems.items SET is_ready = true WHERE id = $1`
	_ = `UPDATE workitems.items SET pipeline_stage = 'Done' WHERE id = $1`
)
