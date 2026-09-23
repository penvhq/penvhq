# Benchmarks: penv and varlock

penv and [varlock](https://varlock.dev) run on the same project files, and the generated env file runs in the runtimes and builds it targets. Both scripts are in [`bench/`](../bench) and run on your machine:

```bash
bench/run.sh                  # penv vs varlock: speed, memory, behaviour
bench/runtimes.sh             # generated env in Node, Bun, Deno, workerd
bench/runtimes.sh --builds    # plus Next.js, Vite and Parcel builds, checked in headless Chromium
```

`PENV=<path>` picks the binary (default: `penv` on PATH). varlock comes from npm (`VARLOCK_VERSION`, default `1.20.0`), or from the binary `VARLOCK=<path>` names, such as its standalone install (`curl -sSfL https://varlock.dev/install.sh | sh`). `RUNS` sets timing iterations (default 30). `run.sh` writes `bench-results.json`.

## Results

penv 1.0.0-beta.1 release build, varlock 1.20.0 from npm and as its standalone binary, Linux x86_64, Node 22.22, median of 30 runs.

### Speed and memory

| | penv | varlock standalone | varlock from npm |
|---|---|---|---|
| `run -- true` | 5.3 ms | 234 ms | 310 ms |
| Validate (`penv check`, `varlock load`) | 4.2 ms | 174 ms | 312 ms |
| Scan a repository | 3.8 ms | 177 ms | 318 ms |
| Peak memory, `run -- true` | 11 MB | 70 MB | 95 MB |
| Binary | 11.7 MB, static | 105 MB, bundles Node.js | the npm package on Node.js |

Absolute times vary by machine; the ratio is what `run.sh` reproduces.

### Behaviour

| Check | penv | varlock |
|---|---|---|
| `NEXT_PUBLIC_` key built from a secret | refused, exit 3 | accepted with a warning; the value goes to the browser bundle |
| Password containing `@ : /` in a URL | valid, via `${DB_PASSWORD \| urlencode}` | `Invalid URL`; no filter exists |
| `${KEY:-fallback}` with `KEY` unset | resolves to the fallback | fails, exit 1 |
| `@assert` across keys | `run` stops before the command, exit 3 | unknown decorator, exit 1 |
| base64 of a secret in a committed file | found by `penv scan`, exit 3 | missed by `varlock scan` |
| Node.js server returning a secret in a response (`run -- node server.js`) | masked in the response bytes | sent unmasked |
| Python server returning a secret in a response (`run -- python3 server.py`) | masked in the response bytes | sent unmasked |
| `.env` holding a secret, not in `.gitignore` | `penv check` fails, exit 3 | `varlock load` passes |

Each row is a block in [`bench/run.sh`](../bench/run.sh) that builds the project from scratch and runs both tools on it.

### Generated env file across runtimes and builds

`penv gen ts` writes one `env` for server and client code. Public keys are read by literal name, which bundlers inline; every other key is read by computed name, which no bundler inlines. Results from `bench/runtimes.sh --builds`:

| Runtime or build | Secret on the server | Secret in the client build | Public key in the client |
|---|---|---|---|
| Node 22 | read | n/a | read |
| Bun 1.4 | read | n/a | read |
| Deno 2.9 (`runtime = "node"` and `"deno"`) | read | n/a | read |
| Cloudflare workerd, `nodejs_compat` (date ≥ 2025-04-01) | read | n/a | read |
| Cloudflare workerd, any flags, `runtime = "workers"` | read | n/a | read |
| Next.js 16: server component, Node route, edge route | read | absent from `.next`; reading it in Chromium throws `server-only` | inlined |
| Vite 8: SSR build, client build | read in SSR | absent from `dist`; throws in Chromium | inlined |
| Parcel 2.16 | n/a | absent from `dist` (a control `process.env.PORT` was inlined, so Parcel does inline literals); throws in Chromium | inlined |

A bundler configured to inline the whole `process.env` object (`define: { "process.env": ... }`) copies every value into the bundle, whatever the generated file does. `penv run` reads the build output afterwards and fails the build when a secret is in it.

## Where varlock is weaker, and why

- **Startup.** Every varlock command starts a Node.js runtime, bundled into its standalone binary or the one on the machine, and loads its dependency tree. That costs 44× (standalone) to 60× (npm) the time and 6× to 8× the memory of penv on each `run`, and it repeats for every script a `package.json` chains.
- **Values built from secrets.** varlock warns when a non-sensitive value contains a sensitive one, then loads it. With a public prefix, the build inlines it into client code. penv treats a value built from a secret as a secret and refuses a public key built from one.
- **No value transforms.** A password holding `@`, `:` or `/` breaks a connection URL, and varlock has no filter to encode it. penv has `urlencode`, `base64`, `lower`, `upper` and `trim`.
- **Encoded leaks.** `varlock scan` matches the plain value only. `penv scan` also matches base64, hex and URL-encoded forms.
- **Masking stops at the terminal.** Under `varlock run`, output is redacted, but a server's responses are not. penv's preload masks responses in Node.js, Bun, Deno and Python at the socket, keeping byte lengths so `Content-Length` stays valid.
- **Committed secrets.** Nothing in varlock checks that the file holding a secret is gitignored. `penv check` asks git and fails.
