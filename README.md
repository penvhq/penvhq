<p align="center"><img src=".github/banner.svg" alt="penv" width="1200"></p>

<p align="center">Typed <code>.env</code> validation, masked process output, coding-agent guards. One static binary.</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#quick-start">Quick Start</a> ·
  <a href="#schema">Schema</a> ·
  <a href="#computed-values">Computed Values</a> ·
  <a href="#output-masking">Output Masking</a> ·
  <a href="#deployment">Deployment</a> ·
  <a href="#migrating-from-varlock">Migrating from varlock</a> ·
  <a href="#commands">Commands</a>
</p>

---

penv validates your `.env` files against a committed `.env.schema`, starts your command with the resolved values, and masks secrets in its output. It needs no account. [penv.cloud](https://penv.cloud) stores values when a team shares them.

## Install

```bash
curl -fsSL https://penv.cloud/install | sh      # macOS, Linux
```

```powershell
irm https://penv.cloud/install.ps1 | iex        # Windows
```

```bash
npm i -g @penvhq/cli@next
```

```dockerfile
COPY --from=ghcr.io/penvhq/penv:1 /penv /usr/local/bin/penv
```

| | |
|---|---|
| Location | `~/.penv/bin`, or `%USERPROFILE%\.penv\bin` on Windows. No rc file is edited; the installer prints the PATH line |
| Verification | release checksum always; checksum signature where OpenSSL 1.1.1+ is available |
| Pin a release | `PENV_VERSION=v1.2.3` |
| Install elsewhere | `PENV_INSTALL_DIR=<dir>` |
| Upgrade | `penv upgrade` |
| Image | [packaging/docker](./packaging/docker/README.md) |

## Quick Start

```bash
penv init
penv run -- npm run dev
```

`init` writes `.env.schema` next to `.env`, with a guessed type per key, and adds `.env` and `.env.*` to `.gitignore`:

```dotenv
# @type=url
DATABASE_URL=

# @type=string
STRIPE_SECRET_KEY=

# @type=port @sensitive=false
PORT=3000

# @type=boolean @sensitive=false
DEBUG=true

# @type=url
NEXT_PUBLIC_API_URL=https://api.example.com
```

Commit `.env.schema`. A schema carries a value only when it is not a secret, such as a port or a public URL.

`penv run` validates, then starts the command:

```console
$ penv run -- sh -c 'echo "key=$STRIPE_SECRET_KEY port=$PORT"'
key=sk▒▒▒▒▒▒ port=3000
```

`penv` with no arguments prints the current state and the next command:

```console
$ penv
version    1.0.0-beta.1
location   local
env        development
next       penv check
Values for development come from .env, later files winning.
```

## Schema

One block per key: decorator comments, then `KEY=default`.

```dotenv
# @type=string(startsWith=sk_) @rotate=90d @docs(https://dashboard.stripe.com/apikeys)
STRIPE_SECRET_KEY=

# @type=enum(development, staging, production) @sensitive=false
APP_ENV=development

# @type=number(isInt=true, min=1, max=64) @sensitive=false
WORKERS=4
```

| Decorator | Effect |
|---|---|
| `@type=` | `string`, `number`, `boolean`, `url`, `email`, `port`, `enum(...)`; constraints such as `startsWith=`, `minLength=`, `isInt=`, `min=`, `max=` |
| `@sensitive` / `@sensitive=false` | masking. Default: sensitive, except keys with a public prefix such as `NEXT_PUBLIC_` |
| `@required` / `@optional` | a key without a default is required unless `@optional` |
| `@required=forEnv(staging, production)` | required in the listed environments only |
| `@rotate=90d` | rotation reminder in `penv check`. Units: `y`, `m` (months), `w`, `d`, `h`, `min`, `s` |
| `@example=`, `@docs(...)`, `@deprecated=` | documentation only |

The vocabulary is [@env-spec](https://varlock.dev). Full reference: [Design, section 2](./docs/Design.md#2-files-in-a-repository).

`penv check` validates without running a command:

```console
$ penv check
ok 7 key(s) in .env.schema for development
rotate STRIPE_SECRET_KEY has @rotate=90d and no recorded write; penv set STRIPE_SECRET_KEY records one
```

## Environments

File order matches [dotenv-flow](https://github.com/kerimdzhanov/dotenv-flow), [Next.js](https://nextjs.org/docs/app/guides/environment-variables) and [Vite](https://vite.dev/guide/env-and-mode). A later file overrides an earlier one:

```text
.env  →  .env.local  →  .env.<env>  →  .env.<env>.local
```

`*.local` files are never pushed. The `test` environment skips `.env.local`.

Environment selection, first match wins:

```text
--env  →  PENV_ENV  →  the key @currentEnv names  →  development
```

```dotenv
# @currentEnv=$APP_ENV

# @type=enum(development, staging, production) @sensitive=false
APP_ENV=development
```

```bash
penv run --env production -- node server.js
APP_ENV=staging penv run -- node server.js
```

The `@currentEnv` key holds the selected environment inside the process. A variable set in the shell overrides every file.

## Computed Values

Computed before the command starts:

```dotenv
# @currentEnv=$APP_ENV

# @type=enum(development, staging, production) @sensitive=false
APP_ENV=development

# @type=string
DB_PASSWORD=

# @type=url @sensitive=false
DATABASE_URL=postgres://app:${DB_PASSWORD | urlencode}@${DB_HOST:-localhost}:5432/app

# @type=url @sensitive=false
API_URL=match($APP_ENV, production: https://api.example.com, staging: https://staging.example.com, _: http://localhost:4000)

# @type=string(minLength=32)
SESSION_SECRET=random(48)
```

| Feature | Syntax |
|---|---|
| Expansion ([dotenv-expand](https://github.com/motdotla/dotenv-expand) rules) | `${KEY}`, `$KEY`, `${KEY:-fallback}`, `${KEY-fallback}`; `\$` is a literal dollar; single-quoted values are not expanded |
| Filters | `urlencode`, `base64`, `lower`, `upper`, `trim` |
| Functions | `match`, `if`, `eq`, `not`, `and`, `or`, `fallback`, `concat`, `isEmpty`, `startsWith`, `endsWith`, `forEnv`, `ref` |
| `random(N)` | generated on the first `penv run`, stored in `.env.local`, never pushed |
| `penv(env/KEY)`, `penv(project/env/KEY)` | another environment's or project's value: from local files, or from penv.cloud after `penv push` writes the `@penv=` header |

`match` with no matching case and no `_` case is an error.

A value built from a sensitive key is masked even when marked `@sensitive=false`; `penv check` reports it. A key read only as a condition does not propagate: `API_URL` branches on `APP_ENV` and copies nothing from it.

### Assertions

```dotenv
# @assert(not(eq($PORT, $ADMIN_PORT)), "PORT and ADMIN_PORT collide")
# @assert(if(forEnv(production), startsWith($STRIPE_SECRET_KEY, sk_live_), true), "test Stripe key in production")
```

A false assertion fails `penv check`, and stops `penv run` before the command starts:

```console
$ penv run --env production -- node server.js
fail test Stripe key in production
```

## Output Masking

`penv run` masks on every run, in terminals, CI and agent sessions. `--no-mask` and `--no-preload` apply only when a person runs penv at a terminal. Through a pipe or under an agent they are ignored, with a warning.

A preload masks values inside the process, which the output stream does not reach. Nothing to install:

| Runtime | Masked |
|---|---|
| Node.js, and anything launched through it (Next.js, Vite, Nuxt, Remix, Astro, tsx, npm scripts) | `console`, including wrappers installed later (Sentry, pino); server responses, down to raw socket writes |
| Bun | same, including `Bun.serve` |
| Deno | same, including `Deno.serve` |
| Python | `logging` records; bytes sent on accepted connections |

Masked response bodies keep their byte length, so `Content-Length` stays valid. Outbound requests are not modified. [Design, inside the process](./docs/Design.md#inside-the-process).

### Client Bundle Checks

Public prefixes: `NEXT_PUBLIC_`, `VITE_`, `PUBLIC_`, `EXPO_PUBLIC_`, `NUXT_PUBLIC_`, `REACT_APP_`, `GATSBY_`, `VUE_APP_`, `STORYBOOK_`. A public key built from a sensitive value fails:

```console
$ penv check
fail NEXT_PUBLIC_CHECKOUT is built from a sensitive value, and its NEXT_PUBLIC_ prefix sends it to the browser. Compute it on the server, or rename it without the prefix.
```

After a build exits 0 under `penv run`, penv reads the files written to `.next/static`, `dist`, `build`, `out` and the other client output folders, including React Native bundles and Hermes bytecode. A secret found there sets exit code 3:

```console
$ penv run -- npm run build
penv: dist/app.js:1 holds the value of STRIPE_SECRET_KEY, and that file ships to the browser or the app.
```

Additional prefixes, such as a custom Vite `envPrefix`, go in `.penv/config.toml`:

```toml
[public]
prefixes = ["APP_PUBLIC_"]
```

### Repository Scanning

```bash
penv scan                   # files git would commit
penv scan --install-hook    # pre-commit hook
```

```console
$ penv scan
leak app.js:1 holds the value of STRIPE_SECRET_KEY
```

Matches raw, base64, hex and URL-encoded forms. Reports file, line and key; never the value.

`penv check` fails when a value file holding a sensitive value is tracked by git or not ignored; `penv run` warns. `penv init` in a folder that already has `.env.schema` keeps the schema and adds the ignore lines. `penv set` adds them when it writes a secret into a file git would pick up.

## Typed Access

```bash
penv gen ts
penv gen py
```

`gen ts` writes one typed `env` and a [Standard Schema](https://standardschema.dev) validator. The same import works in server and client code:

```ts
import { env } from "@/env";   // the path gen prints

env.STRIPE_SECRET_KEY;          // server: read at runtime; in the browser it throws "server-only"
env.NEXT_PUBLIC_API_URL;        // client and server: inlined by the bundler
```

Public keys are read by their literal name (`process.env.NEXT_PUBLIC_API_URL`), the form bundlers inline. Every other key is read by computed name (`process.env[name]`), which no bundler inlines, so a secret's value never reaches client code whatever the bundler. Checked with real builds of Next.js 16 (Node and edge routes), Vite 8 (client and SSR) and Parcel 2.16, in Chromium, and in workerd, Node 22, Bun 1.4 and Deno 2.9.

| `runtime` option | Reads from |
|---|---|
| `node` (default) | `process.env`, then `Deno.env`, then `Netlify.env`: Node, Bun, Deno, Next.js, Netlify Edge, Workers with `nodejs_compat` |
| `vite` | public keys from `import.meta.env`; the rest as `node` (SSR) |
| `deno` | `Deno.env.get` |
| `workers` | `import { env } from "cloudflare:workers"`: any Workers configuration |

A bundler configured to inline the whole environment (`define: { "process.env": … }`) copies every value into the bundle whatever penv generates; the build scan fails that build.

`gen py` writes `penv_env.py`, on the standard library or with pydantic types. The output path is asked once and kept in `.penv/config.toml`, with the target's options (commit it). `--out PATH` skips the prompt.

```toml
# .penv/config.toml
[targets.ts]
output = "src/env.ts"

[targets.ts.options]
key_case = "upper"
runtime = "node"
```

`.penv/<name>.tmpl` overrides a target's template. A language penv does not ship is a `[targets.<name>]` section with `output`, `detect` and `types`, plus `.penv/<name>.tmpl`. [Design, language targets](./docs/Design.md#7-language-targets).

## Coding Agents

```bash
penv guard
```

Writes deny rules for `.env` and `.env.*` for Claude Code, Codex, Cursor, Copilot CLI, Gemini CLI, Cline, Windsurf and Amp. Hooks call the penv binary, so a hook failure denies. [Design, agents](./docs/Design.md#6-agents).

An [Agent Skill](./skills/penv/SKILL.md) tells the agent how to work in a penv project: commands, exit codes, and how to fix each `penv check` failure without touching values. For Claude Code, copy it into the project:

```bash
mkdir -p .claude/skills && cp -r path/to/penvhq/skills/penv .claude/skills/
```

In an agent session penv:
- prints JSON
- ignores `--no-mask` and `--no-preload`
- routes `penv reveal KEY` to a person for approval in the penv.cloud console
- refuses `penv pull`
- refuses a CA bundle the current user can write

## penv.cloud

```bash
penv login
penv push                           # sends values, deletes the files it sent; *.local stays
penv set STRIPE_SECRET_KEY          # hidden input
penv pull --env staging             # writes .env.staging
penv reveal STRIPE_SECRET_KEY       # one value; agents need approval
```

penv.cloud is provider `penv`, the default. `@penv=<provider>:org/project` names another provider, `--provider` overrides it for one command, and `[providers.<slug>] url` in `.penv/config.toml` points at a self-hosted API: [providers](./docs/PROVIDERS.md).

After `push`, teammates run `penv login` and the same `penv run` command. A local file overrides the cloud value on that machine; `penv run` names each overridden key.

## Deployment

Credential per platform ([Design, deploying](./docs/Design.md#deploying)):

| Platform | Credential |
|---|---|
| GitHub Actions, GitLab | job OIDC token, exchanged for a 15-minute credential |
| ECS, EKS (IRSA, Pod Identity), Lambda | the AWS role |
| anything else | `PENV_TOKEN` |

### CI

```yaml
permissions:
  id-token: write
steps:
  - uses: actions/checkout@v4
  - run: curl -fsSL https://penv.cloud/install | sh
  - run: penv check --env staging
  - run: penv scan
  - run: penv run --env staging -- npm test
  - run: penv run --env staging -- npm run build
```

With no credential, `penv check` validates local files and reports that the cloud was not read.

### Docker

```dockerfile
FROM node:22-slim
COPY --from=ghcr.io/penvhq/penv:1 /penv /usr/local/bin/penv
WORKDIR /app
COPY . .
RUN npm ci
RUN --mount=type=secret,id=penv_token,env=PENV_TOKEN \
    penv run --env production -- npm run build
USER node
ENTRYPOINT ["penv", "run", "--env", "production", "--"]
CMD ["node", "server.js"]
```

```bash
docker build --secret id=penv_token,env=PENV_TOKEN -t app .
docker run -e PENV_TOKEN -p 3000:3000 app
```

Values load at container start; no layer holds them. Exclude `.env*` except `.env.schema` in `.dockerignore`.

### AWS Lambda

Layer and wrapper: [packaging/lambda](./packaging/lambda/README.md). Function configuration:

```text
AWS_LAMBDA_EXEC_WRAPPER=/opt/penv-wrapper
PENV_ENV=production
```

Managed runtimes: Node.js, Python, Java, .NET, Ruby. Vercel, Netlify and Cloudflare do not run penv beside the code; they receive values from penv.cloud.

### Custom CA Bundle

penv trusts its built-in root certificates. For a network that re-signs TLS:

```bash
export SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
```

## Migrating from varlock

penv reads a [varlock](https://varlock.dev) project unchanged: `.env.schema`, `.env.*` files, `@currentEnv`, `@import(path, pick=[...])`, and the functions `ref`, `concat`, `fallback`, `if`, `eq`, `not`, `isEmpty`, `forEnv`. Unsupported decorators such as `@generateTypes` and `@plugin` produce notes, not errors.

```bash
penv check
penv run -- npm run dev
```

| varlock | penv |
|---|---|
| plugin functions, e.g. `op(...)` | error naming the function; single-quote the value to pass it literally |
| `exec(...)` | refused; a schema never starts a process |

Measured on one project against varlock 1.20.0 ([method and script](./docs/BENCHMARKS.md)):

| | penv | varlock |
|---|---|---|
| `run -- true` | ~6 ms | ~370 ms |
| Peak memory | 11 MB | 95 MB |
| URL built from a secret | masked | warning; value exposed |
| Password containing `@` in a URL | `${DB_PASSWORD \| urlencode}` | invalid URL |
| `${KEY:-fallback}` | supported | fails |
| `@assert`, `@rotate` | enforced | rejected as unknown decorators |
| base64-encoded secret in a committed file | found by `penv scan` | missed by `varlock scan` |
| `NEXT_PUBLIC_` key built from a secret | refused | accepted with a warning |
| Server response holding a secret (Node.js, Python) | masked | sent unmasked |
| `.env` with a secret, not gitignored | `check` fails | passes |

Also in penv: a static binary with no Node.js, and masking inside Python processes.

A schema using `@assert`, `@rotate`, `match`, `random`, filters or `penv()` no longer loads in varlock. `penv check` lists which of these the schema uses.

## Commands

| Command | Does |
|---|---|
| `penv` | status and next command |
| `init` | `.env.schema` from `.env`; gitignore `.env*` |
| `run [--env E] -- cmd` | validate, compute, run, mask |
| `check [KEY] [--env E]` | validate; assertions, rotation, client bundle checks |
| `scan [PATH...] [--staged] [--install-hook]` | secret values in files |
| `ls` | keys, types, presence |
| `gen ts\|py` | typed file |
| `guard` | agent deny rules |
| `set KEY` / `unset KEY` | write or remove one value |
| `push` / `pull` | sync with penv.cloud |
| `reveal KEY` | show one value; agents need approval |
| `login` / `logout` | penv.cloud session |
| `project`, `env`, `machine` | cloud projects, environments, server identities |
| `upgrade` | replace this binary |
| `completions <shell>` | bash, zsh, fish, powershell, elvish |

JSON output when stdout is not a terminal.

| Exit code | Meaning |
|---|---|
| 0 | ok |
| 1 | error |
| 2 | authentication |
| 3 | validation failed |
| 4 | confirmation required |
| 5 | no credential |
| 6 | environment refused |

## Contributing

[CONTRIBUTING.md](./CONTRIBUTING.md) · [SECURITY.md](./SECURITY.md) · [CODE_OF_CONDUCT.md](./CODE_OF_CONDUCT.md)

## License

MIT.
