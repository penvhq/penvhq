#!/bin/sh
# install.sh against a fake release served from a local directory: sh install.test.sh

set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
shipped=$here/install.sh
# The unsigned cases run the installer as it ships with no release key pasted in.
installer=$(mktemp "${TMPDIR:-/tmp}/penv-install-keyless.XXXXXX")
sed 's|^public_keys=".*"$|public_keys=""|' "$shipped" >"$installer"
tag=v9.9.9
triple=x86_64-unknown-linux-musl
asset=penv-$tag-$triple
sums=penv-$tag-$triple.sha256

# Probed rather than looked up, since Windows puts a store stub on PATH under both names.
python=
for candidate in python3 python py; do
    if "$candidate" -c 'import sys; sys.exit(0 if sys.version_info >= (3, 7) else 1)' >/dev/null 2>&1; then
        python=$candidate
        break
    fi
done
if [ -z "$python" ]; then
    echo "skip: python 3.7 or newer serves the fake release, and there is none on PATH"
    exit 0
fi

work=$(mktemp -d "${TMPDIR:-/tmp}/penv-install-test.XXXXXX")
server=
cleanup() {
    [ -n "$server" ] && kill "$server" 2>/dev/null
    rm -rf "$work" "$installer"
}
trap cleanup EXIT INT TERM

failures=0
check() {
    if [ "$2" = "$3" ]; then
        echo "ok   $1"
    else
        echo "FAIL $1"
        echo "     wanted: $3"
        echo "     got:    $2"
        failures=$((failures + 1))
    fi
}
contains() {
    case "$2" in
        *"$3"*) echo "ok   $1" ;;
        *)
            echo "FAIL $1"
            echo "     wanted a mention of: $3"
            echo "     got:                 $2"
            failures=$((failures + 1))
            ;;
    esac
}

# Three releases laid out the way penv.cloud redirects to them: one whole, one whose
# digest lies, one whose sums file only covers the archive.
for name in good tampered archive-only; do
    mkdir -p "$work/serve/$name/releases/download/$tag"
    printf '{"tag_name":"%s","assets":[{"name":"%s"},{"name":"%s"}]}\n' "$tag" "$asset" "$sums" \
        >"$work/serve/$name/releases/latest"
    printf '#!/bin/sh\necho penv 9.9.9\n' >"$work/serve/$name/releases/download/$tag/$asset"
    printf 'not really a tarball\n' >"$work/serve/$name/releases/download/$tag/$asset.tar.gz"
done
assets() { echo "$work/serve/$1/releases/download/$tag"; }
wrong=eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee
(cd "$(assets good)" && sha256sum "$asset.tar.gz" "$asset" >"$sums")
(cd "$(assets tampered)" && sha256sum "$asset.tar.gz" >"$sums")
printf '%s  %s\n' "$wrong" "$asset" >>"$(assets tampered)/$sums"
(cd "$(assets archive-only)" && sha256sum "$asset.tar.gz" >"$sums")

# tr, because a Windows python ends the line it prints with a carriage return.
port=$("$python" -c 'import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()' | tr -d '\r')
"$python" -m http.server "$port" --bind 127.0.0.1 --directory "$work/serve" >/dev/null 2>&1 &
server=$!
"$python" -c "
import sys, time, urllib.request
for _ in range(100):
    try:
        urllib.request.urlopen('http://127.0.0.1:$port/good/releases/latest').read()
        sys.exit(0)
    except Exception:
        time.sleep(0.1)
sys.exit(1)
" || {
    echo "FAIL the fake release never came up on 127.0.0.1:$port"
    exit 1
}

run_with() {
    script=$1
    release=$2
    shift 2
    status=0
    output=$(env PENV_ALLOW_ROOT=1 PENV_VERSION="$tag" PENV_TARGET="$triple" \
        PENV_RELEASE_BASE="http://127.0.0.1:$port/$release" "$@" \
        sh "$script" 2>&1) || status=$?
}
run() { run_with "$installer" "$@"; }

run good PENV_INSTALL_DIR="$work/bin"
check "a whole release installs" "$status" 0
contains "an installer with no release key says the signature went unchecked" "$output" \
    "signature not checked: this installer carries no release key"
check "the binary lands where PENV_INSTALL_DIR says" "$(cat "$work/bin/penv" 2>/dev/null)" "$(cat "$(assets good)/$asset")"
check "the installed binary is executable" "$([ -x "$work/bin/penv" ] && echo yes || echo no)" yes
contains "the install location is printed" "$output" "$work/bin/penv"

# No PENV_VERSION, so the tag comes from the release the base answers with.
status=0
output=$(env PENV_ALLOW_ROOT=1 PENV_TARGET="$triple" PENV_RELEASE_BASE="http://127.0.0.1:$port/good" \
    HOME="$work/home" sh "$installer" 2>&1) || status=$?
check "an unpinned install reads the tag off the latest release" "$status" 0
contains "the tag it resolved is printed" "$output" "penv $tag"
check "with no override the binary lands under HOME" \
    "$([ -f "$work/home/.penv/bin/penv" ] && echo yes || echo no)" yes

run tampered PENV_INSTALL_DIR="$work/tampered-bin"
check "a digest that does not match refuses" "$([ "$status" -ne 0 ] && echo refused || echo installed)" refused
contains "the refusal names the file" "$output" "is not the file"
check "nothing is installed after a mismatch" \
    "$([ -e "$work/tampered-bin/penv" ] && echo installed || echo nothing)" nothing

run archive-only PENV_INSTALL_DIR="$work/archive-bin"
check "a sums file covering only the archive refuses" "$([ "$status" -ne 0 ] && echo refused || echo installed)" refused
contains "the refusal names the missing digest" "$output" "lists no sha256 digest"

run good PENV_INSTALL_DIR="$work/riscv-bin" PENV_TARGET=riscv64gc-unknown-linux-gnu
check "an unpublished target refuses" "$([ "$status" -ne 0 ] && echo refused || echo installed)" refused
contains "the refusal lists what is published" "$output" "aarch64-pc-windows-msvc"

# The later assignment wins, so these two override the tag the runner pins.
run good PENV_INSTALL_DIR="$work/bare-bin" PENV_VERSION="${tag#v}"
check "a tag with no v installs the same release" "$status" 0
check "the binary lands from the normalised tag" \
    "$([ -f "$work/bare-bin/penv" ] && echo yes || echo no)" yes

run good PENV_INSTALL_DIR="$work/slash-bin" PENV_VERSION="$tag/../etc"
check "a tag holding a slash refuses" "$([ "$status" -ne 0 ] && echo refused || echo installed)" refused
contains "the refusal says what a tag looks like" "$output" "is not a tag such as v1.2.3"

if [ "$(id -u 2>/dev/null || echo 1)" = 0 ]; then
    run good PENV_ALLOW_ROOT= PENV_INSTALL_DIR="$work/root-bin"
    check "root is refused unless PENV_ALLOW_ROOT=1" "$([ "$status" -ne 0 ] && echo refused || echo installed)" refused
fi

# The installer's own probe, so a host where it would not verify anyway skips these.
eval "$(sed -n '/^openssl_verifies()/,/^}/p' "$installer")"
if ! openssl_verifies || ! openssl genpkey -algorithm ed25519 -out "$work/signer.pem" >/dev/null 2>&1; then
    echo "skip: openssl 1.1.1 or newer signs the fake releases, and there is none on PATH"
else
    openssl genpkey -algorithm ed25519 -out "$work/other.pem" >/dev/null 2>&1
    # The raw 32 key bytes, which is the shape the installer and the binary list.
    public=$(openssl pkey -in "$work/signer.pem" -pubout -outform DER | tail -c 32 | openssl base64 -A)

    sign_sums() {
        openssl pkeyutl -sign -inkey "$1" -rawin -in "$2/$sums" -out "$work/signature.bin"
        # One base64 line and a newline, which is what penv-release writes.
        { openssl base64 -A -in "$work/signature.bin"; printf '\n'; } >"$2/$sums.sig"
    }
    for name in signed bad-signature; do
        mkdir -p "$(assets "$name")"
        cp "$(assets good)/$asset" "$(assets good)/$asset.tar.gz" "$(assets good)/$sums" "$(assets "$name")/"
        cp "$work/serve/good/releases/latest" "$work/serve/$name/releases/latest"
    done
    sign_sums "$work/signer.pem" "$(assets signed)"
    sign_sums "$work/other.pem" "$(assets bad-signature)"

    # The installer as it ships once the owner has pasted a release key into it.
    signer=$work/install-signed.sh
    sed "s|^public_keys=\".*\"\$|public_keys=\"$public\"|" "$shipped" >"$signer"
    check "the test embedded a key in its copy of the installer" \
        "$(grep -c "^public_keys=\"$public\"\$" "$signer")" 1

    run_with "$signer" signed PENV_INSTALL_DIR="$work/signed-bin"
    check "a signed release installs" "$status" 0
    check "the signed binary lands" "$([ -f "$work/signed-bin/penv" ] && echo yes || echo no)" yes
    case "$output" in
        *"signature not checked"*)
            echo "FAIL a signed release is verified rather than waved through"
            echo "     got: $output"
            failures=$((failures + 1))
            ;;
        *) echo "ok   a signed release is verified rather than waved through" ;;
    esac

    # The same key as somebody might paste it, without the = base64 ends on.
    unpadded=${public%=}
    check "the release key ends on the padding this case drops" "${#unpadded}" 43
    bare=$work/install-unpadded.sh
    sed "s|^public_keys=\".*\"\$|public_keys=\"$unpadded\"|" "$shipped" >"$bare"
    run_with "$bare" signed PENV_INSTALL_DIR="$work/unpadded-bin"
    check "a key pasted without its padding still verifies" "$status" 0
    check "the binary lands under an unpadded key" \
        "$([ -f "$work/unpadded-bin/penv" ] && echo yes || echo no)" yes
    case "$output" in
        *"signature not checked"*)
            echo "FAIL an unpadded key verifies rather than being waved through"
            echo "     got: $output"
            failures=$((failures + 1))
            ;;
        *) echo "ok   an unpadded key verifies rather than being waved through" ;;
    esac

    run_with "$signer" bad-signature PENV_INSTALL_DIR="$work/badsig-bin"
    check "a signature from another key refuses" "$([ "$status" -ne 0 ] && echo refused || echo installed)" refused
    contains "the refusal names the key list" "$output" "is not signed by a penv release key"
    check "nothing is installed after a bad signature" \
        "$([ -e "$work/badsig-bin/penv" ] && echo installed || echo nothing)" nothing

    run_with "$signer" good PENV_INSTALL_DIR="$work/nosig-bin"
    check "a release carrying no signature refuses" "$([ "$status" -ne 0 ] && echo refused || echo installed)" refused
    contains "the refusal says the signature is missing" "$output" ".sig could not be downloaded"
fi

if [ "$failures" -eq 0 ]; then
    echo "install.sh: all checks passed"
else
    echo "install.sh: $failures failed"
    exit 1
fi
