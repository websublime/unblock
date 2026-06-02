// handler_set_state.go owns the §6.2 Tool 13 (`set_state`) handler —
// the dedicated mutator for the four state-machine columns
// (impl_state, review_state, qa_state, pipeline_state) plus an
// optional `intent_comment` written best-effort (non-atomic,
// post-commit) alongside the state change.
//
// # State invariants
//
// All five PRD §6.2 state-machine invariants I-1..I-5 are enforced
// inside workitems.SetStateColumns inside one SQL round-trip per the
// CTE shape documented at SPEC §6.2 Tool 13 (lines 1659-1689):
//
//   - I-1: review_state=needs_rework auto-resets qa_state=pending in
//     the same UPDATE (no rejection; auto-applied).
//   - I-2: qa_state=failed requires review_state=approved
//     (data.invariant=qa_failed_requires_review_approved).
//   - I-3: post-failure rework reset lives in workitems.Claim, NOT
//     here (documented in workitems.go for cross-reference).
//   - I-4: review_state change requires impl_state=done
//     (data.invariant=review_change_requires_impl_done).
//   - I-5: impl_state=done → pending requires the rework path
//     (data.invariant=impl_done_to_pending_requires_rework_path).
//
// The structural pre-check impl_done_requires_claim
// (data.invariant=impl_done_requires_claim) also surfaces from the
// same RPC. Each fires as Encore errs.FailedPrecondition with
// Meta["invariant"] populated — errmap.go (post bead unblock-tv8.21
// amendment) projects Meta["invariant"] into the §7 envelope as both
// `data.invariant` (machine-targeted, per spec §6.2 line 1645) and
// `data.rejection_reason` (legacy mirror).
//
// Layer-1 BLOCK conditions (comment-trail-driven preconditions such
// as `qa_state → passed` requires a (kind=qa, status=success)
// comment) ship in P02 per Plan §3.4 — NOT enforced here.
//
// # intent_comment best-effort non-atomicity + warnings signal
//
// SPEC §6.2 Tool 13 + §4.4 (back-folded by unblock-tv8.63) document
// the intent_comment write as best-effort and NON-atomic: Encore's RPC
// boundary prevents a single Postgres transaction spanning both
// workitems.SetStateColumns and workitems.AppendComment (orchestrator
// DECISION 2026-05-18 on bead unblock-tv8.21 — cross-RPC transactions
// are out of P01 architectural scope). SetStateColumns commits first;
// AppendComment follows on success and CANNOT roll it back.
//
// If AppendComment fails after the state mutation committed, the tool
// STILL returns SUCCESS (the state was genuinely mutated) and surfaces
// the dropped comment two non-error ways per the §7.1 success-side
// warnings contract (activated by unblock-tv8.63):
//
//   - caller-visible: structuredContent.warnings[] carries exactly one
//     {code:intent_comment_dropped, message, details:{kind,status}}
//     entry — the body is never echoed (only length + sha256 reach the
//     rlog diagnostic below).
//   - operator-visible: the rlog.Error diagnostic plus the additive
//     mcp.tool_calls.warning_codes audit column (§8.1.1) record
//     ["intent_comment_dropped"]. result_kind STAYS 'ok' on this path —
//     the call succeeded; the audit widening is the warning column, NOT
//     a new result_kind value.
//
// # intent_comment validation
//
// Validation runs at the MCP boundary BEFORE SetStateColumns is
// invoked, so a malformed intent_comment surfaces as §7 VALIDATION
// without leaving a stale state-only mutation behind:
//
//   - kind ∈ SPEC §6.5 allow-list (mirrors comments_kind_chk DDL).
//   - status ∈ SPEC §6.5 allow-list (mirrors comments_status_chk).
//   - body 1..16384 chars.
//
// Each rejection returns InvalidArgument with `details["field"]`
// identifying the offending sub-field (e.g. `intent_comment.kind`,
// `intent_comment.status`, `intent_comment.body`). The downstream
// workitems.AppendComment RPC + DDL CHECK constraints re-enforce
// kind/status independently — boundary enforcement is the wire-UX
// improvement (post-review DECISION 2026-05-18, S2).
//
// # Cascade publication
//
// The §6.3.0 `state_change` cascade publish (post-commit publish to
// deps.CascadeRequestedTopic when (impl, review, qa) materially
// change) lives INSIDE workitems.SetStateColumns (shipped on bead
// unblock-tv8.53). This MCP handler is unaware of the publish — it
// calls SetStateColumns and trusts the RPC to fire the cascade per
// SPEC §6.3.0 tension #3. The handler still appends the
// intent_comment AFTER SetStateColumns returns; if the comment append
// fails the cascade has already fired (best-effort post-state per
// bead unblock-tv8.21 D-6 INVESTIGATION risk R3 — the existing
// architectural decision is unchanged).
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 6.2 Tool 13 (lines
// 1606-1725) + § 4.4 (workitems.SetStateColumns + AppendComment) +
// § 6.5 (Comment kind + status enums) + § 7 (error envelope).

package mcp

import (
	"context"
	"crypto/sha256"
	"encoding/hex"

	"encore.app/workitems"
	"encore.dev/beta/errs"
	"encore.dev/rlog"
	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

// setStateIntentComment is the optional `intent_comment` block per
// SPEC §6.2 Tool 13 lines 1616-1620. All three fields are required
// when the block is supplied — the workitems.AppendComment RPC will
// reject an empty body / missing kind on its own, but the MCP
// boundary surfaces a clearer §7 VALIDATION envelope.
type setStateIntentComment struct {
	Kind   string `json:"kind"`
	Status string `json:"status"`
	Body   string `json:"body"`
}

// setStateIn mirrors SPEC §6.2 Tool 13 lines 1609-1621. Every state
// field is a *string so the handler can faithfully distinguish
// "unchanged" (nil pointer) from "explicit value" (non-nil), matching
// workitems.SetStateRequest's pointer-nil-is-unchanged convention.
//
// IntentComment is *setStateIntentComment so absence (nil) skips the
// comment write entirely; an explicit block triggers the post-state
// AppendComment call.
type setStateIn struct {
	ItemID        string                 `json:"item_id"`
	ImplState     *string                `json:"impl_state,omitempty"`
	ReviewState   *string                `json:"review_state,omitempty"`
	QAState       *string                `json:"qa_state,omitempty"`
	PipelineState *string                `json:"pipeline_state,omitempty"`
	IntentComment *setStateIntentComment `json:"intent_comment,omitempty"`
}

type setStateOut struct {
	Item primeItem `json:"item"`
	// WithWarnings embeds the §7.1 success-side warnings array
	// (`warnings`, omitempty). set_state is the only wired warning
	// producer in P01/P02: on the intent_comment partial-failure path
	// it carries exactly one {code:intent_comment_dropped, ...} entry;
	// on every other (success) path the embedded slice is nil and the
	// `warnings` key is omitted from structuredContent entirely. The
	// embedded field is promoted by jsonschema-go into the inferred
	// output schema as a sibling of `item`. SPEC §7.1.
	WithWarnings
}

// appendIntentComment is a package-level seam over the best-effort
// post-commit workitems.AppendComment call. Production binds it to the
// real RPC (initialised below); the integration test for the
// §6.2 Tool 13 intent_comment partial-failure path overrides it via
// setAppendIntentCommentForTest to force a post-commit failure.
//
// The seam exists because there is NO black-box input that makes
// AppendComment fail AFTER SetStateColumns has committed: malformed
// intent_comment fields are caught by validateIntentComment at the MCP
// boundary BEFORE SetStateColumns runs, and AppendComment's own
// failure modes (FK-violation→NotFound, generic Internal) cannot fire
// for an item that SetStateColumns just locked + updated and proved to
// exist. Exercising the §7.1 warnings + §8.1.1 warning_codes
// dropped-path (AC#3) therefore requires a test double on exactly this
// call — see the unblock-tv8.63 INVESTIGATION risk R1. The seam wraps
// the production RPC verbatim, so the production path is unchanged.
//
//nolint:gochecknoglobals // test seam over a cross-RPC call by design.
var appendIntentComment = func(ctx context.Context, req *workitems.AppendCommentRequest) error {
	_, err := workitems.AppendComment(ctx, req)
	return err
}

// setStateCommentBodyMax mirrors handler_comment.go's commentBodyMax —
// kept as a separate const so a future divergence (e.g. spec amends
// Tool 13's intent_comment cap) is a single-line edit here without
// affecting the plain `comment` tool.
const setStateCommentBodyMax = 16384

// intentCommentAllowedKinds mirrors SPEC §6.5 / migration
// 0040_workitems.up.sql comments_kind_chk: the canonical kind enum the
// DDL accepts for workitems.comments.kind. Enforced at the MCP
// boundary (post-review DECISION 2026-05-18, S2) so an invalid kind
// inside intent_comment surfaces as §7 VALIDATION
// (details.field="intent_comment.kind") BEFORE the state mutation
// commits — preventing a stale state-only mutation behind a malformed
// comment retry.
var intentCommentAllowedKinds = map[string]struct{}{
	"investigation": {},
	"decision":      {},
	"deviation":     {},
	"completed":     {},
	"review":        {},
	"qa":            {},
	"deferred":      {},
	"pr":            {},
	"needs-human":   {},
	"override":      {},
	"general":       {},
}

// intentCommentAllowedStatuses mirrors SPEC §6.5 / migration
// 0040_workitems.up.sql comments_status_chk. Same boundary-validation
// rationale as intentCommentAllowedKinds above.
var intentCommentAllowedStatuses = map[string]struct{}{
	"error":   {},
	"warning": {},
	"info":    {},
	"success": {},
}

// registerHandleSetState is invoked by transport.go's init — see the
// toolRegistrars rationale there.
func registerHandleSetState(s *sdkmcp.Server) {
	sdkmcp.AddTool(s, &sdkmcp.Tool{
		Name: "set_state",
		Description: "Set one or more of (impl_state, review_state, " +
			"qa_state, pipeline_state) atomically. Enforces the five " +
			"PRD §6.2 state-machine invariants I-1..I-5 (data.invariant " +
			"populated on PRECONDITION_NOT_MET). Optionally writes an " +
			"intent_comment alongside the state change. SPEC § 6.2 Tool 13.",
	}, handleSetState)
}

func handleSetState(ctx context.Context, req *sdkmcp.CallToolRequest, in setStateIn) (*sdkmcp.CallToolResult, setStateOut, error) {
	tool := "set_state"
	state := bindTool(req, tool)

	identity, ok := identityFromReq(req)
	if !ok {
		return nil, setStateOut{}, mapError(state, tool, errMissingIdentityErr())
	}

	mcpCtx, err := withIdentityFromReq(ctx, req)
	if err != nil {
		return nil, setStateOut{}, mapError(state, tool, err)
	}

	if in.ItemID == "" {
		return nil, setStateOut{}, mapError(state, tool, &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "missing item_id",
			Meta:    errs.Metadata{"field": "item_id"},
		})
	}
	if state != nil && state.Call != nil {
		state.Call.ItemID = in.ItemID
	}

	// Validate the intent_comment block BEFORE invoking SetStateColumns
	// so a malformed comment never leaves a state-only mutation behind.
	// The workitems.AppendComment RPC would re-reject the same input
	// later, but the §7 VALIDATION envelope sourced from there would
	// arrive after the state write committed — defeating the
	// "best-effort atomic" contract documented above.
	if in.IntentComment != nil {
		if err := validateIntentComment(in.IntentComment); err != nil {
			return nil, setStateOut{}, mapError(state, tool, err)
		}
	}

	item, err := workitems.SetStateColumns(mcpCtx, &workitems.SetStateRequest{
		ItemID:        in.ItemID,
		ImplState:     in.ImplState,
		ReviewState:   in.ReviewState,
		QAState:       in.QAState,
		PipelineState: in.PipelineState,
	})
	if err != nil {
		return nil, setStateOut{}, mapError(state, tool, err)
	}

	out := setStateOut{Item: itemToPrime(*item)}

	// State write committed. Now best-effort append the intent_comment;
	// any failure here is logged but does NOT roll back the state
	// mutation (orchestrator DECISION 2026-05-18 on bead
	// unblock-tv8.21: cross-RPC Postgres transactions are out of
	// architectural scope for P01). On failure the tool STILL returns
	// SUCCESS — the state was genuinely mutated — and surfaces the
	// dropped comment two non-error ways per SPEC §6.2 / §7.1 / §8.1.1:
	// a structuredContent.warnings[] entry for the caller and a
	// warning_codes audit entry for the operator. result_kind STAYS ok.
	if in.IntentComment != nil {
		commentErr := appendIntentComment(mcpCtx, &workitems.AppendCommentRequest{
			ItemID:      in.ItemID,
			AuthorID:    identity.UserID,
			AuthorAgent: identity.AgentKind,
			Kind:        in.IntentComment.Kind,
			Status:      in.IntentComment.Status,
			Body:        in.IntentComment.Body,
		})
		if commentErr != nil {
			// Payload hash (not the body itself) keeps the rlog line
			// debuggable without leaking comment text into the
			// observability surface.
			bodyHash := sha256.Sum256([]byte(in.IntentComment.Body))
			rlog.Error("mcp: set_state intent_comment append failed; state mutation already committed",
				"err", commentErr,
				"item_id", in.ItemID,
				"intent_comment_kind", in.IntentComment.Kind,
				"intent_comment_status", in.IntentComment.Status,
				"intent_comment_body_sha256", hex.EncodeToString(bodyHash[:]),
				"intent_comment_body_len", len(in.IntentComment.Body),
			)

			// §7.1 caller-visible signal: exactly one warning entry.
			// `details` echoes kind/status only — never the body
			// (SPEC §6.2: body length + sha256 stay in rlog above).
			out.Warnings = append(out.Warnings, Warning{
				Code:    warningCodeIntentCommentDropped,
				Message: "state mutation committed; intent_comment append failed and was dropped",
				Details: map[string]any{
					"intent_comment_kind":   in.IntentComment.Kind,
					"intent_comment_status": in.IntentComment.Status,
				},
			})
			// §8.1.1 operator-visible signal: record the code on the
			// audit row's warning_codes column. result_kind STAYS ok
			// below — the call succeeded.
			if state != nil && state.Call != nil {
				state.Call.WarningCodes = append(state.Call.WarningCodes, warningCodeIntentCommentDropped)
			}
		}
	}

	if state != nil && state.Call != nil {
		state.Call.ResultKind = ResultOK
		state.Call.ProjectID = item.ProjectID
	}
	return nil, out, nil
}

// validateIntentComment enforces the MCP-boundary validation contract
// for the optional intent_comment block:
//
//   - kind ∈ SPEC §6.5 allow-list (matches workitems.comments
//     comments_kind_chk CHECK in migration 0040_workitems.up.sql).
//   - status ∈ SPEC §6.5 allow-list (matches comments_status_chk).
//   - body length ∈ 1..16384 chars.
//
// Validation runs BEFORE workitems.SetStateColumns is invoked so a
// malformed intent_comment never leaves a stale state-only mutation
// behind. The downstream workitems.AppendComment RPC + DDL CHECK
// constraints re-enforce kind/status independently; surfacing
// VALIDATION at the boundary with `data.field` identifying the
// offending sub-field is the meaningful UX win (post-review DECISION
// 2026-05-18, S2).
func validateIntentComment(c *setStateIntentComment) error {
	if c.Kind == "" {
		return &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "intent_comment.kind is required",
			Meta:    errs.Metadata{"field": "intent_comment.kind"},
		}
	}
	if _, ok := intentCommentAllowedKinds[c.Kind]; !ok {
		return &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "intent_comment.kind not in allowed enum (SPEC §6.5)",
			Meta:    errs.Metadata{"field": "intent_comment.kind"},
		}
	}
	if c.Status == "" {
		return &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "intent_comment.status is required",
			Meta:    errs.Metadata{"field": "intent_comment.status"},
		}
	}
	if _, ok := intentCommentAllowedStatuses[c.Status]; !ok {
		return &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "intent_comment.status not in allowed enum (SPEC §6.5)",
			Meta:    errs.Metadata{"field": "intent_comment.status"},
		}
	}
	if c.Body == "" {
		return &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "intent_comment.body must be non-empty",
			Meta:    errs.Metadata{"field": "intent_comment.body"},
		}
	}
	if len(c.Body) > setStateCommentBodyMax {
		return &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "intent_comment.body exceeds 16384 chars",
			Meta:    errs.Metadata{"field": "intent_comment.body"},
		}
	}
	return nil
}
