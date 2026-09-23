# AGENTS.md

Instructions for AI coding agents working in this repository. The contract is [docs/Design.md](./docs/Design.md). Read it first. Where shipped code disagrees with it, the design wins; delete the code rather than adding a compatibility path.

## Build and test

```bash
cargo check --workspace          # the only build that runs on the development machine
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
```

Never run `cargo build --release` locally. Release builds run in CI. `.cargo/config.toml` pins two jobs; do not raise it.

## Culture

- **Drop-in or evict.** A language target, a harness guard, a credential kind, an output format or a command is added by adding a folder or a file and touching nothing that exists. If adding one requires editing a sibling, the seam is wrong; fix the seam.
- **Data over code.** Targets and guards are folders of TOML plus templates. Command metadata is data the manifest publishes. If a thing can be a file the binary reads, it is not Rust.
- **Pure cores.** Parsers, validation, detection, masking and merging are pure functions over values. I/O lives at the edge in the binary crate. Tests need no filesystem, network or clock.
- **One concept, one name.** No aliases for decorators, flags or commands.
- **Say only what is true.** Error messages name the key and the fix. The security claim text lives in one place.
- **Comments:** one short line where the reason is not obvious. No block preambles, no restating the code. Replace a stale comment; never stack a new one on an old one.
- **Never print a value.** No code path outside `reveal` and `pull` writes a sensitive value to stdout, stderr, a log or a test fixture. Tests use obviously fake values. `penv-release keygen` is the one exception and is not the binary: it is the dev tool that mints the release key, and printing the pair is what it is for.

## Before proposing a change as done

All four commands above pass, the fixture schema still renders through every target, and `penv help --json` still validates against `docs/manifest.schema.json` once it exists.
