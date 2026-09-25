# Benchmarks

Both scripts are in [`bench/`](../bench) and run penv and [varlock](https://varlock.dev) on the same files, on your machine:

```bash
bench/run.sh                  # penv vs varlock: speed, memory, behaviour
bench/runtimes.sh             # generated env in Node, Bun, Deno, workerd
bench/runtimes.sh --builds    # plus Next.js, Vite and Parcel builds, checked in headless Chromium
```

| Variable | Default | Sets |
|---|---|---|
| `PENV` | `penv` on PATH | the penv binary |
| `VARLOCK_VERSION` | `1.20.0` | the varlock npm version |
| `VARLOCK` | none | a varlock binary instead, such as the [standalone install](https://varlock.dev/install.sh) |
| `RUNS` | `30` | timing iterations |

`run.sh` writes `bench-results.json`.

## Results

penv 1.0.0-beta.2 release build, [varlock](https://varlock.dev) 1.20.0 from npm and as its standalone binary, Linux x86_64, Node 22.22, median of 30 runs.

### Speed and memory

| | penv | varlock standalone | varlock from npm |
|---|---|---|---|
| `run -- true` | 5.7 ms | 234 ms | 386 ms |
| Validate (`penv check`, `varlock load`) | 6.4 ms | 174 ms | 343 ms |
| Scan a repository | 3.9 ms | 177 ms | 359 ms |
| Peak memory, `run -- true` | 11 MB | 70 MB | 94 MB |
| Binary | 12.7 MB, static (x86_64 Linux) | 105 MB, bundles Node.js | the npm package on Node.js |

Since 1.0.0-beta.2, [`penv check`](https://penv.cloud/docs/cli/check) also reads your source for undeclared variables: 2 ms over beta.1's 4.2 ms. Absolute times vary by machine; `run.sh` reproduces the ratio.

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

Each row is a block in [`bench/run.sh`](../bench/run.sh) that builds a fresh project and runs both tools: [`penv scan`](https://penv.cloud/docs/cli/scan), [`penv run`](https://penv.cloud/docs/cli/run).

### Generated env file across runtimes and builds

[`penv gen ts`](https://penv.cloud/docs/cli/gen) writes one `env` for server and client. Public keys are read by literal name (bundlers inline them), every other key by computed name (none do). From `bench/runtimes.sh --builds`:

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

A bundler that inlines the whole `process.env` (`define: { "process.env": ... }`) copies every value; `penv run` then scans the build output and fails on a secret.

## Differences

- **Startup.** Every [varlock](https://varlock.dev) command starts Node.js and loads its dependencies: 41× to 68× penv's time and 6× to 8.5× its memory per `run`.
- **Encoded leaks.** `penv scan` matches base64, hex and URL-encoded forms; [`varlock scan`](https://varlock.dev) the plain value only.
- **Masking.** penv's preload masks responses in Node.js, Bun, Deno and Python, keeping byte lengths so `Content-Length` stays valid.
