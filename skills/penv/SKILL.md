---
name: penv
description: Work in a project whose environment variables are managed by penv (a .env.schema file, the penv CLI, or penv.cloud). Use when running, testing or building the project, adding or changing an environment variable, fixing a missing or invalid value, or when a command fails with a penv error. Never read .env files directly.
---

# Working with penv

The project declares its environment in `.env.schema`. Values live in `.env*` files or in penv.cloud. You work with names and types; you do not handle values.

## Rules

- Do not read, print, copy or grep `.env`, `.env.local`, `.env.<env>` or `.env.*.local`. `.env.schema` is safe to read and edit.
- Run the project through penv: `penv run -- <command>`. Output is masked, so a value shows as `sk▒▒▒▒▒▒`. That is expected. Do not try to recover it.
- Do not pass `--no-mask` or `--no-preload`. Under an agent they are ignored.
- Do not set `SSL_CERT_FILE` to a file you wrote. penv refuses it in agent sessions.
- If you need a secret's value, stop and ask the user. `penv reveal KEY` asks the user for approval in penv.cloud; it is not a way around them.

## Commands

| Goal | Command |
|---|---|
| Run, test, build | `penv run -- npm run dev` (or `--env staging` / `--env production`) |
| Find what is wrong | `penv check` (add `--env E` for another environment) |
| List keys and whether each has a value | `penv ls` |
| Where a key's value comes from, what it is built on, how penv treats it | `penv why KEY` |
| Variables the code reads that `.env.schema` lacks | `penv check` (notes), `penv check --strict` (fails) |
| Read the schema as JSON | `penv schema` |
| Current state and next step | `penv` |
| Every command, flag and exit code | `penv help --json` |

Output is JSON when stdout is not a terminal or when you pass `--agent`.

## Exit codes

| Code | Meaning | What to do |
|---|---|---|
| 0 | ok | |
| 1 | error | read `message` and `fix` in the JSON |
| 2 | authentication | ask the user to run `penv login` |
| 3 | validation failed | run `penv check` and fix the schema or ask for the value |
| 4 | confirmation required | the user must approve; wait |
| 5 | no credential | ask the user to run `penv login`, or to provide `PENV_TOKEN` in CI |
| 6 | environment refused | the user's account may not read that environment, or the keys named are write-only (`redacted`); ask them |

## Adding an environment variable

1. Add a block to `.env.schema`:

   ```dotenv
   # @type=url
   WEBHOOK_URL=
   ```

   Types: `string`, `number`, `boolean`, `url`, `email`, `port`, `enum(a, b)`. A non-secret default can go after `=`, with `@sensitive=false`. Keys are secret unless marked `@sensitive=false` or named with a browser prefix (`NEXT_PUBLIC_`, `VITE_`, `PUBLIC_`, `EXPO_PUBLIC_`, `NUXT_PUBLIC_`, `REACT_APP_`, `GATSBY_`, `VUE_APP_`, `STORYBOOK_`).
2. Ask the user to set the value: `penv set WEBHOOK_URL` (it prompts, hidden).
3. `penv check`.

A value you generate yourself, such as a random local secret, can use the schema instead: `SESSION_SECRET=random(48)`.

## Fixing `penv check` failures

| Failure | Fix |
|---|---|
| `KEY is required and has no value` | ask the user to `penv set KEY`, or give a non-secret default in the schema |
| `KEY must be …` (type, format, range) | the value is wrong; tell the user which key and rule, never guess the value |
| `@assert … ` message | a rule across keys failed; tell the user the message |
| `NEXT_PUBLIC_X is built from a sensitive value…` | the browser would receive a secret. Compute it on the server, or rename it without the prefix |
| `… holds KEY and is not in .gitignore` | run `penv init`; it keeps the schema and adds the ignore lines |
| `… holds KEY and git tracks it` | tell the user: the file must be removed from git and the values rotated |
| `op() is not a function penv runs` | a varlock plugin call; ask the user where the value should come from |

## Build output

`penv run -- <build>` exits 3 when the build writes a secret into browser output (`.next/static`, `dist`, `build`, `out`, React Native bundles). The fix is in code: read the value on the server, not in client code.

## Typed access

Use the generated file instead of raw `process.env`:

```ts
import { env } from "@/env"; // the path `penv gen ts` prints
```

The same `env` works in server and client code. Reading a secret in the browser throws `… is server-only`: move that code to the server. After changing `.env.schema`, run `penv gen ts`, or the target the repository uses: `py`, `go`, `rust`, `php`, `java`, `csharp` (`penv gen` with no name lists them). In Go, Rust, PHP, Java and C#, a secret field prints `[redacted]`; read it with `.Value()`, `.expose()`, `->expose()` or `.Expose()` only where the code needs the value.

A key with `@hosts` holds a placeholder in your commands, not the value. Do not try to recover the value: send requests to the named host and penv puts it in. If a request to another host needs the key, the schema's `@hosts` is what to change, and a person should approve that.
