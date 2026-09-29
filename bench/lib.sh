# Shared by the bench scripts: a fresh project, the schema every tool loads, and the two measures.
# Sourced, not run. Every secret below is fake.

SECRET=sk_live_BENCH_4242424242424242

die() { echo "bench: $*" >&2; exit 1; }

# A project every tool loads unchanged, under $WORK.
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

json() { node -p 'JSON.stringify(process.argv[1])' "$1"; }
