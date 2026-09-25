# penv.cloud plugin for [varlock](https://varlock.dev)

Loads [penv.cloud](https://penv.cloud) values into [varlock](https://varlock.dev/guides/plugins/). It reads the `@penv=org/project` header and `penv(...)` addresses the [penv CLI](https://github.com/penvhq/penvhq) reads, so one `.env.schema` serves both.

## Install

```bash
npm install -D @penvhq/varlock-plugin
```

Or let [varlock fetch it](https://varlock.dev/guides/plugins/):

```env-spec
# @plugin(@penvhq/varlock-plugin@0.1.1)
```

## Setup

```env-spec
# @plugin(@penvhq/varlock-plugin)
# @penv=acme/api
# @initPenv(environment=$APP_ENV, token=$PENV_TOKEN)
# @currentEnv=$APP_ENV
# ---

# @type=enum(development, staging, production)
APP_ENV=development

# @type=penvToken
PENV_TOKEN=
```

`PENV_TOKEN` is a penv.cloud machine token (`pck_...`). The plugin declares the `penvToken` type sensitive and internal.

| `@initPenv()` option | Default |
|---|---|
| `token` | none; required |
| `environment` | `development`; `feature/foo` is one environment |
| `url` | `https://penv.cloud`; another root only when `PENV_URL` names it too |
| `org`, `project` | from `@penv=` |
| `cacheTtl` | no cache; a TTL such as `"1h"` caches each environment |

## Usage

```env-spec
DATABASE_URL=penv()                                   # the item key, default environment
STRIPE_SECRET_KEY=penv(STRIPE_KEY)                    # another key
PROD_DATABASE_URL=penv(production/DATABASE_URL)       # another environment
BILLING_TOKEN=penv(billing/production/API_TOKEN)      # another project
SENTRY_DSN=penv(acme-shared/observability/production/SENTRY_DSN)   # another org

# every value of one environment: the current one, a named one, one with a /
# @setValuesBulk(penvBulk())
# @setValuesBulk(penvBulk(production))
# @setValuesBulk(penvBulk("feature/foo"))
```

An environment named with a `/` fits only `@initPenv(environment=...)` or `penvBulk(...)`.

## Run under both tools

```bash
varlock run -- npm run dev    # through this plugin
penv run -- npm run dev       # native
```

[penv](https://penv.cloud/docs/cli) ignores `@plugin`, `@initPenv` and `@setValuesBulk`; [`penv check`](https://penv.cloud/docs/cli/check) lists each under `schemaWarnings`.

## Errors

| Error | Fix |
|---|---|
| `penv token is required` | pass `token=$PENV_TOKEN`; set `PENV_TOKEN` |
| `penv.cloud rejected the token for org/project/env` | set `PENV_TOKEN` to a valid machine token |
| `the token may not read org/project/env` | grant the machine identity that environment |
| `org/project/env does not exist on penv.cloud` | check the three names |
| `KEY is not in org/project/env`, `KEY has no stored value in org/project/env` | [`penv set KEY --env env`](https://penv.cloud/docs/cli/set) |
| `penv(...) needs a project` | add `# @penv=org/project` |
| `penv url URL is not penv.cloud, so the token is not sent there` | set `PENV_URL` to it, or drop `url=` |

Requests use https (http only on loopback), follow no redirects, time out after 15 seconds and read each environment once per load. No error prints a token or value.

## Develop

```bash
npm install
npm test          # build, then varlock 1.20.0 against a fake penv.cloud
npm run typecheck
```
