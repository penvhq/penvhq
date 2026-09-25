---
name: penv
description: Work in a project whose environment variables are managed by penv (a .env.schema file, the penv CLI, or penv.cloud). Use when running, testing or building the project, adding or changing an environment variable, fixing a missing or invalid value, or when a command fails with a penv error. Never read .env files directly.
---

# Working with penv

`.env.schema` declares the environment. Values live in `.env*` files or on penv.cloud, our hosted service. You handle names and types, never values.

## Rules

- Never read, print, copy or grep `.env` or `.env.*`. `.env.schema` is safe to read and edit.
- Run the project through [`penv run`](https://penv.cloud/docs/cli/run): `penv run -- <command>`. Output is masked (`sk▒▒▒▒▒▒`); never try to recover a value. `--no-mask` and `--no-preload` are ignored under an agent.
- Do not set `SSL_CERT_FILE` to a file you can write; penv does not trust it in an agent session.
- Need a value? Stop and ask the user. The penv hook refuses [`penv reveal`](https://penv.cloud/docs/cli/reveal) and [`penv pull`](https://penv.cloud/docs/cli/pull). Without the hook, `penv reveal KEY` exits 4 with `approval` and `url`; once they approve, `penv reveal KEY --approval ID` prints it once.

## Commands

| Goal | Command |
|---|---|
| Run, test, build | `penv run -- npm run dev` (`--env production`) |
| Find what is wrong | [`penv check`](https://penv.cloud/docs/cli/check) (`--strict` fails on undeclared variables the code reads) |
| Keys and which have values | [`penv ls`](https://penv.cloud/docs/cli/ls) |
| Where a value comes from | [`penv why KEY`](https://penv.cloud/docs/cli/why) |
| The parsed schema as JSON | [`penv schema`](https://penv.cloud/docs/cli/schema) |
| State and `next` command | `penv` |
| Every command, flag and exit code | [`penv help --json`](https://penv.cloud/docs/cli/help) |

Output is JSON under an agent or when stdout is not a terminal. A failure prints `{"error", "message", "fix"}` on stderr.

## Exit codes

| Code | Meaning | Do |
|---|---|---|
| 1 | error | read `message` and `fix` |
| 2 | auth failed | ask the user to run [`penv login`](https://penv.cloud/docs/cli/login) |
| 3 | validation failed | run `penv check` |
| 4 | a person must confirm | give the user the `url`; wait |
| 5 | no credential | same; in CI, `PENV_TOKEN` |
| 6 | environment refused, or keys write-only (`redacted`) | ask the user |

## Add an environment variable

1. Add a block to `.env.schema`:

   ```dotenv
   # @type=url
   WEBHOOK_URL=
   ```

   Types: `string`, `number`, `boolean`, `url`, `email`, `port`, `enum(a, b)`. A key is secret unless `@sensitive=false` or browser-prefixed (`NEXT_PUBLIC_`, `VITE_`, ...). A non-secret default goes after `=`.
2. Ask the user to run [`penv set WEBHOOK_URL`](https://penv.cloud/docs/cli/set).
3. Run `penv check`.

For a generated local secret, write `SESSION_SECRET=random(48)`; penv keeps it in `.env.local`.

## Fix `penv check` failures

| Message | Fix |
|---|---|
| `KEY is required and has no value.` | ask the user to `penv set KEY` |
| `KEY must be ...`, `@assert line N` | tell the user; never guess a value |
| `KEY is built from a sensitive value, and its PREFIX prefix ...` | compute it on the server, or drop the prefix |
| `FILE holds KEY and is not in .gitignore` | run [`penv init`](https://penv.cloud/docs/cli/init) |
| `FILE holds KEY and git tracks it` | tell the user |
| `op() is not a function penv runs` | a [varlock](https://varlock.dev/reference/functions/) plugin call; ask the user |
| `KEY in ENV is write-only in penv-cloud.` | ask the user |

## Build output

`penv run -- <build>` exits 3 when the build writes a secret into browser output (`.next/static`, `dist`, `build`, `out`, React Native bundles). Read it on the server.

## Typed access

```ts
import { env } from "@/env"; // the path `penv gen ts` prints
```

Use it instead of `process.env`. A secret read in the browser throws `KEY is server-only`. After editing `.env.schema`, rerun [`penv gen`](https://penv.cloud/docs/cli/gen) for the repository's target (no target lists them).

In Go, Rust, PHP, Java and C#, a secret prints `[redacted]`; `.Value()`, `.expose()`, `->expose()` or `.Expose()` returns it.

A key with `@hosts` is a placeholder in your commands; penv puts the value into requests to those hosts. Do not change `@hosts` without asking.
