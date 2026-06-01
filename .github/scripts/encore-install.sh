#!/bin/sh
# ─────────────────────────────────────────────────────────────────────
# VENDORED COPY — do not curl this live from CI (tv8.67).
#
# Upstream:  https://encore.dev/install.sh  (Encore's own CDN; served as
#            application/x-sh, NOT a GitHub-hosted file, so there is no
#            commit SHA to pin a `curl | bash` against — vendoring is the
#            only way to freeze the supply-chain surface).
# Vendored:  2026-06-01.
# sha256:    of the upstream bytes captured on the vendor date is recorded
#            in .github/workflows/apps-api-ci.yml's "install encore cli"
#            step comment. The CLI *binary* version is still pinned at the
#            call site via the ${ENCORE_CLI_VERSION} argument (this script
#            only constructs the CloudFront tarball URL from it).
#
# Refresh procedure (when bumping the Encore CLI or picking up an upstream
# installer fix):
#   1. curl -sSfL https://encore.dev/install.sh -o .github/scripts/encore-install.sh
#   2. Re-apply this vendoring header (git diff to confirm only intended
#      upstream changes landed — review the diff line-by-line).
#   3. Update the vendor date above and the sha256 note in the workflow.
# ─────────────────────────────────────────────────────────────────────
# Based on Deno installer: Copyright 2019 the Deno authors. All rights reserved. MIT license.
# TODO(everyone): Keep this script simple and easily auditable.

# Script will install the latest Encore release by default.
# If a version is provided, it will install the specified version.
# Example: curl -L https://encore.dev/install.sh | bash -s -- 1.50.0

set -e

version=$1

case $(uname -sm) in
	"Darwin x86_64") target="darwin_amd64" ;;
	"Darwin arm64")  target="darwin_arm64" ;;
	"Darwin arm64")  target="darwin_arm64" ;;
	"Linux aarch64") target="linux_arm64"  ;;
	"Linux arm64")   target="linux_arm64"  ;;
	*) target="linux_amd64" ;;
esac

if [ -z "$version" ]; then
  encore_uri=$(curl -sSf -N "https://encore.dev/api/releases?target=${target}&show=url")
  if [ ! "$encore_uri" ]; then
    echo "Error: Unable to determine latest Encore release." 1>&2
    exit 1
  fi
else
  encore_uri="https://d2f391esomvqpi.cloudfront.net/encore-${version}-${target}.tar.gz"
fi

encore_install="${ENCORE_INSTALL:-$HOME/.encore}"

bin_dir="$encore_install/bin"
exe="$bin_dir/encore"
tar="$encore_install/encore.tar.gz"

if [ ! -d "$bin_dir" ]; then
 	mkdir -p "$bin_dir"
fi

curl --fail --location --progress-bar --output "$tar" "$encore_uri"
cd "$encore_install"

# If encore-go already exists, delete it.
# Merging multiple Go releases into the same directory
# leads to very difficult-to-understand fatal errors.
if [ -d "./encore-go" ]; then
	rm -rf "./encore-go"
fi

# Same goes for runtime
if [ -d "./runtimes" ]; then
	rm -rf "./runtimes"
fi

tar -C "$encore_install" -xzf "$tar"
chmod +x "$bin_dir"/*
rm "$tar"

"$exe" version

echo "Encore was installed successfully to $exe"
if command -v encore >/dev/null; then
	echo "Run 'encore --help' to get started"
else
	case $SHELL in
	/bin/zsh) shell_profile=".zshrc" ;;
	*) shell_profile=".bash_profile" ;;
	esac
	echo "Manually add the directory to your \$HOME/$shell_profile (or similar)"
	echo "  export ENCORE_INSTALL=\"$encore_install\""
	echo "  export PATH=\"\$ENCORE_INSTALL/bin:\$PATH\""
	echo "Run '$exe --help' to get started"
fi
