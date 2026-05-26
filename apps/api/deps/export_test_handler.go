// export_test_handler.go exposes the cascade subscriber's package-
// private handleCascadeRequested under an exported name so external
// test packages (notably apps/api/exitcriteriontest/) can drive the
// subscriber directly under `encore test`.
//
// Why an exported symbol on the production import path:
//
//   - Encore Pub/Sub subscriptions DO NOT fire under `encore test`
//     (documented at encore.dev/docs/go/primitives/pubsub#testing-pubsub
//     and recorded verbatim in cascade_subscriber_handler_test.go's
//     header). The test harness records published messages on
//     `et.Topic(...).PublishedMessages()` but never delivers them to
//     the subscriber goroutine, so a publish from `workitems.Close`,
//     `deps.AddEdge`, `deps.RemoveEdge`, `workitems.SetStateColumns`,
//     or `workitems.Claim` (I-3 reset path) never reaches
//     `handleCascadeRequested` and no `deps.cascade_events` row
//     materialises.
//
//   - SPEC §11.1.2 and §11.3 require row-level assertions on
//     `deps.cascade_events` for the four cascade kinds (`close`,
//     `edge_added`, `edge_removed`, `state_change`) plus the
//     idempotency invariant ("byte-identical post-state, exactly one
//     row per (event_id, triggered_by_item_id) under N=100
//     re-deliveries"). Without an exported test hook the assertions
//     are unreachable for the three kinds the subscriber alone writes
//     (`edge_removed` has an inline INSERT in `deps.RemoveEdge` and
//     would in principle be testable, but the unified contract is
//     "drive the subscriber for every kind").
//
//   - The wrapper is a thin pass-through to `handleCascadeRequested`
//     — same body, same idempotency clause, same BFS depth cap, same
//     pipeline_stage update pass. There is no behavioural divergence
//     from production. Tests exercising it exercise the real
//     subscriber.
//
//   - The `ForTest` suffix is the audit trail. Same convention as
//     `mcp.ServeMCPForTest` (`apps/api/mcp/export_test_writer.go:49-65`)
//     and `mcp.WriteToolCallForTest` (same file, lines 41-47).
//     Production callers MUST NOT invoke this — the package-private
//     `handleCascadeRequested` is the production entrypoint, driven by
//     Encore's pubsub.NewSubscription wiring in
//     `cascade_subscriber.go`.
//
//   - The wrapper is a plain Go function, NOT an `//encore:api`.
//     Encore's public RPC catalogue is unaffected and no new HTTP
//     route is registered.
//
// SPEC anchor: round-13 changelog (line 15) and §11.1.1 paragraph
// "Cascade subscriber test invocation". See also the four-step
// invocation pattern codified there:
//
//  1. Invoke the producing RPC through the normal MCP / private-mesh path.
//  2. Capture `et.Topic(deps.CascadeRequestedTopic).PublishedMessages()`.
//  3. For each captured message, invoke `HandleCascadeRequestedForTest`
//     exactly once to materialise the audit row(s) and apply the
//     pipeline_stage updates.
//  4. Assert the row(s) per §11.1.2.
//
// Idempotency assertions (§11.3 re-delivery property test) re-invoke
// this wrapper with the same `event_id` and assert the
// `ON CONFLICT (event_id, triggered_by_item_id) DO NOTHING` clause
// collapses the second insert to no-op.

package deps

import "context"

// HandleCascadeRequestedForTest is the integration-test-only re-export
// of handleCascadeRequested. See the file-level doc-comment for the
// rationale and the four-step invocation pattern. Production code MUST
// NOT call this — the production entrypoint is the cascade subscriber
// wired in cascade_subscriber.go via pubsub.NewSubscription.
func HandleCascadeRequestedForTest(ctx context.Context, msg *CascadeRequested) error {
	return handleCascadeRequested(ctx, msg)
}
