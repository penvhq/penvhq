# @penvhq/cli

The npm launcher for penv, the open source CLI for [penv.cloud](https://penv.cloud).

```bash
npm i -g @penvhq/cli@next

penv init                # reads .env, writes .env.schema, keeps .env out of the repository
penv run -- npm run dev  # validates, injects, masks
```

| Tag | Holds |
|---|---|
| `next` | prereleases (1.0 is in prerelease) |
| `latest` | plain releases; a prerelease never moves it |

This package is a launcher with no postinstall step. npm installs the one platform package that matches your machine: `@penvhq/cli-linux-x64`, `-linux-arm64`, `-darwin-x64`, `-darwin-arm64`, `-win32-x64` or `-win32-arm64`.

Upgrade with npm, not [`penv upgrade`](https://penv.cloud/docs/cli/upgrade), which refuses an npm install. Commands: [`penv init`](https://penv.cloud/docs/cli/init), [`penv run`](https://penv.cloud/docs/cli/run).

Install without Node:

```bash
curl -fsSL https://penv.cloud/install | sh        # macOS, Linux
irm https://penv.cloud/install.ps1 | iex          # Windows
```

[Install the CLI](https://penv.cloud/docs/start/install-the-cli). MIT.
