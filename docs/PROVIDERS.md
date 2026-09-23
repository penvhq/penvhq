# Providers

A provider is where penv reads an environment's values from when the schema names one. penv.cloud is provider `penv`, and the default. Every other provider follows this document and passes the same tests.

## Using a provider

```dotenv
# .env.schema
# @penv=acme/api              # penv.cloud
# @penv=penv:acme/api         # the same, named
# @penv=<slug>:acme/api       # another provider
```

```toml
# .penv/config.toml: optional, per provider
[providers.penv]
url = "https://penv.acme.internal"     # a self-hosted or proxied API root
```

```bash
penv run -- npm run dev                      # the provider @penv= names
penv run --provider penv -- npm run dev      # another one, for this command
```

| Setting | Precedence, first wins |
|---|---|
| Provider | `--provider`, the `@penv=` prefix, `penv` |
| API root | `PENV_URL` (provider `penv` only), `[providers.<slug>] url`, the provider's default |

A slug is a lowercase word: `a-z`, `0-9`, `-`, starting with a letter, at most 32 characters. A slug penv does not have is refused by name before any request is sent. Local files, masking, taint, `check`, the build scan and the agent rules work the same whatever the provider.

## What a provider is

A provider is compiled into penv. penv never loads a provider at runtime and never runs a program a schema or config names, so a repository cannot make penv run code. A new provider arrives as a pull request.

| Part | Where |
|---|---|
| Registry entry: slug, name, default URL, capabilities | `crates/penv-cloud/src/provider.rs`, one element of `PROVIDERS` |
| Client: auth and read, and write if declared | `crates/penv-cloud/src/provider/<slug>.rs` |
| Dispatch | one match arm in `Fetcher::keys` (`crates/penv/src/commands/cloud.rs`) |
| Conformance | `crates/penv/tests/providers/<slug>.rs`, the shared suite run against a mock of the provider's API |
| Docs | a row in the table below, and the address mapping |

Adding a provider touches no other provider.

## Capabilities

| Capability | penv commands | Required |
|---|---|---|
| `read` | `run`, `check`, `ls`, `penv(...)` values, `gen` against a linked schema | yes |
| `write` | `set`, `unset`, `push` | no |
| `manage` | `project`, `env`, `pull` of a project list | no |
| `approve` | `reveal` in an agent session: a person approves in the provider | no |
| `audit` | `@rotate` reminders from the provider's write times; who read what | no |

A command that needs a capability the provider does not declare is refused with the provider and the capability named. An agent's `reveal` is refused outright when the provider has no `approve`.

## Address

penv addresses a value as `org/project/environment/KEY`. Each provider documents how that maps to its own terms, in the table below, and nothing else in penv changes:

| Provider | `org` | `project` | `environment` |
|---|---|---|---|
| `penv` | organization | project | environment |

## The read contract

`read(address) -> keys` returns every key of one environment:

| Field | Meaning |
|---|---|
| `name` | the key, as the schema names it |
| `value` | the value; absent when the key has none stored |
| `version` | the provider's version of this value, when it has one |
| `updatedAt` | when the value was last written, for `@rotate`, when `audit` is declared |

Rules every client follows:

1. **HTTPS only**, except `http://127.0.0.1` and `http://localhost` for tests and local proxies.
2. **No redirects** to another host. A redirect is refused, not followed.
3. **Timeouts** on every request; a read that times out fails the command. It never falls back to an empty environment.
4. **Values stay out of errors, logs and panics.** An error names the provider, the address and the HTTP status or the provider's error code.
5. **Credentials are never written to disk in plain text.** A login goes to the OS keychain, keyed by API root, so an overridden `url` never receives a login stored for another root. A token in the environment goes to whichever root is chosen, so an override in a committed `config.toml` is reviewed like any code change.
6. **Trust** follows penv: the compiled-in roots, or `SSL_CERT_FILE` under its agent rule.
7. **One read per address per command**; penv caches across commands only where the provider's terms allow.

Authentication uses penv's credential kinds where the provider supports them, tried in this order: a token variable (`<SLUG>_TOKEN`, or `PENV_TOKEN` for `penv`), a person's login, an enrolled keypair, CI OIDC, then the AWS role.

## Conformance

The shared suite runs against a mock of the provider's API and must pass on Linux, macOS and Windows:

| Test | Passes when |
|---|---|
| Read | values reach the child process, masked in its output |
| Missing value | a key with no stored value is reported by name; the others still load |
| Unknown address | a clear error naming the address; exit 1 |
| Auth refused | exit 2, and the message names the fix |
| No credential | exit 5 |
| Redirect | refused; no second request is sent |
| Plain HTTP to a remote host | refused before connecting |
| Values in errors | no value appears in any stdout or stderr the suite captures |
| URL override | `[providers.<slug>] url` sends the read there |
| Capability not declared | the command is refused, naming it |

The `penv` provider's tests are the reference: `crates/penv/tests/cloud.rs`.

## Providers

| Slug | Name | Capabilities |
|---|---|---|
| `penv` | [penv.cloud](https://penv.cloud) | read, write, manage, approve, audit |
