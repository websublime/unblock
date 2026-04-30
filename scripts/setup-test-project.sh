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
#   scripts/setup-test-project.sh <owner> <project-number>
#
#   Example:
#     scripts/setup-test-project.sh websublime 7
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
Usage: scripts/setup-test-project.sh <owner> <project-number>

  owner            GitHub org or user that owns the test project
  project-number   The Projects V2 project number (visible in the URL)

Removes the 6 unblock-managed custom fields from the project, leaving the
built-in Status field intact. Idempotent — safe to re-run.

Example:
  scripts/setup-test-project.sh websublime 7
EOF
}

require_tool() {
  local tool="$1"
  if ! command -v "$tool" >/dev/null 2>&1; then
    printf 'error: required tool `%s` not found in PATH\n' "$tool" >&2
    exit 2
  fi
}

main() {
  if [[ $# -ne 2 ]]; then
    usage
    exit 1
  fi

  local owner="$1"
  local project_number="$2"

  if ! [[ "$project_number" =~ ^[1-9][0-9]*$ ]]; then
    printf 'error: project-number must be a positive integer, got %q\n' "$project_number" >&2
    usage
    exit 1
  fi

  require_tool gh
  require_tool jq

  if ! gh auth status >/dev/null 2>&1; then
    printf 'error: `gh` is not authenticated. Run `gh auth login` first.\n' >&2
    exit 3
  fi

  printf 'Listing fields on project %s/%s...\n' "$owner" "$project_number" >&2

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
    printf 'error: `gh project field-list` failed:\n%s\n' "$fields_json" >&2
    exit 3
  fi

  local target_ids
  target_ids=$(printf '%s\n' "$fields_json" | jq -r \
    --argjson names "$names_json" \
    '.fields[] | select(.name as $n | $names | index($n)) | .id')

  if [[ -z "$target_ids" ]]; then
    printf 'No unblock-managed custom fields present — project is already clean.\n' >&2
    exit 0
  fi

  local deleted=0
  while IFS= read -r field_id; do
    [[ -z "$field_id" ]] && continue
    printf 'Deleting field id=%s\n' "$field_id" >&2
    if ! gh project field-delete --id "$field_id" >/dev/null 2>&1; then
      printf 'error: failed to delete field id=%s\n' "$field_id" >&2
      exit 3
    fi
    deleted=$((deleted + 1))
  done <<< "$target_ids"

  printf 'Cleanup complete — deleted %d unblock-managed custom field(s).\n' "$deleted" >&2
  printf 'The built-in Status field was left intact (auto-healed by setup_fields).\n' >&2
}

main "$@"
