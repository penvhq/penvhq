# Shared by the sealed end-to-end scripts: a scratch folder, a test CA and a
# localhost certificate, and a trust bundle penv is told about.
set -euo pipefail
PENV=${PENV:?set PENV to the penv binary}
# Git Bash on Windows rewrites arguments that look like paths (/CN=...).
export MSYS_NO_PATHCONV=1
# The first Python that runs: on Windows `python3` can be the Store's stub.
PY=""
for candidate in python3 python; do
  if "$candidate" -c "" 2>/dev/null; then PY=$candidate; break; fi
done
[ -n "$PY" ] || { echo "FAIL: no Python"; exit 1; }
# A path a native program understands: C:\... under Git Bash, as it is elsewhere.
native() { if command -v cygpath >/dev/null; then cygpath -w "$1"; else echo "$1"; fi; }
WORK=$(mktemp -d)
# A server still holding its files (Windows) must not turn a pass into a fail.
cleanup() {
  local status=$?
  kill $(jobs -p) 2>/dev/null || true
  wait 2>/dev/null || true
  rm -rf "$WORK" 2>/dev/null || true
  exit "$status"
}
trap cleanup EXIT
cd "$WORK"
openssl req -x509 -newkey rsa:2048 -nodes -keyout ca.key -out ca.pem -days 2 -subj "/CN=penv e2e CA" \
  -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign" 2>/dev/null
openssl req -newkey rsa:2048 -nodes -keyout srv.key -out srv.csr -subj "/CN=localhost" 2>/dev/null
printf 'subjectAltName=DNS:localhost\nextendedKeyUsage=serverAuth\n' > ext.cnf
openssl x509 -req -in srv.csr -CA ca.pem -CAkey ca.key -CAcreateserial -out srv.pem -days 2 -extfile ext.cnf 2>/dev/null
system=""
for f in /etc/ssl/certs/ca-certificates.crt /etc/ssl/cert.pem; do
  if [ -f "$f" ]; then system=$f; break; fi
done
cat ca.pem ${system:+"$system"} > trust.pem
fail() { echo "FAIL: $*"; exit 1; }
wait_port() {
  "$PY" - "$1" <<'PY' || fail "nothing listens on $1"
import socket, sys, time
for _ in range(100):
    try:
        socket.create_connection(("127.0.0.1", int(sys.argv[1])), 0.5).close(); sys.exit(0)
    except OSError:
        time.sleep(0.2)
sys.exit(1)
PY
}
