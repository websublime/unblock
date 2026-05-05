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
#   - Fetches every issue in <owner>/<repo> bearing the canonical
#     fixture label `unblock-fixture` (label applied automatically by
#     the live tests' `fixture_labels()` helper — see bead unblock-1hz).
#   - For each fixture issue:
#       * If currently OPEN, closes it via `gh issue close` so live tests
#         do not leave an orphaned-open trail when the panic-path Drop
#         guard's tokio::spawn cleanup loses its runtime race (bead
#         unblock-ekf documented best-effort caveat).
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
#   - --wipe-issues mode: closes already-closed issues silently (gh issue
#     close on a closed issue returns success); deleteProjectV2Item on a
#     missing item returns a structured error which we log and continue.
#   - Re-running after a successful `cargo test ... -- --ignored` round
#     trip is safe in either mode.
#
# USAGE
#   scripts/setup-test-project.sh [--check] <owner> <project-number>
#   scripts/setup-test-project.sh --wipe-issues <owner> <project-number> <repo>
#
#   Default mode mutates the project (deletes stale custom fields).
#
#   --check / --dry-run mode prints what WOULD be deleted (custom-fields
#   only) without mutating, and exits non-zero if any unblock-managed
#   custom fields are present (exit 4). Useful as a CI preflight
#   assertion that the project is in the canonical clean state before
#   live tests run. --check has no effect on issues; pair with
#   --wipe-issues only when you intend to mutate.
#
#   --wipe-issues mode requires a 3rd positional argument <repo> (just
#   the repository name, not owner/repo). The mode operates on the
#   issues in <owner>/<repo> with the canonical fixture label. Default
#   mode is NOT executed in --wipe-issues mode — to do both, run the
#   script twice. Keeping the modes orthogonal preserves the existing
#   --check semantics on the custom-fields path.
#
#   Examples:
#     scripts/setup-test-project.sh websublime 7
#     scripts/setup-test-project.sh --check websublime 7
#     scripts/setup-test-project.sh --dry-run websublime 7
#     scripts/setup-test-project.sh --wipe-issues websublime 7 unblock-test
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

usage() {
  cat <<'EOF' >&2
Usage:
  scripts/setup-test-project.sh [--check] <owner> <project-number>
  scripts/setup-test-project.sh --wipe-issues <owner> <project-number> <repo>

  --check          Dry-run for default (custom-fields) mode: list
                   (without deleting) any stale unblock-managed custom
                   fields. Exits 0 when the project is already clean,
                   exits 4 when stale fields are present.
  --dry-run        Alias for --check.
  --wipe-issues    Closes every issue in <owner>/<repo> with the
                   `unblock-fixture` label and removes its corresponding
                   project item from <project-number>. View cleanup is
                   not automated (upstream API constraint).
  owner            GitHub org or user that owns the test project
  project-number   The Projects V2 project number (visible in the URL)
  repo             The test repository name (just the repo, not
                   owner/repo). Required only with --wipe-issues.

Default mode removes the 6 unblock-managed custom fields from the project,
leaving the built-in Status field intact. Idempotent — safe to re-run.

Examples:
  scripts/setup-test-project.sh websublime 7
  scripts/setup-test-project.sh --check websublime 7
  scripts/setup-test-project.sh --wipe-issues websublime 7 unblock-test
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

  local search="repo:$owner/$repo label:$FIXTURE_LABEL is:issue"
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
      if gh issue close "$number" \
          --repo "$owner/$repo" \
          --reason "completed" \
          --comment "Automated test cleanup — wiped by scripts/setup-test-project.sh --wipe-issues" \
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

main() {
  local check_mode=0
  local wipe_issues_mode=0
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

  if [[ "$check_mode" -eq 1 && "$wipe_issues_mode" -eq 1 ]]; then
    printf 'error: --check and --wipe-issues are mutually exclusive\n' >&2
    usage
    exit 1
  fi

  if [[ "$wipe_issues_mode" -eq 1 ]]; then
    if [[ ${#positional[@]} -ne 3 ]]; then
      printf 'error: --wipe-issues requires <owner> <project-number> <repo>\n' >&2
      usage
      exit 1
    fi
  elif [[ ${#positional[@]} -ne 2 ]]; then
    usage
    exit 1
  fi

  local owner="${positional[0]}"
  local project_number="${positional[1]}"

  if ! [[ "$project_number" =~ ^[1-9][0-9]*$ ]]; then
    printf 'error: project-number must be a positive integer, got %q\n' "$project_number" >&2
    usage
    exit 1
  fi

  require_tool gh
  require_tool jq

  if ! gh auth status >/dev/null 2>&1; then
    # shellcheck disable=SC2016 # backticks are Markdown-style code, not command substitution
    printf 'error: `gh` is not authenticated. Run `gh auth login` first.\n' >&2
    exit 3
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
