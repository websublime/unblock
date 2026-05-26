// fixture.go pins the canonical §11.1.0 exit-criterion fixture as Go
// constants and struct types. The fixture is the 5-item dependency
// graph the §11.1.2 functional assertions and §11.3 architectural
// invariant property tests run against.
//
// SPEC §11.1.0 topology (verbatim):
//
//   | Item   | Status   | impl_state | review_state | qa_state | is_ready | closed_at |
//   | itm_a  | Done     | done       | approved     | passed   | —        | now       |
//   | itm_b  | Ready    | (default)  | (default)    | (default)| true     | —         |
//   | itm_c  | (default)| (default)  | (default)    | (default)| (default)| —         |
//   | itm_d  | (default)| (default)  | (default)    | (default)| (default)| —         |
//   | itm_e  | Ready    | (default)  | (default)    | (default)| true     | —         |
//
// Edges (all kind='blocks'):
//
//   - itm_a → itm_b
//   - itm_b → itm_c
//   - itm_b → itm_d
//   - itm_d → itm_e
//
// The cycle-attempt edge itm_e → itm_a is NOT seeded — it is what the
// §11.1.2 cycle assertion attempts to add via the `add_dependency`
// tool and expects to be rejected.
//
// Identifier note (SPEC §11.1.0 last paragraph): the displayed ids
// `itm_a..itm_e`, `prj_exit`, `org_exit_criterion`, `usr_alice` are
// illustrative labels — the actual seed mints fresh ULIDs at runtime
// via `apps/api/shared/ulid`. The `Fixture` struct below holds the
// label → ULID map so test bodies can refer to items by label and
// translate to the persisted id at assertion time.

package exitcriteriontest

// Item labels — the canonical names the test bodies use. Match SPEC
// §11.1.0 verbatim. Each value resolves to a freshly-minted ULID at
// seed time; the map lives on the Fixture struct below.
const (
	LabelItemA = "itm_a"
	LabelItemB = "itm_b"
	LabelItemC = "itm_c"
	LabelItemD = "itm_d"
	LabelItemE = "itm_e"
)

// allItemLabels is the ordered list the seed iterates to insert
// workitems.items rows. Order matters because of the FK chain (items
// must exist before dependencies reference them) and because the
// per-row state in itemSpec() below is keyed by label.
var allItemLabels = []string{
	LabelItemA, LabelItemB, LabelItemC, LabelItemD, LabelItemE,
}

// itemSpec returns the SPEC §11.1.0 verbatim per-item state for the
// given label. The seed reads this once per row at INSERT time;
// changing the topology requires editing both this function and the
// edgeSpecs slice below in lockstep.
//
// The (status, impl_state, review_state, qa_state, is_ready,
// closedAtNow) tuple maps to the columns on workitems.items. The
// items_claim_status_chk constraint is satisfied vacuously for itm_a
// (claimed_by_id NULL ∧ claimed_at NULL — first leg of the OR) and
// for the four non-closed rows (same first leg). The
// items_finding_required_fields_chk is vacuous for every row because
// type='task' is the closed alternative.
func itemSpec(label string) (status, impl, review, qa string, isReady, closedAtNow bool) {
	switch label {
	case LabelItemA:
		// Done end-state: §11.1.0 row 1.
		return "Done", "done", "approved", "passed", false, true
	case LabelItemB:
		// Ready, no upstream blockers yet because itm_a→itm_b is
		// inserted AFTER itm_b (the edges loop runs after the items
		// loop). is_ready=true is the SPEC §11.1.0 row 2 verbatim
		// value; once itm_a is Done at seed time, the §6.5 derivation
		// would also yield true, so the explicit write is consistent
		// with the post-seed derived state.
		return "Ready", "pending", "pending", "pending", true, false
	case LabelItemC, LabelItemD:
		// Default-everything blocked items. status defaults to
		// 'Backlog' per the items_status_chk default; the seed writes
		// 'Backlog' explicitly so the row's wire shape is unambiguous
		// even if the schema default ever changes.
		return "Backlog", "pending", "pending", "pending", false, false
	case LabelItemE:
		// Cycle-attempt target. SPEC §11.1.0 row 5: status=Ready,
		// is_ready=true. The seed inserts itm_d→itm_e AFTER this
		// row, and per §11.1.0 final paragraph the displayed
		// is_ready=true reflects the seed-time write — derivation
		// would yield false once the d→e edge lands. The fixture is
		// "ready as of seed time"; tests that mutate the graph
		// re-derive is_ready through the production code paths
		// (Regime A inline recompute) and the assertion bodies are
		// written against the post-mutation derived state, not the
		// seed-time literal.
		return "Ready", "pending", "pending", "pending", true, false
	default:
		// Defensive — itemSpec is called only with allItemLabels values.
		// A panic here surfaces a typo or topology drift at the
		// earliest possible moment.
		panic("exitcriteriontest: unknown item label: " + label)
	}
}

// edgeSpec captures one row of deps.dependencies. The seed inserts
// every edge in order with kind='blocks' (SPEC §11.1.0 line 2512).
type edgeSpec struct {
	From string // label
	To   string // label
}

// edgeSpecs is the §11.1.0 verbatim edge set. Order does not matter
// for correctness (each INSERT is independent) but is preserved as a
// readability anchor.
var edgeSpecs = []edgeSpec{
	{From: LabelItemA, To: LabelItemB},
	{From: LabelItemB, To: LabelItemC},
	{From: LabelItemB, To: LabelItemD},
	{From: LabelItemD, To: LabelItemE},
}

// Fixture is the materialised seed graph. Held in memory between
// TestMain seed-in and the suite so individual test bodies can
// reference labels without re-querying the DB.
//
// The Items map is keyed by the LabelItem* constants and holds the
// freshly-minted ULID for each. RawKey is the in-memory API key
// string the test uses as the Bearer token in MCP assertions — it
// is never persisted (only the HMAC digest lives in mcp.api_keys).
type Fixture struct {
	// OrgID is the persisted ULID for the canonical
	// org_exit_criterion row.
	OrgID string

	// ProjectID is the persisted ULID for the prj_exit row.
	ProjectID string

	// UserID is the persisted ULID for the usr_alice row.
	UserID string

	// APIKeyID is the persisted ULID for the alice-claude-code
	// mcp.api_keys row.
	APIKeyID string

	// RawKey is the freshly-minted raw API key string in production
	// format (`unblock_pat_` + 52-char lowercase base32). Held in
	// memory only — never written to disk. Used as the Bearer token
	// in MCP-tool dispatch assertions.
	RawKey string

	// Items maps the LabelItem* constants to the freshly-minted
	// workitems.items ULID for each row.
	Items map[string]string
}

// ItemID returns the persisted ULID for the given label, panicking
// if the label is unknown. Used by test bodies to translate from the
// readable label form to the wire id form.
func (f *Fixture) ItemID(label string) string {
	id, ok := f.Items[label]
	if !ok {
		panic("exitcriteriontest: unknown item label in Fixture.ItemID: " + label)
	}
	return id
}
