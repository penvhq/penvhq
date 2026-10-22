<p align="center">
  <img src=".github/banner.svg" alt="penv" width="640">
</p>

<h3 align="center">Typed, validated, agent-safe environment variables.<br>One static binary. No account needed.</h3>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#quickstart">Quickstart</a> ·
  <a href="#decorators">Decorators</a> ·
  <a href="#functions">Functions</a> ·
  <a href="#filters">Filters</a> ·
  <a href="#benchmark">Benchmark</a> ·
  <a href="https://penv.cloud/docs">Docs</a>
</p>

---

penv validates your `.env` files against a committed `.env.schema`, starts your command with the resolved values, and masks secrets in its output. It runs offline; [penv.cloud](https://penv.cloud/?ref=gh) is optional.

## Install

```bash
curl -fsSL 'https://penv.cloud/install?ref=gh' | sh     # macOS, Linux
irm 'https://penv.cloud/install.ps1?ref=gh' | iex       # Windows (PowerShell)
npm i -g @penvhq/cli                                    # npm
```

Docker: `COPY --from=ghcr.io/penvhq/penv:1 /penv /usr/local/bin/penv` (signed, `FROM scratch`, amd64 and arm64; see [`packaging/docker`](packaging/docker/README.md)).

The installers put one binary in `~/.penv/bin` (`%USERPROFILE%\.penv\bin` on Windows), check every download against the release digest, and edit no rc file. `PENV_INSTALL_DIR` moves it; `PENV_VERSION` pins a tag. `penv upgrade` updates an installer-placed binary.

## Quickstart

```sh
penv init                 # reads .env, writes .env.schema, gitignores .env and .env.*
penv run -- pnpm dev      # validates, injects, masks
```

Bare `penv` shows the project state and the one next command to run.

## The schema

```sh
# @currentEnv=$APP_ENV

# @type=enum(development,staging,production)
APP_ENV=development

# @type=string @sensitive
DB_PASSWORD=

# @type=url
DATABASE_URL=postgres://app:${DB_PASSWORD | urlencode}@db:5432/app

# @type=string(startsWith=sk_) @rotate=90d @hosts(api.stripe.com)
STRIPE_SECRET_KEY=
```

The format is [`@env-spec`](https://varlock.dev/env-spec/overview/): its decorators (`@currentEnv`, `@defaultRequired`, `@defaultSensitive`, `@import`, `@required`, `@optional`, `@sensitive`, `@type`, `@example`, `@docs`, `@deprecated`), types and functions (`ref`, `concat`, `fallback`, `if`, `eq`, `not`, `isEmpty`, `forEnv`) are documented there. A varlock schema parses; what penv does not know is ignored with a warning. penv parses the `matches` and `precision` type options and passes them to generated types and penv.cloud, but does not enforce them: a failing `matches` does not fail `penv check`. Below is only what penv adds.

### Decorators

| Decorator | Purpose | Example |
|---|---|---|
| `@penv` | Header. Links the schema to a cloud project; absent means local mode. `<provider>:org/project` uses [another provider](docs/PROVIDERS.md). | `# @penv=acme/web` |
| `@assert` | Header or key. `penv check` and `penv run` fail with the message when the expression is false. | `# @assert(not(eq($PORT, $ADMIN_PORT)), "ports must differ")` |
| `@hosts` | Key. Seals the value: under an agent or `penv run --sealed` the command gets a placeholder, and the real value goes only to these hosts (at most 32). A computed `@hosts=url($DATABASE_URL, host)` must be pinned with `penv check --pin-hosts` ([Config](docs/Functions.md#config)). | `# @hosts(api.stripe.com, "*.stripe.com")` |
| `@rotate` | Key. `penv check` reminds you once the value is older than this. Units `y m w d h min s`, largest first. | `# @rotate=90d` |

### Functions

| Function | Purpose | Example |
|---|---|---|
| `match` | The case whose label equals the subject; `_` is the default. | `match($APP_ENV, production: warn, _: debug)` |
| `and` · `or` | `true` when every / any argument is true. | `and(eq($APP_ENV, production), startsWith($STRIPE_KEY, sk_live_))` |
| `startsWith` · `endsWith` | `true` when the value starts / ends with the text. | `startsWith($STRIPE_KEY, sk_live_)` |
| `random` | Random characters, made once by `penv run`, kept in `.env.local`, never pushed. Kinds `alnum`, `hex`, `base64url`, `uuid`; `length` 8 to 512. | `random(kind=hex, length=32)` |
| `url` | Read one part of a URL, or write parts and return the URL. | `url($DATABASE_URL, host)` · `url($DATABASE_URL, host=replica.internal.test)` |
| `json` | Read a JSON field, or write fields and return compact JSON. | `json($X, db.hosts[0])` · `json(username=$U, password=$P)` |
| `raw` | Embed existing JSON as a `json()` option value. | `json(db=raw($INNER))` |
| `hash` | Digest or HMAC: `sha256`, `sha512`, `hmac-sha256`, `hmac-sha512` (`key=`); `encoding=`, `length=`. | `hash($TOKEN, algo=sha256)` |
| `penv` | A value from another environment, project or org; `penv()` reads this key. | `penv(production/SHARED_KEY)` |

`exec()` and plugin functions are refused: a schema never starts a process. A value computed from a sensitive key stays sensitive unless a `public` list declassifies it; `url()` scheme, host, port and origin are public by default ([Config](docs/Functions.md#config)). Full reference: [`docs/Functions.md`](docs/Functions.md).

### Filters

Applied left to right inside `${…}`: `${KEY | trim | urlencode}`.

| Filter | Purpose |
|---|---|
| `urlencode` | Percent-encode for a URL. |
| `base64` · `b64d` | Encode / decode standard base64. |
| `lower` · `upper` | Change case. |
| `trim` | Strip surrounding whitespace. |

## Benchmark

Maintainer benchmark of penv 1.0.0 against varlock 1.23.0 from npm (Linux x86_64, median of 30 runs): `run -- true` 10.6 ms vs 166.8 ms, validate 12.1 ms vs 168.7 ms, scan 6.9 ms vs 180.9 ms, peak memory 11 MB vs 74 MB.

Method and results: [`docs/BENCHMARKS.md`](docs/BENCHMARKS.md); run it yourself with `bench/run.sh`.

## More

Commands, sealed runs, coding-agent guards, typed access, editor support, teams and penv.cloud: [penv.cloud/docs](https://penv.cloud/docs). `penv help --json` prints the command manifest.

[Contributing](CONTRIBUTING.md) · [Security](SECURITY.md) · [Code of conduct](CODE_OF_CONDUCT.md) · [MIT](LICENSE)
