#!/usr/bin/env bash
# penv vs varlock on the same files: speed, memory, and behaviour.
#
#   bench/run.sh                     # penv from PATH, varlock 1.20.0 from npm
#   PENV=./target/release/penv bench/run.sh
#   VARLOCK_VERSION=latest RUNS=50 bench/run.sh
#   VARLOCK=~/.config/varlock/bin/varlock bench/run.sh   # varlock's standalone binary
#
# Needs: penv, node 18+ and npm, git, python3. Writes results to bench-results.json
# in the current directory. Every secret below is fake.
set -uo pipefail

PENV=${PENV:-$(command -v penv || true)}
VARLOCK_VERSION=${VARLOCK_VERSION:-1.20.0}
RUNS=${RUNS:-30}
OUT=${OUT:-$PWD/bench-results.json}
SECRET=sk_live_BENCH_4242424242424242

die() { echo "bench: $*" >&2; exit 1; }
[ -x "$PENV" ] || die "no penv binary: install it or set PENV=<path>"
# Absolute, because every project below runs from its own directory.
case $PENV in /*) ;; *) PENV=$PWD/$PENV ;; esac
for tool in node npm git python3; do command -v "$tool" >/dev/null || die "$tool is required"; done

WORK=$(mktemp -d "${TMPDIR:-/tmp}/penv-bench.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
echo "penv:    $("$PENV" --version)"
if [ -n "${VARLOCK:-}" ]; then
  [ -x "$VARLOCK" ] || die "VARLOCK=$VARLOCK is not an executable"
  case $VARLOCK in /*) ;; *) VARLOCK=$PWD/$VARLOCK ;; esac
  echo "varlock: using $VARLOCK"
else
  echo "varlock: installing $VARLOCK_VERSION from npm into $WORK ..."
  (cd "$WORK" && printf '{"private":true}' > package.json && npm i -s "varlock@$VARLOCK_VERSION" >/dev/null 2>&1) \
    || die "npm could not install varlock@$VARLOCK_VERSION"
  VARLOCK="$WORK/node_modules/.bin/varlock"
fi
echo "varlock: $("$VARLOCK" --version)"
echo "machine: $(uname -sm), node $(node --version), $RUNS runs per timing"
echo

# A project both tools load unchanged.
project() {
  local dir="$WORK/$1"; mkdir -p "$dir"; cd "$dir" || exit 1
  git init -q . 2>/dev/null
  printf '.env\n.env.*\n!.env.schema\n' > .gitignore
}
common_schema() {
  cat > .env.schema <<'EOF'
# @currentEnv=$APP_ENV
# ---

# @type=enum(development, production) @sensitive=false
APP_ENV=development

# @type=port @sensitive=false
PORT=3000

# @type=string @sensitive
DB_PASSWORD=

# @type=string @sensitive
STRIPE_SECRET_KEY=

# @type=url @sensitive=false
API_URL=https://api.example.com
EOF
  printf 'DB_PASSWORD=hunter2hunter2\nSTRIPE_SECRET_KEY=%s\n' "$SECRET" > .env
  mkdir -p src && for i in $(seq 1 40); do echo "export const v$i = $i;" > "src/m$i.js"; done
}

# Median wall time in ms of a command, RUNS times, measured by node.
median_ms() {
  node -e '
    const { spawnSync } = require("child_process");
    const [runs, ...cmd] = process.argv.slice(1);
    const times = [];
    for (let i = 0; i < Number(runs); i++) {
      const t = process.hrtime.bigint();
      const ran = spawnSync(cmd[0], cmd.slice(1), { stdio: "ignore" });
      // A command that never started would be timed as the fastest run.
      if (ran.error) { console.error(`bench: ${cmd[0]}: ${ran.error.message}`); process.exit(1); }
      times.push(Number(process.hrtime.bigint() - t) / 1e6);
    }
    times.sort((a, b) => a - b);
    console.log(times[Math.floor(times.length / 2)].toFixed(1));
  ' "$RUNS" "$@"
}

# Peak resident memory in MB of the command and its children, from getrusage.
peak_mb() {
  python3 - "$@" <<'PY'
import resource, subprocess, sys
subprocess.run(sys.argv[1:], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
peak = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
print(round(peak / (1048576 if sys.platform == "darwin" else 1024)))
PY
}

RESULTS=()
row() { # name, penv, varlock, penv_wins(yes/no/-)
  printf '| %-46s | %-24s | %-30s |\n' "$1" "$2" "$3"
  RESULTS+=("$(printf '{"check":%s,"penv":%s,"varlock":%s}' \
    "$(node -p 'JSON.stringify(process.argv[1])' "$1")" \
    "$(node -p 'JSON.stringify(process.argv[1])' "$2")" \
    "$(node -p 'JSON.stringify(process.argv[1])' "$3")")")
}

echo "| Check                                          | penv                     | varlock                        |"
echo "|------------------------------------------------|--------------------------|--------------------------------|"

# --- speed and memory ------------------------------------------------------------
project speed; common_schema
row "run -- true (median ms)" "$(median_ms "$PENV" run -- true)" "$(median_ms "$VARLOCK" run -- true)"
row "validate: penv check / varlock load (ms)" "$(median_ms "$PENV" check)" "$(median_ms "$VARLOCK" load)"
row "scan a repository (ms)" "$(median_ms "$PENV" scan)" "$(median_ms "$VARLOCK" scan)"
row "peak memory, run -- true (MB)" "$(peak_mb "$PENV" run -- true)" "$(peak_mb "$VARLOCK" run -- true)"

# --- a value built from a secret, marked public ------------------------------------
project public; common_schema
cat >> .env.schema <<'EOF'

# @type=url @sensitive=false
NEXT_PUBLIC_CHECKOUT=https://pay.example.com/?k=${STRIPE_SECRET_KEY}
EOF
"$PENV" check >/dev/null 2>&1; p=$?
"$VARLOCK" load >/dev/null 2>&1; v=$?
row "public key built from a secret" "$([ $p -ne 0 ] && echo "refused (exit $p)" || echo "accepted")" \
  "$([ $v -ne 0 ] && echo "refused (exit $v)" || echo "accepted, warning only")"

# --- a password with @ and : inside a URL ------------------------------------------
project urlencode; common_schema
printf 'DB_PASSWORD=p@ss:w0rd/x\nSTRIPE_SECRET_KEY=%s\n' "$SECRET" > .env
cp .env.schema base.schema
printf '\n# @type=url @sensitive=false\nDATABASE_URL=postgres://app:${DB_PASSWORD | urlencode}@db:5432/app\n' >> .env.schema
"$PENV" check >/dev/null 2>&1; p=$?
cp base.schema .env.schema
printf '\n# @type=url @sensitive=false\nDATABASE_URL=postgres://app:${DB_PASSWORD}@db:5432/app\n' >> .env.schema
"$VARLOCK" load >/dev/null 2>&1; v=$?
row "password with @ : / in a URL" "$([ $p -eq 0 ] && echo "valid (urlencode filter)" || echo "invalid")" \
  "$([ $v -eq 0 ] && echo "valid" || echo "invalid URL (no filter)")"

# --- ${KEY:-fallback} ---------------------------------------------------------------
project fallback; common_schema
printf '\n# @type=string @sensitive=false\nREGION=${AWS_REGION:-us-east-1}\n' >> .env.schema
p=$(env -u AWS_REGION "$PENV" run -- node -e 'process.stdout.write(process.env.REGION||"")' 2>/dev/null)
env -u AWS_REGION "$VARLOCK" load >/dev/null 2>&1; v=$?
row '${KEY:-fallback}' "$([ "$p" = us-east-1 ] && echo "resolved: $p" || echo "failed")" \
  "$([ $v -eq 0 ] && echo "resolved" || echo "failed (exit $v)")"

# --- @assert --------------------------------------------------------------------------
project assert; common_schema
sed -i.bak '1i\
# @assert(not(eq($PORT, 5432)), "PORT collides with Postgres")
' .env.schema && rm -f .env.schema.bak
printf 'PORT=5432\n' >> .env
"$PENV" run -- true >/dev/null 2>&1; p=$?
"$VARLOCK" load >/dev/null 2>&1; v=$?
row "@assert across keys" "$([ $p -eq 3 ] && echo "run stopped (exit 3)" || echo "exit $p")" \
  "$([ $v -ne 0 ] && echo "unknown decorator (exit $v)" || echo "accepted")"

# --- a base64-encoded secret committed to a file -----------------------------------------
project scan; common_schema
echo "export const k = \"$(printf '%s' "$SECRET" | base64)\";" > src/leak.js
"$PENV" scan >/dev/null 2>&1; p=$?
"$VARLOCK" scan >/dev/null 2>&1; v=$?
row "base64 of a secret in a committed file" "$([ $p -ne 0 ] && echo "found (exit $p)" || echo "missed")" \
  "$([ $v -ne 0 ] && echo "found (exit $v)" || echo "missed")"

# --- a secret served over HTTP --------------------------------------------------------
project serve; common_schema
cat > server.js <<'EOF'
const http = require("http");
const s = http.createServer((_, res) => res.end(process.env.STRIPE_SECRET_KEY));
s.listen(0, async () => {
  const body = await (await fetch(`http://127.0.0.1:${s.address().port}/`)).text();
  process.stderr.write(body.includes("BENCH_4242") ? "RAW\n" : "masked\n"); s.close();
});
EOF
cat > server.py <<'EOF'
import os, sys, threading, http.server, urllib.request
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        b = os.environ["STRIPE_SECRET_KEY"].encode()
        self.send_response(200); self.send_header("Content-Length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def log_message(self, *a): pass
s = http.server.HTTPServer(("127.0.0.1", 0), H); threading.Thread(target=s.serve_forever, daemon=True).start()
body = urllib.request.urlopen(f"http://127.0.0.1:{s.server_port}/").read().decode()
sys.stderr.write(("RAW" if "BENCH_4242" in body else "masked") + "\n"); s.shutdown()
EOF
served() { "$@" 2>&1 >/dev/null | grep -oE 'RAW|masked' | tail -1; }
row "Node server response holding a secret" "$(served "$PENV" run -- node server.js)" "$(served "$VARLOCK" run -- node server.js)"
row "Python server response holding a secret" "$(served "$PENV" run -- python3 server.py)" "$(served "$VARLOCK" run -- python3 server.py)"

# --- a secret file git would commit -----------------------------------------------------
project gitignore; common_schema
rm .gitignore
"$PENV" check >/dev/null 2>&1; p=$?
"$VARLOCK" load >/dev/null 2>&1; v=$?
row ".env with a secret, not gitignored" "$([ $p -ne 0 ] && echo "check fails (exit $p)" || echo "passes")" \
  "$([ $v -ne 0 ] && echo "load fails (exit $v)" || echo "passes")"

printf '{"penv":%s,"varlock":%s,"runs":%s,"machine":%s,"results":[%s]}\n' \
  "$(node -p 'JSON.stringify(process.argv[1])' "$("$PENV" --version)")" \
  "$(node -p 'JSON.stringify(process.argv[1])' "$("$VARLOCK" --version)")" "$RUNS" \
  "$(node -p 'JSON.stringify(process.argv[1])' "$(uname -sm)")" \
  "$(IFS=,; echo "${RESULTS[*]}")" > "$OUT"
echo
echo "results: $OUT"
