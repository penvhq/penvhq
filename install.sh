#!/bin/sh
# penv installer. curl -fsSL https://penv.cloud/install | sh
#
# PENV_VERSION       pin a tag, such as v1.2.3; default is the latest release
# PENV_INSTALL_DIR   where the binary lands; default $HOME/.penv/bin
# PENV_RELEASE_BASE  the address the release is read from; default https://penv.cloud
# PENV_TARGET        install for another machine's triple instead of this one's
# PENV_ALLOW_ROOT    1 installs as root; the default refuses

set -eu

# penv.cloud redirects to whichever host serves the releases, so no repository path lives here.
base=${PENV_RELEASE_BASE:-https://penv.cloud}
base=${base%/}
targets="x86_64-unknown-linux-musl aarch64-unknown-linux-musl x86_64-apple-darwin aarch64-apple-darwin x86_64-pc-windows-msvc aarch64-pc-windows-msvc"

# The release keys, base64 Ed25519 public keys one per line, that a checksum file
# may be signed with. Empty until the owner runs: cargo run -p penv-release -- keygen
public_keys="VRJ90W7uzjrwQKeD6KCGQj1dih6z6/4QVeat0M5qL/0="

say() { printf '%s\n' "$*"; }
dim() { if [ -t 1 ]; then printf '\033[2m%s\033[0m\n' "$*"; else printf '%s\n' "$*"; fi; }
die() { printf 'penv: %s\n' "$*" >&2; exit 1; }

if [ "$(id -u 2>/dev/null || echo 1)" = 0 ] && [ "${PENV_ALLOW_ROOT:-}" != 1 ]; then
    die "this installs into a home directory, so run it as the user who will run penv, or set PENV_ALLOW_ROOT=1."
fi

triple=${PENV_TARGET:-}
if [ -n "$triple" ]; then
    case " $targets " in
        *" $triple "*) ;;
        *) die "penv publishes no $triple build. The releases carry $targets." ;;
    esac
else
    os=$(uname -s)
    arch=$(uname -m)
    case "$os" in
        Linux) os=unknown-linux-musl ;;
        Darwin) os=apple-darwin ;;
        MINGW* | MSYS* | CYGWIN* | Windows_NT)
            die "on Windows run the PowerShell installer: irm https://penv.cloud/install.ps1 | iex"
            ;;
        *) die "no penv build for $os. The releases carry $targets." ;;
    esac
    case "$arch" in
        x86_64 | amd64) arch=x86_64 ;;
        aarch64 | arm64) arch=aarch64 ;;
        *) die "no penv build for $arch on $os. The releases carry $targets." ;;
    esac
    triple=$arch-$os
fi
case "$triple" in
    *windows*) exe=.exe ;;
    *) exe= ;;
esac

if command -v curl >/dev/null 2>&1; then
    # --proto-redir constrains redirects only, so a plain http base still works while a
    # redirect off https cannot walk the download back down to http.
    fetch() { curl -fsSL --proto-redir '=https' "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
    # wget has no per-redirect protocol filter, so this path trusts the redirect chain.
    fetch() { wget -qO "$2" "$1"; }
else
    die "neither curl nor wget is on PATH, and one of them downloads the release."
fi

if command -v sha256sum >/dev/null 2>&1; then
    digest_of() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
    digest_of() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
    die "neither sha256sum nor shasum is on PATH, and the download is verified before it is installed."
fi

# Ed25519 arrived in OpenSSL 1.1.1, and only OpenSSL itself takes -rawin; the
# LibreSSL that macOS ships under the same name does not.
openssl_verifies() {
    command -v openssl >/dev/null 2>&1 || return 1
    set -- $(openssl version 2>/dev/null)
    [ "${1:-}" = OpenSSL ] || return 1
    version=${2:-}
    major=${version%%.*}
    rest=${version#*.}
    minor=${rest%%.*}
    patch=${rest#*.}
    patch=${patch%%[!0-9]*}
    case "$major$minor" in
        '' | *[!0-9]*) return 1 ;;
    esac
    [ "$major" -gt 1 ] && return 0
    [ "$major" -eq 1 ] && [ "$minor" -gt 1 ] && return 0
    [ "$major" -eq 1 ] && [ "$minor" -eq 1 ] && [ "${patch:-0}" -ge 1 ]
}

# The 12 SPKI header bytes are a multiple of three, so the PEM body is that
# header's base64 followed by the key's own, unchanged.
signed_by_a_release_key() {
    # -A reads the signature as the one long line it is, whatever ended it.
    tr -d '\r\n \t' <"$2" | openssl base64 -d -A -out "$work/signature.bin" 2>/dev/null || return 1
    for key in $public_keys; do
        # 32 raw bytes are 44 base64 characters, so a key pasted without its
        # trailing = is the same key and gets it back.
        if [ ${#key} -eq 43 ]; then key=$key=; fi
        printf -- '-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEA%s\n-----END PUBLIC KEY-----\n' \
            "$key" >"$work/key.pem"
        if openssl pkeyutl -verify -pubin -inkey "$work/key.pem" -rawin \
            -in "$1" -sigfile "$work/signature.bin" >/dev/null 2>&1; then
            return 0
        fi
    done
    return 1
}

cleanup() {
    [ -n "${temp:-}" ] && rm -f "$temp"
    [ -n "${work:-}" ] && rm -rf "$work"
    return 0
}
trap 'cleanup' EXIT
trap 'cleanup; exit 130' INT TERM

work=$(mktemp -d "${TMPDIR:-/tmp}/penv-install.XXXXXX")

tag=${PENV_VERSION:-}
if [ -z "$tag" ]; then
    latest=$base/releases/latest
    fetch "$latest" "$work/release.json" || die "$latest could not be read. Check the network and try again."
    tag=$(tr ',' '\n' <"$work/release.json" |
        sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)
    [ -n "$tag" ] || die "the latest release carries no tag_name. Try again later."
fi
# One shape whichever way the tag arrived, and nothing in it that could reach past the asset.
tag=v${tag#v}
case $tag in
    *[!0-9A-Za-z.+_-]*) die "$tag is not a tag such as v1.2.3: a tag carries no slash and no whitespace." ;;
esac

asset=penv-$tag-$triple$exe
sums=penv-$tag-$triple.sha256
download=$base/releases/download/$tag

dir=${PENV_INSTALL_DIR:-$HOME/.penv/bin}
binary=$dir/penv$exe
mkdir -p "$dir" || die "$dir could not be created."
# Absolute, so the PATH line printed below works from any directory.
dir=$(CDPATH= cd -P -- "$dir" && pwd) || die "$dir could not be entered."
# The install directory is created before the digest passes, and the binary is downloaded
# into it so the rename is atomic and lands over a running penv; the temp file goes on
# every exit path, so a refusal leaves the directory as it found it.
temp=$dir/.penv.$$

say "penv $tag for $triple"
fetch "$download/$asset" "$temp" || die "$download/$asset could not be downloaded."
fetch "$download/$sums" "$work/$sums" || die "$download/$sums could not be downloaded."

# The signature stands in front of the digest: an unsigned checksum file says
# nothing about the binary it lists.
if [ -z "$public_keys" ]; then
    dim "signature not checked: this installer carries no release key"
elif ! openssl_verifies; then
    dim "signature not checked: OpenSSL 1.1.1 or newer verifies it, and this host has none"
else
    fetch "$download/$sums.sig" "$work/$sums.sig" ||
        die "$download/$sums.sig could not be downloaded, and a release is signed. Nothing was installed."
    signed_by_a_release_key "$work/$sums" "$work/$sums.sig" ||
        die "$sums is not signed by a penv release key. Nothing was installed."
fi

# The checksum file covers the archive too, so the raw binary's line is matched whole.
expected=$(sed -n "s/^\([0-9a-fA-F]\{64\}\)[[:space:]][[:space:]]*[*]\{0,1\}$asset\$/\1/p" "$work/$sums" | head -n 1)
[ -n "$expected" ] || die "$sums lists no sha256 digest for $asset."
actual=$(digest_of "$temp")
if [ "$(printf '%s' "$expected" | tr 'A-F' 'a-f')" != "$(printf '%s' "$actual" | tr 'A-F' 'a-f')" ]; then
    die "$asset is not the file $sums names. Nothing was installed."
fi

chmod 755 "$temp"
mv -f "$temp" "$binary" || die "$binary could not be written."

say "installed $binary"
case ":$PATH:" in
    *":$dir:"*)
        say "run: penv init"
        exit 0
        ;;
esac

say ""
case "$(basename "${SHELL:-sh}")" in
    fish) say "put it on your PATH:  fish_add_path \"$dir\"" ;;
    zsh) say "put it on your PATH:  echo 'export PATH=\"$dir:\$PATH\"' >> ~/.zshrc" ;;
    *) say "put it on your PATH:  echo 'export PATH=\"$dir:\$PATH\"' >> ~/.profile" ;;
esac
say "or run it now:        \"$binary\" init"
