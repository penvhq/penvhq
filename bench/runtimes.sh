#!/usr/bin/env bash
# The env file `penv gen ts` writes, in the runtimes and builds it targets.
# A secret must read on the server and never reach a browser bundle; a public key
# must reach both.
#
#   bench/runtimes.sh            # Node, Bun, Deno, workerd (Cloudflare)
#   bench/runtimes.sh --builds   # also Next.js, Vite and Parcel, checked in headless Chromium
#
# Projects sit under one directory holding node_modules, which Node resolution
# finds by walking up. Needs: penv, node 18+ and npm. Bun and Deno are used when installed. --builds
# downloads the frameworks and a headless Chromium (about 700 MB) into a temp dir.
set -uo pipefail

PENV=${PENV:-$(command -v penv || true)}
BUILDS=0; [ "${1:-}" = "--builds" ] && BUILDS=1
SECRET=sk_live_RUNTIME_4242424242
export STRIPE_SECRET_KEY=$SECRET NEXT_PUBLIC_API_URL=https://api.example.com VITE_API_URL=https://api.example.com PORT=9999 NEXT_TELEMETRY_DISABLED=1

die() { echo "runtimes: $*" >&2; exit 1; }
[ -x "$PENV" ] || die "no penv binary: install it or set PENV=<path>"
# Absolute, because every project below runs from its own directory.
case $PENV in /*) ;; *) PENV=$PWD/$PENV ;; esac
command -v node >/dev/null && command -v npm >/dev/null || die "node and npm are required"
WORK=$(mktemp -d "${TMPDIR:-/tmp}/penv-runtimes.XXXXXX")
PIDS=()
# Servers start in their own process group (set -m), so the whole group, the
# framework's child server included, is stopped on exit.
cleanup() { for p in "${PIDS[@]:-}"; do [ -n "$p" ] && kill -- "-$p" 2>/dev/null; done; rm -rf "$WORK"; }
trap cleanup EXIT

ok=0; bad=0; skipped=0
pass() { ok=$((ok+1)); printf 'ok    %s\n' "$1"; }
fail() { bad=$((bad+1)); printf 'FAIL  %s\n' "$1"; }
skip() { skipped=$((skipped+1)); printf 'skip  %s\n' "$1"; }
absent() { ! grep -rqF "$SECRET" "$@" 2>/dev/null; }

SCHEMA='# @type=string(startsWith=sk_)
STRIPE_SECRET_KEY=

# @type=port @sensitive=false
PORT=3000

# @type=url
NEXT_PUBLIC_API_URL=https://api.example.com

# @type=url
VITE_API_URL=https://api.example.com
'
# gen <dir> <runtime> <out>: write the schema and generate env.ts for that runtime.
gen() {
  mkdir -p "$1/.penv" && printf '%s' "$SCHEMA" > "$1/.env.schema" && printf '{}' > "$1/package.json" && printf '{}' > "$1/tsconfig.json"
  printf '[targets.ts]\noutput = "%s"\n\n[targets.ts.options]\nruntime = "%s"\n' "$3" "$2" > "$1/.penv/config.toml"
  (cd "$1" && "$PENV" gen ts >/dev/null 2>&1) || fail "penv gen ts --runtime $2"
}

echo "penv $("$PENV" --version | awk '{print $2}'), node $(node --version)"

# --- server runtimes -----------------------------------------------------------------
gen "$WORK/rt" node env.ts
cat > "$WORK/rt/main.ts" <<'EOF'
import { env } from "./env.ts";
console.log(JSON.stringify({ secret: env.STRIPE_SECRET_KEY.length, port: env.PORT, public: env.NEXT_PUBLIC_API_URL }));
EOF
want='{"secret":26,"port":9999,"public":"https://api.example.com"}'
check_rt() { # label dir command...
  local label=$1 dir=$2 got; shift 2
  got=$(cd "$dir" && "$@" 2>&1 | tail -1)
  [ "$got" = "$want" ] && pass "$label reads server and public keys" || fail "$label: $got"
}
if node --experimental-strip-types -e '' 2>/dev/null; then
  check_rt node "$WORK/rt" node --experimental-strip-types --no-warnings main.ts
else skip "node: this Node cannot run TypeScript directly (needs 22.6+)"; fi
if command -v bun >/dev/null; then check_rt bun "$WORK/rt" bun main.ts; else skip "bun: not installed"; fi
if command -v deno >/dev/null; then
  check_rt deno "$WORK/rt" deno run --allow-env main.ts
  gen "$WORK/deno" deno env.ts && cp "$WORK/rt/main.ts" "$WORK/deno/"
  check_rt "deno (runtime=deno)" "$WORK/deno" deno run --allow-env main.ts
else skip "deno: not installed"; fi

# --- Cloudflare Workers (workerd) ------------------------------------------------------
W="$WORK/workers"; mkdir -p "$W"
(cd "$W" && printf '{"private":true,"type":"module"}' > package.json && npm i -s miniflare@4 esbuild >/dev/null 2>&1) || fail "npm could not install miniflare"
gen "$W/node" node env.ts; gen "$W/workers" workers env.ts
for v in node workers; do
  printf 'import { env } from "./%s/env";\nexport default { async fetch() { let s; try { s = String(env.STRIPE_SECRET_KEY?.length); } catch { s = "threw"; } return new Response(s); } };\n' "$v" > "$W/w-$v.ts"
  "$W/node_modules/.bin/esbuild" "$W/w-$v.ts" --bundle --format=esm --platform=neutral --external:cloudflare:workers --log-level=error --outfile="$W/w-$v.js"
done
cat > "$W/run.mjs" <<'EOF'
import { Miniflare } from "miniflare";
const secret = process.env.STRIPE_SECRET_KEY;
const cases = [
  ["node", "2025-06-01", ["nodejs_compat"], "26"],
  ["workers", "2025-06-01", ["nodejs_compat"], "26"],
  ["workers", "2024-09-01", [], "26"],
  ["node", "2024-09-01", [], "threw"],
];
for (const [runtime, date, flags, want] of cases) {
  const mf = new Miniflare({ modules: true, scriptPath: `w-${runtime}.js`, compatibilityDate: date, compatibilityFlags: flags, bindings: { STRIPE_SECRET_KEY: secret } });
  const got = await (await mf.dispatchFetch("http://x/")).text();
  await mf.dispose();
  const label = `workerd runtime=${runtime} ${flags.join(",") || "no flags"} ${date}`;
  console.log(got === want ? `ok    ${label}: ${want === "threw" ? "server-only error, as documented" : "reads the secret"}` : `FAIL  ${label}: ${got}`);
}
EOF
while read -r line; do case $line in ok*) pass "${line#ok    }";; FAIL*) fail "${line#FAIL  }";; esac; done < <(cd "$W" && node run.mjs 2>&1 | grep -E '^(ok|FAIL)')

# --- builds ---------------------------------------------------------------------------
if [ $BUILDS -eq 0 ]; then
  echo; echo "passed=$ok failed=$bad skipped=$skipped (add --builds for Next.js, Vite and Parcel)"; exit $((bad > 0))
fi
B="$WORK/builds"; mkdir -p "$B"
(cd "$B" && printf '{"private":true,"type":"module"}' > package.json \
  && npm i -s next react react-dom typescript @types/react @types/node vite parcel playwright >/dev/null 2>&1 \
  && npx playwright install --only-shell chromium >/dev/null 2>&1) || die "npm could not install the frameworks"
PORTS=(3123 4173 4280)
for port in "${PORTS[@]}"; do
  curl -s -m 2 "localhost:$port" >/dev/null && die "port $port is in use; stop what holds it and run again"
done
set -m
browse() { # url -> "public=... secret=..." as rendered after scripts ran
  (cd "$B" && node -e '
    const { chromium } = require("playwright");
    (async () => { const b = await chromium.launch(); const p = await b.newPage();
      await p.goto(process.argv[1], { waitUntil: "networkidle" }); await p.waitForTimeout(800);
      const text = await p.textContent("#out"); const html = await p.content(); await b.close();
      console.log(text + (html.includes(process.env.STRIPE_SECRET_KEY) ? " LEAKED" : "")); })();
  ' "$1")
}
CLIENT='let s = "READ"; try { void env.STRIPE_SECRET_KEY; } catch { s = "threw"; }'

# Next.js: server component, Node route, edge route, client component.
N="$B/next"; mkdir -p "$N/app/api/edge" "$N/app/api/node" "$N/lib"; gen "$N" node lib/env.ts
printf '{"compilerOptions":{"target":"ES2020","lib":["dom","esnext"],"strict":true,"module":"esnext","moduleResolution":"bundler","jsx":"preserve","noEmit":true,"esModuleInterop":true,"skipLibCheck":true,"isolatedModules":true,"plugins":[{"name":"next"}],"paths":{"@/*":["./*"]}},"include":["**/*.ts","**/*.tsx"],"exclude":["node_modules"]}' > "$N/tsconfig.json"
printf '{"private":true}' > "$N/package.json"
printf 'export default function L({ children }: { children: React.ReactNode }) { return <html><body>{children}</body></html>; }\n' > "$N/app/layout.tsx"
printf '"use client";\nimport { useEffect, useState } from "react";\nimport { env } from "@/lib/env";\nexport default function C() { const [t, setT] = useState(""); useEffect(() => { %s setT(`public=${env.NEXT_PUBLIC_API_URL} secret=${s}`); }, []); return <p id="out">{t}</p>; }\n' "$CLIENT" > "$N/app/client.tsx"
printf 'import { env } from "@/lib/env";\nimport C from "./client";\nexport const dynamic = "force-dynamic";\nexport default function P() { return <main><p id="server">{env.STRIPE_SECRET_KEY.length}</p><C /></main>; }\n' > "$N/app/page.tsx"
for r in edge node; do printf 'import { env } from "@/lib/env";\nexport const runtime = "%s";\nexport const dynamic = "force-dynamic";\nexport function GET() { return new Response(String(env.STRIPE_SECRET_KEY.length)); }\n' "${r/node/nodejs}" > "$N/app/api/$r/route.ts"; done
if (cd "$N" && "$B/node_modules/.bin/next" build >build.log 2>&1); then
  pass "next build"; absent "$N/.next" && pass "next: secret absent from .next" || fail "next: secret found in .next"
  (cd "$N" && "$B/node_modules/.bin/next" start -p ${PORTS[0]} >start.log 2>&1) & PIDS+=($!); sleep 8
  [ "$(curl -s localhost:${PORTS[0]}/api/node)" = 26 ] && pass "next: Node route reads the secret" || fail "next: Node route"
  [ "$(curl -s localhost:${PORTS[0]}/api/edge)" = 26 ] && pass "next: edge route reads the secret" || fail "next: edge route"
  curl -s localhost:${PORTS[0]}/ | grep -q '<p id="server">26' && pass "next: server component reads the secret" || fail "next: server component"
  got=$(browse "http://localhost:${PORTS[0]}/"); [ "$got" = "public=https://api.example.com secret=threw" ] && pass "next: in Chromium, public key read, secret throws" || fail "next browser: $got"
else fail "next build: $(tail -5 "$N/build.log")"; fi

# Vite: client in Chromium, SSR in Node.
V="$B/vite"; mkdir -p "$V/src"; gen "$V" vite src/env.ts
printf '{"private":true,"type":"module"}' > "$V/package.json"
printf '<!doctype html><html><body><p id="out"></p><script type="module" src="/src/main.ts"></script></body></html>' > "$V/index.html"
printf 'import { env } from "./env";\n%s\ndocument.getElementById("out")!.textContent = `public=${env.VITE_API_URL} secret=${s}`;\n' "$CLIENT" > "$V/src/main.ts"
printf 'import { env } from "./env";\nconsole.log(env.STRIPE_SECRET_KEY.length);\n' > "$V/src/server.ts"
if (cd "$V" && "$B/node_modules/.bin/vite" build --logLevel error && "$B/node_modules/.bin/vite" build --ssr src/server.ts --outDir dist-ssr --logLevel error) >/dev/null 2>&1; then
  pass "vite build (client and SSR)"; absent "$V/dist" "$V/dist-ssr" && pass "vite: secret absent from both builds" || fail "vite: secret in a build"
  [ "$(cd "$V" && node dist-ssr/server.js)" = 26 ] && pass "vite: SSR reads the secret" || fail "vite SSR"
  (cd "$V" && "$B/node_modules/.bin/vite" preview --port ${PORTS[1]} --strictPort >preview.log 2>&1) & PIDS+=($!); sleep 4
  got=$(browse "http://localhost:${PORTS[1]}/"); [ "$got" = "public=https://api.example.com secret=threw" ] && pass "vite: in Chromium, public key read, secret throws" || fail "vite browser: $got"
else fail "vite build"; fi

# Parcel: inlines every literal process.env.X it sees; a control literal proves it.
PA="$B/parcel"; mkdir -p "$PA/src"; gen "$PA" node src/env.ts
printf '<!doctype html><html><body><p id="out"></p><script type="module" src="./main.ts"></script></body></html>' > "$PA/src/index.html"
printf 'import { env } from "./env";\n%s\ndocument.getElementById("out")!.textContent = `public=${env.NEXT_PUBLIC_API_URL} secret=${s}`;\n(window as any).control = process.env.PORT;\n' "$CLIENT" > "$PA/src/main.ts"
if (cd "$PA" && "$B/node_modules/.bin/parcel" build src/index.html --no-cache --dist-dir dist --log-level error) >/dev/null 2>&1; then
  pass "parcel build"; absent "$PA/dist" && pass "parcel: secret absent" || fail "parcel: secret in dist"
  grep -rqF 9999 "$PA/dist" && pass "parcel: the control literal process.env.PORT was inlined" || fail "parcel control"
  (cd "$PA/dist" && python3 -m http.server ${PORTS[2]} >/dev/null 2>&1) & PIDS+=($!); sleep 2
  got=$(browse "http://localhost:${PORTS[2]}/index.html"); [ "$got" = "public=https://api.example.com secret=threw" ] && pass "parcel: in Chromium, public key read, secret throws" || fail "parcel browser: $got"
else fail "parcel build"; fi

echo; echo "passed=$ok failed=$bad skipped=$skipped"
exit $((bad > 0))
