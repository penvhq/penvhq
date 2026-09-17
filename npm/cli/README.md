# @penvhq/cli

penv is the open source CLI for Penv Cloud, a [secrets manager](https://penv.cloud) for `.env` files and API keys. penv reads the `.env` you already have, writes a small committed schema next to it, validates every value before your process starts, generates types for your language, and configures your coding agent's harness so it cannot read the file.

```bash
npm i -g @penvhq/cli

penv init                # reads .env, writes .env.schema, gitignores .env
penv run -- npm run dev  # validates, injects, masks
```

This package is a launcher. The binary itself arrives in one of six platform packages (`@penvhq/cli-linux-x64`, `-linux-arm64`, `-darwin-x64`, `-darwin-arm64`, `-win32-x64`, `-win32-arm64`), and npm installs the one your machine matches.

`npm i -g @penvhq/cli` is how this install upgrades. `penv upgrade` refuses here, because the binary belongs to the package npm put it in.

Without Node, install the binary on its own:

```bash
curl -fsSL https://penv.cloud/install | sh        # macOS, Linux
irm https://penv.cloud/install.ps1 | iex          # Windows
```

Docs at [penv.cloud](https://penv.cloud). MIT.
