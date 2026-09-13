#!/bin/sh
# Build a universal (Apple Silicon + Intel) macOS release package into dist/.
#
# Produces:
#   dist/imsg-macos-universal.tar.gz   binaries + installer, what install.sh downloads
#   dist/install.sh                    the one-line bootstrap installer
#   dist/SHA256SUMS
set -eu
ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd -P)
cd "$ROOT"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
[ -n "$VERSION" ] || { echo "package: could not read version from Cargo.toml" >&2; exit 1; }
if ! command -v cargo >/dev/null 2>&1; then
    for candidate in "$HOME/.cargo/bin" "$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin" /opt/homebrew/opt/rustup/bin; do
        [ -x "$candidate/cargo" ] && PATH=$candidate:$PATH
    done
fi
export PATH
TARGETS="aarch64-apple-darwin x86_64-apple-darwin"
for t in $TARGETS; do
    rustup target list --installed 2>/dev/null | grep -qx "$t" || rustup target add "$t"
    echo "Building $t"
    cargo build --release --locked --target "$t"
done
STAGE=dist/imsg-$VERSION-macos
rm -rf dist
mkdir -p "$STAGE"
for bin in imsg-server imsg; do
    lipo -create -output "$STAGE/$bin" target/aarch64-apple-darwin/release/$bin target/x86_64-apple-darwin/release/$bin
    codesign --force --sign - "$STAGE/$bin"
    codesign --verify "$STAGE/$bin"
    chmod 755 "$STAGE/$bin"
done
cp scripts/install-server.sh "$STAGE/install-server.sh"
cp README.md "$STAGE/README.md"
chmod 755 "$STAGE/install-server.sh"
tar -czf dist/imsg-macos-universal.tar.gz -C dist "imsg-$VERSION-macos"
cp scripts/install.sh dist/install.sh
(cd dist && shasum -a 256 imsg-macos-universal.tar.gz install.sh > SHA256SUMS)
echo "Packaged imsg $VERSION:"
lipo -info "$STAGE/imsg-server"
ls -l dist
