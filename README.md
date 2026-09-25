<p align="center"><img src=".github/banner.svg" alt="penv" width="1200"></p>

<p align="center">Typed <code>.env</code> validation, masked process output, coding-agent guards. One binary.</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#quick-start">Quick start</a> ·
  <a href="#schema-and-checks">Schema</a> ·
  <a href="#coding-agents-and-sealed-runs">Agents</a> ·
  <a href="#deploy-bundle-and-ci">Deploy</a> ·
  <a href="#compatibility">Compatibility</a> ·
  <a href="#commands">Commands</a>
</p>

---

penv validates your `.env` files against a committed `.env.schema`, starts your command with the resolved values, and masks secrets in its output. penv runs offline from your `.env` files; [penv.cloud](https://penv.cloud), our hosted service, is optional.

## Install

```bash
curl -fsSL https://penv.cloud/install | sh      # macOS, Linux
irm https://penv.cloud/install.ps1 | iex        # Windows (PowerShell)
npm i -g @penvhq/cli@next                       # npm; `next` while 1.0 is in prerelease
```

```dockerfile
COPY --from=ghcr.io/penvhq/penv:next /penv /usr/local/bin/penv
```

The installers put `penv` in `~/.penv/bin` (`%USERPROFILE%\.penv\bin` on Windows) and edit no rc file. Pinning, verification and install directories: [install the CLI](https://penv.cloud/docs/start/install-the-cli). Channels for [`penv upgrade`](https://penv.cloud/docs/cli/upgrade): [upgrade the CLI](https://penv.cloud/docs/cli/upgrade-the-cli).

## Quick start

```console
$ penv init
$ penv run -- sh -c 'echo "key=$STRIPE_SECRET_KEY port=$PORT"'
key=sk▒▒▒▒▒▒ port=3000
$ penv
version    1.0.0-beta.4
location   local
env        development
next       penv check
Values for development come from /path/to/app/.env, later files winning.
```

[`penv init`](https://penv.cloud/docs/cli/init) writes `.env.schema` from your `.env` with a guessed type per key, adds `.env` and `.env.*` to `.gitignore`, and writes `.penv/config.toml`. Commit `.env.schema`. [`penv run`](https://penv.cloud/docs/cli/run) validates, then starts the command.

## Schema and checks

```dotenv
# @type=string(startsWith=sk_) @rotate=90d @docs(https://dashboard.stripe.com/apikeys)
STRIPE_SECRET_KEY=

# @type=enum(development, staging, production) @sensitive=false
APP_ENV=development
```

```console
$ penv check
ok 6 key(s) in /path/to/app/.env.schema for development
rotate STRIPE_SECRET_KEY has @rotate=90d and no recorded write; penv set STRIPE_SECRET_KEY records one
note REDIS_URL is read in src/cache.ts:1 and not declared in .env.schema
```

The grammar is [@env-spec](https://varlock.dev/env-spec/overview/). penv adds `@assert`, `@rotate`, `@hosts` and `@penv`: [schema format](https://penv.cloud/docs/cli/schema-format). [`penv check`](https://penv.cloud/docs/cli/check) also reads your source for undeclared variables; `--strict` fails on them. [`penv why KEY`](https://penv.cloud/docs/cli/why) names where a value comes from, never the value. [`penv lsp`](https://penv.cloud/docs/cli/lsp) serves the schema to editors: [packages/vscode](./packages/vscode/README.md).

## Environments

```text
.env  →  .env.local  →  .env.<env>  →  .env.<env>.local        later file wins
--env  →  PENV_ENV  →  the key @currentEnv names  →  development
```

The `test` environment skips `.env.local`. A variable set in your shell overrides every file: [environments and precedence](https://penv.cloud/docs/concepts/environments-and-precedence).

## Computed values

```dotenv
# @type=url @sensitive=false
DATABASE_URL=postgres://app:${DB_PASSWORD | urlencode}@${DB_HOST:-localhost}:5432/app

# @type=url @sensitive=false
API_URL=match($APP_ENV, production: https://api.example.com, _: http://localhost:4000)

# @type=string(minLength=32)
SESSION_SECRET=random(48)
```

penv computes these before the command starts. A value built from a sensitive key stays masked even when marked `@sensitive=false`; `random(N)` is generated once and kept in `.env.local` (`.env.test.local` for `test`). Functions and filters: [dynamic values](https://penv.cloud/docs/concepts/dynamic-values).

## Masking and client-bundle checks

```console
$ penv run -- npm run build
penv: dist/app.js:1 holds the value of STRIPE_SECRET_KEY, and that file ships to the browser or the app.
```

`penv run` masks each sensitive value of 4 or more characters in the command's output; only a person at a terminal can turn that off with `--no-mask`. A preload masks inside Node, Bun, Deno and Python processes too.

After a command under `penv run` exits 0, penv reads the client output folders it wrote (`.next/static`, `dist`, `build`, React Native bundles) and exits 3 on a secret. `penv check` fails a public key (`NEXT_PUBLIC_`, `VITE_`, ...) built from a secret: [check](https://penv.cloud/docs/cli/check).

## Scanning

```bash
penv scan                   # files git would commit
penv scan --install-hook    # pre-commit hook running penv scan --staged
```

[`penv scan`](https://penv.cloud/docs/cli/scan) reports file, line and key, never the value, and exits 3 on a leak.

## Encryption at rest

```console
$ penv encrypt
$ cat .env
STRIPE_SECRET_KEY=enc:v1:…
PORT=3000
```

[`penv encrypt`](https://penv.cloud/docs/cli/encrypt) encrypts the sensitive values in your `.env` files with a per-user key kept in your OS keychain, or in a key file where there is none; every penv command decrypts where it reads. [`penv decrypt`](https://penv.cloud/docs/cli/decrypt) reverses it.

## Typed access

```bash
penv gen ts        # also py, go, rust, php, java, csharp
```

[`penv gen`](https://penv.cloud/docs/cli/gen) writes a typed loader (`src/env.ts`, `penv_env.py`, `env/env.go`, ...) that fails on a missing or malformed value: [typed env for your language](https://penv.cloud/docs/guides/typed-env-for-your-language).

## Coding agents and sealed runs

```dotenv
# @type=string(startsWith=sk_live_, minLength=32) @hosts=api.stripe.com
STRIPE_SECRET_KEY=
```

[`penv guard`](https://penv.cloud/docs/cli/guard) writes config for Claude Code, Codex, Cursor, Copilot CLI, Gemini CLI, Cline, Windsurf and Amp. Its hooks (Claude Code, Cursor, Gemini CLI, Cline, Windsurf) call [`penv hook`](https://penv.cloud/docs/cli/hook), which refuses reads of `.env` files, environment dumps and `penv pull`.

Under an agent, `penv run` hands a key with `@hosts` to the command as a placeholder, and its proxy puts the real value only into requests to those hosts.

Under a detected agent, penv refuses `penv decrypt` and `penv bundle`, refuses `penv pull` unless a person passes `--i-am-human` at a terminal, and asks you to approve [`penv reveal`](https://penv.cloud/docs/cli/reveal): [coding agents](https://penv.cloud/docs/concepts/coding-agents). Agent Skill: [skills/penv](./skills/penv/SKILL.md).

## penv.cloud sync

```bash
penv login
penv push                           # sends values, deletes the files it sent; *.local stays
penv pull --env staging             # writes .env.staging
```

We store your team's values on [penv.cloud](https://penv.cloud) after [`penv login`](https://penv.cloud/docs/cli/login) and [`penv push`](https://penv.cloud/docs/cli/push); [`penv pull`](https://penv.cloud/docs/cli/pull) writes them back to a file. `@penv=<provider>:org/project` names another provider: [docs/PROVIDERS.md](./docs/PROVIDERS.md).

## Deploy, bundle and CI

```bash
penv bundle --env production                      # .penv/production.bundle; prints PENV_BUNDLE_KEY once
penv run --env production -- node server.js       # on the host, with PENV_BUNDLE_KEY set
```

[`penv bundle`](https://penv.cloud/docs/cli/bundle) ships one environment's values encrypted with the deploy, for hosts without penv.cloud. Docker, serverless, Kubernetes and CI: [deploy](https://penv.cloud/docs/deploy), [packaging/docker](./packaging/docker/README.md), [packaging/lambda](./packaging/lambda/README.md).

## Compatibility

```bash
penv check                  # in a varlock project, unchanged
```

penv reads a [varlock](https://varlock.dev) schema and its `.env.*` files. It ignores varlock-only decorators such as `@plugin` with a note, and refuses `exec()` and plugin functions by name. `penv check` names the penv-only features a schema uses: `@rotate`, `@assert`, `match()`, `random()`, `penv()`, filters.

Against [varlock](https://varlock.dev) 1.20.0 (penv 1.0.0-beta.2), `penv run -- true` takes 5.7 ms and 11 MB where the varlock standalone binary takes 234 ms and 70 MB: [docs/BENCHMARKS.md](./docs/BENCHMARKS.md). To read penv.cloud from [varlock](https://varlock.dev): [packages/varlock-plugin](./packages/varlock-plugin/README.md).

## Commands

| Command | Does |
|---|---|
| [`penv`](https://penv.cloud/docs/cli) | status and the next command |
| [`init`](https://penv.cloud/docs/cli/init) | create `.env.schema` from your `.env` and keep `.env` out of git |
| [`run`](https://penv.cloud/docs/cli/run) | run a command with your secrets loaded into it |
| [`check`](https://penv.cloud/docs/cli/check) | find problems in `.env.schema` and missing values |
| [`scan`](https://penv.cloud/docs/cli/scan) | find secret values committed to files |
| [`ls`](https://penv.cloud/docs/cli/ls) | list your keys and which ones have a value |
| [`why`](https://penv.cloud/docs/cli/why) | where a key's value comes from, never the value |
| [`set`](https://penv.cloud/docs/cli/set) / [`unset`](https://penv.cloud/docs/cli/unset) | save or delete one value |
| [`reveal`](https://penv.cloud/docs/cli/reveal) | show one value; an AI agent needs your approval first |
| [`encrypt`](https://penv.cloud/docs/cli/encrypt) / [`decrypt`](https://penv.cloud/docs/cli/decrypt) | encrypt the secrets in your `.env` files, or write them back in plain text |
| [`bundle`](https://penv.cloud/docs/cli/bundle) | write an encrypted file of one environment's values for a deploy |
| [`gen`](https://penv.cloud/docs/cli/gen) | write the typed file for your language |
| [`guard`](https://penv.cloud/docs/cli/guard) | write the rules that keep AI tools out of `.env` |
| [`hook`](https://penv.cloud/docs/cli/hook) | run as a harness hook |
| [`push`](https://penv.cloud/docs/cli/push) / [`pull`](https://penv.cloud/docs/cli/pull) | send your local `.env` to the cloud, then delete the file / write a `.env` file from the cloud |
| [`login`](https://penv.cloud/docs/cli/login) / [`logout`](https://penv.cloud/docs/cli/logout) | sign in, sign out on this machine |
| [`project`](https://penv.cloud/docs/cli/project), [`env`](https://penv.cloud/docs/cli/env), [`machine`](https://penv.cloud/docs/cli/machine) | projects, environments, identities for servers and CI |
| [`schema`](https://penv.cloud/docs/cli/schema) | print the schema as JSON |
| [`lsp`](https://penv.cloud/docs/cli/lsp) | serve `.env.schema` to an editor over the Language Server Protocol |
| [`upgrade`](https://penv.cloud/docs/cli/upgrade) | replace penv with a newer release |
| [`completions`](https://penv.cloud/docs/cli/completions) | print the completion script for bash, zsh, fish, powershell or elvish |
| [`help`](https://penv.cloud/docs/cli/help) | show help for a command; `penv help --json` prints the manifest |

Output is JSON when stdout is not a terminal: [global options](https://penv.cloud/docs/cli/global-options).

## Exit codes

| Code | Name | Meaning |
|---|---|---|
| 0 | `ok` | the command did what it says |
| 1 | `error` | any other failure |
| 2 | `auth` | not signed in, or the credential was rejected |
| 3 | `validation` | the schema or the values did not pass |
| 4 | `confirmation` | a person has to confirm; the JSON carries the replay command |
| 5 | `no_credential` | no credential is available and none can be obtained |
| 6 | `environment_refused` | this identity may not read that environment, or its values are write-only |

Every error code: [errors](https://penv.cloud/docs/reference/errors).

## Contributing

[CONTRIBUTING.md](./CONTRIBUTING.md) · [SECURITY.md](./SECURITY.md) · [CODE_OF_CONDUCT.md](./CODE_OF_CONDUCT.md)

## License

MIT: [LICENSE](./LICENSE).
