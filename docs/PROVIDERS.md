# Providers

A provider is where penv reads an environment's values. We, [penv.cloud](https://penv.cloud), are provider `penv`, the default. This page is the contract a new provider meets.

## Choose a provider

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
| Provider | [`--provider`](https://penv.cloud/docs/cli/global-options), the `@penv=` prefix, `penv` |
| API root | `PENV_URL` (provider `penv` only), `[providers.<slug>] url`, the provider's default |
| Slug | `a-z`, `0-9`, `-`; starts with a letter; at most 32 characters |

An unknown slug fails with `unknown_provider` before any request. Local files, masking, [`penv check`](https://penv.cloud/docs/cli/check) and the agent rules work the same for every provider.

### Roots named only in config.toml

Such a root gets only the login or keypair this machine holds for it. A pull request can change a committed file, so penv withholds `PENV_TOKEN`, CI OIDC and AWS proofs and exits 5 with `credential_withheld`. To send them, set `PENV_URL` to that root.

## Parts of a provider

Providers are compiled in. penv loads none at runtime and runs no program a schema or config names. A new provider arrives as a pull request.

| Part | Where |
|---|---|
| Registry entry: slug, name, default URL, capabilities | one element of `PROVIDERS` in `crates/penv-cloud/src/provider.rs` |
| Client: authenticate, read, and write if declared | today `crates/penv-cloud/src/api.rs` for `penv` |
| Read path | `Fetcher::keys` in `crates/penv/src/commands/cloud.rs` |
| Tests | today `crates/penv/tests/cloud.rs` and `crates/penv-cloud/tests/cloud.rs` for `penv` |
| Docs | a row in [Address](#address) and [Registered providers](#registered-providers) |

## Capabilities

`Capability` in `provider.rs`:

| Capability | What it covers | Required |
|---|---|---|
| `read` | the keys and values of one environment | yes |
| `write` | write and delete one value | no |
| `manage` | list, create and rename projects and environments | no |
| `approve` | show a value only after a person approves ([`penv reveal`](https://penv.cloud/docs/cli/reveal) under an agent) | no |
| `audit` | who read what, and when a value was last written (`@rotate`) | no |

## Address

penv addresses a value as `org/project/environment/KEY`; each provider maps it to its own terms:

| Provider | `org` | `project` | `environment` |
|---|---|---|---|
| `penv` | organization | project | environment |

## Read contract

A read returns every key of one environment (`CloudKey` in `api.rs`):

| Field | Meaning |
|---|---|
| `name` | the key, as the schema names it |
| `value` | the value; absent when none is stored |
| `version` | the provider's version of this value, when it has one |
| `updatedAt` | when the value was last written; `@rotate` counts from it |
| `redacted` | present and withheld from this identity; never the same as no value |

Every client follows the `penv` client's rules:

1. **HTTPS only**, except `http://` to `127.0.0.1`, `localhost` or `[::1]`.
2. **No redirects.** A 3xx other than 304 fails the request; penv never follows it.
3. **A 30 s timeout.** A failed read fails the command, never yields an empty environment.
4. **No values in errors, logs or panics.**
5. **Logins in the OS keychain**, keyed by API root, never in a plain file.
6. **Trust**: the compiled-in roots, or `SSL_CERT_FILE` under its agent rule.
7. **One read per address per command.**

For `penv`, credentials are tried in this order: `PENV_TOKEN`, a person's login, an enrolled keypair, CI OIDC, then AWS (keys, web identity, container).

## Exit codes a read must produce

Your tests show these outcomes ([exit codes](https://penv.cloud/docs/reference/errors)):

| Case | Outcome |
|---|---|
| Key with no stored value | reported by name; the other keys still load |
| Unknown address | the error names the address; exit 1 |
| Credential refused (401) | exit 2, and the message names the fix |
| No credential | exit 5 |
| Environment refused (403) | exit 6 |
| Redirect | refused; no second request |
| Plain HTTP to a remote host | refused before connecting |
| URL override | `[providers.<slug>] url` sends the read there |

## Registered providers

| Slug | Name | Capabilities |
|---|---|---|
| `penv` | [penv.cloud](https://penv.cloud) | read, write, manage, approve, audit |
