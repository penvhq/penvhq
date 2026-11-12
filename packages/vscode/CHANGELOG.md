# Changelog

## 0.2.0

- Messages only. Completions, hover and checks come from the installed `penv`, so the new ones (environment names in `forEnv(` and `penv(`, the options of `string(`, `number(` and `port(`, hover for `${KEY:-default}`, `${KEY-default}` and `|`) arrive with the next penv release, not with this extension.
- Clearer messages when `penv` is missing or the server stops; each names the fix: `penv.path`, `penv upgrade` or `npm i -g @penvhq/cli`.

## 0.1.0

- Diagnostics, completion, hover, go-to-definition and outline for `.env.schema` through [`penv lsp`](https://penv.cloud/docs/cli/lsp).
- Finds `penv` on `PATH`, including the platform binary behind `npm i -g @penvhq/cli`; `penv.path` overrides, user settings only.
