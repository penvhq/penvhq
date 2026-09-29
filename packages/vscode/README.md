# penv for VS Code

Diagnostics, completion, hover, go-to-definition and an outline for `.env.schema`, served by [`penv lsp`](https://penv.cloud/docs/cli/lsp). Tested on VS Code 1.90 and current stable.

## Requires

`penv` 1.0.0-beta.3 or later ([install](https://penv.cloud/docs/start/install-the-cli)). The extension warns when your `penv` has no `lsp` command.

| `penv.path` | Binary started |
|---|---|
| empty (default) | first `penv` on `PATH`; relative entries are skipped, and an `npm i -g @penvhq/cli` launcher resolves to the platform binary behind it |
| a path | that file |

`penv.path` is machine-scoped: only your user settings can set it, so an untrusted workspace cannot choose the program.

## Features

| Feature | Covers |
|---|---|
| Diagnostics | What [`penv check`](https://penv.cloud/docs/cli/check) reports for the schema: parse errors, ignored decorators, `@import` targets, `.penv/config.toml` public prefixes and schema version, overdue `@rotate` keys in local mode |
| Completion | Decorators, `@type` values and constraints, `@rotate` periods, `$KEY`, `${KEY \| filter}`, `penv(env/KEY)`, functions |
| Hover | Each decorator and function, marked penv extension, `@env-spec` or varlock; each key's type, required, sensitive, `@hosts` and rotation |
| Definition | `$KEY`, `${KEY}`, `ref(KEY)`, `@currentEnv=$KEY` |

`penv lsp` serves only files named `.env.schema`, never reads `.env` or `.env.*`, and sends no network request.

## Syntax highlighting

This extension adds no grammar. Install [varlock's @env-spec extension](https://marketplace.visualstudio.com/items?itemName=varlock.env-spec-language) ([varlock.dev](https://varlock.dev)) for highlighting; the two run side by side.

## Commands

| Command | Does |
|---|---|
| `penv: Restart language server` | Stops and starts `penv lsp` |

Changing `penv.path` restarts the server too. Server output is in the **penv** output channel.

## Other editors

Any LSP client can run `penv lsp` on stdio for files named `.env.schema`. Neovim 0.11:

```lua
vim.filetype.add({ filename = { [".env.schema"] = "envschema" } })
vim.lsp.config("penv", { cmd = { "penv", "lsp" }, filetypes = { "envschema" }, root_markers = { ".env.schema" } })
vim.lsp.enable("penv")
```

Helix 25.07, `languages.toml`:

```toml
[language-server.penv]
command = "penv"
args = ["lsp"]

[[language]]
name = "env-schema"
scope = "source.env-schema"
file-types = [{ glob = ".env.schema" }]
language-servers = ["penv"]
```

## License

MIT
