#!/bin/sh
# update-smoke.sh — the LIVE `unblock update` end-to-end smoke (FR-25 / NFR-17; spec:
# docs/plans/ci-cd-and-distribution.md §4; tracked as ub-lp9.26). Unix half; the Windows half is
# update-smoke.ps1 beside it.
#
# WHAT IT PROVES (and what crates/unblock-cli/tests/update_verify.rs cannot): against two REAL
# published GitHub releases, a binary installed by the REAL dist shell installer at <from-tag> carries a
# receipt the shipped command loads; `unblock update --dry-run` resolves the real release source and
# reports <to-tag> WITHOUT swapping (byte-identical binary); `unblock update` downloads, lets the dist
# installer verify the SHA256 against dist-manifest.json, and swaps; the swapped binary reports <to-tag>
# and still serves `version`, `migrate` and `doctor` on a workspace the OLD binary created.
#
# NON-GOALS (recorded, not assumed): it does not re-test dist's own SHA256 implementation (dist's suite
# covers it) and it does not put attestation verification on the update path — attestations stay
# publish-side provenance, verified out-of-band with `gh attestation verify` (NFR-17).
#
# NETWORK: this script talks to github.com by design. It is NEVER part of the per-PR gate and is not
# Rust code, so the workspace `no-network` scan is untouched; it runs only from the manual
# `.github/workflows/update-smoke.yml` dispatch or by hand per RELEASING.md.
#
# ISOLATION: everything lands in one temp dir — install prefix (UNBLOCK_CLI_INSTALL_DIR), receipt
# (XDG_CONFIG_HOME, which both the installer and axoupdater honour), and the scratch workspace. PATH and
# shell rc files are never modified (UNBLOCK_CLI_NO_MODIFY_PATH=1), so a maintainer's real install is
# untouched.
#
# PRECONDITION: <to-tag> must be the LATEST STABLE release — `unblock update` always targets latest.
#
# Usage:  scripts/release/update-smoke.sh <from-tag> <to-tag>      e.g.  v1.0.0 v1.0.1
# Env:    UNBLOCK_SMOKE_REPO       owner/repo of the release source (default websublime/unblock)
#         AXOUPDATER_GITHUB_TOKEN  optional; authenticates the release query against GitHub's per-IP
#                                  rate limit (ci-cd §4) — only when <from-tag> already reads it
#                                  (ub-jh5); v1.0.0 and v1.0.1 ignore it
#         UNBLOCK_SMOKE_KEEP=1     keep the temp dir for inspection
# Exit:   0 = every step passed · 1 = a smoke assertion failed · 2 = bad usage / missing tool.
set -u

say() { printf 'update-smoke: %s\n' "$*" >&2; }
fail() { say "FAIL: $*"; exit 1; }

[ "$#" -eq 2 ] || { say "usage: $0 <from-tag> <to-tag>"; exit 2; }
FROM_TAG=$1
TO_TAG=$2
FROM_VER=${FROM_TAG#v}
TO_VER=${TO_TAG#v}
REPO=${UNBLOCK_SMOKE_REPO:-websublime/unblock}

command -v curl >/dev/null 2>&1 || { say "curl is required"; exit 2; }
if command -v sha256sum >/dev/null 2>&1; then
    sha256() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
    sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
    say "sha256sum or shasum is required"
    exit 2
fi

ROOT=$(mktemp -d "${TMPDIR:-/tmp}/unblock-update-smoke.XXXXXX") || { say "mktemp failed"; exit 2; }
if [ "${UNBLOCK_SMOKE_KEEP:-0}" = "1" ]; then
    say "keeping $ROOT"
else
    trap 'rm -rf "$ROOT"' EXIT
fi

export UNBLOCK_CLI_INSTALL_DIR="$ROOT/prefix"
export UNBLOCK_CLI_NO_MODIFY_PATH=1
export XDG_CONFIG_HOME="$ROOT/config"
# A stray workspace/actor override from the caller's shell must not leak into the scratch workspace.
unset UNBLOCK_DIR UNBLOCK_ACTOR UNBLOCK_OUTPUT_FORMAT
BIN="$UNBLOCK_CLI_INSTALL_DIR/bin/unblock"
RECEIPT="$XDG_CONFIG_HOME/unblock-cli/unblock-cli-receipt.json"
WS="$ROOT/ws"
OUT="$ROOT/out"
ERR="$ROOT/err"

# run <cmd...> — echo the command, run it with stdout/stderr captured, replay both, return its status.
run() {
    printf '\n$ %s\n' "$*"
    "$@" >"$OUT" 2>"$ERR"
    status=$?
    sed 's/^/  [stdout] /' "$OUT"
    sed 's/^/  [stderr] /' "$ERR"
    printf '  [exit] %s\n' "$status"
    return "$status"
}

say "repo=$REPO from=$FROM_TAG to=$TO_TAG host=$(uname -sm)"

# 1. Install <from-tag> with the REAL dist shell installer, so a genuine receipt exists.
INSTALLER_URL="https://github.com/$REPO/releases/download/$FROM_TAG/unblock-cli-installer.sh"
run curl --proto '=https' --tlsv1.2 -fsSL -o "$ROOT/installer.sh" "$INSTALLER_URL" \
    || fail "cannot download $INSTALLER_URL"
run sh "$ROOT/installer.sh" || fail "the $FROM_TAG installer exited non-zero"
[ -x "$BIN" ] || fail "installer did not place an executable at $BIN"
[ -f "$RECEIPT" ] || fail "installer wrote no receipt at $RECEIPT"
printf '\nreceipt (%s):\n' "$RECEIPT"
sed 's/^/  /' "$RECEIPT"
printf '\n'

run "$BIN" version || fail "installed $FROM_TAG cannot run 'version'"
run "$BIN" version --short || fail "installed $FROM_TAG cannot run 'version --short'"
[ "$(cat "$OUT")" = "$FROM_VER" ] || fail "installed binary reports '$(cat "$OUT")', expected $FROM_VER"

# A workspace created by the OLD binary, so the post-swap binary is exercised against real old state.
mkdir -p "$WS"
(cd "$WS" && run "$BIN" init) || fail "$FROM_TAG cannot 'init' a workspace"
(cd "$WS" && run "$BIN" doctor) || fail "$FROM_TAG 'doctor' failed on its own fresh workspace"

SHA_BEFORE=$(sha256 "$BIN")
say "sha256 before: $SHA_BEFORE"

# 2. --dry-run: resolves the real release source, reports <to-tag>, swaps NOTHING.
run "$BIN" update --dry-run || fail "'update --dry-run' exited non-zero"
grep -qF "update available: $TO_VER" "$ERR" \
    || fail "'update --dry-run' did not report $TO_VER (is $TO_TAG the latest STABLE release of $REPO?)"
[ "$(sha256 "$BIN")" = "$SHA_BEFORE" ] || fail "'update --dry-run' modified the binary"
say "dry-run: reported $TO_VER, binary byte-identical"

# 3. The real update: download + dist installer SHA256 verify + swap.
run "$BIN" update || fail "'update' exited non-zero"
grep -qF "updated to $TO_TAG" "$ERR" || fail "'update' did not report 'updated to $TO_TAG'"
SHA_AFTER=$(sha256 "$BIN")
say "sha256 after:  $SHA_AFTER"
[ "$SHA_AFTER" != "$SHA_BEFORE" ] || fail "'update' reported success but the binary is unchanged"
grep -qF "\"version\":\"$TO_VER\"" "$RECEIPT" || fail "receipt does not record version $TO_VER after the swap"

# 4. The swapped-in binary is <to-tag> and still serves real commands.
run "$BIN" version || fail "swapped binary cannot run 'version'"
run "$BIN" version --short || fail "swapped binary cannot run 'version --short'"
[ "$(cat "$OUT")" = "$TO_VER" ] || fail "swapped binary reports '$(cat "$OUT")', expected $TO_VER"
(cd "$WS" && run "$BIN" migrate) || fail "$TO_TAG 'migrate' failed on the $FROM_TAG workspace"
(cd "$WS" && run "$BIN" doctor) || fail "$TO_TAG 'doctor' failed on the $FROM_TAG workspace"
run "$BIN" update --dry-run || fail "post-swap 'update --dry-run' exited non-zero"
grep -qF "already up to date" "$ERR" || fail "post-swap 'update --dry-run' does not report up to date"

say "PASS: $FROM_TAG -> $TO_TAG via the real dist installer on $(uname -sm)"
