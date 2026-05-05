#!/usr/bin/env bash
# scripts/setup-test-project.sh
#
# Idempotently bootstraps the unblock-test GitHub Projects V2 project to a
# clean state for the live integration test job (.github/workflows/ci.yml
# `test-mcp-live`).
#
# WHAT IT DOES (default mode — custom-fields wipe)
#   - Lists every custom field on the target project via `gh project
#     field-list`.
#   - Deletes any field whose name is one of the 6 unblock-managed custom
#     fields: Priority, PipelineStage, Agent, ClaimedAt, StoryPoints,
#     DeferUntil.
#   - Leaves the project's built-in `Status` field intact.
#     `setup_fields()` in the Rust code-base auto-heals the built-in
#     Status options to the spec's canonical set
#     (Backlog/Ready/In Progress/Blocked/Deferred/Closed — TitleCase,
#     board order; sourced from Status::option_name per spec §5.7 and
#     bead unblock-1zj) on every run, so the script has nothing to do
#     here. The auto-heal matcher preserves existing option IDs across
#     the lowercase → TitleCase rename via a normalised name match.
#
# WHAT IT DOES (--wipe-issues mode — fixture-issue wipe)
#   - Resolves the project's GraphQL node id (required for
#     `deleteProjectV2Item`).
#   - Fetches every OPEN issue in <owner>/<repo> bearing the canonical
#     fixture label `unblock-fixture` (label applied automatically by
#     the live tests' `fixture_labels()` helper — see bead unblock-1hz).
#     Closed fixture issues are intentionally skipped: cycle 2 of the
#     bead scoped the search to `is:open` because closed issues whose
#     project items are already removed are the steady-state outcome of
#     a successful prior wipe — there is no further work to do for them
#     and iterating over the closed-issue tail eats CI walltime
#     unboundedly across the project's lifetime.
#   - For each open fixture issue:
#       * Closes it via `gh issue close` (without `--comment`, cycle 2)
#         so live tests do not leave an orphaned-open trail when the
#         panic-path Drop guard's tokio::spawn cleanup loses its runtime
#         race (bead unblock-ekf documented best-effort caveat).
#       * Resolves the `ProjectV2Item` id corresponding to the issue node
#         on the target project and calls `deleteProjectV2Item` so the
#         board card disappears. `close_issue` alone leaves the closed
#         issue visible on the board as a project item — only
#         deleteProjectV2Item removes the card (bead unblock-1hz Risk
#         R2/R3).
#   - View cleanup is NOT performed: GitHub Projects V2 exposes no public
#     API to delete a project view (no `deleteProjectV2View` in GraphQL,
#     no `DELETE /views` in REST — confirmed against the v2 schema and
#     2026-03-10 REST OpenAPI; see
#     `docs/archive/research/github-projectsv2-views-api-findings.md`).
#     The live test suite reuses 3 stable fixture views
#     (test-board-fixture, test-table-fixture, test-roadmap-fixture)
#     instead of creating new timestamp-named ones each run, so the
#     previously unbounded view accumulation is now capped at exactly 3.
#
# WHAT IT DOES (--wipe-labels mode — orphan-label wipe)
#   - Lists every label on <owner>/<repo> via `gh label list`.
#   - For each label whose name matches any of the test-orphan glob
#     patterns (`e2e-test-*`, `test-label-*`, `unblock-run-*`):
#       * If the label name is in CANONICAL_TEST_LABELS (`unblock-fixture`
#         or `unblock-test-label`), it is preserved.
#       * Otherwise, the label is deleted via `gh label delete --yes`.
#     Production labels that do not match any of the test patterns are
#     NEVER touched — the denylist is pattern-scoped, not allowlist-scoped,
#     so a real label called `test` (or any other operator-managed label)
#     is left intact. Cycle 2 of bead `unblock-1hz` introduced this mode
#     after the test repo accumulated ~49 orphan labels (7x e2e-test-*,
#     ~40x test-label-*, plus the cycle-1 unblock-run-* per-run labels)
#     across CI runs. Without bulk delete the only options were the GitHub
#     web UI (one-by-one) or a hand-rolled gh script — this mode is the
#     scripted one-shot.
#   - Idempotent: a no-op when no orphan labels are present.
#
# WHY THE BUILT-IN STATUS IS LEFT ALONE
#   GitHub Projects V2 forbids deletion of the built-in Status field
#   (deleteProjectV2Field returns "Only custom fields can be deleted").
#   Empirical verification against project websublime/unblock-test #7 on
#   2026-04-30 confirmed that updateProjectV2Field can extend / rename
#   options on the built-in Status field while preserving option IDs by
#   name match — which is exactly what unblock-github's setup_fields auto-
#   heal path does. See bead unblock-aa2 for the full investigation.
#
# IDEMPOTENCY
#   - Default mode: when all 6 fields are absent, the script is a no-op
#     (empty input pipeline).
#   - Default mode: when some are present, only the present ones are
#     deleted; subsequent runs are no-ops.
#   - --wipe-issues mode: when no fixture issues exist, the script is a
#     no-op (empty issue list).
#   - --wipe-issues mode: deleteProjectV2Item on a missing item returns a
#     structured error which we log and continue.
#   - --wipe-labels mode: when no orphan labels are present, the script is
#     a no-op. Re-running immediately after a successful wipe is also a
#     no-op (the canonical labels are preserved by name, and the only
#     test-pattern labels left are the ones that just survived the wipe —
#     i.e. none).
#   - Re-running after a successful `cargo test ... -- --ignored` round
#     trip is safe in any mode.
#
# USAGE
#   scripts/setup-test-project.sh [--check] <owner> <project-number>
#   scripts/setup-test-project.sh --wipe-issues <owner> <project-number> <repo>
#   scripts/setup-test-project.sh --wipe-labels <owner> <repo>
#
#   Default mode mutates the project (deletes stale custom fields).
#
#   --check / --dry-run mode prints what WOULD be deleted (custom-fields
#   only) without mutating, and exits non-zero if any unblock-managed
#   custom fields are present (exit 4). Useful as a CI preflight
#   assertion that the project is in the canonical clean state before
#   live tests run. --check has no effect on issues or labels; pair with
#   --wipe-issues / --wipe-labels only when you intend to mutate.
#
#   --wipe-issues mode requires a 3rd positional argument <repo> (just
#   the repository name, not owner/repo). The mode operates on the
#   issues in <owner>/<repo> with the canonical fixture label.
#
#   --wipe-labels mode takes <owner> and <repo> only (no project number —
#   labels are repo-scoped, not project-scoped). It deletes orphan
#   timestamp-suffixed test labels and preserves the two canonical
#   labels plus all production labels.
#
#   The three mutating modes are orthogonal — to perform multiple wipes,
#   run the script multiple times. Keeping them orthogonal preserves the
#   existing --check semantics on the custom-fields path and avoids the
#   ambiguity of a combined --all flag.
#
#   Examples:
#     scripts/setup-test-project.sh websublime 7
#     scripts/setup-test-project.sh --check websublime 7
#     scripts/setup-test-project.sh --dry-run websublime 7
#     scripts/setup-test-project.sh --wipe-issues websublime 7 unblock-test
#     scripts/setup-test-project.sh --wipe-labels websublime unblock-test
#
# REQUIREMENTS
#   - `gh` CLI authenticated against the target owner with
#     `project` + `repo` scopes.
#   - `jq` for JSON filtering.
#
# EXIT CODES
#   0  success (including the no-op case where nothing needed deleting)
#   1  invalid argument count or non-numeric project number
#   2  required tooling missing (gh, jq)
#   3  `gh` auth failure or unrecoverable API error during list/delete
#   4  --check mode found stale unblock-managed custom fields (drift)
#
# QUALITY
#   - shellcheck-clean (shellcheck 0.11.0, 2026-04-30) — verified manually
#     after the unblock-aa2 rework. The few SC2016 occurrences are
#     suppressed inline because the literal backticks are intentional
#     (Markdown-style inline code in operator-facing error messages, not
#     command substitution).
#   - bash -n parse check is run by CI per CLAUDE.md quality gate.

set -euo pipefail

# The 6 unblock-managed custom fields. The built-in `Status` field is
# intentionally NOT in this list — see header comment.
readonly UNBLOCK_CUSTOM_FIELDS=(
  "Priority"
  "PipelineStage"
  "Agent"
  "ClaimedAt"
  "StoryPoints"
  "DeferUntil"
)

# Canonical fixture marker label (see crates/unblock-github/tests/integration.rs
# and crates/unblock-mcp/tests/common/mod.rs — `FIXTURE_LABEL`). Live tests
# attach this label to every create_issue call; --wipe-issues selects on
# it exactly.
readonly FIXTURE_LABEL="unblock-fixture"

# Canonical labels created by the live tests. The `--wipe-labels` mode
# preserves these (alongside any non-test-pattern repo labels — see
# WIPE_LABEL_PATTERNS below) and deletes only the orphan timestamp-suffixed
# labels accumulated by previous test runs. Cycle 2 of bead `unblock-1hz`
# pinned the live test surface to exactly two canonical labels:
#   * unblock-fixture   — applied to every fixture issue (wipe anchor)
#   * unblock-test-label — per-test discriminator used by e2e_workflow's
#                          ready/list filtering and the ensure_labels
#                          integration test
readonly CANONICAL_TEST_LABELS=(
  "unblock-fixture"
  "unblock-test-label"
)

# Patterns matched by `--wipe-labels`. A repo label is deleted iff its name
# matches any pattern in this list AND is not present in
# CANONICAL_TEST_LABELS. Patterns are matched against the full label name
# with bash's `[[ == ]]` glob (so `*` is the wildcard). The list captures
# every shape of timestamp-suffixed test label the codebase has ever
# emitted:
#   * e2e-test-*    — historical per-run label from e2e_workflow.rs
#                     (`e2e-test-<seconds>` and `e2e-test-<millis>`)
#   * test-label-*  — historical per-run label from the
#                     `ensure_labels_creates_missing_labels` integration
#                     test
#   * unblock-run-* — cycle-1 per-run discriminator, dropped in cycle 2
# Production labels that don't match any of these patterns are NEVER
# touched. The denylist-style check (pattern match + canonical-label
# preservation) is intentional: an allowlist would risk silently leaving
# orphans behind whenever a future test introduces a new naming scheme.
readonly WIPE_LABEL_PATTERNS=(
  "e2e-test-*"
  "test-label-*"
  "unblock-run-*"
)

usage() {
  cat <<'EOF' >&2
Usage:
  scripts/setup-test-project.sh [--check] <owner> <project-number>
  scripts/setup-test-project.sh --wipe-issues <owner> <project-number> <repo>
  scripts/setup-test-project.sh --wipe-labels <owner> <repo>

  --check          Dry-run for default (custom-fields) mode: list
                   (without deleting) any stale unblock-managed custom
                   fields. Exits 0 when the project is already clean,
                   exits 4 when stale fields are present.
  --dry-run        Alias for --check.
  --wipe-issues    Closes every OPEN issue in <owner>/<repo> with the
                   `unblock-fixture` label and removes its corresponding
                   project item from <project-number>. View cleanup is
                   not automated (upstream API constraint).
  --wipe-labels    Deletes orphan timestamp-suffixed test labels from
                   <owner>/<repo> (e2e-test-*, test-label-*,
                   unblock-run-*). Preserves the canonical labels
                   (`unblock-fixture`, `unblock-test-label`) and any
                   production labels.
  owner            GitHub org or user that owns the test project / repo
  project-number   The Projects V2 project number (visible in the URL).
                   Required by default and --wipe-issues; not used by
                   --wipe-labels.
  repo             The test repository name (just the repo, not
                   owner/repo). Required by --wipe-issues and
                   --wipe-labels.

Default mode removes the 6 unblock-managed custom fields from the project,
leaving the built-in Status field intact. Idempotent — safe to re-run.

Examples:
  scripts/setup-test-project.sh websublime 7
  scripts/setup-test-project.sh --check websublime 7
  scripts/setup-test-project.sh --wipe-issues websublime 7 unblock-test
  scripts/setup-test-project.sh --wipe-labels websublime unblock-test
EOF
}

require_tool() {
  local tool="$1"
  if ! command -v "$tool" >/dev/null 2>&1; then
    # shellcheck disable=SC2016 # backticks are Markdown-style code, not command substitution
    printf 'error: required tool `%s` not found in PATH\n' "$tool" >&2
    exit 2
  fi
}

# Resolves the GraphQL node id for the Projects V2 project owned by
# <owner> with number <project-number>. Echoes the node id on stdout.
# Exits with code 3 on API failure.
resolve_project_node_id() {
  local owner="$1"
  local project_number="$2"

  # The owner can be either a User or an Organization; the GraphQL
  # `repositoryOwner` field on Query resolves both. `projectV2(number:)`
  # is exposed on both ProjectV2Owner implementers (User and
  # Organization) and returns null when the project doesn't exist.
  local query
  query=$(cat <<'GRAPHQL'
query($owner: String!, $number: Int!) {
  repositoryOwner(login: $owner) {
    ... on ProjectV2Owner {
      projectV2(number: $number) {
        id
      }
    }
  }
}
GRAPHQL
)

  local response
  if ! response=$(gh api graphql \
      -f query="$query" \
      -F owner="$owner" \
      -F number="$project_number" 2>&1); then
    # shellcheck disable=SC2016 # backticks are Markdown-style code, not command substitution
    printf 'error: `gh api graphql` failed while resolving project node id:\n%s\n' "$response" >&2
    exit 3
  fi

  local node_id
  node_id=$(printf '%s' "$response" | jq -r '.data.repositoryOwner.projectV2.id // empty')
  if [[ -z "$node_id" ]]; then
    printf 'error: project %s/%s not found (no node id in GraphQL response)\n' \
      "$owner" "$project_number" >&2
    exit 3
  fi

  printf '%s' "$node_id"
}

# Fetches every fixture issue in <owner>/<repo> bearing the canonical
# fixture label, plus its corresponding ProjectV2Item id on
# <project_node_id> (when present). Echoes one tab-separated line per
# issue: `<issue_number>\t<issue_state>\t<project_item_id_or_-->`.
#
# Uses the GraphQL search API to paginate over 1000 issues max
# (timestamp_millis discriminator labels accumulate quickly in CI; raise
# this if a regression test ever exceeds 1000 issues, but practically a
# single CI run produces ~25 fixture issues so the cap is generous).
fetch_fixture_issues_with_items() {
  local owner="$1"
  local repo="$2"
  local project_node_id="$3"

  local query
  query=$(cat <<'GRAPHQL'
query($search: String!, $cursor: String) {
  search(query: $search, type: ISSUE, first: 100, after: $cursor) {
    pageInfo {
      hasNextPage
      endCursor
    }
    nodes {
      ... on Issue {
        number
        state
        projectItems(first: 50) {
          nodes {
            id
            project {
              id
            }
          }
        }
      }
    }
  }
}
GRAPHQL
)

  # Cycle 2 of bead `unblock-1hz` (review SUGGESTION-2): scope the search
  # to OPEN issues only. Closed fixture issues without project items are
  # the steady-state outcome of a successful prior wipe (`gh issue close`
  # does a REST PATCH state=closed; GitHub does not expose a delete-issue
  # API for non-admins) — there is nothing left to do for them and
  # iterating over the closed-issue tail grows linearly with the project's
  # lifetime, eating CI walltime for no work. Restricting the query to
  # `is:open` keeps the wipe O(open fixtures) regardless of total run
  # count. Any closed-but-still-on-board fixture from a pre-cycle-2 run
  # is a one-time backlog that an operator can clear by hand or by
  # widening this query temporarily.
  local search="repo:$owner/$repo label:$FIXTURE_LABEL is:issue is:open"
  local cursor="null"
  local total=0

  while true; do
    local response
    if [[ "$cursor" == "null" ]]; then
      if ! response=$(gh api graphql \
          -f query="$query" \
          -F search="$search" 2>&1); then
        # shellcheck disable=SC2016 # backticks are Markdown-style code, not command substitution
        printf 'error: `gh api graphql` failed while listing fixture issues:\n%s\n' "$response" >&2
        exit 3
      fi
    else
      if ! response=$(gh api graphql \
          -f query="$query" \
          -F search="$search" \
          -F cursor="$cursor" 2>&1); then
        # shellcheck disable=SC2016 # backticks are Markdown-style code, not command substitution
        printf 'error: `gh api graphql` failed while paginating fixture issues:\n%s\n' "$response" >&2
        exit 3
      fi
    fi

    # Per-issue: emit `<number>\t<state>\t<projectItemId-or-->`.
    # If the issue has multiple project item ids on the same project
    # (shouldn't happen but defensive), pick the first match. If the
    # issue is no longer attached to the target project, emit `-` so
    # the wipe still closes the issue.
    printf '%s' "$response" | jq -r --arg pid "$project_node_id" '
      .data.search.nodes[]
      | [
          .number,
          .state,
          (
            (.projectItems.nodes | map(select(.project.id == $pid)) | .[0].id)
            // "-"
          )
        ]
      | @tsv
    '

    local page_count
    page_count=$(printf '%s' "$response" | jq '.data.search.nodes | length')
    total=$((total + page_count))

    local has_next
    has_next=$(printf '%s' "$response" | jq -r '.data.search.pageInfo.hasNextPage')
    if [[ "$has_next" != "true" ]]; then
      break
    fi

    cursor=$(printf '%s' "$response" | jq -r '.data.search.pageInfo.endCursor')

    if [[ "$total" -ge 1000 ]]; then
      printf 'warning: reached 1000-issue safety cap; stopping pagination\n' >&2
      break
    fi
  done
}

# Removes a single ProjectV2Item by node id. Soft-fails (logs and
# returns 0) on per-item errors so a stale or already-deleted item does
# not abort the whole wipe.
delete_project_item() {
  local project_node_id="$1"
  local item_id="$2"

  local mutation
  mutation=$(cat <<'GRAPHQL'
mutation($projectId: ID!, $itemId: ID!) {
  deleteProjectV2Item(input: {projectId: $projectId, itemId: $itemId}) {
    deletedItemId
  }
}
GRAPHQL
)

  if ! gh api graphql \
      -f query="$mutation" \
      -F projectId="$project_node_id" \
      -F itemId="$item_id" \
      >/dev/null 2>&1; then
    printf '  warning: failed to delete project item id=%s (continuing)\n' "$item_id" >&2
    return 0
  fi
  return 0
}

# --wipe-issues mode entry point. Closes + de-cards every fixture issue
# in <owner>/<repo> on project <project_number>. Idempotent.
wipe_fixture_issues() {
  local owner="$1"
  local project_number="$2"
  local repo="$3"

  printf 'Resolving project node id for %s/%s...\n' "$owner" "$project_number" >&2
  local project_node_id
  project_node_id=$(resolve_project_node_id "$owner" "$project_number")

  # shellcheck disable=SC2016 # backticks are Markdown-style code, not command substitution
  printf 'Listing fixture issues in %s/%s with label `%s`...\n' \
    "$owner" "$repo" "$FIXTURE_LABEL" >&2

  local issues
  issues=$(fetch_fixture_issues_with_items "$owner" "$repo" "$project_node_id")

  if [[ -z "$issues" ]]; then
    printf 'No fixture issues present — project is already clean.\n' >&2
    return 0
  fi

  local closed=0
  local card_deleted=0
  local skipped=0

  while IFS=$'\t' read -r number state item_id; do
    [[ -z "$number" ]] && continue

    if [[ "$state" == "OPEN" ]]; then
      # Cycle 2 (bead `unblock-1hz` review SUGGESTION-3): no `--comment`.
      # The forensic note ("Automated test cleanup — …") was permanent
      # noise on every fixture issue's timeline — closure alone is
      # sufficient signal, and `gh issue close --reason completed` is
      # already self-documenting in the GitHub UI.
      if gh issue close "$number" \
          --repo "$owner/$repo" \
          --reason "completed" \
          >/dev/null 2>&1; then
        printf '  closed issue #%s\n' "$number" >&2
        closed=$((closed + 1))
      else
        printf '  warning: failed to close issue #%s (continuing)\n' "$number" >&2
      fi
    fi

    if [[ "$item_id" != "-" && -n "$item_id" ]]; then
      delete_project_item "$project_node_id" "$item_id"
      printf '  removed project item for issue #%s (item_id=%s)\n' "$number" "$item_id" >&2
      card_deleted=$((card_deleted + 1))
    else
      skipped=$((skipped + 1))
    fi
  done <<< "$issues"

  # Use `printf -- ...` so the leading '--' in the format string is not
  # parsed as a printf flag (some `printf` implementations choke on a
  # format that starts with two dashes).
  printf -- '--wipe-issues complete: closed %d open issue(s), removed %d project item(s), %d issue(s) had no project item.\n' \
    "$closed" "$card_deleted" "$skipped" >&2
  printf 'Note: project views are NOT cleaned up (no upstream delete-view API). Live tests reuse the 3 canonical fixture views.\n' >&2
}

# Returns 0 (true) if `name` is one of the CANONICAL_TEST_LABELS, else 1.
is_canonical_test_label() {
  local name="$1"
  local canonical
  for canonical in "${CANONICAL_TEST_LABELS[@]}"; do
    if [[ "$name" == "$canonical" ]]; then
      return 0
    fi
  done
  return 1
}

# Returns 0 (true) if `name` matches any pattern in WIPE_LABEL_PATTERNS,
# else 1. Patterns use bash's `[[ == ]]` glob (so `*` is the wildcard).
matches_wipe_pattern() {
  local name="$1"
  local pattern
  for pattern in "${WIPE_LABEL_PATTERNS[@]}"; do
    # shellcheck disable=SC2053 # right-hand side is intentionally a glob, not a literal
    if [[ "$name" == $pattern ]]; then
      return 0
    fi
  done
  return 1
}

# --wipe-labels mode entry point. Deletes orphan test-pattern labels from
# <owner>/<repo>, preserving the canonical labels and every production
# (non-test-pattern) label. Idempotent.
wipe_orphan_labels() {
  local owner="$1"
  local repo="$2"

  # shellcheck disable=SC2016 # backticks are Markdown-style code, not command substitution
  printf 'Listing labels on %s/%s for orphan-test-label cleanup...\n' "$owner" "$repo" >&2

  # `gh label list` paginates by default; --limit 1000 covers any
  # realistic test repo (the observed worst case is ~50 labels at
  # cycle-2 cleanup time).
  local labels_json
  if ! labels_json=$(gh label list --repo "$owner/$repo" --limit 1000 --json name 2>&1); then
    # shellcheck disable=SC2016 # backticks are Markdown-style code, not command substitution
    printf 'error: `gh label list` failed:\n%s\n' "$labels_json" >&2
    exit 3
  fi

  # Extract one label name per line.
  local all_names
  all_names=$(printf '%s' "$labels_json" | jq -r '.[].name')

  if [[ -z "$all_names" ]]; then
    printf 'No labels found on %s/%s.\n' "$owner" "$repo" >&2
    return 0
  fi

  local deleted=0
  local preserved_canonical=0
  local preserved_production=0

  while IFS= read -r name; do
    [[ -z "$name" ]] && continue

    if matches_wipe_pattern "$name"; then
      if is_canonical_test_label "$name"; then
        printf '  preserved canonical test label: %s\n' "$name" >&2
        preserved_canonical=$((preserved_canonical + 1))
        continue
      fi
      if gh label delete "$name" --repo "$owner/$repo" --yes >/dev/null 2>&1; then
        printf '  deleted orphan label: %s\n' "$name" >&2
        deleted=$((deleted + 1))
      else
        printf '  warning: failed to delete label %s (continuing)\n' "$name" >&2
      fi
    else
      preserved_production=$((preserved_production + 1))
    fi
  done <<< "$all_names"

  # Use `printf -- ...` so the leading '--' in the format string is not
  # parsed as a printf flag (some `printf` implementations choke on a
  # format that starts with two dashes).
  printf -- '--wipe-labels complete: deleted %d orphan label(s), preserved %d canonical test label(s) and %d production label(s).\n' \
    "$deleted" "$preserved_canonical" "$preserved_production" >&2
}

main() {
  local check_mode=0
  local wipe_issues_mode=0
  local wipe_labels_mode=0
  local positional=()

  while [[ $# -gt 0 ]]; do
    case "$1" in
      --check|--dry-run)
        check_mode=1
        shift
        ;;
      --wipe-issues)
        wipe_issues_mode=1
        shift
        ;;
      --wipe-labels)
        wipe_labels_mode=1
        shift
        ;;
      -h|--help)
        usage
        exit 0
        ;;
      --)
        shift
        while [[ $# -gt 0 ]]; do
          positional+=("$1")
          shift
        done
        ;;
      -*)
        printf 'error: unknown option %q\n' "$1" >&2
        usage
        exit 1
        ;;
      *)
        positional+=("$1")
        shift
        ;;
    esac
  done

  # The three mutating modes (--check, --wipe-issues, --wipe-labels) are
  # pairwise mutually exclusive. Cycle 2 of bead `unblock-1hz` keeps each
  # mode independent rather than introducing a combined `--all` flag —
  # see header comment for the rationale.
  local mode_count=$((check_mode + wipe_issues_mode + wipe_labels_mode))
  if [[ "$mode_count" -gt 1 ]]; then
    printf 'error: --check, --wipe-issues and --wipe-labels are mutually exclusive\n' >&2
    usage
    exit 1
  fi

  if [[ "$wipe_issues_mode" -eq 1 ]]; then
    if [[ ${#positional[@]} -ne 3 ]]; then
      printf 'error: --wipe-issues requires <owner> <project-number> <repo>\n' >&2
      usage
      exit 1
    fi
  elif [[ "$wipe_labels_mode" -eq 1 ]]; then
    if [[ ${#positional[@]} -ne 2 ]]; then
      printf 'error: --wipe-labels requires <owner> <repo>\n' >&2
      usage
      exit 1
    fi
  elif [[ ${#positional[@]} -ne 2 ]]; then
    usage
    exit 1
  fi

  local owner="${positional[0]}"

  require_tool gh
  require_tool jq

  if ! gh auth status >/dev/null 2>&1; then
    # shellcheck disable=SC2016 # backticks are Markdown-style code, not command substitution
    printf 'error: `gh` is not authenticated. Run `gh auth login` first.\n' >&2
    exit 3
  fi

  if [[ "$wipe_labels_mode" -eq 1 ]]; then
    # --wipe-labels takes <owner> <repo> only. The 2nd positional is the
    # repo (bare name), not a project number — so the project-number
    # numeric check below is intentionally skipped on this code path.
    local repo="${positional[1]}"
    if [[ -z "$repo" || "$repo" == */* ]]; then
      printf 'error: <repo> must be a bare repository name, not owner/repo (got %q)\n' "$repo" >&2
      usage
      exit 1
    fi
    wipe_orphan_labels "$owner" "$repo"
    exit 0
  fi

  local project_number="${positional[1]}"

  if ! [[ "$project_number" =~ ^[1-9][0-9]*$ ]]; then
    printf 'error: project-number must be a positive integer, got %q\n' "$project_number" >&2
    usage
    exit 1
  fi

  if [[ "$wipe_issues_mode" -eq 1 ]]; then
    local repo="${positional[2]}"
    if [[ -z "$repo" || "$repo" == */* ]]; then
      printf 'error: <repo> must be a bare repository name, not owner/repo (got %q)\n' "$repo" >&2
      usage
      exit 1
    fi
    wipe_fixture_issues "$owner" "$project_number" "$repo"
    exit 0
  fi

  if [[ "$check_mode" -eq 1 ]]; then
    printf 'Checking project %s/%s for stale unblock-managed custom fields...\n' \
      "$owner" "$project_number" >&2
  else
    printf 'Listing fields on project %s/%s...\n' "$owner" "$project_number" >&2
  fi

  # Build a jq filter that matches any field whose name appears in the
  # UNBLOCK_CUSTOM_FIELDS list. Built using `--argjson names` to avoid shell
  # quoting hazards.
  local names_json
  names_json=$(printf '%s\n' "${UNBLOCK_CUSTOM_FIELDS[@]}" | jq -R . | jq -s .)

  local fields_json
  if ! fields_json=$(gh project field-list "$project_number" \
      --owner "$owner" \
      --format json \
      --limit 100 2>&1); then
    # shellcheck disable=SC2016 # backticks are Markdown-style code, not command substitution
    printf 'error: `gh project field-list` failed:\n%s\n' "$fields_json" >&2
    exit 3
  fi

  # In --check mode we want both the id AND the name (for the report);
  # in mutate mode we only need the id. Always extract both, formatted as
  # tab-separated lines, so the same parsing logic serves both branches.
  local target_pairs
  target_pairs=$(printf '%s\n' "$fields_json" | jq -r \
    --argjson names "$names_json" \
    '.fields[] | select(.name as $n | $names | index($n)) | "\(.id)\t\(.name)"')

  if [[ -z "$target_pairs" ]]; then
    if [[ "$check_mode" -eq 1 ]]; then
      printf 'OK: project is clean — no unblock-managed custom fields present.\n' >&2
    else
      printf 'No unblock-managed custom fields present — project is already clean.\n' >&2
    fi
    exit 0
  fi

  if [[ "$check_mode" -eq 1 ]]; then
    printf 'DRIFT: the following unblock-managed custom fields are present:\n' >&2
    while IFS=$'\t' read -r field_id field_name; do
      [[ -z "$field_id" ]] && continue
      printf '  - %s (id=%s)\n' "$field_name" "$field_id" >&2
    done <<< "$target_pairs"
    printf 'Re-run without --check to delete them, or run scripts/setup-test-project.sh %s %s\n' \
      "$owner" "$project_number" >&2
    exit 4
  fi

  local deleted=0
  while IFS=$'\t' read -r field_id field_name; do
    [[ -z "$field_id" ]] && continue
    printf 'Deleting field %s (id=%s)\n' "$field_name" "$field_id" >&2
    if ! gh project field-delete --id "$field_id" >/dev/null 2>&1; then
      printf 'error: failed to delete field id=%s\n' "$field_id" >&2
      exit 3
    fi
    deleted=$((deleted + 1))
  done <<< "$target_pairs"

  printf 'Cleanup complete — deleted %d unblock-managed custom field(s).\n' "$deleted" >&2
  printf 'The built-in Status field was left intact (auto-healed by setup_fields).\n' >&2
}

main "$@"
