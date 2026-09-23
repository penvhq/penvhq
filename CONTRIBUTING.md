# Contributing

[docs/Design.md](./docs/Design.md) is the contract. Where code and the design disagree, the design wins: change the design in the same pull request, or change the code to match it.

## Build and test

```bash
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

CI runs `fmt`, `clippy` and `test` on Linux, macOS and Windows. `--all-targets` also lints the tests, which CI does not; run it anyway. Release builds run only in CI; `.cargo/config.toml` pins two build jobs.

## Where things go

| Change | Where |
|---|---|
| Schema syntax, validation, computed values | `crates/penv-schema` (pure, no I/O) |
| `.env` reading, writing, file cascade | `crates/penv-dotenv` (pure) |
| Output masking | `crates/penv-mask` (pure) |
| Agent detection | `crates/penv-agent` |
| Commands, files, processes, the preload | `crates/penv` |
| penv.cloud API, credentials, TLS | `crates/penv-cloud` |
| A language shipped with penv | a folder in `crates/penv-targets/targets/<name>/`: `target.toml` and `env.tmpl` |
| A language for one repository | `[targets.<name>]` in `.penv/config.toml` and `.penv/<name>.tmpl`; no Rust |
| A coding tool for `penv guard` | a folder in `crates/penv-guards/guards/<name>/` |
| A cloud credential kind | one file in `crates/penv-cloud/src/credential/` and one line in `mod.rs` |

Adding a target, guard or credential kind adds files and edits nothing beside them. If it needs a sibling changed, the boundary is wrong; fix the boundary first.

## Rules

- **Never print a value.** Outside `reveal` and `pull`, no code path writes a sensitive value to stdout, stderr, a log or a fixture. Error messages name keys, never values.
- **Fake values in tests.** Use `sk_test_FAKE…`-style strings. A test that could leak asserts the value is absent from every output.
- **Pure cores.** Parsing, validation, masking and resolution take values and return values. Filesystem, network and clock stay in `crates/penv` and `crates/penv-cloud`.
- **No new crates without the maintainer.** The allowed dependency set is in the design.
- **One name per concept.** No aliases for decorators, flags or commands.
- **Portable tests.** Integration tests run under `cmd` on Windows. Shell snippets branch on `cfg!(windows)` (`echo %KEY%` / `echo $KEY`); use `exit 0`, not `true`.
- **Comments** give the reason in one line when the code does not show it.

## Pull requests

1. The four commands above pass.
2. Behaviour changes update `docs/Design.md`; user-facing changes update `README.md`.
3. `penv help --json` changes only when a command, flag or exit code changes.
4. The description says what changed, what was tested, and what was not.

## Security reports

Not in issues. See [SECURITY.md](./SECURITY.md).

## Conduct

[CODE_OF_CONDUCT.md](./CODE_OF_CONDUCT.md) applies to issues, pull requests, discussions and reviews.
