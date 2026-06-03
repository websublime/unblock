#!/usr/bin/env bash
# secrets-provision.sh — provision the NON-PRODUCTION Encore Platform secrets
# for the `local` and `pr` (preview) environment types from the single
# committed source of truth, apps/api/secrets.nonprod.cue.
#
# Bead unblock-f6z. See apps/api/SECRETS.md for the full runbook.
#
# WHAT THIS DOES
# -------------------------------------------------------------------------
# Reads every `<GoField>: "<value>"` line from secrets.nonprod.cue and runs
#   encore secret set --type local,pr <GoField>
# piping the placeholder value via stdin. This makes the Encore Platform's
# local + pr secret matrix match the committed SoT in one idempotent command,
# so a preview-environment deploy boots without tripping the auth.init() /
# mcp/transport.go fail-fast guards.
#
# WHAT THIS DOES NOT DO (by design)
# -------------------------------------------------------------------------
#   - Does NOT touch `--type prod` or `--type dev`. Those carry REAL
#     credentials and are set by a human on the Encore Platform ONLY. This
#     script would clobber them with placeholders — so it refuses to.
#   - Does NOT write apps/api/.secrets.local.cue. For local emulator runs,
#     copy the SoT instead:  cp secrets.nonprod.cue .secrets.local.cue
#     (the CI workflow does exactly this). `encore secret set --type local`
#     populates the PLATFORM local env type, which is separate from the
#     on-disk .secrets.local.cue override the local emulator reads.
#
# IDEMPOTENCY
# -------------------------------------------------------------------------
# `encore secret set` overwrites the existing value for the named types, so
# re-running this script converges to the SoT every time with no side effects
# beyond setting the same placeholder values again.
#
# PREREQUISITES
# -------------------------------------------------------------------------
#   - `encore` CLI on PATH, authenticated (`encore auth login`), and the
#     workspace linked to the cloud app (committed apps/api/encore.app).
#   - Run from anywhere — the script resolves paths relative to its own
#     location (apps/api/scripts/), so the encore commands run in apps/api/.

set -euo pipefail

# Resolve apps/api/ (the encore app root) from this script's location,
# independent of the caller's cwd.
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
api_dir="$(cd "${script_dir}/.." && pwd)"
sot_file="${api_dir}/secrets.nonprod.cue"

if [[ ! -f "${sot_file}" ]]; then
  echo "error: source of truth not found at ${sot_file}" >&2
  exit 1
fi

if ! command -v encore >/dev/null 2>&1; then
  echo "error: 'encore' CLI not found on PATH — install it and run 'encore auth login' first" >&2
  exit 1
fi

cd "${api_dir}"

# Non-prod types ONLY. prod/dev are human-set on the platform and MUST NOT
# be clobbered with placeholders.
types="local,pr"

count=0
# Match CUE lines of the form:  FieldName: "value"
# - field: a leading identifier (Go PascalCase, matches the secrets struct).
# - value: everything inside the first pair of double quotes.
# Comment lines (starting with //) never match because they don't begin with
# an identifier immediately followed by a colon at column 0.
while IFS= read -r line; do
  if [[ "${line}" =~ ^([A-Za-z_][A-Za-z0-9_]*)[[:space:]]*:[[:space:]]*\"(.*)\"[[:space:]]*$ ]]; then
    field="${BASH_REMATCH[1]}"
    value="${BASH_REMATCH[2]}"
    echo "-> encore secret set --type ${types} ${field}"
    # printf (no trailing newline) — encore strips trailing newlines anyway,
    # but this keeps the piped value byte-exact with the SoT.
    printf '%s' "${value}" | encore secret set --type "${types}" "${field}"
    count=$((count + 1))
  fi
done < "${sot_file}"

if [[ "${count}" -eq 0 ]]; then
  echo "error: no secret fields parsed from ${sot_file} — check its format" >&2
  exit 1
fi

echo "[ok] provisioned ${count} non-prod secret(s) for types: ${types}"
echo "  (prod/dev untouched — set those on the Encore Platform manually)"
