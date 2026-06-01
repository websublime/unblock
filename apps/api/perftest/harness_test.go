// harness_test.go is the NFR-1 latency harness: it drives the
// warm-cache prime → ready → claim sequence end-to-end against the
// local Encore emulator, computes the p99 of one full sequence, and
// samples runtime.NumGoroutine across the W4 drain window.
//
// SPEC anchor: §11.2 NFR-1 (round-14). See doc.go for the full
// contract.
//
// One measured "sequence" is:
//
//  1. prime(project_id, ready_limit) — the agent dashboard read.
//  2. ready(project_id, limit=1)     — the deterministic next ready
//     item (ordered by priority asc, created_at asc, id asc).
//  3. claim(item_id)                  — atomic claim on that item.
//
// Each claim consumes one ready row permanently (status flips
// Ready→InProgress), so the seed installs N = 2 × MeasurementIterations
// rows and each sequence — warm-up AND measured — claims a fresh one
// via the ready tool's natural ordering. No row is re-used or
// un-claimed (un-claiming would touch the production single-writer
// claim surface).
//
// Warm cache (SPEC §11.2 clauses (a)-(c)):
//
//   - (a) Postgres pool: established by the dedicated apps/api/db/
//     service's init() before TestMain ran (the blank import in
//     main_test.go fired the BindDB chain).
//   - (b) API key validated once before the timer: the single
//     initializeSession call below runs the full Bearer hot path once;
//     subsequent tool calls reuse the resolved session.
//   - (c) no cold-start outliers: WarmupIterations (M ≥ 10) discarded
//     sequences run before the measurement loop.

package perftest_test

import (
	"encoding/json"
	"os"
	"runtime"
	"sort"
	"testing"
	"time"

	"encore.app/perftest"
)

// primeStructuredContent mirrors the prime tool's structuredContent
// shape (the subset the harness reads). Reused verbatim from
// apps/api/exitcriteriontest/prime_ready_claim_close_test.go.
type primeStructuredContent struct {
	ReadySummary struct {
		CountTotal int `json:"count_total"`
	} `json:"ready_summary"`
	ClaimedByMe []struct {
		ID string `json:"id"`
	} `json:"claimed_by_me"`
}

// readyStructuredContent mirrors the ready tool's structuredContent
// shape (the subset the harness reads).
type readyStructuredContent struct {
	Items []struct {
		ID string `json:"id"`
	} `json:"items"`
	TotalReady int `json:"total_ready"`
}

// claimStructuredContent mirrors the claim tool's structuredContent
// shape (the subset the harness reads).
type claimStructuredContent struct {
	Claimed bool `json:"claimed"`
	Item    struct {
		ID     string `json:"id"`
		Status string `json:"status"`
	} `json:"item"`
}

// runSequence executes one prime → ready → claim sequence against the
// shared session and returns the wall-clock duration of the whole
// sequence. Used by both the warm-up loop (return value discarded) and
// the measurement loop. Any tool-level failure t.Fatal-s through the
// transport helpers, so a returned duration always reflects a
// successful sequence.
func runSequence(t *testing.T, f *perftest.Fixture, sessionID string) time.Duration {
	t.Helper()
	start := time.Now()

	// 1) prime.
	primeEnv := callTool(t, f.RawKey, sessionID, "prime", map[string]any{
		"project_id":  f.ProjectID,
		"ready_limit": 10,
	})
	primeRaw := expectSuccess(t, primeEnv)
	var prime primeStructuredContent
	if err := json.Unmarshal(primeRaw, &prime); err != nil {
		t.Fatalf("unmarshal prime: %v; raw=%s", err, string(primeRaw))
	}

	// 2) ready(limit=1) — the deterministic next ready item.
	readyEnv := callTool(t, f.RawKey, sessionID, "ready", map[string]any{
		"project_id": f.ProjectID,
		"limit":      1,
	})
	readyRaw := expectSuccess(t, readyEnv)
	var ready readyStructuredContent
	if err := json.Unmarshal(readyRaw, &ready); err != nil {
		t.Fatalf("unmarshal ready: %v; raw=%s", err, string(readyRaw))
	}
	if len(ready.Items) != 1 {
		t.Fatalf("ready returned %d items, want 1 — ready pool likely exhausted (seeded %d, ensure seed factor covers warm-up + measurement)",
			len(ready.Items), len(f.ReadyItemIDs))
	}
	itemID := ready.Items[0].ID

	// 3) claim the returned item.
	claimEnv := callTool(t, f.RawKey, sessionID, "claim", map[string]any{
		"item_id": itemID,
	})
	claimRaw := expectSuccess(t, claimEnv)
	var claim claimStructuredContent
	if err := json.Unmarshal(claimRaw, &claim); err != nil {
		t.Fatalf("unmarshal claim: %v; raw=%s", err, string(claimRaw))
	}
	if !claim.Claimed {
		t.Fatalf("claim.claimed = false for item %s; struct=%+v", itemID, claim)
	}
	if claim.Item.Status != "InProgress" {
		t.Fatalf("post-claim status = %q, want InProgress (item %s)", claim.Item.Status, itemID)
	}

	return time.Since(start)
}

// percentile returns the value at the p-th percentile of a sorted
// (ascending) slice of durations using the nearest-rank method. p is
// in [0,1]. The slice MUST be sorted ascending. For p=0.99 over 200
// samples the index is ceil(0.99*200)-1 = 197 (0-based) — the
// 198th-smallest of 200, a robust tail estimate. Panics on an empty
// slice (the harness guarantees a non-empty measurement set).
func percentile(sorted []time.Duration, p float64) time.Duration {
	if len(sorted) == 0 {
		panic("perftest: percentile of empty slice")
	}
	if p <= 0 {
		return sorted[0]
	}
	if p >= 1 {
		return sorted[len(sorted)-1]
	}
	// Nearest-rank: rank = ceil(p * N), 1-based; index = rank-1.
	rank := int(float64(len(sorted))*p + 0.999999)
	if rank < 1 {
		rank = 1
	}
	if rank > len(sorted) {
		rank = len(sorted)
	}
	return sorted[rank-1]
}

// latencySampleLine is one JSON-Lines record per measured sequence —
// LatencyMs is the wall-clock duration of one full prime → ready →
// claim sequence, NOT one individual MCP call.
// Emitted via t.Logf so a CI parser can lift the samples (SPEC §11.2:
// "logs per-call latency samples … as JSON-Lines"; the spec's "per-call"
// wording predates the sequence-level measurement and is realised here
// as per-sequence — see doc.go's NFR-1 sample-unit note). record="sample".
type latencySampleLine struct {
	Record    string `json:"record"`
	Iteration int    `json:"iteration"`
	LatencyMs int64  `json:"latency_ms"`
}

// p99SummaryLine is the single JSON-Lines summary record carrying the
// computed p99, min/max, iteration count, and the goroutine deltas.
// record="summary".
type p99SummaryLine struct {
	Record             string `json:"record"`
	Iterations         int    `json:"iterations"`
	WarmupIterations   int    `json:"warmup_iterations"`
	P50Ms              int64  `json:"p50_ms"`
	P90Ms              int64  `json:"p90_ms"`
	P99Ms              int64  `json:"p99_ms"`
	MinMs              int64  `json:"min_ms"`
	MaxMs              int64  `json:"max_ms"`
	BudgetMs           int64  `json:"budget_ms"`
	GoroutineBaseline  int    `json:"goroutine_baseline"`
	GoroutinePeak      int    `json:"goroutine_peak"`
	GoroutineDrained   int    `json:"goroutine_drained"`
	GoroutineDelta     int    `json:"goroutine_delta"` // drained - baseline
	GoroutineMargin    int    `json:"goroutine_margin"`
	GateEnabled        bool   `json:"gate_enabled"`
	P99WithinBudget    bool   `json:"p99_within_budget"`
	GoroutineWithinMrg bool   `json:"goroutine_within_margin"`
}

// TestNFR1_PrimeReadyClaimP99 is the NFR-1 latency harness. It ALWAYS
// logs per-sequence latency samples + the computed p99 + the goroutine
// deltas as JSON-Lines via t.Logf. It hard-fails (t.Fatalf) only when
// UNBLOCK_PERF_GATE=1 AND (p99 ≥ 2 s OR drained-baseline > 20).
func TestNFR1_PrimeReadyClaimP99(t *testing.T) {
	f := fx(t)

	// Warm-cache step (b): validate the API key once before the timer.
	// initializeSession runs the full Bearer hot path; the resolved
	// session is reused across every subsequent tool call.
	sessionID := initializeSession(t, f.RawKey)

	// W4 baseline: sample goroutines BEFORE the warm-up loop, after the
	// session is established (so the SDK's session goroutines are
	// already counted in the baseline and do not inflate the delta).
	baseline := runtime.NumGoroutine()

	// Warm-cache step (c): discard M ≥ 10 warm-up sequences. These pay
	// the first-request cold-start cost so the measured samples reflect
	// steady-state latency.
	for i := 0; i < perftest.WarmupIterations; i++ {
		_ = runSequence(t, f, sessionID)
	}

	// Measurement loop.
	samples := make([]time.Duration, 0, perftest.MeasurementIterations)
	for i := 0; i < perftest.MeasurementIterations; i++ {
		d := runSequence(t, f, sessionID)
		samples = append(samples, d)
		// Per-sequence JSON-Lines sample (always emitted).
		line, _ := json.Marshal(latencySampleLine{
			Record:    "sample",
			Iteration: i,
			LatencyMs: d.Milliseconds(),
		})
		t.Logf("PERFLINE %s", line)
	}

	// W4 peak: sample immediately after the measurement loop, before
	// the drain sleep. This is when the touchLastUsedAt fire-and-forget
	// goroutines are most likely still in flight.
	//
	// NOTE: peak is diagnostic/observability-only — it is recorded in the
	// summary line and the human-readable log for visibility into in-flight
	// goroutine pressure, but it is deliberately NOT asserted. The sole W4
	// leak gate is the post-drain window below (drained - baseline <=
	// GoroutineDrainMargin); a transient peak during fire-and-forget work is
	// expected and is not a leak. Do not mistake the missing peak assertion
	// for a bug.
	peak := runtime.NumGoroutine()

	// W4 drained: sleep 2 s (two cycles of the 1 s touchLastUsedAt
	// context cap) then sample. A clean run drains back toward the
	// baseline; a leak shows a sustained elevation.
	time.Sleep(perftest.GoroutineDrainSleepSeconds * time.Second)
	drained := runtime.NumGoroutine()

	// Compute percentiles over the sorted measurement set.
	sorted := make([]time.Duration, len(samples))
	copy(sorted, samples)
	sort.Slice(sorted, func(i, j int) bool { return sorted[i] < sorted[j] })

	p50 := percentile(sorted, 0.50)
	p90 := percentile(sorted, 0.90)
	p99 := percentile(sorted, 0.99)
	minD := sorted[0]
	maxD := sorted[len(sorted)-1]

	goroutineDelta := drained - baseline
	p99WithinBudget := p99.Milliseconds() < perftest.P99LatencyBudgetMillis
	goroutineWithinMargin := goroutineDelta <= perftest.GoroutineDrainMargin

	gateEnabled := os.Getenv(perftest.PerfGateEnv) == "1"

	// Single JSON-Lines summary record (always emitted).
	summary, _ := json.Marshal(p99SummaryLine{
		Record:             "summary",
		Iterations:         len(samples),
		WarmupIterations:   perftest.WarmupIterations,
		P50Ms:              p50.Milliseconds(),
		P90Ms:              p90.Milliseconds(),
		P99Ms:              p99.Milliseconds(),
		MinMs:              minD.Milliseconds(),
		MaxMs:              maxD.Milliseconds(),
		BudgetMs:           perftest.P99LatencyBudgetMillis,
		GoroutineBaseline:  baseline,
		GoroutinePeak:      peak,
		GoroutineDrained:   drained,
		GoroutineDelta:     goroutineDelta,
		GoroutineMargin:    perftest.GoroutineDrainMargin,
		GateEnabled:        gateEnabled,
		P99WithinBudget:    p99WithinBudget,
		GoroutineWithinMrg: goroutineWithinMargin,
	})
	t.Logf("PERFLINE %s", summary)

	// Human-readable echo (not parsed; convenience on a failing run).
	t.Logf("NFR-1: p50=%s p90=%s p99=%s (budget %dms) | goroutines baseline=%d peak=%d drained=%d delta=%d (margin %d) | gate=%v",
		p50, p90, p99, perftest.P99LatencyBudgetMillis,
		baseline, peak, drained, goroutineDelta, perftest.GoroutineDrainMargin, gateEnabled)

	// Gate semantics (SPEC §11.2): hard-fail only under
	// UNBLOCK_PERF_GATE=1. Default run is advisory (log-only).
	if !gateEnabled {
		return
	}
	if !p99WithinBudget {
		t.Fatalf("NFR-1 GATE: p99 = %dms ≥ budget %dms (UNBLOCK_PERF_GATE=1)",
			p99.Milliseconds(), perftest.P99LatencyBudgetMillis)
	}
	if !goroutineWithinMargin {
		t.Fatalf("NFR-1 GATE: goroutine drain delta = %d > margin %d (baseline=%d drained=%d; touchLastUsedAt pile-up?) (UNBLOCK_PERF_GATE=1)",
			goroutineDelta, perftest.GoroutineDrainMargin, baseline, drained)
	}
}
