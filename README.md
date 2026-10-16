<p align="center">
  <img src=".github/banner.svg" alt="penv" width="640">
</p>

<h3 align="center">Typed, validated, agent-safe environment variables.<br>One static binary. No account needed.</h3>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#quickstart">Quickstart</a> ·
  <a href="#what-penv-covers">What it covers</a> ·
  <a href="#the-envschema-file">Schema</a> ·
  <a href="#coding-agents">Coding agents</a> ·
  <a href="#commands">Commands</a> ·
  <a href="https://penv.cloud/docs">Docs</a>
</p>

---

penv checks your `.env` against a small committed schema **before your process starts**, keeps secret values out of logs and out of your coding agent's reach, and generates typed access for your language. When a team needs to share values, one command moves them to [penv.cloud](https://penv.cloud). Until then, everything works offline and locally.

```text
   .env  ──►  penv init  ──►  .env.schema   (committed, holds no secrets)
                                   │
        ┌──────────────┬───────────┼────────────────┬──────────────────┐
        ▼              ▼           ▼                ▼                  ▼
    penv check     penv run     penv gen        penv guard        penv push
    validate       inject +     typed code      deny rules for    values to
    every value    mask output  for 7 langs    8 agent harnesses penv.cloud
```

## What penv covers

| You want to… | penv gives you |
|---|---|
| **Stop shipping broken config** | A schema with types, required/optional and sensitive flags. `penv check` and `penv run` fail before your app starts, with line and column. |
| **Keep secrets out of logs** | `penv run` masks every sensitive value in output (raw, hex, base64 and URL-encoded) and inside Node, Bun, Deno and Python processes. |
| **Keep secrets away from AI agents** | `penv guard` writes deny rules and fail-closed hooks for Claude Code, Codex, Cursor, Copilot CLI, Gemini CLI, Cline, Windsurf and Amp. Sealed runs hand the command a placeholder instead of the real value. |
| **Stop `process.env.FOO!`** | `penv gen` writes a typed accessor for TypeScript, Python, Go, Rust, PHP, Java or C#. |
| **Compute values** | `${KEY}` expansion, filters, `if`/`match`, `random(N)`, cross-environment references, `@assert`. |
| **Share with a team** | `penv push` / `penv pull`, CI login without long-lived tokens (OIDC), machine enrolment for servers. |
| **Deploy without a cloud** | `penv bundle` encrypts one environment into a file that `penv run` reads with a key. |
| **Edit schemas comfortably** | `penv lsp` and a VS Code extension: diagnostics, completion, hover, go-to-definition. |

## Install

```sh
curl -fsSL https://penv.cloud/install | sh      # macOS, Linux
irm https://penv.cloud/install.ps1 | iex        # Windows (PowerShell)
npm i -g @penvhq/cli@next                       # anywhere npm should own it
docker pull ghcr.io/penvhq/penv:next            # signed, minimal image
```

<details>
<summary><b>Install details</b></summary>

- The shell and PowerShell installers put one binary in `~/.penv/bin` (`%USERPROFILE%\.penv\bin` on Windows) and print the PATH line to add. No rc file is edited. `PENV_INSTALL_DIR` moves it; `PENV_VERSION` pins a tag.
- Builds: Linux and macOS on x86_64 and arm64; Windows on x86_64 and arm64.
- Every download is checked against the release's published digest. Each release also signs its checksum file with an Ed25519 key; the shell installer verifies it wherever OpenSSL 1.1.1 or newer is on `PATH`, and the PowerShell installer verifies the digest.
- `penv upgrade [--check]` replaces a binary placed by the installers. It refuses on npm, Homebrew, Nix, winget and Scoop installs, where the package manager owns the upgrade.
- Deploy targets: ECS task roles, EKS IRSA and Pod Identity, an AWS Lambda layer, and a signed `scratch` Docker image.
- Shell completions: `penv completions bash|zsh|fish|powershell|elvish`.

</details>

## Quickstart

No account. No sign-in.

```sh
penv init                 # reads .env, writes .env.schema, gitignores .env and .env.*
penv run -- pnpm dev      # validates, injects, masks
```

`penv init` also detects the coding agents installed on your machine and offers to write their deny rules (`--guards NAMES` or `--no-guards` to decide up front), and can generate typed files for your language.

Run bare `penv` at any time: it shows the state of the project and the **one next command** to run.

## The `.env.schema` file

`init` writes this, and only this, into your repo. **No value that could be a secret is ever copied into the schema.**

```sh
# @currentEnv=$APP_ENV

# @type=enum(development,staging,production)
APP_ENV=development

# @type=url
DATABASE_URL=

# @type=string(startsWith=sk_) @rotate=90d @hosts(api.stripe.com)
STRIPE_SECRET_KEY=

# @type=port @sensitive=false
PORT=3000

# @type=url @sensitive=false
NEXT_PUBLIC_APP_URL=http://localhost:3000
```

The format is [`@env-spec`](https://varlock.dev/env-spec/overview/), the same one [varlock](https://varlock.dev) reads, so schemas move between the tools. penv ignores what it doesn't know with a warning, so a varlock schema still parses.

**Defaults you don't have to write**

- `KEY=` (empty) is **required**. `KEY=value` makes `value` the default and the key optional.
- Every key is **sensitive** unless it carries `@sensitive=false`, the header sets `@defaultSensitive=false`, or the name starts with a bundler-public prefix: `NEXT_PUBLIC_`, `VITE_`, `PUBLIC_`, `EXPO_PUBLIC_`, `NUXT_PUBLIC_`, `REACT_APP_`, `GATSBY_`, `VUE_APP_`, `STORYBOOK_`, or any you add under `[public] prefixes` in `.penv/config.toml`.
- A key named like a secret (`STRIPE_SECRET_KEY`, `…_ANON_KEY`) stays sensitive whatever it holds.
- A bundler-prefixed key marked `@sensitive` is an error (`sensitive_public_key`): the prefix ships the value to the browser, so penv cannot keep it secret.

<details>
<summary><b>Decorators</b></summary>

| Decorator | Where | Takes |
|---|---|---|
| `@penv` | header | `org/project` or `<provider>:org/project`. Present means cloud mode; absent means local mode. |
| `@currentEnv` | header | A key, e.g. `$APP_ENV`. Chooses the environment. |
| `@defaultRequired` | header | nothing, `true`, `false`, `infer` |
| `@defaultSensitive` | header | nothing, `true`, `false` |
| `@import` | header | `(path, KEY, …)` or `(path, pick=[KEY, …])`. Schema files only, never value files. |
| `@assert` | header or key | `(expression, "message")`. A false expression fails `check` and `run`. |
| `@required` / `@optional` | key | nothing, `true`, `false`, or `forEnv(env, …)` |
| `@sensitive` | key | nothing, `true`, `false` |
| `@type` | key | one of the types below |
| `@example`, `@docs`, `@deprecated` | key | a value / a note |
| `@hosts` | key | one host, or `(a.com, "*.b.com")`: at most 32, no scheme, port, path or bare `*`. Enables [sealed runs](#sealed-runs). |
| `@rotate` | key | A duration (`90d`, `1y6m`, `12h`, `30min`), largest unit first. `penv check` prints a reminder counted from the last write; it never changes the exit code. |

`@dynamic` and `@static` are parsed and kept; penv acts on neither. Anything else is ignored with a warning (`unknown_decorator`, or `varlock_only` for varlock's `@plugin` and similar).

**Types**

| Type | Constraints |
|---|---|
| `string` | `startsWith`, `endsWith`, `minLength`, `maxLength`, `matches` |
| `number` | `min`, `max`, `isInt`, `precision` |
| `port` | `min`, `max` |
| `boolean`, `url`, `email` | none |
| `enum(a,b,c)` | none; bare arguments are the members |

A key with no `@type` is a `string`. There is no `integer`; write `number(isInt=true)`.

> **Heads up:** `matches` and `precision` are carried into generated types and sent to penv.cloud, but penv does not enforce them locally. A failing `matches` will not fail `penv check`.

</details>

<details>
<summary><b>Computed values, filters and functions</b></summary>

A value that contains `${KEY}`, or is a single function call, is computed. Single-quoted and backticked values are always literal.

```sh
DATABASE_URL=postgres://app:${DB_PASSWORD | urlencode}@db:5432/app
LOG_LEVEL=${LEVEL:-info}
SESSION_SECRET=random(32)
SHARED_KEY=penv(production/SHARED_KEY)
LITERAL='${NOT_EXPANDED}'
```

| Form | What it does |
|---|---|
| `${KEY}` · `${KEY:-or}` · `${KEY-or}` | Reference; default when empty-or-unset; default when unset only. |
| `${KEY \| urlencode}` · `base64` · `lower` · `upper` · `trim` | **Filters**, applied left to right. |
| `ref` `concat` `fallback` `if` `eq` `not` `isEmpty` `forEnv` | `@env-spec` functions. |
| `match(subject, label: value, _: value)` · `and` · `or` · `startsWith` · `endsWith` | penv additions. `_` is the default branch. |
| `random(N)` | Generated once by `penv run` (N from 8 to 512), kept in `.env.local`, never sent by `penv push`. |
| `penv(KEY)` · `penv(env/KEY)` · `penv(project/env/KEY)` · `penv(org/project/env/KEY)` | Read a value from another environment, project or org. `penv()` alone reads the key it sits on. |
| `exec()` and plugin functions | **Refused.** A schema never starts a process. |

**Taint:** a value computed from a sensitive key is itself sensitive, through templates, filters, defaults, `concat`, `fallback`, the chosen `if`/`match` branch, reference chains and `penv()`. A computed value that lands on a bundler-public key fails `check` and `run`.

</details>

## Running safely

```sh
penv run -- node server.js
```

`penv run` validates every value, injects them into the **child process only**, and masks sensitive values in the child's output. Masking is on for everyone, in every run.

- **In-process masking.** A preload covers `console` and `Response` bodies in Node, Bun, Deno and Python, not just the terminal. The file `penv gen ts` writes does the same in deployed code (Node, Bun, Deno, edge, Workers) with no wrapper process.
- **Browser safety.** A public key built from a secret fails `check` and `run`. After a build, client output (`.next/static`, `dist`, `build`, `out`, React Native bundles) is scanned for secret values.
- **Git exposure.** `penv check` fails when a value file that holds a secret is tracked or not ignored.
- **Process hygiene.** Signals are forwarded to the child group, and variables that would change how the command runs (for example `NODE_OPTIONS`) are checked in cloud mode.

### Sealed runs

Under an AI agent, or with `penv run --sealed`, a key marked `@hosts` is not handed to the command at all.

```sh
# @type=string(startsWith=sk_live_, minLength=32) @hosts=api.stripe.com
STRIPE_SECRET_KEY=
```

```sh
penv run --sealed -- npm run dev      # always sealed under an AI agent
```

The command sees a **placeholder shaped like the real value** (it honors `startsWith`, `endsWith` and length constraints). A proxy inside `penv run` puts the real value into requests to the listed hosts only.

| Target | What happens |
|---|---|
| HTTPS to an allowed host | The value goes into the request line and headers (Basic auth included). Bodies keep the placeholder. Echoed values come back as the placeholder. |
| WebSocket through an allowed host | The handshake gets the value; server messages come back with it swapped for the placeholder. |
| `postgres://` URL | Connects through a local proxy; penv logs in to the real server (SCRAM-SHA-256, MD5 or cleartext). |
| `redis://` URL | Same idea, using `AUTH` and `HELLO … AUTH`. |
| `AWS_SECRET_ACCESS_KEY` | The SDK signs with a placeholder; the proxy re-signs each request (SigV4) with the real key, which never goes on the wire. |

A type with no room for a placeholder is `cannot_seal`.

## Coding agents

An agent runs as you, so it can read whatever you can read. penv narrows that, and is explicit about where it stops.

**How penv knows it's an agent session.** It reads harness environment variables first, then walks the process ancestry. `--agent` declares one. `--i-am-human` overrides a false detection, and only counts when stdin and stdout are both real terminals.

**What changes under an agent**

| Behavior | Detail |
|---|---|
| Output | JSON, as with `--agent`. |
| Masking and sealing | Always on. |
| `penv reveal KEY` | Asks a **person** to approve in the console. The agent can ask; a human clicks. |
| `penv pull`, `penv decrypt` | Refused. |
| Cloud credentials | 5-minute lifetime instead of 15. |
| Custom CA bundles | Refused if the user can write the file. |

**`penv guard`** reads your schema and writes what each harness actually enforces:

| Harness | What gets written |
|---|---|
| Claude Code | `.claude/settings.json`: deny rules, a sandbox block, a `PreToolUse` hook |
| Codex | `.codex/config.toml`: a permission profile |
| Cursor | `.cursor/cli.json` and `.cursor/hooks.json`: deny rules and fail-closed hooks |
| Copilot CLI · Gemini CLI · Cline · Windsurf · Amp | The equivalent for each |

```sh
penv guard             # detect installed harnesses and write their rules
penv guard cursor      # target one
penv guard --all       # every harness penv knows (handy for a template repo)
penv guard --check     # show status without writing; exits non-zero if anything is stale (CI-friendly)
```

The hook **is the penv binary** (`penv hook <harness>`), never a script that fails open. Guards also deny penv's own local key file, and re-running `penv guard` never weakens an existing rule.

**What penv does and does not claim.** It keeps secrets out of the files, the repo, the shell history and the captured output an agent reads, and it blocks reads of the value files where a harness allows it. It is **not a sandbox**: a process running as you can still look. That is why cloud-issued values are short-lived, scoped and attributable to the session that used them. `penv guard --check` prints the exact claim for your setup.

Known harness limits: Claude Code's sandbox is not available on native Windows (deny rules and the hook still apply), and GitHub has not published a schema for Copilot CLI's permissions file.

## Local-first values

penv works fully offline.

- **Layering.** Value files cascade in this order: `.env` → `.env.local` → `.env.<env>` → `.env.<env>.local`. (`test` skips `.env.local`.) `.env.vault`, `.env.keys` and `.env.me` are never read as values.
- **Choosing the environment.** `--env` > `PENV_ENV` > `@currentEnv` > `development`.
- **Process wins.** A key already set in the process overrides files, as in dotenv and varlock.
- **Encryption at rest (opt-in).** `penv encrypt` converts sensitive values in value files to `enc:v1:…` and turns on `[local] encrypt`; later `penv set`, `penv pull` and `random()` write ciphertext. The key lives in your OS keychain (override with `PENV_LOCAL_KEY`). `penv decrypt` reverses it, and is refused for agents. A value this machine can't decrypt stops the command and names the key, never the value.
- **Deploys without a cloud.** `penv bundle --env production` writes `.penv/production.bundle` and prints a `PENV_BUNDLE_KEY`. At deploy, `penv run`, `check`, `ls` and `why` read the bundle where they would otherwise read penv.cloud. The platform's own variables and value files still win.

### Find out why

```sh
penv why STRIPE_SECRET_KEY    # where the value comes from, what it's built on,
                              # and whether it's masked, public or sealed (never prints it)
penv check                    # schema, values, drift, guard coverage, git exposure
penv check --strict           # also fail on variables your code reads but the schema doesn't declare
penv scan                     # find secret values committed to other files
penv ls                       # names and types, values masked
```

`penv check` reads your code: it names each variable the code reads that `.env.schema` doesn't declare, and each declared key nothing mentions.

## Typed access

```sh
penv gen ts       # one typed `env` for server and client; secrets read by computed name so no bundler inlines them
penv gen py       # standard library, or pydantic when your project uses it
penv gen go | rust | php | java | csharp
```

- Secret fields print `[redacted]` in every language, and CI compiles and runs each generated target.
- penv asks where the file goes once, and remembers it in `.penv/config.toml` under `[targets.<name>]`. `--out <PATH>` answers it up front; non-interactive runs without a remembered answer write nothing and say which flag to pass.
- `penv gen <target> --options` prints a target's options and their current values. `penv gen <target> --check` verifies the file is up to date.
- `penv gen ts` never edits your `tsconfig.json`: it prints the import line to paste.
- A target is a template plus settings, so adding a language is adding a template.

## With a team

```sh
penv login      # device code in the browser; type the code your terminal shows
penv push       # values go to penv.cloud, .env is deleted
penv pull       # regenerate a .env view whenever you need one
```

A teammate clones the repo and runs `penv run -- pnpm dev`. That's the whole onboarding.

- **CI** presents its OIDC token and gets a short-lived credential. **Servers** with nothing to present enrol a keypair once (`penv machine enroll`); AWS roles work too.
- Cloud mode is chosen by the `@penv=org/project` header. It's a *provider*: `@penv=<provider>:org/project`, `--provider`, and `[providers.<slug>] url` in `.penv/config.toml`. A root named only in the settings file never receives a token, OIDC or AWS credential.
- Offline outside development, penv fails closed.
- `penv set KEY` reads the value without echoing it. Write-only values come back from `pull` as a `# penv:redacted KEY` comment, never as "missing".
- Manage structure with `penv project …` and `penv env …` (`ls`, `new`, `rename`, `copy`, `rm`).
- **Using varlock already?** `@penvhq/varlock-plugin` reads penv.cloud from varlock.

## Editor support

`penv lsp` serves `.env.schema` over the Language Server Protocol: diagnostics (what `penv check` reports for the schema), completion, hover on every decorator, function and filter (marked *penv*, *@env-spec* or *varlock*), go-to-definition for key references, and an outline. It only ever reads files named `.env.schema`, never value files, and makes no network requests.

- **VS Code:** install *penv* from the Marketplace or Open VSX (needs penv 1.0.0-beta.3 or newer).
- **Neovim 0.11:**
  ```lua
  vim.filetype.add({ filename = { [".env.schema"] = "envschema" } })
  vim.lsp.config("penv", { cmd = { "penv", "lsp" }, filetypes = { "envschema" }, root_markers = { ".env.schema" } })
  vim.lsp.enable("penv")
  ```
- **Helix 25.07:** run `penv lsp` as the language server for the `.env.schema` glob.

## Commands

Run `penv help --json` for the machine-readable manifest (completions are generated from it).

| Command | What it does |
|---|---|
| `penv` | Project state and the one next command |
| `init` | `.env` → `.env.schema`; picks harnesses to guard; generates typed files |
| `run -- cmd` | Validate, inject, mask. `--sealed` for sealed runs |
| `check [KEY]` | Schema, values, drift, guard coverage, git exposure, unused/undeclared keys |
| `why KEY` | Where a value comes from and how it's treated, never the value |
| `ls` | Names and types, values masked |
| `scan` | Find secret values committed to other files |
| `gen <target>` | Typed file for a language |
| `schema` | Print the parsed schema as JSON |
| `lsp` | Language server for `.env.schema` |
| `guard` | Write or check agent-harness rules from the schema |
| `hook <harness>` | The fail-closed hook harnesses call; you rarely run it by hand |
| `reveal KEY` | Print one value, after console approval |
| `set` / `unset` | Write or remove a value, never echoed |
| `encrypt` / `decrypt` | Turn local encryption at rest on or off |
| `bundle` | Encrypt one environment for a deploy without penv.cloud |
| `login` / `logout` | Device-code sign-in; credential in the OS keychain |
| `push` / `pull` | Values to and from penv.cloud |
| `machine enroll` | Bind a server keypair |
| `project …` / `env …` | Manage projects and environments |
| `upgrade [--check]` | Replace this binary from the latest release |
| `completions <shell>` | bash, zsh, fish, powershell, elvish |
| `help --json` | The command manifest |

**Output and exit codes.** Output is JSON when stdout isn't a terminal, and under agents. Exit codes: `0` ok · `1` error · `2` auth · `3` validation · `4` confirmation required · `5` no credential · `6` environment refused.

**Configuration** lives in `.penv/config.toml`, written by `penv init` with every setting at its default and a comment saying what each value does: `[schema] version`, `[run] preload`, `[local] encrypt`, `[public] prefixes`, `[targets.<name>]`, `[providers.<slug>]`.

<details>
<summary><b>Environment variables penv reads</b></summary>

`PENV_ENV` (environment) · `PENV_URL` (API root) · `PENV_TOKEN` (credential) · `PENV_BUNDLE_KEY` (read a bundle) · `PENV_LOCAL_KEY` (local encryption key) · `PENV_INSTALL_DIR`, `PENV_VERSION` (installer) · `SSL_CERT_FILE` (CA bundle). `penv run` strips penv's own credentials and keys from the child.

</details>

## How penv compares with varlock

penv reads varlock's schema format and exists alongside it. The honest differences:

| | **penv** | **varlock** |
|---|---|---|
| Schema format | `@env-spec`, plus penv extensions (`@hosts`, `@rotate`, `@assert`, filters, `match`, `random`, `penv()`) | `@env-spec` |
| Reads the other's schema | A varlock schema parses; unknown parts warn | penv-specific parts are outside its vocabulary |
| `exec()` and plugin functions | Refused by design | Supported, with a large plugin ecosystem |
| Secrets backends | penv.cloud, local encrypted files, bundles | Plugins for 1Password, AWS, Vault, Infisical and more |
| Agent protection | Harness guards, agent detection, sealed runs with proxies | Log redaction and leak prevention |
| Packaging | One static Rust binary | Node-based |

Maintainer benchmark (Linux x86_64, varlock 1.20.0 from npm): `run -- true` 5.7 ms vs 374 ms, validate 5.0 ms vs 329 ms, scan 3.8 ms vs 349 ms, peak memory 11 MB vs 95 MB. Method and results are in [`docs/BENCHMARKS.md`](docs/BENCHMARKS.md); run it yourself with `bench/run.sh`.

## Status

`1.0.0-rc.3`. Behavior has been frozen since `rc.1`; only fixes land before `1.0.0`. penv.cloud is the hosted service; the CLI is the open-source part and works without it.

- Docs: [penv.cloud/docs](https://penv.cloud/docs)
- Contributing: [`CONTRIBUTING.md`](CONTRIBUTING.md) · Security: [`SECURITY.md`](SECURITY.md) · [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md)
- The repo ships a penv **Agent Skill** so coding agents know how to use the CLI.

## License

[MIT](LICENSE)
