<p align="center"><img src=".github/banner.svg" alt="penv" width="1200"></p>

<p align="center">One static binary. Works before you have an account. The cloud is the upgrade.</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#the-first-minute">First minute</a> ·
  <a href="#with-a-team">With a team</a> ·
  <a href="#coding-agents">Coding agents</a> ·
  <a href="#commands">Commands</a> ·
</p>

---

penv is the open source CLI for Penv Cloud, a [secrets manager](https://penv.cloud) for `.env` files and API keys. penv reads the `.env` you already have, writes a small committed schema next to it, validates every value before your process starts, generates types for your language, and configures your coding agent's harness so it cannot read the file. When you are ready for a team, one command moves the values to [penv.cloud](https://penv.cloud) and deletes the file.

## Install

```bash
curl -fsSL https://penv.cloud/install | sh   # macOS, Linux
```

```powershell
irm https://penv.cloud/install.ps1 | iex     # Windows
```

```bash
npm i -g @penvhq/cli@next                         # any of them, when npm should own it
```

```dockerfile
COPY --from=ghcr.io/penvhq/penv:1 /penv /usr/local/bin/penv   # any Linux image
```

The image is the signed static binary on `scratch`: [packaging/docker](./packaging/docker/README.md).

The first two put one static binary in `~/.penv/bin`, or `%USERPROFILE%\.penv\bin` on Windows, and print the line that adds it to your PATH; no rc file is edited and nothing else is written. `PENV_INSTALL_DIR` moves it, `PENV_VERSION` pins a tag, and every download is checked against the digest the release publishes before it lands. The PowerShell one writes your user PATH when you ask: `-AddToPath` when you run the file, `$env:PENV_ADD_TO_PATH = '1'` when you pipe it through `iex`, which has no flags to pass. Builds: linux and macOS on x86\_64 and arm64, Windows on x86\_64 and arm64.

Every release signs its checksum file with penv's Ed25519 release key. A build that carries a release key refuses a download whose signature does not hold; a build carrying none cannot upgrade at all and is replaced by running an installer above. The shell installer checks the signature wherever OpenSSL 1.1.1 or newer is on PATH, and the PowerShell one says so and installs on the digest, since .NET has no Ed25519.

`penv upgrade` replaces the binary those two installers placed. It refuses on an npm install, where the binary lives inside `node_modules` and `npm i -g @penvhq/cli` is the upgrade, and likewise under Homebrew, Nix, winget and Scoop.

## The first minute

No account, no sign-in.

```bash
penv init                 # reads .env, writes .env.schema, gitignores .env
penv run -- pnpm dev      # validates, injects, masks
```

`init` writes this, and only this, into your repository:

```dotenv
# @schema=1

# @type=url
DATABASE_URL=

# @type=string
STRIPE_SECRET_KEY=

# @type=port @sensitive=false
PORT=3000

# @type=boolean @sensitive=false
DEBUG=true
```

Every key is sensitive and required unless a bundler prefix like `NEXT_PUBLIC_` or a dull value like `3000` or `us-east-1` says otherwise, and a key named for what it holds, such as `STRIPE_SECRET_KEY` or `NEXT_PUBLIC_SUPABASE_ANON_KEY`, keeps its value out whatever the value looks like and whatever prefix it carries. No value that could be a secret is ever copied into the schema. Edit the file if a guess is wrong; the decorators follow the [@env-spec](https://varlock.dev) vocabulary, so a varlock user reads it on sight.

## With a team

```bash
penv login          # device code in the browser
penv push           # values go to the cloud, .env is deleted
```

From then on the cloud is the store and `.env` is a view you can regenerate with `penv pull`. A teammate clones the repo and types `penv run -- pnpm dev`; that is the whole onboarding. CI presents its OIDC token and gets a fifteen minute credential. A server with nothing to present enrols a keypair once.

## Typed access

```bash
penv gen ts        # a typed cast over your runtime's env plus a Standard Schema validator
penv gen py        # penv_env.py on the standard library, or pydantic when your project uses it
```

penv asks where the file goes, once, and never guesses:

```text
where should env.ts go? [apps/web/src/env.ts] (Enter, number, path, none):
```

The suggestions come from the directories that hold a `package.json`, a `pyproject.toml` and so on, shallowest first, with the repository root last so Enter in a monorepo lands on a package. Enter takes the first, a number takes another, a path is your own, and `none` skips. The answer is remembered in `.penv/targets/<name>/target.toml`, which you commit, so nobody is asked twice. `--out <PATH>` answers it up front, and is what a script or CI passes: without a flag or a remembered answer, a non-interactive run writes nothing and says which flag to pass.

`ts` reads through one accessor, `process.env` by default and `import.meta.env` or `Deno.env.get` when a `vite.config.*` or a `deno.json` sits beside it. penv never edits your `tsconfig.json`: it prints the import line to paste, following `extends` and using an alias from your `paths` map when one already reaches the file.

Each target takes a few options, and `penv gen <target> --options` prints them with what they are set to now:

| Target | Option | Values (default in bold) | Changes |
|---|---|---|---|
| `ts` | `key_case` | **`upper`**, `camel` | Property names in the exported object: the environment key, or its camel form. |
| `ts` | `runtime` | **`node`**, `vite`, `deno` | The accessor every key is read through: `process.env`, `import.meta.env` or `Deno.env.get`. |
| `py` | `pydantic` | **`false`**, `true` | Pydantic types for urls and secrets; off keeps the output on the standard library. |

They live in the `[options]` table of `.penv/targets/<name>/target.toml`, the file penv wrote when it remembered where your file goes, each under a comment saying what it changes. Edit a value there and the next `penv gen` uses it.

A language target is a folder holding a `target.toml` and a template. Drop one into `.penv/targets/go/` and `penv gen go` works; a folder holding only a `target.toml` inherits the rest. The binary knows no language by name.

## Local files

penv reads the files dotenv-flow, Next.js and Vite users already have, and any [varlock](https://varlock.dev) `.env.schema`: decorators penv does not act on are warnings, not failures.

```bash
penv run --env production -- node server.js   # .env -> .env.local -> .env.production -> .env.production.local
penv check --env staging                      # same layers, validated; @rotate reminders included
penv scan --install-hook                      # refuse a commit that holds a secret value
```

```dotenv
# .env.schema
# @schema=1 @currentEnv=$APP_ENV

# @type=string(startsWith=sk_) @rotate=90d
STRIPE_SECRET_KEY=

# @type=url
DATABASE_URL=postgres://${DB_HOST}:${DB_PORT:-5432}/app

# @type=url
REPLICA_URL=penv(production/DATABASE_URL)

# @type=url @sensitive=false
POOL_URL=postgres://app:${DB_PASS | urlencode}@pool/app   # still masked: built from DB_PASS

# @type=string
SESSION_SECRET=random(32)                                 # generated on first run, kept in .env.local

# @assert(if(forEnv(production), startsWith($STRIPE_SECRET_KEY, sk_live_), true), "test Stripe key in production")
```

Functions, `${KEY}` expansion and `penv()` addresses: [Design, section 2](./docs/Design.md#values). Under an `@penv=` header, a local file still wins over the cloud on the machine that holds it, and `run` names every key it replaced.

## CI, containers, Lambda

GitHub Actions and GitLab prove themselves with OIDC; ECS, EKS (IRSA and Pod Identity) and Lambda with their AWS role; anything else with `PENV_TOKEN`. Recipes, and the Lambda layer in [`packaging/lambda`](./packaging/lambda/README.md): [Design, deploying](./docs/Design.md#deploying).

## Coding agents

An agent runs as you, so it can read what you can read. penv narrows that:

- Nothing at rest once pushed. There is no `.env` to `cat`.
- `penv run` injects into the child process only, and scrubs every sensitive value from the child's output on every run, in each encoded form [`penv-mask`](./crates/penv-mask/src/lib.rs) lists. `--no-mask` works only for a person at a terminal. For Node, Bun, Deno and Python, a preload also masks what the app hands `console` or `logging` and what it serves over HTTP: [Design, inside the process](./docs/Design.md#inside-the-process).
- A `NEXT_PUBLIC_`, `VITE_`, `PUBLIC_`, `EXPO_PUBLIC_`, `NUXT_PUBLIC_`, `REACT_APP_`, `GATSBY_`, `VUE_APP_` or `STORYBOOK_` key built from a secret fails `check` and `run`. After a build, `run` reads `.next/static`, `dist`, `build`, `out` and the other browser output folders and fails on any secret it finds: [Design, browser safety](./docs/Design.md#browser-safety).
- `penv guard` writes what each harness actually enforces, from the schema: deny rules and a sandbox block for Claude Code, a permission profile for Codex, deny rules and fail-closed hooks for Cursor, and the equivalents for Copilot, Gemini, Cline, Windsurf and Amp. The hook is the penv binary itself, never a script that fails open.
- `reveal` needs a person to approve in the console. An agent can ask; a human clicks.

The claim penv makes, printed by `penv guard --check`, is only what is true: it keeps secrets out of the files, the repo, the shell history and the captured output an agent reads. It cannot stop a process running as you from looking, so every value the cloud issues is short-lived, scoped and attributable to the session that used it.

## Commands

| Command | Does |
|---|---|
| `penv` | State and the one next command |
| `init [--guards NAMES\|--no-guards] [--output PATH]` | `.env` to `.env.schema`, picks the harnesses to guard, generates the typed files |
| `run [--env E] -- cmd` | Layer, compute, validate, inject, mask |
| `check [KEY] [--env E]` | Schema, values, drift, `@rotate` reminders, guard coverage |
| `scan [PATH...] [--staged] [--install-hook]` | Secret values in what git would commit |
| `ls` | Names and types, values masked |
| `gen <target> [--out PATH] [--check] [--options]` | Typed file for a language |
| `guard` | Harness configs from the schema |
| `push` / `pull` | Values to and from the cloud |
| `set` / `unset` | Write a value, never echoed; to the cloud under a header, to `.env` or `.env.<env>` without one |
| `reveal KEY` | One value, after console approval |
| `login` / `logout` | Device code, credential in the OS keychain |
| `machine enroll` | Bind a server keypair |
| `upgrade [--check]` | Replace this binary from the latest release |
| `completions <shell>` | The completion script for your shell |
| `help --json` | The command manifest |

JSON when stdout is not a terminal. Exit codes: 0 ok, 1 error, 2 auth, 3 validation, 4 confirmation required, 5 no credential, 6 environment refused.

## Completions

```bash
penv completions bash > ~/.local/share/bash-completion/completions/penv
penv completions zsh > ~/.zfunc/_penv                  # a directory on your $fpath
penv completions fish > ~/.config/fish/completions/penv.fish
penv completions powershell >> $PROFILE
penv completions elvish >> ~/.config/elvish/rc.elv
```

The scripts are generated from the same manifest `penv help --json` publishes, so they never fall behind the commands.


MIT.
