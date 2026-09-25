# Contributing

[docs/Design.md](./docs/Design.md) is the contract. Where code and the design disagree, the design wins: change the design in the same pull request, or change the code to match it.

## Build and test

```bash
cargo check --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
```

Release builds run only in CI. `.cargo/config.toml` pins two build jobs; do not raise it.

## CI

| Workflow | Trigger | Runs |
|---|---|---|
| `ci.yml` | every pull request | the commands above on Linux, macOS, Windows; installer and launcher tests |
| `targets.yml` | `crates/penv-targets/**` | `tests/compile.sh`: builds and runs every language snapshot |
| `sealed.yml` | `crates/penv/**`, `crates/penv-schema/**` | sealed runs against real clients |
| `varlock-plugin.yml` | `packages/varlock-plugin/**` | typecheck and `npm test` |
| `vscode.yml` | `packages/vscode/**`, `crates/penv-schema/**`, the `lsp` command | typecheck and `npm test` |

`compile.sh` skips a toolchain you lack; CI fails instead.

## Where things go

| Change | Where |
|---|---|
| Schema syntax, validation, computed values | `crates/penv-schema` (pure) |
| `.env` reading, writing, file cascade | `crates/penv-dotenv` (pure) |
| Output masking | `crates/penv-mask` (pure) |
| Agent detection | `crates/penv-agent` |
| Commands, files, processes, preload | `crates/penv` |
| penv.cloud API, credentials, TLS | `crates/penv-cloud` |
| A language shipped with penv | `crates/penv-targets/targets/<name>/` (`target.toml`, `env.tmpl`), its line in `BUILT_IN` (`src/load.rs`), a snapshot in `tests/snapshots/`, a block in `tests/compile.sh` |
| A coding tool for [`penv guard`](https://penv.cloud/docs/cli/guard) | `crates/penv-guards/guards/<name>/` and its line in `BUILT_IN` (`src/load.rs`) |
| A cloud credential kind | a file in `crates/penv-cloud/src/credential/`, plus its `mod` line and its place in `resolve` (`mod.rs`) |
| Sealed runs: the HTTPS, Postgres and Redis proxies | `crates/penv/src/sealed/` |
| Encryption at rest | `crates/penv/src/localcrypt.rs` |
| The varlock plugin ([varlock.dev](https://varlock.dev)) | `packages/varlock-plugin/` (`@penvhq/varlock-plugin`) |
| The editor extension | `packages/vscode/` |

## Rules

[AGENTS.md](./AGENTS.md#culture), plus:

- **Fake values.** `sk_test_FAKE…`-style strings. A test that could leak asserts the value is absent from all output.
- **Portable.** Integration tests run under `cmd` on Windows. Shell snippets branch on `cfg!(windows)` (`echo %KEY%` / `echo $KEY`); use `exit 0`, not `true`.

## Pull requests

1. The four commands pass; `cargo test` also checks [`penv help`](https://penv.cloud/docs/cli/help) `--json` against `docs/manifest.schema.json`.
2. Behaviour changes update `docs/Design.md`; user-facing changes update `README.md`.
3. `penv help --json` changes only when a command, flag or exit code changes.
4. The description says what changed, what was tested, and what was not.

## Security reports

[SECURITY.md](./SECURITY.md), never an issue.

## Conduct

[CODE_OF_CONDUCT.md](./CODE_OF_CONDUCT.md) applies to issues, pull requests, discussions and reviews.
