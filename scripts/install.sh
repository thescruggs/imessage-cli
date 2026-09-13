#!/bin/sh
# One-line installer for imsg on macOS.
#
#   curl -fsSL https://github.com/thescruggs/imessage-cli/releases/latest/download/install.sh | sh
#
# Downloads the latest release package, then runs the package's installer,
# which puts imsg-server in ~/.local/bin and starts it at login.
# Environment: IMSG_VERSION=vX.Y.Z picks a release; IMSG_BIND=127.0.0.1:8787
# keeps the server local to this Mac; IMSG_REPO overrides the GitHub repo.
set -eu
REPO=${IMSG_REPO:-thescruggs/imessage-cli}
VERSION=${IMSG_VERSION:-latest}
ASSET=imsg-macos-universal.tar.gz
die() { echo "install: $*" >&2; exit 1; }
[ "$(uname -s)" = Darwin ] || die "imsg runs on macOS only"
[ "$(id -u)" -ne 0 ] || die "do not run with sudo; imsg installs for the current user"
command -v curl >/dev/null 2>&1 || die "curl was not found"
command -v tar >/dev/null 2>&1 || die "tar was not found"
if [ "$VERSION" = latest ]; then
    URL=https://github.com/$REPO/releases/latest/download/$ASSET
else
    URL=https://github.com/$REPO/releases/download/$VERSION/$ASSET
fi
TMP=$(mktemp -d "${TMPDIR:-/tmp}/imsg-install.XXXXXX") || die "could not create a temporary directory"
trap 'rm -rf "$TMP"' 0
trap 'exit 1' 1 2 3 15
echo "Downloading imsg ($VERSION) from github.com/$REPO"
curl -fsSL --proto '=https' -o "$TMP/$ASSET" "$URL" || die "download failed: $URL"
tar -xzf "$TMP/$ASSET" -C "$TMP" || die "could not unpack $ASSET"
INSTALLER=$(find "$TMP" -name install-server.sh -type f | head -n 1)
[ -n "$INSTALLER" ] || die "the package does not contain install-server.sh"
sh "$INSTALLER" --open-settings "$@"
