#!/bin/sh
# d57-config-layers-claims.sh — the REQUIRED-LANDING gate for D57, FR-13 FULL: six config layers under
# one order, the user and DB layers limited to the three layer keys (`jsonl_export` excluded until
# `ub-p1h`) with a typed refusal of a startup key, the `issue` tool's `config` action as the DB
# layer's writer, live reload of the runtime keys and schema step 3 (PRD §4 D57, tracked as
# `ub-lp9.2`). Spec: docs/plans/ci-cd-and-distribution.md §2.1, the paragraph beginning "Named
# sub-check (its D57 sibling…)". Runs as a step of the required `doc-lint` job, immediately after its
# `d56` sibling.
#
# THAT PARAGRAPH IS NORMATIVE OVER THIS FILE. Every landing enforced here is named there; a rule that
# exists in one and not the other is a defect to be fixed in the SAME change. Do not "tidy" a row away
# as unspecified — read the spec paragraph first.
#
# WHY THIS ONE IS POSITIVE-ONLY. Every behaviour D57 adds is a cell: the storage contract cells
# (`contract_config_…`), the NFR-19 seed-stamp-2 cell and the per-branch lying-stamp cells, the config
# cells (the six-layer precedence suite with its golden resolution and the user_config/db_layer/
# reload unit cells), the `config_…` engine and MCP cells, the duplicate-key corpus cell for the
# `config` arm, the `config_toml` fuzz target and the `capabilities`/`schema_bundle` goldens with
# `CONTRACT_HASH`. What no test carries is the mechanism's own survival, the documentation edits this
# decision makes across the tree, and the published artefacts outside every lint corpus
# (`README.md`'s tool table and `docs/roadmap.html`). A negative prose sweep is dodged by a claim
# REWRAPPED across two lines, so every row is a positive landing. The ONE NEGATIVE row (N1, no
# non-comment line of any `Cargo.toml` names the `directories` crate) is a dependency rule, not a
# prose sweep: D57 clause (3) resolves the user file on EVERY operating system as
# `$XDG_CONFIG_HOME/unblock/config.toml` or `<home>/.config/unblock/config.toml`, and the platform
# directories `directories` resolves (`~/Library/Application Support`, `%APPDATA%`) are rejected.
#
# WHAT NO ROW HERE CAN SEE, stated so no row is over-read. A line grep cannot prove that `config_set`
# validates and reloads BEFORE it commits, that a bad DB row fail-closes every gated method while
# `open`, `config unset` and `shutdown` stay reachable, that a `config_unset` with no row swaps the
# snapshot (the cross-process repair), that `init` checks `is_blocking()` after its open, that the
# hint dispatches on `gaps` rather than `from == to`, that the commit and the swap cannot be separated
# by cancellation, that the swapped snapshot reaches the live `Arc<Session>`, that every mutation
# reads the snapshot once after `acquire()`, that a startup key is refused rather than skipped, that
# `RecordingStorage` records the two new mutations, or that a cell RUNS rather than is `#[ignore]`d.
# The engine, mcp and contract cells carry those. This file carries NO structural check.
#
# SEQUENCING, the same discipline every sibling states. The prose that NAMES this script rides the
# D57 SPEC commit (the ci-cd paragraph, §2.1(a)'s enumeration, `docs/PROCESS.md` §3's list entry and
# NEWEST pointer, and the retargeted pointer rows), so the specification leads the script. The
# IMPLEMENTATION commit carries the code, the doc comments, the schema step with its re-blessed
# digest, the re-pointed D46 `P3`/`P4` and D56 `Q26`/`Q27` rows, the contract bump with its
# re-blessed goldens, README and every sibling's `CONTRACT_RE` knob; the TEST commit carries the new
# cells. This script and its workflow step belong to the `ci(d57)` commit that mints the file, after
# both, because the code and cell rows assert lines that do not exist until those commits. The
# D-range knob is the inverse coupling: it guards PROSE the spec commit already moved, so it is LIVE
# from this file's first commit.
#
# NO ROW IS KNOWN-RED ON THE COMMIT THAT ADDS THIS FILE. `ub-lp9.2` already has a record in
# `.unblock/issues.jsonl` on `main`, so no tracker re-export is needed before this file exists. Every
# row pins a landing of the spec, implementation, test or `ci(d57)` commit — none pins the issue's
# CLOSED state, the run-report, the pull-request number or any other Track-step landing. Rows pin
# PRESENCE, never state, and the export keeps closed rows. D57 names `ub-p1h` (the `jsonl_export`
# consumer), `ub-lp9.44`, `ub-lp9.40` and the REMOTE-mode meaning of a DB row as OUT of scope, and no
# row pins them.
#
# PRE-FIX TREE: `main` at 533453a, the tree before D57's spec commit. Every row asserting a NEW
# landing FAILS there. Exactly THREE rows pass, by design, because they pin what already holds: P1
# (`ub-lp9.2` is already exported), the negative row N1 (no manifest names `directories` today) and
# Q14 (the sentinel already reads `COMMENTS_STEP_COLUMNS` to witness step 2; the row pins that the
# D57 union KEEPS that half).
#
# THE CONTRACT KNOB MOVES, stated affirmatively. D57 adds one input arm, one output arm, one
# description change and one `hint_shape` move (`CONFIG_ERROR` -> `contextual_text`) to the published
# documents, so `unblock.mcp.v1.11` becomes `unblock.mcp.v1.12` in the implementation commit (spine
# §5.4 D57 entry), and this file pins the LIVE id from its first commit.
#
# THREE RULE KINDS
#   P-n   REQUIRED landing — a presence predicate over the GENERATED export `.unblock/issues.jsonl`.
#   N-n   NEGATIVE landing — over a git PATHSPEC, no NON-COMMENT line may match. No line at all is
#         the pass.
#   Q-n   ROW-ANCHORED landing — at least ONE line must match the anchor, and EVERY line matching the
#         anchor must ALSO match the requirement. A vanished anchor is a FAILURE, never a pass. Every
#         code anchor starts `^` followed by the line's own indentation and then code, so no `///` or
#         `//!` line can satisfy it; in every production file that carries a `mod tests {`, the
#         production rows read only the lines before it, so no test body can satisfy them either (a
#         duplicated or missing `mod tests {` line in such a file is exit 2).
#
# Exit: 0 = pass · 1 = BLOCK (a required landing is missing) · 2 = cannot evaluate (fail-closed).
set -u

# PORTABILITY (2 of 2). Every variable expansion goes through `printf`, never `echo`. POSIX-mode `echo`
# interprets backslash escapes, so a `\b` inside a regex literal becomes a BACKSPACE byte and the
# pattern silently stops matching. Not a style choice.
say() { printf 'd57-claims: %s\n' "$*" >&2; }

git rev-parse --show-toplevel >/dev/null 2>&1 || { say "not a git repository"; exit 2; }
cd "$(git rev-parse --show-toplevel)" || { say "cannot cd to the repo root"; exit 2; }

# The LIVE D-id range, in its TWO spellings. It tracks the LIVE range, never a frozen historical one:
# the day a D58 is minted, every file `docs/PROCESS.md` §3 enumerates moves with it or a required step
# goes red. §3 deliberately states that cascade as a LIST WITH NO COUNT — a derived count rotted there
# five times — and the Q rows below are what make the list self-checking.
#
# WHY TWO SPELLINGS. `xtask/src/doc_lint.rs`'s bump site is ONE physical line carrying BOTH halves, the
# prose range `(D1..D57)` and the tokenizer's regex ALTERNATION `\bD(57|56|55|…)\b`. Pinning only the
# prose is exactly how that site rots into an undefined-D58 finding — the lint would stop tokenizing
# the id it is being told exists.
RANGE_RE='D1\.\.D57'
RANGE_ALT_RE='D\(57\|56\|'

# The LIVE published contract version. D57 MOVES it (`unblock.mcp.v1.11` -> `unblock.mcp.v1.12`), so
# this knob is LIVE from this file's first commit. It tracks the LIVE id, so a later decision's bump
# moves it in that decision's implementation commit.
CONTRACT_RE='unblock\.mcp\.v1\.12'

# The SAME spellings as they appear INSIDE a sibling script's knob line, where each backslash is a
# literal byte rather than a regex operator. DERIVED, never hand-written a second time. `printf '%s'`
# never interprets its ARGUMENT; `sed` then turns each `\` into `\\\`, i.e. ERE for "a literal
# backslash followed by the escaped char".
knob_re() { printf '%s' "$1" | sed 's/\\/\\\\\\/g'; }
RANGE_KNOB_RE="$(knob_re "$RANGE_RE")"
RANGE_KNOB_ALT_RE="$(knob_re "$RANGE_ALT_RE")"
CONTRACT_KNOB_RE="$(knob_re "$CONTRACT_RE")"

# The negative row's two patterns, held in variables for the reason d47's Q12 gives (`$@` in the
# double-quoted `@`-separated tables below would expand to the positional parameters). Both run over
# `git grep -n` output (`path:lineno:text`). A manifest comment line is one whose first non-blank
# character is `#`; the crate name is matched as a whole word, so `directories-next` and a path
# segment such as `test_directories` are both still caught or skipped exactly as the name requires.
TOML_COMMENT_LINE_RE='^[^:]+:[0-9]+:[[:space:]]*#'
DIRECTORIES_RE='(^|[^[:alnum:]_-])directories([^[:alnum:]_]|$)'

# =================================================================================================
# REQUIRED LANDINGS — `code@path@regex@what it proves`
#
# P1 is the work's own id. D57 names `ub-p1h`, `ub-lp9.44` and `ub-lp9.40` as OUT of scope, and no row
# pins them.
# =================================================================================================
REQUIRE="
P1@.unblock/issues.jsonl@\"id\":\"ub-lp9\.2\"@the tracker record names the work D57 implements. The export keeps closed rows, so the row survives the merge-time close
"

# =================================================================================================
# NEGATIVE LANDINGS — `code@pathspec@regex@what it proves`
# =================================================================================================
REQUIRE_NONE="
N1@*Cargo.toml@$DIRECTORIES_RE@NEGATIVE: no manifest in the tree names the directories crate, so the user path can never fall back to the platform-native directories D57 clause (3) rejects
"

# =================================================================================================
# ROW-ANCHORED LANDINGS — `code@path@anchor@regex@what it proves`
#
# Q1..Q46 are THE MECHANISM, each anchored on its own production line: the unblock-model
#      declarations (with the ONE blocking predicate file-scoped and `(self)`-exact, because
#      dependency.rs already declares a `pub const fn is_blocking(&self)`), the three Storage trait
#      declarations, schema step 3 with its columns, the sentinel's step-2 read (Q14, green on the
#      pre-fix tree by design), the SchemaGap carrier, the sentinel-subject assertion and the version
#      constant, the unblock-config user-layer, DB-layer and reload entries with the three ConfigError
#      variants and the two new ConfigLayer arms, the four Session config methods with the snapshot
#      field and the transparent Config source, the mcp input and output arms, the duplicate-key
#      corpus cell and BOTH copies of the new issue description, the cli's user tier and init
#      pre-check, the unblock-error hint_shape arm head, and the config_toml fuzz [[bin]]. Where the
#      requirement merely restates the anchor it is said plainly — the anchor is the teeth.
# Q47..Q76 are the cells, each on its own declaration: the four contract_config_ storage cells and
#      their four runner registrations, every config_ engine cell, the four config_ mcp cells, and the
#      six-layer golden's snapshot file.
# Q77..Q80 are the CONTRACT_RE knob's sites: CONTRACT_VERSION, the public_api.rs pin, README.md and
#      the unblock-mcp plan's declaring row.
# Q81..Q84 are the sibling rows the IMPLEMENTATION commit re-pointed: d46's P3 and P4 (the step-3
#      sentinel assertion and `= 3`) and d56's Q26 and Q27 (`restore, relabel, or close_eligible
#      issues[.,]`).
# Q85..Q115 are the SIBLING SCRIPTS' knobs — the NEWEST script pins the OLDER ones, and THIS script's
#      own knobs have NO row, deliberately: they are the REFERENCE the other rows are compared
#      against, and a self-row could never fail. Q114/Q115 are the marginal pair: d56's two range
#      knobs were pinned by nothing in the tree until this script landed.
# Q116..Q119 are the THREE PROSE bump sites over FOUR rows.
# Q120..Q145 are the normative and published texts: the D57 row and the [D57] notes on FR-13, D10,
#      D46, D56, FR-20 and the PRD §7 on-disk-artifacts row; spine §2.1's ConfigError note, §3.2.1's
#      Config table paragraph and its EventType row, §4's Layered configuration block, the §4.1
#      runtime-configuration block and the §5.4 ledger entry; the config plan's v1.1 heading and its
#      three new module rows, the storage plan's trait bullet, the engine plan's session/config.rs
#      row, the unblock-mcp plan's D57 row and docs/plans/README.md's CORRECTED CF-E resolution; the
#      markdown roadmap's FR-13 note and the RENDERED v1.1 card's bullet; README's tool-table line;
#      the T4.2 task and its acceptance list citing D57's criteria by letter.
# Q146..Q150 are this gate's wiring: SPECIFIED, RUNNING, LISTED in PROCESS.md §3 with both knob
#      names, PROCESS.md §3's NEWEST sentence, and ci-cd §2.1(a)'s enumeration naming it the newest.
# Q151..Q166 are the SIBLING ROWS the D57 spec commit retargeted to this script: d53's Q53, d54's Q67
#      and Q72, d55's Q78, Q80, Q81 and Q82, and d56's Q101, Q102 (reworded so it stops calling d56
#      the newest) and Q103..Q109.
# =================================================================================================
REQUIRE_ROW="
Q1@crates/unblock-model/src/results.rs@^    pub const fn is_blocking\(@pub const fn is_blocking\(self\) -> bool \{@ConfigFaultKind still declares the ONE blocking predicate, by value — file-scoped because dependency.rs declares an is_blocking(&self) a crate-wide row would match
Q2@crates/unblock-model/src/results.rs@^pub enum ConfigSource \{@pub enum ConfigSource \{@the layer-source enum is still declared; the anchor is the teeth
Q3@crates/unblock-model/src/results.rs@^pub struct ConfigEntry \{@pub struct ConfigEntry \{@the config action's entry is still declared; the anchor is the teeth
Q4@crates/unblock-model/src/results.rs@^pub struct ConfigFault \{@pub struct ConfigFault \{@the row fault is still declared; the anchor is the teeth
Q5@crates/unblock-model/src/results.rs@^pub struct DbConfigFault \{@pub struct DbConfigFault \{@the faulty-row carrier is still declared; the anchor is the teeth
Q6@crates/unblock-storage/src/trait_def.rs@^    async fn read_config\(@async fn read_config\(@the Storage trait still declares the ordered config-row read, NOT commented
Q7@crates/unblock-storage/src/trait_def.rs@^    async fn set_config\(@async fn set_config\(@the Storage trait still declares the one-transaction upsert
Q8@crates/unblock-storage/src/trait_def.rs@^    async fn unset_config\(@async fn unset_config\(@the Storage trait still declares the delete reporting whether a row went
Q9@crates/unblock-storage/src/libsql/migrations.rs@^ +\"CREATE TABLE IF NOT EXISTS config @\(key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL\)\",@schema step 3's MigrationKind::Sql entry still creates the config table with its IF NOT EXISTS idempotence, which the lying-stamp recovery's replay needs
Q10@crates/unblock-storage/src/libsql/migrations.rs@^ +kind: MigrationKind::Sql\(\$@kind: MigrationKind::Sql\(\$@the ladder still carries an ordinary Sql step — step 3, the first one
Q11@crates/unblock-storage/src/libsql/migrations.rs@^pub\(crate\) const CONFIG_STEP_COLUMNS: @&\[\(\"key\", \"TEXT\"\), \(\"value\", \"TEXT\"\)\];@the step-3 postcondition the sentinel witnesses is still declared with both columns
Q12@crates/unblock-storage/src/error.rs@^pub enum SchemaGap \{@pub enum SchemaGap \{@the lying-stamp dispatch's structured carrier is still declared
Q13@crates/unblock-storage/src/error.rs@^        gaps: @gaps: Vec<SchemaGap>,@StorageError::Migration still carries the gaps the hint dispatches on, never from == to
Q14@crates/unblock-storage/src/libsql/migrations.rs@^ +(let missing: Vec<&str> = )?COMMENTS_STEP_COLUMNS\$@COMMENTS_STEP_COLUMNS\$@the sentinel still READS step 2's comments columns, so the D57 union keeps witnessing step 2 — green on the pre-fix tree by design
Q15@crates/unblock-storage/src/libsql/migrations.rs@^ +(&& )?newest\.kind\.discriminant\(\) == @MigrationKind::Sql\(\"\"\)\.discriminant\(\)@the sentinel-subject const assertion names the version-3 Sql step as the newest step the sentinel witnesses
Q16@crates/unblock-storage/src/libsql/schema.rs@^pub\(crate\) const CURRENT_SCHEMA_VERSION: i32 = @CURRENT_SCHEMA_VERSION: i32 = 3;@the version constant carries the 2 -> 3 bump, by value
Q17@crates/unblock-config/src/user_config.rs@^pub fn user_config_path\(@pub fn user_config_path\(@the user-file path resolver is still public
Q18@crates/unblock-config/src/user_config.rs@^pub fn load_user_config\(@pub fn load_user_config\(@the SINGLE home of every user-file refusal is still public
Q19@crates/unblock-config/src/user_config.rs@^pub fn user_output_format\(@pub fn user_output_format\(@the cli format path's one config entry is still public
Q20@crates/unblock-config/src/user_config.rs@^    if let Some\(xdg\) = @absolute\(env\.get\(\"XDG_CONFIG_HOME\"\)\)@the user path still reads XDG_CONFIG_HOME through the injected EnvSource, absolute only
Q21@crates/unblock-config/src/db_layer.rs@^pub fn decode_db_rows\(@pub fn decode_db_rows\(@the pure tolerant fault decode the engine's get/list use is still public
Q22@crates/unblock-config/src/db_layer.rs@^pub\(crate\) fn db_layer\(@pub\(crate\) fn db_layer\(@the tolerant DB-layer decode is still declared
Q23@crates/unblock-config/src/reload.rs@^    pub fn new\(@pub fn new\(@ConfigReloader's public constructor is still declared
Q24@crates/unblock-config/src/reload.rs@^    pub fn reload_runtime\(@pub fn reload_runtime\(@the live re-resolution of the runtime keys is still declared
Q25@crates/unblock-config/src/reload.rs@^pub fn classify_runtime_key\(@pub fn classify_runtime_key\(@the config action's key classifier is still public
Q26@crates/unblock-config/src/reload.rs@^pub fn canonicalize_runtime_value\(@pub fn canonicalize_runtime_value\(@the config action's value canonicalizer is still public
Q27@crates/unblock-config/src/error.rs@^    StartupKeyInRuntimeLayer \{@StartupKeyInRuntimeLayer \{@the typed startup-key refusal is still a ConfigError variant
Q28@crates/unblock-config/src/error.rs@^    UnknownKey \{@UnknownKey \{@the unknown-key refusal is still a ConfigError variant
Q29@crates/unblock-config/src/error.rs@^    FaultyDbConfig \{@FaultyDbConfig \{@the fail-closed gate's error is still a ConfigError variant
Q30@crates/unblock-config/src/merge.rs@^    User\(@User\(&'a ProjectConfig\),@the six-arm ConfigLayer still carries the user layer
Q31@crates/unblock-config/src/merge.rs@^    Db\(@Db\(&'a DbLayer\),@the six-arm ConfigLayer still carries the DB layer
Q32@crates/unblock-engine/src/session/config.rs@^    pub async fn config_get\(@pub async fn config_get\(@the Session config get is still declared
Q33@crates/unblock-engine/src/session/config.rs@^    pub async fn config_list\(@pub async fn config_list\(@the Session config list is still declared
Q34@crates/unblock-engine/src/session/config.rs@^    pub async fn config_set\(@pub async fn config_set\(@the Session config set is still declared
Q35@crates/unblock-engine/src/session/config.rs@^    pub async fn config_unset\(@pub async fn config_unset\(@the Session config unset is still declared
Q36@crates/unblock-engine/src/session/mod.rs@^    pub\(crate\) runtime: @runtime: Arc<(std::sync::)?RwLock<Arc<RuntimeConfig>>>,@the Session still holds its runtime keys as the swappable std RwLock snapshot cell (RwLock is imported from std::sync)
Q37@crates/unblock-engine/src/error.rs@^    Config \{@Config \{@EngineError still carries the transparent Config source
Q38@crates/unblock-mcp/src/tools/issue.rs@^    Config \{@Config \{@the issue input still carries the config arm; the anchor is the teeth
Q39@crates/unblock-mcp/src/tools/output.rs@^    Config\(@Config\(ConfigList\),@the issue output still answers config with the object-wrapped ConfigList
Q40@crates/unblock-mcp/src/duplicate_key_corpus.rs@^ +arguments_text: r#\"\{\"action\":\"config\"@\"op\":\"get\",\"key\":\"actor\",\"op\":\"unset\"@the duplicate-key corpus still carries the config arm's read-to-write op escalation cell
Q41@crates/unblock-mcp/src/tools/issue.rs@^ +description = \"Create, show, update, close, @or get, set, unset and list runtime configuration with config\.@the issue tool description names the config action (tool-router copy)
Q42@crates/unblock-mcp/src/resources/capabilities.rs@^ +\"Create, show, update, close, @or get, set, unset and list runtime configuration with config\.@…and its capabilities() twin says the same
Q43@crates/unblock-cli/src/output.rs@^    match unblock_config::user_output_format\(@user_output_format\(env\)@the cli format path still reads the user tier through config's one entry
Q44@crates/unblock-cli/src/commands/init.rs@^    load_user_config\(@load_user_config\(&ProcessEnv\)\?;@init still loads the user file before it writes anything
Q45@crates/unblock-error/src/code.rs@^            Self::ValidationFailed \| Self::SchemaMismatch@Self::ValidationFailed \| Self::SchemaMismatch \| Self::ConfigError => \{@CONFIG_ERROR still sits in the ContextualText hint_shape arm (rustfmt keeps the arm head on one line)
Q46@crates/unblock-fuzz/fuzz/Cargo.toml@^name = \"config_toml\"@name = \"config_toml\"@the fuzz manifest still carries the config_toml [[bin]]
Q47@crates/unblock-storage/src/testkit.rs@^pub async fn contract_config_read_orders_by_key<@pub async fn contract_config_read_orders_by_key<@the read-orders-by-key contract cell is still declared
Q48@crates/unblock-storage/src/testkit.rs@^pub async fn contract_config_set_inserts_then_overwrites<@pub async fn contract_config_set_inserts_then_overwrites<@the insert-then-overwrite contract cell is still declared
Q49@crates/unblock-storage/src/testkit.rs@^pub async fn contract_config_unset_true_then_false<@pub async fn contract_config_unset_true_then_false<@the unset true-then-false contract cell is still declared
Q50@crates/unblock-storage/src/testkit.rs@^pub async fn contract_config_writes_no_event<@pub async fn contract_config_writes_no_event<@the no-event contract cell is still declared
Q51@crates/unblock-storage/src/testkit.rs@^    contract_config_read_orders_by_key\(@contract_config_read_orders_by_key\(factory\(\)\.await\)\.await;@the read-orders-by-key cell is REGISTERED in run_storage_contract_suite, so every backend runs it
Q52@crates/unblock-storage/src/testkit.rs@^    contract_config_set_inserts_then_overwrites\(@contract_config_set_inserts_then_overwrites\(factory\(\)\.await\)\.await;@the insert-then-overwrite cell is registered in the runner
Q53@crates/unblock-storage/src/testkit.rs@^    contract_config_unset_true_then_false\(@contract_config_unset_true_then_false\(factory\(\)\.await\)\.await;@the unset true-then-false cell is registered in the runner
Q54@crates/unblock-storage/src/testkit.rs@^    contract_config_writes_no_event\(@contract_config_writes_no_event\(factory\(\)\.await\)\.await;@the no-event cell is registered in the runner
Q55@crates/unblock-engine/tests/config.rs@^async fn config_startup_key_row_fail_closes_and_unset_repairs_the_same_session\(@\(\) \{@the startup-key-row gate-and-repair cell is still declared
Q56@crates/unblock-engine/tests/config.rs@^async fn config_undecodable_layer_row_fail_closes_and_set_replaces_it\(@\(\) \{@the undecodable-layer-row gate-and-replace cell is still declared
Q57@crates/unblock-engine/tests/config.rs@^async fn config_cross_process_unset_of_a_stale_fault_succeeds_without_a_write\(@\(\) \{@the cross-process stale-fault unset cell is still declared
Q58@crates/unblock-engine/tests/config.rs@^async fn config_cross_process_set_of_any_layer_key_clears_a_stale_gate\(@\(\) \{@the cross-process set-clears-the-gate cell is still declared
Q59@crates/unblock-engine/tests/config.rs@^async fn config_jsonl_export_is_refused_without_a_row_and_removable_with_one\(@\(\) \{@the non-layer jsonl_export cell is still declared
Q60@crates/unblock-engine/tests/config.rs@^async fn config_unknown_keys_warn_list_and_never_block\(@\(\) \{@the unknown-key cell is still declared
Q61@crates/unblock-engine/tests/config.rs@^async fn config_startup_keys_are_refused_by_the_action\(@\(\) \{@the startup-keys-in-the-action cell is still declared
Q62@crates/unblock-engine/tests/config.rs@^async fn config_set_actor_trims_and_refuses_blank\(@\(\) \{@the actor trim-and-blank cell is still declared
Q63@crates/unblock-engine/tests/config.rs@^async fn config_set_search_cap_reaches_the_next_search_without_a_reopen\(@\(\) \{@the set-then-search on one live session cell is still declared
Q64@crates/unblock-engine/tests/config.rs@^async fn config_set_actor_reaches_events_and_dep_created_by_and_unset_restores\(@\(\) \{@the actor reload cell is still declared
Q65@crates/unblock-engine/tests/config.rs@^async fn config_invalid_values_leave_table_and_snapshot_unchanged\(@\(\) \{@the validation-before-commit cell is still declared
Q66@crates/unblock-engine/tests/config.rs@^async fn config_reload_failure_from_a_malformed_user_file_writes_nothing\(@\(\) \{@the reload-failure-writes-nothing cell is still declared
Q67@crates/unblock-engine/tests/config.rs@^async fn config_db_value_shadowed_by_env_reports_both\(@\(\) \{@the shadow cell is still declared
Q68@crates/unblock-engine/tests/config.rs@^async fn config_empty_table_shapes\(@\(\) \{@the empty-table shapes cell is still declared
Q69@crates/unblock-engine/tests/config.rs@^async fn config_set_holds_permit_and_write_lock_while_get_and_list_do_not\(@\(\) \{@the locking-split cell is still declared
Q70@crates/unblock-engine/tests/config.rs@^async fn config_set_future_dropped_after_commit_still_swaps\(@\(\) \{@the dropped-future cancellation cell is still declared
Q71@crates/unblock-mcp/tests/config.rs@^async fn config_action_round_trips_every_op\(@\(\) \{@the every-op round-trip cell is still declared
Q72@crates/unblock-mcp/tests/config.rs@^async fn config_field_rule_rejects_in_band_with_zero_writes\(@\(\) \{@the per-op field-rule zero-write cell is still declared
Q73@crates/unblock-mcp/tests/config.rs@^async fn config_fault_gate_over_tools_resources_and_prompts\(@\(\) \{@the fault gate over tools, resources and prompts cell is still declared
Q74@crates/unblock-mcp/tests/config.rs@^async fn config_contract_arms_last_and_v1_12\(@\(\) \{@the arms-last and live-id contract cell is still declared
Q75@crates/unblock-config/tests/snapshots/precedence__six_layer_resolution.snap@^source: @crates/unblock-config/tests/precedence\.rs@the six-layer golden resolution's snapshot file exists and is the precedence suite's
Q76@crates/unblock-config/tests/snapshots/precedence__six_layer_resolution.snap@^        actor: @actor: (Cli|Env|Project|User|Db|OsUser|Default),@…and it pins the winning source of the actor tier
Q77@crates/unblock-mcp/src/options.rs@^pub const CONTRACT_VERSION@$CONTRACT_RE@D57 adds one input arm, one output arm, a description change and a hint_shape move to the published documents, so the code constant carries the LIVE id
Q78@crates/unblock-mcp/tests/public_api.rs@^ +assert_eq!\(CONTRACT_VERSION, \"@$CONTRACT_RE@the second, independent CONTRACT_VERSION pin moved with it
Q79@README.md@unblock\.mcp\.v1\.[0-9]@$CONTRACT_RE@every README line naming a contract id names the LIVE one; README keeps no bump history
Q80@docs/plans/crates/unblock-mcp.md@^\| .pub const CONTRACT_VERSION@\*\*.$CONTRACT_RE.\*\* \(bumped from@the OWNING crate plan's CONTRACT_VERSION row DECLARES the live id as its value; the bump chain after it keeps the retired ones
Q81@scripts/checks/d46-schema-migration-claims.sh@^P3[^0-9]@MigrationKind::Sql@the D46 sibling's P3 is RE-POINTED at the step-3 sentinel-subject assertion (D57)
Q82@scripts/checks/d46-schema-migration-claims.sh@^P4[^0-9]@CURRENT_SCHEMA_VERSION: i32 = 3@the D46 sibling's P4 is RE-POINTED at = 3 (D57)
Q83@scripts/checks/d56-organization-claims.sh@^Q26[^0-9]@close_eligible issues\[\.,\]@the D56 sibling's Q26 is RE-POINTED at the description that now continues past issues (D57)
Q84@scripts/checks/d56-organization-claims.sh@^Q27[^0-9]@close_eligible issues\[\.,\]@…and its Q27 twin, for the capabilities() copy
Q85@scripts/checks/d44-create-deps-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D44 sibling's live-range knob carries the SAME range as this script — anchored on the knob line, so the file's own prose about the knob cannot satisfy the pin
Q86@scripts/checks/d44-create-deps-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@…and that sibling's contract knob carries the SAME live id as this script
Q87@scripts/checks/ub-lp9.25-dangling-blocker-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D45 sibling's live-range knob, same anchoring and same reason
Q88@scripts/checks/ub-lp9.25-dangling-blocker-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, a separate line and so a separate way to be half-bumped
Q89@scripts/checks/ub-lp9.25-dangling-blocker-claims.sh@^CONTRACT_RE=@mcp.*v1.*[^0-9]12'\$@…and that sibling's contract knob, which spells the id with its \\\\? segments (it pins a regex-literal knob line too), so this row checks the LIVE minor at its end instead of re-deriving the spelling
Q90@scripts/checks/d46-schema-migration-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D46 sibling's live-range knob
Q91@scripts/checks/d46-schema-migration-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q92@scripts/checks/d47-envelope-id-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D47 sibling's live-range knob
Q93@scripts/checks/d47-envelope-id-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q94@scripts/checks/d47-envelope-id-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@…and that sibling's contract knob
Q95@scripts/checks/d48-stdout-channel-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D48 sibling's live-range knob
Q96@scripts/checks/d48-stdout-channel-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q97@scripts/checks/d48-stdout-channel-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@…and that sibling's contract knob
Q98@scripts/checks/d49-startup-failure-render-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D49 sibling's live-range knob
Q99@scripts/checks/d49-startup-failure-render-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q100@scripts/checks/d49-startup-failure-render-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@…and that sibling's contract knob
Q101@scripts/checks/d50-pre-handshake-gate-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D50 sibling's live-range knob
Q102@scripts/checks/d50-pre-handshake-gate-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q103@scripts/checks/d50-pre-handshake-gate-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@…and that sibling's contract knob
Q104@scripts/checks/d53-request-integrity-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D53 sibling's live-range knob
Q105@scripts/checks/d53-request-integrity-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q106@scripts/checks/d53-request-integrity-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@…and that sibling's contract knob
Q107@scripts/checks/d54-parse-error-id-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D54 sibling's live-range knob
Q108@scripts/checks/d54-parse-error-id-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q109@scripts/checks/d54-parse-error-id-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@…and that sibling's contract knob
Q110@scripts/checks/d55-update-refusal-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D55 sibling's live-range knob
Q111@scripts/checks/d55-update-refusal-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob
Q112@scripts/checks/d55-update-refusal-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@…and that sibling's contract knob
Q113@scripts/checks/d56-organization-claims.sh@^CONTRACT_RE=@$CONTRACT_KNOB_RE@the D56 sibling's contract knob
Q114@scripts/checks/d56-organization-claims.sh@^RANGE_RE=@$RANGE_KNOB_RE@the D56 sibling's live-range knob — the PREVIOUS newest script, which by the self-row rule pins everyone except itself and so needs this row
Q115@scripts/checks/d56-organization-claims.sh@^RANGE_ALT_RE=@$RANGE_KNOB_ALT_RE@…and that sibling's ALTERNATION knob, the other of the two knobs d56 cannot pin itself
Q116@CLAUDE.md@^\| .docs/PRD\.md. \| Product truth@$RANGE_RE@PROSE D-range bump site, LOCATED on the document-map row that states the range
Q117@docs/plans/ci-cd-and-distribution.md@\*\*\(a\) D-id coherence\*\*@$RANGE_RE@PROSE D-range bump site, LOCATED on the class-(a) statement — the ONE place in that file allowed to quote the live range
Q118@xtask/src/doc_lint.rs@Spec tokenizes@$RANGE_RE@PROSE D-range bump site, half 1 of 2: the PROSE range on the tokenizer comment line
Q119@xtask/src/doc_lint.rs@Spec tokenizes@$RANGE_ALT_RE@…half 2 of 2: the TOKENIZER ALTERNATION on that same line
Q120@docs/PRD.md@^\| \*\*D57\*\* \|@Tracked as .ub-lp9\.2.@the D57 decision row is in PRD §4 and names the work it tracks
Q121@docs/PRD.md@^- \*\*FR-13 \[@\*\*\[D57\] The v1\.1 remainder is specified by PRD §4 D57@FR-13 carries the reciprocal note naming the decision that specifies its v1.1 layers
Q122@docs/PRD.md@^\| \*\*D10\*\* \|@\*\*\[D57\] AMENDED by PRD §4 D57 clause \(5\):\*\*@D10 carries the reciprocal note on its amended actor chain
Q123@docs/PRD.md@^\| \*\*D46\*\* \|@\*\*\[D57\] The first ORDINARY step is step 3\*\*@D46 carries the note on its ladder's first ordinary step
Q124@docs/PRD.md@^\| \*\*D56\*\* \|@\*\*\[D57\]\*\* The .issue. description is extended again by D57@D56 carries the note that its issue description continues
Q125@docs/PRD.md@^- \*\*FR-20 \[must\] — MCP stdio server@\*\*\[D57\]\*\* the FR-13 DB config layer's writer adds NO tool@FR-20 carries the note: the DB layer's writer rides the issue tool, never a ninth one
Q126@docs/PRD.md@^\| \*\*On-disk artifacts\*\*@\*\*\[D57\] \(v1\.1\)\*\* .unblock\.db. gains a .config. table@the PRD §7 on-disk-artifacts row carries the config table
Q127@docs/plans/01-design-spine.md@^> \*\*\[D57\] \(v1\.1\) — three more variants@StartupKeyInRuntimeLayer \{ key: String@spine §2.1's ConfigError note names the new variants
Q128@docs/plans/01-design-spine.md@^\*\*Config table \(D57, v1\.1 — normative\)\.\*\*@three FR-13-full methods@spine §3.2.1's normative Config table paragraph is present
Q129@docs/plans/01-design-spine.md@^\| config set / unset \(.issue action: config., D57\)@\*\*none\*\*@the EventType-per-mutation table carries the config row, which records no event
Q130@docs/plans/01-design-spine.md@^> \*\*Layered configuration — FR-13 full \(D57@CLI > env@spine §4's normative Layered configuration block is present and states the order
Q131@docs/plans/01-design-spine.md@^    // --- runtime configuration \(FR-13 full, D57, v1\.1\) — .src/session/config\.rs.@\(NEW\)@spine §4.1's runtime-configuration block is present; the anchor is the teeth
Q132@docs/plans/01-design-spine.md@^\*\*D57 \(v1\.1\) — the contract bumps to@bumps to .unblock\.mcp\.v1\.12.@the spine §5.4 ledger carries the D57 entry
Q133@docs/plans/crates/unblock-config.md@^### v1\.1 \(full — FR-13 full;@\*\*PRD §4 D57\*\*, .ub-lp9\.2.@the config plan's v1.1 heading names the decision and the work
Q134@docs/plans/crates/unblock-config.md@^\| .src/user_config\.rs. \|@\*\*v1\.1 \(D57\)\*\*@the config plan's src/user_config.rs row is present
Q135@docs/plans/crates/unblock-config.md@^\| .src/db_layer\.rs. \|@\*\*v1\.1 \(D57\)\*\*@the config plan's src/db_layer.rs row is present
Q136@docs/plans/crates/unblock-config.md@^\| .src/reload\.rs. \|@\*\*v1\.1 \(D57\)\*\*@the config plan's src/reload.rs row is present
Q137@docs/plans/crates/unblock-storage.md@^- Trait grows \(additive\):@\*\*\[D57\] — the config-table accessors are specified@the storage plan's trait-growth bullet carries the D57 methods
Q138@docs/plans/crates/unblock-engine.md@^\| .src/session/config\.rs.@\*\*PRD §4 D57\*\*, .ub-lp9\.2.@the engine plan's src/session/config.rs row names the decision and the work
Q139@docs/plans/crates/unblock-mcp.md@^\| .src/tools/issue\.rs. \+ .src/tools/output\.rs. \+ .src/resources/capabilities\.rs. \+ .src/duplicate_key_corpus\.rs.@\*\*PRD §4 D57\*\*, .ub-lp9\.2.@the unblock-mcp plan's D57 row names the decision and the work
Q140@docs/plans/README.md@^- \*\*CF-E — \[RESOLVED\]@\*\*\[D57\] CORRECTED \(2026-10-30\):\*\*@docs/plans/README.md's CF-E resolution carries the D57 correction
Q141@docs/plans/00-roadmap.md@^\| FR-13 \(full\) \|@\*\*\[D57\]\*\* Specified by PRD §4 D57 \(.ub-lp9\.2.@the markdown roadmap §2 FR-13 row carries the D57 note and names the tracked work
Q142@docs/roadmap.html@^ +<li>Full config \(FR-13\):@live reload \(D57\)@the RENDERED v1.1 card's FR-13 bullet names D57 — that file is OUTSIDE the doc-lint corpus
Q143@README.md@^\| .issue. \|@· .config. \(.op.: .get. · .set. · .unset. · .list.@the README tool table's issue line names the config action and its four ops — README is OUTSIDE the doc-lint corpus
Q144@docs/plans/implementation-plan.md@^- \*\*T4\.2 — @FR-13 full@the task checklist exists and names FR-13 full
Q145@docs/plans/implementation-plan.md@^  D57 clause \(12\) \(a\) ORDER@\(d4\) INIT@the task's acceptance list cites D57's criteria by letter
Q146@docs/plans/ci-cd-and-distribution.md@Named sub-check \(its D57 sibling@d57-config-layers-claims@this gate is SPECIFIED in its own paragraph — §2.1(a) of that same file also carries this filename, so a bare token stays green with the paragraph deleted
Q147@.github/workflows/ci.yml@d57-config-layers-claims@:[0-9]+: +- run: scripts/checks/d57-config-layers-claims\.sh@this gate actually RUNS in the required doc-lint job; every line naming it must BE the run step, so a commented-out step fails
Q148@docs/PROCESS.md@d57-config-layers-claims\.sh. .RANGE_RE@RANGE_ALT_RE@the count-free LIST that IS the rule carries this script's own entry with BOTH knob names
Q149@docs/PROCESS.md@always the NEWEST script@d57-config-layers-claims\.sh@PROCESS.md §3 names this gate as the newest reference, which pins every OLDER script's knobs (the next mint retargets it)
Q150@docs/plans/ci-cd-and-distribution.md@\*\*\(a\) D-id coherence\*\*@the .ci\(d57\). commit that mints it, .scripts/checks/d57-config-layers-claims\.sh. .RANGE_RE. \*\*and\*\* .RANGE_ALT_RE., the newest of them@ci-cd §2.1(a)'s enumeration names this script as the newest
Q151@scripts/checks/d53-request-integrity-claims.sh@^Q53[^0-9]@d57-config-layers-claims@the D53 sibling's newest-pointer row names this gate
Q152@scripts/checks/d54-parse-error-id-claims.sh@^Q67[^0-9]@d57-config-layers-claims@the D54 sibling's row pinning d53's Q53 follows the retarget
Q153@scripts/checks/d54-parse-error-id-claims.sh@^Q72[^0-9]@d57-config-layers-claims@the D54 sibling's newest-pointer row names this gate
Q154@scripts/checks/d55-update-refusal-claims.sh@^Q78[^0-9]@d57-config-layers-claims@the D55 sibling's newest-pointer row names this gate
Q155@scripts/checks/d55-update-refusal-claims.sh@^Q80[^0-9]@d57-config-layers-claims@the D55 sibling's row pinning d53's Q53 follows the retarget
Q156@scripts/checks/d55-update-refusal-claims.sh@^Q81[^0-9]@d57-config-layers-claims@the D55 sibling's row pinning d54's Q67 follows the retarget
Q157@scripts/checks/d55-update-refusal-claims.sh@^Q82[^0-9]@d57-config-layers-claims@the D55 sibling's row pinning d54's Q72 follows the retarget
Q158@scripts/checks/d56-organization-claims.sh@^Q101[^0-9]@d57-config-layers-claims@the D56 sibling's newest-pointer row names this gate
Q159@scripts/checks/d56-organization-claims.sh@^Q102[^0-9]@stopped naming this script the newest@the D56 sibling's Q102 is reworded so it stops calling d56 the newest
Q160@scripts/checks/d56-organization-claims.sh@^Q103[^0-9]@d57-config-layers-claims@the D56 sibling's row pinning d53's Q53 follows the retarget
Q161@scripts/checks/d56-organization-claims.sh@^Q104[^0-9]@d57-config-layers-claims@the D56 sibling's row pinning d54's Q67 follows the retarget
Q162@scripts/checks/d56-organization-claims.sh@^Q105[^0-9]@d57-config-layers-claims@the D56 sibling's row pinning d54's Q72 follows the retarget
Q163@scripts/checks/d56-organization-claims.sh@^Q106[^0-9]@d57-config-layers-claims@the D56 sibling's row pinning d55's Q78 follows the retarget
Q164@scripts/checks/d56-organization-claims.sh@^Q107[^0-9]@d57-config-layers-claims@the D56 sibling's row pinning d55's Q80 follows the retarget
Q165@scripts/checks/d56-organization-claims.sh@^Q108[^0-9]@d57-config-layers-claims@the D56 sibling's row pinning d55's Q81 follows the retarget
Q166@scripts/checks/d56-organization-claims.sh@^Q109[^0-9]@d57-config-layers-claims@the D56 sibling's row pinning d55's Q82 follows the retarget
"

blocked=0

# PORTABILITY (1 of 2), deliberate. Every `$( … )` in this file substitutes a FUNCTION CALL, never an
# inline loop containing a `case`. macOS `/bin/sh` (bash 3.2) mis-parses a `case` arm's `)` inside
# `$( )` and silently produces garbage instead of failing. Every sibling avoids it the same way.
#
# PRODUCTION PREFIXES. A row on one of the files below reads ONLY the lines before that file's
# `mod tests {`, so no test body can satisfy it. Each of these files carries exactly one such line;
# a missing or duplicated one is fail-closed (exit 2), never a pass. A file absent from the tree is
# not evaluated here: its rows fail as missing targets below.
PROD_FILES='crates/unblock-model/src/results.rs
crates/unblock-storage/src/trait_def.rs
crates/unblock-storage/src/error.rs
crates/unblock-storage/src/libsql/schema.rs
crates/unblock-config/src/user_config.rs
crates/unblock-config/src/error.rs
crates/unblock-config/src/merge.rs
crates/unblock-engine/src/session/mod.rs
crates/unblock-engine/src/error.rs
crates/unblock-mcp/src/tools/output.rs
crates/unblock-mcp/src/resources/capabilities.rs
crates/unblock-mcp/src/options.rs
crates/unblock-cli/src/output.rs
crates/unblock-cli/src/commands/init.rs
crates/unblock-error/src/code.rs'
tests_line() { git grep -n -E '^mod tests \{$' -- "$1" 2>/dev/null | cut -d: -f2; }
is_prod_file() { printf '%s\n' "$PROD_FILES" | grep -q -x -F "$1"; }
for f in $PROD_FILES; do
  [ -f "$f" ] || continue
  n="$(tests_line "$f")"
  case "$n" in
    ''|*[!0-9]*) say "cannot find exactly one '^mod tests {' line in $f"; exit 2 ;;
  esac
done

check_landings() {
  printf '%s\n' "$REQUIRE" | while IFS='@' read -r code path re reason; do
    [ -n "$code" ] || continue
    if [ ! -f "$path" ]; then
      printf '%s\n' "$path: [$code] REQUIRED D57 target is missing from the tree ($reason)"
      continue
    fi
    git grep -q -I -E "$re" -- "$path" 2>/dev/null \
      || printf '%s\n' "$path: [$code] the landing is GONE — no line matches /$re/ ($reason)"
  done
  printf '%s\n' "$REQUIRE_NONE" | while IFS='@' read -r code spec re reason; do
    [ -n "$code" ] || continue
    # A pathspec, not a file: no existence check. `git grep` exits 1 on no match, which is the pass.
    hits="$(git grep -n -I -E "$re" -- "$spec" 2>/dev/null | grep -v -E "$TOML_COMMENT_LINE_RE")"
    if [ -n "$hits" ]; then
      printf '%s\n' "$spec: [$code] forbidden line(s) $(printf '%s' "$hits" | cut -d: -f1,2 | tr '\n' ' ')($reason)"
    fi
  done
  printf '%s\n' "$REQUIRE_ROW" | while IFS='@' read -r code path anchor re reason; do
    [ -n "$code" ] || continue
    if [ ! -e "$path" ]; then
      printf '%s\n' "$path: [$code] REQUIRED D57 target is missing from the tree ($reason)"
      continue
    fi
    rows="$(git grep -n -I -E "$anchor" -- "$path" 2>/dev/null)"
    if is_prod_file "$path"; then
      rows="$(printf '%s\n' "$rows" | awk -F: -v end="$(tests_line "$path")" 'NF && $2 + 0 < end + 0')"
    fi
    if [ -z "$rows" ]; then
      printf '%s\n' "$path: [$code] the anchor line /$anchor/ no longer exists, so this pin now proves NOTHING ($reason)"
      continue
    fi
    # `git grep -n` prints `path:lineno:text`, so `path:lineno` is fields 1-2.
    bad="$(printf '%s\n' "$rows" | grep -v -E "$re" | cut -d: -f1,2)"
    if [ -n "$bad" ]; then
      printf '%s\n' "$path: [$code] the anchored line(s) $(printf '%s' "$bad" | tr '\n' ' ') do not match /$re/ ($reason)"
    fi
  done
}

missing="$(check_landings)"
if [ -n "$missing" ]; then
  printf '%s\n' "$missing" >&2
  say "BLOCKED — the D57 config-layers surface or its cascade is incomplete; the sites above are missing."
  blocked=1
fi

# SELF-TEST — no rule table may have SHRUNK below the counts it shipped with; the three tables are
# counted SEPARATELY so adding a row to one can never mask the deletion of a row from another.
count_rows() { # $1 = table text, $2 = code prefix regex
  printf '%s\n' "$1" | grep -c "$2"
}

check_table_floor() { # $1 label, $2 actual, $3 floor
  if [ "$2" -lt "$3" ]; then
    say "BLOCKED — the $1 table has $2 rows; it shipped with $3. A rule was dropped."
    return 1
  fi
  return 0
}

p_count="$(count_rows "$REQUIRE" '^P[0-9]')"
n_count="$(count_rows "$REQUIRE_NONE" '^N[0-9]')"
q_count="$(count_rows "$REQUIRE_ROW" '^Q[0-9]')"

# The floors are the counts this file shipped with. A floor may only move down together with the rows
# it counts moving somewhere another floor counts them — never because a rule became inconvenient.
check_table_floor 'required-landing (P)' "$p_count" 1 || blocked=1
check_table_floor 'negative (N)' "$n_count" 1 || blocked=1
check_table_floor 'row-anchored (Q)' "$q_count" 166 || blocked=1

[ "$blocked" = "0" ] || exit 1
say "OK — the model types and the one blocking predicate, the three Storage config methods, schema step 3 with its columns, the sentinel's union and subject assertion, the SchemaGap carrier and the version constant, the config user/DB/reload entries with the three ConfigError variants and the two ConfigLayer arms, the four Session config methods with the snapshot cell and the Config source, the mcp arms, corpus cell and both description twins, the cli user tier and init pre-check, the hint_shape arm and the config_toml fuzz bin are in the tree, no manifest names directories, every contract_config_ and config_ cell and the six-layer golden are present with every contract cell registered, the tracker names ub-lp9.2, the PRD row and its notes, the spine, the crate plans, both roadmaps, README and the task carry D57, the re-pointed and retargeted sibling rows are in place, this gate is both specified and wired, the contract reads the live id, and the live D-range is current at every prose site and every sibling script's knob"
exit 0
