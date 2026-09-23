# @penvhq/varlock-plugin

A [varlock](https://varlock.dev) plugin that loads secrets from [penv.cloud](https://penv.cloud). It reads the same `@penv=org/project` header and `penv(...)` addresses as the [penv CLI](https://github.com/penvhq/penvhq), so one `.env.schema` runs under both tools.

## Install

```bash
npm install -D @penvhq/varlock-plugin
```

Or load it by name and version from the schema; varlock fetches it (see varlock's [plugins guide](https://varlock.dev/guides/plugins/)):

```env-spec
# @plugin(@penvhq/varlock-plugin@0.1.0)
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

`PENV_TOKEN` is a penv.cloud machine token (`pck_…`) scoped to the environments the app reads. `penvToken` is sensitive and internal: varlock uses it and does not pass it to your app.

| `@initPenv()` option | Default |
|---|---|
| `token` | none; required before any value is read |
| `environment` | `development`. Pointing it at the `@currentEnv` item keeps them in step. Taken whole, so `feature/foo` is one environment |
| `url` | `https://penv.cloud`. Any other root is used only when `PENV_URL` names the same root, so a changed schema cannot send the token elsewhere |
| `org`, `project` | from `@penv=` |
| `cacheTtl` | no cache. `"5m"`, `"1h"`, `"1d"` or `"forever"` keeps each environment's values in varlock's local cache for that long |

## Usage

```env-spec
DATABASE_URL=penv()                                   # the item key, default environment
STRIPE_SECRET_KEY=penv(STRIPE_KEY)                    # another key
PROD_DATABASE_URL=penv(production/DATABASE_URL)       # another environment
BILLING_TOKEN=penv(billing/production/API_TOKEN)      # another project
SENTRY_DSN=penv(acme-shared/observability/production/SENTRY_DSN)   # another org
```

An environment whose name holds a `/` cannot be written in a `penv(...)` address; name it in `@initPenv(environment=...)` or `penvBulk(...)`.

Bulk-load an environment:

```env-spec
# @setValuesBulk(penvBulk())
# @setValuesBulk(penvBulk(production))
# @setValuesBulk(penvBulk("feature/foo"))
```

## The same schema under the penv CLI

```bash
varlock run -- npm run dev    # through this plugin
penv run -- npm run dev       # native; penv ignores @plugin and @initPenv
```

## Errors

| Error | Fix |
|---|---|
| `penv.cloud rejected the token` | create a machine token and set `PENV_TOKEN` |
| `the token may not read org/project/env` | give the machine identity that project and environment |
| `org/project/env does not exist on penv.cloud` | check the names; `penv project ls` |
| `KEY is not in org/project/env` | `penv set KEY --env env` |
| `KEY has no stored value` | `penv set KEY --env env` |
| `penv url … is not penv.cloud, so the token is not sent there` | set `PENV_URL` to that root, or drop `url=` |

Requests are https only (`http://localhost` for tests), follow no redirects, time out after 15 seconds, and each environment is read once per load. No error prints a token or a value.

## Develop

```bash
npm install
npm test          # builds, then runs the published varlock CLI against a fake penv.cloud
npm run typecheck
```
