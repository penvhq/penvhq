# penv for VS Code

Diagnostics, completion, hover, go-to-definition and an outline for `.env.schema`, served by the `penv` binary (`penv lsp`). On the VS Code Marketplace and [Open VSX](https://open-vsx.org/extension/penvhq/penv); tested on VS Code 1.90 and current stable.

## Requires

A `penv` that has `penv lsp`, 1.0.0-beta.3 or later, on `PATH`: [install](https://github.com/penvhq/penvhq#install). The extension says so when yours is older.

Installed off `PATH`? Set `penv.path` in your user settings. The setting is machine-scoped; a workspace cannot set it.

## What it reads

| Feature | Covers |
|---|---|
| Diagnostics | What [`penv check`](https://github.com/penvhq/penvhq#commands) reports for the schema: parse errors, ignored decorators, `@import` targets, `.penv/config.toml` public prefixes and schema version, overdue `@rotate` keys in local mode |
| Completion | Decorators, `@type` values and constraints, `@rotate` periods, `$KEY`, `${KEY \| filter}`, `penv(env/KEY)`, functions |
| Hover | Each decorator and function, marked penv extension, `@env-spec` or varlock-only; each key's type, required, sensitive, `@hosts` and rotation |
| Definition | `$KEY`, `${KEY}`, `ref(KEY)`, `@currentEnv=$KEY` |

Only files named `.env.schema` are served. `.env` and `.env.*` are never read. The server sends no network request.

## With varlock's extension

This extension adds no grammar. Install [varlock's @env-spec extension](https://marketplace.visualstudio.com/items?itemName=varlock.env-spec-language) for syntax highlighting; the two run side by side.

## Commands

| Command | Does |
|---|---|
| `penv: Restart language server` | Stops and starts `penv lsp` |

Server output is in the **penv** output channel.

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
