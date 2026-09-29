# Security Policy

## Reporting

Report privately through GitHub: [open a draft security advisory](https://github.com/penvhq/penvhq/security/advisories/new). Do not open a public issue, pull request or discussion for a vulnerability.

Include the penv version (`penv --version`), the operating system, the steps that reproduce it, and what an attacker gains. Use fake secret values in the report.

## Supported versions

Only the latest release receives fixes. Upgrade: [upgrade the CLI](https://penv.cloud/docs/cli/upgrade-the-cli), or [`penv upgrade`](https://penv.cloud/docs/cli/upgrade).

## Scope

In scope:

| Area | Examples |
|---|---|
| Value disclosure | a sensitive value printed by any command other than `reveal` and `pull`; a masking bypass in `run` output or in the preload; a secret missed by the client bundle check |
| Agent controls | a way for an agent session to disable masking, the preload or approval, read `.env` past a [`penv guard`](https://penv.cloud/docs/cli/guard) rule, or use a CA bundle it wrote |
| Supply chain | an installer, `penv upgrade` or the image accepting a binary whose checksum or signature does not verify |
| Credentials | a penv.cloud credential written in plain text, sent to a host other than the API root it belongs to, or kept past its lifetime |
| Schema and files | a schema, value file or `@import` that makes penv read, write or run something outside the project |

Out of scope, as documented in [Design, inside the process](./docs/Design.md#inside-the-process) and the claim `penv guard --check` prints:

- a process running as your user reading its own environment or memory
- code written to disguise a value before printing it (reversed, encrypted, split across writes)
- runtimes the preload does not load into (Ruby, Java, .NET, PHP, compiled languages), and native event loops such as uvloop
- values an app sends to an outbound request

## Release signing

Each release signs its checksum files with an Ed25519 key. The public key is in `install.sh`, `install.ps1` and `crates/penv/src/upgrade.rs`. Rotation and verification: [Design, distribution](./docs/Design.md#10-distribution).
