#!/bin/sh
# Build penv-lambda-layer-<arch>.zip from a published release:
#   ./build-layer.sh v1.0.0 x86_64      # or arm64
# The binary is fetched by the repository's install.sh, so the layer gets the
# same checks an install does: the release signature, then the digest.
set -eu
version=${1:?release tag, such as v1.0.0}
arch=${2:-x86_64}
case "$arch" in
  x86_64) target=x86_64-unknown-linux-musl ;;
  arm64) target=aarch64-unknown-linux-musl ;;
  *) echo "arch is x86_64 or arm64" >&2; exit 2 ;;
esac
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/layer/bin"
PENV_VERSION="$version" PENV_TARGET="$target" PENV_INSTALL_DIR="$work/layer/bin" PENV_ALLOW_ROOT=1 \
  sh "$here/../../install.sh" >&2
install -m 0755 "$here/penv-wrapper" "$work/layer/penv-wrapper"
out="$(pwd)/penv-lambda-layer-$arch.zip"
rm -f "$out"
(cd "$work/layer" && zip -qr "$out" .)
echo "$out"
