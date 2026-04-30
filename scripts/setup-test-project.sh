#!/usr/bin/env bash
# scripts/setup-test-project.sh
#
# Idempotently bootstraps the unblock-test GitHub Projects V2 project to a
# clean state for the live integration test job (.github/workflows/ci.yml
# `test-mcp-live`).
#
# WHAT IT DOES
#   - Lists every custom field on the target project via `gh project
#     field-list`.
#   - Deletes any field whose name is one of the 6 unblock-managed custom
#     fields: Priority, PipelineStage, Agent, ClaimedAt, StoryPoints,
#     DeferUntil.
#   - Leaves the project's built-in `Status` field intact.
#     `setup_fields()` in the Rust code-base auto-heals the built-in
#     Status options to the spec's canonical set
#     (ready/in_progress/blocked/deferred/closed) on every run, so the
#     script has nothing to do here.
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
#   - When all 6 fields are absent, the script is a no-op (empty input
#     pipeline).
#   - When some are present, only the present ones are deleted; subsequent
#     runs are no-ops.
#   - Re-running after a successful `cargo test ... -- --ignored` round
#     trip is safe.
#
# USAGE
#   scripts/setup-test-project.sh [--check] <owner> <project-number>
#
#   Default mode mutates the project (deletes stale custom fields).
#
#   --check / --dry-run mode prints what WOULD be deleted without
#   mutating, and exits non-zero if any unblock-managed custom fields are
#   present (exit 4). Useful as a CI preflight assertion that the project
#   is in the canonical clean state before live tests run.
#
#   Examples:
#     scripts/setup-test-project.sh websublime 7
#     scripts/setup-test-project.sh --check websublime 7
#     scripts/setup-test-project.sh --dry-run websublime 7
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

usage() {
  cat <<'EOF' >&2
Usage: scripts/setup-test-project.sh [--check] <owner> <project-number>

  --check          Dry-run: list (without deleting) any stale unblock-
                   managed custom fields. Exits 0 when the project is
                   already clean, exits 4 when stale fields are present.
  --dry-run        Alias for --check.
  owner            GitHub org or user that owns the test project
  project-number   The Projects V2 project number (visible in the URL)

Default mode removes the 6 unblock-managed custom fields from the project,
leaving the built-in Status field intact. Idempotent — safe to re-run.

Examples:
  scripts/setup-test-project.sh websublime 7
  scripts/setup-test-project.sh --check websublime 7
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

main() {
  local check_mode=0
  local positional=()

  while [[ $# -gt 0 ]]; do
    case "$1" in
      --check|--dry-run)
        check_mode=1
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

  if [[ ${#positional[@]} -ne 2 ]]; then
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
