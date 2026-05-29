// fixture.go pins the perftest harness's measurement parameters and
// the materialised seed fixture as Go constants and struct types. The
// fixture is intentionally minimal — an org + user + project + api_key
// + a pool of ready items — because the prime → ready → claim hot path
// does not need the canonical §11.1.0 five-item dependency graph
// (there are no edges to traverse on this path).
//
// SPEC anchors: §11.2 NFR-1 (round-14).

package perftest

// MeasurementIterations is the number of measured prime → ready →
// claim sequences. The spec wants a stable p99; ≥ 100 iterations is
// the floor for a meaningful 99th-percentile estimate (the p99 index
// is iterations-1 of the sorted samples; with 100 samples that is the
// single slowest of the 100, with 200 it is the 2nd-slowest, giving a
// more robust tail estimate). We use 200 to balance tail stability
// against total suite wall-clock (each iteration is three serial MCP
// round-trips).
const MeasurementIterations = 200

// WarmupIterations is the number of discarded warm-up sequences run
// BEFORE measurement begins (SPEC §11.2 warm-cache clause (c): M ≥ 10
// warm-up iterations discarded). These pay the first-request
// cold-start cost (SDK Connect, first-call Bearer hot-path warm-up,
// connection-pool ramp) so the measured samples reflect steady-state
// latency only.
const WarmupIterations = 10

// readyItemSeedFactor is the multiplier on MeasurementIterations used
// to size the seeded ready-item pool. Each claim (warm-up AND
// measured) consumes one ready row permanently, so the pool must cover
// WarmupIterations + MeasurementIterations with headroom. A factor of
// 2 over MeasurementIterations gives 2 × 200 = 400 rows, comfortably
// above the 210 consumed — the headroom absorbs the `ready` tool's
// limit semantics and any retry. SPEC §11.2: "Seed N = 2 × iterations
// ready items".
const readyItemSeedFactor = 2

// readyItemCount is the total number of ready rows the seed installs.
const readyItemCount = MeasurementIterations * readyItemSeedFactor

// GoroutineDrainMargin is the absolute tolerance for the W4 drain
// check: assertion is `drained - baseline ≤ GoroutineDrainMargin`
// (SPEC §11.2 W4 closure, round-14). 20 is the margin for runtime/SDK
// overhead — NOT a ratio of iterations (DRIFT-C resolution rejected
// the 1.5×iterations heuristic as never-tripping). Exported so the
// harness in the external `perftest_test` package can read it.
const GoroutineDrainMargin = 20

// GoroutineDrainSleepSeconds is the post-loop sleep (in seconds)
// before the `drained` sample. The touchLastUsedAt goroutine has a 1 s
// context cap (apps/api/auth/auth.go:249); a 2 s sleep gives that cap
// two cycles to expire so a clean run is distinguishable from a slow
// leak (R2). Converted to time.Duration at the harness call site.
const GoroutineDrainSleepSeconds = 2

// P99LatencyBudgetMillis is the NFR-1 latency ceiling in milliseconds:
// p99 < 2 s. Expressed as an integer-millisecond budget so the gate
// comparison is exact (SPEC §11.2 NFR-1).
const P99LatencyBudgetMillis = 2000

// PerfGateEnv is the environment variable that flips the harness from
// advisory (log-only) to hard-fail. When UNBLOCK_PERF_GATE=1 the
// harness t.Fatalf-s on a budget breach; otherwise it only logs (SPEC
// §11.2 gate semantics).
const PerfGateEnv = "UNBLOCK_PERF_GATE"

// Seed item parameters. Every seeded ready row is status='Ready',
// is_ready=true, closed_at=NULL, priority='P2' — the exact predicate
// the items_ready_partial_idx covers
// (apps/api/db/migrations/0040_workitems.up.sql:144-146), so the
// `ready` MCP tool's hot path uses the index. priority='P2' is an
// arbitrary mid-range valid value (items_priority_chk allows P0..P4);
// a uniform priority keeps the (priority asc, created_at asc, id asc)
// ordering deterministic and index-driven.
const (
	seedItemStatus   = "Ready"
	seedItemPriority = "P2"
	seedItemType     = "task"
)

// Fixture is the materialised seed installed by SeedFixture. Held in
// memory between TestMain seed-in and the suite. The harness reads
// RawKey as the Bearer token and ProjectID to scope the prime/ready
// tool calls; ReadyItemIDs is retained for diagnostics and teardown
// reasoning (teardown cascades via OrgID, so the slice is not strictly
// needed for cleanup, but it documents how many rows were seeded).
type Fixture struct {
	// OrgID is the persisted ULID for the harness org row.
	OrgID string

	// ProjectID is the persisted ULID for the harness project row.
	ProjectID string

	// UserID is the persisted ULID for the harness user row.
	UserID string

	// APIKeyID is the persisted ULID for the harness mcp.api_keys row.
	APIKeyID string

	// RawKey is the freshly-minted raw API key string in production
	// format (`unblock_pat_` + 52-char lowercase base32). Held in
	// memory only — never written to disk. Used as the Bearer token in
	// the prime → ready → claim measurement loop.
	RawKey string

	// ReadyItemIDs holds the persisted ULIDs of every seeded ready
	// item, in insertion order. Length == readyItemCount.
	ReadyItemIDs []string
}
