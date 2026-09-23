#!/bin/sh
# Put the released Linux binaries where the Dockerfile expects them:
#   <context>/bin/amd64/penv   from x86_64-unknown-linux-musl
#   <context>/bin/arm64/penv   from aarch64-unknown-linux-musl
# Each comes through install.sh, which refuses a binary whose checksum file is not
# signed by a release key or whose digest does not match. The image therefore
# holds exactly the files the release signed.
#
# fetch.sh <tag> <context> [release base]
set -eu
tag=${1:?usage: fetch.sh <tag> <context> [release base]}
context=${2:?usage: fetch.sh <tag> <context> [release base]}
base=${3:-https://penv.cloud}
here=$(cd "$(dirname "$0")" && pwd)
installer=$here/../../install.sh
for pair in amd64:x86_64-unknown-linux-musl arm64:aarch64-unknown-linux-musl; do
    arch=${pair%%:*}
    triple=${pair#*:}
    said=$(PENV_VERSION=$tag PENV_TARGET=$triple PENV_RELEASE_BASE=$base \
        PENV_INSTALL_DIR=$context/bin/$arch PENV_ALLOW_ROOT=1 \
        sh "$installer" 2>&1) || { printf '%s\n' "$said" >&2; exit 1; }
    # install.sh installs on the digest alone when it cannot check a signature;
    # an image is never built that way.
    case $said in
        *"signature not checked"*)
            printf 'fetch.sh: %s\n' "$said" >&2
            rm -rf "$context/bin/$arch"
            exit 1
            ;;
    esac
    [ -x "$context/bin/$arch/penv" ] || { echo "fetch.sh: no $triple binary for $tag" >&2; exit 1; }
    echo "$arch: $triple $tag verified"
done
