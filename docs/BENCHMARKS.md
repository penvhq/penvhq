# Benchmarks

The scripts are in [`bench/`](../bench). `run.sh` runs penv and [varlock](https://varlock.dev) on the same files; `startup.sh` times `run -- true` for penv, varlock, [dotenvx](https://dotenvx.com) and [dotenv-cli](https://github.com/entropitor/dotenv-cli); `runtimes.sh` runs the file `penv gen ts` writes. Run them on your machine:

```bash
bench/run.sh                  # penv vs varlock: speed, memory, behaviour
bench/startup.sh              # run -- true and peak memory: penv, varlock, dotenvx, dotenv-cli
bench/runtimes.sh             # generated env in Node, Bun, Deno, workerd
bench/runtimes.sh --builds    # plus Next.js, Vite and Parcel builds, checked in headless Chromium
```

| Variable | Default | Sets |
|---|---|---|
| `PENV` | `penv` on PATH for `run.sh`, none for `startup.sh` | the penv binary; for `startup.sh`, adds a row for it |
| `VARLOCK_VERSION` | `1.23.0` | the varlock npm version |
| `VARLOCK` | none | a varlock binary instead, such as the [standalone install](https://varlock.dev/install.sh); for `startup.sh`, adds a row for it |
| `PENV_VERSION`, `DOTENVX_VERSION`, `DOTENV_CLI_VERSION` | `1.0.0`, `2.38.2`, `11.0.0` | the npm versions `startup.sh` installs |
| `DOTENVX` | none | for `startup.sh`, adds a row for a dotenvx binary |
| `RUNS` | `30` | timing iterations |

`run.sh` writes `bench-results.json`; `startup.sh` writes `startup-results.json`.

## Results

penv 1.0.0 (the signed release binary, and `@penvhq/cli` from npm), varlock 1.23.0 from npm, dotenvx 2.38.2 from npm and as its binary (the `@dotenvx/dotenvx-linux-x86_64` package), dotenv-cli 11.0.0 from npm. Linux x86_64, 4 vCPU, Node 22.22, median of 30 runs, 22 October 2026.

### Startup and memory

From `bench/startup.sh`, every tool on the same `.env`:

| Tool | Build | `run -- true` | Peak memory |
|---|---|---|---|
| penv | binary | 10.6 ms | 11 MB |
| penv | npm | 43.1 ms | 45 MB |
| dotenv-cli | npm | 134.7 ms | 56 MB |
| varlock | npm | 166.8 ms | 74 MB |
| dotenvx | npm | 253.3 ms | 81 MB |
| dotenvx | binary | 264.7 ms | 73 MB |

varlock's standalone binary is not in this run; `VARLOCK=<path> bench/startup.sh` adds it. dotenvx and dotenv-cli load `.env` only: they have no schema, validation or scanner, so the rows below are penv against varlock.

### Validate and scan

From `bench/run.sh`:

| | penv binary | varlock npm |
|---|---|---|
| Validate ([`penv check`](https://penv.cloud/docs/cli/check), `varlock load`) | 12.1 ms | 168.7 ms |
| Scan a repository | 6.9 ms | 180.9 ms |

### Behaviour

| Check | penv | varlock |
|---|---|---|
| `NEXT_PUBLIC_` key built from a secret | refused, exit 3 | accepted with a warning |
| Password containing `@ : /` in a URL | valid, via `${DB_PASSWORD \| urlencode}` | invalid URL; no filter |
| `${KEY:-fallback}` with `KEY` unset | resolves to the fallback | fails, exit 1 |
| `@assert` across keys | `run` stops before the command, exit 3 | unknown decorator, exit 1 |
| base64 of a secret in a committed file | found by `penv scan`, exit 3 | missed by `varlock scan` |
| Node.js server returning a secret in a response (`run -- node server.js`) | masked in the response bytes | sent unmasked |
| Python server returning a secret in a response (`run -- python3 server.py`) | masked in the response bytes | sent unmasked |
| `.env` holding a secret, not in `.gitignore` | `penv check` fails, exit 3 | `varlock load` passes |
| `url()`, `json()`, `hash()` in one schema: validate | valid | unsupported, exit 1 |
| `url($DATABASE_URL, host)` | resolves to the host | unsupported, exit 1 |
| `json($DB_SECRET, database)` | resolves to the field | unsupported, exit 1 |
| `hash($KEY, algo=sha256, length=8)` | resolves to an 8-character digest | unsupported, exit 1 |

varlock has no `url()`, `json()` or `hash()`, so its column is whether it loads a schema that uses them.

Each row is a block in [`bench/run.sh`](../bench/run.sh) that builds a fresh project and runs both tools: [`penv scan`](https://penv.cloud/docs/cli/scan), [`penv run`](https://penv.cloud/docs/cli/run).

### Generated env file across runtimes and builds

[`penv gen ts`](https://penv.cloud/docs/cli/gen) writes one `env` for server and client. Public keys are read by literal name (bundlers inline them), every other key by computed name (none do). From `bench/runtimes.sh --builds`:

| Runtime or build | Secret on the server | Secret in the client build | Public key in the client |
|---|---|---|---|
| Node 22 | read | n/a | read |
| Bun 1.4.2 | read | n/a | read |
| Deno 2.9 (`runtime = "node"` and `"deno"`), last run on 1.0.0-rc.1 | read | n/a | read |
| Cloudflare workerd, `nodejs_compat`, date 2025-06-01 | read | n/a | n/a |
| Cloudflare workerd, `runtime = "workers"`, with or without `nodejs_compat` | read | n/a | n/a |
| Next.js 16.4: server component, Node route, edge route | read | absent from `.next`; throws in Chromium | read |
| Vite 8.3: SSR build, client build | read in SSR | absent from `dist`; throws in Chromium | read |
| Parcel 2.16.4 | n/a | absent from `dist` (a control `process.env.PORT` was inlined, so Parcel does inline literals); throws in Chromium | read |

A bundler that inlines the whole `process.env` (`define: { "process.env": ... }`) copies every value; `penv run` then scans the build output and fails on a secret.

## Differences

- **Startup.** Against penv's binary, varlock from npm took 16× its time and 7× its memory per `run` above; dotenvx from npm took 24× and 7×.
- **Encoded leaks.** `penv scan` matches base64, hex, URL-encoded and JSON-escaped forms of a value.
- **Masking.** penv's preload masks responses in Node.js, Bun, Deno and Python, keeping byte lengths so `Content-Length` stays valid.
