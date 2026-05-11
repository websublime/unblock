#!/usr/bin/env bash
# Sync GitHub labels across one repo or every repo in an org.
#
# Usage:
#   ./sync-labels.sh --repo OWNER/REPO [--dry-run] [--no-renames] [--no-deletions]
#   ./sync-labels.sh --org  OWNER       [--dry-run] [--no-renames] [--no-deletions]
#                                       [--include-archived] [--include-forks]
#                                       [--exclude REPO,REPO,...]
#
# Requires: gh, jq

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SPEC_FILE="${SPEC_FILE:-$SCRIPT_DIR/labels.json}"

DRY_RUN=false
DO_RENAMES=true
DO_DELETIONS=true
INCLUDE_ARCHIVED=false
INCLUDE_FORKS=false
ORG=""
REPO=""
EXCLUDE=""

usage() {
  sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
  exit "${1:-0}"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo)              REPO="$2"; shift 2 ;;
    --org)               ORG="$2"; shift 2 ;;
    --spec)              SPEC_FILE="$2"; shift 2 ;;
    --dry-run)           DRY_RUN=true; shift ;;
    --no-renames)        DO_RENAMES=false; shift ;;
    --no-deletions)      DO_DELETIONS=false; shift ;;
    --include-archived)  INCLUDE_ARCHIVED=true; shift ;;
    --include-forks)     INCLUDE_FORKS=true; shift ;;
    --exclude)           EXCLUDE="$2"; shift 2 ;;
    -h|--help)           usage 0 ;;
    *) echo "Unknown arg: $1" >&2; usage 1 ;;
  esac
done

# --- Validation ----------------------------------------------------------------

command -v gh >/dev/null || { echo "❌ gh CLI not found" >&2; exit 1; }
command -v jq >/dev/null || { echo "❌ jq not found" >&2; exit 1; }

if [[ -z "$REPO" && -z "$ORG" ]]; then
  echo "❌ Either --repo or --org is required" >&2
  usage 1
fi

if [[ ! -f "$SPEC_FILE" ]]; then
  echo "❌ Spec file not found: $SPEC_FILE" >&2
  exit 1
fi

# --- Output helpers ------------------------------------------------------------

if [[ -t 1 ]]; then
  C_BOLD=$'\033[1m'; C_DIM=$'\033[2m'; C_RESET=$'\033[0m'
  C_GREEN=$'\033[32m'; C_YELLOW=$'\033[33m'; C_RED=$'\033[31m'; C_BLUE=$'\033[34m'
else
  C_BOLD=""; C_DIM=""; C_RESET=""; C_GREEN=""; C_YELLOW=""; C_RED=""; C_BLUE=""
fi

run() {
  if $DRY_RUN; then
    echo "      ${C_DIM}[dry-run]${C_RESET} $*"
    return 0
  fi
  if "$@"; then
    return 0
  else
    echo "      ${C_RED}⚠ command failed:${C_RESET} $*" >&2
    return 1
  fi
}

# --- Discover target repos -----------------------------------------------------

REPOS=()
if [[ -n "$REPO" ]]; then
  REPOS=("$REPO")
else
  echo "${C_BOLD}Listing repos in org $ORG...${C_RESET}"
  GH_FILTER='.[] | select(true'
  $INCLUDE_ARCHIVED || GH_FILTER="${GH_FILTER} and (.isArchived | not)"
  $INCLUDE_FORKS    || GH_FILTER="${GH_FILTER} and (.isFork | not)"
  GH_FILTER="${GH_FILTER}) | .nameWithOwner"

  while IFS= read -r line; do
    [[ -z "$line" ]] && continue
    REPOS+=("$line")
  done < <(
    gh repo list "$ORG" --limit 1000 \
      --json nameWithOwner,isArchived,isFork \
      --jq "$GH_FILTER" \
    | sort
  )

  if [[ -n "$EXCLUDE" && ${#REPOS[@]} -gt 0 ]]; then
    IFS=',' read -r -a EX_LIST <<< "$EXCLUDE"
    FILTERED=()
    for r in "${REPOS[@]}"; do
      skip=false
      for x in "${EX_LIST[@]}"; do
        if [[ "$r" == "$ORG/$x" || "$r" == "$x" ]]; then
          skip=true
          break
        fi
      done
      $skip || FILTERED+=("$r")
    done
    REPOS=("${FILTERED[@]}")
  fi
fi

if [[ ${#REPOS[@]} -eq 0 ]]; then
  echo "${C_YELLOW}No repos to process${C_RESET}"
  exit 0
fi

echo "${C_BOLD}Targets:${C_RESET} ${#REPOS[@]} repo(s)"
$DRY_RUN && echo "${C_YELLOW}Dry-run mode — no changes will be applied${C_RESET}"
echo

# --- Per-repo summary counters -------------------------------------------------

TOTAL_RENAMED=0
TOTAL_UPSERTED=0
TOTAL_DELETED=0
TOTAL_FAILED_REPOS=0

# --- Process each repo ---------------------------------------------------------

process_repo() {
  local repo="$1"
  echo "${C_BOLD}${C_BLUE}═══ $repo ═══${C_RESET}"

  # Snapshot existing labels (name only)
  local existing_json
  if ! existing_json=$(gh label list -R "$repo" --limit 200 --json name 2>&1); then
    echo "  ${C_RED}❌ Could not list labels (no access?):${C_RESET} $existing_json"
    TOTAL_FAILED_REPOS=$((TOTAL_FAILED_REPOS + 1))
    return
  fi
  local existing
  existing=$(jq -r '.[].name' <<< "$existing_json")

  # 1. Renames -----------------------------------------------------------------
  if $DO_RENAMES; then
    echo "  ${C_BOLD}→ Renames${C_RESET}"
    local n_renames=0
    while IFS=$'\t' read -r FROM TO; do
      [[ -z "$FROM" ]] && continue
      if grep -Fxq "$FROM" <<< "$existing"; then
        if grep -Fxq "$TO" <<< "$existing"; then
          # Target already exists → cannot rename, will let upsert refresh and the
          # old name will be picked up by deletions step (if listed) or left alone
          echo "    ${C_YELLOW}skip (target exists):${C_RESET} $FROM → $TO"
        else
          echo "    ${C_GREEN}$FROM${C_RESET} → ${C_GREEN}$TO${C_RESET}"
          if run gh label edit "$FROM" -R "$repo" --name "$TO"; then
            n_renames=$((n_renames + 1))
            # Update local snapshot so upserts see the new name
            existing=$(echo "$existing" | sed "s|^${FROM}\$|${TO}|")
          fi
        fi
      fi
    done < <(jq -r '.renames[] | "\(.from)\t\(.to)"' "$SPEC_FILE")
    [[ $n_renames -eq 0 ]] && echo "    ${C_DIM}(nothing to rename)${C_RESET}"
    TOTAL_RENAMED=$((TOTAL_RENAMED + n_renames))
  fi

  # 2. Upsert canonical labels -------------------------------------------------
  echo "  ${C_BOLD}→ Apply spec${C_RESET}"
  local n_upserts=0
  while IFS=$'\t' read -r NAME COLOR DESC; do
    [[ -z "$NAME" ]] && continue
    echo "    ${C_GREEN}upsert${C_RESET} $NAME"
    if run gh label create "$NAME" -R "$repo" \
        --color "$COLOR" \
        --description "$DESC" \
        --force; then
      n_upserts=$((n_upserts + 1))
    fi
  done < <(jq -r '.labels[] | "\(.name)\t\(.color)\t\(.description)"' "$SPEC_FILE")
  TOTAL_UPSERTED=$((TOTAL_UPSERTED + n_upserts))

  # 3. Explicit deletions ------------------------------------------------------
  if $DO_DELETIONS; then
    echo "  ${C_BOLD}→ Deletions${C_RESET}"
    local n_deletes=0
    while read -r LABEL; do
      [[ -z "$LABEL" ]] && continue
      if grep -Fxq "$LABEL" <<< "$existing"; then
        echo "    ${C_RED}delete${C_RESET} $LABEL"
        if run gh label delete "$LABEL" -R "$repo" --yes; then
          n_deletes=$((n_deletes + 1))
        fi
      fi
    done < <(jq -r '.deletions[]' "$SPEC_FILE")
    [[ $n_deletes -eq 0 ]] && echo "    ${C_DIM}(nothing to delete)${C_RESET}"
    TOTAL_DELETED=$((TOTAL_DELETED + n_deletes))
  fi

  echo
}

for r in "${REPOS[@]}"; do
  process_repo "$r"
done

# --- Summary -------------------------------------------------------------------

echo "${C_BOLD}═══ Summary ═══${C_RESET}"
echo "  Repos processed: ${#REPOS[@]}"
echo "  Renames:         $TOTAL_RENAMED"
echo "  Upserts:         $TOTAL_UPSERTED"
echo "  Deletions:       $TOTAL_DELETED"
if [[ $TOTAL_FAILED_REPOS -gt 0 ]]; then
  echo "  ${C_RED}Failed repos:    $TOTAL_FAILED_REPOS${C_RESET}"
fi
if $DRY_RUN; then
  echo "${C_YELLOW}(dry-run — nothing was actually changed)${C_RESET}"
fi
exit $(( TOTAL_FAILED_REPOS > 0 ? 1 : 0 ))
