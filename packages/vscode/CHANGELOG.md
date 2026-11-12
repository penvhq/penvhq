# Changelog

## 0.2.0

- Completions, hover and checks now cover everything in penv 1.1.x: new decorators, functions, filters and value lists come from the `penv` binary, so update penv to get them.
- Clearer messages when `penv` is missing or the server stops; each names `penv.path` or `npm i -g @penvhq/cli` as the fix.

## 0.1.0

- Diagnostics, completion, hover, go-to-definition and outline for `.env.schema` through [`penv lsp`](https://penv.cloud/docs/cli/lsp).
- Finds `penv` on `PATH`, including the platform binary behind `npm i -g @penvhq/cli`; `penv.path` overrides, user settings only.
